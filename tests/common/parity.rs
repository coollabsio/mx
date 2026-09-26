//! Side-by-side parity harness: runs the reference MinIO `mc` (MX_MC_BIN, built by
//! `tests/mc_ref.sh`) and `mx` against the same live server and compares their output.
//!
//! Each tool gets its own [`Side`]: a temp HOME (alias configured with the tool's own
//! `alias set`), a work dir used as cwd, and its own bucket. Bucket names have equal length
//! (`<prefix>-p<nanos>-mc` / `-mx`) so table alignment is comparable. Args are templates:
//!
//! - `{alias}`  test alias (`local`)
//! - `{bucket}` the side's bucket name
//! - `{target}` `{alias}/{bucket}`
//! - `{work}`   the side's work dir (cwd of every command)
//!
//! Setup commands ([`Parity::setup`]) always run through the reference `mc` so bugs in `mx`
//! cannot skew the fixture. `mx` is run through a symlink named `mc` (program name appears
//! in usage/errors); [`Parity::mx_program_name`] switches that.
//!
//! Variable output is canonicalized by [`Normalizer`] before comparison (timestamps, ETags,
//! version IDs, request IDs, speeds/durations, presigned query params, paths, bucket names).
#![allow(dead_code)]

use regex::{Captures, Regex};
use serde_json::Value;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use super::live;

/// Reference `mc` binary from MX_MC_BIN, when it exists.
pub fn mc_bin() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("MX_MC_BIN")?);
    path.is_file().then_some(path)
}

/// True when live tests are enabled and a reference `mc` is available.
pub fn enabled() -> bool {
    live::enabled() && mc_bin().is_some()
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn mx_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mx"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Mc,
    Mx,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Mc => "mc",
            Tool::Mx => "mx",
        }
    }
}

/// Captured result of one command.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

/// Bucket options for [`Parity::with`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    /// Create each side's bucket (`{bucket}`) up front.
    pub bucket: bool,
    pub versioning: bool,
    pub lock: bool,
}

/// One tool's environment.
pub struct Side {
    pub tool: Tool,
    pub home: tempfile::TempDir,
    pub work: PathBuf,
    pub bucket: String,
    /// Program to execute (reference mc, or a `mc`-named symlink to mx).
    pub program: PathBuf,
}

impl Side {
    fn config_dir(&self) -> PathBuf {
        self.home.path().join(match self.tool {
            Tool::Mc => ".mc",
            Tool::Mx => ".mx",
        })
    }

    /// Expands `{alias}`, `{bucket}`, `{target}`, `{work}`, `{url}`, `{access_key}`,
    /// `{secret_key}` in `arg`. Second server ([`Parity::second_server`]): `{alias2}`,
    /// `{endpoint2}` (server 2 URL as seen from server 1), `{remote2}` (the same with
    /// credentials, for `replicate add --remote-bucket`), `{access_key2}`, `{secret_key2}`.
    /// Names shared by both sides: `{base}` (bucket name without `-mc`/`-mx`), `{BASE}`
    /// (upper-cased, e.g. a tier name). `{BUCKET}` is the upper-cased bucket name.
    pub fn expand(&self, arg: &str) -> String {
        let alias = live::alias_name();
        let base = self
            .bucket
            .trim_end_matches(&format!("-{}", self.tool.label()))
            .to_string();
        let endpoint2 = env_or("MX_TEST_URL2_INTERNAL", "");
        let (access2, secret2) = (
            env_or("MX_TEST_ACCESS_KEY2", ""),
            env_or("MX_TEST_SECRET_KEY2", ""),
        );
        let remote2 = endpoint2.replacen("://", &format!("://{access2}:{secret2}@"), 1);
        let arg = arg
            .replace("{alias2}", &env_or("MX_TEST_ALIAS2", "local2"))
            .replace("{endpoint2}", &endpoint2)
            .replace("{remote2}", &remote2)
            .replace("{access_key2}", &access2)
            .replace("{secret_key2}", &secret2)
            .replace("{base}", &base)
            .replace("{BASE}", &base.to_uppercase())
            .replace("{BUCKET}", &self.bucket.to_uppercase());
        arg.replace("{target}", &format!("{alias}/{}", self.bucket))
            .replace("{bucket}", &self.bucket)
            .replace("{alias}", &alias)
            .replace("{work}", &self.work.to_string_lossy())
            .replace("{url}", &env_or("MX_TEST_URL", ""))
            .replace("{access_key}", &env_or("MX_TEST_ACCESS_KEY", ""))
            .replace("{secret_key}", &env_or("MX_TEST_SECRET_KEY", ""))
    }

