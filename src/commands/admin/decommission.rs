//! `mx admin decommission` (mc `admin decommission`, alias `decom`): start, cancel and show
//! the status of server pool decommissioning (`pools/*` admin API).
//!
//! Also holds small helpers shared by the TOPO admin commands (`rebalance`, `replicate`):
//! mc's box table, command help on bad argument counts, admin client errors.

use crate::commands::runtime;
use crate::config::ConfigStore;
use crate::error::McError;
use crate::output;
use crate::s3::admin::{self, AdminClient};
use crate::s3::admin_topo::{self, PoolDecommissionInfo, PoolStatus};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct DecommissionArgs {
    #[command(subcommand)]
    pub command: DecommissionCommand,
}

#[derive(Debug, Subcommand)]
pub enum DecommissionCommand {
    #[command(name = "start", about = "start decommissioning a pool")]
    Start(DecommissionTargetArgs),
    #[command(name = "status", about = "show current decommissioning status")]
    Status(DecommissionTargetArgs),
    #[command(name = "cancel", about = "cancel an ongoing decommissioning of a pool")]
    Cancel(DecommissionTargetArgs),
}

#[derive(Debug, Args)]
pub struct DecommissionTargetArgs {
    /// TARGET and POOL, e.g. `myminio/ http://server{5...8}/disk{1...4}`
    #[arg(value_name = "TARGET [POOL]")]
    pub args: Vec<String>,
}

pub fn run(args: DecommissionArgs, json: bool) -> Result<()> {
    match args.command {
        DecommissionCommand::Start(args) => start(args, json),
        DecommissionCommand::Status(args) => status(args, json),
        DecommissionCommand::Cancel(args) => cancel(args),
    }
}

#[derive(Debug, Serialize)]
struct StartMessage<'a> {
    status: &'static str,
    pool: &'a str,
}

fn start(args: DecommissionTargetArgs, json: bool) -> Result<()> {
    if args.args.len() != 2 {
        show_help(&["admin", "decommission", "start"]);
    }
    let (target, pool) = (&args.args[0], &args.args[1]);
    let client = connect(target)?;
    runtime()?
        .block_on(admin_topo::decommission_pool(&client, pool))
        .context("Unable to start decommission on the specified pool")?;
    if json {
        output::print_json(&StartMessage {
            status: "success",
            pool,
        })?;
    } else {
        println!("Decommission started successfully for `{pool}`.");
    }
    Ok(())
}

fn status(args: DecommissionTargetArgs, json: bool) -> Result<()> {
    if args.args.is_empty() || args.args.len() > 2 {
        show_help(&["admin", "decommission", "status"]);
    }
    let client = connect(&args.args[0])?;
    let rt = runtime()?;
    if let Some(pool) = args.args.get(1).filter(|p| !p.is_empty()) {
        let status = rt
            .block_on(admin_topo::status_pool(&client, pool))
            .context("Unable to get status per pool")?;
        if json {
            println!("{}", indent4_json(&status)?);
            return Ok(());
        }
        match pool_status_text(&status) {
            Some(text) => println!("{text}"),
            None => output::error_if(
                "This pool is currently not scheduled for decomissioning",
                "",
            ),
        }
        return Ok(());
    }
    let pools = rt
        .block_on(admin_topo::list_pools_status(&client))
        .context("Unable to get status for all pools")?;
    if json {
        println!("{}", indent4_json(&pools)?);
        return Ok(());
    }
    print!("{}", pools_table(&pools));
    Ok(())
}

fn cancel(args: DecommissionTargetArgs) -> Result<()> {
    if args.args.is_empty() || args.args.len() > 2 {
        show_help(&["admin", "decommission", "cancel"]);
    }
    let client = connect(&args.args[0])?;
    let rt = runtime()?;
    if let Some(pool) = args.args.get(1).filter(|p| !p.is_empty()) {
        // mc prints nothing when the cancel request succeeds.
        rt.block_on(admin_topo::cancel_decommission_pool(&client, pool))
            .context("Unable to cancel decommissioning, please try again")?;
        return Ok(());
    }
    // Without a pool mc only lists the pools being drained (it does not cancel anything).
    let pools = rt
        .block_on(admin_topo::list_pools_status(&client))
        .context("Unable to get status for all pools")?;
    print!("{}", draining_table(&pools));
    Ok(())
}

