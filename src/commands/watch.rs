//! `mx watch` (mc `watch`).
//!
//! Owner: STREAM. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct WatchArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(
        long = "events",
        default_value = "put,delete,get",
        value_name = "VALUE",
        help = "filter specific types of events; defaults to all events by default"
    )]
    pub events: String,
    #[arg(
        long = "prefix",
        value_name = "VALUE",
        help = "filter events for a prefix"
    )]
    pub prefix: Option<String>,
    #[arg(
        long = "suffix",
        value_name = "VALUE",
        help = "filter events for a suffix"
    )]
    pub suffix: Option<String>,
    #[arg(long = "recursive", help = "recursively watch for events")]
    pub recursive: bool,
}

pub fn run(args: WatchArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("watch")
}
