//! `mx replicate add|update|ls|status|resync|export|import|rm|backlog` (MinIO bucket
//! replication: remote targets via the admin API, rules via `?replication`).

use crate::commands::ilm_tier::{Align, render_table};
use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::config::model::ConfigV10;
use crate::error::McError;
use crate::output;
use crate::s3::admin::{self, AdminClient};
use crate::s3::replication::{
    self, BucketTarget, DiffInfo, MetricsV2, ReplQNodeStats, ReplicationConfig, RuleOptions,
    TargetCredentials,
};
use crate::target::TargetRef;
use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{IsTerminal, Read};

#[derive(Debug, Args)]
pub struct ReplicateArgs {
    #[command(subcommand)]
    pub command: ReplicateCommand,
}

#[derive(Debug, Subcommand)]
pub enum ReplicateCommand {
    #[command(about = "add a server side replication configuration rule")]
    Add(ReplicateAddArgs),
    #[command(
        alias = "edit",
        about = "modify an existing server side replication configuration rule"
    )]
    Update(ReplicateUpdateArgs),
    #[command(
        visible_alias = "list",
        about = "list server side replication configuration rules"
    )]
    Ls(ReplicateListArgs),
    #[command(about = "show server side replication status")]
    Status(ReplicateStatusArgs),
    #[command(
        alias = "reset",
        about = "re-replicate all previously replicated objects"
    )]
    Resync(ReplicateResyncArgs),
    #[command(about = "export server side replication configuration")]
    Export(ReplicateTargetArgs),
    #[command(about = "import server side replication configuration in JSON format")]
    Import(ReplicateTargetArgs),
    #[command(
        visible_alias = "remove",
        about = "remove a server side replication configuration rule"
    )]
    Rm(ReplicateRemoveArgs),
    #[command(alias = "diff", about = "show unreplicated object versions")]
    Backlog(ReplicateBacklogArgs),
}

#[derive(Debug, Args)]
pub struct ReplicateTargetArgs {
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ReplicateAddArgs {
    pub target: String,
    /// id for the rule, should be a unique value
    #[arg(long)]
    pub id: Option<String>,
    /// format '<key1>=<value1>&<key2>=<value2>&<key3>=<value3>', multiple values allowed for multiple key/value pairs
    #[arg(long)]
    pub tags: Option<String>,
    /// storage class for destination, valid values are either "STANDARD" or "REDUCED_REDUNDANCY"
    #[arg(long)]
    pub storage_class: Option<String>,
    /// disable the rule
    #[arg(long)]
    pub disable: bool,
    /// priority of the rule, should be unique and is a required field
    #[arg(long, default_value_t = 0)]
    pub priority: i64,
    /// remote bucket, should be a unique value for the configuration
    #[arg(long)]
    pub remote_bucket: Option<String>,
    /// comma separated list to enable replication of soft deletes, permanent deletes, existing objects and metadata sync
    #[arg(
        long,
        default_value = "delete-marker,delete,existing-objects,metadata-sync"
    )]
    pub replicate: String,
    /// bucket path lookup supported by the server. Valid options are ['auto', 'on', 'off']'
    #[arg(long, default_value = "auto")]
    pub path: String,
    /// region of the destination bucket (optional)
    #[arg(long)]
    pub region: Option<String>,
    /// set bandwidth limit in bytes per second (K,B,G,T for metric and Ki,Bi,Gi,Ti for IEC units)
    #[arg(long)]
    pub bandwidth: Option<String>,
    /// enable synchronous replication for this target. default is async
    #[arg(long)]
    pub sync: bool,
    /// health check interval in seconds
    #[arg(long, default_value_t = 60)]
    pub healthcheck_seconds: u64,
    /// disable proxying in active-active replication. If unset, default behavior is to proxy
    #[arg(long)]
    pub disable_proxy: bool,
}

#[derive(Debug, Args)]
pub struct ReplicateUpdateArgs {
    pub target: String,
    /// id for the rule, should be a unique value
    #[arg(long)]
    pub id: Option<String>,
    /// format '<key1>=<value1>&<key2>=<value2>&<key3>=<value3>', multiple values allowed for multiple key/value pairs
    #[arg(long)]
    pub tags: Option<String>,
    /// storage class for destination, valid values are ['STANDARD', 'REDUCED_REDUNDANCY']
    #[arg(long)]
    pub storage_class: Option<String>,
    /// change rule status, valid values are ['enable', 'disable']
    #[arg(long)]
    pub state: Option<String>,
    /// priority of the rule, should be unique and is a required field (default: 0)
    #[arg(long)]
    pub priority: Option<i64>,
    /// destination bucket, should be a unique value for the configuration
    #[arg(long)]
    pub remote_bucket: Option<String>,
    /// comma separated list to enable replication of soft deletes, permanent deletes, existing objects and metadata sync. Valid options are "delete-marker","delete","existing-objects","metadata-sync" and ""'
    #[arg(long)]
    pub replicate: Option<String>,
    /// enable synchronous replication for this target, valid values are ['enable', 'disable']. (default: "disable")
    #[arg(long)]
    pub sync: Option<String>,
    /// enable proxying in active-active replication, valid values are ['enable', 'disable'] (default: "enable")
    #[arg(long)]
    pub proxy: Option<String>,
    /// Set bandwidth limit in bytes per second (K,B,G,T for metric and Ki,Bi,Gi,Ti for IEC units)
    #[arg(long)]
    pub bandwidth: Option<String>,
    /// health check duration in seconds (default: 60)
    #[arg(long)]
    pub healthcheck_seconds: Option<u64>,
    /// bucket path lookup supported by the server, valid options are ['on', 'off', 'auto'] (default: "auto")
    #[arg(long)]
    pub path: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReplicateListArgs {
    pub target: String,
    /// show rules by status. Valid options are [enabled,disabled]
    #[arg(long)]
    pub status: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReplicateStatusArgs {
    pub target: String,
    /// show most recent failures for one or more nodes. Valid values are 'all', or node name
    #[arg(short = 'b', long, default_value = "all")]
    pub backlog: String,
    /// show replication speed for all nodes
    #[arg(short = 'n', long)]
    pub nodes: bool,
}

#[derive(Debug, Args)]
pub struct ReplicateRemoveArgs {
    pub target: String,
    /// id for the rule, should be a unique value
    #[arg(long)]
    pub id: Option<String>,
    /// force remove all the replication configuration rules on the bucket
    #[arg(long)]
    pub force: bool,
    /// remove all replication configuration rules of the bucket, force flag enforced
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct ReplicateBacklogArgs {
    pub target: String,
    /// unique role ARN
    #[arg(long)]
    pub arn: Option<String>,
    /// include replicated versions
    #[arg(short = 'v', long)]
    pub verbose: bool,
    /// show most recent failures for one or more nodes. Valid values are 'all', or node name
    #[arg(short = 'n', long, default_value = "all")]
    pub nodes: String,
    /// list and show all replication failures for bucket
    #[arg(short = 'a', long)]
    pub full: bool,
}

#[derive(Debug, Args)]
pub struct ReplicateResyncArgs {
    #[command(subcommand)]
    pub command: ResyncCommand,
}

#[derive(Debug, Subcommand)]
pub enum ResyncCommand {
    #[command(about = "start replicating back all previously replicated objects")]
    Start(ResyncStartArgs),
    #[command(about = "status of replication recovery")]
    Status(ResyncTargetArgs),
    #[command(about = "cancel an ongoing replication resync")]
    Cancel(ResyncTargetArgs),
}

#[derive(Debug, Args)]
pub struct ResyncStartArgs {
    pub target: String,
    /// replicate back objects older than value in duration string (e.g. 7d10h31s)
    #[arg(long)]
    pub older_than: Option<String>,
    /// remote bucket ARN
    #[arg(long)]
    pub remote_bucket: Option<String>,
}

#[derive(Debug, Args)]
pub struct ResyncTargetArgs {
    pub target: String,
    /// remote bucket ARN
    #[arg(long)]
    pub remote_bucket: Option<String>,
}

pub fn run(args: ReplicateArgs, json: bool) -> Result<()> {
    match args.command {
        ReplicateCommand::Add(args) => add(args, json),
        ReplicateCommand::Update(args) => update(args, json),
        ReplicateCommand::Ls(args) => list(args, json),
        ReplicateCommand::Status(args) => status(args, json),
        ReplicateCommand::Resync(args) => match args.command {
            ResyncCommand::Start(args) => resync_start(args, json),
            ResyncCommand::Status(args) => resync_status(args, json),
            ResyncCommand::Cancel(args) => resync_cancel(args, json),
        },
        ReplicateCommand::Export(args) => export(args, json),
        ReplicateCommand::Import(args) => import(args, json),
        ReplicateCommand::Rm(args) => remove(args, json),
        ReplicateCommand::Backlog(args) => backlog(args, json),
    }
}

/// Source bucket context: admin client, bucket, prefix (object part of the target).
struct Source {
    store: ConfigStore,
    client: AdminClient,
    bucket: String,
    prefix: String,
}

fn source(target: &str) -> Result<Source> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target_ref) = require_s3(&store, target)?;
    let bucket = target_ref.require_bucket()?.to_string();
    if bucket.is_empty() {
        bail!("bucket not specified in `{target}`.");
    }
    let prefix = target_ref.key_with_trailing_slash().unwrap_or_default();
    Ok(Source {
        client: AdminClient::new(&alias)?,
        store,
        bucket,
        prefix,
    })
}

/// mc-style JSON message: `op`, `status`, `url`, then the command specific fields.
#[derive(Serialize)]
struct Message<'a, T: Serialize> {
    op: &'a str,
    status: &'static str,
    url: &'a str,
    #[serde(flatten)]
    body: T,
}

