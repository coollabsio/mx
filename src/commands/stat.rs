use crate::commands::cat::EncCFlag;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::{RewindFlag, VersionIdFlag, resolve_sse};
use crate::s3::{BucketStat, ListOptions, ObjectInfo, ObjectStat, full_key, parse_header_pairs};
use crate::target::TargetRef;
use anyhow::{Context, Result, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::{DateTime, DateTimeFormat};
use clap::Args;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Args)]
pub struct StatArgs {
    /// stat all objects recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
    /// stat all versions
    #[arg(long)]
    pub versions: bool,
    #[command(flatten)]
    pub version_id: VersionIdFlag,
    #[command(flatten)]
    pub rewind: RewindFlag,
    /// show extended bucket(s) stat
    #[arg(short = 'v', long)]
    pub verbose: bool,
    /// disable all LIST operations for stat
    #[arg(long)]
    pub no_list: bool,
    #[command(flatten)]
    pub enc: EncCFlag,
    #[arg(required = true, value_name = "TARGET")]
    pub targets: Vec<String>,
}

/// Validates mc's flag combination rules for `stat`.
pub fn validate(args: &StatArgs) -> Result<()> {
    let has_rewind = args.rewind.rewind.is_some();
    if args.version_id.version_id.is_some() {
        if args.targets.len() > 1 {
            bail!("You cannot specify --version-id with multiple arguments.");
        }
        if args.recursive || args.versions || has_rewind {
            bail!(
                "You cannot specify --version-id with either --rewind, --versions or --recursive."
            );
        }
    }
    if (args.recursive || args.versions) && args.no_list {
        bail!("You cannot specify --no-list with either --versions or --recursive.");
    }
    if args.no_list && has_rewind {
        bail!("You cannot specify --no-list with --rewind.");
    }
    Ok(())
}

pub fn run(args: StatArgs, json: bool) -> Result<()> {
    validate(&args)?;
    let rewind = args.rewind.at(SystemTime::now())?;
    let store = ConfigStore::load_or_create()?;
    let rt = runtime()?;
    for input in &args.targets {
        let target = TargetRef::parse(input)?;
        let alias = alias_config(&store, &target.alias)?;
        let entries = rt
            .block_on(collect(&alias, &target, &args, rewind))
            .with_context(|| format!("Unable to stat `{input}`."))?;
        for entry in &entries {
            if json {
                println!("{}", serde_json::to_string(&entry.json(input))?);
            } else {
                println!("{}", entry.text());
            }
        }
    }
    Ok(())
}

enum StatEntry {
    Bucket(BucketStat),
    Folder {
        name: String,
    },
    Object {
        name: String,
        bucket: String,
        stat: ObjectStat,
    },
}

