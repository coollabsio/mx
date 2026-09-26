use crate::commands::cp::{resolve_destination_key, source_name_from_local};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::{ChecksumFlag, EncFlags, parse_size};
use crate::location::{Location, parse_location};
use crate::s3::{BlockingReader, PutOptions, upload_stream};
use anyhow::{Context, Result, bail};
use clap::Args;
use serde::Serialize;

#[derive(Debug, Args)]
pub struct PutArgs {
    /// upload number of parts in parallel
    #[arg(short = 'P', long, default_value_t = 4, allow_negative_numbers = true)]
    pub parallel: i64,
    /// each part size (e.g. 16MiB, 64MB)
    #[arg(
        short = 's',
        long = "part-size",
        default_value = "16MiB",
        value_name = "SIZE"
    )]
    pub part_size: String,
    /// upload only if object does not exist
    #[arg(long = "if-not-exists", hide = true)]
    pub if_not_exists: bool,
    /// disable multipart upload feature
    #[arg(long = "disable-multipart")]
    pub disable_multipart: bool,
    /// set storage class for new object on target
    #[arg(long = "storage-class", value_name = "CLASS")]
    pub storage_class: Option<String>,
    #[command(flatten)]
    pub checksum: ChecksumFlag,
    #[command(flatten)]
    pub enc: EncFlags,
    /// local file(s) or `-` for standard input
    #[arg(required = true, num_args = 1..)]
    pub sources: Vec<String>,
    pub target: String,
}

pub fn run(args: PutArgs, json: bool) -> Result<()> {
    if args.parallel < 1 {
        bail!("Invalid number of threads `{}`", args.parallel);
    }
    let part_size = parse_size(&args.part_size).context("Unable to parse part size")?;
    let store = ConfigStore::load_or_create()?;
    let Location::S3(dst) = parse_location(&args.target, store.config()) else {
        bail!("`put` target must be an S3 object target.");
    };
    let alias = alias_config(&store, &dst.alias)?;
    let bucket = dst.require_bucket()?.to_string();
    if args.sources.len() > 1
        && dst
            .key_with_trailing_slash()
            .is_some_and(|key| !key.ends_with('/'))
    {
        bail!(
            "Target `{}` must be a folder (end with `/`) when uploading multiple sources.",
            args.target
        );
    }
    let enc = args.enc.entries()?;
    let rt = runtime()?;
    let client = rt.block_on(crate::s3::build_client(&alias))?;
    let client = if args.if_not_exists {
        crate::s3::with_if_none_match(&client)
    } else {
        client
    };

    for source in &args.sources {
        let (path, name) = if source == "-" {
            (None, "stdin".to_string())
        } else {
            match parse_location(source, store.config()) {
                Location::Local(path) if path.is_dir() => {
                    bail!("`{source}` is a folder. Folder cannot be uploaded with `put`.")
                }
                Location::Local(path) => {
                    let name = source_name_from_local(&path)?;
                    (Some(path), name)
                }
                Location::S3(_) => bail!("`put` source must be local path or `-`."),
            }
        };
        let key = resolve_destination_key(&dst, name)?;
        let options = PutOptions {
            storage_class: args.storage_class.clone(),
            sse: crate::flags::resolve_sse(&enc, &format!("{}/{bucket}/{key}", dst.alias)),
            checksum: args.checksum.checksum,
            disable_multipart: args.disable_multipart,
            part_size: Some(part_size),
            parallel: Some(args.parallel as usize),
            ..Default::default()
        };
        let outcome = rt
            .block_on(async {
                match &path {
                    Some(path) => {
                        let file = tokio::fs::File::open(path).await.with_context(|| {
                            format!("Unable to read local file `{}`.", path.display())
                        })?;
                        let size = file.metadata().await.ok().map(|meta| meta.len());
                        upload_stream(&client, &bucket, &key, file, size, &options).await
                    }
                    None => {
                        let stdin = BlockingReader::new(std::io::stdin().lock());
                        upload_stream(&client, &bucket, &key, stdin, None, &options).await
                    }
                }
            })
            .with_context(|| format!("Unable to upload `{source}`."))?;
        let result = PutResult::new(
            source.clone(),
            format!("{}/{bucket}/{key}", dst.alias),
            outcome.size,
        );
        if json {
            crate::output::print_json(&result)?;
        } else {
            println!(
                "Uploaded `{}` -> `{}` successfully.",
                result.source, result.target
            );
        }
    }
    Ok(())
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
