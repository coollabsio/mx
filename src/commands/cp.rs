//! `mx cp` (and the shared copy session used by `mx mv`).
//!
//! Planning follows mc's copy URL types:
//! * A: `cp file file`, B: `cp file dir/` -> `dir/file`,
//! * C: `cp -r dir target` -> `target/dir/...` (`dir/` copies the contents),
//! * D: `cp src1 src2 ... dir/` -> C for every source.
//!
//! The planned tasks run on a bounded worker pool (`--max-workers`).

use crate::commands::alias_config;
use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use crate::error::{McError, nonfatal};
use crate::flags::{
    ChecksumAlgo, ChecksumFlag, EncFlags, MetadataFlags, RewindFlag, Sse, TimeFilterFlags,
    VersionIdFlag, resolve_sse,
};
use crate::location::{Location, parse_location};
use crate::progress::{Progress, ProgressReader};
use crate::s3::S3ResultExt;
use crate::s3::{GetOptions, ObjectRef, PutOptions};
use anyhow::{Context, Result, anyhow, bail};
use aws_sdk_s3::Client;
use clap::Args;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

const DEFAULT_WORKERS: usize = 4;

#[derive(Debug, Default, Args)]
#[command(mut_args(|a| match a.get_id().as_str() {
    "older_than" => a.help("copy objects older than value in duration string (e.g. 7d10h31s)"),
    "newer_than" => a.help("copy objects newer than value in duration string (e.g. 7d10h31s)"),
    "version_id" => a.help("select an object version to copy"),
    _ => a,
}))]
pub struct CopyArgs {
    #[command(flatten)]
    pub rewind: RewindFlag,
    #[command(flatten)]
    pub version: VersionIdFlag,
    /// copy recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub time: TimeFilterFlags,
    #[command(flatten)]
    pub metadata: MetadataFlags,
    /// preserve filesystem attributes (mode, ownership, timestamps)
    #[arg(short = 'a', long)]
    pub preserve: bool,
    /// disable multipart upload feature
    #[arg(long)]
    pub disable_multipart: bool,
    /// retention mode to be applied on the object (governance, compliance)
    #[arg(long, value_name = "MODE")]
    pub retention_mode: Option<String>,
    /// retention duration for the object in d days or y years
    #[arg(long, value_name = "DURATION")]
    pub retention_duration: Option<String>,
    /// apply legal hold to the copied object (on, off)
    #[arg(long, value_name = "on|off")]
    pub legal_hold: Option<String>,
    /// Extract from remote zip file (MinIO server source only)
    #[arg(long, help = "Extract from remote zip file (MinIO server source only)")]
    pub zip: bool,
    /// maximum number of concurrent copies (default: autodetect) (default: 0)
    #[arg(long, value_name = "N")]
    pub max_workers: Option<usize>,
    #[command(flatten)]
    pub checksum: ChecksumFlag,
    #[command(flatten)]
    pub enc: EncFlags,
    /// SOURCE [SOURCE...] TARGET
    #[arg(required = true, num_args = 2.., value_name = "PATH")]
    pub paths: Vec<String>,
}

pub fn run(args: CopyArgs, json: bool) -> Result<()> {
    let now = SystemTime::now();
    let (legal_hold, retention) = lock_options(&args, now)?;
    let options = CopyOptions {
        recursive: args.recursive,
        time: args.time.clone(),
        rewind: args.rewind.at(now)?,
        version_id: args.version.version_id.clone(),
        metadata: args.metadata.clone(),
        preserve: args.preserve,
        disable_multipart: args.disable_multipart,
        checksum: args.checksum.checksum,
        enc: args.enc.entries()?,
        legal_hold,
        retention,
        zip: args.zip,
        max_workers: args.max_workers.unwrap_or(DEFAULT_WORKERS),
        is_move: false,
    };
    run_session(&args.paths, options, json)
}

/// Settings shared by `cp` and `mv`.
#[derive(Debug, Clone, Default)]
pub(crate) struct CopyOptions {
    pub recursive: bool,
    pub time: TimeFilterFlags,
    pub rewind: Option<SystemTime>,
    pub version_id: Option<String>,
    pub metadata: MetadataFlags,
    pub preserve: bool,
    pub disable_multipart: bool,
    pub checksum: Option<ChecksumAlgo>,
    pub enc: Vec<(String, Sse)>,
    pub legal_hold: Option<bool>,
    pub retention: Option<(String, SystemTime)>,
    pub zip: bool,
    pub max_workers: usize,
    pub is_move: bool,
}

/// `(legal hold, (retention mode, retain until))`.
type LockOptions = (Option<bool>, Option<(String, SystemTime)>);

fn lock_options(args: &CopyArgs, now: SystemTime) -> Result<LockOptions> {
    let legal_hold = match args.legal_hold.as_deref().map(str::to_ascii_lowercase) {
        None => None,
        Some(value) if value == "on" => Some(true),
        Some(value) if value == "off" => Some(false),
        Some(value) => bail!("invalid legal hold `{value}`: use `on` or `off`"),
    };
    let retention = match (&args.retention_mode, &args.retention_duration) {
        (None, None) => None,
        (Some(mode), Some(duration)) => {
            let mode = mode.to_ascii_uppercase();
            if mode != "GOVERNANCE" && mode != "COMPLIANCE" {
                bail!("invalid retention mode `{mode}`: use `governance` or `compliance`");
            }
            Some((mode, retention_until(duration, now)?))
        }
        _ => bail!(
            "Both object retention flags `--retention-mode` and `--retention-duration` are required."
        ),
    };
    Ok((legal_hold, retention))
}

