//! Bucket replication helpers (area H, `mx replicate`).
//!
//! The configuration model mirrors minio-go `pkg/replication` (JSON field names match mc's
//! `replicate export`/`import`, XML includes MinIO's `DeleteReplication` extension). Remote
//! targets are managed with the MinIO admin API (`set-remote-target`, ...).

use super::admin::{
    AdminClient, GO_ZERO_TIME, check_s3_status, encrypt_data, error_code, go_f32, go_f64,
};
use anyhow::{Result, anyhow, bail};
use aws_smithy_xml::decode::{Document, ScopedDecoder, try_data};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Configuration model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReplicationConfig {
    /// Go nil slice: `null` when empty.
    #[serde(
        rename = "Rules",
        default,
        serialize_with = "null_if_empty",
        deserialize_with = "vec_or_null"
    )]
    pub rules: Vec<Rule>,
    #[serde(rename = "Role", default)]
    pub role: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(rename = "ID", default)]
    pub id: String,
    #[serde(rename = "Status", default)]
    pub status: String,
    #[serde(rename = "Priority", default)]
    pub priority: i64,
    #[serde(rename = "DeleteMarkerReplication", default)]
    pub delete_marker_replication: StatusField,
    #[serde(rename = "DeleteReplication", default)]
    pub delete_replication: StatusField,
    #[serde(rename = "Destination", default)]
    pub destination: Destination,
    #[serde(rename = "Filter", default)]
    pub filter: Filter,
    #[serde(rename = "SourceSelectionCriteria", default)]
    pub source_selection_criteria: SourceSelectionCriteria,
    #[serde(rename = "ExistingObjectReplication", default)]
    pub existing_object_replication: StatusField,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StatusField {
    #[serde(rename = "Status", default)]
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Destination {
    #[serde(rename = "Bucket", default)]
    pub bucket: String,
    #[serde(
        rename = "StorageClass",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub storage_class: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    #[serde(rename = "Prefix", default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(rename = "And", default)]
    pub and: And,
    #[serde(rename = "Tag", default)]
    pub tag: Tag,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct And {
    #[serde(rename = "Prefix", default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(rename = "Tag", default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    #[serde(rename = "Key", default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    #[serde(rename = "Value", default, skip_serializing_if = "String::is_empty")]
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceSelectionCriteria {
    #[serde(rename = "ReplicaModifications", default)]
    pub replica_modifications: StatusField,
}

/// Serializes an empty Vec as `null` (Go nil slice).
fn null_if_empty<T: Serialize, S: Serializer>(
    items: &[T],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    if items.is_empty() {
        serializer.serialize_none()
    } else {
        items.serialize(serializer)
    }
}

/// Deserializes `null` as an empty Vec.
fn vec_or_null<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

pub const ENABLED: &str = "Enabled";
pub const DISABLED: &str = "Disabled";

fn status(enabled: bool) -> String {
    if enabled { ENABLED } else { DISABLED }.to_string()
}

impl Rule {
    pub fn prefix(&self) -> &str {
        if self.filter.prefix.is_empty() {
            &self.filter.and.prefix
        } else {
            &self.filter.prefix
        }
    }

    fn tag_list(&self) -> Vec<Tag> {
        if self.filter.and.tags.is_empty() {
            vec![self.filter.tag.clone()]
        } else {
            self.filter.and.tags.clone()
        }
    }

    /// Tags as `k1=v1&k2=v2`.
    pub fn tags(&self) -> String {
        self.tag_list()
            .iter()
            .filter(|t| !t.key.is_empty())
            .map(|t| format!("{}={}", t.key, t.value))
            .collect::<Vec<_>>()
            .join("&")
    }
}

/// Parses `k1=v1&k2=v2` (minio-go `Options.Tags`).
pub fn parse_tag_string(input: &str) -> Result<Vec<Tag>> {
    let mut tags = Vec::new();
    for token in input.split('&') {
        if token.is_empty() {
            break;
        }
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| anyhow!("tags should be entered as `k1=v1&k2=v2` pairs"))?;
        if key.is_empty() || key.chars().count() > 128 {
            bail!("invalid Tag Key");
        }
        if value.chars().count() > 256 {
            bail!("invalid Tag Value");
        }
        tags.push(Tag {
            key: key.to_string(),
            value: value.to_string(),
        });
    }
    Ok(tags)
}

fn make_filter(prefix: &str, tags: Vec<Tag>) -> Filter {
    if tags.len() > 1 || !prefix.is_empty() {
        return Filter {
            and: And {
                prefix: prefix.to_string(),
                tags: tags.into_iter().filter(|t| !t.key.is_empty()).collect(),
            },
            ..Default::default()
        };
    }
    Filter {
        tag: tags.into_iter().next().unwrap_or_default(),
        ..Default::default()
    }
}

/// Rule settings for [`add_rule`] / [`edit_rule`]; `None` keeps the current value on edit.
#[derive(Debug, Clone, Default)]
pub struct RuleOptions {
    pub id: String,
    pub prefix: String,
    pub enabled: Option<bool>,
    pub priority: Option<i64>,
    pub tags: Option<String>,
    pub storage_class: Option<String>,
    /// Destination ARN.
    pub dest_bucket: String,
    pub delete_markers: Option<bool>,
    pub deletes: Option<bool>,
    pub replica_sync: Option<bool>,
    pub existing_objects: Option<bool>,
}

/// minio-go `Config.AddRule`. Returns the rule ID (generated when empty).
pub fn add_rule(config: &mut ReplicationConfig, opts: &RuleOptions) -> Result<String> {
    if opts.dest_bucket.split(':').count() != 6 {
        bail!("destination bucket needs to be in Arn format");
    }
    let tags = parse_tag_string(opts.tags.as_deref().unwrap_or_default())?;
    let id = if opts.id.is_empty() {
        new_rule_id()
    } else {
        opts.id.clone()
    };
    if id.len() > 255 {
        bail!("ID must be less than 255 characters");
    }
    let priority = opts.priority.unwrap_or(0);
    let enabled = opts.enabled.unwrap_or(true);
    if priority < 0 && enabled {
        bail!("priority must be set for the rule");
    }
    let rule = Rule {
        id: id.clone(),
        status: status(enabled),
        priority,
        delete_marker_replication: StatusField {
            status: status(opts.delete_markers.unwrap_or(false)),
        },
        delete_replication: StatusField {
            status: status(opts.deletes.unwrap_or(false)),
        },
        destination: Destination {
            bucket: opts.dest_bucket.clone(),
            storage_class: opts.storage_class.clone().unwrap_or_default(),
        },
        filter: make_filter(&opts.prefix, tags),
        source_selection_criteria: SourceSelectionCriteria {
            replica_modifications: StatusField {
                status: status(opts.replica_sync.unwrap_or(true)),
            },
        },
        existing_object_replication: StatusField {
            status: status(opts.existing_objects.unwrap_or(false)),
        },
    };
    migrate_role(config);
    for existing in &config.rules {
        if existing.priority == rule.priority {
            bail!(
                "priority must be unique. Replication configuration already has a rule with this priority"
            );
        }
        if existing.id == rule.id {
            bail!("a rule exists with this ID");
        }
    }
    config.rules.push(rule);
    Ok(id)
}

/// Legacy single-target configs keep the ARN in `Role`; move it into every rule.
fn migrate_role(config: &mut ReplicationConfig) {
    if !config.role.is_empty() && !config.role.starts_with("arn:aws:iam") {
        for rule in &mut config.rules {
            rule.destination.bucket = config.role.clone();
        }
        config.role.clear();
    }
}

/// minio-go `Config.EditRule`.
pub fn edit_rule(config: &mut ReplicationConfig, opts: &RuleOptions) -> Result<()> {
    if opts.id.is_empty() {
        bail!("rule ID missing");
    }
    if config.rules.len() > 1 {
        migrate_role(config);
    }
    let index = config
        .rules
        .iter()
        .position(|r| r.id == opts.id)
        .ok_or_else(|| {
            anyhow!(
                "rule with ID {} not found in replication configuration",
                opts.id
            )
        })?;
    let mut rule = config.rules[index].clone();
    if opts.tags.is_some() || opts.prefix != rule.prefix() {
        let tags = match &opts.tags {
            Some(tags) => parse_tag_string(tags)?,
            None => rule.tag_list(),
        };
        rule.filter = make_filter(&opts.prefix, tags);
    }
    if let Some(enabled) = opts.enabled {
        rule.status = status(enabled);
    }
    if let Some(value) = opts.delete_markers {
        rule.delete_marker_replication.status = status(value);
    }
    if let Some(value) = opts.deletes {
        rule.delete_replication.status = status(value);
    }
    if let Some(value) = opts.replica_sync {
        rule.source_selection_criteria.replica_modifications.status = status(value);
    }
    if let Some(value) = opts.existing_objects {
        rule.existing_object_replication.status = status(value);
    }
    if let Some(storage_class) = &opts.storage_class {
        rule.destination.storage_class = storage_class.clone();
    }
    if let Some(priority) = opts.priority {
        rule.priority = priority;
    }
    if !opts.dest_bucket.is_empty() {
        if opts.dest_bucket.split(':').count() != 6 {
            bail!("destination bucket needs to be in Arn format");
        }
        rule.destination.bucket = opts.dest_bucket.clone();
    }
    for (i, existing) in config.rules.iter().enumerate() {
        if i != index && existing.priority == rule.priority {
            bail!(
                "priority must be unique. Replication configuration already has a rule with this priority"
            );
        }
    }
    config.rules[index] = rule;
    Ok(())
}

/// minio-go `Config.RemoveRule`. Returns the removed rule's destination ARN.
pub fn remove_rule(config: &mut ReplicationConfig, id: &str) -> Result<String> {
    let index = config
        .rules
        .iter()
        .position(|r| r.id == id)
        .ok_or_else(|| anyhow!("Rule with ID {id} not found"))?;
    if config.rules.len() == 1 {
        bail!(
            "replication configuration should have at least one rule (use `--all --force` to remove the whole configuration)"
        );
    }
    Ok(config.rules.remove(index).destination.bucket)
}

/// An xid-like 20 character rule ID (minio-go uses `xid.New()`).
fn new_rule_id() -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuv";
    let mut bytes = [0u8; 20];
    let _ = aws_lc_rs::rand::fill(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[(*b & 31) as usize] as char)
        .collect()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn el(name: &str, value: &str) -> String {
    format!("<{name}>{}</{name}>", escape(value))
}

fn status_el(name: &str, field: &StatusField) -> String {
    format!("<{name}>{}</{name}>", el("Status", &field.status))
}

fn tag_el(tag: &Tag) -> String {
    format!(
        "<Tag>{}{}</Tag>",
        el("Key", &tag.key),
        el("Value", &tag.value)
    )
}

/// Serializes the configuration as PutBucketReplication XML.
pub fn to_xml(config: &ReplicationConfig) -> String {
    let mut xml = String::from(
        r#"<ReplicationConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/">"#,
    );
    if !config.role.is_empty() {
        xml.push_str(&el("Role", &config.role));
    }
    for rule in &config.rules {
        xml.push_str("<Rule>");
        if !rule.id.is_empty() {
            xml.push_str(&el("ID", &rule.id));
        }
        xml.push_str(&el("Status", &rule.status));
        xml.push_str(&el("Priority", &rule.priority.to_string()));
        xml.push_str(&status_el(
            "DeleteMarkerReplication",
            &rule.delete_marker_replication,
        ));
        xml.push_str(&status_el("DeleteReplication", &rule.delete_replication));
        xml.push_str("<Destination>");
        xml.push_str(&el("Bucket", &rule.destination.bucket));
        if !rule.destination.storage_class.is_empty() {
            xml.push_str(&el("StorageClass", &rule.destination.storage_class));
        }
        xml.push_str("</Destination><Filter>");
        let filter = &rule.filter;
        if !filter.and.prefix.is_empty() || !filter.and.tags.is_empty() {
            xml.push_str("<And>");
            if !filter.and.prefix.is_empty() {
                xml.push_str(&el("Prefix", &filter.and.prefix));
            }
            for tag in &filter.and.tags {
                xml.push_str(&tag_el(tag));
            }
            xml.push_str("</And>");
        } else if !filter.tag.key.is_empty() {
            xml.push_str(&tag_el(&filter.tag));
        } else {
            xml.push_str(&el("Prefix", &filter.prefix));
        }
        xml.push_str("</Filter><SourceSelectionCriteria>");
        xml.push_str(&status_el(
            "ReplicaModifications",
            &rule.source_selection_criteria.replica_modifications,
        ));
        xml.push_str("</SourceSelectionCriteria>");
        if !rule.existing_object_replication.status.is_empty() {
            xml.push_str(&status_el(
                "ExistingObjectReplication",
                &rule.existing_object_replication,
            ));
        }
        xml.push_str("</Rule>");
    }
    xml.push_str("</ReplicationConfiguration>");
    xml
}

type XmlResult<T> = std::result::Result<T, aws_smithy_xml::decode::XmlDecodeError>;

fn read_status(tag: &mut ScopedDecoder) -> XmlResult<StatusField> {
    let mut field = StatusField::default();
    while let Some(mut child) = tag.next_tag() {
        if child.start_el().local() == "Status" {
            field.status = try_data(&mut child)?.to_string();
        }
    }
    Ok(field)
}

fn read_tag(tag: &mut ScopedDecoder) -> XmlResult<Tag> {
    let mut out = Tag::default();
    while let Some(mut child) = tag.next_tag() {
        match child.start_el().local() {
            "Key" => out.key = try_data(&mut child)?.to_string(),
            "Value" => out.value = try_data(&mut child)?.to_string(),
            _ => {}
        }
    }
    Ok(out)
}

fn read_rule(tag: &mut ScopedDecoder) -> XmlResult<Rule> {
    let mut rule = Rule::default();
    while let Some(mut child) = tag.next_tag() {
        match child.start_el().local() {
            "ID" => rule.id = try_data(&mut child)?.to_string(),
            "Status" => rule.status = try_data(&mut child)?.to_string(),
            "Priority" => {
                rule.priority = try_data(&mut child)?.trim().parse().unwrap_or_default();
            }
            "DeleteMarkerReplication" => {
                rule.delete_marker_replication = read_status(&mut child)?;
            }
            "DeleteReplication" => rule.delete_replication = read_status(&mut child)?,
            "ExistingObjectReplication" => {
                rule.existing_object_replication = read_status(&mut child)?;
            }
            "Destination" => {
                while let Some(mut field) = child.next_tag() {
                    match field.start_el().local() {
                        "Bucket" => rule.destination.bucket = try_data(&mut field)?.to_string(),
                        "StorageClass" => {
                            rule.destination.storage_class = try_data(&mut field)?.to_string();
                        }
                        _ => {}
                    }
                }
            }
            "SourceSelectionCriteria" => {
                while let Some(mut field) = child.next_tag() {
                    if field.start_el().local() == "ReplicaModifications" {
                        rule.source_selection_criteria.replica_modifications =
                            read_status(&mut field)?;
                    }
                }
            }
            "Filter" => {
                while let Some(mut field) = child.next_tag() {
                    match field.start_el().local() {
                        "Prefix" => rule.filter.prefix = try_data(&mut field)?.to_string(),
                        "Tag" => rule.filter.tag = read_tag(&mut field)?,
                        "And" => {
                            while let Some(mut inner) = field.next_tag() {
                                match inner.start_el().local() {
                                    "Prefix" => {
                                        rule.filter.and.prefix = try_data(&mut inner)?.to_string();
                                    }
                                    "Tag" => rule.filter.and.tags.push(read_tag(&mut inner)?),
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(rule)
}

/// Parses GetBucketReplication XML.
pub fn from_xml(text: &str) -> Result<ReplicationConfig> {
    let parse = || -> XmlResult<ReplicationConfig> {
        let mut doc = Document::new(text);
        let mut root = doc.root_element()?;
        let mut config = ReplicationConfig::default();
        while let Some(mut child) = root.next_tag() {
            match child.start_el().local() {
                "Role" => config.role = try_data(&mut child)?.to_string(),
                "Rule" => config.rules.push(read_rule(&mut child)?),
                _ => {}
            }
        }
        Ok(config)
    };
    parse().map_err(|err| anyhow!("invalid replication configuration XML: {err}"))
}

// ---------------------------------------------------------------------------
// Remote targets (admin API)
// ---------------------------------------------------------------------------

fn zero_time() -> String {
    GO_ZERO_TIME.to_string()
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// madmin `Credentials` of a remote target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetCredentials {
    #[serde(
        rename = "accessKey",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub access_key: String,
    #[serde(
        rename = "secretKey",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub secret_key: String,
    #[serde(
        rename = "sessionToken",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub session_token: String,
    #[serde(default = "zero_time")]
    pub expiration: String,
}

impl Default for TargetCredentials {
    fn default() -> Self {
        Self {
            access_key: String::new(),
            secret_key: String::new(),
            session_token: String::new(),
            expiration: zero_time(),
        }
    }
}

/// madmin `LatencyStat` (nanoseconds).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LatencyStat {
    pub curr: i64,
    pub avg: i64,
    pub max: i64,
}

/// `madmin.BucketTarget` with madmin's field order; times are kept as Go-formatted strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BucketTarget {
    #[serde(rename = "sourcebucket")]
    pub source_bucket: String,
    pub endpoint: String,
    pub credentials: Option<TargetCredentials>,
    #[serde(rename = "targetbucket")]
    pub target_bucket: String,
    pub secure: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub arn: String,
    #[serde(rename = "type")]
    pub target_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub region: String,
    #[serde(rename = "bandwidthlimit", skip_serializing_if = "is_zero")]
    pub bandwidth_limit: i64,
    #[serde(rename = "replicationSync")]
    pub replication_sync: bool,
    #[serde(rename = "storageclass", skip_serializing_if = "String::is_empty")]
    pub storage_class: String,
    /// Nanoseconds (Go `time.Duration`).
    #[serde(rename = "healthCheckDuration", skip_serializing_if = "is_zero")]
    pub health_check_duration: i64,
    #[serde(rename = "disableProxy")]
    pub disable_proxy: bool,
    #[serde(rename = "resetBeforeDate")]
    pub reset_before_date: String,
    #[serde(rename = "resetID", skip_serializing_if = "String::is_empty")]
    pub reset_id: String,
    /// Nanoseconds.
    #[serde(rename = "totalDowntime")]
    pub total_downtime: i64,
    #[serde(rename = "lastOnline")]
    pub last_online: String,
    #[serde(rename = "isOnline")]
    pub online: bool,
    pub latency: LatencyStat,
    #[serde(rename = "deploymentID", skip_serializing_if = "String::is_empty")]
    pub deployment_id: String,
    pub edge: bool,
    #[serde(rename = "edgeSyncBeforeExpiry")]
    pub edge_sync_before_expiry: bool,
    #[serde(rename = "offlineCount")]
    pub offline_count: i64,
}

impl Default for BucketTarget {
    fn default() -> Self {
        Self {
            source_bucket: String::new(),
            endpoint: String::new(),
            credentials: None,
            target_bucket: String::new(),
            secure: false,
            path: String::new(),
            api: String::new(),
            arn: String::new(),
            target_type: String::new(),
            region: String::new(),
            bandwidth_limit: 0,
            replication_sync: false,
            storage_class: String::new(),
            health_check_duration: 0,
            disable_proxy: false,
            reset_before_date: zero_time(),
            reset_id: String::new(),
            total_downtime: 0,
            last_online: zero_time(),
            online: false,
            latency: LatencyStat::default(),
            deployment_id: String::new(),
            edge: false,
            edge_sync_before_expiry: false,
            offline_count: 0,
        }
    }
}

pub async fn set_remote_target(
    client: &AdminClient,
    bucket: &str,
    target: &BucketTarget,
) -> Result<String> {
    let body = encrypt_data(client.secret_key(), &serde_json::to_vec(target)?)?;
    let response = client
        .admin("PUT", "set-remote-target", &[("bucket", bucket)], body)
        .await?;
    Ok(serde_json::from_slice(&response.body)?)
}

/// `UpdateRemoteTarget`; `ops` are query flags like `creds`, `sync`, `proxy`, `bandwidth`,
/// `healthcheck`, `path`.
pub async fn update_remote_target(
    client: &AdminClient,
    target: &BucketTarget,
    ops: &[&str],
) -> Result<String> {
    let body = encrypt_data(client.secret_key(), &serde_json::to_vec(target)?)?;
    let mut query = vec![
        ("bucket", target.source_bucket.as_str()),
        ("update", "true"),
    ];
    query.extend(ops.iter().map(|op| (*op, "true")));
    let response = client
        .admin("PUT", "set-remote-target", &query, body)
        .await?;
    Ok(serde_json::from_slice(&response.body)?)
}

pub async fn list_remote_targets(client: &AdminClient, bucket: &str) -> Result<Vec<BucketTarget>> {
    let response = client
        .admin(
            "GET",
            "list-remote-targets",
            &[("bucket", bucket), ("type", "")],
            Vec::new(),
        )
        .await?;
    let targets: Option<Vec<BucketTarget>> = serde_json::from_slice(&response.body)?;
    Ok(targets.unwrap_or_default())
}

pub async fn remove_remote_target(client: &AdminClient, bucket: &str, arn: &str) -> Result<()> {
    client
        .admin(
            "DELETE",
            "remove-remote-target",
            &[("bucket", bucket), ("arn", arn)],
            Vec::new(),
        )
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Bucket sub-resources
// ---------------------------------------------------------------------------

/// GetBucketReplication; an absent configuration yields an empty config.
pub async fn get_replication(client: &AdminClient, bucket: &str) -> Result<ReplicationConfig> {
    let response = client
        .bucket_raw("GET", bucket, &[("replication", "")], Vec::new())
        .await?;
    if response.status == 404
        && error_code(&response).as_deref() == Some("ReplicationConfigurationNotFoundError")
    {
        return Ok(ReplicationConfig::default());
    }
    from_xml(&check_s3_status(response, bucket)?.text())
}

/// PutBucketReplication (an empty configuration deletes it, like minio-go).
pub async fn put_replication(
    client: &AdminClient,
    bucket: &str,
    config: &ReplicationConfig,
) -> Result<()> {
    if config.rules.is_empty() {
        return delete_replication(client, bucket).await;
    }
    client
        .bucket(
            "PUT",
            bucket,
            &[("replication", "")],
            to_xml(config).into_bytes(),
        )
        .await?;
    Ok(())
}

pub async fn delete_replication(client: &AdminClient, bucket: &str) -> Result<()> {
    client
        .bucket("DELETE", bucket, &[("replication", "")], Vec::new())
        .await?;
    Ok(())
}

/// `GetBucketReplicationMetricsV2`.
pub async fn replication_metrics(client: &AdminClient, bucket: &str) -> Result<MetricsV2> {
    let response = client
        .bucket("GET", bucket, &[("replication-metrics", "2")], Vec::new())
        .await?;
    Ok(serde_json::from_slice(&response.body)?)
}

/// Starts a resync (`ResetBucketReplicationOnTarget`); returns `ResyncTargetsInfo` JSON.
pub async fn resync_start(
    client: &AdminClient,
    bucket: &str,
    arn: &str,
    older_than: Option<std::time::Duration>,
) -> Result<serde_json::Value> {
    let older = older_than.map(go_duration);
    let reset_id = new_uuid();
    let mut query = vec![("replication-reset", "")];
    if let Some(older) = older.as_deref() {
        query.push(("older-than", older));
    }
    query.push(("arn", arn));
    query.push(("reset-id", &reset_id));
    let response = client.bucket("PUT", bucket, &query, Vec::new()).await?;
    Ok(serde_json::from_slice(&response.body)?)
}

pub async fn resync_status(
    client: &AdminClient,
    bucket: &str,
    arn: &str,
) -> Result<serde_json::Value> {
    let mut query = vec![("replication-reset-status", "")];
    if !arn.is_empty() {
        query.push(("arn", arn));
    }
    let response = client.bucket("GET", bucket, &query, Vec::new()).await?;
    Ok(serde_json::from_slice(&response.body)?)
}

/// Cancels a running resync; returns the reset ID reported by the server.
pub async fn resync_cancel(client: &AdminClient, bucket: &str, arn: &str) -> Result<String> {
    let mut query = vec![("replication-reset-cancel", "")];
    if !arn.is_empty() {
        query.push(("arn", arn));
    }
    let response = client.bucket("PUT", bucket, &query, Vec::new()).await?;
    Ok(response.text())
}

/// Admin `replication/mrf`: recent replication failures (stream of JSON objects).
pub async fn replication_mrf(
    client: &AdminClient,
    bucket: &str,
    node: &str,
) -> Result<Vec<ReplicationMrf>> {
    let mut query = vec![("bucket", bucket)];
    if !node.is_empty() {
        query.push(("node", node));
    }
    let response = client
        .admin("GET", "replication/mrf", &query, Vec::new())
        .await?;
    json_stream(&response.body)
}

/// Admin `replication/diff`: unreplicated object versions (stream of JSON objects).
pub async fn replication_diff(
    client: &AdminClient,
    bucket: &str,
    prefix: &str,
    arn: &str,
    verbose: bool,
) -> Result<Vec<DiffInfo>> {
    let mut query = vec![("bucket", bucket)];
    if verbose {
        query.push(("verbose", "true"));
    }
    if !arn.is_empty() {
        query.push(("arn", arn));
    }
    if !prefix.is_empty() {
        query.push(("prefix", prefix));
    }
    let response = client
        .admin("POST", "replication/diff", &query, Vec::new())
        .await?;
    json_stream(&response.body)
}

fn json_stream<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<Vec<T>> {
    serde_json::Deserializer::from_slice(body)
        .into_iter::<T>()
        .map(|item| item.map_err(Into::into))
        .collect()
}

/// madmin `ReplicationMRF`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplicationMrf {
    #[serde(rename = "nodeName")]
    pub node_name: String,
    pub bucket: String,
    pub object: String,
    #[serde(rename = "versionId")]
    pub version_id: String,
    #[serde(rename = "retryCount")]
    pub retry_count: i64,
    #[serde(rename = "error", skip_serializing_if = "String::is_empty")]
    pub err: String,
}

/// madmin `TgtDiffInfo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TgtDiffInfo {
    #[serde(rename = "rStatus", skip_serializing_if = "String::is_empty")]
    pub replication_status: String,
    #[serde(rename = "drStatus", skip_serializing_if = "String::is_empty")]
    pub delete_replication_status: String,
}

/// madmin `DiffInfo` (`error` is the Go-marshaled error value).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffInfo {
    pub object: String,
    #[serde(rename = "versionId")]
    pub version_id: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, TgtDiffInfo>,
    /// Go-marshaled request error (madmin sends failures as one `DiffInfo`).
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub error: Option<crate::error::Detail>,
    #[serde(rename = "rStatus", skip_serializing_if = "String::is_empty")]
    pub replication_status: String,
    #[serde(rename = "dStatus", skip_serializing_if = "String::is_empty")]
    pub delete_replication_status: String,
    #[serde(rename = "replTimestamp")]
    pub replication_timestamp: String,
    #[serde(rename = "lastModified")]
    pub last_modified: String,
    #[serde(rename = "deletemarker")]
    pub is_delete_marker: bool,
}

impl Default for DiffInfo {
    fn default() -> Self {
        Self {
            object: String::new(),
            version_id: String::new(),
            targets: BTreeMap::new(),
            error: None,
            replication_status: String::new(),
            delete_replication_status: String::new(),
            replication_timestamp: zero_time(),
            last_modified: zero_time(),
            is_delete_marker: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Replication metrics (minio-go `replication.MetricsV2`, re-marshaled like mc)
// ---------------------------------------------------------------------------

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

/// minio-go `RStat`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RStat {
    #[serde(serialize_with = "go_f64")]
    pub count: f64,
    pub bytes: i64,
}

/// minio-go `TimedErrStats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimedErrStats {
    #[serde(rename = "lastMinute")]
    pub last_minute: RStat,
    #[serde(rename = "lastHour")]
    pub last_hour: RStat,
    pub totals: RStat,
}

/// minio-go `TargetMetrics`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetMetrics {
    #[serde(rename = "replicationCount", skip_serializing_if = "is_zero_u64")]
    pub replicated_count: u64,
    #[serde(
        rename = "completedReplicationSize",
        skip_serializing_if = "is_zero_u64"
    )]
    pub replicated_size: u64,
    #[serde(rename = "limitInBits", skip_serializing_if = "is_zero")]
    pub bandwidth_limit: i64,
    #[serde(
        rename = "currentBandwidth",
        skip_serializing_if = "is_zero_f64",
        serialize_with = "go_f64"
    )]
    pub current_bandwidth: f64,
    pub failed: TimedErrStats,
    #[serde(rename = "pendingReplicationSize", skip_serializing_if = "is_zero_u64")]
    pub pending_size: u64,
    #[serde(rename = "replicaSize", skip_serializing_if = "is_zero_u64")]
    pub replica_size: u64,
    #[serde(rename = "failedReplicationSize", skip_serializing_if = "is_zero_u64")]
    pub failed_size: u64,
    #[serde(
        rename = "pendingReplicationCount",
        skip_serializing_if = "is_zero_u64"
    )]
    pub pending_count: u64,
    #[serde(rename = "failedReplicationCount", skip_serializing_if = "is_zero_u64")]
    pub failed_count: u64,
}

