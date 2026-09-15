use crate::cli::HeadArgs;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::location::{Location, parse_location};
use anyhow::{Result, bail};
use std::io::{self, BufRead, Write};

pub fn run(args: HeadArgs, json: bool) -> Result<()> {
    if json {
        bail!("`head` does not support `--json` yet.");
    }
    let store = ConfigStore::load_or_create()?;
    let bytes = match parse_location(&args.target, store.config()) {
        Location::S3(target) => {
            let alias = alias_config(&store, &target.alias)?;
            let bucket = target.require_bucket()?.to_string();
            let key = target.require_object_key()?;
            runtime()?.block_on(crate::s3::get_object_bytes(&alias, &bucket, &key))?
        }
        Location::Local(path) => std::fs::read(path)?,
    };
    write_lines(&bytes, args.lines)
}

fn write_lines(bytes: &[u8], lines: usize) -> Result<()> {
    let mut remaining = lines;
    let mut stdout = io::stdout().lock();
    for line in bytes.lines() {
        if remaining == 0 {
            break;
        }
        writeln!(stdout, "{}", line?)?;
        remaining -= 1;
    }
    Ok(())
}