/// mc `newAdminClient` for an aliased URL.
fn connect(target: &str) -> Result<AdminClient> {
    admin::admin_client_for(&ConfigStore::load_or_create()?, target)
}

/// Text for `decommission status TARGET POOL`; `None` when the pool is not being decommissioned
/// (mc reports that as a non-fatal error with exit status 0).
fn pool_status_text(status: &PoolStatus) -> Option<String> {
    let info = status.decommission.clone().unwrap_or_default();
    let pool = &status.cmdline;
    if info.complete {
        return Some(format!(
            "Decommission of pool {pool} is complete, you may now remove it from server command line"
        ));
    }
    if info.failed {
        return Some(format!(
            "Decommission of pool {pool} failed, please retry again"
        ));
    }
    if info.canceled {
        return Some(format!(
            "Decommission of pool {pool} was canceled, you may start again"
        ));
    }
    if info.start_time.is_zero() {
        return None;
    }
    Some(progress_text(&info, info.start_time.elapsed_nanos()))
}

/// mc's decommission rate message (`elapsed` in nanoseconds since the start).
fn progress_text(info: &PoolDecommissionInfo, elapsed: i64) -> String {
    let used_start = info.total_size - info.start_size;
    let used_current = info.total_size - info.current_size;
    let duration = elapsed as f64 / 1e9;
    if used_start > used_current && duration > 10.0 {
        let copied = (used_start - used_current) as u64;
        let speed = (copied as f64 / duration) as u64;
        // go-humanize `RelTime(now, start, "", "ago")`.
        let started = crate::commands::replicate::rel_time(elapsed / 1_000_000_000);
        let started = if started == "now" {
            started
        } else {
            format!("{started}ago")
        };
        format!(
            "Decommissioning rate at {}/sec [{}/{}]\nStarted: {started}",
            admin::ibytes(speed),
            admin::ibytes(used_current as u64),
            admin::ibytes(info.total_size as u64)
        )
    } else {
        "Decommissioning is starting...".to_string()
    }
}

/// `decommission status TARGET` table.
fn pools_table(pools: &[PoolStatus]) -> String {
    let mut rows = vec![vec![
        "ID".to_string(),
        "Pools".to_string(),
        "Drives Usage".to_string(),
        "Status".to_string(),
    ]];
    for pool in pools {
        let info = pool.decommission.clone().unwrap_or_default();
        let total = info.total_size as u64;
        let used = (info.total_size - info.current_size) as u64;
        let capacity = if total == 0 {
            "0% (total: 0B)".to_string()
        } else {
            format!(
                "{:.1}% (total: {})",
                100.0 * used as f64 / total as f64,
                admin::ibytes(total)
            )
        };
        let status = match &pool.decommission {
            Some(info) if info.complete => "Complete",
            Some(info) if info.failed => "Draining(Failed)",
            Some(info) if info.canceled => "Draining(Canceled)",
            Some(info) if !info.start_time.is_zero() => "Draining",
            _ => "Active",
        };
        rows.push(vec![
            ordinal(pool.id + 1),
            pool.cmdline.clone(),
            capacity,
            status.to_string(),
        ]);
    }
    console_table(&rows, &[false; 4])
}

