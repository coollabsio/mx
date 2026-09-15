use crate::config::model::AliasConfig;
use crate::target::TargetRef;
use anyhow::{Context, Result, bail};
use aws_config::BehaviorVersion;
use aws_credential_types::{Credentials, provider::SharedCredentialsProvider};
use aws_sdk_s3::Client;
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use aws_smithy_http_client::{
    Builder as HttpClientBuilder,
    tls::{self, rustls_provider::CryptoMode},
};
use std::io::Read;
use std::path::Path;

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

pub async fn list_target(alias: &AliasConfig, target: &TargetRef) -> Result<Vec<S3ListItem>> {
    let client = build_client(alias).await?;

    match &target.bucket {
        None => list_buckets(&client).await,
        Some(bucket) => {
            list_objects(&client, bucket, target.key_with_trailing_slash().as_deref()).await
        }
    }
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

pub async fn remove_bucket(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client.delete_bucket().bucket(bucket).send().await?;
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
) -> Result<Vec<S3ListItem>> {
    let normalized_prefix = normalize_prefix(prefix);
    let mut request = client.list_objects_v2().bucket(bucket).delimiter("/");

    if let Some(prefix) = &normalized_prefix {
        request = request.prefix(prefix);
    }

    let response = request.send().await?;
    let mut items = Vec::new();

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

    Ok(items)
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
