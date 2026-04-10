use crate::config::model::AliasConfig;
use crate::target::TargetRef;
use anyhow::{Context, Result, bail};
use aws_config::BehaviorVersion;
use aws_credential_types::{Credentials, provider::SharedCredentialsProvider};
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
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

pub async fn make_bucket(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client.create_bucket().bucket(bucket).send().await?;
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

pub async fn put_local_file(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    path: &Path,
) -> Result<i64> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("Unable to read local file `{}`.", path.display()))?;
    put_object_bytes(alias, bucket, key, bytes, None).await
}

pub async fn download_object_to_path(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    path: &Path,
) -> Result<i64> {
    let bytes = get_object_bytes(alias, bucket, key).await?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("Unable to write local file `{}`.", path.display()))?;
    Ok(bytes.len() as i64)
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

    let shared_config = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .credentials_provider(SharedCredentialsProvider::new(credentials))
        .load()
        .await;

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
    use super::force_path_style;
    use crate::config::model::AliasConfig;

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
}
