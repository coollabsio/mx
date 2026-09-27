//! Bucket lifecycle (ILM) configuration (area F; H/G may add tier/restore helpers in their own
//! modules).
//!
//! The model mirrors minio-go `pkg/lifecycle` (JSON field names used by `mc ilm rule export`
//! and `import`) and is sent/read as raw XML so MinIO extensions such as
//! `ExpiredObjectAllVersions` and `DelMarkerExpiration` survive round trips.

use super::bucket::{Capture, S3_XMLNS, XmlNode, has_error_code, raw_body_override, xml_element};
use super::build_client;
use crate::config::model::AliasConfig;
use crate::s3::S3ResultExt;
use anyhow::{Context, Result, anyhow, bail};
use aws_sdk_s3::types::BucketLifecycleConfiguration;
use serde::{Deserialize, Serialize};

fn is_zero(value: &i64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleConfig {
    #[serde(rename = "Rules", default)]
    pub rules: Vec<LifecycleRule>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LifecycleRule {
    #[serde(
        default,
        skip_serializing_if = "AbortIncompleteMultipartUpload::is_null"
    )]
    pub abort_incomplete_multipart_upload: AbortIncompleteMultipartUpload,
    #[serde(default, skip_serializing_if = "Expiration::is_null")]
    pub expiration: Expiration,
    #[serde(default, skip_serializing_if = "DelMarkerExpiration::is_null")]
    pub del_marker_expiration: DelMarkerExpiration,
    #[serde(default, skip_serializing_if = "AllVersionsExpiration::is_null")]
    pub all_versions_expiration: AllVersionsExpiration,
    #[serde(rename = "ID", default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Filter::is_null")]
    pub filter: Filter,
    #[serde(default, skip_serializing_if = "NoncurrentVersionExpiration::is_null")]
    pub noncurrent_version_expiration: NoncurrentVersionExpiration,
    #[serde(default, skip_serializing_if = "NoncurrentVersionTransition::is_null")]
    pub noncurrent_version_transition: NoncurrentVersionTransition,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "Transition::is_null")]
    pub transition: Transition,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AbortIncompleteMultipartUpload {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub days_after_initiation: i64,
}

impl AbortIncompleteMultipartUpload {
    pub fn is_null(&self) -> bool {
        self.days_after_initiation == 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Expiration {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub date: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub days: i64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub expired_object_delete_marker: bool,
    /// MinIO extension: expire all versions of an object.
    #[serde(default, skip_serializing_if = "is_false")]
    pub expired_object_all_versions: bool,
}

impl Expiration {
    pub fn is_null(&self) -> bool {
        self.date.is_empty()
            && self.days == 0
            && !self.expired_object_delete_marker
            && !self.expired_object_all_versions
    }
}

/// MinIO extension.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DelMarkerExpiration {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub days: i64,
}

impl DelMarkerExpiration {
    pub fn is_null(&self) -> bool {
        self.days == 0
    }
}

/// MinIO extension.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AllVersionsExpiration {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub days: i64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub delete_marker: bool,
}

