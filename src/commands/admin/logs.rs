//! `mx admin logs` (mc `admin logs`).
//!
//! Owner: STREAM. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct LogsArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "NODENAME")]
    pub nodename: Option<String>,
    #[arg(
        long = "last",
        short = 'l',
        default_value_t = 10,
        value_name = "VALUE",
        help = "show last n log entries"
    )]
    pub last: i64,
    #[arg(
        long = "type",
        short = 't',
        default_value = "all",
        value_name = "VALUE",
        help = "list error logs by type. Valid options are '[minio, application, all]'"
    )]
    pub type_: String,
}

pub fn run(args: LogsArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin logs")
}
