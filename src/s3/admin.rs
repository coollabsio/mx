//! MinIO admin API client (area H: quota, ilm tier, replication targets).
//!
//! Requests are SigV4-signed (service `s3`, region `us-east-1`, `x-amz-content-sha256`) and sent
//! with the smithy HTTP connector, so `--resolve` pins apply. The same client is used for the
//! MinIO-specific bucket sub-resources (`?replication`, `?replication-metrics`, ...) whose XML/JSON
//! bodies the AWS SDK cannot model.
//!
//! Request bodies that carry credentials are encrypted like `madmin.EncryptData`:
//! `salt(32) | id(1) | nonce(8) | sio-DARE stream`. We use id `0x02` (PBKDF2-SHA256 +
//! AES-256-GCM), which every MinIO server decrypts (it is madmin's FIPS mode).

use crate::config::model::AliasConfig;
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
use aws_smithy_runtime_api::client::http::HttpConnector;
use aws_smithy_runtime_api::client::identity::Identity;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

pub const ADMIN_PREFIX: &str = "/minio/admin/v3";

/// Raw HTTP response.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Signed-request client for one alias.
pub struct AdminClient {
    scheme: String,
    authority: String,
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
    connector: Connector,
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
        let builder = Connector::builder().tls_provider(tls::Provider::Rustls(CryptoMode::AwsLc));
        let connector = if mappings.is_empty() {
            builder.build()
        } else {
            builder.build_with_resolver(crate::resolve::PinnedDnsResolver::new(&mappings)?)
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
            .map_err(|err| anyhow!("request to {} failed: {err}", self.authority))?;
        let status = response.status().as_u16();
        let body = ByteStream::new(response.into_body())
            .collect()
            .await?
            .into_bytes()
            .to_vec();
        Ok(Response { status, body })
    }

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
        let path = format!("/{}", encode_path(bucket));
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

