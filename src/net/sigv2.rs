//! AWS Signature Version 2, ported from minio-go `pkg/signer/request-signature-v2.go`
//! (aliases with `api: S3v2`): header auth (`Authorization: AWS <key>:<sig>`), presigned
//! query auth (`AWSAccessKeyId`, `Expires`, `Signature`) and POST policy signatures.
//!
//! [`SigV2AuthScheme`] plugs the header signer into the SDK in place of SigV4.

use aws_credential_types::Credentials;
use aws_lc_rs::hmac;
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::auth::{
    AuthScheme, AuthSchemeEndpointConfig, AuthSchemeId, Sign,
};
use aws_smithy_runtime_api::client::identity::{Identity, SharedIdentityResolver};
use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
use aws_smithy_runtime_api::client::runtime_components::{GetIdentityResolver, RuntimeComponents};
use aws_smithy_types::config_bag::ConfigBag;
use base64::Engine;
use std::time::SystemTime;

/// Sub-resources that are part of the canonical resource (minio-go `resourceList`, sorted).
const RESOURCE_LIST: &[&str] = &[
    "acl",
    "cors",
    "delete",
    "encryption",
    "legal-hold",
    "lifecycle",
    "location",
    "logging",
    "notification",
    "partNumber",
    "policy",
    "replication",
    "requestPayment",
    "response-cache-control",
    "response-content-disposition",
    "response-content-encoding",
    "response-content-language",
    "response-content-type",
    "response-expires",
    "retention",
    "select",
    "select-type",
    "tagging",
    "torrent",
    "uploadId",
    "uploads",
    "versionId",
    "versioning",
    "versions",
    "website",
];

/// The parts of a request that SigV2 signs.
#[derive(Debug, Clone, Default)]
pub struct SignableV2<'a> {
    pub method: &'a str,
    /// Request host (`host[:port]`), used to find the bucket of virtual-host requests.
    pub host: &'a str,
    /// Percent-encoded request path.
    pub path: &'a str,
    /// Raw query string (without `?`).
    pub query: &'a str,
    /// Request headers (any case, repeated names allowed).
    pub headers: Vec<(&'a str, &'a str)>,
    /// Virtual-host style request: the bucket is the first label of `host`.
    pub virtual_host: bool,
}

impl SignableV2<'_> {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
            .unwrap_or_default()
    }
}

/// minio-go `stringToSignV2` (`date_or_expires` is the `Date` header, or `Expires` when
/// presigning).
pub fn string_to_sign(request: &SignableV2<'_>, date_or_expires: &str) -> String {
    let mut out = format!(
        "{}\n{}\n{}\n{date_or_expires}\n",
        request.method,
        request.header("content-md5"),
        request.header("content-type"),
    );
    // Canonicalized x-amz headers: lower-case names, sorted, repeated values joined by `,`.
    let mut amz: Vec<(String, Vec<&str>)> = Vec::new();
    for (name, value) in &request.headers {
        let lower = name.to_ascii_lowercase();
        if !lower.starts_with("x-amz") {
            continue;
        }
        match amz.iter_mut().find(|(n, _)| *n == lower) {
            Some((_, values)) => values.push(value),
            None => amz.push((lower, vec![value])),
        }
    }
    amz.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, values) in amz {
        out.push_str(&format!("{name}:{}\n", values.join(",")));
    }
    out.push_str(&canonical_resource(request));
    out
}

/// minio-go `writeCanonicalizedResource`: `[/bucket]/path[?sub-resources]`.
fn canonical_resource(request: &SignableV2<'_>) -> String {
    let mut path = percent_decode(request.path);
    if request.virtual_host
        && let Some((bucket, _)) = request.host.split_once('.')
    {
        path = format!("/{bucket}{path}");
    }
    let mut out = encode_path(&path);
    let query = parse_query(request.query);
    let mut first = true;
    for resource in RESOURCE_LIST {
        let Some((_, value)) = query.iter().find(|(k, _)| k == resource) else {
            continue;
        };
        out.push(if first { '?' } else { '&' });
        first = false;
        out.push_str(resource);
        if !value.is_empty() {
            out.push('=');
            out.push_str(value);
        }
    }
    out
}

/// Base64 HMAC-SHA1 of `data` (minio-go signatures and `PostPresignSignatureV2`).
pub fn signature(secret_key: &str, data: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret_key.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hmac::sign(&key, data.as_bytes()))
}

/// `Date` header value (Go `http.TimeFormat`).
pub fn http_date(time: SystemTime) -> String {
    aws_smithy_types::DateTime::from(time)
        .fmt(aws_smithy_types::date_time::Format::HttpDate)
        .unwrap_or_default()
}

