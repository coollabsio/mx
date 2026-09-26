//! `mx admin info` (mc `admin info`).
//!
//! Owner: SERVER. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct InfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(long = "offline", help = "show only offline nodes/drives")]
    pub offline: bool,
}

pub fn run(args: InfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin info")
}
