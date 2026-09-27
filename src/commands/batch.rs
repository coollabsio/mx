//! `mx batch` (mc `batch`): MinIO batch jobs via the admin API (`start-job`, `list-jobs`,
//! `status-job`, `describe-job`, `cancel-job`, `generate-job`, batch-job metrics).

use crate::commands::runtime;
use crate::config::ConfigStore;
use crate::error::McError;
use crate::output;
use crate::s3::admin::{self, AdminClient};
use crate::s3::batch::{self as api, BatchJobResult, JobMetric, RealtimeMetrics};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::io::{IsTerminal, Write};
use std::time::{Duration, SystemTime};

#[derive(Debug, Args)]
pub struct BatchArgs {
    #[command(subcommand)]
    pub command: BatchCommand,
}

#[derive(Debug, Subcommand)]
pub enum BatchCommand {
    #[command(name = "generate", about = "generate a new batch job definition")]
    Generate(BatchGenerateArgs),
    #[command(name = "start", about = "start a new batch job")]
    Start(BatchStartArgs),
    #[command(
        name = "list",
        visible_alias = "ls",
        about = "list all current batch jobs"
    )]
    List(BatchListArgs),
    #[command(
        name = "status",
        about = "summarize job events on MinIO server in real-time"
    )]
    Status(BatchStatusArgs),
    #[command(name = "describe", about = "describe job definition for a job")]
    Describe(BatchDescribeArgs),
    #[command(name = "cancel", about = "cancel ongoing batch job")]
    Cancel(BatchCancelArgs),
}

#[derive(Debug, Args)]
#[command(after_help = "JOBTYPE:
  - replicate
  - keyrotate
  - expire
Use the special value \"list\" to request server to list supported job types.

EXAMPLES:
  1. Generate a new batch 'replication' job definition:
     $ mc batch generate myminio replicate > replication.yaml
  2. List all supported job types:
     $ mc batch generate myminio list")]
pub struct BatchGenerateArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "JOBTYPE")]
    pub jobtype: String,
}

#[derive(Debug, Args)]
#[command(after_help = "EXAMPLES:
  1. Start a new batch 'replication' job:
     $ mc batch start myminio ./replication.yaml")]
pub struct BatchStartArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "JOBFILE")]
    pub jobfile: String,
}

#[derive(Debug, Args)]
#[command(after_help = "EXAMPLES:
  1. List all current batch jobs:
     $ mc batch list myminio

  2. List all current batch jobs of type 'replicate':
     $ mc batch list myminio/ --type \"replicate\"")]
pub struct BatchListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(
        long = "type",
        value_name = "VALUE",
        help = "list all current batch jobs via job type"
    )]
    pub type_: Option<String>,
}

#[derive(Debug, Args)]
#[command(after_help = "EXAMPLES:
   1. Display current in-progress JOB events.
      $ mc batch status myminio/ KwSysDpxcBU9FNhGkn2dCf")]
pub struct BatchStatusArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "JOBID")]
    pub jobid: String,
}

#[derive(Debug, Args)]
#[command(after_help = "EXAMPLES:
  1. Describe current batch job definition:
     $ mc batch describe myminio KwSysDpxcBU9FNhGkn2dCf")]
pub struct BatchDescribeArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "JOBID")]
    pub jobid: String,
}

#[derive(Debug, Args)]
#[command(after_help = "EXAMPLES:
  1. Cancel ongoing batch job:
     $ mc batch cancel myminio <job-id>")]
pub struct BatchCancelArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "JOBID")]
    pub jobid: String,
    /// Accepted like mc, which ignores it (the job ID is the second argument).
    #[arg(long = "id", value_name = "VALUE", help = "job id")]
    pub id: Option<String>,
}

pub fn run(args: BatchArgs, json: bool) -> Result<()> {
    match args.command {
        BatchCommand::Generate(args) => generate(args, json),
        BatchCommand::Start(args) => start(args, json),
        BatchCommand::List(args) => list(args, json),
        BatchCommand::Status(args) => status(args, json),
        BatchCommand::Describe(args) => describe(args),
        BatchCommand::Cancel(args) => cancel(args, json),
    }
}