fn print_json<T: Serialize>(op: &str, url: &str, body: T) -> Result<()> {
    let message = Message {
        op,
        status: "success",
        url,
        body,
    };
    crate::output::print_json(&message)?;
    Ok(())
}

fn op_message(op: &str, url: &str, id: &str, json: bool, text: String) -> Result<()> {
    if json {
        print_json(op, url, json!({ "id": id }))
    } else {
        output::print_plain(&text);
        Ok(())
    }
}

/// Parses `--remote-bucket` (mc `extractCredentialURL`): returns access key, secret key and URL.
fn remote_url(config: &ConfigV10, input: &str) -> Result<(String, String, url::Url)> {
    let (access_key, secret_key, url_text) = if let Some(scheme_end) = ["http://", "https://"]
        .iter()
        .find(|scheme| input.starts_with(**scheme))
        .map(|scheme| scheme.len())
    {
        let rest = &input[scheme_end..];
        let Some((userinfo, host)) = rest.split_once('@') else {
            bail!("no valid credentials were detected in `{input}`");
        };
        let parts: Vec<&str> = userinfo.split(':').collect();
        match parts.as_slice() {
            [access, secret] if !access.is_empty() => (
                access.to_string(),
                secret.to_string(),
                format!("{}{host}", &input[..scheme_end]),
            ),
            [_, _, _, ..] => bail!("temporary tokens are not allowed for remote targets"),
            // Never echo the userinfo: it holds the secret key.
            _ => bail!(
                "unsupported remote target format `{}{host}`, see --help",
                &input[..scheme_end]
            ),
        }
    } else {
        let target = TargetRef::parse(input)?;
        let alias = config
            .aliases
            .get(&target.alias)
            .ok_or_else(|| anyhow!("No such alias `{}` found.", target.alias))?;
        let bucket = target.require_bucket()?;
        (
            alias.access_key.clone(),
            alias.secret_key.clone(),
            format!("{}/{bucket}", alias.url.trim_end_matches('/')),
        )
    };
    let url = url::Url::parse(&url_text)
        .map_err(|err| anyhow!("unsupported URL format `{url_text}`: {err}"))?;
    Ok((access_key, secret_key, url))
}

fn remote_bucket_name(url: &url::Url) -> Result<String> {
    let bucket = url.path().trim_matches('/').to_string();
    if bucket.is_empty() || bucket.contains('/') || bucket.len() < 3 || bucket.len() > 63 {
        bail!("invalid target bucket `{bucket}` in remote bucket URL");
    }
    Ok(bucket)
}

fn url_endpoint(url: &url::Url, default_port: bool) -> Result<String> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("remote bucket URL `{url}` has no host"))?;
    Ok(match (url.port(), default_port) {
        (Some(port), _) => format!("{host}:{port}"),
        (None, true) => format!("{host}:{}", url.port_or_known_default().unwrap_or(80)),
        (None, false) => host.to_string(),
    })
}

fn validate_path(path: &str) -> Result<()> {
    if !matches!(path, "auto" | "on" | "off") {
        bail!("unrecognized bucket path style `{path}`. Valid options are `[on, off, auto]`.");
    }
    Ok(())
}

fn bandwidth(value: Option<&str>) -> Result<i64> {
    match value.filter(|v| !v.is_empty()) {
        Some(value) => {
            Ok(crate::flags::parse_size(value).context("invalid bandwidth value")? as i64)
        }
        None => Ok(0),
    }
}

/// `--replicate` flags: (delete markers, deletes, replica metadata sync, existing objects).
fn replicate_flags(value: &str) -> Result<(bool, bool, bool, bool)> {
    let mut flags = (false, false, false, false);
    for opt in value.split(',') {
        match opt.trim().to_ascii_lowercase().as_str() {
            "delete-marker" => flags.0 = true,
            "delete" => flags.1 = true,
            "metadata-sync" | "replica-metadata-sync" => flags.2 = true,
            "existing-objects" => flags.3 = true,
            "" => {}
            _ => bail!(
                "invalid value for --replicate flag `{value}`: use one or more of \"delete\", \"delete-marker\", \"metadata-sync\", \"existing-objects\" or \"\" to disable these settings"
            ),
        }
    }
    Ok(flags)
}

