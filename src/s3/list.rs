//! Listing: buckets, objects (V2), object versions, rewind, incomplete uploads (area C).

use super::{build_client, debug_timestamp, to_system_time};
use crate::config::model::AliasConfig;
use crate::s3::S3ResultExt;
use crate::target::TargetRef;
use anyhow::Result;
use aws_sdk_s3::Client;
use std::collections::BTreeMap;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub enum S3ListItem {
    Bucket {
        name: String,
        last_modified: Option<String>,
    },
    Prefix {
        name: String,
    },
    Object {
        name: String,
        size: Option<i64>,
        last_modified: Option<String>,
        etag: Option<String>,
        storage_class: Option<String>,
        /// Set only for version listings.
        version_id: Option<String>,
        /// `true` for plain listings; from the server for version listings.
        is_latest: bool,
        is_delete_marker: bool,
    },
}

/// One listed object, object version, delete marker, prefix, or incomplete upload.
///
/// `key` is relative to the listing prefix (same convention as `S3ListItem` names); use
/// [`full_key`] with the prefix to get the absolute key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectInfo {
    pub key: String,
    pub size: i64,
    pub last_modified: Option<SystemTime>,
    pub etag: Option<String>,
    pub storage_class: Option<String>,
    pub version_id: Option<String>,
    pub is_latest: bool,
    pub is_delete_marker: bool,
    /// Common prefix entry from a non-recursive listing (`key` ends with `/`).
    pub is_prefix: bool,
    /// Set for incomplete multipart uploads (`ListOptions::incomplete`).
    pub upload_id: Option<String>,
}

/// Options for [`list_objects_with`].
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    pub recursive: bool,
    /// Return every version and delete marker (ListObjectVersions).
    pub versions: bool,
    /// Return the state as of this time (implies a version listing, see [`resolve_rewind`]).
    pub rewind: Option<SystemTime>,
    /// Return incomplete multipart uploads instead of objects.
    pub incomplete: bool,
    /// MinIO only: list the contents of zip archives (`x-minio-extract: true`).
    pub zip: bool,
}

pub async fn list_target(
    alias: &AliasConfig,
    target: &TargetRef,
    recursive: bool,
) -> Result<Vec<S3ListItem>> {
    let client = build_client(alias).await?;

    match &target.bucket {
        None => list_buckets(&client).await,
        Some(bucket) => {
            list_objects(
                &client,
                bucket,
                target.key_with_trailing_slash().as_deref(),
                recursive,
            )
            .await
        }
    }
}

/// Recursive listing of current objects under `prefix`.
pub async fn list_object_infos(
    alias: &AliasConfig,
    bucket: &str,
    prefix: Option<&str>,
) -> Result<Vec<ObjectInfo>> {
    let client = build_client(alias).await?;
    list_objects_with(
        &client,
        bucket,
        prefix,
        &ListOptions {
            recursive: true,
            ..Default::default()
        },
    )
    .await
}

/// General listing entry point honoring [`ListOptions`]. Prefix entries are only returned for
/// non-recursive object/version listings.
///
/// Version selection: `versions` alone returns every version and delete marker; `rewind` alone
/// returns the state at that time ([`resolve_rewind`]); both together return every version and
/// delete marker modified at or before the rewind time ([`versions_before`]).
pub async fn list_objects_with(
    client: &Client,
    bucket: &str,
    prefix: Option<&str>,
    options: &ListOptions,
) -> Result<Vec<ObjectInfo>> {
    if options.incomplete {
        return list_incomplete_uploads(client, bucket, prefix, options.recursive).await;
    }
    if options.versions || options.rewind.is_some() {
        let versions = list_object_versions(client, bucket, prefix, options.recursive).await?;
        return Ok(match (options.versions, options.rewind) {
            (true, Some(at)) => versions_before(versions, at),
            (false, Some(at)) => resolve_rewind(versions, at),
            _ => versions,
        });
    }
    let normalized_prefix = normalize_prefix(prefix);
    let items = list_objects_raw(
        client,
        bucket,
        normalized_prefix.as_deref(),
        normalized_prefix.as_deref(),
        options.recursive,
        options.zip,
    )
    .await?;
    Ok(items
        .into_iter()
        .filter_map(|(item, modified)| match item {
            S3ListItem::Prefix { name } => Some(ObjectInfo {
                key: name,
                is_prefix: true,
                is_latest: true,
                ..Default::default()
            }),
            S3ListItem::Object {
                name,
                size,
                etag,
                storage_class,
                ..
            } => Some(ObjectInfo {
                key: name,
                size: size.unwrap_or(0),
                last_modified: modified,
                etag,
                storage_class,
                is_latest: true,
                ..Default::default()
            }),
            S3ListItem::Bucket { .. } => None,
        })
        .collect())
}

