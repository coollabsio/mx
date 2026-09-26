//! MinIO batch jobs (madmin-go `batch-job.go`): start, list, status, describe, cancel,
//! generate, plus the batch-job realtime metrics stream (`madmin.Metrics`).

use super::admin::{AdminClient, JsonStream, check_status};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// madmin `SupportedJobTypes` (fallback when the server has no `list-supported-job-types`).
pub const SUPPORTED_JOB_TYPES: &[&str] = &["replicate", "keyrotate", "expire"];

/// Go's zero `time.Time` as JSON.
pub const ZERO_TIME: &str = "0001-01-01T00:00:00Z";

fn zero_time() -> String {
    ZERO_TIME.to_string()
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// madmin `BatchJobResult`. Times stay in the server's RFC3339 form (what Go re-marshals).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatchJobResult {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default)]
    pub job_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    #[serde(default = "zero_time")]
    pub started: String,
    /// `time.Duration` (nanoseconds).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub elapsed: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListBatchJobsResult {
    #[serde(default)]
    pub jobs: Option<Vec<BatchJobResult>>,
}

/// madmin `BatchJobStatus` (no JSON tags: `LastMetric`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BatchJobStatus {
    #[serde(rename = "LastMetric", default)]
    pub last_metric: JobMetric,
}

/// madmin `JobMetric`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobMetric {
    #[serde(rename = "jobID", default)]
    pub job_id: String,
    #[serde(rename = "jobType", default)]
    pub job_type: String,
    #[serde(rename = "startTime", default = "zero_time")]
    pub start_time: String,
    #[serde(rename = "lastUpdate", default = "zero_time")]
    pub last_update: String,
    #[serde(rename = "retryAttempts", default)]
    pub retry_attempts: i64,
    #[serde(default)]
    pub complete: bool,
    #[serde(default)]
    pub failed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replicate: Option<ReplicateInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<KeyRotationInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired: Option<ExpirationInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<CatalogInfo>,
}

impl Default for JobMetric {
    fn default() -> Self {
        Self {
            job_id: String::new(),
            job_type: String::new(),
            start_time: zero_time(),
            last_update: zero_time(),
            retry_attempts: 0,
            complete: false,
            failed: false,
            replicate: None,
            rotation: None,
            expired: None,
            catalog: None,
        }
    }
}

/// madmin `ReplicateInfo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ReplicateInfo {
    pub last_bucket: String,
    pub last_object: String,
    pub objects: i64,
    pub objects_failed: i64,
    pub delete_markers: i64,
    pub delete_markers_failed: i64,
    pub bytes_transferred: i64,
    pub bytes_failed: i64,
}

/// madmin `KeyRotationInfo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct KeyRotationInfo {
    pub last_bucket: String,
    pub last_object: String,
    pub objects: i64,
    pub objects_failed: i64,
}

/// madmin `ExpirationInfo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ExpirationInfo {
    pub last_bucket: String,
    pub last_object: String,
    pub objects: i64,
    pub objects_failed: i64,
    pub delete_markers: i64,
    pub delete_markers_failed: i64,
}

/// madmin `CatalogInfo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CatalogInfo {
    pub last_bucket_scanned: String,
    pub last_object_scanned: String,
    pub last_bucket_matched: String,
    pub last_object_matched: String,
    pub objects_scanned_count: u64,
    pub objects_matched_count: u64,
    pub records_written_count: u64,
    pub output_objects_count: u64,
    pub manifest_path_bucket: String,
    pub manifest_path_object: String,
    pub error_msg: String,
}

