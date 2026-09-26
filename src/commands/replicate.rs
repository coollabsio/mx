//! `mx replicate add|update|ls|status|resync|export|import|rm|backlog` (MinIO bucket
//! replication: remote targets via the admin API, rules via `?replication`).

use crate::commands::ilm_tier::render_table;
use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::config::model::ConfigV10;
use crate::output;
use crate::s3::admin::{self, AdminClient};
use crate::s3::replication::{
    self, BucketTarget, ReplicationConfig, RuleOptions, TargetCredentials,
};
use crate::target::TargetRef;
use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Read;

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
    /// show replication speed for all nodes
    ///
    /// not supported by mx
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
        bail!("Unable to list replication configuration: replication configuration not set");
    }
    let targets = rt
        .block_on(replication::list_remote_targets(&src.client, &src.bucket))
        .context("Unable to fetch remote target")?;
    if !json {
        println!("Rules:");
    }
    let status_filter = args.status.unwrap_or_default();
    for rule in &config.rules {
        if !status_filter.is_empty() && !status_filter.eq_ignore_ascii_case(&rule.status) {
            continue;
        }
        if json {
            #[derive(Serialize)]
            struct Body<'a> {
                rule: &'a replication::Rule,
            }
            print_json("ls", &args.target, Body { rule })?;
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

fn human_duration(seconds: u64) -> String {
    let (value, unit) = match seconds {
        0..=59 => (seconds, "second"),
        60..=3599 => (seconds / 60, "minute"),
        3600..=86399 => (seconds / 3600, "hour"),
        _ => (seconds / 86400, "day"),
    };
    if value == 1 {
        format!("1 {unit}")
    } else {
        format!("{value} {unit}s")
    }
}

fn status(args: ReplicateStatusArgs, json: bool) -> Result<()> {
    if args.nodes {
        bail!("`replicate status --nodes` is not supported by mx yet");
    }
    let src = source(&args.target)?;
    let rt = runtime()?;
    let metrics = rt
        .block_on(replication::replication_metrics(&src.client, &src.bucket))
        .context("Unable to get replication status")?;
    let targets = rt
        .block_on(replication::list_remote_targets(&src.client, &src.bucket))
        .context("Unable to fetch remote target")?;
    let config = rt
        .block_on(replication::get_replication(&src.client, &src.bucket))
        .context("Unable to fetch replication configuration")?;
    if json {
        #[derive(Serialize)]
        struct Body<'a> {
            replicationstats: &'a Value,
            #[serde(rename = "remoteTargets")]
            remote_targets: &'a [BucketTarget],
        }
        let body = Body {
            replicationstats: &metrics,
            remote_targets: &targets,
        };
        return print_json("status", &args.target, body);
    }
    print!("{}", status_text(&metrics, &targets, &config));
    Ok(())
}