    /// Path inside the work dir.
    pub fn path(&self, rel: &str) -> PathBuf {
        self.work.join(rel)
    }
}

/// Paired mc/mx fixture.
pub struct Parity {
    pub mc: Side,
    pub mx: Side,
    /// HOME with the reference mc configured; used for setup and cleanup.
    setup_home: tempfile::TempDir,
    mc_bin: PathBuf,
    _bin_dir: tempfile::TempDir,
    pub normalizer: Normalizer,
    /// Extra env for every compared command.
    pub env: Vec<(String, String)>,
    /// Compare output lines / JSON documents as sorted sets (mc runs transfers in parallel,
    /// so `cp -r` / `mirror` output order is not deterministic). JSON docs are re-serialized,
    /// so key order is not compared in this mode.
    pub unordered: bool,
    /// Second server alias configured ([`Parity::second_server`]).
    alias2: bool,
    /// Commands (already expanded) run with the reference mc on drop, best effort.
    cleanups: Vec<Vec<String>>,
}

impl Parity {
    /// Fixture with one bucket per side. None (with a skip note) unless enabled.
    pub fn new() -> Option<Self> {
        Self::with(Opts {
            bucket: true,
            ..Opts::default()
        })
    }

    /// Fixture with no buckets created (for `mb`, alias or error cases).
    pub fn bare() -> Option<Self> {
        Self::with(Opts::default())
    }

    pub fn with(opts: Opts) -> Option<Self> {
        if !live::enabled() {
            eprintln!("skipping parity test; set MX_LIVE_TESTS=1");
            return None;
        }
        let Some(mc_bin) = mc_bin() else {
            eprintln!("skipping parity test; set MX_MC_BIN (sh tests/mc_ref.sh)");
            return None;
        };
        let bin_dir = tempfile::tempdir().expect("tempdir");
        let mx_as_mc = bin_dir.path().join("mc");
        std::os::unix::fs::symlink(mx_bin(), &mx_as_mc).expect("symlink mx as mc");

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let base = format!("{}-p{nanos}", live::bucket_prefix());
        let side = |tool: Tool, program: PathBuf| {
            let home = tempfile::tempdir().expect("tempdir");
            let work = home.path().join("work");
            std::fs::create_dir_all(&work).expect("work dir");
            Side {
                tool,
                work,
                home,
                bucket: format!("{base}-{}", tool.label()),
                program,
            }
        };
        let mc = side(Tool::Mc, mc_bin.clone());
        let mx = side(Tool::Mx, mx_as_mc);
        let mut parity = Self {
            normalizer: Normalizer::standard(),
            mc,
            mx,
            setup_home: tempfile::tempdir().expect("tempdir"),
            mc_bin,
            _bin_dir: bin_dir,
            env: Vec::new(),
            unordered: false,
            alias2: false,
            cleanups: Vec::new(),
        };
        parity.normalizer.bucket_prefix(&live::bucket_prefix());
        for tool in [Tool::Mc, Tool::Mx] {
            parity.set_alias(tool);
        }
        parity.set_setup_alias();
        if opts.bucket {
            let mut args = vec!["mb"];
            if opts.lock {
                args.push("--with-lock");
            }
            if opts.versioning {
                args.push("--with-versioning");
            }
            args.push("{target}");
            parity.setup(&args);
        }
        Some(parity)
    }

    /// Runs `mx` under its own name instead of `mc` (and maps `mx` to `mc` when comparing).
    pub fn mx_program_name(&mut self) {
        self.mx.program = mx_bin();
        self.normalizer.program_name("mx", "mc");
    }

