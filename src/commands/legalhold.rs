//! `mx legalhold set|clear|info` (area G).

use crate::commands::retention::{LockTarget, center_text, print_json, resolve_objects, selection};
use crate::commands::runtime;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl LegalHoldMessage {
    fn set_text(&self) -> String {
        if let Some(error) = &self.error {
            return format!(
                "Unable to set object legal hold status `{}`. {error}",
                self.key
            );
        }
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
        if let Some(error) = &self.error {
            return format!(
                "Unable to get object legal hold status `{}`. {error}",
                self.key
            );
        }
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
    let selection = selection(
        args.recursive,
        &args.version_id,
        &args.rewind,
        &args.versions,
    )?;
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
    let verb = if hold.is_some() { "setting" } else { "getting" };
    if objects.is_empty() {
        bail!(
            "No objects/versions found while {verb} legal hold on `{}`.",
            args.target
        );
    }

    let mut failed = 0;
    for (key, version_id) in objects {
        let result = match hold {
            Some(on) => rt
                .block_on(lock::put_object_legal_hold(
                    &client,
                    &target.bucket,
                    &key,
                    version_id.as_deref(),
                    on,
                ))
                .map(|_| Some(if on { "ON" } else { "OFF" }.to_string())),
            None => rt.block_on(lock::get_object_legal_hold(
                &client,
                &target.bucket,
                &key,
                version_id.as_deref(),
            )),
        };
        let message = LegalHoldMessage {
            legalhold: result.as_ref().ok().cloned().flatten().unwrap_or_default(),
            urlpath: target.object_url(&key),
            key: target.relative_key(&key),
            version_id: version_id.unwrap_or_default(),
            status: if result.is_ok() { "success" } else { "failure" },
            error: result.as_ref().err().map(|error| format!("{error:#}")),
        };
        if result.is_err() {
            failed += 1;
        }
        if json {
            print_json(&message)?;
        } else if hold.is_some() {
            println!("{}", message.set_text());
        } else {
            println!("{}", message.info_text());
        }
    }
    if failed > 0 {
        bail!("Errors found while {verb} legal hold on {failed} object(s)/version(s).");
    }
    Ok(())
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
            error: None,
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
        assert_eq!(message("", "").info_text(), "[ Not set  ]  k");
        assert_eq!(message("OFF", "v1").info_text(), "[   OFF    ]  v1  k");
        let json = serde_json::to_value(message("ON", "v1")).unwrap();
        assert_eq!(json["legalhold"], "ON");
        assert_eq!(json["versionID"], "v1");
        assert_eq!(json["key"], "k");
        assert!(json.get("error").is_none());
    }
}