/// `decommission cancel TARGET` table: pools currently being drained.
///
/// mc indexes its rows by pool position and panics (index out of range) when a later pool is
/// draining while an earlier one is not; mx prints the intended table instead.
fn draining_table(pools: &[PoolStatus]) -> String {
    let mut rows = vec![vec![
        "ID".to_string(),
        "Pools".to_string(),
        "Capacity".to_string(),
        "Status".to_string(),
    ]];
    for pool in pools {
        let Some(info) = &pool.decommission else {
            continue;
        };
        if info.start_time.is_zero() || info.complete {
            continue;
        }
        let total = info.total_size as u64;
        let used = total.saturating_sub(info.current_size as u64);
        rows.push(vec![
            ordinal(pool.id + 1),
            pool.cmdline.clone(),
            format!(
                "{} (used) / {} (total)",
                admin::ibytes(used),
                admin::ibytes(total)
            ),
            "Draining".to_string(),
        ]);
    }
    console_table(&rows, &[false; 4])
}

/// go-humanize `Ordinal`: `1st`, `2nd`, `3rd`, `4th`, `11th`, `21st`.
pub(crate) fn ordinal(n: i64) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// Go `json.MarshalIndent(v, "", "    ")` printed with colorjson: compact when stdout is not
/// a terminal, four-space indent on a terminal.
fn indent4_json<T: Serialize>(value: &T) -> Result<String> {
    if !output::stdout_is_terminal() {
        return output::format_json(value, true);
    }
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut serializer)?;
    let text = String::from_utf8(buf)?;
    Ok(text
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026"))
}

/// minio `console.Table.DisplayTable` without colors: box table, first row is the header
/// (no separator line), columns padded to the widest cell (by characters).
pub(crate) fn console_table(rows: &[Vec<String>], align_right: &[bool]) -> String {
    let columns = rows.first().map(Vec::len).unwrap_or_default();
    let mut widths = vec![0; columns];
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(columns) {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let border = |left: &str, mid: &str, right: &str| {
        let segments: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        format!("{left}{}{right}\n", segments.join(mid))
    };
    let mut out = border("┌", "┬", "┐");
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .take(columns)
            .map(|(i, cell)| {
                let pad = " ".repeat(widths[i] - cell.chars().count());
                if align_right.get(i).copied().unwrap_or(false) {
                    format!("{pad}{cell}")
                } else {
                    format!("{cell}{pad}")
                }
            })
            .collect();
        out.push_str(&format!("│ {} │\n", cells.join(" │ ")));
    }
    out.push_str(&border("└", "┴", "┘"));
    out
}

/// mc `showCommandHelpAndExit(ctx, 1)`: prints the help of the command at `path` and exits
/// with status 1.
pub(crate) fn show_help(path: &[&str]) -> ! {
    crate::help::show_help_and_exit(path, 1)
}

