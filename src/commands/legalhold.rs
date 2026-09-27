//! `mx legalhold set|clear|info` (area G).

use crate::commands::retention::{LockTarget, center_text, print_json, resolve_objects, selection};
use crate::commands::runtime;
use crate::error::nonfatal;
use crate::flags::{RewindFlag, VersionIdFlag, VersionsFlag};
use crate::s3::lock;
use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct LegalholdArgs {
    #[command(subcommand)]
    pub command: LegalholdCommand,
}

#[derive(Debug, Subcommand)]
pub enum LegalholdCommand {
    #[command(
        about = "set legal hold for object(s)",
        mut_args(|a| match a.get_id().as_str() {
            "recursive" => a.help("apply legal hold recursively"),
            "version_id" => a.help("apply legal hold to a specific object version"),
            "rewind" => a.help("apply legal hold on an object version at specified time"),
            "versions" => a.help("apply legal hold on multiple versions of an object"),
            _ => a,
        })
    )]
    Set(LegalholdTargetArgs),
    #[command(
        about = "clear legal hold for object(s)",
        mut_args(|a| match a.get_id().as_str() {
            "recursive" => a.help("clear legal hold recursively"),
            "version_id" => a.help("clear legal hold of a specific object version"),
            "rewind" => a.help("clear legal hold on an object version at specified time"),
            "versions" => a.help("clear legal hold on multiple versions of object(s)"),
            _ => a,
        })
    )]
    Clear(LegalholdTargetArgs),
    #[command(
        about = "show legal hold info for object(s)",
        mut_args(|a| match a.get_id().as_str() {
            "recursive" => a.help("show legal hold status recursively"),
            "version_id" => a.help("show legal hold status of a specific object version"),
            "rewind" => a.help("show legal hold status of an object version at specified time"),
            "versions" => a.help("show legal hold status of multiple versions of object(s)"),
            _ => a,
        })
    )]
    Info(LegalholdTargetArgs),
}

#[derive(Debug, Args)]
pub struct LegalholdTargetArgs {
    pub target: String,
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub version_id: VersionIdFlag,
    #[command(flatten)]
    pub rewind: RewindFlag,
    #[command(flatten)]
    pub versions: VersionsFlag,
}

/// mc `legalHoldCmdMessage` / `legalHoldInfoMessage` (same JSON shape).
#[derive(Serialize)]
struct LegalHoldMessage {
    legalhold: String,
    urlpath: String,
    key: String,
    #[serde(rename = "versionID")]
    version_id: String,
    status: &'static str,
}

impl LegalHoldMessage {
    fn set_text(&self) -> String {
        let op = if self.legalhold == "OFF" {
            "cleared"
        } else {
            "set"
        };
        let mut msg = format!("Object legal hold successfully {op} for `{}`", self.key);
        if !self.version_id.is_empty() {
            msg.push_str(&format!(" (version-id={})", self.version_id));
        }
        msg.push('.');
        msg
    }

    fn info_text(&self) -> String {
        let status = if self.legalhold.is_empty() {
            "Not set"
        } else {
            &self.legalhold
        };
        let mut msg = format!("[ {} ] ", center_text(status, 8));
        if !self.version_id.is_empty() {
            msg.push_str(&format!(" {} ", self.version_id));
        }
        msg.push(' ');
        msg.push_str(&self.key);
        msg
    }
}

pub fn run(args: LegalholdArgs, json: bool) -> Result<()> {
    match args.command {
        LegalholdCommand::Set(args) => apply(args, Some(true), json),
        LegalholdCommand::Clear(args) => apply(args, Some(false), json),
        LegalholdCommand::Info(args) => apply(args, None, json),
    }
}

