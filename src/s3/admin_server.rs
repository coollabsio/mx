//! MinIO admin API: service, update, server info, config, prometheus, KMS, scanner, cluster
//! metadata (SERVER area). Types mirror madmin-go (Go field order, `omitempty`) so `--json`
//! output re-marshals like mc.

use super::admin::{AdminClient, Response, go_f64};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Go's zero `time.Time` as JSON.
pub const GO_ZERO_TIME: &str = "0001-01-01T00:00:00Z";

fn zero_time() -> String {
    GO_ZERO_TIME.to_string()
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

// ---------------------------------------------------------------------------
// Shared madmin types
// ---------------------------------------------------------------------------

/// `madmin.TimedAction`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimedAction {
    pub count: u64,
    #[serde(rename = "acc_time_ns")]
    pub acc_time: u64,
    #[serde(rename = "min_ns", skip_serializing_if = "is_zero_u64")]
    pub min_time: u64,
    #[serde(rename = "max_ns", skip_serializing_if = "is_zero_u64")]
    pub max_time: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub bytes: u64,
}

impl TimedAction {
    /// Average duration in nanoseconds.
    pub fn avg(&self) -> u64 {
        self.acc_time.checked_div(self.count).unwrap_or(0)
    }

    pub fn avg_bytes(&self) -> u64 {
        self.bytes.checked_div(self.count).unwrap_or(0)
    }
}

/// `madmin.DiskMetrics`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskMetrics {
    #[serde(rename = "lastMinute", skip_serializing_if = "BTreeMap::is_empty")]
    pub last_minute: BTreeMap<String, TimedAction>,
    #[serde(rename = "apiCalls", skip_serializing_if = "BTreeMap::is_empty")]
    pub api_calls: BTreeMap<String, u64>,
    #[serde(rename = "totalTokens", skip_serializing_if = "is_zero_u32")]
    pub total_tokens: u32,
    #[serde(rename = "totalWaiting", skip_serializing_if = "is_zero_u32")]
    pub total_waiting: u32,
    #[serde(
        rename = "totalErrorsAvailability",
        skip_serializing_if = "is_zero_u64"
    )]
    pub total_errors_availability: u64,
    #[serde(rename = "totalErrorsTimeout", skip_serializing_if = "is_zero_u64")]
    pub total_errors_timeout: u64,
    #[serde(rename = "totalWrites", skip_serializing_if = "is_zero_u64")]
    pub total_writes: u64,
    #[serde(rename = "totalDeletes", skip_serializing_if = "is_zero_u64")]
    pub total_deletes: u64,
    #[serde(rename = "apiLatencies", skip_serializing_if = "BTreeMap::is_empty")]
    pub api_latencies: BTreeMap<String, Value>,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// `madmin.ServiceActionPeerResult`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceActionPeerResult {
    pub host: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub err: String,
    #[serde(rename = "waitingDrives", skip_serializing_if = "BTreeMap::is_empty")]
    pub waiting_drives: BTreeMap<String, DiskMetrics>,
}

/// `madmin.ServiceActionResult`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceActionResult {
    pub action: String,
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<ServiceActionPeerResult>,
}

/// madmin `ServiceAction` (`POST service?action=..&dry-run=..&type=2`).
pub async fn service_action(
    client: &AdminClient,
    action: &str,
    dry_run: bool,
) -> Result<ServiceActionResult> {
    client
        .request("POST", "service")
        .query("action", action)
        .query("dry-run", dry_run.to_string())
        .query("type", "2")
        .send_json()
        .await
}

/// Legacy `serviceCallAction` (older servers): `POST service?action=..`.
pub async fn service_action_v1(client: &AdminClient, action: &str) -> Result<()> {
    client
        .request("POST", "service")
        .query("action", action)
        .send()
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Server update
// ---------------------------------------------------------------------------

/// `madmin.ServerPeerUpdateStatus`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerPeerUpdateStatus {
    pub host: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub err: String,
    #[serde(rename = "currentVersion")]
    pub current_version: String,
    #[serde(rename = "updatedVersion")]
    pub updated_version: String,
    #[serde(rename = "waitingDrives", skip_serializing_if = "BTreeMap::is_empty")]
    pub waiting_drives: BTreeMap<String, DiskMetrics>,
}

/// `madmin.ServerUpdateStatusV2`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerUpdateStatus {
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<ServerPeerUpdateStatus>,
}

/// madmin `ServerUpdateV2`.
pub async fn server_update(
    client: &AdminClient,
    update_url: &str,
    dry_run: bool,
) -> Result<ServerUpdateStatus> {
    client
        .request("POST", "update")
        .query("dry-run", dry_run.to_string())
        .query("type", "2")
        .query("updateURL", update_url)
        .send_json()
        .await
}

// ---------------------------------------------------------------------------
// Server info (`madmin.InfoMessage`)
// ---------------------------------------------------------------------------

/// `madmin.Buckets` / `Objects` / `Versions` / `DeleteMarkers` (`count` + `error`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Count {
    pub count: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// `madmin.Usage`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub size: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// `madmin.KMS`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KmsInfo {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub encrypt: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub decrypt: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
}

/// `madmin.Status` / `madmin.LDAP`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
}

/// `madmin.Services`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Services {
    pub kms: KmsInfo,
    #[serde(rename = "kmsStatus", skip_serializing_if = "Vec::is_empty")]
    pub kms_status: Vec<KmsInfo>,
    pub ldap: Status,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub logger: Vec<BTreeMap<String, Status>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub audit: Vec<BTreeMap<String, Status>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notifications: Vec<BTreeMap<String, Vec<BTreeMap<String, Status>>>>,
}

/// `madmin.ErasureBackend`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ErasureBackend {
    #[serde(rename = "backendType")]
    pub backend_type: String,
    #[serde(rename = "onlineDisks")]
    pub online_disks: i64,
    #[serde(rename = "offlineDisks")]
    pub offline_disks: i64,
    #[serde(rename = "standardSCParity")]
    pub standard_sc_parity: i64,
    #[serde(rename = "rrSCParity")]
    pub rr_sc_parity: i64,
    #[serde(rename = "totalSets")]
    pub total_sets: Option<Vec<i64>>,
    #[serde(rename = "totalDrivesPerSet")]
    pub drives_per_set: Option<Vec<i64>>,
}

/// `madmin.MemStats`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MemStats {
    #[serde(rename = "Alloc")]
    pub alloc: u64,
    #[serde(rename = "TotalAlloc")]
    pub total_alloc: u64,
    #[serde(rename = "Mallocs")]
    pub mallocs: u64,
    #[serde(rename = "Frees")]
    pub frees: u64,
    #[serde(rename = "HeapAlloc")]
    pub heap_alloc: u64,
}

/// `madmin.GCStats`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GcStats {
    pub last_gc: String,
    pub num_gc: i64,
    pub pause_total: i64,
    pub pause: Option<Vec<i64>>,
    pub pause_end: Option<Vec<String>>,
}

