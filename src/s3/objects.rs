//! Single-object transfer (areas B/E): Put/Get/Copy options and helpers.
//!
//! Client-level functions (`&Client`) are the building blocks for bulk operations; alias-level
//! wrappers build a client per call and keep the historical signatures working.

use super::client::same_endpoint_and_credentials;
use super::{build_client, upload_stream};
use crate::config::model::AliasConfig;
use crate::flags::{ChecksumAlgo, Sse};
use crate::s3::S3ResultExt;
use anyhow::{Context, Result, anyhow, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::operation::get_object::GetObjectOutput;
use aws_sdk_s3::primitives::{ByteStream, DateTime, DateTimeFormat};
use aws_sdk_s3::types::{
    CompletedMultipartUpload, CompletedPart, MetadataDirective, TaggingDirective,
};
use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
use base64::Engine;
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::SystemTime;

/// Options applied to PutObject, CreateMultipartUpload/UploadPart/CompleteMultipartUpload,
/// and (as destination settings) CopyObject.
#[derive(Clone, Default)]
pub struct PutOptions {
    pub content_type: Option<String>,
    /// mc `--attr` pairs. Standard headers (Content-Type, Cache-Control, Content-Encoding,
    /// Content-Disposition, Content-Language, Expires; case-insensitive) are sent as those
    /// headers; everything else becomes user metadata (`x-amz-meta-` prefix optional).
    pub metadata: Vec<(String, String)>,
    pub tags: Vec<(String, String)>,
    pub storage_class: Option<String>,
    pub sse: Option<Sse>,
    /// Explicit checksum algorithm; sent even though the client defaults to `WhenRequired`.
    pub checksum: Option<ChecksumAlgo>,
    /// Always use a single PutObject (max 5 GiB).
    pub disable_multipart: bool,
    /// Fixed multipart part size in bytes (5 MiB..=5 GiB); also the single-PUT threshold.
    /// Default: minio-go's `OptimalPartInfo` (16 MiB, larger for huge objects, 528 MiB for
    /// unknown sizes).
    pub part_size: Option<u64>,
    /// Number of parts uploaded concurrently (default 4, minio-go `totalWorkers`).
    pub parallel: Option<usize>,
    /// Object lock legal hold ON/OFF.
    pub legal_hold: Option<bool>,
    /// Object lock retention: (`GOVERNANCE` | `COMPLIANCE`, retain-until).
    pub retention: Option<(String, SystemTime)>,
}

impl std::fmt::Debug for PutOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PutOptions")
            .field("content_type", &self.content_type)
            .field("metadata", &self.metadata)
            .field("tags", &self.tags)
            .field("storage_class", &self.storage_class)
            .field("sse", &self.sse)
            .field("checksum", &self.checksum)
            .field("disable_multipart", &self.disable_multipart)
            .field("part_size", &self.part_size)
            .field("parallel", &self.parallel)
            .field("legal_hold", &self.legal_hold)
            .field("retention", &self.retention)
            .finish()
    }
}

/// Options for GetObject / HeadObject.
#[derive(Clone, Default)]
pub struct GetOptions {
    pub version_id: Option<String>,
    /// SSE-C key the object was encrypted with.
    pub sse_c: Option<[u8; 32]>,
    /// Byte range `(start, Some(end_inclusive))` or `(start, None)` for "to the end".
    pub range: Option<(u64, Option<u64>)>,
    pub part_number: Option<i32>,
    /// MinIO: read a file inside a zip object (`x-minio-extract: true`).
    pub zip_extract: bool,
}

impl std::fmt::Debug for GetOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GetOptions")
            .field("version_id", &self.version_id)
            .field("sse_c", &self.sse_c.map(|_| "<redacted>"))
            .field("range", &self.range)
            .field("part_number", &self.part_number)
            .field("zip_extract", &self.zip_extract)
            .finish()
    }
}

/// Result of an upload or copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PutOutcome {
    /// Bytes written; `None` for server-side copies (size not returned by CopyObject).
    pub size: Option<i64>,
    pub etag: Option<String>,
    pub version_id: Option<String>,
}

/// An object location on a specific alias.
#[derive(Debug, Clone, Copy)]
pub struct ObjectRef<'a> {
    pub alias: &'a AliasConfig,
    pub bucket: &'a str,
    pub key: &'a str,
}

/// Standard headers + user metadata split out of [`PutOptions`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectHeaders {
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
    pub content_encoding: Option<String>,
    pub content_disposition: Option<String>,
    pub content_language: Option<String>,
    pub expires: Option<DateTime>,
    pub user_metadata: HashMap<String, String>,
}

