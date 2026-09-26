//! `mx retention set|clear|info` (area G). Also hosts small helpers shared by the other
//! object-lock style commands (`legalhold`, `undo`, `ilm restore`).

use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use crate::flags::{RewindFlag, VersionIdFlag, VersionsFlag};
use crate::s3::lock::{self, Selection, ValidityUnit};
use crate::target::TargetRef;
use anyhow::{Result, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::types::ObjectLockRetentionMode;
use clap::{Args, Subcommand};
use serde::Serialize;
use std::time::{Duration, SystemTime};

#[derive(Debug, Args)]
pub struct RetentionArgs {
    #[command(subcommand)]
    pub command: RetentionCommand,
}

#[derive(Debug, Subcommand)]
pub enum RetentionCommand {
    #[command(about = "apply retention settings on object(s) or bucket")]
    Set(RetentionSetArgs),
    #[command(about = "clear retention for object(s) or bucket")]
    Clear(RetentionTargetArgs),
    #[command(about = "show retention for object(s) or bucket")]
    Info(RetentionTargetArgs),
}

#[derive(Debug, Args)]
pub struct RetentionSetArgs {
    /// GOVERNANCE or COMPLIANCE
    pub mode: String,
    /// retention validity, e.g. 30d or 1y
    pub validity: String,
    pub target: String,
    #[command(flatten)]
    pub common: RetentionCommonFlags,
    /// bypass governance
    #[arg(long)]
    pub bypass: bool,
}

#[derive(Debug, Args)]
pub struct RetentionTargetArgs {
    pub target: String,
    #[command(flatten)]
    pub common: RetentionCommonFlags,
}

#[derive(Debug, Args)]
pub struct RetentionCommonFlags {
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub version_id: VersionIdFlag,
    #[command(flatten)]
    pub rewind: RewindFlag,
    #[command(flatten)]
    pub versions: VersionsFlag,
    /// operate on the bucket default retention
    #[arg(long)]
    pub default: bool,
}

impl RetentionCommonFlags {
    fn check_default(&self, bypass: bool) -> Result<()> {
        if self.default
            && (self.version_id.version_id.is_some()
                || self.rewind.rewind.is_some()
                || self.versions.versions
                || self.recursive
                || bypass)
        {
            bail!(
                "--default cannot be specified with any of --version-id, --rewind, --versions, --recursive, --bypass."
            );
        }
        Ok(())
    }

    fn selection(&self) -> Result<Option<Selection>> {
        selection(
            self.recursive,
            &self.version_id,
            &self.rewind,
            &self.versions,
        )
    }
}

/// Selection for multi-object mode, or `None` for a single object (`--version-id` given, or
/// none of `-r/--versions/--rewind`). `--versions` without `--rewind` means all versions.
pub(crate) fn selection(
    recursive: bool,
    version_id: &VersionIdFlag,
    rewind: &RewindFlag,
    versions: &VersionsFlag,
) -> Result<Option<Selection>> {
    let rewind = rewind.at(SystemTime::now())?;
    if version_id.version_id.is_some() {
        if recursive || versions.versions || rewind.is_some() {
            bail!(
                "You cannot pass --version-id with any of --versions, --recursive and --rewind flags."
            );
        }
        return Ok(None);
    }
    if !recursive && !versions.versions && rewind.is_none() {
        return Ok(None);
    }
    Ok(Some(Selection {
        recursive,
        versions: versions.versions,
        rewind,
    }))
}

pub fn run(args: RetentionArgs, json: bool) -> Result<()> {
    match args.command {
        RetentionCommand::Set(args) => set(args, json),
        RetentionCommand::Clear(args) => clear(args, json),
        RetentionCommand::Info(args) => info(args, json),
    }
}

// ---------------------------------------------------------------------------
// shared helpers (also used by legalhold / undo / ilm restore)
// ---------------------------------------------------------------------------

/// Resolved S3 target for object-lock style commands.
pub(crate) struct LockTarget {
    pub alias_name: String,
    pub alias: AliasConfig,
    pub bucket: String,
    /// Object key or prefix (may be empty, keeps a trailing slash).
    pub key: String,
    pub target: TargetRef,
}

impl LockTarget {
    pub fn resolve(input: &str) -> Result<Self> {
        let store = ConfigStore::load_or_create()?;
        let (alias, target) = require_s3(&store, input)?;
        let bucket = target.require_bucket()?.to_string();
        Ok(Self {
            alias_name: target.alias.clone(),
            alias,
            bucket,
            key: target.key_with_trailing_slash().unwrap_or_default(),
            target,
        })
    }

    /// `ALIAS/BUCKET/KEY` (mc `urlJoinPath(alias, url)`).
    pub fn alias_path(&self, key: &str) -> String {
        format!("{}/{}/{}", self.alias_name, self.bucket, key)
    }

    /// Endpoint URL of an object (mc `ClientURL.String()`).
    pub fn object_url(&self, key: &str) -> String {
        format!(
            "{}/{}/{}",
            self.alias.url.trim_end_matches('/'),
            self.bucket,
            key
        )
    }

    /// Key relative to the target's parent "directory" (mc `prefixPath` trimming).
    pub fn relative_key(&self, key: &str) -> String {
        let path = if self.key.is_empty() {
            let slash = if self.target.trailing_slash { "/" } else { "" };
            format!("/{}{slash}", self.bucket)
        } else {
            format!("/{}/{}", self.bucket, self.key)
        };
        let prefix = &path[..path.rfind('/').map_or(0, |index| index + 1)];
        let full = format!("/{}/{}", self.bucket, key);
        full.strip_prefix(prefix).unwrap_or(&full).to_string()
    }

    pub fn require_key(&self) -> Result<()> {
        if self.key.is_empty() {
            bail!(
                "Target `{}/{}` is missing object key.",
                self.alias_name,
                self.bucket
            );
        }
        Ok(())
    }

    /// Fails with `message` unless object lock is enabled on the bucket.
    pub async fn require_lock_enabled(&self, client: &Client, message: &str) -> Result<()> {
        match lock::get_bucket_lock_config(client, &self.bucket).await {
            Ok(Some(config)) if config.status == "Enabled" => Ok(()),
            Ok(_) => bail!("{message}"),
            Err(error) if format!("{error:#}").contains("NotImplemented") => bail!("{message}"),
            Err(error) => Err(error),
        }
    }
}

/// mc `centerText`: pads `text` to `width` columns, centered.
pub(crate) fn center_text(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        return text.to_string();
    }
    let left = (width - len) / 2;
    format!(
        "{}{text}{}",
        " ".repeat(left),
        " ".repeat(width - len - left)
    )
}

