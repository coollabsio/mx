use crate::commands::cat::EncCFlag;
use crate::commands::cp::{resolve_local_destination, source_name_from_key};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::{VersionIdFlag, resolve_sse};
use crate::location::{Location, parse_location};
use crate::output;
use crate::s3::GetOptions;
use anyhow::{Context, Result};
use clap::Args;

#[derive(Debug, Args)]
#[command(mut_args(|a| if a.get_id().as_str() == "version_id" {
    a.help("get a specific version of an object")
} else {
    a
}))]
pub struct GetArgs {
    #[command(flatten)]
    pub enc: EncCFlag,
    #[command(flatten)]
    pub version: VersionIdFlag,
    pub source: String,
    pub target: Option<String>,
}

pub fn run(args: GetArgs, _json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let Location::S3(source) = parse_location(&args.source, store.config()) else {
        return Err(
            anyhow::Error::new(crate::error::McError::new("Source is not s3."))
                .context(crate::error::nonfatal("Unable to download.")),
        );
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
    let bytes = runtime()?
        .block_on(crate::s3::download_object_to_path_with(
            &alias,
            &bucket,
            &key,
            &destination,
            &options,
        ))
        .context(crate::error::nonfatal("Unable to download."))?;
    output::print_plain(&format!(
        "Downloaded `{}` -> `{}` ({} bytes).",
        args.source,
        destination.display(),
        bytes
    ));
    Ok(())
}
