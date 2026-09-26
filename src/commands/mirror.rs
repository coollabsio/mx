use crate::commands::runtime;
use crate::commands::util::TargetKind;
use crate::config::ConfigStore;
use crate::error::McError;
use crate::flags::{ChecksumFlag, EncFlags, TimeFilterFlags, parse_attr, parse_duration};
use crate::location::{Location, parse_location};
use crate::mirror::Options;
use crate::mirror::diff::PlanOptions;
use anyhow::{Context, Result, bail};
use clap::Args;

#[derive(Debug, Args)]
#[command(mut_args(|a| match a.get_id().as_str() {
    "older_than" => a.help("filter object(s) older than value in duration string (e.g. 7d10h31s)"),
    "newer_than" => a.help("filter object(s) newer than value in duration string (e.g. 7d10h31s)"),
    _ => a,
}))]
pub struct MirrorArgs {
    /// overwrite object(s) on target if it differs from source
    #[arg(long)]
    pub overwrite: bool,
    /// deprecated: same as --overwrite
    #[arg(long, hide = true)]
    pub force: bool,
    /// perform a fake mirror operation
    #[arg(long)]
    pub dry_run: bool,
    /// deprecated: same as --dry-run
    #[arg(long, hide = true)]
    pub fake: bool,
    /// watch and synchronize changes
    #[arg(short = 'w', long)]
    pub watch: bool,
    /// deprecated: same as --watch
    #[arg(long, hide = true)]
    pub multi_master: bool,
    /// rescan interval for --watch (e.g. 1s, 1m)
    #[arg(long, hide = true, default_value = "2s", value_name = "DURATION")]
    pub watch_interval: String,
    /// remove extraneous object(s) on target
    #[arg(long)]
    pub remove: bool,
    /// specify region when creating new bucket(s) on target
    #[arg(long, default_value = "us-east-1")]
    pub region: String,
    /// preserve file(s)/object(s) attributes and bucket(s) policy/locking configuration(s) on target bucket(s)
    #[arg(short = 'a', long)]
    pub preserve: bool,
    /// enable active-active multi-site setup
    #[arg(long)]
    pub active_active: bool,
    /// disable multipart upload feature
    #[arg(long)]
    pub disable_multipart: bool,
    /// exclude object(s) that match specified object name pattern
    #[arg(long, value_name = "PATTERN")]
    pub exclude: Vec<String>,
    /// exclude bucket(s) that match specified bucket name pattern
    #[arg(long, value_name = "PATTERN")]
    pub exclude_bucket: Vec<String>,
    /// exclude object(s) that match the specified storage class
    #[arg(long, value_name = "CLASS")]
    pub exclude_storageclass: Vec<String>,
    #[command(flatten)]
    pub time: TimeFilterFlags,
    /// specify storage class for new object(s) on target
    #[arg(long = "storage-class", visible_alias = "sc", value_name = "CLASS")]
    pub storage_class: Option<String>,
    /// add custom metadata for all objects
    #[arg(long, value_name = "KEY=VALUE;...")]
    pub attr: Option<String>,
    #[arg(
        long,
        value_name = "ADDRESS",
        help = "if specified, a new prometheus endpoint will be created to report mirroring activity. (eg: localhost:8081)"
    )]
    pub monitoring_address: Option<String>,
    /// if specified, will enable retrying on a per object basis if errors occur
    #[arg(long)]
    pub retry: bool,
    /// print a summary of the mirror session
    #[arg(long)]
    pub summary: bool,
    /// skip any errors when mirroring
    #[arg(long)]
    pub skip_errors: bool,
    /// maximum number of concurrent copies (default: autodetect)
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub max_workers: usize,
    #[command(flatten)]
    pub checksum: ChecksumFlag,
    #[command(flatten)]
    pub enc: EncFlags,
    pub source: String,
    pub target: String,
}

