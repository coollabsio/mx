//! `mx admin prometheus generate|metrics` (mc `admin prometheus`): scrape config with an HS512
//! bearer token, and metrics fetched from `/minio/v2/metrics/...` or `/minio/metrics/v3/...`.
//!
//! Owner: SERVER.

use crate::commands::runtime;
use crate::config::model::AliasConfig;
use crate::error::McError;
use crate::s3::admin::{AdminClient, encode_component};
use crate::s3::admin_server as api;
use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct PrometheusArgs {
    #[command(subcommand)]
    pub command: PrometheusCommand,
}

#[derive(Debug, Subcommand)]
pub enum PrometheusCommand {
    #[command(name = "generate", about = "generates prometheus config")]
    Generate(PrometheusGenerateArgs),
    #[command(name = "metrics", about = "print prometheus metrics")]
    Metrics(PrometheusMetricsArgs),
}

#[derive(Debug, Args)]
pub struct PrometheusGenerateArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "METRIC-TYPE")]
    pub metric_type: Option<String>,
    #[arg(
        long = "bucket",
        value_name = "VALUE",
        help = "bucket name to list metrics for. only applicable with api version v3 for metric type 'api, replication'"
    )]
    pub bucket: Option<String>,
    #[arg(
        long = "api-version",
        default_value = "v2",
        value_name = "VALUE",
        help = "version of metrics api to use. valid values are ['v2', 'v3']. defaults to 'v2' if not specified."
    )]
    pub api_version: String,
    #[arg(
        long = "public",
        help = "disable bearer token generation for scrape_configs"
    )]
    pub public: bool,
}

#[derive(Debug, Args)]
pub struct PrometheusMetricsArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "METRIC-TYPE")]
    pub metric_type: Option<String>,
    #[arg(
        long = "bucket",
        value_name = "VALUE",
        help = "bucket name to list metrics for. only applicable with api version v3 for metric type 'api, replication'"
    )]
    pub bucket: Option<String>,
    #[arg(
        long = "api-version",
        default_value = "v2",
        value_name = "VALUE",
        help = "version of metrics api to use. valid values are ['v2', 'v3']. defaults to 'v2' if not specified."
    )]
    pub api_version: String,
}

pub fn run(args: PrometheusArgs, json: bool) -> Result<()> {
    match args.command {
        PrometheusCommand::Generate(args) => generate(args, json),
        PrometheusCommand::Metrics(args) => metrics(args, json),
    }
}

const V2_SUBSYSTEMS: [&str; 4] = ["bucket", "cluster", "node", "resource"];
const V3_SUBSYSTEMS: [&str; 10] = [
    "api",
    "audit",
    "cluster",
    "debug",
    "ilm",
    "logger",
    "notification",
    "replication",
    "scanner",
    "system",
];
const V3_BUCKET_SUBSYSTEMS: [&str; 2] = ["api", "replication"];

/// mc `fatalIf(errInvalidArgument(), message)`.
fn invalid(message: String) -> anyhow::Error {
    anyhow::Error::new(McError::invalid_argument()).context(message)
}

/// mc `cleanAlias` + `isValidAlias` + `mustGetHostConfig`.
fn host_config(target: &str) -> Result<AliasConfig> {
    let alias = target
        .strip_suffix('/')
        .or_else(|| target.strip_suffix('\\'))
        .unwrap_or(target);
    let valid = alias
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err(anyhow!(
            "Alias `{alias}` should have alphanumeric characters such as [helloWorld0, hello_World0, ...] and begin with a letter"
        )
        .context("Invalid alias."));
    }
    let store = crate::config::ConfigStore::load_or_create()?;
    store.config().aliases.get(alias).cloned().ok_or_else(|| {
        anyhow::Error::new(McError::invalid_aliased_url(alias))
            .context(format!("No such alias `{alias}` found."))
    })
}

/// mc `validateV2Args`; returns the metric type (`cluster` by default).
fn v2_subsystem(metric_type: Option<&str>, bucket: Option<&str>) -> Result<String> {
    let subsystem = metric_type.filter(|t| !t.is_empty()).unwrap_or("cluster");
    if bucket.is_some() {
        return Err(invalid(
            "Flag `bucket` is not supported with v2 metrics".to_string(),
        ));
    }
    if !V2_SUBSYSTEMS.contains(&subsystem) {
        return Err(invalid(format!(
            "invalid metric type `{subsystem}`. valid values are `{}`",
            V2_SUBSYSTEMS.join(", ")
        )));
    }
    Ok(subsystem.to_string())
}