impl Default for GcStats {
    fn default() -> Self {
        Self {
            last_gc: zero_time(),
            num_gc: 0,
            pause_total: 0,
            pause: None,
            pause_end: None,
        }
    }
}

/// `madmin.LicenseInfo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LicenseInfo {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Organization")]
    pub organization: String,
    #[serde(rename = "Plan")]
    pub plan: String,
    #[serde(rename = "IssuedAt")]
    pub issued_at: String,
    #[serde(rename = "ExpiresAt")]
    pub expires_at: String,
    #[serde(rename = "Trial")]
    pub trial: bool,
    #[serde(rename = "APIKey")]
    pub api_key: String,
}

impl Default for LicenseInfo {
    fn default() -> Self {
        Self {
            id: String::new(),
            organization: String::new(),
            plan: String::new(),
            issued_at: zero_time(),
            expires_at: zero_time(),
            trial: false,
            api_key: String::new(),
        }
    }
}

/// `madmin.CacheStats`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheStats {
    pub capacity: i64,
    pub used: i64,
    pub hits: i64,
    pub misses: i64,
    #[serde(rename = "delHits")]
    pub del_hits: i64,
    #[serde(rename = "delMisses")]
    pub del_misses: i64,
    pub collisions: i64,
}

/// `madmin.HealingDisk`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HealingDisk {
    pub id: String,
    pub heal_id: String,
    pub pool_index: i64,
    pub set_index: i64,
    pub disk_index: i64,
    pub endpoint: String,
    pub path: String,
    pub started: String,
    pub last_update: String,
    pub retry_attempts: u64,
    pub objects_total_count: u64,
    pub objects_total_size: u64,
    pub items_healed: u64,
    pub items_failed: u64,
    pub items_skipped: u64,
    pub bytes_done: u64,
    pub bytes_failed: u64,
    pub bytes_skipped: u64,
    pub objects_healed: u64,
    pub objects_failed: u64,
    pub current_bucket: String,
    pub current_object: String,
    pub queued_buckets: Option<Vec<String>>,
    pub healed_buckets: Option<Vec<String>>,
    pub finished: bool,
    pub reason: i64,
}

impl Default for HealingDisk {
    fn default() -> Self {
        Self {
            id: String::new(),
            heal_id: String::new(),
            pool_index: 0,
            set_index: 0,
            disk_index: 0,
            endpoint: String::new(),
            path: String::new(),
            started: zero_time(),
            last_update: zero_time(),
            retry_attempts: 0,
            objects_total_count: 0,
            objects_total_size: 0,
            items_healed: 0,
            items_failed: 0,
            items_skipped: 0,
            bytes_done: 0,
            bytes_failed: 0,
            bytes_skipped: 0,
            objects_healed: 0,
            objects_failed: 0,
            current_bucket: String::new(),
            current_object: String::new(),
            queued_buckets: None,
            healed_buckets: None,
            finished: false,
            reason: 0,
        }
    }
}

/// `madmin.Disk`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Disk {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(rename = "rootDisk", skip_serializing_if = "is_false")]
    pub root_disk: bool,
    #[serde(rename = "path", skip_serializing_if = "String::is_empty")]
    pub drive_path: String,
    #[serde(skip_serializing_if = "is_false")]
    pub healing: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub scanning: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uuid: String,
    pub major: u32,
    pub minor: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(rename = "totalspace", skip_serializing_if = "is_zero_u64")]
    pub total_space: u64,
    #[serde(rename = "usedspace", skip_serializing_if = "is_zero_u64")]
    pub used_space: u64,
    #[serde(rename = "availspace", skip_serializing_if = "is_zero_u64")]
    pub available_space: u64,
    #[serde(
        rename = "readthroughput",
        skip_serializing_if = "is_zero_f64",
        serialize_with = "go_f64"
    )]
    pub read_throughput: f64,
    #[serde(
        rename = "writethroughput",
        skip_serializing_if = "is_zero_f64",
        serialize_with = "go_f64"
    )]
    pub write_throughput: f64,
    #[serde(
        rename = "readlatency",
        skip_serializing_if = "is_zero_f64",
        serialize_with = "go_f64"
    )]
    pub read_latency: f64,
    #[serde(
        rename = "writelatency",
        skip_serializing_if = "is_zero_f64",
        serialize_with = "go_f64"
    )]
    pub write_latency: f64,
    #[serde(skip_serializing_if = "is_zero_f64", serialize_with = "go_f64")]
    pub utilization: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<DiskMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heal_info: Option<HealingDisk>,
    pub used_inodes: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub free_inodes: u64,
    #[serde(skip_serializing_if = "is_false")]
    pub local: bool,
    #[serde(rename = "cacheStats", skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheStats>,
    pub pool_index: i64,
    pub set_index: i64,
    pub disk_index: i64,
}

/// `madmin.ServerProperties`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerProperties {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub scheme: String,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub uptime: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    #[serde(rename = "commitID", skip_serializing_if = "String::is_empty")]
    pub commit_id: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub network: BTreeMap<String, String>,
    #[serde(rename = "drives", skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<Disk>,
    #[serde(rename = "poolNumber", skip_serializing_if = "is_zero_i64")]
    pub pool_number: i64,
    #[serde(rename = "poolNumbers", skip_serializing_if = "Vec::is_empty")]
    pub pool_numbers: Vec<i64>,
    pub mem_stats: MemStats,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub go_max_procs: i64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub num_cpu: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub runtime_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gc_stats: Option<GcStats>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub minio_env_vars: BTreeMap<String, String>,
    pub edition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<LicenseInfo>,
    pub is_leader: bool,
    pub ilm_expiry_in_progress: bool,
}

/// `madmin.ErasureSetInfo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ErasureSetInfo {
    pub id: i64,
    #[serde(rename = "rawUsage")]
    pub raw_usage: u64,
    #[serde(rename = "rawCapacity")]
    pub raw_capacity: u64,
    pub usage: u64,
    #[serde(rename = "objectsCount")]
    pub objects_count: u64,
    #[serde(rename = "versionsCount")]
    pub versions_count: u64,
    #[serde(rename = "deleteMarkersCount")]
    pub delete_markers_count: u64,
    #[serde(rename = "healDisks")]
    pub heal_disks: i64,
}

/// `madmin.InfoMessage`. Go map keys (`pools` ints included) marshal sorted as strings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InfoMessage {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub domain: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub region: String,
    #[serde(rename = "sqsARN", skip_serializing_if = "Vec::is_empty")]
    pub sqs_arn: Vec<String>,
    #[serde(rename = "deploymentID", skip_serializing_if = "String::is_empty")]
    pub deployment_id: String,
    pub buckets: Count,
    pub objects: Count,
    pub versions: Count,
    #[serde(rename = "deletemarkers")]
    pub delete_markers: Count,
    pub usage: Usage,
    pub services: Services,
    pub backend: ErasureBackend,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerProperties>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub pools: BTreeMap<String, BTreeMap<String, ErasureSetInfo>>,
}

