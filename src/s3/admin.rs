//! MinIO admin API client (area H: quota, ilm tier, replication targets).
//!
//! Requests are SigV4-signed (service `s3`, region `us-east-1`, `x-amz-content-sha256`) and sent
//! with the smithy HTTP connector, so `--resolve` pins, `--insecure` and `certs/CAs` apply. The
//! same client is used for the MinIO-specific bucket sub-resources (`?replication`,
//! `?replication-metrics`, ...) whose XML/JSON bodies the AWS SDK cannot model.
//!
//! Request bodies that carry credentials are encrypted like `madmin.EncryptData`:
//! `salt(32) | id(1) | nonce(8) | sio-DARE stream`. We use id `0x02` (PBKDF2-SHA256 +
//! AES-256-GCM), which every MinIO server decrypts (it is madmin's FIPS mode).

use crate::config::model::AliasConfig;
use crate::error::McError;
use anyhow::{Context, Result, anyhow, bail};
use aws_credential_types::Credentials;
use aws_sdk_s3::primitives::{ByteStream, SdkBody};
use aws_sigv4::http_request::{
    PayloadChecksumKind, SignableBody, SignableRequest, SigningSettings, sign,
};
use aws_sigv4::sign::v4;
use aws_smithy_http_client::{
    Connector,
    tls::{self, rustls_provider::CryptoMode},
};
use aws_smithy_runtime_api::client::http::{HttpConnector, SharedHttpConnector};
use aws_smithy_runtime_api::client::identity::Identity;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

pub const ADMIN_PREFIX: &str = "/minio/admin/v3";

/// Raw HTTP response.
#[derive(Debug, Default)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    /// Response headers (lower-case names).
    pub headers: Vec<(String, String)>,
}

impl Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// First header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
}

/// Signed-request client for one alias.
pub struct AdminClient {
    scheme: String,
    authority: String,
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
    connector: SharedHttpConnector,
}

impl AdminClient {
    pub fn new(alias: &AliasConfig) -> Result<Self> {
        let url = url::Url::parse(&alias.url)
            .with_context(|| format!("invalid alias URL `{}`", alias.url))?;
        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("alias URL `{}` has no host", alias.url))?;
        let authority = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        let port = url.port_or_known_default();
        let mappings: Vec<_> = crate::resolve::configured()
            .into_iter()
            .filter(|m| m.host.eq_ignore_ascii_case(host) && Some(m.port) == port)
            .collect();
        let resolver = crate::resolve::PinnedDnsResolver::new(&mappings)?;
        // Same TLS trust as the S3 client: `--insecure`, else system roots + `certs/CAs`.
        let connector = if crate::globals::insecure() {
            crate::net::tls::insecure_connector(resolver)?
        } else {
            let mut builder =
                Connector::builder().tls_provider(tls::Provider::Rustls(CryptoMode::AwsLc));
            if let Some(context) = crate::net::tls::custom_ca_context()? {
                builder = builder.tls_context(context);
            }
            SharedHttpConnector::new(if mappings.is_empty() {
                builder.build()
            } else {
                builder.build_with_resolver(resolver)
            })
        };
        Ok(Self {
            scheme: url.scheme().to_string(),
            authority,
            access_key: alias.access_key.clone(),
            secret_key: alias.secret_key.clone(),
            session_token: alias.session_token.clone().filter(|t| !t.is_empty()),
            connector,
        })
    }

    pub fn secret_key(&self) -> &str {
        &self.secret_key
    }

    /// Sends a signed request. `path` must already be percent-encoded.
    pub async fn send(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Result<Response> {
        let (mut response, stream) = self
            .send_streaming(method, path, query, headers, body)
            .await?;
        response.body = stream.collect().await?.into_bytes().to_vec();
        Ok(response)
    }

    /// Like [`Self::send`], but returns the body unread (status and headers filled in).
    pub async fn send_streaming(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Result<(Response, ByteStream)> {
        let uri = format!(
            "{}://{}{}{}",
            self.scheme,
            self.authority,
            path,
            encode_query(query)
        );
        let mut all_headers: Vec<(&str, String)> = vec![("host", self.authority.clone())];
        all_headers.extend(headers.iter().cloned());
        let signed = sign_headers(
            method,
            &uri,
            &all_headers,
            &body,
            &self.access_key,
            &self.secret_key,
            self.session_token.as_deref(),
            SystemTime::now(),
        )?;

        let mut builder = http::Request::builder().method(method).uri(&uri);
        for (name, value) in &all_headers {
            builder = builder.header(*name, value.as_str());
        }
        for (name, value) in &signed {
            builder = builder.header(name.as_str(), value.as_str());
        }
        builder = builder.header("content-length", body.len().to_string());
        let request = builder.body(SdkBody::from(body))?;
        let request = aws_smithy_runtime_api::http::Request::try_from(request)?;
        let response = self
            .connector
            .call(request)
            .await
            .map_err(|err| super::error::go_transport_error(&go_method(method), &uri, &err))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(n, v)| (n.to_ascii_lowercase(), v.to_string()))
            .collect();
        Ok((
            Response {
                status,
                body: Vec::new(),
                headers,
            },
            ByteStream::new(response.into_body()),
        ))
    }

    /// Sends an unsigned request without a body (e.g. Prometheus metrics with a bearer
    /// token); `path` may carry a `?query` and must already be percent-encoded.
    pub async fn send_unsigned(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
    ) -> Result<Response> {
        self.send_anonymous(method, path, headers, Vec::new()).await
    }

    /// Sends an unsigned request (STS `AssumeRoleWith*` form posts, metrics) over the same
    /// connector; `path` must already be percent-encoded. The status is not checked.
    pub async fn send_anonymous(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Result<Response> {
        let uri = format!("{}://{}{}", self.scheme, self.authority, path);
        let mut builder = http::Request::builder()
            .method(method)
            .uri(&uri)
            .header("host", self.authority.as_str());
        for (name, value) in headers {
            builder = builder.header(*name, value.as_str());
        }
        builder = builder.header("content-length", body.len().to_string());
        let request = builder.body(SdkBody::from(body))?;
        let request = aws_smithy_runtime_api::http::Request::try_from(request)?;
        let response = self
            .connector
            .call(request)
            .await
            .map_err(|err| super::error::go_transport_error(&go_method(method), &uri, &err))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(n, v)| (n.to_ascii_lowercase(), v.to_string()))
            .collect();
        let body = ByteStream::new(response.into_body())
            .collect()
            .await?
            .into_bytes()
            .to_vec();
        Ok(Response {
            status,
            body,
            headers,
        })
    }

    /// Admin API request builder for `/minio/admin/v3/<api>` (see [`AdminRequest`]).
    pub fn request(&self, method: &str, api: &str) -> AdminRequest<'_> {
        self.request_at(method, &format!("{ADMIN_PREFIX}/{api}"))
    }

    /// Request builder for any server path (e.g. `/minio/kms/v1/key/list`,
    /// `/minio/v2/metrics/cluster`); `path` is percent-encoded per segment.
    pub fn request_at(&self, method: &str, path: &str) -> AdminRequest<'_> {
        AdminRequest {
            client: self,
            method: method.to_string(),
            path: encode_path(path),
            query: Vec::new(),
            headers: Vec::new(),
            body: Vec::new(),
            decrypt: false,
        }
    }

    /// `GET` admin API, JSON response.
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        api: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        self.request("GET", api).queries(query).send_json().await
    }

    /// `PUT` admin API with a JSON body.
    pub async fn put_json<B: Serialize + ?Sized>(
        &self,
        api: &str,
        query: &[(&str, &str)],
        body: &B,
    ) -> Result<Response> {
        self.request("PUT", api)
            .queries(query)
            .json(body)?
            .send()
            .await
    }

    /// `POST` admin API with a JSON body.
    pub async fn post_json<B: Serialize + ?Sized>(
        &self,
        api: &str,
        query: &[(&str, &str)],
        body: &B,
    ) -> Result<Response> {
        self.request("POST", api)
            .queries(query)
            .json(body)?
            .send()
            .await
    }

    /// `DELETE` admin API.
    pub async fn delete(&self, api: &str, query: &[(&str, &str)]) -> Result<Response> {
        self.request("DELETE", api).queries(query).send().await
    }

    /// Decrypts a madmin `EncryptData` response body with this alias' secret key.
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        decrypt_response(&self.secret_key, data)
    }
}

