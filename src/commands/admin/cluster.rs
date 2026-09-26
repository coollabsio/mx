//! `mx admin cluster bucket|iam import|export` (mc `admin cluster`): bucket metadata and IAM
//! zip archives, saved under mc's file names (`ALIAS-bucket-metadata.zip`,
//! `ALIAS-iam-info.zip`; an existing file is moved aside with a timestamp suffix).
//!
//! Owner: SERVER.

use crate::commands::runtime;
use crate::s3::admin_server::{self as api, BucketStatus, IamEntities, IamErrEntity};
use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Args)]
pub struct ClusterArgs {
    #[command(subcommand)]
    pub command: ClusterCommand,
}

#[derive(Debug, Subcommand)]
pub enum ClusterCommand {
    #[command(name = "bucket", about = "manage bucket metadata on MinIO cluster")]
    Bucket(ClusterBucketArgs),
    #[command(name = "iam", about = "manage IAM info on MinIO cluster")]
    Iam(ClusterIamArgs),
}

#[derive(Debug, Args)]
pub struct ClusterBucketArgs {
    #[command(subcommand)]
    pub command: ClusterBucketCommand,
}

#[derive(Debug, Subcommand)]
pub enum ClusterBucketCommand {
    #[command(name = "import", about = "restore bucket metadata from a zip file")]
    Import(ClusterBucketImportArgs),
    #[command(name = "export", about = "backup bucket metadata to a zip file")]
    Export(ClusterBucketExportArgs),
}

#[derive(Debug, Args)]
pub struct ClusterBucketImportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "FILE")]
    pub file: String,
}

#[derive(Debug, Args)]
pub struct ClusterBucketExportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ClusterIamArgs {
    #[command(subcommand)]
    pub command: ClusterIamCommand,
}

#[derive(Debug, Subcommand)]
pub enum ClusterIamCommand {
    #[command(name = "import", about = "imports IAM info from zipped file")]
    Import(ClusterIamImportArgs),
    #[command(name = "export", about = "exports IAM info to zipped file")]
    Export(ClusterIamExportArgs),
}

#[derive(Debug, Args)]
pub struct ClusterIamImportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "FILE")]
    pub file: String,
}

#[derive(Debug, Args)]
pub struct ClusterIamExportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(
        long = "output",
        short = 'o',
        value_name = "VALUE",
        help = "output iam export to a custom file path"
    )]
    pub output: Option<String>,
}

pub fn run(args: ClusterArgs, json: bool) -> Result<()> {
    match args.command {
        ClusterCommand::Bucket(args) => match args.command {
            ClusterBucketCommand::Import(args) => bucket_import(args, json),
            ClusterBucketCommand::Export(args) => bucket_export(args, json),
        },
        ClusterCommand::Iam(args) => match args.command {
            ClusterIamCommand::Import(args) => iam_import(args, json),
            ClusterIamCommand::Export(args) => iam_export(args, json),
        },
    }
}

const CLIENT_ERROR: &str = "Unable to initialize admin client.";

/// Go `filepath.Clean` (lexical).
fn clean_path(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let rooted = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// mc `url2Alias`: the path after the alias.
fn bucket_of(aliased_url: &str) -> &str {
    aliased_url
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or_default()
}

/// Go `*fs.PathError` for `open PATH` (JSON detail `{"Op","Path","Err":errno}`).
fn open_error(path: &str, err: &std::io::Error) -> anyhow::Error {
    let detail = crate::detail![
        ("Op", "open"),
        ("Path", path),
        ("Err", err.raw_os_error().unwrap_or_default())
    ];
    anyhow::Error::new(crate::error::McError::with_detail(
        format!("open {path}: {}", api::go_errno_text(err)),
        detail,
    ))
}

/// minio/pkg `console.Infof`: `PROG: MESSAGE` on stdout.
fn info_line(text: &str) {
    println!("{}: {text}", crate::output::prog_name());
}

/// mc `dateTimeFormatFilename` (`2006-01-02T15-04-05.999999-07-00`), in UTC.
fn backup_suffix() -> String {
    let now = std::time::SystemTime::now();
    let micros = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_micros())
        .unwrap_or_default();
    let base = crate::commands::util::format_print_time(now);
    // `2006-01-02 15:04:05 UTC`
    let stamp = base
        .trim_end_matches(" UTC")
        .replace(' ', "T")
        .replace(':', "-");
    let frac = format!("{micros:06}");
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{stamp}+00-00")
    } else {
        format!("{stamp}.{frac}+00-00")
    }
}