/// Lists all object versions and delete markers (paginated ListObjectVersions), sorted by key
/// ascending and then newest first.
pub async fn list_object_versions(
    client: &Client,
    bucket: &str,
    prefix: Option<&str>,
    recursive: bool,
) -> Result<Vec<ObjectInfo>> {
    let normalized_prefix = normalize_prefix(prefix);
    list_versions_inner(
        client,
        bucket,
        normalized_prefix.as_deref(),
        normalized_prefix.as_deref(),
        recursive,
    )
    .await
}

/// All versions and delete markers of exactly `key` (absolute key in `ObjectInfo::key`),
/// newest first.
pub async fn list_key_versions(
    client: &Client,
    bucket: &str,
    key: &str,
) -> Result<Vec<ObjectInfo>> {
    let mut items = list_versions_inner(client, bucket, Some(key), None, true).await?;
    items.retain(|item| item.key == key);
    Ok(items)
}

/// `request_prefix` is sent as-is; keys are reported relative to `display_prefix`.
async fn list_versions_inner(
    client: &Client,
    bucket: &str,
    request_prefix: Option<&str>,
    display_prefix: Option<&str>,
    recursive: bool,
) -> Result<Vec<ObjectInfo>> {
    let mut key_marker: Option<String> = None;
    let mut version_marker: Option<String> = None;
    let mut items = Vec::new();
    loop {
        let mut request = client.list_object_versions().bucket(bucket);
        if !recursive {
            request = request.delimiter("/");
        }
        if let Some(prefix) = request_prefix {
            request = request.prefix(prefix);
        }
        if let Some(marker) = key_marker.take() {
            request = request.key_marker(marker);
        }
        if let Some(marker) = version_marker.take() {
            request = request.version_id_marker(marker);
        }
        let response = request.send().await.s3(bucket, "")?;
        for prefix in response.common_prefixes() {
            if let Some(raw) = prefix.prefix() {
                items.push(ObjectInfo {
                    key: display_name(raw, display_prefix),
                    is_prefix: true,
                    is_latest: true,
                    ..Default::default()
                });
            }
        }
        for version in response.versions() {
            let Some(key) = version.key() else { continue };
            items.push(ObjectInfo {
                key: display_name(key, display_prefix),
                size: version.size().unwrap_or(0),
                last_modified: version.last_modified().and_then(to_system_time),
                etag: version.e_tag().map(str::to_string),
                storage_class: version.storage_class().map(|v| v.as_str().to_string()),
                version_id: version.version_id().map(str::to_string),
                is_latest: version.is_latest().unwrap_or(false),
                ..Default::default()
            });
        }
        for marker in response.delete_markers() {
            let Some(key) = marker.key() else { continue };
            items.push(ObjectInfo {
                key: display_name(key, display_prefix),
                last_modified: marker.last_modified().and_then(to_system_time),
                version_id: marker.version_id().map(str::to_string),
                is_latest: marker.is_latest().unwrap_or(false),
                is_delete_marker: true,
                ..Default::default()
            });
        }
        if response.is_truncated() == Some(true) {
            key_marker = response.next_key_marker().map(str::to_string);
            version_marker = response.next_version_id_marker().map(str::to_string);
            if key_marker.is_none() && version_marker.is_none() {
                break;
            }
        } else {
            break;
        }
    }
    sort_versions(&mut items);
    Ok(items)
}

fn sort_versions(items: &mut [ObjectInfo]) {
    items.sort_by(|left, right| {
        left.key
            .cmp(&right.key)
            .then_with(|| right.is_latest.cmp(&left.is_latest))
            .then_with(|| right.last_modified.cmp(&left.last_modified))
    });
}

/// Picks, per key, the newest version with `last_modified <= at`; keys whose chosen entry is a
/// delete marker (or that did not exist yet) are dropped. Prefix entries pass through.
/// The chosen entries keep their `version_id`; `is_latest` is left as reported by the server.
pub fn resolve_rewind(versions: Vec<ObjectInfo>, at: SystemTime) -> Vec<ObjectInfo> {
    let mut chosen: BTreeMap<String, ObjectInfo> = BTreeMap::new();
    let mut prefixes = Vec::new();
    for item in versions {
        if item.is_prefix {
            prefixes.push(item);
            continue;
        }
        let Some(modified) = item.last_modified else {
            continue;
        };
        if modified > at {
            continue;
        }
        match chosen.get(&item.key) {
            Some(existing)
                if existing.last_modified > Some(modified)
                    || (existing.last_modified == Some(modified) && existing.is_latest) => {}
            _ => {
                chosen.insert(item.key.clone(), item);
            }
        }
    }
    prefixes
        .into_iter()
        .chain(chosen.into_values().filter(|item| !item.is_delete_marker))
        .collect()
}