/// One admin request: query parameters, optional (madmin-encrypted) body, optional response
/// decryption. Non-2xx statuses become madmin errors (`send*`, `stream`).
///
/// ```ignore
/// let users: HashMap<String, UserInfo> =
///     client.request("GET", "list-users").decrypt().send_json().await?;
/// client.request("PUT", "add-user").query("accessKey", user).encrypted_json(&req)?.send().await?;
/// let mut trace = client.request("GET", "trace").query("all", "true").stream().await?;
/// ```
pub struct AdminRequest<'a> {
    client: &'a AdminClient,
    method: String,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    decrypt: bool,
}

impl AdminRequest<'_> {
    /// Adds a query parameter (repeat for multi-valued keys).
    pub fn query(mut self, key: &str, value: impl Into<String>) -> Self {
        self.query.push((key.to_string(), value.into()));
        self
    }

    /// Adds query parameters.
    pub fn queries(mut self, query: &[(&str, &str)]) -> Self {
        self.query
            .extend(query.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        self
    }

    /// Adds a request header.
    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    /// Raw request body.
    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    /// JSON request body.
    pub fn json<B: Serialize + ?Sized>(self, body: &B) -> Result<Self> {
        Ok(self.body(serde_json::to_vec(body)?))
    }

    /// Encrypts the current body like `madmin.EncryptData(secretKey, body)`.
    pub fn encrypted(mut self) -> Result<Self> {
        self.body = encrypt_data(&self.client.secret_key, &self.body)?;
        Ok(self)
    }

    /// JSON body encrypted with `madmin.EncryptData`.
    pub fn encrypted_json<B: Serialize + ?Sized>(self, body: &B) -> Result<Self> {
        self.json(body)?.encrypted()
    }

    /// The response body is `madmin.EncryptData` output; `send*` return it decrypted.
    pub fn decrypt(mut self) -> Self {
        self.decrypt = true;
        self
    }

    /// Sends without checking the status (body decrypted only on 2xx).
    pub async fn send_unchecked(self) -> Result<Response> {
        let query: Vec<(&str, &str)> = self
            .query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let mut response = self
            .client
            .send(&self.method, &self.path, &query, &self.headers, self.body)
            .await?;
        if self.decrypt && (200..300).contains(&response.status) {
            response.body = self.client.decrypt(&response.body)?;
        }
        Ok(response)
    }

    /// Sends; non-2xx statuses become madmin `ErrorResponse` errors.
    pub async fn send(self) -> Result<Response> {
        check_status(self.send_unchecked().await?)
    }

    /// Sends and decodes the (decrypted) JSON response.
    pub async fn send_json<T: DeserializeOwned>(self) -> Result<T> {
        let response = self.send().await?;
        Ok(serde_json::from_slice(&response.body)?)
    }

    /// Sends and returns the body as a stream of JSON documents (trace, logs, metrics, ...).
    /// Errors (non-2xx) are read in full and mapped like [`Self::send`].
    pub async fn stream(self) -> Result<JsonStream> {
        let query: Vec<(&str, &str)> = self
            .query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let (mut response, body) = self
            .client
            .send_streaming(&self.method, &self.path, &query, &self.headers, self.body)
            .await?;
        if !(200..300).contains(&response.status) {
            response.body = body.collect().await?.into_bytes().to_vec();
            return Err(madmin_error(&response).into());
        }
        Ok(JsonStream::new(body))
    }
}

// ---------------------------------------------------------------------------
// Streaming JSON documents
// ---------------------------------------------------------------------------

/// JSON documents from a long-lived (chunked) response: newline-delimited or concatenated,
/// whitespace keep-alives (MinIO writes `" "` while idle) are skipped.
pub struct JsonStream {
    body: ByteStream,
    buf: Vec<u8>,
}

impl JsonStream {
    pub fn new(body: ByteStream) -> Self {
        Self {
            body,
            buf: Vec::new(),
        }
    }

    /// Next document; `None` when the server closed the stream.
    pub async fn next<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
        loop {
            if let Some(item) = next_document(&mut self.buf)? {
                return Ok(Some(item));
            }
            match self.body.next().await {
                Some(chunk) => self.buf.extend_from_slice(&chunk?),
                None if self.buf.iter().all(u8::is_ascii_whitespace) => return Ok(None),
                None => bail!("unexpected end of JSON stream"),
            }
        }
    }

    /// Calls `on_item` for every document until the stream ends, Ctrl-C is pressed, or stdout
    /// is closed (an `on_item` error caused by `BrokenPipe`); those end with `Ok(())`.
    pub async fn for_each<T, F>(mut self, mut on_item: F) -> Result<()>
    where
        T: DeserializeOwned,
        F: FnMut(T) -> Result<()>,
    {
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        loop {
            tokio::select! {
                _ = &mut ctrl_c => return Ok(()),
                item = self.next::<T>() => match item? {
                    None => return Ok(()),
                    Some(item) => match on_item(item) {
                        Err(err) if is_broken_pipe(&err) => return Ok(()),
                        other => other?,
                    },
                },
            }
        }
    }
}

/// Parses one complete document from the front of `buf` (consuming it), skipping leading
/// whitespace; `None` when more data is needed.
fn next_document<T: DeserializeOwned>(buf: &mut Vec<u8>) -> Result<Option<T>> {
    let start = buf
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(buf.len());
    buf.drain(..start);
    if buf.is_empty() {
        return Ok(None);
    }
    let mut docs = serde_json::Deserializer::from_slice(buf).into_iter::<T>();
    match docs.next() {
        Some(Ok(item)) => {
            let used = docs.byte_offset();
            buf.drain(..used);
            Ok(Some(item))
        }
        Some(Err(err)) if err.is_eof() => Ok(None),
        Some(Err(err)) => Err(err.into()),
        None => Ok(None),
    }
}

/// True when `err` was caused by writing to a closed pipe (`mx admin trace | head`).
pub fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