/// madmin `ServerInfo` (`GET info?metrics=false`).
pub async fn server_info(client: &AdminClient) -> Result<InfoMessage> {
    client
        .request("GET", "info")
        .query("metrics", "false")
        .send_json()
        .await
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// `madmin.HelpKV`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HelpKv {
    pub key: String,
    pub description: String,
    pub optional: bool,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "multipleTargets")]
    pub multiple_targets: bool,
}

/// `madmin.Help`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Help {
    #[serde(rename = "subSys")]
    pub sub_sys: String,
    pub description: String,
    #[serde(rename = "multipleTargets")]
    pub multiple_targets: bool,
    #[serde(rename = "keysHelp")]
    pub keys_help: Option<Vec<HelpKv>>,
}

/// madmin `HelpConfigKV`.
pub async fn help_config_kv(
    client: &AdminClient,
    sub_sys: &str,
    key: &str,
    env_only: bool,
) -> Result<Help> {
    let mut request = client.request("GET", "help-config-kv");
    if env_only {
        request = request.query("env", "");
    }
    request
        .query("key", key)
        .query("subSys", sub_sys)
        .send_json()
        .await
}

/// madmin `GetConfigKV` (decrypted text).
pub async fn get_config_kv(client: &AdminClient, key: &str) -> Result<Vec<u8>> {
    Ok(client
        .request("GET", "get-config-kv")
        .query("key", key)
        .decrypt()
        .send()
        .await?
        .body)
}

/// True when the server asks for a restart (`x-minio-config-applied` is not `true`).
fn needs_restart(response: &Response) -> bool {
    response.header("x-minio-config-applied").as_deref() != Some("true")
}

/// madmin `SetConfigKV`; returns whether a restart is needed.
pub async fn set_config_kv(client: &AdminClient, kv: &str) -> Result<bool> {
    let response = client
        .request("PUT", "set-config-kv")
        .body(kv.as_bytes().to_vec())
        .encrypted()?
        .send()
        .await?;
    Ok(needs_restart(&response))
}

/// madmin `DelConfigKV`; returns whether a restart is needed.
pub async fn del_config_kv(client: &AdminClient, kv: &str) -> Result<bool> {
    let response = client
        .request("DELETE", "del-config-kv")
        .body(kv.as_bytes().to_vec())
        .encrypted()?
        .send()
        .await?;
    Ok(needs_restart(&response))
}

/// madmin `GetConfig` (full config export, decrypted).
pub async fn get_config(client: &AdminClient) -> Result<Vec<u8>> {
    Ok(client.request("GET", "config").decrypt().send().await?.body)
}

/// Maximum config size madmin `SetConfig` accepts.
pub const MAX_CONFIG_SIZE: usize = 256 * 1024;

/// madmin `SetConfig`.
pub async fn set_config(client: &AdminClient, config: &[u8]) -> Result<()> {
    if config.len() > MAX_CONFIG_SIZE {
        bail!("bytes.Buffer: too large");
    }
    client
        .request("PUT", "config")
        .body(config.to_vec())
        .encrypted()?
        .send()
        .await?;
    Ok(())
}

/// `madmin.ConfigHistoryEntry`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConfigHistoryEntry {
    #[serde(rename = "restoreId")]
    pub restore_id: String,
    #[serde(rename = "createTime")]
    pub create_time: String,
    pub data: String,
}

/// madmin `ListConfigHistoryKV`.
pub async fn list_config_history(
    client: &AdminClient,
    count: i64,
) -> Result<Vec<ConfigHistoryEntry>> {
    let count = if count == 0 { 10 } else { count };
    let response = client
        .request("GET", "list-config-history-kv")
        .query("count", count.to_string())
        .decrypt()
        .send()
        .await?;
    let entries: Option<Vec<ConfigHistoryEntry>> = serde_json::from_slice(&response.body)?;
    Ok(entries.unwrap_or_default())
}

/// madmin `ClearConfigHistoryKV`.
pub async fn clear_config_history(client: &AdminClient, restore_id: &str) -> Result<()> {
    client
        .request("DELETE", "clear-config-history-kv")
        .query("restoreId", restore_id)
        .send()
        .await?;
    Ok(())
}

/// madmin `RestoreConfigHistoryKV`.
pub async fn restore_config_history(client: &AdminClient, restore_id: &str) -> Result<()> {
    client
        .request("PUT", "restore-config-history-kv")
        .query("restoreId", restore_id)
        .send()
        .await?;
    Ok(())
}

/// `madmin.EnvOverride`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnvOverride {
    pub name: String,
    pub value: String,
}

/// `madmin.ConfigKV`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigKv {
    pub key: String,
    pub value: String,
    #[serde(rename = "envOverride", skip_serializing_if = "Option::is_none")]
    pub env_override: Option<EnvOverride>,
}

/// `madmin.SubsysConfig` (`kv` is `null` without keys, like a nil Go slice).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SubsysConfig {
    #[serde(rename = "subSystem")]
    pub sub_system: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub target: String,
    pub kv: Option<Vec<ConfigKv>>,
}

impl SubsysConfig {
    /// madmin `AddConfigKV`: replaces an existing key in place.
    fn add(&mut self, kv: ConfigKv) {
        let list = self.kv.get_or_insert_with(Vec::new);
        match list.iter_mut().find(|existing| existing.key == kv.key) {
            Some(existing) => *existing = kv,
            None => list.push(kv),
        }
    }
}

const ENV_LINE_PREFIX: &str = "# MINIO_";
const DEFAULT_TARGET: &str = "_";

fn invalid_config_kv() -> anyhow::Error {
    anyhow::anyhow!("expected config value in the format `key=value`")
}

/// madmin `parseEnvVarLine`.
fn parse_env_var_line(line: &str, sub_system: &str, target: &str) -> Result<ConfigKv> {
    let line = line.strip_prefix("# ").unwrap_or(line);
    let Some((name, value)) = line.split_once('=') else {
        bail!("expected env var line of the form `# MINIO_...=...`");
    };
    let prefix = format!("MINIO_{}_", sub_system.to_uppercase());
    let Some(config_var) = name.strip_prefix(&prefix) else {
        bail!("expected env {name} to have prefix {prefix}");
    };
    let config_var = if target != DEFAULT_TARGET {
        config_var
            .strip_suffix(&format!("_{target}"))
            .unwrap_or(config_var)
    } else {
        config_var
    };
    Ok(ConfigKv {
        key: config_var.to_lowercase(),
        value: String::new(),
        env_override: Some(EnvOverride {
            name: name.to_string(),
            value: value.to_string(),
        }),
    })
}