fn add(args: ReplicateAddArgs, json: bool) -> Result<()> {
    let Some(remote) = args.remote_bucket.as_deref().filter(|r| !r.is_empty()) else {
        bail!("--remote-bucket flag needs to be specified.");
    };
    validate_path(&args.path)?;
    let (delete_markers, deletes, _metadata_sync, existing) = replicate_flags(&args.replicate)?;
    let src = source(&args.target)?;
    let (access_key, secret_key, url) = remote_url(src.store.config(), remote)?;
    let target_bucket = remote_bucket_name(&url)?;
    let target = BucketTarget {
        source_bucket: src.bucket.clone(),
        endpoint: url_endpoint(&url, false)?,
        credentials: Some(TargetCredentials {
            access_key,
            secret_key,
            ..Default::default()
        }),
        target_bucket: target_bucket.clone(),
        secure: url.scheme() == "https",
        path: args.path.clone(),
        api: "s3v4".into(),
        target_type: "replication".into(),
        region: args.region.clone().unwrap_or_default(),
        bandwidth_limit: bandwidth(args.bandwidth.as_deref())?,
        replication_sync: args.sync,
        health_check_duration: (args.healthcheck_seconds * 1_000_000_000) as i64,
        disable_proxy: args.disable_proxy,
        ..Default::default()
    };
    let mut opts = RuleOptions {
        id: args.id.clone().unwrap_or_default(),
        prefix: src.prefix.clone(),
        enabled: Some(!args.disable),
        priority: Some(args.priority),
        tags: args.tags.clone(),
        storage_class: args.storage_class.clone(),
        dest_bucket: format!("arn:minio:replication::pending:{target_bucket}"),
        delete_markers: Some(delete_markers),
        deletes: Some(deletes),
        // mc always enables replica metadata sync on add.
        replica_sync: Some(true),
        existing_objects: Some(existing),
    };

    let rt = runtime()?;
    let mut config = rt
        .block_on(replication::get_replication(&src.client, &src.bucket))
        .context("unable to fetch replication configuration")?;
    // Validate the rule before creating the remote target.
    replication::add_rule(&mut config.clone(), &opts).context("unable to add replication rule")?;
    let arn = rt
        .block_on(replication::set_remote_target(
            &src.client,
            &src.bucket,
            &target,
        ))
        .context("unable to configure remote target")?;
    opts.dest_bucket = arn;
    replication::add_rule(&mut config, &opts).context("unable to add replication rule")?;
    rt.block_on(replication::put_replication(
        &src.client,
        &src.bucket,
        &config,
    ))
    .context("unable to add replication rule")?;

    let id = args.id.unwrap_or_default();
    let text = if id.is_empty() {
        format!(
            "Replication configuration rule applied to {} successfully.",
            args.target
        )
    } else {
        format!(
            "Replication configuration rule with ID `{id}` applied to {}.",
            args.target
        )
    };
    op_message("add", &args.target, &id, json, text)
}

fn enable_disable(flag: &str, value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "enable" => Ok(true),
        "disable" => Ok(false),
        _ => bail!("--{flag} can be either `enable` or `disable`"),
    }
}

fn update(args: ReplicateUpdateArgs, json: bool) -> Result<()> {
    let Some(id) = args.id.clone().filter(|id| !id.is_empty()) else {
        bail!("--id is a required flag");
    };
    let enabled = args
        .state
        .as_deref()
        .map(|s| enable_disable("state", s))
        .transpose()?;
    if let Some(path) = &args.path {
        validate_path(path)?;
    }
    let sync = args
        .sync
        .as_deref()
        .map(|s| enable_disable("sync", s))
        .transpose()?;
    let proxy = args
        .proxy
        .as_deref()
        .map(|s| enable_disable("proxy", s))
        .transpose()?;
    let flags = args.replicate.as_deref().map(replicate_flags).transpose()?;
    let target_settings = sync.is_some()
        || proxy.is_some()
        || args.bandwidth.is_some()
        || args.healthcheck_seconds.is_some()
        || args.path.is_some();
    if args.remote_bucket.is_none() && target_settings {
        bail!(
            "--remote-bucket is a required flag with --sync, --proxy, --bandwidth, --healthcheck-seconds or --path"
        );
    }

    let src = source(&args.target)?;
    let rt = runtime()?;
    let mut config = rt
        .block_on(replication::get_replication(&src.client, &src.bucket))
        .context("unable to get replication configuration")?;
    let arn = config
        .rules
        .iter()
        .find(|rule| rule.id == id)
        .map(|rule| {
            if config.role.is_empty() {
                rule.destination.bucket.clone()
            } else {
                config.role.clone()
            }
        })
        .unwrap_or_default();

    if let Some(remote) = args.remote_bucket.as_deref() {
        if arn.is_empty() {
            bail!("rule with ID {id} not found in replication configuration");
        }
        let targets = rt
            .block_on(replication::list_remote_targets(&src.client, &src.bucket))
            .context("unable to fetch remote target")?;
        let mut target = targets
            .into_iter()
            .find(|t| t.arn == arn)
            .ok_or_else(|| anyhow!("`{arn}` not found in replication config"))?;
        let mut ops = Vec::new();
        if let Some(sync) = sync {
            target.replication_sync = sync;
            ops.push("sync");
        }
        if let Some(proxy) = proxy {
            target.disable_proxy = !proxy;
            ops.push("proxy");
        }
        let (access_key, secret_key, url) = remote_url(src.store.config(), remote)?;
        let target_bucket = remote_bucket_name(&url)?;
        if target_bucket != target.target_bucket {
            bail!(
                "configured remote target bucket `{target_bucket}` does not match `{}` for this ARN `{}`",
                target.target_bucket,
                target.arn
            );
        }
        if src.bucket != target.source_bucket {
            bail!(
                "configured source bucket `{}` does not match `{}` for this ARN `{}`",
                src.bucket,
                target.source_bucket,
                target.arn
            );
        }
        target.secure = url.scheme() == "https";
        target.endpoint = url_endpoint(&url, true)?;
        target.credentials = Some(TargetCredentials {
            access_key,
            secret_key,
            ..Default::default()
        });
        ops.push("creds");
        if args.bandwidth.is_some() {
            target.bandwidth_limit = bandwidth(args.bandwidth.as_deref())?;
            ops.push("bandwidth");
        }
        if let Some(seconds) = args.healthcheck_seconds {
            target.health_check_duration = (seconds * 1_000_000_000) as i64;
            ops.push("healthcheck");
        }
        if let Some(path) = &args.path {
            target.path = path.clone();
            ops.push("path");
        }
        rt.block_on(replication::update_remote_target(
            &src.client,
            &target,
            &ops,
        ))
        .with_context(|| {
            format!(
                "Unable to update remote target `{}` from `{}` -> `{}`",
                target.endpoint, target.source_bucket, target.target_bucket
            )
        })?;
    }

    let opts = RuleOptions {
        id: id.clone(),
        prefix: src.prefix.clone(),
        enabled,
        priority: args.priority,
        tags: args.tags.clone(),
        storage_class: args.storage_class.clone(),
        dest_bucket: arn,
        delete_markers: flags.map(|f| f.0),
        deletes: flags.map(|f| f.1),
        replica_sync: flags.map(|f| f.2),
        existing_objects: flags.map(|f| f.3),
    };
    replication::edit_rule(&mut config, &opts).context("unable to modify replication rule")?;
    rt.block_on(replication::put_replication(
        &src.client,
        &src.bucket,
        &config,
    ))
    .context("unable to modify replication rule")?;
    let text = format!(
        "Replication configuration rule with ID `{id}` applied to {}.",
        args.target
    );
    op_message("update", &args.target, &id, json, text)
}

