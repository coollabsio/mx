//! `version enable|suspend|info` (mc version), including MinIO's excluded prefixes and
//! excluded folders.

use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::flags::TargetArg;
use crate::output;
use crate::s3::VersioningInfo;
use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct VersionArgs {
    #[command(subcommand)]
    pub command: VersionCommand,
}

#[derive(Debug, Subcommand)]
pub enum VersionCommand {
    #[command(about = "enable bucket versioning")]
    Enable(VersionEnableArgs),
    #[command(about = "suspend bucket versioning")]
    Suspend(TargetArg),
    #[command(about = "show bucket versioning status")]
    Info(TargetArg),
}

#[derive(Debug, Args)]
pub struct VersionEnableArgs {
    /// exclude versioning on these prefix patterns (comma separated, repeatable; MinIO only)
    #[arg(long = "excluded-prefixes", value_name = "PREFIXES")]
    pub excluded_prefixes: Vec<String>,
    /// exclude versioning on folder objects (MinIO only)
    #[arg(long)]
    pub exclude_folders: bool,
    pub target: String,
}

pub fn run(command: VersionCommand, json: bool) -> Result<()> {
    match command {
        VersionCommand::Enable(args) => {
            let info = VersioningInfo {
                status: "Enabled".to_string(),
                excluded_prefixes: args
                    .excluded_prefixes
                    .iter()
                    .flat_map(|value| value.split(','))
                    .filter(|prefix| !prefix.is_empty())
                    .map(str::to_string)
                    .collect(),
                exclude_folders: args.exclude_folders,
                ..Default::default()
            };
            set(&args.target, "enable", info, json)
        }
        VersionCommand::Suspend(args) => {
            let info = VersioningInfo {
                status: "Suspended".to_string(),
                ..Default::default()
            };
            set(&args.target, "suspend", info, json)
        }
        VersionCommand::Info(args) => show(&args.target, json),
    }
}

#[derive(Serialize)]
struct VersionMessage<'a> {
    #[serde(rename = "Op")]
    op: &'a str,
    status: &'a str,
    url: &'a str,
    versioning: &'a VersioningInfo,
}

fn print_json(op: &str, target: &str, info: &VersioningInfo) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&VersionMessage {
            op,
            status: "success",
            url: target,
            versioning: info,
        })?
    );
    Ok(())
}

fn set(target_arg: &str, op: &str, info: VersioningInfo, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, target_arg)?;
    let bucket = target.require_bucket()?.to_string();
    let rt = runtime()?;
    if info.excluded_prefixes.is_empty() && !info.exclude_folders {
        rt.block_on(crate::s3::set_versioning(
            &alias,
            &bucket,
            info.status == "Enabled",
        ))?;
    } else {
        rt.block_on(crate::s3::put_versioning_info(&alias, &bucket, &info))?;
    }
    if json {
        print_json(op, target_arg, &info)
    } else {
        output::print_plain(&format!(
            "{target_arg} versioning is {}",
            info.status.to_ascii_lowercase()
        ));
        Ok(())
    }
}

fn show(target_arg: &str, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, target_arg)?;
    let bucket = target.require_bucket()?.to_string();
    let info = runtime()?.block_on(crate::s3::get_versioning_info(&alias, &bucket))?;
    if json {
        return print_json("info", target_arg, &info);
    }
    println!("{}", info_text(target_arg, &info));
    Ok(())
}

/// mc `versioningInfoMessage.String()`.
pub fn info_text(target: &str, info: &VersioningInfo) -> String {
    if info.status.is_empty() {
        format!("{target} is un-versioned")
    } else {
        format!(
            "{target} versioning is {}",
            info.status.to_ascii_lowercase()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_text_matches_mc() {
        let mut info = VersioningInfo::default();
        assert_eq!(info_text("a/b", &info), "a/b is un-versioned");
        info.status = "Suspended".into();
        assert_eq!(info_text("a/b", &info), "a/b versioning is suspended");
    }

    #[test]
    fn json_message_uses_mc_field_names() {
        let info = VersioningInfo {
            status: "Enabled".into(),
            excluded_prefixes: vec!["x/".into()],
            exclude_folders: true,
            ..Default::default()
        };
        let value = serde_json::to_value(VersionMessage {
            op: "info",
            status: "success",
            url: "a/b",
            versioning: &info,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"Op":"info","status":"success","url":"a/b","versioning":{
                "status":"Enabled","MFADelete":"","ExcludedPrefixes":["x/"],"ExcludeFolders":true}})
        );
    }
}
