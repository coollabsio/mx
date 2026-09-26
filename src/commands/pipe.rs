use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::parse_size;
use crate::flags::{ChecksumFlag, EncFlags, MetadataFlags};
use crate::location::{Location, parse_location};
use crate::progress::format_bytes;
use anyhow::{Context, Result, bail};
use clap::Args;
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Args)]
pub struct PipeArgs {
    #[command(flatten)]
    pub metadata: MetadataFlags,
    /// allow N concurrent uploads [WARNING: will use more memory use it with caution]
    #[arg(
        long,
        default_value_t = 1,
        value_name = "N",
        allow_negative_numbers = true
    )]
    pub concurrent: i64,
    /// customize chunk size for each concurrent upload (default: "528 MiB")
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
                // mc guesses it from the target name, overriding `--attr Content-Type`.
                content_type: Some(crate::s3::guess_content_type(&key)),
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
            let stdin = StdinProgress::new(!quiet && !json);
            let outcome = runtime()?
                .block_on(crate::s3::put_object_reader_with(
                    &alias, &bucket, &key, &stdin, None, &options,
                ))
                .context("Unable to write to one or more targets.");
            stdin.stop();
            outcome?.size.unwrap_or_default()
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
            let mut stdin = StdinProgress::new(!quiet && !json);
            let written = std::io::copy(&mut stdin, &mut file);
            stdin.stop();
            let written = written?;
            file.flush()?;
            written as i64
        }
    };

    if json {
        #[derive(serde::Serialize)]
        struct PipeMessage<'a> {
            status: &'static str,
            target: &'a str,
            size: i64,
        }
        crate::output::print_json(&PipeMessage {
            status: "success",
            target: &args.target,
            size: bytes,
        })?;
    } else {
        // mc prints the result even with --quiet.
        println!("{bytes} bytes -> `{}`", args.target);
    }
    Ok(())
}

/// Stdin with mc's pipe progress: mc starts a total-less progress bar on stdout unless
/// `--quiet`/`--json`, which draws ` 0 B / ? ` at once (also when stdout is not a terminal)
/// and never ends the line, so the result message follows it directly. On a terminal the
/// counter is redrawn (` N B / ?  SPEED/s`) while reading.
struct StdinProgress {
    read: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    drawer: Option<std::thread::JoinHandle<()>>,
}

impl StdinProgress {
    fn new(show: bool) -> Self {
        let read = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let mut drawer = None;
        if show {
            print!("\r 0 B / ? ");
            let _ = std::io::stdout().flush();
            if crate::output::stdout_is_terminal() {
                let (read, stop) = (read.clone(), stop.clone());
                let start = Instant::now();
                drawer = Some(std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(200));
                        let bytes = read.load(Ordering::Relaxed);
                        let speed = bytes as f64 / start.elapsed().as_secs_f64().max(1e-9);
                        print!(
                            "\r {} / ?  {}/s",
                            format_bytes(bytes),
                            format_bytes(speed as u64)
                        );
                        let _ = std::io::stdout().flush();
                    }
                }));
            }
        }
        Self { read, stop, drawer }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(drawer) = self.drawer.take() {
            let _ = drawer.join();
        }
    }
}

impl Read for &StdinProgress {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = std::io::stdin().lock().read(buf)?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl Read for StdinProgress {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        (&*self).read(buf)
    }
}