/// minio-go `SignV2`: the `Authorization` header value for a request carrying `Date`.
pub fn authorization(request: &SignableV2<'_>, access_key: &str, secret_key: &str) -> String {
    let to_sign = string_to_sign(request, request.header("date"));
    format!("AWS {access_key}:{}", signature(secret_key, &to_sign))
}

/// minio-go `PreSignV2`: the presigned query string (existing query parameters, then
/// `AWSAccessKeyId` (`GoogleAccessId` for GCS) and `Expires`, sorted, then `Signature`).
pub fn presign_query(
    request: &SignableV2<'_>,
    access_key: &str,
    secret_key: &str,
    expires_epoch: u64,
) -> String {
    let expires = expires_epoch.to_string();
    let to_sign = string_to_sign(request, &expires);
    let signature = signature(secret_key, &to_sign);
    let id_name = if request.host.contains(".storage.googleapis.com") {
        "GoogleAccessId"
    } else {
        "AWSAccessKeyId"
    };
    let mut query = parse_query(request.query);
    query.push((id_name.to_string(), access_key.to_string()));
    query.push(("Expires".to_string(), expires));
    query.sort_by(|a, b| a.0.cmp(&b.0));
    let encoded: Vec<String> = query
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                encode_path(k).replace('/', "%2F"),
                encode_path(v).replace('/', "%2F")
            )
        })
        .collect();
    format!(
        "{}&Signature={}",
        encoded.join("&"),
        encode_path(&signature)
    )
}

/// minio-go `s3utils.EncodePath`: keeps `A-Za-z0-9-_.~/`, percent-encodes other UTF-8 bytes.
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
            && let Ok(value) = u8::from_str_radix(&input[i + 1..i + 3], 16)
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Go `url.ParseQuery` (first value per key kept by callers): `+` is a space.
fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (
                percent_decode(&key.replace('+', " ")),
                percent_decode(&value.replace('+', " ")),
            )
        })
        .collect()
}

const SIGV4_SCHEME_ID: AuthSchemeId = AuthSchemeId::new("sigv4");

/// SigV2 auth scheme. It registers under the SigV4 scheme id, replacing the SDK's SigV4
/// signer: S3 endpoint rules only carry signing config for `sigv4`, and a scheme missing
/// from them is rejected. Signs with the alias credentials (the SigV4 identity).
#[derive(Debug)]
pub struct SigV2AuthScheme {
    signer: SigV2Signer,
}

impl SigV2AuthScheme {
    /// `virtual_host`: the client uses virtual-host style requests (bucket in the host).
    pub fn new(virtual_host: bool) -> Self {
        Self {
            signer: SigV2Signer { virtual_host },
        }
    }
}

impl AuthScheme for SigV2AuthScheme {
    fn scheme_id(&self) -> AuthSchemeId {
        SIGV4_SCHEME_ID
    }

    fn identity_resolver(
        &self,
        identity_resolvers: &dyn GetIdentityResolver,
    ) -> Option<SharedIdentityResolver> {
        identity_resolvers.identity_resolver(SIGV4_SCHEME_ID)
    }

    fn signer(&self) -> &dyn Sign {
        &self.signer
    }
}

#[derive(Debug)]
struct SigV2Signer {
    virtual_host: bool,
}

