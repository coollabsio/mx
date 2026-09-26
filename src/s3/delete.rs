//! Object deletion: single, prefix, bulk, all versions (area C).

use super::{S3ListItem, build_client, full_key, list_objects};
use crate::config::model::AliasConfig;
use anyhow::{Result, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::types::{Delete, ObjectIdentifier};

pub async fn delete_object(alias: &AliasConfig, bucket: &str, key: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client
        .delete_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await?;
    Ok(())
}

pub async fn delete_prefix(alias: &AliasConfig, bucket: &str, prefix: &str) -> Result<Vec<String>> {
    let client = build_client(alias).await?;
    let keys = list_objects(&client, bucket, Some(prefix), true)
        .await?
        .into_iter()
        .filter_map(|item| match item {
            S3ListItem::Object { name, .. } => Some(full_key(prefix, &name)),
            _ => None,
        })
        .collect::<Vec<_>>();
    delete_keys(&client, bucket, &keys).await?;
    Ok(keys)
}

/// Permanently deletes every version and delete marker in the bucket.
pub async fn delete_all_versions(client: &Client, bucket: &str) -> Result<()> {
    let mut key_marker = None;
    let mut version_marker = None;
    loop {
        let mut request = client.list_object_versions().bucket(bucket);
        if let Some(marker) = key_marker {
            request = request.key_marker(marker);
        }
        if let Some(marker) = version_marker {
            request = request.version_id_marker(marker);
        }
        let response = request.send().await?;
        let mut objects = Vec::new();
        for version in response.versions() {
            if let (Some(key), Some(version_id)) = (version.key(), version.version_id()) {
                objects.push(
                    ObjectIdentifier::builder()
                        .key(key)
                        .version_id(version_id)
                        .build()?,
                );
            }
        }
        for marker in response.delete_markers() {
            if let (Some(key), Some(version_id)) = (marker.key(), marker.version_id()) {
                objects.push(
                    ObjectIdentifier::builder()
                        .key(key)
                        .version_id(version_id)
                        .build()?,
                );
            }
        }
        if !objects.is_empty() {
            let response = client
                .delete_objects()
                .bucket(bucket)
                .delete(
                    Delete::builder()
                        .set_objects(Some(objects))
                        .quiet(true)
                        .build()?,
                )
                .send()
                .await?;
            check_delete_errors(response.errors())?;
        }
        if response.is_truncated() == Some(true) {
            key_marker = response.next_key_marker().map(str::to_string);
            version_marker = response.next_version_id_marker().map(str::to_string);
        } else {
            break;
        }
    }
    Ok(())
}

/// Bulk-deletes absolute keys (current versions) in chunks of 1000.
pub async fn delete_keys(client: &Client, bucket: &str, keys: &[String]) -> Result<()> {
    for chunk in keys.chunks(1_000) {
        if chunk.is_empty() {
            continue;
        }
        let objects = chunk
            .iter()
            .map(|key| ObjectIdentifier::builder().key(key).build())
            .collect::<Result<Vec<_>, _>>()?;
        let response = client
            .delete_objects()
            .bucket(bucket)
            .delete(
                Delete::builder()
                    .set_objects(Some(objects))
                    .quiet(true)
                    .build()?,
            )
            .send()
            .await?;
        check_delete_errors(response.errors())?;
    }
    Ok(())
}

/// Turns per-key DeleteObjects failures (quiet mode only reports those) into an error.
fn check_delete_errors(errors: &[aws_sdk_s3::types::Error]) -> Result<()> {
    if errors.is_empty() {
        return Ok(());
    }
    let details = errors
        .iter()
        .map(|error| {
            let key = error.key().unwrap_or_default();
            let reason = error.message().or(error.code()).unwrap_or("unknown error");
            match error.version_id() {
                Some(version) => format!("`{key}` (versionId={version}): {reason}"),
                None => format!("`{key}`: {reason}"),
            }
        })
        .collect::<Vec<_>>()
        .join("; ");
    bail!("Failed to remove {} object(s): {details}", errors.len())
}

/// One object (version) to delete with [`delete_versions`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeleteTarget {
    pub key: String,
    pub version_id: Option<String>,
}

/// Per-object outcome of a delete request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeleteOutcome {
    pub key: String,
    /// The version that was requested for deletion.
    pub version_id: Option<String>,
    /// A delete marker was created (or the removed version was a delete marker).
    pub delete_marker: bool,
    pub delete_marker_version_id: Option<String>,
    /// Error message when the server refused to delete this object.
    pub error: Option<String>,
}

