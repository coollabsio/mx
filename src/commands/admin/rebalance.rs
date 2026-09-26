//! `mx admin rebalance` (mc `admin rebalance`).
//!
//! Owner: TOPO. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct RebalanceArgs {
    #[command(subcommand)]
    pub command: RebalanceCommand,
}

#[derive(Debug, Subcommand)]
pub enum RebalanceCommand {
    #[command(name = "start", about = "start rebalance operation")]
    Start(RebalanceStartArgs),
    #[command(name = "status", about = "summarize an ongoing rebalance operation")]
    Status(RebalanceStatusArgs),
    #[command(name = "stop", about = "stop an ongoing rebalance operation")]
    Stop(RebalanceStopArgs),
}

#[derive(Debug, Args)]
pub struct RebalanceStartArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct RebalanceStatusArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct RebalanceStopArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
}

pub fn run(args: RebalanceArgs, json: bool) -> Result<()> {
    match args.command {
        RebalanceCommand::Start(args) => start(args, json),
        RebalanceCommand::Status(args) => status(args, json),
        RebalanceCommand::Stop(args) => stop(args, json),
    }
}

fn start(args: RebalanceStartArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin rebalance start")
}

fn status(args: RebalanceStatusArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin rebalance status")
}

fn stop(args: RebalanceStopArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin rebalance stop")
}
