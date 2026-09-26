//! MinIO admin API: site replication, pool decommission, rebalance (TOPO area).
//!
//! Types mirror madmin-go (`cluster-commands.go`, `decommission-commands.go`, `rebalance.go`)
//! with Go's field order and JSON names, so `--json` output re-marshals like mc. Go
//! `time.Time` values are kept as the server's RFC 3339 strings ([`GoTime`]), Go `time.Duration`
//! values as nanoseconds.

use super::admin::{AdminClient, GO_ZERO_TIME, JsonStream, go_f32, go_f64};
use super::replication::{DowntimeInfo, ReplMrfStats, XferStats};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// madmin `SiteReplAPIVersion`.
const SITE_REPL_API_VERSION: &str = "1";

/// Go `time.Time` as sent by the server; the zero value is `0001-01-01T00:00:00Z`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GoTime(pub String);

impl Default for GoTime {
    fn default() -> Self {
        Self(GO_ZERO_TIME.to_string())
    }
}

impl GoTime {
    pub fn is_zero(&self) -> bool {
        self.0.is_empty() || self.0 == GO_ZERO_TIME
    }

    /// Nanoseconds from this time to `other` (0 when either is unparsable).
    pub fn nanos_until(&self, other: &GoTime) -> i64 {
        match (parse_nanos(&self.0), parse_nanos(&other.0)) {
            (Some(a), Some(b)) => b.saturating_sub(a),
            _ => 0,
        }
    }

    /// Nanoseconds elapsed since this time (0 when unparsable).
    pub fn elapsed_nanos(&self) -> i64 {
        let Some(then) = parse_nanos(&self.0) else {
            return 0;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or_default();
        now.saturating_sub(then)
    }
}

/// Unix nanoseconds of an RFC 3339 time.
fn parse_nanos(text: &str) -> Option<i64> {
    use aws_smithy_types::date_time::{DateTime, Format};
    let time = DateTime::from_str(text, Format::DateTime).ok()?;
    Some(
        time.secs()
            .saturating_mul(1_000_000_000)
            .saturating_add(time.subsec_nanos() as i64),
    )
}

// ---------------------------------------------------------------------------
// Pool decommission
// ---------------------------------------------------------------------------

/// madmin `PoolDecommissionInfo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolDecommissionInfo {
    #[serde(rename = "startTime")]
    pub start_time: GoTime,
    #[serde(rename = "startSize")]
    pub start_size: i64,
    #[serde(rename = "totalSize")]
    pub total_size: i64,
    #[serde(rename = "currentSize")]
    pub current_size: i64,
    pub complete: bool,
    pub failed: bool,
    pub canceled: bool,
    #[serde(rename = "objectsDecommissioned")]
    pub objects_decommissioned: i64,
    #[serde(rename = "objectsDecommissionedFailed")]
    pub objects_decommission_failed: i64,
    #[serde(rename = "bytesDecommissioned")]
    pub bytes_done: i64,
    #[serde(rename = "bytesDecommissionedFailed")]
    pub bytes_failed: i64,
}

/// madmin `PoolStatus`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolStatus {
    pub id: i64,
    pub cmdline: String,
    #[serde(rename = "lastUpdate")]
    pub last_update: GoTime,
    #[serde(rename = "decommissionInfo", skip_serializing_if = "Option::is_none")]
    pub decommission: Option<PoolDecommissionInfo>,
}

/// madmin `DecommissionPool`: `POST pools/decommission?pool=`.
pub async fn decommission_pool(client: &AdminClient, pool: &str) -> Result<()> {
    client
        .request("POST", "pools/decommission")
        .query("pool", pool)
        .send()
        .await?;
    Ok(())
}

/// madmin `CancelDecommissionPool`: `POST pools/cancel?pool=`.
pub async fn cancel_decommission_pool(client: &AdminClient, pool: &str) -> Result<()> {
    client
        .request("POST", "pools/cancel")
        .query("pool", pool)
        .send()
        .await?;
    Ok(())
}

/// madmin `StatusPool`: `GET pools/status?pool=`.
pub async fn status_pool(client: &AdminClient, pool: &str) -> Result<PoolStatus> {
    client
        .request("GET", "pools/status")
        .query("pool", pool)
        .send_json()
        .await
}

