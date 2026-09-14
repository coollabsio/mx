pub mod alias;
pub mod cat;
pub mod cp;
pub mod ls;
pub mod mb;
pub mod mirror;
pub mod mv;
pub mod put;
pub mod rb;
pub mod rm;
pub mod stat;

use crate::config::ConfigStore;
use crate::config::model::AliasConfig;

use crate::cli::{Cli, Commands};
use anyhow::{Result, anyhow};

pub fn alias_config(store: &ConfigStore, alias: &str) -> Result<AliasConfig> {
    store
        .config()
        .aliases
        .get(alias)
        .cloned()
        .ok_or_else(|| anyhow!("No such alias `{alias}` found."))
}

pub fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Runtime::new()?)
}

pub fn run(cli: Cli) -> Result<()> {
    let json = cli.json;

    match cli.command {
        Commands::Alias(args) => alias::run(args.command, json),
        Commands::Ls(args) => ls::run(args, json),
        Commands::Mb(args) => mb::run(args, json),
        Commands::Rb(args) => rb::run(args, json),
        Commands::Stat(args) => stat::run(args, json),
        Commands::Cat(args) => cat::run(args, json),
        Commands::Rm(args) => rm::run(args, json),
        Commands::Cp(args) => cp::run(args, json),
        Commands::Mv(args) => mv::run(args, json),
        Commands::Put(args) => put::run(args, json),
        Commands::Mirror(args) => mirror::run(args, json),
    }
}