impl ObjectHeaders {
    fn is_empty(&self) -> bool {
        self == &ObjectHeaders::default()
    }
}

/// mc `guessURLContentType`: MIME type by file extension (case-insensitive), or
/// `application/octet-stream`. mc sends it for uploads from local files and for `pipe`
/// (from the target key).
pub fn guess_content_type(path: impl AsRef<Path>) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

impl PutOptions {
    /// Uploading the local file `path`: sends its guessed Content-Type (mc takes it from the
    /// source file) unless [`PutOptions::content_type`] is set. `--attr Content-Type=...`
    /// entries in `metadata` still win.
    pub fn local_source(&mut self, path: &Path) {
        if self.content_type.is_none() {
            self.metadata
                .insert(0, ("Content-Type".to_string(), guess_content_type(path)));
        }
    }

    /// Splits `metadata` into standard headers and user metadata. `content_type` wins over a
    /// `Content-Type` attr.
    pub fn headers(&self) -> Result<ObjectHeaders> {
        let mut headers = ObjectHeaders::default();
        for (key, value) in &self.metadata {
            let lower = key.trim().to_ascii_lowercase();
            match lower.as_str() {
                "content-type" => headers.content_type = Some(value.clone()),
                "cache-control" => headers.cache_control = Some(value.clone()),
                "content-encoding" => headers.content_encoding = Some(value.clone()),
                "content-disposition" => headers.content_disposition = Some(value.clone()),
                "content-language" => headers.content_language = Some(value.clone()),
                "expires" => {
                    headers.expires = Some(
                        DateTime::from_str(value, DateTimeFormat::HttpDate)
                            .or_else(|_| {
                                DateTime::from_str(value, DateTimeFormat::DateTimeWithOffset)
                            })
                            .map_err(|_| anyhow!("invalid Expires value `{value}`"))?,
                    )
                }
                _ => {
                    let trimmed = key.trim();
                    let name = match trimmed.get(..11) {
                        Some(head) if head.eq_ignore_ascii_case("x-amz-meta-") => &trimmed[11..],
                        _ => trimmed,
                    };
                    if name.is_empty() {
                        bail!("metadata key cannot be empty");
                    }
                    headers
                        .user_metadata
                        .insert(name.to_string(), value.clone());
                }
            }
        }
        if let Some(content_type) = &self.content_type {
            headers.content_type = Some(content_type.clone());
        }
        Ok(headers)
    }
}

