//! Help parity with the reference `mc` (MX_MC_BIN). Needs no server:
//!
//! ```sh
//! MX_MC_BIN=$(sh tests/mc_ref.sh) cargo test --test live_mc_parity_help
//! ```
//!
//! The command list is discovered at test time by walking mc's `COMMANDS:` sections (plus the
//! hidden commands mx has), so commands added to or missing from either side fail loudly.
//! For every path it compares `--help`, `-h`, the bare invocation of command groups, the usage
//! error of an unknown flag (`SUPPORTED FLAGS:`) and urfave's `help` subcommand.
//!
//! mx renders help from `src/help/mc.txt`, captured from the pinned mc. Regenerate it with:
//!
//! ```sh
//! MX_MC_BIN=$(sh tests/mc_ref.sh) MX_HELP_REGEN=1 cargo test --test live_mc_parity_help regenerate
//! ```
//!
//! Deliberate differences (also in COMPATIBILITY.md), applied to mc's text by
//! [`mx_deviations`] both when regenerating and when comparing:
//! - top level: the `license` and `support` commands and `--autocompletion` (not implemented
//!   in mx) are not listed, nor the autocompletion TIP; COPYRIGHT/LICENSE name mx's.
//! - the `--config-dir` default is mx's config dir (`~/.mx`, or `~/.mc` when only that exists)
//!   where mc prints `~/.<program name>`; the `VERSION:` of urfave's `help` page is mx's release.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// mc command groups mx does not implement: `mx` must reject them as unknown commands.
const UNIMPLEMENTED: &[&str] = &["license", "support"];

/// mx commands the pinned mc does not have (help from `src/help/mx.txt`).
const MX_ONLY: &[&str] = &["replicate resync cancel"];

/// The program name both binaries run under (a unique token, so the regenerated data can
/// substitute it).
const PROG: &str = "zqprog";

const DATA_FILE: &str = "src/help/mc.txt";

#[derive(Debug, PartialEq)]
struct Outcome {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

struct Side {
    program: PathBuf,
    home: tempfile::TempDir,
}

impl Side {
    fn new(bin: &Path, dir: &Path, name: &str) -> Self {
        let bin_dir = dir.join(name);
        std::fs::create_dir_all(&bin_dir).expect("bin dir");
        let program = bin_dir.join(PROG);
        #[cfg(unix)]
        std::os::unix::fs::symlink(bin, &program).expect("symlink");
        #[cfg(not(unix))]
        std::fs::copy(bin, &program).expect("copy");
        Side {
            program,
            home: tempfile::tempdir().expect("home"),
        }
    }

    fn run(&self, args: &[&str]) -> Outcome {
        let mut command = Command::new(&self.program);
        command.args(args).env("HOME", self.home.path());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("MC_") {
                command.env_remove(key);
            }
        }
        let out = command
            .output()
            .unwrap_or_else(|err| panic!("run {}: {err}", self.program.display()));
        Outcome {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        }
    }

    fn home(&self) -> String {
        self.home.path().to_string_lossy().into_owned()
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    mc: Side,
    mx: Side,
}

impl Harness {
    fn new() -> Option<Self> {
        let Some(mc_bin) = std::env::var_os("MX_MC_BIN").map(PathBuf::from) else {
            eprintln!("skipping help parity test; set MX_MC_BIN (sh tests/mc_ref.sh)");
            return None;
        };
        assert!(mc_bin.exists(), "MX_MC_BIN {} missing", mc_bin.display());
        let mc_bin = std::fs::canonicalize(mc_bin).expect("canonical mc");
        let dir = tempfile::tempdir().expect("tempdir");
        let mc = Side::new(&mc_bin, dir.path(), "mc");
        let mx = Side::new(Path::new(env!("CARGO_BIN_EXE_mx")), dir.path(), "mx");
        Some(Harness { _dir: dir, mc, mx })
    }

    /// Every mc command path (visible commands, depth first) plus mx's hidden commands.
    fn paths(&self) -> Vec<Vec<String>> {
        let mut paths = Vec::new();
        self.walk(&mut Vec::new(), &mut paths);
        // mx's hidden commands, walked like mc's (hidden commands have subcommands too).
        for mut path in mx_paths() {
            if !paths.contains(&path) {
                self.walk(&mut path, &mut paths);
            }
        }
        let mut seen = BTreeSet::new();
        paths.retain(|path| seen.insert(path.clone()));
        paths
    }

