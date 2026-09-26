//! `mx admin` (mc `admin`): MinIO server administration over the admin API
//! (`crate::s3::admin`). One module per command group; each module names its owner.

pub mod accesskey;
pub mod cluster;
pub mod config;
pub mod decommission;
pub mod deprecated;
pub mod group;
pub mod heal;
pub mod info;
pub mod kms;
pub mod logs;
pub mod policy;
pub mod prometheus;
pub mod rebalance;
pub mod replicate;
pub mod scanner;
pub mod service;
pub mod top;
pub mod trace;
pub mod update;
pub mod user;

use crate::error::McError;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct AdminArgs {
    #[command(subcommand)]
    pub command: AdminCommand,
}

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    #[command(about = "restart or unfreeze a MinIO cluster")]
    Service(service::ServiceArgs),
    #[command(about = "update all MinIO servers")]
    Update(update::UpdateArgs),
    #[command(about = "display MinIO server information")]
    Info(info::InfoArgs),
    #[command(about = "manage users")]
    User(user::UserArgs),
    #[command(about = "manage groups")]
    Group(group::GroupArgs),
    #[command(about = "manage policies defined in the MinIO server")]
    Policy(policy::PolicyArgs),
    #[command(about = "manage MinIO site replication")]
    Replicate(replicate::ReplicateArgs),
    #[command(about = "manage MinIO server configuration")]
    Config(config::ConfigArgs),
    #[command(
        visible_alias = "decom",
        about = "manage MinIO server pool decommissioning"
    )]
    Decommission(decommission::DecommissionArgs),
    #[command(about = "monitor healing for bucket(s) and object(s) on MinIO server")]
    Heal(heal::HealArgs),
    #[command(about = "manages prometheus config")]
    Prometheus(prometheus::PrometheusArgs),
    #[command(about = "perform KMS management operations")]
    Kms(kms::KmsArgs),
    #[command(about = "provide MinIO scanner info")]
    Scanner(scanner::ScannerArgs),
    #[command(about = "provide top like statistics for MinIO")]
    Top(top::TopArgs),
    #[command(about = "Show HTTP call trace for all incoming and internode on MinIO")]
    Trace(trace::TraceArgs),
    #[command(about = "manage MinIO cluster metadata")]
    Cluster(cluster::ClusterArgs),
    #[command(about = "Manage MinIO rebalance")]
    Rebalance(rebalance::RebalanceArgs),
    #[command(about = "show MinIO logs")]
    Logs(logs::LogsArgs),
    #[command(about = "manage access keys defined in the MinIO server")]
    Accesskey(accesskey::AccesskeyArgs),
    // Hidden, deprecated mc commands (`deprecated.rs`).
    #[command(hide = true, about = "inspect files on MinIO server")]
    Inspect(deprecated::DeprecatedArgs),
    #[command(
        hide = true,
        about = "manage MinIO IDentity Provider server configuration"
    )]
    Idp(deprecated::DeprecatedArgs),
    #[command(hide = true, about = "Run server side speed test")]
    Speedtest(deprecated::DeprecatedArgs),
    #[command(hide = true, about = "show MinIO logs")]
    Console(deprecated::ConsoleArgs),
    #[command(hide = true, about = "run health check for Subnet")]
    Health(deprecated::DeprecatedArgs),
    #[command(hide = true, about = "Subnet related commands")]
    Subnet(deprecated::DeprecatedArgs),
    #[command(hide = true, about = "manage buckets defined in the MinIO server")]
    Bucket(deprecated::DeprecatedArgs),
    #[command(hide = true, about = "manage remote tier targets for ILM transition")]
    Tier(deprecated::DeprecatedArgs),
    #[command(hide = true, about = "generate profile data for debugging purposes")]
    Profile(deprecated::DeprecatedArgs),
}

pub fn run(args: AdminArgs, json: bool) -> Result<()> {
    match args.command {
        AdminCommand::Service(args) => service::run(args, json),
        AdminCommand::Update(args) => update::run(args, json),
        AdminCommand::Info(args) => info::run(args, json),
        AdminCommand::User(args) => user::run(args, json),
        AdminCommand::Group(args) => group::run(args, json),
        AdminCommand::Policy(args) => policy::run(args, json),
        AdminCommand::Replicate(args) => replicate::run(args, json),
        AdminCommand::Config(args) => config::run(args, json),
        AdminCommand::Decommission(args) => decommission::run(args, json),
        AdminCommand::Heal(args) => heal::run(args, json),
        AdminCommand::Prometheus(args) => prometheus::run(args, json),
        AdminCommand::Kms(args) => kms::run(args, json),
        AdminCommand::Scanner(args) => scanner::run(args, json),
        AdminCommand::Top(args) => top::run(args, json),
        AdminCommand::Trace(args) => trace::run(args, json),
        AdminCommand::Cluster(args) => cluster::run(args, json),
        AdminCommand::Rebalance(args) => rebalance::run(args, json),
        AdminCommand::Logs(args) => logs::run(args, json),
        AdminCommand::Accesskey(args) => accesskey::run(args, json),
        AdminCommand::Inspect(_) => deprecated("support inspect"),
        AdminCommand::Idp(_) => deprecated("idp ldap|openid"),
        AdminCommand::Speedtest(_) => deprecated("support perf"),
        AdminCommand::Console(args) => deprecated::console(args),
        AdminCommand::Health(args) => deprecated::stub("admin health", args),
        AdminCommand::Subnet(args) => deprecated::stub("admin subnet", args),
        AdminCommand::Bucket(args) => deprecated::stub("admin bucket", args),
        AdminCommand::Tier(args) => deprecated::stub("admin tier", args),
        AdminCommand::Profile(args) => deprecated::stub("admin profile", args),
    }
}

/// mc `deprecatedError`: fatal `Deprecated command. Please use 'mc <new_command>' instead.`
pub fn deprecated(new_command: &str) -> Result<()> {
    let cause = McError::new(format!(
        "Please use '{} {new_command}' instead",
        crate::output::prog_name()
    ));
    Err(anyhow::Error::new(cause).context("Deprecated command"))
}
