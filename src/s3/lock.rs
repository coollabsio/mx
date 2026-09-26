//! Object lock helpers (area G): retention, legal hold, bucket default retention, the object
//! selection used by `retention`/`legalhold`/`undo`/`ilm restore`, and RestoreObject.

use super::{ObjectInfo, error_code, from_system_time, resolve_rewind, to_system_time};
use crate::flags::ValidityUnit;
use crate::s3::S3ResultExt;
use anyhow::{Result, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::types::{
    DefaultRetention, GlacierJobParameters, ObjectLockConfiguration, ObjectLockEnabled,
    ObjectLockLegalHold, ObjectLockLegalHoldStatus, ObjectLockRetention, ObjectLockRetentionMode,
    ObjectLockRule, RestoreRequest, Tier,
};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// object selection
// ---------------------------------------------------------------------------

/// Lists every version and delete marker whose key starts with `prefix` (raw prefix, no
/// delimiter). Keys are absolute. Sorted by key, then latest/newest first.
pub async fn list_versions_under(
    client: &Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<ObjectInfo>> {
    let mut key_marker: Option<String> = None;
    let mut version_marker: Option<String> = None;
    let mut items = Vec::new();
    loop {
        let mut request = client.list_object_versions().bucket(bucket);
        if !prefix.is_empty() {
            request = request.prefix(prefix);
        }
        if let Some(marker) = key_marker.take() {
            request = request.key_marker(marker);
        }
        if let Some(marker) = version_marker.take() {
            request = request.version_id_marker(marker);
        }
        let response = request.send().await.s3(bucket, "")?;
        for version in response.versions() {
            let Some(key) = version.key() else { continue };
            items.push(ObjectInfo {
                key: key.to_string(),
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
                key: key.to_string(),
                last_modified: marker.last_modified().and_then(to_system_time),
                version_id: marker.version_id().map(str::to_string),
                is_latest: marker.is_latest().unwrap_or(false),
                is_delete_marker: true,
                ..Default::default()
            });
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
    items.sort_by(|left, right| {
        left.key
            .cmp(&right.key)
            .then_with(|| right.is_latest.cmp(&left.is_latest))
            .then_with(|| right.last_modified.cmp(&left.last_modified))
    });
    Ok(items)
}

/// Which objects/versions a `retention`/`legalhold` style command operates on.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Match every key starting with the target key (instead of the exact key).
    pub recursive: bool,
    /// Include all (non delete marker) versions up to `rewind` (or now).
    pub versions: bool,
    /// Pick the version current at this time (with `versions`: all versions up to it).
    pub rewind: Option<SystemTime>,
}

/// Applies mc's selection rules to a [`list_versions_under`] result (delete markers are never
/// selected). Without `versions`/`rewind` the current objects are returned with `version_id`
/// cleared, like a plain listing.
pub fn select_versions(
    items: Vec<ObjectInfo>,
    key: &str,
    selection: &Selection,
) -> Vec<ObjectInfo> {
    let items: Vec<ObjectInfo> = items
        .into_iter()
        .filter(|item| {
            if selection.recursive {
                item.key.starts_with(key)
            } else {
                item.key == key
            }
        })
        .collect();
    if selection.versions {
        return items
            .into_iter()
            .filter(|item| !item.is_delete_marker)
            .filter(|item| match (selection.rewind, item.last_modified) {
                (Some(at), Some(modified)) => modified <= at,
                (Some(_), None) => false,
                (None, _) => true,
            })
            .collect();
    }
    if let Some(at) = selection.rewind {
        return resolve_rewind(items, at);
    }
    items
        .into_iter()
        .filter(|item| item.is_latest && !item.is_delete_marker)
        .map(|item| ObjectInfo {
            version_id: None,
            ..item
        })
        .collect()
}

/// Lists and selects objects for `key` in one call (see [`select_versions`]).
pub async fn select_objects(
    client: &Client,
    bucket: &str,
    key: &str,
    selection: &Selection,
) -> Result<Vec<ObjectInfo>> {
    let items = list_versions_under(client, bucket, key).await?;
    Ok(select_versions(items, key, selection))
}

/// Picks what `mx undo` removes for one key: the newest `last` versions/delete markers
/// (`versions` sorted latest first). Returns nothing when `action` (`PUT`/`DELETE`) does not
/// match the latest entry.
pub fn undo_candidates(
    versions: &[ObjectInfo],
    last: usize,
    action: Option<&str>,
) -> Vec<ObjectInfo> {
    if let Some(latest) = versions.iter().find(|item| item.is_latest) {
        match action {
            Some("DELETE") if !latest.is_delete_marker => return Vec::new(),
            Some("PUT") if latest.is_delete_marker => return Vec::new(),
            _ => {}
        }
    }
    versions.iter().take(last).cloned().collect()
}

/// Permanently deletes one object version or delete marker.
pub async fn delete_object_version(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: &str,
) -> Result<()> {
    client
        .delete_object()
        .bucket(bucket)
        .key(key)
        .version_id(version_id)
        .send()
        .await
        .s3(bucket, "")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// retention
// ---------------------------------------------------------------------------

/// Parses `GOVERNANCE` / `COMPLIANCE` (case-insensitive).
pub fn parse_retention_mode(value: &str) -> Result<ObjectLockRetentionMode> {
    match value.to_ascii_uppercase().as_str() {
        "GOVERNANCE" => Ok(ObjectLockRetentionMode::Governance),
        "COMPLIANCE" => Ok(ObjectLockRetentionMode::Compliance),
        _ => bail!("invalid retention mode '{value}': use GOVERNANCE or COMPLIANCE"),
    }
}

/// PutObjectRetention. `retention = None` clears the retention (needs `bypass` for
/// GOVERNANCE).
pub async fn put_object_retention(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
    retention: Option<(ObjectLockRetentionMode, SystemTime)>,
    bypass: bool,
) -> Result<()> {
    let mut body = ObjectLockRetention::builder();
    if let Some((mode, until)) = retention {
        body = body.mode(mode).retain_until_date(from_system_time(until));
    }
    let mut request = client
        .put_object_retention()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .retention(body.build());
    if bypass {
        request = request.bypass_governance_retention(true);
    }
    request.send().await.s3(bucket, "")?;
    Ok(())
}

/// GetObjectRetention: `(mode, retain until)`; `None` when the object has no retention.
pub async fn get_object_retention(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
) -> Result<Option<(String, Option<SystemTime>)>> {
    let result = client
        .get_object_retention()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .send()
        .await;
    match result {
        Ok(output) => Ok(output.retention().and_then(|retention| {
            let mode = retention.mode()?.as_str().to_string();
            Some((mode, retention.retain_until_date().and_then(to_system_time)))
        })),
        Err(error) if error_code(&error) == Some("NoSuchObjectLockConfiguration") => Ok(None),
        Err(error) => Err(super::error::s3_error(&error, bucket, "")),
    }
}

// ---------------------------------------------------------------------------
// legal hold
// ---------------------------------------------------------------------------

pub async fn put_object_legal_hold(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
    on: bool,
) -> Result<()> {
    let status = if on {
        ObjectLockLegalHoldStatus::On
    } else {
        ObjectLockLegalHoldStatus::Off
    };
    client
        .put_object_legal_hold()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .legal_hold(ObjectLockLegalHold::builder().status(status).build())
        .send()
        .await
        .s3(bucket, "")?;
    Ok(())
}

/// GetObjectLegalHold: `ON`/`OFF`, or `None` when never set.
pub async fn get_object_legal_hold(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
) -> Result<Option<String>> {
    let result = client
        .get_object_legal_hold()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .send()
        .await;
    match result {
        Ok(output) => Ok(output
            .legal_hold()
            .and_then(|hold| hold.status())
            .map(|status| status.as_str().to_string())),
        Err(error) if error_code(&error) == Some("NoSuchObjectLockConfiguration") => Ok(None),
        Err(error) => Err(super::error::s3_error(&error, bucket, "")),
    }
}

// ---------------------------------------------------------------------------
// bucket lock configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketLockConfig {
    /// `Enabled` when object lock is enabled on the bucket.
    pub status: String,
    /// Default retention rule `(mode, validity, unit)`, if any.
    pub rule: Option<(String, u32, ValidityUnit)>,
}

/// GetObjectLockConfiguration; `None` when the bucket has no object lock.
pub async fn get_bucket_lock_config(
    client: &Client,
    bucket: &str,
) -> Result<Option<BucketLockConfig>> {
    let result = client
        .get_object_lock_configuration()
        .bucket(bucket)
        .send()
        .await;
    let output = match result {
        Ok(output) => output,
        Err(error) if error_code(&error) == Some("ObjectLockConfigurationNotFoundError") => {
            super::bucket::require_bucket(client, bucket).await?;
            return Ok(None);
        }
        Err(error) => return Err(super::error::s3_error(&error, bucket, "")),
    };
    let Some(config) = output.object_lock_configuration() else {
        return Ok(None);
    };
    let rule = config
        .rule()
        .and_then(|rule| rule.default_retention())
        .and_then(|retention| {
            let mode = retention.mode()?.as_str().to_string();
            match (retention.days(), retention.years()) {
                (Some(days), _) if days > 0 => Some((mode, days as u32, ValidityUnit::Days)),
                (_, Some(years)) if years > 0 => Some((mode, years as u32, ValidityUnit::Years)),
                _ => None,
            }
        });
    Ok(Some(BucketLockConfig {
        status: config
            .object_lock_enabled()
            .map(|status| status.as_str().to_string())
            .unwrap_or_default(),
        rule,
    }))
}

/// PutObjectLockConfiguration with the given default retention, or none (clears it).
pub async fn put_bucket_lock_config(
    client: &Client,
    bucket: &str,
    rule: Option<(ObjectLockRetentionMode, u32, ValidityUnit)>,
) -> Result<()> {
    super::bucket::require_bucket(client, bucket).await?;
    client
        .put_object_lock_configuration()
        .bucket(bucket)
        .object_lock_configuration(bucket_lock_configuration(rule))
        .send()
        .await
        .s3(bucket, "")?;
    Ok(())
}

fn bucket_lock_configuration(
    rule: Option<(ObjectLockRetentionMode, u32, ValidityUnit)>,
) -> ObjectLockConfiguration {
    let mut config =
        ObjectLockConfiguration::builder().object_lock_enabled(ObjectLockEnabled::Enabled);
    if let Some((mode, count, unit)) = rule {
        let retention = DefaultRetention::builder().mode(mode);
        let retention = match unit {
            ValidityUnit::Days => retention.days(count as i32),
            ValidityUnit::Years => retention.years(count as i32),
        };
        config = config.rule(
            ObjectLockRule::builder()
                .default_retention(retention.build())
                .build(),
        );
    }
    config.build()
}

// ---------------------------------------------------------------------------
// restore (ilm restore)
// ---------------------------------------------------------------------------

/// RestoreObject request body used by mc: `Days` + expedited Glacier tier.
pub fn restore_request(days: i32) -> Result<RestoreRequest> {
    Ok(RestoreRequest::builder()
        .days(days)
        .glacier_job_parameters(
            GlacierJobParameters::builder()
                .tier(Tier::Expedited)
                .build()?,
        )
        .build())
}

pub async fn restore_object(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
    days: i32,
) -> Result<()> {
    client
        .restore_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .restore_request(restore_request(days)?)
        .send()
        .await
        .s3(bucket, "")?;
    Ok(())
}

/// Restore state from HeadObject `x-amz-restore`: `None` = no restore requested,
/// `Some(true)` = still in progress, `Some(false)` = restored.
pub async fn restore_ongoing(
    client: &Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
) -> Result<Option<bool>> {
    let head = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .send()
        .await
        .s3(bucket, "")?;
    Ok(head.restore().map(parse_restore_ongoing))
}

/// Parses `ongoing-request="true", expiry-date="..."`.
pub fn parse_restore_ongoing(header: &str) -> bool {
    header
        .split(',')
        .filter_map(|part| part.trim().split_once('='))
        .any(|(name, value)| name.trim() == "ongoing-request" && value.trim_matches('"') == "true")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn version(key: &str, secs: u64, id: &str, latest: bool, delete: bool) -> ObjectInfo {
        ObjectInfo {
            key: key.into(),
            last_modified: Some(at(secs)),
            version_id: Some(id.into()),
            is_latest: latest,
            is_delete_marker: delete,
            ..Default::default()
        }
    }

    fn sample() -> Vec<ObjectInfo> {
        vec![
            version("a", 30, "a3", true, false),
            version("a", 20, "a2", false, false),
            version("a", 10, "a1", false, false),
            version("ab", 10, "ab1", true, false),
            version("d/x", 40, "x2", true, true),
            version("d/x", 5, "x1", false, false),
        ]
    }

    fn ids(items: &[ObjectInfo]) -> Vec<String> {
        items
            .iter()
            .map(|item| format!("{}@{}", item.key, item.version_id.as_deref().unwrap_or("-")))
            .collect()
    }

    #[test]
    fn selects_current_exact_object() {
        let selected = select_versions(sample(), "a", &Selection::default());
        assert_eq!(ids(&selected), ["a@-"]);
        let selected = select_versions(sample(), "d/x", &Selection::default());
        assert!(selected.is_empty(), "delete marker is not selectable");
    }

    #[test]
    fn selects_recursively_by_raw_prefix() {
        let selection = Selection {
            recursive: true,
            ..Default::default()
        };
        assert_eq!(
            ids(&select_versions(sample(), "a", &selection)),
            ["a@-", "ab@-"]
        );
        assert_eq!(
            ids(&select_versions(sample(), "", &selection)),
            ["a@-", "ab@-"]
        );
    }

    #[test]
    fn selects_versions_and_rewind() {
        let versions = Selection {
            versions: true,
            ..Default::default()
        };
        assert_eq!(
            ids(&select_versions(sample(), "a", &versions)),
            ["a@a3", "a@a2", "a@a1"]
        );
        let versions_rewind = Selection {
            versions: true,
            rewind: Some(at(20)),
            ..Default::default()
        };
        assert_eq!(
            ids(&select_versions(sample(), "a", &versions_rewind)),
            ["a@a2", "a@a1"]
        );
        let rewind = Selection {
            recursive: true,
            rewind: Some(at(25)),
            ..Default::default()
        };
        assert_eq!(
            ids(&select_versions(sample(), "", &rewind)),
            ["a@a2", "ab@ab1", "d/x@x1"]
        );
    }

    #[test]
    fn picks_undo_candidates() {
        let put_latest = vec![
            version("k", 30, "v3", true, false),
            version("k", 20, "v2", false, true),
            version("k", 10, "v1", false, false),
        ];
        assert_eq!(ids(&undo_candidates(&put_latest, 1, None)), ["k@v3"]);
        assert_eq!(
            ids(&undo_candidates(&put_latest, 5, None)),
            ["k@v3", "k@v2", "k@v1"]
        );
        assert_eq!(ids(&undo_candidates(&put_latest, 1, Some("PUT"))), ["k@v3"]);
        assert!(undo_candidates(&put_latest, 1, Some("DELETE")).is_empty());
        let delete_latest = &put_latest[1..]
            .iter()
            .map(|item| ObjectInfo {
                is_latest: item.version_id.as_deref() == Some("v2"),
                ..item.clone()
            })
            .collect::<Vec<_>>();
        assert!(undo_candidates(delete_latest, 1, Some("PUT")).is_empty());
        assert_eq!(
            ids(&undo_candidates(delete_latest, 1, Some("DELETE"))),
            ["k@v2"]
        );
    }

    #[test]
    fn parses_mode_and_validity() {
        assert_eq!(
            parse_retention_mode("governance").unwrap(),
            ObjectLockRetentionMode::Governance
        );
        assert_eq!(
            parse_retention_mode("COMPLIANCE").unwrap(),
            ObjectLockRetentionMode::Compliance
        );
        assert!(parse_retention_mode("legal").is_err());
    }

    #[test]
    fn builds_bucket_lock_configuration() {
        let config = bucket_lock_configuration(Some((
            ObjectLockRetentionMode::Governance,
            30,
            ValidityUnit::Days,
        )));
        let retention = config.rule().unwrap().default_retention().unwrap();
        assert_eq!(retention.mode(), Some(&ObjectLockRetentionMode::Governance));
        assert_eq!(retention.days(), Some(30));
        assert_eq!(retention.years(), None);
        let cleared = bucket_lock_configuration(None);
        assert!(cleared.rule().is_none());
        assert_eq!(
            cleared.object_lock_enabled(),
            Some(&ObjectLockEnabled::Enabled)
        );
    }

    #[test]
    fn builds_restore_request() {
        let request = restore_request(3).unwrap();
        assert_eq!(request.days(), Some(3));
        assert_eq!(
            request.glacier_job_parameters().map(|p| p.tier()),
            Some(&Tier::Expedited)
        );
    }

    #[test]
    fn parses_restore_header() {
        assert!(parse_restore_ongoing(r#"ongoing-request="true""#));
        assert!(!parse_restore_ongoing(
            r#"ongoing-request="false", expiry-date="Fri, 21 Dec 2012 00:00:00 GMT""#
        ));
    }
}
