//! MinIO admin API: trace, logs, heal (STREAM area), madmin `ServiceTrace` / `GetLogs` /
//! `Heal` / `BackgroundHealStatus`. Streams use `AdminRequest::stream`.
//!
//! Types mirror madmin-go (v3) field by field so `--json` output re-marshals like mc does.

use super::admin::{AdminClient, JsonStream};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Go's zero `time.Time` as JSON.
pub const ZERO_TIME: &str = "0001-01-01T00:00:00Z";

fn zero_time() -> String {
    ZERO_TIME.to_string()
}

// ---------------------------------------------------------------------------
// trace
// ---------------------------------------------------------------------------

/// madmin `TraceType` bits, in declaration order (bit `i` = `TRACE_TYPES[i]`).
pub const TRACE_TYPES: &[&str] = &[
    "OS",
    "Storage",
    "S3",
    "Internal",
    "Scanner",
    "Decommission",
    "Healing",
    "BatchReplication",
    "BatchKeyRotation",
    "BatchExpire",
    "Rebalance",
    "ReplicationResync",
    "Bootstrap",
    "FTP",
    "ILM",
    "KMS",
    "Formatting",
    "Admin",
    "Object",
    "Replication",
    "IAM",
];

pub const TRACE_OS: u64 = 1;
pub const TRACE_STORAGE: u64 = 1 << 1;
pub const TRACE_S3: u64 = 1 << 2;
pub const TRACE_INTERNAL: u64 = 1 << 3;
pub const TRACE_BOOTSTRAP: u64 = 1 << 12;

/// madmin `TraceType.String()` (`S3`, `Scanner`, `TraceType(N)` for unknown values).
pub fn trace_type_name(value: u64) -> String {
    if value == (1 << TRACE_TYPES.len()) - 1 {
        return "All".to_string();
    }
    if value.is_power_of_two() {
        let bit = value.trailing_zeros() as usize;
        if let Some(name) = TRACE_TYPES.get(bit) {
            return name.to_string();
        }
    }
    format!("TraceType({value})")
}

/// Inverse of [`trace_type_name`] (case-insensitive, `0` when unknown), madmin `FindTraceType`.
pub fn find_trace_type(name: &str) -> u64 {
    TRACE_TYPES
        .iter()
        .position(|t| t.eq_ignore_ascii_case(name))
        .map(|bit| 1 << bit)
        .unwrap_or(0)
}

/// madmin `ServiceTraceOpts`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TraceOpts {
    pub s3: bool,
    pub internal: bool,
    pub storage: bool,
    pub os: bool,
    pub scanner: bool,
    pub decommission: bool,
    pub healing: bool,
    pub batch_replication: bool,
    pub batch_key_rotation: bool,
    pub batch_expire: bool,
    pub rebalance: bool,
    pub replication_resync: bool,
    pub bootstrap: bool,
    pub ftp: bool,
    pub ilm: bool,
    pub kms: bool,
    pub formatting: bool,
    pub only_errors: bool,
    /// `threshold` in nanoseconds.
    pub threshold: i64,
}

impl TraceOpts {
    /// Query parameters as madmin `AddParams` sets them (the request encodes them sorted by
    /// key, like Go `url.Values.Encode`).
    pub fn params(&self) -> Vec<(&'static str, String)> {
        let b = |v: bool| v.to_string();
        vec![
            ("err", b(self.only_errors)),
            ("threshold", go_duration(self.threshold)),
            ("s3", b(self.s3)),
            ("internal", b(self.internal)),
            ("storage", b(self.storage)),
            ("os", b(self.os)),
            ("scanner", b(self.scanner)),
            ("decommission", b(self.decommission)),
            ("healing", b(self.healing)),
            ("batch-replication", b(self.batch_replication)),
            ("batch-keyrotation", b(self.batch_key_rotation)),
            ("batch-expire", b(self.batch_expire)),
            ("rebalance", b(self.rebalance)),
            ("replication-resync", b(self.replication_resync)),
            ("bootstrap", b(self.bootstrap)),
            ("ftp", b(self.ftp)),
            ("ilm", b(self.ilm)),
            ("kms", b(self.kms)),
            ("formatting", b(self.formatting)),
        ]
    }
}