/// The parts of madmin `RealtimeMetrics` that `batch status` reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RealtimeMetrics {
    #[serde(default)]
    pub aggregated: AggregatedMetrics,
    #[serde(rename = "final", default)]
    pub final_: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AggregatedMetrics {
    #[serde(rename = "batchJobs", default)]
    pub batch_jobs: Option<BatchJobMetrics>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BatchJobMetrics {
    #[serde(rename = "Jobs", default)]
    pub jobs: Option<HashMap<String, JobMetric>>,
}

/// madmin `MetricsBatchJobs` (`1 << 3`).
const METRICS_BATCH_JOBS: &str = "8";

/// `StartBatchJob`: POST `start-job` with the YAML job definition.
pub async fn start_job(client: &AdminClient, job: Vec<u8>) -> Result<BatchJobResult> {
    client
        .request("POST", "start-job")
        .body(job)
        .send_json()
        .await
}

/// `ListBatchJobs` (`jobType` may be empty for all types).
pub async fn list_jobs(client: &AdminClient, job_type: &str) -> Result<Vec<BatchJobResult>> {
    let result: ListBatchJobsResult = client
        .request("GET", "list-jobs")
        .query("jobType", job_type)
        .send_json()
        .await?;
    Ok(result.jobs.unwrap_or_default())
}

/// `BatchJobStatus`: GET `status-job`.
pub async fn job_status(client: &AdminClient, job_id: &str) -> Result<BatchJobStatus> {
    client
        .request("GET", "status-job")
        .query("jobId", job_id)
        .send_json()
        .await
}

/// `DescribeBatchJob`: the job definition as YAML text.
pub async fn describe_job(client: &AdminClient, job_id: &str) -> Result<String> {
    let response = client
        .request("GET", "describe-job")
        .query("jobId", job_id)
        .send()
        .await?;
    Ok(response.text())
}

/// `CancelBatchJob`: DELETE `cancel-job`.
pub async fn cancel_job(client: &AdminClient, job_id: &str) -> Result<()> {
    client
        .request("DELETE", "cancel-job")
        .query("id", job_id)
        .send()
        .await?;
    Ok(())
}

/// True when madmin treats the status as "API not available" (404 / 426).
fn api_unavailable(status: u16) -> bool {
    status == 404 || status == 426
}

/// `GetSupportedBatchJobTypes`; `None` when the server does not have the API.
pub async fn supported_job_types(client: &AdminClient) -> Result<Option<Vec<String>>> {
    let response = client
        .request("GET", "list-supported-job-types")
        .send_unchecked()
        .await?;
    if api_unavailable(response.status) {
        return Ok(None);
    }
    let response = check_status(response)?;
    Ok(Some(
        serde_json::from_slice::<Option<Vec<String>>>(&response.body)?.unwrap_or_default(),
    ))
}

/// `GenerateBatchJobV2`; `None` when the server does not have the API.
pub async fn generate_job(client: &AdminClient, job_type: &str) -> Result<Option<String>> {
    if job_type.is_empty() {
        anyhow::bail!("batch job type is required");
    }
    let response = client
        .request("GET", "generate-job")
        .query("jobType", job_type)
        .send_unchecked()
        .await?;
    if api_unavailable(response.status) {
        return Ok(None);
    }
    Ok(Some(check_status(response)?.text()))
}

/// madmin `GenerateBatchJob`: the static templates.
pub fn job_template(job_type: &str) -> Option<&'static str> {
    match job_type {
        "replicate" => Some(REPLICATE_TEMPLATE),
        "keyrotate" => Some(KEYROTATE_TEMPLATE),
        "expire" => Some(EXPIRE_TEMPLATE),
        _ => None,
    }
}

/// `Metrics` with `Type: MetricsBatchJobs, ByJobID, Interval: 1s` (endless stream).
pub async fn job_metrics(client: &AdminClient, job_id: &str) -> Result<JsonStream> {
    client
        .request("GET", "metrics")
        .query("types", METRICS_BATCH_JOBS)
        .query("n", "0")
        .query("interval", "1s")
        .query("hosts", "")
        .query("disks", "")
        .query("by-jobID", job_id)
        .stream()
        .await
}

pub const REPLICATE_TEMPLATE: &str = r##"replicate:
  apiVersion: v1
  # source of the objects to be replicated
  source:
    type: TYPE # valid values are "s3" or "minio"
    bucket: BUCKET
    prefix: PREFIX # 'PREFIX' is optional
    # If your source is the 'local' alias specified to 'mc batch start', then the 'endpoint' and 'credentials' fields are optional and can be omitted
    # Either the 'source' or 'remote' *must* be the "local" deployment
    endpoint: "http[s]://HOSTNAME:PORT"
    # path: "on|off|auto" # "on" enables path-style bucket lookup. "off" enables virtual host (DNS)-style bucket lookup. Defaults to "auto"
    credentials:
      accessKey: ACCESS-KEY # Required
      secretKey: SECRET-KEY # Required
    # sessionToken: SESSION-TOKEN # Optional only available when rotating credentials are used
    snowball: # automatically activated if the source is local
      disable: false # optionally turn-off snowball archive transfer
      batch: 100 # upto this many objects per archive
      inmemory: true # indicates if the archive must be staged locally or in-memory
      compress: false # S2/Snappy compressed archive
      smallerThan: 256KiB # create archive for all objects smaller than 256KiB
      skipErrs: false # skips any source side read() errors

  # target where the objects must be replicated
  target:
    type: TYPE # valid values are "s3" or "minio"
    bucket: BUCKET
    prefix: PREFIX # 'PREFIX' is optional
    # If your source is the 'local' alias specified to 'mc batch start', then the 'endpoint' and 'credentials' fields are optional and can be omitted

    # Either the 'source' or 'remote' *must* be the "local" deployment
    endpoint: "http[s]://HOSTNAME:PORT"
    # path: "on|off|auto" # "on" enables path-style bucket lookup. "off" enables virtual host (DNS)-style bucket lookup. Defaults to "auto"
    credentials:
      accessKey: ACCESS-KEY
      secretKey: SECRET-KEY
    # sessionToken: SESSION-TOKEN # Optional only available when rotating credentials are used

  # NOTE: All flags are optional
  # - filtering criteria only applies for all source objects match the criteria
  # - configurable notification endpoints
  # - configurable retries for the job (each retry skips successfully previously replaced objects)
  flags:
    filter:
      newerThan: "7d" # match objects newer than this value (e.g. 7d10h31s)
      olderThan: "7d" # match objects older than this value (e.g. 7d10h31s)
      createdAfter: "date" # match objects created after "date"
      createdBefore: "date" # match objects created before "date"

      ## NOTE: tags are not supported when "source" is remote.
      # tags:
      #   - key: "name"
      #     value: "pick*" # match objects with tag 'name', with all values starting with 'pick'

      # metadata:
      #   - key: "content-type"
      #     value: "image/*" # match objects with 'content-type', with all values starting with 'image/'

    notify:
      endpoint: "https://notify.endpoint" # notification endpoint to receive job status events
      token: "Bearer xxxxx" # optional authentication token for the notification endpoint

    retry:
      attempts: 10 # number of retries for the job before giving up
      delay: "500ms" # least amount of delay between each retry
