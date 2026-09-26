//! Hidden, deprecated `mc admin` commands (`inspect`, `idp`, `speedtest`, `console`, `health`,
//! `subnet`, `bucket`, `tier`, `profile`). mc answers most with `deprecatedError`; some have
//! subcommands with their own replacement text (`admin bucket remote add` -> `mc replicate
//! add`) or still work (`admin tier ls` runs `ilm tier ls`).
//!
//! Owner: SERVER. `stub` entries return "not implemented yet" until implemented.

use super::deprecated;
use crate::commands::not_implemented;
use anyhow::Result;
use clap::Args;

/// Any arguments (mc ignores them for deprecated commands).
#[derive(Debug, Args)]
pub struct DeprecatedArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ConsoleArgs {
    #[arg(
        short = 'l',
        long = "limit",
        value_name = "VALUE",
        help = "show last n log entries"
    )]
    pub limit: Option<i64>,
    #[arg(
        short = 't',
        long = "type",
        value_name = "VALUE",
        help = "list error logs by type. Valid options are '[minio, application, all]'"
    )]
    pub log_type: Option<String>,
    #[arg(hide = true)]
    pub args: Vec<String>,
}

/// `mc admin console`: points to `admin logs` with the equivalent flags.
pub fn console(args: ConsoleArgs) -> Result<()> {
    let mut new_command = vec!["admin logs".to_string()];
    if let Some(limit) = args.limit {
        new_command.push(format!("--last {limit}"));
    }
    if let Some(log_type) = args.log_type {
        new_command.push(format!("--type {}", log_type.to_lowercase()));
    }
    new_command.extend(args.args);
    deprecated(&new_command.join(" "))
}

pub fn stub(command: &str, args: DeprecatedArgs) -> Result<()> {
    let _ = args;
    not_implemented(command)
}