impl AdminClient {
    /// Sends an admin API request (`/minio/admin/v3/<api>`); non-2xx statuses become errors.
    pub async fn admin(
        &self,
        method: &str,
        api: &str,
        query: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<Response> {
        let path = format!("{ADMIN_PREFIX}/{}", encode_path(api));
        let response = self.send(method, &path, query, &[], body).await?;
        check_status(response)
    }

    /// Sends a bucket sub-resource request (path-style) without checking the status.
    pub async fn bucket_raw(
        &self,
        method: &str,
        bucket: &str,
        query: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<Response> {
        // minio-go requests bucket sub-resources as `/bucket/?...` (the server echoes the
        // path as the error `Resource`).
        let path = format!("/{}/", encode_path(bucket));
        let mut headers = Vec::new();
        if !body.is_empty() {
            use base64::Engine;
            use md5::Digest;
            let md5 = md5::Md5::digest(&body);
            headers.push((
                "content-md5",
                base64::engine::general_purpose::STANDARD.encode(md5),
            ));
        }
        self.send(method, &path, query, &headers, body).await
    }

    /// Like [`Self::bucket_raw`], but non-2xx statuses become errors (minio-go
    /// `ErrorResponse`, the S3 API errors mc reports).
    pub async fn bucket(
        &self,
        method: &str,
        bucket: &str,
        query: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<Response> {
        check_s3_status(self.bucket_raw(method, bucket, query, body).await?, bucket)
    }
}

/// Go `net/http` method spelling in `url.Error` texts (`Get "URL": ...`).
fn go_method(method: &str) -> String {
    let lower = method.to_ascii_lowercase();
    let mut chars = lower.chars();
    chars
        .next()
        .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

/// mc `newAdminClient(aliasedURL)`: the alias is the first path element; failures carry
/// mc's message `Unable to initialize admin connection.`.
pub fn admin_client_for(
    store: &crate::config::ConfigStore,
    aliased_url: &str,
) -> Result<AdminClient> {
    let alias = aliased_url.split('/').next().unwrap_or_default();
    let config = if store.is_invalid_env_alias(alias) {
        store.alias(alias)
    } else {
        store.config().aliases.get(alias).cloned().ok_or_else(|| {
            let lower = aliased_url.to_ascii_lowercase();
            if lower.starts_with("http://") || lower.starts_with("https://") {
                McError::invalid_aliased_url(aliased_url).into()
            } else {
                McError::new(format!(
                    "No valid configuration found for '{aliased_url}' host alias"
                ))
                .into()
            }
        })
    };
    config
        .and_then(|config| AdminClient::new(&config))
        .context("Unable to initialize admin connection.")
}

/// Returns SigV4 headers (`authorization`, `x-amz-date`, `x-amz-content-sha256`, ...) to add.
#[allow(clippy::too_many_arguments)]
pub fn sign_headers(
    method: &str,
    uri: &str,
    headers: &[(&str, String)],
    body: &[u8],
    access_key: &str,
    secret_key: &str,
    session_token: Option<&str>,
    time: SystemTime,
) -> Result<Vec<(String, String)>> {
    let identity: Identity = Credentials::new(
        access_key,
        secret_key,
        session_token.map(str::to_string),
        None,
        "mx",
    )
    .into();
    let mut settings = SigningSettings::default();
    settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region("us-east-1")
        .name("s3")
        .time(time)
        .settings(settings)
        .build()?
        .into();
    let signable = SignableRequest::new(
        method,
        uri,
        headers.iter().map(|(n, v)| (*n, v.as_str())),
        SignableBody::Bytes(body),
    )?;
    let (instructions, _) = sign(signable, &params)?.into_parts();
    Ok(instructions
        .headers()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect())
}

/// Admin API status check: non-2xx responses become madmin `ErrorResponse` errors.
pub fn check_status(response: Response) -> Result<Response> {
    if (200..300).contains(&response.status) {
        return Ok(response);
    }
    Err(madmin_error(&response).into())
}

/// S3 API status check for bucket sub-resources: minio-go `ErrorResponse` errors.
pub fn check_s3_status(response: Response, bucket: &str) -> Result<Response> {
    if (200..300).contains(&response.status) {
        return Ok(response);
    }
    let resp = super::error::ErrorResponse::from_http(
        response.status,
        |name| response.header(name),
        &response.body,
        bucket,
        "",
    );
    Err(resp.to_raw_error().into())
}

/// Decodes an admin API error like madmin `httpRespToErrorResponse` (JSON, then XML body);
/// the detail is madmin's `ErrorResponse` as Go marshals it.
pub fn madmin_error(response: &Response) -> McError {
    let text = response.text();
    let status_line = || {
        format!(
            "{} {}",
            response.status,
            http::StatusCode::from_u16(response.status)
                .ok()
                .and_then(|s| s.canonical_reason())
                .unwrap_or_default()
        )
    };
    let mut fields: Vec<(&str, String)> = [
        "Code",
        "Message",
        "BucketName",
        "Key",
        "RequestID",
        "HostID",
        "Region",
    ]
    .iter()
    .map(|name| (*name, String::new()))
    .collect();
    let mut set = |name: &str, value: String| {
        if let Some(field) = fields
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            field.1 = value;
        }
    };
    match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text) {
        Ok(map) => {
            for (key, value) in map {
                // Go matches JSON keys case-insensitively (`RequestId` -> `RequestID`).
                if let Some(value) = value.as_str() {
                    set(&key, value.to_string());
                }
            }
        }
        Err(json_err) => {
            let xml_code = xml_text(&text, "Code");
            if xml_code.is_some() {
                for (tag, name) in [
                    ("Code", "Code"),
                    ("Message", "Message"),
                    ("BucketName", "BucketName"),
                    ("Key", "Key"),
                    ("RequestId", "RequestID"),
                    ("HostId", "HostID"),
                    ("Region", "Region"),
                ] {
                    set(name, xml_text(&text, tag).unwrap_or_default());
                }
            } else {
                let json_text = if text.trim().is_empty() {
                    "unexpected end of JSON input".to_string()
                } else {
                    json_err.to_string()
                };
                let mut body = text.clone();
                if body.len() > 1024 {
                    body = format!("{}...", &body[..body.floor_char_boundary(1021)]);
                }
                set("Code", status_line());
                set(
                    "Message",
                    format!("Failed to parse server response ({json_text}): {body}"),
                );
            }
        }
    }
    let message = fields[1].1.clone();
    let code = fields[0].1.clone();
    let detail = crate::error::Detail(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_string(), serde_json::Value::String(value)))
            .collect(),
    );
    McError::with_detail(message, detail).with_code(code)
}

/// Readable message of a MinIO admin (JSON) or S3 (XML) error body.
pub fn error_message(response: &Response) -> String {
    madmin_error(response).message
}

/// Returns the S3 error code of an XML error body.
pub fn error_code(response: &Response) -> Option<String> {
    xml_text(&response.text(), "Code")
}

fn xml_text(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find(&close)?;
    Some(text[start..end].to_string())
}

