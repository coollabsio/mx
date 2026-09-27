use crate::commands::cat::EncCFlag;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::error::{McError, nonfatal};
use crate::flags::{RewindFlag, VersionIdFlag, resolve_sse};
use crate::location::{Location, parse_location};
use crate::s3::S3ResultExt;
use crate::s3::{
    BucketStat, LifecycleConfig, ListOptions, NotificationConfig, NotificationTarget, ObjectInfo,
    ObjectStat, full_key, parse_header_pairs,
};
use crate::target::TargetRef;
use anyhow::{Context, Result, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::{DateTime, DateTimeFormat};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

mod local;

#[derive(Debug, Args)]
#[command(mut_args(|a| match a.get_id().as_str() {
    "rewind" => a.help("stat on older version(s)"),
    "version_id" => a.help("stat a specific object version"),
    _ => a,
}))]
pub struct StatArgs {
    #[command(flatten)]
    pub rewind: RewindFlag,
    /// stat all versions
    #[arg(long)]
    pub versions: bool,
    #[command(flatten)]
    pub version_id: VersionIdFlag,
    /// stat all objects recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
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
    Ok(())
}

pub fn run(args: StatArgs, json: bool) -> Result<()> {
    validate(&args)?;
    // mc parses the encryption keys before looking at any target.
    args.enc
        .entries()
        .context("Unable to parse encryption keys.")?;
    let rewind = args.rewind.at(SystemTime::now())?;
    let store = ConfigStore::load_or_create()?;
    let rt = runtime()?;
    for input in &args.targets {
        // mc treats unknown aliases as local paths.
        if let Location::Local(_) = parse_location(input, store.config()) {
            local::stat(input, &args, json)?;
            continue;
        }
        let target = TargetRef::parse(input)?;
        let alias = alias_config(&store, &target.alias)?;
        let entries = rt
            .block_on(collect(&alias, &target, &args, rewind))
            .with_context(|| format!("Unable to stat `{input}`."))?;
        for entry in &entries {
            if json {
                entry.print_json()?;
            } else {
                println!("{}", entry.text());
            }
        }
    }
    Ok(())
}

enum StatEntry {
    Bucket(Box<BucketInfo>),
    /// A folder; `time` is None for buckets listed at the alias root (no date).
    Folder {
        name: String,
        time: Option<SystemTime>,
    },
    Object {
        name: String,
        stat: Box<ObjectStat>,
    },
}

/// Everything mc prints for a bucket (`bucketInfoMessage`).
struct BucketInfo {
    stat: BucketStat,
    lifecycle: Option<LifecycleConfig>,
    usage: BucketUsage,
}

async fn bucket_info(
    alias: &crate::config::model::AliasConfig,
    client: &Client,
    bucket: &str,
) -> Result<StatEntry> {
    let stat = crate::s3::stat_bucket(client, bucket).await?;
    let lifecycle = crate::s3::get_lifecycle(alias, bucket)
        .await
        .ok()
        .flatten()
        .map(|info| info.config);
    Ok(StatEntry::Bucket(Box::new(BucketInfo {
        stat,
        lifecycle,
        usage: bucket_usage(alias, bucket).await,
    })))
}

/// madmin `BucketUsageInfo` of `bucket` from the MinIO admin data usage API (zero values when
/// unavailable, like mc).
async fn bucket_usage(alias: &crate::config::model::AliasConfig, bucket: &str) -> BucketUsage {
    let Ok(admin) = crate::s3::admin::AdminClient::new(alias) else {
        return BucketUsage::default();
    };
    let Ok(response) = admin
        .admin("GET", "datausageinfo", &[("capacity", "true")], Vec::new())
        .await
    else {
        return BucketUsage::default();
    };
    serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|value| value.get("bucketsUsageInfo")?.get(bucket).cloned())
        .and_then(|usage| serde_json::from_value(usage).ok())
        .unwrap_or_default()
}

