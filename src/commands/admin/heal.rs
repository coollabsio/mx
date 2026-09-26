//! `mx admin heal` (mc `admin heal`).
//!
//! Owner: STREAM. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct HealArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(long = "force", help = "avoid showing a warning prompt")]
    pub force: bool,
    #[arg(long = "verbose", short = 'v', help = "show verbose information")]
    pub verbose: bool,
    #[arg(
        long = "all-drives",
        short = 'a',
        help = "select all drives for verbose printing"
    )]
    pub all_drives: bool,
    #[arg(
        long = "pool",
        hide = true,
        value_name = "VALUE",
        help = "heal only the given pool"
    )]
    pub pool: Option<i64>,
    #[arg(
        long = "set",
        hide = true,
        value_name = "VALUE",
        help = "heal only the given set"
    )]
    pub set: Option<i64>,
    #[arg(
        long = "scan",
        hide = true,
        default_value = "normal",
        value_name = "VALUE",
        help = "select the healing scan mode (normal/deep)"
    )]
    pub scan: String,
    #[arg(
        long = "recursive",
        short = 'r',
        hide = true,
        help = "heal recursively"
    )]
    pub recursive: bool,
    #[arg(
        long = "dry-run",
        short = 'n',
        hide = true,
        help = "only inspect data, but do not mutate"
    )]
    pub dry_run: bool,
    #[arg(
        long = "force-start",
        short = 'f',
        hide = true,
        help = "force start a new heal sequence"
    )]
    pub force_start: bool,
    #[arg(
        long = "force-stop",
        short = 's',
        hide = true,
        help = "force stop a running heal sequence"
    )]
    pub force_stop: bool,
    #[arg(
        long = "remove",
        hide = true,
        help = "remove dangling objects in heal sequence"
    )]
    pub remove: bool,
    #[arg(
        long = "storage-class",
        hide = true,
        value_name = "VALUE",
        help = "show server/drives failure tolerance with the given storage class"
    )]
    pub storage_class: Option<String>,
    #[arg(
        long = "rewrite",
        hide = true,
        help = "rewrite objects from older to newer format"
    )]
    pub rewrite: bool,
}

pub fn run(args: HealArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin heal")
}
