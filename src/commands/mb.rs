use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::error::{McError, nonfatal};
use crate::target::TargetRef;
use anyhow::{Context, Result};
use aws_sdk_s3::primitives::ByteStream;
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
    #[arg(required = true, value_name = "TARGET")]
    pub targets: Vec<String>,
}

pub fn run(args: MakeBucketArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let rt = runtime()?;
    let mut failed = false;
    for input in &args.targets {
        match make(&store, &rt, &args, input) {
            Ok(()) => {
                let message = BucketMessage {
                    status: "success",
                    bucket: input,
                    // mc never fills in the region.
                    region: "",
                };
                if json {
                    crate::output::print_json(&message)?;
                } else {
                    println!("Bucket created successfully `{input}`.");
                }
            }
            Err(error) => {
                crate::output::print_error(&error);
                failed = true;
            }
        }
    }
    if failed {
        return Err(crate::output::Exit(1).into());
    }
    Ok(())
}

fn make(
    store: &ConfigStore,
    rt: &tokio::runtime::Runtime,
    args: &MakeBucketArgs,
    input: &str,
) -> Result<()> {
    let target = TargetRef::parse(input)?;
    let alias = alias_config(store, &target.alias)?;
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
    let options = crate::s3::MakeBucketOptions {
        ignore_existing: args.ignore_existing,
        with_versioning: args.with_versioning,
        with_lock: args.with_lock,
        region: args.region.clone(),
    };
    rt.block_on(async {
        match target.key_with_trailing_slash() {
            // mc creates a folder marker for `ALIAS/BUCKET/DIR`, creating the bucket if needed.
            Some(key) => make_folder(&alias, &bucket, &key, &options).await,
            None => crate::s3::make_bucket_with(&alias, &bucket, &options).await,
        }
    })
    .with_context(context)
}

async fn make_folder(
    alias: &crate::config::model::AliasConfig,
    bucket: &str,
    key: &str,
    options: &crate::s3::MakeBucketOptions,
) -> Result<()> {
    let key = if key.ends_with('/') {
        key.to_string()
    } else {
        format!("{key}/")
    };
    let client = crate::s3::build_client(alias).await?;
    let put = || async {
        client
            .put_object()
            .bucket(bucket)
            .key(&key)
            .body(ByteStream::from_static(b""))
            // Required by object-lock buckets; the MD5 of an empty body.
            .content_md5("1B2M2Y8AsgTpgAmY7PhCfg==")
            .send()
            .await
    };
    match put().await {
        Ok(_) => Ok(()),
        Err(error) if crate::s3::error_code(&error) == Some("NoSuchBucket") => {
            let create = crate::s3::MakeBucketOptions {
                with_versioning: false,
                ..options.clone()
            };
            crate::s3::make_bucket_with(alias, bucket, &create).await?;
            put()
                .await
                .map_err(|error| crate::s3::s3_error(&error, bucket, &key))?;
            Ok(())
        }
        Err(error) => Err(crate::s3::s3_error(&error, bucket, &key)),
    }
}

/// mc `makeBucketMessage`.
#[derive(Debug, Serialize)]
struct BucketMessage<'a> {
    status: &'static str,
    bucket: &'a str,
    region: &'a str,
}