    /// Like [`Self::bucket_raw`], but non-2xx statuses become errors.
    pub async fn bucket(
        &self,
        method: &str,
        bucket: &str,
        query: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<Response> {
        check_status(self.bucket_raw(method, bucket, query, body).await?)
    }
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

pub fn check_status(response: Response) -> Result<Response> {
    if (200..300).contains(&response.status) {
        return Ok(response);
    }
    Err(anyhow!(error_message(&response)))
}

/// Extracts a readable message from a MinIO admin (JSON) or S3 (XML) error body.
pub fn error_message(response: &Response) -> String {
    #[derive(Deserialize)]
    struct AdminError {
        #[serde(rename = "Code", default)]
        code: String,
        #[serde(rename = "Message", default)]
        message: String,
    }
    let text = response.text();
    if let Ok(err) = serde_json::from_str::<AdminError>(&text) {
        if !err.message.is_empty() {
            return err.message;
        }
        if !err.code.is_empty() {
            return err.code;
        }
    }
    if let Some(message) = xml_text(&text, "Message").filter(|m| !m.is_empty()) {
        return message;
    }
    if let Some(code) = xml_text(&text, "Code") {
        return code;
    }
    match response.status {
        403 => "Access denied (admin privileges required).".to_string(),
        404 => {
            "The requested resource does not exist on the server (not a MinIO server?).".to_string()
        }
        status => format!("server returned HTTP {status}"),
    }
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
    out.extend(sio_seal(&key, nonce, data)?);
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

fn aes_key(key: &[u8]) -> Result<aws_lc_rs::aead::LessSafeKey> {
    let unbound = aws_lc_rs::aead::UnboundKey::new(&aws_lc_rs::aead::AES_256_GCM, key)
        .map_err(|_| anyhow!("invalid AES key"))?;
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
fn sio_seal(key_bytes: &[u8], nonce: &[u8; 8], data: &[u8]) -> Result<Vec<u8>> {
    let key = aes_key(key_bytes)?;
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

/// Inverse of [`sio_seal`].
fn sio_open(key_bytes: &[u8], nonce: &[u8; 8], data: &[u8]) -> Result<Vec<u8>> {
    let key = aes_key(key_bytes)?;
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

/// Key derivation for argon2id-encrypted data (ids `0x00`/`0x01`); not linked into the binary.
pub type Argon2idFn<'a> = &'a dyn Fn(&str, &[u8]) -> [u8; 32];

/// Decrypts `madmin.EncryptData` output (PBKDF2 always; argon2id/AES-GCM when `argon2id`
/// is given).
pub fn decrypt_data(password: &str, data: &[u8], argon2id: Option<Argon2idFn>) -> Result<Vec<u8>> {
    if data.len() < 41 {
        bail!("unexpected header");
    }
    let (salt, rest) = data.split_at(32);
    let id = rest[0];
    let nonce: [u8; 8] = rest[1..9].try_into().expect("8 bytes");
    let key = match (id, argon2id) {
        (PBKDF2_AES_GCM, _) => pbkdf2_key(password, salt),
        (0x00, Some(derive)) => derive(password, salt),
        _ => bail!("unsupported encryption algorithm ID {id:#04x}"),
    };
    sio_open(&key, &nonce, &rest[9..])
}

// ---------------------------------------------------------------------------
// Human-readable sizes (go-humanize compatible)
// ---------------------------------------------------------------------------

/// `humanize.ParseBytes`: "1GiB", "1 gb", "10k", "1.5TB", "123".
pub fn parse_bytes(input: &str) -> Result<u64> {
    let text = input.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(text.len());
    let number: f64 = text[..split]
        .replace(',', "")
        .parse()
        .map_err(|_| anyhow!("invalid size `{input}`"))?;
    let unit = text[split..].trim().to_ascii_lowercase();
    let multiplier: f64 = match unit.as_str() {
        "" | "b" | "byte" | "bytes" => 1.0,
        "k" | "kb" => 1e3,
        "ki" | "kib" => 1024.0,
        "m" | "mb" => 1e6,
        "mi" | "mib" => 1024f64.powi(2),
        "g" | "gb" => 1e9,
        "gi" | "gib" => 1024f64.powi(3),
        "t" | "tb" => 1e12,
        "ti" | "tib" => 1024f64.powi(4),
        "p" | "pb" => 1e15,
        "pi" | "pib" => 1024f64.powi(5),
        "e" | "eb" => 1e18,
        "ei" | "eib" => 1024f64.powi(6),
        _ => bail!("unhandled size name: {unit}"),
    };
    let value = number * multiplier;
    if value >= u64::MAX as f64 {
        bail!("too large: {input}");
    }
    Ok(value as u64)
}

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

type JsonObject = serde_json::Map<String, serde_json::Value>;

/// `madmin.TierConfig`. Tier-type specific sections are kept as raw JSON objects so that
/// unknown fields (and azure/gcs tiers) round-trip untouched.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TierConfig {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Type")]
    pub tier_type: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "S3", default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<JsonObject>,
    #[serde(rename = "Azure", default, skip_serializing_if = "Option::is_none")]
    pub azure: Option<JsonObject>,
    #[serde(rename = "GCS", default, skip_serializing_if = "Option::is_none")]
    pub gcs: Option<JsonObject>,
    #[serde(rename = "MinIO", default, skip_serializing_if = "Option::is_none")]
    pub minio: Option<JsonObject>,
}

impl TierConfig {
    fn section(&self) -> Option<&JsonObject> {
        match self.tier_type.as_str() {
            "s3" => self.s3.as_ref(),
            "azure" => self.azure.as_ref(),
            "gcs" => self.gcs.as_ref(),
            "minio" => self.minio.as_ref(),
            _ => None,
        }
    }

    /// String field of the type-specific section (`Endpoint`, `Bucket`, ...), or "".
    pub fn field(&self, name: &str) -> String {
        self.section()
            .and_then(|s| s.get(name))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
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

/// `madmin.TierInfo` list from `tier-stats` (kept as JSON; see `ilm tier info`).
pub async fn tier_stats(client: &AdminClient) -> Result<Vec<serde_json::Value>> {
    let response = client.admin("GET", "tier-stats", &[], Vec::new()).await?;
    let infos: Option<Vec<serde_json::Value>> = serde_json::from_slice(&response.body)?;
    Ok(infos.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argon2id(password: &str, salt: &[u8]) -> [u8; 32] {
        use argon2::{Algorithm, Argon2, Params, Version};
        let params = Params::new(64 * 1024, 1, 4, Some(32)).unwrap();
        let mut key = [0u8; 32];
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(password.as_bytes(), salt, &mut key)
            .unwrap();
        key
    }

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
            let plain = decrypt_data(password, &hex(data), Some(&argon2id)).unwrap();
            assert_eq!(plain, vec![0u8; *len]);
        }
        assert!(decrypt_data("nope", &hex(vectors[1].1), Some(&argon2id)).is_err());
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
        let aead = aes_key(&key).unwrap();
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
    fn parses_and_formats_sizes() {
        assert_eq!(parse_bytes("1GiB").unwrap(), 1 << 30);
        assert_eq!(parse_bytes("1gi").unwrap(), 1 << 30);
        assert_eq!(parse_bytes("1GB").unwrap(), 1_000_000_000);
        assert_eq!(parse_bytes("1.5 KiB").unwrap(), 1536);
        assert_eq!(parse_bytes("42").unwrap(), 42);
        assert_eq!(parse_bytes("2G").unwrap(), 2_000_000_000);
        assert!(parse_bytes("1XB").is_err());
        assert!(parse_bytes("abc").is_err());
        assert_eq!(ibytes(1 << 30), "1.0 GiB");
        assert_eq!(ibytes(5), "5 B");
        assert_eq!(ibytes(1_000_000_000), "954 MiB");
        assert_eq!(si_bytes(2_000_000), "2.0 MB");
        assert_eq!(comma(1_234_567), "1,234,567");
        assert_eq!(comma(-1000), "-1,000");
        assert_eq!(comma(12), "12");
    }

    #[test]
    fn extracts_error_messages() {
        let json = Response {
            status: 404,
            body: br#"{"Code":"XMinioAdminNoSuchQuotaConfiguration","Message":"The quota configuration does not exist","Resource":"/x"}"#.to_vec(),
        };
        assert_eq!(
            error_message(&json),
            "The quota configuration does not exist"
        );
        let xml = Response {
            status: 404,
            body: b"<?xml version=\"1.0\"?><Error><Code>NoSuchBucket</Code><Message>The specified bucket does not exist</Message></Error>".to_vec(),
        };
        assert_eq!(error_message(&xml), "The specified bucket does not exist");
        assert_eq!(error_code(&xml).as_deref(), Some("NoSuchBucket"));
        let empty = Response {
            status: 403,
            body: Vec::new(),
        };
        assert!(error_message(&empty).contains("Access denied"));
    }

    #[test]
    fn tier_config_json_matches_madmin() {
        let json = r#"{"Version":"v1","Type":"minio","Name":"WARM","MinIO":{"Endpoint":"http://h:9000","AccessKey":"a","SecretKey":"REDACTED","Bucket":"b","Prefix":"p/"}}"#;
        let tier: TierConfig = serde_json::from_str(json).unwrap();
        assert_eq!(tier.field("Endpoint"), "http://h:9000");
        assert_eq!(tier.field("Region"), "");
        assert_eq!(
            serde_json::to_value(&tier).unwrap(),
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
        let creds = TierCreds {
            access_key: "a".into(),
            secret_key: "s".into(),
            aws_role: false,
        };
        assert_eq!(
            serde_json::to_string(&creds).unwrap(),
            r#"{"access":"a","secret":"s","awsrole":false}"#
        );
    }
}
