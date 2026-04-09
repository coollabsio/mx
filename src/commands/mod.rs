pub mod alias;
pub mod ls;

use crate::cli::{Cli, Commands};
use anyhow::Result;

pub fn run(cli: Cli) -> Result<()> {
    let json = cli.json;

    match cli.command {
        Commands::Alias(args) => alias::run(args.command, json),
        Commands::Ls(args) => ls::run(args, json),
    }
}
