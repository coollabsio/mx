//! `mx admin replicate` (mc `admin replicate`).
//!
//! Owner: TOPO. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct ReplicateArgs {
    #[command(subcommand)]
    pub command: ReplicateCommand,
}

#[derive(Debug, Subcommand)]
pub enum ReplicateCommand {
    #[command(name = "add", about = "add one or more sites for replication")]
    Add(ReplicateAddArgs),
    #[command(
        name = "update",
        alias = "edit",
        about = "modify endpoint of site participating in site replication"
    )]
    Update(ReplicateUpdateArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "remove one or more sites from site replication"
    )]
    Remove(ReplicateRemoveArgs),
    #[command(name = "info", about = "get site replication information")]
    Info(ReplicateInfoArgs),
    #[command(name = "status", about = "display site replication status")]
    Status(ReplicateStatusArgs),
    #[command(name = "resync", about = "resync content to site")]
    Resync(ReplicateResyncArgs),
}

#[derive(Debug, Args)]
pub struct ReplicateAddArgs {
    #[arg(value_name = "ALIASES", required = true)]
    pub aliases: Vec<String>,
    #[arg(long = "replicate-ilm-expiry", help = "replicate ILM expiry rules")]
    pub replicate_ilm_expiry: bool,
}

#[derive(Debug, Args)]
pub struct ReplicateUpdateArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(
        long = "deployment-id",
        value_name = "VALUE",
        help = "deployment id of the site, should be a unique value"
    )]
    pub deployment_id: Option<String>,
    #[arg(
        long = "endpoint",
        value_name = "VALUE",
        help = "endpoint for the site"
    )]
    pub endpoint: Option<String>,
    #[arg(
        long = "mode",
        value_name = "VALUE",
        help = "change mode of replication for this target, valid values are ['sync', 'async']."
    )]
    pub mode: Option<String>,
    #[arg(
        long = "bucket-bandwidth",
        value_name = "VALUE",
        help = "Set default bandwidth limit for bucket in bytes per second (K,B,G,T for metric and Ki,Bi,Gi,Ti for IEC units)"
    )]
    pub bucket_bandwidth: Option<String>,
    #[arg(
        long = "disable-ilm-expiry-replication",
        help = "disable ILM expiry rules replication"
    )]
    pub disable_ilm_expiry_replication: bool,
    #[arg(
        long = "enable-ilm-expiry-replication",
        help = "enable ILM expiry rules replication"
    )]
    pub enable_ilm_expiry_replication: bool,
    #[arg(
        long = "sync",
        hide = true,
        default_value = "disable",
        value_name = "VALUE",
        help = "enable synchronous replication for this target, valid values are ['enable', 'disable']."
    )]
    pub sync: String,
}

#[derive(Debug, Args)]
pub struct ReplicateRemoveArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "SITES")]
    pub sites: Vec<String>,
    #[arg(
        long = "all",
        help = "remove site replication from all participating sites"
    )]
    pub all: bool,
    #[arg(
        long = "force",
        help = "force removal of site(s) from site replication configuration"
    )]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ReplicateInfoArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct ReplicateStatusArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(long = "buckets", help = "display only buckets")]
    pub buckets: bool,
    #[arg(long = "policies", help = "display only policies")]
    pub policies: bool,
    #[arg(long = "users", help = "display only users")]
    pub users: bool,
    #[arg(long = "groups", help = "display only groups")]
    pub groups: bool,
    #[arg(long = "ilm-expiry-rules", help = "display only ilm expiry rules")]
    pub ilm_expiry_rules: bool,
    #[arg(long = "all", help = "display all available site replication status")]
    pub all: bool,
    #[arg(
        long = "bucket",
        value_name = "VALUE",
        help = "display bucket sync status"
    )]
    pub bucket: Option<String>,
    #[arg(
        long = "policy",
        value_name = "VALUE",
        help = "display policy sync status"
    )]
    pub policy: Option<String>,
    #[arg(long = "user", value_name = "VALUE", help = "display user sync status")]
    pub user: Option<String>,
    #[arg(
        long = "group",
        value_name = "VALUE",
        help = "display group sync status"
    )]
    pub group: Option<String>,
    #[arg(
        long = "ilm-expiry-rule",
        value_name = "VALUE",
        help = "display ILM expiry rule sync status"
    )]
    pub ilm_expiry_rule: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReplicateResyncArgs {
    #[command(subcommand)]
    pub command: ReplicateResyncCommand,
}

#[derive(Debug, Subcommand)]
pub enum ReplicateResyncCommand {
    #[command(name = "start", about = "start resync to site")]
    Start(ReplicateResyncStartArgs),
    #[command(name = "status", about = "show site replication resync status")]
    Status(ReplicateResyncStatusArgs),
    #[command(name = "cancel", about = "cancel ongoing resync operation")]
    Cancel(ReplicateResyncCancelArgs),
}

#[derive(Debug, Args)]
pub struct ReplicateResyncStartArgs {
    #[arg(value_name = "ALIAS1")]
    pub alias1: String,
    #[arg(value_name = "ALIAS2")]
    pub alias2: String,
}

#[derive(Debug, Args)]
pub struct ReplicateResyncStatusArgs {
    #[arg(value_name = "ALIAS1")]
    pub alias1: String,
    #[arg(value_name = "ALIAS2")]
    pub alias2: String,
}

#[derive(Debug, Args)]
pub struct ReplicateResyncCancelArgs {
    #[arg(value_name = "ALIAS1")]
    pub alias1: String,
    #[arg(value_name = "ALIAS2")]
    pub alias2: String,
}

pub fn run(args: ReplicateArgs, json: bool) -> Result<()> {
    match args.command {
        ReplicateCommand::Add(args) => add(args, json),
        ReplicateCommand::Update(args) => update(args, json),
        ReplicateCommand::Remove(args) => remove(args, json),
        ReplicateCommand::Info(args) => info(args, json),
        ReplicateCommand::Status(args) => status(args, json),
        ReplicateCommand::Resync(args) => resync(args, json),
    }
}

fn add(args: ReplicateAddArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin replicate add")
}

fn update(args: ReplicateUpdateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin replicate update")
}

fn remove(args: ReplicateRemoveArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin replicate remove")
}

fn info(args: ReplicateInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin replicate info")
}

fn status(args: ReplicateStatusArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin replicate status")
}

fn resync(args: ReplicateResyncArgs, json: bool) -> Result<()> {
    match args.command {
        ReplicateResyncCommand::Start(args) => resync_start(args, json),
        ReplicateResyncCommand::Status(args) => resync_status(args, json),
        ReplicateResyncCommand::Cancel(args) => resync_cancel(args, json),
    }
}

fn resync_start(args: ReplicateResyncStartArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin replicate resync start")
}

fn resync_status(args: ReplicateResyncStatusArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin replicate resync status")
}

fn resync_cancel(args: ReplicateResyncCancelArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin replicate resync cancel")
}