/// madmin `ListPoolsStatus`: `GET pools/list`.
pub async fn list_pools_status(client: &AdminClient) -> Result<Vec<PoolStatus>> {
    let pools: Option<Vec<PoolStatus>> = client.request("GET", "pools/list").send_json().await?;
    Ok(pools.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Rebalance
// ---------------------------------------------------------------------------

/// madmin `RebalPoolProgress` (durations in nanoseconds).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RebalPoolProgress {
    #[serde(rename = "objects")]
    pub num_objects: u64,
    #[serde(rename = "versions")]
    pub num_versions: u64,
    pub bytes: u64,
    pub bucket: String,
    pub object: String,
    pub elapsed: i64,
    pub eta: i64,
}

/// madmin `RebalancePoolStatus`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RebalancePoolStatus {
    pub id: i64,
    pub status: String,
    #[serde(serialize_with = "go_f64")]
    pub used: f64,
    pub progress: RebalPoolProgress,
}

/// madmin `RebalanceStatus`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RebalanceStatus {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "stoppedAt")]
    pub stopped_at: GoTime,
    pub pools: Option<Vec<RebalancePoolStatus>>,
}

/// madmin `RebalanceStart`: `POST rebalance/start`, returns the operation id.
pub async fn rebalance_start(client: &AdminClient) -> Result<String> {
    #[derive(Deserialize)]
    struct Started {
        #[serde(default)]
        id: String,
    }
    let started: Started = client
        .request("POST", "rebalance/start")
        .send_json()
        .await?;
    Ok(started.id)
}

/// madmin `RebalanceStatus`: `GET rebalance/status`.
pub async fn rebalance_status(client: &AdminClient) -> Result<RebalanceStatus> {
    client.request("GET", "rebalance/status").send_json().await
}

/// madmin `RebalanceStop`: `POST rebalance/stop`.
pub async fn rebalance_stop(client: &AdminClient) -> Result<()> {
    client.request("POST", "rebalance/stop").send().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Site replication
// ---------------------------------------------------------------------------

/// madmin `PeerSite` (`site-replication/add` request item).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PeerSite {
    pub name: String,
    #[serde(rename = "endpoints")]
    pub endpoint: String,
    #[serde(rename = "accessKey")]
    pub access_key: String,
    #[serde(rename = "secretKey")]
    pub secret_key: String,
}

/// madmin `ReplicateAddStatus`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplicateAddStatus {
    pub success: bool,
    pub status: String,
    #[serde(rename = "errorDetail", skip_serializing_if = "String::is_empty")]
    pub err_detail: String,
    #[serde(
        rename = "initialSyncErrorMessage",
        skip_serializing_if = "String::is_empty"
    )]
    pub initial_sync_error_message: String,
}

/// madmin `BucketBandwidth`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BucketBandwidth {
    #[serde(rename = "bandwidthLimitPerBucket")]
    pub limit: u64,
    #[serde(rename = "set")]
    pub is_set: bool,
    #[serde(rename = "updatedAt")]
    pub updated_at: GoTime,
}

/// madmin `PeerInfo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PeerInfo {
    pub endpoint: String,
    pub name: String,
    #[serde(rename = "deploymentID")]
    pub deployment_id: String,
    /// `enable` / `disable` (madmin `SyncStatus`).
    pub sync: String,
    #[serde(rename = "defaultbandwidth")]
    pub default_bandwidth: BucketBandwidth,
    #[serde(rename = "replicate-ilm-expiry")]
    pub replicate_ilm_expiry: bool,
    #[serde(rename = "apiVersion", skip_serializing_if = "String::is_empty")]
    pub api_version: String,
}

/// madmin `SiteReplicationInfo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SiteReplicationInfo {
    pub enabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<PeerInfo>,
    #[serde(
        rename = "serviceAccountAccessKey",
        skip_serializing_if = "String::is_empty"
    )]
    pub service_account_access_key: String,
    #[serde(rename = "apiVersion", skip_serializing_if = "String::is_empty")]
    pub api_version: String,
}

/// madmin `ReplicateEditStatus`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplicateEditStatus {
    pub success: bool,
    pub status: String,
    #[serde(rename = "errorDetail", skip_serializing_if = "String::is_empty")]
    pub err_detail: String,
}

