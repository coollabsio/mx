use crate::commands::stat::{human_bytes, print_date};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use crate::error::nonfatal;
use crate::flags::RewindFlag;
use crate::location::{Location, parse_location};
use crate::s3::{ListOptions, ObjectInfo};
use crate::target::TargetRef;
use anyhow::{Result, bail};
use aws_sdk_s3::primitives::{DateTime, DateTimeFormat};
use clap::Args;
use serde::Serialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Args)]
#[command(mut_args(|a| if a.get_id().as_str() == "rewind" {
    a.help("list all object versions no later than specified date")
} else {
    a
}))]
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
    #[arg(long = "storage-class", visible_alias = "sc", value_name = "CLASS")]
    pub storage_class: Option<String>,
    /// list files inside zip archive (MinIO servers only)
    #[arg(long)]
    pub zip: bool,
    /// targets to list (default: current folder)
    #[arg(value_name = "TARGET")]
    pub targets: Vec<String>,
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
    let store = ConfigStore::load_or_create()?;
    let rewind = args.rewind.at(SystemTime::now())?;
    let targets = if args.targets.is_empty() {
        vec![".".to_string()]
    } else {
        args.targets.clone()
    };
    if targets.iter().any(|target| target.trim().is_empty()) {
        return Err(
            anyhow::Error::new(crate::error::McError::invalid_argument())
                .context("Unable to validate empty argument."),
        );
    }
    let opts = ListOpts {
        recursive: args.recursive,
        incomplete: args.incomplete,
        versions: args.versions,
        rewind,
        zip: args.zip,
    };
    let rt = runtime()?;
    let mut failed = false;
    for input in &targets {
        if let Location::S3(target) = parse_location(input, store.config()) {
            validate(&args, &target)?;
        }
        let listing = list(&store, &rt, input, &opts).map_err(|error| {
            // mc checks a bucket root with `bucketStat` first (``Bucket `b` does not exist.``);
            // other listings report the server's message.
            match parse_location(input, store.config()) {
                Location::S3(TargetRef {
                    bucket: Some(bucket),
                    key: None,
                    ..
                }) if !args.recursive
                    && !args.versions
                    && rewind.is_none()
                    && !args.incomplete
                    && crate::error::error_code(&error) == Some("NoSuchBucket") =>
                {
                    crate::error::McError::bucket_not_found(&bucket).into()
                }
                _ => error,
            }
        });
        let (objects, size) = match listing {
            Ok(listing) => {
                // mc keeps walking past unreadable folders, reports each and exits 1; an
                // unreadable walk root is skipped silently.
                for denied in listing.denied.iter().filter(|denied| !denied.root) {
                    crate::output::print_error(&denied.error());
                    failed = true;
                }
                print_listing(&listing, args.storage_class.as_deref(), json)?
            }
            Err(error) => {
                crate::output::print_error(&error.context(nonfatal("Unable to list folder.")));
                failed = true;
                (0, 0)
            }
        };
        if args.summarize {
            print_summary(objects, size, json)?;
        }
    }
    if failed {
        return Err(crate::output::Exit(1).into());
    }
    Ok(())
}

/// Listing options (mc `doListOptions` / `ListOptions`).
#[derive(Debug, Clone, Default)]
pub(crate) struct ListOpts {
    pub recursive: bool,
    pub incomplete: bool,
    /// Every version and delete marker.
    pub versions: bool,
    /// State as of this time (with `versions`: every version at or before it).
    pub rewind: Option<SystemTime>,
    pub zip: bool,
}

/// One listed entry (mc `ClientContent` plus its version ordinal).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Content {
    /// Key relative to the listed folder; folders end with `/`.
    pub key: String,
    pub time: SystemTime,
    pub size: i64,
    /// ETag without quotes.
    pub etag: String,
    pub is_dir: bool,
    pub storage_class: String,
    pub version_id: String,
    pub is_delete_marker: bool,
    /// mc `versionOrdinal` (latest = highest; 1 for plain listings).
    pub ordinal: usize,
}

impl Content {
    fn folder(key: String, time: SystemTime, size: i64) -> Self {
        Self {
            key,
            time,
            size,
            etag: String::new(),
            is_dir: true,
            storage_class: String::new(),
            version_id: String::new(),
            is_delete_marker: false,
            ordinal: 1,
        }
    }
}