/// Percent-encodes everything except RFC 3986 unreserved characters.
pub fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .map(encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_query(query: &[(&str, &str)]) -> String {
    if query.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = query
        .iter()
        .map(|(k, v)| format!("{}={}", encode_component(k), encode_component(v)))
        .collect();
    format!("?{}", pairs.join("&"))
}

// ---------------------------------------------------------------------------
// madmin EncryptData
// ---------------------------------------------------------------------------

const SIO_BUF_SIZE: usize = 1 << 14;
const TAG_LEN: usize = 16;
const PBKDF2_AES_GCM: u8 = 0x02;
const PBKDF2_COST: u32 = 8192;

/// Encrypts `data` the way `madmin.EncryptData(password, data)` does (PBKDF2/AES-GCM variant).
pub fn encrypt_data(password: &str, data: &[u8]) -> Result<Vec<u8>> {
    let mut salt = [0u8; 32];
    let mut nonce = [0u8; 8];
    aws_lc_rs::rand::fill(&mut salt).map_err(|_| anyhow!("random generator failed"))?;
    aws_lc_rs::rand::fill(&mut nonce).map_err(|_| anyhow!("random generator failed"))?;
    encrypt_data_with(password, data, &salt, &nonce)
}

fn encrypt_data_with(
    password: &str,
    data: &[u8],
    salt: &[u8; 32],
    nonce: &[u8; 8],
) -> Result<Vec<u8>> {
    let key = pbkdf2_key(password, salt);
    let mut out = Vec::with_capacity(41 + data.len() + TAG_LEN * (data.len() / SIO_BUF_SIZE + 1));
    out.extend_from_slice(salt);
    out.push(PBKDF2_AES_GCM);
    out.extend_from_slice(nonce);
    out.extend(sio_seal(&aws_lc_rs::aead::AES_256_GCM, &key, nonce, data)?);
    Ok(out)
}

fn pbkdf2_key(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    aws_lc_rs::pbkdf2::derive(
        aws_lc_rs::pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(PBKDF2_COST).expect("non-zero"),
        salt,
        password.as_bytes(),
        &mut key,
    );
    key
}

fn aead_key(
    algorithm: &'static aws_lc_rs::aead::Algorithm,
    key: &[u8],
) -> Result<aws_lc_rs::aead::LessSafeKey> {
    let unbound =
        aws_lc_rs::aead::UnboundKey::new(algorithm, key).map_err(|_| anyhow!("invalid key"))?;
    Ok(aws_lc_rs::aead::LessSafeKey::new(unbound))
}

fn sio_nonce(nonce: &[u8; 8], seq: u32) -> aws_lc_rs::aead::Nonce {
    let mut full = [0u8; 12];
    full[..8].copy_from_slice(nonce);
    full[8..].copy_from_slice(&seq.to_le_bytes());
    aws_lc_rs::aead::Nonce::assume_unique_for_key(full)
}

/// sio-go stream associated data: `flag | Seal(nonce||0, "", nil)` (the tag of an empty message).
fn sio_associated_data(key: &aws_lc_rs::aead::LessSafeKey, nonce: &[u8; 8]) -> Result<Vec<u8>> {
    let mut tag = Vec::new();
    key.seal_in_place_append_tag(sio_nonce(nonce, 0), aws_lc_rs::aead::Aad::empty(), &mut tag)
        .map_err(|_| anyhow!("encryption failed"))?;
    let mut ad = vec![0u8];
    ad.extend(tag);
    Ok(ad)
}

/// `sio.AES_256_GCM.Stream(key).EncryptWriter(w, nonce, nil)`: 16 KiB fragments, sequence
/// numbers from 1, final fragment flagged with `0x80` in the associated data.
fn sio_seal(
    algorithm: &'static aws_lc_rs::aead::Algorithm,
    key_bytes: &[u8],
    nonce: &[u8; 8],
    data: &[u8],
) -> Result<Vec<u8>> {
    let key = aead_key(algorithm, key_bytes)?;
    let mut ad = sio_associated_data(&key, nonce)?;
    let mut out = Vec::new();
    for (index, chunk, last) in fragments(data, SIO_BUF_SIZE) {
        ad[0] = if last { 0x80 } else { 0x00 };
        let mut buf = chunk.to_vec();
        key.seal_in_place_append_tag(
            sio_nonce(nonce, index + 1),
            aws_lc_rs::aead::Aad::from(ad.as_slice()),
            &mut buf,
        )
        .map_err(|_| anyhow!("encryption failed"))?;
        out.extend(buf);
    }
    Ok(out)
}

/// Inverse of [`sio_seal`] (`algorithm`: AES-256-GCM or ChaCha20-Poly1305).
fn sio_open(
    algorithm: &'static aws_lc_rs::aead::Algorithm,
    key_bytes: &[u8],
    nonce: &[u8; 8],
    data: &[u8],
) -> Result<Vec<u8>> {
    let key = aead_key(algorithm, key_bytes)?;
    let mut ad = sio_associated_data(&key, nonce)?;
    let mut out = Vec::new();
    for (index, chunk, last) in fragments(data, SIO_BUF_SIZE + TAG_LEN) {
        ad[0] = if last { 0x80 } else { 0x00 };
        let mut buf = chunk.to_vec();
        let plain = key
            .open_in_place(
                sio_nonce(nonce, index + 1),
                aws_lc_rs::aead::Aad::from(ad.as_slice()),
                &mut buf,
            )
            .map_err(|_| anyhow!("data is not authentic"))?;
        out.extend_from_slice(plain);
    }
    Ok(out)
}

/// Splits `data` into `size` chunks; the last (possibly empty or full) chunk is flagged.
fn fragments(data: &[u8], size: usize) -> Vec<(u32, &[u8], bool)> {
    let mut out = Vec::new();
    let mut rest = data;
    let mut index = 0u32;
    while rest.len() > size {
        let (chunk, tail) = rest.split_at(size);
        out.push((index, chunk, false));
        rest = tail;
        index += 1;
    }
    out.push((index, rest, true));
    out
}

/// Key derivation for argon2id-encrypted data (ids `0x00`/`0x01`).
pub type Argon2idFn<'a> = &'a dyn Fn(&str, &[u8]) -> [u8; 32];

/// madmin's argon2id key derivation (`argon2.IDKey(password, salt, 1, 64*1024, 4, 32)`).
pub fn argon2id_key(password: &str, salt: &[u8]) -> [u8; 32] {
    use argon2::{Algorithm, Argon2, Params, Version};
    let params = Params::new(64 * 1024, 1, 4, Some(32)).expect("valid argon2 params");
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .expect("argon2id key derivation");
    key
}

/// Decrypts `madmin.EncryptData` output: PBKDF2/AES-GCM (`0x02`) always, argon2id with
/// AES-GCM (`0x00`) or ChaCha20-Poly1305 (`0x01`) when `argon2id` is given.
pub fn decrypt_data(password: &str, data: &[u8], argon2id: Option<Argon2idFn>) -> Result<Vec<u8>> {
    if data.len() < 41 {
        bail!("unexpected header");
    }
    let (salt, rest) = data.split_at(32);
    let id = rest[0];
    let nonce: [u8; 8] = rest[1..9].try_into().expect("8 bytes");
    let (key, algorithm) = match (id, argon2id) {
        (PBKDF2_AES_GCM, _) => (pbkdf2_key(password, salt), &aws_lc_rs::aead::AES_256_GCM),
        (0x00, Some(derive)) => (derive(password, salt), &aws_lc_rs::aead::AES_256_GCM),
        (0x01, Some(derive)) => (derive(password, salt), &aws_lc_rs::aead::CHACHA20_POLY1305),
        _ => bail!("unsupported encryption algorithm ID {id:#04x}"),
    };
    sio_open(algorithm, &key, &nonce, &rest[9..])
}

/// Decrypts an encrypted admin API response (MinIO servers use argon2id unless in FIPS mode).
pub fn decrypt_response(password: &str, data: &[u8]) -> Result<Vec<u8>> {
    decrypt_data(password, data, Some(&argon2id_key))
}

// ---------------------------------------------------------------------------
// Human-readable sizes (go-humanize compatible)
// ---------------------------------------------------------------------------

fn humanate(size: u64, base: f64, units: &[&str]) -> String {
    if size < 10 {
        return format!("{size} B");
    }
    let exp = ((size as f64).ln() / base.ln()).floor() as usize;
    let exp = exp.min(units.len() - 1);
    let value = ((size as f64) / base.powi(exp as i32) * 10.0 + 0.5).floor() / 10.0;
    if value < 10.0 {
        format!("{value:.1} {}", units[exp])
    } else {
        format!("{value:.0} {}", units[exp])
    }
}

/// `humanize.IBytes`: `1.0 GiB`.
pub fn ibytes(size: u64) -> String {
    humanate(
        size,
        1024.0,
        &["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"],
    )
}

/// `humanize.Bytes`: `1.0 GB`.
pub fn si_bytes(size: u64) -> String {
    humanate(size, 1000.0, &["B", "kB", "MB", "GB", "TB", "PB", "EB"])
}

/// `humanize.Comma`: `1,234,567`.
pub fn comma(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if value < 0 { format!("-{out}") } else { out }
}

/// `humanize.ParseBytes`: `1GB` = 10^9, `1GiB` = 2^30, `1.5k`, `1,024`. Errors are mc's
/// causes, with the Go error value as detail (`*strconv.NumError` for a bad number).
pub fn parse_bytes(input: &str) -> std::result::Result<u64, McError> {
    let digits = input
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == ','))
        .map(|(i, _)| i)
        .unwrap_or(input.len());
    let num = input[..digits].replace(',', "");
    let value: f64 = num.parse().map_err(|_| {
        McError::with_detail(
            format!("strconv.ParseFloat: parsing {num:?}: invalid syntax"),
            crate::detail![
                ("Func", "ParseFloat"),
                ("Num", num),
                ("Err", serde_json::json!({}))
            ],
        )
    })?;
    let extra = input[digits..].trim().to_ascii_lowercase();
    let unit = |prefix: &str| -> Option<f64> {
        let exp = ["", "k", "m", "g", "t", "p", "e"]
            .iter()
            .position(|p| *p == prefix)?;
        Some(exp as f64)
    };
    let multiplier = match extra.as_str() {
        "" | "b" => Some(1.0),
        other => {
            let stem = other.strip_suffix('b').unwrap_or(other);
            match stem.strip_suffix('i') {
                Some(prefix) if !prefix.is_empty() => unit(prefix).map(|e| 1024f64.powf(e)),
                _ => unit(stem).filter(|e| *e > 0.0).map(|e| 1000f64.powf(e)),
            }
        }
    };
    let Some(multiplier) = multiplier else {
        return Err(McError::new(format!("unhandled size name: {extra}")));
    };
    let bytes = value * multiplier;
    if bytes >= u64::MAX as f64 {
        return Err(McError::new(format!("too large: {input}")));
    }
    Ok(bytes as u64)
}