/// madmin `TraceInfo` (with the pre-July-2022 `traceInfoLegacy` fields).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TraceInfo {
    #[serde(rename = "type", default)]
    pub trace_type: u64,
    #[serde(rename = "nodename", default)]
    pub node_name: String,
    #[serde(rename = "funcname", default)]
    pub func_name: String,
    #[serde(default = "zero_time")]
    pub time: String,
    #[serde(default)]
    pub path: String,
    /// Nanoseconds.
    #[serde(rename = "dur", default)]
    pub duration: i64,
    #[serde(default)]
    pub bytes: i64,
    #[serde(rename = "msg", default)]
    pub message: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub custom: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub http: Option<TraceHttpStats>,
    #[serde(rename = "healResult", default)]
    pub heal_result: Option<HealResultItem>,

    // Legacy (servers before July 2022).
    #[serde(rename = "request", default)]
    pub legacy_request: Option<TraceRequestInfo>,
    #[serde(rename = "response", default)]
    pub legacy_response: Option<TraceResponseInfo>,
    #[serde(rename = "stats", default)]
    pub legacy_stats: Option<TraceCallStats>,
    #[serde(rename = "storageStats", default)]
    pub storage_stats: Option<LegacyPathStats>,
    #[serde(rename = "osStats", default)]
    pub os_stats: Option<LegacyPathStats>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacyPathStats {
    #[serde(default)]
    path: String,
    #[serde(default)]
    duration: i64,
}

impl TraceInfo {
    /// madmin `ServiceTrace` conversion of legacy records.
    pub fn normalize(mut self) -> Self {
        if self.trace_type == 0 {
            self.trace_type = if self.func_name.starts_with("s3.") {
                TRACE_S3
            } else {
                TRACE_INTERNAL
            };
            let mut http = TraceHttpStats::default();
            if let Some(request) = self.legacy_request.take() {
                self.path = request.path.clone();
                http.request = request;
            }
            if let Some(response) = self.legacy_response.take() {
                http.response = response;
            }
            if let Some(stats) = self.legacy_stats.take() {
                self.duration = stats.latency;
                http.stats = stats;
            }
            self.http = Some(http);
        }
        if self.trace_type == TRACE_OS
            && let Some(stats) = self.os_stats.take()
        {
            self.path = stats.path;
            self.duration = stats.duration;
        }
        if self.trace_type == TRACE_STORAGE
            && let Some(stats) = self.storage_stats.take()
        {
            self.path = stats.path;
            self.duration = stats.duration;
        }
        self
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TraceHttpStats {
    #[serde(default)]
    pub request: TraceRequestInfo,
    #[serde(default)]
    pub response: TraceResponseInfo,
    #[serde(default)]
    pub stats: TraceCallStats,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TraceCallStats {
    #[serde(rename = "inputbytes", default)]
    pub input_bytes: i64,
    #[serde(rename = "outputbytes", default)]
    pub output_bytes: i64,
    #[serde(default)]
    pub latency: i64,
    #[serde(rename = "timetofirstbyte", default)]
    pub time_to_first_byte: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TraceRequestInfo {
    #[serde(default = "zero_time")]
    pub time: String,
    #[serde(default)]
    pub proto: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub path: String,
    #[serde(rename = "rawquery", default)]
    pub raw_query: String,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default, deserialize_with = "go_bytes")]
    pub body: Vec<u8>,
    #[serde(default)]
    pub client: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TraceResponseInfo {
    #[serde(default = "zero_time")]
    pub time: String,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default, deserialize_with = "go_bytes")]
    pub body: Vec<u8>,
    #[serde(rename = "statuscode", default)]
    pub status_code: i64,
}

/// Go `[]byte` JSON (base64 string or null).
fn go_bytes<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    use base64::Engine;
    let text = Option::<String>::deserialize(deserializer)?.unwrap_or_default();
    base64::engine::general_purpose::STANDARD
        .decode(text.as_bytes())
        .map_err(serde::de::Error::custom)
}

/// Opens madmin `ServiceTrace` (`GET /minio/admin/v3/trace`); read [`TraceInfo`] documents.
pub async fn trace(client: &AdminClient, opts: &TraceOpts) -> Result<JsonStream> {
    let params = opts.params();
    let query: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    client
        .request("GET", "trace")
        .queries(&query)
        .stream()
        .await
}

// ---------------------------------------------------------------------------
// logs
// ---------------------------------------------------------------------------

