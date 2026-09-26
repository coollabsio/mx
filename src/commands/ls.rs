use crate::commands::stat::{human_bytes, rfc3339};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::RewindFlag;
use crate::s3::{ListOptions, ObjectInfo, S3ListItem};
use crate::target::TargetRef;
use anyhow::{Result, bail};
use clap::Args;
use serde::Serialize;
use std::cmp::Ordering;
use std::io::{self, Write};
use std::time::SystemTime;
use tabwriter::TabWriter;

#[derive(Debug, Args)]
pub struct LsArgs {
    #[command(flatten)]
    pub rewind: RewindFlag,
    /// list all versions
    #[arg(long)]
    pub versions: bool,
    /// list recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
    /// list incomplete uploads
    #[arg(short = 'I', long)]
    pub incomplete: bool,
    /// display summary information (number of objects, total size)
    #[arg(long)]
    pub summarize: bool,
    /// filter to specified storage class
    #[arg(long = "storage-class", value_name = "CLASS")]
    pub storage_class: Option<String>,
    /// list files inside zip archive (MinIO servers only)
    #[arg(long)]
    pub zip: bool,
    pub target: String,
}

/// Validates flag combinations for `ls`.
pub fn validate(args: &LsArgs, target: &TargetRef) -> Result<()> {
    let has_rewind = args.rewind.rewind.is_some();
    if args.zip && (args.versions || has_rewind) {
        bail!("Zip file listing can only be performed on the latest version");
    }
    if args.incomplete && (args.versions || has_rewind || args.zip) {
        bail!("You cannot specify --incomplete with --versions, --rewind or --zip.");
    }
    if target.is_alias_root() && (args.versions || has_rewind || args.incomplete || args.zip) {
        bail!(
            "--versions, --rewind, --incomplete and --zip require a bucket target like `ALIAS/BUCKET`."
        );
    }
    Ok(())
}

pub fn run(args: LsArgs, json: bool) -> Result<()> {
    let target = TargetRef::parse(&args.target)?;
    validate(&args, &target)?;
    let rewind = args.rewind.at(SystemTime::now())?;
    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &target.alias)?;

    let runtime = runtime()?;
    let mut entries = match &target.bucket {
        None => runtime
            .block_on(crate::s3::list_target(&alias, &target, args.recursive))?
            .into_iter()
            .map(Entry::from_list_item)
            .collect(),
        Some(bucket) => {
            let prefix = target.key_with_trailing_slash();
            let options = ListOptions {
                recursive: args.recursive,
                versions: args.versions || rewind.is_some(),
                incomplete: args.incomplete,
                zip: args.zip,
                ..Default::default()
            };
            let items = runtime.block_on(async {
                let client = crate::s3::build_client(&alias).await?;
                crate::s3::list_objects_with(&client, bucket, prefix.as_deref(), &options).await
            })?;
            let show_versions = args.versions || rewind.is_some();
            select_entries(items, show_versions, args.versions, rewind)
        }
    };
    if let Some(class) = args.storage_class.as_deref() {
        entries.retain(|entry| entry.matches_storage_class(class));
    }
    entries.sort_by(compare_entries);

    let objects: Vec<_> = entries
        .iter()
        .filter(|entry| entry.kind == Kind::Object)
        .collect();
    let total_objects = objects.len();
    let total_size: i64 = objects.iter().filter_map(|entry| entry.size).sum();
    let show_versions = args.versions || rewind.is_some();

    if json {
        for entry in &entries {
            crate::output::print_json(&ListMessage::from_entry(&args.target, entry))?;
        }
        if args.summarize {
            crate::output::print_json(&serde_json::json!({
                "totalObjects": total_objects,
                "totalSize": total_size,
            }))?;
        }
        return Ok(());
    }

    print_plain(&entries, show_versions)?;
    if args.summarize {
        println!(
            "\nTotal Size: {}\nTotal Objects: {total_objects}",
            human_bytes(total_size.max(0) as u64)
        );
    }
    Ok(())
}