    pub fn side(&self, tool: Tool) -> &Side {
        match tool {
            Tool::Mc => &self.mc,
            Tool::Mx => &self.mx,
        }
    }

    fn alias_args() -> Vec<String> {
        let url = std::env::var("MX_TEST_URL").expect("MX_TEST_URL");
        let access = std::env::var("MX_TEST_ACCESS_KEY").expect("MX_TEST_ACCESS_KEY");
        let secret = std::env::var("MX_TEST_SECRET_KEY").expect("MX_TEST_SECRET_KEY");
        let api = std::env::var("MX_TEST_API").unwrap_or_else(|_| "S3v4".to_string());
        let path = std::env::var("MX_TEST_PATH").unwrap_or_else(|_| "auto".to_string());
        vec![
            "alias".into(),
            "set".into(),
            live::alias_name(),
            url,
            access,
            secret,
            "--api".into(),
            api,
            "--path".into(),
            path,
        ]
    }

    fn set_alias(&self, tool: Tool) {
        let side = self.side(tool);
        let out = self.exec(side, &Self::alias_args(), None, &[]);
        assert_eq!(
            out.code,
            Some(0),
            "{} alias set failed: {out:?}",
            tool.label()
        );
    }

    fn set_setup_alias(&self) {
        let out = run_program(
            &self.mc_bin,
            self.setup_home.path(),
            self.setup_home.path(),
            &Self::alias_args(),
            None,
            &[],
        );
        assert_eq!(out.code, Some(0), "setup alias set failed: {out:?}");
    }

    fn exec(
        &self,
        side: &Side,
        args: &[String],
        stdin: Option<&[u8]>,
        env: &[(String, String)],
    ) -> Outcome {
        run_program(
            &side.program,
            side.home.path(),
            &side.work,
            args,
            stdin,
            env,
        )
    }

    /// Runs `args` (templated) with the reference mc for each side; panics on failure.
    pub fn setup(&self, args: &[&str]) {
        for side in [&self.mc, &self.mx] {
            let args: Vec<String> = args.iter().map(|arg| side.expand(arg)).collect();
            let out = run_program(
                &self.mc_bin,
                self.setup_home.path(),
                &side.work,
                &args,
                None,
                &[],
            );
            assert_eq!(
                out.code,
                Some(0),
                "setup `mc {}` failed: {out:?}",
                args.join(" ")
            );
        }
    }

    /// Configures the second server (`{alias2}`) for both sides and for setup. False (with a
    /// skip note) when MX_TEST_URL2 / MX_TEST_URL2_INTERNAL are not set.
    pub fn second_server(&mut self) -> bool {
        let (Some(server), Ok(_)) = (
            live::second_server(),
            std::env::var("MX_TEST_URL2_INTERNAL"),
        ) else {
            eprintln!("skipping parity test; second server (MX_TEST_URL2*) not configured");
            return false;
        };
        let args: Vec<String> = vec![
            "alias".into(),
            "set".into(),
            server.alias,
            server.url,
            server.access_key,
            server.secret_key,
        ];
        for side in [&self.mc, &self.mx] {
            let out = self.exec(side, &args, None, &[]);
            assert_eq!(out.code, Some(0), "alias set (server 2) failed: {out:?}");
        }
        let out = run_program(
            &self.mc_bin,
            self.setup_home.path(),
            self.setup_home.path(),
            &args,
            None,
            &[],
        );
        assert_eq!(
            out.code,
            Some(0),
            "setup alias set (server 2) failed: {out:?}"
        );
        self.alias2 = true;
        true
    }

    /// Runs a templated setup command once (expanded for the mc side), for fixtures shared
    /// by both sides (`{base}`, `{BASE}` names).
    pub fn setup_once(&self, args: &[&str]) {
        let args: Vec<String> = args.iter().map(|arg| self.mc.expand(arg)).collect();
        let out = run_program(
            &self.mc_bin,
            self.setup_home.path(),
            &self.mc.work,
            &args,
            None,
            &[],
        );
        assert_eq!(
            out.code,
            Some(0),
            "setup `mc {}` failed: {out:?}",
            args.join(" ")
        );
    }