/// mc `--retention-duration`: `<N>d` (days) or `<N>y` (calendar years) from `now`.
fn retention_until(validity: &str, now: SystemTime) -> Result<SystemTime> {
    let (count, unit) = crate::flags::parse_validity(validity)
        .map_err(|_| anyhow!("invalid retention duration `{validity}`: use e.g. `30d` or `1y`"))?;
    crate::flags::retain_until(now, count, unit)
}

// ---------------------------------------------------------------------------
// endpoints and planning
// ---------------------------------------------------------------------------

/// A parsed command-line path.
#[derive(Debug, Clone)]
enum Endpoint {
    Local {
        raw: String,
        path: PathBuf,
    },
    S3 {
        raw: String,
        alias: String,
        bucket: String,
        /// Object key or prefix; empty for the bucket root. Keeps a trailing `/`.
        key: String,
    },
}

impl Endpoint {
    fn parse(input: &str, store: &ConfigStore) -> Result<Self> {
        Ok(match parse_location(input, store.config()) {
            Location::Local(path) => Endpoint::Local {
                raw: input.replace('\\', "/"),
                path,
            },
            Location::S3(target) => {
                let bucket = target
                    .bucket
                    .clone()
                    .filter(|bucket| !bucket.is_empty())
                    .ok_or_else(|| anyhow!("`{input}` does not contain bucket name."))?;
                let mut key = target.key.clone().unwrap_or_default();
                if target.trailing_slash && !key.is_empty() && !key.ends_with('/') {
                    key.push('/');
                }
                Endpoint::S3 {
                    raw: input.to_string(),
                    alias: target.alias,
                    bucket,
                    key,
                }
            }
        })
    }

    fn raw(&self) -> &str {
        match self {
            Endpoint::Local { raw, .. } | Endpoint::S3 { raw, .. } => raw,
        }
    }

    fn is_s3(&self) -> bool {
        matches!(self, Endpoint::S3 { .. })
    }

    /// The path mc uses to derive recursive target names (`bucket/key` for S3).
    fn rule_path(&self) -> String {
        match self {
            Endpoint::Local { raw, .. } => raw.clone(),
            Endpoint::S3 {
                raw, bucket, key, ..
            } => {
                if key.is_empty() && raw.trim_end().ends_with('/') {
                    format!("{bucket}/")
                } else if key.is_empty() {
                    bucket.clone()
                } else {
                    format!("{bucket}/{key}")
                }
            }
        }
    }
}

/// One side of a planned copy.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Item {
    Local(PathBuf),
    S3 {
        alias: String,
        bucket: String,
        key: String,
    },
}

impl Item {
    fn display(&self) -> String {
        match self {
            Item::Local(path) => path.display().to_string(),
            Item::S3 { alias, bucket, key } => format!("{alias}/{bucket}/{key}"),
        }
    }
}

#[derive(Debug, Clone)]
struct CopyTask {
    source: Item,
    target: Item,
    size: u64,
    modified: Option<SystemTime>,
    version_id: Option<String>,
}

#[derive(Debug, Default)]
struct Plan {
    tasks: Vec<CopyTask>,
    /// Local source folders `(path, remove_root)`, cleaned up when empty after `mv`.
    local_dirs: Vec<(PathBuf, bool)>,
}

/// Target-relative name for `full` when copying `source` recursively (mc type C):
/// everything after the last `/` of `source` is kept, so `dir` -> `dir/...` and `dir/` ->
/// `...`. Empty, `.` and `..` components are dropped.
fn relative_suffix(source: &str, full: &str) -> String {
    let parent = source.rfind('/').map(|i| &source[..=i]).unwrap_or("");
    full.strip_prefix(parent)
        .unwrap_or(full)
        .split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect::<Vec<_>>()
        .join("/")
}

/// `target` treated as a folder, joined with `suffix`.
fn join_target(target: &Endpoint, suffix: &str) -> Item {
    match target {
        Endpoint::Local { path, .. } => Item::Local(path.join(suffix)),
        Endpoint::S3 {
            alias, bucket, key, ..
        } => {
            let prefix = key.trim_end_matches('/');
            Item::S3 {
                alias: alias.clone(),
                bucket: bucket.clone(),
                key: if prefix.is_empty() {
                    suffix.to_string()
                } else {
                    format!("{prefix}/{suffix}")
                },
            }
        }
    }
}

/// mc `isURLPrefix`: `/`-separated components match up to the shorter one (a trailing `/`
/// on either side matches anything).
fn is_url_prefix(source: &str, target: &str) -> bool {
    let src: Vec<&str> = source.split('/').collect();
    let dst: Vec<&str> = target.split('/').collect();
    let min = src.len().min(dst.len());
    (0..min).all(|i| (i == min - 1 && (src[i].is_empty() || dst[i].is_empty())) || src[i] == dst[i])
}

/// mc `isURLContains`: `target` lies inside `source`.
fn is_url_contains(source: &str, target: &str) -> bool {
    let with_slash = |s: &str| {
        if s.ends_with('/') {
            s.to_string()
        } else {
            format!("{s}/")
        }
    };
    with_slash(target).starts_with(&with_slash(source))
}

fn requires_recursive(raw: &str) -> anyhow::Error {
    anyhow!("To copy or move `{raw}` the --recursive flag is required.")
}

/// Per-alias clients built once per session.
struct Clients(HashMap<String, (AliasConfig, Client)>);

impl Clients {
    async fn new(store: &ConfigStore, endpoints: &[&Endpoint]) -> Result<Self> {
        let mut clients = HashMap::new();
        for endpoint in endpoints {
            if let Endpoint::S3 { alias, .. } = endpoint
                && !clients.contains_key(alias)
            {
                let config = alias_config(store, alias)?;
                let client = crate::s3::build_client(&config).await?;
                clients.insert(alias.clone(), (config, client));
            }
        }
        Ok(Self(clients))
    }

