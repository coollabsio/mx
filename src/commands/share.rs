//! `share download|upload|list` (mc share). Generated shares are persisted in mc's share DB
//! format under `<config dir>/share/{uploads,downloads}.json`; `share list` shows the ones that
//! have not expired yet.

use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use crate::flags::VersionIdFlag;
use crate::s3::{GetOptions, ListOptions};
use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// mc `shareDefaultExpiry` and the maximum presign lifetime.
const MAX_EXPIRY: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Debug, Args)]
pub struct ShareArgs {
    #[command(subcommand)]
    pub command: ShareCommand,
}

#[derive(Debug, Subcommand)]
pub enum ShareCommand {
    #[command(about = "generate URLs for download access")]
    Download(ShareDownloadArgs),
    #[command(
        about = "generate `curl` command to upload objects without requiring access/secret keys"
    )]
    Upload(ShareUploadArgs),
    #[command(visible_alias = "ls", about = "list previously shared objects")]
    List(ShareListArgs),
}

#[derive(Debug, Args)]
#[command(mut_args(|a| if a.get_id().as_str() == "version_id" {
    a.help("share a particular object version")
} else {
    a
}))]
pub struct ShareDownloadArgs {
    /// share all objects recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub version: VersionIdFlag,
    /// set expiry in NN[h|m|s]
    #[arg(short = 'E', long, default_value = "168h")]
    pub expire: String,
    #[arg(required = true)]
    pub targets: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ShareUploadArgs {
    /// recursively upload any object matching the prefix
    #[arg(short = 'r', long)]
    pub recursive: bool,
    /// set expiry in NN[h|m|s]
    #[arg(short = 'E', long, default_value = "168h")]
    pub expire: String,
    /// specify a content-type to allow
    #[arg(short = 'T', long = "content-type")]
    pub content_type: Option<String>,
    #[arg(required = true)]
    pub targets: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ShareListArgs {
    pub kind: ShareKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ShareKind {
    Upload,
    Download,
}

pub fn run(command: ShareCommand, json: bool) -> Result<()> {
    match command {
        ShareCommand::Download(args) => download(args, json),
        ShareCommand::Upload(args) => upload(args, json),
        ShareCommand::List(args) => list(args.kind, json),
    }
}

/// Parses a Go-style duration (`168h`, `1h30m`, `90s`, `1.5h`, `500ms`); `d` is accepted as
/// 24h for backward compatibility.
pub fn parse_go_duration(input: &str) -> Result<Duration> {
    let text = input.trim();
    let invalid = || anyhow!("Unable to parse expire=`{input}`.");
    if text.is_empty() {
        return Err(invalid());
    }
    let mut total = 0f64;
    let mut rest = text;
    while !rest.is_empty() {
        let number_len = rest
            .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
            .ok_or_else(invalid)?;
        let number: f64 = rest[..number_len].parse().map_err(|_| invalid())?;
        rest = &rest[number_len..];
        let unit_len = rest
            .find(|ch: char| ch.is_ascii_digit() || ch == '.')
            .unwrap_or(rest.len());
        let seconds = match &rest[..unit_len] {
            "ns" => 1e-9,
            "us" | "µs" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            "d" => 86_400.0,
            _ => return Err(invalid()),
        };
        total += number * seconds;
        rest = &rest[unit_len..];
    }
    Ok(Duration::from_secs_f64(total))
}

fn parse_expiry(input: &str) -> Result<Duration> {
    let expiry = parse_go_duration(input)?;
    if expiry < Duration::from_secs(1) {
        bail!("Expiry cannot be lesser than 1 second.");
    }
    if expiry > MAX_EXPIRY {
        bail!("Expiry cannot be larger than 7 days.");
    }
    Ok(expiry)
}

/// Full endpoint URL of an object (what mc prints as the object URL).
fn object_url(alias: &AliasConfig, bucket: &str, key: &str) -> Result<String> {
    Ok(format!("{}{key}", crate::s3::bucket_url(alias, bucket)?))
}

fn download(args: ShareDownloadArgs, json: bool) -> Result<()> {
    let expiry = parse_expiry(&args.expire)?;
    let version_id = args.version.version_id.clone();
    if version_id.is_some() && args.recursive {
        bail!("--version-id cannot be specified with --recursive flag.");
    }
    let store = ConfigStore::load_or_create()?;
    let db_path = share_dir(&store)?.join("downloads.json");
    let mut db = ShareDb::load(&db_path)?;
    let rt = runtime()?;
    for target_arg in &args.targets {
        super::util::stat_target(&store, target_arg)
            .with_context(|| format!("Unable to stat `{target_arg}`."))?;
        let (alias, target) = require_s3(&store, target_arg)?;
        let bucket = target.require_bucket()?.to_string();
        let objects = rt
            .block_on(share_download_targets(
                &alias,
                &bucket,
                target.key_with_trailing_slash().as_deref(),
                version_id.as_deref(),
                args.recursive,
            ))
            .with_context(|| format!("Unable to share target `{target_arg}`."))?;
        for (key, version) in objects {
            let share = rt.block_on(crate::s3::presign_get_version(
                &alias,
                &bucket,
                &key,
                version.as_deref(),
                expiry,
            ))?;
            let url = object_url(&alias, &bucket, &key)?;
            print_share(&url, &share, expiry, "", json)?;
            db.set(&url, &share, expiry, "");
        }
    }
    db.save(&db_path)
}

/// Objects to share: the object itself, or the objects under a prefix (recursively with -r).
async fn share_download_targets(
    alias: &AliasConfig,
    bucket: &str,
    key: Option<&str>,
    version_id: Option<&str>,
    recursive: bool,
) -> Result<Vec<(String, Option<String>)>> {
    let client = crate::s3::build_client(alias).await?;
    let prefix = match key {
        Some(key) if !key.ends_with('/') => {
            let options = GetOptions {
                version_id: version_id.map(str::to_string),
                ..Default::default()
            };
            match crate::s3::head_object_with(&client, bucket, key, &options).await {
                Ok(_) => return Ok(vec![(key.to_string(), version_id.map(str::to_string))]),
                Err(error) => {
                    // Maybe a "directory" given without the trailing slash.
                    let dir = format!("{key}/");
                    let listed = crate::s3::list_objects_with(
                        &client,
                        bucket,
                        Some(&dir),
                        &ListOptions::default(),
                    )
                    .await
                    .unwrap_or_default();
                    if listed.is_empty() {
                        return Err(error);
                    }
                    Some(dir)
                }
            }
        }
        other => other.map(str::to_string),
    };
    let items = crate::s3::list_objects_with(
        &client,
        bucket,
        prefix.as_deref(),
        &ListOptions {
            recursive,
            ..Default::default()
        },
    )
    .await?;
    let base = prefix.unwrap_or_default();
    Ok(items
        .into_iter()
        .filter(|item| !item.is_prefix)
        .map(|item| (crate::s3::full_key(&base, &item.key), None))
        .collect())
}

fn shell_quote(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if "&;#$` \t\n<>()|'\"".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// `curl URL -F field=value ... -F key=KEY[<NAME>] -F file=@<FILE>` (mc makeCurlCmd).
pub fn curl_command(url: &str, fields: &BTreeMap<String, String>, recursive: bool) -> String {
    let mut command = format!("curl {url} ");
    for (name, value) in fields {
        if name != "key" {
            command.push_str(&format!("-F {name}={value} "));
        }
    }
    let key = fields.get("key").map(String::as_str).unwrap_or_default();
    if recursive {
        command.push_str(&format!("-F key={}<NAME> ", shell_quote(key)));
    } else {
        command.push_str(&format!("-F key={} ", shell_quote(key)));
    }
    command.push_str("-F file=@<FILE>");
    command
}

fn upload(args: ShareUploadArgs, json: bool) -> Result<()> {
    let expiry = parse_expiry(&args.expire)?;
    for target in &args.targets {
        if target.ends_with('/') && !args.recursive {
            bail!("Use --recursive flag to generate curl command for prefixes.");
        }
    }
    let content_type = args
        .content_type
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let store = ConfigStore::load_or_create()?;
    let db_path = share_dir(&store)?.join("uploads.json");
    let mut db = ShareDb::load(&db_path)?;
    let rt = runtime()?;
    for target_arg in &args.targets {
        let (alias, target) = require_s3(&store, target_arg)?;
        let bucket = target.require_bucket()?.to_string();
        let key = target.key_with_trailing_slash().unwrap_or_default();
        if key.is_empty() && !args.recursive {
            bail!("Object name is empty; use --recursive to share uploads to a bucket.");
        }
        let form = rt
            .block_on(crate::s3::presign_post(
                &alias,
                &bucket,
                &key,
                args.recursive,
                content_type,
                expiry,
            ))
            .with_context(|| {
                format!("Unable to generate curl command for upload `{target_arg}`.")
            })?;
        let share = curl_command(&form.url, &form.fields, args.recursive);
        let url = object_url(&alias, &bucket, &key)?;
        let content_type = content_type.unwrap_or_default();
        print_share(&url, &share, expiry, content_type, json)?;
        db.set(&url, &share, expiry, content_type);
    }
    db.save(&db_path)
}

fn list(kind: ShareKind, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let file = match kind {
        ShareKind::Upload => "uploads.json",
        ShareKind::Download => "downloads.json",
    };
    let path = share_dir(&store)?.join(file);
    let db = ShareDb::load(&path)?;
    db.save(&path)?;
    let now = SystemTime::now();
    let mut shares = db.shares.iter().collect::<Vec<_>>();
    shares.sort_by(|left, right| left.1.date.cmp(&right.1.date));
    for (share, entry) in shares {
        print_share(
            &entry.url,
            share,
            entry.time_left(now),
            entry.content_type.as_deref().unwrap_or_default(),
            json,
        )?;
    }
    Ok(())
}

#[derive(Serialize)]
struct ShareMessage<'a> {
    status: &'a str,
    url: &'a str,
    share: &'a str,
    /// Nanoseconds (Go `time.Duration`).
    #[serde(rename = "timeLeft")]
    time_left: u128,
    #[serde(rename = "contentType", skip_serializing_if = "str::is_empty")]
    content_type: &'a str,
}

fn print_share(
    url: &str,
    share: &str,
    time_left: Duration,
    content_type: &str,
    json: bool,
) -> Result<()> {
    if json {
        let doc = crate::output::json_string(&ShareMessage {
            status: "success",
            url,
            share,
            time_left: time_left.as_nanos(),
            content_type,
        })?;
        // mc un-escapes `&`, `<`, `>` so the share URL stays usable.
        println!(
            "{}",
            doc.replace("\\u0026", "&")
                .replace("\\u003c", "<")
                .replace("\\u003e", ">")
        );
        return Ok(());
    }
    println!("URL: {url}");
    println!("Expire: {}", humanize_duration(time_left));
    if !content_type.is_empty() {
        println!("Content-Type: {content_type}");
    }
    println!("Share: {share}");
    Ok(())
}

/// mc `timeDurationToHumanizedDuration(...).String()`.
pub fn humanize_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs == 0 {
        return format!("{} milliseconds", duration.as_millis());
    }
    let (days, hours, minutes, seconds) =
        (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if secs < 60 {
        format!("{seconds} seconds")
    } else if secs < 3600 {
        format!("{} minutes {seconds} seconds", secs / 60)
    } else if secs < 86_400 {
        format!("{} hours {minutes} minutes {seconds} seconds", secs / 3600)
    } else {
        format!("{days} days {hours} hours {minutes} minutes {seconds} seconds")
    }
}

// ---------------------------------------------------------------------------
// share DB (mc `shareDBV1`)
// ---------------------------------------------------------------------------

fn share_dir(store: &ConfigStore) -> Result<PathBuf> {
    Ok(store
        .path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("share"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareEntry {
    /// Object URL.
    #[serde(rename = "share")]
    pub url: String,
    #[serde(rename = "versionID", default)]
    pub version_id: String,
    /// RFC3339 creation time.
    pub date: String,
    /// Nanoseconds (Go `time.Duration`).
    pub expiry: u64,
    #[serde(
        rename = "contentType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub content_type: Option<String>,
}

impl ShareEntry {
    fn created(&self) -> Option<SystemTime> {
        let parsed = aws_sdk_s3::primitives::DateTime::from_str(
            &self.date,
            aws_sdk_s3::primitives::DateTimeFormat::DateTimeWithOffset,
        )
        .ok()?;
        SystemTime::try_from(parsed).ok()
    }

    pub fn time_left(&self, now: SystemTime) -> Duration {
        let Some(created) = self.created() else {
            return Duration::ZERO;
        };
        let elapsed = now.duration_since(created).unwrap_or_default();
        Duration::from_nanos(self.expiry).saturating_sub(elapsed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareDb {
    pub version: String,
    /// Keyed by share URL (or curl command).
    #[serde(default)]
    pub shares: BTreeMap<String, ShareEntry>,
}

impl Default for ShareDb {
    fn default() -> Self {
        Self {
            version: "1".to_string(),
            shares: BTreeMap::new(),
        }
    }
}

impl ShareDb {
    /// Loads the DB (empty if missing) and drops expired entries.
    pub fn load(path: &Path) -> Result<Self> {
        let mut db = if path.exists() {
            let text = fs::read_to_string(path)
                .with_context(|| format!("Unable to read `{}`.", path.display()))?;
            serde_json::from_str::<ShareDb>(&text)
                .with_context(|| format!("Unable to parse `{}`.", path.display()))?
        } else {
            ShareDb::default()
        };
        let now = SystemTime::now();
        db.shares.retain(|_, entry| !entry.time_left(now).is_zero());
        Ok(db)
    }

    pub fn set(&mut self, url: &str, share: &str, expiry: Duration, content_type: &str) {
        self.shares.insert(
            share.to_string(),
            ShareEntry {
                url: url.to_string(),
                version_id: String::new(),
                date: rfc3339_nanos(SystemTime::now()),
                expiry: expiry.as_nanos() as u64,
                content_type: (!content_type.is_empty()).then(|| content_type.to_string()),
            },
        );
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Unable to create `{}`.", parent.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).ok();
            }
        }
        fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("Unable to write `{}`.", path.display()))
    }
}

fn rfc3339_nanos(time: SystemTime) -> String {
    let parts = crate::s3::utc_parts(time);
    let nanos = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{nanos:09}Z",
        parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_go_durations() {
        assert_eq!(parse_go_duration("168h").unwrap(), MAX_EXPIRY);
        assert_eq!(
            parse_go_duration("1h30m").unwrap(),
            Duration::from_secs(5400)
        );
        assert_eq!(
            parse_go_duration("1.5h").unwrap(),
            Duration::from_secs(5400)
        );
        assert_eq!(
            parse_go_duration("500ms").unwrap(),
            Duration::from_millis(500)
        );
        assert_eq!(
            parse_go_duration("2d").unwrap(),
            Duration::from_secs(172_800)
        );
        assert!(parse_go_duration("10").is_err());
        assert!(parse_go_duration("h").is_err());
        assert!(parse_go_duration("5x").is_err());
    }

    #[test]
    fn validates_expiry_range() {
        assert!(parse_expiry("7d").is_ok());
        assert!(
            parse_expiry("169h")
                .unwrap_err()
                .to_string()
                .contains("larger than 7 days")
        );
        assert!(
            parse_expiry("500ms")
                .unwrap_err()
                .to_string()
                .contains("lesser than 1 second")
        );
    }

    #[test]
    fn humanizes_like_mc() {
        assert_eq!(
            humanize_duration(MAX_EXPIRY),
            "7 days 0 hours 0 minutes 0 seconds"
        );
        assert_eq!(
            humanize_duration(Duration::from_secs(3661)),
            "1 hours 1 minutes 1 seconds"
        );
        assert_eq!(
            humanize_duration(Duration::from_secs(61)),
            "1 minutes 1 seconds"
        );
        assert_eq!(humanize_duration(Duration::from_secs(5)), "5 seconds");
        assert_eq!(
            humanize_duration(Duration::from_millis(5)),
            "5 milliseconds"
        );
    }

    #[test]
    fn builds_curl_command() {
        let fields = BTreeMap::from([
            ("bucket".to_string(), "b".to_string()),
            ("key".to_string(), "dir with space/".to_string()),
            ("policy".to_string(), "cG9s".to_string()),
        ]);
        assert_eq!(
            curl_command("http://h/b/", &fields, true),
            r"curl http://h/b/ -F bucket=b -F policy=cG9s -F key=dir\ with\ space/<NAME> -F file=@<FILE>"
        );
        assert!(
            curl_command("http://h/b/", &fields, false)
                .contains(r"-F key=dir\ with\ space/ -F file")
        );
    }

    #[test]
    fn share_db_round_trips_and_drops_expired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("share").join("downloads.json");
        let mut db = ShareDb::load(&path).unwrap();
        db.set(
            "http://h/b/o",
            "http://h/b/o?sig",
            Duration::from_secs(60),
            "",
        );
        db.shares.insert(
            "old".into(),
            ShareEntry {
                url: "http://h/b/old".into(),
                version_id: String::new(),
                date: "2020-01-01T00:00:00.000000000Z".into(),
                expiry: 1_000_000_000,
                content_type: Some("text/plain".into()),
            },
        );
        db.save(&path).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["version"], "1");
        assert_eq!(raw["shares"]["http://h/b/o?sig"]["share"], "http://h/b/o");
        assert_eq!(
            raw["shares"]["http://h/b/o?sig"]["expiry"],
            60_000_000_000u64
        );
        let loaded = ShareDb::load(&path).unwrap();
        assert_eq!(loaded.shares.len(), 1);
        let entry = &loaded.shares["http://h/b/o?sig"];
        let left = entry.time_left(SystemTime::now());
        assert!(left > Duration::from_secs(50) && left <= Duration::from_secs(60));
    }

    #[test]
    fn reads_mc_share_db() {
        let text = r#"{
	"version": "1",
	"shares": {
		"http://x/b/o?X-Amz-Signature=1": {
			"share": "http://x/b/o",
			"versionID": "",
			"date": "2999-01-01T10:00:00.123456789+02:00",
			"expiry": 604800000000000
		}
	}
}"#;
        let db: ShareDb = serde_json::from_str(text).unwrap();
        let entry = &db.shares["http://x/b/o?X-Amz-Signature=1"];
        assert_eq!(entry.time_left(SystemTime::now()), MAX_EXPIRY);
    }
}
