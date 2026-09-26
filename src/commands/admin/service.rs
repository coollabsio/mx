//! `mx admin service` (mc `admin service`): `restart`, `unfreeze` and the hidden `stop` /
//! `freeze`.
//!
//! Owner: SERVER.

use crate::commands::runtime;
use crate::s3::admin::AdminClient;
use crate::s3::admin_server::{self as api, ServiceActionResult};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Debug, Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceCommand,
}

#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    #[command(name = "restart", about = "restart a MinIO cluster")]
    Restart(ServiceRestartArgs),
    #[command(name = "stop", hide = true, about = "stop a MinIO cluster")]
    Stop(ServiceStopArgs),
    #[command(name = "unfreeze", about = "unfreeze S3 API calls on MinIO cluster")]
    Unfreeze(ServiceUnfreezeArgs),
    #[command(
        name = "freeze",
        hide = true,
        about = "freeze S3 API calls on MinIO cluster"
    )]
    Freeze(ServiceFreezeArgs),
}

#[derive(Debug, Args)]
pub struct ServiceRestartArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(
        long = "dry-run",
        help = "do not attempt a restart, however verify the peer status"
    )]
    pub dry_run: bool,
    #[arg(
        long = "wait",
        short = 'w',
        help = "wait for background initializations to complete"
    )]
    pub wait: bool,
}

#[derive(Debug, Args)]
pub struct ServiceStopArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ServiceUnfreezeArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ServiceFreezeArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

pub fn run(args: ServiceArgs, json: bool) -> Result<()> {
    match args.command {
        ServiceCommand::Restart(args) => restart(args, json),
        ServiceCommand::Stop(args) => simple(&args.target, "stop", json),
        ServiceCommand::Unfreeze(args) => simple(&args.target, "unfreeze", json),
        ServiceCommand::Freeze(args) => simple(&args.target, "freeze", json),
    }
}

/// mc `serviceStopMessage` / `serviceFreezeCommand` / `serviceUnfreezeCommand`.
#[derive(Serialize)]
struct ServiceMessage<'a> {
    status: &'static str,
    #[serde(rename = "serverURL")]
    server_url: &'a str,
}

/// `stop`, `freeze`, `unfreeze`: one service action; `unfreeze` falls back to the legacy API.
fn simple(target: &str, action: &str, json: bool) -> Result<()> {
    let client = api::admin_client(target, "Unable to initialize admin connection.")?;
    runtime()?
        .block_on(async {
            let result = api::service_action(&client, action, false).await;
            match result {
                Err(_) if action == "unfreeze" => api::service_action_v1(&client, action).await,
                other => other.map(|_| ()),
            }
        })
        .with_context(|| format!("Unable to {action} the server."))?;
    if json {
        return crate::output::print_json(&ServiceMessage {
            status: "success",
            server_url: target,
        });
    }
    let text = match action {
        "stop" => format!("Stopped `{target}` successfully."),
        "freeze" => format!("Freeze command successfully sent to `{target}`."),
        _ => format!("Unfreeze command successfully sent to `{target}`."),
    };
    println!("{text}");
    Ok(())
}

const RESTARTING: i32 = 0;
const WAITING: i32 = 1;
const DONE: i32 = 2;

/// mc `serviceRestartMessage` (durations in nanoseconds, like Go `time.Duration`).
#[derive(Serialize)]
struct RestartMessage<'a> {
    status: &'static str,
    #[serde(rename = "serverURL")]
    server_url: &'a str,
    result: &'a ServiceActionResult,
    #[serde(rename = "restartDuration")]
    restart_duration: i64,
    #[serde(rename = "waitingDuration")]
    waiting_duration: i64,
    #[serde(rename = "timeTaken")]
    time_taken: i64,
    state: i32,
}

fn nanos(duration: Duration) -> i64 {
    duration.as_nanos().min(i64::MAX as u128) as i64
}

