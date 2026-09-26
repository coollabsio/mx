//! `encrypt set|info|clear` (mc encrypt): default bucket encryption (SSE-S3 / SSE-KMS).

use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::flags::TargetArg;
use crate::output;
use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct EncryptArgs {
    #[command(subcommand)]
    pub command: EncryptCommand,
}

#[derive(Debug, Subcommand)]
pub enum EncryptCommand {
    #[command(about = "set encryption config: `sse-s3 TARGET` or `sse-kms KEY_ID TARGET`")]
    Set(EncryptSetArgs),
    #[command(about = "clear encryption config")]
    Clear(TargetArg),
    #[command(about = "show bucket encryption status")]
    Info(TargetArg),
}

#[derive(Debug, Args)]
pub struct EncryptSetArgs {
    /// ALGORITHM [KMS_KEY_ID] TARGET
    #[arg(num_args = 2..=3, required = true, value_names = ["ALGORITHM", "TARGET"])]
    pub args: Vec<String>,
}

pub fn run(command: EncryptCommand, json: bool) -> Result<()> {
    match command {
        EncryptCommand::Set(args) => set(args, json),
        EncryptCommand::Info(args) => info(args, json),
        EncryptCommand::Clear(args) => clear(args, json),
    }
}

#[derive(Debug, Default, Serialize)]
struct Encryption {
    #[serde(skip_serializing_if = "String::is_empty")]
    algorithm: String,
    #[serde(rename = "keyId", skip_serializing_if = "String::is_empty")]
    key_id: String,
}

#[derive(Serialize)]
struct EncryptMessage<'a> {
    op: &'a str,
    status: &'a str,
    url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    encryption: Option<Encryption>,
}

fn print_json(op: &str, url: &str, encryption: Option<Encryption>) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&EncryptMessage {
            op,
            status: "success",
            url,
            encryption,
        })?
    );
    Ok(())
}

/// Parses `ALGORITHM [KEY] TARGET` into (algorithm, kms key, target).
pub fn parse_set_args(args: &[String]) -> Result<(String, Option<String>, String)> {
    let (algorithm, key, target) = match args {
        [algorithm, target] => (algorithm, None, target),
        [algorithm, key, target] => (algorithm, Some(key.clone()), target),
        _ => bail!("usage: mx encrypt set sse-s3 TARGET | mx encrypt set sse-kms KEY_ID TARGET"),
    };
    let algorithm = match algorithm.to_ascii_lowercase().as_str() {
        "sse-s3" | "aes256" => "sse-s3",
        "sse-kms" | "aws:kms" => "sse-kms",
        _ => bail!("Invalid encryption algorithm: Unknown argument `{algorithm}` passed"),
    };
    if algorithm == "sse-s3" && key.is_some() {
        bail!("sse-s3 does not take a KMS key id");
    }
    Ok((algorithm.to_string(), key, target.clone()))
}

fn set(args: EncryptSetArgs, json: bool) -> Result<()> {
    let (algorithm, key, target_arg) = parse_set_args(&args.args)?;
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &target_arg)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?.block_on(crate::s3::put_encryption(&alias, &bucket, key.as_deref()))?;
    if json {
        return print_json(
            "set",
            &target_arg,
            Some(Encryption {
                algorithm,
                key_id: String::new(),
            }),
        );
    }
    output::print_plain(&format!(
        "Auto encryption configuration has been set successfully for {target_arg}"
    ));
    Ok(())
}

fn info(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let config = runtime()?.block_on(crate::s3::get_encryption_config(&alias, &bucket))?;
    let (algorithm, key_id) = config
        .map(|(algorithm, key)| (algorithm, key.unwrap_or_default()))
        .unwrap_or_default();
    if json {
        return print_json("info", &args.target, Some(Encryption { algorithm, key_id }));
    }
    if !key_id.is_empty() {
        println!("Auto encryption 'sse-kms' is enabled with KeyID: {key_id}");
    } else if !algorithm.is_empty() {
        println!("Auto encryption 'sse-s3' is enabled");
    } else {
        println!("Auto encryption is not enabled for {} ", args.target);
    }
    Ok(())
}

fn clear(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?.block_on(crate::s3::delete_encryption(&alias, &bucket))?;
    if json {
        return print_json("clear", &args.target, None);
    }
    output::print_plain(&format!(
        "Auto encryption configuration has been cleared successfully for {}",
        args.target
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_set_args;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn parses_set_arguments() {
        assert_eq!(
            parse_set_args(&args(&["SSE-S3", "a/b"])).unwrap(),
            ("sse-s3".into(), None, "a/b".into())
        );
        assert_eq!(
            parse_set_args(&args(&["sse-kms", "key", "a/b"])).unwrap(),
            ("sse-kms".into(), Some("key".into()), "a/b".into())
        );
        assert!(parse_set_args(&args(&["sse-c", "a/b"])).is_err());
        assert!(parse_set_args(&args(&["sse-s3", "key", "a/b"])).is_err());
    }
}