    /// Registers a templated command to run with the reference mc when the fixture is
    /// dropped (best effort, expanded for each side; duplicates run once), e.g. removing a
    /// tier.
    pub fn cleanup_on_drop(&mut self, args: &[&str]) {
        for side in [&self.mc, &self.mx] {
            let args: Vec<String> = args.iter().map(|arg| side.expand(arg)).collect();
            if !self.cleanups.contains(&args) {
                self.cleanups.push(args);
            }
        }
    }

    /// Writes `rel` (relative to each side's work dir) with `contents`.
    pub fn file(&self, rel: &str, contents: &str) {
        for side in [&self.mc, &self.mx] {
            let path = side.path(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("mkdir");
            }
            std::fs::write(path, contents).expect("write");
        }
    }

    /// Writes local `rel` and uploads it to `{target}/<key>` on both sides.
    pub fn object(&self, key: &str, contents: &str) {
        let rel = format!(".seed/{key}");
        self.file(&rel, contents);
        self.setup(&["cp", "-q", &rel, &format!("{{target}}/{key}")]);
    }

    /// Runs one templated command with mc and then mx.
    pub fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> (Outcome, Outcome) {
        let run = |side: &Side| {
            let args: Vec<String> = args.iter().map(|arg| side.expand(arg)).collect();
            self.exec(side, &args, stdin, &self.env)
        };
        (run(&self.mc), run(&self.mx))
    }

    /// Normalized copy of `out` for `tool`.
    pub fn normalize(&self, tool: Tool, text: &str) -> String {
        self.normalizer.apply(self.side(tool), text)
    }

    /// Compares normalized stdout, stderr and exit code; panics with a diff on mismatch.
    pub fn assert_parity(&self, args: &[&str], stdin: Option<&[u8]>) {
        let (mc, mx) = self.run(args, stdin);
        let mut report = String::new();
        self.compare_text("stdout", &mc.stdout, &mx.stdout, &mut report);
        self.compare_text("stderr", &mc.stderr, &mx.stderr, &mut report);
        compare_code(&mc, &mx, &mut report);
        finish(args, &report);
    }

    /// Compares stdout (and stderr when it holds JSON) as JSON documents field by field, then
    /// the normalized bytes (key order, compact lines); also the exit code.
    pub fn assert_json_parity(&self, args: &[&str], stdin: Option<&[u8]>) {
        let (mc, mx) = self.run(args, stdin);
        let mut report = String::new();
        self.compare_json("stdout", &mc.stdout, &mx.stdout, &mut report);
        let mc_json = looks_json(&mc.stderr);
        if mc_json || looks_json(&mx.stderr) {
            self.compare_json("stderr", &mc.stderr, &mx.stderr, &mut report);
        } else {
            self.compare_text("stderr", &mc.stderr, &mx.stderr, &mut report);
        }
        compare_code(&mc, &mx, &mut report);
        finish(args, &report);
    }

    /// Normalized text, with lines sorted when [`Parity::unordered`] is set.
    fn canonical(&self, tool: Tool, text: &str) -> String {
        let text = self.normalize(tool, text);
        if !self.unordered {
            return text;
        }
        // Sort whole JSON documents when the stream parses (pretty-printed docs span lines).
        if looks_json(&text)
            && let Ok(mut docs) = json_docs(&text)
        {
            // Entries (by source/key/target) first, summary/other docs last.
            docs.sort_by_cached_key(|doc| {
                let entry = ["source", "key", "target"]
                    .iter()
                    .find_map(|field| doc.get(*field).and_then(Value::as_str))
                    .map(str::to_string);
                (entry.is_none(), entry, doc.to_string())
            });
            let pretty = text.trim_start().starts_with("{\n");
            return docs
                .iter()
                .map(|doc| {
                    let doc = if pretty {
                        serde_json::to_string_pretty(doc).unwrap_or_default()
                    } else {
                        doc.to_string()
                    };
                    format!("{doc}\n")
                })
                .collect();
        }
        let mut lines: Vec<&str> = text.lines().collect();
        lines.sort_unstable();
        lines.iter().map(|line| format!("{line}\n")).collect()
    }

