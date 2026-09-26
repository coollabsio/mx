use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::output;
use crate::target::TargetRef;
use anyhow::{Result, bail};
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

    for (input, target) in args.targets.iter().zip(&targets) {
        let alias = alias_config(&store, &target.alias)?;
        let client = rt.block_on(crate::s3::build_client(&alias))?;
        let buckets = match &target.bucket {
            Some(bucket) => vec![bucket.clone()],
            None => rt
                .block_on(client.list_buckets().send())?
                .buckets()
                .iter()
                .filter_map(|bucket| bucket.name().map(str::to_string))
                .collect(),
        };
        for bucket in buckets {
            let removed = rt.block_on(async {
                if !args.force {
                    if !crate::s3::bucket_is_empty(&client, &bucket).await? {
                        bail!(
                            "`{input}` is not empty. Retry this command with ‘--force’ flag if you want to remove `{input}` and all its contents"
                        );
                    }
                } else if let Err(error) = client.head_bucket().bucket(&bucket).send().await {
                    if error.as_service_error().is_some_and(|e| e.is_not_found()) {
                        // mc: `rb --force` on a missing bucket is a no-op.
                        return Ok(false);
                    }
                    return Err(error.into());
                }
                crate::s3::remove_bucket(&alias, &bucket, args.force).await?;
                Ok(true)
            })?;
            if !removed {
                continue;
            }
            let bucket_target = format!("{}/{bucket}", target.alias);
            if json {
                crate::output::print_json(&BucketMessage {
                    status: "success",
                    target: if target.bucket.is_some() {
                        input
                    } else {
                        &bucket_target
                    },
                    bucket: &bucket,
                })?;
            } else {
                output::print_plain(&format!("Bucket `{bucket}` removed successfully."));
            }
        }
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct BucketMessage<'a> {
    status: &'static str,
    target: &'a str,
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
