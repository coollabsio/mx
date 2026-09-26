use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::parse_size;
use crate::flags::{ChecksumFlag, EncFlags, MetadataFlags};
use crate::location::{Location, parse_location};
use anyhow::{Context, Result, bail};
use clap::Args;
use std::io::Write;

#[derive(Debug, Args)]
pub struct PipeArgs {
    #[command(flatten)]
    pub metadata: MetadataFlags,
    /// allow N concurrent part uploads (uses more memory)
    #[arg(
        long,
        default_value_t = 1,
        value_name = "N",
        allow_negative_numbers = true
    )]
    pub concurrent: i64,
    /// customize chunk size for each concurrent upload (e.g. 16MiB)
    #[arg(long = "part-size", value_name = "SIZE")]
    pub part_size: Option<String>,
    #[command(flatten)]
    pub checksum: ChecksumFlag,
    #[command(flatten)]
    pub enc: EncFlags,
    pub target: String,
}

pub fn run(args: PipeArgs, json: bool) -> Result<()> {
    let quiet = crate::globals::quiet();
    if args.concurrent < 1 {
        bail!("Invalid --concurrent value `{}`", args.concurrent);
    }
    let part_size = args
        .part_size
        .as_deref()
        .map(parse_size)
        .transpose()
        .context("Unable to parse --part-size")?;
    let store = ConfigStore::load_or_create()?;
    let bytes = match parse_location(&args.target, store.config()) {
        Location::S3(target) => {
            let alias = alias_config(&store, &target.alias)?;
            let bucket = target.require_bucket()?.to_string();
            let key = target.require_object_key()?;
            let options = crate::s3::PutOptions {
                metadata: args.metadata.attr_pairs()?,
                tags: args.metadata.tag_pairs()?,
                storage_class: args.metadata.storage_class.clone(),
                sse: args
                    .enc
                    .resolve(&format!("{}/{bucket}/{key}", target.alias))?,
                checksum: args.checksum.checksum,
                part_size,
                parallel: Some(args.concurrent as usize),
                ..Default::default()
            };
            let outcome = runtime()?.block_on(crate::s3::put_object_reader_with(
                &alias,
                &bucket,
                &key,
                std::io::stdin().lock(),
                None,
                &options,
            ))?;
            outcome.size.unwrap_or_default()
        }
        Location::Local(path) => {
            if args.metadata.attr.is_some()
                || args.metadata.tags.is_some()
                || args.metadata.storage_class.is_some()
                || args.checksum.checksum.is_some()
                || !args.enc.is_empty()
                || args.part_size.is_some()
                || args.concurrent != 1
            {
                bail!(
                    "`pipe` flags --attr, --tags, --storage-class, --checksum, --enc-*, --part-size and --concurrent are only supported for S3 targets"
                );
            }
            let mut file = std::fs::File::create(&path)
                .with_context(|| format!("Unable to write local file `{}`.", path.display()))?;
            let written = std::io::copy(&mut std::io::stdin().lock(), &mut file)?;
            file.flush()?;
            written as i64
        }
    };

    if json {
        #[derive(serde::Serialize)]
        struct PipeMessage<'a> {
            status: &'static str,
            size: i64,
            target: &'a str,
        }
        crate::output::print_json(&PipeMessage {
            status: "success",
            size: bytes,
            target: &args.target,
        })?;
    } else if !quiet {
        println!("{bytes} bytes -> `{}`", args.target);
    }
    Ok(())
}
