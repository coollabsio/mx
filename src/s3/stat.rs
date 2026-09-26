//! HeadBucket / HeadObject and bucket/object property collection for `stat` (area C).

use super::{GetOptions, to_system_time};
use anyhow::Result;
use aws_sdk_s3::Client;
use aws_sdk_s3::operation::head_object::HeadObjectOutput;
use aws_sdk_s3::types::ChecksumMode;
use std::collections::BTreeMap;
use std::time::SystemTime;

/// Object properties as reported by HeadObject.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectStat {
    pub key: String,
    pub size: i64,
    pub last_modified: Option<SystemTime>,
    /// ETag without surrounding quotes.
    pub etag: Option<String>,
    pub content_type: Option<String>,
    pub storage_class: Option<String>,
    pub version_id: Option<String>,
    pub delete_marker: bool,
    /// Raw `Expires` header.
    pub expires: Option<String>,
    /// Raw `x-amz-expiration` header (`expiry-date="...", rule-id="..."`).
    pub expiration: Option<String>,
    /// Raw `x-amz-restore` header (`ongoing-request="...", expiry-date="..."`).
    pub restore: Option<String>,
    pub replication_status: Option<String>,
    /// `(ALGORITHM, value)` pairs, e.g. `("CRC32C", "abc=")`.
    pub checksums: Vec<(String, String)>,
    /// Canonical header name -> value (standard headers, `X-Amz-Meta-*`, encryption headers).
    pub metadata: BTreeMap<String, String>,
}

/// HeadObject (checksum mode enabled) for `key`, optionally at `version_id`.
pub async fn stat_object(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
) -> Result<ObjectStat> {
    stat_object_sse_c(client, bucket, key, version_id, None).await
}

/// [`stat_object`] with an optional SSE-C key (needed to HEAD SSE-C objects).
pub async fn stat_object_sse_c(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
    sse_c: Option<[u8; 32]>,
) -> Result<ObjectStat> {
    let mut request = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .checksum_mode(ChecksumMode::Enabled);
    if let Some(sse_c) = &sse_c {
        let (algorithm, encoded, md5) = super::objects::sse_c_headers(sse_c);
        request = request
            .sse_customer_algorithm(algorithm)
            .sse_customer_key(encoded)
            .sse_customer_key_md5(md5);
    }
    let response = request.send().await?;
    Ok(object_stat_from_head(key, &response))
}

fn object_stat_from_head(key: &str, head: &HeadObjectOutput) -> ObjectStat {
    let mut metadata = BTreeMap::new();
    let mut put = |name: &str, value: Option<&str>| {
        if let Some(value) = value {
            metadata.insert(name.to_string(), value.to_string());
        }
    };
    put("Content-Type", head.content_type());
    put("Cache-Control", head.cache_control());
    put("Content-Encoding", head.content_encoding());
    put("Content-Disposition", head.content_disposition());
    put("Content-Language", head.content_language());
    put(
        "X-Amz-Server-Side-Encryption",
        head.server_side_encryption().map(|value| value.as_str()),
    );
    put(
        "X-Amz-Server-Side-Encryption-Aws-Kms-Key-Id",
        head.ssekms_key_id(),
    );
    put(
        "X-Amz-Server-Side-Encryption-Customer-Algorithm",
        head.sse_customer_algorithm(),
    );
    put(
        "X-Amz-Server-Side-Encryption-Customer-Key-Md5",
        head.sse_customer_key_md5(),
    );
    if let Some(enabled) = head.bucket_key_enabled() {
        metadata.insert(
            "X-Amz-Server-Side-Encryption-Bucket-Key-Enabled".into(),
            enabled.to_string(),
        );
    }
    for (name, value) in head.metadata().into_iter().flatten() {
        metadata.insert(
            canonical_header(&format!("x-amz-meta-{name}")),
            value.clone(),
        );
    }

    let checksums = [
        ("CRC32", head.checksum_crc32()),
        ("CRC32C", head.checksum_crc32_c()),
        ("CRC64NVME", head.checksum_crc64_nvme()),
        ("SHA1", head.checksum_sha1()),
        ("SHA256", head.checksum_sha256()),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|value| (name.to_string(), value.to_string())))
    .collect();

    ObjectStat {
        key: key.to_string(),
        size: head.content_length().unwrap_or(0),
        last_modified: head.last_modified().and_then(to_system_time),
        etag: head.e_tag().map(|etag| etag.trim_matches('"').to_string()),
        content_type: head.content_type().map(str::to_string),
        storage_class: head.storage_class().map(|value| value.as_str().to_string()),
        version_id: head.version_id().map(str::to_string),
        delete_marker: head.delete_marker().unwrap_or(false),
        expires: head.expires_string().map(str::to_string),
        expiration: head.expiration().map(str::to_string),
        restore: head.restore().map(str::to_string),
        replication_status: head
            .replication_status()
            .map(|value| value.as_str().to_string()),
        checksums,
        metadata,
    }
}

