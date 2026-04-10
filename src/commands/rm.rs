use crate::cli::TargetArg;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::target::TargetRef;
use anyhow::Result;
use serde::Serialize;

pub fn run(args: TargetArg, json: bool) -> Result<()> {
    let target = TargetRef::parse(&args.target)?;
    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &target.alias)?;
    let bucket = target.require_bucket()?.to_string();
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
        println!("Removed `{}` successfully.", args.target);
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
