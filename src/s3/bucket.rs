//! Bucket-level operations: mb/rb, versioning, CORS, encryption, policy, tags, presign, ping
//! (area F, except mb/rb).
//!
//! MinIO extensions that the SDK types cannot express (versioning excluded prefixes, lifecycle
//! `ExpiredObjectAllVersions`, raw CORS XML) are sent by replacing the serialized request body
//! ([`RawBody`]) and read by capturing the raw response ([`Capture`]).

use super::{ObjectInfo, build_client, delete_all_versions, delete_keys, list_object_infos};
use crate::config::model::AliasConfig;
use anyhow::{Context, Result, anyhow, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{ConfigBag, Intercept, RuntimeComponents};
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::SdkBody;
use aws_sdk_s3::types::{
    BucketLocationConstraint, BucketVersioningStatus, CorsConfiguration, CorsRule,
    CreateBucketConfiguration, ServerSideEncryption, ServerSideEncryptionByDefault,
    ServerSideEncryptionConfiguration, ServerSideEncryptionRule, Tag, Tagging,
    VersioningConfiguration,
};
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::context::{
    AfterDeserializationInterceptorContextRef, BeforeDeserializationInterceptorContextRef,
    BeforeTransmitInterceptorContextMut,
};
use base64::Engine;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Options for [`make_bucket_with`].
#[derive(Debug, Clone, Default)]
pub struct MakeBucketOptions {
    pub ignore_existing: bool,
    pub with_versioning: bool,
    /// Enables object lock (S3 implies versioning).
    pub with_lock: bool,
    pub region: Option<String>,
}

pub async fn make_bucket(alias: &AliasConfig, bucket: &str, ignore_existing: bool) -> Result<()> {
    make_bucket_with(
        alias,
        bucket,
        &MakeBucketOptions {
            ignore_existing,
            ..Default::default()
        },
    )
    .await
}

pub async fn make_bucket_with(
    alias: &AliasConfig,
    bucket: &str,
    options: &MakeBucketOptions,
) -> Result<()> {
    let client = build_client(alias).await?;
    let mut request = client.create_bucket().bucket(bucket);
    if options.with_lock {
        request = request.object_lock_enabled_for_bucket(true);
    }
    if let Some(region) = options
        .region
        .as_deref()
        .filter(|region| !region.is_empty() && *region != "us-east-1")
    {
        request = request.create_bucket_configuration(
            CreateBucketConfiguration::builder()
                .location_constraint(BucketLocationConstraint::from(region))
                .build(),
        );
    }
    let mut created = true;
    if let Err(error) = request.send().await {
        let already_exists = error
            .as_service_error()
            .and_then(ProvideErrorMetadata::code)
            .is_some_and(|code| matches!(code, "BucketAlreadyExists" | "BucketAlreadyOwnedByYou"));
        if !(options.ignore_existing && already_exists) {
            return Err(error.into());
        }
        created = false;
    }
    if created && options.with_versioning && !options.with_lock {
        set_versioning_client(&client, bucket, true).await?;
    }
    Ok(())
}

pub async fn remove_bucket(alias: &AliasConfig, bucket: &str, force: bool) -> Result<()> {
    let client = build_client(alias).await?;
    if force {
        delete_all_versions(&client, bucket).await?;
        let keys = list_object_infos(alias, bucket, None)
            .await?
            .into_iter()
            .map(|item| item.key)
            .collect::<Vec<_>>();
        delete_keys(&client, bucket, &keys).await?;
    }
    client.delete_bucket().bucket(bucket).send().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// raw request/response helpers
// ---------------------------------------------------------------------------

/// Replaces the SDK-serialized request body with `self.0` (sets Content-Length/Content-MD5).
/// Runs before the SDK computes request checksums, so those cover the new body.
#[derive(Debug)]
struct RawBody(Vec<u8>);

impl Intercept for RawBody {
    fn name(&self) -> &'static str {
        "mx-raw-body"
    }

    fn modify_before_retry_loop(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let request = context.request_mut();
        *request.body_mut() = SdkBody::from(self.0.clone());
        let md5 = base64::engine::general_purpose::STANDARD.encode(Md5::digest(&self.0));
        let headers = request.headers_mut();
        headers.insert("content-length", self.0.len().to_string());
        headers.insert("content-md5", md5);
        Ok(())
    }
}

/// Raw HTTP response captured by [`Capture`].
#[derive(Debug, Clone, Default)]
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RawResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Records the status, headers and (buffered) body of the response.
#[derive(Debug, Clone, Default)]
pub(crate) struct Capture(Arc<Mutex<Option<RawResponse>>>);

impl Capture {
    pub(crate) fn take(&self) -> Option<RawResponse> {
        self.0.lock().expect("capture lock").take()
    }

    /// Per-request config override installing this capture.
    pub(crate) fn config(&self) -> aws_sdk_s3::config::Builder {
        aws_sdk_s3::config::Builder::default().interceptor(self.clone())
    }
}

impl Intercept for Capture {
    fn name(&self) -> &'static str {
        "mx-capture-response"
    }

    fn read_after_transmit(
        &self,
        context: &BeforeDeserializationInterceptorContextRef<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let response = context.response();
        *self.0.lock().expect("capture lock") = Some(RawResponse {
            status: response.status().as_u16(),
            headers: response
                .headers()
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            body: Vec::new(),
        });
        Ok(())
    }

    fn read_after_deserialization(
        &self,
        context: &AfterDeserializationInterceptorContextRef<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        if let (Some(bytes), Some(raw)) = (
            context.response().body().bytes(),
            self.0.lock().expect("capture lock").as_mut(),
        ) {
            raw.body = bytes.to_vec();
        }
        Ok(())
    }
}