async fn collect(
    alias: &crate::config::model::AliasConfig,
    target: &TargetRef,
    args: &StatArgs,
    rewind: Option<SystemTime>,
) -> Result<Vec<StatEntry>> {
    let client = crate::s3::build_client(alias).await?;
    let Some(bucket) = target.bucket.clone() else {
        let response = client.list_buckets().send().await?;
        let mut entries = Vec::new();
        for entry in response.buckets() {
            let name = entry.name().unwrap_or_default();
            if args.verbose {
                entries.push(StatEntry::Bucket(
                    crate::s3::stat_bucket(&client, name).await?,
                ));
            } else {
                entries.push(StatEntry::Folder {
                    name: format!("{name}/"),
                });
            }
        }
        return Ok(entries);
    };
    let key = target.key_with_trailing_slash();
    // Names are printed relative to the target's directory, like mc.
    let dir = match &key {
        Some(key) if !key.ends_with('/') => key[..key.rfind('/').map_or(0, |i| i + 1)].to_string(),
        Some(key) => key.clone(),
        None => String::new(),
    };
    let relative = |key: &str| key.strip_prefix(dir.as_str()).unwrap_or(key).to_string();
    let enc = args.enc.entries()?;
    let sse_c = |key: &str| {
        resolve_sse(&enc, &format!("{}/{bucket}/{key}", target.alias))
            .and_then(|sse| sse.customer_key())
    };

    let Some(key) = key else {
        if args.recursive || target.trailing_slash {
            return list_entries(&client, &bucket, None, args, rewind, &relative, &sse_c).await;
        }
        return Ok(vec![StatEntry::Bucket(
            crate::s3::stat_bucket(&client, &bucket).await?,
        )]);
    };

    if args.no_list || args.version_id.version_id.is_some() {
        let stat = crate::s3::stat_object_sse_c(
            &client,
            &bucket,
            &key,
            args.version_id.version_id.as_deref(),
            sse_c(&key),
        )
        .await?;
        return Ok(vec![object_entry(&bucket, stat, &relative)]);
    }
    if args.recursive || key.ends_with('/') {
        return list_entries(
            &client,
            &bucket,
            Some(&key),
            args,
            rewind,
            &relative,
            &sse_c,
        )
        .await;
    }
    if args.versions || rewind.is_some() {
        let mut versions = crate::s3::list_key_versions(&client, &bucket, &key).await?;
        versions = match (args.versions, rewind) {
            (true, Some(at)) => crate::s3::versions_before(versions, at),
            (false, Some(at)) => crate::s3::resolve_rewind(versions, at),
            _ => versions,
        };
        if versions.is_empty() {
            bail!("Object does not exist.");
        }
        let mut entries = Vec::new();
        for version in versions {
            let stat = stat_version(&client, &bucket, &key, &version, sse_c(&key)).await?;
            entries.push(object_entry(&bucket, stat, &relative));
        }
        return Ok(entries);
    }
    match crate::s3::stat_object_sse_c(&client, &bucket, &key, None, sse_c(&key)).await {
        Ok(stat) => Ok(vec![object_entry(&bucket, stat, &relative)]),
        Err(error) => {
            // Not an object: report it as a folder if it is a non-empty prefix.
            let prefix = format!("{key}/");
            let children = client
                .list_objects_v2()
                .bucket(&bucket)
                .prefix(&prefix)
                .max_keys(1)
                .send()
                .await?;
            if children.contents().is_empty() && children.common_prefixes().is_empty() {
                return Err(error);
            }
            Ok(vec![StatEntry::Folder {
                name: relative(&prefix),
            }])
        }
    }
}

fn object_entry(bucket: &str, stat: ObjectStat, relative: &dyn Fn(&str) -> String) -> StatEntry {
    StatEntry::Object {
        name: relative(&stat.key),
        bucket: bucket.to_string(),
        stat,
    }
}

async fn list_entries(
    client: &Client,
    bucket: &str,
    prefix: Option<&str>,
    args: &StatArgs,
    rewind: Option<SystemTime>,
    relative: &dyn Fn(&str) -> String,
    sse_c: &dyn Fn(&str) -> Option<[u8; 32]>,
) -> Result<Vec<StatEntry>> {
    let options = ListOptions {
        recursive: args.recursive,
        versions: args.versions,
        rewind,
        ..Default::default()
    };
    let items = crate::s3::list_objects_with(client, bucket, prefix, &options).await?;
    if items.is_empty() {
        bail!("Object does not exist.");
    }
    let base = prefix.unwrap_or_default();
    let mut entries = Vec::new();
    for item in items {
        let key = full_key(base, &item.key);
        if item.is_prefix {
            entries.push(StatEntry::Folder {
                name: relative(&format!("{}/", key.trim_end_matches('/'))),
            });
            continue;
        }
        let stat = stat_version(client, bucket, &key, &item, sse_c(&key)).await?;
        entries.push(object_entry(bucket, stat, relative));
    }
    Ok(entries)
}

/// HEAD for a listed object/version; delete markers are described from the listing.
async fn stat_version(
    client: &Client,
    bucket: &str,
    key: &str,
    item: &ObjectInfo,
    sse_c: Option<[u8; 32]>,
) -> Result<ObjectStat> {
    if item.is_delete_marker {
        return Ok(ObjectStat {
            key: key.to_string(),
            last_modified: item.last_modified,
            version_id: item.version_id.clone(),
            delete_marker: true,
            ..Default::default()
        });
    }
    crate::s3::stat_object_sse_c(client, bucket, key, item.version_id.as_deref(), sse_c).await
}

// ---------------------------------------------------------------------------
// formatting helpers shared with ls/rm
// ---------------------------------------------------------------------------

/// mc `printDate` layout (`2006-01-02 15:04:05 MST`), always in UTC.
pub(crate) fn print_date(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let text = DateTime::from_secs(secs as i64)
        .fmt(DateTimeFormat::DateTime)
        .unwrap_or_default();
    format!("{} UTC", text.trim_end_matches('Z').replacen('T', " ", 1))
}