/// madmin `ReplicateRemoveStatus`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplicateRemoveStatus {
    pub status: String,
    #[serde(rename = "errorDetail", skip_serializing_if = "String::is_empty")]
    pub err_detail: String,
    #[serde(rename = "apiVersion", skip_serializing_if = "String::is_empty")]
    pub api_version: String,
}

/// madmin `ReplicateRemoveStatusSuccess`.
pub const REPLICATE_REMOVE_STATUS_SUCCESS: &str =
    "Requested site(s) were removed from cluster replication successfully.";

/// madmin `SRRemoveReq`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SrRemoveReq {
    #[serde(rename = "requestingDepID")]
    pub requesting_dep_id: String,
    /// Go nil slice (`null`) when no site names are given.
    #[serde(rename = "sites")]
    pub site_names: Option<Vec<String>>,
    #[serde(rename = "all")]
    pub remove_all: bool,
}

/// madmin `ResyncBucketStatus`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResyncBucketStatus {
    pub bucket: String,
    pub status: String,
    #[serde(rename = "errorDetail", skip_serializing_if = "String::is_empty")]
    pub err_detail: String,
}

/// madmin `SRResyncOpStatus`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrResyncOpStatus {
    #[serde(rename = "op")]
    pub op_type: String,
    #[serde(rename = "id")]
    pub resync_id: String,
    pub status: String,
    pub buckets: Option<Vec<ResyncBucketStatus>>,
    #[serde(rename = "errorDetail", skip_serializing_if = "String::is_empty")]
    pub err_detail: String,
}

/// madmin `SRPolicyStatsSummary`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SrPolicyStatsSummary {
    #[serde(rename = "DeploymentID")]
    pub deployment_id: String,
    pub policy_mismatch: bool,
    pub has_policy: bool,
    #[serde(rename = "APIVersion")]
    pub api_version: String,
}

/// madmin `SRUserStatsSummary`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SrUserStatsSummary {
    #[serde(rename = "DeploymentID")]
    pub deployment_id: String,
    pub policy_mismatch: bool,
    pub user_info_mismatch: bool,
    pub has_user: bool,
    pub has_policy_mapping: bool,
    #[serde(rename = "APIVersion")]
    pub api_version: String,
}

/// madmin `SRGroupStatsSummary`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SrGroupStatsSummary {
    #[serde(rename = "DeploymentID")]
    pub deployment_id: String,
    pub policy_mismatch: bool,
    pub has_group: bool,
    pub group_desc_mismatch: bool,
    pub has_policy_mapping: bool,
    #[serde(rename = "APIVersion")]
    pub api_version: String,
}

/// madmin `SRBucketStatsSummary`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SrBucketStatsSummary {
    #[serde(rename = "DeploymentID")]
    pub deployment_id: String,
    pub has_bucket: bool,
    pub bucket_marked_deleted: bool,
    pub tag_mismatch: bool,
    pub versioning_config_mismatch: bool,
    #[serde(rename = "OLockConfigMismatch")]
    pub olock_config_mismatch: bool,
    pub policy_mismatch: bool,
    #[serde(rename = "SSEConfigMismatch")]
    pub sse_config_mismatch: bool,
    pub replication_cfg_mismatch: bool,
    pub quota_cfg_mismatch: bool,
    pub cors_cfg_mismatch: bool,
    pub has_tags_set: bool,
    #[serde(rename = "HasOLockConfigSet")]
    pub has_olock_config_set: bool,
    pub has_policy_set: bool,
    #[serde(rename = "HasSSECfgSet")]
    pub has_sse_cfg_set: bool,
    pub has_replication_cfg: bool,
    pub has_quota_cfg_set: bool,
    pub has_cors_cfg_set: bool,
    #[serde(rename = "APIVersion")]
    pub api_version: String,
}

/// madmin `SRILMExpiryStatsSummary`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SrIlmExpiryStatsSummary {
    #[serde(rename = "DeploymentID")]
    pub deployment_id: String,
    #[serde(rename = "ILMExpiryRuleMismatch")]
    pub ilm_expiry_rule_mismatch: bool,
    #[serde(rename = "HasILMExpiryRules")]
    pub has_ilm_expiry_rules: bool,
    #[serde(rename = "APIVersion")]
    pub api_version: String,
}

