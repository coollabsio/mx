//! Reusable clap flag groups and parsers shared by many commands.
//!
//! Embed a group in a command's Args with `#[command(flatten)]`, e.g.
//!
//! ```ignore
//! #[derive(Debug, clap::Args)]
//! pub struct CopyArgs {
//!     #[command(flatten)]
//!     pub time: crate::flags::TimeFilterFlags,
//!     #[command(flatten)]
//!     pub enc: crate::flags::EncFlags,
//!     pub source: String,
//!     pub target: String,
//! }
//! ```

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use clap::Args;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Single positional `TARGET` argument shared by simple commands/subcommands.
#[derive(Debug, Clone, Args)]
pub struct TargetArg {
    pub target: String,
}

// ---------------------------------------------------------------------------
// argv preprocessing
// ---------------------------------------------------------------------------

/// Rewrites mc-style multi-character short flags that clap cannot express.
///
/// * `-vid` / `-vid=X` -> `--version-id` / `--version-id=X`
/// * `-sc` / `-sc=X` -> `--storage-class` / `--storage-class=X`
///
/// Only standalone tokens are rewritten; everything after a bare `--` is left untouched.
pub fn rewrite_argv<I, T>(args: I) -> Vec<std::ffi::OsString>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    const REWRITES: &[(&str, &str)] = &[("-vid", "--version-id"), ("-sc", "--storage-class")];
    let mut out = Vec::new();
    let mut passthrough = false;
    for arg in args {
        let arg: std::ffi::OsString = arg.into();
        if passthrough {
            out.push(arg);
            continue;
        }
        let Some(text) = arg.to_str() else {
            out.push(arg);
            continue;
        };
        if text == "--" {
            passthrough = true;
            out.push(arg);
            continue;
        }
        let mut replaced = None;
        for (short, long) in REWRITES {
            if text == *short {
                replaced = Some(long.to_string());
            } else if let Some(value) = text.strip_prefix(&format!("{short}=")) {
                replaced = Some(format!("{long}={value}"));
            }
        }
        out.push(replaced.map(Into::into).unwrap_or(arg));
    }
    out
}

// ---------------------------------------------------------------------------
// durations and timestamps
// ---------------------------------------------------------------------------

/// Parses mc duration strings such as `7d10h31s`, `1h`, `30m`, `90s`, `2w`.
///
/// Units: `w` (weeks), `d` (days), `h`, `m`, `s`. A bare number is rejected.
pub fn parse_duration(input: &str) -> Result<Duration> {
    let text = input.trim();
    if text.is_empty() {
        bail!("invalid duration `{input}`");
    }
    let mut total: u64 = 0;
    let mut digits = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        if digits.is_empty() {
            bail!("invalid duration `{input}`");
        }
        let count: u64 = digits
            .parse()
            .with_context(|| format!("invalid duration `{input}`"))?;
        let unit = match ch.to_ascii_lowercase() {
            'w' => 7 * 86_400,
            'd' => 86_400,
            'h' => 3_600,
            'm' => 60,
            's' => 1,
            _ => bail!("invalid duration `{input}`: unknown unit `{ch}`"),
        };
        total = count
            .checked_mul(unit)
            .and_then(|value| total.checked_add(value))
            .ok_or_else(|| anyhow!("duration `{input}` is too large"))?;
        digits.clear();
    }
    if !digits.is_empty() {
        bail!("invalid duration `{input}`: missing unit");
    }
    Ok(Duration::from_secs(total))
}

/// Parses an RFC3339 timestamp (`2024-01-02T03:04:05Z`, offsets allowed) or the mc short
/// form `2006.01.02T15:04` / `2006.01.02T15:04:05` (interpreted as UTC).
pub fn parse_timestamp(input: &str) -> Result<SystemTime> {
    let text = input.trim();
    if let Ok(parsed) = aws_sdk_s3::primitives::DateTime::from_str(
        text,
        aws_sdk_s3::primitives::DateTimeFormat::DateTimeWithOffset,
    ) {
        return SystemTime::try_from(parsed).map_err(|_| anyhow!("invalid timestamp `{input}`"));
    }
    parse_dotted_timestamp(text).ok_or_else(|| {
        anyhow!("invalid time `{input}`: use a duration like `1d`, RFC3339, or `2006.01.02T15:04`")
    })
}

