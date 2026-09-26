//! Maps AWS SDK errors to mc's errors (see [`crate::error`]).
//!
//! The server response is decoded like minio-go's `httpRespToErrorResponse` (XML body, MinIO
//! `x-minio-error-*` headers, status-code fallbacks for bodiless HEAD responses). Like mc,
//! most operations report the server's message with the full minio-go `ErrorResponse` as
//! detail ([`S3ResultExt::s3`]). Object data operations (HEAD/GET/PUT/COPY object, mc
//! `getObjectStat`/`Get`/`Put`/`Copy`) apply mc's translations ([`S3ResultExt::s3_object`]):
//! `NoSuchBucket` → ``Bucket `b` does not exist.``, `NoSuchKey` → `Object does not exist`,
//! `InvalidBucketName` → ``Bucket name `b` not valid.``.

use super::bucket::XmlNode;
use crate::error::{Detail, McError};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_smithy_runtime_api::http::Response as HttpResponse;

/// minio-go `ErrorResponse`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    pub bucket_name: String,
    pub key: String,
    pub resource: String,
    pub request_id: String,
    pub host_id: String,
    pub region: String,
    pub server: String,
}

impl ErrorResponse {
    /// Decodes an error response like minio-go `httpRespToErrorResponse`.
    pub fn from_http(
        status: u16,
        header: impl Fn(&str) -> Option<String>,
        body: &[u8],
        bucket: &str,
        key: &str,
    ) -> Self {
        let parsed = std::str::from_utf8(body)
            .ok()
            .filter(|text| !text.trim().is_empty())
            .and_then(|text| XmlNode::parse(text).ok())
            .filter(|node| node.name == "Error");
        let mut resp = match parsed {
            Some(node) => {
                let text = |name: &str| node.child_text(name).unwrap_or_default().to_string();
                Self {
                    code: text("Code"),
                    message: text("Message"),
                    bucket_name: text("BucketName"),
                    key: text("Key"),
                    resource: text("Resource"),
                    request_id: text("RequestId"),
                    host_id: text("HostId"),
                    region: text("Region"),
                    ..Self::default()
                }
            }
            None => {
                let (code, message, with_key) = match status {
                    404 if key.is_empty() => (
                        "NoSuchBucket",
                        "The specified bucket does not exist.",
                        false,
                    ),
                    404 => ("NoSuchKey", "The specified key does not exist.", true),
                    403 => ("AccessDenied", "Access Denied.", true),
                    409 => ("Conflict", "Bucket not empty.", false),
                    412 => (
                        "PreconditionFailed",
                        "At least one of the pre-conditions you specified did not hold",
                        true,
                    ),
                    _ => ("", "", false),
                };
                let (code, message) = if code.is_empty() {
                    let status_text = format!(
                        "{status} {}",
                        http::StatusCode::from_u16(status)
                            .ok()
                            .and_then(|s| s.canonical_reason())
                            .unwrap_or_default()
                    );
                    let body = String::from_utf8_lossy(body).into_owned();
                    let message = if body.is_empty() {
                        status_text.clone()
                    } else {
                        body.chars().take(1024).collect()
                    };
                    (status_text, message)
                } else {
                    (code.to_string(), message.to_string())
                };
                Self {
                    code,
                    message,
                    bucket_name: bucket.to_string(),
                    key: if with_key {
                        key.to_string()
                    } else {
                        String::new()
                    },
                    ..Self::default()
                }
            }
        };
        resp.server = header("Server").unwrap_or_default();
        if let Some(code) = header("x-minio-error-code").filter(|c| !c.is_empty()) {
            resp.code = code;
        }
        if let Some(desc) = header("x-minio-error-desc").filter(|d| !d.is_empty()) {
            resp.message = desc.trim_matches('"').to_string();
        }
        if resp.request_id.is_empty() {
            resp.request_id = header("x-amz-request-id").unwrap_or_default();
        }
        if resp.host_id.is_empty() {
            resp.host_id = header("x-amz-id-2").unwrap_or_default();
        }
        if resp.region.is_empty() {
            resp.region = header("x-amz-bucket-region").unwrap_or_default();
        }
        if resp.code == "InvalidRegion" && !resp.region.is_empty() {
            resp.message = format!("Region does not match, expecting region ‘{}’.", resp.region);
        }
        resp
    }

    /// The error as Go marshals it (`cause.error`).
    pub fn detail(&self) -> Detail {
        crate::detail![
            ("Code", self.code),
            ("Message", self.message),
            ("BucketName", self.bucket_name),
            ("Key", self.key),
            ("Resource", self.resource),
            ("RequestID", self.request_id),
            ("HostID", self.host_id),
            ("Region", self.region),
            ("Server", self.server),
        ]
    }

