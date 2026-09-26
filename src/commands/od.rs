//! `mx od if=... of=... size=... parts=...` (area G). Stub until implemented.

use anyhow::{Result, bail};
use clap::Args;

#[derive(Debug, Args)]
pub struct OdArgs {
    /// operands: if=SOURCE of=TARGET size=SIZE parts=N skip=N
    #[arg(value_name = "KEY=VALUE")]
    pub operands: Vec<String>,
}

pub fn run(_args: OdArgs, _json: bool) -> Result<()> {
    bail!("`od` is not implemented yet.")
}
