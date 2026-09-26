//! mc-style usage errors (mc `onUsageError` / `commandNotFound`).
//!
//! - Unknown or malformed flags: `PROG: <ERROR> Invalid command usage, flag provided but not
//!   defined: -bogus`, a blank line and a `SUPPORTED FLAGS:` block rendered from the clap
//!   command like urfave/cli (`--name value, -n value   usage (default: x) [$ENV]`).
//! - Unknown commands: ``PROG: <ERROR> `x` is not a recognized command. Get help using
//!   `--help` flag.`` with mc's "Did you mean one of these?" suggestions.
//! - Missing arguments: the command help on stdout, exit status 1.

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Arg, ArgAction, Command, CommandFactory};
use std::error::Error as _;

/// Prints `err` like mc and exits (help/version requests exit 0, everything else 1).
pub fn usage_error(err: clap::Error, argv: &[String]) -> ! {
    let root = crate::cli::Cli::command();
    let path = command_path(&root, argv);
    match err.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => err.exit(),
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        | ErrorKind::MissingRequiredArgument
        | ErrorKind::MissingSubcommand
            if !missing_flag(&err) =>
        {
            // mc prints the command help and exits with status 1.
            let mut cmd = find_command(root, &path);
            let _ = cmd.print_help();
            std::process::exit(1);
        }
        ErrorKind::InvalidSubcommand => {
            let name = context_string(&err, ContextKind::InvalidSubcommand).unwrap_or_default();
            let parent = find_command(root, &path);
            let names: Vec<String> = parent
                .get_subcommands()
                .filter(|sub| !sub.is_hide_set())
                .map(|sub| sub.get_name().to_string())
                .collect();
            let json = crate::globals::json_requested(argv);
            crate::globals::init(crate::globals::Globals {
                json,
                ..Default::default()
            });
            if path.is_empty() {
                // mc prints an (empty) update notice on stdout before the error.
                if json {
                    println!(r#"{{"status":"success","message":""}}"#);
                } else {
                    println!();
                }
            }
            let message = unknown_command_message(&name, &names);
            crate::output::print_report(&crate::output::Report {
                message,
                cause: String::new(),
                detail: Default::default(),
                kind: crate::output::ErrorKind::Fatal,
            });
            std::process::exit(1);
        }
        _ => {
            let cmd = find_command(root.clone(), &path);
            let reason = usage_reason(&err, &cmd, &root, argv);
            let mut text = format!(
                "{}: <ERROR> Invalid command usage, {reason}\n",
                crate::output::prog_name()
            );
            if !path.is_empty() {
                text.push_str("\nSUPPORTED FLAGS:\n");
                text.push_str(&supported_flags(&cmd, &root, &path));
            }
            eprint!("{text}");
            std::process::exit(1);
        }
    }
}

/// Subcommand names in `argv` (after the program name), e.g. `["ilm", "rule", "add"]`.
fn command_path(root: &Command, argv: &[String]) -> Vec<String> {
    let mut path = Vec::new();
    let mut current = root.clone();
    for arg in argv.iter().skip(1) {
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') {
            continue;
        }
        let Some(sub) = current
            .get_subcommands()
            .find(|sub| sub.get_name() == arg || sub.get_all_aliases().any(|alias| alias == arg))
            .cloned()
        else {
            continue;
        };
        path.push(sub.get_name().to_string());
        current = sub;
    }
    path
}

fn find_command(mut cmd: Command, path: &[String]) -> Command {
    for name in path {
        match cmd.find_subcommand(name) {
            Some(sub) => cmd = sub.clone(),
            None => break,
        }
    }
    cmd
}

/// True when a required `--flag` (not a positional argument) is missing.
fn missing_flag(err: &clap::Error) -> bool {
    match err.get(ContextKind::InvalidArg) {
        Some(ContextValue::Strings(values)) => values.iter().any(|v| v.starts_with('-')),
        Some(ContextValue::String(value)) => value.starts_with('-'),
        _ => false,
    }
}

fn context_string(err: &clap::Error, kind: ContextKind) -> Option<String> {
    match err.get(kind)? {
        ContextValue::String(value) => Some(value.clone()),
        ContextValue::Strings(values) => values.first().cloned(),
        _ => None,
    }
}