/// Bulk-deletes objects/versions with DeleteObjects in batches of 1000 and reports every
/// result. `bypass` sets `x-amz-bypass-governance-retention`.
pub async fn delete_versions(
    client: &Client,
    bucket: &str,
    targets: &[DeleteTarget],
    bypass: bool,
) -> Result<Vec<DeleteOutcome>> {
    let mut outcomes = Vec::new();
    for chunk in targets.chunks(1_000) {
        let objects = chunk
            .iter()
            .map(|target| {
                ObjectIdentifier::builder()
                    .key(&target.key)
                    .set_version_id(target.version_id.clone())
                    .build()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut request = client.delete_objects().bucket(bucket).delete(
            Delete::builder()
                .set_objects(Some(objects))
                .quiet(false)
                .build()?,
        );
        if bypass {
            request = request.bypass_governance_retention(true);
        }
        let response = request.send().await?;
        for deleted in response.deleted() {
            outcomes.push(DeleteOutcome {
                key: deleted.key().unwrap_or_default().to_string(),
                version_id: deleted.version_id().map(str::to_string),
                delete_marker: deleted.delete_marker().unwrap_or(false),
                delete_marker_version_id: deleted.delete_marker_version_id().map(str::to_string),
                error: None,
            });
        }
        for error in response.errors() {
            outcomes.push(DeleteOutcome {
                key: error.key().unwrap_or_default().to_string(),
                version_id: error.version_id().map(str::to_string),
                error: Some(
                    error
                        .message()
                        .or(error.code())
                        .unwrap_or("unknown error")
                        .to_string(),
                ),
                ..Default::default()
            });
        }
    }
    Ok(outcomes)
}

/// DeleteObject for one key/version. `bypass` sets `x-amz-bypass-governance-retention`;
/// `purge` sets MinIO's `x-minio-force-delete: true` (removes the whole prefix/object tree).
pub async fn delete_object_with(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
    bypass: bool,
    purge: bool,
) -> Result<DeleteOutcome> {
    let mut request = client
        .delete_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string));
    if bypass {
        request = request.bypass_governance_retention(true);
    }
    let response = if purge {
        request
            .customize()
            .mutate_request(add_force_delete_header)
            .send()
            .await?
    } else {
        request.send().await?
    };
    let delete_marker = response.delete_marker().unwrap_or(false);
    Ok(DeleteOutcome {
        key: key.to_string(),
        version_id: version_id.map(str::to_string),
        delete_marker,
        delete_marker_version_id: if delete_marker {
            response.version_id().map(str::to_string)
        } else {
            None
        },
        error: None,
    })
}

/// Aborts one incomplete multipart upload.
pub async fn abort_upload(client: &Client, bucket: &str, key: &str, upload_id: &str) -> Result<()> {
    client
        .abort_multipart_upload()
        .bucket(bucket)
        .key(key)
        .upload_id(upload_id)
        .send()
        .await?;
    Ok(())
}

/// MinIO force bucket removal (`x-minio-force-delete: true`): deletes the bucket with all its
/// contents, including object-locked data.
pub async fn delete_bucket_force(client: &Client, bucket: &str) -> Result<()> {
    client
        .delete_bucket()
        .bucket(bucket)
        .customize()
        .mutate_request(add_force_delete_header)
        .send()
        .await?;
    Ok(())
}

fn add_force_delete_header(
    request: &mut aws_smithy_runtime_api::client::orchestrator::HttpRequest,
) {
    request.headers_mut().insert("x-minio-force-delete", "true");
}

#[cfg(test)]
mod tests {
    use super::check_delete_errors;
    use aws_sdk_s3::types::Error;

    #[test]
    fn quiet_delete_errors_are_reported() {
        assert!(check_delete_errors(&[]).is_ok());
        let errors = [
            Error::builder()
                .key("locked.txt")
                .version_id("v1")
                .code("AccessDenied")
                .message("Object is WORM protected")
                .build(),
            Error::builder().key("b").code("InternalError").build(),
        ];
        let message = check_delete_errors(&errors).unwrap_err().to_string();
        assert_eq!(
            message,
            "Failed to remove 2 object(s): `locked.txt` (versionId=v1): Object is WORM protected; `b`: InternalError"
        );
    }
}
