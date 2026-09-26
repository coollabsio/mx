//! `mx ilm restore` (area G): RestoreObject for transitioned objects, then waits until the
//! restored copies are available (like `mc ilm restore`).
//! Wired from `ilm.rs` as `IlmCommand::Restore`; area G edits only this file.

use crate::commands::retention::{LockTarget, print_json};
use crate::commands::runtime;
use crate::flags::{VersionIdFlag, VersionsFlag};
use crate::s3::lock::{self, Selection};
use anyhow::{Result, bail};
use clap::Args;
use std::time::Duration;

/// Poll interval while waiting for restores to finish (mc uses 5s).
const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Args)]
#[command(mut_args(|a| match a.get_id().as_str() {
    "versions" => a.help("apply on versions"),
    "version_id" => a.help("select a specific version id"),
    _ => a,
}))]
pub struct IlmRestoreArgs {
    pub target: String,
    /// keep the restored copy for N days
    #[arg(long, default_value_t = 1)]
    pub days: i32,
    /// apply recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub versions: VersionsFlag,
    #[command(flatten)]
    pub version_id: VersionIdFlag,
}

fn validate(args: &IlmRestoreArgs) -> Result<()> {
    if args.days <= 0 {
        bail!("--days should be equal or greater than 1");
    }
    if args.version_id.version_id.is_some() && (args.recursive || args.versions.versions) {
        bail!("You cannot combine --version-id with --recursive or --versions flags.");
    }
    if args.versions.versions && !args.recursive {
        bail!("--versions requires --recursive.");
    }
    Ok(())
}

pub fn run(args: IlmRestoreArgs, json: bool) -> Result<()> {
    validate(&args)?;
    let target = LockTarget::resolve(&args.target)?;
    if !args.recursive {
        target.require_key()?;
    }
    let rt = runtime()?;
    let client = rt.block_on(crate::s3::build_client(&target.alias))?;
    let objects: Vec<(String, Option<String>)> = if args.recursive {
        let selection = Selection {
            recursive: true,
            versions: args.versions.versions,
            rewind: None,
        };
        rt.block_on(lock::select_objects(
            &client,
            &target.bucket,
            &target.key,
            &selection,
        ))?
        .into_iter()
        .map(|item| (item.key, item.version_id))
        .collect()
    } else {
        vec![(target.key.clone(), args.version_id.version_id.clone())]
    };

    let mut failed = 0;
    let mut sent = Vec::new();
    for (key, version_id) in objects {
        match rt.block_on(lock::restore_object(
            &client,
            &target.bucket,
            &key,
            version_id.as_deref(),
            args.days,
        )) {
            Ok(()) => sent.push((key, version_id)),
            Err(error) => {
                crate::output::print_error(&error.context("Unable to send restore request."));
                failed += 1;
            }
        }
    }
    if !json {
        println!("Sent restore requests to {} object(s)", sent.len());
    }

    let mut restored = 0;
    for (key, version_id) in &sent {
        loop {
            match rt.block_on(lock::restore_ongoing(
                &client,
                &target.bucket,
                key,
                version_id.as_deref(),
            )) {
                Ok(Some(true)) => std::thread::sleep(POLL_INTERVAL),
                Ok(Some(false)) => {
                    restored += 1;
                    break;
                }
                Ok(None) => {
                    crate::output::error_if(
                        "Unable to check for restore status",
                        &format!(
                            "`{}` did not receive restore request",
                            target.alias_path(key)
                        ),
                    );
                    failed += 1;
                    break;
                }
                Err(error) => {
                    crate::output::print_error(
                        &error.context("Unable to check for restore status"),
                    );
                    failed += 1;
                    break;
                }
            }
        }
    }
    if json {
        #[derive(serde::Serialize)]
        struct IlmRestoreMessage {
            status: &'static str,
            restored: usize,
        }
        print_json(&IlmRestoreMessage {
            status: if failed > 0 { "failure" } else { "success" },
            restored: sent.len(),
        })?;
    } else {
        println!("{restored}/{} object(s) successfully restored", sent.len());
    }
    if failed > 0 {
        bail!("Unable to restore {failed} object(s).");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(days: i32, recursive: bool, versions: bool, vid: Option<&str>) -> IlmRestoreArgs {
        IlmRestoreArgs {
            target: "local/b/k".into(),
            days,
            recursive,
            version_id: VersionIdFlag {
                version_id: vid.map(str::to_string),
            },
            versions: VersionsFlag { versions },
        }
    }

    #[test]
    fn validates_flags() {
        assert!(validate(&args(1, false, false, None)).is_ok());
        assert!(validate(&args(0, false, false, None)).is_err());
        assert!(validate(&args(1, true, false, Some("v"))).is_err());
        assert!(validate(&args(1, false, true, None)).is_err());
        assert!(validate(&args(1, true, true, None)).is_ok());
        assert!(validate(&args(3, false, false, Some("v"))).is_ok());
    }
}