/// Turns a version listing into entries with mc version ordinals (latest = highest).
/// Without `all_versions`, only the newest entry per key is kept and deleted keys are dropped
/// (this is the `--rewind` view; the listing was already restricted to `<= rewind`).
fn select_entries(
    items: Vec<ObjectInfo>,
    show_versions: bool,
    all_versions: bool,
    rewind: Option<SystemTime>,
) -> Vec<Entry> {
    if !show_versions {
        return items.into_iter().map(Entry::from_object_info).collect();
    }
    let items = match rewind {
        Some(at) => crate::s3::versions_before(items, at),
        None => items,
    };
    let mut entries = Vec::new();
    let mut index = 0;
    while index < items.len() {
        let end = if items[index].is_prefix {
            index + 1
        } else {
            index
                + items[index..]
                    .iter()
                    .take_while(|item| !item.is_prefix && item.key == items[index].key)
                    .count()
        };
        let count = end - index;
        for (position, item) in items[index..end].iter().enumerate() {
            let mut entry = Entry::from_object_info(item.clone());
            if !item.is_prefix {
                entry.version_ordinal = Some(count - position);
            }
            if all_versions || item.is_prefix {
                entries.push(entry);
            } else {
                if !item.is_delete_marker {
                    entries.push(entry);
                }
                break;
            }
        }
        index = end;
    }
    entries
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Bucket,
    Prefix,
    Object,
}

#[derive(Debug, Clone)]
struct Entry {
    kind: Kind,
    name: String,
    size: Option<i64>,
    last_modified: Option<String>,
    etag: Option<String>,
    storage_class: Option<String>,
    version_id: Option<String>,
    version_ordinal: Option<usize>,
    is_delete_marker: bool,
}

impl Entry {
    fn from_list_item(item: S3ListItem) -> Self {
        match item {
            S3ListItem::Bucket {
                name,
                last_modified,
            } => Self::new(Kind::Bucket, name, last_modified),
            S3ListItem::Prefix { name } => Self::new(Kind::Prefix, name, None),
            S3ListItem::Object {
                name,
                size,
                last_modified,
                etag,
                storage_class,
                ..
            } => Self {
                size,
                etag,
                storage_class,
                ..Self::new(Kind::Object, name, last_modified)
            },
        }
    }

    fn from_object_info(item: ObjectInfo) -> Self {
        if item.is_prefix {
            return Self::new(Kind::Prefix, item.key, None);
        }
        Self {
            size: Some(item.size),
            etag: item.etag,
            storage_class: item.storage_class,
            version_id: item.version_id,
            is_delete_marker: item.is_delete_marker,
            ..Self::new(Kind::Object, item.key, item.last_modified.map(rfc3339))
        }
    }

    fn new(kind: Kind, name: String, last_modified: Option<String>) -> Self {
        Self {
            kind,
            name,
            size: None,
            last_modified,
            etag: None,
            storage_class: None,
            version_id: None,
            version_ordinal: None,
            is_delete_marker: false,
        }
    }

    /// mc `--storage-class` filter: entries without a storage class always pass.
    fn matches_storage_class(&self, class: &str) -> bool {
        match self.storage_class.as_deref() {
            Some(value) if !class.is_empty() && class != "*" => value == class,
            _ => true,
        }
    }

    /// `VERSION_ID vN PUT|DEL` like mc's version column.
    fn version_text(&self) -> String {
        match (&self.version_id, self.version_ordinal) {
            (Some(id), Some(ordinal)) => {
                let op = if self.is_delete_marker { "DEL" } else { "PUT" };
                format!("{id} v{ordinal} {op}")
            }
            _ => "-".into(),
        }
    }
}

fn print_plain(entries: &[Entry], show_versions: bool) -> Result<()> {
    let mut writer = TabWriter::new(io::stdout()).padding(2);
    if show_versions {
        writeln!(writer, "Type\tModified\tSize\tVersion\tName")?;
    } else {
        writeln!(writer, "Type\tModified\tSize\tName")?;
    }

    for entry in entries {
        let (kind, modified, size, name) = plain_row(entry);
        if show_versions {
            let version = entry.version_text();
            writeln!(writer, "{kind}\t{modified}\t{size}\t{version}\t{name}")?;
        } else {
            writeln!(writer, "{kind}\t{modified}\t{size}\t{name}")?;
        }
    }

    writer.flush()?;
    Ok(())
}

fn plain_row(entry: &Entry) -> (&'static str, String, String, String) {
    let modified = entry.last_modified.clone().unwrap_or_else(|| "-".into());
    match entry.kind {
        Kind::Bucket => ("BUCKET", modified, "-".into(), format!("{}/", entry.name)),
        Kind::Prefix => ("PREFIX", "-".into(), "-".into(), entry.name.clone()),
        Kind::Object => (
            "OBJECT",
            modified,
            entry
                .size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            entry.name.clone(),
        ),
    }
}