    fn get(&self, alias: &str) -> &(AliasConfig, Client) {
        &self.0[alias]
    }

    fn client(&self, alias: &str) -> &Client {
        &self.get(alias).1
    }
}

/// Validates flag combinations that do not need the network.
fn validate(sources: &[Endpoint], target: &Endpoint, options: &CopyOptions) -> Result<()> {
    if options.version_id.is_some() {
        if sources.len() > 1 {
            bail!("Unable to pass --version-id flag with multiple copy sources arguments.");
        }
        if options.recursive {
            bail!("`--version-id` cannot be used with `--recursive`.");
        }
        if options.rewind.is_some() {
            bail!("`--version-id` cannot be used with `--rewind`.");
        }
    }
    if options.zip && options.rewind.is_some() {
        bail!("--zip and --rewind cannot be used together");
    }
    for source in sources {
        if !source.is_s3() {
            for (set, flag) in [
                (options.version_id.is_some(), "--version-id"),
                (options.rewind.is_some(), "--rewind"),
                (options.zip, "--zip"),
            ] {
                if set {
                    bail!(
                        "`{flag}` requires an S3 source; `{}` is local.",
                        source.raw()
                    );
                }
            }
        }
    }
    if !target.is_s3() {
        let meta = &options.metadata;
        for (set, flag) in [
            (meta.attr.is_some(), "--attr"),
            (meta.tags.is_some(), "--tags"),
            (meta.storage_class.is_some(), "--storage-class"),
            (options.checksum.is_some(), "--checksum"),
            (options.disable_multipart, "--disable-multipart"),
            (options.legal_hold.is_some(), "--legal-hold"),
            (options.retention.is_some(), "--retention-mode"),
        ] {
            if set {
                bail!(
                    "`{flag}` requires an S3 target; `{}` is local.",
                    target.raw()
                );
            }
        }
    }
    if options.max_workers == 0 {
        bail!("`--max-workers` must be at least 1.");
    }
    options.time.parsed()?;
    options.metadata.attr_pairs()?;
    options.metadata.tag_pairs()?;
    #[cfg(not(unix))]
    if options.preserve {
        bail!("Permissions are not preserved on this platform.");
    }
    Ok(())
}

async fn plan(
    clients: &Clients,
    sources: &[Endpoint],
    target: &Endpoint,
    options: &CopyOptions,
) -> Result<Plan> {
    let multi = sources.len() > 1;
    if multi && !target_is_dir(clients, target).await? {
        bail!("Target `{}` is not a folder.", target.raw());
    }
    if options.recursive
        && let Endpoint::Local { path, .. } = target
        && path.is_file()
    {
        bail!("Target `{}` is not a folder.", target.raw());
    }

    let mut plan = Plan::default();
    for source in sources {
        if options.recursive || multi {
            expand_source(clients, source, target, options, &mut plan)
                .await
                .context(nonfatal("Unable to prepare URL for copying."))?;
        } else {
            let task = single_source(clients, source, target, options)
                .await
                .context(nonfatal("Unable to prepare URL for copying."))?;
            plan.tasks.push(task);
        }
    }

    check_target_collisions(&plan.tasks)?;
    let mut seen = HashSet::new();
    plan.tasks.retain(|task| seen.insert(task.target.clone()));
    if let Some(task) = plan.tasks.iter().find(|task| task.source == task.target) {
        bail!(
            "Source and target `{}` are the same object.",
            task.source.display()
        );
    }
    if options.time.is_set() {
        let now = SystemTime::now();
        plan.tasks
            .retain(|task| options.time.matches(task.modified, now));
    }
    Ok(plan)
}

/// mc `PathNotFound` for a missing local source (cp reports it without error fields).
fn local_stat_error(error: &std::io::Error, path: &std::path::Path) -> anyhow::Error {
    let mut err = crate::error::io_error(error, &path.to_string_lossy());
    err.detail = Default::default();
    err.into()
}

/// Distinct sources mapped to one target (e.g. keys `a//b` and `a/b`, see [`relative_suffix`])
/// would overwrite each other, and `mv` would then delete both sources.
fn check_target_collisions(tasks: &[CopyTask]) -> Result<()> {
    let mut sources: HashMap<&Item, &Item> = HashMap::new();
    for task in tasks {
        if let Some(previous) = sources.insert(&task.target, &task.source)
            && previous != &task.source
        {
            bail!(
                "Sources `{}` and `{}` both map to target `{}`.",
                previous.display(),
                task.source.display(),
                task.target.display()
            );
        }
    }
    Ok(())
}

/// mc `isAliasURLDir`: an existing folder, a bucket root, or a path ending in `/`.
async fn target_is_dir(clients: &Clients, target: &Endpoint) -> Result<bool> {
    Ok(match target {
        Endpoint::Local { raw, path } => path.is_dir() || raw.ends_with('/'),
        Endpoint::S3 {
            alias, bucket, key, ..
        } => {
            // mc `isAliasURLDir`: a failing lookup (e.g. missing bucket) is not a folder; the
            // copy itself then reports the error.
            key.is_empty()
                || key.ends_with('/')
                || crate::s3::prefix_exists(clients.client(alias), bucket, &format!("{key}/"))
                    .await
                    .unwrap_or(false)
        }
    })
}

fn source_sse_c(options: &CopyOptions, display: &str) -> Option<[u8; 32]> {
    resolve_sse(&options.enc, display).and_then(|sse| sse.customer_key())
}

