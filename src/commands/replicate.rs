//! `mx replicate ...` (area H, MinIO bucket replication). Stub until implemented.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct ReplicateArgs {
    #[command(subcommand)]
    pub command: ReplicateCommand,
}

#[derive(Debug, Subcommand)]
pub enum ReplicateCommand {
    #[command(about = "add a server side replication configuration rule")]
    Add(ReplicateTargetArgs),
    #[command(about = "modify an existing server side replication configuration rule")]
    Update(ReplicateTargetArgs),
    #[command(
        visible_alias = "list",
        about = "list server side replication configuration rules"
    )]
    Ls(ReplicateTargetArgs),
    #[command(about = "show server side replication status")]
    Status(ReplicateTargetArgs),
    #[command(about = "re-replicate objects")]
    Resync(ReplicateTargetArgs),
    #[command(about = "export server side replication configuration")]
    Export(ReplicateTargetArgs),
    #[command(about = "import server side replication configuration in JSON format")]
    Import(ReplicateTargetArgs),
    #[command(
        visible_alias = "remove",
        about = "remove a server side replication configuration rule"
    )]
    Rm(ReplicateTargetArgs),
    #[command(about = "show unreplicated object versions")]
    Backlog(ReplicateTargetArgs),
}

#[derive(Debug, Args)]
pub struct ReplicateTargetArgs {
    pub target: String,
}

pub fn run(args: ReplicateArgs, _json: bool) -> Result<()> {
    let name = match args.command {
        ReplicateCommand::Add(_) => "add",
        ReplicateCommand::Update(_) => "update",
        ReplicateCommand::Ls(_) => "ls",
        ReplicateCommand::Status(_) => "status",
        ReplicateCommand::Resync(_) => "resync",
        ReplicateCommand::Export(_) => "export",
        ReplicateCommand::Import(_) => "import",
        ReplicateCommand::Rm(_) => "rm",
        ReplicateCommand::Backlog(_) => "backlog",
    };
    bail!("`replicate {name}` is not implemented yet.")
}