/// minio-go `QStat`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QStat {
    #[serde(serialize_with = "go_f64")]
    pub count: f64,
    #[serde(serialize_with = "go_f64")]
    pub bytes: f64,
}

/// minio-go `InQueueMetric` (its `peak` field never matches the server's `max`, so it is
/// always zero in mc).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InQueueMetric {
    pub curr: QStat,
    pub avg: QStat,
    pub peak: QStat,
}

/// minio-go `Metrics`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Metrics {
    #[serde(rename = "Stats")]
    pub stats: Option<BTreeMap<String, TargetMetrics>>,
    #[serde(
        rename = "completedReplicationSize",
        skip_serializing_if = "is_zero_u64"
    )]
    pub replicated_size: u64,
    #[serde(rename = "replicaSize", skip_serializing_if = "is_zero_u64")]
    pub replica_size: u64,
    #[serde(rename = "replicaCount", skip_serializing_if = "is_zero")]
    pub replica_count: i64,
    #[serde(rename = "replicationCount", skip_serializing_if = "is_zero")]
    pub replicated_count: i64,
    #[serde(rename = "failed")]
    pub errors: TimedErrStats,
    #[serde(rename = "queued")]
    pub qstats: InQueueMetric,
    #[serde(rename = "pendingReplicationSize", skip_serializing_if = "is_zero_u64")]
    pub pending_size: u64,
    #[serde(rename = "failedReplicationSize", skip_serializing_if = "is_zero_u64")]
    pub failed_size: u64,
    #[serde(
        rename = "pendingReplicationCount",
        skip_serializing_if = "is_zero_u64"
    )]
    pub pending_count: u64,
    #[serde(rename = "failedReplicationCount", skip_serializing_if = "is_zero_u64")]
    pub failed_count: u64,
}