/// RFC3339 timestamp in the format `mx` uses for JSON (`2024-01-02T03:04:05.123Z`).
pub(crate) fn rfc3339(time: SystemTime) -> String {
    crate::s3::debug_timestamp(&crate::s3::from_system_time(time))
}

/// go-humanize `IBytes` (e.g. `0 B`, `17 B`, `1.0 KiB`, `12 MiB`).
pub(crate) fn human_bytes(size: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if size < 10 {
        return format!("{size} B");
    }
    let mut exponent = 0;
    while exponent + 1 < UNITS.len() && size >= 1024u64.pow(exponent as u32 + 1) {
        exponent += 1;
    }
    let value = (size as f64 / 1024f64.powi(exponent as i32) * 10.0 + 0.5).floor() / 10.0;
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[exponent])
    } else {
        format!("{value:.0} {}", UNITS[exponent])
    }
}

fn http_date(value: &str) -> Option<SystemTime> {
    DateTime::from_str(value.trim(), DateTimeFormat::HttpDate)
        .ok()
        .and_then(|date| SystemTime::try_from(date).ok())
}

const ENCRYPTION_PREFIX: &str = "x-amz-server-side-encryption";

impl StatEntry {
    fn text(&self) -> String {
        match self {
            StatEntry::Bucket(bucket) => bucket_text(bucket),
            StatEntry::Folder { name } => {
                format!("{:<10}: {name}\n{:<10}: folder \n", "Name", "Type")
            }
            StatEntry::Object { name, stat, .. } => object_text(name, stat),
        }
    }

    fn json(&self, target: &str) -> StatJson {
        match self {
            StatEntry::Bucket(bucket) => StatJson {
                status: "success",
                target: target.to_string(),
                kind: "bucket",
                bucket: Some(bucket.name.clone()),
                name: bucket.name.clone(),
                last_modified: bucket.created.map(rfc3339),
                size: Some(0),
                versioning: Some(VersioningJson {
                    status: bucket.versioning.clone(),
                    mfa_delete: bucket.mfa_delete.clone(),
                }),
                encryption: Some(EncryptionJson {
                    algorithm: bucket.encryption_algorithm.clone(),
                    key_id: bucket.encryption_key_id.clone(),
                }),
                object_lock: Some(LockJson {
                    enabled: bucket.lock_enabled.clone(),
                    mode: bucket.lock_mode.clone(),
                    validity: bucket.lock_validity.clone(),
                }),
                replication: Some(ReplicationJson {
                    enabled: bucket.replication,
                }),
                policy: Some(PolicyJson {
                    kind: if bucket.anonymous { "custom" } else { "none" },
                }),
                location: Some(bucket.location.clone()),
                tagging: (!bucket.tags.is_empty()).then(|| bucket.tags.iter().cloned().collect()),
                ..Default::default()
            },
            StatEntry::Folder { name } => StatJson {
                status: "success",
                target: target.to_string(),
                kind: "folder",
                name: name.clone(),
                ..Default::default()
            },
            StatEntry::Object { name, bucket, stat } => {
                let expiration = stat.expiration.as_deref().map(parse_header_pairs);
                let restore = stat.restore.as_deref().map(parse_header_pairs);
                StatJson {
                    status: "success",
                    target: target.to_string(),
                    kind: "file",
                    bucket: Some(bucket.clone()),
                    key: Some(stat.key.clone()),
                    name: name.clone(),
                    size: Some(stat.size),
                    last_modified: stat.last_modified.map(rfc3339),
                    etag_legacy: stat.etag.clone(),
                    etag: stat.etag.clone(),
                    content_type: stat.content_type.clone(),
                    storage_class: stat.storage_class.clone(),
                    version_id: stat.version_id.clone(),
                    delete_marker: stat.delete_marker,
                    expires: stat.expires.as_deref().and_then(http_date).map(rfc3339),
                    expiration: expiration
                        .as_ref()
                        .and_then(|pairs| pairs.get("expiry-date"))
                        .and_then(|value| http_date(value))
                        .map(rfc3339),
                    expiration_rule_id: expiration
                        .as_ref()
                        .and_then(|pairs| pairs.get("rule-id").cloned()),
                    replication_status: stat.replication_status.clone(),
                    metadata: (!stat.metadata.is_empty()).then(|| stat.metadata.clone()),
                    restore: restore.map(|pairs| RestoreJson {
                        ongoing: pairs.get("ongoing-request").is_some_and(|v| v == "true"),
                        expiry: pairs
                            .get("expiry-date")
                            .and_then(|value| http_date(value))
                            .map(rfc3339),
                    }),
                    checksum: (!stat.checksums.is_empty())
                        .then(|| stat.checksums.iter().cloned().collect()),
                    ..Default::default()
                }
            }
        }
    }
}