/// Size and mtime of one S3 object (None if it does not exist).
async fn stat_object(
    client: &Client,
    bucket: &str,
    key: &str,
    options: &GetOptions,
) -> Result<Option<(u64, Option<SystemTime>)>> {
    match crate::s3::head_object_with(client, bucket, key, options).await {
        Ok(head) => Ok(Some((
            head.content_length().unwrap_or(0).max(0) as u64,
            head.last_modified().and_then(crate::s3::to_system_time),
        ))),
        Err(error) if crate::error::error_code(&error) == Some("NoSuchKey") => Ok(None),
        Err(error) => Err(error),
    }
}

/// Resolves a single S3 object source (honoring `--version-id`, `--rewind`, `--zip`).
async fn resolve_s3_object(
    clients: &Clients,
    alias: &str,
    bucket: &str,
    key: &str,
    options: &CopyOptions,
) -> Result<Option<(u64, Option<SystemTime>, Option<String>)>> {
    let client = clients.client(alias);
    if let Some(at) = options.rewind {
        return Ok(crate::s3::object_version_at(client, bucket, key, at)
            .await?
            .map(|info| (info.size.max(0) as u64, info.last_modified, info.version_id)));
    }
    let get = GetOptions {
        version_id: options.version_id.clone(),
        sse_c: source_sse_c(options, &format!("{alias}/{bucket}/{key}")),
        zip_extract: options.zip,
        ..Default::default()
    };
    Ok(stat_object(client, bucket, key, &get)
        .await?
        .map(|(size, modified)| (size, modified, options.version_id.clone())))
}

/// Types A and B: one file to a file or into a folder.
async fn single_source(
    clients: &Clients,
    source: &Endpoint,
    target: &Endpoint,
    options: &CopyOptions,
) -> Result<CopyTask> {
    let (item, size, modified, version_id, name) = match source {
        Endpoint::Local { raw, path } => {
            let meta = std::fs::metadata(path).map_err(|error| local_stat_error(&error, path))?;
            if meta.is_dir() {
                return Err(requires_recursive(raw));
            }
            let name = source_name_from_local(path)?;
            (
                Item::Local(path.clone()),
                meta.len(),
                meta.modified().ok(),
                None,
                name,
            )
        }
        Endpoint::S3 {
            raw,
            alias,
            bucket,
            key,
        } => {
            if key.is_empty() || key.ends_with('/') {
                return Err(requires_recursive(raw));
            }
            let Some((size, modified, version_id)) =
                resolve_s3_object(clients, alias, bucket, key, options).await?
            else {
                if crate::s3::prefix_exists(clients.client(alias), bucket, &format!("{key}/"))
                    .await?
                {
                    return Err(requires_recursive(raw));
                }
                return Err(McError::object_missing().into());
            };
            let item = Item::S3 {
                alias: alias.clone(),
                bucket: bucket.clone(),
                key: key.clone(),
            };
            (item, size, modified, version_id, source_name_from_key(key))
        }
    };
    let target_item = if target_is_dir(clients, target).await? {
        join_target(target, &name)
    } else {
        match target {
            Endpoint::Local { path, .. } => Item::Local(path.clone()),
            Endpoint::S3 {
                alias, bucket, key, ..
            } => Item::S3 {
                alias: alias.clone(),
                bucket: bucket.clone(),
                key: key.clone(),
            },
        }
    };
    Ok(CopyTask {
        source: item,
        target: target_item,
        size,
        modified,
        version_id,
    })
}