/// Result of listing one target: mc's target URL (`url` in JSON) and the entries in mc order.
#[derive(Debug, Clone, Default)]
pub(crate) struct Listing {
    pub url: String,
    pub contents: Vec<Content>,
    /// Local recursive listings: folders skipped for lack of permission (mc
    /// `PathInsufficientPermission` entries), the walked root first if it was unreadable.
    pub denied: Vec<Denied>,
}

/// An unreadable folder met by a local recursive listing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Denied {
    pub path: String,
    /// The folder the walk started from (mc `ls` skips its error silently, `du` reports it).
    pub root: bool,
}

impl Denied {
    /// mc's error for the entry (`Unable to list folder.` + `PathInsufficientPermission`).
    pub fn error(&self) -> anyhow::Error {
        anyhow::Error::new(crate::error::McError::path_insufficient_permission(
            &self.path,
        ))
        .context(nonfatal("Unable to list folder."))
    }
}

/// Lists one target like mc `ls` (a folder target without a trailing `/` is listed as a
/// folder): S3 targets, the alias root (buckets) and local paths.
pub(crate) fn list(
    store: &ConfigStore,
    rt: &tokio::runtime::Runtime,
    input: &str,
    opts: &ListOpts,
) -> Result<Listing> {
    match parse_location(input, store.config()) {
        Location::Local(_) => list_local(input, opts),
        Location::S3(target) => {
            let alias = alias_config(store, &target.alias)?;
            rt.block_on(list_s3(&alias, &target, opts))
        }
    }
}

async fn list_s3(alias: &AliasConfig, target: &TargetRef, opts: &ListOpts) -> Result<Listing> {
    let client = crate::s3::build_client(alias).await?;
    let base = alias.url.trim_end_matches('/');
    let options = ListOptions {
        recursive: opts.recursive,
        versions: opts.versions || opts.rewind.is_some(),
        incomplete: opts.incomplete,
        zip: opts.zip,
        ..Default::default()
    };
    let Some(bucket) = &target.bucket else {
        let buckets = crate::s3::list_bucket_infos(&client).await?;
        let mut contents = Vec::new();
        for (name, created) in buckets {
            if !opts.recursive {
                contents.push(Content::folder(
                    format!("{name}/"),
                    created.unwrap_or(UNIX_EPOCH),
                    0,
                ));
                continue;
            }
            let items = crate::s3::list_raw(&client, &name, "", &options).await?;
            contents.extend(
                select(items, opts)
                    .into_iter()
                    .map(|(item, ordinal)| content(&name, "", item, ordinal)),
            );
        }
        return Ok(Listing {
            url: format!("{base}/"),
            contents,
            ..Default::default()
        });
    };
    let mut key = target.key_with_trailing_slash().unwrap_or_default();
    if !key.is_empty()
        && !key.ends_with('/')
        && crate::s3::prefix_has_entries(&client, bucket, &format!("{key}/"), options.versions)
            .await
            .unwrap_or(false)
    {
        key.push('/');
    }
    let url = format!("{base}/{bucket}/{key}");
    let prefix_path = &key[..key.rfind('/').map_or(0, |index| index + 1)];
    // MinIO lists zip contents only below `archive.zip/`; mc keeps the keys relative to the
    // target's folder (`archive.zip/inner.txt`).
    let request = if opts.zip && !key.is_empty() && !key.ends_with('/') {
        format!("{key}/")
    } else {
        key.clone()
    };
    let items = crate::s3::list_raw(&client, bucket, &request, &options).await?;
    let contents = select(items, opts)
        .into_iter()
        .map(|(item, ordinal)| content("", prefix_path, item, ordinal))
        .collect();
    Ok(Listing {
        url,
        contents,
        ..Default::default()
    })
}

/// Version selection: `versions` keeps every version (at or before `rewind`), numbered newest
/// highest; `rewind` alone keeps the newest version at that time unless it is a delete
/// marker (numbered with the count of versions). Plain listings pass through.
fn select(items: Vec<ObjectInfo>, opts: &ListOpts) -> Vec<(ObjectInfo, usize)> {
    if !opts.versions && opts.rewind.is_none() {
        return items.into_iter().map(|item| (item, 1)).collect();
    }
    let items = match opts.rewind {
        Some(at) => crate::s3::versions_before(items, at),
        None => items,
    };
    let mut out = Vec::new();
    let mut index = 0;
    while index < items.len() {
        if items[index].is_prefix {
            out.push((items[index].clone(), 1));
            index += 1;
            continue;
        }
        let end = index
            + items[index..]
                .iter()
                .take_while(|item| !item.is_prefix && item.key == items[index].key)
                .count();
        let mut group = items[index..end].to_vec();
        if opts.rewind.is_some() {
            group.sort_by_key(|item| std::cmp::Reverse(item.last_modified));
        }
        let count = group.len();
        for (position, item) in group.into_iter().enumerate() {
            if !opts.versions {
                if !item.is_delete_marker {
                    out.push((item, count));
                }
                break;
            }
            out.push((item, count - position));
        }
        index = end;
    }
    out
}