/// Go `flag` package wording for the clap error.
fn usage_reason(err: &clap::Error, cmd: &Command, root: &Command, argv: &[String]) -> String {
    let invalid_arg = context_string(err, ContextKind::InvalidArg).unwrap_or_default();
    match err.kind() {
        ErrorKind::UnknownArgument => {
            let flag = invalid_arg.split('=').next().unwrap_or_default();
            format!(
                "flag provided but not defined: -{}",
                flag.trim_start_matches('-')
            )
        }
        ErrorKind::InvalidValue | ErrorKind::ValueValidation | ErrorKind::NoEquals => {
            let value = context_string(err, ContextKind::InvalidValue).unwrap_or_default();
            let name = typed_flag_name(&invalid_arg, cmd, root, argv);
            // Go's flag package says `parse error`; mx keeps its validation message.
            let reason = match err.source() {
                Some(source)
                    if err.kind() == ErrorKind::ValueValidation
                        && !source.is::<std::num::ParseIntError>()
                        && !source.is::<std::num::ParseFloatError>()
                        && !source.is::<std::str::ParseBoolError>() =>
                {
                    source.to_string()
                }
                _ => "parse error".to_string(),
            };
            if value.is_empty() && err.kind() == ErrorKind::InvalidValue {
                format!("flag needs an argument: -{name}")
            } else {
                format!("invalid value \"{value}\" for flag -{name}: {reason}")
            }
        }
        ErrorKind::MissingRequiredArgument => {
            let missing = match err.get(ContextKind::InvalidArg) {
                Some(ContextValue::Strings(values)) => values
                    .iter()
                    .map(|v| v.split(' ').next().unwrap_or_default().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => invalid_arg.clone(),
            };
            format!("required flag \"{missing}\" not set")
        }
        _ => {
            let rendered = err.render().to_string();
            let text = rendered.strip_prefix("error: ").unwrap_or(&rendered);
            text.lines().next().unwrap_or_default().to_string()
        }
    }
}

/// The name of the flag as the user typed it (`n` for `-n`, `lines` for `--lines`).
fn typed_flag_name(display: &str, cmd: &Command, root: &Command, argv: &[String]) -> String {
    let first = display.split([' ', '=']).next().unwrap_or_default();
    let fallback = first.trim_start_matches('-').to_string();
    let arg = all_flags(cmd, root, &[]).into_iter().find(|arg| {
        arg.get_long()
            .is_some_and(|long| format!("--{long}") == first)
            || arg
                .get_short()
                .is_some_and(|short| format!("-{short}") == first)
    });
    let Some(arg) = arg else {
        return fallback;
    };
    let names = flag_names(&arg);
    for token in argv.iter().skip(1) {
        let name = token.trim_start_matches('-');
        let name = name.split('=').next().unwrap_or_default();
        if token.starts_with('-') && names.iter().any(|n| n == name) {
            return name.to_string();
        }
    }
    fallback
}

fn flag_names(arg: &Arg) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    // mc declares `head -n` as "n,lines" (short name first).
    if arg.get_long() == Some("lines") {
        names.extend(arg.get_short().map(String::from));
    }
    names.extend(arg.get_long().map(str::to_string));
    names.extend(
        arg.get_visible_aliases()
            .unwrap_or_default()
            .into_iter()
            .map(str::to_string),
    );
    if arg.get_long() != Some("lines") {
        names.extend(arg.get_short().map(String::from));
    }
    names.extend(
        arg.get_visible_short_aliases()
            .unwrap_or_default()
            .into_iter()
            .map(String::from),
    );
    names
}

/// The command's visible flags and the global flags, in mc's order: mc appends the globals
/// to most commands, but lists them first for `get`, `version enable`, `ilm tier` and
/// `replicate` commands, and after the encryption flags for `put`.
fn all_flags(cmd: &Command, root: &Command, path: &[String]) -> Vec<Arg> {
    let visible = |arg: &Arg| {
        !arg.is_positional()
            && !arg.is_hide_set()
            && !matches!(arg.get_action(), ArgAction::Help | ArgAction::Version)
    };
    let own: Vec<Arg> = cmd
        .get_arguments()
        .filter(|arg| !arg.is_global_set() && visible(arg))
        .cloned()
        .collect();
    let globals: Vec<Arg> = root
        .get_arguments()
        .filter(|arg| arg.is_global_set() && visible(arg))
        .cloned()
        .collect();
    let path = path.join(" ");
    let at = match path.as_str() {
        "get" | "version enable" | "ilm tier add" | "ilm tier edit" | "ilm tier rm" => 0,
        _ if path.starts_with("replicate") => 0,
        "put" => own
            .iter()
            .take_while(|arg| arg.get_long().is_some_and(|l| l.starts_with("enc-")))
            .count(),
        _ => own.len(),
    };
    let mut flags = own;
    flags.splice(at..at, globals);
    flags
}