fn object_text(name: &str, stat: &ObjectStat) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{:<10}: {name}", "Name");
    if let Some(date) = stat.last_modified {
        let _ = writeln!(out, "{:<10}: {} ", "Date", print_date(date));
    }
    let size = human_bytes(stat.size.max(0) as u64);
    let _ = writeln!(out, "{:<10}: {size:<6} ", "Size");
    if let Some(etag) = stat.etag.as_deref().filter(|etag| !etag.is_empty()) {
        let _ = writeln!(out, "{:<10}: {etag} ", "ETag");
    }
    if let Some(version_id) = &stat.version_id {
        let marker = if stat.delete_marker {
            " (delete-marker)"
        } else {
            ""
        };
        let _ = writeln!(out, "{:<10}: {version_id}{marker} ", "VersionID");
    }
    let _ = writeln!(out, "{:<10}: file ", "Type");
    if let Some(expires) = stat.expires.as_deref().and_then(http_date) {
        let _ = writeln!(out, "{:<10}: {} ", "Expires", print_date(expires));
    }
    if let Some(pairs) = stat.expiration.as_deref().map(parse_header_pairs)
        && let Some(date) = pairs.get("expiry-date").and_then(|value| http_date(value))
    {
        let rule = pairs.get("rule-id").map(String::as_str).unwrap_or_default();
        let _ = writeln!(
            out,
            "{:<10}: {} (lifecycle-rule-id: {rule}) ",
            "Expiration",
            print_date(date)
        );
    }
    if !stat.checksums.is_empty() {
        let joined = stat
            .checksums
            .iter()
            .map(|(name, value)| format!("{name}:{value}"))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(out, "{:<10}: {joined}", "Checksum");
    }
    if let Some(pairs) = stat.restore.as_deref().map(parse_header_pairs) {
        let _ = writeln!(out, "{:<10}:", "Restore");
        if let Some(date) = pairs.get("expiry-date").and_then(|value| http_date(value)) {
            let _ = writeln!(out, "  {:<10}: {}", "ExpiryTime", print_date(date));
        }
        let ongoing = pairs.get("ongoing-request").is_some_and(|v| v == "true");
        let _ = writeln!(out, "  {:<10}: {ongoing}", "Ongoing");
    }
    if let Some(encryption) = encryption_label(&stat.metadata) {
        let _ = writeln!(out, "{:<10}: {encryption}", "Encryption");
    }
    let plain: Vec<_> = stat
        .metadata
        .iter()
        .filter(|(key, _)| !key.to_ascii_lowercase().starts_with(ENCRYPTION_PREFIX))
        .collect();
    if !plain.is_empty() {
        let width = plain.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
        let _ = writeln!(out, "{:<10}:", "Metadata");
        for (key, value) in plain {
            let _ = writeln!(out, "  {key:<width$}: {value} ");
        }
    }
    if let Some(status) = &stat.replication_status {
        let _ = write!(out, "{:<10}: {status} ", "Replication Status");
    }
    out
}

/// mc's `Encryption` line, derived from the encryption response headers.
fn encryption_label(metadata: &BTreeMap<String, String>) -> Option<String> {
    if !metadata
        .keys()
        .any(|key| key.to_ascii_lowercase().starts_with(ENCRYPTION_PREFIX))
    {
        return None;
    }
    if let Some(enabled) = metadata.get("X-Amz-Server-Side-Encryption-Bucket-Key-Enabled") {
        // `false` means "no SSE" on some servers; mc prints nothing in that case.
        return (enabled == "true").then(|| "SSE-KMS".to_string());
    }
    if let Some(key_id) = metadata.get("X-Amz-Server-Side-Encryption-Aws-Kms-Key-Id") {
        return Some(format!("SSE-KMS ({key_id})"));
    }
    if metadata.contains_key("X-Amz-Server-Side-Encryption-Customer-Key-Md5") {
        return Some("SSE-C".into());
    }
    if metadata
        .get("X-Amz-Server-Side-Encryption")
        .is_some_and(|algorithm| algorithm == "AES256")
    {
        return Some("SSE-S3".into());
    }
    Some("SSE-Unknown".into())
}