/// Listed S3 entry; `bucket` prefixes keys of alias-root recursive listings.
fn content(bucket: &str, prefix_path: &str, item: ObjectInfo, ordinal: usize) -> Content {
    let key = item.key.strip_prefix(prefix_path).unwrap_or(&item.key);
    let key = if bucket.is_empty() {
        key.to_string()
    } else {
        format!("{bucket}/{key}")
    };
    if item.is_prefix {
        // mc reports common prefixes with the current time.
        return Content::folder(key, SystemTime::now(), 0);
    }
    let storage_class = match item.storage_class {
        Some(class) => class,
        // MinIO reports delete markers as STANDARD; the SDK drops the field.
        None if item.is_delete_marker => "STANDARD".to_string(),
        None => String::new(),
    };
    Content {
        // mc reports folder-marker objects (`dir/`) as folders.
        is_dir: key.ends_with('/'),
        key,
        time: item.last_modified.unwrap_or(UNIX_EPOCH),
        size: item.size,
        etag: item.etag.unwrap_or_default().trim_matches('"').to_string(),
        storage_class,
        version_id: item.version_id.unwrap_or_default(),
        is_delete_marker: item.is_delete_marker,
        ordinal,
    }
}

// ---------------------------------------------------------------------------
// local listing (mc fsClient)
// ---------------------------------------------------------------------------

/// Local folder listing like mc: a folder lists its entries, a file itself, anything else
/// the entries of its parent folder starting with the path; `recursive` walks files only.
fn list_local(input: &str, opts: &ListOpts) -> Result<Listing> {
    let abs = crate::error::abs_path(input);
    let is_dir = input.ends_with('/') || Path::new(&abs).is_dir();
    let fpath = if is_dir && abs != "/" {
        format!("{abs}/")
    } else {
        abs
    };
    let prefix_path = fpath[..fpath.rfind('/').map_or(0, |index| index + 1)].to_string();
    let mut found = Vec::new();
    let mut denied = Vec::new();
    if opts.recursive {
        let (dir, file_prefix) = if fpath.ends_with('/') {
            (fpath.clone(), String::new())
        } else {
            (prefix_path.clone(), fpath.clone())
        };
        walk(
            &dir,
            &file_prefix,
            true,
            &mut found,
            &mut denied,
            &read_dir_entries,
        )?;
    } else if fpath.ends_with('/') {
        let dir = if fpath == "/" {
            "/"
        } else {
            fpath.trim_end_matches('/')
        };
        for (path, meta) in read_dir_sorted(dir)? {
            found.push((path, meta));
        }
    } else if let Ok(meta) = std::fs::metadata(&fpath) {
        found.push((fpath.clone(), meta));
    } else {
        let dir = if prefix_path == "/" {
            "/"
        } else {
            prefix_path.trim_end_matches('/')
        };
        for (path, meta) in read_dir_sorted(dir)? {
            if path.starts_with(&fpath) {
                found.push((path, meta));
            }
        }
    }
    let contents = found
        .into_iter()
        .map(|(path, meta)| {
            let mut key = path.strip_prefix(&prefix_path).unwrap_or(&path).to_string();
            if meta.is_dir() {
                key.push('/');
            }
            Content {
                time: meta.modified().unwrap_or(UNIX_EPOCH),
                size: meta.len() as i64,
                is_dir: meta.is_dir(),
                ..Content::folder(key, UNIX_EPOCH, 0)
            }
        })
        .collect();
    Ok(Listing {
        url: fpath,
        contents,
        denied,
    })
}

pub(crate) type DirEntries = Vec<(String, std::fs::Metadata)>;