    fn compare_text(&self, stream: &str, mc: &str, mx: &str, report: &mut String) {
        let mc = self.canonical(Tool::Mc, mc);
        let mx = self.canonical(Tool::Mx, mx);
        if mc != mx {
            let _ = writeln!(
                report,
                "{stream} differs (-mc +mx):\n{}",
                line_diff(&mc, &mx)
            );
        }
    }

    fn compare_json(&self, stream: &str, mc: &str, mx: &str, report: &mut String) {
        let mc_norm = self.canonical(Tool::Mc, mc);
        let mx_norm = self.canonical(Tool::Mx, mx);
        match (json_docs(&mc_norm), json_docs(&mx_norm)) {
            (Ok(mc_docs), Ok(mx_docs)) => {
                let mut diffs = Vec::new();
                if mc_docs.len() != mx_docs.len() {
                    diffs.push(format!(
                        "document count: mc={} mx={}",
                        mc_docs.len(),
                        mx_docs.len()
                    ));
                }
                for (index, (a, b)) in mc_docs.iter().zip(&mx_docs).enumerate() {
                    json_diff(&format!("[{index}]"), a, b, &mut diffs);
                }
                for (index, doc) in mc_docs.iter().enumerate().skip(mx_docs.len()) {
                    diffs.push(format!("[{index}] only in mc: {doc}"));
                }
                for (index, doc) in mx_docs.iter().enumerate().skip(mc_docs.len()) {
                    diffs.push(format!("[{index}] only in mx: {doc}"));
                }
                if !diffs.is_empty() {
                    let _ = writeln!(report, "{stream} JSON differs:");
                    for diff in diffs {
                        let _ = writeln!(report, "  {diff}");
                    }
                } else if mc_norm != mx_norm {
                    let _ = writeln!(
                        report,
                        "{stream} JSON equal but layout differs (-mc +mx):\n{}",
                        line_diff(&mc_norm, &mx_norm)
                    );
                }
            }
            (mc_res, mx_res) => {
                for (tool, res) in [("mc", mc_res), ("mx", mx_res)] {
                    if let Err(err) = res {
                        let _ = writeln!(report, "{stream}: {tool} output is not JSON: {err}");
                    }
                }
                if mc_norm != mx_norm {
                    let _ = writeln!(
                        report,
                        "{stream} differs (-mc +mx):\n{}",
                        line_diff(&mc_norm, &mx_norm)
                    );
                }
            }
        }
    }
}

impl Drop for Parity {
    fn drop(&mut self) {
        for args in &self.cleanups {
            let _ = run_program(
                &self.mc_bin,
                self.setup_home.path(),
                self.setup_home.path(),
                args,
                None,
                &[],
            );
        }
        let mut aliases = vec![live::alias_name()];
        if self.alias2 {
            aliases.push(env_or("MX_TEST_ALIAS2", "local2"));
        }
        for alias in aliases {
            self.remove_buckets(&alias);
        }
    }
}

impl Parity {
    /// Removes every bucket the fixture may have created on `alias` (`{base}`, `{bucket}`
    /// and `{bucket}-*`).
    fn remove_buckets(&self, alias: &str) {
        let out = run_program(
            &self.mc_bin,
            self.setup_home.path(),
            self.setup_home.path(),
            &["ls".into(), "--json".into(), format!("{alias}/")],
            None,
            &[],
        );
        let prefix = self.mc.bucket.trim_end_matches("-mc").to_string();
        for doc in json_docs(&out.stdout).unwrap_or_default() {
            let Some(key) = doc.get("key").and_then(Value::as_str) else {
                continue;
            };
            let name = key.trim_end_matches('/');
            if name.starts_with(&prefix) {
                let target = format!("{alias}/{name}");
                for args in [
                    vec!["rm", "-r", "--force", "--versions", "--bypass", &target],
                    vec!["rb", "--force", &target],
                ] {
                    let args: Vec<String> = args.into_iter().map(String::from).collect();
                    let _ = run_program(
                        &self.mc_bin,
                        self.setup_home.path(),
                        self.setup_home.path(),
                        &args,
                        None,
                        &[],
                    );
                }
            }
        }
    }
}