fn bucket_text(bucket: &BucketStat) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{:<10}: {}", "Name", bucket.name);
    let created = bucket.created.unwrap_or_else(SystemTime::now);
    let _ = writeln!(out, "{:<10}: {} ", "Date", print_date(created));
    let _ = writeln!(out, "{:<10}: {:<6} ", "Size", "N/A");
    let _ = writeln!(out, "{:<10}: folder ", "Type");
    let _ = writeln!(out);
    let _ = writeln!(out, "Properties:");
    if !bucket.encryption_algorithm.is_empty() {
        let _ = write!(out, "  Encryption: ");
        if bucket.encryption_algorithm == "aws:kms" {
            let _ = write!(
                out,
                "\n\tKey Type: SSE-KMS\n\tKey ID: {}",
                bucket.encryption_key_id
            );
        } else {
            let algorithm = bucket.encryption_algorithm.to_ascii_uppercase();
            let _ = write!(out, "\n\tKey Type: {algorithm}");
        }
        let _ = writeln!(out);
    }
    let versioning = if bucket.versioning.is_empty() {
        "Un-versioned"
    } else {
        &bucket.versioning
    };
    let _ = writeln!(out, "  Versioning: {versioning}");
    if !bucket.lock_mode.is_empty() {
        let _ = writeln!(out, "  LockConfiguration: ");
        let _ = writeln!(out, "    RetentionMode: {}", bucket.lock_mode);
        let _ = writeln!(out, "    Retention Until Date: {}", bucket.lock_validity);
    }
    if bucket.notification {
        let _ = writeln!(out, "  Notification: Set");
    }
    if bucket.replication {
        let _ = writeln!(out, "  Replication: Enabled");
    }
    let _ = writeln!(out, "  Location: {}", bucket.location);
    let anonymous = if bucket.anonymous {
        "Enabled"
    } else {
        "Disabled"
    };
    let _ = writeln!(out, "  Anonymous: {anonymous}");
    if !bucket.tags.is_empty() {
        let tags = bucket
            .tags
            .iter()
            .map(|(key, value)| format!("{key}:{value}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "  Tagging: {tags}");
    }
    let ilm = if bucket.ilm { "Enabled" } else { "Disabled" };
    let _ = writeln!(out, "  ILM: {ilm}");
    out
}

#[derive(Debug, Default, Serialize)]
struct StatJson {
    status: &'static str,
    target: String,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    bucket: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<i64>,
    #[serde(rename = "lastModified", skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
    #[serde(rename = "eTag", skip_serializing_if = "Option::is_none")]
    etag_legacy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    #[serde(rename = "storageClass", skip_serializing_if = "Option::is_none")]
    storage_class: Option<String>,
    #[serde(rename = "versionID", skip_serializing_if = "Option::is_none")]
    version_id: Option<String>,
    #[serde(rename = "deleteMarker", skip_serializing_if = "std::ops::Not::not")]
    delete_marker: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expiration: Option<String>,
    #[serde(rename = "expirationRuleID", skip_serializing_if = "Option::is_none")]
    expiration_rule_id: Option<String>,
    #[serde(rename = "replicationStatus", skip_serializing_if = "Option::is_none")]
    replication_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restore: Option<RestoreJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum: Option<BTreeMap<String, String>>,
    #[serde(rename = "Versioning", skip_serializing_if = "Option::is_none")]
    versioning: Option<VersioningJson>,
    #[serde(rename = "Encryption", skip_serializing_if = "Option::is_none")]
    encryption: Option<EncryptionJson>,
    #[serde(rename = "ObjectLock", skip_serializing_if = "Option::is_none")]
    object_lock: Option<LockJson>,
    #[serde(rename = "Replication", skip_serializing_if = "Option::is_none")]
    replication: Option<ReplicationJson>,
    #[serde(rename = "Policy", skip_serializing_if = "Option::is_none")]
    policy: Option<PolicyJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tagging: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Serialize)]
struct RestoreJson {
    #[serde(rename = "OngoingRestore")]
    ongoing: bool,
    #[serde(rename = "ExpiryTime", skip_serializing_if = "Option::is_none")]
    expiry: Option<String>,
}

