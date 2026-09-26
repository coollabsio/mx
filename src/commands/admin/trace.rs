//! `mx admin trace` and `mx admin scanner trace` (mc `admin trace`, `admin scanner trace`):
//! stream madmin `ServiceTrace` records, filter them like mc (`matchOpts`) and print mc's
//! short or verbose (`-v`) text / JSON. `--stats` (and `--in FILE` replay) render mc's call
//! statistics table.
//!
//! Also holds the helpers shared by the STREAM commands (help-and-exit, admin client, signal
//! exit codes, line output).

use crate::commands::admin::scanner::ScannerTraceArgs;
use crate::error::McError;
use crate::output::{self, Exit};
use crate::s3::admin::{self, AdminClient, ibytes};
use crate::s3::admin_stream::{
    self as stream, HealResultItem, TRACE_BOOTSTRAP, TRACE_INTERNAL, TRACE_S3, TraceInfo,
    TraceOpts, go_duration, round_duration, status_text, trace_time,
};
use anyhow::{Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};

#[derive(Debug, Args)]
pub struct TraceArgs {
    #[arg(value_name = "TARGET")]
    pub target: Vec<String>,
    #[arg(long = "verbose", short = 'v', help = "print verbose trace")]
    pub verbose: bool,
    #[arg(long = "all", short = 'a', help = "trace all call types")]
    pub all: bool,
    #[arg(
        long = "call",
        value_name = "VALUE",
        help = "trace only matching call types. See CALL TYPES below for list. (default: s3)"
    )]
    pub call: Vec<String>,
    #[arg(
        long = "status-code",
        value_name = "VALUE",
        value_parser = parse_status_code,
        help = "trace only matching status code"
    )]
    pub status_code: Vec<i64>,
    #[arg(
        long = "method",
        value_name = "VALUE",
        help = "trace only matching HTTP method"
    )]
    pub method: Vec<String>,
    #[arg(
        long = "funcname",
        value_name = "VALUE",
        help = "trace only matching func name"
    )]
    pub funcname: Vec<String>,
    #[arg(long = "path", value_name = "VALUE", help = "trace only matching path")]
    pub path: Vec<String>,
    #[arg(
        long = "node",
        value_name = "VALUE",
        help = "trace only matching servers"
    )]
    pub node: Vec<String>,
    #[arg(
        long = "request-header",
        value_name = "VALUE",
        help = "trace only matching request headers"
    )]
    pub request_header: Vec<String>,
    #[arg(
        long = "request-query",
        value_name = "VALUE",
        help = "trace only matching request queries"
    )]
    pub request_query: Vec<String>,
    #[arg(long = "errors", short = 'e', help = "trace only failed requests")]
    pub errors: bool,
    #[arg(
        long = "stats",
        help = "print statistical summary of all the traced calls"
    )]
    pub stats: bool,
    #[arg(
        long = "filter-request",
        help = "trace calls only with request bytes greater than this threshold, use with filter-size"
    )]
    pub filter_request: bool,
    #[arg(
        long = "filter-response",
        help = "trace calls only with response bytes greater than this threshold, use with filter-size"
    )]
    pub filter_response: bool,
    #[arg(
        long = "response-duration",
        default_value = "0s",
        value_name = "5ms",
        value_parser = parse_duration_flag,
        help = "trace calls only with response duration greater than this threshold (e.g. 5ms)"
    )]
    pub response_duration: i64,
    #[arg(
        long = "filter-size",
        value_name = "VALUE",
        help = "filter size, use with filter (see UNITS)"
    )]
    pub filter_size: Option<String>,
    #[arg(
        long = "in",
        value_name = "VALUE",
        help = "read previously saved json from file and replay"
    )]
    pub in_: Option<String>,
    #[arg(
        long = "stats-n",
        hide = true,
        default_value_t = 20,
        value_name = "VALUE",
        help = "maximum number of stat entries"
    )]
    pub stats_n: i64,
}

/// A Go `flag` parse failure with Go's error text (shown as `invalid value ... : TEXT`).
#[derive(Debug)]
pub struct GoParseError(String);

impl std::fmt::Display for GoParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GoParseError {}

fn parse_status_code(value: &str) -> std::result::Result<i64, GoParseError> {
    value.parse().map_err(|_| {
        GoParseError(format!(
            "strconv.Atoi: parsing {}: invalid syntax",
            serde_json::to_string(value).unwrap_or_default()
        ))
    })
}

fn parse_duration_flag(value: &str) -> std::result::Result<i64, GoParseError> {
    stream::parse_go_duration(value).ok_or_else(|| GoParseError("parse error".into()))
}

// ---------------------------------------------------------------------------
// shared STREAM helpers
// ---------------------------------------------------------------------------

/// mc `showCommandHelpAndExit(ctx, 1)`: the command's help on stdout, exit status 1.
pub(crate) fn help_exit(path: &[&str]) -> Result<()> {
    use clap::CommandFactory;
    let mut cmd = crate::cli::Cli::command();
    for name in path {
        match cmd.find_subcommand(name) {
            Some(sub) => cmd = sub.clone(),
            None => break,
        }
    }
    let _ = cmd.print_help();
    Err(Exit(1).into())
}

/// mc `newAdminClient` failure message used by trace/logs/heal.
pub(crate) fn admin_client(target: &str) -> Result<AdminClient> {
    let store = crate::config::ConfigStore::load_or_create()?;
    admin::admin_client_for(&store, target)
        .map_err(|err| relabel(err, "Unable to initialize admin client."))
}

/// Replaces the outer context of `err` with `message`, keeping mc's typed cause.
pub(crate) fn relabel(err: anyhow::Error, message: &'static str) -> anyhow::Error {
    let cause = match crate::error::mc_error(&err) {
        Some(mc) => mc.clone(),
        None => McError::new(err.root_cause().to_string()),
    };
    anyhow::Error::new(cause).context(message)
}

