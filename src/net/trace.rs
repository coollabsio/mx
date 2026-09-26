//! `--debug` HTTP trace, modeled on mc's `traceV4`: request and response headers are dumped to
//! stderr with a `mx: <DEBUG>` prefix. Credentials, signatures, SSE-C keys and session tokens
//! are redacted.

use aws_smithy_runtime_api::http::Headers;

const REDACTED: &str = "**REDACTED**";

/// Headers whose values are never printed.
const SECRET_HEADERS: &[&str] = &[
    "x-amz-server-side-encryption-customer-key",
    "x-amz-copy-source-server-side-encryption-customer-key",
    "x-amz-security-token",
];

/// Formats a request like Go's `httputil.DumpRequestOut` (headers only).
pub fn format_request(method: &str, uri: &str, headers: &Headers) -> String {
    let (host, path) = match url::Url::parse(uri) {
        Ok(url) => {
            let host = match url.port() {
                Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
                None => url.host_str().unwrap_or_default().to_string(),
            };
            let path = match url.query() {
                Some(query) => format!("{}?{query}", url.path()),
                None => url.path().to_string(),
            };
            (host, path)
        }
        Err(_) => (String::new(), uri.to_string()),
    };
    let mut out = format!("{method} {path} HTTP/1.1\nHost: {host}\n");
    out.push_str(&format_headers(headers, true));
    out.push('\n');
    out
}

/// Formats a response status line and headers.
pub fn format_response(status: u16, headers: &Headers) -> String {
    let reason = http::StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or_default();
    let mut out = format!("HTTP/1.1 {status} {reason}\n");
    out.push_str(&format_headers(headers, false));
    out.push('\n');
    out
}

fn format_headers(headers: &Headers, request: bool) -> String {
    let mut lines: Vec<(String, String)> = headers
        .iter()
        .filter(|(name, _)| !(request && name.eq_ignore_ascii_case("host")))
        .map(|(name, value)| (canonical_name(name), redact(name, value)))
        .collect();
    lines.sort();
    lines
        .into_iter()
        .map(|(name, value)| format!("{name}: {value}\n"))
        .collect()
}

fn redact(name: &str, value: &str) -> String {
    if SECRET_HEADERS
        .iter()
        .any(|secret| name.eq_ignore_ascii_case(secret))
    {
        return REDACTED.to_string();
    }
    if name.eq_ignore_ascii_case("authorization") {
        return redact_authorization(value);
    }
    value.to_string()
}

/// `Credential=<access key>/...` and `Signature=<hex>` are replaced with `**REDACTED**`.
fn redact_authorization(value: &str) -> String {
    value
        .split(", ")
        .map(|part| {
            let (prefix, rest) = part.rsplit_once(' ').unwrap_or(("", part));
            let redacted = if let Some(credential) = rest.strip_prefix("Credential=") {
                match credential.split_once('/') {
                    Some((_, scope)) => format!("Credential={REDACTED}/{scope}"),
                    None => format!("Credential={REDACTED}"),
                }
            } else if rest.starts_with("Signature=") {
                format!("Signature={REDACTED}")
            } else {
                rest.to_string()
            };
            if prefix.is_empty() {
                redacted
            } else {
                format!("{prefix} {redacted}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `x-amz-date` -> `X-Amz-Date` (Go's canonical header form, as printed by mc).
fn canonical_name(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Writes one trace block to stderr.
pub fn print(block: &str) {
    eprint!("mx: <DEBUG> {block}");
}

#[cfg(test)]
mod tests {
    use super::{format_request, format_response};
    use aws_smithy_runtime_api::http::Headers;

    #[test]
    fn redacts_credentials_signature_and_secret_headers() {
        let mut headers = Headers::new();
        headers.insert(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20250101/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=abcdef0123",
        );
        headers.insert("x-amz-server-side-encryption-customer-key", "c2VjcmV0");
        headers.insert("x-amz-security-token", "token");
        headers.insert("x-amz-date", "20250101T000000Z");
        let out = format_request("GET", "https://example.com:9000/bucket/key?x=1", &headers);
        assert!(out.starts_with("GET /bucket/key?x=1 HTTP/1.1\nHost: example.com:9000\n"));
        assert!(out.contains(
            "Authorization: AWS4-HMAC-SHA256 Credential=**REDACTED**/20250101/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-date, Signature=**REDACTED**"
        ));
        assert!(out.contains("X-Amz-Server-Side-Encryption-Customer-Key: **REDACTED**"));
        assert!(out.contains("X-Amz-Security-Token: **REDACTED**"));
        assert!(out.contains("X-Amz-Date: 20250101T000000Z"));
        assert!(!out.contains("AKIAEXAMPLE"));
        assert!(!out.contains("abcdef0123"));
        assert!(!out.contains("c2VjcmV0"));
    }

    #[test]
    fn formats_response_status() {
        let mut headers = Headers::new();
        headers.insert("content-length", "0");
        let out = format_response(404, &headers);
        assert_eq!(out, "HTTP/1.1 404 Not Found\nContent-Length: 0\n\n");
    }
}