/// Bucket part of `arn:minio:replication:REGION:ID:BUCKET`.
fn arn_bucket(arn: &str) -> &str {
    let tokens: Vec<&str> = arn.split(':').collect();
    if tokens.len() == 6 && tokens[0] == "arn" {
        tokens[5]
    } else {
        arn
    }
}

fn list(args: ReplicateListArgs, json: bool) -> Result<()> {
    let src = source(&args.target)?;
    let rt = runtime()?;
    let config = rt
        .block_on(replication::get_replication(&src.client, &src.bucket))
        .context("Unable to get replication configuration")?;
    if config.rules.is_empty() {
        return Err(
            anyhow::Error::new(McError::new("replication configuration not set"))
                .context("Unable to list replication configuration"),
        );
    }
    if !json {
        println!("Rules:");
    }
    let targets = rt
        .block_on(replication::list_remote_targets(&src.client, &src.bucket))
        .context("Unable to fetch remote target.")?;
    let status_filter = args.status.unwrap_or_default();
    for rule in &config.rules {
        if !status_filter.is_empty() && !status_filter.eq_ignore_ascii_case(&rule.status) {
            continue;
        }
        if json {
            // mc never fills `op`/`url` for `replicate ls`.
            #[derive(Serialize)]
            struct Body<'a> {
                rule: &'a replication::Rule,
            }
            print_json("", "", Body { rule })?;
            continue;
        }
        let arn = &rule.destination.bucket;
        let endpoint = targets
            .iter()
            .find(|t| &t.arn == arn)
            .map(|t| t.endpoint.as_str())
            .unwrap_or(arn);
        let mut text = format!("Remote Bucket: {endpoint}/{}\n", arn_bucket(arn));
        text.push_str(&format!("  Rule ID: {}\n", rule.id));
        text.push_str(&format!("  Priority: {}\n", rule.priority));
        text.push_str(&format!("  ARN: {arn}\n"));
        if !rule.filter.and.prefix.is_empty() {
            text.push_str(&format!("  Prefix: {}\n", rule.filter.and.prefix));
        }
        if !rule.tags().is_empty() {
            text.push_str(&format!("  Tags: {}\n", rule.tags()));
        }
        let class = &rule.destination.storage_class;
        if !class.is_empty() && class != "STANDARD" {
            text.push_str(&format!("  StorageClass: {class}\n"));
        }
        println!("{text}");
    }
    Ok(())
}

/// go-humanize `RelTime(now, now+uptime, "", "")` for a number of seconds (keeps Go's
/// trailing space for the empty label, e.g. `2 minutes `).
pub(crate) fn rel_time(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 12 * MONTH;
    let diff = seconds.abs();
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
    format!("{text} ")
}

/// mc `timeDurationToHumanizedDuration(d).String()`.
pub(crate) fn humanized_duration(nanos: i64) -> String {
    let millis = nanos / 1_000_000;
    let secs = nanos as f64 / 1e9;
    if millis < 1000 {
        return format!("{millis} milliseconds");
    }
    if secs < 60.0 {
        return format!("{} seconds", secs as i64);
    }
    let (s, m, h) = (
        (secs % 60.0) as i64,
        (secs / 60.0 % 60.0) as i64,
        (secs / 3600.0) as i64,
    );
    if secs < 3600.0 {
        return format!("{} minutes {s} seconds", (secs / 60.0) as i64);
    }
    if secs < 86400.0 {
        return format!("{h} hours {m} minutes {s} seconds");
    }
    format!(
        "{} days {} hours {m} minutes {s} seconds",
        (secs / 86400.0) as i64,
        h % 24
    )
}