    /// The server error as mc reports it without translation.
    pub fn to_raw_error(&self) -> McError {
        McError::with_detail(self.message.clone(), self.detail()).with_code(&self.code)
    }

    /// mc's translation of the server error for an object operation on `bucket`.
    pub fn to_mc_error(&self, bucket: &str) -> McError {
        let bucket = if bucket.is_empty() {
            self.bucket_name.as_str()
        } else {
            bucket
        };
        match self.code.as_str() {
            "NoSuchBucket" => McError::bucket_not_found(bucket),
            "NoSuchKey" => McError::object_missing(),
            "InvalidBucketName" => McError::bucket_invalid(bucket),
            _ => self.to_raw_error(),
        }
    }
}

/// Converts an SDK error for an operation on `bucket`/`key` into mc's error: the server's
/// message (minio-go `ErrorResponse`).
pub fn map_sdk_error<E>(err: &SdkError<E, HttpResponse>, bucket: &str, key: &str) -> McError
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    match error_response(err, bucket, key) {
        Some(resp) => resp.to_raw_error(),
        None => McError::new(transport_message(err)),
    }
}

/// [`map_sdk_error`] for object data operations, with mc's translations.
pub fn map_sdk_object_error<E>(err: &SdkError<E, HttpResponse>, bucket: &str, key: &str) -> McError
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    match error_response(err, bucket, key) {
        Some(resp) => resp.to_mc_error(bucket),
        None => McError::new(transport_message(err)),
    }
}

/// The server's error response, decoded like minio-go (None without a response).
pub fn error_response<E>(
    err: &SdkError<E, HttpResponse>,
    bucket: &str,
    key: &str,
) -> Option<ErrorResponse>
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    let raw = err.raw_response()?;
    let headers = raw.headers();
    let header = |name: &str| headers.get(name).map(str::to_string);
    let body = raw.body().bytes().unwrap_or_default();
    let mut resp = ErrorResponse::from_http(raw.status().as_u16(), header, body, bucket, key);
    // Streamed bodies are not buffered; fall back to the SDK's parsed metadata.
    if let Some(service) = err.as_service_error()
        && resp.code.chars().next().is_some_and(|c| c.is_ascii_digit())
        && let Some(code) = service.code()
    {
        resp.code = code.to_string();
        resp.message = service.message().unwrap_or(code).to_string();
    }
    Some(resp)
}

/// minio-go `s3utils.CheckValidBucketNameStrict` (checked before `MakeBucket`).
pub fn check_bucket_name_strict(bucket: &str) -> Result<(), McError> {
    let invalid = |text: &str| Err(McError::new(text).with_code("InvalidBucketName"));
    if bucket.trim().is_empty() {
        return invalid("Bucket name cannot be empty");
    }
    if bucket.len() < 3 {
        return invalid("Bucket name cannot be shorter than 3 characters");
    }
    if bucket.len() > 63 {
        return invalid("Bucket name cannot be longer than 63 characters");
    }
    let parts: Vec<&str> = bucket.split('.').collect();
    if parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return invalid("Bucket name cannot be an ip address");
    }
    let edge = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let valid = bucket.starts_with(edge)
        && bucket.ends_with(edge)
        && bucket.chars().all(|c| edge(c) || c == '.' || c == '-')
        && !bucket.contains("..")
        && !bucket.contains(".-")
        && !bucket.contains("-.");
    if !valid {
        return invalid("Bucket name contains invalid characters");
    }
    Ok(())
}