/// RFC3339 like Go's `time.Time` JSON (zero time when unset).
pub(crate) fn rfc3339(time: Option<SystemTime>) -> String {
    time.and_then(|time| {
        crate::s3::from_system_time(time)
            .fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
            .ok()
    })
    .unwrap_or_else(|| "0001-01-01T00:00:00Z".to_string())
}

/// mc `humanizedDuration.StringShort`.
pub(crate) fn short_duration(duration: Duration) -> String {
    let millis = duration.as_millis();
    let secs = duration.as_secs();
    let (days, hours, minutes) = (secs / 86_400, (secs / 3600) % 24, (secs / 60) % 60);
    if millis < 1000 {
        format!("{millis} milliseconds")
    } else if secs < 60 {
        format!("{secs} seconds")
    } else if secs < 3600 {
        format!("{minutes} minutes")
    } else if secs < 86_400 {
        format!("{hours} hours {minutes} minutes")
    } else if days <= 2 {
        format!("{days} days, {hours} hours")
    } else {
        format!("{days} days")
    }
}

pub(crate) fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

const LOCK_UNSUPPORTED: &str = "does not support locking";

// ---------------------------------------------------------------------------
// set / clear
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Set,
    Clear,
}

impl Op {
    fn as_str(self) -> &'static str {
        match self {
            Op::Set => "set",
            Op::Clear => "clear",
        }
    }
}

#[derive(Serialize)]
struct RetentionMessage<'a> {
    op: &'static str,
    mode: &'a str,
    validity: &'a str,
    urlpath: String,
    #[serde(rename = "versionID")]
    version_id: &'a str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl RetentionMessage<'_> {
    fn text(&self) -> String {
        let mut msg = match &self.error {
            Some(error) => format!(
                "Unable to {} object retention on `{}`: {error}",
                self.op, self.urlpath
            ),
            None => {
                let ed = if self.op == "clear" { "ed" } else { "" };
                format!(
                    "Object retention successfully {}{ed} for `{}`",
                    self.op, self.urlpath
                )
            }
        };
        if !self.version_id.is_empty() {
            msg.push_str(&format!(" (version-id={})", self.version_id));
        }
        msg.push('.');
        msg
    }
}

#[derive(Serialize)]
struct BucketMessage<'a> {
    op: &'static str,
    enabled: &'a str,
    mode: &'a str,
    validity: String,
    status: &'static str,
}

