pub mod cli;
pub mod commands;
pub mod config;
pub mod flags;
pub mod globals;
pub mod location;
pub mod net;
pub mod mirror;
pub mod output;
pub mod progress;
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
    let cli = cli::Cli::parse_from(flags::rewrite_argv(args));
    resolve::configure(cli.resolve.clone());
    config::configure_dir(cli.config_dir.clone());
    globals::init(globals::Globals {
        json: cli.json,
        quiet: cli.quiet,
        insecure: cli.insecure,
        debug: cli.debug,
        no_color: cli.no_color,
        disable_pager: cli.disable_pager,
        custom_headers: cli.custom_header.clone(),
        // 0 means unlimited, like mc.
        limit_upload: cli.limit_upload.filter(|rate| *rate > 0),
        limit_download: cli.limit_download.filter(|rate| *rate > 0),
        config_dir: cli.config_dir.clone(),
    });
    commands::run(cli)
}