/// minio-go `WorkerStat`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerStat {
    pub curr: i32,
    #[serde(serialize_with = "go_f32")]
    pub avg: f32,
    pub max: i32,
}

/// minio-go `XferStats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct XferStats {
    #[serde(rename = "avgRate", serialize_with = "go_f64")]
    pub avg_rate: f64,
    #[serde(rename = "peakRate", serialize_with = "go_f64")]
    pub peak_rate: f64,
    #[serde(rename = "currRate", serialize_with = "go_f64")]
    pub curr_rate: f64,
}

/// minio-go `ReplMRFStats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplMrfStats {
    #[serde(rename = "failedCount_last5min")]
    pub last_failed_count: u64,
    #[serde(rename = "droppedCount_since_uptime")]
    pub total_dropped_count: u64,
    #[serde(rename = "droppedBytes_since_uptime")]
    pub total_dropped_bytes: u64,
}

/// minio-go `CounterSummary`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CounterSummary {
    pub last1hr: u64,
    pub last1m: u64,
    pub total: u64,
}

/// minio-go `ReplQNodeStats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplQNodeStats {
    #[serde(rename = "nodeName")]
    pub node_name: String,
    pub uptime: i64,
    #[serde(rename = "activeWorkers")]
    pub workers: WorkerStat,
    #[serde(rename = "transferSummary")]
    pub xfer_stats: Option<BTreeMap<String, XferStats>>,
    #[serde(rename = "tgtTransferStats")]
    pub tgt_xfer_stats: Option<BTreeMap<String, BTreeMap<String, XferStats>>>,
    #[serde(rename = "queueStats")]
    pub qstats: InQueueMetric,
    #[serde(rename = "mrfStats")]
    pub mrf_stats: ReplMrfStats,
    pub retries: CounterSummary,
    pub errors: CounterSummary,
}

