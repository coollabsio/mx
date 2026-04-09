use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "mx", bin_name = "mx")]
#[command(about = "MaxIO Client", long_about = None)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(about = "manage server credentials in configuration file")]
    Alias(AliasArgs),
    #[command(about = "list buckets and objects")]
    Ls(LsArgs),
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