/// Types C and D: every file under `source` into the `target` folder.
async fn expand_source(
    clients: &Clients,
    source: &Endpoint,
    target: &Endpoint,
    options: &CopyOptions,
    plan: &mut Plan,
) -> Result<()> {
    let rule = source.rule_path();
    match source {
        Endpoint::Local { raw, path } => {
            let meta = std::fs::metadata(path).map_err(|error| local_stat_error(&error, path))?;
            if !meta.is_dir() {
                plan.tasks.push(CopyTask {
                    source: Item::Local(path.clone()),
                    target: join_target(target, &relative_suffix(&rule, raw)),
                    size: meta.len(),
                    modified: meta.modified().ok(),
                    version_id: None,
                });
                return Ok(());
            }
            if !options.recursive {
                return Err(requires_recursive(raw));
            }
            if let Endpoint::Local { path: target, .. } = target
                && is_url_contains(
                    &std::path::absolute(path)?.to_string_lossy(),
                    &std::path::absolute(target)?.to_string_lossy(),
                )
            {
                bail!("Copying or moving `{raw}` into itself is not allowed.");
            }
            for entry in crate::transfer::local_inventory(path)? {
                let full = if rule.ends_with('/') {
                    format!("{rule}{}", entry.relative)
                } else {
                    format!("{rule}/{}", entry.relative)
                };
                let modified = std::fs::metadata(&entry.path)
                    .ok()
                    .and_then(|m| m.modified().ok());
                plan.tasks.push(CopyTask {
                    source: Item::Local(entry.path),
                    target: join_target(target, &relative_suffix(&rule, &full)),
                    size: entry.size,
                    modified,
                    version_id: None,
                });
            }
            plan.local_dirs.push((path.clone(), !raw.ends_with('/')));
        }
        Endpoint::S3 {
            raw,
            alias,
            bucket,
            key,
        } => {
            let is_folder = key.is_empty() || key.ends_with('/');
            let single = |size, modified, version_id| CopyTask {
                source: Item::S3 {
                    alias: alias.clone(),
                    bucket: bucket.clone(),
                    key: key.clone(),
                },
                target: join_target(target, &source_name_from_key(key)),
                size,
                modified,
                version_id,
            };
            if !options.recursive {
                // Multiple sources without -r: each must be a single object.
                if !is_folder
                    && let Some((size, modified, version_id)) =
                        resolve_s3_object(clients, alias, bucket, key, options).await?
                {
                    plan.tasks.push(single(size, modified, version_id));
                    return Ok(());
                }
                if is_folder
                    || crate::s3::prefix_exists(clients.client(alias), bucket, &format!("{key}/"))
                        .await?
                {
                    return Err(requires_recursive(raw));
                }
                return Err(McError::object_missing().into());
            }

            if let Endpoint::S3 {
                alias: target_alias,
                bucket: target_bucket,
                key: target_key,
                ..
            } = target
                && target_alias == alias
                && target_bucket == bucket
                && is_url_contains(&format!("/{key}"), &format!("/{target_key}"))
                && (is_folder
                    || crate::s3::prefix_exists(clients.client(alias), bucket, &format!("{key}/"))
                        .await?)
            {
                bail!("Copying or moving `{raw}` into itself is not allowed.");
            }

            let client = clients.client(alias);
            let objects = if options.zip {
                let prefix = if is_folder {
                    key.clone()
                } else {
                    format!("{key}/")
                };
                crate::s3::list_zip_objects(client, bucket, &prefix).await?
            } else {
                let prefix = (!key.is_empty()).then_some(key.as_str());
                crate::s3::list_objects_with(
                    client,
                    bucket,
                    prefix,
                    &crate::s3::ListOptions {
                        recursive: true,
                        rewind: options.rewind,
                        ..Default::default()
                    },
                )
                .await?
                .into_iter()
                .map(|mut info| {
                    info.key = crate::s3::full_key(key, &info.key);
                    info
                })
                .collect()
            };
            let before = plan.tasks.len();
            for info in objects {
                if info.is_prefix || info.is_delete_marker || info.key.ends_with('/') {
                    continue;
                }
                let full = format!("{bucket}/{}", info.key);
                plan.tasks.push(CopyTask {
                    target: join_target(target, &relative_suffix(&rule, &full)),
                    source: Item::S3 {
                        alias: alias.clone(),
                        bucket: bucket.clone(),
                        key: info.key,
                    },
                    size: info.size.max(0) as u64,
                    modified: info.last_modified,
                    version_id: info.version_id.filter(|_| options.rewind.is_some()),
                });
            }
            if plan.tasks.len() == before && !is_folder {
                // `cp -r alias/bucket/file` copies the single object.
                let Some((size, modified, version_id)) =
                    resolve_s3_object(clients, alias, bucket, key, options).await?
                else {
                    return Err(McError::object_missing().into());
                };
                plan.tasks.push(single(size, modified, version_id));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// execution
// ---------------------------------------------------------------------------

/// mc `copyMessage`.
#[derive(Debug, Serialize)]
struct CopyMessage<'a> {
    status: &'static str,
    source: &'a str,
    target: &'a str,
    size: u64,
    #[serde(rename = "totalCount")]
    total_count: u64,
    #[serde(rename = "totalSize")]
    total_size: u64,
}

struct Session {
    clients: Clients,
    options: CopyOptions,
    progress: Arc<Progress>,
    json: bool,
    total_count: u64,
    total_size: u64,
}

impl Session {
    fn put_options(&self, target: &str) -> Result<PutOptions> {
        let options = &self.options;
        Ok(PutOptions {
            metadata: options.metadata.attr_pairs()?,
            tags: options.metadata.tag_pairs().unwrap_or_default(),
            storage_class: options.metadata.storage_class.clone(),
            sse: resolve_sse(&options.enc, target),
            checksum: options.checksum,
            disable_multipart: options.disable_multipart,
            legal_hold: options.legal_hold,
            retention: options.retention.clone(),
            ..Default::default()
        })
    }

    fn get_options(&self, task: &CopyTask) -> GetOptions {
        GetOptions {
            version_id: task.version_id.clone(),
            sse_c: source_sse_c(&self.options, &task.source.display()),
            zip_extract: self.options.zip,
            ..Default::default()
        }
    }

    fn announce(&self, task: &CopyTask) -> Result<()> {
        let source = task.source.display();
        if self.progress.is_bar() {
            self.progress.set_caption(&format!("{source}:"));
            return Ok(());
        }
        let target = task.target.display();
        if self.json {
            crate::output::print_json(&CopyMessage {
                status: "success",
                source: &source,
                target: &target,
                size: task.size,
                total_count: self.total_count,
                total_size: self.total_size,
            })?;
        } else {
            println!("`{source}` -> `{target}`");
        }
        Ok(())
    }

    async fn copy(&self, task: &CopyTask) -> Result<()> {
        self.announce(task)?;
        match (&task.source, &task.target) {
            (Item::Local(path), Item::S3 { alias, bucket, key }) => {
                self.upload(task, path, alias, bucket, key).await?
            }
            (Item::S3 { alias, bucket, key }, Item::Local(path)) => {
                self.download(task, alias, bucket, key, path).await?
            }
            (Item::S3 { .. }, Item::S3 { .. }) => self.copy_s3(task).await?,
            (Item::Local(source), Item::Local(target)) => self.copy_local(source, target).await?,
        }
        if self.options.is_move {
            match &task.source {
                Item::Local(path) => std::fs::remove_file(path)
                    .with_context(|| format!("Unable to remove `{}`.", path.display()))?,
                Item::S3 { alias, bucket, key } => {
                    self.clients
                        .client(alias)
                        .delete_object()
                        .bucket(bucket)
                        .key(key)
                        .send()
                        .await
                        .s3(bucket, key)?;
                }
            }
        }
        Ok(())
    }

    async fn upload(
        &self,
        task: &CopyTask,
        path: &Path,
        alias: &str,
        bucket: &str,
        key: &str,
    ) -> Result<()> {
        let mut put = self.put_options(&task.target.display())?;
        if self.options.preserve {
            put.metadata.insert(
                0,
                (
                    crate::transfer::ATTRS_METADATA_KEY.to_string(),
                    crate::transfer::file_attrs(path)?,
                ),
            );
        }
        let file = tokio::fs::File::open(path)
            .await
            .with_context(|| format!("Unable to read local file `{}`.", path.display()))?;
        let reader = ProgressReader::new(file, self.progress.clone());
        crate::s3::upload_stream(
            self.clients.client(alias),
            bucket,
            key,
            reader,
            Some(task.size),
            &put,
        )
        .await?;
        Ok(())
    }

    async fn download(
        &self,
        task: &CopyTask,
        alias: &str,
        bucket: &str,
        key: &str,
        path: &Path,
    ) -> Result<()> {
        let response = crate::s3::get_object(
            self.clients.client(alias),
            bucket,
            key,
            &self.get_options(task),
        )
        .await?;
        let attrs = if self.options.preserve {
            response.metadata().and_then(|metadata| {
                metadata
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("mc-attrs"))
                    .map(|(_, value)| crate::transfer::parse_attrs(value))
            })
        } else {
            None
        };
        let reader = ProgressReader::new(response.body.into_async_read(), self.progress.clone());
        write_local(reader, path).await?;
        if let Some(attrs) = attrs {
            crate::transfer::apply_attrs(path, &attrs)?;
        }
        Ok(())
    }

    async fn copy_local(&self, source: &Path, target: &Path) -> Result<()> {
        let file = tokio::fs::File::open(source)
            .await
            .with_context(|| format!("Unable to read local file `{}`.", source.display()))?;
        write_local(ProgressReader::new(file, self.progress.clone()), target).await?;
        if self.options.preserve {
            let attrs = crate::transfer::parse_attrs(&crate::transfer::file_attrs(source)?);
            crate::transfer::apply_attrs(target, &attrs)?;
        }
        Ok(())
    }

    async fn copy_s3(&self, task: &CopyTask) -> Result<()> {
        let (
            Item::S3 {
                alias: src_alias,
                bucket: src_bucket,
                key: src_key,
            },
            Item::S3 {
                alias: dst_alias,
                bucket: dst_bucket,
                key: dst_key,
            },
        ) = (&task.source, &task.target)
        else {
            unreachable!("copy_s3 called with a local item");
        };
        let (src_config, src_client) = self.clients.get(src_alias);
        let (dst_config, dst_client) = self.clients.get(dst_alias);
        let get = self.get_options(task);
        let mut put = self.put_options(&task.target.display())?;

        let server_side = !self.options.zip
            && crate::s3::client::same_endpoint_and_credentials(src_config, dst_config);
        if server_side {
            if !put.metadata.is_empty() {
                // mc merges --attr into the source metadata.
                let head =
                    crate::s3::head_object_with(src_client, src_bucket, src_key, &get).await?;
                put.metadata = crate::s3::merge_metadata(
                    [
                        ("Content-Type", head.content_type()),
                        ("Cache-Control", head.cache_control()),
                        ("Content-Encoding", head.content_encoding()),
                        ("Content-Disposition", head.content_disposition()),
                        ("Content-Language", head.content_language()),
                    ],
                    head.metadata(),
                    &put.metadata,
                );
            }
            crate::s3::server_side_copy_sized(
                dst_client,
                ObjectRef {
                    alias: src_config,
                    bucket: src_bucket,
                    key: src_key,
                },
                ObjectRef {
                    alias: dst_config,
                    bucket: dst_bucket,
                    key: dst_key,
                },
                &get,
                &put,
                task.size,
            )
            .await?;
            self.progress.add(task.size);
            return Ok(());
        }

        let response = crate::s3::get_object(src_client, src_bucket, src_key, &get).await?;
        put.metadata = crate::s3::merge_metadata(
            [
                ("Content-Type", response.content_type()),
                ("Cache-Control", response.cache_control()),
                ("Content-Encoding", response.content_encoding()),
                ("Content-Disposition", response.content_disposition()),
                ("Content-Language", response.content_language()),
            ],
            response.metadata(),
            &put.metadata,
        );
        let size_hint = response
            .content_length()
            .and_then(|length| u64::try_from(length).ok());
        let reader = ProgressReader::new(response.body.into_async_read(), self.progress.clone());
        crate::s3::upload_stream(dst_client, dst_bucket, dst_key, reader, size_hint, &put).await?;
        Ok(())
    }
}

/// Streams `reader` into `path` via `<path>.part.minio`, creating parent folders.
async fn write_local<R: tokio::io::AsyncRead + Unpin>(mut reader: R, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Unable to create folder `{}`.", parent.display()))?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part.minio");
    let part = PathBuf::from(part);
    let result = async {
        let mut file = tokio::fs::File::create(&part)
            .await
            .with_context(|| format!("Unable to write local file `{}`.", path.display()))?;
        tokio::io::copy(&mut reader, &mut file).await?;
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);
        tokio::fs::rename(&part, path)
            .await
            .with_context(|| format!("Unable to write local file `{}`.", path.display()))
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
}

/// Removes empty folders below `root` (and `root` itself when `include_root`).
fn remove_empty_dirs(root: &Path, include_root: bool) {
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_dirs(&entry.path(), true);
            }
        }
    }
    if include_root {
        let _ = std::fs::remove_dir(root);
    }
}