/// Rewrites the request path and query (used to reach MinIO's unsigned health endpoints with
/// the alias' configured transport).
#[derive(Debug)]
struct RewritePath {
    path: String,
    query: Option<String>,
}

impl Intercept for RewritePath {
    fn name(&self) -> &'static str {
        "mx-rewrite-path"
    }

    fn modify_before_signing(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let request = context.request_mut();
        let mut uri = url::Url::parse(request.uri())?;
        uri.set_path(&self.path);
        uri.set_query(self.query.as_deref());
        request.set_uri(uri.to_string())?;
        Ok(())
    }
}

/// Per-request config override replacing the request body with `body`.
pub(crate) fn raw_body_override(body: String) -> aws_sdk_s3::config::Builder {
    aws_sdk_s3::config::Builder::default().interceptor(RawBody(body.into_bytes()))
}

pub(crate) fn has_error_code<E, R>(
    error: &aws_sdk_s3::error::SdkError<E, R>,
    codes: &[&str],
) -> bool
where
    E: ProvideErrorMetadata,
{
    super::error_code(error).is_some_and(|code| codes.contains(&code))
}

// ---------------------------------------------------------------------------
// minimal XML reader/writer
// ---------------------------------------------------------------------------

/// A parsed XML element (namespace prefixes are dropped; attributes are ignored).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XmlNode {
    pub name: String,
    pub text: String,
    pub children: Vec<XmlNode>,
}

impl XmlNode {
    pub fn parse(input: &str) -> Result<XmlNode> {
        let mut parser = XmlParser {
            input: input.as_bytes(),
            pos: 0,
        };
        parser.skip_misc()?;
        parser.element()
    }

    pub fn child(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|child| child.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a XmlNode> {
        self.children.iter().filter(move |child| child.name == name)
    }

    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|child| child.text.trim())
    }
}

struct XmlParser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl XmlParser<'_> {
    fn rest(&self) -> &[u8] {
        &self.input[self.pos..]
    }

    fn skip_until(&mut self, end: &str) -> Result<()> {
        let found =
            find_bytes(self.rest(), end.as_bytes()).ok_or_else(|| anyhow!("invalid XML"))?;
        self.pos += found + end.len();
        Ok(())
    }

    /// Skips whitespace, `<?...?>`, comments and doctype.
    fn skip_misc(&mut self) -> Result<()> {
        loop {
            while self.rest().first().is_some_and(u8::is_ascii_whitespace) {
                self.pos += 1;
            }
            if self.rest().starts_with(b"<?") {
                self.skip_until("?>")?;
            } else if self.rest().starts_with(b"<!--") {
                self.skip_until("-->")?;
            } else if self.rest().starts_with(b"<!") && !self.rest().starts_with(b"<![CDATA[") {
                self.skip_until(">")?;
            } else {
                return Ok(());
            }
        }
    }

    fn element(&mut self) -> Result<XmlNode> {
        if !self.rest().starts_with(b"<") {
            bail!("invalid XML: expected an element");
        }
        let close = find_bytes(self.rest(), b">").ok_or_else(|| anyhow!("invalid XML"))?;
        let tag = std::str::from_utf8(&self.rest()[1..close])?.to_string();
        self.pos += close + 1;
        let self_closing = tag.ends_with('/');
        let raw_name = tag
            .trim_end_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        let mut node = XmlNode {
            name: raw_name.rsplit(':').next().unwrap_or_default().to_string(),
            ..Default::default()
        };
        if self_closing {
            return Ok(node);
        }
        loop {
            if self.rest().is_empty() {
                bail!("invalid XML: unterminated <{raw_name}>");
            }
            if self.rest().starts_with(b"</") {
                let end = find_bytes(self.rest(), b">").ok_or_else(|| anyhow!("invalid XML"))?;
                let closing = std::str::from_utf8(&self.rest()[2..end])?
                    .trim()
                    .to_string();
                if closing != raw_name {
                    bail!("invalid XML: <{raw_name}> closed by </{closing}>");
                }
                self.pos += end + 1;
                return Ok(node);
            }
            if self.rest().starts_with(b"<![CDATA[") {
                self.pos += 9;
                let end = find_bytes(self.rest(), b"]]>").ok_or_else(|| anyhow!("invalid XML"))?;
                node.text
                    .push_str(std::str::from_utf8(&self.rest()[..end])?);
                self.pos += end + 3;
            } else if self.rest().starts_with(b"<!--") || self.rest().starts_with(b"<?") {
                self.skip_misc()?;
            } else if self.rest().starts_with(b"<") {
                node.children.push(self.element()?);
            } else {
                let end = find_bytes(self.rest(), b"<").unwrap_or(self.rest().len());
                node.text
                    .push_str(&xml_unescape(std::str::from_utf8(&self.rest()[..end])?)?);
                self.pos += end;
            }
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn xml_unescape(text: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find(';')
            .ok_or_else(|| anyhow!("invalid XML entity"))?;
        let entity = &rest[start + 1..start + end];
        match entity {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse().ok()
                } else {
                    None
                };
                out.push(
                    code.and_then(char::from_u32)
                        .ok_or_else(|| anyhow!("invalid XML entity `&{entity};`"))?,
                );
            }
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Escapes text for XML element content.
pub fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Appends `<name>escaped value</name>`.
pub fn xml_element(out: &mut String, name: &str, value: &str) {
    out.push_str(&format!("<{name}>{}</{name}>", xml_escape(value)));
}

pub const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

// ---------------------------------------------------------------------------
// health / ping
// ---------------------------------------------------------------------------

/// Liveness check via `ListBuckets` (signed).
pub async fn ping(alias: &AliasConfig) -> Result<Duration> {
    let client = build_client(alias).await?;
    let started = std::time::Instant::now();
    client.list_buckets().send().await?;
    Ok(started.elapsed())
}

/// Sends a GET to `path?query` on the alias endpoint (MinIO health API) and returns the raw
/// response status/headers. No retries; transport errors are returned as errors.
pub async fn health_request(
    alias: &AliasConfig,
    path: &str,
    query: Option<&str>,
) -> Result<RawResponse> {
    let client = build_client(alias).await?;
    let capture = Capture::default();
    let result = client
        .list_buckets()
        .customize()
        .config_override(
            capture
                .config()
                .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
                .interceptor(RewritePath {
                    path: path.to_string(),
                    query: query.map(str::to_string),
                }),
        )
        .send()
        .await;
    if let Some(response) = capture.take() {
        return Ok(response);
    }
    match result {
        Ok(_) => bail!("no response from `{}`", alias.url),
        Err(error) => Err(anyhow!("{}", aws_sdk_s3::error::DisplayErrorContext(error))),
    }
}

// ---------------------------------------------------------------------------
// presign
// ---------------------------------------------------------------------------

pub async fn presign_get(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    expire: Duration,
) -> Result<String> {
    presign_get_version(alias, bucket, key, None, expire).await
}

pub async fn presign_get_version(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
    expire: Duration,
) -> Result<String> {
    let client = build_client(alias).await?;
    let request = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .presigned(PresigningConfig::expires_in(expire)?)
        .await?;
    Ok(request.uri().to_string())
}

pub async fn presign_put(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    expire: Duration,
) -> Result<String> {
    let client = build_client(alias).await?;
    let request = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .presigned(PresigningConfig::expires_in(expire)?)
        .await?;
    Ok(request.uri().to_string())
}

/// Presigned POST form (browser-style upload), as produced by minio-go `PresignedPostPolicy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostPolicyForm {
    /// Bucket URL the form is POSTed to.
    pub url: String,
    /// Form fields (`key`, `bucket`, `policy`, `x-amz-*`, optional `Content-Type`).
    pub fields: BTreeMap<String, String>,
}

/// Inputs for [`sign_post_policy`].
#[derive(Debug, Clone)]
pub struct PostPolicyInput<'a> {
    pub bucket: &'a str,
    /// Exact key, or key prefix when `starts_with` is set.
    pub key: &'a str,
    pub starts_with: bool,
    pub content_type: Option<&'a str>,
    pub expiration: SystemTime,
    pub now: SystemTime,
    pub region: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub session_token: Option<&'a str>,
}