    fn walk(&self, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        out.push(path.clone());
        let mut args = strs(path);
        args.push("--help");
        let help = self.mc.run(&args).stdout;
        for sub in subcommands(&help) {
            path.push(sub);
            self.walk(path, out);
            path.pop();
        }
    }
}

fn strs(path: &[String]) -> Vec<&str> {
    path.iter().map(String::as_str).collect()
}

/// Command names in a help page's `COMMANDS:` section (first name of `name, alias`).
fn subcommands(help: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;
    for line in help.lines() {
        if line == "COMMANDS:" {
            inside = true;
        } else if inside {
            if line.trim().is_empty() {
                break;
            }
            let name = line.trim_start().split("  ").next().unwrap_or_default();
            names.push(name.split(", ").next().unwrap_or_default().to_string());
        }
    }
    names
}

/// All command paths of mx's clap tree (including hidden commands).
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
    walk(
        &<mx::cli::Cli as clap::CommandFactory>::command(),
        &mut Vec::new(),
        &mut out,
    );
    out
}

fn unimplemented(path: &[String]) -> bool {
    path.first()
        .is_some_and(|first| UNIMPLEMENTED.contains(&first.as_str()))
}

fn is_group(help: &str) -> bool {
    help.lines().any(|line| line == "COMMANDS:")
}

/// mx's deliberate differences from mc's help text (see the module docs).
fn mx_deviations(path: &[String], text: &str) -> String {
    if !path.is_empty() {
        return text.to_string();
    }
    let mut out = String::new();
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if UNIMPLEMENTED
            .iter()
            .any(|name| trimmed.starts_with(&format!("{name} ")))
            || trimmed.starts_with("--autocompletion ")
        {
            continue;
        }
        match line.trim_end() {
            "TIP:" => {
                // The TIP section (and its trailing blank line).
                for next in lines.by_ref() {
                    if next.trim().is_empty() {
                        break;
                    }
                }
                continue;
            }
            "COPYRIGHT:" => {
                out.push_str(line);
                lines.next();
                out.push_str("  Copyright (c) mx contributors\n");
                continue;
            }
            "LICENSE:" => {
                out.push_str(line);
                lines.next();
                out.push_str("  Apache-2.0 <https://www.apache.org/licenses/LICENSE-2.0>\n");
                continue;
            }
            _ => {}
        }
        out.push_str(line);
    }
    out
}

/// Replaces side-specific values: the `--config-dir` default and the release tag.
fn normalize(side: &Side, text: &str) -> String {
    let home = side.home();
    let mut out = text.to_string();
    for dir in [".mx", ".mc", &format!(".{PROG}")] {
        out = out.replace(
            &format!("(default: \"{home}/{dir}\")"),
            "(default: \"<CONFIG_DIR>\")",
        );
    }
    let release = regex::Regex::new(r"(?m)^(VERSION:\n\s+)\S+$").unwrap();
    release.replace_all(&out, "${1}<VERSION>").into_owned()
}

fn describe(outcome: &Outcome) -> String {
    format!(
        "exit {:?}\n--- stdout\n{}--- stderr\n{}",
        outcome.code, outcome.stdout, outcome.stderr
    )
}

fn first_difference(a: &str, b: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i), b.get(i));
        if x != y {
            return format!("line {}:\n  mc: {x:?}\n  mx: {y:?}", i + 1);
        }
    }
    "(trailing newline/whitespace differs)".to_string()
}