/// madmin `LogInfo` (embedded `logEntry` fields, then `ConsoleMsg` and `node`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LogInfo {
    #[serde(
        rename = "deploymentid",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub deployment_id: String,
    #[serde(default)]
    pub level: String,
    #[serde(rename = "errKind", default)]
    pub log_kind: String,
    #[serde(default)]
    pub time: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<LogApi>,
    #[serde(
        rename = "remotehost",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub remote_host: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    #[serde(
        rename = "requestID",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub request_id: String,
    #[serde(
        rename = "userAgent",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub user_agent: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(rename = "error", default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<LogTrace>,
    #[serde(rename = "ConsoleMsg", default)]
    pub console_msg: String,
    #[serde(rename = "node", default)]
    pub node_name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LogApi {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<LogArgs>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LogArgs {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bucket: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub object: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LogTrace {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<BTreeMap<String, serde_json::Value>>,
}

/// Opens madmin `GetLogs` (`GET /minio/admin/v3/log?node=&limit=&logType=`).
pub async fn logs(
    client: &AdminClient,
    node: &str,
    limit: i64,
    log_type: &str,
) -> Result<JsonStream> {
    client
        .request("GET", "log")
        .query("node", node)
        .query("limit", limit.to_string())
        .query("logType", log_type)
        .stream()
        .await
}

// ---------------------------------------------------------------------------
// heal
// ---------------------------------------------------------------------------

/// madmin `HealOpts` (request body).
#[derive(Debug, Clone, Default, Serialize)]
pub struct HealOpts {
    pub recursive: bool,
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    pub remove: bool,
    pub recreate: bool,
    /// 1 = normal, 2 = deep.
    #[serde(rename = "scanMode")]
    pub scan_mode: i64,
    #[serde(rename = "updateParity")]
    pub update_parity: bool,
    #[serde(rename = "nolock")]
    pub no_lock: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<i64>,
}

/// madmin `HealStartSuccess`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HealStartSuccess {
    #[serde(rename = "clientToken", default)]
    pub client_token: String,
    #[serde(rename = "clientAddress", default)]
    pub client_address: String,
    #[serde(rename = "startTime", default = "zero_time")]
    pub start_time: String,
}

/// madmin `HealTaskStatus` (the server marshals its own struct with capitalized keys; Go
/// decodes keys case-insensitively).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HealTaskStatus {
    #[serde(default, alias = "Summary")]
    pub summary: String,
    #[serde(rename = "detail", alias = "Detail", default)]
    pub failure_detail: String,
    #[serde(rename = "startTime", alias = "StartTime", default = "zero_time")]
    pub start_time: String,
    #[serde(rename = "settings", alias = "Settings", default)]
    pub heal_settings: serde_json::Value,
    #[serde(
        default,
        alias = "Items",
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub items: Vec<HealResultItem>,
}

/// JSON `null` as the type's default (Go nil slices).
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// madmin `HealDriveInfo`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HealDriveInfo {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HealDrives {
    #[serde(default)]
    pub drives: Option<Vec<HealDriveInfo>>,
}

impl HealDrives {
    pub fn list(&self) -> &[HealDriveInfo] {
        self.drives.as_deref().unwrap_or_default()
    }

    /// Drives in `state` (madmin `Get*Counts`).
    pub fn count(&self, state: &str) -> usize {
        self.list().iter().filter(|d| d.state == state).count()
    }
}

/// madmin `HealResultItem`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HealResultItem {
    #[serde(rename = "resultId", default)]
    pub result_index: i64,
    #[serde(rename = "type", default)]
    pub item_type: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub object: String,
    #[serde(rename = "versionId", default)]
    pub version_id: String,
    #[serde(default)]
    pub detail: String,
    #[serde(rename = "parityBlocks", default, skip_serializing_if = "is_zero")]
    pub parity_blocks: i64,
    #[serde(rename = "dataBlocks", default, skip_serializing_if = "is_zero")]
    pub data_blocks: i64,
    #[serde(rename = "diskCount", default)]
    pub disk_count: i64,
    #[serde(rename = "setCount", default)]
    pub set_count: i64,
    #[serde(default)]
    pub before: HealDrives,
    #[serde(default)]
    pub after: HealDrives,
    #[serde(rename = "objectSize", default)]
    pub object_size: i64,
}

