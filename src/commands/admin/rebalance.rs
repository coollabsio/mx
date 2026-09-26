//! `mx admin rebalance` (mc `admin rebalance`): start, stop and summarize a rebalance of
//! data across server pools (`rebalance/*` admin API).

use super::decommission::{admin_client_with, console_table, show_help};
use crate::commands::runtime;
use crate::output;
use crate::s3::admin::{self, AdminClient};
use crate::s3::admin_topo::{self, RebalanceStatus};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct RebalanceArgs {
    #[command(subcommand)]
    pub command: RebalanceCommand,
}

#[derive(Debug, Subcommand)]
pub enum RebalanceCommand {
    #[command(name = "start", about = "start rebalance operation")]
    Start(RebalanceAliasArgs),
    #[command(name = "status", about = "summarize an ongoing rebalance operation")]
    Status(RebalanceAliasArgs),
    #[command(name = "stop", about = "stop an ongoing rebalance operation")]
    Stop(RebalanceAliasArgs),
}

#[derive(Debug, Args)]
pub struct RebalanceAliasArgs {
    #[arg(value_name = "ALIAS")]
    pub args: Vec<String>,
}

pub fn run(args: RebalanceArgs, json: bool) -> Result<()> {
    match args.command {
        RebalanceCommand::Start(args) => start(args, json),
        RebalanceCommand::Status(args) => status(args, json),
        RebalanceCommand::Stop(args) => stop(args, json),
    }
}

/// The single ALIAS argument; any other count shows the command help (exit 1) like mc.
fn alias_arg(args: RebalanceAliasArgs, command: &str) -> Result<(String, AdminClient)> {
    if args.args.len() != 1 {
        show_help(&["admin", "rebalance", command]);
    }
    let alias = args.args[0].clone();
    let client = admin_client_with(&alias, "Unable to initialize admin client")?;
    Ok((alias, client))
}

#[derive(Debug, Serialize)]
struct StartMessage<'a> {
    status: &'static str,
    url: &'a str,
    id: &'a str,
}

#[derive(Debug, Serialize)]
struct StopMessage<'a> {
    status: &'static str,
    url: &'a str,
}

fn start(args: RebalanceAliasArgs, json: bool) -> Result<()> {
    let (alias, client) = alias_arg(args, "start")?;
    let id = runtime()?
        .block_on(admin_topo::rebalance_start(&client))
        .context("Unable to start rebalance")?;
    if json {
        output::print_json(&StartMessage {
            status: "success",
            url: &alias,
            id: &id,
        })?;
    } else {
        println!("Rebalance started for {alias}");
    }
    Ok(())
}

fn status(args: RebalanceAliasArgs, json: bool) -> Result<()> {
    let (_, client) = alias_arg(args, "status")?;
    let status = runtime()?
        .block_on(admin_topo::rebalance_status(&client))
        .context("Unable to get rebalance status")?;
    if json {
        // mc prints `json.Marshal` output (compact, also on a terminal).
        println!("{}", output::format_json(&status, true)?);
    } else {
        print!("{}", status_text(&status));
    }
    Ok(())
}

fn stop(args: RebalanceAliasArgs, json: bool) -> Result<()> {
    let (alias, client) = alias_arg(args, "stop")?;
    runtime()?
        .block_on(admin_topo::rebalance_stop(&client))
        .context("Unable to stop rebalance operation")?;
    if json {
        output::print_json(&StopMessage {
            status: "success",
            url: &alias,
        })?;
    } else {
        println!("Rebalance stopped for {alias}");
    }
    Ok(())
}

/// mc `admin rebalance status` text: per-pool usage table and a summary.
fn status_text(status: &RebalanceStatus) -> String {
    let pools = status.pools.as_deref().unwrap_or_default();
    let headers: Vec<String> = (0..pools.len()).map(|i| format!("Pool-{i}")).collect();
    let (mut bytes, mut objects, mut versions) = (0u64, 0u64, 0u64);
    let (mut elapsed, mut eta) = (0i64, 0i64);
    let usage: Vec<String> = pools
        .iter()
        .map(|pool| {
            bytes += pool.progress.bytes;
            objects += pool.progress.num_objects;
            versions += pool.progress.num_versions;
            elapsed = elapsed.max(pool.progress.elapsed);
            eta = eta.max(pool.progress.eta);
            let mut cell = format!("{:.2}%", pool.used * 100.0);
            if pool.status == "Started" {
                // Rebalance in progress on this pool.
                cell.push_str(" *");
            }
            cell
        })
        .collect();
    let table = console_table(&[headers, usage], &vec![false; pools.len()]);
    format!(
        "Per-pool usage:\n{table}Summary: \nData: {} ({objects} objects, {versions} versions) \nTime: {} ({} to completion)\n",
        admin::ibytes(bytes),
        crate::commands::replicate::go_duration_string(elapsed),
        crate::commands::replicate::go_duration_string(eta)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::admin_topo::{RebalPoolProgress, RebalancePoolStatus};

    #[test]
    fn status_text_matches_mc() {
        let status = RebalanceStatus {
            pools: Some(vec![
                RebalancePoolStatus {
                    id: 0,
                    status: "Started".into(),
                    used: 0.5086507767922406,
                    progress: RebalPoolProgress {
                        num_objects: 10,
                        num_versions: 12,
                        bytes: 3 << 20,
                        elapsed: 90_000_000_000,
                        eta: 1_500_000_000,
                        ..Default::default()
                    },
                },
                RebalancePoolStatus {
                    id: 1,
                    status: "None".into(),
                    used: 0.1,
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        assert_eq!(
            status_text(&status),
            "Per-pool usage:\n\
             ┌──────────┬────────┐\n\
             │ Pool-0   │ Pool-1 │\n\
             │ 50.87% * │ 10.00% │\n\
             └──────────┴────────┘\n\
             Summary: \n\
             Data: 3.0 MiB (10 objects, 12 versions) \n\
             Time: 1m30s (1.5s to completion)\n"
        );
    }

    #[test]
    fn status_text_without_pools() {
        assert_eq!(
            status_text(&RebalanceStatus::default()),
            "Per-pool usage:\n┌┐\n│  │\n│  │\n└┘\nSummary: \nData: 0 B (0 objects, 0 versions) \nTime: 0s (0s to completion)\n"
        );
    }
}
