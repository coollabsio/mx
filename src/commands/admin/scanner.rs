//! `mx admin scanner` (mc `admin scanner`).
//!
//! Owner: SERVER (`status`). `trace` runs `super::trace::scanner_trace` (owner: STREAM); its
//! flags live here.
//!
//! `status` streams `madmin.RealtimeMetrics` (scanner type): JSON documents with `--json`,
//! otherwise mc's live view (a terminal UI in mc, so it needs a terminal like mc).
//! `--bucket` prints per-erasure-set bucket scan stats instead.

use crate::commands::runtime;
use crate::s3::admin_server::{self as api, BucketScanInfo, RealtimeMetrics, ScannerMetrics};
use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::io::Write;

#[derive(Debug, Args)]
pub struct ScannerArgs {
    #[command(subcommand)]
    pub command: ScannerCommand,
}

#[derive(Debug, Subcommand)]
pub enum ScannerCommand {
    #[command(
        name = "status",
        alias = "info",
        about = "summarize scanner events on MinIO server in real-time"
    )]
    Status(ScannerStatusArgs),
    #[command(name = "trace", about = "show trace for MinIO scanner operations")]
    Trace(ScannerTraceArgs),
}

#[derive(Debug, Args)]
pub struct ScannerStatusArgs {
    #[arg(value_name = "TARGET", required_unless_present = "in_")]
    pub target: Option<String>,
    #[arg(
        long = "nodes",
        value_name = "VALUE",
        help = "show only on matching servers, comma separate multiple"
    )]
    pub nodes: Option<String>,
    #[arg(
        short = 'n',
        default_value_t = 0,
        value_name = "VALUE",
        help = "number of requests to run before exiting. 0 for endless"
    )]
    pub count_n: i64,
    #[arg(
        long = "interval",
        default_value_t = 3,
        value_name = "VALUE",
        help = "interval between requests in seconds"
    )]
    pub interval: i64,
    #[arg(long = "max-paths", default_value_t = -1, value_name = "VALUE", help = "maximum number of active paths to show. -1 for unlimited")]
    pub max_paths: i64,
    #[arg(
        long = "in",
        value_name = "VALUE",
        help = "read previously saved json from file and replay"
    )]
    pub in_: Option<String>,
    #[arg(
        long = "bucket",
        value_name = "VALUE",
        help = "show scan stats about a given bucket"
    )]
    pub bucket: Option<String>,
}

#[derive(Debug, Args)]
pub struct ScannerTraceArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(long = "verbose", short = 'v', help = "print verbose trace")]
    pub verbose: bool,
    #[arg(
        long = "funcname",
        value_name = "VALUE",
        help = "trace only matching func name (eg 'scanner.ScanObject')"
    )]
    pub funcname: Vec<String>,
    #[arg(
        long = "node",
        value_name = "VALUE",
        help = "trace only matching servers"
    )]
    pub node: Vec<String>,
    #[arg(long = "path", value_name = "VALUE", help = "trace only matching path")]
    pub path: Vec<String>,
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
        value_name = "VALUE",
        help = "trace calls only with response duration greater than this threshold (e.g. 5ms)"
    )]
    pub response_duration: Option<String>,
    #[arg(
        long = "filter-size",
        value_name = "VALUE",
        help = "filter size, use with filter (see UNITS)"
    )]
    pub filter_size: Option<String>,
}

pub fn run(args: ScannerArgs, json: bool) -> Result<()> {
    match args.command {
        ScannerCommand::Status(args) => status(args, json),
        ScannerCommand::Trace(args) => trace(args, json),
    }
}

