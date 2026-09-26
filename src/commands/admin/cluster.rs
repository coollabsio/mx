//! `mx admin cluster` (mc `admin cluster`).
//!
//! Owner: SERVER. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct ClusterArgs {
    #[command(subcommand)]
    pub command: ClusterCommand,
}

#[derive(Debug, Subcommand)]
pub enum ClusterCommand {
    #[command(name = "bucket", about = "manage bucket metadata on MinIO cluster")]
    Bucket(ClusterBucketArgs),
    #[command(name = "iam", about = "manage IAM info on MinIO cluster")]
    Iam(ClusterIamArgs),
}

#[derive(Debug, Args)]
pub struct ClusterBucketArgs {
    #[command(subcommand)]
    pub command: ClusterBucketCommand,
}

#[derive(Debug, Subcommand)]
pub enum ClusterBucketCommand {
    #[command(name = "import", about = "restore bucket metadata from a zip file")]
    Import(ClusterBucketImportArgs),
    #[command(name = "export", about = "backup bucket metadata to a zip file")]
    Export(ClusterBucketExportArgs),
}

#[derive(Debug, Args)]
pub struct ClusterBucketImportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "FILE")]
    pub file: String,
}

#[derive(Debug, Args)]
pub struct ClusterBucketExportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ClusterIamArgs {
    #[command(subcommand)]
    pub command: ClusterIamCommand,
}

#[derive(Debug, Subcommand)]
pub enum ClusterIamCommand {
    #[command(name = "import", about = "imports IAM info from zipped file")]
    Import(ClusterIamImportArgs),
    #[command(name = "export", about = "exports IAM info to zipped file")]
    Export(ClusterIamExportArgs),
}

#[derive(Debug, Args)]
pub struct ClusterIamImportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "FILE")]
    pub file: String,
}

#[derive(Debug, Args)]
pub struct ClusterIamExportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(
        long = "output",
        short = 'o',
        value_name = "VALUE",
        help = "output iam export to a custom file path"
    )]
    pub output: Option<String>,
}

pub fn run(args: ClusterArgs, json: bool) -> Result<()> {
    match args.command {
        ClusterCommand::Bucket(args) => bucket(args, json),
        ClusterCommand::Iam(args) => iam(args, json),
    }
}

fn bucket(args: ClusterBucketArgs, json: bool) -> Result<()> {
    match args.command {
        ClusterBucketCommand::Import(args) => bucket_import(args, json),
        ClusterBucketCommand::Export(args) => bucket_export(args, json),
    }
}

fn bucket_import(args: ClusterBucketImportArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin cluster bucket import")
}

fn bucket_export(args: ClusterBucketExportArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin cluster bucket export")
}

fn iam(args: ClusterIamArgs, json: bool) -> Result<()> {
    match args.command {
        ClusterIamCommand::Import(args) => iam_import(args, json),
        ClusterIamCommand::Export(args) => iam_export(args, json),
    }
}

fn iam_import(args: ClusterIamImportArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin cluster iam import")
}

fn iam_export(args: ClusterIamExportArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin cluster iam export")
}
