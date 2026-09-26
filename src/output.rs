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

/// True when stdout is a terminal. mc prints indented JSON on a terminal and one compact
/// JSON document per line otherwise (`globalJSONLine`).
pub fn stdout_is_terminal() -> bool {
    static TTY: OnceLock<bool> = OnceLock::new();
    *TTY.get_or_init(|| std::io::stdout().is_terminal())
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

/// Splits an anyhow error into mc's (message, cause): the outermost context is the message,
/// the rest of the chain (joined with `: `) is the cause. A single-level error has no cause.
pub fn split_error(err: &anyhow::Error) -> (String, String) {
    let mut chain = err.chain().map(|e| e.to_string());
    let message = chain.next().unwrap_or_default();
    let cause = chain.collect::<Vec<_>>().join(": ");
    (message, cause)
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
    format!("{msg} {errmsg}").trim().to_string()
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
    pub error: serde_json::Map<String, serde_json::Value>,
}

pub fn error_json<'a>(message: &'a str, cause: &'a str, kind: ErrorKind) -> ErrorDocument<'a> {
    ErrorDocument {
        status: "error",
        error: ErrorBody {
            message,
            cause: ErrorCause {
                message: cause,
                error: serde_json::Map::new(),
            },
            kind: kind.as_str(),
        },
    }
}

/// Full error output (text line for stderr, or JSON document for stdout) without printing.
/// mc prints error JSON indented (`MarshalIndent`) even on a non-terminal.
pub fn format_error(message: &str, cause: &str, kind: ErrorKind, json: bool) -> String {
    if json {
        return json_indent(&error_json(message, cause, kind)).unwrap_or_default();
    }
    let text = match kind {
        ErrorKind::Fatal => format_fatal_text(message, cause),
        // mc `errorIf` joins message and cause with a space, no punctuation rules.
        ErrorKind::Error => format!("{} {}", message.trim(), cause.trim())
            .trim()
            .to_string(),
    };
    format!("{}: <ERROR> {text}", prog_name())
}

/// Prints an error: JSON on stdout with `--json`, else `PROG: <ERROR> ...` on stderr.
pub fn print_error_parts(message: &str, cause: &str, kind: ErrorKind) {
    let json = crate::globals::json();
    let text = format_error(message, cause, kind, json);
    if json {
        println!("{text}");
    } else {
        eprintln!("{text}");
    }
}

/// mc `errorIf` for a non-fatal error: reports it and lets the command continue.
pub fn error_if(message: &str, cause: &str) {
    print_error_parts(message, cause, ErrorKind::Error);
}

/// Non-fatal report of an anyhow error (message = outermost context, cause = the rest).
pub fn print_error(err: &anyhow::Error) {
    let (message, cause) = split_error(err);
    print_error_parts(&message, &cause, ErrorKind::Error);
}

/// mc `fatalIf`: reports `err` as fatal and exits with status 1.
pub fn fatal(err: &anyhow::Error) -> ! {
    let (message, cause) = split_error(err);
    print_error_parts(&message, &cause, ErrorKind::Fatal);
    std::process::exit(1);
}

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
        assert_eq!(
            format_fatal_text("No such alias `x` found.", ""),
            "No such alias `x` found."
        );
        assert_eq!(format_fatal_text("", "bad value"), "bad value");
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
    fn error_formats_text_and_json() {
        let prog = prog_name();
        assert_eq!(
            format_error("Unable to list", "Access Denied", ErrorKind::Fatal, false),
            format!("{prog}: <ERROR> Unable to list. Access Denied.")
        );
        assert_eq!(
            format_error("Failed to copy", "boom", ErrorKind::Error, false),
            format!("{prog}: <ERROR> Failed to copy boom")
        );
        assert_eq!(
            format_error("Unable to list", "boom", ErrorKind::Fatal, true),
            "{\n \"status\": \"error\",\n \"error\": {\n  \"message\": \"Unable to list\",\n  \"cause\": {\n   \"message\": \"boom\",\n   \"error\": {}\n  },\n  \"type\": \"fatal\"\n }\n}"
        );
    }
}