/// Every version and delete marker with `last_modified <= at` (prefix entries pass through).
pub fn versions_before(versions: Vec<ObjectInfo>, at: SystemTime) -> Vec<ObjectInfo> {
    versions
        .into_iter()
        .filter(|item| item.is_prefix || item.last_modified.is_some_and(|modified| modified <= at))
        .collect()
}

/// Lists incomplete multipart uploads (paginated ListMultipartUploads). `size` is 0; each
/// entry has `upload_id` and `last_modified` = initiation time.
pub async fn list_incomplete_uploads(
    client: &Client,
    bucket: &str,
    prefix: Option<&str>,
    recursive: bool,
) -> Result<Vec<ObjectInfo>> {
    let normalized_prefix = normalize_prefix(prefix);
    let items = list_uploads_inner(
        client,
        bucket,
        normalized_prefix.as_deref(),
        normalized_prefix.as_deref(),
        recursive,
    )
    .await?;
    let Some(prefix) = normalized_prefix.filter(|_| recursive && items.is_empty()) else {
        return Ok(items);
    };
    // MinIO only honors a prefix that names an object; list the bucket and filter instead.
    Ok(list_uploads_inner(client, bucket, None, None, true)
        .await?
        .into_iter()
        .filter_map(|mut item| {
            item.key = item.key.strip_prefix(&prefix)?.to_string();
            Some(item)
        })
        .collect())
}

/// Incomplete multipart uploads of exactly `key` (absolute key in `ObjectInfo::key`).
pub async fn list_key_uploads(client: &Client, bucket: &str, key: &str) -> Result<Vec<ObjectInfo>> {
    let mut items = list_uploads_inner(client, bucket, Some(key), None, true).await?;
    items.retain(|item| item.key == key);
    Ok(items)
}

async fn list_uploads_inner(
    client: &Client,
    bucket: &str,
    request_prefix: Option<&str>,
    display_prefix: Option<&str>,
    recursive: bool,
) -> Result<Vec<ObjectInfo>> {
    let mut key_marker: Option<String> = None;
    let mut upload_marker: Option<String> = None;
    let mut items = Vec::new();
    loop {
        let mut request = client.list_multipart_uploads().bucket(bucket);
        if !recursive {
            request = request.delimiter("/");
        }
        if let Some(prefix) = request_prefix {
            request = request.prefix(prefix);
        }
        if let Some(marker) = key_marker.take() {
            request = request.key_marker(marker);
        }
        if let Some(marker) = upload_marker.take() {
            request = request.upload_id_marker(marker);
        }
        let response = request.send().await.s3(bucket, "")?;
        for prefix in response.common_prefixes() {
            if let Some(raw) = prefix.prefix() {
                items.push(ObjectInfo {
                    key: display_name(raw, display_prefix),
                    is_prefix: true,
                    ..Default::default()
                });
            }
        }
        for upload in response.uploads() {
            let Some(key) = upload.key() else { continue };
            items.push(ObjectInfo {
                key: display_name(key, display_prefix),
                last_modified: upload.initiated().and_then(to_system_time),
                storage_class: upload.storage_class().map(|v| v.as_str().to_string()),
                upload_id: upload.upload_id().map(str::to_string),
                ..Default::default()
            });
        }
        if response.is_truncated() == Some(true) {
            key_marker = response.next_key_marker().map(str::to_string);
            upload_marker = response.next_upload_id_marker().map(str::to_string);
            if key_marker.is_none() && upload_marker.is_none() {
                break;
            }
        } else {
            break;
        }
    }
    Ok(items)
}

pub(crate) async fn list_buckets(client: &Client) -> Result<Vec<S3ListItem>> {
    let response = client.list_buckets().send().await.s3("", "")?;
    let mut items = Vec::new();

    for bucket in response.buckets() {
        let name = bucket.name().unwrap_or_default().to_string();
        items.push(S3ListItem::Bucket {
            name,
            last_modified: bucket.creation_date().map(debug_timestamp),
        });
    }

    Ok(items)
}

