//! mc's help pages (`--help`, `-h`, command groups without a subcommand, missing arguments)
//! and the `SUPPORTED FLAGS:` rows of usage errors.
//!
//! The pages are mc's rendered urfave/cli templates, captured from the pinned reference mc
//! into `mc.txt` by `tests/live_mc_parity_help.rs` (`regenerate`); `mx.txt` holds the
//! mx-only commands in the same format. Records are `=== <kind> <path>` headers (`.` is the
//! top level) followed by the text plus one newline:
//!
//! - `help`: the `--help` page. `@FLAGS:<n>@` lines expand to the command's `flags` rows
//!   indented by `n` spaces, so help and usage errors list flags from one place.
//! - `flags`: the command's flag rows (urfave `stringifyFlag`, tab-aligned), unindented.
//! - `helpcmd`: the page of urfave's `help` subcommand (`cors help`).
//! - `nohelp`: the command rejects `--help` like an unknown flag (urfave `HideHelp`).
//! - `incorrect`: flag errors are urfave's own `Incorrect Usage<sep> ...` (the text is the
//!   separator, `.` for command groups), not mc's `SUPPORTED FLAGS:` report.
//!
//! Placeholders: `@PROG@` (invoked program name), `@CONFIG_DIR@` (the default config dir),
//! `@VERSION@` (release tag).

use std::collections::HashMap;
use std::sync::OnceLock;

const MC_PAGES: &str = include_str!("mc.txt");
const MX_PAGES: &str = include_str!("mx.txt");

#[derive(Default)]
struct Page {
    help: Option<&'static str>,
    flags: Option<&'static str>,
    helpcmd: Option<&'static str>,
    nohelp: bool,
    incorrect: Option<&'static str>,
}

fn pages() -> &'static HashMap<String, Page> {
    static PAGES: OnceLock<HashMap<String, Page>> = OnceLock::new();
    PAGES.get_or_init(|| {
        let mut pages = HashMap::new();
        for data in [MC_PAGES, MX_PAGES] {
            for (kind, path, text) in records(data) {
                let page: &mut Page = pages.entry(path.to_string()).or_default();
                match kind {
                    "help" => page.help = Some(text),
                    "flags" => page.flags = Some(text),
                    "helpcmd" => page.helpcmd = Some(text),
                    "nohelp" => page.nohelp = true,
                    "incorrect" => page.incorrect = Some(text),
                    other => panic!("unknown help record kind {other}"),
                }
            }
        }
        pages
    })
}

/// `(kind, path, text)` records of a help data file.
fn records(data: &'static str) -> Vec<(&'static str, &'static str, &'static str)> {
    let start = if data.starts_with("=== ") {
        0
    } else {
        data.find("\n=== ").map_or(data.len(), |at| at + 1)
    };
    let mut rest = &data[start..];
    let mut out = Vec::new();
    while let Some(record) = rest.strip_prefix("=== ") {
        let (header, body) = record.split_once('\n').unwrap_or((record, ""));
        let (text, next) = match body.find("\n=== ") {
            Some(at) => (&body[..at], &body[at + 1..]),
            None => (body.strip_suffix('\n').unwrap_or(body), ""),
        };
        let (kind, path) = header.split_once(' ').unwrap_or((header, "."));
        out.push((kind, if path == "." { "" } else { path }, text));
        rest = next;
    }
    out
}

fn key(path: &[&str]) -> String {
    path.join(" ")
}

fn expand(text: &str, flags: Option<&str>) -> String {
    let mut out = String::with_capacity(text.len() + 1024);
    for line in text.split_inclusive('\n') {
        let indent = line
            .strip_prefix("@FLAGS:")
            .and_then(|rest| rest.strip_suffix("@\n"))
            .and_then(|n| n.parse::<usize>().ok());
        match indent {
            Some(indent) => {
                for row in flags.unwrap_or_default().lines() {
                    out.push_str(&" ".repeat(indent));
                    out.push_str(row);
                    out.push('\n');
                }
            }
            None => out.push_str(line),
        }
    }
    out.replace("@PROG@", &crate::output::prog_name())
        .replace(
            "@CONFIG_DIR@",
            &crate::config::default_dir().display().to_string(),
        )
        .replace("@VERSION@", env!("MX_RELEASE"))
}

