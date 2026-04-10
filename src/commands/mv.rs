use crate::cli::CopyArgs;
use crate::commands::cp::{
    resolve_destination_key, resolve_local_destination, source_name_from_key,
    source_name_from_local,
};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::location::{Location, parse_location};
use anyhow::{Result, bail};
use serde::Serialize;

pub fn run(args: CopyArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let source = parse_location(&args.source, store.config());
    let target = parse_location(&args.target, store.config());

    let result = match (&source, &target) {
        (Location::S3(src), Location::S3(dst)) => {
            let src_alias = alias_config(&store, &src.alias)?;
            let dst_alias = alias_config(&store, &dst.alias)?;
            let src_bucket = src.require_bucket()?.to_string();
            let src_key = src.require_object_key()?;
            let dst_bucket = dst.require_bucket()?.to_string();
            let dst_key = resolve_destination_key(dst, source_name_from_key(&src_key))?;

            let rt = runtime()?;
            rt.block_on(crate::s3::copy_object(
                &src_alias,
                &src_bucket,
                &src_key,
                &dst_alias,
                &dst_bucket,
                &dst_key,
            ))?;
            rt.block_on(crate::s3::delete_object(&src_alias, &src_bucket, &src_key))?;
            MoveResult::new(
                args.source,
                format!("{}/{dst_bucket}/{}", dst.alias, dst_key),
            )
        }
        (Location::Local(src), Location::S3(dst)) => {
            let alias = alias_config(&store, &dst.alias)?;
            let bucket = dst.require_bucket()?.to_string();
            let key = resolve_destination_key(dst, source_name_from_local(src)?)?;
            runtime()?.block_on(crate::s3::put_local_file(&alias, &bucket, &key, src))?;
            std::fs::remove_file(src)?;
            MoveResult::new(args.source, format!("{}/{bucket}/{}", dst.alias, key))
        }
        (Location::S3(src), Location::Local(dst)) => {
            let alias = alias_config(&store, &src.alias)?;
            let bucket = src.require_bucket()?.to_string();
            let key = src.require_object_key()?;
            let path = resolve_local_destination(dst, source_name_from_key(&key))?;
            let rt = runtime()?;
            rt.block_on(crate::s3::download_object_to_path(
                &alias, &bucket, &key, &path,
            ))?;
            rt.block_on(crate::s3::delete_object(&alias, &bucket, &key))?;
            MoveResult::new(args.source, path.display().to_string())
        }
        (Location::Local(_), Location::Local(_)) => {
            bail!("Local-to-local move is not supported by `mx mv`.")
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!(
            "Moved `{}` -> `{}` successfully.",
            result.source, result.target
        );
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct MoveResult {
    status: &'static str,
    source: String,
    target: String,
}

impl MoveResult {
    fn new(source: String, target: String) -> Self {
        Self {
            status: "success",
            source,
            target,
        }
    }
}
