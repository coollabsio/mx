use crate::commands::util::{format_print_time, glob_match, humanize_ibytes};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use crate::flags::parse_size;
use crate::flags::{TimeFilterFlags, VersionsFlag};
use crate::location::{Location, parse_location};
use crate::s3::{ListOptions, S3ResultExt, list_objects_with};
use anyhow::{Context, Result, anyhow, bail};
use aws_sdk_s3::Client;
use clap::Args;
use regex::Regex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Args)]
#[command(after_help = FIND_HELP, mut_args(|a| match a.get_id().as_str() {
    "versions" => a.help("include all objects versions"),
    "older_than" => a.help("match all objects older than value in duration string (e.g. 7d10h31s)"),
    "newer_than" => a.help("match all objects newer than value in duration string (e.g. 7d10h31s)"),
    _ => a,
}))]
pub struct FindArgs {
    /// spawn an external process for each matching object (see FORMAT)
    #[arg(long, value_name = "COMMAND")]
    pub exec: Option<String>,
    /// exclude objects matching the wildcard pattern
    #[arg(long, value_name = "GLOB")]
    pub ignore: Option<String>,
    #[command(flatten)]
    pub versions: VersionsFlag,
    /// find object names matching wildcard pattern
    #[arg(long, value_name = "GLOB")]
    pub name: Option<String>,
    #[command(flatten)]
    pub time: TimeFilterFlags,
    /// match directory names matching wildcard pattern
    #[arg(long, value_name = "GLOB")]
    pub path: Option<String>,
    /// print in custom format to STDOUT (see FORMAT)
    #[arg(long, value_name = "FORMAT")]
    pub print: Option<String>,
    /// match directory and object name with RE2 regex pattern
    #[arg(long, value_name = "REGEX")]
    pub regex: Option<String>,
    /// match all objects larger than specified size in units (see UNITS)
    #[arg(long, value_name = "SIZE")]
    pub larger: Option<String>,
    /// match all objects smaller than specified size in units (see UNITS)
    #[arg(long, value_name = "SIZE")]
    pub smaller: Option<String>,
    /// limit directory navigation to specified depth (default: 0)
    #[arg(long)]
    pub maxdepth: Option<usize>,
    /// monitor a specified path for newly created object(s)
    #[arg(long)]
    pub watch: bool,
    /// match metadata with RE2 regex pattern. Specify each with key=regex. MinIO server only.
    #[arg(
        long,
        value_name = "KEY=REGEX",
        help = "match metadata with RE2 regex pattern. Specify each with key=regex. MinIO server only."
    )]
    pub metadata: Vec<String>,
    /// match tags with RE2 regex pattern. Specify each with key=regex. MinIO server only.
    #[arg(
        long,
        value_name = "KEY=REGEX",
        help = "match tags with RE2 regex pattern. Specify each with key=regex. MinIO server only."
    )]
    pub tags: Vec<String>,
    #[arg(default_value = ".")]
    pub target: String,
}

const FIND_HELP: &str = "UNITS:
  --smaller, --larger accept human-readable sizes: k, m, g, t (KB, MB, GB, TB) and
  ki, mi, gi, ti (KiB, MiB, GiB, TiB); a trailing b is optional. Without suffix: bytes.