fn run_program(
    program: &Path,
    home: &Path,
    cwd: &Path,
    args: &[String],
    stdin: Option<&[u8]>,
    env: &[(String, String)],
) -> Outcome {
    let mut command = Command::new(program);
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy().into_owned();
        if key.starts_with("MC_") || key.starts_with("MX_") {
            command.env_remove(key);
        }
    }
    command
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("TZ", "UTC")
        .env("LANG", "C")
        .env_remove("NO_COLOR")
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .unwrap_or_else(|err| panic!("spawn {}: {err}", program.display()));
    if let Some(input) = stdin {
        let mut pipe = child.stdin.take().expect("stdin");
        pipe.write_all(input).expect("write stdin");
    }
    let output = child.wait_with_output().expect("wait");
    Outcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code(),
    }
}

fn compare_code(mc: &Outcome, mx: &Outcome, report: &mut String) {
    if mc.code != mx.code {
        let _ = writeln!(
            report,
            "exit code differs: mc={:?} mx={:?}",
            mc.code, mx.code
        );
    }
}

fn finish(args: &[&str], report: &str) {
    if !report.is_empty() {
        panic!("parity mismatch for `mc {}`:\n{report}", args.join(" "));
    }
}

fn looks_json(text: &str) -> bool {
    matches!(text.trim_start().chars().next(), Some('{' | '['))
}

/// Parses a stream of JSON documents (compact lines or pretty-printed).
pub fn json_docs(text: &str) -> Result<Vec<Value>, String> {
    serde_json::Deserializer::from_str(text)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())
}

/// Field-by-field JSON comparison (`a` = mc, `b` = mx) appending readable differences.
pub fn json_diff(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, av) in a {
                let sub = format!("{path}.{key}");
                match b.get(key) {
                    Some(bv) => json_diff(&sub, av, bv, out),
                    None => out.push(format!("{sub}: missing in mx (mc={av})")),
                }
            }
            for (key, bv) in b {
                if !a.contains_key(key) {
                    out.push(format!("{path}.{key}: extra in mx (mx={bv})"));
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                out.push(format!(
                    "{path}: array length mc={} mx={}",
                    a.len(),
                    b.len()
                ));
            }
            for (index, (av, bv)) in a.iter().zip(b).enumerate() {
                json_diff(&format!("{path}[{index}]"), av, bv, out);
            }
        }
        _ if a != b => out.push(format!("{path}: mc={a} mx={b}")),
        _ => {}
    }
}

/// Minimal LCS line diff; `-` lines are mc, `+` lines are mx, with 2 lines of context.
pub fn line_diff(a: &str, b: &str) -> String {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            ops.push(('-', a[i]));
            i += 1;
        } else {
            ops.push(('+', b[j]));
            j += 1;
        }
    }
    let changed: Vec<usize> = (0..ops.len()).filter(|&k| ops[k].0 != ' ').collect();
    let mut out = String::new();
    let mut last: Option<usize> = None;
    for (k, (op, line)) in ops.iter().enumerate() {
        let near = changed.iter().any(|&c| c.abs_diff(k) <= 2);
        if !near {
            continue;
        }
        if last.is_some_and(|l| k > l + 1) {
            out.push_str("  ...\n");
        }
        let _ = writeln!(out, "{op} {line}");
        last = Some(k);
    }
    if a.len() == b.len() && out.is_empty() {
        out.push_str("  (whitespace / trailing newline differs)\n");
    }
    out
}

/// Ordered regex rewrite rules applied to both outputs before comparison.
pub struct Normalizer {
    rules: Vec<(Regex, String)>,
}

impl Normalizer {
    /// No rules besides the per-side literals (paths, bucket).
    pub fn empty() -> Self {
        Self { rules: Vec::new() }
    }

