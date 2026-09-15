use crate::cli::GetArgs;
use crate::commands::cp::{resolve_local_destination, source_name_from_key};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::location::{Location, parse_location};
use crate::output;
use anyhow::{Result, bail};

pub fn run(args: GetArgs, _json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let Location::S3(source) = parse_location(&args.source, store.config()) else {
        bail!("`get` source must be an S3 object");
    };
    let alias = alias_config(&store, &source.alias)?;
    let bucket = source.require_bucket()?.to_string();
    let key = source.require_object_key()?;
    let fallback = source_name_from_key(&key);
    let destination = match args.target {
        Some(path) => resolve_local_destination(std::path::Path::new(&path), fallback)?,
        None => std::env::current_dir()?.join(fallback),
    };
    let bytes = runtime()?.block_on(crate::s3::download_object_to_path(
        &alias,
        &bucket,
        &key,
        &destination,
    ))?;
    output::print_plain(&format!(
        "Downloaded `{}` -> `{}` ({} bytes).",
        args.source,
        destination.display(),
        bytes
    ));
    Ok(())
}
