pub mod alias;
pub mod anonymous;
pub mod cat;
pub mod cors;
pub mod cp;
pub mod diff;
pub mod du;
pub mod encrypt;
pub mod find;
pub mod get;
pub mod head;
pub mod ilm;
pub mod ls;
pub mod mb;
pub mod mirror;
pub mod mv;
pub mod ping;
pub mod pipe;
pub mod put;
pub mod rb;
pub mod ready;
pub mod rm;
pub mod share;
pub mod stat;
pub mod tag;
pub mod tree;
pub mod util;
pub mod version;

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
        Commands::Pipe(args) => pipe::run(args, json),
        Commands::Get(args) => get::run(args, json),
        Commands::Head(args) => head::run(args, json),
        Commands::Du(args) => du::run(args, json),
        Commands::Find(args) => find::run(args, json),
        Commands::Tree(args) => tree::run(args, json),
        Commands::Diff(args) => diff::run(args, json),
        Commands::Share(args) => share::run(args.command, json),
        Commands::Ready(args) => ready::run(args, json),
        Commands::Ping(args) => ping::run(args, json),
        Commands::Tag(args) => tag::run(args.command, json),
        Commands::Version(args) => version::run(args.command, json),
        Commands::Cors(args) => cors::run(args.command, json),
        Commands::Encrypt(args) => encrypt::run(args.command, json),
        Commands::Anonymous(args) => anonymous::run(args.command, json),
        Commands::Ilm(args) => ilm::run(args.command, json),
    }
}
