//! `mx retention set|clear|info` (area G). Stub until implemented.

use crate::flags::{RewindFlag, VersionIdFlag, VersionsFlag};
use anyhow::{Result, bail};
use clap::{Args, Subcommand};

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

pub fn run(args: RetentionArgs, _json: bool) -> Result<()> {
    let name = match args.command {
        RetentionCommand::Set(_) => "set",
        RetentionCommand::Clear(_) => "clear",
        RetentionCommand::Info(_) => "info",
    };
    bail!("`retention {name}` is not implemented yet.")
}