FORMAT:
  {}        full path          {base}    basename of path
  {dir}     dirname of path    {size}    object size
  {time}    modified time      {version} version ID
  {url}     presigned URL valid for 7 days (S3 only)
  Wrap a keyword in quotes, e.g. {\"base\"}, to get a quoted value.";

/// One listed object (or version / delete marker), bucket or local file/folder.
#[derive(Debug, Clone, Default)]
struct Entry {
    /// mc `Key`: `ALIAS/BUCKET/KEY`, or the absolute local path (`{}`).
    key: String,
    /// S3 bucket and key (None for local files).
    object: Option<(String, String)>,
    size: i64,
    modified: Option<SystemTime>,
    version_id: Option<String>,
    is_delete_marker: bool,
}

/// All non-metadata filters.
#[derive(Debug, Default)]
struct Matcher {
    ignore: Option<String>,
    name: Option<String>,
    path: Option<String>,
    regex: Option<Regex>,
    time: TimeFilterFlags,
    larger: Option<u64>,
    smaller: Option<u64>,
    metadata: Vec<(String, Option<Regex>)>,
    tags: Vec<(String, Option<Regex>)>,
}

impl Matcher {
    fn from_args(args: &FindArgs) -> Result<Self> {
        args.time.parsed()?;
        let regex = args
            .regex
            .as_deref()
            .map(|re| Regex::new(re).with_context(|| format!("invalid --regex `{re}`")))
            .transpose()?;
        let size = |value: &Option<String>| {
            value
                .as_deref()
                .map(|v| parse_size(v).context("Unable to parse input bytes."))
                .transpose()
        };
        Ok(Self {
            ignore: args.ignore.clone(),
            name: args.name.clone(),
            path: args.path.clone(),
            regex,
            time: args.time.clone(),
            larger: size(&args.larger)?.filter(|v| *v > 0),
            smaller: size(&args.smaller)?.filter(|v| *v > 0),
            metadata: parse_regex_map(&args.metadata, "--metadata")?,
            tags: parse_regex_map(&args.tags, "--tags")?,
        })
    }

    /// mc `matchFind` without the metadata/tag checks. `path` is the key without the target
    /// prefix ([`match_path`]).
    fn matches(&self, entry: &Entry, path: &str, now: SystemTime) -> bool {
        if self.ignore.as_deref().is_some_and(|p| glob_match(p, path)) {
            return false;
        }
        if self.name.as_deref().is_some_and(|p| !name_match(p, path)) {
            return false;
        }
        if self.path.as_deref().is_some_and(|p| !glob_match(p, path)) {
            return false;
        }
        if self.regex.as_ref().is_some_and(|re| !re.is_match(path)) {
            return false;
        }
        if !self.time.matches(entry.modified, now) {
            return false;
        }
        if self.larger.is_some_and(|limit| entry.size <= limit as i64) {
            return false;
        }
        if self.smaller.is_some_and(|limit| entry.size >= limit as i64) {
            return false;
        }
        true
    }

    fn needs_object_details(&self) -> bool {
        !self.metadata.is_empty() || !self.tags.is_empty()
    }
}

/// Parses repeated `KEY=REGEX` values; an empty regex requires the value to be empty/absent.
fn parse_regex_map(values: &[String], flag: &str) -> Result<Vec<(String, Option<Regex>)>> {
    values
        .iter()
        .map(|value| {
            let (key, pattern) = value.split_once('=').ok_or_else(|| {
                anyhow!("Unable to split key+value `{value}`. {flag} must be key=regex")
            })?;
            let regex = if pattern.is_empty() {
                None
            } else {
                Some(Regex::new(pattern).with_context(|| {
                    format!("Unable to compile {flag} regex for {key}={pattern}")
                })?)
            };
            Ok((key.to_string(), regex))
        })
        .collect()
}

/// Every `(key, regex)` must match `values`. Keys are compared case-insensitively;
/// `strip_meta_prefix` also accepts `X-Amz-Meta-KEY` spellings.
fn match_regex_map(
    filters: &[(String, Option<Regex>)],
    values: &HashMap<String, String>,
    strip_meta_prefix: bool,
) -> bool {
    filters.iter().all(|(key, regex)| {
        let mut key = key.to_ascii_lowercase();
        if strip_meta_prefix && let Some(stripped) = key.strip_prefix("x-amz-meta-") {
            key = stripped.to_string();
        }
        let value = values
            .iter()
            .find(|(name, _)| name.to_ascii_lowercase() == key)
            .map(|(_, value)| value.as_str());
        match regex {
            None => value.unwrap_or_default().is_empty(),
            Some(regex) => value.is_some_and(|value| regex.is_match(value)),
        }
    })
}

/// mc `nameMatch`: shell pattern on the basename, or any path component equal to `pattern`.
fn name_match(pattern: &str, path: &str) -> bool {
    let base = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    shell_match(pattern, base) || path.split('/').any(|component| component == pattern)
}

/// Go `filepath.Match` on a single path component: `*`, `?`, `[...]`/`[!...]`, `\` escapes.
fn shell_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|i| go(&p[1..], &t[i..])),
            Some('?') => !t.is_empty() && go(&p[1..], &t[1..]),
            Some('[') => {
                let Some(c) = t.first() else { return false };
                let mut i = 1;
                let negate = matches!(p.get(1), Some('!') | Some('^'));
                if negate {
                    i += 1;
                }
                let mut matched = false;
                let mut first = true;
                while i < p.len() && (p[i] != ']' || first) {
                    first = false;
                    let lo = p[i];
                    if p.get(i + 1) == Some(&'-') && p.get(i + 2).is_some_and(|c| *c != ']') {
                        if lo <= *c && *c <= p[i + 2] {
                            matched = true;
                        }
                        i += 3;
                    } else {
                        if lo == *c {
                            matched = true;
                        }
                        i += 1;
                    }
                }
                if i >= p.len() {
                    return false; // unterminated class
                }
                matched != negate && go(&p[i + 1..], &t[1..])
            }
            Some('\\') if p.len() > 1 => t.first() == Some(&p[1]) && go(&p[2..], &t[1..]),
            Some(ch) => t.first() == Some(ch) && go(&p[1..], &t[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t)
}

