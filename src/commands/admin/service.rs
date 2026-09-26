//! `mx admin service` (mc `admin service`).
//!
//! Owner: SERVER. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceCommand,
}

#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    #[command(name = "restart", about = "restart a MinIO cluster")]
    Restart(ServiceRestartArgs),
    #[command(name = "stop", hide = true, about = "stop a MinIO cluster")]
    Stop(ServiceStopArgs),
    #[command(name = "unfreeze", about = "unfreeze S3 API calls on MinIO cluster")]
    Unfreeze(ServiceUnfreezeArgs),
    #[command(
        name = "freeze",
        hide = true,
        about = "freeze S3 API calls on MinIO cluster"
    )]
    Freeze(ServiceFreezeArgs),
}

#[derive(Debug, Args)]
pub struct ServiceRestartArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(
        long = "dry-run",
        help = "do not attempt a restart, however verify the peer status"
    )]
    pub dry_run: bool,
    #[arg(
        long = "wait",
        short = 'w',
        help = "wait for background initializations to complete"
    )]
    pub wait: bool,
}

#[derive(Debug, Args)]
pub struct ServiceStopArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ServiceUnfreezeArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ServiceFreezeArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

pub fn run(args: ServiceArgs, json: bool) -> Result<()> {
    match args.command {
        ServiceCommand::Restart(args) => restart(args, json),
        ServiceCommand::Stop(args) => stop(args, json),
        ServiceCommand::Unfreeze(args) => unfreeze(args, json),
        ServiceCommand::Freeze(args) => freeze(args, json),
    }
}

fn restart(args: ServiceRestartArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin service restart")
}

fn stop(args: ServiceStopArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin service stop")
}

fn unfreeze(args: ServiceUnfreezeArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin service unfreeze")
}

fn freeze(args: ServiceFreezeArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin service freeze")
}