async fn collect(
    alias: &crate::config::model::AliasConfig,
    target: &TargetRef,
    args: &StatArgs,
    rewind: Option<SystemTime>,
) -> Result<Vec<StatEntry>> {
    let client = crate::s3::build_client(alias).await?;
    let Some(bucket) = target.bucket.clone() else {
        let response = client.list_buckets().send().await.s3("", "")?;
        let mut entries = Vec::new();
        for entry in response.buckets() {
            let name = entry.name().unwrap_or_default();
            if args.verbose {
                entries.push(bucket_info(alias, &client, name).await?);
            } else {
                entries.push(StatEntry::Folder {
                    name: format!("{name}/"),
                    time: None,
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
        return match bucket_info(alias, &client, &bucket).await {
            Ok(entry) => Ok(vec![entry]),
            Err(error) if crate::error::error_code(&error) == Some("NoSuchBucket") => {
                // mc reports the missing bucket (`bucketStat`) and then a missing object.
                crate::output::print_error(
                    &anyhow::Error::new(McError::bucket_not_found(&bucket))
                        .context(nonfatal("Unable to list folder.")),
                );
                Err(McError::object_missing().into())
            }
            Err(error) => Err(error),
        };
    };

    // mc skips the HEAD request when rewinding.
    if (args.no_list && rewind.is_none()) || args.version_id.version_id.is_some() {
        let stat = crate::s3::stat_object_sse_c(
            &client,
            &bucket,
            &key,
            args.version_id.version_id.as_deref(),
            sse_c(&key),
        )
        .await?;
        return Ok(vec![object_entry(stat, &relative)]);
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
            return Err(McError::object_missing().into());
        }
        let mut entries = Vec::new();
        for version in versions {
            let stat = stat_version(&client, &bucket, &key, &version, sse_c(&key)).await?;
            entries.push(object_entry(stat, &relative));
        }
        return Ok(entries);
    }
    match crate::s3::stat_object_sse_c(&client, &bucket, &key, None, sse_c(&key)).await {
        Ok(stat) => Ok(vec![object_entry(stat, &relative)]),
        Err(error) if !crate::error::is_not_found(&error) => Err(error),
        Err(_) => {
            // Not an object: report it as a folder if it is a non-empty prefix. Like mc, a
            // listing failure is reported and the object is missing.
            let prefix = format!("{key}/");
            let children = client
                .list_objects_v2()
                .bucket(&bucket)
                .prefix(&prefix)
                .max_keys(1)
                .send()
                .await
                .s3(&bucket, "");
            let children = match children {
                Ok(children) => children,
                Err(error) => {
                    crate::output::print_error(&error.context(nonfatal("Unable to list folder.")));
                    return Err(McError::object_missing().into());
                }
            };
            if children.contents().is_empty() && children.common_prefixes().is_empty() {
                return Err(McError::object_missing().into());
            }
            Ok(vec![StatEntry::Folder {
                name: relative(&prefix),
                time: Some(SystemTime::now()),
            }])
        }
    }
}

fn object_entry(stat: ObjectStat, relative: &dyn Fn(&str) -> String) -> StatEntry {
    StatEntry::Object {
        name: relative(&stat.key),
        stat: Box::new(stat),
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
    let mut items = crate::s3::list_objects_with(client, bucket, prefix, &options).await?;
    // mc lists objects before common prefixes.
    items.sort_by_key(|item| item.is_prefix);
    if items.is_empty() {
        return Err(McError::object_missing().into());
    }
    let base = prefix.unwrap_or_default();
    let mut entries = Vec::new();
    for item in items {
        let key = full_key(base, &item.key);
        if item.is_prefix {
            entries.push(StatEntry::Folder {
                name: relative(&format!("{}/", key.trim_end_matches('/'))),
                time: Some(SystemTime::now()),
            });
            continue;
        }
        let stat = stat_version(client, bucket, &key, &item, sse_c(&key)).await?;
        entries.push(object_entry(stat, relative));
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
            StatEntry::Bucket(info) => bucket_text(info),
            StatEntry::Folder { name, time } => {
                let mut out = format!("{:<10}: {name}\n", "Name");
                if let Some(time) = time {
                    let _ = writeln!(out, "{:<10}: {} ", "Date", print_date(*time));
                }
                let _ = writeln!(out, "{:<10}: folder ", "Type");
                out
            }
            StatEntry::Object { name, stat } => object_text(name, stat),
        }
    }

    fn print_json(&self) -> Result<()> {
        match self {
            StatEntry::Bucket(info) => crate::output::print_json(&bucket_json(info)),
            StatEntry::Folder { name, time } => crate::output::print_json(&StatJson {
                status: "success",
                name: name.clone(),
                last_modified: crate::commands::ls::go_time(time.unwrap_or(UNIX_EPOCH)),
                kind: "folder",
                ..Default::default()
            }),
            StatEntry::Object { name, stat } => {
                let expiration = stat.expiration.as_deref().map(parse_header_pairs);
                let restore = stat.restore.as_deref().map(parse_header_pairs);
                let go_time = crate::commands::ls::go_time;
                crate::output::print_json(&StatJson {
                    status: "success",
                    name: name.clone(),
                    last_modified: go_time(stat.last_modified.unwrap_or(UNIX_EPOCH)),
                    size: stat.size,
                    etag: stat.etag.clone().unwrap_or_default(),
                    kind: "file",
                    expires: stat.expires.as_deref().and_then(http_date).map(go_time),
                    expiration: expiration
                        .as_ref()
                        .and_then(|pairs| pairs.get("expiry-date"))
                        .and_then(|value| http_date(value))
                        .map(go_time),
                    expiration_rule_id: expiration
                        .as_ref()
                        .and_then(|pairs| pairs.get("rule-id").cloned()),
                    replication_status: stat.replication_status.clone(),
                    metadata: (!stat.metadata.is_empty()).then(|| stat.metadata.clone()),
                    version_id: stat.version_id.clone(),
                    delete_marker: stat.delete_marker,
                    restore: restore.map(|pairs| RestoreJson {
                        ongoing: pairs.get("ongoing-request").is_some_and(|v| v == "true"),
                        expiry: pairs
                            .get("expiry-date")
                            .and_then(|value| http_date(value))
                            .map(go_time),
                    }),
                    checksum: (!stat.checksums.is_empty())
                        .then(|| stat.checksums.iter().cloned().collect()),
                })
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

/// mc `GetAccess` policy type: a canned policy, `custom`, or `none`.
fn policy_type(bucket: &BucketStat) -> &'static str {
    if bucket.policy.is_empty() {
        return "none";
    }
    match crate::commands::anonymous::parse_policy(&bucket.policy) {
        Ok(document) => {
            match crate::commands::anonymous::get_policy(&document.statements, &bucket.name, "") {
                crate::commands::anonymous::BucketPolicy::None => "custom",
                policy => policy.as_str(),
            }
        }
        // mc leaves the type empty when the policy cannot be parsed.
        Err(_) => "",
    }
}

/// mc `bucketInfoMessage.String()` without its final newline.
fn bucket_text(info: &BucketInfo) -> String {
    let bucket = &info.stat;
    let mut out = String::new();
    let _ = writeln!(out, "{:<10}: {}", "Name", bucket.name);
    let _ = writeln!(out, "{:<10}: {} ", "Date", print_date(SystemTime::now()));
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
    // mc only reports topic notifications here.
    if bucket
        .notification
        .as_ref()
        .is_some_and(|config| !config.topic.is_empty())
    {
        let _ = writeln!(out, "  Notification: Set");
    }
    if bucket.replication {
        let _ = writeln!(out, "  Replication: Enabled");
    }
    let _ = writeln!(out, "  Location: {}", bucket.location);
    let anonymous = if policy_type(bucket) == "none" {
        "Disabled"
    } else {
        "Enabled"
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
    let _ = writeln!(out);
    out.push_str(&usage_text(&info.usage));
    out.pop();
    out
}

/// mc `Usage:` block (and the object size histogram when the server reports one).
fn usage_text(usage: &BucketUsage) -> String {
    let comma = |value: u64| crate::s3::admin::comma(value as i64);
    let mut out = String::from("Usage:\n");
    let _ = writeln!(out, "{:>16}: {}", "Total size", human_bytes(usage.size));
    let _ = writeln!(
        out,
        "{:>16}: {}",
        "Objects count",
        comma(usage.objects_count)
    );
    let _ = writeln!(
        out,
        "{:>16}: {}",
        "Versions count",
        comma(usage.versions_count)
    );
    let _ = writeln!(out);
    if let Some(histogram) = usage.sizes_histogram.as_ref().filter(|h| !h.is_empty()) {
        out.push_str("Object sizes histogram:\n");
        let width = histogram
            .values()
            .map(|value| {
                if *value == 0 {
                    0
                } else {
                    value.to_string().len()
                }
            })
            .max()
            .unwrap_or(0);
        for (name, value) in histogram {
            let _ = writeln!(out, "   {value:>width$} object(s) {name}");
        }
    }
    out
}

fn bucket_json(info: &BucketInfo) -> BucketJson<'_> {
    let bucket = &info.stat;
    BucketJson {
        status: "success",
        name: format!("{}/", bucket.name),
        last_modified: crate::commands::ls::go_time(SystemTime::now()),
        size: 0,
        versioning: VersioningJson {
            status: bucket.versioning.clone(),
            mfa_delete: bucket.mfa_delete.clone(),
        },
        encryption: EncryptionJson {
            algorithm: bucket.encryption_algorithm.clone(),
            key_id: bucket.encryption_key_id.clone(),
        },
        object_lock: LockJson {
            enabled: bucket.lock_enabled.clone(),
            mode: bucket.lock_mode.clone(),
            validity: bucket.lock_validity.clone(),
        },
        // mc never fills in the replication config here.
        replication: ReplicationJson {
            enabled: bucket.replication,
            config: ReplicationConfigJson {
                rules: None,
                role: "",
            },
        },
        policy: PolicyJson {
            kind: policy_type(bucket),
            policy: &bucket.policy,
        },
        location: &bucket.location,
        tagging: (!bucket.tags.is_empty()).then(|| bucket.tags.iter().cloned().collect()),
        ilm: IlmJson {
            config: info.lifecycle.as_ref(),
        },
        notification: NotificationJson {
            config: NotificationDoc::new(bucket.notification.as_ref()),
        },
        usage: &info.usage,
    }
}

/// mc `statMessage` (objects and folders).
#[derive(Debug, Default, Serialize)]
struct StatJson {
    status: &'static str,
    name: String,
    #[serde(rename = "lastModified")]
    last_modified: String,
    size: i64,
    etag: String,
    #[serde(rename = "type")]
    kind: &'static str,
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
    #[serde(rename = "versionID", skip_serializing_if = "Option::is_none")]
    version_id: Option<String>,
    #[serde(rename = "deleteMarker", skip_serializing_if = "std::ops::Not::not")]
    delete_marker: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    restore: Option<RestoreJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Serialize)]
struct RestoreJson {
    #[serde(rename = "OngoingRestore")]
    ongoing: bool,
    #[serde(rename = "ExpiryTime", skip_serializing_if = "Option::is_none")]
    expiry: Option<String>,
}

/// mc `bucketInfoMessage` JSON.
#[derive(Debug, Serialize)]
struct BucketJson<'a> {
    status: &'static str,
    name: String,
    #[serde(rename = "lastModified")]
    last_modified: String,
    size: i64,
    #[serde(rename = "Versioning")]
    versioning: VersioningJson,
    #[serde(rename = "Encryption")]
    encryption: EncryptionJson,
    #[serde(rename = "ObjectLock")]
    object_lock: LockJson,
    #[serde(rename = "Replication")]
    replication: ReplicationJson,
    #[serde(rename = "Policy")]
    policy: PolicyJson<'a>,
    location: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tagging: Option<BTreeMap<String, String>>,
    ilm: IlmJson<'a>,
    notification: NotificationJson<'a>,
    #[serde(rename = "Usage")]
    usage: &'a BucketUsage,
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
    config: ReplicationConfigJson,
}

#[derive(Debug, Serialize)]
struct ReplicationConfigJson {
    #[serde(rename = "Rules")]
    rules: Option<Vec<()>>,
    #[serde(rename = "Role")]
    role: &'static str,
}

#[derive(Debug, Serialize)]
struct PolicyJson<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "str::is_empty")]
    policy: &'a str,
}

#[derive(Debug, Serialize)]
struct IlmJson<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<&'a LifecycleConfig>,
}