/// [`read_dir_entries`] with mc's errors for a plain (non-recursive) folder listing.
fn read_dir_sorted(dir: &str) -> Result<DirEntries> {
    read_dir_entries(dir).map_err(|error| match error.kind() {
        // mc reports the raw `open` error here (`PathInsufficientPermission` only when walking).
        std::io::ErrorKind::PermissionDenied => anyhow::Error::new(crate::error::McError::new(
            format!("open {}/: permission denied", dir.trim_end_matches('/')),
        )),
        _ => crate::error::io_error(&error, dir).into(),
    })
}

/// Regular files and folders of `dir` (symlinks followed, broken ones skipped) as absolute
/// paths, in mc's lexical order (folders compare with a trailing `/`).
pub(crate) fn read_dir_entries(dir: &str) -> std::io::Result<DirEntries> {
    let entries = std::fs::read_dir(dir)?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "lost+found" {
            continue;
        }
        let path = if dir.ends_with('/') {
            format!("{dir}{name}")
        } else {
            format!("{dir}/{name}")
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_file() || meta.is_dir() {
            out.push((path, meta));
        }
    }
    out.sort_by_cached_key(|(path, meta)| {
        if meta.is_dir() {
            format!("{path}/")
        } else {
            path.clone()
        }
    });
    Ok(out)
}

/// mc `listRecursiveInRoutine`: regular files below `dir` whose path starts with
/// `file_prefix` (when set). Symlinked folders are not followed. Unreadable folders are
/// collected in `denied` and skipped, like mc (which keeps walking); other errors abort.
pub(crate) fn walk(
    dir: &str,
    file_prefix: &str,
    root: bool,
    out: &mut DirEntries,
    denied: &mut Vec<Denied>,
    read_dir: &dyn Fn(&str) -> std::io::Result<DirEntries>,
) -> Result<()> {
    let entries = match read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            denied.push(Denied {
                path: crate::error::abs_path(dir),
                root,
            });
            return Ok(());
        }
        Err(error) => return Err(crate::error::io_error(&error, dir).into()),
    };
    for (path, meta) in entries {
        let linked_dir = meta.is_dir()
            && std::fs::symlink_metadata(&path).is_ok_and(|link| link.file_type().is_symlink());
        if !file_prefix.is_empty() && !path.starts_with(file_prefix) {
            if meta.is_dir() && !linked_dir && file_prefix.starts_with(&path) {
                walk(&path, file_prefix, false, out, denied, read_dir)?;
            }
            continue;
        }
        if meta.is_dir() {
            if !linked_dir {
                walk(&path, file_prefix, false, out, denied, read_dir)?;
            }
        } else {
            out.push((path, meta));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// output
// ---------------------------------------------------------------------------

/// Prints a listing like mc `doList` (entries outside the `--storage-class` filter are
/// skipped) and returns (entries, total size) for `--summarize`.
pub(crate) fn print_listing(
    listing: &Listing,
    filter: Option<&str>,
    json: bool,
) -> Result<(i64, i64)> {
    let filter = filter.unwrap_or_default();
    let (mut objects, mut size) = (0, 0);
    for content in &listing.contents {
        if !content.storage_class.is_empty()
            && !filter.is_empty()
            && filter != "*"
            && content.storage_class != filter
        {
            continue;
        }
        objects += 1;
        size += content.size;
        if json {
            crate::output::print_json(&ContentMessage::new(content, &listing.url))?;
        } else {
            println!("{}", content_line(content));
        }
    }
    Ok((objects, size))
}

/// mc `contentMessage.String()`: `[DATE]   SIZE [CLASS] [VERSION vN PUT|DEL] KEY`.
pub(crate) fn content_line(content: &Content) -> String {
    let size = human_bytes(content.size.max(0) as u64).replace(' ', "");
    let mut line = format!("[{}]{size:>7}", print_date(content.time));
    if !content.storage_class.is_empty() {
        line.push(' ');
        line.push_str(&content.storage_class);
    }
    if !content.version_id.is_empty() {
        let op = if content.is_delete_marker {
            "DEL"
        } else {
            "PUT"
        };
        line.push_str(&format!(
            " {} v{} {op}",
            content.version_id, content.ordinal
        ));
    }
    line.push(' ');
    line.push_str(&content.key);
    line
}

fn print_summary(objects: i64, size: i64, json: bool) -> Result<()> {
    if !json {
        println!(
            "\nTotal Size: {}\nTotal Objects: {objects}",
            human_bytes(size.max(0) as u64)
        );
    } else if crate::output::stdout_is_terminal() {
        // mc marshals the summary with an empty indent.
        println!("{{\n\"totalObjects\": {objects},\n\"totalSize\": {size}\n}}");
    } else {
        crate::output::print_json(&serde_json::json!({
            "totalObjects": objects,
            "totalSize": size,
        }))?;
    }
    Ok(())
}

/// Go `time.Time` JSON encoding (RFC 3339 with trimmed nanoseconds), in UTC.
pub(crate) fn go_time(time: SystemTime) -> String {
    let (secs, nanos) = match time.duration_since(UNIX_EPOCH) {
        Ok(value) => (value.as_secs() as i64, value.subsec_nanos()),
        Err(_) => (0, 0),
    };
    let text = DateTime::from_secs(secs)
        .fmt(DateTimeFormat::DateTime)
        .unwrap_or_default();
    let base = text.trim_end_matches('Z');
    if nanos == 0 {
        return format!("{base}Z");
    }
    let fraction = format!("{nanos:09}");
    format!("{base}.{}Z", fraction.trim_end_matches('0'))
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// mc `contentMessage` JSON.
#[derive(Debug, Serialize)]
struct ContentMessage<'a> {
    status: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "lastModified")]
    last_modified: String,
    size: i64,
    key: &'a str,
    etag: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    url: &'a str,
    #[serde(rename = "versionId", skip_serializing_if = "str::is_empty")]
    version_id: &'a str,
    #[serde(rename = "versionOrdinal", skip_serializing_if = "is_zero")]
    version_ordinal: usize,
    #[serde(rename = "isDeleteMarker", skip_serializing_if = "std::ops::Not::not")]
    is_delete_marker: bool,
    #[serde(rename = "storageClass", skip_serializing_if = "str::is_empty")]
    storage_class: &'a str,
}