/// minio-go `ReplQueueStats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplQueueStats {
    pub nodes: Option<Vec<ReplQNodeStats>>,
}

/// minio-go `Stat`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stat {
    pub total: i64,
    pub avg: i64,
    pub max: i64,
}

/// minio-go `DowntimeInfo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DowntimeInfo {
    pub duration: Stat,
    pub count: Stat,
}

/// minio-go `MetricsV2`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsV2 {
    pub uptime: i64,
    #[serde(rename = "currStats")]
    pub current_stats: Metrics,
    #[serde(rename = "queueStats")]
    pub queue_stats: ReplQueueStats,
    #[serde(rename = "downtimeInfo")]
    pub downtime_info: Option<BTreeMap<String, DowntimeInfo>>,
}

/// Cluster-wide queue summary (minio-go `ReplQueueStats.QStats()`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplQStats {
    pub workers: WorkerStat,
    pub xfer_stats: BTreeMap<String, XferStats>,
    pub tgt_xfer_stats: BTreeMap<String, BTreeMap<String, XferStats>>,
}

impl ReplQueueStats {
    fn nodes(&self) -> &[ReplQNodeStats] {
        self.nodes.as_deref().unwrap_or_default()
    }

    /// minio-go `Workers()`: summed current/average workers averaged over nodes, max of max.
    pub fn workers(&self) -> WorkerStat {
        let mut total = WorkerStat::default();
        for node in self.nodes() {
            total.avg += node.workers.avg;
            total.curr += node.workers.curr;
            total.max = total.max.max(node.workers.max);
        }
        let count = self.nodes().len();
        if count > 0 {
            total.avg /= count as f32;
            total.curr /= count as i32;
        }
        total
    }

