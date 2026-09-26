use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use crate::error::McError;
use crate::location::{Location, parse_location};
use crate::s3::ObjectInfo;
use crate::target::TargetRef;
use anyhow::{Result, bail};
use std::time::Duration;

pub use crate::flags::parse_tags;

pub fn parse_expire(input: &str) -> Result<Duration> {
    let input = input.trim();
    if input.len() < 2 {
        bail!("invalid duration `{input}`");
    }
    let (digits, unit) = input.split_at(input.len() - 1);
    let count: u64 = digits.parse()?;
    Ok(match unit {
        "s" => Duration::from_secs(count),
        "m" => Duration::from_secs(count * 60),
        "h" => Duration::from_secs(count * 3600),
        "d" => Duration::from_secs(count * 86400),
        _ => bail!("invalid duration `{input}`"),
    })
}

pub fn require_s3(store: &ConfigStore, input: &str) -> Result<(AliasConfig, TargetRef)> {
    match parse_location(input, store.config()) {
        Location::S3(target) => {
            let alias = super::alias_config(store, &target.alias)?;
            Ok((alias, target))
        }
        // mc: "No valid configuration found" for an unknown alias.
        Location::Local(_) => {
            let alias = input.split('/').next().unwrap_or(input);
            Err(McError::new(format!(
                "No valid configuration found for '{alias}' host alias."
            ))
            .into())
        }
    }
}

pub fn object_infos(store: &ConfigStore, input: &str) -> Result<(String, Vec<ObjectInfo>)> {
    match parse_location(input, store.config()) {
        Location::S3(target) => {
            let alias = super::alias_config(store, &target.alias)?;
            let bucket = target.require_bucket()?.to_string();
            let prefix = target.key_with_trailing_slash();
            let items = super::runtime()?.block_on(crate::s3::list_object_infos(
                &alias,
                &bucket,
                prefix.as_deref(),
            ))?;
            Ok((input.to_string(), items))
        }
        Location::Local(path) => {
            let root = if path.is_file() {
                vec![crate::s3::ObjectInfo {
                    key: path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    size: path.metadata()?.len() as i64,
                    is_latest: true,
                    ..Default::default()
                }]
            } else {
                crate::transfer::local_inventory(&path)?
                    .into_iter()
                    .map(|entry| crate::s3::ObjectInfo {
                        key: entry.relative,
                        size: entry.size as i64,
                        is_latest: true,
                        ..Default::default()
                    })
                    .collect()
            };
            Ok((input.to_string(), root))
        }
    }
}

/// What [`stat_target`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Folder,
    File,
}

/// mc `url2Stat` existence check for a local path or `ALIAS[/BUCKET[/KEY]]`, with mc's errors:
/// a missing local path (or unknown alias) is ``Requested path `ABS` not found``, a missing
/// bucket ``Bucket `b` does not exist.``, a missing key `Object does not exist`.
pub fn stat_target(store: &ConfigStore, input: &str) -> Result<TargetKind> {
    let target = match parse_location(input, store.config()) {
        Location::Local(path) => {
            let meta = std::fs::metadata(&path)
                .map_err(|error| crate::error::io_error(&error, &path.to_string_lossy()))?;
            return Ok(if meta.is_dir() {
                TargetKind::Folder
            } else {
                TargetKind::File
            });
        }
        Location::S3(target) => target,
    };
    let alias = super::alias_config(store, &target.alias)?;
    let Some(bucket) = target.bucket.clone() else {
        return Ok(TargetKind::Folder);
    };
    super::runtime()?.block_on(async {
        let client = crate::s3::build_client(&alias).await?;
        let Some(key) = target.key_with_trailing_slash() else {
            let head = client.head_bucket().bucket(&bucket).send().await;
            return match head {
                Ok(_) => Ok(TargetKind::Folder),
                Err(error) => {
                    let error = crate::s3::s3_object_error(&error, &bucket, "");
                    if crate::error::is_not_found(&error) {
                        Err(McError::bucket_not_found(&bucket).into())
                    } else {
                        Err(error)
                    }
                }
            };
        };
        if !key.ends_with('/') {
            match crate::s3::stat_object(&client, &bucket, &key, None).await {
                Ok(_) => return Ok(TargetKind::File),
                Err(error)
                    if !crate::error::is_not_found(&error)
                        || crate::error::error_code(&error) == Some("NoSuchBucket") =>
                {
                    return Err(error);
                }
                Err(_) => {}
            }
        }
        let prefix = if key.ends_with('/') {
            key.clone()
        } else {
            format!("{key}/")
        };
        if crate::s3::prefix_exists(&client, &bucket, &prefix).await? {
            Ok(TargetKind::Folder)
        } else {
            Err(McError::object_missing().into())
        }
    })
}

pub fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_bytes(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_bytes(pattern: &[u8], text: &[u8]) -> bool {
    match (pattern.first(), text.first()) {
        (Some(&b'*'), _) => {
            glob_match_bytes(&pattern[1..], text)
                || (!text.is_empty() && glob_match_bytes(pattern, &text[1..]))
        }
        (Some(&b'?'), Some(_)) => glob_match_bytes(&pattern[1..], &text[1..]),
        (Some(left), Some(right)) if left == right => glob_match_bytes(&pattern[1..], &text[1..]),
        (None, None) => true,
        _ => false,
    }
}

/// Formats bytes like go-humanize `IBytes`: `9 B`, `1.0 KiB`, `16 MiB`.
pub fn humanize_ibytes(size: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if size < 10 {
        return format!("{size} B");
    }
    let exponent = ((size as f64).ln() / 1024_f64.ln()).floor() as usize;
    let exponent = exponent.min(UNITS.len() - 1);
    let value = (size as f64 / 1024_f64.powi(exponent as i32) * 10.0 + 0.5).floor() / 10.0;
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[exponent])
    } else {
        format!("{value:.0} {}", UNITS[exponent])
    }
}

/// Formats a time like mc's `printDate` layout (`2006-01-02 15:04:05 MST`), in UTC.
pub fn format_print_time(time: std::time::SystemTime) -> String {
    let rendered = crate::s3::from_system_time(time)
        .fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
        .unwrap_or_default();
    let base = rendered.get(..19).unwrap_or(&rendered).replace('T', " ");
    format!("{base} UTC")
}

pub fn key_depth(key: &str) -> usize {
    key.trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
}

#[cfg(test)]
mod tests {
    use super::{format_print_time, glob_match, humanize_ibytes, parse_expire};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn formats_iec_sizes() {
        assert_eq!(humanize_ibytes(0), "0 B");
        assert_eq!(humanize_ibytes(9), "9 B");
        assert_eq!(humanize_ibytes(11), "11 B");
        assert_eq!(humanize_ibytes(1024), "1.0 KiB");
        assert_eq!(humanize_ibytes(1536), "1.5 KiB");
        assert_eq!(humanize_ibytes(16 << 20), "16 MiB");
        assert_eq!(humanize_ibytes(5 << 30), "5.0 GiB");
    }

    #[test]
    fn formats_print_time() {
        let time = UNIX_EPOCH + Duration::from_millis(1_704_164_645_250);
        assert_eq!(format_print_time(time), "2024-01-02 03:04:05 UTC");
    }

    #[test]
    fn parses_hour_duration() {
        assert_eq!(parse_expire("48h").unwrap(), Duration::from_secs(48 * 3600));
    }

    #[test]
    fn matches_glob_names() {
        assert!(glob_match("*.txt", "notes.txt"));
        assert!(!glob_match("*.txt", "notes.md"));
    }
}
