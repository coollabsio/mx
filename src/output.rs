//! Output helpers shared by all commands: mc-style JSON printing (`printMsg`) and
//! mc-style error reporting (`fatalIf` / `errorIf`). Global settings live in [`crate::globals`].

use serde::Serialize;
use std::io::IsTerminal;
use std::sync::OnceLock;

pub fn quiet() -> bool {
    crate::globals::quiet()
}

pub fn insecure() -> bool {
    crate::globals::insecure()
}

pub fn print_plain(message: &str) {
    if !quiet() {
        println!("{message}");
    }
}

/// Program name as invoked (basename of argv[0]), like mc's `console.ProgramName()`.
/// Prints `mc` when the binary is installed as `mc`.
pub fn prog_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        std::env::args_os()
            .next()
            .and_then(|path| {
                std::path::Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "mx".to_string())
    })
    .clone()
}

/// mc's app name (help, usage, `--version`): the program name with `.exe` trimmed on Windows.
/// Error/info prefixes keep [`prog_name`] (mc `console.ProgramName`).
pub fn app_name() -> String {
    let name = prog_name();
    if cfg!(windows) && name.len() > 4 && name.to_ascii_lowercase().ends_with(".exe") {
        name[..name.len() - 4].to_string()
    } else {
        name
    }
}

/// True when stdout is a terminal. mc prints indented JSON on a terminal and one compact
/// JSON document per line otherwise (`globalJSONLine`).
pub fn stdout_is_terminal() -> bool {
    static TTY: OnceLock<bool> = OnceLock::new();
    *TTY.get_or_init(|| std::io::stdout().is_terminal())
}

/// mc `isTerminal`: stdout and stderr are both terminals (gates confirmation prompts).
pub fn is_terminal() -> bool {
    stdout_is_terminal() && std::io::stderr().is_terminal()
}

/// mc's `bufio.NewReader(os.Stdin).ReadString('\n')` confirmation read: a line without
/// its newline (closed or redirected stdin) is an `EOF` error. Returns the trimmed,
/// lowercased answer.
pub fn read_answer() -> anyhow::Result<String> {
    use std::io::BufRead;
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|err| crate::error::McError::new(err.to_string()))?;
    if !answer.ends_with('\n') {
        return Err(crate::error::McError::new("EOF").into());
    }
    Ok(answer.trim().to_lowercase())
}

/// Serializes `value` like mc: compact when `line` is true (JSON lines, non-TTY), otherwise
/// Go `json.MarshalIndent(v, "", " ")` (one-space indent). Like Go, `<`, `>`, `&`, U+2028 and
/// U+2029 are escaped.
pub fn format_json<T: Serialize + ?Sized>(value: &T, line: bool) -> anyhow::Result<String> {
    let text = if line {
        serde_json::to_string(value)?
    } else {
        json_indent(value)?
    };
    Ok(go_escape(&text))
}

/// Go `json.MarshalIndent(v, "", " ")` without the TTY decision (for text-mode JSON dumps).
pub fn json_indent<T: Serialize + ?Sized>(value: &T) -> anyhow::Result<String> {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut serializer)?;
    Ok(go_escape(&String::from_utf8(buf)?))
}

/// Go's encoding/json escapes HTML characters and line/paragraph separators. These can only
/// occur inside JSON strings, so a plain replace is safe.
fn go_escape(text: &str) -> String {
    if !text.contains(['<', '>', '&', '\u{2028}', '\u{2029}']) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 16);
    for ch in text.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(ch),
        }
    }
    out
}

/// JSON document for [`print_json`], formatted for the current stdout (compact unless TTY).
pub fn json_string<T: Serialize + ?Sized>(value: &T) -> anyhow::Result<String> {
    format_json(value, !stdout_is_terminal())
}

/// mc `printMsg` for `--json`: prints one JSON document to stdout. Compact single line when
/// stdout is not a terminal, one-space indented when it is. Use this for every `--json` output.
pub fn print_json<T: Serialize + ?Sized>(value: &T) -> anyhow::Result<()> {
    println!("{}", json_string(value)?);
    Ok(())
}

// ------------------------------------------------------------------------------------------
// errors
// ------------------------------------------------------------------------------------------

/// Error severity: `Fatal` for errors that end the command (mc `fatalIf`), `Error` for errors
/// the command reports and continues after (mc `errorIf`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Fatal,
    Error,
}

impl ErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Fatal => "fatal",
            ErrorKind::Error => "error",
        }
    }
}