#[derive(Debug, Serialize)]
struct NotificationJson<'a> {
    config: NotificationDoc<'a>,
}

/// minio-go `notification.Configuration` (Go field names, null for empty lists).
#[derive(Debug, Serialize)]
struct NotificationDoc<'a> {
    #[serde(rename = "XMLName")]
    xml_name: XmlNameJson,
    #[serde(rename = "LambdaConfigs")]
    lambda: Option<Vec<NotificationTargetJson<'a>>>,
    #[serde(rename = "TopicConfigs")]
    topic: Option<Vec<NotificationTargetJson<'a>>>,
    #[serde(rename = "QueueConfigs")]
    queue: Option<Vec<NotificationTargetJson<'a>>>,
}

impl<'a> NotificationDoc<'a> {
    fn new(config: Option<&'a NotificationConfig>) -> Self {
        let targets = |list: &'a [NotificationTarget], field: &'static str| {
            (!list.is_empty()).then(|| {
                list.iter()
                    .map(|target| NotificationTargetJson::new(target, field))
                    .collect()
            })
        };
        Self {
            xml_name: XmlNameJson {
                space: if config.is_some() {
                    crate::s3::bucket::S3_XMLNS
                } else {
                    ""
                },
                local: if config.is_some() {
                    "NotificationConfiguration"
                } else {
                    ""
                },
            },
            lambda: config.and_then(|c| targets(&c.lambda, "Lambda")),
            topic: config.and_then(|c| targets(&c.topic, "Topic")),
            queue: config.and_then(|c| targets(&c.queue, "Queue")),
        }
    }
}

