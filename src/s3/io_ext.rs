//! Extra S3 helpers for cat/head/get/put/pipe/find (area E).

use super::to_system_time;
use anyhow::{Result, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{ConfigBag, Intercept, RuntimeComponents};
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::context::BeforeTransmitInterceptorContextMut;
use std::collections::HashMap;
use std::time::SystemTime;

/// Returns a copy of `client` that sends `If-None-Match: *` on PutObject and
/// CompleteMultipartUpload, so an upload fails when the object already exists.
pub fn with_if_none_match(client: &Client) -> Client {
    let config = client
        .config()
        .to_builder()
        .interceptor(IfNoneMatchInterceptor)
        .build();
    Client::from_conf(config)
}

#[derive(Debug)]
struct IfNoneMatchInterceptor;

impl Intercept for IfNoneMatchInterceptor {
    fn name(&self) -> &'static str {
        "if-none-match"
    }

    fn modify_before_signing(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let request = context.request_mut();
        if is_object_commit(request.method(), request.uri()) {
            request.headers_mut().insert("If-None-Match", "*");
        }
        Ok(())
    }
}

/// PutObject (PUT without `uploadId`) or CompleteMultipartUpload (POST with `uploadId`).
fn is_object_commit(method: &str, uri: &str) -> bool {
    let has_upload_id = uri.contains("uploadId=");
    (method.eq_ignore_ascii_case("PUT") && !has_upload_id && !uri.contains("x-id=UploadPart"))
        || (method.eq_ignore_ascii_case("POST") && has_upload_id)
}

/// Version ID of `key` as it was at `at` (newest version with `LastModified <= at`).
/// Fails when the object did not exist or was deleted at that time.
pub async fn version_at(
    client: &Client,
    bucket: &str,
    key: &str,
    at: SystemTime,
) -> Result<String> {
    let mut key_marker: Option<String> = None;
    let mut version_marker: Option<String> = None;
    // (last_modified, version_id, is_delete_marker)
    let mut best: Option<(SystemTime, String, bool)> = None;
    let mut consider = |modified: Option<SystemTime>, version: Option<&str>, marker: bool| {
        let (Some(modified), Some(version)) = (modified, version) else {
            return;
        };
        if modified <= at && best.as_ref().is_none_or(|(time, _, _)| modified > *time) {
            best = Some((modified, version.to_string(), marker));
        }
    };
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
                consider(
                    version.last_modified().and_then(to_system_time),
                    version.version_id(),
                    false,
                );
            }
        }
        for marker in response.delete_markers() {
            if marker.key() == Some(key) {
                consider(
                    marker.last_modified().and_then(to_system_time),
                    marker.version_id(),
                    true,
                );
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
    match best {
        Some((_, version, false)) => Ok(version),
        _ => bail!("Object `{key}` does not exist at the requested rewind time."),
    }
}

/// Standard headers (`Content-Type`, ...) and user metadata of an object, keyed by lowercase
/// name (user metadata without the `x-amz-meta-` prefix).
pub async fn object_metadata(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
) -> Result<HashMap<String, String>> {
    let head = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .send()
        .await?;
    let mut map = HashMap::new();
    for (name, value) in [
        ("content-type", head.content_type()),
        ("cache-control", head.cache_control()),
        ("content-encoding", head.content_encoding()),
        ("content-disposition", head.content_disposition()),
        ("content-language", head.content_language()),
    ] {
        if let Some(value) = value {
            map.insert(name.to_string(), value.to_string());
        }
    }
    for (name, value) in head.metadata().into_iter().flatten() {
        map.insert(name.to_ascii_lowercase(), value.clone());
    }
    Ok(map)
}

/// Object tags as a map.
pub async fn object_tags(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
) -> Result<HashMap<String, String>> {
    let response = client
        .get_object_tagging()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .send()
        .await?;
    Ok(response
        .tag_set
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::is_object_commit;

    #[test]
    fn detects_object_commit_requests() {
        assert!(is_object_commit("PUT", "http://h/b/k"));
        assert!(is_object_commit("PUT", "http://h/b/k?x-id=PutObject"));
        assert!(is_object_commit(
            "POST",
            "http://h/b/k?uploadId=abc&x-id=CompleteMultipartUpload"
        ));
        assert!(!is_object_commit(
            "PUT",
            "http://h/b/k?partNumber=1&uploadId=abc&x-id=UploadPart"
        ));
        assert!(!is_object_commit(
            "POST",
            "http://h/b/k?uploads&x-id=CreateMultipartUpload"
        ));
        assert!(!is_object_commit("GET", "http://h/b/k"));
    }
}