fn is_zero<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Result of [`heal`]: a start (no client token) or a status request.
#[derive(Debug, Clone)]
pub enum HealReply {
    Started(HealStartSuccess),
    Status(HealTaskStatus),
}

/// madmin `Heal`: `POST /minio/admin/v3/heal/<bucket>[/<prefix>]`. With `client_token` it
/// fetches the sequence status (empty body), else it starts (or `force_stop`s) a sequence.
pub async fn heal(
    client: &AdminClient,
    bucket: &str,
    prefix: &str,
    opts: &HealOpts,
    client_token: &str,
    force_start: bool,
    force_stop: bool,
) -> Result<HealReply> {
    let mut api = format!("heal/{bucket}");
    if !bucket.is_empty() && !prefix.is_empty() {
        api = format!("{api}/{prefix}");
    }
    let mut request = client.request("POST", &api);
    if client_token.is_empty() {
        request = request.json(opts)?;
    } else {
        request = request.query("clientToken", client_token);
    }
    if force_start {
        request = request.query("forceStart", "true");
    } else if force_stop {
        request = request.query("forceStop", "true");
    }
    let response = request.send().await?;
    let parsed = if client_token.is_empty() {
        serde_json::from_slice(&response.body).map(HealReply::Started)
    } else {
        serde_json::from_slice(&response.body).map(HealReply::Status)
    };
    match parsed {
        Ok(reply) => Ok(reply),
        // The server may answer an error document after a success status.
        Err(err) => match serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(
            &response.body,
        ) {
            Ok(_) => Err(super::admin::madmin_error(&response).into()),
            Err(_) => Err(err.into()),
        },
    }
}

/// madmin `BgHealState`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BgHealState {
    #[serde(rename = "offline_nodes", default)]
    pub offline_endpoints: Option<Vec<String>>,
    #[serde(rename = "ScannedItemsCount", default)]
    pub scanned_items_count: i64,
    #[serde(rename = "HealDisks", default)]
    pub heal_disks: Option<Vec<String>>,
    #[serde(default)]
    pub sets: Option<Vec<SetStatus>>,
    #[serde(default)]
    pub mrf: Option<BTreeMap<String, MrfStatus>>,
    #[serde(default)]
    pub sc_parity: Option<BTreeMap<String, i64>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MrfStatus {
    #[serde(default)]
    pub bytes_healed: u64,
    #[serde(default)]
    pub items_healed: u64,
}

/// madmin `SetStatus`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SetStatus {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub pool_index: i64,
    #[serde(default)]
    pub set_index: i64,
    #[serde(default)]
    pub heal_status: String,
    #[serde(default)]
    pub heal_priority: String,
    #[serde(default)]
    pub total_objects: i64,
    #[serde(default)]
    pub disks: Option<Vec<Disk>>,
}

/// madmin `Disk`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Disk {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    #[serde(rename = "rootDisk", default, skip_serializing_if = "is_zero")]
    pub root_disk: bool,
    #[serde(rename = "path", default, skip_serializing_if = "String::is_empty")]
    pub drive_path: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub healing: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub scanning: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uuid: String,
    #[serde(default)]
    pub major: u32,
    #[serde(default)]
    pub minor: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(rename = "totalspace", default, skip_serializing_if = "is_zero")]
    pub total_space: u64,
    #[serde(rename = "usedspace", default, skip_serializing_if = "is_zero")]
    pub used_space: u64,
    #[serde(rename = "availspace", default, skip_serializing_if = "is_zero")]
    pub available_space: u64,
    #[serde(
        rename = "readthroughput",
        default,
        skip_serializing_if = "is_zero",
        serialize_with = "super::admin::go_f64"
    )]
    pub read_throughput: f64,
    #[serde(
        rename = "writethroughput",
        default,
        skip_serializing_if = "is_zero",
        serialize_with = "super::admin::go_f64"
    )]
    pub write_throughput: f64,
    #[serde(
        rename = "readlatency",
        default,
        skip_serializing_if = "is_zero",
        serialize_with = "super::admin::go_f64"
    )]
    pub read_latency: f64,
    #[serde(
        rename = "writelatency",
        default,
        skip_serializing_if = "is_zero",
        serialize_with = "super::admin::go_f64"
    )]
    pub write_latency: f64,
    #[serde(
        default,
        skip_serializing_if = "is_zero",
        serialize_with = "super::admin::go_f64"
    )]
    pub utilization: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<DiskMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heal_info: Option<HealingDisk>,
    #[serde(default)]
    pub used_inodes: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub free_inodes: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub local: bool,
    #[serde(
        rename = "cacheStats",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cache: Option<CacheStats>,
    #[serde(default)]
    pub pool_index: i64,
    #[serde(default)]
    pub set_index: i64,
    #[serde(default)]
    pub disk_index: i64,
}