/// The part of `key` that mc matches patterns against: the key without the target argument
/// as typed (plus `/` unless it starts with `/`, a quirk of mc's `matchFind`). Local keys are
/// absolute, so for a relative local target the whole absolute path is matched.
fn match_path<'a>(target: &str, key: &'a str) -> &'a str {
    let prefix = if target.starts_with('/') {
        target.to_string()
    } else {
        format!("{target}/")
    };
    key.strip_prefix(&prefix).unwrap_or(key)
}

/// mc `trimSuffixAtMaxDepth`: with `--maxdepth N` mc does not filter but truncates each key to
/// its first N components after the target argument (printing duplicates).
fn trim_at_max_depth(target: &str, key: &str, max_depth: usize) -> String {
    if max_depth == 0 {
        return key.to_string();
    }
    let rest = key.strip_prefix(target).unwrap_or(key);
    let kept: String = rest.split_inclusive('/').take(max_depth).collect();
    format!("{target}{kept}")
}

/// Go `time.RFC3339Nano` in UTC (trailing zeros of the fraction trimmed).
fn rfc3339_nano(time: SystemTime) -> String {
    let since = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let base = aws_sdk_s3::primitives::DateTime::from_secs(since.as_secs() as i64)
        .fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
        .unwrap_or_default();
    let nanos = since.subsec_nanos();
    if nanos == 0 {
        return base;
    }
    let fraction = format!("{nanos:09}");
    format!(
        "{}.{}Z",
        base.trim_end_matches('Z'),
        fraction.trim_end_matches('0')
    )
}

/// Go `filepath.Base`.
fn base_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "." } else { "/" };
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// Go `filepath.Dir` (without full path cleaning).
fn dir_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(index) => path[..index].trim_end_matches('/'),
        None => ".",
    }
}

/// Expands mc `--print` / `--exec` keywords. `url` is only called when `{url}` is used.
fn expand(
    format: &str,
    entry: &Entry,
    url: &mut dyn FnMut(&Entry) -> Result<String>,
) -> Result<String> {
    let quote = |value: &str| serde_json::to_string(value).unwrap_or_default();
    let size = humanize_ibytes(entry.size.max(0) as u64);
    let time = entry.modified.map(format_print_time).unwrap_or_default();
    let version = entry.version_id.clone().unwrap_or_default();
    let mut out = format
        .replace("{}", &entry.key)
        .replace("{\"\"}", &quote(&entry.key))
        .replace("{base}", base_name(&entry.key))
        .replace("{\"base\"}", &quote(base_name(&entry.key)))
        .replace("{dir}", dir_name(&entry.key))
        .replace("{\"dir\"}", &quote(dir_name(&entry.key)))
        .replace("{size}", &size)
        .replace("{\"size\"}", &quote(&size))
        .replace("{time}", &time)
        .replace("{\"time\"}", &quote(&time));
    if out.contains("{url}") || out.contains("{\"url\"}") {
        let url = url(entry)?;
        out = out
            .replace("{url}", &url)
            .replace("{\"url\"}", &quote(&url));
    }
    Ok(out
        .replace("{version}", &version)
        .replace("{\"version\"}", &quote(&version)))
}

enum Source {
    S3 {
        alias_name: String,
        alias: Box<AliasConfig>,
        client: Client,
        /// None = all buckets of the alias.
        bucket: Option<String>,
        /// Key prefix exactly as typed (mc lists `b/dir` with prefix `dir`, matching `dirx/`).
        prefix: Option<String>,
    },
    /// Absolute local path.
    Local(PathBuf),
}

