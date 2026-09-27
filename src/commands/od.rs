//! `mx od if=SOURCE of=TARGET [size=SIZE] [parts=N] [skip=N]` (area G): measures a single
//! stream upload (local/S3 -> S3 or local) or download (S3 -> local), like `mc od`.

use crate::commands::retention::print_json;
use crate::commands::util::TargetKind;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::error::McError;
use crate::location::{Location, parse_location};
use crate::s3::{GetOptions, MAX_PART_SIZE, MIN_PART_SIZE, PutOptions};
use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};

#[derive(Debug, Args)]
pub struct OdArgs {
    /// operands: if=SOURCE of=TARGET size=SIZE parts=N skip=N
    #[arg(value_name = "KEY=VALUE", required = true)]
    pub operands: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Operands {
    input: String,
    output: String,
    size: Option<u64>,
    parts: Option<i64>,
    skip: Option<i64>,
}

fn parse_operands(operands: &[String]) -> Result<Operands> {
    let mut parsed = Operands::default();
    for operand in operands {
        let (key, value) = operand
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid operand `{operand}`: use KEY=VALUE"))?;
        let number = |value: &str| -> Result<i64> {
            value
                .parse()
                .with_context(|| format!("invalid value for `{key}`: `{value}`"))
        };
        match key {
            "if" => parsed.input = value.to_string(),
            "of" => parsed.output = value.to_string(),
            "size" => parsed.size = Some(crate::flags::parse_size(value)?),
            "parts" => parsed.parts = Some(number(value)?),
            "skip" => parsed.skip = Some(number(value)?),
            _ => bail!("unknown operand `{key}`: supported operands are if, of, size, parts, skip"),
        }
    }
    if parsed.parts.is_some_and(|parts| parts < 0) || parsed.skip.is_some_and(|skip| skip < 0) {
        bail!("parts and skip must not be negative");
    }
    Ok(parsed)
}

/// go-humanize `IBytes`: `3.0 MiB`, `25 MiB`, `512 B`.
fn ibytes(size: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if size < 10 {
        return format!("{size} B");
    }
    let exponent = ((size as f64).ln() / 1024f64.ln()).floor() as usize;
    let exponent = exponent.min(UNITS.len() - 1);
    let value = (size as f64 / 1024f64.powi(exponent as i32) * 10.0 + 0.5).floor() / 10.0;
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[exponent])
    } else {
        format!("{value:.0} {}", UNITS[exponent])
    }
}

