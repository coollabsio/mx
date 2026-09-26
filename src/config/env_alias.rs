//! Aliases from the environment, like mc: `MC_HOST_<alias>=https://ACCESS:SECRET[:TOKEN]@HOST`
//! and `MC_CONFIG_ENV_FILE` (a file of `MC_HOST_<alias>=URL` lines).

use crate::config::model::AliasConfig;
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub const HOST_ENV_PREFIX: &str = "MC_HOST_";
pub const CONFIG_ENV_FILE: &str = "MC_CONFIG_ENV_FILE";

const INVALID_ARGUMENT: &str =
    "Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.";

static ENV_FILE_ALIASES: OnceLock<BTreeMap<String, AliasConfig>> = OnceLock::new();

/// Parses an `MC_HOST_*` value like mc `parseEnvURLStr` + `expandAliasFromEnv`.
///
/// Like mc, credentials are taken literally (no percent-decoding) when the value has the
/// `scheme://ACCESS:SECRET@host` form; the host part is everything after the last `@`.
pub fn parse_env_url(value: &str) -> Result<AliasConfig> {
    // Go RE2 and Rust regex have the same leftmost-first semantics for these patterns.
    static KEYS: OnceLock<Regex> = OnceLock::new();
    static KEY_TOKENS: OnceLock<Regex> = OnceLock::new();
    let keys = KEYS.get_or_init(|| Regex::new("^(https?://)(.*?):(.*)@(.*?)$").unwrap());
    let key_tokens =
        KEY_TOKENS.get_or_init(|| Regex::new("^(https?://)(.*?):(.*?):(.*)@(.*?)$").unwrap());

    let (mut access_key, mut secret_key, mut session_token) = (String::new(), String::new(), None);
    let url_text = if let Some(caps) = key_tokens.captures(value) {
        access_key = caps[2].to_string();
        secret_key = caps[3].to_string();
        session_token = Some(caps[4].to_string()).filter(|token| !token.is_empty());
        format!("{}{}", &caps[1], &caps[5])
    } else if let Some(caps) = keys.captures(value) {
        access_key = caps[2].to_string();
        secret_key = caps[3].to_string();
        format!("{}{}", &caps[1], &caps[4])
    } else {
        value.to_string()
    };

    let parsed = url::Url::parse(&url_text).map_err(|_| anyhow::anyhow!(INVALID_ARGUMENT))?;
    let rest = url_text
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    let (host, path) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    // Drop userinfo that was not matched by the key patterns.
    let host = host.rsplit_once('@').map(|(_, host)| host).unwrap_or(host);
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none_or(str::is_empty)
        || !(path.is_empty() || path == "/")
    {
        bail!(INVALID_ARGUMENT);
    }
    if access_key.is_empty() && secret_key.is_empty() {
        // Userinfo without a `:` (e.g. `https://KEY@host`): Go decodes it via `url.User`.
        access_key = decode(parsed.username());
        secret_key = parsed.password().map(decode).unwrap_or_default();
    }

    Ok(AliasConfig {
        url: format!("{}://{host}{path}", parsed.scheme()),
        access_key,
        secret_key,
        session_token,
        api: "S3v4".to_string(),
        // mc leaves the lookup empty, which means `auto`.
        path: String::new(),
        src: Some("env".to_string()),
        ..Default::default()
    })
}

fn decode(text: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", text.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

/// All `MC_HOST_<alias>` variables, parsed. Invalid values are kept as errors so that using
/// that alias fails like mc.
pub fn host_env_aliases() -> Vec<(String, std::result::Result<AliasConfig, String>)> {
    std::env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            let alias = key.strip_prefix(HOST_ENV_PREFIX)?.to_string();
            if alias.is_empty() {
                return None;
            }
            let parsed = value
                .into_string()
                .map_err(|_| INVALID_ARGUMENT.to_string())
                .and_then(|value| parse_env_url(&value).map_err(|err| format!("{err:#}")));
            Some((alias, parsed))
        })
        .collect()
}

