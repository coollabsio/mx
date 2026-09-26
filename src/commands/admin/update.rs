//! `mx admin update` (mc `admin update`).
//!
//! Owner: SERVER. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct UpdateArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(long = "yes", short = 'y', help = "Confirms the server update")]
    pub yes: bool,
}

pub fn run(args: UpdateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin update")
}