fn parse_dotted_timestamp(text: &str) -> Option<SystemTime> {
    let (date, time) = text.split_once('T')?;
    let mut date_parts = date.split('.');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let mut time_parts = time.split(':');
    let hour: u64 = time_parts.next()?.parse().ok()?;
    let minute: u64 = time_parts.next()?.parse().ok()?;
    let second: u64 = match time_parts.next() {
        Some(value) => value.parse().ok()?,
        None => 0,
    };
    if time_parts.next().is_some()
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + (hour * 3600 + minute * 60 + second) as i64;
    if secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(secs as u64))
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i64;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `--older-than` / `--newer-than` filters.
#[derive(Debug, Clone, Default, Args)]
pub struct TimeFilterFlags {
    /// filter objects older than value in duration string (e.g. 7d10h31s)
    #[arg(long, value_name = "DURATION")]
    pub older_than: Option<String>,
    /// filter objects newer than value in duration string (e.g. 7d10h31s)
    #[arg(long, value_name = "DURATION")]
    pub newer_than: Option<String>,
}

impl TimeFilterFlags {
    /// Validates and parses both durations.
    pub fn parsed(&self) -> Result<(Option<Duration>, Option<Duration>)> {
        Ok((
            self.older_than.as_deref().map(parse_duration).transpose()?,
            self.newer_than.as_deref().map(parse_duration).transpose()?,
        ))
    }

    pub fn is_set(&self) -> bool {
        self.older_than.is_some() || self.newer_than.is_some()
    }

    /// mc semantics: `--older-than X` keeps objects whose age >= X,
    /// `--newer-than X` keeps objects whose age <= X. Unknown modification time never matches
    /// when a filter is set. Invalid duration strings are treated as "no match"; call
    /// [`TimeFilterFlags::parsed`] first to surface parse errors.
    pub fn matches(&self, last_modified: Option<SystemTime>, now: SystemTime) -> bool {
        if !self.is_set() {
            return true;
        }
        let Ok((older, newer)) = self.parsed() else {
            return false;
        };
        let Some(modified) = last_modified else {
            return false;
        };
        let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
        older.is_none_or(|limit| age >= limit) && newer.is_none_or(|limit| age <= limit)
    }
}

/// `--rewind` accepts a duration (now minus duration) or a timestamp.
#[derive(Debug, Clone, Default, Args)]
pub struct RewindFlag {
    /// roll back object(s) to current version at specified time (duration like 1d or date)
    #[arg(long, value_name = "TIME")]
    pub rewind: Option<String>,
}

impl RewindFlag {
    pub fn at(&self, now: SystemTime) -> Result<Option<SystemTime>> {
        self.rewind
            .as_deref()
            .map(|value| parse_rewind(value, now))
            .transpose()
    }
}

/// Parses an mc `--rewind` value relative to `now`.
pub fn parse_rewind(value: &str, now: SystemTime) -> Result<SystemTime> {
    if let Ok(duration) = parse_duration(value) {
        return now
            .checked_sub(duration)
            .ok_or_else(|| anyhow!("rewind `{value}` is out of range"));
    }
    parse_timestamp(value)
}

/// `--version-id` (also `--vid`, and `-vid` via [`rewrite_argv`]).
#[derive(Debug, Clone, Default, Args)]
pub struct VersionIdFlag {
    /// select an object version
    #[arg(long = "version-id", visible_alias = "vid", value_name = "VERSION_ID")]
    pub version_id: Option<String>,
}

/// `--versions`.
#[derive(Debug, Clone, Default, Args)]
pub struct VersionsFlag {
    /// include all object versions
    #[arg(long)]
    pub versions: bool,
}

// ---------------------------------------------------------------------------
// metadata
// ---------------------------------------------------------------------------

/// `--attr`, `--tags`, `--storage-class` (also `-sc` via [`rewrite_argv`]).
#[derive(Debug, Clone, Default, Args)]
pub struct MetadataFlags {
    /// add custom metadata for the object, e.g. "key1=value1;key2=value2"
    #[arg(long, value_name = "KEY=VALUE;...")]
    pub attr: Option<String>,
    /// apply tags to the uploaded object(s), e.g. "key1=value1&key2=value2"
    #[arg(long, value_name = "KEY=VALUE&...")]
    pub tags: Option<String>,
    /// set storage class for new object(s) on target
    #[arg(long = "storage-class", value_name = "CLASS")]
    pub storage_class: Option<String>,
}

