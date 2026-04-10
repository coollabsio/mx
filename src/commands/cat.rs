use crate::cli::TargetArg;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::target::TargetRef;
use anyhow::{Result, bail};
use std::io::{self, Write};

pub fn run(args: TargetArg, json: bool) -> Result<()> {
    if json {
        bail!("`cat` does not support `--json` yet.");
    }

    let target = TargetRef::parse(&args.target)?;
    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &target.alias)?;
    let bucket = target.require_bucket()?.to_string();
    let key = target.require_object_key()?;

    let bytes = runtime()?.block_on(crate::s3::get_object_bytes(&alias, &bucket, &key))?;
    io::stdout().write_all(&bytes)?;
    Ok(())
}