/// `x-amz-meta-foo-bar` -> `X-Amz-Meta-Foo-Bar`.
pub fn canonical_header(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Parses `key="value", key2="value2"` header values (`x-amz-expiration`, `x-amz-restore`).
pub fn parse_header_pairs(value: &str) -> BTreeMap<String, String> {
    let mut pairs = BTreeMap::new();
    let mut rest = value.trim();
    while !rest.is_empty() {
        let Some((key, after)) = rest.split_once('=') else {
            break;
        };
        let key = key.trim().trim_start_matches(',').trim().to_string();
        let after = after.trim_start();
        let (value, remaining) = if let Some(quoted) = after.strip_prefix('"') {
            match quoted.split_once('"') {
                Some((value, remaining)) => (value.to_string(), remaining),
                None => (quoted.to_string(), ""),
            }
        } else {
            match after.split_once(',') {
                Some((value, remaining)) => (value.trim().to_string(), remaining),
                None => (after.trim().to_string(), ""),
            }
        };
        pairs.insert(key, value);
        rest = remaining.trim_start().trim_start_matches(',').trim_start();
    }
    pairs
}

/// Bucket properties gathered for `stat` (best effort: unreadable settings stay unset).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BucketStat {
    pub name: String,
    pub created: Option<SystemTime>,
    /// `Enabled` / `Suspended`; empty when never versioned.
    pub versioning: String,
    pub mfa_delete: String,
    /// SSE algorithm (`AES256`, `aws:kms`) and KMS key ID.
    pub encryption_algorithm: String,
    pub encryption_key_id: String,
    pub lock_enabled: String,
    pub lock_mode: String,
    /// e.g. `1DAYS`.
    pub lock_validity: String,
    pub replication: bool,
    /// A bucket policy is set.
    pub anonymous: bool,
    pub location: String,
    pub tags: Vec<(String, String)>,
    pub ilm: bool,
    pub notification: bool,
}

