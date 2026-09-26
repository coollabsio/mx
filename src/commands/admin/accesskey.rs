//! `mx admin accesskey` (mc `admin accesskey`).
//!
//! Owner: IAM. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct AccesskeyArgs {
    #[command(subcommand)]
    pub command: AccesskeyCommand,
}

#[derive(Debug, Subcommand)]
pub enum AccesskeyCommand {
    #[command(
        name = "list",
        visible_alias = "ls",
        about = "list access key pairs for builtin users"
    )]
    List(AccesskeyListArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "delete access key pairs for builtin users"
    )]
    Remove(AccesskeyRemoveArgs),
    #[command(name = "info", about = "info about given access key pairs")]
    Info(AccesskeyInfoArgs),
    #[command(name = "create", about = "create access key pairs for users")]
    Create(AccesskeyCreateArgs),
    #[command(name = "edit", about = "edit existing access keys")]
    Edit(AccesskeyEditArgs),
    #[command(name = "enable", about = "enable an access key")]
    Enable(AccesskeyEnableArgs),
    #[command(name = "disable", about = "disable an access key")]
    Disable(AccesskeyDisableArgs),
    #[command(
        name = "sts-revoke",
        about = "revokes all STS accounts or specified types for the specified user"
    )]
    StsRevoke(AccesskeyStsRevokeArgs),
}

#[derive(Debug, Args)]
pub struct AccesskeyListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "DN")]
    pub dn: Vec<String>,
    #[arg(long = "users-only", help = "only list user DNs")]
    pub users_only: bool,
    #[arg(long = "temp-only", help = "only list temporary access keys")]
    pub temp_only: bool,
    #[arg(long = "svcacc-only", help = "only list service account access keys")]
    pub svcacc_only: bool,
    #[arg(long = "self", help = "list access keys for the authenticated user")]
    pub self_: bool,
    #[arg(long = "all", help = "list all access keys for all builtin users")]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct AccesskeyRemoveArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: String,
}

#[derive(Debug, Args)]
pub struct AccesskeyInfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "ACCESSKEY", required = true)]
    pub accesskey: Vec<String>,
}

#[derive(Debug, Args)]
pub struct AccesskeyCreateArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "USER")]
    pub user: Option<String>,
    #[arg(
        long = "access-key",
        value_name = "VALUE",
        help = "set an access key for the account"
    )]
    pub access_key: Option<String>,
    #[arg(
        long = "secret-key",
        value_name = "VALUE",
        help = "set a secret key for the  account"
    )]
    pub secret_key: Option<String>,
    #[arg(
        long = "policy",
        value_name = "VALUE",
        help = "path to a JSON policy file"
    )]
    pub policy: Option<String>,
    #[arg(
        long = "name",
        value_name = "VALUE",
        help = "friendly name for the account"
    )]
    pub name: Option<String>,
    #[arg(
        long = "description",
        value_name = "VALUE",
        help = "description for the account"
    )]
    pub description: Option<String>,
    #[arg(
        long = "expiry-duration",
        value_name = "VALUE",
        help = "duration before the access key expires"
    )]
    pub expiry_duration: Option<String>,
    #[arg(
        long = "expiry",
        value_name = "VALUE",
        help = "expiry date for the access key"
    )]
    pub expiry: Option<String>,
}

#[derive(Debug, Args)]
pub struct AccesskeyEditArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Option<String>,
    #[arg(
        long = "secret-key",
        value_name = "VALUE",
        help = "set a secret key for the  account"
    )]
    pub secret_key: Option<String>,
    #[arg(
        long = "policy",
        value_name = "VALUE",
        help = "path to a JSON policy file"
    )]
    pub policy: Option<String>,
    #[arg(
        long = "name",
        value_name = "VALUE",
        help = "friendly name for the account"
    )]
    pub name: Option<String>,
    #[arg(
        long = "description",
        value_name = "VALUE",
        help = "description for the account"
    )]
    pub description: Option<String>,
    #[arg(
        long = "expiry-duration",
        value_name = "VALUE",
        help = "duration before the access key expires"
    )]
    pub expiry_duration: Option<String>,
    #[arg(
        long = "expiry",
        value_name = "VALUE",
        help = "expiry date for the access key"
    )]
    pub expiry: Option<String>,
}

#[derive(Debug, Args)]
pub struct AccesskeyEnableArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Option<String>,
}

#[derive(Debug, Args)]
pub struct AccesskeyDisableArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Option<String>,
}

#[derive(Debug, Args)]
pub struct AccesskeyStsRevokeArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "USER")]
    pub user: Option<String>,
    #[arg(long = "all", help = "revoke all STS accounts for the specified user")]
    pub all: bool,
    #[arg(
        long = "self",
        help = "revoke all STS accounts for the authenticated user"
    )]
    pub self_: bool,
    #[arg(
        long = "token-type",
        value_name = "VALUE",
        help = "specify the token type to revoke"
    )]
    pub token_type: Option<String>,
}

pub fn run(args: AccesskeyArgs, json: bool) -> Result<()> {
    match args.command {
        AccesskeyCommand::List(args) => list(args, json),
        AccesskeyCommand::Remove(args) => remove(args, json),
        AccesskeyCommand::Info(args) => info(args, json),
        AccesskeyCommand::Create(args) => create(args, json),
        AccesskeyCommand::Edit(args) => edit(args, json),
        AccesskeyCommand::Enable(args) => enable(args, json),
        AccesskeyCommand::Disable(args) => disable(args, json),
        AccesskeyCommand::StsRevoke(args) => sts_revoke(args, json),
    }
}

fn list(args: AccesskeyListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin accesskey list")
}

fn remove(args: AccesskeyRemoveArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin accesskey remove")
}

fn info(args: AccesskeyInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin accesskey info")
}

fn create(args: AccesskeyCreateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin accesskey create")
}

fn edit(args: AccesskeyEditArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin accesskey edit")
}

fn enable(args: AccesskeyEnableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin accesskey enable")
}

fn disable(args: AccesskeyDisableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin accesskey disable")
}

fn sts_revoke(args: AccesskeyStsRevokeArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin accesskey sts-revoke")
}
