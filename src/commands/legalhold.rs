//! `mx legalhold set|clear|info` (area G). Stub until implemented.

use crate::flags::{RewindFlag, VersionIdFlag, VersionsFlag};
use anyhow::{Result, bail};
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct LegalholdArgs {
    #[command(subcommand)]
    pub command: LegalholdCommand,
}

#[derive(Debug, Subcommand)]
pub enum LegalholdCommand {
    #[command(about = "set legal hold for object(s)")]
    Set(LegalholdTargetArgs),
    #[command(about = "clear legal hold for object(s)")]
    Clear(LegalholdTargetArgs),
    #[command(about = "show legal hold info for object(s)")]
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

pub fn run(args: LegalholdArgs, _json: bool) -> Result<()> {
    let name = match args.command {
        LegalholdCommand::Set(_) => "set",
        LegalholdCommand::Clear(_) => "clear",
        LegalholdCommand::Info(_) => "info",
    };
    bail!("`legalhold {name}` is not implemented yet.")
}
