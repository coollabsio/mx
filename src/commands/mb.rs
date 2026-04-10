use crate::cli::BucketTargetArgs;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::target::TargetRef;
use anyhow::{Result, bail};
use serde::Serialize;

pub fn run(args: BucketTargetArgs, json: bool) -> Result<()> {
    let target = TargetRef::parse(&args.target)?;
    if !target.is_bucket_root() {
        bail!("`mb` requires a bucket target like `alias/bucket`.");
    }

    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &target.alias)?;
    let bucket = target.require_bucket()?.to_string();

    runtime()?.block_on(crate::s3::make_bucket(&alias, &bucket))?;

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