    /// minio-go `QStats()` (the transfer part used by `replicate status`).
    pub fn qstats(&self) -> ReplQStats {
        let mut summary = ReplQStats {
            workers: self.workers(),
            ..Default::default()
        };
        let merge = |st: &mut XferStats, v: &XferStats| {
            st.avg_rate += v.avg_rate;
            st.curr_rate += v.curr_rate;
            st.peak_rate = st.peak_rate.max(v.peak_rate);
        };
        for node in self.nodes() {
            for (arn, xmap) in node.tgt_xfer_stats.iter().flatten() {
                for (metric, value) in xmap {
                    // minio-go starts from the cluster totals so far (without updating them).
                    let mut st = summary.xfer_stats.get(metric).cloned().unwrap_or_default();
                    merge(&mut st, value);
                    summary
                        .tgt_xfer_stats
                        .entry(arn.clone())
                        .or_default()
                        .insert(metric.clone(), st);
                }
            }
            for (metric, value) in node.xfer_stats.iter().flatten() {
                merge(summary.xfer_stats.entry(metric.clone()).or_default(), value);
            }
        }
        summary
    }
}

/// Go `time.Duration.String()` for whole seconds (`1440h0m0s`, `1m30s`, `5s`).
pub fn go_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m}m{s}s")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{s}s")
    }
}