/// Parses `MC_CONFIG_ENV_FILE` contents: `MC_HOST_<alias>=URL` (or `<alias>=URL`) per line.
pub fn parse_env_file(text: &str, source: &str) -> Result<BTreeMap<String, AliasConfig>> {
    let mut aliases = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            bail!("parsing error at {line}");
        };
        let alias = key.strip_prefix(HOST_ENV_PREFIX).unwrap_or(key);
        if alias.is_empty() {
            bail!("parsing error at {line}");
        }
        let mut cfg = parse_env_url(value)?;
        cfg.src = Some(source.to_string());
        aliases.insert(alias.to_string(), cfg);
    }
    Ok(aliases)
}

/// Reads `MC_CONFIG_ENV_FILE` once (mc does this at startup and fails on errors).
pub fn load_env_file() -> Result<()> {
    let Some(path) = std::env::var_os(CONFIG_ENV_FILE) else {
        return Ok(());
    };
    let source = path.to_string_lossy().into_owned();
    let aliases = std::fs::read_to_string(&path)
        .map_err(anyhow::Error::from)
        .and_then(|text| parse_env_file(&text, &source))
        .with_context(|| format!("Unable to parse {source}"))?;
    let _ = ENV_FILE_ALIASES.set(aliases);
    Ok(())
}

/// Aliases loaded by [`load_env_file`] (empty if not loaded).
pub fn env_file_aliases() -> &'static BTreeMap<String, AliasConfig> {
    ENV_FILE_ALIASES.get_or_init(BTreeMap::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys_and_host() {
        let cfg = parse_env_url("https://AKIA:se/cr+et@play.min.io:9000").unwrap();
        assert_eq!(cfg.url, "https://play.min.io:9000");
        assert_eq!(cfg.access_key, "AKIA");
        assert_eq!(cfg.secret_key, "se/cr+et");
        assert_eq!(cfg.session_token, None);
        assert_eq!(cfg.api, "S3v4");
        assert_eq!(cfg.path, "");
        assert_eq!(cfg.src.as_deref(), Some("env"));
    }

    #[test]
    fn parses_session_token_and_trailing_slash() {
        let cfg = parse_env_url("http://a:b:tok@localhost:9000/").unwrap();
        assert_eq!(cfg.url, "http://localhost:9000/");
        assert_eq!(
            (cfg.access_key.as_str(), cfg.secret_key.as_str()),
            ("a", "b")
        );
        assert_eq!(cfg.session_token.as_deref(), Some("tok"));
    }

    #[test]
    fn keeps_encoded_and_at_signs_literally_like_mc() {
        // mc takes the host after the last `@` and does not percent-decode keys.
        let cfg = parse_env_url("https://key:p%2Fw@rd@example.com").unwrap();
        assert_eq!(cfg.secret_key, "p%2Fw@rd");
        assert_eq!(cfg.url, "https://example.com");
    }

    #[test]
    fn no_credentials_and_username_only() {
        let cfg = parse_env_url("https://example.com").unwrap();
        assert_eq!(
            (cfg.access_key.as_str(), cfg.url.as_str()),
            ("", "https://example.com")
        );
        let cfg = parse_env_url("https://us%40er@example.com").unwrap();
        assert_eq!(cfg.access_key, "us@er");
    }

    #[test]
    fn rejects_invalid_values() {
        for bad in [
            "ftp://a:b@host",
            "https://a:b@host/bucket",
            "https://a:b@host?x=1",
            "https://a:b@host#frag",
            "not a url",
            "",
        ] {
            assert!(parse_env_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn env_file_lines() {
        let aliases = parse_env_file(
            "MC_HOST_one=https://a:b@one.example\ntwo=http://c:d@two.example:9000\n",
            "/tmp/f",
        )
        .unwrap();
        assert_eq!(aliases["one"].url, "https://one.example");
        assert_eq!(aliases["two"].src.as_deref(), Some("/tmp/f"));
        assert!(parse_env_file("garbage\n", "f").is_err());
    }
}