/// Buckets, then prefixes, then objects; by name. Stable, so versions keep newest-first order.
fn compare_entries(left: &Entry, right: &Entry) -> Ordering {
    left.kind
        .cmp(&right.kind)
        .then_with(|| left.name.cmp(&right.name))
}

#[derive(Debug, Serialize)]
struct ListMessage<'a> {
    status: &'static str,
    target: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    name: &'a str,
    #[serde(rename = "lastModified", skip_serializing_if = "Option::is_none")]
    last_modified: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<&'a str>,
    #[serde(rename = "storageClass", skip_serializing_if = "Option::is_none")]
    storage_class: Option<&'a str>,
    #[serde(rename = "versionId", skip_serializing_if = "Option::is_none")]
    version_id: Option<&'a str>,
    #[serde(rename = "versionOrdinal", skip_serializing_if = "Option::is_none")]
    version_ordinal: Option<usize>,
    #[serde(rename = "isDeleteMarker", skip_serializing_if = "std::ops::Not::not")]
    is_delete_marker: bool,
}

impl<'a> ListMessage<'a> {
    fn from_entry(target: &'a str, entry: &'a Entry) -> Self {
        Self {
            status: "success",
            target,
            kind: match entry.kind {
                Kind::Bucket => "bucket",
                Kind::Prefix => "prefix",
                Kind::Object => "object",
            },
            name: &entry.name,
            last_modified: entry.last_modified.as_deref(),
            size: entry.size,
            etag: entry.etag.as_deref(),
            storage_class: entry.storage_class.as_deref(),
            version_id: entry.version_id.as_deref(),
            version_ordinal: entry.version_ordinal,
            is_delete_marker: entry.is_delete_marker,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn version(key: &str, secs: u64, id: &str, marker: bool) -> ObjectInfo {
        ObjectInfo {
            key: key.into(),
            size: 1,
            last_modified: Some(UNIX_EPOCH + Duration::from_secs(secs)),
            version_id: Some(id.into()),
            is_delete_marker: marker,
            ..Default::default()
        }
    }

    fn sample() -> Vec<ObjectInfo> {
        vec![
            ObjectInfo {
                key: "dir/".into(),
                is_prefix: true,
                ..Default::default()
            },
            version("a", 30, "a3", false),
            version("a", 20, "a2", false),
            version("a", 10, "a1", false),
            version("b", 25, "b2", true),
            version("b", 5, "b1", false),
        ]
    }

    fn rows(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| format!("{} {}", entry.name, entry.version_text()))
            .collect()
    }

    #[test]
    fn numbers_versions_newest_highest() {
        let entries = select_entries(sample(), true, true, None);
        assert_eq!(
            rows(&entries),
            [
                "dir/ -",
                "a a3 v3 PUT",
                "a a2 v2 PUT",
                "a a1 v1 PUT",
                "b b2 v2 DEL",
                "b b1 v1 PUT"
            ]
        );
    }

    #[test]
    fn rewind_shows_state_at_time() {
        let at = Some(UNIX_EPOCH + Duration::from_secs(22));
        let entries = select_entries(sample(), true, false, at);
        assert_eq!(rows(&entries), ["dir/ -", "a a2 v2 PUT", "b b1 v1 PUT"]);
        let later = Some(UNIX_EPOCH + Duration::from_secs(26));
        let entries = select_entries(sample(), true, false, later);
        assert_eq!(rows(&entries), ["dir/ -", "a a2 v2 PUT"]);
        let entries = select_entries(sample(), true, true, at);
        assert_eq!(
            rows(&entries),
            ["dir/ -", "a a2 v2 PUT", "a a1 v1 PUT", "b b1 v1 PUT"]
        );
    }

    #[test]
    fn storage_class_filter_matches_mc() {
        let mut entry = Entry::from_object_info(version("a", 1, "v", false));
        assert!(entry.matches_storage_class("GLACIER"));
        entry.storage_class = Some("STANDARD".into());
        assert!(entry.matches_storage_class("STANDARD"));
        assert!(entry.matches_storage_class("*"));
        assert!(!entry.matches_storage_class("GLACIER"));
    }
}
