//! `mx batch` (mc `batch`).
//!
//! Owner: JOBS. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct BatchArgs {
    #[command(subcommand)]
    pub command: BatchCommand,
}

#[derive(Debug, Subcommand)]
pub enum BatchCommand {
    #[command(name = "generate", about = "generate a new batch job definition")]
    Generate(BatchGenerateArgs),
    #[command(name = "start", about = "start a new batch job")]
    Start(BatchStartArgs),
    #[command(
        name = "list",
        visible_alias = "ls",
        about = "list all current batch jobs"
    )]
    List(BatchListArgs),
    #[command(
        name = "status",
        about = "summarize job events on MinIO server in real-time"
    )]
    Status(BatchStatusArgs),
    #[command(name = "describe", about = "describe job definition for a job")]
    Describe(BatchDescribeArgs),
    #[command(name = "cancel", about = "cancel ongoing batch job")]
    Cancel(BatchCancelArgs),
}

#[derive(Debug, Args)]
pub struct BatchGenerateArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "JOBTYPE")]
    pub jobtype: String,
}

#[derive(Debug, Args)]
pub struct BatchStartArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "JOBFILE")]
    pub jobfile: String,
}

#[derive(Debug, Args)]
pub struct BatchListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(
        long = "type",
        value_name = "VALUE",
        help = "list all current batch jobs via job type"
    )]
    pub type_: Option<String>,
}

#[derive(Debug, Args)]
pub struct BatchStatusArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "JOBID")]
    pub jobid: String,
}

#[derive(Debug, Args)]
pub struct BatchDescribeArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "JOBID")]
    pub jobid: String,
}

#[derive(Debug, Args)]
pub struct BatchCancelArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "JOBID")]
    pub jobid: Option<String>,
    #[arg(long = "id", value_name = "VALUE", help = "job id")]
    pub id: Option<String>,
}

pub fn run(args: BatchArgs, json: bool) -> Result<()> {
    match args.command {
        BatchCommand::Generate(args) => generate(args, json),
        BatchCommand::Start(args) => start(args, json),
        BatchCommand::List(args) => list(args, json),
        BatchCommand::Status(args) => status(args, json),
        BatchCommand::Describe(args) => describe(args, json),
        BatchCommand::Cancel(args) => cancel(args, json),
    }
}

fn generate(args: BatchGenerateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("batch generate")
}

fn start(args: BatchStartArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("batch start")
}

fn list(args: BatchListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("batch list")
}

fn status(args: BatchStatusArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("batch status")
}

fn describe(args: BatchDescribeArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("batch describe")
}

fn cancel(args: BatchCancelArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("batch cancel")
}
