use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::error::nonfatal;
use crate::s3::S3ResultExt;
use crate::target::TargetRef;
use anyhow::{Context, Result, bail};
use clap::Args;
use serde::Serialize;

#[derive(Debug, Args)]
pub struct RemoveBucketArgs {
    /// force a recursive remove operation on all object versions
    #[arg(long)]
    pub force: bool,
    /// allow site-wide removal of objects
    #[arg(long)]
    pub dangerous: bool,
    #[arg(required = true, value_name = "TARGET")]
    pub targets: Vec<String>,
}

/// Validates every target before anything is removed.
pub fn validate(args: &RemoveBucketArgs) -> Result<Vec<TargetRef>> {
    let mut targets = Vec::new();
    for input in &args.targets {
        let target = TargetRef::parse(input)?;
        if target.is_alias_root() {
            if !(args.force && args.dangerous) {
                bail!(
                    "This operation results in **site-wide** removal of buckets. If you are really sure, retry this command with ‘--force’ and ‘--dangerous’ flags."
                );
            }
        } else if !target.is_bucket_root() {
            bail!("`rb` requires a bucket target like `alias/bucket`.");
        }
        targets.push(target);
    }
    Ok(targets)
}

pub fn run(args: RemoveBucketArgs, json: bool) -> Result<()> {
    let targets = validate(&args)?;
    let store = ConfigStore::load_or_create()?;
    let rt = runtime()?;
    let mut failed = false;
    for (input, target) in args.targets.iter().zip(&targets) {
        let alias = alias_config(&store, &target.alias)?;
        let client = rt.block_on(crate::s3::build_client(&alias))?;
        let buckets = match &target.bucket {
            Some(bucket) => vec![bucket.clone()],
            None => rt
                .block_on(client.list_buckets().send())
                .s3("", "")?
                .buckets()
                .iter()
                .filter_map(|bucket| bucket.name().map(str::to_string))
                .collect(),
        };
        for bucket in buckets {
            let removed = rt.block_on(async {
                let validate = || nonfatal(format!("Unable to validate target `{input}`."));
                if !args.force {
                    let empty = crate::s3::bucket_is_empty(&client, &bucket)
                        .await
                        .map_err(|error| {
                            if crate::error::error_code(&error) == Some("NoSuchBucket") {
                                crate::error::McError::bucket_not_found(&bucket).into()
                            } else {
                                error
                            }
                        })
                        .with_context(validate)?;
                    if !empty {
                        bail!(
                            "`{input}` is not empty. Retry this command with ‘--force’ flag if you want to remove `{input}` and all its contents"
                        );
                    }
                } else if let Err(error) = client.head_bucket().bucket(&bucket).send().await {
                    if error.as_service_error().is_some_and(|e| e.is_not_found()) {
                        // mc: `rb --force` on a missing bucket is a no-op.
                        return Ok(false);
                    }
                    return Err(crate::s3::s3_error(&error, &bucket, "").context(validate()));
                }
                crate::s3::remove_bucket(&alias, &bucket, args.force)
                    .await
                    .with_context(|| format!("Failed to remove `{input}`."))?;
                Ok(true)
            });
            match removed {
                Ok(true) => {}
                Ok(false) => continue,
                // mc reports validation errors and goes on with the next target.
                Err(error) if error.downcast_ref::<crate::error::NonFatal>().is_some() => {
                    crate::output::print_error(&error);
                    failed = true;
                    continue;
                }
                Err(error) => return Err(error),
            }
            let bucket_url = match &target.bucket {
                Some(_) => input.clone(),
                None => format!("{}/{bucket}", input.trim_end_matches('/')),
            };
            if json {
                crate::output::print_json(&BucketMessage {
                    status: "success",
                    bucket: &bucket_url,
                })?;
            } else {
                println!("Removed `{bucket_url}` successfully.");
            }
        }
    }
    if failed {
        return Err(crate::output::Exit(1).into());
    }
    Ok(())
}

/// mc `removeBucketMessage`.
#[derive(Debug, Serialize)]
struct BucketMessage<'a> {
    status: &'static str,
    bucket: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(force: bool, dangerous: bool, targets: &[&str]) -> RemoveBucketArgs {
        RemoveBucketArgs {
            force,
            dangerous,
            targets: targets.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn alias_root_requires_force_and_dangerous() {
        assert!(validate(&args(false, false, &["a/b", "a/c"])).is_ok());
        let error = validate(&args(true, false, &["a"]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("site-wide"));
        assert!(validate(&args(false, true, &["a"])).is_err());
        assert!(validate(&args(true, true, &["a"])).is_ok());
        assert!(validate(&args(true, true, &["a/b/key"])).is_err());
    }
}
