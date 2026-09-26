use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::flags::TargetArg;
use crate::output;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct VersionArgs {
    #[command(subcommand)]
    pub command: VersionCommand,
}

#[derive(Debug, Subcommand)]
pub enum VersionCommand {
    #[command(about = "enable bucket versioning")]
    Enable(TargetArg),
    #[command(about = "suspend bucket versioning")]
    Suspend(TargetArg),
    #[command(about = "show bucket versioning")]
    Info(TargetArg),
}

pub fn run(command: VersionCommand, json: bool) -> Result<()> {
    match command {
        VersionCommand::Enable(args) => set(args, true, json),
        VersionCommand::Suspend(args) => set(args, false, json),
        VersionCommand::Info(args) => info(args, json),
    }
}

fn set(args: TargetArg, enabled: bool, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?.block_on(crate::s3::set_versioning(&alias, &bucket, enabled))?;
    let status = if enabled { "Enabled" } else { "Suspended" };
    if json {
        println!(r#"{{"status":"success","versioning":"{status}"}}"#);
    } else {
        output::print_plain(&format!("Bucket versioning is now `{status}`."));
    }
    Ok(())
}

fn info(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let status = runtime()?.block_on(crate::s3::get_versioning(&alias, &bucket))?;
    if json {
        println!(r#"{{"status":"success","versioning":"{status}"}}"#);
    } else {
        println!("{status}");
    }
    Ok(())
}