/// Runs a full `cp`/`mv` session: `paths` is `SOURCE... TARGET`.
pub(crate) fn run_session(paths: &[String], options: CopyOptions, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (target, sources) = paths
        .split_last()
        .filter(|(_, sources)| !sources.is_empty())
        .ok_or_else(|| anyhow!("Unable to parse source and target arguments."))?;
    let target = Endpoint::parse(target, &store)?;
    let sources = sources
        .iter()
        .map(|source| Endpoint::parse(source, &store))
        .collect::<Result<Vec<_>>>()?;
    validate(&sources, &target, &options)?;
    if options.is_move
        && let [source] = sources.as_slice()
        && is_url_prefix(source.raw(), target.raw())
    {
        bail!(
            "The source {} and destination {} cannot be subdirectories of each other",
            source.raw(),
            target.raw()
        );
    }

    let runtime = crate::commands::runtime()?;
    runtime.block_on(async {
        let mut endpoints: Vec<&Endpoint> = sources.iter().collect();
        endpoints.push(&target);
        let clients = Clients::new(&store, &endpoints).await?;
        let plan = plan(&clients, &sources, &target, &options).await?;
        let total_size = plan.tasks.iter().map(|task| task.size).sum();
        let bar = !json && !crate::globals::quiet() && std::io::stdout().is_terminal();
        let session = Arc::new(Session {
            clients,
            progress: Progress::new(total_size, bar),
            json,
            total_count: plan.tasks.len() as u64,
            total_size,
            options,
        });
        let failures = execute(session.clone(), plan.tasks.clone()).await;
        let verb = if session.options.is_move {
            "move"
        } else {
            "copy"
        };
        session.progress.finish(failures.is_empty());

        if !failures.is_empty() {
            // mc reports each failure (`errorIf`) and exits with status 1.
            for (source, error) in failures {
                crate::output::print_error(
                    &error.context(nonfatal(format!("Failed to {verb} `{source}`."))),
                );
            }
            return Err(crate::output::Exit(1).into());
        }
        if session.options.is_move {
            for (dir, include_root) in &plan.local_dirs {
                remove_empty_dirs(dir, *include_root);
            }
        }
        if !session.progress.is_bar() {
            let stat = session.progress.stat();
            if json {
                crate::output::print_json(&stat)?;
            } else {
                println!("{}", stat.table());
            }
        }
        Ok(())
    })
}