/// madmin `DiskMetrics`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DiskMetrics {
    #[serde(
        rename = "lastMinute",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_minute: Option<BTreeMap<String, TimedAction>>,
    #[serde(rename = "apiCalls", default, skip_serializing_if = "Option::is_none")]
    pub api_calls: Option<BTreeMap<String, u64>>,
    #[serde(rename = "totalTokens", default, skip_serializing_if = "is_zero")]
    pub total_tokens: u32,
    #[serde(rename = "totalWaiting", default, skip_serializing_if = "is_zero")]
    pub total_waiting: u32,
    #[serde(
        rename = "totalErrorsAvailability",
        default,
        skip_serializing_if = "is_zero"
    )]
    pub total_errors_availability: u64,
    #[serde(
        rename = "totalErrorsTimeout",
        default,
        skip_serializing_if = "is_zero"
    )]
    pub total_errors_timeout: u64,
    #[serde(rename = "totalWrites", default, skip_serializing_if = "is_zero")]
    pub total_writes: u64,
    #[serde(rename = "totalDeletes", default, skip_serializing_if = "is_zero")]
    pub total_deletes: u64,
    #[serde(
        rename = "apiLatencies",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub api_latencies: Option<BTreeMap<String, serde_json::Value>>,
}

/// madmin `TimedAction`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TimedAction {
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub acc_time_ns: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub min_ns: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max_ns: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bytes: u64,
}

/// madmin `CacheStats`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CacheStats {
    #[serde(default)]
    pub capacity: i64,
    #[serde(default)]
    pub used: i64,
    #[serde(default)]
    pub hits: i64,
    #[serde(default)]
    pub misses: i64,
    #[serde(rename = "delHits", default)]
    pub del_hits: i64,
    #[serde(rename = "delMisses", default)]
    pub del_misses: i64,
    #[serde(default)]
    pub collisions: i64,
}

/// madmin `HealingDisk`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HealingDisk {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub heal_id: String,
    #[serde(default)]
    pub pool_index: i64,
    #[serde(default)]
    pub set_index: i64,
    #[serde(default)]
    pub disk_index: i64,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub path: String,
    #[serde(default = "zero_time")]
    pub started: String,
    #[serde(default = "zero_time")]
    pub last_update: String,
    #[serde(default)]
    pub retry_attempts: u64,
    #[serde(default)]
    pub objects_total_count: u64,
    #[serde(default)]
    pub objects_total_size: u64,
    #[serde(default)]
    pub items_healed: u64,
    #[serde(default)]
    pub items_failed: u64,
    #[serde(default)]
    pub items_skipped: u64,
    #[serde(default)]
    pub bytes_done: u64,
    #[serde(default)]
    pub bytes_failed: u64,
    #[serde(default)]
    pub bytes_skipped: u64,
    #[serde(default)]
    pub objects_healed: u64,
    #[serde(default)]
    pub objects_failed: u64,
    #[serde(rename = "current_bucket", default)]
    pub bucket: String,
    #[serde(rename = "current_object", default)]
    pub object: String,
    #[serde(default)]
    pub queued_buckets: Option<Vec<String>>,
    #[serde(default)]
    pub healed_buckets: Option<Vec<String>>,
    #[serde(default)]
    pub finished: bool,
    #[serde(default)]
    pub reason: i64,
}

/// madmin `BackgroundHealStatus` (`POST /minio/admin/v3/background-heal/status`).
pub async fn background_heal_status(client: &AdminClient) -> Result<BgHealState> {
    client
        .request("POST", "background-heal/status")
        .send_json()
        .await
}

// ---------------------------------------------------------------------------
// Go formatting helpers
// ---------------------------------------------------------------------------

/// Go `time.Duration.String()` for nanoseconds.
pub fn go_duration(nanos: i64) -> String {
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
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    let seconds = frac(s * 1_000_000_000 + sub, 9);
    if h > 0 {
        format!("{sign}{h}h{m}m{seconds}s")
    } else if m > 0 {
        format!("{sign}{m}m{seconds}s")
    } else {
        format!("{sign}{seconds}s")
    }
}