/// Go `time.Duration.String()`.
pub(crate) fn go_duration_string(nanos: i64) -> String {
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

/// Go `d.Round(time.Millisecond)`.
fn round_millis(nanos: i64) -> i64 {
    let m = 1_000_000;
    let r = nanos.rem_euclid(m);
    if r + r < m { nanos - r } else { nanos + m - r }
}

fn comma(value: f64) -> String {
    admin::comma(value as i64)
}

/// olekukonko/tablewriter single-column table without borders (mc `replicate status`):
/// each line is `  CELL  ` padded to the widest line.
fn tablewriter_column(rows: &[String]) -> String {
    let lines: Vec<&str> = rows.iter().flat_map(|row| row.split('\n')).collect();
    let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    lines
        .iter()
        .map(|line| {
            let pad = width - line.chars().count();
            format!("  {line}{}  \n", " ".repeat(pad))
        })
        .collect()
}

const DOT: &str = "●";

/// mc `replicateStatusMessage.String()`.
fn status_text(
    metrics: &MetricsV2,
    targets: &[BucketTarget],
    config: &ReplicationConfig,
) -> String {
    if config.rules.is_empty() {
        return "Replication is not configured.".to_string();
    }
    let rs = &metrics.current_stats;
    let empty = Default::default();
    let stats = rs.stats.as_ref().unwrap_or(&empty);
    let qs = metrics.queue_stats.qstats();
    let is_configured =
        |arn: &str| config.rules.iter().any(|r| r.destination.bucket == arn) || config.role == arn;
    let (mut repl_size, mut repl_count) = (rs.replicated_size as i128, rs.replicated_count as i128);
    for (arn, st) in stats {
        if !is_configured(arn) {
            repl_size -= st.replicated_size as i128;
            repl_count -= st.replicated_count as i128;
        }
    }
    let (repl_size, repl_count) = (repl_size.max(0) as u64, repl_count.max(0) as i64);
    let q = &rs.qstats;
    let queued = format!(
        "Queued:                       {DOT} {} objects, {} (avg: {} objects, {} ; max: {} objects, {})",
        comma(q.curr.count),
        admin::ibytes(q.curr.bytes as u64),
        comma(q.avg.count),
        admin::ibytes(q.avg.bytes as u64),
        comma(q.peak.count),
        admin::ibytes(q.peak.bytes as u64)
    );
    let workers = format!(
        "Workers:                      {} (avg: {}; max: {})",
        admin::comma(qs.workers.curr as i64),
        admin::comma(qs.workers.avg as i64),
        admin::comma(qs.workers.max as i64)
    );
    let errors = |failed: &replication::TimedErrStats| {
        format!(
            "Errors:                       {} in last 1 minute; {} in last 1hr; {} since uptime",
            comma(failed.last_minute.count),
            comma(failed.last_hour.count),
            comma(failed.totals.count)
        )
    };
    let rate = |value: f64| format!("{}/s", admin::si_bytes(value as u64));

    let mut rows = vec![format!(
        "Replication status since {}",
        rel_time(metrics.uptime)
    )];
    let single = stats.len() == 1;
    let mut stale = false;
    for (index, (arn, stat)) in stats.iter().enumerate() {
        if index > 0 && !stale {
            rows.push("\n".to_string());
        }
        stale = !is_configured(arn);
        if stale {
            continue;
        }
        let target = targets.iter().find(|t| &t.arn == arn);
        let default_target = BucketTarget::default();
        let tgt = target.unwrap_or(&default_target);
        let node = if tgt.endpoint.is_empty() {
            arn.as_str()
        } else {
            tgt.endpoint.as_str()
        };
        let mut current_downtime = 0i64;
        if !tgt.online && tgt.last_online != crate::s3::admin::GO_ZERO_TIME {
            current_downtime = since_nanos(&tgt.last_online);
        }
        let total_downtime = tgt.total_downtime.max(current_downtime);
        rows.push(node.to_string());
        rows.push(format!(
            "Replicated:                   {} objects ({})",
            admin::comma(stat.replicated_count as i64),
            admin::ibytes(stat.replicated_size)
        ));
        let link = if tgt.online {
            format!(
                "{DOT} online (total downtime: {})",
                humanized_duration(total_downtime)
            )
        } else {
            format!(
                "{DOT} offline {} (total downtime: {})",
                humanized_duration(current_downtime),
                humanized_duration(total_downtime)
            )
        };
        if single {
            rows.push(queued.clone());
            rows.push(workers.clone());
        }
        let xfer = qs
            .tgt_xfer_stats
            .get(arn)
            .and_then(|m| m.get("Total"))
            .cloned()
            .unwrap_or_default();
        // mc omits the closing parenthesis here.
        rows.push(format!(
            "Transfer Rate:                {} (avg: {}; max: {}",
            rate(xfer.curr_rate),
            rate(xfer.avg_rate),
            rate(xfer.peak_rate)
        ));
        let latency = |nanos: i64| go_duration_string(round_millis(nanos));
        rows.push(format!(
            "Latency:                      {} (avg: {}; max: {})",
            latency(tgt.latency.curr),
            latency(tgt.latency.avg),
            latency(tgt.latency.max)
        ));
        rows.push(format!("Link:                         {link}"));
        rows.push(errors(&stat.failed));
        if stat.bandwidth_limit > 0 {
            let current = if stat.current_bandwidth > 0.0 {
                rate(stat.current_bandwidth)
            } else {
                "N/A".to_string()
            };
            rows.push(format!(
                "Configured Max Bandwidth (Bps): {}   Current Bandwidth (Bps): {current}",
                rate(stat.bandwidth_limit as f64)
            ));
        }
    }
    if !single {
        let xfer = qs.xfer_stats.get("Total").cloned().unwrap_or_default();
        rows.push("\nSummary:".to_string());
        rows.push(format!(
            "Replicated:                   {} objects ({})",
            admin::comma(repl_count),
            admin::ibytes(repl_size)
        ));
        rows.push(queued);
        rows.push(workers);
        rows.push(format!(
            "Received:                     {} objects ({})",
            admin::comma(rs.replica_count),
            admin::ibytes(rs.replica_size)
        ));
        rows.push(format!(
            "Transfer Rate:                {} (avg: {}; max: {})",
            rate(xfer.curr_rate),
            rate(xfer.avg_rate),
            rate(xfer.peak_rate)
        ));
        rows.push(errors(&rs.errors));
    }
    tablewriter_column(&rows).trim_end_matches('\n').to_string()
}

/// Nanoseconds elapsed since an RFC 3339 time (0 when it does not parse).
fn since_nanos(time: &str) -> i64 {
    use aws_smithy_types::date_time::{DateTime, Format};
    let Ok(then) = DateTime::from_str(time, Format::DateTime) else {
        return 0;
    };
    let now = DateTime::from(std::time::SystemTime::now());
    let secs = now.secs() - then.secs();
    secs * 1_000_000_000 + (now.subsec_nanos() as i64 - then.subsec_nanos() as i64)
}

/// mc `PrettyTable.buildRow`: `%-N.Ns` columns (cut with `...`) joined by ` | `.
pub(crate) fn pretty_row(widths: &[usize], contents: &[&str]) -> String {
    let columns = widths.len().min(contents.len());
    (0..columns)
        .map(|i| {
            let width = widths[i];
            let content = contents[i];
            let cell = if content.len() > width {
                format!("{}...", &content[..width.saturating_sub(3)])
            } else {
                content.to_string()
            };
            let cell: String = cell.chars().take(width).collect();
            let pad = width - cell.chars().count();
            let sep = if i + 1 < columns { " | " } else { "" };
            format!("{cell}{}{sep}", " ".repeat(pad))
        })
        .collect()
}

/// mc `replicateXferMessage.String()` (`replicate status --nodes`).
fn nodes_text(nodes: &[ReplQNodeStats]) -> String {
    let max_len = nodes.iter().map(|n| n.node_name.len()).max().unwrap_or(0);
    let mut lines = vec![
        pretty_row(
            &[max_len + 3, 15, 25, 42, 12],
            &[
                "Node Name",
                "Uptime",
                "Label",
                "         Transfer Rate      ",
                "Workers",
            ],
        ),
        pretty_row(
            &[max_len + 3, 15, 25, 12, 12, 12, 10],
            &["", "", "", "Avg", "Peak", "Current", ""],
        ),
    ];
    for node in nodes {
        let uptime = rel_time(node.uptime);
        let workers = (node.workers.avg as i64).to_string();
        let empty = Default::default();
        let xfer = node.xfer_stats.as_ref().unwrap_or(&empty);
        for (metric, label) in [
            ("Large", "Large Objects (>=128 MiB)"),
            ("Small", "Small Objects (<128 MiB)"),
        ] {
            let x = xfer.get(metric).cloned().unwrap_or_default();
            let rate = |v: f64| format!("{}/s", admin::si_bytes(v as u64));
            lines.push(pretty_row(
                &[node.node_name.len() + 3, 15, 25, 12, 12, 12, 10],
                &[
                    &node.node_name,
                    &uptime,
                    label,
                    &rate(x.avg_rate),
                    &rate(x.peak_rate),
                    &rate(x.curr_rate),
                    &workers,
                ],
            ));
        }
    }
    lines.join("\n")
}

fn status(args: ReplicateStatusArgs, json: bool) -> Result<()> {
    let src = source(&args.target)?;
    let rt = runtime()?;
    let metrics = rt
        .block_on(replication::replication_metrics(&src.client, &src.bucket))
        .context("Unable to get replication status")?;
    let targets = rt
        .block_on(replication::list_remote_targets(&src.client, &src.bucket))
        .context("Unable to fetch remote target.")?;
    let config = rt
        .block_on(replication::get_replication(&src.client, &src.bucket))
        .context("Unable to fetch replication configuration.")?;
    if args.nodes {
        if json {
            #[derive(Serialize)]
            struct Xfer<'a> {
                op: &'a str,
                status: &'a str,
                nodes: &'a Option<Vec<ReplQNodeStats>>,
            }
            return output::print_json(&Xfer {
                op: "status",
                status: "success",
                nodes: &metrics.queue_stats.nodes,
            });
        }
        let nodes = metrics.queue_stats.nodes.as_deref().unwrap_or_default();
        println!("{}", nodes_text(nodes));
        return Ok(());
    }
    if json {
        #[derive(Serialize)]
        struct StatusMessage<'a> {
            op: &'a str,
            url: &'a str,
            status: &'a str,
            replicationstats: &'a MetricsV2,
            #[serde(rename = "remoteTargets")]
            remote_targets: Option<&'a [BucketTarget]>,
        }
        return output::print_json(&StatusMessage {
            op: "status",
            url: &args.target,
            status: "success",
            replicationstats: &metrics,
            remote_targets: (!targets.is_empty()).then_some(targets.as_slice()),
        });
    }
    println!("{}", status_text(&metrics, &targets, &config));
    Ok(())
}