fn client(target: &str) -> Result<AdminClient> {
    admin::admin_client_for(&ConfigStore::load_or_create()?, target)
}

fn generate(args: BatchGenerateArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    let rt = runtime()?;
    let job_type = args.jobtype;
    if job_type == "list" {
        let types = rt
            .block_on(api::supported_job_types(&client))
            .context("Unable to list supported job types")?
            .unwrap_or_else(|| {
                api::SUPPORTED_JOB_TYPES
                    .iter()
                    .map(|t| t.to_string())
                    .collect()
            });
        if json {
            // mc prints `json.Marshal(types)`: always compact.
            println!("{}", serde_json::to_string(&types)?);
        } else {
            for job_type in types {
                println!("{job_type}");
            }
        }
        return Ok(());
    }
    let template = rt
        .block_on(api::generate_job(&client, &job_type))
        .with_context(|| format!("Unable to generate template for {job_type}"))?;
    let template = match template {
        Some(template) => template,
        None => api::job_template(&job_type)
            .ok_or_else(|| {
                anyhow::Error::new(McError::invalid_argument())
                    .context("Unable to generate a job template for the specified job type")
            })?
            .to_string(),
    };
    println!("{template}");
    Ok(())
}

/// Go `*fs.PathError` for a local file operation: `OP PATH: ERRNO` with its JSON form.
fn path_error(op: &str, path: &str, err: &std::io::Error) -> McError {
    let text = err.to_string();
    let text = match text.find(" (os error") {
        Some(index) => text[..index].to_lowercase(),
        None => text,
    };
    McError::with_detail(
        format!("{op} {path}: {text}"),
        crate::detail![
            ("Op", op),
            ("Path", path),
            ("Err", err.raw_os_error().unwrap_or_default())
        ],
    )
}

/// mc `os.ReadFile`.
fn read_file(path: &str) -> Result<Vec<u8>, McError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|err| path_error("open", path, &err))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|err| path_error("read", path, &err))?;
    Ok(buf)
}

#[derive(Debug, Serialize)]
struct StartMessage<'a> {
    status: &'static str,
    result: &'a BatchJobResult,
}

fn start(args: BatchStartArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    // mc's JSON error message keeps the unformatted `%s`.
    let context = if json {
        "Unable to read %s".to_string()
    } else {
        format!("Unable to read {}", args.jobfile)
    };
    let job = read_file(&args.jobfile).context(context)?;
    let result = runtime()?
        .block_on(api::start_job(&client, job))
        .context("Unable to start job")?;
    if json {
        output::print_json(&StartMessage {
            status: "success",
            result: &result,
        })
    } else {
        println!(
            "Successfully started '{}' job `{}` on '{}'",
            result.job_type,
            result.id,
            go_time_string(&result.started)
        );
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct ListMessage {
    jobs: Vec<ListJob>,
    status: &'static str,
}

/// mc marshals a map: keys in alphabetical order.
#[derive(Debug, Serialize)]
struct ListJob {
    id: String,
    started: String,
    status: &'static str,
    #[serde(rename = "type")]
    job_type: String,
    user: String,
}

fn list(args: BatchListArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    let rt = runtime()?;
    let jobs = rt
        .block_on(api::list_jobs(
            &client,
            args.type_.as_deref().unwrap_or_default(),
        ))
        .context("Unable to list jobs")?;
    if !json && jobs.is_empty() {
        println!("currently no jobs are running");
        return Ok(());
    }
    let now = SystemTime::now();
    let mut rows = Vec::new();
    let mut entries = Vec::new();
    for job in jobs {
        let status = match rt.block_on(api::job_status(&client, &job.id)) {
            Ok(status) => job_state(&status.last_metric),
            Err(err) => {
                eprintln!(
                    "Failed to fetch job status for Job ID: {} Error: {err}",
                    job.id
                );
                "unknown"
            }
        };
        if json {
            entries.push(ListJob {
                id: job.id,
                started: job.started,
                status,
                job_type: job.job_type,
                user: job.user,
            });
        } else {
            rows.push(vec![
                job.id,
                job.job_type,
                job.user,
                humanize_time(parse_time(&job.started), now),
                status.to_string(),
            ]);
        }
    }
    if json {
        return output::print_json(&ListMessage {
            jobs: entries,
            status: "success",
        });
    }
    let headers = ["ID", "TYPE", "USER", "STARTED", "STATUS"].map(String::from);
    print!("{}", render_table(Some(&headers), &rows));
    Ok(())
}

fn job_state(metric: &JobMetric) -> &'static str {
    if metric.failed {
        "failed"
    } else if metric.complete {
        "completed"
    } else {
        "in-progress"
    }
}

fn describe(args: BatchDescribeArgs) -> Result<()> {
    let client = client(&args.target)?;
    let job = runtime()?
        .block_on(api::describe_job(&client, &args.jobid))
        .context("Unable to fetch the job definition")?;
    println!("{job}");
    Ok(())
}

#[derive(Debug, Serialize)]
struct CancelMessage<'a> {
    status: &'static str,
    #[serde(rename = "job-id")]
    job_id: &'a str,
}