/// mc `validateV3Args`.
fn validate_v3(subsystem: &str, bucket: &str) -> Result<()> {
    if !subsystem.is_empty() && !V3_SUBSYSTEMS.contains(&subsystem) {
        return Err(invalid(format!(
            "invalid metric type `{subsystem}`. valid values are `{}`",
            V3_SUBSYSTEMS.join(", ")
        )));
    }
    if !bucket.is_empty() {
        let valid = V3_BUCKET_SUBSYSTEMS.join(", ");
        if subsystem.is_empty() {
            return Err(invalid(format!(
                "metric type must be passed with --bucket. valid values are `{valid}`"
            )));
        }
        if !V3_BUCKET_SUBSYSTEMS.contains(&subsystem) {
            return Err(invalid(format!(
                "--bucket is applicable only for metric types `{valid}`"
            )));
        }
    }
    Ok(())
}

/// mc `getMetricsV3Path`.
fn v3_path(subsystem: &str, bucket: &str) -> String {
    let mut path = "/minio/metrics/v3".to_string();
    if !bucket.is_empty() {
        path.push_str("/bucket");
    }
    if !subsystem.is_empty() {
        path.push('/');
        path.push_str(subsystem);
    }
    if !bucket.is_empty() {
        path.push('/');
        path.push_str(bucket);
    }
    path
}

fn invalid_api_version(version: &str) -> anyhow::Error {
    invalid(format!("Invalid api version `{version}`"))
}

// ---------------------------------------------------------------------------
// generate
// ---------------------------------------------------------------------------

/// mc `StatConfig`.
#[derive(Serialize)]
struct StatConfig {
    targets: Vec<String>,
}

/// mc `ScrapeConfig` (JSON field names; YAML is rendered by [`scrape_yaml`]).
#[derive(Serialize)]
struct ScrapeConfig {
    #[serde(rename = "jobName")]
    job_name: String,
    #[serde(rename = "bearerToken", skip_serializing_if = "String::is_empty")]
    bearer_token: String,
    #[serde(rename = "metricsPath")]
    metrics_path: String,
    scheme: String,
    #[serde(rename = "staticConfigs")]
    static_configs: Vec<StatConfig>,
}

fn generate(args: PrometheusGenerateArgs, json: bool) -> Result<()> {
    let host = host_config(&args.target)?;
    let url = url::Url::parse(&host.url).with_context(|| format!("parse {:?}", host.url))?;
    let metric_type = args.metric_type.as_deref().unwrap_or_default();
    let (job_name, metrics_path) = match args.api_version.as_str() {
        "v2" => {
            let subsystem = v2_subsystem(Some(metric_type), args.bucket.as_deref())?;
            let job = if subsystem == "cluster" {
                "minio-job".to_string()
            } else {
                format!("minio-job-{subsystem}")
            };
            (job, format!("/minio/v2/metrics/{subsystem}"))
        }
        "v3" => {
            let bucket = args.bucket.as_deref().unwrap_or_default();
            validate_v3(metric_type, bucket)?;
            let job = if metric_type.is_empty() {
                "minio-job".to_string()
            } else {
                format!("minio-job-{metric_type}")
            };
            (job, v3_path(metric_type, bucket))
        }
        other => return Err(invalid_api_version(other)),
    };
    let bearer_token = if args.public {
        String::new()
    } else {
        api::prometheus_token(&host.access_key, &host.secret_key, api::now_unix())
    };
    let authority = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let config = ScrapeConfig {
        job_name,
        bearer_token,
        metrics_path,
        scheme: url.scheme().to_string(),
        static_configs: vec![StatConfig {
            targets: vec![authority],
        }],
    };
    if json {
        return crate::output::print_json(&config);
    }
    print!("{}", scrape_yaml(&config));
    Ok(())
}

/// Scalars that YAML 1.1 would resolve to something other than a string.
fn yaml_non_string(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "" | "~"
            | "null"
            | "true"
            | "false"
            | "yes"
            | "no"
            | "on"
            | "off"
            | "y"
            | "n"
            | ".inf"
            | "-.inf"
            | "+.inf"
            | ".nan"
    ) || value.parse::<f64>().is_ok()
}

/// yaml.v2 scalar style: double quotes for non-string lookalikes, single quotes when the
/// plain style is not allowed (indicators; `:` anywhere inside a flow sequence).
fn yaml_scalar(value: &str, flow: bool) -> String {
    if yaml_non_string(value) {
        return format!("\"{value}\"");
    }
    let first = value.chars().next().unwrap_or(' ');
    let needs_quotes = "-?:,[]{}#&*!|>'\"%@`".contains(first)
        || value.starts_with(' ')
        || value.ends_with(' ')
        || value.contains(": ")
        || value.contains(" #")
        || value.ends_with(':')
        || (flow && value.contains([',', '?', '[', ']', '{', '}', ':']));
    if needs_quotes {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        value.to_string()
    }
}

