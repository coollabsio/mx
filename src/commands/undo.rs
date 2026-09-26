//! `mx undo` (area G). Stub until implemented.

use anyhow::{Result, bail};
use clap::Args;

#[derive(Debug, Args)]
pub struct UndoArgs {
    pub target: String,
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[arg(long)]
    pub force: bool,
    /// undo N last changes
    #[arg(long, default_value_t = 1)]
    pub last: usize,
    /// undo only if the latest version is of the following type: PUT or DELETE
    #[arg(long)]
    pub action: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(_args: UndoArgs, _json: bool) -> Result<()> {
    bail!("`undo` is not implemented yet.")
}