/// Serializes an `f64` like Go's `encoding/json` (integral values without `.0`).
pub fn go_f64<S: serde::Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

/// [`go_f64`] for `f32`.
pub fn go_f32<S: serde::Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f32(*value)
    }
}

// ---------------------------------------------------------------------------
// Bucket quota
// ---------------------------------------------------------------------------

/// `madmin.BucketQuota` (`quota` is the deprecated field older servers/clients use).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BucketQuota {
    #[serde(default)]
    pub quota: u64,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub rate: u64,
    #[serde(default)]
    pub requests: u64,
    #[serde(
        rename = "quotatype",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub quota_type: String,
}

pub async fn get_bucket_quota(client: &AdminClient, bucket: &str) -> Result<BucketQuota> {
    let response = client
        .admin("GET", "get-bucket-quota", &[("bucket", bucket)], Vec::new())
        .await?;
    Ok(serde_json::from_slice(&response.body)?)
}

pub async fn set_bucket_quota(
    client: &AdminClient,
    bucket: &str,
    quota: &BucketQuota,
) -> Result<()> {
    client
        .admin(
            "PUT",
            "set-bucket-quota",
            &[("bucket", bucket)],
            serde_json::to_vec(quota)?,
        )
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Remote tiers
// ---------------------------------------------------------------------------

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

/// madmin `TierS3` (Go field order, `omitempty`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TierS3 {
    #[serde(rename = "Endpoint", skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(rename = "AccessKey", skip_serializing_if = "String::is_empty")]
    pub access_key: String,
    #[serde(rename = "SecretKey", skip_serializing_if = "String::is_empty")]
    pub secret_key: String,
    #[serde(rename = "Bucket", skip_serializing_if = "String::is_empty")]
    pub bucket: String,
    #[serde(rename = "Prefix", skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(rename = "Region", skip_serializing_if = "String::is_empty")]
    pub region: String,
    #[serde(rename = "StorageClass", skip_serializing_if = "String::is_empty")]
    pub storage_class: String,
    #[serde(rename = "AWSRole", skip_serializing_if = "is_false")]
    pub aws_role: bool,
    #[serde(
        rename = "AWSRoleWebIdentityTokenFile",
        skip_serializing_if = "String::is_empty"
    )]
    pub aws_role_web_identity_token_file: String,
    #[serde(rename = "AWSRoleARN", skip_serializing_if = "String::is_empty")]
    pub aws_role_arn: String,
    #[serde(
        rename = "AWSRoleSessionName",
        skip_serializing_if = "String::is_empty"
    )]
    pub aws_role_session_name: String,
    #[serde(rename = "AWSRoleDurationSeconds", skip_serializing_if = "is_zero_i64")]
    pub aws_role_duration_seconds: i64,
}

/// madmin `ServicePrincipalAuth` (a struct, so Go always marshals it, even when empty).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServicePrincipalAuth {
    #[serde(rename = "TenantID", skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(rename = "ClientID", skip_serializing_if = "String::is_empty")]
    pub client_id: String,
    #[serde(rename = "ClientSecret", skip_serializing_if = "String::is_empty")]
    pub client_secret: String,
}

/// madmin `TierAzure`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TierAzure {
    #[serde(rename = "Endpoint", skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(rename = "AccountName", skip_serializing_if = "String::is_empty")]
    pub account_name: String,
    #[serde(rename = "AccountKey", skip_serializing_if = "String::is_empty")]
    pub account_key: String,
    #[serde(rename = "Bucket", skip_serializing_if = "String::is_empty")]
    pub bucket: String,
    #[serde(rename = "Prefix", skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(rename = "Region", skip_serializing_if = "String::is_empty")]
    pub region: String,
    #[serde(rename = "StorageClass", skip_serializing_if = "String::is_empty")]
    pub storage_class: String,
    #[serde(rename = "SPAuth")]
    pub sp_auth: ServicePrincipalAuth,
}

/// madmin `TierGCS` (`Creds` is the URL-safe base64 of the credentials JSON file).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TierGCS {
    #[serde(rename = "Endpoint", skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(rename = "Creds", skip_serializing_if = "String::is_empty")]
    pub creds: String,
    #[serde(rename = "Bucket", skip_serializing_if = "String::is_empty")]
    pub bucket: String,
    #[serde(rename = "Prefix", skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(rename = "Region", skip_serializing_if = "String::is_empty")]
    pub region: String,
    #[serde(rename = "StorageClass", skip_serializing_if = "String::is_empty")]
    pub storage_class: String,
}

/// madmin `TierMinIO`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TierMinIO {
    #[serde(rename = "Endpoint", skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(rename = "AccessKey", skip_serializing_if = "String::is_empty")]
    pub access_key: String,
    #[serde(rename = "SecretKey", skip_serializing_if = "String::is_empty")]
    pub secret_key: String,
    #[serde(rename = "Bucket", skip_serializing_if = "String::is_empty")]
    pub bucket: String,
    #[serde(rename = "Prefix", skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(rename = "Region", skip_serializing_if = "String::is_empty")]
    pub region: String,
}

/// `madmin.TierConfig`; sections serialize in madmin's field order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TierConfig {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Type")]
    pub tier_type: String,
    #[serde(rename = "Name", default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(rename = "S3", default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<TierS3>,
    #[serde(rename = "Azure", default, skip_serializing_if = "Option::is_none")]
    pub azure: Option<TierAzure>,
    #[serde(rename = "GCS", default, skip_serializing_if = "Option::is_none")]
    pub gcs: Option<TierGCS>,
    #[serde(rename = "MinIO", default, skip_serializing_if = "Option::is_none")]
    pub minio: Option<TierMinIO>,
}

/// Common fields of a tier section: endpoint, bucket, prefix, region, storage class.
type TierFields<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

impl TierConfig {
    fn fields(&self) -> TierFields<'_> {
        match self.tier_type.as_str() {
            "s3" => self.s3.as_ref().map(|t| {
                (
                    t.endpoint.as_str(),
                    t.bucket.as_str(),
                    t.prefix.as_str(),
                    t.region.as_str(),
                    t.storage_class.as_str(),
                )
            }),
            "azure" => self.azure.as_ref().map(|t| {
                (
                    t.endpoint.as_str(),
                    t.bucket.as_str(),
                    t.prefix.as_str(),
                    t.region.as_str(),
                    t.storage_class.as_str(),
                )
            }),
            "gcs" => self.gcs.as_ref().map(|t| {
                (
                    t.endpoint.as_str(),
                    t.bucket.as_str(),
                    t.prefix.as_str(),
                    t.region.as_str(),
                    t.storage_class.as_str(),
                )
            }),
            "minio" => self.minio.as_ref().map(|t| {
                (
                    t.endpoint.as_str(),
                    t.bucket.as_str(),
                    t.prefix.as_str(),
                    t.region.as_str(),
                    "",
                )
            }),
            _ => None,
        }
        .unwrap_or_default()
    }

    pub fn endpoint(&self) -> &str {
        self.fields().0
    }

    pub fn bucket(&self) -> &str {
        self.fields().1
    }

    pub fn prefix(&self) -> &str {
        self.fields().2
    }

    pub fn region(&self) -> &str {
        self.fields().3
    }

    /// Storage class (mc `storageClass`: empty for minio tiers).
    pub fn storage_class(&self) -> &str {
        self.fields().4
    }

    /// Common field by madmin name (`Endpoint`, `Bucket`, `Prefix`, `Region`,
    /// `StorageClass`), or "".
    pub fn field(&self, name: &str) -> String {
        match name {
            "Endpoint" => self.endpoint(),
            "Bucket" => self.bucket(),
            "Prefix" => self.prefix(),
            "Region" => self.region(),
            "StorageClass" => self.storage_class(),
            _ => "",
        }
        .to_string()
    }
}