/// madmin `parseConfigLine`: `subsys[:target] k=v k2="v 2"`.
fn parse_config_line(line: &str) -> Result<Vec<ConfigKv>> {
    let mut out: Vec<ConfigKv> = Vec::new();
    let Some((_, rest)) = line.split_once(' ') else {
        return Ok(out);
    };
    let mut text = rest.trim();
    while !text.is_empty() {
        let Some((key, rem)) = text.split_once('=') else {
            return Err(invalid_config_kv());
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(invalid_config_kv());
        }
        let (value, rem) = if let Some(quoted) = rem.strip_prefix('"') {
            let Some((value, rem)) = quoted.split_once('"') else {
                return Err(invalid_config_kv());
            };
            (value, rem.trim())
        } else {
            match rem.split_once(' ') {
                Some((value, rem)) => (value, rem.trim()),
                None => (rem, ""),
            }
        };
        let kv = ConfigKv {
            key: key.to_string(),
            value: value.to_string(),
            env_override: None,
        };
        match out.iter_mut().find(|existing| existing.key == kv.key) {
            Some(existing) => *existing = kv,
            None => out.push(kv),
        }
        text = rem;
    }
    Ok(out)
}

/// madmin `ParseServerConfigOutput`: groups env lines (`# MINIO_...`) with the config line
/// that follows them.
pub fn parse_server_config_output(output: &str) -> Result<Vec<SubsysConfig>> {
    let mut result = Vec::new();
    let mut group: Vec<&str> = Vec::new();
    for line in output.split('\n').map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with(ENV_LINE_PREFIX) {
            group.push(line);
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let head = line.split(' ').next().unwrap_or_default();
        let (sub_system, target) = head.split_once(':').unwrap_or((head, DEFAULT_TARGET));
        let mut config = SubsysConfig {
            sub_system: sub_system.to_string(),
            target: if target == DEFAULT_TARGET {
                String::new()
            } else {
                target.to_string()
            },
            kv: None,
        };
        for env_line in group.drain(..) {
            config.add(parse_env_var_line(env_line, sub_system, target)?);
        }
        for kv in parse_config_line(line)? {
            let existing = config
                .kv
                .as_mut()
                .and_then(|list| list.iter_mut().find(|e| e.key == kv.key));
            match existing {
                Some(existing) => existing.value = kv.value,
                None => config.add(kv),
            }
        }
        result.push(config);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// KMS (`/minio/kms/v1/...`)
// ---------------------------------------------------------------------------

const KMS_PREFIX: &str = "/minio/kms/v1";

/// `madmin.KMSKeyStatus`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct KmsKeyStatus {
    #[serde(rename = "key-id")]
    pub key_id: String,
    #[serde(rename = "encryption-error")]
    pub encryption_err: String,
    #[serde(rename = "decryption-error")]
    pub decryption_err: String,
}

/// `madmin.KMSKeyInfo`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct KmsKeyInfo {
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "createdBy")]
    pub created_by: String,
    pub name: String,
}

/// madmin `CreateKey`.
pub async fn kms_create_key(client: &AdminClient, key_id: &str) -> Result<()> {
    client
        .request_at("POST", &format!("{KMS_PREFIX}/key/create"))
        .query("key-id", key_id)
        .send()
        .await?;
    Ok(())
}

/// madmin `GetKeyStatus`.
pub async fn kms_key_status(client: &AdminClient, key_id: &str) -> Result<KmsKeyStatus> {
    client
        .request_at("GET", &format!("{KMS_PREFIX}/key/status"))
        .query("key-id", key_id)
        .send_json()
        .await
}

/// madmin `ListKeys`.
pub async fn kms_list_keys(client: &AdminClient, pattern: &str) -> Result<Vec<KmsKeyInfo>> {
    let keys: Option<Vec<KmsKeyInfo>> = client
        .request_at("GET", &format!("{KMS_PREFIX}/key/list"))
        .query("pattern", pattern)
        .send_json()
        .await?;
    Ok(keys.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// `madmin.BucketScanInfo` (no JSON tags: Go field names).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BucketScanInfo {
    #[serde(rename = "Pool")]
    pub pool: i64,
    #[serde(rename = "Set")]
    pub set: i64,
    #[serde(rename = "Cycle")]
    pub cycle: u64,
    #[serde(rename = "Ongoing")]
    pub ongoing: bool,
    #[serde(rename = "LastUpdate")]
    pub last_update: String,
    #[serde(rename = "LastStarted")]
    pub last_started: String,
    #[serde(rename = "Completed")]
    pub completed: Option<Vec<String>>,
}

impl Default for BucketScanInfo {
    fn default() -> Self {
        Self {
            pool: 0,
            set: 0,
            cycle: 0,
            ongoing: false,
            last_update: zero_time(),
            last_started: zero_time(),
            completed: None,
        }
    }
}

/// madmin `BucketScanInfo` (`GET scanner/status/<bucket>`).
pub async fn bucket_scan_info(client: &AdminClient, bucket: &str) -> Result<Vec<BucketScanInfo>> {
    let info: Option<Vec<BucketScanInfo>> = client
        .request("GET", &format!("scanner/status/{bucket}"))
        .send_json()
        .await?;
    Ok(info.unwrap_or_default())
}

/// `ScannerMetrics.LastMinute`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScannerLastMinute {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<String, TimedAction>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub ilm: BTreeMap<String, TimedAction>,
}

/// `madmin.ScannerMetrics`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScannerMetrics {
    #[serde(rename = "collected")]
    pub collected_at: String,
    pub current_cycle: u64,
    pub current_started: String,
    #[serde(rename = "cycle_complete_times")]
    pub cycles_completed_at: Option<Vec<String>>,
    pub ongoing_buckets: i64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub per_bucket_stats: BTreeMap<String, Vec<BucketScanInfo>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub life_time_ops: BTreeMap<String, u64>,
    #[serde(rename = "ilm_ops", skip_serializing_if = "BTreeMap::is_empty")]
    pub life_time_ilm: BTreeMap<String, u64>,
    pub last_minute: ScannerLastMinute,
    #[serde(rename = "active", skip_serializing_if = "Vec::is_empty")]
    pub active_paths: Vec<String>,
}

impl Default for ScannerMetrics {
    fn default() -> Self {
        Self {
            collected_at: zero_time(),
            current_cycle: 0,
            current_started: zero_time(),
            cycles_completed_at: None,
            ongoing_buckets: 0,
            per_bucket_stats: BTreeMap::new(),
            life_time_ops: BTreeMap::new(),
            life_time_ilm: BTreeMap::new(),
            last_minute: ScannerLastMinute::default(),
            active_paths: Vec::new(),
        }
    }
}

/// `madmin.Metrics` (mx requests scanner metrics only; other sections are kept as JSON).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Metrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanner: Option<ScannerMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<Value>,
    #[serde(rename = "batchJobs", skip_serializing_if = "Option::is_none")]
    pub batch_jobs: Option<Value>,
    #[serde(rename = "siteResync", skip_serializing_if = "Option::is_none")]
    pub site_resync: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub go: Option<Value>,
}

/// `madmin.RealtimeMetrics`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RealtimeMetrics {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    pub hosts: Option<Vec<String>>,
    pub aggregated: Metrics,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub by_host: BTreeMap<String, Metrics>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub by_disk: BTreeMap<String, Value>,
    #[serde(rename = "final")]
    pub is_final: bool,
}

