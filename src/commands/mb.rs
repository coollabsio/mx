use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::target::TargetRef;
use anyhow::{Result, bail};
use clap::Args;
use serde::Serialize;

#[derive(Debug, Args)]
pub struct MakeBucketArgs {
    #[arg(short = 'p', long)]
    pub ignore_existing: bool,
    /// enable object versioning on the new bucket
    #[arg(long)]
    pub with_versioning: bool,
    /// enable object lock (implies versioning)
    #[arg(short = 'l', long)]
    pub with_lock: bool,
    /// bucket region
    #[arg(long)]
    pub region: Option<String>,
    pub target: String,
}

pub fn run(args: MakeBucketArgs, json: bool) -> Result<()> {
    let target = TargetRef::parse(&args.target)?;
    if !target.is_bucket_root() {
        bail!("`mb` requires a bucket target like `alias/bucket`.");
    }

    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &target.alias)?;
    let bucket = target.require_bucket()?.to_string();

    runtime()?.block_on(crate::s3::make_bucket_with(
        &alias,
        &bucket,
        &crate::s3::MakeBucketOptions {
            ignore_existing: args.ignore_existing,
            with_versioning: args.with_versioning,
            with_lock: args.with_lock,
            region: args.region.clone(),
        },
    ))?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&BucketMessage {
                status: "success",
                target: &args.target,
                bucket: &bucket,
            })?
        );
    } else {
        println!("Bucket `{bucket}` created successfully.");
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct BucketMessage<'a> {
    status: &'static str,
    target: &'a str,
    bucket: &'a str,
}