/// Builds the POST policy document (minio-go condition order) and SigV4-signs it.
/// Returns the form fields.
pub fn sign_post_policy(input: &PostPolicyInput<'_>) -> BTreeMap<String, String> {
    let date = utc_parts(input.now);
    let amz_date = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        date.year, date.month, date.day, date.hour, date.minute, date.second
    );
    let credential = format!(
        "{}/{:04}{:02}{:02}/{}/s3/aws4_request",
        input.access_key, date.year, date.month, date.day, input.region
    );
    let mut fields = BTreeMap::new();
    let mut conditions: Vec<(&str, &str, String)> = Vec::new();
    if let Some(content_type) = input.content_type {
        conditions.push(("eq", "$Content-Type", content_type.to_string()));
        fields.insert("Content-Type".to_string(), content_type.to_string());
    }
    conditions.push(("eq", "$bucket", input.bucket.to_string()));
    if input.starts_with {
        conditions.push(("starts-with", "$key", input.key.to_string()));
    } else {
        conditions.push(("eq", "$key", input.key.to_string()));
    }
    conditions.push(("eq", "$x-amz-date", amz_date.clone()));
    conditions.push(("eq", "$x-amz-algorithm", "AWS4-HMAC-SHA256".to_string()));
    conditions.push(("eq", "$x-amz-credential", credential.clone()));
    if let Some(token) = input.session_token {
        conditions.push(("eq", "$x-amz-security-token", token.to_string()));
    }
    let expiration = utc_parts(input.expiration);
    let conditions = conditions
        .iter()
        .map(|(kind, name, value)| {
            format!(
                "[{},{},{}]",
                serde_json::Value::from(*kind),
                serde_json::Value::from(*name),
                serde_json::Value::from(value.as_str())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let policy = format!(
        r#"{{"expiration":"{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z","conditions":[{conditions}]}}"#,
        expiration.year,
        expiration.month,
        expiration.day,
        expiration.hour,
        expiration.minute,
        expiration.second,
        expiration.millis
    );
    let policy_base64 = base64::engine::general_purpose::STANDARD.encode(policy);
    let signing_key =
        aws_sigv4::sign::v4::generate_signing_key(input.secret_key, input.now, input.region, "s3");
    let signature = aws_sigv4::sign::v4::calculate_signature(signing_key, policy_base64.as_bytes());
    fields.insert("bucket".to_string(), input.bucket.to_string());
    fields.insert("key".to_string(), input.key.to_string());
    fields.insert("policy".to_string(), policy_base64);
    fields.insert(
        "x-amz-algorithm".to_string(),
        "AWS4-HMAC-SHA256".to_string(),
    );
    fields.insert("x-amz-credential".to_string(), credential);
    fields.insert("x-amz-date".to_string(), amz_date);
    if let Some(token) = input.session_token {
        fields.insert("x-amz-security-token".to_string(), token.to_string());
    }
    fields.insert("x-amz-signature".to_string(), signature);
    fields
}

/// Presigned POST policy for uploading `key` (or any key starting with `key` when
/// `starts_with`) into `bucket` until `now + expire`.
pub async fn presign_post(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    starts_with: bool,
    content_type: Option<&str>,
    expire: Duration,
) -> Result<PostPolicyForm> {
    let client = build_client(alias).await?;
    let location = client
        .get_bucket_location()
        .bucket(bucket)
        .send()
        .await?
        .location_constraint()
        .map(|value| value.as_str().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "us-east-1".to_string());
    let now = SystemTime::now();
    let fields = sign_post_policy(&PostPolicyInput {
        bucket,
        key,
        starts_with,
        content_type,
        expiration: now + expire,
        now,
        region: &location,
        access_key: &alias.access_key,
        secret_key: &alias.secret_key,
        session_token: alias
            .session_token
            .as_deref()
            .filter(|token| !token.is_empty()),
    });
    Ok(PostPolicyForm {
        url: bucket_url(alias, bucket)?,
        fields,
    })
}

/// `http(s)://host[:port]/bucket/` or `http(s)://bucket.host[:port]/` depending on the
/// alias path style.
pub fn bucket_url(alias: &AliasConfig, bucket: &str) -> Result<String> {
    let mut url = url::Url::parse(&alias.url)?;
    if super::force_path_style(alias)? {
        let path = format!("{}/{bucket}/", url.path().trim_end_matches('/'));
        url.set_path(&path);
    } else {
        let host = url.host_str().unwrap_or_default().to_string();
        url.set_host(Some(&format!("{bucket}.{host}")))?;
        url.set_path("/");
    }
    Ok(url.to_string())
}

/// UTC calendar components of a timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcParts {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u64,
    pub minute: u64,
    pub second: u64,
    pub millis: u32,
}

/// Splits a timestamp into UTC calendar parts (Howard Hinnant's civil_from_days).
pub fn utc_parts(time: SystemTime) -> UtcParts {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    UtcParts {
        year,
        month,
        day,
        hour: rem / 3600,
        minute: rem % 3600 / 60,
        second: rem % 60,
        millis: since.subsec_millis(),
    }
}

// ---------------------------------------------------------------------------
// tags
// ---------------------------------------------------------------------------

pub async fn put_object_tags(
    alias: &AliasConfig,
    bucket: &str,
    key: Option<&str>,
    tags: Vec<(String, String)>,
) -> Result<()> {
    let client = build_client(alias).await?;
    put_tags(&client, bucket, key, None, &tags).await
}

/// Sets object (or bucket, when `key` is None) tags, optionally on a specific version.
pub async fn put_tags(
    client: &Client,
    bucket: &str,
    key: Option<&str>,
    version_id: Option<&str>,
    tags: &[(String, String)],
) -> Result<()> {
    let tag_set = tags
        .iter()
        .map(|(key, value)| Tag::builder().key(key).value(value).build())
        .collect::<Result<Vec<_>, _>>()?;
    let tagging = Tagging::builder().set_tag_set(Some(tag_set)).build()?;
    if let Some(key) = key {
        client
            .put_object_tagging()
            .bucket(bucket)
            .key(key)
            .set_version_id(version_id.map(str::to_string))
            .tagging(tagging)
            .send()
            .await?;
    } else {
        client
            .put_bucket_tagging()
            .bucket(bucket)
            .tagging(tagging)
            .send()
            .await?;
    }
    Ok(())
}

pub async fn get_object_tags(
    alias: &AliasConfig,
    bucket: &str,
    key: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let client = build_client(alias).await?;
    Ok(get_tags(&client, bucket, key, None)
        .await?
        .unwrap_or_default())
}

/// Reads object (or bucket, when `key` is None) tags, optionally of a specific version.
/// Returns None when the server reports `NoSuchTagSet`.
pub async fn get_tags(
    client: &Client,
    bucket: &str,
    key: Option<&str>,
    version_id: Option<&str>,
) -> Result<Option<Vec<(String, String)>>> {
    let tags = if let Some(key) = key {
        client
            .get_object_tagging()
            .bucket(bucket)
            .key(key)
            .set_version_id(version_id.map(str::to_string))
            .send()
            .await?
            .tag_set
    } else {
        match client.get_bucket_tagging().bucket(bucket).send().await {
            Ok(response) => response.tag_set,
            Err(error) if has_error_code(&error, &["NoSuchTagSet"]) => return Ok(None),
            Err(error) => return Err(error.into()),
        }
    };
    Ok(Some(
        tags.into_iter().map(|tag| (tag.key, tag.value)).collect(),
    ))
}

pub async fn delete_object_tags(
    alias: &AliasConfig,
    bucket: &str,
    key: Option<&str>,
) -> Result<()> {
    let client = build_client(alias).await?;
    delete_tags(&client, bucket, key, None).await
}

pub async fn delete_tags(
    client: &Client,
    bucket: &str,
    key: Option<&str>,
    version_id: Option<&str>,
) -> Result<()> {
    if let Some(key) = key {
        client
            .delete_object_tagging()
            .bucket(bucket)
            .key(key)
            .set_version_id(version_id.map(str::to_string))
            .send()
            .await?;
    } else {
        client.delete_bucket_tagging().bucket(bucket).send().await?;
    }
    Ok(())
}

/// All versions and delete markers of exactly `key`, newest first. `ObjectInfo::key` is the
/// full key.
pub async fn list_key_versions(
    client: &Client,
    bucket: &str,
    key: &str,
) -> Result<Vec<ObjectInfo>> {
    let mut key_marker: Option<String> = None;
    let mut version_marker: Option<String> = None;
    let mut items = Vec::new();
    loop {
        let response = client
            .list_object_versions()
            .bucket(bucket)
            .prefix(key)
            .set_key_marker(key_marker.take())
            .set_version_id_marker(version_marker.take())
            .send()
            .await?;
        for version in response.versions() {
            if version.key() == Some(key) {
                items.push(ObjectInfo {
                    key: key.to_string(),
                    size: version.size().unwrap_or(0),
                    last_modified: version.last_modified().and_then(super::to_system_time),
                    etag: version.e_tag().map(str::to_string),
                    version_id: version.version_id().map(str::to_string),
                    is_latest: version.is_latest().unwrap_or(false),
                    ..Default::default()
                });
            }
        }
        for marker in response.delete_markers() {
            if marker.key() == Some(key) {
                items.push(ObjectInfo {
                    key: key.to_string(),
                    last_modified: marker.last_modified().and_then(super::to_system_time),
                    version_id: marker.version_id().map(str::to_string),
                    is_latest: marker.is_latest().unwrap_or(false),
                    is_delete_marker: true,
                    ..Default::default()
                });
            }
        }
        if !response.is_truncated().unwrap_or(false) {
            break;
        }
        key_marker = response.next_key_marker().map(str::to_string);
        version_marker = response.next_version_id_marker().map(str::to_string);
        if key_marker.is_none() {
            break;
        }
    }
    items.sort_by(|left, right| {
        right
            .is_latest
            .cmp(&left.is_latest)
            .then_with(|| right.last_modified.cmp(&left.last_modified))
    });
    Ok(items)
}

// ---------------------------------------------------------------------------
// versioning
// ---------------------------------------------------------------------------

pub async fn set_versioning(alias: &AliasConfig, bucket: &str, enabled: bool) -> Result<()> {
    let client = build_client(alias).await?;
    set_versioning_client(&client, bucket, enabled).await
}

pub async fn set_versioning_client(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    enabled: bool,
) -> Result<()> {
    let status = if enabled {
        BucketVersioningStatus::Enabled
    } else {
        BucketVersioningStatus::Suspended
    };
    client
        .put_bucket_versioning()
        .bucket(bucket)
        .versioning_configuration(VersioningConfiguration::builder().status(status).build())
        .send()
        .await?;
    Ok(())
}

pub async fn get_versioning(alias: &AliasConfig, bucket: &str) -> Result<String> {
    let status = get_versioning_info(alias, bucket).await?.status;
    Ok(if status.is_empty() {
        "Off".to_string()
    } else {
        status
    })
}

/// Bucket versioning configuration including MinIO extensions. Serializes like the
/// `versioning` object of `mc version info --json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct VersioningInfo {
    /// `Enabled`, `Suspended`, or empty (never configured).
    #[serde(rename = "status")]
    pub status: String,
    #[serde(rename = "MFADelete")]
    pub mfa_delete: String,
    #[serde(rename = "ExcludedPrefixes", skip_serializing_if = "Vec::is_empty")]
    pub excluded_prefixes: Vec<String>,
    #[serde(rename = "ExcludeFolders", skip_serializing_if = "std::ops::Not::not")]
    pub exclude_folders: bool,
}