fn restart(args: ServiceRestartArgs, json: bool) -> Result<()> {
    let client = api::admin_client(&args.target, "Unable to initialize admin connection.")?;
    if !json {
        // mc renders a bubbletea UI, which needs a terminal.
        api::require_tty().context("Unable to initialize service restart UI")?;
    }
    let rt = runtime()?;
    let started = Instant::now();
    let result = rt
        .block_on(async {
            match api::service_action(&client, "restart", args.dry_run).await {
                Ok(result) => Ok(result),
                Err(_) => api::service_action_v1(&client, "restart")
                    .await
                    .map(|_| ServiceActionResult::default()),
            }
        })
        .context("Unable to restart the server.")?;
    let restart_duration = nanos(started.elapsed());
    let message = |state: i32, waiting: i64| RestartMessage {
        status: "success",
        server_url: &args.target,
        result: &result,
        restart_duration,
        waiting_duration: waiting,
        time_taken: restart_duration,
        state,
    };
    let mut waiting_duration = 0;
    if args.wait {
        if json {
            crate::output::print_json(&message(RESTARTING, 0))?;
        }
        let wait_start = Instant::now();
        rt.block_on(wait_healthy(&client, || {
            if json {
                let _ = crate::output::print_json(&message(WAITING, nanos(wait_start.elapsed())));
            }
        }));
        waiting_duration = nanos(wait_start.elapsed());
    }
    if json {
        return crate::output::print_json(&message(DONE, waiting_duration));
    }
    print!(
        "{}",
        restart_view(&result, restart_duration, waiting_duration)
    );
    Ok(())
}

/// Polls the anonymous cluster health endpoint (2 s timeout, every 500 ms) until healthy.
async fn wait_healthy(client: &AdminClient, mut on_waiting: impl FnMut()) {
    loop {
        let check = client.send_unsigned("GET", "/minio/health/cluster", &[]);
        if let Ok(Ok(response)) = tokio::time::timeout(Duration::from_secs(2), check).await
            && response.status == 200
        {
            return;
        }
        on_waiting();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The final frame of mc's restart UI.
fn restart_view(result: &ServiceActionResult, restart: i64, waiting: i64) -> String {
    let total = result.results.len();
    let offline = result.results.iter().filter(|r| !r.err.is_empty()).count();
    let hung = result
        .results
        .iter()
        .filter(|r| r.err.is_empty() && !r.waiting_drives.is_empty())
        .count();
    let mut rows = vec![
        vec![
            "Servers:".to_string(),
            format!(
                "{} online, {offline} offline, {hung} hung",
                total - offline - hung
            ),
        ],
        vec![
            "Restart Time:".to_string(),
            api::go_duration_string(restart),
        ],
    ];
    if waiting > 0 {
        rows.push(vec![
            "Background Init Time:".to_string(),
            api::go_duration_string(waiting),
        ]);
    }
    format!(
        "Service status: ▰▰▰ [DONE]\nSummary:\n{}",
        super::info::console_table(&rows, 4)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_message_marshals_like_mc() {
        let result: ServiceActionResult = serde_json::from_str(
            r#"{"action":"restart","dryRun":true,"results":[{"host":"127.0.0.1:9000"}]}"#,
        )
        .unwrap();
        let message = RestartMessage {
            status: "success",
            server_url: "e",
            result: &result,
            restart_duration: 913_913,
            waiting_duration: 0,
            time_taken: 913_913,
            state: DONE,
        };
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"status":"success","serverURL":"e","result":{"action":"restart","dryRun":true,"results":[{"host":"127.0.0.1:9000"}]},"restartDuration":913913,"waitingDuration":0,"timeTaken":913913,"state":2}"#
        );
    }

    #[test]
    fn restart_view_summarizes_peers() {
        let result: ServiceActionResult = serde_json::from_str(
            r#"{"action":"restart","results":[{"host":"a"},{"host":"b","err":"x"}]}"#,
        )
        .unwrap();
        assert_eq!(
            restart_view(&result, 967_988, 0),
            "Service status: ▰▰▰ [DONE]\nSummary:\n    ┌───────────────┬─────────────────────────────┐\n    │ Servers:      │ 1 online, 1 offline, 0 hung │\n    │ Restart Time: │ 967.988µs                   │\n    └───────────────┴─────────────────────────────┘\n"
        );
    }
}