fn status(args: ScannerStatusArgs, json: bool) -> Result<()> {
    if let Some(input) = args.in_.as_deref().filter(|i| !i.is_empty()) {
        return replay(input, args.max_paths);
    }
    let target = args.target.clone().unwrap_or_default();
    let client = api::admin_client(&target, "Unable to initialize admin client.")?;
    let rt = runtime()?;
    if let Some(bucket) = args.bucket.as_deref().filter(|b| !b.is_empty()) {
        let stats = rt
            .block_on(api::bucket_scan_info(&client, bucket))
            .context("Unable to get bucket stats.")?;
        if json {
            return crate::output::print_json(&BucketScanMessage {
                status: "success",
                stats: Some(&stats),
            });
        }
        let text = render_bucket_stats(stats, api::now_unix());
        println!("{}", text.strip_suffix('\n').unwrap_or(&text));
        return Ok(());
    }
    let nodes = args.nodes.clone().unwrap_or_default();
    let opts = api::MetricsOptions {
        types: api::METRICS_SCANNER,
        n: args.count_n,
        interval_secs: args.interval,
        hosts: &nodes,
    };
    if json {
        let mut stdout = std::io::stdout();
        return rt
            .block_on(api::metrics(&client, &opts, |metrics| {
                writeln!(stdout, "{}", metrics_json(&metrics)?)?;
                Ok(())
            }))
            .or_else(ignore_broken_pipe)
            .context("Unable to fetch scanner metrics");
    }
    api::require_tty().context("Unable to fetch scanner metrics")?;
    let mut screen = Screen::default();
    rt.block_on(api::metrics(&client, &opts, |metrics| {
        screen.draw(&render_view(&metrics, args.max_paths, metrics.is_final))
    }))
    .or_else(ignore_broken_pipe)
    .context("Unable to fetch scanner metrics")
}

fn ignore_broken_pipe(err: anyhow::Error) -> Result<()> {
    if crate::s3::admin::is_broken_pipe(&err) {
        Ok(())
    } else {
        Err(err)
    }
}

/// mc `metricsMessage.JSON()` (a JSON encoder, see [`api::encoder_json`]).
fn metrics_json(metrics: &RealtimeMetrics) -> Result<String> {
    api::encoder_json(metrics, &["status", "info"])
}

/// `--in FILE`: replays saved `RealtimeMetrics` JSON lines through the live view.
fn replay(input: &str, max_paths: i64) -> Result<()> {
    api::require_tty().context("Unable to fetch scanner metrics")?;
    if input.ends_with(".zst") {
        return Err(
            anyhow!("zstd compressed input is not supported").context("Unable to open input")
        );
    }
    let text = std::fs::read_to_string(input)
        .map_err(|err| anyhow!("open {input}: {}", api::go_errno_text(&err)))
        .context("Unable to open input")?;
    let mut screen = Screen::default();
    let mut last: Option<i64> = None;
    for line in text.lines() {
        let Ok(metrics) = serde_json::from_str::<RealtimeMetrics>(line) else {
            continue;
        };
        let Some(scanner) = &metrics.aggregated.scanner else {
            continue;
        };
        let collected = api::unix_seconds(&scanner.collected_at);
        if let (Some(last), Some(now)) = (last, collected)
            && now > last
        {
            std::thread::sleep(std::time::Duration::from_secs((now - last).min(3) as u64));
        }
        last = collected;
        screen.draw(&render_view(&metrics, max_paths, metrics.is_final))?;
    }
    Ok(())
}

/// Redraws a multi-line view in place (terminal only).
#[derive(Default)]
struct Screen {
    lines: usize,
}

impl Screen {
    fn draw(&mut self, view: &str) -> Result<()> {
        let mut stdout = std::io::stdout().lock();
        if self.lines > 0 {
            write!(stdout, "\x1b[{}A\r\x1b[J", self.lines)?;
        }
        write!(stdout, "{view}")?;
        stdout.flush()?;
        self.lines = view.matches('\n').count();
        Ok(())
    }
}

/// mc `bucketScanMsg` (`stats` is `null` when the server returns none).
#[derive(Serialize)]
struct BucketScanMessage<'a> {
    status: &'static str,
    stats: Option<&'a Vec<BucketScanInfo>>,
}