impl Source {
    fn list(&self, rt: &tokio::runtime::Runtime, versions: bool) -> Result<Vec<Entry>> {
        match self {
            Source::S3 {
                alias_name,
                client,
                bucket,
                prefix,
                ..
            } => {
                let mut entries = Vec::new();
                let Some(bucket) = bucket else {
                    // Alias root: each bucket, then its objects.
                    let buckets = rt.block_on(client.list_buckets().send()).s3("", "")?;
                    for bucket in buckets.buckets() {
                        let name = bucket.name().unwrap_or_default();
                        entries.push(Entry {
                            key: format!("{alias_name}/{name}"),
                            modified: bucket.creation_date().and_then(crate::s3::to_system_time),
                            ..Default::default()
                        });
                        entries.extend(
                            rt.block_on(list_objects(client, alias_name, name, None, versions))?,
                        );
                    }
                    return Ok(entries);
                };
                rt.block_on(list_objects(
                    client,
                    alias_name,
                    bucket,
                    prefix.as_deref(),
                    versions,
                ))
            }
            Source::Local(root) => {
                let root_key = root.to_string_lossy().into_owned();
                let meta = std::fs::metadata(root)
                    .map_err(|error| crate::error::io_error(&error, &root_key))?;
                let mut entries = vec![local_entry(root_key.clone(), &meta)];
                if meta.is_dir() {
                    walk_local(root, &root_key, &mut entries);
                }
                Ok(entries)
            }
        }
    }
}

/// Recursive S3 listing of `bucket` under the raw `prefix`, keys as `ALIAS/BUCKET/KEY`.
/// GLACIER objects are skipped like mc.
async fn list_objects(
    client: &Client,
    alias_name: &str,
    bucket: &str,
    prefix: Option<&str>,
    versions: bool,
) -> Result<Vec<Entry>> {
    let items = if versions {
        // Version listings take a folder prefix: list the parent folder and filter.
        let prefix = prefix.unwrap_or_default();
        let parent = &prefix[..prefix.rfind('/').map_or(0, |i| i + 1)];
        let options = ListOptions {
            recursive: true,
            versions: true,
            ..Default::default()
        };
        list_objects_with(
            client,
            bucket,
            Some(parent).filter(|p| !p.is_empty()),
            &options,
        )
        .await?
        .into_iter()
        .map(|mut item| {
            item.key = format!("{parent}{}", item.key);
            item
        })
        .filter(|item| !item.is_prefix && item.key.starts_with(prefix))
        .collect()
    } else {
        let mut pages = client
            .list_objects_v2()
            .bucket(bucket)
            .set_prefix(prefix.map(str::to_string))
            .into_paginator()
            .send();
        let mut items = Vec::new();
        while let Some(page) = pages.next().await {
            let page = page.s3(bucket, "")?;
            items.extend(page.contents().iter().map(|object| crate::s3::ObjectInfo {
                key: object.key().unwrap_or_default().to_string(),
                size: object.size().unwrap_or(0),
                last_modified: object.last_modified().and_then(crate::s3::to_system_time),
                storage_class: object.storage_class().map(|c| c.as_str().to_string()),
                ..Default::default()
            }));
        }
        items
    };
    Ok(items
        .into_iter()
        .filter(|item| item.storage_class.as_deref() != Some("GLACIER"))
        .map(|item| Entry {
            key: format!("{alias_name}/{bucket}/{}", item.key),
            object: Some((bucket.to_string(), item.key)),
            size: item.size,
            modified: item.last_modified,
            version_id: item.version_id,
            is_delete_marker: item.is_delete_marker,
        })
        .collect())
}

fn local_entry(key: String, meta: &std::fs::Metadata) -> Entry {
    Entry {
        key,
        size: meta.len() as i64,
        modified: meta.modified().ok(),
        ..Default::default()
    }
}