/// URL-encodes `k=v&k2=v2` for the `x-amz-tagging` header.
pub fn encode_tags(tags: &[(String, String)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in tags {
        serializer.append_pair(key, value);
    }
    serializer.finish().replace('+', "%20")
}

/// `(algorithm, base64 key, base64 md5(key))` for SSE-C headers.
pub fn sse_c_headers(key: &[u8; 32]) -> (&'static str, String, String) {
    let engine = base64::engine::general_purpose::STANDARD;
    (
        "AES256",
        engine.encode(key),
        engine.encode(Md5::digest(key)),
    )
}

/// Per-request config forcing SDK checksum calculation for an explicitly requested algorithm
/// (the client default is `WhenRequired`, which would otherwise skip it).
pub fn checksum_override() -> aws_sdk_s3::config::Builder {
    aws_sdk_s3::config::Builder::default()
        .request_checksum_calculation(aws_sdk_s3::config::RequestChecksumCalculation::WhenSupported)
}

pub(crate) fn add_zip_extract_header(request: &mut HttpRequest) {
    request.headers_mut().insert("x-minio-extract", "true");
}

/// Percent-encodes a key for `x-amz-copy-source` (keeps `/`).
pub fn encode_copy_source(bucket: &str, key: &str, version_id: Option<&str>) -> String {
    let mut out = format!("{bucket}/");
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    if let Some(version_id) = version_id {
        out.push_str("?versionId=");
        out.push_str(
            &url::form_urlencoded::byte_serialize(version_id.as_bytes()).collect::<String>(),
        );
    }
    out
}

/// Applies headers, metadata, tags, storage class, SSE and object lock settings to any of the
/// PutObject / CreateMultipartUpload / CopyObject fluent builders.
macro_rules! apply_put_options {
    ($request:expr, $options:expr, $headers:expr) => {{
        let options: &$crate::s3::objects::PutOptions = $options;
        let headers: &$crate::s3::objects::ObjectHeaders = $headers;
        let mut request = $request
            .set_content_type(headers.content_type.clone())
            .set_cache_control(headers.cache_control.clone())
            .set_content_encoding(headers.content_encoding.clone())
            .set_content_disposition(headers.content_disposition.clone())
            .set_content_language(headers.content_language.clone())
            .set_expires(headers.expires);
        if !headers.user_metadata.is_empty() {
            request = request.set_metadata(Some(headers.user_metadata.clone()));
        }
        if let Some(class) = &options.storage_class {
            request = request.storage_class(::aws_sdk_s3::types::StorageClass::from(
                class.to_ascii_uppercase().as_str(),
            ));
        }
        if !options.tags.is_empty() {
            request = request.tagging($crate::s3::objects::encode_tags(&options.tags));
        }
        match &options.sse {
            Some($crate::flags::Sse::C { key }) => {
                let (algorithm, encoded, md5) = $crate::s3::objects::sse_c_headers(key);
                request = request
                    .sse_customer_algorithm(algorithm)
                    .sse_customer_key(encoded)
                    .sse_customer_key_md5(md5);
            }
            Some($crate::flags::Sse::S3) => {
                request = request
                    .server_side_encryption(::aws_sdk_s3::types::ServerSideEncryption::Aes256);
            }
            Some($crate::flags::Sse::Kms { key_id }) => {
                request = request
                    .server_side_encryption(::aws_sdk_s3::types::ServerSideEncryption::AwsKms)
                    .ssekms_key_id(key_id.clone());
            }
            None => {}
        }
        if let Some(hold) = options.legal_hold {
            request = request.object_lock_legal_hold_status(if hold {
                ::aws_sdk_s3::types::ObjectLockLegalHoldStatus::On
            } else {
                ::aws_sdk_s3::types::ObjectLockLegalHoldStatus::Off
            });
        }
        if let Some((mode, until)) = &options.retention {
            request = request
                .object_lock_mode(::aws_sdk_s3::types::ObjectLockMode::from(
                    mode.to_ascii_uppercase().as_str(),
                ))
                .object_lock_retain_until_date(::aws_sdk_s3::primitives::DateTime::from(*until));
        }
        request
    }};
}
pub(crate) use apply_put_options;

/// Effective checksum: explicit, or CRC32 when object-lock headers need an integrity header
/// (S3/MinIO reject lock parameters without Content-MD5 or a checksum).
pub(crate) fn effective_checksum(options: &PutOptions) -> Option<ChecksumAlgo> {
    options.checksum.or_else(|| {
        (options.legal_hold.is_some() || options.retention.is_some()).then_some(ChecksumAlgo::Crc32)
    })
}

// ---------------------------------------------------------------------------
// put
// ---------------------------------------------------------------------------

/// Single PutObject with all [`PutOptions`] applied (ignores multipart settings).
pub async fn put_object_with(
    client: &Client,
    bucket: &str,
    key: &str,
    bytes: Vec<u8>,
    options: &PutOptions,
) -> Result<PutOutcome> {
    let size = bytes.len() as i64;
    let headers = options.headers()?;
    let request = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from(bytes));
    let mut request = apply_put_options!(request, options, &headers);
    let response = if let Some(algorithm) = effective_checksum(options) {
        request = request.checksum_algorithm(algorithm.to_sdk());
        request
            .customize()
            .config_override(checksum_override())
            .send()
            .await
            .s3_object(bucket, "")?
    } else {
        request.send().await.s3_object(bucket, "")?
    };
    Ok(PutOutcome {
        size: Some(size),
        etag: response.e_tag().map(str::to_string),
        version_id: response.version_id().map(str::to_string),
    })
}

pub async fn put_object_bytes(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    bytes: Vec<u8>,
    content_type: Option<&str>,
) -> Result<i64> {
    let client = build_client(alias).await?;
    let options = PutOptions {
        content_type: content_type.map(str::to_string),
        ..Default::default()
    };
    let outcome = put_object_with(&client, bucket, key, bytes, &options).await?;
    Ok(outcome.size.unwrap_or_default())
}

/// Uploads from a blocking reader (stdin, file). Small inputs use PutObject, larger ones a
/// multipart upload (see [`upload_stream`]).
pub async fn put_object_reader_with<R: Read + Unpin>(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    reader: R,
    size_hint: Option<u64>,
    options: &PutOptions,
) -> Result<PutOutcome> {
    let client = build_client(alias).await?;
    upload_stream(
        &client,
        bucket,
        key,
        super::multipart::BlockingReader::new(reader),
        size_hint,
        options,
    )
    .await
}

pub async fn put_object_reader<R: Read + Unpin>(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    reader: R,
) -> Result<i64> {
    let outcome =
        put_object_reader_with(alias, bucket, key, reader, None, &PutOptions::default()).await?;
    Ok(outcome.size.unwrap_or_default())
}