fn print_bucket_message(message: &BucketMessage<'_>, json: bool) -> Result<()> {
    if json {
        return print_json(message);
    }
    if message.op == "clear" {
        println!("Object lock configuration cleared successfully.");
    } else if message.mode.is_empty() {
        println!("Object locking is not enabled.");
    } else {
        println!(
            "Object locking '{}' is configured for {}.",
            message.mode, message.validity
        );
    }
    Ok(())
}

fn set(args: RetentionSetArgs, json: bool) -> Result<()> {
    args.common.check_default(args.bypass)?;
    let mode = lock::parse_retention_mode(&args.mode)?;
    let (count, unit) = lock::parse_validity(&args.validity)?;
    let target = LockTarget::resolve(&args.target)?;
    if args.common.default {
        return apply_bucket_lock(&target, Op::Set, Some((mode, count, unit)), json);
    }
    let until = lock::retain_until(SystemTime::now(), count, unit)?;
    apply_retention(
        &target,
        &args.common,
        Op::Set,
        Some((mode, until)),
        &args.validity,
        args.bypass,
        json,
    )
}

fn clear(args: RetentionTargetArgs, json: bool) -> Result<()> {
    args.common.check_default(false)?;
    let target = LockTarget::resolve(&args.target)?;
    if args.common.default {
        return apply_bucket_lock(&target, Op::Clear, None, json);
    }
    apply_retention(&target, &args.common, Op::Clear, None, "", true, json)
}

fn apply_bucket_lock(
    target: &LockTarget,
    op: Op,
    rule: Option<(ObjectLockRetentionMode, u32, ValidityUnit)>,
    json: bool,
) -> Result<()> {
    if target.target.key.is_some() {
        bail!("--default requires a bucket target (ALIAS/BUCKET).");
    }
    runtime()?.block_on(async {
        let client = crate::s3::build_client(&target.alias).await?;
        let message = format!("Remote bucket `{}` {LOCK_UNSUPPORTED}", target.bucket);
        target.require_lock_enabled(&client, &message).await?;
        lock::put_bucket_lock_config(&client, &target.bucket, rule.clone()).await
    })?;
    let (mode, validity) = match &rule {
        Some((mode, count, unit)) => (mode.as_str(), format!("{count}{}", unit.as_str())),
        None => ("", "0".to_string()),
    };
    print_bucket_message(
        &BucketMessage {
            op: op.as_str(),
            enabled: "Enabled",
            mode,
            validity,
            status: "success",
        },
        json,
    )
}

/// Resolves the `(key, version id)` pairs a command operates on.
pub(crate) fn resolve_objects(
    rt: &tokio::runtime::Runtime,
    client: &Client,
    target: &LockTarget,
    selection: Option<&Selection>,
    version_id: Option<&str>,
) -> Result<Vec<(String, Option<String>)>> {
    match selection {
        None => {
            target.require_key()?;
            Ok(vec![(target.key.clone(), version_id.map(str::to_string))])
        }
        Some(selection) => Ok(rt
            .block_on(lock::select_objects(
                client,
                &target.bucket,
                &target.key,
                selection,
            ))?
            .into_iter()
            .map(|item| (item.key, item.version_id))
            .collect()),
    }
}

