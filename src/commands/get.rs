use crate::commands::cat::EncCFlag;
use crate::commands::cp::{resolve_local_destination, source_name_from_key};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::{VersionIdFlag, resolve_sse};
use crate::location::{Location, parse_location};
use crate::output;
use crate::s3::GetOptions;
use anyhow::{Result, bail};
use clap::Args;

#[derive(Debug, Args)]
pub struct GetArgs {
    #[command(flatten)]
    pub version: VersionIdFlag,
    #[command(flatten)]
    pub enc: EncCFlag,
    pub source: String,
    pub target: Option<String>,
}

pub fn run(args: GetArgs, _json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let Location::S3(source) = parse_location(&args.source, store.config()) else {
        bail!("`get` source must be an S3 object");
    };
    let alias = alias_config(&store, &source.alias)?;
    let bucket = source.require_bucket()?.to_string();
    let key = source.require_object_key()?;
    let options = GetOptions {
        version_id: args.version.version_id.clone(),
        sse_c: resolve_sse(
            &args.enc.entries()?,
            &format!("{}/{bucket}/{key}", source.alias),
        )
        .and_then(|sse| sse.customer_key()),
        ..Default::default()
    };
    let fallback = source_name_from_key(&key);
    let destination = match args.target {
        Some(path) => resolve_local_destination(std::path::Path::new(&path), fallback)?,
        None => std::env::current_dir()?.join(fallback),
    };
    let bytes = runtime()?.block_on(crate::s3::download_object_to_path_with(
        &alias,
        &bucket,
        &key,
        &destination,
        &options,
    ))?;
    output::print_plain(&format!(
        "Downloaded `{}` -> `{}` ({} bytes).",
        args.source,
        destination.display(),
        bytes
    ));
    Ok(())
}