/// One reported error, split like mc: `message` (command context), `cause` (error text),
/// `detail` (Go-marshaled error value for JSON `cause.error`) and severity.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub message: String,
    pub cause: String,
    pub detail: crate::error::Detail,
    pub kind: ErrorKind,
}

/// Splits an anyhow error into mc's parts: the outermost context is the message, the rest of
/// the chain (joined with `: `) is the cause. A single-level error has no cause. The kind is
/// [`ErrorKind::Error`] when the message is a [`crate::error::NonFatal`] context, else `Fatal`.
/// SDK noise (`service error`, repeated texts) is dropped.
pub fn report(err: &anyhow::Error) -> Report {
    let mut chain = err.chain();
    let first = chain.next();
    let message = first.map(|e| e.to_string()).unwrap_or_default();
    let kind = if err.downcast_ref::<crate::error::NonFatal>().is_some() {
        ErrorKind::Error
    } else {
        ErrorKind::Fatal
    };
    let mut parts: Vec<String> = Vec::new();
    for cause in chain {
        let text = cause.to_string();
        if text == "service error" || text.is_empty() || parts.contains(&text) {
            continue;
        }
        let mapped = cause.downcast_ref::<crate::error::McError>().is_some();
        parts.push(text);
        // mc's typed error is the whole cause; SDK sources below it only repeat it.
        if mapped {
            break;
        }
    }
    Report {
        message,
        cause: parts.join(": "),
        detail: crate::error::mc_error(err)
            .map(|e| e.detail.clone())
            .unwrap_or_default(),
        kind,
    }
}

/// (message, cause) of [`report`].
pub fn split_error(err: &anyhow::Error) -> (String, String) {
    let report = report(err);
    (report.message, report.cause)
}

/// mc `fatal` text: `MESSAGE CAUSE` with mc's punctuation rules (without the prefix).
pub fn format_fatal_text(message: &str, cause: &str) -> String {
    let mut msg = message.trim().to_string();
    let mut errmsg = cause.trim().to_string();
    if !msg.is_empty() && !errmsg.is_empty() {
        if !msg.ends_with(':') && !msg.ends_with('.') {
            // A capitalized cause starts a new sentence.
            if errmsg.chars().next().is_some_and(char::is_uppercase) {
                msg.push('.');
            } else {
                msg.push(':');
            }
        }
        if !errmsg.ends_with('.') {
            errmsg.push('.');
        }
    }
    format!("{msg} {errmsg}")
}

/// mc error JSON document: `{"status":"error","error":{"message","cause":{"message","error"},"type"}}`.
#[derive(Debug, Serialize)]
pub struct ErrorDocument<'a> {
    pub status: &'static str,
    pub error: ErrorBody<'a>,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody<'a> {
    pub message: &'a str,
    pub cause: ErrorCause<'a>,
    #[serde(rename = "type")]
    pub kind: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ErrorCause<'a> {
    pub message: &'a str,
    /// Go marshals the wrapped `error` value; plain Go errors encode as `{}`.
    pub error: &'a crate::error::Detail,
}

pub fn error_json(report: &Report) -> ErrorDocument<'_> {
    ErrorDocument {
        status: "error",
        error: ErrorBody {
            message: &report.message,
            cause: ErrorCause {
                message: &report.cause,
                error: &report.detail,
            },
            kind: report.kind.as_str(),
        },
    }
}

/// Full error output (text line for stderr, or JSON document for stdout) without printing.
/// JSON is compact unless stdout is a terminal, like mc.
pub fn format_error(report: &Report, json: bool) -> String {
    if json {
        return json_string(&error_json(report)).unwrap_or_default();
    }
    let text = match report.kind {
        ErrorKind::Fatal => format_fatal_text(&report.message, &report.cause),
        // mc `errorIf` joins message and cause with a space, no punctuation rules.
        ErrorKind::Error => format!("{} {}", report.message, report.cause),
    };
    format!("{}: <ERROR> {text}", prog_name())
}

/// Prints an error: JSON on stdout with `--json`, else `PROG: <ERROR> ...` on stderr.
pub fn print_report(report: &Report) {
    let json = crate::globals::json();
    let text = format_error(report, json);
    if json {
        println!("{text}");
    } else {
        eprintln!("{text}");
    }
}

/// mc `errorIf` with plain texts: reports a non-fatal error and lets the command continue.
pub fn error_if(message: &str, cause: &str) {
    print_report(&Report {
        message: message.to_string(),
        cause: cause.to_string(),
        detail: Default::default(),
        kind: ErrorKind::Error,
    });
}

