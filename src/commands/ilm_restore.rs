//! `mx ilm restore` (area G). Stub until implemented.
//! Wired from `ilm.rs` as `IlmCommand::Restore`; area G edits only this file.

use crate::flags::{VersionIdFlag, VersionsFlag};
use anyhow::{Result, bail};
use clap::Args;

#[derive(Debug, Args)]
pub struct IlmRestoreArgs {
    pub target: String,
    /// keep restored copies for N days
    #[arg(long, default_value_t = 1)]
    pub days: i32,
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub version_id: VersionIdFlag,
    #[command(flatten)]
    pub versions: VersionsFlag,
}

pub fn run(_args: IlmRestoreArgs, _json: bool) -> Result<()> {
    bail!("`ilm restore` is not implemented yet.")
}