/// mc `newPrettyTable(" | ", Field{"Pool", 5}, Field{"Set", 5}, Field{"LastUpdate", 20})`.
fn pretty_row(cells: [&str; 3]) -> String {
    let widths = [5, 5, 20];
    cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| {
            let cell: String = if cell.chars().count() > width {
                cell.chars().take(width - 3).collect::<String>() + "..."
            } else {
                cell.to_string()
            };
            format!("{cell:<width$.width$}")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Go `%dd%dh%dm` of a duration in seconds.
fn day_hour_min(secs: i64) -> String {
    format!("{}d{}h{}m", secs / 86_400, secs / 3600 % 24, secs / 60 % 60)
}

/// mc `bucketScanMsg.String()`.
fn render_bucket_stats(mut stats: Vec<BucketScanInfo>, now: i64) -> String {
    let secs = |time: &str| api::unix_seconds(time).unwrap_or(i64::MIN / 2);
    stats.sort_by_key(|s| secs(&s.last_update));
    let mut out = String::from("\n");
    out.push_str(&pretty_row(["Pool", "Set", "Last Update"]));
    out.push('\n');
    for stat in &stats {
        out.push_str(&pretty_row([
            &(stat.pool + 1).to_string(),
            &(stat.set + 1).to_string(),
            &api::rel_time(secs(&stat.last_update) - now, "", "ago"),
        ]));
        out.push('\n');
    }
    // A full bucket scan needs 16 completed cycles on every erasure set.
    let mut earliest: Option<i64> = None;
    let mut latest: Option<i64> = None;
    let mut full = true;
    for stat in &stats {
        let completed = stat.completed.as_deref().unwrap_or_default();
        if completed.len() < 16 {
            full = false;
            break;
        }
        let first = secs(&completed[0]);
        let last = secs(&completed[completed.len() - 1]);
        earliest = Some(earliest.map_or(first, |e: i64| e.max(first)));
        latest = Some(latest.map_or(last, |l: i64| l.min(last)));
    }
    out.push('\n');
    if full && let (Some(earliest), Some(latest)) = (earliest, latest) {
        out.push_str(&format!(
            "Full bucket scan:  {} (took {})\n",
            api::rel_time(latest - now, "", "ago"),
            day_hour_min(latest - earliest)
        ));
    }
    out.push('\n');
    out
}

/// mc `metricsDuration`: rounded Go duration, `0ms` for zero.
fn metrics_duration(nanos: u64) -> String {
    if nanos == 0 {
        return "0ms".to_string();
    }
    let round = |value: u64, unit: u64| (value + unit / 2) / unit * unit;
    let mut d = nanos;
    if d > 1_000_000 {
        d = round(d, 1_000);
    }
    if d > 1_000_000_000 {
        d = round(d, 1_000_000);
    }
    if d > 60_000_000_000 {
        d = round(d, 100_000_000);
    }
    api::go_duration_string(d as i64)
}

/// Go `time.Time.String()` of an RFC3339 UTC time (`2006-01-02 15:04:05.999999999 +0000 UTC`).
fn go_time_string(time: &str) -> String {
    let Some(rest) = time.strip_suffix('Z') else {
        return time.to_string();
    };
    format!("{} +0000 UTC", rest.replacen('T', " ", 1))
}

/// mc `scannerMetricsUI.View()` (without colors and spinner).
fn render_view(metrics: &RealtimeMetrics, max_paths: i64, quitting: bool) -> String {
    let mut out = String::new();
    if !quitting {
        out.push_str("Scanner Activity: \n");
    }
    let Some(sc) = &metrics.aggregated.scanner else {
        out.push_str("(waiting for data)");
        return out;
    };
    let mut rows: Vec<String> = vec![String::new()];
    rows.extend(scanner_rows(sc));
    if !metrics.errors.is_empty() {
        rows.push("------------------------------------------- Errors --------------------------------------------------".to_string());
        rows.extend(metrics.errors.iter().cloned());
    }
    if max_paths != 0 && !sc.active_paths.is_empty() {
        rows.push("------------------------------------- Currently Scanning Paths --------------------------------------".to_string());
        for (index, path) in sc.active_paths.iter().enumerate() {
            if index as i64 == max_paths {
                break;
            }
            let path = if path.chars().count() > 100 {
                path.chars().take(97).collect::<String>() + "..."
            } else {
                path.clone()
            };
            rows.push(path.replace('\\', "/"));
        }
    }
    for row in rows {
        out.push_str(&row);
        out.push('\n');
    }
    out
}

fn scanner_rows(sc: &ScannerMetrics) -> Vec<String> {
    const WANT_CYCLES: usize = 16;
    let mut rows = Vec::new();
    let mut cycles: Vec<i64> = sc
        .cycles_completed_at
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| api::unix_seconds(t))
        .collect();
    if sc.current_cycle == 0
        && sc.current_started == api::GO_ZERO_TIME
        && sc.cycles_completed_at.is_none()
    {
        rows.push(format!("     Scanning: {} bucket(s)", sc.ongoing_buckets));
    } else {
        if cycles.len() < 2 {
            rows.push("Last full scan time:             Unknown (not enough data)".to_string());
        } else {
            rows.push("Overall Statistics".to_string());
            rows.push("------------------".to_string());
            cycles.sort_unstable_by(|a, b| b.cmp(a));
            let (label, since_last) = if cycles.len() >= WANT_CYCLES {
                ("Last full scan time:", cycles[0] - cycles[WANT_CYCLES - 1])
            } else {
                (
                    "Est. full scan time:",
                    (cycles[0] - cycles[1]) * WANT_CYCLES as i64,
                )
            };
            let per_month = (30.0 * 86_400.0) / since_last.max(1) as f64;
            rows.push(format!(
                "{label}   {}; Estimated {per_month:.2}/month",
                day_hour_min(since_last)
            ));
        }
        if sc.current_cycle > 0 {
            rows.push(format!(
                "Current cycle:         {}; Started: {}",
                sc.current_cycle,
                go_time_string(&sc.current_started)
            ));
        } else {
            rows.push("Current cycle:         (between cycles)".to_string());
        }
    }
    rows.push(format!("Active drives: {}", sc.active_paths.len()));
    let action = |name: &str| {
        sc.last_minute
            .actions
            .get(name)
            .cloned()
            .unwrap_or_default()
    };
    let rate = |x: &api::TimedAction| {
        if x.acc_time > 0 {
            let per_day = (86_400e9 / (60e9 / x.count as f64)) as u64;
            format!("; Rate: {per_day}/day")
        } else {
            String::new()
        }
    };
    rows.push(String::new());
    rows.push("Last Minute Statistics".to_string());
    rows.push("----------------------".to_string());
    let x = action("ScanObject");
    let objects = x.count;
    rows.push(format!(
        "Objects Scanned:       {} objects; Avg: {}{}",
        x.count,
        metrics_duration(x.avg()),
        rate(&x)
    ));
    let x = action("ApplyVersion");
    rows.push(format!(
        "Versions Scanned:      {} versions; Avg: {}{}",
        x.count,
        metrics_duration(x.avg()),
        rate(&x)
    ));
    let x = action("HealCheck");
    rows.push(format!(
        "Versions Heal Checked: {} versions; Avg: {}{}",
        x.count,
        metrics_duration(x.avg()),
        rate(&x)
    ));
    let x = action("ReadMetadata");
    rows.push(format!(
        "Read Metadata:         {} objects; Avg: {}, Size: {} bytes/obj",
        x.count,
        metrics_duration(x.avg()),
        x.avg_bytes()
    ));
    let x = action("ILM");
    rows.push(format!(
        "ILM checks:            {} versions; Avg: {}",
        x.count,
        metrics_duration(x.avg())
    ));
    let x = action("CheckReplication");
    rows.push(format!(
        "Check Replication:     {} versions; Avg: {}",
        x.count,
        metrics_duration(x.avg())
    ));
    let x = action("TierObjSweep");
    if x.count > 0 {
        rows.push(format!(
            "Sweep Tiered:        {} versions; Avg: {}",
            x.count,
            metrics_duration(x.avg())
        ));
    }
    let x = action("CheckMissing");
    rows.push(format!(
        "Verify Deleted:        {} folders; Avg: {}",
        x.count,
        metrics_duration(x.avg())
    ));
    let x = action("HealAbandonedObject");
    if x.count > 0 {
        rows.push(format!(
            " Missing Objects:      {} objects healed; Avg: {}{}",
            x.count,
            metrics_duration(x.avg()),
            rate(&x)
        ));
    }
    let x = action("HealAbandonedVersion");
    if x.count > 0 {
        rows.push(format!(
            " Missing Versions:     {} versions healed; Avg: {}{}; {} bytes/v",
            x.count,
            metrics_duration(x.avg()),
            rate(&x),
            x.avg_bytes()
        ));
    }
    for (name, x) in &sc.last_minute.ilm {
        rows.push(format!(
            "ILM, {:<17} {} actions; Avg: {}.",
            format!("{name}:"),
            x.count,
            metrics_duration(x.avg())
        ));
    }
    let x = action("Yield");
    let avg = match x.acc_time.checked_div(objects) {
        Some(per_object) => format!("{}/obj", metrics_duration(per_object)),
        None => metrics_duration(x.avg()),
    };
    rows.push(format!(
        "Yield:                 {} total; Avg: {avg}",
        metrics_duration(x.acc_time)
    ));
    rows
}

fn trace(args: ScannerTraceArgs, json: bool) -> Result<()> {
    super::trace::scanner_trace(args, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_stats_render_like_mc() {
        let stats: Vec<BucketScanInfo> = serde_json::from_str(
            r#"[{"Pool":0,"Set":1,"LastUpdate":"1970-01-01T00:00:50Z","LastStarted":"1970-01-01T00:00:00Z","Completed":null}]"#,
        )
        .unwrap();
        assert_eq!(
            render_bucket_stats(stats, 60),
            "\nPool  | Set   | Last Update         \n1     | 2     | 10 seconds ago      \n\n\n"
        );
        assert_eq!(
            serde_json::to_string(&BucketScanMessage {
                status: "success",
                stats: None
            })
            .unwrap(),
            r#"{"status":"success","stats":null}"#
        );
    }

    #[test]
    fn view_summarizes_scanner_metrics() {
        let metrics: RealtimeMetrics = serde_json::from_str(
            r#"{"hosts":["h"],"aggregated":{"scanner":{"collected":"2026-09-26T19:25:59Z","current_cycle":3,"current_started":"2026-09-26T19:25:08.5Z","cycle_complete_times":["2026-09-26T19:25:08Z"],"ongoing_buckets":0,"last_minute":{"actions":{"ScanObject":{"count":2,"acc_time_ns":4000000}}}}},"final":true}"#,
        )
        .unwrap();
        let view = render_view(&metrics, -1, true);
        assert!(view.starts_with(
            "\nLast full scan time:             Unknown (not enough data)\nCurrent cycle:         3; Started: 2026-09-26 19:25:08.5 +0000 UTC\nActive drives: 0\n"
        ));
        assert!(view.contains("Objects Scanned:       2 objects; Avg: 2ms; Rate: 2880/day\n"));
        assert!(view.ends_with("Yield:                 0ms total; Avg: 0ms/obj\n"));
        assert_eq!(metrics_duration(1_234_567), "1.235ms");
        assert_eq!(metrics_duration(0), "0ms");
    }
}
