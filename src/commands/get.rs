use crate::commands::cat::EncCFlag;
use crate::commands::cp::{resolve_local_destination, source_name_from_key, write_local};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::error::{McError, nonfatal};
use crate::flags::{VersionIdFlag, resolve_sse};
use crate::local_fs::clean_path;
use crate::location::{Location, parse_location};
use crate::output;
use crate::progress::{CopyMessage, Progress, ProgressReader};
use crate::s3::GetOptions;
use anyhow::Result;
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

pub fn run(args: GetArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    // mc reports problems found while preparing the download with `errorIf` and exits 0.
    let prepare_error = |message: &str| {
        output::print_error(
            &anyhow::Error::new(McError::new(message)).context(nonfatal("Unable to download.")),
        );
        Ok(())
    };
    let Location::S3(source) = parse_location(&args.source, store.config()) else {
        return prepare_error("Source is not s3.");
    };
    let Some(bucket) = source.bucket.clone().filter(|bucket| !bucket.is_empty()) else {
        return prepare_error("Please set bucket for s3 resource.");
    };
    let key = source.key_with_trailing_slash().unwrap_or_default();
    if key.is_empty() {
        return prepare_error("Please set a full path for s3 resource.");
    }
    let target = args.target.as_deref().unwrap_or(".");
    if matches!(parse_location(target, store.config()), Location::S3(_)) {
        return prepare_error("Target is not local filesystem.");
    }
    let alias = alias_config(&store, &source.alias)?;
    let options = GetOptions {
        version_id: args.version.version_id.clone(),
        sse_c: resolve_sse(
            &args.enc.entries()?,
            &format!("{}/{bucket}/{key}", source.alias),
        )
        .and_then(|sse| sse.customer_key()),
        ..Default::default()
    };
    let destination =
        resolve_local_destination(std::path::Path::new(target), source_name_from_key(&key))?;

    let bar = !json && !crate::globals::quiet() && output::stdout_is_terminal();
    // mc `get` does not stat the source: the total and the announced size stay 0.
    let progress = Progress::new(0, bar);
    progress.announce(
        &CopyMessage {
            source: &format!("{}/{bucket}/{key}", source.alias),
            target: &clean_path(&destination.to_string_lossy()),
            size: 0,
            total_count: 0,
        },
        json,
    )?;
    let result = runtime()?.block_on(async {
        let client = crate::s3::build_client(&alias).await?;
        let response = crate::s3::get_object(&client, &bucket, &key, &options).await?;
        let reader = ProgressReader::new(response.body.into_async_read(), progress.clone());
        write_local(reader, &destination).await
    });
    if let Err(err) = result {
        progress.finish(false);
        // mc returns the error from the command; `main` prints it as plain text (also with
        // --json) and exits 1.
        let report = output::report(&err);
        let text = output::format_fatal_text(&report.message, &report.cause);
        eprintln!("{}: <ERROR> {}", output::prog_name(), text.trim_end());
        return Err(output::Exit(1).into());
    }
    progress.finish(true);
    progress.print_summary(json)
}
