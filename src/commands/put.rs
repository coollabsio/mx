use crate::cli::CopyArgs;
use crate::commands::cp::{resolve_destination_key, source_name_from_local};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::location::{Location, parse_location};
use anyhow::{Result, bail};
use serde::Serialize;
use std::io::Read;

pub fn run(args: CopyArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let target = parse_location(&args.target, store.config());

    let Location::S3(dst) = target else {
        bail!("`put` target must be an S3 object target.");
    };

    let alias = alias_config(&store, &dst.alias)?;
    let result = if args.source == "-" {
        run_stdin(&args.source, &dst, &alias)?
    } else {
        let source = parse_location(&args.source, store.config());
        match source {
            Location::Local(path) => {
                let bucket = dst.require_bucket()?.to_string();
                let key = resolve_destination_key(&dst, source_name_from_local(&path)?)?;
                let bytes =
                    runtime()?.block_on(crate::s3::put_local_file(&alias, &bucket, &key, &path))?;
                PutResult::new(
                    args.source,
                    format!("{}/{bucket}/{}", dst.alias, key),
                    Some(bytes),
                )
            }
            Location::S3(_) => bail!("`put` source must be local path or `-`."),
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!(
            "Uploaded `{}` -> `{}` successfully.",
            result.source, result.target
        );
    }

    Ok(())
}

pub fn run_stdin(
    source: &str,
    target: &crate::target::TargetRef,
    alias: &crate::config::model::AliasConfig,
) -> Result<PutResult> {
    let bucket = target.require_bucket()?.to_string();
    let key = resolve_destination_key(target, "stdin".to_string())?;
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes)?;
    let written = runtime()?.block_on(crate::s3::put_object_bytes(
        alias, &bucket, &key, bytes, None,
    ))?;
    Ok(PutResult::new(
        source.to_string(),
        format!("{}/{bucket}/{}", target.alias, key),
        Some(written),
    ))
}

#[derive(Debug, Serialize)]
pub struct PutResult {
    status: &'static str,
    source: String,
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<i64>,
}

impl PutResult {
    fn new(source: String, target: String, bytes: Option<i64>) -> Self {
        Self {
            status: "success",
            source,
            target,
            bytes,
        }
    }
}
