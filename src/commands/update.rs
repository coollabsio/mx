//! `mx update` (mc `update`).
//!
//! Owner: SERVER. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct UpdateArgs {}

pub fn run(args: UpdateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("update")
}