/// yaml.v2 marshal of mc `PrometheusConfig`.
fn scrape_yaml(config: &ScrapeConfig) -> String {
    let mut out = format!(
        "scrape_configs:\n- job_name: {}\n",
        yaml_scalar(&config.job_name, false)
    );
    if !config.bearer_token.is_empty() {
        out.push_str(&format!(
            "  bearer_token: {}\n",
            yaml_scalar(&config.bearer_token, false)
        ));
    }
    if !config.metrics_path.is_empty() {
        out.push_str(&format!(
            "  metrics_path: {}\n",
            yaml_scalar(&config.metrics_path, false)
        ));
    }
    if !config.scheme.is_empty() {
        out.push_str(&format!(
            "  scheme: {}\n",
            yaml_scalar(&config.scheme, false)
        ));
    }
    out.push_str("  static_configs:\n");
    for stat in &config.static_configs {
        let targets: Vec<String> = stat.targets.iter().map(|t| yaml_scalar(t, true)).collect();
        out.push_str(&format!("  - targets: [{}]\n", targets.join(", ")));
    }
    out
}

// ---------------------------------------------------------------------------
// metrics
// ---------------------------------------------------------------------------

fn metrics(args: PrometheusMetricsArgs, json: bool) -> Result<()> {
    let host = host_config(&args.target)?;
    let token = api::prometheus_token(&host.access_key, &host.secret_key, api::now_unix());
    let metric_type = args.metric_type.as_deref().unwrap_or_default();
    let (path, version) = match args.api_version.as_str() {
        "v2" => {
            let subsystem = v2_subsystem(Some(metric_type), args.bucket.as_deref())?;
            (format!("/minio/v2/metrics/{subsystem}"), "v2")
        }
        "v3" => {
            let bucket = args.bucket.as_deref().unwrap_or_default();
            validate_v3(metric_type, bucket)?;
            (v3_path(metric_type, &encode_component(bucket)), "v3")
        }
        other => return Err(invalid_api_version(other)),
    };
    let client = AdminClient::new(&host)?;
    let message = format!("Unable to list prometheus metrics with api-version {version}.");
    let response = runtime()?
        .block_on(api::fetch_metrics(&client, &path, &token))
        .context(message.clone())?;
    if response.status != 200 {
        let reason = http::StatusCode::from_u16(response.status)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or_default();
        return Err(anyhow!("{} {reason}", response.status).context(message));
    }
    if json {
        let families = api::parse_prometheus(&response.text())
            .map_err(|err| anyhow!("reading text format failed: {err}"))
            .context("Unable to parse Prometheus metrics.")?;
        return crate::output::print_json(&families);
    }
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&response.body)?;
    writeln!(stdout)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(token: &str) -> ScrapeConfig {
        ScrapeConfig {
            job_name: "minio-job-node".into(),
            bearer_token: token.into(),
            metrics_path: "/minio/v2/metrics/node".into(),
            scheme: "http".into(),
            static_configs: vec![StatConfig {
                targets: vec!["127.0.0.1:9000".into()],
            }],
        }
    }

    #[test]
    fn renders_yaml_like_mc() {
        assert_eq!(
            scrape_yaml(&config("")),
            "scrape_configs:\n- job_name: minio-job-node\n  metrics_path: /minio/v2/metrics/node\n  scheme: http\n  static_configs:\n  - targets: ['127.0.0.1:9000']\n"
        );
        let mut plain = config("abc.def-ghi_j");
        plain.static_configs[0].targets[0] = "play.min.io".into();
        assert!(scrape_yaml(&plain).contains("  bearer_token: abc.def-ghi_j\n"));
        assert!(scrape_yaml(&plain).ends_with("  - targets: [play.min.io]\n"));
    }

    #[test]
    fn json_omits_empty_token() {
        assert_eq!(
            serde_json::to_string(&config("")).unwrap(),
            r#"{"jobName":"minio-job-node","metricsPath":"/minio/v2/metrics/node","scheme":"http","staticConfigs":[{"targets":["127.0.0.1:9000"]}]}"#
        );
    }

    #[test]
    fn validates_metric_types_like_mc() {
        assert_eq!(v2_subsystem(None, None).unwrap(), "cluster");
        let err = v2_subsystem(Some("bogus"), None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid metric type `bogus`. valid values are `bucket, cluster, node, resource`"
        );
        assert!(v2_subsystem(Some("node"), Some("b")).is_err());
        assert!(validate_v3("", "b").is_err());
        assert!(validate_v3("system", "b").is_err());
        assert!(validate_v3("api", "b").is_ok());
        assert_eq!(v3_path("api", "b"), "/minio/metrics/v3/bucket/api/b");
        assert_eq!(v3_path("", ""), "/minio/metrics/v3");
    }
}
