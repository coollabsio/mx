use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::output;
use crate::target::TargetRef;
use anyhow::{Result, bail};
use clap::Args;
use serde::Serialize;

#[derive(Debug, Args)]
pub struct RemoveBucketArgs {
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub dangerous: bool,
    pub target: String,
}

pub fn run(args: RemoveBucketArgs, json: bool) -> Result<()> {
    let target = TargetRef::parse(&args.target)?;
    if !target.is_bucket_root() {
        bail!("`rb` requires a bucket target like `alias/bucket`.");
    }

    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &target.alias)?;
    let bucket = target.require_bucket()?.to_string();

    runtime()?.block_on(crate::s3::remove_bucket(&alias, &bucket, args.force))?;

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
        output::print_plain(&format!("Bucket `{bucket}` removed successfully."));
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct BucketMessage<'a> {
    status: &'static str,
    target: &'a str,
    bucket: &'a str,
}
