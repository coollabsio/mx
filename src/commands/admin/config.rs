//! `mx admin config` (mc `admin config`).
//!
//! Owner: SERVER. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    #[command(name = "get", about = "interactively retrieve a config key parameters")]
    Get(ConfigGetArgs),
    #[command(name = "set", about = "interactively set a config key parameters")]
    Set(ConfigSetArgs),
    #[command(name = "reset", about = "interactively reset a config key parameters")]
    Reset(ConfigResetArgs),
    #[command(name = "history", about = "show all historic configuration changes")]
    History(ConfigHistoryArgs),
    #[command(
        name = "restore",
        about = "rollback back changes to a specific config history"
    )]
    Restore(ConfigRestoreArgs),
    #[command(name = "export", about = "export all config keys to STDOUT")]
    Export(ConfigExportArgs),
    #[command(name = "import", about = "import multiple config keys from STDIN")]
    Import(ConfigImportArgs),
}

#[derive(Debug, Args)]
pub struct ConfigGetArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "KEYS")]
    pub keys: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ConfigSetArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "ARGS")]
    pub args: Vec<String>,
    #[arg(long = "env", help = "list all the env only help")]
    pub env: bool,
}

#[derive(Debug, Args)]
pub struct ConfigResetArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "CONFIG-KEYS")]
    pub config_keys: Vec<String>,
    #[arg(long = "env", help = "list all the env only help")]
    pub env: bool,
}

#[derive(Debug, Args)]
pub struct ConfigHistoryArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(
        long = "count",
        short = 'n',
        default_value_t = 10,
        value_name = "VALUE",
        help = "list only last 'n' entries"
    )]
    pub count: i64,
    #[arg(long = "clear", short = 'c', help = "clear all history")]
    pub clear: bool,
}

#[derive(Debug, Args)]
pub struct ConfigRestoreArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "RESTOREID")]
    pub restoreid: String,
}

#[derive(Debug, Args)]
pub struct ConfigExportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ConfigImportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

pub fn run(args: ConfigArgs, json: bool) -> Result<()> {
    match args.command {
        ConfigCommand::Get(args) => get(args, json),
        ConfigCommand::Set(args) => set(args, json),
        ConfigCommand::Reset(args) => reset(args, json),
        ConfigCommand::History(args) => history(args, json),
        ConfigCommand::Restore(args) => restore(args, json),
        ConfigCommand::Export(args) => export(args, json),
        ConfigCommand::Import(args) => import(args, json),
    }
}

fn get(args: ConfigGetArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin config get")
}

fn set(args: ConfigSetArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin config set")
}

fn reset(args: ConfigResetArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin config reset")
}

fn history(args: ConfigHistoryArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin config history")
}

fn restore(args: ConfigRestoreArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin config restore")
}

fn export(args: ConfigExportArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin config export")
}

fn import(args: ConfigImportArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin config import")
}
