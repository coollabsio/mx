//! `mx admin scanner` (mc `admin scanner`).
//!
//! Owner: SERVER (`status`). `trace` runs `super::trace::scanner_trace` (owner: STREAM); its
//! flags live here. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

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
    #[arg(value_name = "TARGET")]
    pub target: String,
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
    let _ = (args, json);
    not_implemented("admin scanner status")
}

fn trace(args: ScannerTraceArgs, json: bool) -> Result<()> {
    super::trace::scanner_trace(args, json)
}