#[derive(Debug, Serialize)]
struct VersioningJson {
    status: String,
    #[serde(rename = "MFADelete")]
    mfa_delete: String,
}

#[derive(Debug, Serialize)]
struct EncryptionJson {
    #[serde(skip_serializing_if = "String::is_empty")]
    algorithm: String,
    #[serde(rename = "keyId", skip_serializing_if = "String::is_empty")]
    key_id: String,
}

#[derive(Debug, Serialize)]
struct LockJson {
    enabled: String,
    mode: String,
    validity: String,
}

#[derive(Debug, Serialize)]
struct ReplicationJson {
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct PolicyJson {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::time::Duration;

    #[derive(Debug, Parser)]
    struct Harness {
        #[command(flatten)]
        args: StatArgs,
    }

    fn parse(args: &[&str]) -> StatArgs {
        let argv = crate::flags::rewrite_argv(std::iter::once("stat").chain(args.iter().copied()));
        Harness::try_parse_from(argv).unwrap().args
    }

    #[test]
    fn validates_flag_combinations() {
        assert!(validate(&parse(&["a/b/c"])).is_ok());
        assert!(validate(&parse(&["--vid", "v1", "a/b/c", "a/b/d"])).is_err());
        assert!(validate(&parse(&["-vid", "v1", "-r", "a/b/c"])).is_err());
        assert!(validate(&parse(&["--version-id", "v1", "--rewind", "1d", "a/b/c"])).is_err());
        assert!(validate(&parse(&["--no-list", "--versions", "a/b/c"])).is_err());
        assert!(validate(&parse(&["--no-list", "--rewind", "1d", "a/b/c"])).is_err());
        assert!(validate(&parse(&["--no-list", "a/b/c"])).is_ok());
    }

    #[test]
    fn formats_sizes_and_dates_like_mc() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(17), "17 B");
        assert_eq!(human_bytes(1000), "1000 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(9 * 1024 * 1024), "9.0 MiB");
        assert_eq!(human_bytes(12 * 1024 * 1024), "12 MiB");
        let time = UNIX_EPOCH + Duration::from_secs(1_704_164_645);
        assert_eq!(print_date(time), "2024-01-02 03:04:05 UTC");
    }

    #[test]
    fn object_text_uses_mc_layout() {
        let mut metadata = BTreeMap::new();
        metadata.insert("Content-Type".to_string(), "text/plain".to_string());
        metadata.insert(
            "X-Amz-Server-Side-Encryption".to_string(),
            "AES256".to_string(),
        );
        let stat = ObjectStat {
            key: "dir/a.txt".into(),
            size: 17,
            last_modified: Some(UNIX_EPOCH + Duration::from_secs(1_704_164_645)),
            etag: Some("abc".into()),
            version_id: Some("v1".into()),
            checksums: vec![("CRC32C".into(), "xyz=".into())],
            metadata,
            ..Default::default()
        };
        assert_eq!(
            object_text("a.txt", &stat),
            "Name      : a.txt\n\
             Date      : 2024-01-02 03:04:05 UTC \n\
             Size      : 17 B   \n\
             ETag      : abc \n\
             VersionID : v1 \n\
             Type      : file \n\
             Checksum  : CRC32C:xyz=\n\
             Encryption: SSE-S3\n\
             Metadata  :\n  Content-Type: text/plain \n"
        );
    }

    #[test]
    fn bucket_text_lists_properties() {
        let text = bucket_text(&BucketStat {
            name: "b".into(),
            created: Some(UNIX_EPOCH),
            versioning: "Enabled".into(),
            location: "us-east-1".into(),
            lock_mode: "GOVERNANCE".into(),
            lock_validity: "1DAYS".into(),
            tags: vec![("k".into(), "v".into())],
            ..Default::default()
        });
        assert!(text.starts_with("Name      : b\nDate      : 1970-01-01 00:00:00 UTC \n"));
        assert!(text.contains("Size      : N/A    \n"));
        assert!(text.contains("  Versioning: Enabled\n"));
        assert!(text.contains("    RetentionMode: GOVERNANCE\n"));
        assert!(text.contains("  Anonymous: Disabled\n  Tagging: k:v\n"));
        assert!(text.ends_with("  ILM: Disabled\n"));
    }
}