/// `madmin.TierCreds` for `EditTier`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TierCreds {
    #[serde(rename = "access", skip_serializing_if = "String::is_empty")]
    pub access_key: String,
    #[serde(rename = "secret", skip_serializing_if = "String::is_empty")]
    pub secret_key: String,
    #[serde(rename = "awsrole")]
    pub aws_role: bool,
    #[serde(
        rename = "awsroleWebIdentity",
        skip_serializing_if = "String::is_empty"
    )]
    pub aws_role_web_identity_token_file: String,
    #[serde(rename = "awsroleARN", skip_serializing_if = "String::is_empty")]
    pub aws_role_arn: String,
    #[serde(rename = "azSP")]
    pub az_sp: ServicePrincipalAuth,
    /// GCS credentials file contents (Go `[]byte`: standard base64 in JSON).
    #[serde(
        rename = "creds",
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_base64"
    )]
    pub creds_json: Vec<u8>,
}

fn serialize_base64<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    use base64::Engine;
    serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// `madmin.TierStats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TierStats {
    #[serde(rename = "totalSize")]
    pub total_size: u64,
    #[serde(rename = "numVersions")]
    pub num_versions: i64,
    #[serde(rename = "numObjects")]
    pub num_objects: i64,
}

/// Go's zero `time.Time` as JSON.
pub const GO_ZERO_TIME: &str = "0001-01-01T00:00:00Z";

/// `madmin.DailyTierStats` (`Bins` is a Go `[24]TierStats` array).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DailyTierStats {
    #[serde(rename = "Bins")]
    pub bins: Vec<TierStats>,
    #[serde(rename = "UpdatedAt")]
    pub updated_at: String,
}

impl Default for DailyTierStats {
    fn default() -> Self {
        Self {
            bins: vec![TierStats::default(); 24],
            updated_at: GO_ZERO_TIME.to_string(),
        }
    }
}

/// `madmin.TierInfo` from `tier-stats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TierInfo {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Type")]
    pub tier_type: String,
    #[serde(rename = "Stats")]
    pub stats: TierStats,
    #[serde(rename = "DailyStats")]
    pub daily_stats: DailyTierStats,
}

pub async fn add_tier(client: &AdminClient, config: &TierConfig, force: bool) -> Result<()> {
    let body = encrypt_data(client.secret_key(), &serde_json::to_vec(config)?)?;
    let force = if force { "true" } else { "false" };
    client
        .admin("PUT", "tier", &[("force", force)], body)
        .await?;
    Ok(())
}

pub async fn list_tiers(client: &AdminClient) -> Result<Vec<TierConfig>> {
    let response = client.admin("GET", "tier", &[], Vec::new()).await?;
    let tiers: Option<Vec<TierConfig>> = serde_json::from_slice(&response.body)?;
    Ok(tiers.unwrap_or_default())
}

pub async fn edit_tier(client: &AdminClient, name: &str, creds: &TierCreds) -> Result<()> {
    let body = encrypt_data(client.secret_key(), &serde_json::to_vec(creds)?)?;
    client
        .admin("POST", &format!("tier/{name}"), &[], body)
        .await?;
    Ok(())
}

pub async fn remove_tier(client: &AdminClient, name: &str, force: bool) -> Result<()> {
    let force = if force { "true" } else { "false" };
    client
        .admin(
            "DELETE",
            &format!("tier/{name}"),
            &[("force", force)],
            Vec::new(),
        )
        .await?;
    Ok(())
}

pub async fn verify_tier(client: &AdminClient, name: &str) -> Result<()> {
    client
        .admin("GET", &format!("tier/{name}"), &[], Vec::new())
        .await?;
    Ok(())
}