/// madmin `SRSiteSummary`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SrSiteSummary {
    pub replicated_buckets: i64,
    pub replicated_tags: i64,
    pub replicated_bucket_policies: i64,
    #[serde(rename = "ReplicatedIAMPolicies")]
    pub replicated_iam_policies: i64,
    pub replicated_users: i64,
    pub replicated_groups: i64,
    pub replicated_lock_config: i64,
    #[serde(rename = "ReplicatedSSEConfig")]
    pub replicated_sse_config: i64,
    pub replicated_versioning_config: i64,
    pub replicated_quota_config: i64,
    pub replicated_user_policy_mappings: i64,
    pub replicated_group_policy_mappings: i64,
    #[serde(rename = "ReplicatedILMExpiryRules")]
    pub replicated_ilm_expiry_rules: i64,
    pub replicated_cors_config: i64,
    pub total_buckets_count: i64,
    pub total_tags_count: i64,
    pub total_bucket_policies_count: i64,
    #[serde(rename = "TotalIAMPoliciesCount")]
    pub total_iam_policies_count: i64,
    pub total_lock_config_count: i64,
    #[serde(rename = "TotalSSEConfigCount")]
    pub total_sse_config_count: i64,
    pub total_versioning_config_count: i64,
    pub total_quota_config_count: i64,
    pub total_users_count: i64,
    pub total_groups_count: i64,
    pub total_user_policy_mapping_count: i64,
    pub total_group_policy_mapping_count: i64,
    #[serde(rename = "TotalILMExpiryRulesCount")]
    pub total_ilm_expiry_rules_count: i64,
    pub total_cors_config_count: i64,
    #[serde(rename = "APIVersion")]
    pub api_version: String,
}

/// madmin `WorkerStat`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerStat {
    pub curr: i64,
    #[serde(serialize_with = "go_f32")]
    pub avg: f32,
    pub max: i64,
}

/// madmin `QStat`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QStat {
    #[serde(serialize_with = "go_f64")]
    pub count: f64,
    #[serde(serialize_with = "go_f64")]
    pub bytes: f64,
}

/// madmin `InQueueMetric`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InQueueMetric {
    pub curr: QStat,
    pub avg: QStat,
    pub max: QStat,
}

/// madmin `ReplProxyMetric`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplProxyMetric {
    #[serde(rename = "putTaggingProxyTotal")]
    pub put_tag_total: u64,
    #[serde(rename = "getTaggingProxyTotal")]
    pub get_tag_total: u64,
    #[serde(rename = "removeTaggingProxyTotal")]
    pub rmv_tag_total: u64,
    #[serde(rename = "getProxyTotal")]
    pub get_total: u64,
    #[serde(rename = "headProxyTotal")]
    pub head_total: u64,
    #[serde(rename = "putTaggingProxyFailed")]
    pub put_tag_failed_total: u64,
    #[serde(rename = "getTaggingProxyFailed")]
    pub get_tag_failed_total: u64,
    #[serde(rename = "removeTaggingProxyFailed")]
    pub rmv_tag_failed_total: u64,
    #[serde(rename = "getProxyFailed")]
    pub get_failed_total: u64,
    #[serde(rename = "headProxyFailed")]
    pub head_failed_total: u64,
}

/// madmin `Counter`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Counter {
    pub last1hr: u64,
    pub last1m: u64,
    pub total: u64,
}

/// madmin `LatencyStat` (nanoseconds).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LatencyStat {
    pub curr: i64,
    pub avg: i64,
    pub max: i64,
}

/// madmin `RStat`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RStat {
    #[serde(serialize_with = "go_f64")]
    pub count: f64,
    pub bytes: i64,
}

/// madmin `TimedErrStats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimedErrStats {
    #[serde(rename = "lastMinute")]
    pub last_minute: RStat,
    #[serde(rename = "lastHour")]
    pub last_hour: RStat,
    pub totals: RStat,
    #[serde(rename = "errCounts", skip_serializing_if = "BTreeMap::is_empty")]
    pub err_counts: BTreeMap<String, i64>,
}