fn status_text(metrics: &Value, targets: &[BucketTarget], config: &ReplicationConfig) -> String {
    if config.rules.is_empty() {
        return "Replication is not configured.\n".to_string();
    }
    let num = |v: &Value| v.as_f64().unwrap_or_default() as i64;
    let current = &metrics["currStats"];
    let queued = &current["queued"]["curr"];
    let mut out = format!(
        "Replication status since {}\n",
        human_duration(metrics["uptime"].as_u64().unwrap_or_default())
    );
    let arns: Vec<&String> = config
        .rules
        .iter()
        .map(|rule| &rule.destination.bucket)
        .fold(Vec::new(), |mut acc, arn| {
            if !acc.contains(&arn) {
                acc.push(arn);
            }
            acc
        });
    for arn in arns {
        let target = targets.iter().find(|t| &t.arn == arn);
        let stat = &current["Stats"][arn.as_str()];
        let online = target
            .and_then(|t| t.extra.get("isOnline"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        out.push_str(&format!(
            "{}\n",
            target.map(|t| t.endpoint.as_str()).unwrap_or(arn)
        ));
        out.push_str(&format!(
            "  Replicated:    {} objects ({})\n",
            admin::comma(num(&stat["replicationCount"])),
            admin::ibytes(num(&stat["completedReplicationSize"]) as u64)
        ));
        out.push_str(&format!(
            "  Link:          {}\n",
            if online { "online" } else { "offline" }
        ));
        out.push_str(&format!(
            "  Errors:        {} in last 1 minute; {} in last 1hr; {} since uptime\n",
            admin::comma(num(&stat["failed"]["lastMinute"]["count"])),
            admin::comma(num(&stat["failed"]["lastHour"]["count"])),
            admin::comma(num(&stat["failed"]["totals"]["count"]))
        ));
    }
    out.push_str(&format!(
        "Queued:          {} objects, {}\n",
        admin::comma(num(&queued["count"])),
        admin::ibytes(num(&queued["bytes"]) as u64)
    ));
    out.push_str(&format!(
        "Received:        {} objects ({})\n",
        admin::comma(num(&current["replicaCount"])),
        admin::ibytes(num(&current["replicaSize"]) as u64)
    ));
    out
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
        return print_json(
            "start",
            &args.target,
            json!({ "resyncInfo": info, "targetArn": "" }),
        );
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
        return print_json(
            "status",
            &args.target,
            json!({ "resyncInfo": info, "targetArn": arn }),
        );
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
        println!("{}", crate::output::json_indent(&config)?);
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
        return print_json("import", &args.target, ConfigBody { config: &config });
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
    op_message("rm", &args.target, &id, json, text)
}

fn backlog(args: ReplicateBacklogArgs, json: bool) -> Result<()> {
    let src = source(&args.target)?;
    let rt = runtime()?;
    if !args.full {
        let entries = rt
            .block_on(replication::replication_mrf(
                &src.client,
                &src.bucket,
                &args.nodes,
            ))
            .context("Unable to fetch replication backlog")?;
        for entry in &entries {
            if let Some(err) = entry["error"].as_str().filter(|e| !e.is_empty()) {
                bail!("Unable to fetch replication backlog: {err}");
            }
        }
        if json {
            for entry in &entries {
                print_json("mrf", &args.target, entry)?;
            }
            return Ok(());
        }
        if entries.is_empty() {
            println!("No recent replication failures found for {}.", args.target);
            return Ok(());
        }
        for entry in &entries {
            println!(
                "{} | Retry={} | {} ({})",
                entry["nodeName"].as_str().unwrap_or_default(),
                entry["retryCount"].as_i64().unwrap_or_default(),
                entry["object"].as_str().unwrap_or_default(),
                entry["versionId"].as_str().unwrap_or_default()
            );
        }
        return Ok(());
    }

    let arn = args.arn.clone().unwrap_or_default();
    let entries = rt
        .block_on(replication::replication_diff(
            &src.client,
            &src.bucket,
            &src.prefix,
            &arn,
            args.verbose,
        ))
        .context("Unable to fetch replication backlog")?;
    if json {
        for entry in &entries {
            println!("{}", crate::output::json_indent(entry)?);
        }
        return Ok(());
    }
    let rows: Vec<Vec<String>> = entries
        .iter()
        .filter(|d| d["object"].as_str().is_some_and(|o| !o.is_empty()))
        .map(|d| {
            let version = d["versionId"].as_str().unwrap_or_default();
            let op = match (version.is_empty(), d["deletemarker"].as_bool()) {
                (true, _) => "",
                (false, Some(true)) => "DEL",
                (false, _) => "PUT",
            };
            let status = if arn.is_empty() {
                d["rStatus"].as_str()
            } else {
                d["targets"][arn.as_str()]["rStatus"].as_str()
            }
            .unwrap_or_default();
            vec![
                d["lastModified"].as_str().unwrap_or_default().to_string(),
                status.to_string(),
                version.to_string(),
                op.to_string(),
                d["object"].as_str().unwrap_or_default().to_string(),
            ]
        })
        .collect();
    if rows.is_empty() {
        println!("No unreplicated versions found for {}.", args.target);
        return Ok(());
    }
    print!(
        "{}",
        render_table(
            &["Last Modified", "Status", "VersionID", "Op", "Object"],
            &rows
        )
    );
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
    fn formats_arn_and_status() {
        assert_eq!(arn_bucket("arn:minio:replication::id:dest"), "dest");
        assert_eq!(arn_bucket("dest"), "dest");
        assert_eq!(human_duration(1), "1 second");
        assert_eq!(human_duration(7200), "2 hours");
        let empty = status_text(&json!({}), &[], &ReplicationConfig::default());
        assert_eq!(empty, "Replication is not configured.\n");
        assert_eq!(
            crate::output::json_indent(&json!({"a": 1})).unwrap(),
            "{\n \"a\": 1\n}"
        );
    }
}
