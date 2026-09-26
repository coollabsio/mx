//! `mx admin kms` (mc `admin kms`).
//!
//! Owner: SERVER. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct KmsArgs {
    #[command(subcommand)]
    pub command: KmsCommand,
}

#[derive(Debug, Subcommand)]
pub enum KmsCommand {
    #[command(
        name = "key",
        about = "manage KMS master keys: Request key status information"
    )]
    Key(KmsKeyArgs),
}

#[derive(Debug, Args)]
pub struct KmsKeyArgs {
    #[command(subcommand)]
    pub command: KmsKeyCommand,
}

#[derive(Debug, Subcommand)]
pub enum KmsKeyCommand {
    #[command(name = "create", about = "creates a new master KMS key")]
    Create(KmsKeyCreateArgs),
    #[command(
        name = "status",
        about = "request status information for a KMS master key"
    )]
    Status(KmsKeyStatusArgs),
    #[command(name = "list", about = "request list of KMS master keys")]
    List(KmsKeyListArgs),
}

#[derive(Debug, Args)]
pub struct KmsKeyCreateArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "KEY-NAME")]
    pub key_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct KmsKeyStatusArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "KEY-NAME")]
    pub key_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct KmsKeyListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

pub fn run(args: KmsArgs, json: bool) -> Result<()> {
    match args.command {
        KmsCommand::Key(args) => key(args, json),
    }
}

fn key(args: KmsKeyArgs, json: bool) -> Result<()> {
    match args.command {
        KmsKeyCommand::Create(args) => key_create(args, json),
        KmsKeyCommand::Status(args) => key_status(args, json),
        KmsKeyCommand::List(args) => key_list(args, json),
    }
}

fn key_create(args: KmsKeyCreateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin kms key create")
}

fn key_status(args: KmsKeyStatusArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin kms key status")
}

fn key_list(args: KmsKeyListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin kms key list")
}