/// madmin `MetricsScanner` type bit.
pub const METRICS_SCANNER: u32 = 1;

/// madmin `MetricsOptions` (the subset mc uses).
pub struct MetricsOptions<'a> {
    pub types: u32,
    pub n: i64,
    pub interval_secs: i64,
    pub hosts: &'a str,
}

/// madmin `Metrics`: streams `RealtimeMetrics` documents until `final` (a stream that ends
/// early is madmin's `unexpected EOF`).
pub async fn metrics<F>(client: &AdminClient, opts: &MetricsOptions<'_>, mut out: F) -> Result<()>
where
    F: FnMut(RealtimeMetrics) -> Result<()>,
{
    let mut stream = client
        .request("GET", "metrics")
        .query("disks", "")
        .query("hosts", opts.hosts)
        .query(
            "interval",
            go_duration_string(opts.interval_secs.saturating_mul(1_000_000_000)),
        )
        .query("n", opts.n.to_string())
        .query("types", opts.types.to_string())
        .stream()
        .await?;
    loop {
        match stream.next::<RealtimeMetrics>().await? {
            None => bail!("unexpected EOF"),
            Some(metrics) => {
                let done = metrics.is_final;
                out(metrics)?;
                if done {
                    return Ok(());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cluster metadata import/export
// ---------------------------------------------------------------------------

/// madmin `ExportBucketMetadata` (zip archive).
pub async fn export_bucket_metadata(client: &AdminClient, bucket: &str) -> Result<Vec<u8>> {
    Ok(client
        .request("GET", "export-bucket-metadata")
        .query("bucket", bucket)
        .send()
        .await?
        .body)
}

/// `madmin.MetaStatus`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MetaStatus {
    #[serde(rename = "isSet")]
    pub is_set: bool,
    #[serde(rename = "error", skip_serializing_if = "String::is_empty")]
    pub err: String,
}

/// `madmin.BucketStatus`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BucketStatus {
    #[serde(rename = "olock")]
    pub object_lock: MetaStatus,
    pub versioning: MetaStatus,
    pub policy: MetaStatus,
    pub tagging: MetaStatus,
    #[serde(rename = "sse")]
    pub sse_config: MetaStatus,
    pub lifecycle: MetaStatus,
    pub notification: MetaStatus,
    pub quota: MetaStatus,
    pub cors: MetaStatus,
    #[serde(rename = "error", skip_serializing_if = "String::is_empty")]
    pub err: String,
}

impl BucketStatus {
    /// True when any part (or the bucket itself) failed to import.
    pub fn has_error(&self) -> bool {
        [
            &self.object_lock,
            &self.versioning,
            &self.sse_config,
            &self.tagging,
            &self.lifecycle,
            &self.quota,
            &self.policy,
            &self.notification,
            &self.cors,
        ]
        .iter()
        .any(|status| !status.err.is_empty())
            || !self.err.is_empty()
    }
}

/// `madmin.BucketMetaImportErrs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BucketMetaImportErrs {
    pub buckets: Option<BTreeMap<String, BucketStatus>>,
}

/// madmin `ImportBucketMetadata`.
pub async fn import_bucket_metadata(
    client: &AdminClient,
    bucket: &str,
    content: Vec<u8>,
) -> Result<BucketMetaImportErrs> {
    client
        .request("PUT", "import-bucket-metadata")
        .query("bucket", bucket)
        .body(content)
        .send_json()
        .await
}

/// madmin `ExportIAM` (zip archive).
pub async fn export_iam(client: &AdminClient) -> Result<Vec<u8>> {
    Ok(client.request("GET", "export-iam").send().await?.body)
}

/// `madmin.IAMEntities`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IamEntities {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    #[serde(rename = "serviceAccounts", skip_serializing_if = "Vec::is_empty")]
    pub service_accounts: Vec<String>,
    #[serde(rename = "userPolicies", skip_serializing_if = "Vec::is_empty")]
    pub user_policies: Vec<BTreeMap<String, Vec<String>>>,
    #[serde(rename = "groupPolicies", skip_serializing_if = "Vec::is_empty")]
    pub group_policies: Vec<BTreeMap<String, Vec<String>>>,
    #[serde(rename = "stsPolicies", skip_serializing_if = "Vec::is_empty")]
    pub sts_policies: Vec<BTreeMap<String, Vec<String>>>,
}

/// `madmin.IAMErrEntity` / `IAMErrPolicyEntity` (a Go `error` value marshals as `{}`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IamErrEntity {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

/// `madmin.IAMErrEntities`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IamErrEntities {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<IamErrEntity>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<IamErrEntity>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<IamErrEntity>,
    #[serde(rename = "serviceAccounts", skip_serializing_if = "Vec::is_empty")]
    pub service_accounts: Vec<IamErrEntity>,
    #[serde(rename = "userPolicies", skip_serializing_if = "Vec::is_empty")]
    pub user_policies: Vec<IamErrEntity>,
    #[serde(rename = "groupPolicies", skip_serializing_if = "Vec::is_empty")]
    pub group_policies: Vec<IamErrEntity>,
    #[serde(rename = "stsPolicies", skip_serializing_if = "Vec::is_empty")]
    pub sts_policies: Vec<IamErrEntity>,
}

/// `madmin.ImportIAMResult`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportIamResult {
    pub skipped: IamEntities,
    pub removed: IamEntities,
    pub added: IamEntities,
    pub failed: IamErrEntities,
}

/// madmin `ImportIAMV2`.
pub async fn import_iam_v2(client: &AdminClient, content: Vec<u8>) -> Result<ImportIamResult> {
    client
        .request("PUT", "import-iam-v2")
        .body(content)
        .send_json()
        .await
}

/// madmin `ImportIAM` (older servers).
pub async fn import_iam(client: &AdminClient, content: Vec<u8>) -> Result<()> {
    client
        .request("PUT", "import-iam")
        .body(content)
        .send()
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Prometheus
// ---------------------------------------------------------------------------

/// Validity of generated Prometheus tokens (mc `defaultPrometheusJWTExpiry`, 100 years).
pub const PROMETHEUS_JWT_EXPIRY_SECS: i64 = 100 * 365 * 24 * 3600;

/// mc `getPrometheusToken`: HS512 JWT `{"iss":"prometheus","sub":ACCESS_KEY,"exp":...}`
/// signed with the secret key.
pub fn prometheus_token(access_key: &str, secret_key: &str, now_unix: i64) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = b64.encode(r#"{"alg":"HS512","typ":"JWT"}"#);
    // jwt `RegisteredClaims` field order: iss, sub, exp.
    let payload = format!(
        r#"{{"iss":"prometheus","sub":{},"exp":{}}}"#,
        Value::String(access_key.to_string()),
        now_unix + PROMETHEUS_JWT_EXPIRY_SECS
    );
    let signing_input = format!("{header}.{}", b64.encode(payload));
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA512, secret_key.as_bytes());
    let tag = aws_lc_rs::hmac::sign(&key, signing_input.as_bytes());
    format!("{signing_input}.{}", b64.encode(tag.as_ref()))
}

