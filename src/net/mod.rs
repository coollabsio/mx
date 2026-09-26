//! Networking helpers behind the global flags: TLS trust (`--insecure`, `certs/CAs`),
//! bandwidth limits (`--limit-upload/--limit-download`) and the `--debug` HTTP trace.
//! Wired into every S3 client by [`crate::s3::client::build_client`].

pub mod throttle;
pub mod tls;
pub mod trace;

use anyhow::{Result, anyhow, bail};

/// Parses a byte size like mc (go-humanize `ParseBytes`): `1MiB`, `500 KiB`, `10MB`, `1.5k`,
/// `1024`. SI units are powers of 1000, IEC units powers of 1024; case-insensitive.
pub fn parse_bytes(value: &str) -> Result<u64> {
    let digits = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(value.len());
    let number: f64 = value[..digits]
        .replace(',', "")
        .parse()
        .map_err(|_| anyhow!("invalid size `{value}`"))?;
    let unit = value[digits..].trim().to_ascii_lowercase();
    let multiplier: u64 = match unit.as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1000,
        "ki" | "kib" => 1 << 10,
        "m" | "mb" => 1000u64.pow(2),
        "mi" | "mib" => 1 << 20,
        "g" | "gb" => 1000u64.pow(3),
        "gi" | "gib" => 1 << 30,
        "t" | "tb" => 1000u64.pow(4),
        "ti" | "tib" => 1 << 40,
        "p" | "pb" => 1000u64.pow(5),
        "pi" | "pib" => 1 << 50,
        "e" | "eb" => 1000u64.pow(6),
        "ei" | "eib" => 1 << 60,
        _ => bail!("unhandled size name `{unit}` in `{value}`"),
    };
    let bytes = number * multiplier as f64;
    if bytes >= u64::MAX as f64 {
        bail!("size `{value}` is too large");
    }
    Ok(bytes as u64)
}

/// Parses a `-H/--custom-header` value in `key:value` form (both sides trimmed).
pub fn parse_custom_header(value: &str) -> Result<(String, String)> {
    let invalid = || anyhow!("invalid custom header entry `{value}`, expected `key:value`");
    let (name, header_value) = value.split_once(':').ok_or_else(invalid)?;
    let (name, header_value) = (name.trim(), header_value.trim());
    if name.is_empty()
        || http::HeaderName::from_bytes(name.as_bytes()).is_err()
        || http::HeaderValue::from_str(header_value).is_err()
    {
        return Err(invalid());
    }
    Ok((name.to_string(), header_value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{parse_bytes, parse_custom_header};

    #[test]
    fn parses_humanized_sizes() {
        assert_eq!(parse_bytes("1024").unwrap(), 1024);
        assert_eq!(parse_bytes("1MiB").unwrap(), 1 << 20);
        assert_eq!(parse_bytes("500 KiB").unwrap(), 500 * 1024);
        assert_eq!(parse_bytes("10MB").unwrap(), 10_000_000);
        assert_eq!(parse_bytes("1.5k").unwrap(), 1500);
        assert_eq!(parse_bytes("2gib").unwrap(), 2 << 30);
        assert_eq!(parse_bytes("1,000b").unwrap(), 1000);
        assert!(parse_bytes("fast").is_err());
        assert!(parse_bytes("10 parsecs").is_err());
        assert!(parse_bytes("").is_err());
    }

    #[test]
    fn parses_custom_headers() {
        assert_eq!(
            parse_custom_header(" X-Test : a:b ").unwrap(),
            ("X-Test".to_string(), "a:b".to_string())
        );
        assert!(parse_custom_header("novalue").is_err());
        assert!(parse_custom_header(":v").is_err());
        assert!(parse_custom_header("bad header:v").is_err());
        assert!(parse_custom_header("x:bad\nvalue").is_err());
    }
}
