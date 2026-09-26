//! `mx admin prometheus` (mc `admin prometheus`).
//!
//! Owner: SERVER. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

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

fn generate(args: PrometheusGenerateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin prometheus generate")
}

fn metrics(args: PrometheusMetricsArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin prometheus metrics")
}