/// Go `time.Duration.Round(unit)` (halves away from zero).
pub fn round_duration(nanos: i64, unit: i64) -> i64 {
    if unit <= 0 {
        return nanos;
    }
    let rem = nanos % unit;
    if nanos < 0 {
        let rem = -rem;
        if rem + rem < unit {
            nanos + rem
        } else {
            nanos + rem - unit
        }
    } else if rem + rem < unit {
        nanos - rem
    } else {
        nanos + unit - rem
    }
}

/// Go `time.ParseDuration` (`5ms`, `1h2m`, `1.5s`, `-3s`); `None` when invalid.
pub fn parse_go_duration(input: &str) -> Option<i64> {
    let (neg, mut rest) = match input.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, input.strip_prefix('+').unwrap_or(input)),
    };
    if rest == "0" {
        return Some(0);
    }
    if rest.is_empty() {
        return None;
    }
    let mut total: f64 = 0.0;
    while !rest.is_empty() {
        let number_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let number = &rest[..number_len];
        if number.is_empty() || number == "." {
            return None;
        }
        let value: f64 = number.parse().ok()?;
        rest = &rest[number_len..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let scale = match &rest[..unit_len] {
            "ns" => 1.0,
            "us" | "µs" | "μs" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60e9,
            "h" => 3600e9,
            _ => return None,
        };
        rest = &rest[unit_len..];
        total += value * scale;
    }
    if total > i64::MAX as f64 {
        return None;
    }
    let nanos = total.round() as i64;
    Some(if neg { -nanos } else { nanos })
}

/// Parses an RFC3339 (nano) time into (unix seconds, nanoseconds).
pub fn parse_time(text: &str) -> Option<(i64, u32)> {
    let parsed = aws_sdk_s3::primitives::DateTime::from_str(
        text,
        aws_sdk_s3::primitives::DateTimeFormat::DateTimeWithOffset,
    )
    .ok()?;
    Some((parsed.secs(), parsed.subsec_nanos()))
}

/// Formats unix seconds/nanos as Go layout `2006-01-02T15:04:05.000` (UTC, truncated).
pub fn format_millis(secs: i64, nanos: u32) -> String {
    let base = aws_sdk_s3::primitives::DateTime::from_secs(secs)
        .fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
        .unwrap_or_default();
    format!("{}.{:03}", base.trim_end_matches('Z'), nanos / 1_000_000)
}

/// `format_millis` of an RFC3339 string (zero time when unparsable).
pub fn trace_time(text: &str) -> String {
    let (secs, nanos) = parse_time(text).unwrap_or((-62_135_596_800, 0));
    format_millis(secs, nanos)
}

/// Go `http.StatusText`.
pub fn status_text(code: i64) -> &'static str {
    u16::try_from(code)
        .ok()
        .and_then(|c| http::StatusCode::from_u16(c).ok())
        .and_then(|s| s.canonical_reason())
        .unwrap_or("")
}

/// go-humanize `Ordinal` (`1st`, `2nd`, `11th`).
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

