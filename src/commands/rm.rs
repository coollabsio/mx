use crate::cli::RemoveArgs;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::output;
use crate::target::TargetRef;
use anyhow::{Result, bail};
use serde::Serialize;

pub fn run(args: RemoveArgs, json: bool) -> Result<()> {
    let target = TargetRef::parse(&args.target)?;
    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &target.alias)?;
    let bucket = target.require_bucket()?.to_string();

    if args.recursive {
        if !args.force {
            bail!("This operation requires `--force`.");
        }
        let prefix = target.key_with_trailing_slash().unwrap_or_default();
        let keys = runtime()?.block_on(crate::s3::delete_prefix(&alias, &bucket, &prefix))?;
        if json {
            for key in &keys {
                println!(
                    "{}",
                    serde_json::to_string(&RemoveMessage {
                        status: "success",
                        target: &args.target,
                        bucket: &bucket,
                        key,
                    })?
                );
            }
        } else {
            output::print_plain(&format!("Removed `{}` objects successfully.", keys.len()));
        }
        return Ok(());
    }

    let key = target.require_object_key()?;
    runtime()?.block_on(crate::s3::delete_object(&alias, &bucket, &key))?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&RemoveMessage {
                status: "success",
                target: &args.target,
                bucket: &bucket,
                key: &key,
            })?
        );
    } else {
        output::print_plain(&format!("Removed `{}` successfully.", args.target));
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct RemoveMessage<'a> {
    status: &'static str,
    target: &'a str,
    bucket: &'a str,
    key: &'a str,
}