#[derive(Debug, Serialize)]
struct XmlNameJson {
    #[serde(rename = "Space")]
    space: &'static str,
    #[serde(rename = "Local")]
    local: &'static str,
}

#[derive(Debug, Serialize)]
struct NotificationTargetJson<'a> {
    #[serde(rename = "ID")]
    id: &'a str,
    #[serde(rename = "Arn")]
    arn: ArnJson,
    #[serde(rename = "Events")]
    events: &'a [String],
    #[serde(rename = "Filter")]
    filter: Option<FilterJson<'a>>,
    /// `Lambda` / `Topic` / `Queue` -> ARN.
    #[serde(flatten)]
    target: BTreeMap<&'static str, &'a str>,
}

impl<'a> NotificationTargetJson<'a> {
    fn new(target: &'a NotificationTarget, field: &'static str) -> Self {
        Self {
            id: &target.id,
            arn: ArnJson::default(),
            events: &target.events,
            filter: target.filter.as_ref().map(|rules| FilterJson {
                s3_key: S3KeyJson {
                    rules: (!rules.is_empty()).then(|| {
                        rules
                            .iter()
                            .map(|(name, value)| FilterRuleJson { name, value })
                            .collect()
                    }),
                },
            }),
            target: BTreeMap::from([(field, target.arn.as_str())]),
        }
    }
}

