use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use crate::location::{Location, parse_location};
use crate::s3::ObjectInfo;
use crate::target::TargetRef;
use anyhow::{Result, bail};
use std::time::Duration;

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
                }]
            } else {
                crate::transfer::local_inventory(&path)?
                    .into_iter()
                    .map(|entry| crate::s3::ObjectInfo {
                        key: entry.relative,
                        size: entry.size as i64,
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

pub fn parse_tags(input: &str) -> Result<Vec<(String, String)>> {
    let mut tags = Vec::new();
    for part in input.split('&') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("tag `{part}` must use key=value"))?;
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

pub fn key_depth(key: &str) -> usize {
    key.trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
}

#[cfg(test)]
mod tests {
    use super::{glob_match, parse_expire, parse_tags};
    use std::time::Duration;

    #[test]
    fn parses_hour_duration() {
        assert_eq!(parse_expire("48h").unwrap(), Duration::from_secs(48 * 3600));
    }

    #[test]
    fn matches_glob_names() {
        assert!(glob_match("*.txt", "notes.txt"));
        assert!(!glob_match("*.txt", "notes.md"));
    }

    #[test]
    fn parses_ampersand_tags() {
        assert_eq!(
            parse_tags("env=prod&team=core").unwrap(),
            vec![
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "core".to_string())
            ]
        );
    }
}