/// ListObjectsV2 (paginated). Names are relative to the normalized prefix.
pub async fn list_objects(
    client: &Client,
    bucket: &str,
    prefix: Option<&str>,
    recursive: bool,
) -> Result<Vec<S3ListItem>> {
    let normalized_prefix = normalize_prefix(prefix);
    Ok(list_objects_raw(
        client,
        bucket,
        normalized_prefix.as_deref(),
        normalized_prefix.as_deref(),
        recursive,
        false,
    )
    .await?
    .into_iter()
    .map(|(item, _)| item)
    .collect())
}

/// `request_prefix` is sent as-is; names are reported relative to `display_prefix`.
async fn list_objects_raw(
    client: &Client,
    bucket: &str,
    request_prefix: Option<&str>,
    display_prefix: Option<&str>,
    recursive: bool,
    zip: bool,
) -> Result<Vec<(S3ListItem, Option<SystemTime>)>> {
    let mut continuation = None;
    let mut items = Vec::new();

    loop {
        let mut request = client.list_objects_v2().bucket(bucket);
        if !recursive {
            request = request.delimiter("/");
        }
        if let Some(prefix) = request_prefix {
            request = request.prefix(prefix);
        }
        if let Some(token) = continuation {
            request = request.continuation_token(token);
        }

        let response = if zip {
            request
                .customize()
                .mutate_request(super::objects::add_zip_extract_header)
                .send()
                .await
                .s3(bucket, "")?
        } else {
            request.send().await.s3(bucket, "")?
        };

        for prefix in response.common_prefixes() {
            if let Some(raw) = prefix.prefix() {
                items.push((
                    S3ListItem::Prefix {
                        name: display_name(raw, display_prefix),
                    },
                    None,
                ));
            }
        }

        for object in response.contents() {
            if let Some(key) = object.key() {
                items.push((
                    S3ListItem::Object {
                        name: display_name(key, display_prefix),
                        size: object.size(),
                        last_modified: object.last_modified().map(debug_timestamp),
                        etag: object.e_tag().map(str::to_string),
                        storage_class: object
                            .storage_class()
                            .map(|value| value.as_str().to_string()),
                        version_id: None,
                        is_latest: true,
                        is_delete_marker: false,
                    },
                    object.last_modified().and_then(to_system_time),
                ));
            }
        }

        if response.is_truncated() == Some(true) {
            continuation = response.next_continuation_token().map(str::to_string);
        } else {
            break;
        }
    }

    Ok(items)
}

/// Listing like mc's S3 client for `ls`/`tree`/`du`: `prefix` is sent as-is (no `/` added) and
/// keys are absolute. Objects (or versions and delete markers, newest first per key) come
/// before common prefixes. `options.rewind` is ignored; filter the versions instead.
pub async fn list_raw(
    client: &Client,
    bucket: &str,
    prefix: &str,
    options: &ListOptions,
) -> Result<Vec<ObjectInfo>> {
    let prefix = Some(prefix).filter(|prefix| !prefix.is_empty());
    let mut items = if options.incomplete {
        let items = list_uploads_inner(client, bucket, prefix, None, options.recursive).await?;
        match prefix.filter(|_| options.recursive && items.is_empty()) {
            // MinIO only honors a prefix that names an object; list the bucket and filter.
            Some(prefix) => list_uploads_inner(client, bucket, None, None, true)
                .await?
                .into_iter()
                .filter(|item| item.key.starts_with(prefix))
                .collect(),
            None => items,
        }
    } else if options.versions {
        list_versions_inner(client, bucket, prefix, None, options.recursive).await?
    } else {
        list_objects_raw(client, bucket, prefix, None, options.recursive, options.zip)
            .await?
            .into_iter()
            .filter_map(|(item, modified)| match item {
                S3ListItem::Prefix { name } => Some(ObjectInfo {
                    key: name,
                    is_prefix: true,
                    is_latest: true,
                    ..Default::default()
                }),
                S3ListItem::Object {
                    name,
                    size,
                    etag,
                    storage_class,
                    ..
                } => Some(ObjectInfo {
                    key: name,
                    size: size.unwrap_or(0),
                    last_modified: modified,
                    etag,
                    storage_class,
                    is_latest: true,
                    ..Default::default()
                }),
                S3ListItem::Bucket { .. } => None,
            })
            .collect()
    };
    // Stable: keeps the server (or version) order within objects and within prefixes.
    items.sort_by_key(|item| item.is_prefix);
    Ok(items)
}