impl VersioningInfo {
    pub fn to_xml(&self) -> String {
        let mut out = format!(r#"<VersioningConfiguration xmlns="{S3_XMLNS}">"#);
        xml_element(&mut out, "Status", &self.status);
        if !self.mfa_delete.is_empty() {
            xml_element(&mut out, "MfaDelete", &self.mfa_delete);
        }
        for prefix in &self.excluded_prefixes {
            out.push_str("<ExcludedPrefixes>");
            xml_element(&mut out, "Prefix", prefix);
            out.push_str("</ExcludedPrefixes>");
        }
        if self.exclude_folders {
            xml_element(&mut out, "ExcludeFolders", "true");
        }
        out.push_str("</VersioningConfiguration>");
        out
    }

    pub fn from_xml(xml: &str) -> Result<Self> {
        let node = XmlNode::parse(xml)?;
        Ok(Self {
            status: node.child_text("Status").unwrap_or_default().to_string(),
            mfa_delete: node
                .child_text("MfaDelete")
                .or_else(|| node.child_text("MFADelete"))
                .unwrap_or_default()
                .to_string(),
            excluded_prefixes: node
                .children_named("ExcludedPrefixes")
                .filter_map(|item| item.child_text("Prefix"))
                .map(str::to_string)
                .collect(),
            exclude_folders: node
                .child_text("ExcludeFolders")
                .is_some_and(|value| value.eq_ignore_ascii_case("true")),
        })
    }
}

/// PutBucketVersioning with a raw body (supports MinIO `ExcludedPrefixes`/`ExcludeFolders`).
pub async fn put_versioning_info(
    alias: &AliasConfig,
    bucket: &str,
    info: &VersioningInfo,
) -> Result<()> {
    let client = build_client(alias).await?;
    client
        .put_bucket_versioning()
        .bucket(bucket)
        .versioning_configuration(
            VersioningConfiguration::builder()
                .status(BucketVersioningStatus::from(info.status.as_str()))
                .build(),
        )
        .customize()
        .config_override(raw_body_override(info.to_xml()))
        .send()
        .await?;
    Ok(())
}

pub async fn get_versioning_info(alias: &AliasConfig, bucket: &str) -> Result<VersioningInfo> {
    let client = build_client(alias).await?;
    let capture = Capture::default();
    let response = client
        .get_bucket_versioning()
        .bucket(bucket)
        .customize()
        .config_override(capture.config())
        .send()
        .await?;
    let raw = capture
        .take()
        .map(|raw| raw.body_text())
        .unwrap_or_default();
    let mut info = if raw.trim().is_empty() {
        VersioningInfo::default()
    } else {
        VersioningInfo::from_xml(&raw)?
    };
    if info.status.is_empty() {
        info.status = response
            .status()
            .map(|status| status.as_str().to_string())
            .unwrap_or_default();
    }
    Ok(info)
}

// ---------------------------------------------------------------------------
// CORS
// ---------------------------------------------------------------------------

/// AWS CLI style CORS JSON (`{"CORSRules":[{"AllowedOrigins":[...],...}]}`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CorsDocument {
    #[serde(rename = "CORSRules")]
    pub rules: Vec<CorsDocumentRule>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CorsDocumentRule {
    #[serde(rename = "ID", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "AllowedOrigins", default)]
    pub allowed_origins: Vec<String>,
    #[serde(rename = "AllowedMethods", default)]
    pub allowed_methods: Vec<String>,
    #[serde(rename = "AllowedHeaders", default)]
    pub allowed_headers: Vec<String>,
    #[serde(rename = "ExposeHeaders", default)]
    pub expose_headers: Vec<String>,
    #[serde(rename = "MaxAgeSeconds")]
    pub max_age_seconds: Option<i32>,
}

impl CorsDocument {
    /// `CORSConfiguration` XML.
    pub fn to_xml(&self) -> String {
        let mut out = format!(r#"<CORSConfiguration xmlns="{S3_XMLNS}">"#);
        for rule in &self.rules {
            out.push_str("<CORSRule>");
            for value in &rule.allowed_headers {
                xml_element(&mut out, "AllowedHeader", value);
            }
            for value in &rule.allowed_methods {
                xml_element(&mut out, "AllowedMethod", value);
            }
            for value in &rule.allowed_origins {
                xml_element(&mut out, "AllowedOrigin", value);
            }
            for value in &rule.expose_headers {
                xml_element(&mut out, "ExposeHeader", value);
            }
            if let Some(id) = &rule.id {
                xml_element(&mut out, "ID", id);
            }
            if let Some(max_age) = rule.max_age_seconds {
                xml_element(&mut out, "MaxAgeSeconds", &max_age.to_string());
            }
            out.push_str("</CORSRule>");
        }
        out.push_str("</CORSConfiguration>");
        out
    }

    pub fn from_xml(xml: &str) -> Result<Self> {
        let node = XmlNode::parse(xml)?;
        if node.name != "CORSConfiguration" {
            bail!("expected <CORSConfiguration>, found <{}>", node.name);
        }
        let texts = |rule: &XmlNode, name: &str| {
            rule.children_named(name)
                .map(|item| item.text.trim().to_string())
                .collect::<Vec<_>>()
        };
        let mut rules = Vec::new();
        for rule in node.children_named("CORSRule") {
            rules.push(CorsDocumentRule {
                id: rule.child_text("ID").map(str::to_string),
                allowed_origins: texts(rule, "AllowedOrigin"),
                allowed_methods: texts(rule, "AllowedMethod"),
                allowed_headers: texts(rule, "AllowedHeader"),
                expose_headers: texts(rule, "ExposeHeader"),
                max_age_seconds: rule
                    .child_text("MaxAgeSeconds")
                    .map(str::parse)
                    .transpose()
                    .context("invalid MaxAgeSeconds")?,
            });
        }
        Ok(Self { rules })
    }

    /// JSON in minio-go `cors.Config` shape (as printed by `mc cors get --json`).
    pub fn to_mc_json(&self) -> serde_json::Value {
        serde_json::json!({
            "CORSRules": self.rules.iter().map(|rule| serde_json::json!({
                "AllowedHeader": rule.allowed_headers,
                "AllowedMethod": rule.allowed_methods,
                "AllowedOrigin": rule.allowed_origins,
                "ExposeHeader": rule.expose_headers,
                "ID": rule.id.clone().unwrap_or_default(),
                "MaxAgeSeconds": rule.max_age_seconds.unwrap_or(0),
            })).collect::<Vec<_>>()
        })
    }
}

pub async fn put_cors(alias: &AliasConfig, bucket: &str, document: CorsDocument) -> Result<()> {
    put_cors_xml(alias, bucket, &document.to_xml()).await
}

/// PutBucketCors with a raw `CORSConfiguration` XML body (as `mc cors set` sends it).
pub async fn put_cors_xml(alias: &AliasConfig, bucket: &str, xml: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client
        .put_bucket_cors()
        .bucket(bucket)
        .cors_configuration(
            CorsConfiguration::builder()
                .set_cors_rules(Some(vec![
                    CorsRule::builder()
                        .allowed_methods("GET")
                        .allowed_origins("*")
                        .build()?,
                ]))
                .build()?,
        )
        .customize()
        .config_override(raw_body_override(xml.to_string()))
        .send()
        .await?;
    Ok(())
}

pub async fn get_cors(alias: &AliasConfig, bucket: &str) -> Result<CorsDocument> {
    get_cors_xml(alias, bucket)
        .await?
        .map(|(_, document)| document)
        .ok_or_else(|| anyhow!("No bucket CORS configuration found."))
}

/// Returns the raw CORS XML and its parsed form, or None when no CORS is configured.
pub async fn get_cors_xml(
    alias: &AliasConfig,
    bucket: &str,
) -> Result<Option<(String, CorsDocument)>> {
    let client = build_client(alias).await?;
    let capture = Capture::default();
    match client
        .get_bucket_cors()
        .bucket(bucket)
        .customize()
        .config_override(capture.config())
        .send()
        .await
    {
        Ok(_) => {
            let xml = capture
                .take()
                .map(|raw| raw.body_text())
                .unwrap_or_default();
            let document = CorsDocument::from_xml(&xml)?;
            Ok(Some((xml, document)))
        }
        Err(error) if has_error_code(&error, &["NoSuchCORSConfiguration"]) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub async fn delete_cors(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client.delete_bucket_cors().bucket(bucket).send().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// default encryption
// ---------------------------------------------------------------------------

/// Sets SSE-S3 (`kms_key_id` None) or SSE-KMS default bucket encryption.
pub async fn put_encryption(
    alias: &AliasConfig,
    bucket: &str,
    kms_key_id: Option<&str>,
) -> Result<()> {
    let client = build_client(alias).await?;
    let default = match kms_key_id {
        Some(key_id) => ServerSideEncryptionByDefault::builder()
            .sse_algorithm(ServerSideEncryption::AwsKms)
            .kms_master_key_id(key_id),
        None => {
            ServerSideEncryptionByDefault::builder().sse_algorithm(ServerSideEncryption::Aes256)
        }
    }
    .build()?;
    client
        .put_bucket_encryption()
        .bucket(bucket)
        .server_side_encryption_configuration(
            ServerSideEncryptionConfiguration::builder()
                .rules(
                    ServerSideEncryptionRule::builder()
                        .apply_server_side_encryption_by_default(default)
                        .build(),
                )
                .build()?,
        )
        .send()
        .await?;
    Ok(())
}

pub async fn put_encryption_s3(alias: &AliasConfig, bucket: &str) -> Result<()> {
    put_encryption(alias, bucket, None).await
}

/// Default bucket encryption as `(algorithm, kms key id)`; None when not configured.
pub async fn get_encryption_config(
    alias: &AliasConfig,
    bucket: &str,
) -> Result<Option<(String, Option<String>)>> {
    let client = build_client(alias).await?;
    let response = match client.get_bucket_encryption().bucket(bucket).send().await {
        Ok(response) => response,
        Err(error)
            if has_error_code(&error, &["ServerSideEncryptionConfigurationNotFoundError"]) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let mut result = None;
    for rule in response
        .server_side_encryption_configuration
        .map(|config| config.rules)
        .unwrap_or_default()
    {
        if let Some(default) = rule.apply_server_side_encryption_by_default {
            let key_id = default.kms_master_key_id.filter(|key| !key.is_empty());
            let found = key_id.is_some();
            result = Some((default.sse_algorithm.as_str().to_string(), key_id));
            if found {
                break;
            }
        }
    }
    Ok(result)
}

pub async fn get_encryption(alias: &AliasConfig, bucket: &str) -> Result<String> {
    Ok(get_encryption_config(alias, bucket)
        .await?
        .map(|(algorithm, _)| algorithm)
        .unwrap_or_else(|| "none".to_string()))
}

pub async fn delete_encryption(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client
        .delete_bucket_encryption()
        .bucket(bucket)
        .send()
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// bucket policy
// ---------------------------------------------------------------------------

pub async fn put_bucket_policy(alias: &AliasConfig, bucket: &str, policy: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client
        .put_bucket_policy()
        .bucket(bucket)
        .policy(policy)
        .send()
        .await?;
    Ok(())
}

pub async fn get_bucket_policy(alias: &AliasConfig, bucket: &str) -> Result<Option<String>> {
    let client = build_client(alias).await?;
    match client.get_bucket_policy().bucket(bucket).send().await {
        Ok(response) => Ok(response.policy.filter(|policy| !policy.trim().is_empty())),
        Err(error) if has_error_code(&error, &["NoSuchBucketPolicy"]) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub async fn delete_bucket_policy(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client.delete_bucket_policy().bucket(bucket).send().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_key_matches_aws_example() {
        // AWS SigV4 documentation example (secret, 20120215, us-east-1, iam).
        let time = UNIX_EPOCH + Duration::from_secs(1_329_264_000);
        let key = aws_sigv4::sign::v4::generate_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            time,
            "us-east-1",
            "iam",
        );
        let hex: String = key.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
    }

    #[test]
    fn post_policy_matches_reference_signature() {
        // 2015-12-29T00:00:00Z, expiring 2015-12-30T12:00:00Z; the reference values were
        // computed independently (Python hmac/hashlib/base64) over the same policy document.
        let now = UNIX_EPOCH + Duration::from_secs(1_451_347_200);
        let fields = sign_post_policy(&PostPolicyInput {
            bucket: "sigv4examplebucket",
            key: "user/user1/",
            starts_with: true,
            content_type: None,
            expiration: now + Duration::from_secs(36 * 3600),
            now,
            region: "us-east-1",
            access_key: "AKIAIOSFODNN7EXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            session_token: None,
        });
        assert_eq!(
            fields["policy"],
            "eyJleHBpcmF0aW9uIjoiMjAxNS0xMi0zMFQxMjowMDowMC4wMDBaIiwiY29uZGl0aW9ucyI6W1siZXEiLCIkYnVja2V0Iiwic2lndjRleGFtcGxlYnVja2V0Il0sWyJzdGFydHMtd2l0aCIsIiRrZXkiLCJ1c2VyL3VzZXIxLyJdLFsiZXEiLCIkeC1hbXotZGF0ZSIsIjIwMTUxMjI5VDAwMDAwMFoiXSxbImVxIiwiJHgtYW16LWFsZ29yaXRobSIsIkFXUzQtSE1BQy1TSEEyNTYiXSxbImVxIiwiJHgtYW16LWNyZWRlbnRpYWwiLCJBS0lBSU9TRk9ETk43RVhBTVBMRS8yMDE1MTIyOS91cy1lYXN0LTEvczMvYXdzNF9yZXF1ZXN0Il1dfQ=="
        );
        assert_eq!(
            fields["x-amz-signature"],
            "d1705f9fcf311e61497af2fd1bff140b21df94b85bfbdb218e56bd097433d4bc"
        );
        assert_eq!(fields["x-amz-date"], "20151229T000000Z");
        assert_eq!(
            fields["x-amz-credential"],
            "AKIAIOSFODNN7EXAMPLE/20151229/us-east-1/s3/aws4_request"
        );
        assert_eq!(fields["key"], "user/user1/");
        assert!(!fields.contains_key("Content-Type"));
    }

    #[test]
    fn post_policy_includes_content_type_and_token() {
        let now = UNIX_EPOCH + Duration::from_secs(1_451_347_200);
        let fields = sign_post_policy(&PostPolicyInput {
            bucket: "b",
            key: "k.png",
            starts_with: false,
            content_type: Some("image/png"),
            expiration: now + Duration::from_secs(60),
            now,
            region: "us-east-1",
            access_key: "a",
            secret_key: "s",
            session_token: Some("tok"),
        });
        let policy = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(&fields["policy"])
                .unwrap(),
        )
        .unwrap();
        assert!(policy.starts_with(
            r#"{"expiration":"2015-12-29T00:01:00.000Z","conditions":[["eq","$Content-Type","image/png"],["eq","$bucket","b"],["eq","$key","k.png"]"#
        ));
        assert!(policy.contains(r#"["eq","$x-amz-security-token","tok"]"#));
        assert_eq!(fields["Content-Type"], "image/png");
        assert_eq!(fields["x-amz-security-token"], "tok");
    }

    #[test]
    fn utc_parts_converts_dates() {
        let parts = utc_parts(UNIX_EPOCH + Duration::from_millis(951_827_696_123));
        assert_eq!(
            (
                parts.year,
                parts.month,
                parts.day,
                parts.hour,
                parts.minute,
                parts.second,
                parts.millis
            ),
            (2000, 2, 29, 12, 34, 56, 123)
        );
    }

    #[test]
    fn bucket_url_honors_path_style() {
        let mut alias = AliasConfig {
            url: "http://localhost:9000".into(),
            path: "auto".into(),
            ..Default::default()
        };
        assert_eq!(bucket_url(&alias, "b").unwrap(), "http://localhost:9000/b/");
        alias.url = "https://s3.amazonaws.com".into();
        alias.path = "dns".into();
        assert_eq!(
            bucket_url(&alias, "b").unwrap(),
            "https://b.s3.amazonaws.com/"
        );
    }

    #[test]
    fn parses_xml_with_namespaces_entities_and_cdata() {
        let node = XmlNode::parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- c --><a:Root xmlns:a="x"><A>1 &amp; 2 &#65;</A><B/><C><D><![CDATA[<x>]]></D></C></a:Root>"#,
        )
        .unwrap();
        assert_eq!(node.name, "Root");
        assert_eq!(node.child_text("A"), Some("1 & 2 A"));
        assert_eq!(node.child("B").unwrap().text, "");
        assert_eq!(node.child("C").unwrap().child_text("D"), Some("<x>"));
        assert!(XmlNode::parse("<A><B></A>").is_err());
        assert!(XmlNode::parse("<A>").is_err());
    }

    #[test]
    fn versioning_xml_round_trips_minio_extensions() {
        let info = VersioningInfo {
            status: "Enabled".into(),
            mfa_delete: String::new(),
            excluded_prefixes: vec!["tmp/".into(), "a&b/".into()],
            exclude_folders: true,
        };
        let xml = info.to_xml();
        assert_eq!(
            xml,
            format!(
                r#"<VersioningConfiguration xmlns="{S3_XMLNS}"><Status>Enabled</Status><ExcludedPrefixes><Prefix>tmp/</Prefix></ExcludedPrefixes><ExcludedPrefixes><Prefix>a&amp;b/</Prefix></ExcludedPrefixes><ExcludeFolders>true</ExcludeFolders></VersioningConfiguration>"#
            )
        );
        assert_eq!(VersioningInfo::from_xml(&xml).unwrap(), info);
        assert_eq!(
            VersioningInfo::from_xml("<VersioningConfiguration/>").unwrap(),
            VersioningInfo::default()
        );
    }

    #[test]
    fn cors_xml_round_trips() {
        let document = CorsDocument {
            rules: vec![CorsDocumentRule {
                id: Some("r1".into()),
                allowed_origins: vec!["https://example.com".into()],
                allowed_methods: vec!["GET".into(), "PUT".into()],
                allowed_headers: vec!["*".into()],
                expose_headers: vec!["ETag".into()],
                max_age_seconds: Some(300),
            }],
        };
        let parsed = CorsDocument::from_xml(&document.to_xml()).unwrap();
        assert_eq!(parsed, document);
        assert_eq!(
            document.to_mc_json()["CORSRules"][0]["AllowedMethod"],
            serde_json::json!(["GET", "PUT"])
        );
        assert!(CorsDocument::from_xml("<Other/>").is_err());
    }
}