/// Go `time.Duration.String()` for a millisecond duration: `0s`, `12ms`, `1.5s`, `2m3.004s`.
fn go_duration(millis: u64) -> String {
    if millis == 0 {
        return "0s".to_string();
    }
    if millis < 1000 {
        return format!("{millis}ms");
    }
    let (hours, minutes) = (millis / 3_600_000, (millis / 60_000) % 60);
    let (secs, frac) = ((millis / 1000) % 60, millis % 1000);
    let mut out = String::new();
    if hours > 0 {
        out.push_str(&format!("{hours}h{minutes}m"));
    } else if minutes > 0 {
        out.push_str(&format!("{minutes}m"));
    }
    out.push_str(&secs.to_string());
    if frac > 0 {
        out.push_str(format!(".{frac:03}").trim_end_matches('0'));
    }
    out.push('s');
    out
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OdMessage {
    status: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    source: String,
    target: String,
    part_size: u64,
    total_size: i64,
    parts: i64,
    skip: i64,
    elapsed: u64,
}

impl OdMessage {
    fn text(&self) -> String {
        let size = ibytes(self.total_size.max(0) as u64);
        let seconds = self.elapsed.max(1) as f64 / 1000.0;
        let speed = ibytes((self.total_size.max(0) as f64 / seconds) as u64);
        let time = go_duration(self.elapsed);
        if self.kind == "S3toFS" && self.parts == 0 {
            format!("Transferred: {size}, Full file, Time: {time}, Speed: {speed}/s")
        } else {
            format!(
                "Transferred: {size}, Parts: {}, Time: {time}, Speed: {speed}/s",
                self.parts
            )
        }
    }
}

/// mc `odSetSizes`: `(combined size or -1 for "rest of stream", part size, parts, skip bytes)`.
fn upload_sizes(source_size: i64, ops: &Operands) -> (i64, u64, i64, i64) {
    let mut parts = ops.parts.unwrap_or(0);
    let skip_parts = ops.skip.unwrap_or(0);
    let Some(part_size) = ops.size else {
        if parts <= 1 {
            return (source_size, source_size as u64, 1, 0);
        }
        let part_size = (source_size as u64).div_ceil(parts as u64);
        return (
            -1,
            part_size,
            parts - skip_parts,
            skip_parts * part_size as i64,
        );
    };
    let skip = skip_parts * part_size as i64;
    if source_size == 0 {
        if parts == 0 {
            parts = 1;
        }
        return ((part_size as i64) * parts, part_size, parts, skip);
    }
    if part_size > source_size as u64 {
        return (source_size, source_size as u64, 1, 0);
    }
    let total_parts = (source_size as u64).div_ceil(part_size) as i64;
    if parts < 1 {
        return (source_size, part_size, total_parts - skip_parts, skip);
    }
    let mut combined = part_size as i64 * parts;
    if source_size - skip < combined {
        combined = source_size - skip;
        parts = total_parts - skip_parts;
    }
    (combined, part_size, parts, skip)
}

enum Endpoint {
    S3 {
        alias: Box<crate::config::model::AliasConfig>,
        bucket: String,
        key: String,
        display: String,
    },
    Local(PathBuf),
}

impl Endpoint {
    fn resolve(store: &ConfigStore, input: &str) -> Result<Self> {
        match parse_location(input, store.config()) {
            Location::S3(target) => {
                let alias = Box::new(alias_config(store, &target.alias)?);
                let bucket = target.require_bucket()?.to_string();
                let key = target.require_object_key()?;
                Ok(Endpoint::S3 {
                    display: format!("{}/{bucket}/{key}", target.alias),
                    alias,
                    bucket,
                    key,
                })
            }
            Location::Local(path) => Ok(Endpoint::Local(path)),
        }
    }

    fn display(&self) -> String {
        match self {
            Endpoint::S3 { display, .. } => display.clone(),
            Endpoint::Local(path) => path.display().to_string(),
        }
    }

    /// mc prints a local source as an absolute path.
    fn source_display(&self) -> String {
        match self {
            Endpoint::Local(path) => crate::error::abs_path(&path.to_string_lossy()),
            _ => self.display(),
        }
    }

    fn is_s3(&self) -> bool {
        matches!(self, Endpoint::S3 { .. })
    }
}

pub fn run(args: OdArgs, json: bool) -> Result<()> {
    let ops = parse_operands(&args.operands)?;
    let store = ConfigStore::load_or_create()?;
    let empty_path = || McError::new("Invalid path, path cannot be empty.");
    if ops.input.is_empty() {
        return Err(empty_path()).context("Unable to guess copy URL type.");
    }
    // mc `guessCopyURLType` stats the source first.
    let kind = crate::commands::util::stat_target(&store, &ops.input)
        .context("Unable to guess copy URL type.")?;
    let urls_error = "Unable to get source and target URLs";
    if kind == TargetKind::Folder {
        return Err(anyhow!(
            "invalid source path {}, source cannot be a directory",
            ops.input
        ))
        .context(urls_error);
    }
    if ops.output.is_empty() {
        return Err(empty_path()).context("Unable to initialize target client.");
    }
    let source = Endpoint::resolve(&store, &ops.input)?;
    let target = Endpoint::resolve(&store, &ops.output)?;
    if let Endpoint::Local(path) = &target
        && (path.is_dir() || ops.output.ends_with('/'))
    {
        return Err(anyhow!(
            "invalid source path {}, destination cannot be a directory",
            ops.output
        ))
        .context(urls_error);
    }
    let rt = runtime()?;
    let message = match (&source, &target) {
        (Endpoint::S3 { .. }, Endpoint::Local(path)) => rt
            .block_on(download(&ops, &source, path))
            .context("Unable to transfer object")?,
        _ => rt.block_on(copy(&ops, &source, &target))?,
    };
    if json {
        print_json(&message)
    } else {
        println!("{}", message.text());
        Ok(())
    }
}

async fn copy(ops: &Operands, source: &Endpoint, target: &Endpoint) -> Result<OdMessage> {
    let kind = match (source.is_s3(), target.is_s3()) {
        (false, true) => "FStoS3",
        (true, true) => "S3toS3",
        _ => "FStoFS",
    };
    let source_size = match source {
        Endpoint::Local(path) => std::fs::metadata(path)
            .with_context(|| format!("Unable to read `{}`", path.display()))?
            .len() as i64,
        Endpoint::S3 {
            alias, bucket, key, ..
        } => {
            let client = crate::s3::build_client(alias).await?;
            crate::s3::head_object_with(&client, bucket, key, &GetOptions::default())
                .await?
                .content_length()
                .unwrap_or(0)
        }
    };
    let (combined, part_size, parts, skip) = upload_sizes(source_size, ops);
    if skip < 0 || (source_size > 0 && skip > source_size) {
        bail!("skip is beyond the end of the source");
    }

    let start = Instant::now();
    let reader: Box<dyn AsyncRead + Unpin + Send> = match source {
        Endpoint::Local(path) => {
            let mut file = tokio::fs::File::open(path).await?;
            file.seek(std::io::SeekFrom::Start(skip as u64)).await?;
            Box::new(file)
        }
        Endpoint::S3 {
            alias, bucket, key, ..
        } => {
            let client = crate::s3::build_client(alias).await?;
            let options = GetOptions {
                range: (skip > 0).then_some((skip as u64, None)),
                ..Default::default()
            };
            let output = crate::s3::get_object(&client, bucket, key, &options).await?;
            Box::new(output.body.into_async_read())
        }
    };
    let reader: Box<dyn AsyncRead + Unpin + Send> = if combined > 0 {
        Box::new(reader.take(combined as u64))
    } else {
        reader
    };

    let total = match target {
        Endpoint::S3 {
            alias, bucket, key, ..
        } => {
            let client = crate::s3::build_client(alias).await?;
            let disable_multipart = combined > 0 && combined < MIN_PART_SIZE as i64;
            let fixed =
                if ops.size.is_some() || (MIN_PART_SIZE..=MAX_PART_SIZE).contains(&part_size) {
                    Some(part_size)
                } else {
                    None
                };
            let options = PutOptions {
                disable_multipart,
                part_size: if disable_multipart { None } else { fixed },
                ..Default::default()
            };
            let size_hint = (combined > 0).then_some(combined as u64);
            crate::s3::upload_stream(&client, bucket, key, reader, size_hint, &options)
                .await
                .context("Unable to upload")?
                .size
                .unwrap_or(0)
        }
        Endpoint::Local(path) => write_file(path, reader).await?,
    };
    Ok(OdMessage {
        status: "success",
        kind,
        source: source.source_display(),
        target: target.display(),
        part_size,
        total_size: total,
        parts,
        skip: if part_size > 0 {
            skip / part_size as i64
        } else {
            0
        },
        elapsed: start.elapsed().as_millis() as u64,
    })
}

async fn download(ops: &Operands, source: &Endpoint, path: &Path) -> Result<OdMessage> {
    if ops.size.is_some() {
        bail!("size cannot be specified getting from server");
    }
    let parts = ops.parts.unwrap_or(0);
    let skip = ops.skip.unwrap_or(0);
    if ops.parts.is_some() && parts < 1 {
        bail!("parts must be at least 1");
    }
    if ops.skip.is_some() && ops.parts.is_none() {
        bail!("skip requires parts");
    }
    let Endpoint::S3 {
        alias, bucket, key, ..
    } = source
    else {
        unreachable!("download source is always S3");
    };
    let client = crate::s3::build_client(alias).await?;
    let start = Instant::now();
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("Unable to create `{}`", path.display()))?;
    let numbers: Vec<Option<i32>> = if parts == 0 {
        vec![None]
    } else {
        (1 + skip..=parts).map(|n| Some(n as i32)).collect()
    };
    let mut total = 0;
    for part_number in numbers {
        let options = GetOptions {
            part_number,
            ..Default::default()
        };
        let output = crate::s3::get_object(&client, bucket, key, &options).await?;
        let mut body = output.body.into_async_read();
        total += crate::transfer::copy_to_file(&mut body, &mut file).await? as i64;
    }
    Ok(OdMessage {
        status: "success",
        kind: "S3toFS",
        source: source.display(),
        target: path.display().to_string(),
        part_size: 0,
        total_size: total,
        parts,
        skip,
        elapsed: start.elapsed().as_millis() as u64,
    })
}

