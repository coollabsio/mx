//! `mx admin decommission` (mc `admin decommission`).
//!
//! Owner: TOPO. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct DecommissionArgs {
    #[command(subcommand)]
    pub command: DecommissionCommand,
}

#[derive(Debug, Subcommand)]
pub enum DecommissionCommand {
    #[command(name = "start", about = "start decommissioning a pool")]
    Start(DecommissionStartArgs),
    #[command(name = "status", about = "show current decommissioning status")]
    Status(DecommissionStatusArgs),
    #[command(name = "cancel", about = "cancel an ongoing decommissioning of a pool")]
    Cancel(DecommissionCancelArgs),
}

#[derive(Debug, Args)]
pub struct DecommissionStartArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POOL")]
    pub pool: String,
}

#[derive(Debug, Args)]
pub struct DecommissionStatusArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POOL")]
    pub pool: Option<String>,
}

#[derive(Debug, Args)]
pub struct DecommissionCancelArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POOL")]
    pub pool: Option<String>,
}

pub fn run(args: DecommissionArgs, json: bool) -> Result<()> {
    match args.command {
        DecommissionCommand::Start(args) => start(args, json),
        DecommissionCommand::Status(args) => status(args, json),
        DecommissionCommand::Cancel(args) => cancel(args, json),
    }
}

fn start(args: DecommissionStartArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin decommission start")
}

fn status(args: DecommissionStatusArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin decommission status")
}

fn cancel(args: DecommissionCancelArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin decommission cancel")
}