/// mc's local listing: folders (listed before their contents) and files, sorted by name with
/// folders compared as `name/`. Symlinks are listed but not followed into; unreadable folders
/// are reported (mc "Unable to list folder.") and skipped.
fn walk_local(dir: &Path, dir_key: &str, entries: &mut Vec<Entry>) {
    let items = match std::fs::read_dir(dir) {
        Ok(items) => items,
        Err(error) => {
            let error = anyhow::Error::from(crate::error::io_error(&error, dir_key));
            crate::output::print_error(
                &error.context(crate::error::nonfatal("Unable to list folder.")),
            );
            return;
        }
    };
    let mut children: Vec<(String, PathBuf, bool)> = items
        .filter_map(|item| item.ok())
        .map(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            let is_dir = item.file_type().is_ok_and(|t| t.is_dir());
            let sort = if is_dir { format!("{name}/") } else { name };
            (sort, item.path(), is_dir)
        })
        .collect();
    children.sort();
    for (sort, path, is_dir) in children {
        let key = format!("{dir_key}/{}", sort.trim_end_matches('/'));
        let meta = std::fs::metadata(&path).or_else(|_| std::fs::symlink_metadata(&path));
        let Ok(meta) = meta else { continue };
        entries.push(local_entry(key.clone(), &meta));
        if is_dir {
            walk_local(&path, &key, entries);
        }
    }
}

#[derive(Debug, Serialize)]
struct FindMessage<'a> {
    status: &'static str,
    /// mc leaves the content type empty for find results.
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "lastModified")]
    last_modified: String,
    size: i64,
    key: &'a str,
    etag: &'static str,
    #[serde(rename = "versionId", skip_serializing_if = "Option::is_none")]
    version_id: Option<&'a str>,
}

struct Finder<'a> {
    args: &'a FindArgs,
    matcher: Matcher,
    /// The target argument as typed (`.` becomes `./`, like mc).
    target: String,
    source: Source,
    rt: tokio::runtime::Runtime,
    json: bool,
}

impl Finder<'_> {
    fn handle(&self, entry: &Entry, now: SystemTime) -> Result<()> {
        let mut entry = entry.clone();
        entry.key = trim_at_max_depth(&self.target, &entry.key, self.args.maxdepth.unwrap_or(0));
        let path = match_path(&self.target, &entry.key);
        if !self.matcher.matches(&entry, path, now) || !self.details_match(&entry)? {
            return Ok(());
        }
        let mut url = |entry: &Entry| self.share_url(entry);
        if let Some(command) = &self.args.exec {
            return run_exec(command, &entry, &mut url);
        }
        let key = match &self.args.print {
            Some(format) => expand(format, &entry, &mut url)?,
            None => entry.key.clone(),
        };
        let version = entry.version_id.as_deref().filter(|v| !v.is_empty());
        if self.json {
            crate::output::print_json(&FindMessage {
                status: "success",
                kind: "",
                last_modified: entry.modified.map(rfc3339_nano).unwrap_or_default(),
                size: entry.size,
                key: &key,
                etag: "",
                version_id: version,
            })?;
        } else if let Some(version) = version {
            println!("{key} ({version})");
        } else {
            println!("{key}");
        }
        Ok(())
    }

    /// `--metadata` / `--tags` checks (one HeadObject / GetObjectTagging per candidate).
    fn details_match(&self, entry: &Entry) -> Result<bool> {
        if !self.matcher.needs_object_details() {
            return Ok(true);
        }
        let (Source::S3 { client, .. }, Some((bucket, key))) = (&self.source, &entry.object) else {
            return Ok(false);
        };
        if entry.is_delete_marker {
            return Ok(false);
        }
        let version = entry.version_id.as_deref();
        if !self.matcher.metadata.is_empty() {
            let metadata = self
                .rt
                .block_on(crate::s3::object_metadata(client, bucket, key, version))?;
            if !match_regex_map(&self.matcher.metadata, &metadata, true) {
                return Ok(false);
            }
        }
        if !self.matcher.tags.is_empty() {
            let tags = self
                .rt
                .block_on(crate::s3::object_tags(client, bucket, key, version))?;
            if !match_regex_map(&self.matcher.tags, &tags, false) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn share_url(&self, entry: &Entry) -> Result<String> {
        let (Source::S3 { alias, .. }, Some((bucket, key))) = (&self.source, &entry.object) else {
            bail!("{{url}} is only supported for S3 targets");
        };
        self.rt.block_on(crate::s3::presign_get(
            alias,
            bucket,
            key,
            Duration::from_secs(7 * 24 * 3600),
        ))
    }
}

/// Runs `--exec` for one match (mc: parse with shell quoting, substitute keywords, print the
/// command's stdout; on failure print stderr and exit with the command's status).
fn run_exec(
    command: &str,
    entry: &Entry,
    url: &mut dyn FnMut(&Entry) -> Result<String>,
) -> Result<()> {
    let words =
        shlex::split(command).ok_or_else(|| anyhow!("Unable to parse --exec `{command}`"))?;
    let Some((program, rest)) = words.split_first() else {
        return Ok(());
    };
    let program = expand(program, entry, url)?;
    let rest = rest
        .iter()
        .map(|arg| expand(arg, entry, url))
        .collect::<Result<Vec<_>>>()?;
    let output = std::process::Command::new(&program)
        .args(&rest)
        .output()
        .with_context(|| format!("Unable to run `{program}`"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr.trim());
        }
        crate::output::error_if(
            &format!("Unable to run `{program}`."),
            &output.status.to_string(),
        );
        std::process::exit(output.status.code().unwrap_or(1));
    }
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&output.stdout)?;
    stdout.flush()?;
    Ok(())
}