#[test]
fn help_matches_mc_for_every_command() {
    let Some(h) = Harness::new() else { return };
    let paths = h.paths();
    assert!(paths.len() > 200, "walked only {} mc paths", paths.len());
    let mut failures = Vec::new();
    let mut compare = |path: &[String], args: Vec<&str>| {
        let mc = h.mc.run(&args);
        let mx = h.mx.run(&args);
        let norm = |side: &Side, o: &Outcome, stdout: String| Outcome {
            stdout: normalize(side, &stdout),
            stderr: normalize(side, &o.stderr),
            code: o.code,
        };
        let mc = norm(&h.mc, &mc, mx_deviations(path, &mc.stdout));
        let mx = norm(&h.mx, &mx, mx.stdout.clone());
        if mc != mx {
            let diff = if mc.stdout != mx.stdout {
                first_difference(&mc.stdout, &mx.stdout)
            } else if mc.stderr != mx.stderr {
                first_difference(&mc.stderr, &mx.stderr)
            } else {
                format!("exit {:?} vs {:?}", mc.code, mx.code)
            };
            failures.push(format!(
                "`{PROG} {}`: {diff}\n=== mc {}=== mx {}",
                args.join(" "),
                describe(&mc),
                describe(&mx)
            ));
        }
    };
    for path in &paths {
        let base = strs(path);
        if MX_ONLY.contains(&base.join(" ").as_str()) {
            let mut args = base.clone();
            args.push("--help");
            let (mc, mx) = (h.mc.run(&args), h.mx.run(&args));
            assert_ne!(
                mc.code,
                Some(0),
                "mc has `{}` now; drop it from MX_ONLY",
                base.join(" ")
            );
            assert!(
                mx.code == Some(0) && mx.stdout.starts_with("NAME:\n"),
                "{}",
                describe(&mx)
            );
            continue;
        }
        if unimplemented(path) {
            let mut args = base.clone();
            args.push("--help");
            let mx = h.mx.run(&args);
            assert!(
                mx.code == Some(1) && mx.stderr.contains("is not a recognized command"),
                "`{}` is implemented now; drop it from UNIMPLEMENTED\n{}",
                base.join(" "),
                describe(&mx)
            );
            continue;
        }
        let with = |extra: &[&'static str]| {
            let mut args = base.clone();
            args.extend_from_slice(extra);
            args
        };
        compare(path, with(&["--help"]));
        compare(path, with(&["-h"]));
        compare(path, with(&["--zzbogus"]));
        let help = h.mc.run(&with(&["--help"])).stdout;
        if is_group(&help) || path.is_empty() {
            compare(path, base.clone());
        }
        let subs = subcommands(&help);
        if subs.iter().any(|sub| sub == "help") {
            compare(path, with(&["help"]));
            compare(path, with(&["h"]));
            if let Some(first) = subs.first() {
                let mut args = with(&["help"]);
                args.push(first);
                compare(path, args);
            }
        }
    }
    // Typed aliases resolve to the canonical command.
    compare(&["admin".into()], vec!["admin", "decom", "--help"]);
    compare(
        &["admin".into()],
        vec!["admin", "decom", "status", "--help"],
    );
    // Help wins over positional arguments; unknown flags win over help.
    compare(&["ls".into()], vec!["ls", "extra", "--help"]);
    compare(&["ls".into()], vec!["ls", "--help", "--zzbogus"]);
    compare(&["ls".into()], vec!["--json", "ls", "-h"]);
    // Usage errors other than unknown flags (Go `flag` wording + SUPPORTED FLAGS).
    compare(&["head".into()], vec!["head", "-n", "abc", "x"]);
    compare(&["ls".into()], vec!["ls", "--rewind"]);
    compare(
        &["admin".into()],
        vec!["admin", "trace", "--response-duration", "abc", "x"],
    );
    compare(&["cp".into()], vec!["cp", "--max-workers", "x", "a", "b"]);
    compare(&["mb".into()], vec!["mb", "--region"]);
    compare(&["od".into()], vec!["od", "--size"]);
    compare(&[], vec!["-C"]);
    // Missing arguments print the help and exit 1.
    compare(&["cp".into()], vec!["cp", "one"]);
    compare(&["admin".into()], vec!["admin", "user", "add"]);
    compare(&[], vec!["help"]);
    compare(&[], vec!["help", "ls"]);
    compare(&[], vec!["admin", "help"]);
    assert!(
        failures.is_empty(),
        "{} help differences:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// mx under its own name prints its name where mc prints `mc`.
#[test]
fn help_uses_invoked_program_name() {
    let out = Command::new(env!("CARGO_BIN_EXE_mx"))
        .args(["ls", "--help"])
        .output()
        .expect("run mx");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("NAME:\n  mx ls - list buckets and objects\n"),
        "{text}"
    );
    assert!(text.contains("\n     $ mx ls s3\n"), "{text}");
}

/// Writes `src/help/mc.txt` from the reference mc (MX_HELP_REGEN=1).
///
/// Records are `=== <kind> <path>` headers followed by the text (the file adds one newline
/// after each record). Kinds: `help` (`--help` output), `flags` (the `SUPPORTED FLAGS:` rows
/// without their indent), `helpcmd` (urfave's `help` subcommand page), `nohelp` (commands
/// that reject `--help`, urfave `HideHelp`) and `incorrect` (flag errors are urfave's
/// `Incorrect Usage<sep>` on stdout; the text is the separator). Placeholders: `@PROG@`, `@CONFIG_DIR@`, `@VERSION@` and `@FLAGS:<indent>@` (the `flags` rows).
#[test]
fn regenerate() {
    if std::env::var_os("MX_HELP_REGEN").is_none() {
        return;
    }
    let Some(h) = Harness::new() else { return };
    let home = h.mc.home();
    let placeholders = |text: &str| {
        let text = text.replace(&format!("\"{home}/.{PROG}\""), "\"@CONFIG_DIR@\"");
        let text = text.replace(PROG, "@PROG@");
        let release = regex::Regex::new(r"(?m)^(VERSION:\n\s+)\S+$").unwrap();
        release.replace_all(&text, "${1}@VERSION@").into_owned()
    };
    let mut data = String::from(
        "# Generated by tests/live_mc_parity_help.rs (regenerate) from the pinned mc; do not edit.\n",
    );
    let mut record = |kind: &str, path: &[String], text: &str| {
        assert!(!text.contains("\n=== "), "record separator in {path:?}");
        let key = if path.is_empty() {
            ".".to_string()
        } else {
            path.join(" ")
        };
        data.push_str(&format!("=== {kind} {key}\n{text}\n"));
    };
    for path in h.paths() {
        if unimplemented(&path) {
            continue;
        }
        let base = strs(&path);
        let mut args = base.clone();
        args.push("--help");
        let help = h.mc.run(&args);
        if MX_ONLY.contains(&base.join(" ").as_str()) {
            continue;
        }
        if help.code != Some(0) {
            // mc commands with `HideHelp` reject `--help` as an unknown flag.
            assert!(help.stderr.contains("flag: help requested"), "mc {args:?}");
            record("nohelp", &path, "");
        }
        let mut text = placeholders(&mx_deviations(&path, &help.stdout));
        let mut args = base.clone();
        args.push("--zzbogus");
        let bogus_out = h.mc.run(&args);
        if let Some(rest) = bogus_out.stdout.strip_prefix("Incorrect Usage") {
            // urfave reports the flag error itself (command groups, commands without mc's
            // usage-error handler): `Incorrect Usage. ...` or `Incorrect Usage: ...`.
            let sep = &rest[..1];
            assert_eq!(
                rest,
                format!("{sep} flag provided but not defined: -zzbogus\n\n"),
                "mc {args:?}"
            );
            record("incorrect", &path, sep);
        }
        let bogus = placeholders(&bogus_out.stderr);
        if let Some((_, rows)) = bogus.split_once("\nSUPPORTED FLAGS:\n") {
            let rows: Vec<&str> = rows
                .lines()
                .map(|row| row.strip_prefix("   ").expect("indented flag row"))
                .collect();
            for indent in [2, 3] {
                let block: String = rows
                    .iter()
                    .map(|row| format!("{}{row}\n", " ".repeat(indent)))
                    .collect();
                if text.contains(&block) {
                    text = text.replacen(&block, &format!("@FLAGS:{indent}@\n"), 1);
                    break;
                }
            }
            record("flags", &path, &rows.join("\n"));
        }
        if help.code == Some(0) {
            record("help", &path, &text);
        }
        if subcommands(&help.stdout).iter().any(|sub| sub == "help") {
            let mut args = base.clone();
            args.push("help");
            record("helpcmd", &path, &placeholders(&h.mc.run(&args).stdout));
        }
    }
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(DATA_FILE);
    std::fs::write(&file, data).expect("write help data");
    eprintln!("wrote {}", file.display());
}