/// mc `errorIf` for an anyhow error (message = outermost context, cause = the rest); the
/// command continues.
pub fn print_error(err: &anyhow::Error) {
    if err.downcast_ref::<Exit>().is_some() {
        return;
    }
    let mut report = report(err);
    report.kind = ErrorKind::Error;
    print_report(&report);
}

/// Reports `err` (as `fatalIf`, or `errorIf` for a [`crate::error::NonFatal`] message) and
/// exits with status 1. [`Exit`] errors exit silently with their status.
pub fn fatal(err: &anyhow::Error) -> ! {
    if let Some(Exit(code)) = err.downcast_ref::<Exit>() {
        std::process::exit(*code);
    }
    print_report(&report(err));
    std::process::exit(1);
}

/// Error for a failure that was already reported (e.g. with [`print_error`]): `main` exits
/// with the status without printing anything.
#[derive(Debug, Clone, Copy)]
pub struct Exit(pub i32);

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit status {}", self.0)
    }
}

impl std::error::Error for Exit {}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[derive(Serialize)]
    struct Msg {
        status: &'static str,
        key: &'static str,
        list: Vec<u8>,
    }

    #[test]
    fn json_line_mode_is_compact_and_tty_mode_is_one_space_indented() {
        let msg = Msg {
            status: "success",
            key: "a<b>&c",
            list: vec![1],
        };
        assert_eq!(
            format_json(&msg, true).unwrap(),
            r#"{"status":"success","key":"a\u003cb\u003e\u0026c","list":[1]}"#
        );
        assert_eq!(
            format_json(&msg, false).unwrap(),
            "{\n \"status\": \"success\",\n \"key\": \"a\\u003cb\\u003e\\u0026c\",\n \"list\": [\n  1\n ]\n}"
        );
        assert_eq!(format_json(&serde_json::json!({}), false).unwrap(), "{}");
    }

    #[test]
    fn fatal_text_follows_mc_punctuation() {
        assert_eq!(
            format_fatal_text("Unable to list", "Access Denied"),
            "Unable to list. Access Denied."
        );
        assert_eq!(
            format_fatal_text("Unable to list", "connection refused"),
            "Unable to list: connection refused."
        );
        assert_eq!(format_fatal_text("Unable:", "x."), "Unable: x.");
        // mc prints `MSG CAUSE` without trimming: an empty cause leaves a trailing space.
        assert_eq!(
            format_fatal_text("No such alias `x` found.", ""),
            "No such alias `x` found. "
        );
        assert_eq!(format_fatal_text("", "bad value"), " bad value");
    }

    #[test]
    fn anyhow_chain_maps_to_message_and_cause() {
        let err = Err::<(), _>(anyhow::anyhow!("root cause"))
            .context("middle")
            .context("Unable to stat")
            .unwrap_err();
        assert_eq!(
            split_error(&err),
            (
                "Unable to stat".to_string(),
                "middle: root cause".to_string()
            )
        );
        let single = anyhow::anyhow!("just this");
        assert_eq!(split_error(&single), ("just this".into(), String::new()));
    }

    #[test]
    fn report_uses_mc_error_detail_and_nonfatal_kind() {
        let err = Err::<(), _>(crate::error::McError::bucket_not_found("b"))
            .context(crate::error::nonfatal("Unable to list folder."))
            .unwrap_err();
        let report = report(&err);
        assert_eq!(report.kind, ErrorKind::Error);
        assert_eq!(report.cause, "Bucket `b` does not exist.");
        assert_eq!(
            format_json(&error_json(&report), true).unwrap(),
            r#"{"status":"error","error":{"message":"Unable to list folder.","cause":{"message":"Bucket `b` does not exist.","error":{"Bucket":"b"}},"type":"error"}}"#
        );
        let prog = prog_name();
        assert_eq!(
            format_error(&report, false),
            format!("{prog}: <ERROR> Unable to list folder. Bucket `b` does not exist.")
        );
    }

    #[test]
    fn error_formats_text() {
        let prog = prog_name();
        let mut report = Report {
            message: "Unable to list".into(),
            cause: "Access Denied".into(),
            detail: Default::default(),
            kind: ErrorKind::Fatal,
        };
        assert_eq!(
            format_error(&report, false),
            format!("{prog}: <ERROR> Unable to list. Access Denied.")
        );
        report.kind = ErrorKind::Error;
        assert_eq!(
            format_error(&report, false),
            format!("{prog}: <ERROR> Unable to list Access Denied")
        );
    }
}
