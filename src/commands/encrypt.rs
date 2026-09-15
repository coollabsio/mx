use crate::cli::{EncryptCommand, EncryptSetArgs, TargetArg};
use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::output;
use anyhow::{Result, bail};

pub fn run(command: EncryptCommand, json: bool) -> Result<()> {
    match command {
        EncryptCommand::Set(args) => set(args, json),
        EncryptCommand::Info(args) => info(args, json),
        EncryptCommand::Clear(args) => clear(args, json),
    }
}

fn set(args: EncryptSetArgs, json: bool) -> Result<()> {
    if !args.algorithm.eq_ignore_ascii_case("sse-s3")
        && !args.algorithm.eq_ignore_ascii_case("AES256")
    {
        bail!("only `sse-s3` encryption is supported");
    }
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?.block_on(crate::s3::put_encryption_s3(&alias, &bucket))?;
    if json {
        println!(r#"{{"status":"success","algorithm":"AES256"}}"#);
    } else {
        output::print_plain("Default encryption set to SSE-S3.");
    }
    Ok(())
}

fn info(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let algorithm = runtime()?.block_on(crate::s3::get_encryption(&alias, &bucket))?;
    if json {
        println!(r#"{{"status":"success","algorithm":"{algorithm}"}}"#);
    } else {
        println!("{algorithm}");
    }
    Ok(())
}

fn clear(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?.block_on(crate::s3::delete_encryption(&alias, &bucket))?;
    if json {
        println!(r#"{{"status":"success"}}"#);
    } else {
        output::print_plain("Default encryption cleared.");
    }
    Ok(())
}