impl AllVersionsExpiration {
    pub fn is_null(&self) -> bool {
        self.days == 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Tag {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
}

impl Tag {
    pub fn is_empty(&self) -> bool {
        self.key.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct And {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub object_size_less_than: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub object_size_greater_than: i64,
}

impl And {
    pub fn is_empty(&self) -> bool {
        self.prefix.is_empty()
            && self.tags.is_empty()
            && self.object_size_less_than == 0
            && self.object_size_greater_than == 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Filter {
    #[serde(default, skip_serializing_if = "And::is_empty")]
    pub and: And,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(default, skip_serializing_if = "Tag::is_empty")]
    pub tag: Tag,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub object_size_less_than: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub object_size_greater_than: i64,
}

impl Filter {
    pub fn is_null(&self) -> bool {
        self.and.is_empty()
            && self.prefix.is_empty()
            && self.tag.is_empty()
            && self.object_size_less_than == 0
            && self.object_size_greater_than == 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NoncurrentVersionExpiration {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub noncurrent_days: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub newer_noncurrent_versions: i64,
}

impl NoncurrentVersionExpiration {
    pub fn is_null(&self) -> bool {
        self.noncurrent_days == 0 && self.newer_noncurrent_versions == 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NoncurrentVersionTransition {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub storage_class: String,
    #[serde(default)]
    pub noncurrent_days: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub newer_noncurrent_versions: i64,
}

impl NoncurrentVersionTransition {
    pub fn is_null(&self) -> bool {
        self.storage_class.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Transition {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub storage_class: String,
    #[serde(default)]
    pub days: i64,
}

impl Transition {
    pub fn is_null(&self) -> bool {
        self.storage_class.is_empty()
    }
}

// ---------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------

impl LifecycleConfig {
    pub fn to_xml(&self) -> String {
        let mut out = format!(r#"<LifecycleConfiguration xmlns="{S3_XMLNS}">"#);
        for rule in &self.rules {
            rule.write_xml(&mut out);
        }
        out.push_str("</LifecycleConfiguration>");
        out
    }

    pub fn from_xml(xml: &str) -> Result<Self> {
        let node = XmlNode::parse(xml)?;
        if node.name != "LifecycleConfiguration" {
            bail!("expected <LifecycleConfiguration>, found <{}>", node.name);
        }
        Ok(Self {
            rules: node
                .children_named("Rule")
                .map(LifecycleRule::from_xml)
                .collect::<Result<_>>()?,
        })
    }
}

fn int(node: &XmlNode, name: &str) -> Result<i64> {
    node.child_text(name)
        .filter(|text| !text.is_empty())
        .map(|text| {
            text.parse::<i64>()
                .with_context(|| format!("invalid <{name}> value `{text}`"))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn text(node: &XmlNode, name: &str) -> String {
    node.child_text(name).unwrap_or_default().to_string()
}

fn flag(node: &XmlNode, name: &str) -> bool {
    node.child_text(name)
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

fn tag_from_xml(node: &XmlNode) -> Tag {
    Tag {
        key: text(node, "Key"),
        value: text(node, "Value"),
    }
}

fn write_tag(out: &mut String, tag: &Tag) {
    out.push_str("<Tag>");
    xml_element(out, "Key", &tag.key);
    xml_element(out, "Value", &tag.value);
    out.push_str("</Tag>");
}

fn write_int(out: &mut String, name: &str, value: i64) {
    if value != 0 {
        xml_element(out, name, &value.to_string());
    }
}

impl LifecycleRule {
    fn write_xml(&self, out: &mut String) {
        out.push_str("<Rule>");
        if !self.abort_incomplete_multipart_upload.is_null() {
            out.push_str("<AbortIncompleteMultipartUpload>");
            write_int(
                out,
                "DaysAfterInitiation",
                self.abort_incomplete_multipart_upload.days_after_initiation,
            );
            out.push_str("</AbortIncompleteMultipartUpload>");
        }
        if !self.expiration.is_null() {
            let expiration = &self.expiration;
            out.push_str("<Expiration>");
            if !expiration.date.is_empty() {
                xml_element(out, "Date", &expiration.date);
            }
            write_int(out, "Days", expiration.days);
            if expiration.expired_object_delete_marker {
                xml_element(out, "ExpiredObjectDeleteMarker", "true");
            }
            if expiration.expired_object_all_versions {
                xml_element(out, "ExpiredObjectAllVersions", "true");
            }
            out.push_str("</Expiration>");
        }
        if !self.del_marker_expiration.is_null() {
            out.push_str("<DelMarkerExpiration>");
            write_int(out, "Days", self.del_marker_expiration.days);
            out.push_str("</DelMarkerExpiration>");
        }
        if !self.all_versions_expiration.is_null() {
            out.push_str("<AllVersionsExpiration>");
            write_int(out, "Days", self.all_versions_expiration.days);
            if self.all_versions_expiration.delete_marker {
                xml_element(out, "DeleteMarker", "true");
            }
            out.push_str("</AllVersionsExpiration>");
        }
        xml_element(out, "ID", &self.id);
        if !self.filter.is_null() || self.prefix.is_empty() {
            self.filter.write_xml(out);
        }
        if !self.noncurrent_version_expiration.is_null() {
            let item = &self.noncurrent_version_expiration;
            out.push_str("<NoncurrentVersionExpiration>");
            write_int(out, "NoncurrentDays", item.noncurrent_days);
            write_int(
                out,
                "NewerNoncurrentVersions",
                item.newer_noncurrent_versions,
            );
            out.push_str("</NoncurrentVersionExpiration>");
        }
        if !self.noncurrent_version_transition.is_null() {
            let item = &self.noncurrent_version_transition;
            out.push_str("<NoncurrentVersionTransition>");
            xml_element(out, "StorageClass", &item.storage_class);
            write_int(out, "NoncurrentDays", item.noncurrent_days);
            write_int(
                out,
                "NewerNoncurrentVersions",
                item.newer_noncurrent_versions,
            );
            out.push_str("</NoncurrentVersionTransition>");
        }
        if !self.prefix.is_empty() {
            xml_element(out, "Prefix", &self.prefix);
        }
        xml_element(out, "Status", &self.status);
        if !self.transition.is_null() {
            let transition = &self.transition;
            out.push_str("<Transition>");
            if !transition.date.is_empty() {
                xml_element(out, "Date", &transition.date);
            }
            xml_element(out, "StorageClass", &transition.storage_class);
            write_int(out, "Days", transition.days);
            out.push_str("</Transition>");
        }
        out.push_str("</Rule>");
    }

    fn from_xml(node: &XmlNode) -> Result<Self> {
        let mut rule = LifecycleRule {
            id: text(node, "ID"),
            prefix: text(node, "Prefix"),
            status: text(node, "Status"),
            ..Default::default()
        };
        if let Some(item) = node.child("AbortIncompleteMultipartUpload") {
            rule.abort_incomplete_multipart_upload.days_after_initiation =
                int(item, "DaysAfterInitiation")?;
        }
        if let Some(item) = node.child("Expiration") {
            rule.expiration = Expiration {
                date: text(item, "Date"),
                days: int(item, "Days")?,
                expired_object_delete_marker: flag(item, "ExpiredObjectDeleteMarker"),
                expired_object_all_versions: flag(item, "ExpiredObjectAllVersions"),
            };
        }
        if let Some(item) = node.child("DelMarkerExpiration") {
            rule.del_marker_expiration.days = int(item, "Days")?;
        }
        if let Some(item) = node.child("AllVersionsExpiration") {
            rule.all_versions_expiration = AllVersionsExpiration {
                days: int(item, "Days")?,
                delete_marker: flag(item, "DeleteMarker"),
            };
        }
        if let Some(item) = node.child("Filter") {
            rule.filter = Filter {
                prefix: text(item, "Prefix"),
                tag: item.child("Tag").map(tag_from_xml).unwrap_or_default(),
                object_size_less_than: int(item, "ObjectSizeLessThan")?,
                object_size_greater_than: int(item, "ObjectSizeGreaterThan")?,
                and: match item.child("And") {
                    Some(and) => And {
                        prefix: text(and, "Prefix"),
                        tags: and.children_named("Tag").map(tag_from_xml).collect(),
                        object_size_less_than: int(and, "ObjectSizeLessThan")?,
                        object_size_greater_than: int(and, "ObjectSizeGreaterThan")?,
                    },
                    None => And::default(),
                },
            };
        }
        if let Some(item) = node.child("NoncurrentVersionExpiration") {
            rule.noncurrent_version_expiration = NoncurrentVersionExpiration {
                noncurrent_days: int(item, "NoncurrentDays")?,
                newer_noncurrent_versions: int(item, "NewerNoncurrentVersions")?,
            };
        }
        if let Some(item) = node.child("NoncurrentVersionTransition") {
            rule.noncurrent_version_transition = NoncurrentVersionTransition {
                storage_class: text(item, "StorageClass"),
                noncurrent_days: int(item, "NoncurrentDays")?,
                newer_noncurrent_versions: int(item, "NewerNoncurrentVersions")?,
            };
        }
        if let Some(item) = node.child("Transition") {
            rule.transition = Transition {
                date: text(item, "Date"),
                storage_class: text(item, "StorageClass"),
                days: int(item, "Days")?,
            };
        }
        Ok(rule)
    }
}

impl Filter {
    /// minio-go `Filter.MarshalXML`: `And` wins, then `Tag`, then a single size predicate,
    /// else `Prefix` (possibly empty).
    fn write_xml(&self, out: &mut String) {
        out.push_str("<Filter>");
        if !self.and.is_empty() {
            out.push_str("<And>");
            xml_element(out, "Prefix", &self.and.prefix);
            for tag in &self.and.tags {
                write_tag(out, tag);
            }
            write_int(out, "ObjectSizeLessThan", self.and.object_size_less_than);
            write_int(
                out,
                "ObjectSizeGreaterThan",
                self.and.object_size_greater_than,
            );
            out.push_str("</And>");
        } else if !self.tag.is_empty() {
            write_tag(out, &self.tag);
        } else if self.object_size_less_than > 0 {
            write_int(out, "ObjectSizeLessThan", self.object_size_less_than);
        } else if self.object_size_greater_than > 0 {
            write_int(out, "ObjectSizeGreaterThan", self.object_size_greater_than);
        } else {
            xml_element(out, "Prefix", &self.prefix);
        }
        out.push_str("</Filter>");
    }
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

/// Lifecycle configuration plus the MinIO `updatedAt` timestamp (if reported).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleInfo {
    pub config: LifecycleConfig,
    /// Go `time.Time` JSON form (RFC3339Nano, zero time when the server sends none).
    pub updated_at: String,
}

/// `20260926T163004Z` (minio-go `iso8601DateFormat` header) -> Go RFC3339 JSON form
/// `2026-09-26T16:30:04Z`; the zero time when the server sends none.
fn go_time(header: Option<&str>) -> String {
    let value = header.unwrap_or_default();
    let digits = |range: std::ops::Range<usize>| {
        value
            .get(range)
            .filter(|part| part.bytes().all(|b| b.is_ascii_digit()))
    };
    match (
        digits(0..4),
        digits(4..6),
        digits(6..8),
        digits(9..11),
        digits(11..13),
        digits(13..15),
    ) {
        (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) if value.len() == 16 => {
            format!("{y}-{mo}-{d}T{h}:{mi}:{s}Z")
        }
        _ => "0001-01-01T00:00:00Z".to_string(),
    }
}

/// Returns None when the bucket has no lifecycle configuration.
pub async fn get_lifecycle(alias: &AliasConfig, bucket: &str) -> Result<Option<LifecycleInfo>> {
    fetch_lifecycle(alias, bucket, true).await
}

/// Like [`get_lifecycle`], but a missing configuration is the server's error (mc `ilm rule ls`
/// / `export`).
pub async fn get_lifecycle_required(alias: &AliasConfig, bucket: &str) -> Result<LifecycleInfo> {
    Ok(fetch_lifecycle(alias, bucket, false)
        .await?
        .unwrap_or_default())
}

async fn fetch_lifecycle(
    alias: &AliasConfig,
    bucket: &str,
    missing_ok: bool,
) -> Result<Option<LifecycleInfo>> {
    let client = build_client(alias).await?;
    let capture = Capture::default();
    match client
        .get_bucket_lifecycle_configuration()
        .bucket(bucket)
        .customize()
        .config_override(capture.config())
        // minio-go asks MinIO for the `X-Minio-LifecycleConfig-UpdatedAt` header.
        .mutate_request(|request| {
            let uri = format!("{}&withUpdatedAt=true", request.uri());
            let _ = request.set_uri(uri);
        })
        .send()
        .await
    {
        Ok(_) => {
            let raw = capture
                .take()
                .ok_or_else(|| anyhow!("empty lifecycle response"))?;
            Ok(Some(LifecycleInfo {
                config: LifecycleConfig::from_xml(&raw.body_text())?,
                updated_at: go_time(raw.header("x-minio-lifecycleconfig-updatedat")),
            }))
        }
        Err(error) if missing_ok && has_error_code(&error, &["NoSuchLifecycleConfiguration"]) => {
            Ok(None)
        }
        Err(error) => Err(super::error::s3_error(&error, bucket, "")),
    }
}

/// Writes the configuration (raw XML); an empty configuration deletes it.
pub async fn put_lifecycle(
    alias: &AliasConfig,
    bucket: &str,
    config: &LifecycleConfig,
) -> Result<()> {
    let client = build_client(alias).await?;
    if config.rules.is_empty() {
        client
            .delete_bucket_lifecycle()
            .bucket(bucket)
            .send()
            .await
            .s3(bucket, "")?;
        return Ok(());
    }
    client
        .put_bucket_lifecycle_configuration()
        .bucket(bucket)
        .lifecycle_configuration(
            BucketLifecycleConfiguration::builder()
                .set_rules(Some(Vec::new()))
                .build()?,
        )
        .customize()
        .config_override(raw_body_override(config.to_xml()))
        .send()
        .await
        .s3(bucket, "")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updated_at_header_becomes_go_time() {
        assert_eq!(go_time(Some("20260926T163542Z")), "2026-09-26T16:35:42Z");
        assert_eq!(go_time(None), "0001-01-01T00:00:00Z");
        assert_eq!(go_time(Some("garbage")), "0001-01-01T00:00:00Z");
    }

    fn sample() -> LifecycleConfig {
        LifecycleConfig {
            rules: vec![
                LifecycleRule {
                    id: "r1".into(),
                    status: "Enabled".into(),
                    expiration: Expiration {
                        days: 30,
                        expired_object_all_versions: true,
                        ..Default::default()
                    },
                    filter: Filter {
                        and: And {
                            prefix: "logs/".into(),
                            tags: vec![Tag {
                                key: "a".into(),
                                value: "1&2".into(),
                            }],
                            object_size_less_than: 1024,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    noncurrent_version_expiration: NoncurrentVersionExpiration {
                        noncurrent_days: 7,
                        newer_noncurrent_versions: 3,
                    },
                    ..Default::default()
                },
                LifecycleRule {
                    id: "r2".into(),
                    status: "Disabled".into(),
                    transition: Transition {
                        storage_class: "WARM".into(),
                        days: 10,
                        ..Default::default()
                    },
                    noncurrent_version_transition: NoncurrentVersionTransition {
                        storage_class: "WARM".into(),
                        noncurrent_days: 5,
                        ..Default::default()
                    },
                    expiration: Expiration {
                        expired_object_delete_marker: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn xml_round_trips() {
        let config = sample();
        let xml = config.to_xml();
        assert!(xml.contains("<ExpiredObjectAllVersions>true</ExpiredObjectAllVersions>"));
        assert!(xml.contains("<And><Prefix>logs/</Prefix><Tag><Key>a</Key><Value>1&amp;2</Value></Tag><ObjectSizeLessThan>1024</ObjectSizeLessThan></And>"));
        assert!(xml.contains("<ID>r2</ID><Filter><Prefix></Prefix></Filter>"));
        assert!(
            xml.contains(
                "<Transition><StorageClass>WARM</StorageClass><Days>10</Days></Transition>"
            )
        );
        assert_eq!(LifecycleConfig::from_xml(&xml).unwrap(), config);
    }

    #[test]
    fn filter_xml_picks_single_predicate() {
        let mut out = String::new();
        Filter {
            object_size_greater_than: 5,
            ..Default::default()
        }
        .write_xml(&mut out);
        assert_eq!(
            out,
            "<Filter><ObjectSizeGreaterThan>5</ObjectSizeGreaterThan></Filter>"
        );
        let mut out = String::new();
        Filter {
            prefix: "p/".into(),
            ..Default::default()
        }
        .write_xml(&mut out);
        assert_eq!(out, "<Filter><Prefix>p/</Prefix></Filter>");
    }

    #[test]
    fn json_matches_minio_go_shape() {
        let json = serde_json::to_value(sample()).unwrap();
        let rule = &json["Rules"][0];
        assert_eq!(rule["ID"], "r1");
        assert_eq!(rule["Expiration"]["Days"], 30);
        assert_eq!(rule["Expiration"]["ExpiredObjectAllVersions"], true);
        assert!(
            rule["Expiration"]
                .get("ExpiredObjectDeleteMarker")
                .is_none()
        );
        assert_eq!(rule["Filter"]["And"]["Tags"][0]["Key"], "a");
        assert!(rule.get("Transition").is_none());
        assert_eq!(
            rule["NoncurrentVersionExpiration"]["NewerNoncurrentVersions"],
            3
        );
        let rule2 = &json["Rules"][1];
        assert!(rule2.get("Filter").is_none());
        assert_eq!(rule2["Transition"]["StorageClass"], "WARM");
        assert_eq!(rule2["NoncurrentVersionTransition"]["NoncurrentDays"], 5);
        let parsed: LifecycleConfig = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, sample());
    }

    #[test]
    fn parses_server_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<LifecycleConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Rule><ID>x</ID><Status>Enabled</Status><Filter><Tag><Key>k</Key><Value>v</Value></Tag></Filter><Expiration><Days>3</Days></Expiration><DelMarkerExpiration><Days>2</Days></DelMarkerExpiration></Rule></LifecycleConfiguration>"#;
        let config = LifecycleConfig::from_xml(xml).unwrap();
        let rule = &config.rules[0];
        assert_eq!(rule.filter.tag.key, "k");
        assert_eq!(rule.expiration.days, 3);
        assert_eq!(rule.del_marker_expiration.days, 2);
        assert!(LifecycleConfig::from_xml("<Nope/>").is_err());
    }
}
