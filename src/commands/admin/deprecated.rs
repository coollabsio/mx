//! Hidden, deprecated `mc admin` commands (`inspect`, `idp`, `speedtest`, `console`, `health`,
//! `subnet`, `bucket`, `tier`, `profile`). mc answers most with `deprecatedError`; some have
//! subcommands with their own replacement text (`admin bucket remote add` -> `mc replicate
//! add`) or still work (`admin tier ls` runs `ilm tier ls`).
//!
//! Owner: SERVER.

use super::deprecated;
use crate::commands::ilm_tier::{
    self, IlmTierAddArgs, IlmTierAliasArgs, IlmTierArgs, IlmTierCommand, IlmTierEditArgs,
    IlmTierInfoArgs, IlmTierNameArgs, IlmTierRmArgs,
};
use crate::error::McError;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};

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

/// `admin health|subnet|bucket|tier|profile` (hidden in mc). A global `--json` given after
/// the command lands in the trailing arguments; errors are then printed as JSON like mc.
pub fn stub(command: &str, args: DeprecatedArgs) -> Result<()> {
    let late_json = !crate::globals::json() && args.args.iter().any(|a| a == "--json");
    let result = dispatch(command, args.args);
    match result {
        Err(err) if late_json && err.downcast_ref::<crate::output::Exit>().is_none() => {
            let report = crate::output::report(&err);
            println!("{}", crate::output::format_error(&report, true));
            Err(crate::output::Exit(1).into())
        }
        other => other,
    }
}

fn dispatch(command: &str, args: Vec<String>) -> Result<()> {
    let first = args.first().map(String::as_str).unwrap_or_default();
    match command {
        "admin health" => diag(&args),
        "admin subnet" => match first {
            "health" => diag(&args[1..]),
            "register" => deprecated("support register"),
            _ => deprecated("support"),
        },
        "admin profile" => match first {
            "start" => deprecated("support profile start"),
            "stop" => deprecated("support profile stop"),
            _ => deprecated("support profile"),
        },
        "admin bucket" => bucket(&args),
        "admin tier" => tier(&args),
        _ => deprecated(""),
    }
}

/// mc `admin subnet health` / `admin health`: `support diag` with the arguments first and the
/// flags after them.
fn diag(args: &[String]) -> Result<()> {
    let (flags, positional): (Vec<&String>, Vec<&String>) =
        args.iter().partition(|arg| arg.starts_with('-'));
    let mut new_command = vec!["support diag".to_string()];
    new_command.extend(positional.into_iter().cloned());
    new_command.extend(flags.into_iter().cloned());
    deprecated(&new_command.join(" "))
}

/// mc `commandNotFound`: fatal ``PROG: <ERROR> `x` is not a recognized command.``.
fn not_a_command(name: &str, commands: &[&str]) -> Result<()> {
    let commands: Vec<String> = commands.iter().map(|c| c.to_string()).collect();
    let message = crate::usage::unknown_command_message(name, &commands);
    Err(anyhow::Error::new(McError::new("")).context(message))
}

/// Help of a hidden command group (mc prints it and exits 0).
fn group_help(path: &[&str]) -> Result<()> {
    crate::help::print_help(path);
    Ok(())
}

/// `mc admin bucket remote|quota|info`: all deprecated, pointing to `replicate`, `quota`,
/// `stat`.
fn bucket(args: &[String]) -> Result<()> {
    let first = args.first().map(String::as_str);
    match first {
        None => group_help(&["admin", "bucket"]),
        Some("quota") => deprecated("quota"),
        Some("info") => deprecated("stat"),
        Some("remote") => match args.get(1).map(String::as_str) {
            None => group_help(&["admin", "bucket", "remote"]),
            Some("add") => deprecated("replicate add"),
            Some("edit") => deprecated("replicate update"),
            Some("remove" | "rm") => deprecated("replicate rm"),
            Some(other) => not_a_command(other, &["add", "edit", "remove"]),
        },
        Some(other) => not_a_command(other, &["remote", "quota", "info"]),
    }
}

/// `mc admin tier info|ls|add|edit|verify|rm` still run the `ilm tier` commands.
#[derive(Debug, Parser)]
#[command(name = "admin tier", no_binary_name = true)]
struct TierCli {
    #[command(subcommand)]
    command: TierCommand,
}

#[derive(Debug, Subcommand)]
enum TierCommand {
    Info(IlmTierInfoArgs),
    Ls(IlmTierAliasArgs),
    Add(Box<IlmTierAddArgs>),
    Edit(IlmTierEditArgs),
    Verify(IlmTierNameArgs),
    Rm(IlmTierRmArgs),
}

fn tier(args: &[String]) -> Result<()> {
    let first = args.first().map(String::as_str).unwrap_or_default();
    if !["info", "ls", "add", "edit", "verify", "rm"].contains(&first) {
        return deprecated("ilm tier");
    }
    // Global `--json` given after the command lands in the trailing arguments.
    let json = crate::globals::json() || args.iter().any(|a| a == "--json");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--json").collect();
    let cli = match TierCli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            let argv: Vec<String> = std::env::args().collect();
            crate::usage::usage_error(err, &argv)
        }
    };
    let command = match cli.command {
        TierCommand::Info(args) => IlmTierCommand::Info(args),
        TierCommand::Ls(args) => IlmTierCommand::Ls(args),
        TierCommand::Add(args) => IlmTierCommand::Add(args),
        TierCommand::Edit(args) => IlmTierCommand::Edit(args),
        TierCommand::Verify(args) => IlmTierCommand::Verify(args),
        TierCommand::Rm(args) => IlmTierCommand::Rm(args),
    };
    ilm_tier::run(IlmTierArgs { command }, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> DeprecatedArgs {
        DeprecatedArgs {
            args: list.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn cause(result: Result<()>) -> String {
        let err = result.unwrap_err();
        crate::output::split_error(&err).1
    }

    #[test]
    fn points_to_replacements_like_mc() {
        let prog = "mc";
        assert_eq!(
            cause(stub("admin health", args(&["e", "--offline", "x"]))),
            format!("Please use '{prog} support diag e x --offline' instead")
        );
        assert_eq!(
            cause(stub("admin subnet", args(&["register", "e"]))),
            format!("Please use '{prog} support register' instead")
        );
        assert_eq!(
            cause(stub("admin profile", args(&["stop", "e"]))),
            format!("Please use '{prog} support profile stop' instead")
        );
        assert_eq!(
            cause(stub("admin bucket", args(&["remote", "rm", "e"]))),
            format!("Please use '{prog} replicate rm' instead")
        );
        assert_eq!(
            cause(stub("admin tier", args(&["bogus"]))),
            format!("Please use '{prog} ilm tier' instead")
        );
    }

    #[test]
    fn unknown_bucket_subcommands_are_usage_errors() {
        let err = stub("admin bucket", args(&["bogus"])).unwrap_err();
        assert!(
            crate::output::split_error(&err)
                .0
                .starts_with("`bogus` is not a recognized command.")
        );
    }
}
