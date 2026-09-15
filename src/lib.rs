pub mod cli;
pub mod commands;
pub mod config;
pub mod location;
pub mod output;
pub mod resolve;
pub mod s3;
pub mod target;
pub mod transfer;

use anyhow::Result;
use clap::Parser;

pub fn run<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = cli::Cli::parse_from(args);
    resolve::configure(cli.resolve.clone());
    config::configure_dir(cli.config_dir.clone());
    output::configure(cli.quiet, cli.insecure);
    commands::run(cli)
}