impl MetadataFlags {
    pub fn attr_pairs(&self) -> Result<Vec<(String, String)>> {
        self.attr
            .as_deref()
            .map(parse_attr)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub fn tag_pairs(&self) -> Result<Vec<(String, String)>> {
        self.tags
            .as_deref()
            .map(parse_tags)
            .transpose()
            .map(Option::unwrap_or_default)
    }
}

/// Parses mc `--attr` values: `key1=value1;key2=value2`.
pub fn parse_attr(input: &str) -> Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    for part in input.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| anyhow!("metadata `{part}` must use key=value"))?;
        let key = key.trim();
        if key.is_empty() {
            bail!("metadata key cannot be empty");
        }
        pairs.push((key.to_string(), value.to_string()));
    }
    Ok(pairs)
}

/// Parses tag strings: `key1=value1&key2=value2`. At least one tag is required.
pub fn parse_tags(input: &str) -> Result<Vec<(String, String)>> {
    let mut tags = Vec::new();
    for part in input.split('&') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| anyhow!("tag `{part}` must use key=value"))?;
        if key.is_empty() {
            bail!("tag key cannot be empty");
        }
        tags.push((key.to_string(), value.to_string()));
    }
    if tags.is_empty() {
        bail!("at least one tag is required");
    }
    Ok(tags)
}

// ---------------------------------------------------------------------------
// encryption
// ---------------------------------------------------------------------------

/// Server-side encryption selection for one object.
#[derive(Clone, PartialEq, Eq)]
pub enum Sse {
    /// SSE-C with a 32-byte customer key.
    C { key: [u8; 32] },
    /// SSE-S3 (AES256).
    S3,
    /// SSE-KMS with the given key ID.
    Kms { key_id: String },
}

impl std::fmt::Debug for Sse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Sse::C { .. } => f.write_str("Sse::C { key: <redacted> }"),
            Sse::S3 => f.write_str("Sse::S3"),
            Sse::Kms { key_id } => write!(f, "Sse::Kms {{ key_id: {key_id:?} }}"),
        }
    }
}

impl Sse {
    pub fn customer_key(&self) -> Option<[u8; 32]> {
        match self {
            Sse::C { key } => Some(*key),
            _ => None,
        }
    }
}

/// `--enc-c`, `--enc-s3`, `--enc-kms`.
#[derive(Debug, Clone, Default, Args)]
pub struct EncFlags {
    /// encrypt/decrypt objects using client provided keys: "ALIAS/BUCKET/PREFIX=KEY" (repeatable, comma-separated)
    #[arg(long = "enc-c", value_name = "PATH=KEY")]
    pub enc_c: Vec<String>,
    /// encrypt objects using server-side default keys: "ALIAS/BUCKET/PREFIX" (repeatable, comma-separated)
    #[arg(long = "enc-s3", value_name = "PATH")]
    pub enc_s3: Vec<String>,
    /// encrypt objects using KMS keys: "ALIAS/BUCKET/PREFIX=KMS-KEY-ID" (repeatable, comma-separated)
    #[arg(long = "enc-kms", value_name = "PATH=KEY_ID")]
    pub enc_kms: Vec<String>,
}

impl EncFlags {
    /// All configured `(prefix, Sse)` pairs, including env `MC_ENC_S3` / `MC_ENC_KMS`.
    pub fn entries(&self) -> Result<Vec<(String, Sse)>> {
        let env_s3 = std::env::var("MC_ENC_S3").ok();
        let env_kms = std::env::var("MC_ENC_KMS").ok();
        self.entries_with_env(env_s3.as_deref(), env_kms.as_deref())
    }

    fn entries_with_env(
        &self,
        env_s3: Option<&str>,
        env_kms: Option<&str>,
    ) -> Result<Vec<(String, Sse)>> {
        let mut entries = Vec::new();
        for value in self.enc_c.iter().flat_map(|v| split_list(v)) {
            let (path, key) = value.split_once('=').ok_or_else(|| {
                anyhow!("`--enc-c` value `{value}` must be ALIAS/BUCKET/PREFIX=KEY")
            })?;
            entries.push((
                normalize_enc_path(path)?,
                Sse::C {
                    key: parse_sse_c_key(key)?,
                },
            ));
        }
        let s3_values = self
            .enc_s3
            .iter()
            .map(String::as_str)
            .chain(env_s3)
            .flat_map(split_list);
        for value in s3_values {
            entries.push((normalize_enc_path(value)?, Sse::S3));
        }
        let kms_values = self
            .enc_kms
            .iter()
            .map(String::as_str)
            .chain(env_kms)
            .flat_map(split_list);
        for value in kms_values {
            let (path, key_id) = value.split_once('=').ok_or_else(|| {
                anyhow!("`--enc-kms` value `{value}` must be ALIAS/BUCKET/PREFIX=KMS-KEY-ID")
            })?;
            if key_id.trim().is_empty() {
                bail!("`--enc-kms` value `{value}` is missing a key ID");
            }
            entries.push((
                normalize_enc_path(path)?,
                Sse::Kms {
                    key_id: key_id.trim().to_string(),
                },
            ));
        }
        Ok(entries)
    }