/// mc `replicateResyncMessage`: `op`, `url`, `resyncInfo`, `status`, `targetArn`.
fn print_resync_json(op: &str, url: &str, info: &Value, arn: &str) -> Result<()> {
    #[derive(Serialize)]
    struct Resync<'a> {
        op: &'a str,
        url: &'a str,
        #[serde(rename = "resyncInfo")]
        resync_info: &'a Value,
        status: &'a str,
        #[serde(rename = "targetArn")]
        target_arn: &'a str,
    }
    output::print_json(&Resync {
        op,
        url,
        resync_info: info,
        status: "success",
        target_arn: arn,
    })
}

fn resync_start(args: ResyncStartArgs, json: bool) -> Result<()> {
    let Some(arn) = args.remote_bucket.as_deref().filter(|a| !a.is_empty()) else {
        bail!("--remote-bucket flag needs to be specified.");
    };
    let older_than = match args.older_than.as_deref().filter(|v| !v.is_empty()) {
        Some(value) => {
            let duration = crate::flags::parse_duration(value)
                .with_context(|| format!("Unable to parse older-than=`{value}`."))?;
            if duration.is_zero() {
                bail!("older-than cannot be set to zero");
            }
            Some(duration)
        }
        None => None,
    };
    let src = source(&args.target)?;
    let info = runtime()?
        .block_on(replication::resync_start(
            &src.client,
            &src.bucket,
            arn,
            older_than,
        ))
        .context("Unable to reset replication")?;
    if json {
        return print_resync_json("start", &args.target, &info, "");
    }
    let targets = info["target"].as_array().cloned().unwrap_or_default();
    if targets.len() == 1 {
        output::print_plain(&format!(
            "Replication reset started for {} with ID {}",
            args.target,
            targets[0]["resetid"].as_str().unwrap_or_default()
        ));
    } else {
        output::print_plain(&format!("Replication reset started for {}", args.target));
    }
    Ok(())
}

fn resync_status(args: ResyncTargetArgs, json: bool) -> Result<()> {
    let src = source(&args.target)?;
    let arn = args.remote_bucket.clone().unwrap_or_default();
    let info = runtime()?
        .block_on(replication::resync_status(&src.client, &src.bucket, &arn))
        .context("Unable to get replication resync status")?;
    if json {
        return print_resync_json("status", &args.target, &info, &arn);
    }
    let targets = info["target"].as_array().cloned().unwrap_or_default();
    if targets.is_empty() {
        println!("No replication resync status available.");
        return Ok(());
    }
    let num = |v: &Value| v.as_i64().unwrap_or_default();
    let mut out = String::from("Resync status summary:\n");
    for target in &targets {
        out.push_str(&format!(
            "● {}\n",
            target["arn"].as_str().unwrap_or_default()
        ));
        out.push_str(&format!(
            "   Status: {}\n",
            target["resyncStatus"].as_str().unwrap_or_default()
        ));
        out.push_str(&format!(
            "   {:<21} | {:<15} | {:<15}\n",
            "Replication Status", "Size (Bytes)", "Count"
        ));
        out.push_str(&format!(
            "   {:<21} | {:<15} | {:<15}\n",
            "Replicated",
            admin::ibytes(num(&target["completedReplicationSize"]) as u64),
            admin::comma(num(&target["replicationCount"]))
        ));
        out.push_str(&format!(
            "   {:<21} | {:<15} | {:<15}\n",
            "Failed",
            admin::ibytes(num(&target["failedReplicationSize"]) as u64),
            admin::comma(num(&target["failedReplicationCount"]))
        ));
    }
    print!("{out}");
    Ok(())
}

fn resync_cancel(args: ResyncTargetArgs, json: bool) -> Result<()> {
    let Some(arn) = args.remote_bucket.as_deref().filter(|a| !a.is_empty()) else {
        bail!("--remote-bucket flag needs to be specified.");
    };
    let src = source(&args.target)?;
    let id = runtime()?
        .block_on(replication::resync_cancel(&src.client, &src.bucket, arn))
        .context("Unable to cancel replication resync")?;
    if json {
        return print_json(
            "cancel",
            &args.target,
            json!({ "targetArn": arn, "resetID": id }),
        );
    }
    output::print_plain(&format!("Replication resync cancelled for {}", args.target));
    Ok(())
}

#[derive(Serialize)]
struct ConfigBody<'a> {
    config: &'a ReplicationConfig,
}

fn export(args: ReplicateTargetArgs, json: bool) -> Result<()> {
    let src = source(&args.target)?;
    let config = runtime()?
        .block_on(replication::get_replication(&src.client, &src.bucket))
        .context("Unable to get replication configuration")?;
    if json {
        return print_json("export", &args.target, ConfigBody { config: &config });
    }
    if config.rules.is_empty() {
        println!("No replication configuration found for {}.", args.target);
    } else {
        // mc marshals with colorjson: indented on a terminal, compact otherwise.
        println!("{}", output::json_string(&config)?);
    }
    Ok(())
}