impl MirrorArgs {
    fn options(&self, json: bool) -> Result<Options> {
        if self.force {
            let with = if self.remove { " with `--remove`" } else { "" };
            crate::output::error_if(
                &format!(
                    "`--force` is deprecated, please use `--overwrite` instead{with} for the same functionality."
                ),
                &McError::invalid_argument().to_string(),
            );
        }
        self.time.parsed()?;
        let watch_interval = parse_duration(&self.watch_interval)?;
        if watch_interval.is_zero() {
            bail!("`--watch-interval` must be greater than zero.");
        }
        Ok(Options {
            plan: PlanOptions {
                overwrite: self.overwrite || self.force,
                dry_run: self.dry_run || self.fake,
                active_active: self.active_active,
                remove: self.remove,
                exclude: self.exclude.clone(),
                exclude_bucket: self.exclude_bucket.clone(),
                exclude_storage_class: self.exclude_storageclass.clone(),
                time: self.time.clone(),
            },
            watch: self.watch || self.multi_master,
            watch_interval,
            region: self.region.clone(),
            preserve: self.preserve,
            attr: self
                .attr
                .as_deref()
                .map(parse_attr)
                .transpose()
                .context("Unable to parse attribute")?
                .unwrap_or_default(),
            storage_class: self.storage_class.clone(),
            disable_multipart: self.disable_multipart,
            checksum: self.checksum.checksum,
            enc: self.enc.entries()?,
            retry: self.retry,
            summary: self.summary,
            skip_errors: self.skip_errors,
            max_workers: self.max_workers,
            json,
        })
    }
}

pub fn run(args: MirrorArgs, json: bool) -> Result<()> {
    let options = args.options(json)?;
    let store = ConfigStore::load_or_create()?;
    let source = parse_location(&args.source, store.config());
    if !options.keep_running() && !matches!(&source, Location::S3(t) if t.bucket.is_none()) {
        let kind = crate::commands::util::stat_target(&store, &args.source)
            .with_context(|| format!("Unable to stat source `{}`.", args.source))?;
        if kind != TargetKind::Folder {
            return Err(McError::invalid_argument()).with_context(|| {
                format!(
                    "Source `{}` is not a folder. Only folders are supported by mirror command.",
                    args.source
                )
            });
        }
    }

    runtime()?.block_on(async {
        let source = crate::mirror::endpoint(&store, &args.source, true).await?;
        let target = crate::mirror::endpoint(&store, &args.target, false).await?;
        crate::mirror::validate(&source, &target)?;
        if let Some(address) = &args.monitoring_address {
            crate::mirror::metrics::serve(address)
                .await
                .context("Unable to setup monitoring endpoint.")?;
        }
        crate::mirror::run(source, target, options).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Harness {
        #[command(flatten)]
        args: MirrorArgs,
    }

    fn parse(extra: &[&str]) -> MirrorArgs {
        let mut argv = vec!["mirror"];
        argv.extend_from_slice(extra);
        argv.extend_from_slice(&["src", "dst"]);
        Harness::try_parse_from(crate::flags::rewrite_argv(argv))
            .unwrap()
            .args
    }

    #[test]
    fn maps_flags_to_options() {
        let args = parse(&[
            "--overwrite",
            "--remove",
            "-w",
            "-a",
            "--exclude",
            "*.tmp",
            "--exclude",
            ".*",
            "--exclude-bucket",
            "test*",
            "--exclude-storageclass",
            "GLACIER",
            "--older-than",
            "1d",
            "-sc",
            "REDUCED_REDUNDANCY",
            "--attr",
            "k=v;Cache-Control=max-age=1",
            "--max-workers",
            "3",
            "--checksum",
            "crc32c",
            "--watch-interval",
            "1s",
        ]);
        let options = args.options(true).unwrap();
        assert!(options.plan.overwrite && options.plan.remove && options.watch);
        assert!(options.preserve && options.json);
        assert_eq!(options.plan.exclude, vec!["*.tmp", ".*"]);
        assert_eq!(options.plan.exclude_bucket, vec!["test*"]);
        assert_eq!(options.plan.exclude_storage_class, vec!["GLACIER"]);
        assert_eq!(options.storage_class.as_deref(), Some("REDUCED_REDUNDANCY"));
        assert_eq!(options.attr.len(), 2);
        assert_eq!(options.max_workers, 3);
        assert_eq!(options.watch_interval, std::time::Duration::from_secs(1));
        assert_eq!(options.region, "us-east-1");
    }

    #[test]
    fn deprecated_flags_map_to_new_ones() {
        let options = parse(&["--fake", "--force", "--multi-master"])
            .options(false)
            .unwrap();
        assert!(options.plan.dry_run && options.plan.overwrite && options.watch);
    }

    #[test]
    fn rejects_invalid_values() {
        assert_eq!(
            parse(&["--monitoring-address", "localhost:8081"])
                .monitoring_address
                .as_deref(),
            Some("localhost:8081")
        );
        assert!(parse(&["--newer-than", "abc"]).options(false).is_err());
        assert!(parse(&["--attr", "novalue"]).options(false).is_err());
        assert!(parse(&["--watch-interval", "0s"]).options(false).is_err());
    }
}
