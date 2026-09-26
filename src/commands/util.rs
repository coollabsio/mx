use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
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
        Location::Local(_) => bail!("target `{input}` is not an S3 alias path"),
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

/// Parses human sizes like go-humanize `ParseBytes`: `100`, `64MB` (10^6), `16MiB` (2^20),
/// `1.5GiB`, `5 k`. Units are case-insensitive; a trailing `b` is optional.
pub fn parse_size(input: &str) -> Result<u64> {
    let text = input.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(text.len());
    let number: f64 = text[..split]
        .replace(',', "")
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid size `{input}`"))?;
    let unit = text[split..].trim().to_ascii_lowercase();
    let multiplier: u64 = match unit.as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1000,
        "ki" | "kib" => 1 << 10,
        "m" | "mb" => 1000_u64.pow(2),
        "mi" | "mib" => 1 << 20,
        "g" | "gb" => 1000_u64.pow(3),
        "gi" | "gib" => 1 << 30,
        "t" | "tb" => 1000_u64.pow(4),
        "ti" | "tib" => 1 << 40,
        "p" | "pb" => 1000_u64.pow(5),
        "pi" | "pib" => 1 << 50,
        "e" | "eb" => 1000_u64.pow(6),
        "ei" | "eib" => 1 << 60,
        _ => bail!("invalid size `{input}`: unknown unit `{unit}`"),
    };
    let value = number * multiplier as f64;
    if !value.is_finite() || value >= u64::MAX as f64 {
        bail!("size `{input}` is too large");
    }
    Ok(value as u64)
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
    use super::{format_print_time, glob_match, humanize_ibytes, parse_expire, parse_size};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn parses_human_sizes() {
        assert_eq!(parse_size("100").unwrap(), 100);
        assert_eq!(parse_size("16MiB").unwrap(), 16 << 20);
        assert_eq!(parse_size("64MB").unwrap(), 64_000_000);
        assert_eq!(parse_size("64mb").unwrap(), 64_000_000);
        assert_eq!(parse_size("5 KiB").unwrap(), 5 * 1024);
        assert_eq!(parse_size("1k").unwrap(), 1000);
        assert_eq!(parse_size("1.5GiB").unwrap(), 3 << 29);
        assert_eq!(parse_size("1,000b").unwrap(), 1000);
        assert!(parse_size("").is_err());
        assert!(parse_size("MiB").is_err());
        assert!(parse_size("10 parsecs").is_err());
        assert!(parse_size("100EiB").is_err());
    }

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
