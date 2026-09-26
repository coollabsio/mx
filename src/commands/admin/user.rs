//! `mx admin user` (mc `admin user`).
//!
//! Owner: IAM. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct UserArgs {
    #[command(subcommand)]
    pub command: UserCommand,
}

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    #[command(name = "add", about = "add a new user")]
    Add(UserAddArgs),
    #[command(name = "disable", about = "disable user")]
    Disable(UserDisableArgs),
    #[command(name = "enable", about = "enable user")]
    Enable(UserEnableArgs),
    #[command(name = "remove", visible_alias = "rm", about = "remove user")]
    Remove(UserRemoveArgs),
    #[command(name = "list", visible_alias = "ls", about = "list all users")]
    List(UserListArgs),
    #[command(name = "info", about = "display info of a user")]
    Info(UserInfoArgs),
    #[command(name = "policy", about = "export user policies in JSON format")]
    Policy(UserPolicyArgs),
    #[command(name = "svcacct", about = "manage service accounts")]
    Svcacct(UserSvcacctArgs),
    #[command(name = "sts", about = "manage STS accounts")]
    Sts(UserStsArgs),
}

#[derive(Debug, Args)]
pub struct UserAddArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Option<String>,
    #[arg(value_name = "SECRETKEY")]
    pub secretkey: Option<String>,
}

#[derive(Debug, Args)]
pub struct UserDisableArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "USERNAME")]
    pub username: String,
}

#[derive(Debug, Args)]
pub struct UserEnableArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "USERNAME")]
    pub username: String,
}

#[derive(Debug, Args)]
pub struct UserRemoveArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "USERNAME")]
    pub username: String,
}

#[derive(Debug, Args)]
pub struct UserListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct UserInfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "USERNAME")]
    pub username: String,
}

#[derive(Debug, Args)]
pub struct UserPolicyArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "USERNAME")]
    pub username: String,
}

#[derive(Debug, Args)]
pub struct UserSvcacctArgs {
    #[command(subcommand)]
    pub command: UserSvcacctCommand,
}

#[derive(Debug, Subcommand)]
pub enum UserSvcacctCommand {
    #[command(name = "add", about = "add a new service account")]
    Add(UserSvcacctAddArgs),
    #[command(name = "list", visible_alias = "ls", about = "list services accounts")]
    List(UserSvcacctListArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "remove a service account"
    )]
    Remove(UserSvcacctRemoveArgs),
    #[command(name = "info", about = "display service account info")]
    Info(UserSvcacctInfoArgs),
    #[command(
        name = "edit",
        visible_alias = "set",
        about = "edit an existing service account"
    )]
    Edit(UserSvcacctEditArgs),
    #[command(name = "enable", about = "enable a service account")]
    Enable(UserSvcacctEnableArgs),
    #[command(name = "disable", about = "disable a service account")]
    Disable(UserSvcacctDisableArgs),
}

#[derive(Debug, Args)]
pub struct UserSvcacctAddArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "ACCOUNT")]
    pub account: String,
    #[arg(
        long = "access-key",
        value_name = "VALUE",
        help = "set an access key for the service account"
    )]
    pub access_key: Option<String>,
    #[arg(
        long = "secret-key",
        value_name = "VALUE",
        help = "set a secret key for the service account"
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
        help = "friendly name for the service account"
    )]
    pub name: Option<String>,
    #[arg(
        long = "description",
        value_name = "VALUE",
        help = "description for the service account"
    )]
    pub description: Option<String>,
    #[arg(
        long = "expiry",
        value_name = "VALUE",
        help = "time of expiration for the service account"
    )]
    pub expiry: Option<String>,
    #[arg(
        long = "comment",
        hide = true,
        value_name = "VALUE",
        help = "description for the service account (DEPRECATED: use --description instead)"
    )]
    pub comment: Option<String>,
}

#[derive(Debug, Args)]
pub struct UserSvcacctListArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "TARGET-ACCOUNT")]
    pub target_account: String,
}

#[derive(Debug, Args)]
pub struct UserSvcacctRemoveArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "SERVICE-ACCOUNT")]
    pub service_account: String,
}