fn import(args: ReplicateTargetArgs, json: bool) -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let config: ReplicationConfig = serde_json::Deserializer::from_str(&input)
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no replication configuration on standard input"))?
        .context("Unable to read replication configuration")?;
    let src = source(&args.target)?;
    runtime()?
        .block_on(replication::put_replication(
            &src.client,
            &src.bucket,
            &config,
        ))
        .context("Unable to set replication configuration")?;
    if json {
        // mc leaves the message's config empty.
        let empty = ReplicationConfig::default();
        return print_json("import", &args.target, ConfigBody { config: &empty });
    }
    output::print_plain(&format!(
        "Replication configuration successfully set on `{}`.",
        args.target
    ));
    Ok(())
}

fn remove(args: ReplicateRemoveArgs, json: bool) -> Result<()> {
    if args.all != args.force {
        bail!("It is mandatory to specify --all and --force flag together for mx replicate rm.");
    }
    let id = args.id.clone().unwrap_or_default();
    if !args.all && id.is_empty() {
        bail!("rule ID cannot be empty");
    }
    let src = source(&args.target)?;
    let rt = runtime()?;
    let mut config = rt
        .block_on(replication::get_replication(&src.client, &src.bucket))
        .context("Unable to get replication configuration")?;
    if args.all {
        rt.block_on(replication::delete_replication(&src.client, &src.bucket))
            .context("Unable to remove replication configuration")?;
    } else if !config.rules.is_empty() {
        let arn = replication::remove_rule(&mut config, &id)
            .context("Could not remove replication rule")?;
        rt.block_on(replication::put_replication(
            &src.client,
            &src.bucket,
            &config,
        ))
        .context("Could not remove replication rule")?;
        rt.block_on(replication::remove_remote_target(
            &src.client,
            &src.bucket,
            &arn,
        ))
        .context("Unable to remove remote target")?;
    }
    let text = if id.is_empty() {
        format!(
            "Replication configuration removed from {} successfully.",
            args.target
        )
    } else {
        format!(
            "Replication configuration rule with ID `{id}` removed from {}.",
            args.target
        )
    };
    // mc reports the command name (`remove`) as the op.
    op_message("remove", &args.target, &id, json, text)
}

/// mc renders `replicate backlog` with bubbletea, which needs a terminal for input: with a
/// non-terminal stdin it opens `/dev/tty` and fails without one. Returns that error.
fn backlog_tty_error() -> Option<anyhow::Error> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    let err = std::fs::File::open("/dev/tty").err()?;
    let text = err.to_string();
    let text = text.split(" (os error").next().unwrap_or(&text).to_string();
    let mut chars = text.chars();
    let text = chars
        .next()
        .map(|c| c.to_ascii_lowercase().to_string() + chars.as_str())
        .unwrap_or_default();
    Some(
        anyhow::Error::new(McError::new(format!(
            "could not open a new TTY: open /dev/tty: {text}"
        )))
        .context("Unable to fetch replication backlog"),
    )
}

/// mc `printDate` (`2006-01-02 15:04:05 MST`) for an RFC 3339 UTC time.
fn print_date(time: &str) -> String {
    match time.get(..19) {
        Some(prefix) if time.ends_with('Z') => format!("{} UTC", prefix.replace('T', " ")),
        _ => time.to_string(),
    }
}