/// `madmin.TierInfo` list from `tier-stats`.
pub async fn tier_stats(client: &AdminClient) -> Result<Vec<TierInfo>> {
    let response = client.admin("GET", "tier-stats", &[], Vec::new()).await?;
    let mut infos: Vec<TierInfo> =
        serde_json::from_slice::<Option<Vec<TierInfo>>>(&response.body)?.unwrap_or_default();
    for info in &mut infos {
        info.daily_stats.bins.resize(24, TierStats::default());
    }
    Ok(infos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Vectors from madmin-go `encrypt_test.go` (argon2id + AES-GCM, produced by Go).
    #[test]
    fn decrypts_madmin_test_vectors() {
        let vectors: &[(&str, &str, usize)] = &[
            (
                "",
                "828aa81599df0651c0461adb82283e8b89956baee9f6e719947ef9cddc849028001dc9d3ac0938f66b07bacc9751437e1985f8a9763c240e81",
                0,
            ),
            (
                r#"xPl.8/rhR"Q_1xLt"#,
                "b5c016e93b84b473fc8a37af94936563630c36d6df1841d23a86ee51ca161f9e00ac19116b32f643ff6a56a212b265d8c56195bb0d12ce199e13dfdc5272f80c1564da2c6fc2fa18da91d8062de02af5cdafea491c6f3cae1f",
                32,
            ),
        ];
        for (password, data, len) in vectors {
            let plain = decrypt_response(password, &hex(data)).unwrap();
            assert_eq!(plain, vec![0u8; *len]);
        }
        assert!(decrypt_response("nope", &hex(vectors[1].1)).is_err());
        // Same key and stream construction with ChaCha20-Poly1305 (id 0x01).
        let salt = [7u8; 32];
        let nonce = [9u8; 8];
        let key = argon2id_key("pw", &salt);
        let mut sealed = salt.to_vec();
        sealed.push(0x01);
        sealed.extend_from_slice(&nonce);
        sealed.extend(
            sio_seal(
                &aws_lc_rs::aead::CHACHA20_POLY1305,
                &key,
                &nonce,
                b"{\"a\":1}",
            )
            .unwrap(),
        );
        assert_eq!(decrypt_response("pw", &sealed).unwrap(), b"{\"a\":1}");
    }

    #[test]
    fn encrypt_round_trips_across_fragments() {
        for len in [
            0,
            1,
            SIO_BUF_SIZE - 1,
            SIO_BUF_SIZE,
            SIO_BUF_SIZE + 1,
            3 * SIO_BUF_SIZE + 7,
        ] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let sealed = encrypt_data("secret", &data).unwrap();
            assert_eq!(sealed[32], PBKDF2_AES_GCM);
            let fragments = len.div_ceil(SIO_BUF_SIZE).max(1);
            assert_eq!(sealed.len(), 41 + len + TAG_LEN * fragments);
            assert_eq!(decrypt_data("secret", &sealed, None).unwrap(), data);
        }
    }

    /// Fixed salt/nonce vector: PBKDF2-SHA256(8192) key, sio AES-256-GCM stream.
    #[test]
    fn encryption_matches_sio_construction() {
        let sealed = encrypt_data_with("pw", b"{}", &[1; 32], &[2; 8]).unwrap();
        assert_eq!(&sealed[..32], &[1; 32]);
        assert_eq!(sealed[32], PBKDF2_AES_GCM);
        assert_eq!(&sealed[33..41], &[2; 8]);
        // Recompute the single final fragment by hand.
        let key = pbkdf2_key("pw", &[1; 32]);
        let aead = aead_key(&aws_lc_rs::aead::AES_256_GCM, &key).unwrap();
        let mut header_tag = Vec::new();
        let mut nonce0 = [2u8; 12];
        nonce0[8..].copy_from_slice(&0u32.to_le_bytes());
        aead.seal_in_place_append_tag(
            aws_lc_rs::aead::Nonce::assume_unique_for_key(nonce0),
            aws_lc_rs::aead::Aad::empty(),
            &mut header_tag,
        )
        .unwrap();
        let mut ad = vec![0x80];
        ad.extend(header_tag);
        let mut nonce1 = [2u8; 12];
        nonce1[8..].copy_from_slice(&1u32.to_le_bytes());
        let mut body = b"{}".to_vec();
        aead.seal_in_place_append_tag(
            aws_lc_rs::aead::Nonce::assume_unique_for_key(nonce1),
            aws_lc_rs::aead::Aad::from(ad.as_slice()),
            &mut body,
        )
        .unwrap();
        assert_eq!(to_hex(&sealed[41..]), to_hex(&body));
    }

    #[test]
    fn signs_admin_request() {
        // 2015-08-30T12:36:00Z
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_440_938_160);
        let headers = vec![("host", "example.amazonaws.com".to_string())];
        let signed = sign_headers(
            "GET",
            "https://example.amazonaws.com/minio/admin/v3/get-bucket-quota?bucket=b1",
            &headers,
            b"",
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            None,
            time,
        )
        .unwrap();
        let get = |name: &str| {
            signed
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(get("x-amz-date"), "20150830T123600Z");
        let empty_sha = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(get("x-amz-content-sha256"), empty_sha);

        // Independent SigV4 computation (canonical request -> string to sign -> HMAC chain).
        use aws_lc_rs::{digest, hmac};
        let canonical = format!(
            "GET\n/minio/admin/v3/get-bucket-quota\nbucket=b1\nhost:example.amazonaws.com\nx-amz-content-sha256:{empty_sha}\nx-amz-date:20150830T123600Z\n\nhost;x-amz-content-sha256;x-amz-date\n{empty_sha}"
        );
        let hash = digest::digest(&digest::SHA256, canonical.as_bytes());
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/s3/aws4_request\n{}",
            to_hex(hash.as_ref())
        );
        let mac = |key: &[u8], data: &str| {
            hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data.as_bytes())
                .as_ref()
                .to_vec()
        };
        let k = mac(b"AWS4wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "20150830");
        let k = mac(&k, "us-east-1");
        let k = mac(&k, "s3");
        let k = mac(&k, "aws4_request");
        let expected = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature={}",
            to_hex(&mac(&k, &to_sign))
        );
        assert_eq!(get("authorization"), expected);
    }

    #[test]
    fn encodes_query_components() {
        assert_eq!(
            encode_query(&[("bucket", "a b"), ("replication", "")]),
            "?bucket=a%20b&replication="
        );
        assert_eq!(encode_path("tier/WARM TIER"), "tier/WARM%20TIER");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(ibytes(1 << 30), "1.0 GiB");
        assert_eq!(ibytes(5), "5 B");
        assert_eq!(ibytes(1_000_000_000), "954 MiB");
        assert_eq!(si_bytes(2_000_000), "2.0 MB");
        assert_eq!(comma(1_234_567), "1,234,567");
        assert_eq!(comma(-1000), "-1,000");
        assert_eq!(comma(12), "12");
    }

    #[test]
    fn maps_admin_errors_like_madmin() {
        let json = Response {
            status: 404,
            body: br#"{"Code":"XMinioAdminNoSuchQuotaConfiguration","Message":"The quota configuration does not exist","Resource":"/x","RequestId":"R1","HostId":"H1"}"#.to_vec(),
            ..Default::default()
        };
        let err = madmin_error(&json);
        assert_eq!(err.message, "The quota configuration does not exist");
        assert_eq!(
            err.code.as_deref(),
            Some("XMinioAdminNoSuchQuotaConfiguration")
        );
        assert_eq!(
            serde_json::to_string(&err.detail).unwrap(),
            r#"{"Code":"XMinioAdminNoSuchQuotaConfiguration","Message":"The quota configuration does not exist","BucketName":"","Key":"","RequestID":"R1","HostID":"H1","Region":""}"#
        );
        let xml = Response {
            status: 404,
            body: b"<?xml version=\"1.0\"?><Error><Code>NoSuchBucket</Code><Message>The specified bucket does not exist</Message><BucketName>b</BucketName></Error>".to_vec(),
            ..Default::default()
        };
        assert_eq!(error_message(&xml), "The specified bucket does not exist");
        assert_eq!(madmin_error(&xml).detail.get("BucketName").unwrap(), "b");
        assert_eq!(error_code(&xml).as_deref(), Some("NoSuchBucket"));
        let empty = Response {
            status: 403,
            ..Default::default()
        };
        let err = madmin_error(&empty);
        assert_eq!(err.code.as_deref(), Some("403 Forbidden"));
        assert_eq!(
            err.message,
            "Failed to parse server response (unexpected end of JSON input): "
        );
        // S3 API errors (bucket sub-resources) keep minio-go's shape.
        let err = check_s3_status(xml, "b").unwrap_err();
        let mapped = crate::error::mc_error(&err).unwrap();
        assert_eq!(mapped.message, "The specified bucket does not exist");
        assert!(mapped.detail.get("Server").is_some());
        assert_eq!(go_method("GET"), "Get");
    }

    #[test]
    fn parses_sizes_like_go_humanize() {
        assert_eq!(parse_bytes("1GB").unwrap(), 1_000_000_000);
        assert_eq!(parse_bytes("64MiB").unwrap(), 64 << 20);
        assert_eq!(parse_bytes("64mi").unwrap(), 64 << 20);
        assert_eq!(parse_bytes("1.5 k").unwrap(), 1500);
        assert_eq!(parse_bytes("1,024").unwrap(), 1024);
        assert_eq!(parse_bytes("42").unwrap(), 42);
        assert_eq!(parse_bytes("7b").unwrap(), 7);
        assert_eq!(
            parse_bytes("abc").unwrap_err().to_string(),
            r#"strconv.ParseFloat: parsing "": invalid syntax"#
        );
        assert_eq!(
            parse_bytes("5 zonks").unwrap_err().to_string(),
            "unhandled size name: zonks"
        );
    }

    #[test]
    fn serializes_floats_like_go() {
        #[derive(Serialize)]
        struct F {
            #[serde(serialize_with = "go_f64")]
            a: f64,
            #[serde(serialize_with = "go_f64")]
            b: f64,
            #[serde(serialize_with = "go_f32")]
            c: f32,
        }
        assert_eq!(
            serde_json::to_string(&F {
                a: 0.0,
                b: 2.189294201170696e-21,
                c: 1.5
            })
            .unwrap(),
            r#"{"a":0,"b":2.189294201170696e-21,"c":1.5}"#
        );
    }

    #[test]
    fn tier_config_json_matches_madmin() {
        // `ListTiers` response (madmin `TierConfig.Clone()` fills every section).
        let json = r#"{"Version":"v1","Type":"minio","Name":"WARM","S3":{},"Azure":{"SPAuth":{}},"GCS":{},"MinIO":{"Endpoint":"http://h:9000","AccessKey":"a","SecretKey":"REDACTED","Bucket":"b","Prefix":"p/"}}"#;
        let tier: TierConfig = serde_json::from_str(json).unwrap();
        assert_eq!(tier.endpoint(), "http://h:9000");
        assert_eq!(tier.region(), "");
        assert_eq!(tier.field("Prefix"), "p/");
        assert_eq!(serde_json::to_string(&tier).unwrap(), json);
        let creds = TierCreds {
            access_key: "a".into(),
            secret_key: "s".into(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&creds).unwrap(),
            r#"{"access":"a","secret":"s","awsrole":false,"azSP":{}}"#
        );
        let creds = TierCreds {
            creds_json: b"{}".to_vec(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&creds).unwrap(),
            r#"{"awsrole":false,"azSP":{},"creds":"e30="}"#
        );
    }

    #[test]
    fn tier_info_defaults_like_go() {
        let info: TierInfo =
            serde_json::from_str(r#"{"Name":"W","Type":"minio","Stats":{"totalSize":5}}"#).unwrap();
        let value = serde_json::to_value(&info).unwrap();
        assert_eq!(value["Stats"]["numObjects"], 0);
        assert_eq!(value["DailyStats"]["Bins"].as_array().unwrap().len(), 24);
        assert_eq!(value["DailyStats"]["UpdatedAt"], GO_ZERO_TIME);
    }

    #[test]
    fn splits_json_documents_across_chunks() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Doc {
            n: u32,
        }
        let mut buf = Vec::new();
        let mut got = Vec::new();
        // Keep-alive spaces, a document split mid-number, two documents in one chunk.
        for chunk in [" ", "  {\"n\":1", "2}\n", " {\"n\":3}{\"n\"", ":4}\n "] {
            buf.extend_from_slice(chunk.as_bytes());
            while let Some(doc) = next_document::<Doc>(&mut buf).unwrap() {
                got.push(doc.n);
            }
        }
        assert_eq!(got, vec![12, 3, 4]);
        assert!(buf.iter().all(u8::is_ascii_whitespace));
        let mut bad = b"{\"n\":\"x\"}".to_vec();
        assert!(next_document::<Doc>(&mut bad).is_err());
    }

    #[test]
    fn detects_broken_pipe() {
        let err = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            .context("writing");
        assert!(is_broken_pipe(&err));
        assert!(!is_broken_pipe(&anyhow!("other")));
    }

    #[test]
    fn request_builder_encrypts_and_decrypts() {
        let secret = "minio123";
        let reply = encrypt_data(secret, br#"{"u1":{"status":"enabled"}}"#).unwrap();
        let server = mock::serve(vec![mock::reply(200, &reply)]);
        let client = mock::client(&server, secret);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let users: serde_json::Value = rt
            .block_on(
                client
                    .request("PUT", "add-user")
                    .query("accessKey", "a b")
                    .encrypted_json(&serde_json::json!({"secretKey": "s"}))
                    .unwrap()
                    .decrypt()
                    .send_json(),
            )
            .unwrap();
        assert_eq!(users["u1"]["status"], "enabled");
        let request = server.requests().remove(0);
        assert!(
            request
                .head
                .starts_with("PUT /minio/admin/v3/add-user?accessKey=a%20b HTTP/1.1"),
            "{}",
            request.head
        );
        assert!(
            request
                .head
                .to_ascii_lowercase()
                .contains("authorization: aws4-hmac-sha256")
        );
        assert_eq!(
            decrypt_response(secret, &request.body).unwrap(),
            br#"{"secretKey":"s"}"#
        );
    }

    #[test]
    fn convenience_helpers_map_errors() {
        let server = mock::serve(vec![
            mock::reply(200, br#"{"a":1}"#),
            mock::reply(
                404,
                br#"{"Code":"XMinioAdminNoSuchUser","Message":"The specified user does not exist."}"#,
            ),
            mock::reply(200, b""),
        ]);
        let client = mock::client(&server, "s");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let value: serde_json::Value = rt.block_on(client.get_json("info", &[("x", "1")])).unwrap();
        assert_eq!(value["a"], 1);
        let err = rt
            .block_on(client.post_json("user-info", &[], &serde_json::json!({})))
            .unwrap_err();
        let mapped = crate::error::mc_error(&err).unwrap();
        assert_eq!(mapped.message, "The specified user does not exist.");
        assert_eq!(mapped.code.as_deref(), Some("XMinioAdminNoSuchUser"));
        rt.block_on(client.delete("remove-user", &[("accessKey", "u")]))
            .unwrap();
        let heads: Vec<String> = server.requests().into_iter().map(|r| r.head).collect();
        assert!(heads[0].starts_with("GET /minio/admin/v3/info?x=1 "));
        assert!(heads[1].starts_with("POST /minio/admin/v3/user-info "));
        assert!(heads[2].starts_with("DELETE /minio/admin/v3/remove-user?accessKey=u "));
    }

    #[test]
    fn streams_chunked_json() {
        #[derive(Debug, Deserialize)]
        struct Entry {
            path: String,
        }
        let server = mock::serve(vec![mock::chunked(
            200,
            &[" ", "{\"path\":\"/a\"}\n", " ", "{\"path\":", "\"/b\"}\n"],
        )]);
        let client = mock::client(&server, "s");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let paths = rt
            .block_on(async {
                let stream = client
                    .request("GET", "trace")
                    .query("all", "true")
                    .stream()
                    .await?;
                let mut paths = Vec::new();
                stream
                    .for_each(|entry: Entry| {
                        paths.push(entry.path);
                        Ok(())
                    })
                    .await?;
                anyhow::Ok(paths)
            })
            .unwrap();
        assert_eq!(paths, vec!["/a", "/b"]);

        // A closed stdout ends the stream without an error.
        let server = mock::serve(vec![mock::chunked(200, &["{}\n{}\n"])]);
        let client = mock::client(&server, "s");
        let result = rt.block_on(async {
            let stream = client.request("GET", "trace").stream().await?;
            stream
                .for_each(|_: serde_json::Value| {
                    Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe).into())
                })
                .await
        });
        assert!(result.is_ok());

        let server = mock::serve(vec![mock::reply(
            403,
            br#"{"Code":"AccessDenied","Message":"Access Denied."}"#,
        )]);
        let client = mock::client(&server, "s");
        let err = rt
            .block_on(client.request("GET", "log").stream())
            .err()
            .unwrap();
        assert_eq!(
            crate::error::mc_error(&err).unwrap().message,
            "Access Denied."
        );
    }
}

