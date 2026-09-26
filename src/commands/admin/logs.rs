//! `mx admin logs` (mc `admin logs`): stream the server console log (madmin `GetLogs`).

use crate::commands::admin::trace::{admin_client, emit, help_exit, interrupted, quiet_pipe};
use crate::error::McError;
use crate::output::Exit;
use crate::s3::admin::{JsonStream, madmin_error};
use crate::s3::admin_stream::LogInfo;
use anyhow::Result;
use clap::Args;
use serde::Serialize;

#[derive(Debug, Args)]
pub struct LogsArgs {
    /// TARGET [NODENAME]
    #[arg(value_name = "TARGET")]
    pub args: Vec<String>,
    #[arg(
        long = "last",
        short = 'l',
        value_name = "VALUE",
        help = "show last n log entries (default: 10)"
    )]
    pub last: Option<i64>,
    #[arg(
        long = "type",
        short = 't',
        default_value = "all",
        value_name = "VALUE",
        help = "list error logs by type. Valid options are '[minio, application, all]'"
    )]
    pub type_: String,
}

/// mc `logMessage`: `status` plus the embedded `madmin.LogInfo`.
#[derive(Debug, Serialize)]
struct LogMessage<'a> {
    status: &'static str,
    #[serde(flatten)]
    info: &'a LogInfo,
}

/// Go `time.Parse(RFC3339Nano)` + `Format("15:04:05 MST 01/02/2006")`.
fn log_time(text: &str) -> String {
    let Some((secs, _)) = crate::s3::admin_stream::parse_time(text) else {
        return text.to_string();
    };
    let offset = offset_seconds(text);
    let local = aws_sdk_s3::primitives::DateTime::from_secs(secs + offset)
        .fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
        .unwrap_or_default();
    // local = YYYY-MM-DDTHH:MM:SSZ
    let (date, time) = local.split_once('T').unwrap_or_default();
    let time = time.trim_end_matches('Z');
    let mut parts = date.split('-');
    let (year, month, day) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    let zone = if offset == 0 && text.ends_with(['Z', 'z']) {
        "UTC".to_string()
    } else {
        let minutes = offset / 60;
        let sign = if minutes < 0 { '-' } else { '+' };
        format!("{sign}{:02}{:02}", minutes.abs() / 60, minutes.abs() % 60)
    };
    format!("{time} {zone} {month}/{day}/{year}")
}

/// Offset of an RFC3339 time in seconds (`Z` = 0).
fn offset_seconds(text: &str) -> i64 {
    if text.ends_with(['Z', 'z']) {
        return 0;
    }
    let Some(at) = text.rfind(['+', '-']).filter(|at| *at > 10) else {
        return 0;
    };
    let (sign, rest) = (&text[at..at + 1], &text[at + 1..]);
    let mut nums = rest.split(':');
    let hours: i64 = nums.next().and_then(|h| h.parse().ok()).unwrap_or(0);
    let minutes: i64 = nums.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    let value = hours * 3600 + minutes * 60;
    if sign == "-" { -value } else { value }
}

/// Go `%v` of a JSON value decoded into `interface{}` (`%s` of a non-string prints Go's
/// `%!s(...)` form, which mc shows as is).
fn go_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => format!("%!s(float64={n})"),
        serde_json::Value::Bool(b) => format!("%!s(bool={b})"),
        serde_json::Value::Null => "%!s(<nil>)".to_string(),
        other => other.to_string(),
    }
}

/// mc `logMessage.String()` (without colors, variables sorted).
fn log_text(info: &LogInfo) -> String {
    let host = if info.node_name.is_empty() {
        String::new()
    } else {
        format!("{} ", info.node_name)
    };
    let mut b = String::new();
    if let Some(api) = &info.api {
        let mut api_string = format!("API: {}(", api.name);
        if let Some(args) = &api.args {
            if !args.bucket.is_empty() {
                api_string.push_str(&format!("bucket={}", args.bucket));
            }
            if !args.object.is_empty() {
                api_string.push_str(&format!(", object={}", args.object));
            }
        }
        api_string.push(')');
        b.push_str(&format!("\n{host} {api_string}"));
    }
    let fields = [
        ("Time", log_time(&info.time), !info.time.is_empty()),
        ("DeploymentID", info.deployment_id.clone(), true),
        ("RequestID", info.request_id.clone(), true),
        ("RemoteHost", info.remote_host.clone(), true),
        ("UserAgent", info.user_agent.clone(), true),
        ("Message", info.message.clone(), true),
    ];
    for (name, value, show) in fields {
        if show && !value.is_empty() {
            b.push_str(&format!("\n{host} {name}: {value}"));
        }
    }
    if let Some(trace) = &info.trace {
        if !trace.message.is_empty() {
            b.push_str(&format!("\n{host} Error: {}", trace.message));
        }
        for (key, value) in trace.variables.iter().flatten() {
            if value.as_str() != Some("") {
                b.push_str(&format!("\n{host} {key}={}", go_value(value)));
            }
        }
        if let Some(source) = &trace.source {
            for (i, element) in source.iter().enumerate() {
                b.push_str(&format!("\n{host} {:>8}: {element}", source.len() - i));
            }
        }
    }
    b.strip_prefix('\n').unwrap_or(&b).to_string()
}