fn open_source(store: &ConfigStore, input: &str, rt: &tokio::runtime::Runtime) -> Result<Source> {
    match parse_location(input, store.config()) {
        Location::S3(target) => {
            let alias = alias_config(store, &target.alias)?;
            if target.bucket.is_some() {
                super::util::stat_target(store, input)
                    .with_context(|| format!("Unable to stat `{input}`."))?;
            }
            let client = rt.block_on(crate::s3::build_client(&alias))?;
            Ok(Source::S3 {
                alias_name: target.alias.clone(),
                client,
                alias: Box::new(alias),
                bucket: target.bucket.clone(),
                prefix: target.key.clone().filter(|key| !key.is_empty()),
            })
        }
        Location::Local(root) => {
            super::util::stat_target(store, input)
                .with_context(|| format!("Unable to stat `{input}`."))?;
            Ok(Source::Local(PathBuf::from(crate::error::abs_path(
                &root.to_string_lossy(),
            ))))
        }
    }
}

pub fn run(args: FindArgs, json: bool) -> Result<()> {
    let matcher = Matcher::from_args(&args)?;
    let store = ConfigStore::load_or_create()?;
    let rt = runtime()?;
    let source = open_source(&store, &args.target, &rt)?;
    if let Source::Local(_) = source {
        if args.versions.versions {
            bail!("--versions is only supported for S3 targets");
        }
        if matcher.needs_object_details() {
            bail!("--metadata and --tags are only supported for S3 targets");
        }
    }
    let target = if args.target == "." {
        "./".to_string()
    } else {
        args.target.clone()
    };
    let finder = Finder {
        args: &args,
        matcher,
        target,
        source,
        rt,
        json,
    };

    let mut seen = HashSet::new();
    loop {
        let now = SystemTime::now();
        for entry in finder.source.list(&finder.rt, args.versions.versions)? {
            let identity = (
                entry.key.clone(),
                entry.version_id.clone(),
                entry.modified,
                entry.size,
            );
            if seen.insert(identity) {
                finder.handle(&entry, now)?;
            }
        }
        if !args.watch {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rel: &str, size: i64) -> Entry {
        Entry {
            key: format!("play/bucket/{rel}"),
            object: Some(("bucket".into(), rel.into())),
            size,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_704_164_645)),
            version_id: Some("v1".into()),
            is_delete_marker: false,
        }
    }

    #[test]
    fn matches_names_like_mc() {
        assert!(name_match("*.txt", "dir/a.txt"));
        assert!(!name_match("*.txt", "dir/a.log"));
        assert!(name_match("dir", "dir/a.log"));
        assert!(name_match("[ab].txt", "x/b.txt"));
        assert!(!name_match("[!ab].txt", "x/b.txt"));
        assert!(name_match("a?.txt", "a1.txt"));
        assert!(name_match("\\*.txt", "*.txt"));
        assert!(!name_match("[a", "a"));
    }

    #[test]
    fn applies_filters() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_704_164_645 + 86_400 * 2);
        let matcher = Matcher {
            ignore: Some("*.log".into()),
            path: Some("dir/*".into()),
            regex: Some(Regex::new(r"\.txt$").unwrap()),
            larger: Some(10),
            smaller: Some(100),
            time: TimeFilterFlags {
                older_than: Some("1d".into()),
                newer_than: None,
            },
            ..Default::default()
        };
        let matches = |rel: &str, size: i64, now: SystemTime| {
            let entry = entry(rel, size);
            matcher.matches(&entry, match_path("play/bucket", &entry.key), now)
        };
        assert!(matches("dir/a.txt", 50, now));
        assert!(matches("dir/sub/a.txt", 50, now));
        assert!(!matches("dir/a.log", 50, now));
        assert!(!matches("other/a.txt", 50, now));
        assert!(!matches("dir/a.txt", 10, now));
        assert!(!matches("dir/a.txt", 100, now));
        let recent = SystemTime::UNIX_EPOCH + Duration::from_secs(1_704_164_645 + 60);
        assert!(!matches("dir/a.txt", 50, recent));
    }

    #[test]
    fn match_path_and_max_depth_follow_mc_quirks() {
        assert_eq!(match_path("play/b", "play/b/dir/x"), "dir/x");
        // A trailing slash on the target is not trimmed (mc checks the prefix, not the suffix).
        assert_eq!(match_path("play/b/", "play/b/dir/x"), "play/b/dir/x");
        // Local keys are absolute: relative targets never match, absolute ones keep a `/`.
        assert_eq!(match_path("src", "/w/src/a"), "/w/src/a");
        assert_eq!(match_path("/w/src", "/w/src/a"), "/a");
        assert_eq!(
            trim_at_max_depth("play/b", "play/b/dir/sub/c", 0),
            "play/b/dir/sub/c"
        );
        assert_eq!(
            trim_at_max_depth("play/b", "play/b/dir/sub/c", 1),
            "play/b/"
        );
        assert_eq!(
            trim_at_max_depth("play/b", "play/b/dir/sub/c", 2),
            "play/b/dir/"
        );
        assert_eq!(
            trim_at_max_depth("play/b", "play/b/a.txt", 2),
            "play/b/a.txt"
        );
        assert_eq!(trim_at_max_depth("/w/o", "/w/o", 2), "/w/o");
    }

    #[test]
    fn formats_rfc3339_nano() {
        let at = SystemTime::UNIX_EPOCH + Duration::new(1_704_164_645, 868_000_000);
        assert_eq!(rfc3339_nano(at), "2024-01-02T03:04:05.868Z");
        let at = SystemTime::UNIX_EPOCH + Duration::new(1_704_164_645, 60_238_938);
        assert_eq!(rfc3339_nano(at), "2024-01-02T03:04:05.060238938Z");
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_704_164_645);
        assert_eq!(rfc3339_nano(at), "2024-01-02T03:04:05Z");
    }

    #[test]
    fn matches_metadata_and_tag_maps() {
        let filters = parse_regex_map(
            &[
                "Content-Type=^text/".into(),
                "X-Amz-Meta-Owner=ali".into(),
                "empty=".into(),
            ],
            "--metadata",
        )
        .unwrap();
        let mut values = HashMap::from([
            ("content-type".to_string(), "text/plain".to_string()),
            ("owner".to_string(), "alice".to_string()),
        ]);
        assert!(match_regex_map(&filters, &values, true));
        values.insert("empty".into(), "x".into());
        assert!(!match_regex_map(&filters, &values, true));
        let tags = parse_regex_map(&["env=^prod$".into()], "--tags").unwrap();
        assert!(match_regex_map(
            &tags,
            &HashMap::from([("env".into(), "prod".into())]),
            false
        ));
        assert!(!match_regex_map(&tags, &HashMap::new(), false));
        assert!(parse_regex_map(&["novalue".into()], "--tags").is_err());
        assert!(parse_regex_map(&["k=(".into()], "--tags").is_err());
    }

    #[test]
    fn expands_print_format() {
        let entry = entry("dir/a.txt", 2048);
        let mut url = |_: &Entry| Ok("https://signed".to_string());
        assert_eq!(
            expand("{} {base} {dir} {size} {version}", &entry, &mut url).unwrap(),
            "play/bucket/dir/a.txt a.txt play/bucket/dir 2.0 KiB v1"
        );
        assert_eq!(
            expand("{\"base\"} {time} {url}", &entry, &mut url).unwrap(),
            "\"a.txt\" 2024-01-02 03:04:05 UTC https://signed"
        );
        let mut failing = |_: &Entry| -> Result<String> { panic!("url not requested") };
        assert_eq!(expand("{base}", &entry, &mut failing).unwrap(), "a.txt");
        assert_eq!(dir_name("a.txt"), ".");
        assert_eq!(base_name("dir/"), "dir");
    }
}