/// Resolves with mc's exit status for SIGINT (130) or SIGTERM (143).
pub(crate) async fn interrupted() -> i32 {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    let terminate = async {
        match term.as_mut() {
            Some(term) => {
                term.recv().await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => 130,
        _ = terminate => 143,
    }
}

/// Writes one line to stdout (flushed).
pub(crate) fn emit(text: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{text}")?;
    out.flush()?;
    Ok(())
}

/// Maps a closed-stdout error to a silent exit.
pub(crate) fn quiet_pipe(result: Result<()>) -> Result<()> {
    match result {
        Err(err) if admin::is_broken_pipe(&err) => Err(Exit(0).into()),
        other => other,
    }
}

/// Go error text of an OS error (`no such file or directory`).
pub(crate) fn go_os_error(err: &std::io::Error) -> String {
    let text = err.to_string();
    let text = text.split(" (os error").next().unwrap_or(&text);
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_ascii_lowercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

/// Go `*os.PathError` as mc's cause: `OP PATH: TEXT`, detail `{"Op","Path","Err"}`.
pub(crate) fn path_error(op: &str, path: &str, err: &std::io::Error) -> McError {
    McError::with_detail(
        format!("{op} {path}: {}", go_os_error(err)),
        crate::detail![
            ("Op", op),
            ("Path", path),
            ("Err", err.raw_os_error().unwrap_or_default())
        ],
    )
}

/// True when `--json` was given at the level of `parent`'s subcommand (after the `parent`
/// token) or through `MC_JSON`: mc then prints JSON lines on a non-terminal. With `--json`
/// only before `parent`, mc keeps its `SetIndent` encoders' indented output.
fn json_line(parent: &str) -> bool {
    if output::stdout_is_terminal() {
        return false;
    }
    let env = std::env::var("MC_JSON").unwrap_or_default();
    if matches!(env.as_str(), "1" | "t" | "T" | "true" | "TRUE" | "True") {
        return true;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(at) = args.iter().position(|arg| arg == parent) else {
        return false;
    };
    args[at + 1..].iter().any(|arg| {
        let name = arg.trim_start_matches('-');
        arg.starts_with('-') && (name == "json" || name == "json=true" || name == "json=1")
    })
}

/// Trace JSON: Go `json.Encoder` with `SetIndent("", " ")` and `SetEscapeHTML(false)`.
fn json_text<T: Serialize>(value: &T, line: bool) -> Result<String> {
    Ok(if line {
        serde_json::to_string(value)?
    } else {
        let mut buf = Vec::new();
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
        let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
        value.serialize(&mut serializer)?;
        String::from_utf8(buf)?
    })
}

// ---------------------------------------------------------------------------
// command
// ---------------------------------------------------------------------------

/// mc `traceCallTypes` names.
const CALL_TYPES: &[&str] = &[
    "storage",
    "internal",
    "s3",
    "os",
    "scanner",
    "bootstrap",
    "ilm",
    "ftp",
    "healing",
    "batch-replication",
    "batch-keyrotation",
    "batch-expiration",
    "decommission",
    "rebalance",
    "replication-resync",
];

/// Sets the `ServiceTraceOpts` flag of a call type or alias; false when unknown.
fn set_call(opts: &mut TraceOpts, name: &str) -> bool {
    match name {
        "storage" => opts.storage = true,
        "internal" => opts.internal = true,
        "s3" => opts.s3 = true,
        "os" => opts.os = true,
        "scanner" => opts.scanner = true,
        "bootstrap" => opts.bootstrap = true,
        "ilm" => opts.ilm = true,
        "ftp" => opts.ftp = true,
        "healing" | "heal" => opts.healing = true,
        "batch-replication" | "brep" => opts.batch_replication = true,
        "batch-keyrotation" | "brot" => opts.batch_key_rotation = true,
        "batch-expiration" | "bexp" => opts.batch_expire = true,
        "decommission" | "decom" => opts.decommission = true,
        "rebalance" => opts.rebalance = true,
        "replication-resync" | "resync" => opts.replication_resync = true,
        _ => return false,
    }
    true
}

/// mc `tracingOpts`.
pub fn tracing_opts(
    all: bool,
    errors: bool,
    threshold: i64,
    calls: &[String],
) -> Result<TraceOpts> {
    let mut opts = TraceOpts {
        threshold,
        only_errors: errors,
        ..Default::default()
    };
    if all {
        for name in CALL_TYPES {
            set_call(&mut opts, name);
        }
    }
    if calls.is_empty() {
        opts.s3 = true;
        return Ok(opts);
    }
    for call in calls {
        for name in call.split(',') {
            if !set_call(&mut opts, name) {
                return Err(anyhow::Error::new(McError::new(format!(
                    "unknown call name: `{name}`"
                )))
                .context("Unable to start tracing"));
            }
        }
    }
    Ok(opts)
}

/// `--request-header` / `--request-query` value (`!` negates).
#[derive(Debug, Clone, Default)]
pub struct MatchString {
    val: String,
    reverse: bool,
}

impl MatchString {
    fn parse(value: &str) -> Self {
        Self {
            reverse: value.starts_with('!'),
            val: value.trim_start_matches('!').to_string(),
        }
    }
}

/// mc `matchOpts`.
#[derive(Debug, Clone, Default)]
pub struct MatchOpts {
    pub status_codes: Vec<i64>,
    pub methods: Vec<String>,
    pub func_names: Vec<String>,
    pub api_paths: Vec<String>,
    pub nodes: Vec<String>,
    pub req_headers: Vec<MatchString>,
    pub req_queries: Vec<MatchString>,
    pub request_size: u64,
    pub response_size: u64,
}

/// Go `path.Join("/", p)` (lexically cleaned absolute path).
fn clean_abs(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

/// mc `nameMatch`: shell pattern on the base name, or a path component equal to `pattern`.
fn name_match(pattern: &str, path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    crate::commands::util::glob_match(pattern, base)
        || path.split('/').any(|component| component == pattern)
}

/// mc `patternMatch` (case-insensitive wildcard).
fn pattern_match(pattern: &str, text: &str) -> bool {
    crate::commands::util::glob_match(&pattern.to_lowercase(), &text.to_lowercase())
}

/// Go `url.ParseQuery` (pairs; `None` on a malformed escape).
fn parse_query(raw: &str) -> Option<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    for part in raw.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        pairs.push((query_unescape(key)?, query_unescape(value)?));
    }
    Some(pairs)
}

/// Go `url.QueryUnescape` (`+` is a space).
pub(crate) fn query_unescape(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = text.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

impl MatchOpts {
    /// mc `matchOpts.matches`.
    pub fn matches(&self, info: &TraceInfo) -> bool {
        let http = info.http.as_ref();
        if !self.api_paths.is_empty()
            && !self
                .api_paths
                .iter()
                .any(|p| crate::commands::util::glob_match(&clean_abs(p), &info.path))
        {
            return false;
        }
        if !self.status_codes.is_empty()
            && !self
                .status_codes
                .iter()
                .any(|code| http.is_some_and(|h| h.response.status_code == *code))
        {
            return false;
        }
        if !self.methods.is_empty()
            && !self
                .methods
                .iter()
                .any(|m| http.is_some_and(|h| &h.request.method == m))
        {
            return false;
        }
        if !self.func_names.is_empty()
            && !self
                .func_names
                .iter()
                .any(|f| name_match(f, &info.func_name))
        {
            return false;
        }
        if !self.nodes.is_empty() && !self.nodes.iter().any(|n| name_match(n, &info.node_name)) {
            return false;
        }
        if let Some(http) = http {
            if !self.req_headers.is_empty() {
                let headers = http.request.headers.clone().unwrap_or_default();
                let matched = self.req_headers.iter().any(|hdr| {
                    let found = headers.iter().any(|(name, values)| {
                        values
                            .iter()
                            .any(|v| pattern_match(&hdr.val, &format!("{name}: {v}")))
                    });
                    found != hdr.reverse
                });
                if !matched {
                    return false;
                }
            }
            if !self.req_queries.is_empty() {
                let matched = self.req_queries.iter().any(|qry| {
                    let Some(pairs) = parse_query(&http.request.raw_query) else {
                        return false;
                    };
                    let found = pairs
                        .iter()
                        .any(|(k, v)| pattern_match(&qry.val, &format!("{k}={v}")));
                    found != qry.reverse
                });
                if !matched {
                    return false;
                }
            }
            let (input, output) = (http.stats.input_bytes, http.stats.output_bytes);
            if (self.request_size > 0 && input < self.request_size as i64)
                || (self.response_size > 0 && output < self.response_size as i64)
            {
                return false;
            }
        }
        true
    }
}

fn filter_size(
    filter_request: bool,
    filter_response: bool,
    size: Option<&str>,
) -> Result<(u64, u64)> {
    let parse = |enabled: bool| -> Result<u64> {
        match (enabled, size) {
            (true, Some(size)) if !size.is_empty() => {
                admin::parse_bytes(size).context("Unable to parse input bytes.")
            }
            _ => Ok(0),
        }
    };
    Ok((parse(filter_request)?, parse(filter_response)?))
}

pub fn run(args: TraceArgs, json: bool) -> Result<()> {
    let in_file = args.in_.clone().unwrap_or_default();
    if args.target.len() != 1 && in_file.is_empty() {
        return help_exit(&["admin", "trace"]);
    }
    let filter = args.filter_request || args.filter_response;
    if filter && args.filter_size.as_deref().unwrap_or_default().is_empty() {
        return help_exit(&["admin", "trace"]);
    }
    if args.all && !args.call.is_empty() {
        return Err(anyhow::Error::new(McError::new(""))
            .context("You cannot specify both --all and --call flags at the same time."));
    }
    let target = args.target.first().cloned().unwrap_or_default();
    let (client, opts) = if in_file.is_empty() {
        let client = admin_client(&target)?;
        let opts = tracing_opts(args.all, args.errors, args.response_duration, &args.call)?;
        (Some(client), opts)
    } else {
        (None, TraceOpts::default())
    };
    let (request_size, response_size) = filter_size(
        args.filter_request,
        args.filter_response,
        args.filter_size.as_deref(),
    )?;
    let mopts = MatchOpts {
        status_codes: args.status_code.clone(),
        methods: args.method.clone(),
        func_names: args.funcname.clone(),
        api_paths: args.path.clone(),
        nodes: args.node.clone(),
        req_headers: args
            .request_header
            .iter()
            .map(|s| MatchString::parse(s))
            .collect(),
        req_queries: args
            .request_query
            .iter()
            .map(|s| MatchString::parse(s))
            .collect(),
        request_size,
        response_size,
    };
    match client {
        None => {
            let traces = read_saved(&in_file)?;
            stats_ui(args.all, args.stats_n, Source::Saved(traces), &mopts)
        }
        Some(client) if args.stats => {
            stats_ui(args.all, args.stats_n, Source::Live(client, opts), &mopts)
        }
        Some(client) => {
            let style = Style {
                verbose: args.verbose,
                json,
                line: json_line("admin"),
            };
            follow(&client, &opts, &mopts, style)
        }
    }
}

/// `mx admin scanner trace` (Args in `scanner.rs`).
pub fn scanner_trace(args: ScannerTraceArgs, json: bool) -> Result<()> {
    // mc declares `--response-duration` as a boolean flag there, so a value becomes a second
    // positional argument (usage error).
    if args.response_duration.is_some() {
        return help_exit(&["admin", "scanner", "trace"]);
    }
    let filter = args.filter_request || args.filter_response;
    if filter && args.filter_size.as_deref().unwrap_or_default().is_empty() {
        return help_exit(&["admin", "scanner", "trace"]);
    }
    let client = admin_client(&args.target)?;
    let opts = tracing_opts(false, false, 0, &["scanner".to_string()])?;
    let (request_size, response_size) = filter_size(
        args.filter_request,
        args.filter_response,
        args.filter_size.as_deref(),
    )?;
    let mopts = MatchOpts {
        func_names: args.funcname.clone(),
        api_paths: args.path.clone(),
        nodes: args.node.clone(),
        request_size,
        response_size,
        ..Default::default()
    };
    let style = Style {
        verbose: args.verbose,
        json,
        line: json_line("scanner"),
    };
    follow(&client, &opts, &mopts, style)
}

#[derive(Debug, Clone, Copy)]
struct Style {
    verbose: bool,
    json: bool,
    /// JSON lines instead of indented documents.
    line: bool,
}

fn listen_error(cause: &str) -> anyhow::Error {
    anyhow::Error::new(McError::new(cause)).context("Unable to listen to http trace")
}

/// Streams traces until the server closes the stream (fatal, like mc) or a signal arrives.
fn follow(client: &AdminClient, opts: &TraceOpts, mopts: &MatchOpts, style: Style) -> Result<()> {
    crate::commands::runtime()?.block_on(async {
        let signal = interrupted();
        tokio::pin!(signal);
        let mut traces = tokio::select! {
            code = &mut signal => return Err(Exit(code).into()),
            traces = stream::trace(client, opts) => {
                traces.map_err(|err| relabel(err, "Unable to listen to http trace"))?
            }
        };
        loop {
            let next = tokio::select! {
                code = &mut signal => return Err(Exit(code).into()),
                next = traces.next::<TraceInfo>() => next,
            };
            let info = match next {
                Ok(Some(info)) => info.normalize(),
                Ok(None) => return Err(listen_error("EOF")),
                Err(err) if err.to_string().contains("end of JSON") => {
                    return Err(listen_error("unexpected EOF"));
                }
                Err(err) => return Err(relabel(err, "Unable to listen to http trace")),
            };
            if !mopts.matches(&info) {
                continue;
            }
            let text = format_trace(&info, style)?;
            quiet_pipe(emit(&text))?;
        }
    })
}

fn format_trace(info: &TraceInfo, style: Style) -> Result<String> {
    Ok(match (style.verbose, style.json) {
        (false, false) => short_trace(info).text(),
        (false, true) => {
            let mut short = short_trace(info);
            short.status = "success".into();
            json_text(&short, style.line)?
        }
        (true, false) => verbose_text(info),
        (true, true) => json_text(&verbose_trace(info), style.line)?,
    })
}

// ---------------------------------------------------------------------------
// short trace
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallStats {
    #[serde(default)]
    pub rx: i64,
    #[serde(default)]
    pub tx: i64,
    #[serde(default)]
    pub duration: i64,
    #[serde(rename = "timeToFirstByte", default)]
    pub ttfb: i64,
}

/// mc `shortTraceMsg` (also the `--in` replay record).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShortTrace {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub host: String,
    #[serde(default = "zero_time")]
    pub time: String,
    #[serde(default)]
    pub client: String,
    #[serde(rename = "callStats", default, skip_serializing_if = "Option::is_none")]
    pub call_stats: Option<CallStats>,
    #[serde(default)]
    pub duration: i64,
    #[serde(rename = "timeToFirstByte", default)]
    pub ttfb: i64,
    #[serde(rename = "api", default)]
    pub func_name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub query: String,
    #[serde(rename = "statusCode", default)]
    pub status_code: i64,
    #[serde(rename = "statusMsg", default)]
    pub status_msg: String,
    #[serde(rename = "type", default)]
    pub type_name: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub size: i64,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub extra: Option<BTreeMap<String, String>>,
    #[serde(skip)]
    pub trace_type: u64,
}

fn zero_time() -> String {
    stream::ZERO_TIME.to_string()
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// mc `shortTrace`.
pub fn short_trace(info: &TraceInfo) -> ShortTrace {
    let mut s = ShortTrace {
        trace_type: info.trace_type,
        type_name: stream::trace_type_name(info.trace_type),
        func_name: info.func_name.clone(),
        time: info.time.clone(),
        path: info.path.clone(),
        error: info.error.clone(),
        host: info.node_name.clone(),
        duration: info.duration,
        status_msg: info.message.clone(),
        extra: info.custom.clone(),
        size: info.bytes,
        ..Default::default()
    };
    if matches!(info.trace_type, TRACE_S3 | TRACE_INTERNAL) {
        let http = info.http.clone().unwrap_or_default();
        s.query = http.request.raw_query.clone();
        s.status_code = http.response.status_code;
        s.status_msg = status_text(http.response.status_code).to_string();
        s.client = http.request.client.clone();
        s.call_stats = Some(CallStats {
            duration: info.duration,
            rx: http.stats.input_bytes,
            tx: http.stats.output_bytes,
            ttfb: http.stats.time_to_first_byte,
        });
    }
    s
}

/// Go `fmt.Sprintf("%*s", width, " ")`.
fn pad(width: i64) -> String {
    " ".repeat(width.unsigned_abs().max(1) as usize)
}

fn ibytes_i(value: i64) -> String {
    ibytes(value as u64)
}

impl ShortTrace {
    /// mc `shortTraceMsg.String()` (without colors).
    pub fn text(&self) -> String {
        let mut b = format!("{} ", trace_time(&self.time));
        let upper = self.type_name.to_uppercase();
        match self.trace_type {
            TRACE_S3 | TRACE_INTERNAL => {}
            TRACE_BOOTSTRAP => {
                b.push_str(&format!(
                    "[{upper}] {} {} {}",
                    self.func_name, self.host, self.status_msg
                ));
                return b;
            }
            _ => {
                let dur = format!("{:>2}", go_duration(self.duration));
                if !self.error.is_empty() {
                    b.push_str(&format!(
                        "[{upper}] {} {} {} err='{}' {dur}",
                        self.func_name, self.host, self.path, self.error
                    ));
                } else {
                    let size = if self.size != 0 {
                        format!(" {}", ibytes_i(self.size))
                    } else {
                        String::new()
                    };
                    b.push_str(&format!(
                        "[{upper}] {} {} {} {dur}{size}",
                        self.func_name, self.host, self.path
                    ));
                }
                return b;
            }
        }
        let stats = self.call_stats.clone().unwrap_or_default();
        b.push_str(&format!(
            "[{} {}] {} ",
            self.status_code, self.status_msg, self.func_name
        ));
        b.push_str(&format!("{}{}", self.host, self.path));
        if !self.query.is_empty() {
            b.push_str(&format!("?{} ", self.query));
        }
        b.push_str(&format!(" {} ", self.client));
        b.push_str(&pad(15 - self.client.len() as i64));
        let dur = format!("{:>2}", go_duration(round_duration(stats.duration, 1_000)));
        b.push_str(&format!(" {dur}"));
        b.push_str(&pad(12 - dur.len() as i64));
        b.push_str(" ⇣ ");
        let ttfb = format!("{:>2}", go_duration(stats.ttfb));
        b.push_str(&format!(" {ttfb}"));
        b.push_str(&pad(10 - ttfb.len() as i64));
        b.push_str(" ↑ ");
        b.push_str(&ibytes_i(stats.rx));
        b.push_str(" ↓ ");
        b.push_str(&ibytes_i(stats.tx));
        b
    }
}

// ---------------------------------------------------------------------------
// verbose trace
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct RequestInfo {
    time: String,
    proto: String,
    method: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    path: String,
    #[serde(rename = "rawQuery", skip_serializing_if = "String::is_empty")]
    raw_query: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    body: String,
}

#[derive(Debug, Serialize)]
struct ResponseInfo {
    time: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    body: String,
    #[serde(rename = "statusCode", skip_serializing_if = "is_zero")]
    status_code: i64,
}

/// mc `verboseTrace` (verbose `--json`).
#[derive(Debug, Serialize)]
struct VerboseTrace {
    #[serde(rename = "type")]
    type_name: String,
    host: String,
    api: String,
    time: String,
    duration: i64,
    path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    error: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    request: Option<RequestInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<ResponseInfo>,
    #[serde(rename = "callStats", skip_serializing_if = "Option::is_none")]
    call_stats: Option<CallStats>,
    #[serde(rename = "healResult", skip_serializing_if = "Option::is_none")]
    heal_result: Option<HealResultItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<BTreeMap<String, String>>,
}

fn joined(headers: &Option<BTreeMap<String, Vec<String>>>, sep: &str) -> BTreeMap<String, String> {
    headers
        .iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.join(sep)))
        .collect()
}

fn verbose_trace(info: &TraceInfo) -> VerboseTrace {
    let mut trc = VerboseTrace {
        type_name: stream::trace_type_name(info.trace_type),
        host: info.node_name.clone(),
        api: info.func_name.clone(),
        time: info.time.clone(),
        duration: info.duration,
        path: info.path.clone(),
        error: info.error.clone(),
        message: info.message.clone(),
        request: None,
        response: None,
        call_stats: None,
        heal_result: info.heal_result.clone(),
        extra: info.custom.clone().filter(|m| !m.is_empty()),
    };
    if let Some(http) = &info.http {
        let (rq, rs) = (&http.request, &http.response);
        trc.request = Some(RequestInfo {
            time: rq.time.clone(),
            proto: rq.proto.clone(),
            method: rq.method.clone(),
            path: rq.path.clone(),
            raw_query: rq.raw_query.clone(),
            headers: joined(&rq.headers, " "),
            body: String::from_utf8_lossy(&rq.body).into_owned(),
        });
        trc.response = Some(ResponseInfo {
            time: rs.time.clone(),
            headers: joined(&rs.headers, " "),
            body: String::from_utf8_lossy(&rs.body).into_owned(),
            status_code: rs.status_code,
        });
        trc.call_stats = Some(CallStats {
            duration: info.duration,
            rx: http.stats.input_bytes,
            tx: http.stats.output_bytes,
            ttfb: http.stats.time_to_first_byte,
        });
    }
    trc
}

/// mc `traceMessage.String()` (without colors; headers in sorted order).
fn verbose_text(info: &TraceInfo) -> String {
    let node = if info.node_name.is_empty() {
        String::new()
    } else {
        format!("{} ", info.node_name)
    };
    let extra: String = info
        .custom
        .iter()
        .flatten()
        .map(|(k, v)| format!(" {k}={v}"))
        .collect();
    let upper = stream::trace_type_name(info.trace_type).to_uppercase();
    let time = trace_time(&info.time);
    match info.trace_type {
        TRACE_S3 | TRACE_INTERNAL => {
            if info.http.is_none() {
                return String::new();
            }
        }
        TRACE_BOOTSTRAP => {
            return format!(
                "{node} [{upper} {}] [{time}] {}{extra}",
                info.func_name, info.message
            );
        }
        _ => {
            let size = if info.bytes != 0 {
                format!(" {}", ibytes_i(info.bytes))
            } else {
                String::new()
            };
            let dur = go_duration(info.duration);
            return if !info.error.is_empty() {
                format!(
                    "{node} [{upper} {}] [{time}] {}{extra} err='{}' {dur}{size}",
                    info.func_name, info.path, info.error
                )
            } else {
                format!(
                    "{node} [{upper} {}] [{time}] {}{extra} {dur}{size}",
                    info.func_name, info.path
                )
            };
        }
    }
    let http = info.http.clone().unwrap_or_default();
    let (ri, rs) = (&http.request, &http.response);
    let mut b = String::new();
    b.push_str(&format!("{node}[REQUEST {}] ", info.func_name));
    b.push_str(&format!(
        "[{}] [Client IP: {}]\n",
        trace_time(&ri.time),
        ri.client
    ));
    b.push_str(&format!("{node}{} {}", ri.method, ri.path));
    if !ri.raw_query.is_empty() {
        b.push_str(&format!("?{}", ri.raw_query));
    }
    b.push('\n');
    b.push_str(&format!("{node}Proto: {}\n", ri.proto));
    let mut headers = ri.headers.clone().unwrap_or_default();
    let host = headers.remove("Host").unwrap_or_default().join("");
    b.push_str(&format!("{node}Host: {host}\n"));
    for (k, v) in &headers {
        b.push_str(&format!("{node}{k}: {}\n", v.join("")));
    }
    b.push_str(&format!("{node}{}\n", String::from_utf8_lossy(&ri.body)));
    b.push_str(&format!("{node}[RESPONSE] "));
    b.push_str(&format!("[{}] ", trace_time(&rs.time)));
    b.push_str(&format!(
        "[ Duration {:>2} TTFB {:>2} ↑ {}  ↓ {} ]\n",
        go_duration(round_duration(info.duration, 1_000)),
        go_duration(http.stats.time_to_first_byte),
        ibytes_i(http.stats.input_bytes),
        ibytes_i(http.stats.output_bytes)
    ));
    b.push_str(&format!(
        "{node}{} {}\n",
        rs.status_code,
        status_text(rs.status_code)
    ));
    for (k, v) in rs.headers.iter().flatten() {
        b.push_str(&format!("{node}{k}: {}\n", v.join(",")));
    }
    if !extra.is_empty() {
        b.push_str(&format!("{node}{extra}\n"));
    }
    b.push_str(&format!("{node}{}\n", String::from_utf8_lossy(&rs.body)));
    b.push_str(&node);
    b
}

// ---------------------------------------------------------------------------
// --stats / --in
// ---------------------------------------------------------------------------

enum Source {
    Live(AdminClient, TraceOpts),
    Saved(Vec<TraceInfo>),
}

/// `--in FILE`: mc's saved short-trace JSON lines (bootstrap records skipped).
fn read_saved(path: &str) -> Result<Vec<TraceInfo>> {
    let text = std::fs::read(path)
        .map_err(|err| anyhow::Error::new(path_error("open", path, &err)))
        .context("Unable to open input")?;
    if path.ends_with(".zst") {
        return Err(
            anyhow::Error::new(McError::new("zstd compressed input is not supported"))
                .context("Unable to open input"),
        );
    }
    let mut traces = Vec::new();
    for line in String::from_utf8_lossy(&text).lines() {
        let Ok(t) = serde_json::from_str::<ShortTrace>(line) else {
            continue;
        };
        if t.type_name == "Bootstrap" {
            continue;
        }
        traces.push(TraceInfo {
            trace_type: stream::find_trace_type(&t.type_name),
            node_name: t.host,
            func_name: t.func_name,
            time: t.time,
            path: t.path,
            duration: t.duration,
            bytes: t.size,
            message: t.status_msg,
            error: t.error,
            custom: t.extra,
            ..Default::default()
        });
    }
    Ok(traces)
}

/// mc renders `--stats` with bubbletea, which needs a terminal: with a non-terminal stdin it
/// opens `/dev/tty` and fails without one.
fn tty_error() -> Option<anyhow::Error> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    let err = std::fs::File::open("/dev/tty").err()?;
    Some(
        anyhow::Error::new(McError::new(format!(
            "could not open a new TTY: open /dev/tty: {}",
            go_os_error(&err)
        )))
        .context("Unable to fetch http trace statistics"),
    )
}

/// mc `statItem`.
#[derive(Debug, Clone, Default)]
struct StatItem {
    name: String,
    count: i64,
    duration: i64,
    errors: i64,
    call_stats_count: i64,
    rx: i64,
    tx: i64,
    ttfb: i64,
    max_ttfb: i64,
    max_dur: i64,
    min_dur: i64,
    size: i64,
}

/// mc `statTrace`.
#[derive(Debug, Default)]
struct StatTrace {
    calls: BTreeMap<String, StatItem>,
    /// Unix nanoseconds.
    oldest: i128,
    latest: i128,
}

fn time_nanos(text: &str) -> i128 {
    stream::parse_time(text)
        .map(|(s, n)| s as i128 * 1_000_000_000 + n as i128)
        .unwrap_or(0)
}

impl StatTrace {
    fn add(&mut self, t: &TraceInfo) {
        if t.trace_type != TRACE_BOOTSTRAP {
            let ended = time_nanos(&t.time) + t.duration as i128;
            if self.oldest == 0 {
                self.oldest = ended;
            }
            if ended > self.latest {
                self.latest = ended;
            }
        }
        let got = self.calls.entry(t.func_name.clone()).or_default();
        if got.name.is_empty() {
            got.name = t.func_name.clone();
        }
        got.max_dur = got.max_dur.max(t.duration);
        if got.min_dur <= 0 || got.min_dur > t.duration {
            got.min_dur = t.duration;
        }
        got.count += 1;
        got.duration += t.duration;
        if !t.error.is_empty() {
            got.errors += 1;
        }
        got.size += t.bytes;
        if let Some(http) = &t.http {
            got.call_stats_count += 1;
            got.rx += http.stats.input_bytes;
            got.tx += http.stats.output_bytes;
            got.ttfb += http.stats.time_to_first_byte;
            got.max_ttfb = got.max_ttfb.max(http.stats.time_to_first_byte);
        }
    }

    /// mc `traceStatsUI.View()` (no colors, no spinner).
    fn view(&self, all: bool, max_entries: i64) -> String {
        let dur_nanos = (self.latest - self.oldest) as i64;
        let minutes = dur_nanos as f64 / 60e9;
        let mut s = format!(
            "Duration: {}\n",
            go_duration(round_duration(dur_nanos, 1_000_000_000))
        );
        let mut entries: Vec<&StatItem> = self.calls.values().collect();
        if entries.is_empty() {
            s.push_str("(waiting for data)");
            return s;
        }
        let total: i64 = entries.iter().map(|e| e.count).sum();
        let total_rx: i64 = entries.iter().map(|e| e.rx).sum();
        let total_tx: i64 = entries.iter().map(|e| e.tx).sum();
        entries.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
        let mut end_trunc = false;
        if max_entries > 0 && entries.len() as i64 > max_entries {
            entries.truncate(max_entries as usize);
            end_trunc = true;
        }
        let has_ttfb = entries.iter().any(|e| e.ttfb > 0);
        if !all {
            if total_rx > 0 {
                s.push_str(&format!(
                    "RX Rate:↑ {}/m\n",
                    ibytes((total_rx as f64 / minutes) as u64)
                ));
            }
            if total_tx > 0 {
                s.push_str(&format!(
                    "TX Rate:↓ {}/m\n",
                    ibytes((total_tx as f64 / minutes) as u64)
                ));
            }
        }
        s.push_str(&format!("RPM    :  {:.1}\n", total as f64 / minutes));
        s.push_str("-------------\n");
        let mut header = vec!["Call", "Count", "RPM", "Avg Time", "Min Time", "Max Time"];
        if has_ttfb {
            header.extend(["Avg TTFB", "Max TTFB"]);
        }
        header.extend(["Avg Size", "Rate /min", "Errors"]);
        let mut rows: Vec<Vec<String>> = vec![header.iter().map(|h| h.to_string()).collect()];
        let last = entries.len().saturating_sub(1);
        for (i, v) in entries.iter().enumerate() {
            if v.count <= 0 {
                continue;
            }
            let avg = v.duration / v.count;
            let mut sz = "-".to_string();
            let mut rate = "-".to_string();
            if v.size > 0 {
                sz = ibytes_short((v.size / v.count) as u64);
                rate = ibytes_short((v.size as f64 / minutes) as u64);
            }
            if v.call_stats_count > 0 {
                let (mut s, mut r) = (Vec::new(), Vec::new());
                if v.rx > 0 {
                    s.push(format!(
                        "↑{}",
                        ibytes_short((v.rx / v.call_stats_count) as u64)
                    ));
                    r.push(format!("↑{}", ibytes_short((v.rx as f64 / minutes) as u64)));
                }
                if v.tx > 0 {
                    s.push(format!(
                        "↓{}",
                        ibytes_short((v.tx / v.call_stats_count) as u64)
                    ));
                    r.push(format!("↓{}", ibytes_short((v.tx as f64 / minutes) as u64)));
                }
                if !s.is_empty() {
                    sz = s.join(" ");
                }
                if !r.is_empty() {
                    rate = r.join(" ");
                }
            }
            let pre = if end_trunc && i == last { "↓ " } else { "" };
            let mut row = vec![
                format!("{pre}{}", v.name),
                format!(
                    "{} ({:.1}%)",
                    v.count,
                    v.count as f64 / total as f64 * 100.0
                ),
                format!("{:.1}", v.count as f64 / minutes),
                go_duration(round_dur(avg)),
                go_duration(round_dur(v.min_dur)),
                go_duration(round_dur(v.max_dur)),
            ];
            if has_ttfb {
                if v.ttfb > 0 {
                    row.push(go_duration(round_dur(v.ttfb / v.count)));
                    row.push(go_duration(round_dur(v.max_ttfb)));
                } else {
                    row.extend(["-".to_string(), "-".to_string()]);
                }
            }
            row.extend([sz, rate, v.errors.to_string()]);
            rows.push(row);
        }
        let widths: Vec<usize> = (0..rows[0].len())
            .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
            .collect();
        for row in rows {
            let line: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(c, cell)| {
                    format!(
                        "{cell}{}",
                        " ".repeat(widths[c].saturating_sub(cell.chars().count()))
                    )
                })
                .collect();
            s.push_str(line.join("  ").trim_end());
            s.push('\n');
        }
        s
    }
}