pub async fn put_local_file_with(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    path: &Path,
    options: &PutOptions,
) -> Result<PutOutcome> {
    let size = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("Unable to read local file `{}`.", path.display()))?
        .len();
    let client = build_client(alias).await?;
    super::upload_file(&client, bucket, key, path, size, options, None).await
}

pub async fn put_local_file(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    path: &Path,
) -> Result<i64> {
    let outcome = put_local_file_with(alias, bucket, key, path, &PutOptions::default()).await?;
    Ok(outcome.size.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------

/// GetObject with [`GetOptions`] applied. Stream the body with
/// `output.body.into_async_read()` or collect it.
pub async fn get_object(
    client: &Client,
    bucket: &str,
    key: &str,
    options: &GetOptions,
) -> Result<GetObjectOutput> {
    let mut request = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(options.version_id.clone())
        .set_part_number(options.part_number);
    if let Some((start, end)) = options.range {
        let range = match end {
            Some(end) => format!("bytes={start}-{end}"),
            None => format!("bytes={start}-"),
        };
        request = request.range(range);
    }
    if let Some(key) = &options.sse_c {
        let (algorithm, encoded, md5) = sse_c_headers(key);
        request = request
            .sse_customer_algorithm(algorithm)
            .sse_customer_key(encoded)
            .sse_customer_key_md5(md5);
    }
    if options.zip_extract {
        Ok(request
            .customize()
            .mutate_request(add_zip_extract_header)
            .send()
            .await
            .s3_object(bucket, key)?)
    } else {
        Ok(request.send().await.s3_object(bucket, key)?)
    }
}

pub async fn get_object_bytes_with(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    options: &GetOptions,
) -> Result<Vec<u8>> {
    let client = build_client(alias).await?;
    let response = get_object(&client, bucket, key, options).await?;
    Ok(response.body.collect().await?.into_bytes().to_vec())
}

pub async fn get_object_bytes(alias: &AliasConfig, bucket: &str, key: &str) -> Result<Vec<u8>> {
    get_object_bytes_with(alias, bucket, key, &GetOptions::default()).await
}

pub async fn download_object_to_path_with(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    path: &Path,
    options: &GetOptions,
) -> Result<i64> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let client = build_client(alias).await?;
    let response = get_object(&client, bucket, key, options).await?;
    let mut source = response.body.into_async_read();
    let mut destination = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("Unable to write local file `{}`.", path.display()))?;
    let bytes = crate::transfer::copy_to_file(&mut source, &mut destination).await?;
    Ok(bytes as i64)
}

pub async fn download_object_to_path(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    path: &Path,
) -> Result<i64> {
    download_object_to_path_with(alias, bucket, key, path, &GetOptions::default()).await
}

// ---------------------------------------------------------------------------
// copy
// ---------------------------------------------------------------------------

/// Copies one object. Same endpoint + credentials: server-side CopyObject (metadata/tagging
/// directive REPLACE when `put.metadata`/`put.content_type`/`put.tags` are set; source SSE-C
/// via copy-source headers; single CopyObject is limited to 5 GiB). Otherwise the object is
/// streamed through this process (GetObject -> multipart upload), keeping the source
/// content type and user metadata unless `put` overrides them.
pub async fn copy_object_with(
    source: ObjectRef<'_>,
    target: ObjectRef<'_>,
    source_options: &GetOptions,
    put: &PutOptions,
) -> Result<PutOutcome> {
    if same_endpoint_and_credentials(source.alias, target.alias) {
        let client = build_client(target.alias).await?;
        return server_side_copy(&client, source, target, source_options, put).await;
    }

    let source_client = build_client(source.alias).await?;
    let target_client = build_client(target.alias).await?;
    let response = get_object(&source_client, source.bucket, source.key, source_options).await?;
    let size_hint = response
        .content_length()
        .and_then(|v| u64::try_from(v).ok());
    let mut put = put.clone();
    if put.content_type.is_none()
        && put.metadata.is_empty()
        && let Some(content_type) = response.content_type()
    {
        put.content_type = Some(content_type.to_string());
    }
    if put.metadata.is_empty()
        && let Some(metadata) = response.metadata()
    {
        let mut pairs: Vec<_> = metadata
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        pairs.sort();
        put.metadata = pairs;
    }
    let reader = response.body.into_async_read();
    upload_stream(
        &target_client,
        target.bucket,
        target.key,
        reader,
        size_hint,
        &put,
    )
    .await
}