/// Minimal HTTP/1.1 server for unit tests: serves canned responses in order, one request
/// per connection, and records the requests.
#[cfg(test)]
pub(crate) mod mock {
    use super::AdminClient;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub struct Request {
        /// Request line and headers.
        pub head: String,
        pub body: Vec<u8>,
    }

    pub struct Server {
        pub url: String,
        requests: Arc<Mutex<Vec<Request>>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Server {
        /// Requests received so far (waits until every canned response was served).
        pub fn requests(mut self) -> Vec<Request> {
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
            std::mem::take(&mut *self.requests.lock().unwrap())
        }
    }

    /// Response: status line + headers, then body chunks (written with small pauses).
    pub struct Reply {
        head: String,
        chunks: Vec<Vec<u8>>,
        chunked: bool,
    }

    pub fn reply(status: u16, body: &[u8]) -> Reply {
        Reply {
            head: format!(
                "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            ),
            chunks: vec![body.to_vec()],
            chunked: false,
        }
    }

    pub fn chunked(status: u16, chunks: &[&str]) -> Reply {
        Reply {
            head: format!(
                "HTTP/1.1 {status} X\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
            ),
            chunks: chunks.iter().map(|c| c.as_bytes().to_vec()).collect(),
            chunked: true,
        }
    }

    pub fn serve(replies: Vec<Reply>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let handle = std::thread::spawn(move || {
            for reply in replies {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let length = head
                    .lines()
                    .find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                recorded.lock().unwrap().push(Request { head, body });
                let mut stream = stream;
                stream.write_all(reply.head.as_bytes()).unwrap();
                for chunk in &reply.chunks {
                    if reply.chunked {
                        let _ = write!(stream, "{:x}\r\n", chunk.len());
                        let _ = stream.write_all(chunk);
                        let _ = stream.write_all(b"\r\n");
                        let _ = stream.flush();
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    } else {
                        let _ = stream.write_all(chunk);
                    }
                }
                if reply.chunked {
                    let _ = stream.write_all(b"0\r\n\r\n");
                }
            }
        });
        Server {
            url,
            requests,
            handle: Some(handle),
        }
    }

    pub fn client(server: &Server, secret_key: &str) -> AdminClient {
        AdminClient::new(&crate::config::model::AliasConfig {
            url: server.url.clone(),
            access_key: "minio".into(),
            secret_key: secret_key.into(),
            api: "S3v4".into(),
            path: "auto".into(),
            ..Default::default()
        })
        .unwrap()
    }
}
