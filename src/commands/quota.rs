//! `mx quota set|info|clear` (area H, MinIO admin API). Stub until implemented.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct QuotaArgs {
    #[command(subcommand)]
    pub command: QuotaCommand,
}

#[derive(Debug, Subcommand)]
pub enum QuotaCommand {
    #[command(about = "set bucket quota")]
    Set(QuotaSetArgs),
    #[command(about = "show bucket quota")]
    Info(QuotaTargetArgs),
    #[command(about = "clear bucket quota")]
    Clear(QuotaTargetArgs),
}

#[derive(Debug, Args)]
pub struct QuotaSetArgs {
    pub target: String,
    /// quota size, e.g. 1GiB
    #[arg(long)]
    pub size: Option<String>,
}

#[derive(Debug, Args)]
pub struct QuotaTargetArgs {
    pub target: String,
}

pub fn run(args: QuotaArgs, _json: bool) -> Result<()> {
    let name = match args.command {
        QuotaCommand::Set(_) => "set",
        QuotaCommand::Info(_) => "info",
        QuotaCommand::Clear(_) => "clear",
    };
    bail!("`quota {name}` is not implemented yet.")
}
