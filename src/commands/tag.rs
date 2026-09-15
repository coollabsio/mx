use crate::cli::{TagCommand, TagSetArgs, TargetArg};
use crate::commands::runtime;
use crate::commands::util::{parse_tags, require_s3};
use crate::config::ConfigStore;
use crate::output;
use anyhow::Result;

pub fn run(command: TagCommand, json: bool) -> Result<()> {
    match command {
        TagCommand::Set(args) => set(args, json),
        TagCommand::List(args) => list(args, json),
        TagCommand::Remove(args) => remove(args, json),
    }
}

fn set(args: TagSetArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let key = target.key.clone();
    let tags = parse_tags(&args.tags)?;
    runtime()?.block_on(crate::s3::put_object_tags(
        &alias,
        &bucket,
        key.as_deref(),
        tags,
    ))?;
    if json {
        println!(r#"{{"status":"success"}}"#);
    } else {
        output::print_plain(&format!("Tags set for `{}`.", args.target));
    }
    Ok(())
}

fn list(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let tags = runtime()?.block_on(crate::s3::get_object_tags(
        &alias,
        &bucket,
        target.key.as_deref(),
    ))?;
    if json {
        println!("{}", serde_json::to_string(&tags)?);
    } else if tags.is_empty() {
        println!("No tags");
    } else {
        for (key, value) in tags {
            println!("{key}={value}");
        }
    }
    Ok(())
}

fn remove(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?.block_on(crate::s3::delete_object_tags(
        &alias,
        &bucket,
        target.key.as_deref(),
    ))?;
    if json {
        println!(r#"{{"status":"success"}}"#);
    } else {
        output::print_plain(&format!("Tags removed from `{}`.", args.target));
    }
    Ok(())
}