/// Runs tasks with at most `max_workers` in flight; returns `(source, error)` per failure.
async fn execute(session: Arc<Session>, tasks: Vec<CopyTask>) -> Vec<(String, anyhow::Error)> {
    let mut set = tokio::task::JoinSet::new();
    let mut failures = Vec::new();
    let mut record = |joined: Result<(CopyTask, Result<()>), tokio::task::JoinError>| match joined {
        Ok((_, Ok(()))) => {}
        Ok((task, Err(error))) => failures.push((task.source.display(), error)),
        Err(error) => failures.push((String::from("?"), anyhow!(error))),
    };
    for task in tasks {
        while set.len() >= session.options.max_workers {
            if let Some(joined) = set.join_next().await {
                record(joined);
            }
        }
        let session = session.clone();
        set.spawn(async move {
            let result = session.copy(&task).await;
            (task, result)
        });
    }
    while let Some(joined) = set.join_next().await {
        record(joined);
    }
    failures
}

// ---------------------------------------------------------------------------
// helpers shared with other commands
// ---------------------------------------------------------------------------

pub(crate) fn source_name_from_local(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("Unable to determine file name from `{}`.", path.display()))
}

pub(crate) fn source_name_from_key(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(key)
        .to_string()
}

pub(crate) fn resolve_destination_key(
    target: &crate::target::TargetRef,
    fallback: String,
) -> Result<String> {
    let key = match target.key_with_trailing_slash() {
        Some(key) if key.ends_with('/') => format!("{key}{fallback}"),
        Some(key) => key,
        None => fallback,
    };

    if key.is_empty() {
        bail!("Target is missing object key.");
    }

    Ok(key)
}