/// GET `path` with `Authorization: Bearer TOKEN` (unsigned, like mc's metrics client).
pub async fn fetch_metrics(client: &AdminClient, path: &str, token: &str) -> Result<Response> {
    let mut headers = Vec::new();
    if !token.is_empty() {
        headers.push(("authorization", format!("Bearer {token}")));
    }
    client.send_unsigned("GET", path, &headers).await
}

/// prom2json `Metric` / `Summary` / `Histogram` entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PromMetric {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub timestamp_ms: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantiles: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buckets: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// prom2json `Family`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PromFamily {
    pub name: String,
    pub help: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metrics: Vec<PromMetric>,
}

type Sample = (String, BTreeMap<String, String>, f64, String);

/// One sample line: `name{labels} value [timestamp]`.
fn parse_sample(line: &str) -> Result<Sample> {
    let name_end = line.find(['{', ' ', '\t']).unwrap_or(line.len());
    let name = line[..name_end].to_string();
    let mut rest = &line[name_end..];
    let mut labels = BTreeMap::new();
    if let Some(after) = rest.strip_prefix('{') {
        let mut chars = after.char_indices().peekable();
        let mut end = None;
        loop {
            while chars
                .peek()
                .is_some_and(|(_, c)| *c == ',' || c.is_whitespace())
            {
                chars.next();
            }
            let Some((start, c)) = chars.next() else {
                break;
            };
            if c == '}' {
                end = Some(start + 1);
                break;
            }
            let mut key = c.to_string();
            for (_, c) in chars.by_ref() {
                if c == '=' {
                    break;
                }
                key.push(c);
            }
            if chars.next().map(|(_, c)| c) != Some('"') {
                bail!("expected '\"' to start label value");
            }
            let mut value = String::new();
            while let Some((_, c)) = chars.next() {
                match c {
                    '\\' => match chars.next().map(|(_, c)| c) {
                        Some('n') => value.push('\n'),
                        Some(other) => value.push(other),
                        None => break,
                    },
                    '"' => break,
                    other => value.push(other),
                }
            }
            labels.insert(key.trim().to_string(), value);
        }
        let Some(end) = end else {
            bail!("unexpected end of label set");
        };
        rest = &after[end..];
    }
    let mut fields = rest.split_whitespace();
    let value = fields.next().unwrap_or_default();
    let value = match value {
        "+Inf" | "Inf" => f64::INFINITY,
        "-Inf" => f64::NEG_INFINITY,
        "NaN" => f64::NAN,
        other => other
            .parse::<f64>()
            .map_err(|_| anyhow::anyhow!("expected float as value, got {other:?}"))?,
    };
    let timestamp = fields.next().unwrap_or_default().to_string();
    Ok((name, labels, value, timestamp))
}

fn family_index(families: &mut Vec<PromFamily>, name: &str) -> usize {
    match families.iter().position(|f| f.name == name) {
        Some(index) => index,
        None => {
            families.push(PromFamily {
                name: name.to_string(),
                kind: "UNTYPED".to_string(),
                ..Default::default()
            });
            families.len() - 1
        }
    }
}

/// madmin `ParsePrometheusResults`: Prometheus text format to prom2json families (in the
/// order families first appear; mc's order comes from a Go map and is random).
pub fn parse_prometheus(text: &str) -> Result<Vec<PromFamily>> {
    let mut families: Vec<PromFamily> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix('#') {
            let mut parts = comment.trim_start().splitn(3, char::is_whitespace);
            let kind = parts.next().unwrap_or_default();
            let name = parts.next().unwrap_or_default();
            let rest = parts.next().unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            match kind {
                "HELP" => {
                    let index = family_index(&mut families, name);
                    families[index].help = rest.replace("\\n", "\n").replace("\\\\", "\\");
                }
                "TYPE" => {
                    let index = family_index(&mut families, name);
                    families[index].kind = rest.trim().to_uppercase();
                }
                _ => {}
            }
            continue;
        }
        let (name, mut labels, value, timestamp) = parse_sample(line)?;
        // Histogram / summary samples belong to their base family.
        let base = ["_bucket", "_sum", "_count"].iter().find_map(|suffix| {
            let base = name.strip_suffix(suffix)?;
            families
                .iter()
                .any(|f| f.name == base && (f.kind == "HISTOGRAM" || f.kind == "SUMMARY"))
                .then(|| (base.to_string(), *suffix))
        });
        let (family_name, suffix) = base.unwrap_or((name.clone(), ""));
        let index = family_index(&mut families, &family_name);
        let kind = families[index].kind.clone();
        let formatted = go_format_float(value);
        if kind != "HISTOGRAM" && kind != "SUMMARY" {
            families[index].metrics.push(PromMetric {
                labels,
                timestamp_ms: timestamp,
                value: Some(formatted),
                ..Default::default()
            });
            continue;
        }
        let bound = match suffix {
            "_bucket" => labels.remove("le"),
            "" => labels.remove("quantile"),
            _ => None,
        };
        let family = &mut families[index];
        let position = match family.metrics.iter().position(|m| m.labels == labels) {
            Some(position) => position,
            None => {
                family.metrics.push(PromMetric {
                    labels,
                    timestamp_ms: timestamp,
                    count: Some("0".to_string()),
                    sum: Some("0".to_string()),
                    ..Default::default()
                });
                family.metrics.len() - 1
            }
        };
        let metric = &mut family.metrics[position];
        match suffix {
            "_sum" => metric.sum = Some(formatted),
            "_count" => metric.count = Some(formatted),
            _ => {
                let map = if kind == "HISTOGRAM" {
                    metric.buckets.get_or_insert_with(BTreeMap::new)
                } else {
                    metric.quantiles.get_or_insert_with(BTreeMap::new)
                };
                if let Some(bound) = bound {
                    map.insert(bound, formatted);
                }
            }
        }
    }
    Ok(families)
}

/// Go `strconv.FormatFloat(v, 'g', -1, 64)` (`17`, `1.639184e+06`, `0.0001`, `1e-05`): the
/// shortest digits, `%e` when the exponent is < -4 or >= 6.
pub fn go_format_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "+Inf" } else { "-Inf" }.to_string();
    }
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    let sci = format!("{value:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or_default();
    if !(-4..6).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exp.abs());
    }
    format!("{value}")
}

// ---------------------------------------------------------------------------
// Go formatting helpers
// ---------------------------------------------------------------------------

