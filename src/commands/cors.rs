//! `cors set|get|remove` (mc cors). `set` accepts mc's `CORSConfiguration` XML (file or `-`
//! for stdin) and, for compatibility with earlier mx releases, AWS CLI style CORS JSON.

use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::flags::TargetArg;
use crate::output;
use crate::s3::CorsDocument;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::io::Read;

#[derive(Debug, Args)]
pub struct CorsArgs {
    #[command(subcommand)]
    pub command: CorsCommand,
}

#[derive(Debug, Subcommand)]
pub enum CorsCommand {
    #[command(about = "set a bucket CORS configuration (XML or JSON file, `-` for stdin)")]
    Set(CorsSetArgs),
    #[command(about = "get a bucket CORS configuration")]
    Get(TargetArg),
    #[command(about = "remove a bucket CORS configuration")]
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

/// Converts the CORS file contents to `CORSConfiguration` XML (validating it).
pub fn cors_xml(contents: &str) -> Result<String> {
    let trimmed = contents.trim_start();
    if trimmed.starts_with('<') {
        CorsDocument::from_xml(trimmed).context("Unable to parse bucket CORS configuration")?;
        Ok(contents.to_string())
    } else {
        let document: CorsDocument = serde_json::from_str(contents)
            .context("Unable to parse bucket CORS configuration (expected XML or JSON)")?;
        Ok(document.to_xml())
    }
}

fn print_status(message: &str, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::json!({"status": "success"}));
    } else {
        output::print_plain(message);
    }
    Ok(())
}

fn set(args: CorsSetArgs, json: bool) -> Result<()> {
    let mut contents = String::new();
    if args.file == "-" {
        std::io::stdin().read_to_string(&mut contents)?;
    } else {
        contents = std::fs::read_to_string(&args.file).with_context(|| {
            format!(
                "Unable to open bucket CORS configuration file `{}`.",
                args.file
            )
        })?;
    }
    let xml = cors_xml(&contents)?;
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?
        .block_on(crate::s3::put_cors_xml(&alias, &bucket, &xml))
        .with_context(|| {
            format!(
                "Unable to set bucket CORS configuration for {}",
                args.target
            )
        })?;
    print_status("Set bucket CORS config successfully.", json)
}

fn get(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let found = runtime()?
        .block_on(crate::s3::get_cors_xml(&alias, &bucket))
        .with_context(|| {
            format!(
                "Unable to get bucket CORS configuration for {}",
                args.target
            )
        })?;
    match (found, json) {
        (Some((_, document)), true) => println!(
            "{}",
            serde_json::json!({"status": "success", "cors": document.to_mc_json()})
        ),
        (Some((xml, _)), false) => println!("{}", xml.trim()),
        (None, true) => println!("{}", serde_json::json!({"status": "not found"})),
        (None, false) => println!("No bucket CORS configuration found."),
    }
    Ok(())
}

fn remove(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?
        .block_on(crate::s3::delete_cors(&alias, &bucket))
        .with_context(|| {
            format!(
                "Unable to remove bucket CORS configuration for {}",
                args.target
            )
        })?;
    print_status("Removed bucket CORS config successfully.", json)
}

#[cfg(test)]
mod tests {
    use super::cors_xml;

    #[test]
    fn accepts_xml_and_json() {
        let xml = "<CORSConfiguration><CORSRule><AllowedMethod>GET</AllowedMethod><AllowedOrigin>*</AllowedOrigin></CORSRule></CORSConfiguration>";
        assert_eq!(cors_xml(xml).unwrap(), xml);
        let converted = cors_xml(
            r#"{"CORSRules":[{"AllowedOrigins":["*"],"AllowedMethods":["PUT"],"MaxAgeSeconds":10}]}"#,
        )
        .unwrap();
        assert!(converted.contains("<AllowedMethod>PUT</AllowedMethod>"));
        assert!(converted.contains("<MaxAgeSeconds>10</MaxAgeSeconds>"));
        assert!(cors_xml("nonsense").is_err());
        assert!(cors_xml("<Other/>").is_err());
    }
}