/// Saves `data` at `path` like mc: an existing file is moved to `PATH.<timestamp>`, the new
/// file gets mode 0600.
fn save(path: &str, data: &[u8]) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_dir() => {
            // mc `moveFile`: copy into a new file (default mode), then remove the original.
            let backup = format!("{path}.{}", backup_suffix());
            std::fs::read(path)
                .and_then(|data| std::fs::write(&backup, data))
                .and_then(|_| std::fs::remove_file(path))
                .map_err(|err| anyhow!(api::go_errno_text(&err)))
                .with_context(|| format!("Unable to create a backup of {path}"))?;
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(anyhow!(api::go_errno_text(&err)).context("Unable to download file data"));
        }
    }
    std::fs::write(path, data)
        .map_err(|err| open_error(path, &err))
        .context("Unable to rename downloaded data")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|err| anyhow!(api::go_errno_text(&err)))
            .with_context(|| format!("Unable to set file permissions for {path}"))?;
    }
    Ok(())
}

/// JSON line mc prints for exports (`json.Marshal`, never indented).
#[derive(Serialize)]
struct FileMessage<'a> {
    file: &'a str,
}

fn print_saved(path: &str, text: &str, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            crate::output::format_json(&FileMessage { file: path }, true)?
        );
    } else {
        info_line(text);
    }
    Ok(())
}

/// Reads an archive and validates it like Go `zip.NewReader`.
fn read_zip(path: &str, open_message: &str) -> Result<Vec<u8>> {
    let data = std::fs::read(path)
        .map_err(|err| open_error(path, &err))
        .context(open_message.to_string())?;
    if !valid_zip(&data) {
        return Err(
            anyhow!("zip: not a valid zip file").context(format!("Unable to read zip file {path}"))
        );
    }
    Ok(data)
}

/// Finds the end-of-central-directory record and checks the directory it points to.
fn valid_zip(data: &[u8]) -> bool {
    const EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const CDH: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    if data.len() < 22 {
        return false;
    }
    let search_from = data.len().saturating_sub(22 + 65_535);
    let Some(pos) = (search_from..=data.len() - 22)
        .rev()
        .find(|&i| data[i..i + 4] == EOCD)
    else {
        return false;
    };
    let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]) as usize;
    let u32_at =
        |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
    let records = u16_at(pos + 10);
    let size = u32_at(pos + 12);
    let offset = u32_at(pos + 16);
    if offset.checked_add(size).is_none_or(|end| end > pos) {
        return false;
    }
    records == 0 || data.get(offset..offset + 4) == Some(&CDH[..])
}

// ---------------------------------------------------------------------------
// bucket metadata
// ---------------------------------------------------------------------------

fn bucket_export(args: ClusterBucketExportArgs, json: bool) -> Result<()> {
    let client = api::admin_client(&args.target, CLIENT_ERROR)?;
    let aliased = clean_path(&args.target);
    let bucket = bucket_of(&aliased).to_string();
    let data = runtime()?
        .block_on(api::export_bucket_metadata(&client, &bucket))
        .context("Unable to export bucket metadata.")?;
    let bucket = if bucket.is_empty() { "bucket" } else { &bucket };
    let path = format!("{aliased}-{bucket}-metadata.zip");
    if let Some(dir) = Path::new(&path)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
    {
        std::fs::create_dir_all(dir)
            .map_err(|err| anyhow!(api::go_errno_text(&err)))
            .context("Unable to create download directory")?;
    }
    save(&path, &data)?;
    print_saved(
        &path,
        &format!("Bucket metadata successfully downloaded as {path}"),
        json,
    )
}

