//! `mx ilm tier add|edit|ls|info|check|rm` (area H, MinIO admin API). Stub until implemented.
//! Wired from `ilm.rs` as `IlmCommand::Tier`; area H edits only this file.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct IlmTierArgs {
    #[command(subcommand)]
    pub command: IlmTierCommand,
}

#[derive(Debug, Subcommand)]
pub enum IlmTierCommand {
    #[command(about = "add a new remote tier target")]
    Add(IlmTierAddArgs),
    #[command(about = "update an existing remote tier target")]
    Edit(IlmTierNameArgs),
    #[command(visible_alias = "list", about = "list remote tier targets")]
    Ls(IlmTierAliasArgs),
    #[command(about = "show tier statistics")]
    Info(IlmTierInfoArgs),
    #[command(about = "check tier configuration")]
    Check(IlmTierNameArgs),
    #[command(visible_alias = "remove", about = "remove a tier")]
    Rm(IlmTierNameArgs),
}

#[derive(Debug, Args)]
pub struct IlmTierAddArgs {
    /// tier type: minio, s3, azure, gcs
    pub tier_type: String,
    pub alias: String,
    pub name: String,
}

#[derive(Debug, Args)]
pub struct IlmTierNameArgs {
    pub alias: String,
    pub name: String,
}

#[derive(Debug, Args)]
pub struct IlmTierAliasArgs {
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct IlmTierInfoArgs {
    pub alias: String,
    pub name: Option<String>,
}

pub fn run(args: IlmTierArgs, _json: bool) -> Result<()> {
    let name = match args.command {
        IlmTierCommand::Add(_) => "add",
        IlmTierCommand::Edit(_) => "edit",
        IlmTierCommand::Ls(_) => "ls",
        IlmTierCommand::Info(_) => "info",
        IlmTierCommand::Check(_) => "check",
        IlmTierCommand::Rm(_) => "rm",
    };
    bail!("`ilm tier {name}` is not implemented yet.")
}
