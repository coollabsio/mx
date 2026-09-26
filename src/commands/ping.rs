//! `ping` (mc ping): liveness checks against MinIO's `/minio/health/live` endpoint.

use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use anyhow::{Context, Result, bail};
use clap::Args;
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Debug, Args)]
pub struct PingArgs {
    /// perform liveliness check for count number of times (default: 0)
    #[arg(short = 'c', long)]
    pub count: Option<u64>,
    /// exit after N consecutive ping errors (default: 0)
    #[arg(short = 'e', long = "error-count")]
    pub error_count: Option<u64>,
    /// exit when server(s) responds and reports being online
    #[arg(short = 'x', long)]
    pub exit: bool,
    /// wait interval between each request in seconds
    #[arg(short = 'i', long, default_value_t = 1)]
    pub interval: u64,
    /// ping all the servers in the cluster, use it when you have direct access to nodes/pods
    ///
    /// requires the MinIO admin API; not supported by mx
    #[arg(short = 'a', long)]
    pub distributed: bool,
    /// ping the specified node
    ///
    /// requires the MinIO admin API; not supported by mx
    #[arg(long)]
    pub node: Option<String>,
    pub target: String,
}

/// Running statistics for one endpoint (mc `ServerStats`, durations in nanoseconds).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ServerStats {
    pub endpoint: Endpoint,
    pub min: u64,
    pub max: u64,
    pub sum: u64,
    pub avg: u64,
    pub dns: u64,
    #[serde(rename = "errorCount")]
    pub error_count: u64,
    pub err: String,
    pub counter: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Endpoint {
    #[serde(rename = "Scheme")]
    pub scheme: String,
    #[serde(rename = "Host")]
    pub host: String,
}

impl ServerStats {
    /// mc `pingStats`: errors keep min/max/avg and count consecutive failures.
    pub fn record(&mut self, elapsed: Duration, error: Option<String>) {
        match error {
            Some(error) => {
                self.err = error;
                self.error_count += 1;
            }
            None => {
                let nanos = elapsed.as_nanos() as u64;
                self.err.clear();
                self.error_count = 0;
                self.min = if self.counter == 0 || self.min == 0 {
                    nanos
                } else {
                    self.min.min(nanos)
                };
                self.max = self.max.max(nanos);
                self.sum += nanos;
                self.counter += 1;
                self.avg = self.sum / self.counter;
            }
        }
    }
}

/// mc `trimToTwoDecimal`.
pub fn format_time(duration: Duration) -> String {
    let (value, unit, width) = if duration >= Duration::from_secs(1) {
        (duration.as_secs_f64(), "s", 7)
    } else {
        (duration.as_secs_f64() * 1000.0, "ms", 6)
    };
    let number = format!("{value:.2}");
    let pad = width - number.len().min(width);
    format!("{number}{unit}{}", " ".repeat(pad))
}

#[derive(Serialize)]
struct EndpointStat<'a> {
    endpoint: &'a Endpoint,
    dns: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    error: &'a str,
    time: &'a str,
}

#[derive(Serialize)]
struct PingResult<'a> {
    status: &'a str,
    counter: &'a str,
    servers: Vec<EndpointStat<'a>>,
}

#[derive(Serialize)]
struct PingSummary<'a> {
    status: &'a str,
    #[serde(rename = "serverMap")]
    server_map: &'a BTreeMap<String, ServerStats>,
}

pub fn run(args: PingArgs, json: bool) -> Result<()> {
    if args.distributed || args.node.is_some() {
        bail!(
            "`--distributed` and `--node` need the MinIO admin API (server info), which mx does not support yet"
        );
    }
    if args.count == Some(0) {
        bail!("ping count cannot be less than 1");
    }
    let store = ConfigStore::load_or_create()?;
    let alias_name = args.target.split('/').next().unwrap_or_default();
    let alias = alias_config(&store, alias_name)
        .with_context(|| format!("Unable to initialize admin client for `{}`.", args.target))?;
    let url = url::Url::parse(&alias.url)?;
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let endpoint = Endpoint {
        scheme: url.scheme().to_string(),
        host: host.clone(),
    };
    let rt = runtime()?;
    let mut summary: BTreeMap<String, ServerStats> = BTreeMap::new();
    let mut index: u64 = 1;
    loop {
        let started = Instant::now();
        let result = rt.block_on(crate::s3::health_request(
            &alias,
            "/minio/health/live",
            None,
        ));
        let elapsed = started.elapsed();
        let (online, error) = match result {
            Ok(response) => (
                response.status == 200
                    && response.header("x-minio-server-status") != Some("offline"),
                None,
            ),
            Err(error) => (false, Some(format!("{error:#}"))),
        };
        let stats = summary.entry(host.clone()).or_insert_with(|| ServerStats {
            endpoint: endpoint.clone(),
            ..Default::default()
        });
        stats.record(elapsed, error.clone());
        let status = if online { "ok " } else { "failed " };
        let time = format_time(elapsed);
        let counter = format!("{index:>3}");
        if json {
            crate::output::print_json(&PingResult {
                status: "success",
                counter: &counter,
                servers: vec![EndpointStat {
                    endpoint: &endpoint,
                    dns: "0s",
                    status,
                    error: error.as_deref().unwrap_or_default(),
                    time: &time,
                }],
            })?;
        } else {
            println!(
                "{counter}: {}://{host}   status={status} time={time}",
                endpoint.scheme
            );
        }
        let stop = (args.exit && online)
            || args
                .error_count
                .is_some_and(|limit| stats.error_count >= limit)
            || args.count.is_some_and(|count| index >= count);
        if stop {
            break;
        }
        index += 1;
        std::thread::sleep(Duration::from_secs(args.interval));
    }
    print_summary(&summary, json)
}

fn print_summary(summary: &BTreeMap<String, ServerStats>, json: bool) -> Result<()> {
    if json {
        crate::output::print_json(&PingSummary {
            status: "success",
            server_map: summary,
        })?;
        return Ok(());
    }
    let rows = summary
        .iter()
        .map(|(host, stats)| {
            vec![
                format!("{}://{host}", stats.endpoint.scheme),
                format_time(Duration::from_nanos(stats.min)),
                format_time(Duration::from_nanos(stats.avg)),
                format_time(Duration::from_nanos(stats.max)),
                stats.error_count.to_string(),
                stats.counter.to_string(),
            ]
        })
        .collect::<Vec<_>>();
    print!(
        "{}",
        crate::commands::ilm::render_table(
            None,
            &["Endpoint", "Min", "Avg", "Max", "Error", "Count"],
            &rows
        )
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_times_like_mc() {
        assert_eq!(format_time(Duration::from_micros(1234)), "1.23ms  ");
        assert_eq!(format_time(Duration::from_millis(250)), "250.00ms");
        assert_eq!(format_time(Duration::from_millis(1500)), "1.50s   ");
    }

    #[test]
    fn records_stats() {
        let mut stats = ServerStats::default();
        stats.record(Duration::from_nanos(300), None);
        stats.record(Duration::from_nanos(100), None);
        stats.record(Duration::from_nanos(0), Some("boom".into()));
        stats.record(Duration::from_nanos(0), Some("boom".into()));
        assert_eq!(
            (
                stats.min,
                stats.max,
                stats.avg,
                stats.counter,
                stats.error_count
            ),
            (100, 300, 200, 2, 2)
        );
        stats.record(Duration::from_nanos(200), None);
        assert_eq!(stats.error_count, 0);
        assert_eq!(stats.counter, 3);
    }
}
