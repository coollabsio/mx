use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::error::{McError, nonfatal};
use crate::target::TargetRef;
use anyhow::{Context, Result, bail};
use clap::Args;
use serde::Serialize;

#[derive(Debug, Args)]
pub struct MakeBucketArgs {
    /// specify bucket region; defaults to 'us-east-1'
    #[arg(long, default_value = "us-east-1")]
    pub region: Option<String>,
    /// ignore if bucket/directory already exists
    #[arg(short = 'p', long)]
    pub ignore_existing: bool,
    /// enable object lock
    #[arg(short = 'l', long)]
    pub with_lock: bool,
    /// enable versioned bucket
    #[arg(long)]
    pub with_versioning: bool,
    pub target: String,
}

pub fn run(args: MakeBucketArgs, json: bool) -> Result<()> {
    let input = &args.target;
    let target = TargetRef::parse(input)?;
    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &target.alias)?;
    let Some(bucket) = target.bucket.clone().filter(|bucket| !bucket.is_empty()) else {
        let example = format!("{}/your-bucket-name", input.trim_end_matches('/'));
        return Err(
            anyhow::Error::new(McError::bucket_name_empty()).context(nonfatal(format!(
                "Unable to make bucket, please use `mc mb {example}`."
            ))),
        );
    };
    let context = || nonfatal(format!("Unable to make bucket `{input}`."));
    crate::s3::error::check_bucket_name_strict(&bucket).context(context())?;
    if !target.is_bucket_root() {
        bail!("`mb` requires a bucket target like `alias/bucket`.");
    }

    runtime()?
        .block_on(crate::s3::make_bucket_with(
            &alias,
            &bucket,
            &crate::s3::MakeBucketOptions {
                ignore_existing: args.ignore_existing,
                with_versioning: args.with_versioning,
                with_lock: args.with_lock,
                region: args.region.clone(),
            },
        ))
        .with_context(context)?;

    if json {
        crate::output::print_json(&BucketMessage {
            status: "success",
            target: &args.target,
            bucket: &bucket,
        })?;
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