pub fn run(args: LogsArgs, json: bool) -> Result<()> {
    if args.args.is_empty() || args.args.len() > 3 {
        return help_exit(&["admin", "logs"]);
    }
    let last = match args.last {
        Some(last) if last <= 0 => {
            return Err(anyhow::Error::new(McError::invalid_argument()).context(
                "please set a proper limit, for example: '--last 5' to display last 5 logs, omit this flag to display all available logs",
            ));
        }
        Some(last) => last,
        None => 0,
    };
    let log_type = args.type_.to_lowercase();
    if !matches!(log_type.as_str(), "minio" | "application" | "all") {
        return Err(anyhow::Error::new(McError::invalid_argument()).context(
            "Invalid value for --type flag. Valid options are [minio, application, all]",
        ));
    }
    let target = &args.args[0];
    let node = args.args.get(1).cloned().unwrap_or_default();
    let client = admin_client(target)?;
    crate::commands::runtime()?.block_on(async {
        let signal = interrupted();
        tokio::pin!(signal);
        // madmin `GetLogs`: reconnect whenever the server ends the stream; a transport error
        // ends the command quietly, an error status is fatal.
        let limit = last.to_string();
        loop {
            let query = [
                ("node", node.as_str()),
                ("limit", limit.as_str()),
                ("logType", log_type.as_str()),
            ];
            let path = format!("{}/log", crate::s3::admin::ADMIN_PREFIX);
            let opened = tokio::select! {
                code = &mut signal => return Err(Exit(code).into()),
                opened = client.send_streaming("GET", &path, &query, &[], Vec::new()) => opened,
            };
            let Ok((mut response, body)) = opened else {
                return Ok(());
            };
            if response.status != 200 {
                response.body = body.collect().await?.into_bytes().to_vec();
                return Err(anyhow::Error::new(madmin_error(&response))
                    .context("Unable to listen to console logs"));
            }
            let mut logs = JsonStream::new(body);
            loop {
                let next = tokio::select! {
                    code = &mut signal => return Err(Exit(code).into()),
                    next = logs.next::<LogInfo>() => next,
                };
                let Ok(Some(mut info)) = next else {
                    break;
                };
                if !node.is_empty() {
                    info.node_name.clear();
                }
                if info.deployment_id.is_empty() {
                    continue;
                }
                let text = if json {
                    crate::output::json_string(&LogMessage {
                        status: "success",
                        info: &info,
                    })?
                } else {
                    log_text(&info)
                };
                quiet_pipe(emit(&text))?;
            }
            // Don't spin (and replay `--last N`) when the server keeps ending the stream.
            tokio::select! {
                code = &mut signal => return Err(Exit(code).into()),
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::admin_stream::{LogApi, LogArgs, LogTrace};
    use std::collections::BTreeMap;

    fn entry() -> LogInfo {
        let mut variables = BTreeMap::new();
        variables.insert("targetID".to_string(), serde_json::json!("MXTEST:webhook"));
        variables.insert("empty".to_string(), serde_json::json!(""));
        LogInfo {
            deployment_id: "d1".into(),
            level: "ERROR".into(),
            time: "2026-09-26T19:30:39.91059134Z".into(),
            api: Some(LogApi {
                name: "SYSTEM.notify".into(),
                args: Some(LogArgs::default()),
            }),
            trace: Some(LogTrace {
                message: "dial tcp: refused".into(),
                source: Some(vec!["a.go:1".into(), "b.go:2".into()]),
                variables: Some(variables),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn text_matches_mc_layout() {
        assert_eq!(
            log_text(&entry()),
            " API: SYSTEM.notify()\n Time: 19:30:39 UTC 09/26/2026\n DeploymentID: d1\n Error: dial tcp: refused\n targetID=MXTEST:webhook\n        2: a.go:1\n        1: b.go:2"
        );
        let mut with_node = entry();
        with_node.node_name = "n1".into();
        with_node.api.as_mut().unwrap().args = Some(LogArgs {
            bucket: "b".into(),
            object: "o".into(),
            ..Default::default()
        });
        assert!(log_text(&with_node).starts_with("n1  API: SYSTEM.notify(bucket=b, object=o)\n"));
    }

    #[test]
    fn json_embeds_log_info() {
        let text = serde_json::to_string(&LogMessage {
            status: "success",
            info: &entry(),
        })
        .unwrap();
        assert!(
            text.starts_with(
                r#"{"status":"success","deploymentid":"d1","level":"ERROR","errKind":"","time":"2026-09-26T19:30:39.91059134Z","api":{"name":"SYSTEM.notify","args":{}},"error":{"message":"dial tcp: refused""#
            ),
            "{text}"
        );
        assert!(text.ends_with(r#""ConsoleMsg":"","node":""}"#), "{text}");
    }

    #[test]
    fn log_times_follow_go_layout() {
        assert_eq!(log_time("2026-01-02T03:04:05Z"), "03:04:05 UTC 01/02/2026");
        assert_eq!(
            log_time("2026-01-02T03:04:05+02:00"),
            "03:04:05 +0200 01/02/2026"
        );
        assert_eq!(log_time("garbage"), "garbage");
    }
}