fn apply_retention(
    target: &LockTarget,
    flags: &RetentionCommonFlags,
    op: Op,
    retention: Option<(ObjectLockRetentionMode, SystemTime)>,
    validity: &str,
    bypass: bool,
    json: bool,
) -> Result<()> {
    let selection = flags.selection()?;
    let mode = retention
        .as_ref()
        .map(|(mode, _)| mode.as_str())
        .unwrap_or_default();
    let rt = runtime()?;
    let client = rt.block_on(crate::s3::build_client(&target.alias))?;
    let unsupported = format!("Remote bucket `{}` {LOCK_UNSUPPORTED}", target.bucket);
    rt.block_on(target.require_lock_enabled(&client, &unsupported))?;
    let objects = resolve_objects(
        &rt,
        &client,
        target,
        selection.as_ref(),
        flags.version_id.version_id.as_deref(),
    )?;
    if objects.is_empty() {
        bail!(
            "Unable to find any object/version to {} its retention.",
            op.as_str()
        );
    }

    let mut failed = 0;
    for (key, version_id) in &objects {
        let result = rt.block_on(lock::put_object_retention(
            &client,
            &target.bucket,
            key,
            version_id.as_deref(),
            retention.clone(),
            bypass,
        ));
        let message = RetentionMessage {
            op: op.as_str(),
            mode,
            validity,
            urlpath: target.alias_path(key),
            version_id: version_id.as_deref().unwrap_or_default(),
            status: if result.is_ok() { "success" } else { "failure" },
            error: result.as_ref().err().map(|error| format!("{error:#}")),
        };
        if json {
            print_json(&message)?;
        } else {
            println!("{}", message.text());
        }
        if result.is_err() {
            failed += 1;
        }
    }
    if failed > 0 {
        bail!(
            "Unable to {} retention on {failed} object(s)/version(s).",
            op.as_str()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// info
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct RetentionInfoMessage {
    mode: String,
    until: String,
    urlpath: String,
    #[serde(rename = "versionID")]
    version_id: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn info(args: RetentionTargetArgs, json: bool) -> Result<()> {
    args.common.check_default(false)?;
    let target = LockTarget::resolve(&args.target)?;
    let selection = args.common.selection()?;
    let rt = runtime()?;
    let client = rt.block_on(crate::s3::build_client(&target.alias))?;
    let unsupported = format!("Remote bucket `{}` {LOCK_UNSUPPORTED}", target.bucket);
    rt.block_on(target.require_lock_enabled(&client, &unsupported))?;

    if args.common.default || (selection.is_none() && target.key.is_empty()) {
        if target.target.key.is_some() {
            bail!("--default requires a bucket target (ALIAS/BUCKET).");
        }
        let config = rt.block_on(lock::get_bucket_lock_config(&client, &target.bucket))?;
        let status = config
            .as_ref()
            .map(|config| config.status.clone())
            .unwrap_or_default();
        let (mode, validity) = match config.and_then(|config| config.rule) {
            Some((mode, count, unit)) => (mode, format!("{count}{}", unit.as_str())),
            None => (String::new(), "0".to_string()),
        };
        return print_bucket_message(
            &BucketMessage {
                op: "info",
                enabled: &status,
                mode: &mode,
                validity,
                status: "success",
            },
            json,
        );
    }

    let list_style = selection.is_some();
    let objects = resolve_objects(
        &rt,
        &client,
        &target,
        selection.as_ref(),
        args.common.version_id.version_id.as_deref(),
    )?;
    if objects.is_empty() {
        bail!("Unable to find any object/version to show its retention.");
    }

    let now = SystemTime::now();
    let mut failed = 0;
    for (key, version_id) in objects {
        let result = rt.block_on(lock::get_object_retention(
            &client,
            &target.bucket,
            &key,
            version_id.as_deref(),
        ));
        let (mode, until) = match &result {
            Ok(Some((mode, until))) => (mode.clone(), *until),
            _ => (String::new(), None),
        };
        let message = RetentionInfoMessage {
            mode,
            until: rfc3339(until),
            urlpath: target.alias_path(&key),
            version_id: version_id.unwrap_or_default(),
            status: if result.is_ok() { "success" } else { "failure" },
            error: result.as_ref().err().map(|error| format!("{error:#}")),
        };
        if result.is_err() {
            failed += 1;
        }
        if json {
            print_json(&message)?;
        } else if list_style {
            println!("{}", info_list_line(&message, until, now));
        } else {
            println!("{}", info_record(&message, until, now));
        }
    }
    if failed > 0 {
        bail!("Unable to get retention of {failed} object(s)/version(s).");
    }
    Ok(())
}

fn info_list_line(
    message: &RetentionInfoMessage,
    until: Option<SystemTime>,
    now: SystemTime,
) -> String {
    if let Some(error) = &message.error {
        return format!(
            "Unable to get object retention on `{}`: {error}",
            message.urlpath
        );
    }
    let field = if message.mode.is_empty() {
        "NO RETENTION".to_string()
    } else {
        let expired = message.mode == "GOVERNANCE" && until.is_some_and(|until| now > until);
        format!("{} {}", message.mode, if expired { "EXPIRED" } else { "" })
    };
    let mut line = format!("[ {} ]  ", center_text(&field, 18));
    if !message.version_id.is_empty() {
        line.push_str(&format!("{}  ", message.version_id));
    }
    line.push_str(&message.urlpath);
    line
}

fn info_record(
    message: &RetentionInfoMessage,
    until: Option<SystemTime>,
    now: SystemTime,
) -> String {
    if let Some(error) = &message.error {
        return format!(
            "Unable to get object retention on `{}`: {error}",
            message.urlpath
        );
    }
    let mut out = format!("Name    : {}\n", message.urlpath);
    if !message.version_id.is_empty() {
        out.push_str(&format!("Version : {}\n", message.version_id));
    }
    out.push_str("Mode    : ");
    if message.mode.is_empty() {
        out.push_str("NO RETENTION");
    } else {
        out.push_str(&message.mode);
        if let Some(until) = until {
            match now.duration_since(until) {
                Ok(ago) => out.push_str(&format!(", expired {} ago", short_duration(ago))),
                Err(_) => out.push_str(&format!(
                    ", expiring in {}",
                    short_duration(until.duration_since(now).unwrap_or_default())
                )),
            }
        }
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn centers_text_like_mc() {
        assert_eq!(center_text("ON", 8), "   ON   ");
        assert_eq!(center_text("OFF", 8), "  OFF   ");
        assert_eq!(center_text("NO RETENTION", 8), "NO RETENTION");
    }

    #[test]
    fn formats_short_durations() {
        assert_eq!(
            short_duration(Duration::from_millis(250)),
            "250 milliseconds"
        );
        assert_eq!(short_duration(Duration::from_secs(42)), "42 seconds");
        assert_eq!(short_duration(Duration::from_secs(125)), "2 minutes");
        assert_eq!(
            short_duration(Duration::from_secs(3 * 3600 + 120)),
            "3 hours 2 minutes"
        );
        assert_eq!(
            short_duration(Duration::from_secs(2 * 86_400 + 3600)),
            "2 days, 1 hours"
        );
        assert_eq!(short_duration(Duration::from_secs(30 * 86_400)), "30 days");
    }

    #[test]
    fn formats_info_messages() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let until = Some(now + Duration::from_secs(10 * 86_400 + 60));
        let message = RetentionInfoMessage {
            mode: "GOVERNANCE".into(),
            until: rfc3339(until),
            urlpath: "local/b/k".into(),
            version_id: "v1".into(),
            status: "success",
            error: None,
        };
        assert_eq!(
            info_record(&message, until, now),
            "Name    : local/b/k\nVersion : v1\nMode    : GOVERNANCE, expiring in 10 days\n"
        );
        assert_eq!(
            info_list_line(&message, until, now),
            "[    GOVERNANCE      ]  v1  local/b/k"
        );
        let expired = Some(now - Duration::from_secs(5));
        assert!(info_list_line(&message, expired, now).contains("GOVERNANCE EXPIRED"));
        let none = RetentionInfoMessage {
            mode: String::new(),
            until: rfc3339(None),
            urlpath: "local/b/k".into(),
            version_id: String::new(),
            status: "success",
            error: None,
        };
        assert_eq!(none.until, "0001-01-01T00:00:00Z");
        assert_eq!(
            info_list_line(&none, None, now),
            "[    NO RETENTION    ]  local/b/k"
        );
        assert_eq!(
            info_record(&none, None, now),
            "Name    : local/b/k\nMode    : NO RETENTION\n"
        );
    }

    #[test]
    fn formats_set_messages() {
        let mut message = RetentionMessage {
            op: "clear",
            mode: "",
            validity: "",
            urlpath: "local/b/k".into(),
            version_id: "v1",
            status: "success",
            error: None,
        };
        assert_eq!(
            message.text(),
            "Object retention successfully cleared for `local/b/k` (version-id=v1)."
        );
        message.op = "set";
        message.version_id = "";
        message.error = Some("AccessDenied".into());
        assert_eq!(
            message.text(),
            "Unable to set object retention on `local/b/k`: AccessDenied."
        );
    }

    #[test]
    fn relative_keys_follow_mc_prefix_trimming() {
        let make = |input: &str| {
            let target = TargetRef::parse(input).unwrap();
            LockTarget {
                alias_name: "local".into(),
                alias: AliasConfig {
                    url: "http://127.0.0.1:9000/".into(),
                    ..Default::default()
                },
                bucket: target.bucket.clone().unwrap(),
                key: target.key_with_trailing_slash().unwrap_or_default(),
                target,
            }
        };
        let object = make("local/b/dir/obj.txt");
        assert_eq!(object.relative_key("dir/obj.txt"), "obj.txt");
        assert_eq!(
            object.object_url("dir/obj.txt"),
            "http://127.0.0.1:9000/b/dir/obj.txt"
        );
        assert_eq!(object.alias_path("dir/obj.txt"), "local/b/dir/obj.txt");
        let dir = make("local/b/dir/");
        assert_eq!(dir.relative_key("dir/sub/x"), "sub/x");
        let bucket = make("local/b/");
        assert_eq!(bucket.relative_key("dir/x"), "dir/x");
        let bare = make("local/b");
        assert_eq!(bare.relative_key("dir/x"), "b/dir/x");
    }
}