/// The `--help` page of the command at `path` (canonical names; empty for the top level).
pub fn help_page(path: &[&str]) -> Option<String> {
    let page = pages().get(&key(path))?;
    Some(expand(page.help?, page.flags))
}

/// The `SUPPORTED FLAGS:` rows of a usage error (mc `onUsageError`), when mc lists them.
pub fn supported_flags(path: &[&str]) -> Option<String> {
    let flags = pages().get(&key(path))?.flags?;
    let rows: String = flags.lines().map(|row| format!("   {row}\n")).collect();
    Some(expand(&rows, None))
}

/// True when the command group at `path` has urfave's `help` subcommand (`cors help`).
pub fn has_help_command(path: &[&str]) -> bool {
    pages()
        .get(&key(path))
        .is_some_and(|page| page.helpcmd.is_some())
}

/// Prints the help page of `path` on stdout (clap's help for a command without a page).
pub fn print_help(path: &[&str]) {
    match help_page(path) {
        Some(text) => write_stdout(&text),
        None => {
            let mut cmd = find_command(path);
            let _ = cmd.print_help();
        }
    }
}

/// Writes to stdout, ignoring errors (`mx --help | head` closes the pipe early).
fn write_stdout(text: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
}

/// mc `showCommandHelpAndExit(ctx, code)`.
pub fn show_help_and_exit(path: &[&str], code: i32) -> ! {
    print_help(path);
    std::process::exit(code);
}

/// urfave's `Incorrect Usage` separator when it (not mc) reports flag errors of `path`.
pub fn incorrect_usage(path: &[&str]) -> Option<&'static str> {
    pages().get(&key(path))?.incorrect
}

/// True when the page of `path` lists subcommands (a command group).
pub fn is_group(path: &[&str]) -> bool {
    help_page(path).is_some_and(|text| text.contains("\nCOMMANDS:\n"))
}

/// urfave's `help` subcommand of a group: `P help` prints the group page, `P help CMD` the
/// page of CMD, `P help --help` the help command's own page. Exits.
fn help_command(path: &[String], args: &[String]) -> ! {
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    let mut own = path.clone();
    own.push("help");
    for arg in args.iter().take_while(|arg| *arg != "--") {
        let Some(name) = arg.strip_prefix('-') else {
            continue;
        };
        let name = name.trim_start_matches('-');
        let name = name.split('=').next().unwrap_or_default();
        if name == "help" || name == "h" {
            show_help_and_exit(&own, 0);
        }
        crate::usage::flag_error_exit(&own, &format!("flag provided but not defined: -{name}"));
    }
    let Some(topic) = args.first() else {
        if let Some(text) = pages().get(&key(&path)).and_then(|page| page.helpcmd) {
            write_stdout(&expand(text, None));
        }
        std::process::exit(0);
    };
    if topic == "help" || topic == "h" {
        show_help_and_exit(&own, 0);
    }
    let group = find_command(&path);
    let name = group
        .get_subcommands()
        .find(|sub| sub.get_name() == topic || sub.get_all_aliases().any(|alias| alias == topic))
        .map_or(topic.as_str(), |sub| sub.get_name());
    let mut full = path.clone();
    full.push(name);
    if help_page(&full).is_some() {
        show_help_and_exit(&full, 0);
    }
    eprintln!("No help topic for '{topic}'");
    std::process::exit(3);
}

/// A deprecated command mx models as hidden trailing arguments (`admin bucket`, `admin tier`,
/// `admin top locks`, ...): clap accepts any flag there, so mx checks them against mc's.
fn is_stub(cmd: &clap::Command) -> bool {
    cmd.get_positionals()
        .any(|arg| arg.is_trailing_var_arg_set() && arg.is_hide_set())
}

struct Resolved {
    path: Vec<String>,
    stub: bool,
    /// Index of urfave's `help` subcommand in `argv`.
    help_at: Option<usize>,
}

fn resolve(argv: &[String]) -> Resolved {
    let root = find_command(&[]);
    let mut cmd = &root;
    let mut resolved = Resolved {
        path: Vec::new(),
        stub: false,
        help_at: None,
    };
    for (i, arg) in argv.iter().enumerate().skip(1) {
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') {
            continue;
        }
        let path: Vec<&str> = resolved.path.iter().map(String::as_str).collect();
        if (arg == "help" || arg == "h") && has_help_command(&path) {
            resolved.help_at = Some(i);
            break;
        }
        if resolved.stub {
            // mc's hidden subcommands of a stub (`admin bucket remote add`).
            let mut next = path.clone();
            next.push(arg);
            if pages().contains_key(&key(&next)) {
                resolved.path.push(arg.clone());
            }
        } else if let Some(sub) = cmd.find_subcommand(arg) {
            resolved.path.push(sub.get_name().to_string());
            resolved.stub = is_stub(sub);
            cmd = sub;
        }
    }
    resolved
}