/// urfave/cli `stringifyFlag`: (`--name value, -n value`, `usage (default: x) [$ENV]`).
pub fn flag_help(arg: &Arg) -> (String, String) {
    let takes_value = arg.get_action().takes_values();
    let names = flag_names(arg)
        .into_iter()
        .map(|name| {
            let prefix = if name.chars().count() == 1 { "-" } else { "--" };
            if takes_value {
                format!("{prefix}{name} value")
            } else {
                format!("{prefix}{name}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut usage = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
    if takes_value {
        let defaults: Vec<String> = arg
            .get_default_values()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        if let [value] = defaults.as_slice() {
            if !value.is_empty() {
                if value.parse::<f64>().is_ok() {
                    usage.push_str(&format!(" (default: {value})"));
                } else {
                    usage.push_str(&format!(" (default: {value:?})"));
                }
            }
        } else if arg.get_long() == Some("config-dir") {
            usage.push_str(&format!(
                " (default: {:?})",
                crate::config::default_dir().display().to_string()
            ));
        }
    }
    let mut usage = usage.trim().to_string();
    if let Some(env) = arg.get_env() {
        usage.push_str(&format!(" [${}]", env.to_string_lossy()));
    }
    (names, usage)
}

/// The `SUPPORTED FLAGS:` block (mc `onUsageError`).
pub fn supported_flags(cmd: &Command, root: &Command, path: &[String]) -> String {
    let mut rows: Vec<(String, String)> =
        all_flags(cmd, root, path).iter().map(flag_help).collect();
    rows.push(("--help, -h".to_string(), "show help".to_string()));
    let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0) + 2;
    rows.iter()
        .map(|(name, usage)| format!("   {name}{}{usage}\n", " ".repeat(width - name.len())))
        .collect()
}

/// mc `commandNotFound` message with "Did you mean" suggestions.
pub fn unknown_command_message(name: &str, commands: &[String]) -> String {
    let mut msg = format!("`{name}` is not a recognized command. Get help using `--help` flag.");
    let closest = closest_commands(name, commands);
    if !closest.is_empty() {
        msg.push_str("\n\nDid you mean one of these?\n");
        if let [only] = closest.as_slice() {
            msg.push_str(&format!("        `{only}`"));
        } else {
            for cmd in &closest {
                msg.push_str(&format!("        `{cmd}`\n"));
            }
        }
    }
    msg
}

/// mc `findClosestCommands`: prefix matches (sorted), then commands within Damerau-Levenshtein
/// distance 1.
fn closest_commands(name: &str, commands: &[String]) -> Vec<String> {
    let mut sorted: Vec<&String> = commands.iter().collect();
    sorted.sort();
    let mut closest: Vec<String> = sorted
        .iter()
        .filter(|cmd| cmd.starts_with(name))
        .map(|cmd| cmd.to_string())
        .collect();
    for cmd in sorted {
        // mc skips values that sort before an existing match (`sort.SearchStrings` quirk).
        if closest.iter().any(|c| c.as_str() >= cmd.as_str()) {
            continue;
        }
        if damerau_levenshtein(name, cmd) < 2 {
            closest.push(cmd.to_string());
        }
    }
    closest
}

fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + cost);
            }
        }
    }
    d[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_flags_like_urfave_cli() {
        let root = crate::cli::Cli::command();
        let ls = root.find_subcommand("ls").unwrap().clone();
        let block = supported_flags(&ls, &root, &["ls".to_string()]);
        let lines: Vec<&str> = block.lines().collect();
        assert_eq!(
            lines[0],
            "   --rewind value                     list all object versions no later than specified date"
        );
        assert!(
            block.contains(
                "   --storage-class value, --sc value  filter to specified storage class\n"
            )
        );
        assert!(block.contains(
            "   --json                             enable JSON lines formatted output [$MC_JSON]\n"
        ));
        assert_eq!(
            *lines.last().unwrap(),
            "   --help, -h                         show help"
        );
    }

    #[test]
    fn unknown_command_suggests_close_commands() {
        let cmds: Vec<String> = ["cat", "cors", "cp", "ls", "mb", "rb"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            unknown_command_message("bogus", &cmds),
            "`bogus` is not a recognized command. Get help using `--help` flag."
        );
        assert_eq!(
            unknown_command_message("lss", &cmds),
            "`lss` is not a recognized command. Get help using `--help` flag.\n\nDid you mean one of these?\n        `ls`"
        );
        assert_eq!(
            closest_commands("c", &cmds),
            vec!["cat".to_string(), "cors".into(), "cp".into()]
        );
    }

    #[test]
    fn maps_unknown_flag_to_go_wording() {
        let argv: Vec<String> = ["mc", "ls", "--bogus", "x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err = <crate::cli::Cli as clap::Parser>::try_parse_from(&argv).unwrap_err();
        let root = crate::cli::Cli::command();
        let ls = root.find_subcommand("ls").unwrap().clone();
        assert_eq!(
            usage_reason(&err, &ls, &root, &argv),
            "flag provided but not defined: -bogus"
        );
        let argv: Vec<String> = ["mc", "head", "-n", "abc", "x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err = <crate::cli::Cli as clap::Parser>::try_parse_from(&argv).unwrap_err();
        let head = root.find_subcommand("head").unwrap().clone();
        assert_eq!(
            usage_reason(&err, &head, &root, &argv),
            "invalid value \"abc\" for flag -n: parse error"
        );
    }
}
