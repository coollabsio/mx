use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::flags::TargetArg;
use crate::output;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct CorsArgs {
    #[command(subcommand)]
    pub command: CorsCommand,
}

#[derive(Debug, Subcommand)]
pub enum CorsCommand {
    #[command(about = "set bucket CORS from a JSON file")]
    Set(CorsSetArgs),
    #[command(about = "show bucket CORS")]
    Get(TargetArg),
    #[command(about = "remove bucket CORS")]
    Remove(TargetArg),
}

#[derive(Debug, Args)]
pub struct CorsSetArgs {
    pub target: String,
    pub file: String,
}

pub fn run(command: CorsCommand, json: bool) -> Result<()> {
    match command {
        CorsCommand::Set(args) => set(args, json),
        CorsCommand::Get(args) => get(args, json),
        CorsCommand::Remove(args) => remove(args, json),
    }
}

fn set(args: CorsSetArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let document = serde_json::from_str(&std::fs::read_to_string(&args.file)?)?;
    runtime()?.block_on(crate::s3::put_cors(&alias, &bucket, document))?;
    if json {
        println!(r#"{{"status":"success"}}"#);
    } else {
        output::print_plain(&format!("CORS set on `{}`.", args.target));
    }
    Ok(())
}

fn get(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let document = runtime()?.block_on(crate::s3::get_cors(&alias, &bucket))?;
    println!("{}", serde_json::to_string_pretty(&document)?);
    let _ = json;
    Ok(())
}

fn remove(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?.block_on(crate::s3::delete_cors(&alias, &bucket))?;
    if json {
        println!(r#"{{"status":"success"}}"#);
    } else {
        output::print_plain(&format!("CORS removed from `{}`.", args.target));
    }
    Ok(())
}