/// The canonical command path named by `argv` (e.g. `["ilm", "rule", "add"]`), including mc's
/// hidden subcommands of commands mx models as trailing arguments; the flag tells whether the
/// command is such a stub.
pub fn command_path(argv: &[String]) -> (Vec<String>, bool) {
    let resolved = resolve(argv);
    (resolved.path, resolved.stub)
}

/// Flag names (without dashes) of the command at `path`: its `flags` rows, else the globals.
fn flag_names(path: &[&str]) -> Vec<String> {
    let names = |rows: &str| -> Vec<String> {
        rows.lines()
            .flat_map(|row| {
                let column = row.trim_start().split("  ").next().unwrap_or_default();
                column
                    .split(", ")
                    .map(|name| {
                        // `--name value` / `--name 5ms` (urfave's value placeholder).
                        let name = name.trim_start_matches('-');
                        name.split(' ').next().unwrap_or_default().to_string()
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    match pages().get(&key(path)).and_then(|page| page.flags) {
        Some(rows) => names(rows),
        None => {
            // Command groups take the global flags (the top-level `GLOBAL FLAGS:` rows).
            let top = pages()
                .get("")
                .and_then(|page| page.help)
                .unwrap_or_default();
            let rows = top
                .split_once("\nGLOBAL FLAGS:\n")
                .map_or("", |(_, rest)| rest);
            let end = rows.find("\n  \n").unwrap_or(rows.len());
            names(&rows[..end])
        }
    }
}

/// Handles, before clap parses `argv`, what clap cannot see: urfave's `help` subcommand
/// (`cors help set`) and flags/`--help` of mc's hidden subcommands that mx models as trailing
/// arguments (`admin bucket remote add --help`). Returns when there is nothing to do.
pub fn intercept(argv: &[String]) {
    let resolved = resolve(argv);
    if let Some(at) = resolved.help_at {
        help_command(&resolved.path, &argv[at + 1..]);
    }
    if !resolved.stub {
        return;
    }
    let path: Vec<&str> = resolved.path.iter().map(String::as_str).collect();
    let known = flag_names(&path);
    let mut wants_help = false;
    for arg in argv.iter().skip(1).take_while(|arg| *arg != "--") {
        let Some(name) = arg.strip_prefix('-') else {
            continue;
        };
        let name = name.trim_start_matches('-');
        let name = name.split('=').next().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        if name == "help" || name == "h" {
            wants_help = true;
        } else if !known.iter().any(|known| known == name) {
            crate::usage::flag_error_exit(
                &path,
                &format!("flag provided but not defined: -{name}"),
            );
        }
    }
    if wants_help {
        if pages().get(&key(&path)).is_some_and(|page| page.nohelp) {
            crate::usage::flag_error_exit(&path, "flag: help requested");
        }
        show_help_and_exit(&path, 0);
    }
}

fn find_command(path: &[&str]) -> clap::Command {
    let mut cmd = <crate::cli::Cli as clap::CommandFactory>::command();
    cmd.build();
    for name in path {
        match cmd.find_subcommand(name) {
            Some(sub) => cmd = sub.clone(),
            None => break,
        }
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// Every mx command path (canonical names), hidden ones included.
    fn mx_paths() -> Vec<Vec<String>> {
        fn walk(cmd: &clap::Command, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
            out.push(path.clone());
            for sub in cmd.get_subcommands() {
                path.push(sub.get_name().to_string());
                walk(sub, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(&crate::cli::Cli::command(), &mut Vec::new(), &mut out);
        out
    }

    fn find<'a>(root: &'a clap::Command, path: &[String]) -> &'a clap::Command {
        path.iter().fold(root, |cmd, name| {
            cmd.find_subcommand(name).expect("subcommand")
        })
    }

    #[test]
    fn parses_records() {
        let data =
            "# comment\n=== help .\nTOP\n\n=== flags ls\n--a  x\n--b  y\n=== help ls\nno newline";
        let recs = records(Box::leak(data.to_string().into_boxed_str()));
        assert_eq!(
            recs,
            vec![
                ("help", "", "TOP\n"),
                ("flags", "ls", "--a  x\n--b  y"),
                ("help", "ls", "no newline"),
            ]
        );
    }

    #[test]
    fn expands_flags_and_placeholders() {
        let text = expand(
            "NAME:\n  @PROG@ ls\nFLAGS:\n@FLAGS:2@\n  \n",
            Some("--a  x\n--b  y"),
        );
        let prog = crate::output::prog_name();
        assert_eq!(
            text,
            format!("NAME:\n  {prog} ls\nFLAGS:\n  --a  x\n  --b  y\n  \n")
        );
    }

    #[test]
    fn ls_help_and_supported_flags_share_rows() {
        let help = help_page(&["ls"]).unwrap();
        let flags = supported_flags(&["ls"]).unwrap();
        assert!(help.contains("\nFLAGS:\n  --rewind value                     list all object versions no later than specified date\n"));
        assert!(flags.starts_with("   --rewind value                     list all object versions no later than specified date\n"));
        for row in flags.lines() {
            assert!(help.contains(&format!("{}\n", &row[1..])), "{row}");
        }
        assert!(flags.ends_with("   --help, -h                         show help\n"));
        assert!(!help.contains('@'), "{help}");
    }

    /// mc flags mx does not accept (`path -flag`).
    const MISSING_MC_FLAGS: &[&str] = &[
        "anonymous -recursive",
        "anonymous -r",
        "anonymous set -recursive",
        "anonymous set -r",
        "anonymous set-json -recursive",
        "anonymous set-json -r",
        "anonymous get -recursive",
        "anonymous get -r",
        "anonymous get-json -recursive",
        "anonymous get-json -r",
        "anonymous list -recursive",
        "anonymous list -r",
        "ilm restore -enc-c",
    ];

    /// Visible mx flags mc does not have (`path --flag`); help lists mc's flags only.
    const MX_EXTENSION_FLAGS: &[&str] = &["ilm rule add --id"];

    fn arg_names(arg: &clap::Arg) -> Vec<String> {
        let mut names: Vec<String> = arg.get_long().into_iter().map(String::from).collect();
        names.extend(arg.get_short().map(String::from));
        names.extend((arg.get_all_aliases().unwrap_or_default().into_iter()).map(String::from));
        names.extend(
            (arg.get_all_short_aliases().unwrap_or_default().into_iter()).map(String::from),
        );
        names
    }

    /// Every mx command has a help page; mc's flags (from the page) and mx's clap flags agree
    /// up to the documented lists.
    #[test]
    fn every_command_has_a_page_with_known_flags() {
        let root = crate::cli::Cli::command();
        let mut missing = Vec::new();
        let mut extensions = Vec::new();
        for path in mx_paths() {
            let path_ref: Vec<&str> = path.iter().map(String::as_str).collect();
            let page = pages().get(&key(&path_ref));
            assert!(
                page.is_some_and(|p| p.help.is_some()),
                "no help page for {path:?}"
            );
            let cmd = find(&root, &path);
            if is_stub(cmd) {
                continue;
            }
            let mc_names = flag_names(&path_ref);
            let known: Vec<String> = cmd
                .get_arguments()
                .chain(root.get_arguments())
                .flat_map(arg_names)
                .chain(["help".to_string(), "h".to_string()])
                .collect();
            for name in &mc_names {
                if !known.contains(name) {
                    missing.push(format!("{} -{name}", path.join(" ")));
                }
            }
            for arg in cmd.get_arguments() {
                if arg.is_positional() || arg.is_hide_set() || arg.is_global_set() {
                    continue;
                }
                if matches!(arg.get_action(), clap::ArgAction::Help) {
                    continue;
                }
                if !arg_names(arg).iter().any(|name| mc_names.contains(name)) {
                    extensions.push(format!(
                        "{} --{}",
                        path.join(" "),
                        arg.get_long().unwrap_or_default()
                    ));
                }
            }
        }
        assert_eq!(missing, MISSING_MC_FLAGS, "mc flags missing in mx");
        assert_eq!(extensions, MX_EXTENSION_FLAGS, "mx-only flags");
    }
}