/// `hold = Some(on)` sets/clears the legal hold, `None` shows it.
fn apply(args: LegalholdTargetArgs, hold: Option<bool>, json: bool) -> Result<()> {
    // mc takes the multi-object path only with --recursive or --versions (a lone --rewind
    // still addresses the single latest object).
    let selection = selection(
        args.recursive,
        &args.version_id,
        &args.rewind,
        &args.versions,
    )?
    .filter(|selection| selection.recursive || selection.versions);
    let target = LockTarget::resolve(&args.target)?;
    let rt = runtime()?;
    let client = rt.block_on(crate::s3::build_client(&target.alias))?;
    let enabled = rt
        .block_on(target.lock_enabled(&client))
        .with_context(|| match hold {
            Some(true) => format!("Unable to set legalhold on `{}`", args.target),
            Some(false) => format!("Unable to clear legalhold of `{}`", args.target),
            None => format!("Unable to get legalhold info of `{}`", args.target),
        })?;
    if !enabled {
        if hold == Some(false) {
            bail!("Bucket locking needs to be enabled in order to use this feature.");
        }
        bail!("Bucket lock needs to be enabled in order to use this feature.");
    }
    let objects = resolve_objects(
        &rt,
        &client,
        &target,
        selection.as_ref(),
        args.version_id.version_id.as_deref(),
    )?;

    let single = selection.is_none();
    let mut failed = false;
    for (key, version_id) in &objects {
        let result = match hold {
            Some(on) => rt
                .block_on(lock::put_object_legal_hold(
                    &client,
                    &target.bucket,
                    key,
                    version_id.as_deref(),
                    on,
                ))
                .map(|_| Some(if on { "ON" } else { "OFF" }.to_string())),
            None => rt.block_on(lock::get_object_legal_hold(
                &client,
                &target.bucket,
                key,
                version_id.as_deref(),
            )),
        };
        let legalhold = match result {
            Ok(legalhold) => legalhold.unwrap_or_default(),
            // mc: one object without --recursive/--versions: `info` errors are fatal.
            Err(err) if single && hold.is_none() => {
                return Err(err).context(format!(
                    "Failed to show legal hold information of `{}`.",
                    args.target
                ));
            }
            // Otherwise each failure is reported (errorIf) and the command still exits 0.
            Err(err) => {
                crate::output::print_error(&err.context(nonfatal(failure_text(
                    hold.is_some(),
                    single,
                    &args.target,
                    &format!("/{}/{key}", target.bucket),
                ))));
                failed = true;
                continue;
            }
        };
        let message = LegalHoldMessage {
            legalhold,
            urlpath: target.object_url(key),
            key: target.relative_key(key),
            version_id: version_id.clone().unwrap_or_default(),
            status: "success",
        };
        if json {
            print_json(&message)?;
        } else if hold.is_some() {
            println!("{}", message.set_text());
        } else {
            println!("{}", message.info_text());
        }
    }
    if !single
        && !json
        && let Some(summary) =
            summary_text(hold.is_some(), objects.is_empty(), failed, &args.target)
    {
        println!("{summary}");
    }
    Ok(())
}

/// mc's per-object error message (`urlpath` is the object's URL path, `/BUCKET/KEY`).
fn failure_text(set: bool, single: bool, target: &str, urlpath: &str) -> String {
    match (set, single) {
        (true, true) => format!("Failed to set legal hold on `{target}` successfully"),
        (true, false) => format!("Failed to set legal hold on `{urlpath}` successfully"),
        (false, _) => format!("Failed to get legal hold information on `{urlpath}`"),
    }
}

/// mc's closing line of a multi-object text run (note mc's space before the newline).
fn summary_text(set: bool, empty: bool, failed: bool, target: &str) -> Option<String> {
    match (set, empty, failed) {
        (true, true, _) => Some(format!(
            "No objects/versions found while setting legal hold on `{target}`. "
        )),
        (false, true, _) => Some(format!(
            "No objects/versions found while getting legal hold status with prefix `{target}`. "
        )),
        (false, false, true) => Some(format!(
            "Errors found while getting legal hold status on objects with prefix `{target}`. "
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(legalhold: &str, version_id: &str) -> LegalHoldMessage {
        LegalHoldMessage {
            legalhold: legalhold.into(),
            urlpath: "http://127.0.0.1:9000/b/dir/k".into(),
            key: "k".into(),
            version_id: version_id.into(),
            status: "success",
        }
    }

    #[test]
    fn formats_like_mc() {
        assert_eq!(
            message("ON", "").set_text(),
            "Object legal hold successfully set for `k`."
        );
        assert_eq!(
            message("OFF", "v1").set_text(),
            "Object legal hold successfully cleared for `k` (version-id=v1)."
        );
        assert_eq!(message("ON", "").info_text(), "[    ON    ]  k");
        assert_eq!(message("", "").info_text(), "[  Not set ]  k");
        assert_eq!(message("OFF", "v1").info_text(), "[    OFF   ]  v1  k");
        let json = serde_json::to_value(message("ON", "v1")).unwrap();
        assert_eq!(json["legalhold"], "ON");
        assert_eq!(json["versionID"], "v1");
        assert_eq!(json["key"], "k");
    }

    #[test]
    fn multi_object_messages_match_mc() {
        assert_eq!(
            failure_text(true, false, "a/b/dir/", "/b/dir/k"),
            "Failed to set legal hold on `/b/dir/k` successfully"
        );
        assert_eq!(
            failure_text(true, true, "a/b/k", "/b/k"),
            "Failed to set legal hold on `a/b/k` successfully"
        );
        assert_eq!(
            failure_text(false, false, "a/b/", "/b/k"),
            "Failed to get legal hold information on `/b/k`"
        );
        assert_eq!(
            summary_text(true, true, false, "a/b/x").unwrap(),
            "No objects/versions found while setting legal hold on `a/b/x`. "
        );
        assert_eq!(
            summary_text(false, true, false, "a/b/x").unwrap(),
            "No objects/versions found while getting legal hold status with prefix `a/b/x`. "
        );
        assert_eq!(
            summary_text(false, false, true, "a/b/").unwrap(),
            "Errors found while getting legal hold status on objects with prefix `a/b/`. "
        );
        // Per-object `set`/`clear` failures only print their own errors.
        assert_eq!(summary_text(true, false, true, "a/b/"), None);
        assert_eq!(summary_text(false, false, false, "a/b/"), None);
    }
}