/// Go `time.Duration.String()`.
pub fn go_duration_string(nanos: i64) -> String {
    if nanos == 0 {
        return "0s".to_string();
    }
    let sign = if nanos < 0 { "-" } else { "" };
    let u = nanos.unsigned_abs();
    let frac = |value: u64, digits: u32| -> String {
        let scale = 10u64.pow(digits);
        let (whole, rest) = (value / scale, value % scale);
        if rest == 0 {
            whole.to_string()
        } else {
            let rest = format!("{rest:0width$}", width = digits as usize);
            format!("{whole}.{}", rest.trim_end_matches('0'))
        }
    };
    if u < 1_000 {
        return format!("{sign}{u}ns");
    }
    if u < 1_000_000 {
        return format!("{sign}{}µs", frac(u, 3));
    }
    if u < 1_000_000_000 {
        return format!("{sign}{}ms", frac(u, 6));
    }
    let secs = u / 1_000_000_000;
    let sub = u % 1_000_000_000;
    let seconds = frac((secs % 60) * 1_000_000_000 + sub, 9);
    let (h, m) = (secs / 3600, secs / 60 % 60);
    if h > 0 {
        format!("{sign}{h}h{m}m{seconds}s")
    } else if secs >= 60 {
        format!("{sign}{m}m{seconds}s")
    } else {
        format!("{sign}{seconds}s")
    }
}

/// go-humanize `RelTime(a, b, albl, blbl)` for `diff = b - a` in seconds (this go-humanize
/// version keeps the space before an empty label: `7 seconds `).
pub fn rel_time(diff_secs: i64, albl: &str, blbl: &str) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 12 * MONTH;
    let label = if diff_secs < 0 { blbl } else { albl };
    let diff = diff_secs.abs();
    let (format, divisor): (&str, i64) = match diff {
        d if d < 1 => return "now".to_string(),
        d if d < 2 => ("1 second", 0),
        d if d < MINUTE => ("{} seconds", 1),
        d if d < 2 * MINUTE => ("1 minute", 0),
        d if d < HOUR => ("{} minutes", MINUTE),
        d if d < 2 * HOUR => ("1 hour", 0),
        d if d < DAY => ("{} hours", HOUR),
        d if d < 2 * DAY => ("1 day", 0),
        d if d < WEEK => ("{} days", DAY),
        d if d < 2 * WEEK => ("1 week", 0),
        d if d < MONTH => ("{} weeks", WEEK),
        d if d < 2 * MONTH => ("1 month", 0),
        d if d < YEAR => ("{} months", MONTH),
        d if d < 18 * MONTH => ("1 year", 0),
        d if d < 2 * YEAR => ("2 years", 0),
        d if d < 37 * YEAR => ("{} years", YEAR),
        _ => ("a long while", 0),
    };
    let text = if divisor > 0 {
        format.replace("{}", &(diff / divisor).to_string())
    } else {
        format.to_string()
    };
    format!("{text} {label}")
}

/// go-humanize `Ordinal` (`1st`, `2nd`, `11th`, `23rd`).
pub fn ordinal(n: i64) -> String {
    let suffix = match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// go-humanize `english.Plural(n, singular, "")` for the words mc uses (`1 drive`,
/// `0 drives`, `2 Delete Markers`).
pub fn plural(n: i64, singular: &str) -> String {
    if n == 1 {
        return format!("{n} {singular}");
    }
    format!("{n} {singular}s")
}

/// Seconds since the Unix epoch of an RFC3339 time.
pub fn unix_seconds(time: &str) -> Option<i64> {
    aws_smithy_types::DateTime::from_str(
        time,
        aws_smithy_types::date_time::Format::DateTimeWithOffset,
    )
    .ok()
    .map(|t| t.secs())
}

/// Go `t.Format(http.TimeFormat)` (`Mon, 02 Jan 2006 15:04:05 GMT`) of an RFC3339 time.
pub fn http_time(time: &str) -> String {
    aws_smithy_types::DateTime::from_str(
        time,
        aws_smithy_types::date_time::Format::DateTimeWithOffset,
    )
    .ok()
    .and_then(|t| t.fmt(aws_smithy_types::date_time::Format::HttpDate).ok())
    .unwrap_or_else(|| "Mon, 01 Jan 0001 00:00:00 GMT".to_string())
}

/// Current Unix time in seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Client setup
// ---------------------------------------------------------------------------

/// mc `newAdminClient(aliasedURL)` + `fatalIf(err, message)`: like
/// [`super::admin::admin_client_for`], with the command's own failure message.
pub fn admin_client(target: &str, message: &str) -> Result<AdminClient> {
    let store = crate::config::ConfigStore::load_or_create()?;
    super::admin::admin_client_for(&store, target).map_err(|err| {
        let cause = match crate::error::mc_error(&err) {
            Some(cause) => anyhow::Error::new(cause.clone()),
            None => anyhow::anyhow!(crate::output::split_error(&err).1),
        };
        cause.context(message.to_string())
    })
}

/// Output of mc messages built with a (colorjson) JSON encoder and one-space indent. mc
/// compacts them only when `--json` is given after the command itself (`globalJSONLine` is
/// false for a leading global `--json`); `commands` are the command's names.
pub fn encoder_json<T: Serialize>(value: &T, commands: &[&str]) -> Result<String> {
    let argv: Vec<String> = std::env::args().collect();
    let command = argv.iter().position(|a| commands.contains(&a.as_str()));
    let flag = argv
        .iter()
        .rposition(|a| a == "--json" || a == "-json" || a.starts_with("--json="));
    let after = matches!((command, flag), (Some(c), Some(f)) if f > c);
    if after && !crate::output::stdout_is_terminal() {
        return Ok(serde_json::to_string(value)?);
    }
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut serializer)?;
    Ok(String::from_utf8(buf)?)
}