/// madmin `SRMetric`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrMetric {
    #[serde(rename = "deploymentID")]
    pub deployment_id: String,
    pub endpoint: String,
    #[serde(rename = "totalDowntime")]
    pub total_downtime: i64,
    #[serde(rename = "lastOnline")]
    pub last_online: GoTime,
    #[serde(rename = "isOnline")]
    pub online: bool,
    pub latency: LatencyStat,
    #[serde(rename = "replicatedSize")]
    pub replicated_size: i64,
    #[serde(rename = "replicatedCount")]
    pub replicated_count: i64,
    pub failed: TimedErrStats,
    #[serde(rename = "transferSummary")]
    pub xfer_stats: Option<BTreeMap<String, XferStats>>,
    #[serde(rename = "mrfStats")]
    pub mrf_stats: ReplMrfStats,
    #[serde(rename = "downtimeInfo")]
    pub downtime_info: DowntimeInfo,
}

/// madmin `SRMetricsSummary`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrMetricsSummary {
    #[serde(rename = "activeWorkers")]
    pub active_workers: WorkerStat,
    #[serde(rename = "replicaSize")]
    pub replica_size: i64,
    #[serde(rename = "replicaCount")]
    pub replica_count: i64,
    pub queued: InQueueMetric,
    pub proxied: ReplProxyMetric,
    #[serde(rename = "replMetrics")]
    pub metrics: Option<BTreeMap<String, SrMetric>>,
    pub uptime: i64,
    pub retries: Counter,
    pub errors: Counter,
}

/// `entity name -> deployment id -> stats` (Go map; `null` when nil).
pub type StatsMap<T> = Option<BTreeMap<String, BTreeMap<String, T>>>;

/// madmin `SRStatusInfo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SrStatusInfo {
    pub enabled: bool,
    pub max_buckets: i64,
    pub max_users: i64,
    pub max_groups: i64,
    pub max_policies: i64,
    #[serde(rename = "MaxILMExpiryRules")]
    pub max_ilm_expiry_rules: i64,
    pub sites: Option<BTreeMap<String, PeerInfo>>,
    pub stats_summary: Option<BTreeMap<String, SrSiteSummary>>,
    pub bucket_stats: StatsMap<SrBucketStatsSummary>,
    pub policy_stats: StatsMap<SrPolicyStatsSummary>,
    pub user_stats: StatsMap<SrUserStatsSummary>,
    pub group_stats: StatsMap<SrGroupStatsSummary>,
    pub metrics: SrMetricsSummary,
    #[serde(rename = "ILMExpiryStats")]
    pub ilm_expiry_stats: StatsMap<SrIlmExpiryStatsSummary>,
    #[serde(rename = "apiVersion", skip_serializing_if = "String::is_empty")]
    pub api_version: String,
}

/// madmin `SREntityType`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SrEntity {
    #[default]
    Unspecified,
    Bucket,
    Policy,
    User,
    Group,
    IlmExpiryRule,
}

impl SrEntity {
    /// Value of the `entity` query parameter.
    fn query_value(self) -> Option<&'static str> {
        match self {
            SrEntity::Unspecified => None,
            SrEntity::Bucket => Some("bucket"),
            SrEntity::Policy => Some("policy"),
            SrEntity::User => Some("user"),
            SrEntity::Group => Some("group"),
            SrEntity::IlmExpiryRule => Some("ilm-expiry-rule"),
        }
    }
}

/// madmin `SRStatusOptions`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SrStatusOptions {
    pub buckets: bool,
    pub policies: bool,
    pub users: bool,
    pub groups: bool,
    pub metrics: bool,
    pub ilm_expiry_rules: bool,
    pub entity: SrEntity,
    pub entity_value: String,
}

impl SrStatusOptions {
    /// madmin `getURLValues` plus `api-version`.
    fn query(&self) -> Vec<(&'static str, String)> {
        let mut query = vec![
            ("buckets", self.buckets.to_string()),
            ("policies", self.policies.to_string()),
            ("users", self.users.to_string()),
            ("groups", self.groups.to_string()),
            ("showDeleted", "false".to_string()),
            ("metrics", self.metrics.to_string()),
            ("ilm-expiry-rules", self.ilm_expiry_rules.to_string()),
            ("peer-state", "false".to_string()),
        ];
        if let Some(entity) = self.entity.query_value() {
            query.push(("entityvalue", self.entity_value.clone()));
            query.push(("entity", entity.to_string()));
        }
        query.push(("api-version", SITE_REPL_API_VERSION.to_string()));
        query
    }
}