fn new_uuid() -> String {
    let mut b = [0u8; 16];
    let _ = aws_lc_rs::rand::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARN: &str = "arn:minio:replication::abc:dest";

    fn opts() -> RuleOptions {
        RuleOptions {
            id: "r1".into(),
            priority: Some(1),
            dest_bucket: ARN.into(),
            delete_markers: Some(true),
            deletes: Some(true),
            replica_sync: Some(true),
            existing_objects: Some(true),
            ..Default::default()
        }
    }

    #[test]
    fn add_rule_builds_minio_xml() {
        let mut config = ReplicationConfig::default();
        add_rule(&mut config, &opts()).unwrap();
        assert_eq!(
            to_xml(&config),
            concat!(
                r#"<ReplicationConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/">"#,
                "<Rule><ID>r1</ID><Status>Enabled</Status><Priority>1</Priority>",
                "<DeleteMarkerReplication><Status>Enabled</Status></DeleteMarkerReplication>",
                "<DeleteReplication><Status>Enabled</Status></DeleteReplication>",
                "<Destination><Bucket>arn:minio:replication::abc:dest</Bucket></Destination>",
                "<Filter><Prefix></Prefix></Filter>",
                "<SourceSelectionCriteria><ReplicaModifications><Status>Enabled</Status></ReplicaModifications></SourceSelectionCriteria>",
                "<ExistingObjectReplication><Status>Enabled</Status></ExistingObjectReplication>",
                "</Rule></ReplicationConfiguration>"
            )
        );
    }

    #[test]
    fn add_rule_validates() {
        let mut config = ReplicationConfig::default();
        add_rule(&mut config, &opts()).unwrap();
        let err = add_rule(&mut config, &opts()).unwrap_err().to_string();
        assert!(err.contains("priority must be unique"), "{err}");
        let err = add_rule(
            &mut config,
            &RuleOptions {
                priority: Some(2),
                ..opts()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("a rule exists with this ID"), "{err}");
        let err = add_rule(
            &mut config,
            &RuleOptions {
                dest_bucket: "bucket".into(),
                ..opts()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Arn format"), "{err}");
        let id = add_rule(
            &mut config,
            &RuleOptions {
                id: String::new(),
                priority: Some(3),
                ..opts()
            },
        )
        .unwrap();
        assert_eq!(id.len(), 20);
    }

    #[test]
    fn filters_use_and_for_prefix_or_multiple_tags() {
        let mut config = ReplicationConfig::default();
        add_rule(
            &mut config,
            &RuleOptions {
                prefix: "logs/".into(),
                tags: Some("a=1&b=2".into()),
                storage_class: Some("STANDARD".into()),
                ..opts()
            },
        )
        .unwrap();
        let xml = to_xml(&config);
        assert!(xml.contains("<Filter><And><Prefix>logs/</Prefix><Tag><Key>a</Key><Value>1</Value></Tag><Tag><Key>b</Key><Value>2</Value></Tag></And></Filter>"), "{xml}");
        assert!(xml.contains("<StorageClass>STANDARD</StorageClass>"));
        assert_eq!(config.rules[0].tags(), "a=1&b=2");
        assert_eq!(config.rules[0].prefix(), "logs/");

        let single = make_filter("", parse_tag_string("k=v").unwrap());
        assert_eq!(single.tag.key, "k");
        assert!(single.and.tags.is_empty());
        assert!(parse_tag_string("novalue").is_err());
    }

    #[test]
    fn xml_round_trips() {
        let mut config = ReplicationConfig::default();
        add_rule(&mut config, &opts()).unwrap();
        add_rule(
            &mut config,
            &RuleOptions {
                id: "r2".into(),
                priority: Some(2),
                prefix: "a&b/".into(),
                tags: Some("k=v".into()),
                enabled: Some(false),
                ..opts()
            },
        )
        .unwrap();
        let parsed = from_xml(&to_xml(&config)).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn parses_server_xml_with_role() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ReplicationConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Rule><ID>x</ID><Status>Enabled</Status><Priority>0</Priority><DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication><Destination><Bucket>arn:aws:s3:::dest</Bucket></Destination><Filter><Tag><Key>k</Key><Value>v</Value></Tag></Filter></Rule><Role>arn:minio:replication::id:dest</Role></ReplicationConfiguration>"#;
        let mut config = from_xml(xml).unwrap();
        assert_eq!(config.role, "arn:minio:replication::id:dest");
        assert_eq!(config.rules[0].tags(), "k=v");
        assert_eq!(config.rules[0].delete_replication.status, "");
        add_rule(
            &mut config,
            &RuleOptions {
                id: "y".into(),
                ..opts()
            },
        )
        .unwrap();
        assert!(config.role.is_empty());
        assert_eq!(
            config.rules[0].destination.bucket,
            "arn:minio:replication::id:dest"
        );
    }

    #[test]
    fn edit_and_remove_rules() {
        let mut config = ReplicationConfig::default();
        add_rule(&mut config, &opts()).unwrap();
        add_rule(
            &mut config,
            &RuleOptions {
                id: "r2".into(),
                priority: Some(2),
                ..opts()
            },
        )
        .unwrap();
        edit_rule(
            &mut config,
            &RuleOptions {
                id: "r2".into(),
                enabled: Some(false),
                storage_class: Some("REDUCED_REDUNDANCY".into()),
                deletes: Some(false),
                tags: Some("x=y".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let rule = &config.rules[1];
        assert_eq!(rule.status, DISABLED);
        assert_eq!(rule.delete_replication.status, DISABLED);
        assert_eq!(rule.delete_marker_replication.status, ENABLED);
        assert_eq!(rule.destination.storage_class, "REDUCED_REDUNDANCY");
        assert_eq!(rule.tags(), "x=y");
        assert_eq!(rule.priority, 2);
        let err = edit_rule(
            &mut config,
            &RuleOptions {
                id: "r2".into(),
                priority: Some(1),
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("priority must be unique"));
        assert!(
            edit_rule(
                &mut config,
                &RuleOptions {
                    id: "zz".into(),
                    ..Default::default()
                }
            )
            .is_err()
        );

        assert_eq!(remove_rule(&mut config, "r1").unwrap(), ARN);
        assert!(remove_rule(&mut config, "r1").is_err());
        assert!(remove_rule(&mut config, "r2").is_err());
    }

    #[test]
    fn export_json_matches_mc_shape() {
        let mut config = ReplicationConfig::default();
        add_rule(&mut config, &opts()).unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "Rules": [{
                    "ID": "r1",
                    "Status": "Enabled",
                    "Priority": 1,
                    "DeleteMarkerReplication": {"Status": "Enabled"},
                    "DeleteReplication": {"Status": "Enabled"},
                    "Destination": {"Bucket": ARN},
                    "Filter": {"And": {}, "Tag": {}},
                    "SourceSelectionCriteria": {"ReplicaModifications": {"Status": "Enabled"}},
                    "ExistingObjectReplication": {"Status": "Enabled"}
                }],
                "Role": ""
            })
        );
        let back: ReplicationConfig = serde_json::from_value(value).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn bucket_target_json_matches_madmin() {
        let server = r#"{"sourcebucket":"src","endpoint":"h:9000","credentials":{"accessKey":"ak"},"targetbucket":"dst","secure":false,"path":"auto","api":"s3v4","arn":"arn:minio:replication::x:dst","type":"replication","replicationSync":false,"healthCheckDuration":60000000000,"disableProxy":false,"isOnline":true,"totalDowntime":0,"lastOnline":"2026-09-26T16:31:48.449423944Z","latency":{"curr":1,"avg":2,"max":3},"unknown":1}"#;
        let target: BucketTarget = serde_json::from_str(server).unwrap();
        assert!(target.online);
        assert_eq!(target.health_check_duration, 60_000_000_000);
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            r#"{"sourcebucket":"src","endpoint":"h:9000","credentials":{"accessKey":"ak","expiration":"0001-01-01T00:00:00Z"},"targetbucket":"dst","secure":false,"path":"auto","api":"s3v4","arn":"arn:minio:replication::x:dst","type":"replication","replicationSync":false,"healthCheckDuration":60000000000,"disableProxy":false,"resetBeforeDate":"0001-01-01T00:00:00Z","totalDowntime":0,"lastOnline":"2026-09-26T16:31:48.449423944Z","isOnline":true,"latency":{"curr":1,"avg":2,"max":3},"edge":false,"edgeSyncBeforeExpiry":false,"offlineCount":0}"#
        );
    }

    /// Server metrics re-marshaled through minio-go's types, like mc does (`queued.max` is
    /// dropped because minio-go names it `peak`, unknown fields vanish, zero counters are
    /// omitted, integral floats print without a fraction).
    #[test]
    fn metrics_json_matches_minio_go() {
        let server = r#"{"currStats":{"Stats":{"arn1":{"completedReplicationSize":3,"currentBandwidth":0,"failed":{"lastHour":{"bytes":0,"count":0},"lastMinute":{"bytes":0,"count":0},"totals":{"bytes":0,"count":0}},"replicationCount":1,"replicationLatency":{}}},"completedReplicationSize":3,"queued":{"avg":{"bytes":0,"count":0},"curr":{"bytes":0,"count":0},"max":{"bytes":5,"count":1}},"replicationCount":1},"proxyStats":{},"queueStats":{"nodes":[{"activeWorkers":{"avg":0,"curr":0,"max":0},"nodeName":"n1","tgtTransferStats":{"arn1":{"Small":{"avgRate":0,"currRate":1.5,"n":0,"peakRate":2}}},"transferSummary":{"Total":{"avgRate":0,"currRate":1.5,"n":0,"peakRate":0}},"uptime":165}],"uptime":165},"uptime":165}"#;
        let metrics: MetricsV2 = serde_json::from_str(server).unwrap();
        assert_eq!(
            serde_json::to_string(&metrics).unwrap(),
            r#"{"uptime":165,"currStats":{"Stats":{"arn1":{"replicationCount":1,"completedReplicationSize":3,"failed":{"lastMinute":{"count":0,"bytes":0},"lastHour":{"count":0,"bytes":0},"totals":{"count":0,"bytes":0}}}},"completedReplicationSize":3,"replicationCount":1,"failed":{"lastMinute":{"count":0,"bytes":0},"lastHour":{"count":0,"bytes":0},"totals":{"count":0,"bytes":0}},"queued":{"curr":{"count":0,"bytes":0},"avg":{"count":0,"bytes":0},"peak":{"count":0,"bytes":0}}},"queueStats":{"nodes":[{"nodeName":"n1","uptime":165,"activeWorkers":{"curr":0,"avg":0,"max":0},"transferSummary":{"Total":{"avgRate":0,"peakRate":0,"currRate":1.5}},"tgtTransferStats":{"arn1":{"Small":{"avgRate":0,"peakRate":2,"currRate":1.5}}},"queueStats":{"curr":{"count":0,"bytes":0},"avg":{"count":0,"bytes":0},"peak":{"count":0,"bytes":0}},"mrfStats":{"failedCount_last5min":0,"droppedCount_since_uptime":0,"droppedBytes_since_uptime":0},"retries":{"last1hr":0,"last1m":0,"total":0},"errors":{"last1hr":0,"last1m":0,"total":0}}]},"downtimeInfo":null}"#
        );
        let qs = metrics.queue_stats.qstats();
        assert_eq!(qs.xfer_stats["Total"].curr_rate, 1.5);
        // Target stats start from the cluster totals seen so far (none yet).
        assert_eq!(qs.tgt_xfer_stats["arn1"]["Small"].peak_rate, 2.0);
    }

    #[test]
    fn empty_rules_marshal_as_null() {
        let config = ReplicationConfig::default();
        assert_eq!(
            serde_json::to_string(&config).unwrap(),
            r#"{"Rules":null,"Role":""}"#
        );
        let back: ReplicationConfig = serde_json::from_str(r#"{"Rules":null,"Role":""}"#).unwrap();
        assert!(back.rules.is_empty());
        let diff = DiffInfo {
            object: "o".into(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&diff).unwrap(),
            r#"{"object":"o","versionId":"","replTimestamp":"0001-01-01T00:00:00Z","lastModified":"0001-01-01T00:00:00Z","deletemarker":false}"#
        );
    }

    #[test]
    fn formats_go_durations() {
        use std::time::Duration;
        assert_eq!(go_duration(Duration::from_secs(60 * 86400)), "1440h0m0s");
        assert_eq!(go_duration(Duration::from_secs(90)), "1m30s");
        assert_eq!(go_duration(Duration::from_secs(5)), "5s");
        assert_eq!(new_uuid().len(), 36);
    }
}
