//! `mx admin trace` (mc `admin trace`).
//!
//! Owner: STREAM (also `admin scanner trace`, whose Args are in `scanner.rs`). Stubs return
//! "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct TraceArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(long = "verbose", short = 'v', help = "print verbose trace")]
    pub verbose: bool,
    #[arg(long = "all", short = 'a', help = "trace all call types")]
    pub all: bool,
    #[arg(
        long = "call",
        value_name = "VALUE",
        help = "trace only matching call types. See CALL TYPES below for list."
    )]
    pub call: Vec<String>,
    #[arg(
        long = "status-code",
        value_name = "VALUE",
        help = "trace only matching status code"
    )]
    pub status_code: Vec<i64>,
    #[arg(
        long = "method",
        value_name = "VALUE",
        help = "trace only matching HTTP method"
    )]
    pub method: Vec<String>,
    #[arg(
        long = "funcname",
        value_name = "VALUE",
        help = "trace only matching func name"
    )]
    pub funcname: Vec<String>,
    #[arg(long = "path", value_name = "VALUE", help = "trace only matching path")]
    pub path: Vec<String>,
    #[arg(
        long = "node",
        value_name = "VALUE",
        help = "trace only matching servers"
    )]
    pub node: Vec<String>,
    #[arg(
        long = "request-header",
        value_name = "VALUE",
        help = "trace only matching request headers"
    )]
    pub request_header: Vec<String>,
    #[arg(
        long = "request-query",
        value_name = "VALUE",
        help = "trace only matching request queries"
    )]
    pub request_query: Vec<String>,
    #[arg(long = "errors", short = 'e', help = "trace only failed requests")]
    pub errors: bool,
    #[arg(
        long = "stats",
        help = "print statistical summary of all the traced calls"
    )]
    pub stats: bool,
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
        default_value = "0s",
        value_name = "VALUE",
        help = "trace calls only with response duration greater than this threshold (e.g. 5ms)"
    )]
    pub response_duration: String,
    #[arg(
        long = "filter-size",
        value_name = "VALUE",
        help = "filter size, use with filter (see UNITS)"
    )]
    pub filter_size: Option<String>,
    #[arg(
        long = "in",
        value_name = "VALUE",
        help = "read previously saved json from file and replay"
    )]
    pub in_: Option<String>,
    #[arg(
        long = "stats-n",
        hide = true,
        default_value_t = 20,
        value_name = "VALUE",
        help = "maximum number of stat entries"
    )]
    pub stats_n: i64,
}

pub fn run(args: TraceArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin trace")
}

/// `mx admin scanner trace` (Args in `scanner.rs`).
pub fn scanner_trace(args: super::scanner::ScannerTraceArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin scanner trace")
}