/// madmin `SiteReplicationAdd`: `PUT site-replication/add` (encrypted body).
pub async fn site_replication_add(
    client: &AdminClient,
    sites: &[PeerSite],
    replicate_ilm_expiry: bool,
) -> Result<ReplicateAddStatus> {
    client
        .request("PUT", "site-replication/add")
        .query("replicateILMExpiry", replicate_ilm_expiry.to_string())
        .query("force", "false")
        .query("api-version", SITE_REPL_API_VERSION)
        .encrypted_json(sites)?
        .send_json()
        .await
}

/// madmin `SiteReplicationInfo`: `GET site-replication/info`.
pub async fn site_replication_info(client: &AdminClient) -> Result<SiteReplicationInfo> {
    client
        .request("GET", "site-replication/info")
        .query("api-version", SITE_REPL_API_VERSION)
        .send_json()
        .await
}

/// madmin `SiteReplicationEdit`: `PUT site-replication/edit` (encrypted body).
pub async fn site_replication_edit(
    client: &AdminClient,
    site: &PeerInfo,
    disable_ilm_expiry: bool,
    enable_ilm_expiry: bool,
) -> Result<ReplicateEditStatus> {
    client
        .request("PUT", "site-replication/edit")
        .query(
            "disableILMExpiryReplication",
            disable_ilm_expiry.to_string(),
        )
        .query("enableILMExpiryReplication", enable_ilm_expiry.to_string())
        .query("api-version", SITE_REPL_API_VERSION)
        .encrypted_json(site)?
        .send_json()
        .await
}

/// madmin `SiteReplicationRemove`: `PUT site-replication/remove`.
pub async fn site_replication_remove(
    client: &AdminClient,
    request: &SrRemoveReq,
) -> Result<ReplicateRemoveStatus> {
    client
        .request("PUT", "site-replication/remove")
        .query("api-version", SITE_REPL_API_VERSION)
        .json(request)?
        .send_json()
        .await
}

/// madmin `SRStatusInfo`: `GET site-replication/status`.
pub async fn sr_status_info(client: &AdminClient, opts: &SrStatusOptions) -> Result<SrStatusInfo> {
    let query = opts.query();
    let pairs: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
    client
        .request("GET", "site-replication/status")
        .queries(&pairs)
        .send_json()
        .await
}

/// madmin `SiteReplicationResyncOp`: `PUT site-replication/resync/op?operation=start|cancel`.
pub async fn site_replication_resync_op(
    client: &AdminClient,
    site: &PeerInfo,
    operation: &str,
) -> Result<SrResyncOpStatus> {
    client
        .request("PUT", "site-replication/resync/op")
        .query("operation", operation)
        .query("api-version", SITE_REPL_API_VERSION)
        .json(site)?
        .send_json()
        .await
}

/// Deployment ID of the cluster behind `client` (madmin `ServerInfo().DeploymentID`).
pub async fn deployment_id(client: &AdminClient) -> Result<String> {
    #[derive(Deserialize)]
    struct Info {
        #[serde(rename = "deploymentID", default)]
        deployment_id: String,
    }
    let info: Info = client.request("GET", "info").send_json().await?;
    Ok(info.deployment_id)
}

/// madmin `SiteResyncMetrics`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SiteResyncMetrics {
    #[serde(rename = "resyncStatus")]
    pub resync_status: String,
    #[serde(rename = "startTime")]
    pub start_time: GoTime,
    #[serde(rename = "lastUpdate")]
    pub last_update: GoTime,
    #[serde(rename = "resyncID")]
    pub resync_id: String,
    #[serde(rename = "completedReplicationSize")]
    pub replicated_size: i64,
    #[serde(rename = "replicationCount")]
    pub replicated_count: i64,
    #[serde(rename = "failedReplicationCount")]
    pub failed_count: i64,
    pub bucket: String,
    pub object: String,
}

impl SiteResyncMetrics {
    /// madmin `Complete()`.
    pub fn complete(&self) -> bool {
        self.resync_status.eq_ignore_ascii_case("completed")
    }
}

/// The parts of madmin `RealtimeMetrics` used for site resync status.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ResyncRealtimeMetrics {
    pub aggregated: ResyncAggregated,
    #[serde(rename = "final")]
    pub is_final: bool,
}

/// madmin `Metrics` (site resync part).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ResyncAggregated {
    #[serde(rename = "siteResync")]
    pub site_resync: Option<SiteResyncMetrics>,
}