    /// Standard rules for inherently variable output.
    pub fn standard() -> Self {
        let mut n = Self::empty();
        // Presigned URL query values (before timestamps/hex rules touch them).
        n.rule(
            r"(X-Amz-(?:Algorithm|Credential|Date|Expires|SignedHeaders|Signature|Security-Token))=[^&\s\x22\\]+",
            "$1=<X>",
        );
        // Volatile JSON numbers (transfer timing); before the hex rules, which would match
        // long float fractions.
        n.rule(r#"("(?:duration|speed)":\s*)[0-9.eE+-]+"#, "${1}0");
        // Go time.Time default format: 2026-09-26 12:00:00.123 +0000 UTC
        n.rule(
            r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d+)? [+-]\d{4} [A-Z]{3,4}",
            "<TIME>",
        );
        // RFC3339 / mc `[2026-09-26 12:00:00 UTC]` / ISO with offset.
        n.rule(
            r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z| UTC| GMT|[+-]\d{2}:?\d{2})?",
            "<TIME>",
        );
        // RFC1123 (HTTP dates).
        n.rule(
            r"(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun), \d{2} (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) \d{4} \d{2}:\d{2}:\d{2} GMT",
            "<TIME>",
        );
        // Version IDs (UUIDs).
        n.rule(
            r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
            "<VERSION_ID>",
        );
        // Host IDs / sha256.
        n.rule(r"\b[0-9a-f]{64}\b", "<HASH>");
        // ETags (plain or multipart `-N`).
        n.rule(r"\b[0-9a-f]{32}(?:-\d+)?\b", "<ETAG>");
        // MinIO request IDs.
        n.rule(r"\b[0-9A-F]{16}\b", "<REQUEST_ID>");
        // xid-style generated IDs (ILM rule IDs).
        n.rule(r"\b[0-9a-v]{20}\b", "<ID>");
        // Retention countdown.
        n.rule(
            r"expiring in \d+ hours? \d+ minutes?",
            "expiring in <REMAINING>",
        );
        // Speeds and durations.
        n.rule(r"\d+(?:\.\d+)? ?(?:[KMGTPE]i?)?B/s", "<SPEED>");
        n.rule(r"\b\d{2}m\d{2}s\b", "<DURATION>");
        n.rule(
            r"\b(?:\d+h)?(?:\d+m)?\d+(?:\.\d+)?(?:ns|µs|us|ms|s)\b",
            "<DURATION>",
        );
        n
    }

    /// Appends a rule; `replacement` may use `$1`-style groups.
    pub fn rule(&mut self, pattern: &str, replacement: &str) -> &mut Self {
        let regex = Regex::new(pattern).unwrap_or_else(|err| panic!("bad regex {pattern}: {err}"));
        self.rules.push((regex, replacement.to_string()));
        self
    }

    /// Canonicalizes fixture bucket names (`<prefix>-p<nanos>-mc|mx`) to `<BUCKET>`.
    pub fn bucket_prefix(&mut self, prefix: &str) -> &mut Self {
        let pattern = format!(r"{}-p\d+-m[cx]", regex::escape(prefix));
        self.rules
            .insert(0, (Regex::new(&pattern).unwrap(), "<BUCKET>".into()));
        self
    }

    /// Maps the program name `from` (word, e.g. `mx`) to `to`.
    pub fn program_name(&mut self, from: &str, to: &str) -> &mut Self {
        let pattern = format!(r"\b{}\b", regex::escape(from));
        self.rules
            .insert(0, (Regex::new(&pattern).unwrap(), to.into()));
        self
    }

    /// Applies side literals (config dir, work dir, home) and then every rule.
    pub fn apply(&self, side: &Side, text: &str) -> String {
        let home = side.home.path().to_string_lossy().into_owned();
        let mut out = text
            .replace(
                &side.config_dir().to_string_lossy().into_owned(),
                "<CONFIG_DIR>",
            )
            .replace(&side.work.to_string_lossy().into_owned(), "<WORK>")
            .replace(&home, "<HOME>");
        for (regex, replacement) in &self.rules {
            out = regex
                .replace_all(&out, |caps: &Captures| {
                    let mut dst = String::new();
                    caps.expand(replacement, &mut dst);
                    dst
                })
                .into_owned();
        }
        out
    }
}