/// `true` when anything (an object, or with `versions` any version or delete marker) is
/// stored under `prefix`. mc treats such a target as a folder.
pub async fn prefix_has_entries(
    client: &Client,
    bucket: &str,
    prefix: &str,
    versions: bool,
) -> Result<bool> {
    if versions {
        let response = client
            .list_object_versions()
            .bucket(bucket)
            .prefix(prefix)
            .max_keys(1)
            .send()
            .await
            .s3(bucket, "")?;
        return Ok(!response.versions().is_empty()
            || !response.delete_markers().is_empty()
            || !response.common_prefixes().is_empty());
    }
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

/// Bucket names with their creation time.
pub async fn list_bucket_infos(client: &Client) -> Result<Vec<(String, Option<SystemTime>)>> {
    let response = client.list_buckets().send().await.s3("", "")?;
    Ok(response
        .buckets()
        .iter()
        .map(|bucket| {
            (
                bucket.name().unwrap_or_default().to_string(),
                bucket.creation_date().and_then(to_system_time),
            )
        })
        .collect())
}

/// `true` when the bucket has no objects, versions or delete markers.
pub async fn bucket_is_empty(client: &Client, bucket: &str) -> Result<bool> {
    match client
        .list_object_versions()
        .bucket(bucket)
        .max_keys(1)
        .send()
        .await
    {
        Ok(response) => Ok(response.versions().is_empty() && response.delete_markers().is_empty()),
        Err(_) => {
            let response = client
                .list_objects_v2()
                .bucket(bucket)
                .max_keys(1)
                .send()
                .await
                .s3(bucket, "")?;
            Ok(response.contents().is_empty())
        }
    }
}

pub(crate) fn normalize_prefix(prefix: Option<&str>) -> Option<String> {
    prefix
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let mut normalized = value.trim_matches('/').to_string();
            if !normalized.is_empty() && !normalized.ends_with('/') {
                normalized.push('/');
            }
            normalized
        })
        .filter(|value| !value.is_empty())
}

pub(crate) fn display_name(raw: &str, prefix: Option<&str>) -> String {
    let stripped = prefix
        .and_then(|prefix| raw.strip_prefix(prefix))
        .unwrap_or(raw);
    stripped.to_string()
}

/// Joins a listing prefix and a relative name into an absolute key.
///
/// Listings strip the normalized prefix (`dir/`), so an empty name is the folder-marker object
/// `dir/` itself, never the sibling key `dir`.
pub fn full_key(prefix: &str, name: &str) -> String {
    let prefix = prefix.trim_matches('/');
    if prefix.is_empty() {
        name.to_string()
    } else if name.is_empty() {
        format!("{prefix}/")
    } else {
        format!("{prefix}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ObjectInfo, display_name, full_key, normalize_prefix, resolve_rewind, versions_before,
    };
    use std::time::{Duration, UNIX_EPOCH};

    fn version(key: &str, secs: u64, id: &str, delete: bool) -> ObjectInfo {
        ObjectInfo {
            key: key.into(),
            last_modified: Some(UNIX_EPOCH + Duration::from_secs(secs)),
            version_id: Some(id.into()),
            is_delete_marker: delete,
            ..Default::default()
        }
    }

    #[test]
    fn full_key_keeps_folder_marker_distinct_from_sibling() {
        for input in ["pre", "pre/"] {
            let prefix = normalize_prefix(Some(input)).unwrap();
            for raw in ["pre/", "pre/inner", "pre//x"] {
                assert_eq!(full_key(input, &display_name(raw, Some(&prefix))), raw);
            }
        }
        assert_eq!(full_key("", "a/b"), "a/b");
    }

    #[test]
    fn rewind_picks_newest_version_before_time() {
        let versions = vec![
            version("a", 10, "a1", false),
            version("a", 20, "a2", false),
            version("a", 30, "a3", false),
            version("b", 10, "b1", false),
            version("b", 15, "b2", true),
            version("c", 40, "c1", false),
            version("d", 5, "d1", true),
            version("d", 12, "d2", false),
        ];
        let result = resolve_rewind(versions, UNIX_EPOCH + Duration::from_secs(25));
        let picked: Vec<_> = result
            .iter()
            .map(|item| item.version_id.as_deref().unwrap())
            .collect();
        assert_eq!(picked, ["a2", "d2"]);
    }

    #[test]
    fn versions_before_keeps_all_older_versions_and_prefixes() {
        let mut versions = vec![
            version("a", 10, "a1", false),
            version("a", 30, "a3", false),
            version("b", 15, "b2", true),
        ];
        versions.push(ObjectInfo {
            key: "dir/".into(),
            is_prefix: true,
            ..Default::default()
        });
        let result = versions_before(versions, UNIX_EPOCH + Duration::from_secs(20));
        let keys: Vec<_> = result
            .iter()
            .map(|item| item.version_id.as_deref().unwrap_or(&item.key))
            .collect();
        assert_eq!(keys, ["a1", "b2", "dir/"]);
    }
}