#[derive(Debug, Args)]
pub struct UserSvcacctInfoArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "SERVICE-ACCOUNT")]
    pub service_account: String,
    #[arg(long = "policy", help = "print policy in JSON format")]
    pub policy: bool,
}

#[derive(Debug, Args)]
pub struct UserSvcacctEditArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "SERVICE-ACCOUNT")]
    pub service_account: String,
    #[arg(
        long = "secret-key",
        value_name = "VALUE",
        help = "set a secret key for the service account"
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
        help = "name for the service account"
    )]
    pub name: Option<String>,
    #[arg(
        long = "description",
        value_name = "VALUE",
        help = "description for the service account"
    )]
    pub description: Option<String>,
    #[arg(
        long = "expiry",
        value_name = "VALUE",
        help = "time of expiration for the service account"
    )]
    pub expiry: Option<String>,
}

#[derive(Debug, Args)]
pub struct UserSvcacctEnableArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "SERVICE-ACCOUNT")]
    pub service_account: String,
}

#[derive(Debug, Args)]
pub struct UserSvcacctDisableArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "SERVICE-ACCOUNT")]
    pub service_account: String,
}

#[derive(Debug, Args)]
pub struct UserStsArgs {
    #[command(subcommand)]
    pub command: UserStsCommand,
}

#[derive(Debug, Subcommand)]
pub enum UserStsCommand {
    #[command(name = "info", about = "display temporary account info")]
    Info(UserStsInfoArgs),
}

#[derive(Debug, Args)]
pub struct UserStsInfoArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "STS-ACCOUNT")]
    pub sts_account: String,
    #[arg(long = "policy", help = "print policy in JSON format")]
    pub policy: bool,
}

pub fn run(args: UserArgs, json: bool) -> Result<()> {
    match args.command {
        UserCommand::Add(args) => add(args, json),
        UserCommand::Disable(args) => disable(args, json),
        UserCommand::Enable(args) => enable(args, json),
        UserCommand::Remove(args) => remove(args, json),
        UserCommand::List(args) => list(args, json),
        UserCommand::Info(args) => info(args, json),
        UserCommand::Policy(args) => policy(args, json),
        UserCommand::Svcacct(args) => svcacct(args, json),
        UserCommand::Sts(args) => sts(args, json),
    }
}

fn add(args: UserAddArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user add")
}

fn disable(args: UserDisableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user disable")
}

fn enable(args: UserEnableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user enable")
}

fn remove(args: UserRemoveArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user remove")
}

fn list(args: UserListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user list")
}

fn info(args: UserInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user info")
}

fn policy(args: UserPolicyArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user policy")
}

fn svcacct(args: UserSvcacctArgs, json: bool) -> Result<()> {
    match args.command {
        UserSvcacctCommand::Add(args) => svcacct_add(args, json),
        UserSvcacctCommand::List(args) => svcacct_list(args, json),
        UserSvcacctCommand::Remove(args) => svcacct_remove(args, json),
        UserSvcacctCommand::Info(args) => svcacct_info(args, json),
        UserSvcacctCommand::Edit(args) => svcacct_edit(args, json),
        UserSvcacctCommand::Enable(args) => svcacct_enable(args, json),
        UserSvcacctCommand::Disable(args) => svcacct_disable(args, json),
    }
}

fn svcacct_add(args: UserSvcacctAddArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user svcacct add")
}

fn svcacct_list(args: UserSvcacctListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user svcacct list")
}

fn svcacct_remove(args: UserSvcacctRemoveArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user svcacct remove")
}

fn svcacct_info(args: UserSvcacctInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user svcacct info")
}

fn svcacct_edit(args: UserSvcacctEditArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user svcacct edit")
}

fn svcacct_enable(args: UserSvcacctEnableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user svcacct enable")
}

fn svcacct_disable(args: UserSvcacctDisableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user svcacct disable")
}

fn sts(args: UserStsArgs, json: bool) -> Result<()> {
    match args.command {
        UserStsCommand::Info(args) => sts_info(args, json),
    }
}

fn sts_info(args: UserStsInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin user sts info")
}
