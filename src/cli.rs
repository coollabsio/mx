use crate::resolve::ResolveMapping;
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(about = "MaxIO Client", long_about = None)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[arg(long, global = true, value_name = "HOST:PORT=IP")]
    pub resolve: Vec<ResolveMapping>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(about = "manage server credentials in configuration file")]
    Alias(AliasArgs),
    #[command(about = "list buckets and objects")]
    Ls(LsArgs),
    #[command(about = "make bucket")]
    Mb(MakeBucketArgs),
    #[command(about = "remove bucket")]
    Rb(BucketTargetArgs),
    #[command(about = "show object or bucket information")]
    Stat(TargetArg),
    #[command(about = "print object contents to stdout")]
    Cat(TargetArg),
    #[command(about = "remove object")]
    Rm(RemoveArgs),
    #[command(about = "copy objects and files")]
    Cp(CopyArgs),
    #[command(about = "move object")]
    Mv(CopyArgs),
    #[command(visible_alias = "out", about = "upload object")]
    Put(PutArgs),
    #[command(about = "mirror a directory tree")]
    Mirror(MirrorArgs),
    #[command(about = "upload standard input to an object")]
    Pipe(PipeArgs),
}

#[derive(Debug, Args)]
pub struct AliasArgs {
    #[command(subcommand)]
    pub command: AliasCommand,
}

#[derive(Debug, Subcommand)]
pub enum AliasCommand {
    #[command(visible_alias = "s", about = "set a new alias to configuration file")]
    Set(AliasSetArgs),
    #[command(visible_alias = "ls", about = "list aliases in configuration file")]
    List(AliasListArgs),
    #[command(
        visible_alias = "rm",
        about = "remove an alias from configuration file"
    )]
    Remove(AliasRemoveArgs),
}

#[derive(Debug, Args)]
pub struct AliasSetArgs {
    pub alias: String,
    pub url: String,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    #[arg(long, default_value = "S3v4")]
    pub api: String,
    #[arg(long, default_value = "auto")]
    pub path: String,
}

#[derive(Debug, Args)]
pub struct AliasListArgs {
    pub alias: Option<String>,
}

#[derive(Debug, Args)]
pub struct AliasRemoveArgs {
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct LsArgs {
    pub target: String,
}

#[derive(Debug, Args)]
pub struct BucketTargetArgs {
    pub target: String,
}

#[derive(Debug, Args)]
pub struct MakeBucketArgs {
    #[arg(long)]
    pub ignore_existing: bool,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct TargetArg {
    pub target: String,
}

#[derive(Debug, Args)]
pub struct CopyArgs {
    #[arg(short = 'r', long)]
    pub recursive: bool,
    pub source: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    #[arg(short = 'r', long)]
    pub recursive: bool,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct PutArgs {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct PipeArgs {
    #[arg(long)]
    pub quiet: bool,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct MirrorArgs {
    #[arg(long)]
    pub remove: bool,
    pub source: String,
    pub target: String,
}
