pub mod cli;
pub mod commands;
pub mod config;
pub mod s3;
pub mod target;

use anyhow::Result;
use clap::Parser;

pub fn run<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = cli::Cli::parse_from(args);
    commands::run(cli)
}