/// go-humanize `Time`: relative time (`3 minutes ago`) between `then` and `now` (unix secs).
pub fn relative_time(then: i64, now: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 3600;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 12 * MONTH;
    let (label, diff) = if then <= now {
        ("ago", now - then)
    } else {
        ("from now", then - now)
    };
    let steps: &[(i64, &str, i64)] = &[
        (1, "now", 1),
        (2, "1 second %s", 1),
        (MINUTE, "%d seconds %s", 1),
        (2 * MINUTE, "1 minute %s", 1),
        (HOUR, "%d minutes %s", MINUTE),
        (2 * HOUR, "1 hour %s", 1),
        (DAY, "%d hours %s", HOUR),
        (2 * DAY, "1 day %s", 1),
        (WEEK, "%d days %s", DAY),
        (2 * WEEK, "1 week %s", 1),
        (MONTH, "%d weeks %s", WEEK),
        (2 * MONTH, "1 month %s", 1),
        (YEAR, "%d months %s", MONTH),
        (18 * MONTH, "1 year %s", 1),
        (2 * YEAR, "2 years %s", 1),
        (37 * YEAR, "%d years %s", YEAR),
    ];
    for (limit, format, div) in steps {
        if diff < *limit {
            return format
                .replace("%d", &(diff / div).to_string())
                .replace("%s", label);
        }
    }
    format!("a long while {label}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::admin::mock;

    #[test]
    fn trace_type_names_match_madmin() {
        assert_eq!(trace_type_name(TRACE_S3), "S3");
        assert_eq!(trace_type_name(1 << 4), "Scanner");
        assert_eq!(trace_type_name(1 << 20), "IAM");
        assert_eq!(trace_type_name(3), "TraceType(3)");
        assert_eq!(find_trace_type("scanner"), 1 << 4);
        assert_eq!(find_trace_type("bogus"), 0);
    }

    #[test]
    fn go_durations_format_and_parse() {
        assert_eq!(go_duration(0), "0s");
        assert_eq!(go_duration(999), "999ns");
        assert_eq!(go_duration(190_000), "190µs");
        assert_eq!(go_duration(3_068_000), "3.068ms");
        assert_eq!(go_duration(1_500_000_000), "1.5s");
        assert_eq!(go_duration(3_723_000_000_000), "1h2m3s");
        assert_eq!(go_duration(-2_000_000_000), "-2s");
        assert_eq!(round_duration(3_067_748, 1_000), 3_068_000);
        assert_eq!(round_duration(189_451, 1_000), 189_000);
        assert_eq!(parse_go_duration("5ms"), Some(5_000_000));
        assert_eq!(parse_go_duration("1h2m"), Some(3_720_000_000_000));
        assert_eq!(parse_go_duration("1.5s"), Some(1_500_000_000));
        assert_eq!(parse_go_duration("0"), Some(0));
        assert_eq!(parse_go_duration("5"), None);
        assert_eq!(parse_go_duration("bogus"), None);
    }

    #[test]
    fn humanize_helpers() {
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(22), "22nd");
        assert_eq!(relative_time(100, 100), "now");
        assert_eq!(relative_time(0, 150), "2 minutes ago");
        assert_eq!(relative_time(0, 3 * 3600), "3 hours ago");
        assert_eq!(
            trace_time("2026-09-26T19:26:46.434305953Z"),
            "2026-09-26T19:26:46.434"
        );
        assert_eq!(status_text(404), "Not Found");
    }

    #[test]
    fn trace_request_query_matches_madmin() {
        let opts = TraceOpts {
            s3: true,
            only_errors: true,
            threshold: 5_000_000,
            ..Default::default()
        };
        let server = mock::serve(vec![mock::chunked(
            200,
            &[
                r#"{"type":4,"nodename":"n1","funcname":"s3.PutObject","time":"2026-01-01T00:00:00Z","path":"/b/o","dur":1000,"http":{"request":{"method":"PUT","headers":{"Host":["h"]},"body":"aGk=","client":"1.2.3.4"},"response":{"statuscode":200},"stats":{"inputbytes":5,"outputbytes":0,"timetofirstbyte":10}}}"#,
                "\n",
                r#"{"funcname":"s3.GetObject","request":{"path":"/legacy"},"stats":{"latency":7}}"#,
            ],
        )]);
        let client = mock::client(&server, "s");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let infos = rt
            .block_on(async {
                let mut stream = trace(&client, &opts).await?;
                let mut infos = Vec::new();
                while let Some(info) = stream.next::<TraceInfo>().await? {
                    infos.push(info.normalize());
                }
                anyhow::Ok(infos)
            })
            .unwrap();
        assert_eq!(infos.len(), 2);
        let http = infos[0].http.as_ref().unwrap();
        assert_eq!(http.request.body, b"hi");
        assert_eq!(http.stats.input_bytes, 5);
        assert_eq!(infos[1].trace_type, TRACE_S3);
        assert_eq!(infos[1].path, "/legacy");
        assert_eq!(infos[1].duration, 7);
        let head = server.requests().remove(0).head;
        assert!(
            head.starts_with(
                "GET /minio/admin/v3/trace?batch-expire=false&batch-keyrotation=false&batch-replication=false&bootstrap=false&decommission=false&err=true&formatting=false&ftp=false&healing=false&ilm=false&internal=false&kms=false&os=false&rebalance=false&replication-resync=false&s3=true&scanner=false&storage=false&threshold=5ms "
            ),
            "{head}"
        );
    }

    #[test]
    fn heal_requests_follow_madmin() {
        let server = mock::serve(vec![
            mock::reply(
                200,
                br#"{"clientToken":"tok","clientAddress":"a","startTime":"2026-01-01T00:00:00Z"}"#,
            ),
            mock::reply(
                200,
                br#"{"Summary":"finished","StartTime":"2026-01-01T00:00:00Z","Settings":{},"Items":[{"resultId":1,"type":"bucket","bucket":"b","before":{"drives":[{"uuid":"","endpoint":"/d1","state":"ok"}]},"after":{"drives":null}}]}"#,
            ),
        ]);
        let client = mock::client(&server, "s");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let opts = HealOpts {
            recursive: true,
            scan_mode: 1,
            ..Default::default()
        };
        let (start, status) = rt
            .block_on(async {
                let start = heal(&client, "b", "p/q", &opts, "", false, false).await?;
                let status = heal(&client, "b", "p/q", &opts, "tok", false, false).await?;
                anyhow::Ok((start, status))
            })
            .unwrap();
        let HealReply::Started(start) = start else {
            panic!("start")
        };
        assert_eq!(start.client_token, "tok");
        assert!(matches!(&status, HealReply::Status(s) if s.summary == "finished"));
        let HealReply::Status(status) = status else {
            panic!("status")
        };
        assert_eq!(status.items[0].before.count("ok"), 1);
        assert!(status.items[0].after.drives.is_none());
        let requests = server.requests();
        assert!(
            requests[0]
                .head
                .starts_with("POST /minio/admin/v3/heal/b/p/q "),
            "{}",
            requests[0].head
        );
        assert_eq!(
            String::from_utf8_lossy(&requests[0].body),
            r#"{"recursive":true,"dryRun":false,"remove":false,"recreate":false,"scanMode":1,"updateParity":false,"nolock":false}"#
        );
        assert!(
            requests[1]
                .head
                .starts_with("POST /minio/admin/v3/heal/b/p/q?clientToken=tok ")
        );
        assert!(requests[1].body.is_empty());
    }

    #[test]
    fn background_heal_state_round_trips_like_go() {
        let text = r#"{"offline_nodes":null,"ScannedItemsCount":0,"HealDisks":null,"sets":[{"id":"0-0","pool_index":0,"set_index":0,"heal_status":"","heal_priority":"","total_objects":0,"disks":[{"endpoint":"/data1","path":"/data1","state":"ok","uuid":"u","major":0,"minor":246,"totalspace":10,"usedspace":5,"availspace":5,"metrics":{"lastMinute":{"DiskInfo":{"count":9,"acc_time_ns":20057}},"apiCalls":{"B":1,"A":0}},"used_inodes":1,"free_inodes":2,"local":true,"pool_index":0,"set_index":0,"disk_index":0}]}],"mrf":null,"sc_parity":{"REDUCED_REDUNDANCY":1,"STANDARD":2}}"#;
        let state: BgHealState = serde_json::from_str(text).unwrap();
        let expected = text.replace(r#""B":1,"A":0"#, r#""A":0,"B":1"#);
        assert_eq!(serde_json::to_string(&state).unwrap(), expected);
    }

    #[test]
    fn logs_request_and_entry_json() {
        let server = mock::serve(vec![mock::chunked(
            200,
            &[
                r#"{"deploymentid":"d","level":"ERROR","errKind":"","time":"2026-01-01T00:00:00Z","api":{"name":"SYSTEM.notify","args":{}},"error":{"message":"boom","source":["a.go:1"],"variables":{"k":"v"}},"node":""}"#,
            ],
        )]);
        let client = mock::client(&server, "s");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let entry = rt
            .block_on(async {
                let mut stream = logs(&client, "", 0, "all").await?;
                stream.next::<LogInfo>().await
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_string(&entry).unwrap(),
            r#"{"deploymentid":"d","level":"ERROR","errKind":"","time":"2026-01-01T00:00:00Z","api":{"name":"SYSTEM.notify","args":{}},"error":{"message":"boom","source":["a.go:1"],"variables":{"k":"v"}},"ConsoleMsg":"","node":""}"#
        );
        assert!(
            server.requests()[0]
                .head
                .starts_with("GET /minio/admin/v3/log?limit=0&logType=all&node= ")
        );
    }
}