fn backlog(args: ReplicateBacklogArgs, json: bool) -> Result<()> {
    let bucket = args.target.split('/').nth(1).unwrap_or_default();
    if bucket.is_empty() {
        return Err(anyhow::Error::new(McError::invalid_argument())
            .context(format!("bucket not specified in `{}`.", args.target)));
    }
    let src = source(&args.target)?;
    let rt = runtime()?;
    if !args.full {
        if !json && let Some(err) = backlog_tty_error() {
            return Err(err);
        }
        let entries = rt
            .block_on(replication::replication_mrf(
                &src.client,
                &src.bucket,
                &args.nodes,
            ))
            .map_err(|err| McError::new(err.to_string()))
            .context("Unable to fetch replication backlog.")?;
        if json {
            #[derive(Serialize)]
            struct Mrf<'a> {
                op: &'a str,
                status: &'a str,
                #[serde(flatten)]
                mrf: &'a replication::ReplicationMrf,
            }
            for entry in &entries {
                if !entry.err.is_empty() {
                    return Err(anyhow::Error::new(McError::new(entry.err.clone()))
                        .context("Unable to fetch replication backlog."));
                }
                output::print_json(&Mrf {
                    op: "mrf",
                    status: "success",
                    mrf: entry,
                })?;
            }
            return Ok(());
        }
        let rows: Vec<Vec<String>> = entries
            .iter()
            .filter(|e| !e.object.is_empty())
            .map(|e| {
                vec![
                    e.node_name.clone(),
                    e.version_id.clone(),
                    e.retry_count.to_string(),
                    format!("{}/{}", e.bucket, e.object),
                ]
            })
            .collect();
        if rows.is_empty() {
            println!("No recent replication failures found for {}.", args.target);
            return Ok(());
        }
        let headers = ["Node", "VersionID", "Retry", "Object"];
        println!("{}", render_table(&headers, &rows, |_, _| Align::Center));
        return Ok(());
    }

    let arn = args.arn.clone().unwrap_or_default();
    let entries = rt.block_on(replication::replication_diff(
        &src.client,
        &src.bucket,
        &src.prefix,
        &arn,
        args.verbose,
    ));
    if json {
        // madmin reports a failed request as one `DiffInfo` carrying the error.
        let entries = entries.unwrap_or_else(|err| {
            let detail = crate::error::mc_error(&err)
                .map(|e| e.detail.clone())
                .unwrap_or_default();
            vec![DiffInfo {
                error: Some(detail),
                ..Default::default()
            }]
        });
        for entry in &entries {
            output::print_json(entry)?;
        }
        return Ok(());
    }
    if let Some(err) = backlog_tty_error() {
        return Err(err);
    }
    let entries = entries.context("Unable to fetch replication backlog")?;
    let rows: Vec<Vec<String>> = entries
        .iter()
        .filter(|d| !d.object.is_empty())
        .map(|d| {
            let op = match (d.version_id.is_empty(), d.is_delete_marker) {
                (true, _) => "",
                (false, true) => "DEL",
                (false, false) => "PUT",
            };
            let status = if arn.is_empty() {
                if d.delete_replication_status.is_empty() {
                    d.replication_status.clone()
                } else {
                    d.delete_replication_status.clone()
                }
            } else {
                d.targets
                    .get(&arn)
                    .map(|t| {
                        if t.delete_replication_status.is_empty() {
                            t.replication_status.clone()
                        } else {
                            t.delete_replication_status.clone()
                        }
                    })
                    .unwrap_or_default()
            };
            let attempted = if status == "PENDING" || op == "DEL" {
                String::new()
            } else {
                print_date(&d.replication_timestamp)
            };
            vec![
                attempted,
                print_date(&d.last_modified),
                status,
                d.version_id.clone(),
                op.to_string(),
                d.object.clone(),
            ]
        })
        .collect();
    if rows.is_empty() {
        println!("No unreplicated versions found for {}.", args.target);
        return Ok(());
    }
    let headers = [
        "Attempted At",
        "Created",
        "Status",
        "VersionID",
        "Op",
        "Object",
    ];
    println!("{}", render_table(&headers, &rows, |_, _| Align::Center));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remote_bucket_urls() {
        let store = ConfigV10::new_with_defaults();
        let (ak, sk, url) = remote_url(&store, "https://ak:sk@minio.example.com/dest").unwrap();
        assert_eq!((ak.as_str(), sk.as_str()), ("ak", "sk"));
        assert_eq!(remote_bucket_name(&url).unwrap(), "dest");
        assert_eq!(url_endpoint(&url, false).unwrap(), "minio.example.com");
        assert_eq!(url_endpoint(&url, true).unwrap(), "minio.example.com:443");
        let (_, _, url) = remote_url(&store, "http://ak:sk@10.0.0.2:9000/dest/").unwrap();
        assert_eq!(url_endpoint(&url, false).unwrap(), "10.0.0.2:9000");
        assert_eq!(remote_bucket_name(&url).unwrap(), "dest");
        assert!(
            remote_url(&store, "https://ak:sk:token@h/b")
                .unwrap_err()
                .to_string()
                .contains("temporary tokens")
        );
        assert!(remote_url(&store, "https://h/b").is_err());
        let error = remote_url(&store, "https://:SECRET@h/b")
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "unsupported remote target format `https://h/b`, see --help"
        );
        let (_, _, url) = remote_url(&store, "https://ak:sk@h/").unwrap();
        assert!(remote_bucket_name(&url).is_err());
    }

    #[test]
    fn parses_replicate_flags() {
        assert_eq!(
            replicate_flags("delete-marker,delete,existing-objects,metadata-sync").unwrap(),
            (true, true, true, true)
        );
        assert_eq!(
            replicate_flags("delete").unwrap(),
            (false, true, false, false)
        );
        assert_eq!(replicate_flags("").unwrap(), (false, false, false, false));
        assert!(replicate_flags("bogus").is_err());
    }

    #[test]
    fn formats_arn_and_durations() {
        assert_eq!(arn_bucket("arn:minio:replication::id:dest"), "dest");
        assert_eq!(arn_bucket("dest"), "dest");
        assert_eq!(rel_time(0), "now");
        assert_eq!(rel_time(1), "1 second ");
        assert_eq!(rel_time(165), "2 minutes ");
        assert_eq!(rel_time(7200), "2 hours ");
        assert_eq!(rel_time(3 * 86400), "3 days ");
        assert_eq!(humanized_duration(0), "0 milliseconds");
        assert_eq!(humanized_duration(5_500_000_000), "5 seconds");
        assert_eq!(humanized_duration(61_000_000_000), "1 minutes 1 seconds");
        assert_eq!(
            humanized_duration(90_061_000_000_000),
            "1 days 1 hours 1 minutes 1 seconds"
        );
        assert_eq!(go_duration_string(round_millis(474_139)), "0s");
        assert_eq!(go_duration_string(round_millis(4_223_561)), "4ms");
        assert_eq!(go_duration_string(round_millis(170_898_880)), "171ms");
        assert_eq!(go_duration_string(1_500_000_000), "1.5s");
        assert_eq!(go_duration_string(3_661_000_000_000), "1h1m1s");
        assert_eq!(go_duration_string(1_500), "1.5µs");
        assert_eq!(
            print_date("2026-09-26T16:31:48.449Z"),
            "2026-09-26 16:31:48 UTC"
        );
    }

    fn rule(arn: &str) -> replication::Rule {
        replication::Rule {
            destination: replication::Destination {
                bucket: arn.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn status_text_matches_mc_layout() {
        assert_eq!(
            status_text(&MetricsV2::default(), &[], &ReplicationConfig::default()),
            "Replication is not configured."
        );
        let metrics: MetricsV2 = serde_json::from_str(
            r#"{"uptime":165,"currStats":{"Stats":{"arn1":{"replicationCount":1,"completedReplicationSize":3}}}}"#,
        )
        .unwrap();
        let config = ReplicationConfig {
            rules: vec![rule("arn1")],
            ..Default::default()
        };
        let target = BucketTarget {
            arn: "arn1".into(),
            endpoint: "h:9000".into(),
            online: true,
            latency: replication::LatencyStat {
                curr: 474_139,
                avg: 4_223_561,
                max: 170_898_880,
            },
            ..Default::default()
        };
        let text = status_text(&metrics, &[target], &config);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 9, "{text}");
        let width = lines[3].chars().count();
        assert!(lines.iter().all(|l| l.chars().count() == width), "{text}");
        assert_eq!(lines[0].trim_end(), "  Replication status since 2 minutes");
        assert_eq!(lines[1].trim_end(), "  h:9000");
        assert_eq!(
            lines[3],
            "  Queued:                       ● 0 objects, 0 B (avg: 0 objects, 0 B ; max: 0 objects, 0 B)  "
        );
        assert_eq!(
            lines[5].trim_end(),
            "  Transfer Rate:                0 B/s (avg: 0 B/s; max: 0 B/s"
        );
        assert_eq!(
            lines[6].trim_end(),
            "  Latency:                      0s (avg: 4ms; max: 171ms)"
        );
        assert_eq!(
            lines[7].trim_end(),
            "  Link:                         ● online (total downtime: 0 milliseconds)"
        );

        // No per-target stats yet: summary section only.
        let metrics = MetricsV2 {
            uptime: 60,
            ..Default::default()
        };
        let text = status_text(&metrics, &[], &config);
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        assert_eq!(
            lines[..4],
            [
                "  Replication status since 1 minute",
                "",
                "  Summary:",
                "  Replicated:                   0 objects (0 B)"
            ]
        );
    }

    #[test]
    fn nodes_text_matches_mc_pretty_table() {
        let node: ReplQNodeStats =
            serde_json::from_str(r#"{"nodeName":"127.0.0.1:9000","uptime":165}"#).unwrap();
        assert_eq!(
            nodes_text(&[node]),
            [
                "Node Name         | Uptime          | Label                     |          Transfer Rate                     | Workers     ",
                "                  |                 |                           | Avg          | Peak         | Current      |           ",
                "127.0.0.1:9000    | 2 minutes       | Large Objects (>=128 MiB) | 0 B/s        | 0 B/s        | 0 B/s        | 0         ",
                "127.0.0.1:9000    | 2 minutes       | Small Objects (<128 MiB)  | 0 B/s        | 0 B/s        | 0 B/s        | 0         ",
            ]
            .join("\n")
        );
        assert_eq!(pretty_row(&[5, 3], &["abcdefgh", "x"]), "ab... | x  ");
    }
}