/// mc `newAdminClient` failure reported with a command-specific message, e.g.
/// `Unable to initialize admin client` (rebalance).
pub(crate) fn admin_client_with(target: &str, message: &'static str) -> Result<AdminClient> {
    let store = ConfigStore::load_or_create()?;
    admin::admin_client_for(&store, target).map_err(|err| match err.downcast_ref::<McError>() {
        Some(cause) => anyhow::Error::new(cause.clone()).context(message),
        None => err,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::admin_topo::GoTime;

    fn pool(id: i64, info: Option<PoolDecommissionInfo>) -> PoolStatus {
        PoolStatus {
            id,
            cmdline: format!("/data{{{}...{}}}", id * 4 + 1, id * 4 + 4),
            decommission: info,
            ..Default::default()
        }
    }

    fn info(total: i64, current: i64) -> PoolDecommissionInfo {
        PoolDecommissionInfo {
            total_size: total,
            current_size: current,
            ..Default::default()
        }
    }

    #[test]
    fn ordinals_match_go_humanize() {
        let got: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111]
            .into_iter()
            .map(ordinal)
            .collect();
        assert_eq!(
            got,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st",
                "111th"
            ]
        );
    }

    #[test]
    fn status_table_matches_mc() {
        let gib = 1i64 << 30;
        let mut draining = info(822 * gib, 400 * gib);
        draining.start_time = GoTime("2026-09-26T19:00:00Z".into());
        let mut canceled = draining.clone();
        canceled.canceled = true;
        let table = pools_table(&[
            pool(0, Some(info(822 * gib, 404 * gib))),
            pool(1, Some(draining)),
            pool(2, Some(canceled)),
            pool(3, Some(info(0, 0))),
        ]);
        assert_eq!(
            table,
            "┌─────┬────────────────┬────────────────────────┬────────────────────┐\n\
             │ ID  │ Pools          │ Drives Usage           │ Status             │\n\
             │ 1st │ /data{1...4}   │ 50.9% (total: 822 GiB) │ Active             │\n\
             │ 2nd │ /data{5...8}   │ 51.3% (total: 822 GiB) │ Draining           │\n\
             │ 3rd │ /data{9...12}  │ 51.3% (total: 822 GiB) │ Draining(Canceled) │\n\
             │ 4th │ /data{13...16} │ 0% (total: 0B)         │ Active             │\n\
             └─────┴────────────────┴────────────────────────┴────────────────────┘\n"
        );
    }

    #[test]
    fn cancel_table_lists_draining_pools() {
        let mut draining = info(1 << 30, 1 << 29);
        draining.start_time = GoTime("2026-09-26T19:00:00Z".into());
        let table = draining_table(&[pool(0, Some(info(10, 5))), pool(1, Some(draining))]);
        let capacity = "512 MiB (used) / 1.0 GiB (total)";
        let dashes = "─".repeat(capacity.len() + 2);
        assert_eq!(
            table,
            format!(
                "┌─────┬──────────────┬{dashes}┬──────────┐\n\
                 │ ID  │ Pools        │ {:<w$} │ Status   │\n\
                 │ 2nd │ /data{{5...8}} │ {capacity} │ Draining │\n\
                 └─────┴──────────────┴{dashes}┴──────────┘\n",
                "Capacity",
                w = capacity.len()
            )
        );
        assert_eq!(
            draining_table(&[]),
            "┌────┬───────┬──────────┬────────┐\n│ ID │ Pools │ Capacity │ Status │\n└────┴───────┴──────────┴────────┘\n"
        );
    }

    #[test]
    fn pool_status_messages() {
        let mut status = pool(1, Some(info(10, 5)));
        assert_eq!(pool_status_text(&status), None);
        status.decommission.as_mut().unwrap().complete = true;
        assert_eq!(
            pool_status_text(&status).unwrap(),
            "Decommission of pool /data{5...8} is complete, you may now remove it from server command line"
        );
        let mut status = pool(1, Some(info(10, 5)));
        status.decommission.as_mut().unwrap().failed = true;
        assert_eq!(
            pool_status_text(&status).unwrap(),
            "Decommission of pool /data{5...8} failed, please retry again"
        );
        let mut status = pool(1, Some(info(10, 5)));
        status.decommission.as_mut().unwrap().canceled = true;
        assert_eq!(
            pool_status_text(&status).unwrap(),
            "Decommission of pool /data{5...8} was canceled, you may start again"
        );
    }

    #[test]
    fn progress_rate_needs_ten_seconds_and_progress() {
        let gib = 1i64 << 30;
        let mut info = info(100 * gib, 60 * gib);
        info.start_size = 50 * gib;
        assert_eq!(
            progress_text(&info, 5_000_000_000),
            "Decommissioning is starting..."
        );
        // 10 GiB drained in 100s.
        assert_eq!(
            progress_text(&info, 100_000_000_000),
            "Decommissioning rate at 102 MiB/sec [40 GiB/100 GiB]\nStarted: 1 minute ago"
        );
    }

    #[test]
    fn console_table_right_alignment() {
        let rows = vec![
            vec!["a".to_string(), "bb".to_string()],
            vec!["ccc".to_string(), "d".to_string()],
        ];
        assert_eq!(
            console_table(&rows, &[false, true]),
            "┌─────┬────┐\n│ a   │ bb │\n│ ccc │  d │\n└─────┴────┘\n"
        );
        // mc renders a table without columns (no pools) as empty borders.
        assert_eq!(
            console_table(&[vec![], vec![]], &[]),
            "┌┐\n│  │\n│  │\n└┘\n"
        );
    }
}
