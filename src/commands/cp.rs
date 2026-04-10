use crate::cli::CopyArgs;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::location::{Location, parse_location};
use anyhow::{Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub fn run(args: CopyArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let source = parse_location(&args.source, store.config());
    let target = parse_location(&args.target, store.config());

    let result = match (&source, &target) {
        (Location::Local(src), Location::S3(dst)) => {
            let alias = alias_config(&store, &dst.alias)?;
            let bucket = dst.require_bucket()?.to_string();
            let key = resolve_destination_key(dst, source_name_from_local(src)?)?;
            let bytes =
                runtime()?.block_on(crate::s3::put_local_file(&alias, &bucket, &key, src))?;
            CopyResult::new(
                args.source,
                format!("{}/{bucket}/{}", dst.alias, key),
                Some(bytes),
            )
        }
        (Location::S3(src), Location::Local(dst)) => {
            let alias = alias_config(&store, &src.alias)?;
            let bucket = src.require_bucket()?.to_string();
            let key = src.require_object_key()?;
            let path = resolve_local_destination(dst, source_name_from_key(&key))?;
            let bytes = runtime()?.block_on(crate::s3::download_object_to_path(
                &alias, &bucket, &key, &path,
            ))?;
            CopyResult::new(args.source, path.display().to_string(), Some(bytes))
        }
        (Location::S3(src), Location::S3(dst)) => {
            let src_alias = alias_config(&store, &src.alias)?;
            let dst_alias = alias_config(&store, &dst.alias)?;
            let src_bucket = src.require_bucket()?.to_string();
            let src_key = src.require_object_key()?;
            let dst_bucket = dst.require_bucket()?.to_string();
            let dst_key = resolve_destination_key(dst, source_name_from_key(&src_key))?;
            runtime()?.block_on(crate::s3::copy_object(
                &src_alias,
                &src_bucket,
                &src_key,
                &dst_alias,
                &dst_bucket,
                &dst_key,
            ))?;
            CopyResult::new(
                args.source,
                format!("{}/{dst_bucket}/{}", dst.alias, dst_key),
                None,
            )
        }
        (Location::Local(_), Location::Local(_)) => {
            bail!("Local-to-local copy is not supported by `mx cp`.")
        }
    };

    print_result(&result, json)
}

pub(crate) fn source_name_from_local(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("Unable to determine file name from `{}`.", path.display()))
}

pub(crate) fn source_name_from_key(key: &str) -> String {
    key.rsplit('/').next().unwrap_or(key).to_string()
}

pub(crate) fn resolve_destination_key(
    target: &crate::target::TargetRef,
    fallback: String,
) -> Result<String> {
    let key = match target.key_with_trailing_slash() {
        Some(key) if key.ends_with('/') => format!("{key}{fallback}"),
        Some(key) => key,
        None => fallback,
    };

    if key.is_empty() {
        bail!("Target is missing object key.");
    }

    Ok(key)
}

pub(crate) fn resolve_local_destination(target: &Path, fallback: String) -> Result<PathBuf> {
    if target.exists() && target.is_dir() {
        return Ok(target.join(fallback));
    }

    let rendered = target.to_string_lossy();
    if rendered.ends_with(std::path::MAIN_SEPARATOR) || rendered.ends_with('/') {
        return Ok(target.join(fallback));
    }

    Ok(target.to_path_buf())
}

fn print_result(result: &CopyResult, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        println!(
            "Copied `{}` -> `{}` successfully.",
            result.source, result.target
        );
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct CopyResult {
    status: &'static str,
    source: String,
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<i64>,
}

impl CopyResult {
    fn new(source: String, target: String, bytes: Option<i64>) -> Self {
        Self {
            status: "success",
            source,
            target,
            bytes,
        }
    }
}
