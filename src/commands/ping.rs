//! `ping` (mc ping): liveness checks against MinIO's `/minio/health/live` endpoint.

use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::error::McError;
use crate::s3::admin::AdminClient;
use crate::s3::admin_info::server_info;
use anyhow::{Context, Result};
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
    #[arg(short = 'a', long)]
    pub distributed: bool,
    /// ping the specified node
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

/// Go `*url.URL` as `encoding/json` renders it (mc prints the struct verbatim).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Endpoint {
    pub scheme: String,
    pub opaque: String,
    pub user: Option<String>,
    pub host: String,
    pub path: String,
    pub raw_path: String,
    pub omit_host: bool,
    pub force_query: bool,
    pub raw_query: String,
    pub fragment: String,
    pub raw_fragment: String,
}

impl Endpoint {
    /// `scheme://host[:port]` as Go keeps it (`Host` holds the port only when given).
    pub fn new(scheme: &str, host: &str) -> Self {
        Self {
            scheme: scheme.to_string(),
            host: host.to_string(),
            ..Default::default()
        }
    }

    pub fn url(&self) -> String {
        format!("{}://{}", self.scheme, self.host)
    }
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
    let store = ConfigStore::load_or_create()?;
    let alias_name = args.target.split('/').next().unwrap_or_default();
    let alias = alias_config(&store, alias_name)
        .with_context(|| format!("Unable to initialize admin client for `{}`.", args.target))?;
    let url = url::Url::parse(&alias.url)?;
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let rt = runtime()?;
    // `-a` pings every node the cluster reports, `--node` just that one (mc `filterAdminInfo`).
    let endpoints = if args.distributed || args.node.is_some() {
        let client = AdminClient::new(&alias)
            .with_context(|| format!("Unable to initialize admin client for `{}`.", args.target))?;
        let servers = rt
            .block_on(server_info(&client))
            .context("Unable to get server info")?;
        let servers = match &args.node {
            Some(node) if !servers.is_empty() => {
                let Some(server) = servers.into_iter().find(|s| &s.endpoint == node) else {
                    return Err(McError::invalid_argument())
                        .context(format!("Node {node} not exist"));
                };
                vec![server]
            }
            _ => servers,
        };
        let mut endpoints: Vec<Endpoint> = servers
            .iter()
            .map(|server| {
                let scheme = if server.scheme.is_empty() {
                    url.scheme()
                } else {
                    &server.scheme
                };
                Endpoint::new(scheme, &server.endpoint)
            })
            .collect();
        endpoints.sort_by(|a, b| a.host.cmp(&b.host));
        endpoints
    } else {
        Vec::new()
    };
    let endpoints = if endpoints.is_empty() {
        vec![Endpoint::new(url.scheme(), &host)]
    } else {
        endpoints
    };
    if args.count.is_some_and(|count| count < 1) {
        return Err(McError::invalid_argument()).context("ping count cannot be less than 1");
    }

    let mut summary: BTreeMap<String, ServerStats> = BTreeMap::new();
    let mut index: u64 = 1;
    loop {
        let mut all_ok = true;
        let mut stop = false;
        let mut lines = Vec::new();
        for endpoint in &endpoints {
            let node = crate::config::model::AliasConfig {
                url: endpoint.url(),
                ..alias.clone()
            };
            let started = Instant::now();
            let result = rt.block_on(crate::s3::health_request(&node, "/minio/health/live", None));
            let elapsed = started.elapsed();
            let (online, error) = match result {
                Ok(response) => (
                    response.status == 200
                        && response.header("x-minio-server-status") != Some("offline"),
                    None,
                ),
                Err(error) => (false, Some(format!("{error:#}"))),
            };
            all_ok &= online;
            let stats = summary
                .entry(endpoint.host.clone())
                .or_insert_with(|| ServerStats {
                    endpoint: endpoint.clone(),
                    ..Default::default()
                });
            stats.record(elapsed, error.clone());
            stop |= args
                .error_count
                .is_some_and(|limit| stats.error_count >= limit);
            lines.push((
                endpoint,
                if online { "ok " } else { "failed " },
                error.unwrap_or_default(),
                format_time(elapsed),
            ));
        }
        let counter = format!("{index:>3}");
        if json {
            crate::output::print_json(&PingResult {
                status: "success",
                counter: &counter,
                servers: lines
                    .iter()
                    .map(|(endpoint, status, error, time)| EndpointStat {
                        endpoint,
                        dns: "0s",
                        status,
                        error,
                        time,
                    })
                    .collect(),
            })?;
        } else {
            // Go tabwriter (padding 3) over `N: URL<TAB>status=... time=...`.
            let width = lines
                .iter()
                .map(|(endpoint, ..)| endpoint.url().len())
                .max()
                .unwrap_or(0);
            for (endpoint, status, _, time) in &lines {
                println!(
                    "{counter}: {:<width$}   status={status} time={time}",
                    endpoint.url()
                );
            }
        }
        stop |= (args.exit && all_ok) || args.count.is_some_and(|count| index >= count);
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
    print!("{}", console_table(&header_row(), &rows));
    Ok(())
}

fn header_row() -> Vec<String> {
    ["Endpoint", "Min", "Avg", "Max", "Error", "Count"]
        .map(String::from)
        .to_vec()
}

/// minio/pkg `console.Table.PopulateTable`: borders, no header separator, left-aligned cells.
fn console_table(header: &[String], rows: &[Vec<String>]) -> String {
    let mut all = vec![header.to_vec()];
    all.extend(rows.iter().cloned());
    let mut widths = vec![0; header.len()];
    for row in &all {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let border = |left: &str, mid: &str, right: &str| {
        let segments: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        format!("{left}{}{right}\n", segments.join(mid))
    };
    let mut out = border("┌", "┬", "┐");
    for row in &all {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        out.push_str(&format!("│ {} │\n", cells.join(" │ ")));
    }
    out.push_str(&border("└", "┴", "┘"));
    out
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
    fn summary_table_matches_mc_console_table() {
        let rows = vec![vec![
            "http://h:9000".to_string(),
            "0.51ms  ".to_string(),
            "0.62ms  ".to_string(),
            "0.73ms  ".to_string(),
            "0".to_string(),
            "2".to_string(),
        ]];
        assert_eq!(
            console_table(&header_row(), &rows),
            "┌───────────────┬──────────┬──────────┬──────────┬───────┬───────┐\n\
             │ Endpoint      │ Min      │ Avg      │ Max      │ Error │ Count │\n\
             │ http://h:9000 │ 0.51ms   │ 0.62ms   │ 0.73ms   │ 0     │ 2     │\n\
             └───────────────┴──────────┴──────────┴──────────┴───────┴───────┘\n"
        );
    }

    #[test]
    fn endpoint_serializes_like_go_url() {
        let value = serde_json::to_value(Endpoint::new("http", "h:9000")).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"Scheme":"http","Opaque":"","User":null,"Host":"h:9000","Path":"",
                "RawPath":"","OmitHost":false,"ForceQuery":false,"RawQuery":"","Fragment":"",
                "RawFragment":""})
        );
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