/// mc `ibytesShort`.
fn ibytes_short(value: u64) -> String {
    ibytes(value).trim_end_matches("iB").replace(' ', "")
}

/// mc `roundDur`.
fn round_dur(d: i64) -> i64 {
    const MS: i64 = 1_000_000;
    const S: i64 = 1_000_000_000;
    if d > 60 * S {
        round_duration(d, S)
    } else if d > S {
        round_duration(d, MS)
    } else if d > MS {
        round_duration(d, MS / 10)
    } else {
        round_duration(d, 1_000)
    }
}

/// `--stats`: redraws the statistics table every second until interrupted; for `--in` the
/// final table is printed once the file is consumed.
fn stats_ui(all: bool, max_entries: i64, source: Source, mopts: &MatchOpts) -> Result<()> {
    if let Some(err) = tty_error() {
        return Err(err);
    }
    let mut stats = StatTrace::default();
    let (client, opts) = match source {
        Source::Saved(traces) => {
            for t in traces.iter().filter(|t| mopts.matches(t)) {
                stats.add(t);
            }
            return quiet_pipe(emit(&stats.view(all, max_entries)));
        }
        Source::Live(client, opts) => (client, opts),
    };
    let failed = |err: anyhow::Error| relabel(err, "Unable to fetch http trace statistics");
    crate::commands::runtime()?.block_on(async {
        let signal = interrupted();
        tokio::pin!(signal);
        let mut traces = stream::trace(&client, &opts).await.map_err(failed)?;
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        let mut drawn = 0usize;
        loop {
            tokio::select! {
                code = &mut signal => return Err(Exit(code).into()),
                _ = tick.tick() => {
                    let view = stats.view(all, max_entries);
                    let mut out = std::io::stdout().lock();
                    if drawn > 0 {
                        let _ = write!(out, "\x1b[{drawn}A\x1b[J");
                    }
                    drawn = view.lines().count();
                    let _ = writeln!(out, "{}", view.trim_end_matches('\n'));
                    let _ = out.flush();
                }
                next = traces.next::<TraceInfo>() => match next {
                    Ok(Some(info)) => {
                        let info = info.normalize();
                        if mopts.matches(&info) {
                            stats.add(&info);
                        }
                    }
                    Ok(None) => return Err(failed(anyhow::anyhow!("EOF"))),
                    Err(err) => return Err(failed(err)),
                },
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::admin_stream::{
        TraceCallStats, TraceHttpStats, TraceRequestInfo, TraceResponseInfo,
    };

    fn s3_info() -> TraceInfo {
        let mut headers = BTreeMap::new();
        headers.insert("Host".to_string(), vec!["127.0.0.1:9000".to_string()]);
        headers.insert(
            "X-Amz-Date".to_string(),
            vec!["20260101T000000Z".to_string()],
        );
        let mut resp_headers = BTreeMap::new();
        resp_headers.insert(
            "Vary".to_string(),
            vec!["Origin".to_string(), "Accept-Encoding".to_string()],
        );
        TraceInfo {
            trace_type: TRACE_S3,
            node_name: "127.0.0.1:9000".into(),
            func_name: "s3.PutBucket".into(),
            time: "2026-09-26T19:26:46.434305953Z".into(),
            path: "/bucket2/".into(),
            duration: 3_067_748,
            bytes: 93,
            http: Some(TraceHttpStats {
                request: TraceRequestInfo {
                    time: "2026-09-26T19:26:46.434305953Z".into(),
                    proto: "HTTP/1.1".into(),
                    method: "PUT".into(),
                    path: "/bucket2/".into(),
                    headers: Some(headers),
                    client: "172.17.0.1".into(),
                    ..Default::default()
                },
                response: TraceResponseInfo {
                    time: "2026-09-26T19:26:46.437372055Z".into(),
                    headers: Some(resp_headers),
                    status_code: 200,
                    ..Default::default()
                },
                stats: TraceCallStats {
                    input_bytes: 93,
                    output_bytes: 0,
                    time_to_first_byte: 2_998_868,
                    ..Default::default()
                },
            }),
            ..Default::default()
        }
    }

    #[test]
    fn short_s3_line_matches_mc_layout() {
        assert_eq!(
            short_trace(&s3_info()).text(),
            "2026-09-26T19:26:46.434 [200 OK] s3.PutBucket 127.0.0.1:9000/bucket2/ 172.17.0.1       3.068ms      ⇣  2.998868ms  ↑ 93 B ↓ 0 B"
        );
        let mut info = s3_info();
        info.duration = 189_751;
        let http = info.http.as_mut().unwrap();
        http.request.raw_query = "location=".into();
        http.stats.time_to_first_byte = 178_273;
        http.stats.output_bytes = 128;
        assert_eq!(
            short_trace(&info).text(),
            "2026-09-26T19:26:46.434 [200 OK] s3.PutBucket 127.0.0.1:9000/bucket2/?location=  172.17.0.1       190µs       ⇣  178.273µs  ↑ 93 B ↓ 128 B"
        );
    }

    #[test]
    fn short_json_keeps_mc_fields() {
        let mut short = short_trace(&s3_info());
        short.status = "success".into();
        assert_eq!(
            json_text(&short, true).unwrap(),
            r#"{"status":"success","host":"127.0.0.1:9000","time":"2026-09-26T19:26:46.434305953Z","client":"172.17.0.1","callStats":{"rx":93,"tx":0,"duration":3067748,"timeToFirstByte":2998868},"duration":3067748,"timeToFirstByte":0,"api":"s3.PutBucket","path":"/bucket2/","query":"","statusCode":200,"statusMsg":"OK","type":"S3","size":93,"error":"","extra":null}"#
        );
    }

    #[test]
    fn non_http_short_and_verbose_lines() {
        let info = TraceInfo {
            trace_type: 1 << 4,
            node_name: "n1".into(),
            func_name: "scanner.ScanObject".into(),
            time: "2026-01-01T00:00:00.5Z".into(),
            path: "b/o".into(),
            duration: 1_500_000,
            bytes: 2048,
            ..Default::default()
        };
        assert_eq!(
            short_trace(&info).text(),
            "2026-01-01T00:00:00.500 [SCANNER] scanner.ScanObject n1 b/o 1.5ms 2.0 KiB"
        );
        assert_eq!(
            verbose_text(&info),
            "n1  [SCANNER scanner.ScanObject] [2026-01-01T00:00:00.500] b/o 1.5ms 2.0 KiB"
        );
        let err = TraceInfo {
            error: "boom".into(),
            ..info
        };
        assert_eq!(
            short_trace(&err).text(),
            "2026-01-01T00:00:00.500 [SCANNER] scanner.ScanObject n1 b/o err='boom' 1.5ms"
        );
    }

    #[test]
    fn verbose_http_text_and_json() {
        let text = verbose_text(&s3_info());
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(
            lines[0],
            "127.0.0.1:9000 [REQUEST s3.PutBucket] [2026-09-26T19:26:46.434] [Client IP: 172.17.0.1]"
        );
        assert_eq!(lines[1], "127.0.0.1:9000 PUT /bucket2/");
        assert_eq!(lines[3], "127.0.0.1:9000 Host: 127.0.0.1:9000");
        assert_eq!(lines[4], "127.0.0.1:9000 X-Amz-Date: 20260101T000000Z");
        assert_eq!(
            lines[6],
            "127.0.0.1:9000 [RESPONSE] [2026-09-26T19:26:46.437] [ Duration 3.068ms TTFB 2.998868ms ↑ 93 B  ↓ 0 B ]"
        );
        assert_eq!(lines[7], "127.0.0.1:9000 200 OK");
        assert_eq!(lines[8], "127.0.0.1:9000 Vary: Origin,Accept-Encoding");
        assert_eq!(*lines.last().unwrap(), "127.0.0.1:9000 ");
        let json = json_text(&verbose_trace(&s3_info()), true).unwrap();
        assert!(
            json.starts_with(
                r#"{"type":"S3","host":"127.0.0.1:9000","api":"s3.PutBucket","time":"2026-09-26T19:26:46.434305953Z","duration":3067748,"path":"/bucket2/","request":{"time":"2026-09-26T19:26:46.434305953Z","proto":"HTTP/1.1","method":"PUT","path":"/bucket2/","headers":{"Host":"127.0.0.1:9000","X-Amz-Date":"20260101T000000Z"}},"response":{"time":"2026-09-26T19:26:46.437372055Z","headers":{"Vary":"Origin Accept-Encoding"},"statusCode":200},"callStats":"#
            ),
            "{json}"
        );
    }

    #[test]
    fn tracing_opts_follow_mc() {
        let opts = tracing_opts(false, true, 5, &[]).unwrap();
        assert!(opts.s3 && opts.only_errors && opts.threshold == 5);
        let opts = tracing_opts(false, false, 0, &["heal,brep".into(), "os".into()]).unwrap();
        assert!(opts.healing && opts.batch_replication && opts.os && !opts.s3);
        let opts = tracing_opts(true, false, 0, &[]).unwrap();
        assert!(opts.s3 && opts.storage && opts.replication_resync && opts.bootstrap);
        let err = tracing_opts(false, false, 0, &["bogus".into()]).unwrap_err();
        assert_eq!(
            output::split_error(&err),
            (
                "Unable to start tracing".to_string(),
                "unknown call name: `bogus`".to_string()
            )
        );
    }

    #[test]
    fn match_opts_filter_like_mc() {
        let info = s3_info();
        let mut m = MatchOpts {
            api_paths: vec!["bucket2/*".into()],
            ..Default::default()
        };
        assert!(m.matches(&info));
        m.api_paths = vec!["other/*".into()];
        assert!(!m.matches(&info));
        let m = MatchOpts {
            status_codes: vec![404],
            ..Default::default()
        };
        assert!(!m.matches(&info));
        let m = MatchOpts {
            methods: vec!["PUT".into()],
            func_names: vec!["s3.Put*".into()],
            ..Default::default()
        };
        assert!(m.matches(&info));
        let m = MatchOpts {
            req_headers: vec![MatchString::parse("!x-amz-date: *")],
            ..Default::default()
        };
        assert!(!m.matches(&info));
        let m = MatchOpts {
            request_size: 1000,
            ..Default::default()
        };
        assert!(!m.matches(&info));
        let mut q = s3_info();
        q.http.as_mut().unwrap().request.raw_query = "prefix=a%2Fb&x=1".into();
        let m = MatchOpts {
            req_queries: vec![MatchString::parse("prefix=a/*")],
            ..Default::default()
        };
        assert!(m.matches(&q));
    }

    #[test]
    fn stats_view_lists_calls() {
        let mut stats = StatTrace::default();
        stats.add(&s3_info());
        let mut other = s3_info();
        other.time = "2026-09-26T19:27:46.434305953Z".into();
        stats.add(&other);
        let view = stats.view(false, 20);
        assert!(view.starts_with("Duration: 1m0s\n"), "{view}");
        assert!(view.contains("RPM    :  2.0\n"), "{view}");
        assert!(view.contains("s3.PutBucket  2 (100.0%)"), "{view}");
    }

    #[test]
    fn go_parse_errors() {
        assert_eq!(
            parse_status_code("abc").unwrap_err().to_string(),
            "strconv.Atoi: parsing \"abc\": invalid syntax"
        );
        assert_eq!(parse_duration_flag("5ms").unwrap(), 5_000_000);
        assert_eq!(
            parse_duration_flag("x").unwrap_err().to_string(),
            "parse error"
        );
        assert_eq!(query_unescape("a%2Fb+c").unwrap(), "a/b c");
        assert_eq!(clean_abs("my-bucket//x/./y/"), "/my-bucket/x/y");
    }
}