    /// Picks the SSE setting whose prefix is the longest prefix of `full_path`
    /// (`ALIAS/BUCKET/KEY`). Returns `None` when nothing matches.
    pub fn resolve(&self, full_path: &str) -> Result<Option<Sse>> {
        Ok(resolve_sse(&self.entries()?, full_path))
    }

    pub fn is_empty(&self) -> bool {
        self.enc_c.is_empty() && self.enc_s3.is_empty() && self.enc_kms.is_empty()
    }
}

/// Longest-prefix match over pre-parsed entries (see [`EncFlags::entries`]).
pub fn resolve_sse(entries: &[(String, Sse)], full_path: &str) -> Option<Sse> {
    let path = full_path.trim_start_matches('/');
    entries
        .iter()
        .filter(|(prefix, _)| path.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, sse)| sse.clone())
}

fn split_list(value: &str) -> Vec<&str> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

fn normalize_enc_path(path: &str) -> Result<String> {
    let path = path.trim().trim_start_matches('/');
    if path.is_empty() {
        bail!("encryption path must be ALIAS/BUCKET[/PREFIX]");
    }
    Ok(path.to_string())
}

/// Parses an SSE-C key: 32 bytes as base64 (standard, padded or raw) or 64 hex characters.
pub fn parse_sse_c_key(input: &str) -> Result<[u8; 32]> {
    let input = input.trim();
    let bytes = if input.len() == 64 && input.chars().all(|c| c.is_ascii_hexdigit()) {
        (0..32)
            .map(|i| u8::from_str_radix(&input[i * 2..i * 2 + 2], 16))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(input)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(input))
            .map_err(|_| anyhow!("SSE-C key must be 32 bytes in base64 or 64 hex characters"))?
    };
    bytes
        .try_into()
        .map_err(|_| anyhow!("SSE-C key must be exactly 32 bytes"))
}

// ---------------------------------------------------------------------------
// checksum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgo {
    Crc64Nvme,
    Crc32,
    Crc32c,
    Sha1,
    Sha256,
}

impl ChecksumAlgo {
    pub fn as_str(self) -> &'static str {
        match self {
            ChecksumAlgo::Crc64Nvme => "CRC64NVME",
            ChecksumAlgo::Crc32 => "CRC32",
            ChecksumAlgo::Crc32c => "CRC32C",
            ChecksumAlgo::Sha1 => "SHA1",
            ChecksumAlgo::Sha256 => "SHA256",
        }
    }

    pub fn to_sdk(self) -> aws_sdk_s3::types::ChecksumAlgorithm {
        use aws_sdk_s3::types::ChecksumAlgorithm as A;
        match self {
            ChecksumAlgo::Crc64Nvme => A::Crc64Nvme,
            ChecksumAlgo::Crc32 => A::Crc32,
            ChecksumAlgo::Crc32c => A::Crc32C,
            ChecksumAlgo::Sha1 => A::Sha1,
            ChecksumAlgo::Sha256 => A::Sha256,
        }
    }
}

impl std::str::FromStr for ChecksumAlgo {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "CRC64NVME" => Ok(ChecksumAlgo::Crc64Nvme),
            "CRC32" => Ok(ChecksumAlgo::Crc32),
            "CRC32C" => Ok(ChecksumAlgo::Crc32c),
            "SHA1" => Ok(ChecksumAlgo::Sha1),
            "SHA256" => Ok(ChecksumAlgo::Sha256),
            _ => bail!("invalid checksum `{value}`: use CRC64NVME, CRC32, CRC32C, SHA1, or SHA256"),
        }
    }
}