/// minio-go `notification.Arn` is not read from XML, so it is always empty.
#[derive(Debug, Default, Serialize)]
struct ArnJson {
    #[serde(rename = "Partition")]
    partition: &'static str,
    #[serde(rename = "Service")]
    service: &'static str,
    #[serde(rename = "Region")]
    region: &'static str,
    #[serde(rename = "AccountID")]
    account_id: &'static str,
    #[serde(rename = "Resource")]
    resource: &'static str,
}

#[derive(Debug, Serialize)]
struct FilterJson<'a> {
    #[serde(rename = "S3Key")]
    s3_key: S3KeyJson<'a>,
}

#[derive(Debug, Serialize)]
struct S3KeyJson<'a> {
    #[serde(rename = "FilterRules")]
    rules: Option<Vec<FilterRuleJson<'a>>>,
}

#[derive(Debug, Serialize)]
struct FilterRuleJson<'a> {
    #[serde(rename = "Name")]
    name: &'a str,
    #[serde(rename = "Value")]
    value: &'a str,
}

/// madmin `BucketUsageInfo`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct BucketUsage {
    size: u64,
    #[serde(rename = "objectsPendingReplicationTotalSize")]
    pending_size: u64,
    #[serde(rename = "objectsFailedReplicationTotalSize")]
    failed_size: u64,
    #[serde(rename = "objectsReplicatedTotalSize")]
    replicated_size: u64,
    #[serde(rename = "objectReplicaTotalSize")]
    replica_size: u64,
    #[serde(rename = "objectsPendingReplicationCount")]
    pending_count: u64,
    #[serde(rename = "objectsFailedReplicationCount")]
    failed_count: u64,
    #[serde(rename = "versionsCount")]
    versions_count: u64,
    #[serde(rename = "objectsCount")]
    objects_count: u64,
    #[serde(rename = "deleteMarkersCount")]
    delete_markers_count: u64,
    #[serde(rename = "objectsSizesHistogram")]
    sizes_histogram: Option<BTreeMap<String, u64>>,
    #[serde(rename = "objectsVersionsHistogram")]
    versions_histogram: Option<BTreeMap<String, u64>>,
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
        assert!(validate(&parse(&["--no-list", "--rewind", "1d", "a/b/c"])).is_ok());
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
    fn bucket_text_lists_properties_and_usage() {
        let mut info = BucketInfo {
            stat: BucketStat {
                name: "b".into(),
                versioning: "Enabled".into(),
                location: "us-east-1".into(),
                lock_mode: "GOVERNANCE".into(),
                lock_validity: "1DAYS".into(),
                tags: vec![("k".into(), "v".into())],
                ..Default::default()
            },
            lifecycle: None,
            usage: BucketUsage {
                size: 2048,
                objects_count: 1234,
                versions_count: 1234,
                ..Default::default()
            },
        };
        let text = bucket_text(&info);
        assert!(text.starts_with("Name      : b\nDate      : "));
        assert!(text.contains("Size      : N/A    \nType      : folder \n\nProperties:\n"));
        assert!(text.contains("  Versioning: Enabled\n"));
        assert!(text.contains("    RetentionMode: GOVERNANCE\n"));
        assert!(text.contains("  Anonymous: Disabled\n  Tagging: k:v\n"));
        assert!(text.ends_with(
            "  ILM: Disabled\n\nUsage:\n      Total size: 2.0 KiB\n   Objects count: 1,234\n  Versions count: 1,234\n"
        ));
        info.usage.sizes_histogram = Some(BTreeMap::from([
            ("A".to_string(), 0),
            ("B".to_string(), 12),
        ]));
        assert!(bucket_text(&info).ends_with(
            "Versions count: 1,234\n\nObject sizes histogram:\n    0 object(s) A\n   12 object(s) B"
        ));
    }

    #[test]
    fn bucket_json_matches_mc_shape() {
        let info = BucketInfo {
            stat: BucketStat {
                name: "b".into(),
                location: "us-east-1".into(),
                notification: Some(NotificationConfig {
                    queue: vec![NotificationTarget {
                        id: "1".into(),
                        events: vec!["s3:ObjectCreated:*".into()],
                        filter: Some(vec![("prefix".into(), "p/".into())]),
                        arn: "arn:minio:sqs::X:webhook".into(),
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
            lifecycle: None,
            usage: BucketUsage::default(),
        };
        let doc = serde_json::to_string(&bucket_json(&info)).unwrap();
        assert!(doc.starts_with(r#"{"status":"success","name":"b/","lastModified":"#));
        assert!(doc.contains(r#""Replication":{"enabled":false,"config":{"Rules":null,"Role":""}},"Policy":{"type":"none"},"location":"us-east-1","ilm":{},"notification""#));
        assert!(doc.contains(r#""QueueConfigs":[{"ID":"1","Arn":{"Partition":"","Service":"","Region":"","AccountID":"","Resource":""},"Events":["s3:ObjectCreated:*"],"Filter":{"S3Key":{"FilterRules":[{"Name":"prefix","Value":"p/"}]}},"Queue":"arn:minio:sqs::X:webhook"}]"#));
        assert!(doc.ends_with(r#""objectsSizesHistogram":null,"objectsVersionsHistogram":null}}"#));
    }
}
