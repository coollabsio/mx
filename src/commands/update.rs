//! `mx update` (mc `update`): mc replaces its own binary with the latest release. mx does not
//! self-update; it reports that like mc reports a failed update (`errorIf` + exit status 255).
//!
//! Owner: SERVER.

use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct UpdateArgs {}

pub fn run(args: UpdateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    let prog = crate::output::prog_name();
    crate::output::error_if(
        &format!("Unable to update ‘{prog}’."),
        "self-update is not supported; install a newer release manually",
    );
    Err(crate::output::Exit(255).into())
}
