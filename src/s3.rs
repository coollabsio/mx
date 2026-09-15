use crate::config::model::AliasConfig;
use crate::target::TargetRef;
use anyhow::{Context, Result, bail};
use aws_config::BehaviorVersion;
use aws_credential_types::{Credentials, provider::SharedCredentialsProvider};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{ConfigBag, Intercept, RuntimeComponents};
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{
    BucketLifecycleConfiguration, BucketVersioningStatus, CompletedMultipartUpload, CompletedPart,
    CorsConfiguration, CorsRule, Delete, LifecycleExpiration, LifecycleRule, LifecycleRuleFilter,
    ObjectIdentifier, ServerSideEncryption, ServerSideEncryptionByDefault,
    ServerSideEncryptionConfiguration, ServerSideEncryptionRule, Tag, Tagging,
    VersioningConfiguration,
};
use aws_smithy_http_client::{
    Builder as HttpClientBuilder,
    tls::{self, rustls_provider::CryptoMode},
};
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::context::BeforeTransmitInterceptorContextMut;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum S3ListItem {
    Bucket {
        name: String,
        last_modified: Option<String>,
    },
    Prefix {
        name: String,
    },
    Object {
        name: String,
        size: Option<i64>,
        last_modified: Option<String>,
        etag: Option<String>,
        storage_class: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub enum S3Stat {
    Bucket {
        bucket: String,
    },
    Object {
        bucket: String,
        key: String,
        size: Option<i64>,
        last_modified: Option<String>,
        etag: Option<String>,
        content_type: Option<String>,
        storage_class: Option<String>,
    },
}

pub async fn list_target(
    alias: &AliasConfig,
    target: &TargetRef,
    recursive: bool,
) -> Result<Vec<S3ListItem>> {
    let client = build_client(alias).await?;

    match &target.bucket {
        None => list_buckets(&client).await,
        Some(bucket) => {
            list_objects(
                &client,
                bucket,
                target.key_with_trailing_slash().as_deref(),
                recursive,
            )
            .await
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub key: String,
    pub size: i64,
}

pub async fn list_object_infos(
    alias: &AliasConfig,
    bucket: &str,
    prefix: Option<&str>,
) -> Result<Vec<ObjectInfo>> {
    let client = build_client(alias).await?;
    let items = list_objects(&client, bucket, prefix, true).await?;
    Ok(items
        .into_iter()
        .filter_map(|item| match item {
            S3ListItem::Object { name, size, .. } => Some(ObjectInfo {
                key: name,
                size: size.unwrap_or(0),
            }),
            _ => None,
        })
        .collect())
}

pub async fn make_bucket(alias: &AliasConfig, bucket: &str, ignore_existing: bool) -> Result<()> {
    let client = build_client(alias).await?;
    if let Err(error) = client.create_bucket().bucket(bucket).send().await {
        let already_exists = error
            .as_service_error()
            .and_then(ProvideErrorMetadata::code)
            .is_some_and(|code| matches!(code, "BucketAlreadyExists" | "BucketAlreadyOwnedByYou"));
        if !(ignore_existing && already_exists) {
            return Err(error.into());
        }
    }
    Ok(())
}

pub async fn remove_bucket(alias: &AliasConfig, bucket: &str, force: bool) -> Result<()> {
    let client = build_client(alias).await?;
    if force {
        delete_all_versions(&client, bucket).await?;
        let keys = list_object_infos(alias, bucket, None)
            .await?
            .into_iter()
            .map(|item| item.key)
            .collect::<Vec<_>>();
        delete_keys(&client, bucket, &keys).await?;
    }
    client.delete_bucket().bucket(bucket).send().await?;
    Ok(())
}

pub async fn delete_prefix(alias: &AliasConfig, bucket: &str, prefix: &str) -> Result<Vec<String>> {
    let client = build_client(alias).await?;
    let keys = list_objects(&client, bucket, Some(prefix), true)
        .await?
        .into_iter()
        .filter_map(|item| match item {
            S3ListItem::Object { name, .. } => Some(full_key(prefix, &name)),
            _ => None,
        })
        .collect::<Vec<_>>();
    delete_keys(&client, bucket, &keys).await?;
    Ok(keys)
}

fn full_key(prefix: &str, name: &str) -> String {
    let prefix = prefix.trim_matches('/');
    if prefix.is_empty() {
        name.to_string()
    } else if name.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

async fn delete_all_versions(client: &Client, bucket: &str) -> Result<()> {
    let mut key_marker = None;
    let mut version_marker = None;
    loop {
        let mut request = client.list_object_versions().bucket(bucket);
        if let Some(marker) = key_marker {
            request = request.key_marker(marker);
        }
        if let Some(marker) = version_marker {
            request = request.version_id_marker(marker);
        }
        let response = request.send().await?;
        let mut objects = Vec::new();
        for version in response.versions() {
            if let (Some(key), Some(version_id)) = (version.key(), version.version_id()) {
                objects.push(
                    ObjectIdentifier::builder()
                        .key(key)
                        .version_id(version_id)
                        .build()?,
                );
            }
        }
        for marker in response.delete_markers() {
            if let (Some(key), Some(version_id)) = (marker.key(), marker.version_id()) {
                objects.push(
                    ObjectIdentifier::builder()
                        .key(key)
                        .version_id(version_id)
                        .build()?,
                );
            }
        }
        if !objects.is_empty() {
            client
                .delete_objects()
                .bucket(bucket)
                .delete(
                    Delete::builder()
                        .set_objects(Some(objects))
                        .quiet(true)
                        .build()?,
                )
                .send()
                .await?;
        }
        if response.is_truncated() == Some(true) {
            key_marker = response.next_key_marker().map(str::to_string);
            version_marker = response.next_version_id_marker().map(str::to_string);
        } else {
            break;
        }
    }
    Ok(())
}

async fn delete_keys(client: &Client, bucket: &str, keys: &[String]) -> Result<()> {
    for chunk in keys.chunks(1_000) {
        if chunk.is_empty() {
            continue;
        }
        let objects = chunk
            .iter()
            .map(|key| ObjectIdentifier::builder().key(key).build())
            .collect::<Result<Vec<_>, _>>()?;
        client
            .delete_objects()
            .bucket(bucket)
            .delete(
                Delete::builder()
                    .set_objects(Some(objects))
                    .quiet(true)
                    .build()?,
            )
            .send()
            .await?;
    }
    Ok(())
}

pub async fn stat_target(alias: &AliasConfig, target: &TargetRef) -> Result<S3Stat> {
    let client = build_client(alias).await?;
    let bucket = target.require_bucket()?;

    if target.key.is_none() {
        client.head_bucket().bucket(bucket).send().await?;
        return Ok(S3Stat::Bucket {
            bucket: bucket.to_string(),
        });
    }

    let key = target.require_object_key()?;
    let response = client.head_object().bucket(bucket).key(&key).send().await?;
    Ok(S3Stat::Object {
        bucket: bucket.to_string(),
        key,
        size: response.content_length(),
        last_modified: response.last_modified().map(debug_timestamp),
        etag: response.e_tag().map(str::to_string),
        content_type: response.content_type().map(str::to_string),
        storage_class: response
            .storage_class()
            .map(|value| value.as_str().to_string()),
    })
}

pub async fn get_object_bytes(alias: &AliasConfig, bucket: &str, key: &str) -> Result<Vec<u8>> {
    let client = build_client(alias).await?;
    let response = client.get_object().bucket(bucket).key(key).send().await?;
    let bytes = response.body.collect().await?.into_bytes();
    Ok(bytes.to_vec())
}

pub async fn put_object_bytes(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    bytes: Vec<u8>,
    content_type: Option<&str>,
) -> Result<i64> {
    let client = build_client(alias).await?;
    let request = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from(bytes.clone()));
    let request = if let Some(content_type) = content_type {
        request.content_type(content_type)
    } else {
        request
    };
    request.send().await?;
    Ok(bytes.len() as i64)
}

pub async fn put_object_reader<R: Read>(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    mut reader: R,
) -> Result<i64> {
    const MAX_OBJECT_SIZE: i64 = 5 * 1024 * 1024 * 1024 * 1024;
    let client = build_client(alias).await?;
    let mut buffer = vec![0; multipart_part_size(1)];
    let first_len = read_part(&mut reader, &mut buffer)?;
    if first_len == 0 {
        client
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(ByteStream::from(Vec::new()))
            .send()
            .await?;
        return Ok(0);
    }

    let created = client
        .create_multipart_upload()
        .bucket(bucket)
        .key(key)
        .send()
        .await?;
    let upload_id = created
        .upload_id()
        .context("S3 did not return a multipart upload ID")?
        .to_string();
    let upload = async {
        let mut parts = Vec::new();
        let mut part_number = 1;
        let mut total = 0_i64;
        let mut length = first_len;
        loop {
            if part_number > 10_000 {
                bail!("input exceeds the S3 multipart part limit");
            }
            if total + length as i64 > MAX_OBJECT_SIZE {
                bail!("input exceeds the S3 object size limit");
            }
            buffer.truncate(length);
            let response = client
                .upload_part()
                .bucket(bucket)
                .key(key)
                .upload_id(&upload_id)
                .part_number(part_number)
                .body(ByteStream::from(buffer))
                .send()
                .await?;
            parts.push(
                CompletedPart::builder()
                    .part_number(part_number)
                    .set_e_tag(response.e_tag().map(str::to_string))
                    .build(),
            );
            total += length as i64;
            part_number += 1;
            buffer = vec![0; multipart_part_size(part_number)];
            length = read_part(&mut reader, &mut buffer)?;
            if length == 0 {
                break;
            }
        }
        client
            .complete_multipart_upload()
            .bucket(bucket)
            .key(key)
            .upload_id(&upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .send()
            .await?;
        Ok::<i64, anyhow::Error>(total)
    }
    .await;

    if upload.is_err() {
        let _ = client
            .abort_multipart_upload()
            .bucket(bucket)
            .key(key)
            .upload_id(&upload_id)
            .send()
            .await;
    }
    upload
}

fn multipart_part_size(part_number: i32) -> usize {
    let growth = ((part_number.saturating_sub(1) as usize) / 1_000).min(9);
    (8 * 1024 * 1024_usize) << growth
}

fn read_part(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize> {
    let mut length = 0;
    while length < buffer.len() {
        match reader.read(&mut buffer[length..]) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
            Ok(0) => break,
            Ok(count) => length += count,
        }
    }
    Ok(length)
}

pub async fn put_local_file(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    path: &Path,
) -> Result<i64> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Unable to read local file `{}`.", path.display()))?;
    put_object_reader(alias, bucket, key, file).await
}

pub async fn download_object_to_path(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    path: &Path,
) -> Result<i64> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let client = build_client(alias).await?;
    let response = client.get_object().bucket(bucket).key(key).send().await?;
    let mut source = response.body.into_async_read();
    let mut destination = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("Unable to write local file `{}`.", path.display()))?;
    let bytes = tokio::io::copy(&mut source, &mut destination).await?;
    Ok(bytes as i64)
}

pub async fn delete_object(alias: &AliasConfig, bucket: &str, key: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client
        .delete_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await?;
    Ok(())
}

pub async fn copy_object(
    src_alias: &AliasConfig,
    src_bucket: &str,
    src_key: &str,
    dst_alias: &AliasConfig,
    dst_bucket: &str,
    dst_key: &str,
) -> Result<()> {
    let source = format!("{src_bucket}/{src_key}");

    if same_endpoint_and_credentials(src_alias, dst_alias) {
        let client = build_client(dst_alias).await?;
        client
            .copy_object()
            .bucket(dst_bucket)
            .key(dst_key)
            .copy_source(source)
            .send()
            .await?;
        return Ok(());
    }

    let bytes = get_object_bytes(src_alias, src_bucket, src_key).await?;
    put_object_bytes(dst_alias, dst_bucket, dst_key, bytes, None).await?;
    Ok(())
}

async fn build_client(alias: &AliasConfig) -> Result<Client> {
    if alias.api.eq_ignore_ascii_case("S3v2") {
        bail!("API `{}` is not supported yet.", alias.api);
    }

    let credentials = Credentials::new(
        alias.access_key.clone(),
        alias.secret_key.clone(),
        alias.session_token.clone(),
        None,
        "mx",
    );

    let mut loader = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .credentials_provider(SharedCredentialsProvider::new(credentials));
    let endpoint = url::Url::parse(&alias.url)?;
    let endpoint_host = endpoint.host_str().unwrap_or_default();
    let endpoint_port = endpoint.port_or_known_default();
    let mappings: Vec<_> = crate::resolve::configured()
        .into_iter()
        .filter(|mapping| {
            mapping.host.eq_ignore_ascii_case(endpoint_host) && Some(mapping.port) == endpoint_port
        })
        .collect();
    if !mappings.is_empty() {
        let resolver = crate::resolve::PinnedDnsResolver::new(&mappings)?;
        let http_client = HttpClientBuilder::new()
            .tls_provider(tls::Provider::Rustls(CryptoMode::AwsLc))
            .build_with_resolver(resolver);
        loader = loader.http_client(http_client);
    }
    let shared_config = loader.load().await;

    let force_path_style = force_path_style(alias)?;
    let config = aws_sdk_s3::config::Builder::from(&shared_config)
        .endpoint_url(alias.url.clone())
        .force_path_style(force_path_style)
        .request_checksum_calculation(aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired)
        .response_checksum_validation(aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired)
        .interceptor(StripFlexibleChecksums)
        .build();

    Ok(Client::from_conf(config))
}

async fn list_buckets(client: &Client) -> Result<Vec<S3ListItem>> {
    let response = client.list_buckets().send().await?;
    let mut items = Vec::new();

    for bucket in response.buckets() {
        let name = bucket.name().unwrap_or_default().to_string();
        items.push(S3ListItem::Bucket {
            name,
            last_modified: bucket.creation_date().map(debug_timestamp),
        });
    }

    Ok(items)
}

async fn list_objects(
    client: &Client,
    bucket: &str,
    prefix: Option<&str>,
    recursive: bool,
) -> Result<Vec<S3ListItem>> {
    let normalized_prefix = normalize_prefix(prefix);
    let mut continuation = None;
    let mut items = Vec::new();

    loop {
        let mut request = client.list_objects_v2().bucket(bucket);
        if !recursive {
            request = request.delimiter("/");
        }
        if let Some(prefix) = &normalized_prefix {
            request = request.prefix(prefix);
        }
        if let Some(token) = continuation {
            request = request.continuation_token(token);
        }

        let response = request.send().await?;

        for prefix in response.common_prefixes() {
            if let Some(raw) = prefix.prefix() {
                items.push(S3ListItem::Prefix {
                    name: display_name(raw, normalized_prefix.as_deref()),
                });
            }
        }

        for object in response.contents() {
            if let Some(key) = object.key() {
                items.push(S3ListItem::Object {
                    name: display_name(key, normalized_prefix.as_deref()),
                    size: object.size(),
                    last_modified: object.last_modified().map(debug_timestamp),
                    etag: object.e_tag().map(str::to_string),
                    storage_class: object
                        .storage_class()
                        .map(|value| value.as_str().to_string()),
                });
            }
        }

        if response.is_truncated() == Some(true) {
            continuation = response.next_continuation_token().map(str::to_string);
        } else {
            break;
        }
    }

    Ok(items)
}

pub async fn ping(alias: &AliasConfig) -> Result<Duration> {
    let client = build_client(alias).await?;
    let started = std::time::Instant::now();
    client.list_buckets().send().await?;
    Ok(started.elapsed())
}

pub async fn presign_get(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    expire: Duration,
) -> Result<String> {
    let client = build_client(alias).await?;
    let request = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .presigned(PresigningConfig::expires_in(expire)?)
        .await?;
    Ok(request.uri().to_string())
}

pub async fn presign_put(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    expire: Duration,
) -> Result<String> {
    let client = build_client(alias).await?;
    let request = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .presigned(PresigningConfig::expires_in(expire)?)
        .await?;
    Ok(request.uri().to_string())
}

pub async fn put_object_tags(
    alias: &AliasConfig,
    bucket: &str,
    key: Option<&str>,
    tags: Vec<(String, String)>,
) -> Result<()> {
    let client = build_client(alias).await?;
    let tag_set = tags
        .into_iter()
        .map(|(key, value)| Tag::builder().key(key).value(value).build())
        .collect::<Result<Vec<_>, _>>()?;
    let tagging = Tagging::builder().set_tag_set(Some(tag_set)).build()?;
    if let Some(key) = key {
        client
            .put_object_tagging()
            .bucket(bucket)
            .key(key)
            .tagging(tagging)
            .send()
            .await?;
    } else {
        client
            .put_bucket_tagging()
            .bucket(bucket)
            .tagging(tagging)
            .send()
            .await?;
    }
    Ok(())
}

pub async fn get_object_tags(
    alias: &AliasConfig,
    bucket: &str,
    key: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let client = build_client(alias).await?;
    let tags = if let Some(key) = key {
        client
            .get_object_tagging()
            .bucket(bucket)
            .key(key)
            .send()
            .await?
            .tag_set
    } else {
        client
            .get_bucket_tagging()
            .bucket(bucket)
            .send()
            .await?
            .tag_set
    };
    Ok(tags.into_iter().map(|tag| (tag.key, tag.value)).collect())
}

pub async fn delete_object_tags(
    alias: &AliasConfig,
    bucket: &str,
    key: Option<&str>,
) -> Result<()> {
    let client = build_client(alias).await?;
    if let Some(key) = key {
        client
            .delete_object_tagging()
            .bucket(bucket)
            .key(key)
            .send()
            .await?;
    } else {
        client.delete_bucket_tagging().bucket(bucket).send().await?;
    }
    Ok(())
}

pub async fn set_versioning(alias: &AliasConfig, bucket: &str, enabled: bool) -> Result<()> {
    let client = build_client(alias).await?;
    let status = if enabled {
        BucketVersioningStatus::Enabled
    } else {
        BucketVersioningStatus::Suspended
    };
    client
        .put_bucket_versioning()
        .bucket(bucket)
        .versioning_configuration(VersioningConfiguration::builder().status(status).build())
        .send()
        .await?;
    Ok(())
}

pub async fn get_versioning(alias: &AliasConfig, bucket: &str) -> Result<String> {
    let client = build_client(alias).await?;
    let response = client.get_bucket_versioning().bucket(bucket).send().await?;
    Ok(response
        .status()
        .map(|status| status.as_str().to_string())
        .unwrap_or_else(|| "Off".to_string()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsDocument {
    #[serde(rename = "CORSRules")]
    pub rules: Vec<CorsDocumentRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsDocumentRule {
    #[serde(rename = "AllowedOrigins", default)]
    pub allowed_origins: Vec<String>,
    #[serde(rename = "AllowedMethods", default)]
    pub allowed_methods: Vec<String>,
    #[serde(rename = "AllowedHeaders", default)]
    pub allowed_headers: Vec<String>,
    #[serde(rename = "ExposeHeaders", default)]
    pub expose_headers: Vec<String>,
    #[serde(rename = "MaxAgeSeconds")]
    pub max_age_seconds: Option<i32>,
}

pub async fn put_cors(alias: &AliasConfig, bucket: &str, document: CorsDocument) -> Result<()> {
    let client = build_client(alias).await?;
    let mut rules = Vec::new();
    for rule in document.rules {
        let mut builder = CorsRule::builder()
            .set_allowed_origins(Some(rule.allowed_origins))
            .set_allowed_methods(Some(rule.allowed_methods))
            .set_allowed_headers(Some(rule.allowed_headers))
            .set_expose_headers(Some(rule.expose_headers));
        if let Some(max_age) = rule.max_age_seconds {
            builder = builder.max_age_seconds(max_age);
        }
        rules.push(builder.build()?);
    }
    client
        .put_bucket_cors()
        .bucket(bucket)
        .cors_configuration(
            CorsConfiguration::builder()
                .set_cors_rules(Some(rules))
                .build()?,
        )
        .send()
        .await?;
    Ok(())
}

pub async fn get_cors(alias: &AliasConfig, bucket: &str) -> Result<CorsDocument> {
    let client = build_client(alias).await?;
    let response = client.get_bucket_cors().bucket(bucket).send().await?;
    Ok(CorsDocument {
        rules: response
            .cors_rules
            .unwrap_or_default()
            .into_iter()
            .map(|rule| CorsDocumentRule {
                allowed_origins: rule.allowed_origins,
                allowed_methods: rule.allowed_methods,
                allowed_headers: rule.allowed_headers.unwrap_or_default(),
                expose_headers: rule.expose_headers.unwrap_or_default(),
                max_age_seconds: rule.max_age_seconds,
            })
            .collect(),
    })
}

pub async fn delete_cors(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client.delete_bucket_cors().bucket(bucket).send().await?;
    Ok(())
}

pub async fn put_encryption_s3(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    let default = ServerSideEncryptionByDefault::builder()
        .sse_algorithm(ServerSideEncryption::Aes256)
        .build()?;
    client
        .put_bucket_encryption()
        .bucket(bucket)
        .server_side_encryption_configuration(
            ServerSideEncryptionConfiguration::builder()
                .rules(
                    ServerSideEncryptionRule::builder()
                        .apply_server_side_encryption_by_default(default)
                        .build(),
                )
                .build()?,
        )
        .send()
        .await?;
    Ok(())
}

pub async fn get_encryption(alias: &AliasConfig, bucket: &str) -> Result<String> {
    let client = build_client(alias).await?;
    let response = client.get_bucket_encryption().bucket(bucket).send().await?;
    let algorithm = response
        .server_side_encryption_configuration
        .and_then(|config| config.rules.into_iter().next())
        .and_then(|rule| rule.apply_server_side_encryption_by_default)
        .map(|default| default.sse_algorithm.as_str().to_string())
        .unwrap_or_else(|| "none".to_string());
    Ok(algorithm)
}

pub async fn delete_encryption(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client
        .delete_bucket_encryption()
        .bucket(bucket)
        .send()
        .await?;
    Ok(())
}

pub async fn put_bucket_policy(alias: &AliasConfig, bucket: &str, policy: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client
        .put_bucket_policy()
        .bucket(bucket)
        .policy(policy)
        .send()
        .await?;
    Ok(())
}

pub async fn get_bucket_policy(alias: &AliasConfig, bucket: &str) -> Result<Option<String>> {
    let client = build_client(alias).await?;
    match client.get_bucket_policy().bucket(bucket).send().await {
        Ok(response) => Ok(response.policy),
        Err(error)
            if error
                .as_service_error()
                .and_then(ProvideErrorMetadata::code)
                .is_some_and(|code| code == "NoSuchBucketPolicy") =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn delete_bucket_policy(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client.delete_bucket_policy().bucket(bucket).send().await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct LifecycleRuleView {
    pub id: String,
    pub prefix: Option<String>,
    pub expire_days: Option<i32>,
    pub status: String,
}

pub async fn add_lifecycle_rule(
    alias: &AliasConfig,
    bucket: &str,
    id: &str,
    prefix: Option<&str>,
    expire_days: i32,
) -> Result<()> {
    let client = build_client(alias).await?;
    let mut rules = get_lifecycle_rules(alias, bucket).await.unwrap_or_default();
    let mut builder = LifecycleRule::builder()
        .id(id)
        .status(aws_sdk_s3::types::ExpirationStatus::Enabled)
        .expiration(LifecycleExpiration::builder().days(expire_days).build());
    if let Some(prefix) = prefix {
        builder = builder.filter(LifecycleRuleFilter::builder().prefix(prefix).build());
    }
    rules.push(builder.build()?);
    client
        .put_bucket_lifecycle_configuration()
        .bucket(bucket)
        .lifecycle_configuration(
            BucketLifecycleConfiguration::builder()
                .set_rules(Some(rules))
                .build()?,
        )
        .send()
        .await?;
    Ok(())
}

pub async fn list_lifecycle_rules(
    alias: &AliasConfig,
    bucket: &str,
) -> Result<Vec<LifecycleRuleView>> {
    let client = build_client(alias).await?;
    let rules = match client
        .get_bucket_lifecycle_configuration()
        .bucket(bucket)
        .send()
        .await
    {
        Ok(response) => response.rules.unwrap_or_default(),
        Err(error)
            if error
                .as_service_error()
                .and_then(ProvideErrorMetadata::code)
                .is_some_and(|code| {
                    matches!(code, "NoSuchLifecycleConfiguration" | "NoSuchBucket")
                }) =>
        {
            Vec::new()
        }
        Err(error) => return Err(error.into()),
    };
    Ok(rules
        .into_iter()
        .map(|rule| LifecycleRuleView {
            id: rule.id.clone().unwrap_or_default(),
            prefix: rule.filter.and_then(|filter| filter.prefix),
            expire_days: rule.expiration.and_then(|expiration| expiration.days),
            status: rule.status.as_str().to_string(),
        })
        .collect())
}

pub async fn remove_lifecycle_rule(alias: &AliasConfig, bucket: &str, id: &str) -> Result<()> {
    let client = build_client(alias).await?;
    let rules = get_lifecycle_rules(alias, bucket)
        .await?
        .into_iter()
        .filter(|rule| rule.id.as_deref() != Some(id))
        .collect::<Vec<_>>();
    if rules.is_empty() {
        client
            .delete_bucket_lifecycle()
            .bucket(bucket)
            .send()
            .await?;
        return Ok(());
    }
    client
        .put_bucket_lifecycle_configuration()
        .bucket(bucket)
        .lifecycle_configuration(
            BucketLifecycleConfiguration::builder()
                .set_rules(Some(rules))
                .build()?,
        )
        .send()
        .await?;
    Ok(())
}

async fn get_lifecycle_rules(alias: &AliasConfig, bucket: &str) -> Result<Vec<LifecycleRule>> {
    let client = build_client(alias).await?;
    match client
        .get_bucket_lifecycle_configuration()
        .bucket(bucket)
        .send()
        .await
    {
        Ok(response) => Ok(response.rules.unwrap_or_default()),
        Err(error)
            if error
                .as_service_error()
                .and_then(ProvideErrorMetadata::code)
                .is_some_and(|code| code == "NoSuchLifecycleConfiguration") =>
        {
            Ok(Vec::new())
        }
        Err(error) => Err(error.into()),
    }
}

pub fn force_path_style(alias: &AliasConfig) -> Result<bool> {
    match alias.path.to_ascii_lowercase().as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        "dns" => Ok(false),
        _ => {
            let parsed = url::Url::parse(&alias.url)?;
            let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
            Ok(!(host.ends_with("amazonaws.com") || host == "storage.googleapis.com"))
        }
    }
}

fn normalize_prefix(prefix: Option<&str>) -> Option<String> {
    prefix
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let mut normalized = value.trim_matches('/').to_string();
            if !normalized.is_empty() && !normalized.ends_with('/') {
                normalized.push('/');
            }
            normalized
        })
        .filter(|value| !value.is_empty())
}

fn display_name(raw: &str, prefix: Option<&str>) -> String {
    let stripped = prefix
        .and_then(|prefix| raw.strip_prefix(prefix))
        .unwrap_or(raw);
    stripped.to_string()
}

fn debug_timestamp<T: std::fmt::Debug>(value: &T) -> String {
    format!("{value:?}")
}

#[derive(Debug)]
struct StripFlexibleChecksums;

impl Intercept for StripFlexibleChecksums {
    fn name(&self) -> &'static str {
        "strip-flexible-checksums"
    }

    fn modify_before_signing(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        strip_unsigned_sdk_headers(context.request_mut().headers_mut());
        Ok(())
    }
}

fn strip_unsigned_sdk_headers(headers: &mut aws_smithy_runtime_api::http::Headers) {
    for name in ["amz-sdk-invocation-id", "amz-sdk-request"] {
        let _ = headers.remove(name);
    }
}

fn same_endpoint_and_credentials(left: &AliasConfig, right: &AliasConfig) -> bool {
    left.url == right.url
        && left.access_key == right.access_key
        && left.secret_key == right.secret_key
        && left.session_token == right.session_token
}

#[cfg(test)]
mod tests {
    use super::{force_path_style, multipart_part_size, read_part};
    use crate::config::model::AliasConfig;
    use std::io::{self, Read};

    #[test]
    fn forces_path_style_for_minio_auto() {
        let alias = AliasConfig {
            url: "http://localhost:9000".into(),
            path: "auto".into(),
            ..Default::default()
        };

        assert!(force_path_style(&alias).unwrap());
    }

    #[test]
    fn disables_path_style_for_aws_dns() {
        let alias = AliasConfig {
            url: "https://s3.amazonaws.com".into(),
            path: "dns".into(),
            ..Default::default()
        };

        assert!(!force_path_style(&alias).unwrap());
    }

    #[test]
    fn retries_interrupted_reader() {
        struct InterruptedOnce(bool);
        impl Read for InterruptedOnce {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    return Err(io::ErrorKind::Interrupted.into());
                }
                buffer[..4].copy_from_slice(b"data");
                Ok(4)
            }
        }

        let mut output = [0; 4];
        let length = read_part(&mut InterruptedOnce(false), &mut output).unwrap();
        assert_eq!(length, 4);
        assert_eq!(&output, b"data");
    }

    #[test]
    fn multipart_parts_grow_for_large_unknown_streams() {
        assert_eq!(multipart_part_size(1), 8 * 1024 * 1024);
        assert_eq!(multipart_part_size(1_000), 8 * 1024 * 1024);
        assert_eq!(multipart_part_size(1_001), 16 * 1024 * 1024);
        assert_eq!(multipart_part_size(9_001), 4 * 1024 * 1024 * 1024);
    }
}