impl Sign for SigV2Signer {
    fn sign_http_request(
        &self,
        request: &mut HttpRequest,
        identity: &Identity,
        _auth_scheme_endpoint_config: AuthSchemeEndpointConfig<'_>,
        _runtime_components: &RuntimeComponents,
        _config_bag: &ConfigBag,
    ) -> Result<(), BoxError> {
        let credentials = identity
            .data::<Credentials>()
            .ok_or("SigV2 signing needs access/secret key credentials")?;
        // minio-go skips signing for anonymous credentials.
        if credentials.access_key_id().is_empty() || credentials.secret_access_key().is_empty() {
            return Ok(());
        }
        if let Some(token) = credentials.session_token() {
            request
                .headers_mut()
                .insert("x-amz-security-token", token.to_string());
        }
        if request.headers().get("date").is_none() {
            request
                .headers_mut()
                .insert("date", http_date(SystemTime::now()));
        }
        let uri = url::Url::parse(request.uri())?;
        let host = match uri.port() {
            Some(port) => format!("{}:{port}", uri.host_str().unwrap_or_default()),
            None => uri.host_str().unwrap_or_default().to_string(),
        };
        let authorization = {
            let signable = SignableV2 {
                method: request.method(),
                host: &host,
                path: uri.path(),
                query: uri.query().unwrap_or_default(),
                headers: request.headers().iter().collect(),
                virtual_host: self.virtual_host,
            };
            authorization(
                &signable,
                credentials.access_key_id(),
                credentials.secret_access_key(),
            )
        };
        request.headers_mut().insert("authorization", authorization);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    #[test]
    fn signs_aws_documentation_examples() {
        // https://docs.aws.amazon.com/AmazonS3/latest/userguide/RESTAuthentication.html
        let get = SignableV2 {
            method: "GET",
            host: "awsexamplebucket1.s3.us-west-1.amazonaws.com",
            path: "/photos/puppy.jpg",
            headers: vec![("Date", "Tue, 27 Mar 2007 19:36:42 +0000")],
            virtual_host: true,
            ..Default::default()
        };
        assert_eq!(
            string_to_sign(&get, "Tue, 27 Mar 2007 19:36:42 +0000"),
            "GET\n\n\nTue, 27 Mar 2007 19:36:42 +0000\n/awsexamplebucket1/photos/puppy.jpg"
        );
        assert_eq!(
            authorization(&get, "AKIAIOSFODNN7EXAMPLE", SECRET),
            "AWS AKIAIOSFODNN7EXAMPLE:qgk2+6Sv9/oM7G3qLEjTH1a1l1g="
        );

        let list = SignableV2 {
            method: "GET",
            host: "awsexamplebucket1.s3.us-west-1.amazonaws.com",
            path: "/",
            query: "prefix=photos&max-keys=50&marker=puppy",
            headers: vec![("Date", "Tue, 27 Mar 2007 19:42:41 +0000")],
            virtual_host: true,
        };
        assert_eq!(
            authorization(&list, "AKIAIOSFODNN7EXAMPLE", SECRET),
            "AWS AKIAIOSFODNN7EXAMPLE:m0WP8eCtspQl5Ahe6L1SozdX9YA="
        );

        let acl = SignableV2 {
            method: "GET",
            host: "awsexamplebucket1.s3.us-west-1.amazonaws.com",
            path: "/",
            query: "acl",
            headers: vec![("Date", "Tue, 27 Mar 2007 19:44:46 +0000")],
            virtual_host: true,
        };
        assert_eq!(
            string_to_sign(&acl, "Tue, 27 Mar 2007 19:44:46 +0000"),
            "GET\n\n\nTue, 27 Mar 2007 19:44:46 +0000\n/awsexamplebucket1/?acl"
        );
        assert_eq!(
            authorization(&acl, "AKIAIOSFODNN7EXAMPLE", SECRET),
            "AWS AKIAIOSFODNN7EXAMPLE:82ZHiFIjc+WbcwFKGUVEQspPn+0="
        );
    }

    #[test]
    fn canonicalizes_amz_headers_and_sub_resources() {
        let request = SignableV2 {
            method: "PUT",
            host: "127.0.0.1:9000",
            path: "/bucket/dir/a%20b%2Bc.txt",
            query: "uploadId=abc%2Fdef&partNumber=2&x-id=UploadPart",
            headers: vec![
                ("Content-Type", "text/plain"),
                ("Content-MD5", "md5=="),
                ("X-Amz-Meta-B", "2"),
                ("x-amz-meta-a", "1"),
                ("x-amz-meta-b", "3"),
                ("Date", "D"),
            ],
            virtual_host: false,
        };
        assert_eq!(
            string_to_sign(&request, "D"),
            "PUT\nmd5==\ntext/plain\nD\nx-amz-meta-a:1\nx-amz-meta-b:2,3\n\
             /bucket/dir/a%20b%2Bc.txt?partNumber=2&uploadId=abc/def"
        );
    }

    #[test]
    fn presigns_like_minio_go() {
        let request = SignableV2 {
            method: "GET",
            host: "127.0.0.1:9000",
            path: "/bucket/my%20key",
            query: "versionId=v1",
            ..Default::default()
        };
        let query = presign_query(&request, "minio", "minio123", 1_700_000_000);
        let expected_sig = signature(
            "minio123",
            "GET\n\n\n1700000000\n/bucket/my%20key?versionId=v1",
        );
        assert_eq!(
            query,
            format!(
                "AWSAccessKeyId=minio&Expires=1700000000&versionId=v1&Signature={}",
                encode_path(&expected_sig)
            )
        );
        assert!(!query.contains('+') && !query.ends_with('='));
    }

    #[test]
    fn encodes_paths_like_s3utils() {
        assert_eq!(encode_path("a/b-c_d.e~f"), "a/b-c_d.e~f");
        assert_eq!(encode_path("a b+é"), "a%20b%2B%C3%A9");
        assert_eq!(
            http_date(SystemTime::UNIX_EPOCH),
            "Thu, 01 Jan 1970 00:00:00 GMT"
        );
    }
}
