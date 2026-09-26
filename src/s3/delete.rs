//! Object deletion: single, prefix, bulk, all versions (area C).

use super::{S3ListItem, build_client, full_key, list_objects};
use crate::config::model::AliasConfig;
use anyhow::Result;
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
            client
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
        client
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
    }
    Ok(())
}