/// Server-side CopyObject on one client (source and target on the same endpoint).
pub async fn server_side_copy(
    client: &Client,
    source: ObjectRef<'_>,
    target: ObjectRef<'_>,
    source_options: &GetOptions,
    put: &PutOptions,
) -> Result<PutOutcome> {
    let headers = put.headers()?;
    let request = client
        .copy_object()
        .bucket(target.bucket)
        .key(target.key)
        .copy_source(encode_copy_source(
            source.bucket,
            source.key,
            source_options.version_id.as_deref(),
        ));
    let mut request = apply_put_options!(request, put, &headers);
    if !headers.is_empty() {
        request = request.metadata_directive(MetadataDirective::Replace);
    }
    if !put.tags.is_empty() {
        request = request.tagging_directive(TaggingDirective::Replace);
    }
    if let Some(key) = &source_options.sse_c {
        let (algorithm, encoded, md5) = sse_c_headers(key);
        request = request
            .copy_source_sse_customer_algorithm(algorithm)
            .copy_source_sse_customer_key(encoded)
            .copy_source_sse_customer_key_md5(md5);
    }
    if let Some(algorithm) = put.checksum {
        request = request.checksum_algorithm(algorithm.to_sdk());
    }
    let response = request.send().await.s3_object(target.bucket, "")?;
    Ok(PutOutcome {
        size: None,
        etag: response
            .copy_object_result()
            .and_then(|result| result.e_tag())
            .map(str::to_string),
        version_id: response.version_id().map(str::to_string),
    })
}

