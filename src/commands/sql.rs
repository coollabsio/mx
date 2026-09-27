//! `mx sql` (mc `sql`): S3 Select (`SelectObjectContent`) on objects, records to stdout.
//!
//! Like mc, per-target failures are reported with `errorIf` and the command still exits 0;
//! invalid serialization options are fatal.

use crate::commands::runtime;
use crate::commands::util::{TargetKind, stat_target};
use crate::config::ConfigStore;
use crate::error::{McError, nonfatal};
use crate::flags::{EncFlags, Sse, resolve_sse};
use crate::location::{Location, parse_location};
use crate::output;
use crate::s3::select::{self, SelectOpts};
use anyhow::{Context, Result};
use clap::Args;
use std::io::{BufRead, Read, Write};

#[derive(Debug, Args)]
#[command(after_help = "SERIALIZATION OPTIONS:
  For query serialization options, refer to https://docs.min.io/community/minio-object-store/reference/minio-mc/mc-sql.html

EXAMPLES:
  1. Run a query on a set of objects recursively on AWS S3.
     $ mc sql --recursive --query \"select * from S3Object\" s3/personalbucket/my-large-csvs/

  2. Run a query on an object on MinIO.
     $ mc sql --query \"select count(s.power) from S3Object s\" myminio/iot-devices/power-ratio.csv

  3. Run a query on an encrypted object with customer provided keys.
     $ mc sql --enc-c \"myminio/iot-devices=MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDA\" \\
         --query \"select count(s.power) from S3Object s\" myminio/iot-devices/power-ratio-encrypted.csv

  4. Run a query on an object on MinIO in gzip format using ; as field delimiter,
     newline as record delimiter and file header to be used
     $ mc sql --compression GZIP --csv-input \"rd=\\n,fh=USE,fd=;\" \\
         --query \"select count(s.power) from S3Object s\" myminio/iot-devices/power-ratio.csv.gz

  5. Run a query on an object on MinIO in gzip format using ; as field delimiter,
     newline as record delimiter and file header to be used
     $ mc sql --compression GZIP --csv-input \"rd=\\n,fh=USE,fd=;\" \\
         --json-output \"rd=\\n\\n\" --query \"select * from S3Object\" myminio/iot-devices/data.csv

  6. Run same query as in 5., but specify csv output headers. If --csv-output-headers is
     specified as \"\", first row of csv is interpreted as header
     $ mc sql --compression GZIP --csv-input \"rd=\\n,fh=USE,fd=;\" \\
         --csv-output \"rd=\\n\" --csv-output-header \"device_id,uptime,lat,lon\" \\
         --query \"select * from S3Object\" myminio/iot-devices/data.csv")]
pub struct SqlArgs {
    #[arg(value_name = "TARGETS", required = true)]
    pub targets: Vec<String>,
    #[arg(
        long = "query",
        short = 'e',
        default_value = "select * from s3object",
        value_name = "VALUE",
        help = "sql query expression"
    )]
    pub query: String,
    #[arg(long = "recursive", short = 'r', help = "sql query recursively")]
    pub recursive: bool,
    #[arg(
        long = "csv-input",
        value_name = "VALUE",
        help = "csv input serialization option"
    )]
    pub csv_input: Option<String>,
    #[arg(
        long = "json-input",
        value_name = "VALUE",
        help = "json input serialization option"
    )]
    pub json_input: Option<String>,
    #[arg(
        long = "compression",
        value_name = "VALUE",
        help = "input compression type"
    )]
    pub compression: Option<String>,
    #[arg(
        long = "csv-output",
        value_name = "VALUE",
        help = "csv output serialization option"
    )]
    pub csv_output: Option<String>,
    #[arg(
        long = "csv-output-header",
        value_name = "VALUE",
        help = "optional csv output header"
    )]
    pub csv_output_header: Option<String>,
    #[arg(
        long = "json-output",
        value_name = "VALUE",
        help = "json output serialization option"
    )]
    pub json_output: Option<String>,
    #[arg(
        long = "enc-c",
        value_name = "VALUE",
        help = "encrypt/decrypt objects using client provided keys. (multiple keys can be provided) Formats: RawBase64 or Hex."
    )]
    pub enc_c: Vec<String>,
}

/// mc `supportedContentTypes` for objects found by listing a folder.
const SUPPORTED_CONTENT_TYPES: [&str; 4] = ["csv", "json", "gzip", "bzip2"];