/// Collects bucket properties. Fails only when the bucket itself cannot be read.
pub async fn stat_bucket(client: &Client, bucket: &str) -> Result<BucketStat> {
    let location = client.get_bucket_location().bucket(bucket).send().await?;
    let mut stat = BucketStat {
        name: bucket.to_string(),
        location: location
            .location_constraint()
            .map(|value| value.as_str().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "us-east-1".to_string()),
        ..Default::default()
    };
    if let Ok(response) = client.list_buckets().send().await {
        stat.created = response
            .buckets()
            .iter()
            .find(|entry| entry.name() == Some(bucket))
            .and_then(|entry| entry.creation_date())
            .and_then(to_system_time);
    }
    if let Ok(response) = client.get_bucket_versioning().bucket(bucket).send().await {
        stat.versioning = response
            .status()
            .map(|value| value.as_str().to_string())
            .unwrap_or_default();
        stat.mfa_delete = response
            .mfa_delete()
            .map(|value| value.as_str().to_string())
            .unwrap_or_default();
    }
    if let Ok(response) = client
        .get_object_lock_configuration()
        .bucket(bucket)
        .send()
        .await
        && let Some(config) = response.object_lock_configuration()
    {
        stat.lock_enabled = config
            .object_lock_enabled()
            .map(|value| value.as_str().to_string())
            .unwrap_or_default();
        if let Some(retention) = config.rule().and_then(|rule| rule.default_retention()) {
            stat.lock_mode = retention
                .mode()
                .map(|value| value.as_str().to_string())
                .unwrap_or_default();
            stat.lock_validity = match (retention.days(), retention.years()) {
                (Some(days), _) => format!("{days}DAYS"),
                (None, Some(years)) => format!("{years}YEARS"),
                _ => String::new(),
            };
        }
    }
    if let Ok(response) = client.get_bucket_replication().bucket(bucket).send().await {
        stat.replication = response
            .replication_configuration()
            .is_some_and(|config| !config.rules().is_empty());
    }
    if let Ok(response) = client.get_bucket_encryption().bucket(bucket).send().await
        && let Some(default) = response
            .server_side_encryption_configuration()
            .and_then(|config| config.rules().first())
            .and_then(|rule| rule.apply_server_side_encryption_by_default())
    {
        stat.encryption_algorithm = default.sse_algorithm().as_str().to_string();
        stat.encryption_key_id = default.kms_master_key_id().unwrap_or_default().to_string();
    }
    if let Ok(response) = client.get_bucket_policy().bucket(bucket).send().await {
        stat.anonymous = response
            .policy()
            .is_some_and(|policy| !policy.trim().is_empty());
    }
    if let Ok(response) = client.get_bucket_tagging().bucket(bucket).send().await {
        stat.tags = response
            .tag_set()
            .iter()
            .map(|tag| (tag.key().to_string(), tag.value().to_string()))
            .collect();
        stat.tags.sort();
    }
    if let Ok(response) = client
        .get_bucket_lifecycle_configuration()
        .bucket(bucket)
        .send()
        .await
    {
        stat.ilm = !response.rules().is_empty();
    }
    if let Ok(response) = client
        .get_bucket_notification_configuration()
        .bucket(bucket)
        .send()
        .await
    {
        stat.notification = !response.topic_configurations().is_empty()
            || !response.queue_configurations().is_empty()
            || !response.lambda_function_configurations().is_empty();
    }
    Ok(stat)
}

/// HeadObject honoring `version_id`, `sse_c`, `part_number` and `zip_extract` from
/// [`GetOptions`] (`range` is ignored).
pub async fn head_object_with(
    client: &Client,
    bucket: &str,
    key: &str,
    options: &GetOptions,
) -> Result<HeadObjectOutput> {
    let mut request = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(options.version_id.clone())
        .set_part_number(options.part_number);
    if let Some(key) = &options.sse_c {
        let (algorithm, encoded, md5) = super::objects::sse_c_headers(key);
        request = request
            .sse_customer_algorithm(algorithm)
            .sse_customer_key(encoded)
            .sse_customer_key_md5(md5);
    }
    if options.zip_extract {
        Ok(request
            .customize()
            .mutate_request(super::objects::add_zip_extract_header)
            .send()
            .await?)
    } else {
        Ok(request.send().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_header, parse_header_pairs};

    #[test]
    fn canonicalizes_header_names() {
        assert_eq!(canonical_header("x-amz-meta-foo-bar"), "X-Amz-Meta-Foo-Bar");
        assert_eq!(canonical_header("X-AMZ-META-ABC"), "X-Amz-Meta-Abc");
    }

    #[test]
    fn parses_expiration_and_restore_headers() {
        let pairs = parse_header_pairs(
            r#"expiry-date="Fri, 23 Dec 2012 00:00:00 GMT", rule-id="picture-deletion-rule""#,
        );
        assert_eq!(pairs["expiry-date"], "Fri, 23 Dec 2012 00:00:00 GMT");
        assert_eq!(pairs["rule-id"], "picture-deletion-rule");
        let pairs = parse_header_pairs(r#"ongoing-request="true""#);
        assert_eq!(pairs["ongoing-request"], "true");
    }
}