pub async fn copy_object(
    src_alias: &AliasConfig,
    src_bucket: &str,
    src_key: &str,
    dst_alias: &AliasConfig,
    dst_bucket: &str,
    dst_key: &str,
) -> Result<()> {
    copy_object_with(
        ObjectRef {
            alias: src_alias,
            bucket: src_bucket,
            key: src_key,
        },
        ObjectRef {
            alias: dst_alias,
            bucket: dst_bucket,
            key: dst_key,
        },
        &GetOptions::default(),
        &PutOptions::default(),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// copy helpers (cp/mv)
// ---------------------------------------------------------------------------

/// Minimum part size for server-side multipart copies (UploadPartCopy).
pub const COPY_PART_SIZE: u64 = 512 * 1024 * 1024;

/// Byte ranges `(start, end_inclusive)` for copying `size` bytes with UploadPartCopy:
/// parts of `max(512 MiB, ceil(size / 10000))` rounded up to 1 MiB.
pub fn plan_copy_parts(size: u64) -> Vec<(u64, u64)> {
    const MIB: u64 = 1024 * 1024;
    let needed = size.div_ceil(super::MAX_PARTS as u64).div_ceil(MIB) * MIB;
    let part = needed.max(COPY_PART_SIZE);
    (0..size.div_ceil(part))
        .map(|index| {
            let start = index * part;
            (start, (start + part).min(size) - 1)
        })
        .collect()
}

/// Server-side copy that switches to a multipart copy (UploadPartCopy) above 5 GiB.
pub async fn server_side_copy_sized(
    client: &Client,
    source: ObjectRef<'_>,
    target: ObjectRef<'_>,
    source_options: &GetOptions,
    put: &PutOptions,
    size: u64,
) -> Result<PutOutcome> {
    if size <= super::MAX_SINGLE_PUT_SIZE {
        return server_side_copy(client, source, target, source_options, put).await;
    }
    if put.disable_multipart {
        bail!("object exceeds the 5 GiB single copy limit; enable multipart");
    }
    multipart_copy(client, source, target, source_options, put, size).await
}

/// Copies `size` bytes server-side with CreateMultipartUpload + UploadPartCopy. Keeps the
/// source content headers, user metadata and tags unless `put` overrides them.
pub async fn multipart_copy(
    client: &Client,
    source: ObjectRef<'_>,
    target: ObjectRef<'_>,
    source_options: &GetOptions,
    put: &PutOptions,
    size: u64,
) -> Result<PutOutcome> {
    let mut put = put.clone();
    if put.metadata.is_empty() && put.content_type.is_none() {
        let head =
            super::head_object_with(client, source.bucket, source.key, source_options).await?;
        put.metadata = merge_metadata(
            [
                ("Content-Type", head.content_type()),
                ("Cache-Control", head.cache_control()),
                ("Content-Encoding", head.content_encoding()),
                ("Content-Disposition", head.content_disposition()),
                ("Content-Language", head.content_language()),
            ],
            head.metadata(),
            &[],
        );
    }
    if put.tags.is_empty() {
        let tagging = client
            .get_object_tagging()
            .bucket(source.bucket)
            .key(source.key)
            .set_version_id(source_options.version_id.clone())
            .send()
            .await
            .s3(source.bucket, "")?;
        put.tags = tagging
            .tag_set()
            .iter()
            .map(|tag| (tag.key().to_string(), tag.value().to_string()))
            .collect();
    }
    let headers = put.headers()?;
    let mut request = apply_put_options!(
        client
            .create_multipart_upload()
            .bucket(target.bucket)
            .key(target.key),
        &put,
        &headers
    );
    if let Some(algorithm) = effective_checksum(&put) {
        request = request.checksum_algorithm(algorithm.to_sdk());
    }
    let created = request.send().await.s3_object(target.bucket, "")?;
    let upload_id = created
        .upload_id()
        .context("S3 did not return a multipart upload ID")?
        .to_string();
    let copy_source = encode_copy_source(
        source.bucket,
        source.key,
        source_options.version_id.as_deref(),
    );
    let target_key = put.sse.as_ref().and_then(Sse::customer_key);

    let result = async {
        let mut tasks = tokio::task::JoinSet::new();
        let mut parts = Vec::new();
        for (index, (start, end)) in plan_copy_parts(size).into_iter().enumerate() {
            while tasks.len() >= 4 {
                if let Some(joined) = tasks.join_next().await {
                    parts.push(joined??);
                }
            }
            let part_number = index as i32 + 1;
            let mut request = client
                .upload_part_copy()
                .bucket(target.bucket)
                .key(target.key)
                .upload_id(&upload_id)
                .part_number(part_number)
                .copy_source(&copy_source)
                .copy_source_range(format!("bytes={start}-{end}"));
            if let Some(key) = &source_options.sse_c {
                let (algorithm, encoded, md5) = sse_c_headers(key);
                request = request
                    .copy_source_sse_customer_algorithm(algorithm)
                    .copy_source_sse_customer_key(encoded)
                    .copy_source_sse_customer_key_md5(md5);
            }
            if let Some(key) = &target_key {
                let (algorithm, encoded, md5) = sse_c_headers(key);
                request = request
                    .sse_customer_algorithm(algorithm)
                    .sse_customer_key(encoded)
                    .sse_customer_key_md5(md5);
            }
            let task_bucket = target.bucket.to_string();
            tasks.spawn(async move {
                let response = request.send().await.s3_object(&task_bucket, "")?;
                let result = response.copy_part_result();
                Ok::<_, anyhow::Error>(
                    CompletedPart::builder()
                        .part_number(part_number)
                        .set_e_tag(result.and_then(|r| r.e_tag()).map(str::to_string))
                        .set_checksum_crc32(
                            result.and_then(|r| r.checksum_crc32()).map(str::to_string),
                        )
                        .set_checksum_crc32_c(
                            result
                                .and_then(|r| r.checksum_crc32_c())
                                .map(str::to_string),
                        )
                        .set_checksum_sha1(
                            result.and_then(|r| r.checksum_sha1()).map(str::to_string),
                        )
                        .set_checksum_sha256(
                            result.and_then(|r| r.checksum_sha256()).map(str::to_string),
                        )
                        .build(),
                )
            });
        }
        while let Some(joined) = tasks.join_next().await {
            parts.push(joined??);
        }
        parts.sort_by_key(|part: &CompletedPart| part.part_number());
        let mut complete = client
            .complete_multipart_upload()
            .bucket(target.bucket)
            .key(target.key)
            .upload_id(&upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            );
        if let Some(key) = &target_key {
            let (algorithm, encoded, md5) = sse_c_headers(key);
            complete = complete
                .sse_customer_algorithm(algorithm)
                .sse_customer_key(encoded)
                .sse_customer_key_md5(md5);
        }
        let response = complete.send().await.s3_object(target.bucket, "")?;
        Ok::<PutOutcome, anyhow::Error>(PutOutcome {
            size: Some(size as i64),
            etag: response.e_tag().map(str::to_string),
            version_id: response.version_id().map(str::to_string),
        })
    }
    .await;

    if result.is_err() {
        let _ = client
            .abort_multipart_upload()
            .bucket(target.bucket)
            .key(target.key)
            .upload_id(&upload_id)
            .send()
            .await;
    }
    result
}

/// Source content headers + user metadata as `--attr`-style pairs, with `overrides` replacing
/// entries of the same name (case-insensitive, `X-Amz-Meta-` prefix optional).
pub fn merge_metadata(
    standard: [(&str, Option<&str>); 5],
    user: Option<&HashMap<String, String>>,
    overrides: &[(String, String)],
) -> Vec<(String, String)> {
    fn normalized(key: &str) -> String {
        let lower = key.trim().to_ascii_lowercase();
        lower
            .strip_prefix("x-amz-meta-")
            .map(str::to_string)
            .unwrap_or(lower)
    }
    let mut pairs: Vec<(String, String)> = standard
        .iter()
        .filter_map(|(name, value)| value.map(|v| (name.to_string(), v.to_string())))
        .collect();
    let mut user: Vec<_> = user
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    user.sort();
    pairs.extend(user);
    for (key, value) in overrides {
        let name = normalized(key);
        pairs.retain(|(existing, _)| normalized(existing) != name);
        pairs.push((key.clone(), value.clone()));
    }
    pairs
}

/// True when at least one object exists under `prefix`.
pub async fn prefix_exists(client: &Client, bucket: &str, prefix: &str) -> Result<bool> {
    let response = client
        .list_objects_v2()
        .bucket(bucket)
        .prefix(prefix)
        .max_keys(1)
        .send()
        .await
        .s3(bucket, "")?;
    Ok(!response.contents().is_empty() || !response.common_prefixes().is_empty())
}

/// The version of exactly `key` that was current at `at` (None if it did not exist or was
/// deleted then).
pub async fn object_version_at(
    client: &Client,
    bucket: &str,
    key: &str,
    at: SystemTime,
) -> Result<Option<super::ObjectInfo>> {
    let mut items = Vec::new();
    let mut key_marker: Option<String> = None;
    let mut version_marker: Option<String> = None;
    loop {
        let response = client
            .list_object_versions()
            .bucket(bucket)
            .prefix(key)
            .set_key_marker(key_marker.take())
            .set_version_id_marker(version_marker.take())
            .send()
            .await
            .s3(bucket, "")?;
        for version in response.versions() {
            if version.key() == Some(key) {
                items.push(super::ObjectInfo {
                    key: key.to_string(),
                    size: version.size().unwrap_or(0),
                    last_modified: version.last_modified().and_then(super::to_system_time),
                    version_id: version.version_id().map(str::to_string),
                    is_latest: version.is_latest().unwrap_or(false),
                    ..Default::default()
                });
            }
        }
        for marker in response.delete_markers() {
            if marker.key() == Some(key) {
                items.push(super::ObjectInfo {
                    key: key.to_string(),
                    last_modified: marker.last_modified().and_then(super::to_system_time),
                    version_id: marker.version_id().map(str::to_string),
                    is_latest: marker.is_latest().unwrap_or(false),
                    is_delete_marker: true,
                    ..Default::default()
                });
            }
        }
        if response.is_truncated() != Some(true) {
            break;
        }
        key_marker = response.next_key_marker().map(str::to_string);
        version_marker = response.next_version_id_marker().map(str::to_string);
        if key_marker.is_none() && version_marker.is_none() {
            break;
        }
    }
    Ok(super::resolve_rewind(items, at).into_iter().next())
}

/// Recursively lists the files inside a zip object on MinIO (`x-minio-extract: true`).
/// `prefix` is `path/to/archive.zip/` (optionally followed by a folder). Keys are absolute.
pub async fn list_zip_objects(
    client: &Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<super::ObjectInfo>> {
    let mut items = Vec::new();
    let mut continuation = None;
    loop {
        let response = client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_continuation_token(continuation)
            .customize()
            .mutate_request(add_zip_extract_header)
            .send()
            .await
            .s3(bucket, "")?;
        for object in response.contents() {
            let Some(key) = object.key() else { continue };
            items.push(super::ObjectInfo {
                key: key.to_string(),
                size: object.size().unwrap_or(0),
                last_modified: object.last_modified().and_then(super::to_system_time),
                is_latest: true,
                ..Default::default()
            });
        }
        if response.is_truncated() != Some(true) {
            break;
        }
        continuation = response.next_continuation_token().map(str::to_string);
        if continuation.is_none() {
            break;
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_content_type_by_extension() {
        assert_eq!(guess_content_type("a/b.TXT"), "text/plain");
        assert_eq!(guess_content_type("x.json"), "application/json");
        assert_eq!(guess_content_type("x.gz"), "application/gzip");
        assert_eq!(guess_content_type("noext"), "application/octet-stream");
        let mut options = PutOptions {
            metadata: vec![("Content-Type".into(), "text/x-custom".into())],
            ..Default::default()
        };
        options.local_source(Path::new("page.html"));
        let headers = options.headers().unwrap();
        assert_eq!(headers.content_type.as_deref(), Some("text/x-custom"));
        let mut options = PutOptions::default();
        options.local_source(Path::new("page.html"));
        assert_eq!(
            options.headers().unwrap().content_type.as_deref(),
            Some("text/html")
        );
    }

    #[test]
    fn splits_standard_headers_from_user_metadata() {
        let options = PutOptions {
            metadata: vec![
                ("Cache-Control".into(), "max-age=60".into()),
                ("content-type".into(), "text/plain".into()),
                ("X-Amz-Meta-Owner".into(), "alice".into()),
                ("project".into(), "mx".into()),
                ("Expires".into(), "Wed, 21 Oct 2015 07:28:00 GMT".into()),
            ],
            ..Default::default()
        };
        let headers = options.headers().unwrap();
        assert_eq!(headers.cache_control.as_deref(), Some("max-age=60"));
        assert_eq!(headers.content_type.as_deref(), Some("text/plain"));
        assert_eq!(headers.expires.unwrap().secs(), 1_445_412_480);
        assert_eq!(
            headers.user_metadata.get("Owner").map(String::as_str),
            Some("alice")
        );
        assert_eq!(
            headers.user_metadata.get("project").map(String::as_str),
            Some("mx")
        );

        let overridden = PutOptions {
            content_type: Some("application/json".into()),
            ..options
        };
        assert_eq!(
            overridden.headers().unwrap().content_type.as_deref(),
            Some("application/json")
        );
        assert!(PutOptions::default().headers().unwrap().is_empty());
    }

    #[test]
    fn encodes_tags_and_copy_sources() {
        assert_eq!(
            encode_tags(&[("a b".into(), "x&y".into()), ("k".into(), "v".into())]),
            "a%20b=x%26y&k=v"
        );
        assert_eq!(
            encode_copy_source("bkt", "dir/a b+c.txt", None),
            "bkt/dir/a%20b%2Bc.txt"
        );
        assert_eq!(
            encode_copy_source("bkt", "k", Some("v1")),
            "bkt/k?versionId=v1"
        );
    }

    #[test]
    fn plans_multipart_copy_parts() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let parts = plan_copy_parts(6 * GIB);
        assert_eq!(parts.len(), 12);
        assert_eq!(parts[0], (0, COPY_PART_SIZE - 1));
        assert_eq!(parts[11].1, 6 * GIB - 1);
        let parts = plan_copy_parts(5 * GIB + 1);
        assert_eq!(parts.len(), 11);
        assert_eq!(parts[10], (5 * GIB, 5 * GIB));
        for window in parts.windows(2) {
            assert_eq!(window[0].1 + 1, window[1].0);
        }
        let huge = 5 * 1024 * GIB;
        let parts = plan_copy_parts(huge);
        assert!(parts.len() <= 10_000);
        assert_eq!(parts.last().unwrap().1, huge - 1);
        assert!(plan_copy_parts(0).is_empty());
    }

    #[test]
    fn merges_source_metadata_with_overrides() {
        let user: HashMap<String, String> = [
            ("owner".to_string(), "alice".to_string()),
            ("a".into(), "1".into()),
        ]
        .into();
        let merged = merge_metadata(
            [
                ("Content-Type", Some("text/plain")),
                ("Cache-Control", None),
                ("Content-Encoding", None),
                ("Content-Disposition", None),
                ("Content-Language", None),
            ],
            Some(&user),
            &[
                ("X-Amz-Meta-Owner".into(), "bob".into()),
                ("content-type".into(), "application/json".into()),
            ],
        );
        assert_eq!(
            merged,
            vec![
                ("a".to_string(), "1".to_string()),
                ("X-Amz-Meta-Owner".to_string(), "bob".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
            ]
        );
    }

    #[test]
    fn computes_sse_c_headers() {
        let (algorithm, key, md5) = sse_c_headers(&[0u8; 32]);
        assert_eq!(algorithm, "AES256");
        assert_eq!(key, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        assert_eq!(md5, "cLyPS3KoaSFGi/joRB3OUQ==");
    }

    #[test]
    fn lock_options_force_a_checksum() {
        assert_eq!(effective_checksum(&PutOptions::default()), None);
        assert_eq!(
            effective_checksum(&PutOptions {
                legal_hold: Some(true),
                ..Default::default()
            }),
            Some(ChecksumAlgo::Crc32)
        );
        assert_eq!(
            effective_checksum(&PutOptions {
                checksum: Some(ChecksumAlgo::Sha256),
                retention: Some(("GOVERNANCE".into(), SystemTime::now())),
                ..Default::default()
            }),
            Some(ChecksumAlgo::Sha256)
        );
    }
}