fn bucket_import(args: ClusterBucketImportArgs, json: bool) -> Result<()> {
    let data = read_zip(&args.file, "Unable to get bucket metadata")?;
    let client = api::admin_client(&args.target, CLIENT_ERROR)?;
    let aliased = clean_path(&args.target);
    let result = runtime()?
        .block_on(api::import_bucket_metadata(
            &client,
            bucket_of(&aliased),
            data,
        ))
        .context("Unable to import bucket metadata.")?;
    if json {
        println!("{}", api::encoder_json(&result.buckets, &["import"])?);
        return Ok(());
    }
    let text = render_bucket_import(&result.buckets.unwrap_or_default());
    println!("{}", text.strip_suffix('\n').unwrap_or(&text));
    Ok(())
}

fn status_tick(status: &api::MetaStatus) -> &'static str {
    if !status.err.is_empty() {
        "✗ "
    } else if !status.is_set {
        " "
    } else {
        "✔ "
    }
}

/// mc `importMetaMsg.String()` (buckets with errors sorted by name).
fn render_bucket_import(buckets: &std::collections::BTreeMap<String, BucketStatus>) -> String {
    let errors = buckets.values().filter(|s| s.has_error()).count();
    let mut out = format!(
        "\n{}/{} buckets were imported successfully.\n",
        buckets.len() - errors,
        buckets.len()
    );
    if errors == 0 {
        return out;
    }
    out.push_str("Errors: \n\n");
    for (bucket, status) in buckets.iter().filter(|(_, s)| s.has_error()) {
        out.push_str(&format!("{:<10}: {bucket}\n", "Name"));
        if !status.err.is_empty() {
            out.push_str(&format!("  Error:  {}\n", status.err));
        }
        let parts = [
            ("Object lock: ", &status.object_lock),
            ("Versioning: ", &status.versioning),
            ("Encryption: ", &status.sse_config),
            ("Lifecycle: ", &status.lifecycle),
            ("Notification: ", &status.notification),
            ("Quota: ", &status.quota),
            ("Policy: ", &status.policy),
            ("Tagging: ", &status.tagging),
            ("CORS: ", &status.cors),
        ];
        for (name, part) in parts {
            if part.is_set {
                out.push_str(&format!("  {name} {}\n", status_tick(part)));
            }
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// IAM
// ---------------------------------------------------------------------------

fn iam_export(args: ClusterIamExportArgs, json: bool) -> Result<()> {
    let aliased = clean_path(&args.target);
    let client = api::admin_client(&aliased, CLIENT_ERROR)?;
    let data = runtime()?
        .block_on(api::export_iam(&client))
        .context("Unable to export IAM info.")?;
    let path = match args.output.as_deref().filter(|o| !o.is_empty()) {
        Some(output) => output.to_string(),
        None => format!("{aliased}-iam-info.zip"),
    };
    save(&path, &data)?;
    print_saved(
        &path,
        &format!("IAM info successfully downloaded as {path}"),
        json,
    )
}

fn iam_import(args: ClusterIamImportArgs, json: bool) -> Result<()> {
    let aliased = clean_path(&args.target);
    let data = read_zip(&args.file, "Unable to get IAM info")?;
    let client = api::admin_client(&aliased, CLIENT_ERROR)?;
    let rt = runtime()?;
    match rt.block_on(api::import_iam_v2(&client, data.clone())) {
        Ok(result) => {
            if json {
                return crate::output::print_json(&result);
            }
            println!("{}", render_iam_import(&result));
        }
        Err(_) => {
            rt.block_on(api::import_iam(&client, data))
                .context("Unable to import IAM info.")?;
            if !json {
                info_line(&format!(
                    "IAM info imported to {aliased} from {}",
                    args.file
                ));
            }
        }
    }
    Ok(())
}

/// mc `processIAMEntities`.
fn iam_entities(entities: &IamEntities, action: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |what: &str, names: &[String]| {
        if !names.is_empty() {
            out.push(format!("{action} {what}: {}", names.join(", ")));
        }
    };
    push("policies", &entities.policies);
    push("users", &entities.users);
    push("groups", &entities.groups);
    push("service accounts", &entities.service_accounts);
    let keys = |maps: &[std::collections::BTreeMap<String, Vec<String>>]| -> Vec<String> {
        maps.iter().flat_map(|m| m.keys().cloned()).collect()
    };
    push("policies for users", &keys(&entities.user_policies));
    push("policies for groups", &keys(&entities.group_policies));
    push("policies for sts", &keys(&entities.sts_policies));
    out
}

/// mc `processErrIAMEntities`.
fn iam_failures(failed: &api::IamErrEntities) -> Vec<String> {
    let names =
        |list: &[IamErrEntity]| -> Vec<String> { list.iter().map(|e| e.name.clone()).collect() };
    [
        ("policies", &failed.policies),
        ("users", &failed.users),
        ("groups", &failed.groups),
        ("service accounts", &failed.service_accounts),
        ("policies for users", &failed.user_policies),
        ("policies for groups", &failed.group_policies),
        ("policies for sts", &failed.sts_policies),
    ]
    .into_iter()
    .filter(|(_, list)| !list.is_empty())
    .map(|(what, list)| format!("Failed to add {what}: {}", names(list).join(", ")))
    .collect()
}

/// mc `iamImportInfo.String()`.
fn render_iam_import(result: &api::ImportIamResult) -> String {
    let mut lines = iam_entities(&result.skipped, "Skipped");
    lines.extend(iam_entities(&result.removed, "Removed"));
    lines.extend(iam_entities(&result.added, "Added"));
    lines.extend(iam_failures(&result.failed));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_paths_like_go() {
        assert_eq!(clean_path("e/"), "e");
        assert_eq!(clean_path("e//b/./c/../"), "e/b");
        assert_eq!(clean_path("/a/../.."), "/");
        assert_eq!(bucket_of("e/b"), "b");
        assert_eq!(bucket_of("e"), "");
    }

    #[test]
    fn validates_zip_archives() {
        // Empty archive: just the end-of-central-directory record.
        let mut empty = vec![0x50, 0x4b, 0x05, 0x06];
        empty.extend([0u8; 18]);
        assert!(valid_zip(&empty));
        assert!(!valid_zip(b"not a zip at all, definitely not"));
        assert!(!valid_zip(b""));
    }

    #[test]
    fn renders_import_reports() {
        let buckets: std::collections::BTreeMap<String, BucketStatus> = serde_json::from_str(
            r#"{"b1":{"olock":{"isSet":false},"versioning":{"isSet":true}},"b2":{"versioning":{"isSet":true,"error":"boom"},"policy":{"isSet":true}}}"#,
        )
        .unwrap();
        assert_eq!(
            render_bucket_import(&buckets),
            "\n1/2 buckets were imported successfully.\nErrors: \n\nName      : b2\n  Versioning:  ✗ \n  Policy:  ✔ \n\n"
        );
        let result: api::ImportIamResult = serde_json::from_str(
            r#"{"skipped":{},"removed":{},"added":{"policies":["p1","p2"],"userPolicies":[{"u1":["p1"]}]},"failed":{"users":[{"name":"u2"}]}}"#,
        )
        .unwrap();
        assert_eq!(
            render_iam_import(&result),
            "Added policies: p1, p2\nAdded policies for users: u1\nFailed to add users: u2"
        );
        assert_eq!(
            serde_json::to_string(&api::ImportIamResult::default()).unwrap(),
            r#"{"skipped":{},"removed":{},"added":{},"failed":{}}"#
        );
    }
}