fn parse_checksum_arg(value: &str) -> Result<ChecksumAlgo, String> {
    value
        .parse()
        .map_err(|error: anyhow::Error| error.to_string())
}

/// `--checksum CRC64NVME|CRC32|CRC32C|SHA1|SHA256` (case-insensitive).
#[derive(Debug, Clone, Default, Args)]
pub struct ChecksumFlag {
    /// add checksum to uploaded object: CRC64NVME, CRC32, CRC32C, SHA1, SHA256
    #[arg(long, value_name = "ALGO", value_parser = parse_checksum_arg)]
    pub checksum: Option<ChecksumAlgo>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn rewrites_mc_multichar_short_flags() {
        let out = rewrite_argv(["mx", "cp", "-vid", "abc", "-sc=REDUCED", "a", "--", "-sc"]);
        let out: Vec<_> = out.iter().map(|s| s.to_str().unwrap()).collect();
        assert_eq!(
            out,
            [
                "mx",
                "cp",
                "--version-id",
                "abc",
                "--storage-class=REDUCED",
                "a",
                "--",
                "-sc"
            ]
        );
        let out = rewrite_argv(["mx", "stat", "-vid=v1", "-s", "--vid", "x"]);
        let out: Vec<_> = out.iter().map(|s| s.to_str().unwrap()).collect();
        assert_eq!(out, ["mx", "stat", "--version-id=v1", "-s", "--vid", "x"]);
    }

    #[test]
    fn parses_mc_durations() {
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(
            parse_duration("2w").unwrap(),
            Duration::from_secs(14 * 86400)
        );
        assert_eq!(
            parse_duration("7d10h31s").unwrap(),
            Duration::from_secs(7 * 86400 + 10 * 3600 + 31)
        );
        assert!(parse_duration("").is_err());
        assert!(parse_duration("10").is_err());
        assert!(parse_duration("h").is_err());
        assert!(parse_duration("5y").is_err());
    }

    #[test]
    fn time_filter_semantics() {
        let now = UNIX_EPOCH + Duration::from_secs(10 * 86400);
        let two_days_old = Some(now - Duration::from_secs(2 * 86400));
        let older = TimeFilterFlags {
            older_than: Some("1d".into()),
            newer_than: None,
        };
        assert!(older.matches(two_days_old, now));
        assert!(!older.matches(Some(now), now));
        let newer = TimeFilterFlags {
            older_than: None,
            newer_than: Some("1d".into()),
        };
        assert!(!newer.matches(two_days_old, now));
        assert!(newer.matches(Some(now - Duration::from_secs(60)), now));
        assert!(TimeFilterFlags::default().matches(None, now));
        assert!(!older.matches(None, now));
        let both = TimeFilterFlags {
            older_than: Some("1d".into()),
            newer_than: Some("3d".into()),
        };
        assert!(both.matches(two_days_old, now));
    }

    #[test]
    fn parses_rewind_values() {
        let now = UNIX_EPOCH + Duration::from_secs(100 * 86400);
        assert_eq!(
            parse_rewind("1d", now).unwrap(),
            now - Duration::from_secs(86400)
        );
        let expected = UNIX_EPOCH + Duration::from_secs(1_704_164_645); // 2024-01-02T03:04:05Z
        assert_eq!(parse_rewind("2024-01-02T03:04:05Z", now).unwrap(), expected);
        assert_eq!(
            parse_rewind("2024-01-02T05:04:05+02:00", now).unwrap(),
            expected
        );
        assert_eq!(
            parse_rewind("2024.01.02T03:04", now).unwrap(),
            expected - Duration::from_secs(5)
        );
        assert_eq!(parse_rewind("2024.01.02T03:04:05", now).unwrap(), expected);
        assert!(parse_rewind("yesterday", now).is_err());
    }

