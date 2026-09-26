//! `mx event add|rm|ls` (area G). Stub until implemented.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct EventArgs {
    #[command(subcommand)]
    pub command: EventCommand,
}

#[derive(Debug, Subcommand)]
pub enum EventCommand {
    #[command(about = "add a new bucket notification")]
    Add(EventAddArgs),
    #[command(visible_alias = "remove", about = "remove a bucket notification")]
    Rm(EventRmArgs),
    #[command(visible_alias = "list", about = "list bucket notifications")]
    Ls(EventLsArgs),
}

#[derive(Debug, Args)]
pub struct EventAddArgs {
    pub target: String,
    pub arn: String,
    /// filter specific type of events: put, delete, get, ...
    #[arg(long, default_value = "put,delete,get")]
    pub event: String,
    #[arg(long)]
    pub prefix: Option<String>,
    #[arg(long)]
    pub suffix: Option<String>,
    #[arg(short = 'p', long)]
    pub ignore_existing: bool,
}

#[derive(Debug, Args)]
pub struct EventRmArgs {
    pub target: String,
    pub arn: Option<String>,
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub event: Option<String>,
    #[arg(long)]
    pub prefix: Option<String>,
    #[arg(long)]
    pub suffix: Option<String>,
}

#[derive(Debug, Args)]
pub struct EventLsArgs {
    pub target: String,
    pub arn: Option<String>,
}

pub fn run(args: EventArgs, _json: bool) -> Result<()> {
    let name = match args.command {
        EventCommand::Add(_) => "add",
        EventCommand::Rm(_) => "rm",
        EventCommand::Ls(_) => "ls",
    };
    bail!("`event {name}` is not implemented yet.")
}
