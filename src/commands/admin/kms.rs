//! `mx admin kms key create|status|list` (mc `admin kms key`), via `/minio/kms/v1/...`.
//!
//! Owner: SERVER.

use super::info::json_4;
use crate::commands::runtime;
use crate::s3::admin_server as api;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct KmsArgs {
    #[command(subcommand)]
    pub command: KmsCommand,
}

#[derive(Debug, Subcommand)]
pub enum KmsCommand {
    #[command(
        name = "key",
        about = "manage KMS master keys: Request key status information"
    )]
    Key(KmsKeyArgs),
}

#[derive(Debug, Args)]
pub struct KmsKeyArgs {
    #[command(subcommand)]
    pub command: KmsKeyCommand,
}

#[derive(Debug, Subcommand)]
pub enum KmsKeyCommand {
    #[command(name = "create", about = "creates a new master KMS key")]
    Create(KmsKeyCreateArgs),
    #[command(
        name = "status",
        about = "request status information for a KMS master key"
    )]
    Status(KmsKeyStatusArgs),
    #[command(name = "list", about = "request list of KMS master keys")]
    List(KmsKeyListArgs),
}

#[derive(Debug, Args)]
pub struct KmsKeyCreateArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "KEY-NAME")]
    pub key_name: String,
}

#[derive(Debug, Args)]
pub struct KmsKeyStatusArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "KEY-NAME")]
    pub key_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct KmsKeyListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

pub fn run(args: KmsArgs, json: bool) -> Result<()> {
    match args.command {
        KmsCommand::Key(args) => match args.command {
            KmsKeyCommand::Create(args) => key_create(args),
            KmsKeyCommand::Status(args) => key_status(args, json),
            KmsKeyCommand::List(args) => key_list(args, json),
        },
    }
}

/// mc prints the confirmation only when stdout is a terminal (also with `--json`).
fn key_create(args: KmsKeyCreateArgs) -> Result<()> {
    let client = api::admin_client(&args.target, "Cannot get a configured admin connection.")?;
    runtime()?
        .block_on(api::kms_create_key(&client, &args.key_name))
        .context("Failed to create master key")?;
    if crate::output::stdout_is_terminal() {
        println!("Created master key `{}` successfully", args.key_name);
    }
    Ok(())
}

/// mc `kmsKeyStatusMsg`.
#[derive(Serialize)]
struct KeyStatusMessage<'a> {
    #[serde(rename = "keyId")]
    key_id: &'a str,
    #[serde(rename = "encryptionError", skip_serializing_if = "str::is_empty")]
    encryption_err: &'a str,
    #[serde(rename = "decryptionError", skip_serializing_if = "str::is_empty")]
    decryption_err: &'a str,
    status: &'static str,
}

fn render_status(status: &api::KmsKeyStatus) -> String {
    let line = |name: &str, unknown: bool, err: &str| {
        let mark = if unknown {
            "?".to_string()
        } else if err.is_empty() {
            "✔".to_string()
        } else {
            format!("✗ ({err})")
        };
        format!("   - {name} {mark}\n")
    };
    format!(
        "Key: {}\n{}{}",
        status.key_id,
        line("Encryption", false, &status.encryption_err),
        line(
            "Decryption",
            !status.encryption_err.is_empty(),
            &status.decryption_err
        )
    )
}

fn key_status(args: KmsKeyStatusArgs, json: bool) -> Result<()> {
    let client = api::admin_client(&args.target, "Unable to get a configured admin connection.")?;
    let key = args.key_name.as_deref().unwrap_or_default();
    let status = runtime()?
        .block_on(api::kms_key_status(&client, key))
        .context("Failed to get status information")?;
    if json {
        let message = KeyStatusMessage {
            key_id: &status.key_id,
            encryption_err: &status.encryption_err,
            decryption_err: &status.decryption_err,
            status: "success",
        };
        println!("{}", json_4(&message)?);
        return Ok(());
    }
    let text = render_status(&status);
    println!("{}", text.strip_suffix('\n').unwrap_or(&text));
    Ok(())
}

/// mc `kmsKeysMsg`.
#[derive(Serialize)]
struct KeysMessage<'a> {
    status: &'static str,
    target: &'a str,
    keys: Vec<&'a str>,
}

fn key_list(args: KmsKeyListArgs, json: bool) -> Result<()> {
    let client = api::admin_client(&args.target, "Unable to initialize admin connection.")?;
    let keys = runtime()?
        .block_on(api::kms_list_keys(&client, "*"))
        .context("Unable to list KMS keys")?;
    if json {
        let message = KeysMessage {
            status: "success",
            target: &args.target,
            keys: keys.iter().map(|k| k.name.as_str()).collect(),
        };
        println!("{}", json_4(&message)?);
        return Ok(());
    }
    print!("{}", render_list(&keys));
    Ok(())
}

/// go-pretty `StyleLight` table titled `KMS Keys` (serial numbers right-aligned).
fn render_list(keys: &[api::KmsKeyInfo]) -> String {
    let rows: Vec<Vec<String>> = keys
        .iter()
        .enumerate()
        .map(|(index, key)| vec![(index + 1).to_string(), key.name.clone()])
        .collect();
    crate::commands::ilm::render_table(Some("KMS Keys"), &["S N", "Name"], &rows, &[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_key_list_like_mc() {
        let keys = vec![api::KmsKeyInfo {
            name: "mx-test-key".into(),
            ..Default::default()
        }];
        assert_eq!(
            render_list(&keys),
            "┌───────────────────┐\n│ KMS Keys          │\n├─────┬─────────────┤\n│ S N │ NAME        │\n├─────┼─────────────┤\n│   1 │ mx-test-key │\n└─────┴─────────────┘\n"
        );
    }

    #[test]
    fn renders_key_status_like_mc() {
        let status = api::KmsKeyStatus {
            key_id: "nokey".into(),
            encryption_err: "key with given key ID does not exist".into(),
            decryption_err: String::new(),
        };
        assert_eq!(
            render_status(&status),
            "Key: nokey\n   - Encryption ✗ (key with given key ID does not exist)\n   - Decryption ?\n"
        );
    }
}