    #[test]
    fn parses_attr_and_tags() {
        assert_eq!(
            parse_attr("Cache-Control=max-age=90;key2=v2;").unwrap(),
            vec![
                ("Cache-Control".to_string(), "max-age=90".to_string()),
                ("key2".to_string(), "v2".to_string())
            ]
        );
        assert!(parse_attr("novalue").is_err());
        assert_eq!(
            parse_tags("env=prod&team=core").unwrap(),
            vec![
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "core".to_string())
            ]
        );
        assert!(parse_tags("").is_err());
    }

    #[test]
    fn parses_sse_c_keys() {
        let hex = "00".repeat(31) + "ff";
        assert_eq!(parse_sse_c_key(&hex).unwrap()[31], 0xff);
        let b64 = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        assert_eq!(parse_sse_c_key(&b64).unwrap(), [7u8; 32]);
        assert_eq!(
            parse_sse_c_key(b64.trim_end_matches('=')).unwrap(),
            [7u8; 32]
        );
        assert!(parse_sse_c_key("short").is_err());
    }

    #[test]
    fn resolves_longest_enc_prefix() {
        let key = base64::engine::general_purpose::STANDARD.encode([1u8; 32]);
        let flags = EncFlags {
            enc_c: vec![format!("play/bucket/secret={key}")],
            enc_s3: vec!["play/bucket".into()],
            enc_kms: vec!["play/other=my-key,play/bucket/kms/deep=k2".into()],
        };
        let entries = flags.entries_with_env(None, None).unwrap();
        assert_eq!(
            resolve_sse(&entries, "play/bucket/secret/a.txt"),
            Some(Sse::C { key: [1u8; 32] })
        );
        assert_eq!(resolve_sse(&entries, "play/bucket/x"), Some(Sse::S3));
        assert_eq!(
            resolve_sse(&entries, "play/bucket/kms/deep/x"),
            Some(Sse::Kms {
                key_id: "k2".into()
            })
        );
        assert_eq!(
            resolve_sse(&entries, "play/other/x"),
            Some(Sse::Kms {
                key_id: "my-key".into()
            })
        );
        assert_eq!(resolve_sse(&entries, "local/bucket/x"), None);

        let env_entries = EncFlags::default()
            .entries_with_env(Some("local/b1"), Some("local/b2=kid"))
            .unwrap();
        assert_eq!(resolve_sse(&env_entries, "local/b1/k"), Some(Sse::S3));
        assert_eq!(
            resolve_sse(&env_entries, "local/b2/k"),
            Some(Sse::Kms {
                key_id: "kid".into()
            })
        );
        assert!(
            EncFlags {
                enc_c: vec!["play/b=bad".into()],
                ..Default::default()
            }
            .entries_with_env(None, None)
            .is_err()
        );
    }

    #[test]
    fn parses_checksum_algorithms() {
        assert_eq!(
            "crc32c".parse::<ChecksumAlgo>().unwrap(),
            ChecksumAlgo::Crc32c
        );
        assert_eq!(
            "CRC64NVME".parse::<ChecksumAlgo>().unwrap(),
            ChecksumAlgo::Crc64Nvme
        );
        assert!("md5".parse::<ChecksumAlgo>().is_err());
    }

    #[derive(Debug, Parser)]
    struct Harness {
        #[command(flatten)]
        time: TimeFilterFlags,
        #[command(flatten)]
        rewind: RewindFlag,
        #[command(flatten)]
        vid: VersionIdFlag,
        #[command(flatten)]
        versions: VersionsFlag,
        #[command(flatten)]
        meta: MetadataFlags,
        #[command(flatten)]
        enc: EncFlags,
        #[command(flatten)]
        checksum: ChecksumFlag,
    }

    #[test]
    fn flag_groups_flatten_into_clap() {
        let args = rewrite_argv([
            "x",
            "--older-than",
            "1d",
            "--rewind",
            "2h",
            "-vid",
            "v1",
            "--versions",
            "--attr",
            "a=b",
            "--tags",
            "t=1",
            "-sc",
            "STANDARD",
            "--enc-s3",
            "play/b",
            "--enc-s3",
            "play/c",
            "--checksum",
            "sha256",
        ]);
        let parsed = Harness::try_parse_from(args).unwrap();
        assert_eq!(parsed.time.older_than.as_deref(), Some("1d"));
        assert_eq!(parsed.rewind.rewind.as_deref(), Some("2h"));
        assert_eq!(parsed.vid.version_id.as_deref(), Some("v1"));
        assert!(parsed.versions.versions);
        assert_eq!(parsed.meta.storage_class.as_deref(), Some("STANDARD"));
        assert_eq!(parsed.enc.enc_s3.len(), 2);
        assert_eq!(parsed.checksum.checksum, Some(ChecksumAlgo::Sha256));
        let parsed = Harness::try_parse_from(["x", "--vid", "v2"]).unwrap();
        assert_eq!(parsed.vid.version_id.as_deref(), Some("v2"));
        assert!(Harness::try_parse_from(["x", "--checksum", "md5"]).is_err());
    }
}
