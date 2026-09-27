use crate::commands::cp::{resolve_destination_key, source_name_from_local};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::error::{McError, abs_path, nonfatal};
use crate::flags::{ChecksumFlag, EncFlags, parse_size};
use crate::location::{Location, parse_location};
use crate::progress::{CopyMessage, Progress, ProgressReader};
use crate::s3::{BlockingReader, PutOptions, upload_file, upload_stream};
use anyhow::{Context, Result, bail};
use clap::Args;

#[derive(Debug, Args)]
pub struct PutArgs {
    #[command(flatten)]
    pub enc: EncFlags,
    #[command(flatten)]
    pub checksum: ChecksumFlag,
    /// upload number of parts in parallel
    #[arg(short = 'P', long, default_value_t = 4, allow_negative_numbers = true)]
    pub parallel: i64,
    /// each part size
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
    #[arg(long = "storage-class", visible_alias = "sc", value_name = "CLASS")]
    pub storage_class: Option<String>,
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
    let enc = args.enc.entries()?;
    let store = ConfigStore::load_or_create()?;
    // mc reports problems found while preparing the upload with `errorIf` and exits 0.
    let prepare_error = |err: anyhow::Error| {
        crate::output::print_error(&err.context(nonfatal("Unable to upload.")));
        Ok(())
    };
    let dst = match parse_location(&args.target, store.config()) {
        Location::S3(dst) if dst.bucket.as_deref().is_some_and(|b| !b.is_empty()) => dst,
        Location::S3(_) => {
            return prepare_error(McError::new("Bucket should not be empty.").into());
        }
        Location::Local(_) => return prepare_error(McError::new("Target is not s3.").into()),
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

    // Resolve every source first, like mc's URL preparation.
    let mut uploads = Vec::new();
    for source in &args.sources {
        if source == "-" {
            uploads.push((None, "-".to_string(), "stdin".to_string(), 0));
            continue;
        }
        match parse_location(source, store.config()) {
            Location::Local(path) => match std::fs::metadata(&path) {
                Err(error) => return prepare_error(crate::error::io_error(&error, source).into()),
                Ok(meta) if meta.is_dir() => {
                    return prepare_error(McError::invalid_argument().into());
                }
                Ok(meta) => {
                    let name = source_name_from_local(&path)?;
                    uploads.push((Some(path), abs_path(source), name, meta.len()));
                }
            },
            Location::S3(_) => {
                return prepare_error(McError::new("Source is not local filepath.").into());
            }
        }
    }

    let rt = runtime()?;
    let client = rt.block_on(crate::s3::build_client(&alias))?;
    let client = if args.if_not_exists {
        crate::s3::with_if_none_match(&client)
    } else {
        client
    };
    let bar = !json && !crate::globals::quiet() && crate::output::stdout_is_terminal();
    let progress = Progress::new(0, bar);
    for (path, source, name, size) in uploads {
        let key = resolve_destination_key(&dst, name)?;
        let target = format!("{}/{bucket}/{key}", dst.alias);
        progress.add_total(size);
        // mc `put` never fills in the running totals.
        progress.announce(
            &CopyMessage {
                source: &source,
                target: &target,
                size,
                total_count: 0,
            },
            json,
        )?;
        let mut options = PutOptions {
            storage_class: args.storage_class.clone(),
            sse: crate::flags::resolve_sse(&enc, &target),
            checksum: args.checksum.checksum,
            disable_multipart: args.disable_multipart,
            part_size: Some(part_size),
            parallel: Some(args.parallel as usize),
            ..Default::default()
        };
        let result = rt.block_on(async {
            match &path {
                Some(path) => {
                    options.local_source(path);
                    let progress = Some(progress.clone());
                    upload_file(&client, &bucket, &key, path, size, &options, progress).await
                }
                None => {
                    let stdin = BlockingReader::new(std::io::stdin().lock());
                    let reader = ProgressReader::new(stdin, progress.clone());
                    upload_stream(&client, &bucket, &key, reader, None, &options).await
                }
            }
        });
        if let Err(err) = result {
            progress.finish(false);
            return Err(err.context("unable to upload"));
        }
    }
    progress.finish(true);
    progress.print_summary(json)
}