/// A transport failure the way Go's `net/http` prints it: `Get "URL": dial tcp HOST:PORT:
/// connect: connection refused`.
pub fn go_transport_error(
    method: &str,
    url: &str,
    err: &(dyn std::error::Error + 'static),
) -> McError {
    let reason = transport_message(err);
    let detail = if reason.starts_with("connect:") {
        let addr = url::Url::parse(url)
            .ok()
            .and_then(|u| Some(format!("{}:{}", u.host_str()?, u.port_or_known_default()?)))
            .unwrap_or_default();
        format!("dial tcp {addr}: {reason}")
    } else {
        reason
    };
    McError::new(format!("{method} \"{url}\": {detail}"))
}

/// Text for errors without a server response (connection failures, timeouts, ...): the
/// innermost cause, which is what Go's net errors end with.
fn transport_message(err: &(dyn std::error::Error + 'static)) -> String {
    let mut current: &(dyn std::error::Error + 'static) = err;
    let mut text = current.to_string();
    while let Some(source) = current.source() {
        current = source;
        text = current.to_string();
    }
    if text.contains("UnknownIssuer") {
        return "tls: failed to verify certificate: x509: certificate signed by unknown authority"
            .to_string();
    }
    if let Some(io) = find_io_error(err) {
        return match io.kind() {
            std::io::ErrorKind::ConnectionRefused => "connect: connection refused".to_string(),
            std::io::ErrorKind::ConnectionReset => "connection reset by peer".to_string(),
            std::io::ErrorKind::TimedOut => "i/o timeout".to_string(),
            _ => io.to_string(),
        };
    }
    text
}

fn find_io_error<'a>(err: &'a (dyn std::error::Error + 'static)) -> Option<&'a std::io::Error> {
    let mut current = Some(err);
    while let Some(err) = current {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return Some(io);
        }
        current = err.source();
    }
    None
}

/// [`map_sdk_error`] as an anyhow error.
pub fn s3_error<E>(err: &SdkError<E, HttpResponse>, bucket: &str, key: &str) -> anyhow::Error
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    map_sdk_error(err, bucket, key).into()
}

/// [`map_sdk_object_error`] as an anyhow error.
pub fn s3_object_error<E>(err: &SdkError<E, HttpResponse>, bucket: &str, key: &str) -> anyhow::Error
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    map_sdk_object_error(err, bucket, key).into()
}

/// `.s3(bucket, key)` / `.s3_object(bucket, key)` on SDK results.
pub trait S3ResultExt<T> {
    /// Maps the error with [`map_sdk_error`] (server message).
    fn s3(self, bucket: &str, key: &str) -> anyhow::Result<T>;
    /// Maps the error with [`map_sdk_object_error`] (mc's object-operation translations).
    fn s3_object(self, bucket: &str, key: &str) -> anyhow::Result<T>;
}

impl<T, E> S3ResultExt<T> for Result<T, SdkError<E, HttpResponse>>
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
{
    fn s3(self, bucket: &str, key: &str) -> anyhow::Result<T> {
        self.map_err(|err| map_sdk_error(&err, bucket, key).into())
    }

    fn s3_object(self, bucket: &str, key: &str) -> anyhow::Result<T> {
        self.map_err(|err| map_sdk_object_error(&err, bucket, key).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_headers(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn decodes_xml_error_body() {
        let body = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>BucketAlreadyOwnedByYou</Code><Message>Your previous request to create the named bucket succeeded and you already own it.</Message><BucketName>b</BucketName><Resource>/b/</Resource><RequestId>ABC</RequestId><HostId>h</HostId></Error>";
        let header = |name: &str| (name == "Server").then(|| "MinIO".to_string());
        let resp = ErrorResponse::from_http(409, header, body, "b", "");
        assert_eq!(resp.code, "BucketAlreadyOwnedByYou");
        assert_eq!(resp.resource, "/b/");
        assert_eq!(resp.server, "MinIO");
        let err = resp.to_mc_error("b");
        assert_eq!(
            err.message,
            "Your previous request to create the named bucket succeeded and you already own it."
        );
        assert_eq!(err.detail.get("Code").unwrap(), "BucketAlreadyOwnedByYou");
        assert_eq!(err.detail.get("RequestID").unwrap(), "ABC");
        assert_eq!(err.code.as_deref(), Some("BucketAlreadyOwnedByYou"));
    }

    #[test]
    fn validates_bucket_names_like_minio_go() {
        let text = |name: &str| check_bucket_name_strict(name).unwrap_err().message;
        assert_eq!(text("a"), "Bucket name cannot be shorter than 3 characters");
        assert_eq!(text("Bad_Name"), "Bucket name contains invalid characters");
        assert_eq!(text("1.2.3.4"), "Bucket name cannot be an ip address");
        assert_eq!(text("a..b"), "Bucket name contains invalid characters");
        assert!(check_bucket_name_strict("my-bucket.1").is_ok());
    }

    #[test]
    fn maps_head_responses_by_status_and_minio_headers() {
        let resp = ErrorResponse::from_http(404, no_headers, b"", "b", "k");
        assert_eq!(resp.to_mc_error("b").message, "Object does not exist");
        let resp = ErrorResponse::from_http(404, no_headers, b"", "b", "");
        assert_eq!(resp.to_mc_error("b").message, "Bucket `b` does not exist.");
        let minio = |name: &str| (name == "x-minio-error-code").then(|| "NoSuchBucket".into());
        let resp = ErrorResponse::from_http(404, minio, b"", "nob", "k");
        let err = resp.to_mc_error("nob");
        assert_eq!(err.message, "Bucket `nob` does not exist.");
        assert_eq!(err.detail.get("Bucket").unwrap(), "nob");
        let resp = ErrorResponse::from_http(403, no_headers, b"", "b", "k");
        assert_eq!(resp.to_mc_error("b").message, "Access Denied.");
        let resp = ErrorResponse::from_http(501, no_headers, b"", "b", "");
        assert_eq!(resp.to_mc_error("b").message, "501 Not Implemented");
    }
}