impl<'a> ContentMessage<'a> {
    fn new(content: &'a Content, url: &'a str) -> Self {
        Self {
            status: "success",
            kind: if content.is_dir { "folder" } else { "file" },
            last_modified: go_time(content.time),
            size: content.size,
            key: &content.key,
            etag: &content.etag,
            url,
            version_id: &content.version_id,
            version_ordinal: content.ordinal,
            is_delete_marker: content.is_delete_marker,
            storage_class: &content.storage_class,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[cfg(not(windows))] // Windows paths (`C:\`)
    #[test]
    fn walk_skips_unreadable_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap().to_string();
        for sub in ["a", "bad", "c/d"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        std::fs::write(dir.path().join("a/f1"), "1").unwrap();
        std::fs::write(dir.path().join("bad/x"), "1").unwrap();
        std::fs::write(dir.path().join("c/d/f2"), "1").unwrap();
        let bad = format!("{root}/bad");
        let read = |path: &str| {
            if path == bad {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            } else {
                read_dir_entries(path)
            }
        };
        let (mut out, mut denied) = (Vec::new(), Vec::new());
        walk(&root, "", true, &mut out, &mut denied, &read).unwrap();
        let files: Vec<_> = out.iter().map(|(path, _)| path.clone()).collect();
        assert_eq!(files, [format!("{root}/a/f1"), format!("{root}/c/d/f2")]);
        assert_eq!(
            denied,
            [Denied {
                path: bad.clone(),
                root: false
            }]
        );
        // The walk root itself.
        let (mut out, mut denied) = (Vec::new(), Vec::new());
        walk(&bad, "", true, &mut out, &mut denied, &read).unwrap();
        assert!(out.is_empty());
        assert_eq!(
            denied,
            [Denied {
                path: bad,
                root: true
            }]
        );
        // Other errors still abort.
        let broken = |_: &str| Err(std::io::Error::other("boom"));
        assert!(walk(&root, "", true, &mut out, &mut denied, &broken).is_err());
    }

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
            version("a", 30, "a3", false),
            version("a", 20, "a2", false),
            version("a", 10, "a1", false),
            version("b", 25, "b2", true),
            version("b", 5, "b1", false),
            ObjectInfo {
                key: "dir/".into(),
                is_prefix: true,
                ..Default::default()
            },
        ]
    }

    fn rows(items: &[(ObjectInfo, usize)]) -> Vec<String> {
        items
            .iter()
            .map(|(item, ordinal)| {
                format!(
                    "{} {} v{ordinal}",
                    item.key,
                    item.version_id.as_deref().unwrap_or("-")
                )
            })
            .collect()
    }

    fn opts(versions: bool, rewind: Option<u64>) -> ListOpts {
        ListOpts {
            versions,
            rewind: rewind.map(|secs| UNIX_EPOCH + Duration::from_secs(secs)),
            ..Default::default()
        }
    }