/// madmin `MetricsSiteResync` type bit.
const METRICS_SITE_RESYNC: u32 = 1 << 4;

/// madmin `Metrics` with `Type: MetricsSiteResync, ByDepID`: stream of realtime metrics.
pub async fn site_resync_metrics(client: &AdminClient, deployment_id: &str) -> Result<JsonStream> {
    client
        .request("GET", "metrics")
        .query("types", METRICS_SITE_RESYNC.to_string())
        .query("n", "0")
        .query("interval", "0s")
        .query("hosts", "")
        .query("disks", "")
        .query("by-depID", deployment_id)
        .stream()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::admin::mock::{client, reply, serve};

    #[test]
    fn pool_status_round_trips_go_json() {
        let text = r#"{"id":1,"cmdline":"/data{5...8}","lastUpdate":"2026-09-26T19:29:40.228936034Z","decommissionInfo":{"startTime":"0001-01-01T00:00:00Z","startSize":1,"totalSize":10,"currentSize":4,"complete":false,"failed":false,"canceled":true,"objectsDecommissioned":153,"objectsDecommissionedFailed":0,"bytesDecommissioned":1471,"bytesDecommissionedFailed":0}}"#;
        let status: PoolStatus = serde_json::from_str(text).unwrap();
        assert!(status.decommission.as_ref().unwrap().start_time.is_zero());
        assert_eq!(serde_json::to_string(&status).unwrap(), text);
        let bare: PoolStatus = serde_json::from_str(r#"{"id":0,"cmdline":"x"}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&bare).unwrap(),
            r#"{"id":0,"cmdline":"x","lastUpdate":"0001-01-01T00:00:00Z"}"#
        );
    }

    #[test]
    fn rebalance_status_round_trips_go_json() {
        let text = r#"{"ID":"n4Za","stoppedAt":"0001-01-01T00:00:00Z","pools":[{"id":0,"status":"None","used":0.5086507767922406,"progress":{"objects":0,"versions":0,"bytes":0,"bucket":"","object":"","elapsed":0,"eta":0}}]}"#;
        let status: RebalanceStatus = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&status).unwrap(), text);
    }

    #[test]
    fn sr_status_keeps_nulls_and_go_names() {
        let text = r#"{"Enabled":false,"MaxBuckets":0,"MaxUsers":0,"MaxGroups":0,"MaxPolicies":0,"MaxILMExpiryRules":0,"Sites":null,"StatsSummary":null,"BucketStats":{},"PolicyStats":{},"UserStats":{},"GroupStats":{},"Metrics":{"activeWorkers":{"curr":0,"avg":0,"max":0},"replicaSize":0,"replicaCount":0,"queued":{"curr":{"count":0,"bytes":0},"avg":{"count":0,"bytes":0},"max":{"count":0,"bytes":0}},"proxied":{"putTaggingProxyTotal":0,"getTaggingProxyTotal":0,"removeTaggingProxyTotal":0,"getProxyTotal":0,"headProxyTotal":0,"putTaggingProxyFailed":0,"getTaggingProxyFailed":0,"removeTaggingProxyFailed":0,"getProxyFailed":0,"headProxyFailed":0},"replMetrics":null,"uptime":0,"retries":{"last1hr":0,"last1m":0,"total":0},"errors":{"last1hr":0,"last1m":0,"total":0}},"ILMExpiryStats":null}"#;
        let status: SrStatusInfo = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&status).unwrap(), text);
    }

    #[test]
    fn sr_site_summary_uses_go_field_names() {
        let summary = serde_json::to_value(SrSiteSummary::default()).unwrap();
        let keys: Vec<&str> = summary
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for key in [
            "ReplicatedIAMPolicies",
            "ReplicatedSSEConfig",
            "ReplicatedILMExpiryRules",
            "TotalIAMPoliciesCount",
            "TotalCorsConfigCount",
            "APIVersion",
        ] {
            assert!(keys.contains(&key), "{key} missing: {keys:?}");
        }
        let bucket = serde_json::to_string(&SrBucketStatsSummary::default()).unwrap();
        assert!(bucket.starts_with(r#"{"DeploymentID":"","HasBucket":false,"BucketMarkedDeleted":false,"TagMismatch":false,"VersioningConfigMismatch":false,"OLockConfigMismatch":false"#));
    }

    #[test]
    fn sr_metric_round_trips_go_json() {
        let text = r#"{"deploymentID":"d","endpoint":"h:9000","totalDowntime":0,"lastOnline":"2026-09-26T19:29:56.338582317Z","isOnline":true,"latency":{"curr":436523,"avg":696998,"max":1862281},"replicatedSize":3,"replicatedCount":1,"failed":{"lastMinute":{"count":0,"bytes":0},"lastHour":{"count":0,"bytes":0},"totals":{"count":0,"bytes":0}},"transferSummary":{"Large":{"avgRate":0,"peakRate":0,"currRate":0},"Total":{"avgRate":0,"peakRate":1.1003499615370838,"currRate":1.1003499615370838}},"mrfStats":{"failedCount_last5min":0,"droppedCount_since_uptime":0,"droppedBytes_since_uptime":0},"downtimeInfo":{"duration":{"total":0,"avg":0,"max":0},"count":{"total":0,"avg":0,"max":0}}}"#;
        let metric: SrMetric = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&metric).unwrap(), text);
    }

    #[test]
    fn site_info_omits_empty_fields() {
        let info: SiteReplicationInfo = serde_json::from_str(r#"{"enabled":false}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"enabled":false}"#
        );
        let peer = PeerInfo::default();
        assert_eq!(
            serde_json::to_string(&peer).unwrap(),
            r#"{"endpoint":"","name":"","deploymentID":"","sync":"","defaultbandwidth":{"bandwidthLimitPerBucket":0,"set":false,"updatedAt":"0001-01-01T00:00:00Z"},"replicate-ilm-expiry":false}"#
        );
    }

    #[test]
    fn status_query_matches_madmin() {
        let opts = SrStatusOptions {
            users: true,
            entity: SrEntity::User,
            entity_value: "u1".into(),
            ..Default::default()
        };
        let query: Vec<String> = opts
            .query()
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        assert_eq!(
            query.join("&"),
            "buckets=false&policies=false&users=true&groups=false&showDeleted=false&metrics=false&ilm-expiry-rules=false&peer-state=false&entityvalue=u1&entity=user&api-version=1"
        );
    }

    #[test]
    fn remove_request_sends_null_sites_without_names() {
        let req = SrRemoveReq {
            remove_all: true,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&req).unwrap(),
            r#"{"requestingDepID":"","sites":null,"all":true}"#
        );
    }

    #[test]
    fn go_time_arithmetic() {
        let a = GoTime("2026-09-26T19:00:00Z".into());
        let b = GoTime("2026-09-26T19:00:01.5Z".into());
        assert_eq!(a.nanos_until(&b), 1_500_000_000);
        assert!(GoTime::default().is_zero());
        assert!(a.elapsed_nanos() != 0);
    }

    #[test]
    fn topology_requests_use_madmin_paths() {
        let server = serve(vec![
            reply(200, b""),
            reply(200, b"[]"),
            reply(200, br#"{"id":"abc"}"#),
            reply(
                200,
                br#"{"op":"start","id":"r1","status":"success","buckets":null}"#,
            ),
        ]);
        let client = client(&server, "secret");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(decommission_pool(&client, "/data{5...8}"))
            .unwrap();
        assert!(rt.block_on(list_pools_status(&client)).unwrap().is_empty());
        assert_eq!(rt.block_on(rebalance_start(&client)).unwrap(), "abc");
        let status = rt
            .block_on(site_replication_resync_op(
                &client,
                &PeerInfo::default(),
                "start",
            ))
            .unwrap();
        assert_eq!(status.resync_id, "r1");
        let requests = server.requests();
        assert!(
            requests[0]
                .head
                .starts_with("POST /minio/admin/v3/pools/decommission?pool=%2Fdata%7B5...8%7D "),
            "{}",
            requests[0].head
        );
        assert!(
            requests[1]
                .head
                .starts_with("GET /minio/admin/v3/pools/list ")
        );
        assert!(
            requests[2]
                .head
                .starts_with("POST /minio/admin/v3/rebalance/start ")
        );
        assert!(requests[3].head.starts_with(
            "PUT /minio/admin/v3/site-replication/resync/op?api-version=1&operation=start "
        ));
        let body: serde_json::Value = serde_json::from_slice(&requests[3].body).unwrap();
        assert_eq!(body["deploymentID"], "");
    }
}
