pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod flags;
pub mod globals;
pub mod location;
pub mod mirror;
pub mod net;
pub mod output;
pub mod progress;
pub mod resolve;
pub mod s3;
pub mod target;
pub mod transfer;
pub mod usage;

use anyhow::Result;
use clap::{CommandFactory, Parser};

/// Parses `args` and runs the command. Usage errors, `--help` and `--version` are printed here
/// and exit the process (usage errors with status 1, like mc). Runtime errors are returned;
/// `main` prints them with [`output::fatal`].
pub fn run<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let argv = flags::rewrite_argv(args);
    let cli = match cli::Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(err) => {
            let argv: Vec<String> = argv
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            usage::usage_error(err, &argv)
        }
    };
    if cli.version {
        print!("{}", version_text());
        return Ok(());
    }
    if cli.command.is_none() {
        // mc shows the app help and exits with status 1 when no command is given.
        let _ = cli::Cli::command().print_help();
        std::process::exit(1);
    }
    resolve::configure(cli.resolve.clone());
    config::configure_dir(cli.config_dir.clone());
    config::load_env_config_file()?;
    globals::init(globals::Globals {
        json: cli.json,
        quiet: cli.quiet,
        insecure: cli.insecure,
        debug: cli.debug,
        // mc disables colors for JSON lines output.
        no_color: cli.no_color || cli.json,
        disable_pager: cli.disable_pager,
        custom_headers: cli.custom_header.clone(),
        // 0 means unlimited, like mc.
        limit_upload: cli.limit_upload.filter(|rate| *rate > 0),
        limit_download: cli.limit_download.filter(|rate| *rate > 0),
        config_dir: cli.config_dir.clone(),
    });
    s3::client::set_conn_deadlines(cli.conn_read_deadline, cli.conn_write_deadline);
    commands::run(cli)
}

/// Version text like mc's `printMCVersion`.
pub fn version_text() -> String {
    format!(
        "{} version {} (commit-id={})\nRuntime: {} {}/{}\nLicense Apache-2.0 <https://www.apache.org/licenses/LICENSE-2.0>\n",
        output::prog_name(),
        env!("CARGO_PKG_VERSION"),
        option_env!("MX_COMMIT_ID").unwrap_or("unknown"),
        option_env!("MX_RUSTC_VERSION").unwrap_or("rustc"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}