"##;

pub const KEYROTATE_TEMPLATE: &str = r##"keyrotate:
  apiVersion: v1
  bucket: BUCKET
  prefix: PREFIX
  encryption:
    type: sse-s3 # valid values are sse-s3 and sse-kms
    key: <new-kms-key> # valid only for sse-kms
    context: <new-kms-key-context> # valid only for sse-kms

  # optional flags based filtering criteria
  # for all objects
  flags:
    filter:
      newerThan: "7d" # match objects newer than this value (e.g. 7d10h31s)
      olderThan: "7d" # match objects older than this value (e.g. 7d10h31s)
      createdAfter: "date" # match objects created after "date"
      createdBefore: "date" # match objects created before "date"
      tags:
        - key: "name"
          value: "pick*" # match objects with tag 'name', with all values starting with 'pick'
      metadata:
        - key: "content-type"
          value: "image/*" # match objects with 'content-type', with all values starting with 'image/'
      kmskey: "key-id" # match objects with KMS key-id (applicable only for sse-kms)
    notify:
      endpoint: "https://notify.endpoint" # notification endpoint to receive job status events
      token: "Bearer xxxxx" # optional authentication token for the notification endpoint
    retry:
      attempts: 10 # number of retries for the job before giving up
      delay: "500ms" # least amount of delay between each retry
"##;

pub const EXPIRE_TEMPLATE: &str = r##"expire:
  apiVersion: v1
  bucket: mybucket # Bucket where this job will expire matching objects from
  prefix: myprefix # (Optional) Prefix under which this job will expire objects matching the rules below.
  rules:
    - type: object  # objects with zero ore more older versions
      name: NAME # match object names that satisfy the wildcard expression.
      olderThan: 70h # match objects older than this value
      createdBefore: "2006-01-02T15:04:05.00Z" # match objects created before "date"
      tags:
        - key: name
          value: pick* # match objects with tag 'name', all values starting with 'pick'
      metadata:
        - key: content-type
          value: image/* # match objects with 'content-type', all values starting with 'image/'
      size:
        lessThan: 10MiB # match objects with size less than this value (e.g. 10MiB)
        greaterThan: 1MiB # match objects with size greater than this value (e.g. 1MiB)
      purge:
          # retainVersions: 0 # (default) delete all versions of the object. This option is the fastest.
          # retainVersions: 5 # keep the latest 5 versions of the object.

    - type: deleted # objects with delete marker as their latest version
      name: NAME # match object names that satisfy the wildcard expression.
      olderThan: 10h # match objects older than this value (e.g. 7d10h31s)
      createdBefore: "2006-01-02T15:04:05.00Z" # match objects created before "date"
      purge:
          # retainVersions: 0 # (default) delete all versions of the object. This option is the fastest.
          # retainVersions: 5 # keep the latest 5 versions of the object including delete markers.

  notify:
    endpoint: https://notify.endpoint # notification endpoint to receive job completion status
    token: Bearer xxxxx # optional authentication token for the notification endpoint

  retry:
    attempts: 10 # number of retries for the job before giving up
    delay: 500ms # least amount of delay between each retry
"##;
