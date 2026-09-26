//! `mx admin top` (mc `admin top`).
//!
//! Owner: STREAM. Stubs return "not implemented yet" until implemented.

use crate::commands::admin::deprecated;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct TopArgs {
    #[command(subcommand)]
    pub command: TopCommand,
}

#[derive(Debug, Subcommand)]
pub enum TopCommand {
    #[command(
        name = "api",
        hide = true,
        about = "summarize API events on MinIO server in real-time"
    )]
    Api(TopApiArgs),
    #[command(
        name = "locks",
        about = "get a list of the 10 oldest locks on a MinIO cluster."
    )]
    Locks(TopLocksArgs),
}

#[derive(Debug, Args)]
pub struct TopApiArgs {
    /// Accepted and ignored (deprecated command).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct TopLocksArgs {
    /// Accepted and ignored (deprecated command).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub args: Vec<String>,
}

pub fn run(args: TopArgs, json: bool) -> Result<()> {
    match args.command {
        TopCommand::Api(args) => api(args, json),
        TopCommand::Locks(args) => locks(args, json),
    }
}

fn api(_args: TopApiArgs, _json: bool) -> Result<()> {
    deprecated("support top api")
}

fn locks(_args: TopLocksArgs, _json: bool) -> Result<()> {
    deprecated("support top locks")
}