    #[test]
    fn numbers_versions_newest_highest() {
        assert_eq!(
            rows(&select(sample(), &opts(true, None))),
            [
                "a a3 v3",
                "a a2 v2",
                "a a1 v1",
                "b b2 v2",
                "b b1 v1",
                "dir/ - v1"
            ]
        );
    }

    #[test]
    fn rewind_shows_state_at_time() {
        assert_eq!(
            rows(&select(sample(), &opts(false, Some(22)))),
            ["a a2 v2", "b b1 v1", "dir/ - v1"]
        );
        assert_eq!(
            rows(&select(sample(), &opts(false, Some(26)))),
            ["a a2 v2", "dir/ - v1"]
        );
        assert_eq!(
            rows(&select(sample(), &opts(true, Some(22)))),
            ["a a2 v2", "a a1 v1", "b b1 v1", "dir/ - v1"]
        );
    }

    fn entry(key: &str, size: i64) -> Content {
        Content {
            time: UNIX_EPOCH + Duration::from_millis(1_704_164_645_250),
            size,
            is_dir: false,
            ..Content::folder(key.into(), UNIX_EPOCH, 0)
        }
    }

    #[test]
    fn content_lines_match_mc() {
        let mut file = entry("dir/a.txt", 2048);
        file.storage_class = "STANDARD".into();
        assert_eq!(
            content_line(&file),
            "[2024-01-02 03:04:05 UTC] 2.0KiB STANDARD dir/a.txt"
        );
        file.version_id = "v-1".into();
        file.ordinal = 2;
        file.is_delete_marker = true;
        assert_eq!(
            content_line(&file),
            "[2024-01-02 03:04:05 UTC] 2.0KiB STANDARD v-1 v2 DEL dir/a.txt"
        );
        let folder = Content::folder("sub/".into(), UNIX_EPOCH, 0);
        assert_eq!(
            content_line(&folder),
            "[1970-01-01 00:00:00 UTC]     0B sub/"
        );
    }

    #[test]
    fn json_matches_mc_field_order() {
        let mut file = entry("a.txt", 6);
        file.etag = "abc".into();
        let json = serde_json::to_string(&ContentMessage::new(&file, "http://h/b/")).unwrap();
        assert_eq!(
            json,
            r#"{"status":"success","type":"file","lastModified":"2024-01-02T03:04:05.25Z","size":6,"key":"a.txt","etag":"abc","url":"http://h/b/","versionOrdinal":1}"#
        );
    }

    #[cfg(not(windows))] // Windows file times have 100 ns resolution
    #[test]
    fn go_time_trims_fraction() {
        assert_eq!(go_time(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            go_time(UNIX_EPOCH + Duration::new(1, 120_000_000)),
            "1970-01-01T00:00:01.12Z"
        );
        assert_eq!(
            go_time(UNIX_EPOCH + Duration::new(1, 5)),
            "1970-01-01T00:00:01.000000005Z"
        );
    }

    #[cfg(not(windows))] // Windows paths (`C:\`, trailing-dot cleanup)
    #[test]
    fn lists_local_folders_like_mc() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(dir.path().join("d/sub")).unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("d.txt"), "d").unwrap();
        std::fs::write(dir.path().join("d/b.txt"), "bb").unwrap();
        std::fs::write(dir.path().join("d/sub/c.txt"), "ccc").unwrap();
        let keys = |input: &str, recursive: bool| {
            let listing = list_local(
                input,
                &ListOpts {
                    recursive,
                    ..Default::default()
                },
            )
            .unwrap();
            listing
                .contents
                .iter()
                .map(|c| c.key.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&root, false), ["a.txt", "d.txt", "d/"]);
        assert_eq!(keys(&format!("{root}/d"), false), ["b.txt", "sub/"]);
        assert_eq!(keys(&format!("{root}/d."), false), ["d.txt"]);
        assert_eq!(
            keys(&root, true),
            ["a.txt", "d.txt", "d/b.txt", "d/sub/c.txt"]
        );
        assert_eq!(keys(&format!("{root}/d/su"), true), ["sub/c.txt"]);
        assert!(list_local(&format!("{root}/nope/x"), &ListOpts::default()).is_err());
        assert!(
            list_local(&format!("{root}/nope"), &ListOpts::default())
                .unwrap()
                .contents
                .is_empty()
        );
    }
}
