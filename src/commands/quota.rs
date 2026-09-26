//! `mx quota set|info|clear` (MinIO admin API `set-bucket-quota` / `get-bucket-quota`).

use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::output;
use crate::s3::admin::{self, AdminClient, BucketQuota};
use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct QuotaArgs {
    #[command(subcommand)]
    pub command: QuotaCommand,
}

#[derive(Debug, Subcommand)]
pub enum QuotaCommand {
    #[command(about = "set bucket quota")]
    Set(QuotaSetArgs),
    #[command(about = "show bucket quota")]
    Info(QuotaTargetArgs),
    #[command(about = "clear bucket quota")]
    Clear(QuotaTargetArgs),
}

#[derive(Debug, Args)]
pub struct QuotaSetArgs {
    pub target: String,
    /// set a hard quota, disallowing writes after quota is reached
    #[arg(long)]
    pub size: Option<String>,
}

#[derive(Debug, Args)]
pub struct QuotaTargetArgs {
    pub target: String,
}

#[derive(Debug, Serialize)]
struct QuotaMessage<'a> {
    status: &'static str,
    bucket: &'a str,
    #[serde(skip_serializing_if = "is_zero")]
    quota: u64,
    #[serde(rename = "type", skip_serializing_if = "str::is_empty")]
    quota_type: &'a str,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

pub fn run(args: QuotaArgs, json: bool) -> Result<()> {
    match args.command {
        QuotaCommand::Set(args) => set(args, json),
        QuotaCommand::Info(args) => info(args, json),
        QuotaCommand::Clear(args) => clear(args, json),
    }
}

fn client_and_bucket(target: &str) -> Result<(AdminClient, String)> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target_ref) = require_s3(&store, target)?;
    if !target_ref.is_bucket_root() {
        bail!("`quota` requires a bucket target like `alias/bucket`.");
    }
    let bucket = target_ref.require_bucket()?.to_string();
    Ok((AdminClient::new(&alias)?, bucket))
}

fn print(message: &QuotaMessage, text: String, json: bool) -> Result<()> {
    if json {
        crate::output::print_json(message)?;
    } else {
        output::print_plain(&text);
    }
    Ok(())
}

fn set(args: QuotaSetArgs, json: bool) -> Result<()> {
    let Some(size) = args.size.as_deref() else {
        bail!("--size flag needs to be set.");
    };
    let quota = crate::flags::parse_size(size).context("Unable to parse quota")?;
    let (client, bucket) = client_and_bucket(&args.target)?;
    let config = BucketQuota {
        quota,
        size: quota,
        quota_type: "hard".to_string(),
        ..Default::default()
    };
    runtime()?
        .block_on(admin::set_bucket_quota(&client, &bucket, &config))
        .context("Unable to set bucket quota")?;
    let message = QuotaMessage {
        status: "success",
        bucket: &bucket,
        quota,
        quota_type: "hard",
    };
    let text = format!(
        "Successfully set bucket quota of {} on `{bucket}`",
        admin::ibytes(quota)
    );
    print(&message, text, json)
}

fn info(args: QuotaTargetArgs, json: bool) -> Result<()> {
    let (client, bucket) = client_and_bucket(&args.target)?;
    let config = runtime()?
        .block_on(admin::get_bucket_quota(&client, &bucket))
        .context("Unable to get bucket quota")?;
    let quota = if config.size > 0 {
        config.size
    } else {
        config.quota
    };
    let message = QuotaMessage {
        status: "success",
        bucket: &bucket,
        quota,
        quota_type: &config.quota_type,
    };
    let text = format!(
        "Bucket `{bucket}` has {} quota of {}",
        config.quota_type,
        admin::ibytes(quota)
    );
    print(&message, text, json)
}

fn clear(args: QuotaTargetArgs, json: bool) -> Result<()> {
    let (client, bucket) = client_and_bucket(&args.target)?;
    runtime()?
        .block_on(admin::set_bucket_quota(
            &client,
            &bucket,
            &BucketQuota::default(),
        ))
        .context("Unable to clear bucket quota config")?;
    let message = QuotaMessage {
        status: "success",
        bucket: &bucket,
        quota: 0,
        quota_type: "",
    };
    let text = format!("Successfully cleared bucket quota configured on `{bucket}`");
    print(&message, text, json)
}