pub(crate) fn resolve_local_destination(target: &Path, fallback: String) -> Result<PathBuf> {
    if target.exists() && target.is_dir() {
        return Ok(target.join(fallback));
    }

    let rendered = target.to_string_lossy();
    if rendered.ends_with(std::path::MAIN_SEPARATOR) || rendered.ends_with('/') {
        return Ok(target.join(fallback));
    }

    Ok(target.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn colliding_targets_are_rejected() {
        let target = Endpoint::Local {
            raw: "/tmp/out/".into(),
            path: PathBuf::from("/tmp/out/"),
        };
        let task = |key: &str| CopyTask {
            source: Item::S3 {
                alias: "play".into(),
                bucket: "b".into(),
                key: key.into(),
            },
            target: join_target(&target, &relative_suffix("b/", &format!("b/{key}"))),
            size: 0,
            modified: None,
            version_id: None,
        };
        assert!(check_target_collisions(&[task("a/b"), task("a/c"), task("a/b")]).is_ok());
        let error = check_target_collisions(&[task("a/b"), task("a//b")])
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "Sources `play/b/a/b` and `play/b/a//b` both map to target `/tmp/out/a/b`."
        );
    }

    fn s3(key: &str) -> Endpoint {
        Endpoint::S3 {
            raw: format!("play/b/{key}"),
            alias: "play".into(),
            bucket: "b".into(),
            key: key.into(),
        }
    }

    fn s3_item(key: &str) -> Item {
        Item::S3 {
            alias: "play".into(),
            bucket: "b".into(),
            key: key.into(),
        }
    }

    #[test]
    fn recursive_suffix_follows_trailing_slash_rules() {
        // `dir/` copies the contents, `dir` copies the folder itself.
        assert_eq!(relative_suffix("b/dir/", "b/dir/x/y.txt"), "x/y.txt");
        assert_eq!(relative_suffix("b/dir", "b/dir/x/y.txt"), "dir/x/y.txt");
        assert_eq!(relative_suffix("b", "b/x.txt"), "b/x.txt");
        assert_eq!(relative_suffix("b/", "b/x.txt"), "x.txt");
        assert_eq!(relative_suffix("data", "data/a/b"), "data/a/b");
        assert_eq!(relative_suffix("./data/", "./data/a"), "a");
        assert_eq!(relative_suffix("/tmp/data", "/tmp/data/a"), "data/a");
        assert_eq!(relative_suffix(".", "./a"), "a");
        assert_eq!(relative_suffix("../up/", "../up/a"), "a");
        assert_eq!(relative_suffix("dir/file.txt", "dir/file.txt"), "file.txt");
    }

    #[test]
    fn joins_targets_as_folders() {
        assert_eq!(join_target(&s3(""), "x/y"), s3_item("x/y"));
        assert_eq!(join_target(&s3("pre"), "x"), s3_item("pre/x"));
        assert_eq!(join_target(&s3("pre/"), "x"), s3_item("pre/x"));
        let local = Endpoint::Local {
            raw: "/tmp/out".into(),
            path: "/tmp/out".into(),
        };
        assert_eq!(
            join_target(&local, "a/b"),
            Item::Local(PathBuf::from("/tmp/out/a/b"))
        );
    }

    #[test]
    fn rule_path_keeps_bucket_trailing_slash() {
        let root = Endpoint::S3 {
            raw: "play/b/".into(),
            alias: "play".into(),
            bucket: "b".into(),
            key: String::new(),
        };
        assert_eq!(root.rule_path(), "b/");
        let bare = Endpoint::S3 {
            raw: "play/b".into(),
            alias: "play".into(),
            bucket: "b".into(),
            key: String::new(),
        };
        assert_eq!(bare.rule_path(), "b");
        assert_eq!(s3("dir/").rule_path(), "b/dir/");
    }

    #[test]
    fn detects_url_prefixes_and_self_copies() {
        assert!(is_url_prefix("play/b/dir/a.txt", "play/b/"));
        assert!(is_url_prefix("play/b/x", "play/b/x"));
        assert!(!is_url_prefix("play/b/a.txt", "play/b2/"));
        assert!(!is_url_prefix("/tmp/a.txt", "/tmp/dir/"));
        assert!(is_url_contains("/dir", "/dir/sub/"));
        assert!(is_url_contains("/", "/backup/"));
        assert!(!is_url_contains("/dir", "/dir2/"));
    }

    #[test]
    fn source_names_ignore_trailing_slashes() {
        assert_eq!(source_name_from_key("a/b/c.txt"), "c.txt");
        assert_eq!(source_name_from_key("c.txt"), "c.txt");
        assert_eq!(source_name_from_key("dir/"), "dir");
    }

    #[test]
    fn parses_retention_durations() {
        let now = UNIX_EPOCH + Duration::from_secs(1_704_164_645); // 2024-01-02T03:04:05Z
        assert_eq!(
            retention_until("1d", now).unwrap(),
            now + Duration::from_secs(86_400)
        );
        let error = retention_until("5w", now).unwrap_err().to_string();
        assert!(error.contains("invalid retention duration"), "{error}");
    }

    fn copy_args(extra: &[&str]) -> CopyArgs {
        use clap::Parser;
        #[derive(Parser)]
        struct Harness {
            #[command(flatten)]
            args: CopyArgs,
        }
        let mut argv = vec!["cp"];
        argv.extend_from_slice(extra);
        Harness::try_parse_from(argv).unwrap().args
    }

    #[test]
    fn validates_lock_flags() {
        let now = SystemTime::now();
        let args = copy_args(&["--legal-hold", "ON", "a", "b"]);
        assert_eq!(lock_options(&args, now).unwrap().0, Some(true));
        let args = copy_args(&["--legal-hold", "maybe", "a", "b"]);
        assert!(lock_options(&args, now).is_err());
        let args = copy_args(&["--retention-mode", "governance", "a", "b"]);
        assert!(lock_options(&args, now).is_err());
        let args = copy_args(&[
            "--retention-mode",
            "compliance",
            "--retention-duration",
            "2d",
            "a",
            "b",
        ]);
        let (_, retention) = lock_options(&args, now).unwrap();
        assert_eq!(retention.unwrap().0, "COMPLIANCE");
        let args = copy_args(&[
            "--retention-mode",
            "strict",
            "--retention-duration",
            "2d",
            "a",
            "b",
        ]);
        assert!(lock_options(&args, now).is_err());
    }

    #[test]
    fn validates_flag_combinations() {
        let local = |raw: &str| Endpoint::Local {
            raw: raw.into(),
            path: raw.into(),
        };
        let options = CopyOptions {
            max_workers: 1,
            ..Default::default()
        };
        assert!(validate(&[s3("a")], &local("out"), &options).is_ok());
        let with = |f: &dyn Fn(&mut CopyOptions)| {
            let mut options = options.clone();
            f(&mut options);
            options
        };
        let tagged = with(&|o| o.metadata.tags = Some("a=b".into()));
        assert!(validate(&[s3("a")], &local("out"), &tagged).is_err());
        assert!(validate(&[local("a")], &s3(""), &tagged).is_ok());
        let versioned = with(&|o| o.version_id = Some("v".into()));
        assert!(validate(&[s3("a"), s3("b")], &local("out/"), &versioned).is_err());
        assert!(validate(&[local("a")], &s3(""), &versioned).is_err());
        let recursive_version = with(&|o| {
            o.version_id = Some("v".into());
            o.recursive = true;
        });
        assert!(validate(&[s3("a")], &local("out"), &recursive_version).is_err());
        let zip_rewind = with(&|o| {
            o.zip = true;
            o.rewind = Some(SystemTime::now());
        });
        assert!(validate(&[s3("a")], &local("out"), &zip_rewind).is_err());
        let no_workers = with(&|o| o.max_workers = 0);
        assert!(validate(&[s3("a")], &local("out"), &no_workers).is_err());
    }
}