fn cancel(args: BatchCancelArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    runtime()?
        .block_on(api::cancel_job(&client, &args.jobid))
        .context("Unable to cancel job")?;
    if json {
        output::print_json(&CancelMessage {
            status: "success",
            job_id: &args.jobid,
        })
    } else {
        println!("Successfully canceled batch job `{}`", args.jobid);
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------
// status
// ------------------------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct StatusMessage<'a> {
    status: &'static str,
    metric: &'a JobMetric,
}

fn status(args: BatchStatusArgs, json: bool) -> Result<()> {
    // mc uses a different message for this admin client.
    let client = client(&args.target).map_err(|err| match crate::error::mc_error(&err) {
        Some(cause) => {
            anyhow::Error::new(cause.clone()).context("Unable to initialize admin client.")
        }
        None => err,
    })?;
    let rt = runtime()?;
    let job_id = args.jobid;
    let no_such_job = match rt.block_on(api::describe_job(&client, &job_id)) {
        Ok(_) => false,
        Err(err) if crate::error::error_code(&err) == Some("XMinioAdminNoSuchJob") => {
            if !json {
                println!(
                    "{}: Unable to find an active job, attempting to list from previously run jobs",
                    output::prog_name()
                );
            }
            true
        }
        Err(err) => return Err(err.context("Unable to lookup job status")),
    };
    if json {
        return rt.block_on(status_json(&client, &job_id, no_such_job));
    }
    // mc's live view (bubbletea) needs a terminal for input.
    open_tty().context("Unable to get current batch status")?;
    rt.block_on(status_view(&client, &job_id, no_such_job))
}

/// bubbletea opens `/dev/tty` when stdin is not a terminal.
fn open_tty() -> Result<(), McError> {
    if std::io::stdin().is_terminal() {
        return Ok(());
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map(drop)
        .map_err(|err| {
            McError::new(format!(
                "could not open a new TTY: {}",
                path_error("open", "/dev/tty", &err)
            ))
        })
}

fn job_from(metrics: RealtimeMetrics, job_id: &str) -> Option<JobMetric> {
    metrics
        .aggregated
        .batch_jobs
        .and_then(|jobs| jobs.jobs)
        .and_then(|mut jobs| jobs.remove(job_id))
}

async fn status_json(client: &AdminClient, job_id: &str, no_such_job: bool) -> Result<()> {
    if no_such_job {
        let status = api::job_status(client, job_id)
            .await
            .context("Unable to lookup job status")?;
        let metric = status.last_metric;
        output::print_json(&StatusMessage {
            status: "success",
            metric: &metric,
        })?;
        if !(metric.complete || metric.failed) {
            // mc waits for Ctrl-C here.
            let _ = tokio::signal::ctrl_c().await;
        }
        return Ok(());
    }
    let mut stream = api::job_metrics(client, job_id)
        .await
        .context("Unable to get current batch status")?;
    loop {
        let item = tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            item = stream.next::<RealtimeMetrics>() => item,
        };
        let Some(metrics) = item.context("Unable to get current batch status")? else {
            return Err(McError::new("unexpected EOF"))
                .context("Unable to get current batch status");
        };
        let last = metrics.final_;
        let Some(job) = job_from(metrics, job_id) else {
            return Ok(());
        };
        let status = if job.complete {
            "complete"
        } else if job.failed {
            "failed"
        } else {
            "in-progress"
        };
        output::print_json(&StatusMessage {
            status,
            metric: &job,
        })?;
        if job.complete || job.failed || last {
            return Ok(());
        }
    }
}

/// bubbles `spinner.Points`.
const SPINNER: [&str; 4] = ["∙∙∙", "●∙∙", "∙●∙", "∙∙●"];

/// Redraws the view in place on a terminal; otherwise prints only the final view.
struct Screen {
    lines: usize,
    tty: bool,
}

impl Screen {
    fn draw(&mut self, view: &str, last: bool) -> Result<()> {
        let mut out = std::io::stdout().lock();
        if !self.tty {
            if last {
                out.write_all(view.as_bytes())?;
            }
            return Ok(());
        }
        if self.lines > 0 {
            write!(out, "\x1b[{}A\r\x1b[J", self.lines)?;
        }
        out.write_all(view.as_bytes())?;
        out.flush()?;
        self.lines = view.matches('\n').count();
        Ok(())
    }
}

async fn status_view(client: &AdminClient, job_id: &str, no_such_job: bool) -> Result<()> {
    let mut screen = Screen {
        lines: 0,
        tty: output::stdout_is_terminal(),
    };
    let mut metric = JobMetric::default();
    let mut frame = 0;
    let mut stream = None;
    if no_such_job {
        metric = api::job_status(client, job_id)
            .await
            .context("Unable to lookup job status")?
            .last_metric;
    } else {
        stream = Some(
            api::job_metrics(client, job_id)
                .await
                .context("Unable to get current batch status")?,
        );
    }
    let mut ticker = tokio::time::interval(Duration::from_secs(1) / 7);
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    while !(metric.complete || metric.failed) {
        screen.draw(
            &status_view_text(&metric, Some(SPINNER[frame]), false),
            false,
        )?;
        tokio::select! {
            _ = &mut ctrl_c => break,
            _ = ticker.tick() => frame = (frame + 1) % SPINNER.len(),
            item = async {
                match stream.as_mut() {
                    Some(stream) => stream.next::<RealtimeMetrics>().await,
                    None => std::future::pending().await,
                }
            } => match item.context("Unable to get current batch status")? {
                Some(metrics) => {
                    if let Some(job) = job_from(metrics, job_id) {
                        metric = job;
                    }
                }
                None => break,
            },
        }
    }
    screen.draw(&status_view_text(&metric, None, true), true)
}

/// mc `batchJobMetricsUI.View()`; `spinner` is the current frame while running.
fn status_view_text(metric: &JobMetric, spinner: Option<&str>, quitting: bool) -> String {
    let mut out = String::new();
    if !quitting {
        out.push_str(spinner.unwrap_or_default());
    } else if metric.complete {
        out.push_str("✔ ✔ ✔ ");
    } else if metric.failed {
        out.push_str("✗ ✗ ✗ ");
    }
    out.push('\n');
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut add = |label: &str, value: String| rows.push(vec![label.to_string(), value]);
    let elapsed = time_diff_nanos(&metric.last_update, &metric.start_time);
    match metric.job_type.as_str() {
        "replicate" => {
            let r = metric.replicate.clone().unwrap_or_default();
            add("JobType: ", metric.job_type.clone());
            add("Objects: ", r.objects.to_string());
            // mc prints the object count here too.
            add("Versions: ", r.objects.to_string());
            add("FailedObjects: ", r.objects_failed.to_string());
            add("DeleteMarker: ", r.delete_markers.to_string());
            add("FailedDeleteMarker: ", r.delete_markers_failed.to_string());
            if elapsed > 0 {
                let bytes_per_sec = r.bytes_transferred as f64 / (elapsed as f64 / 1e9);
                let objects_per_sec = (1_000_000_000 * r.objects) as f64 / elapsed as f64;
                add(
                    "Throughput: ",
                    format!("{}/s", humanize_ibytes(bytes_per_sec as u64)),
                );
                add("IOPs: ", format!("{objects_per_sec:.2} objs/s"));
            }
            add("Transferred: ", humanize_ibytes(r.bytes_transferred as u64));
            add("Elapsed: ", go_duration(round_seconds(elapsed)));
            add("CurrObjName: ", r.last_object);
        }
        "expire" => {
            let e = metric.expired.clone().unwrap_or_default();
            add("JobType: ", metric.job_type.clone());
            add("Objects: ", e.objects.to_string());
            add("FailedObjects: ", e.objects_failed.to_string());
            add("DeleteMarker: ", e.delete_markers.to_string());
            add("FailedDeleteMarker: ", e.delete_markers_failed.to_string());
            add("CurrObjName: ", e.last_object);
            if metric.last_update != api::ZERO_TIME {
                add("Elapsed: ", go_duration(elapsed));
            }
        }
        "catalog" => {
            let c = metric.catalog.clone().unwrap_or_default();
            add("JobType: ", metric.job_type.clone());
            add("ObjectsScannedCount: ", c.objects_scanned_count.to_string());
            add("ObjectsMatchedCount: ", c.objects_matched_count.to_string());
            add(
                "LastScanned: ",
                format!("{}/{}", c.last_bucket_scanned, c.last_object_scanned),
            );
            add(
                "LastMatched: ",
                format!("{}/{}", c.last_bucket_matched, c.last_object_matched),
            );
            add("RecordsWrittenCount: ", c.records_written_count.to_string());
            add("OutputObjectsCount: ", c.output_objects_count.to_string());
            add("Elapsed: ", go_duration(round_seconds(elapsed)));
            let speed = c.objects_scanned_count as f64 / (elapsed as f64 / 1e9);
            add("Scan Speed: ", format!("{} objects/s", go_float(speed)));
            if !c.error_msg.is_empty() {
                add("Error: ", c.error_msg);
            }
        }
        _ => {}
    }
    out.push_str(&render_table(None, &rows));
    if quitting {
        out.push('\n');
    }
    out
}

// ------------------------------------------------------------------------------------------
// formatting helpers
// ------------------------------------------------------------------------------------------

/// olekukonko/tablewriter as mc configures it for batch output (no borders, left aligned,
/// tab padding, `SetNoWhiteSpace`): cells padded to the column width, each followed by a tab;
/// the last header cell is followed by a space instead.
fn render_table(headers: Option<&[String]>, rows: &[Vec<String>]) -> String {
    let columns = headers
        .map(<[String]>::len)
        .or_else(|| rows.first().map(Vec::len))
        .unwrap_or_default();
    let mut widths = vec![0; columns];
    for row in headers.into_iter().chain(rows.iter().map(Vec::as_slice)) {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let pad = |cell: &str, width: usize| format!("{cell:<width$}");
    let mut out = String::new();
    if let Some(headers) = headers {
        for (index, (cell, width)) in headers.iter().zip(&widths).enumerate() {
            out.push_str(&pad(cell, *width));
            out.push(if index + 1 == columns { ' ' } else { '\t' });
        }
        out.push('\n');
    }
    for row in rows {
        for (cell, width) in row.iter().zip(&widths) {
            out.push_str(&pad(cell, *width));
            out.push('\t');
        }
        out.push('\n');
    }
    out
}

/// RFC3339 time -> Unix nanoseconds (None when unparsable).
fn parse_time(value: &str) -> Option<i128> {
    let time = aws_smithy_types::DateTime::from_str(
        value,
        aws_smithy_types::date_time::Format::DateTimeWithOffset,
    )
    .ok()?;
    Some(time.secs() as i128 * 1_000_000_000 + time.subsec_nanos() as i128)
}

/// Go `a.Sub(b)` in nanoseconds (0 when a time does not parse).
fn time_diff_nanos(a: &str, b: &str) -> i64 {
    match (parse_time(a), parse_time(b)) {
        (Some(a), Some(b)) => (a - b).clamp(i64::MIN as i128, i64::MAX as i128) as i64,
        _ => 0,
    }
}

/// Go `time.Time.String()` of a UTC RFC3339 time: `2006-01-02 15:04:05.999999999 +0000 UTC`.
fn go_time_string(value: &str) -> String {
    match value.strip_suffix('Z') {
        Some(rest) => format!("{} +0000 UTC", rest.replacen('T', " ", 1)),
        None => value.to_string(),
    }
}

/// go-humanize `Time(then)` relative to `now` (`then` in Unix nanoseconds; None = Go's zero
/// time).
fn humanize_time(then: Option<i128>, now: SystemTime) -> String {
    const S: i128 = 1_000_000_000;
    const MIN: i128 = 60 * S;
    const HOUR: i128 = 60 * MIN;
    const DAY: i128 = 24 * HOUR;
    const WEEK: i128 = 7 * DAY;
    const MONTH: i128 = 30 * DAY;
    const YEAR: i128 = 12 * MONTH;
    let now = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i128)
        .unwrap_or_default();
    let then = then.unwrap_or(-62_135_596_800 * S);
    let (label, diff) = if then > now {
        ("from now", then - now)
    } else {
        ("ago", now - then)
    };
    // Go durations saturate at ~292 years.
    let diff = diff.min(i64::MAX as i128);
    let magnitudes: [(i128, &str, i128); 16] = [
        (S, "now", 1),
        (2 * S, "1 second", 1),
        (MIN, "seconds", S),
        (2 * MIN, "1 minute", 1),
        (HOUR, "minutes", MIN),
        (2 * HOUR, "1 hour", 1),
        (DAY, "hours", HOUR),
        (2 * DAY, "1 day", 1),
        (WEEK, "days", DAY),
        (2 * WEEK, "1 week", 1),
        (MONTH, "weeks", WEEK),
        (2 * MONTH, "1 month", 1),
        (YEAR, "months", MONTH),
        (18 * MONTH, "1 year", 1),
        (2 * YEAR, "2 years", 1),
        (37 * YEAR, "years", YEAR),
    ];
    for (limit, text, div) in magnitudes {
        if diff < limit {
            return match text {
                "now" => "now".to_string(),
                _ if div == 1 => format!("{text} {label}"),
                _ => format!("{} {text} {label}", diff / div),
            };
        }
    }
    format!("a long while {label}")
}

fn humanize_ibytes(size: u64) -> String {
    crate::commands::util::humanize_ibytes(size)
}

/// Go `d.Round(time.Second)` (halves away from zero).
fn round_seconds(nanos: i64) -> i64 {
    let s = 1_000_000_000;
    let r = nanos % s;
    if r.abs() * 2 >= s {
        nanos - r + if nanos < 0 { -s } else { s }
    } else {
        nanos - r
    }
}

/// Go `time.Duration.String()`.
fn go_duration(nanos: i64) -> String {
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
    let seconds = frac((secs % 60) * 1_000_000_000 + u % 1_000_000_000, 9);
    let (h, m) = (secs / 3600, secs / 60 % 60);
    if h > 0 {
        format!("{sign}{h}h{m}m{seconds}s")
    } else if secs >= 60 {
        format!("{sign}{m}m{seconds}s")
    } else {
        format!("{sign}{seconds}s")
    }
}

/// Go `%f` including `+Inf` / `NaN`.
fn go_float(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value.is_infinite() {
        if value > 0.0 { "+Inf" } else { "-Inf" }.into()
    } else {
        format!("{value:.6}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::batch::{ExpirationInfo, ReplicateInfo};

    #[test]
    fn list_table_matches_tablewriter() {
        let headers = ["ID", "TYPE", "STATUS"].map(String::from);
        let rows = vec![
            vec![
                "replicate-a:-1".into(),
                "replicate".into(),
                "completed".into(),
            ],
            vec!["expire-b:-1".into(), "expire".into(), "failed".into()],
        ];
        assert_eq!(
            render_table(Some(&headers), &rows),
            "ID            \tTYPE     \tSTATUS    \n\
             replicate-a:-1\treplicate\tcompleted\t\n\
             expire-b:-1   \texpire   \tfailed   \t\n"
        );
    }

    #[test]
    fn humanizes_like_go_humanize() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let at = |secs_ago: i128| Some((1_000_000_000 - secs_ago) * 1_000_000_000);
        assert_eq!(humanize_time(at(0), now), "now");
        assert_eq!(humanize_time(at(1), now), "1 second ago");
        assert_eq!(humanize_time(at(7), now), "7 seconds ago");
        assert_eq!(humanize_time(at(150), now), "2 minutes ago");
        assert_eq!(humanize_time(at(3 * 3600), now), "3 hours ago");
        assert_eq!(humanize_time(at(-90), now), "1 minute from now");
        assert_eq!(humanize_time(at(20 * 86400), now), "2 weeks ago");
        assert_eq!(humanize_time(None, now), "a long while ago");
    }

    #[test]
    fn go_time_and_durations() {
        assert_eq!(
            go_time_string("2026-09-26T19:24:35.169155666Z"),
            "2026-09-26 19:24:35.169155666 +0000 UTC"
        );
        assert_eq!(go_duration(0), "0s");
        assert_eq!(go_duration(12_010_000), "12.01ms");
        assert_eq!(go_duration(61_500_000_000), "1m1.5s");
        assert_eq!(go_duration(round_seconds(1_600_000_000)), "2s");
        assert_eq!(go_duration(round_seconds(12_090_240)), "0s");
        assert_eq!(
            time_diff_nanos(
                "2026-09-26T19:24:35.181245846Z",
                "2026-09-26T19:24:35.169155666Z"
            ),
            12_090_180
        );
        assert_eq!(go_float(f64::NAN), "NaN");
        assert_eq!(go_float(1.5), "1.500000");
    }

    #[test]
    fn status_view_matches_mc() {
        let metric = JobMetric {
            job_id: "replicate-x".into(),
            job_type: "replicate".into(),
            start_time: "2026-09-26T19:24:35.169155666Z".into(),
            last_update: "2026-09-26T19:24:35.181245846Z".into(),
            complete: true,
            replicate: Some(ReplicateInfo {
                last_object: "d/b.txt".into(),
                objects: 2,
                bytes_transferred: 7,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            status_view_text(&metric, None, true),
            "✔ ✔ ✔ \n\
             JobType:            \treplicate    \t\n\
             Objects:            \t2            \t\n\
             Versions:           \t2            \t\n\
             FailedObjects:      \t0            \t\n\
             DeleteMarker:       \t0            \t\n\
             FailedDeleteMarker: \t0            \t\n\
             Throughput:         \t578 B/s      \t\n\
             IOPs:               \t165.42 objs/s\t\n\
             Transferred:        \t7 B          \t\n\
             Elapsed:            \t0s           \t\n\
             CurrObjName:        \td/b.txt      \t\n\
             \n"
        );
        let expire = JobMetric {
            job_type: "expire".into(),
            failed: true,
            expired: Some(ExpirationInfo::default()),
            ..Default::default()
        };
        let view = status_view_text(&expire, None, true);
        assert!(view.starts_with("✗ ✗ ✗ \nJobType:"));
        assert!(!view.contains("Elapsed"));
        assert_eq!(
            status_view_text(&JobMetric::default(), Some("∙∙∙"), false),
            "∙∙∙\n"
        );
    }

    #[test]
    fn job_states() {
        let mut metric = JobMetric::default();
        assert_eq!(job_state(&metric), "in-progress");
        metric.complete = true;
        assert_eq!(job_state(&metric), "completed");
        metric.failed = true;
        assert_eq!(job_state(&metric), "failed");
    }

    #[cfg(not(windows))] // Windows OS error texts differ
    #[test]
    fn path_errors_look_like_go() {
        let err = read_file("/nonexistent/job.yaml").unwrap_err();
        assert_eq!(
            err.to_string(),
            "open /nonexistent/job.yaml: no such file or directory"
        );
        assert_eq!(
            serde_json::to_string(&err.detail).unwrap(),
            r#"{"Op":"open","Path":"/nonexistent/job.yaml","Err":2}"#
        );
    }
}