/// Query, CSV header and select options, derived from the first object (mc
/// `getAndValidateArgs`).
struct Prepared {
    headers: Vec<String>,
    opts: SelectOpts,
}

struct Sql<'a> {
    args: &'a SqlArgs,
    json: bool,
    store: ConfigStore,
    enc: Vec<(String, Sse)>,
    rt: tokio::runtime::Runtime,
    prepared: Option<Prepared>,
    write_header: bool,
}

pub fn run(args: SqlArgs, json: bool) -> Result<()> {
    let enc = EncFlags {
        enc_c: args.enc_c.clone(),
        ..Default::default()
    }
    .entries()
    .context("Unable to parse encryption keys.")?;
    let mut sql = Sql {
        args: &args,
        json,
        store: ConfigStore::load_or_create()?,
        enc,
        rt: runtime()?,
        prepared: None,
        write_header: true,
    };
    for url in &args.targets {
        match sql.stat(url) {
            Err(err) => {
                report(err.context(nonfatal(format!("Unable to run sql for {url}."))));
            }
            Ok(TargetKind::File) => {
                if sql.write_header {
                    sql.prepare(url)?;
                }
                if let Err(err) = sql.select(url) {
                    if crate::s3::admin::is_broken_pipe(&err) {
                        return Ok(());
                    }
                    report(err.context(nonfatal("Unable to run sql")));
                }
                sql.write_header = false;
            }
            Ok(TargetKind::Folder) => {
                if !sql.folder(url)? {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

/// mc `errorIf`: reported, the command continues (and exits 0).
fn report(err: anyhow::Error) {
    output::print_error(&err);
}

impl Sql<'_> {
    /// mc `url2Stat` with the `--enc-c` keys: SSE-C objects need their key for HEAD.
    fn stat(&self, url: &str) -> Result<TargetKind> {
        if let Some(customer_key) = self.sse_c(url)
            && let Location::S3(target) = parse_location(url, self.store.config())
            && let (Some(bucket), Some(key)) = (&target.bucket, &target.key)
            && !key.is_empty()
            && !key.ends_with('/')
        {
            let alias = crate::commands::alias_config(&self.store, &target.alias)?;
            let found = self.rt.block_on(async {
                let client = crate::s3::build_client(&alias).await?;
                crate::s3::stat_object_sse_c(&client, bucket, key, None, Some(customer_key)).await
            });
            if found.is_ok() {
                return Ok(TargetKind::File);
            }
        }
        stat_target(&self.store, url)
    }

    /// Lists `url` like mc (`DirNone`) and queries CSV/JSON/compressed objects. False when
    /// stdout was closed.
    fn folder(&mut self, url: &str) -> Result<bool> {
        let objects = match self.list(url) {
            Ok(objects) => objects,
            Err(err) => {
                report(err.context(nonfatal(format!("Unable to list on target `{url}`."))));
                return Ok(true);
            }
        };
        for object in objects {
            if self.write_header {
                self.prepare(&object)?;
            }
            let content_type = select::type_by_extension(&object);
            for suffix in SUPPORTED_CONTENT_TYPES {
                if content_type.contains(suffix)
                    && let Err(err) = self.select(&object)
                {
                    if crate::s3::admin::is_broken_pipe(&err) {
                        return Ok(false);
                    }
                    report(err.context(nonfatal("Unable to run sql")));
                }
                self.write_header = false;
            }
        }
        Ok(true)
    }

    /// Objects under a folder target as `ALIAS/BUCKET/KEY` (or local paths).
    fn list(&self, url: &str) -> Result<Vec<String>> {
        let recursive = self.args.recursive;
        let target = match parse_location(url, self.store.config()) {
            Location::Local(path) => {
                let root = path.to_string_lossy().trim_end_matches('/').to_string();
                return Ok(if recursive {
                    crate::transfer::local_inventory(&path)?
                        .into_iter()
                        .map(|entry| format!("{root}/{}", entry.relative))
                        .collect()
                } else {
                    let mut files: Vec<String> = std::fs::read_dir(&path)?
                        .filter_map(|entry| entry.ok())
                        .filter(|entry| entry.path().is_file())
                        .map(|entry| format!("{root}/{}", entry.file_name().to_string_lossy()))
                        .collect();
                    files.sort();
                    files
                });
            }
            Location::S3(target) => target,
        };
        let alias = crate::commands::alias_config(&self.store, &target.alias)?;
        let raw = url
            .split_once('/')
            .map(|(_, rest)| rest)
            .unwrap_or_default();
        // mc lists `ALIAS` / `ALIAS/BUCKET` (no trailing slash) as the bucket entries
        // themselves unless recursive: nothing to query.
        let buckets: Vec<String> = match &target.bucket {
            Some(bucket) if recursive || raw.contains('/') => vec![bucket.clone()],
            Some(_) => return Ok(Vec::new()),
            None if recursive => self
                .rt
                .block_on(async {
                    let client = crate::s3::build_client(&alias).await?;
                    crate::s3::list_bucket_infos(&client).await
                })?
                .into_iter()
                .map(|(name, _)| name)
                .collect(),
            None => return Ok(Vec::new()),
        };
        let prefix = raw.split_once('/').map(|(_, key)| key).unwrap_or_default();
        let options = crate::s3::ListOptions {
            recursive,
            ..Default::default()
        };
        let mut objects = Vec::new();
        for bucket in buckets {
            let items = self.rt.block_on(async {
                let client = crate::s3::build_client(&alias).await?;
                crate::s3::list_raw(&client, &bucket, prefix, &options).await
            })?;
            objects.extend(
                items
                    .into_iter()
                    .filter(|item| !item.is_prefix)
                    .map(|item| format!("{}/{bucket}/{}", target.alias, item.key)),
            );
        }
        Ok(objects)
    }

    /// mc `getAndValidateArgs` for the first queried object.
    fn prepare(&mut self, url: &str) -> Result<()> {
        let args = self.args;
        let headers = match &args.csv_output_header {
            None => Vec::new(),
            Some(value) if value.is_empty() && is_select_all(&args.query) => {
                self.csv_header(url).unwrap_or_else(|_| vec![String::new()])
            }
            Some(value) => value.split(',').map(str::to_string).collect(),
        };
        let mut opts = SelectOpts {
            compression: args.compression.clone().unwrap_or_default(),
            ..Default::default()
        };
        // Input serialization.
        if args.csv_input.is_some() && args.json_input.is_some() {
            return Err(McError::invalid_argument()).context(
                "Only one of --csv-input or --json-input can be specified as input serialization option",
            );
        }
        if let Some(value) = args.csv_input.as_deref().filter(|v| !v.is_empty()) {
            opts.csv_input = Some(
                select::parse_serialization_opts(
                    value,
                    select::CSV_INPUT_KEYS,
                    select::CSV_INPUT_ABBR,
                )
                .context("Invalid serialization option(s) specified for --csv-input flag")?,
            );
        }
        if let Some(value) = args.json_input.as_deref().filter(|v| !v.is_empty()) {
            opts.json_input = Some(
                select::parse_serialization_opts(value, select::JSON_INPUT_KEYS, &[])
                    .context("Invalid serialization option(s) specified for --json-input flag")?,
            );
        }
        // Output serialization.
        if args.csv_output.is_some() && args.json_output.is_some() {
            return Err(McError::invalid_argument()).context(
                "Only one of --csv-output, or --json-output can be specified as output serialization option",
            );
        }
        if args.json_output.is_some() && !headers.is_empty() {
            return Err(McError::invalid_argument())
                .context("--csv-output-header incompatible with --json-output option");
        }
        if let Some(value) = &args.csv_output {
            opts.csv_output = Some(
                select::parse_serialization_opts(
                    value,
                    select::CSV_OUTPUT_KEYS,
                    select::CSV_OUTPUT_ABBR,
                )
                .context("Invalid value(s) specified for --csv-output flag")?,
            );
        }
        if args.json_output.is_some() || self.json {
            let value = args.json_output.as_deref().unwrap_or_default();
            opts.json_output = Some(
                select::parse_serialization_opts(
                    value,
                    select::JSON_OUTPUT_KEYS,
                    select::JSON_OUTPUT_ABBR,
                )
                .context("Invalid value(s) specified for --json-output flag")?,
            );
        }
        // mc `validateOpts`.
        if url.ends_with(".parquet") && opts.has_input() {
            return Err(McError::invalid_argument()).context(
                "Input serialization flags --csv-input and --json-input cannot be used for object in .parquet format",
            );
        }
        self.prepared = Some(Prepared { headers, opts });
        Ok(())
    }

    /// mc `getCSVHeader`: the first line of the object (decompressed by Content-Type), split
    /// on commas.
    fn csv_header(&self, url: &str) -> Result<Vec<String>> {
        let (data, content_type) = match parse_location(url, self.store.config()) {
            Location::Local(path) => {
                let mut data = Vec::new();
                std::fs::File::open(&path)?
                    .take(1 << 20)
                    .read_to_end(&mut data)?;
                (data, select::type_by_extension(url))
            }
            Location::S3(target) => {
                let alias = crate::commands::alias_config(&self.store, &target.alias)?;
                let bucket = target.require_bucket()?.to_string();
                let key = target.require_object_key()?;
                let options = crate::s3::GetOptions {
                    sse_c: self.sse_c(url),
                    range: Some((0, Some((1 << 20) - 1))),
                    ..Default::default()
                };
                self.rt.block_on(async {
                    let client = crate::s3::build_client(&alias).await?;
                    let output = crate::s3::get_object(&client, &bucket, &key, &options).await?;
                    let content_type = output.content_type.clone().unwrap_or_default();
                    let data = output.body.collect().await?.into_bytes().to_vec();
                    anyhow::Ok((data, content_type))
                })?
            }
        };
        let mut reader: Box<dyn BufRead> = if content_type.contains("gzip") {
            Box::new(std::io::BufReader::new(flate2::read::GzDecoder::new(
                &data[..],
            )))
        } else if content_type.contains("bzip") {
            Box::new(std::io::BufReader::new(bzip2::read::BzDecoder::new(
                &data[..],
            )))
        } else {
            Box::new(&data[..])
        };
        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line)?;
        if line.is_empty() {
            anyhow::bail!("EOF");
        }
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end_matches('\n').trim_end_matches('\r');
        Ok(line.split(',').map(str::to_string).collect())
    }

    fn sse_c(&self, url: &str) -> Option<[u8; 32]> {
        resolve_sse(&self.enc, url).and_then(|sse| sse.customer_key())
    }

    /// mc `sqlSelect`: runs the query and copies the records to stdout (after the CSV header
    /// for the first object).
    fn select(&self, url: &str) -> Result<()> {
        let target = match parse_location(url, self.store.config()) {
            Location::Local(_) => {
                return Err(McError::with_detail(
                    "`Select` is not supported for `filesystem`.",
                    crate::detail![("API", "Select"), ("APIType", "filesystem")],
                )
                .into());
            }
            Location::S3(target) => target,
        };
        let alias = crate::commands::alias_config(&self.store, &target.alias)?;
        let bucket = target.require_bucket()?.to_string();
        let key = target.require_object_key()?;
        let prepared = self.prepared.as_ref().expect("prepared before select");
        let sse_c = self.sse_c(url);
        self.rt.block_on(async {
            let client = crate::s3::build_client(&alias).await?;
            let mut stream = select::select(
                &client,
                &bucket,
                &key,
                &self.args.query,
                sse_c,
                &prepared.opts,
            )
            .await?;
            let mut stdout = std::io::stdout().lock();
            if !prepared.headers.is_empty() && self.write_header {
                writeln!(stdout, "{}", prepared.headers.join(","))?;
            }
            while let Some(records) = stream.next().await? {
                stdout.write_all(&records)?;
            }
            stdout.flush()?;
            Ok(())
        })
    }
}

/// mc `isSelectAll`: `^\s*?select\s+?\*\s+?.*?$` (case-sensitive, single line).
fn is_select_all(query: &str) -> bool {
    let Some(rest) = query.trim_start().strip_prefix("select") else {
        return false;
    };
    let after = rest.trim_start();
    if after.len() == rest.len() {
        return false;
    }
    let Some(rest) = after.strip_prefix('*') else {
        return false;
    };
    let after = rest.trim_start();
    // `.` does not match newlines, so the tail must be one line (the whitespace may not).
    after.len() != rest.len() && !after.contains('\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_all_matches_mc_regex() {
        assert!(is_select_all("select * from s3object"));
        assert!(is_select_all("  select   *  from S3Object s"));
        assert!(!is_select_all("SELECT * from s3object"));
        assert!(!is_select_all("select s.name from s3object"));
        assert!(!is_select_all("select *from s3object"));
        assert!(!is_select_all("select * from\ns3object"));
    }
}
