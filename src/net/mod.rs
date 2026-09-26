//! Networking helpers behind the global flags: TLS trust (`--insecure`, `certs/CAs`),
//! connection deadlines, bandwidth limits (`--limit-upload/--limit-download`), the `--debug`
//! HTTP trace, and S3 Signature V2.
//! Wired into every S3 client by [`crate::s3::client::build_client`].

pub mod deadline;
pub mod sigv2;
pub mod throttle;
pub mod tls;
pub mod trace;
pub mod x509;

use anyhow::{Result, anyhow};

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
    use super::parse_custom_header;

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