async fn write_file(path: &Path, mut reader: impl AsyncRead + Unpin) -> Result<i64> {
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("Unable to create `{}`", path.display()))?;
    let total = crate::transfer::copy_to_file(&mut reader, &mut file).await?;
    Ok(total as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn ops(size: Option<u64>, parts: Option<i64>, skip: Option<i64>) -> Operands {
        Operands {
            input: "a".into(),
            output: "b".into(),
            size,
            parts,
            skip,
        }
    }

    #[test]
    fn parses_operands() {
        let parsed = parse_operands(&[
            "if=file.txt".into(),
            "of=play/b/o".into(),
            "size=40MiB".into(),
            "parts=5".into(),
            "skip=1".into(),
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Operands {
                input: "file.txt".into(),
                output: "play/b/o".into(),
                size: Some(40 * MIB),
                parts: Some(5),
                skip: Some(1),
            }
        );
        assert_eq!(parse_operands(&["if=a".into()]).unwrap().output, "");
        assert!(parse_operands(&["if=a".into(), "of=b".into(), "bs=1".into()]).is_err());
        assert!(parse_operands(&["if=a".into(), "of".into()]).is_err());
        assert!(parse_operands(&["if=a".into(), "of=b".into(), "parts=x".into()]).is_err());
    }

    #[test]
    fn formats_like_go_humanize() {
        assert_eq!(ibytes(5), "5 B");
        assert_eq!(ibytes(512), "512 B");
        assert_eq!(ibytes(3 * MIB), "3.0 MiB");
        assert_eq!(ibytes(25 * MIB), "25 MiB");
        assert_eq!(ibytes(1536), "1.5 KiB");
        assert_eq!(go_duration(0), "0s");
        assert_eq!(go_duration(12), "12ms");
        assert_eq!(go_duration(1500), "1.5s");
        assert_eq!(go_duration(2000), "2s");
        assert_eq!(go_duration(123_004), "2m3.004s");
        assert_eq!(go_duration(3_600_000), "1h0m0s");
    }

    #[test]
    fn computes_upload_sizes_like_mc() {
        let src = 10 * MIB as i64;
        // Whole file.
        assert_eq!(
            upload_sizes(src, &ops(None, None, None)),
            (src, src as u64, 1, 0)
        );
        // parts only: part size from source, rest of stream.
        assert_eq!(
            upload_sizes(src, &ops(None, Some(5), Some(1))),
            (-1, 2 * MIB, 4, 2 * MIB as i64)
        );
        // size only: all parts.
        assert_eq!(
            upload_sizes(src, &ops(Some(3 * MIB), None, None)),
            (src, 3 * MIB, 4, 0)
        );
        // size + parts.
        assert_eq!(
            upload_sizes(src, &ops(Some(2 * MIB), Some(3), None)),
            (6 * MIB as i64, 2 * MIB, 3, 0)
        );
        // size + parts beyond the source are clamped.
        assert_eq!(
            upload_sizes(src, &ops(Some(4 * MIB), Some(5), Some(1))),
            (6 * MIB as i64, 4 * MIB, 2, 4 * MIB as i64)
        );
        // Part size larger than the source.
        assert_eq!(
            upload_sizes(src, &ops(Some(20 * MIB), Some(2), None)),
            (src, src as u64, 1, 0)
        );
        // Empty source (e.g. /dev/zero): size * parts.
        assert_eq!(
            upload_sizes(0, &ops(Some(MIB), Some(3), None)),
            (3 * MIB as i64, MIB, 3, 0)
        );
    }

    #[test]
    fn formats_messages_like_mc() {
        let message = OdMessage {
            status: "success",
            kind: "FStoS3",
            source: "a".into(),
            target: "b".into(),
            part_size: MIB,
            total_size: 3 * MIB as i64,
            parts: 3,
            skip: 0,
            elapsed: 1500,
        };
        assert_eq!(
            message.text(),
            "Transferred: 3.0 MiB, Parts: 3, Time: 1.5s, Speed: 2.0 MiB/s"
        );
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["type"], "FStoS3");
        assert_eq!(json["partSize"], MIB);
        assert_eq!(json["totalSize"], 3 * MIB);
        let full = OdMessage {
            kind: "S3toFS",
            parts: 0,
            ..message
        };
        assert!(full.text().contains("Full file"));
    }
}