/// Go `syscall.Errno` text of an I/O error (`no such device or address`).
pub fn go_errno_text(err: &std::io::Error) -> String {
    let text = err.to_string();
    let text = match text.find(" (os error") {
        Some(index) => &text[..index],
        None => &text,
    };
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// bubbletea's terminal requirement: stdin must be a terminal or `/dev/tty` must open
/// (`could not open a new TTY: open /dev/tty: ...` otherwise).
pub fn require_tty() -> Result<()> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return Ok(());
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map(|_| ())
        .map_err(|err| {
            anyhow::anyhow!(
                "could not open a new TTY: open /dev/tty: {}",
                go_errno_text(&err)
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_floats_like_go_g() {
        assert_eq!(go_format_float(17.0), "17");
        assert_eq!(go_format_float(1_639_184.0), "1.639184e+06");
        assert_eq!(go_format_float(100_000.0), "100000");
        assert_eq!(go_format_float(1e6), "1e+06");
        assert_eq!(go_format_float(0.0001), "0.0001");
        assert_eq!(go_format_float(0.00001), "1e-05");
        assert_eq!(go_format_float(89.65), "89.65");
        assert_eq!(go_format_float(1.5e21), "1.5e+21");
        assert_eq!(go_format_float(12_345_678.0), "1.2345678e+07");
        assert_eq!(go_format_float(f64::INFINITY), "+Inf");
        assert_eq!(go_format_float(0.0), "0");
    }

    #[test]
    fn go_helpers_match_go() {
        assert_eq!(go_duration_string(913_913), "913.913µs");
        assert_eq!(go_duration_string(3_000_000_000), "3s");
        assert_eq!(go_duration_string(90_500_000_000), "1m30.5s");
        assert_eq!(rel_time(7, "", ""), "7 seconds ");
        assert_eq!(rel_time(-7200, "", "ago"), "2 hours ago");
        assert_eq!(rel_time(0, "", ""), "now");
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(23), "23rd");
        assert_eq!(plural(1, "drive"), "1 drive");
        assert_eq!(plural(0, "drive"), "0 drives");
        assert_eq!(plural(2, "Delete Marker"), "2 Delete Markers");
        assert_eq!(
            http_time("2026-09-26T19:27:36.123Z"),
            "Sat, 26 Sep 2026 19:27:36 GMT"
        );
        assert_eq!(unix_seconds("1970-01-01T00:01:00Z"), Some(60));
    }

    #[test]
    fn prometheus_token_matches_mc() {
        // Token printed by mc for minioadmin/minioadmin with `exp` 4944051017.
        let token = prometheus_token(
            "minioadmin",
            "minioadmin",
            4_944_051_017 - PROMETHEUS_JWT_EXPIRY_SECS,
        );
        assert_eq!(
            token,
            "eyJhbGciOiJIUzUxMiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJwcm9tZXRoZXVzIiwic3ViIjoibWluaW9hZG1pbiIsImV4cCI6NDk0NDA1MTAxN30.QATUdctQbd2wL8pwx_wYESlsSzgAZlokUm7t8AsYakwitIjk6Tp-nPu2FhMhMnbILXd3BT-Av5aHP5YGhCxOrQ"
        );
    }

    #[test]
    fn parses_prometheus_text() {
        let text = "# HELP a_total Total \\\\ things\n# TYPE a_total counter\na_total{server=\"x\",api=\"get\"} 3\na_total{server=\"y\"} 1.5e+06\n# TYPE h histogram\nh_bucket{le=\"1\"} 2\nh_bucket{le=\"+Inf\"} 3\nh_sum 4.5\nh_count 3\nplain 7 1700000000000\n";
        let families = parse_prometheus(text).unwrap();
        assert_eq!(families.len(), 3);
        assert_eq!(
            serde_json::to_string(&families[0]).unwrap(),
            r#"{"name":"a_total","help":"Total \\ things","type":"COUNTER","metrics":[{"labels":{"api":"get","server":"x"},"value":"3"},{"labels":{"server":"y"},"value":"1.5e+06"}]}"#
        );
        assert_eq!(
            serde_json::to_string(&families[1]).unwrap(),
            r#"{"name":"h","help":"","type":"HISTOGRAM","metrics":[{"buckets":{"+Inf":"3","1":"2"},"count":"3","sum":"4.5"}]}"#
        );
        assert_eq!(
            serde_json::to_string(&families[2]).unwrap(),
            r#"{"name":"plain","help":"","type":"UNTYPED","metrics":[{"timestamp_ms":"1700000000000","value":"7"}]}"#
        );
        assert!(parse_prometheus("x{a=\"b\" 1").is_err());
    }

    #[test]
    fn parses_server_config_output() {
        let text = "# MINIO_REGION_NAME=us-east-9\nregion name=\n# comment line\nnotify_webhook:1 endpoint=\"http://x y\" queue_limit=0 \n";
        let parsed = parse_server_config_output(text).unwrap();
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            r#"[{"subSystem":"region","kv":[{"key":"name","value":"","envOverride":{"name":"MINIO_REGION_NAME","value":"us-east-9"}}]},{"subSystem":"notify_webhook","target":"1","kv":[{"key":"endpoint","value":"http://x y"},{"key":"queue_limit","value":"0"}]}]"#
        );
        assert_eq!(
            serde_json::to_string(&parse_server_config_output("site").unwrap()).unwrap(),
            r#"[{"subSystem":"site","kv":null}]"#
        );
        assert!(parse_server_config_output("site name=\"x").is_err());
    }

    #[test]
    fn info_message_zero_value_marshals_like_go() {
        assert_eq!(
            serde_json::to_string(&InfoMessage::default()).unwrap(),
            r#"{"buckets":{"count":0},"objects":{"count":0},"versions":{"count":0},"deletemarkers":{"count":0},"usage":{"size":0},"services":{"kms":{},"ldap":{}},"backend":{"backendType":"","onlineDisks":0,"offlineDisks":0,"standardSCParity":0,"rrSCParity":0,"totalSets":null,"totalDrivesPerSet":null}}"#
        );
    }

    #[test]
    fn info_message_round_trips_server_json() {
        let text = r#"{"mode":"online","deploymentID":"d","buckets":{"count":0},"objects":{"count":0},"versions":{"count":0},"deletemarkers":{"count":0},"usage":{"size":0},"services":{"kms":{},"kmsStatus":[{"status":"online","endpoint":"127.0.0.1"}],"ldap":{}},"backend":{"backendType":"Erasure","onlineDisks":1,"offlineDisks":0,"standardSCParity":0,"rrSCParity":0,"totalSets":[1],"totalDrivesPerSet":[1]},"servers":[{"state":"online","endpoint":"127.0.0.1:9000","uptime":7,"version":"v","commitID":"c","network":{"127.0.0.1:9000":"online"},"drives":[{"endpoint":"/data","path":"/data","state":"ok","uuid":"u","major":259,"minor":2,"totalspace":10,"usedspace":5,"availspace":5,"metrics":{},"used_inodes":1,"free_inodes":2,"local":true,"pool_index":0,"set_index":0,"disk_index":0}],"poolNumber":1,"poolNumbers":[1],"mem_stats":{"Alloc":1,"TotalAlloc":2,"Mallocs":3,"Frees":4,"HeapAlloc":5},"go_max_procs":12,"num_cpu":12,"runtime_version":"go1","gc_stats":{"last_gc":"2026-09-26T19:24:08.137299585Z","num_gc":7,"pause_total":5,"pause":[1,2],"pause_end":["2026-09-26T19:24:08.083815355Z"]},"minio_env_vars":{"MINIO_ROOT_USER":"x"},"edition":"","is_leader":false,"ilm_expiry_in_progress":false}],"pools":{"0":{"0":{"id":0,"rawUsage":1,"rawCapacity":2,"usage":0,"objectsCount":0,"versionsCount":0,"deleteMarkersCount":0,"healDisks":0}}}}"#;
        let info: InfoMessage = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&info).unwrap(), text);
    }

    #[test]
    fn realtime_metrics_round_trip_mc_output() {
        let text = r#"{"hosts":["127.0.0.1:9000"],"aggregated":{"scanner":{"collected":"2026-09-26T19:25:59.038470293Z","current_cycle":0,"current_started":"0001-01-01T00:00:00Z","cycle_complete_times":["2026-09-26T19:25:08.106279743Z"],"ongoing_buckets":0,"life_time_ops":{"ScanCycle":1},"last_minute":{}}},"final":true}"#;
        let metrics: RealtimeMetrics = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&metrics).unwrap(), text);
    }
}
