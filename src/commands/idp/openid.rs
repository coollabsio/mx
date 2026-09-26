//! `mx idp openid` (mc `idp openid`).
//!
//! Owner: IDP. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct OpenidArgs {
    #[command(subcommand)]
    pub command: OpenidCommand,
}

#[derive(Debug, Subcommand)]
pub enum OpenidCommand {
    #[command(name = "add", about = "Create an OpenID IDP server configuration")]
    Add(OpenidAddArgs),
    #[command(name = "update", about = "Update an OpenID IDP configuration")]
    Update(OpenidUpdateArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "remove OpenID IDP server configuration"
    )]
    Remove(OpenidRemoveArgs),
    #[command(
        name = "list",
        visible_alias = "ls",
        about = "list OpenID IDP server configuration(s)"
    )]
    List(OpenidListArgs),
    #[command(name = "info", about = "get OpenID IDP server configuration info")]
    Info(OpenidInfoArgs),
    #[command(name = "enable", about = "enable an OpenID IDP server configuration")]
    Enable(OpenidEnableArgs),
    #[command(name = "disable", about = "Disable an OpenID IDP server configuration")]
    Disable(OpenidDisableArgs),
    #[command(name = "accesskey", about = "manage OpenID access key pairs")]
    Accesskey(OpenidAccesskeyArgs),
}

#[derive(Debug, Args)]
pub struct OpenidAddArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "CFG-NAME")]
    pub cfg_name: Option<String>,
    #[arg(value_name = "CFG-PARAMS")]
    pub cfg_params: Vec<String>,
}

#[derive(Debug, Args)]
pub struct OpenidUpdateArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "CFG-NAME")]
    pub cfg_name: Option<String>,
    #[arg(value_name = "CFG-PARAMS")]
    pub cfg_params: Vec<String>,
}

#[derive(Debug, Args)]
pub struct OpenidRemoveArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "CFG-NAME")]
    pub cfg_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct OpenidListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct OpenidInfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "CFG-NAME")]
    pub cfg_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct OpenidEnableArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "CFG-NAME")]
    pub cfg_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct OpenidDisableArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "CFG-NAME")]
    pub cfg_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct OpenidAccesskeyArgs {
    #[command(subcommand)]
    pub command: OpenidAccesskeyCommand,
}

#[derive(Debug, Subcommand)]
pub enum OpenidAccesskeyCommand {
    #[command(
        name = "list",
        visible_alias = "ls",
        about = "list access key pairs for OpenID"
    )]
    List(OpenidAccesskeyListArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "delete access key pairs for OpenID"
    )]
    Remove(OpenidAccesskeyRemoveArgs),
    #[command(name = "info", about = "info about given access key pairs for OpenID")]
    Info(OpenidAccesskeyInfoArgs),
    #[command(name = "edit", about = "edit existing access keys for OpenID")]
    Edit(OpenidAccesskeyEditArgs),
    #[command(name = "enable", about = "enable an access key")]
    Enable(OpenidAccesskeyEnableArgs),
    #[command(name = "disable", about = "disable an access key")]
    Disable(OpenidAccesskeyDisableArgs),
}

#[derive(Debug, Args)]
pub struct OpenidAccesskeyListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "USERS")]
    pub users: Vec<String>,
    #[arg(long = "users-only", help = "only list user DNs")]
    pub users_only: bool,
    #[arg(long = "temp-only", help = "only list temporary access keys")]
    pub temp_only: bool,
    #[arg(long = "svcacc-only", help = "only list service account access keys")]
    pub svcacc_only: bool,
    #[arg(long = "self", help = "list access keys for the authenticated user")]
    pub self_: bool,
    #[arg(long = "all", help = "list all access keys for all OpenID users")]
    pub all: bool,
    #[arg(
        long = "all-configs",
        help = "list access keys for all OpenID configurations"
    )]
    pub all_configs: bool,
}

#[derive(Debug, Args)]
pub struct OpenidAccesskeyRemoveArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: String,
}

#[derive(Debug, Args)]
pub struct OpenidAccesskeyInfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "ACCESSKEY", required = true)]
    pub accesskey: Vec<String>,
}

#[derive(Debug, Args)]
pub struct OpenidAccesskeyEditArgs {
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
pub struct OpenidAccesskeyEnableArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Option<String>,
}

#[derive(Debug, Args)]
pub struct OpenidAccesskeyDisableArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Option<String>,
}

pub fn run(args: OpenidArgs, json: bool) -> Result<()> {
    match args.command {
        OpenidCommand::Add(args) => add(args, json),
        OpenidCommand::Update(args) => update(args, json),
        OpenidCommand::Remove(args) => remove(args, json),
        OpenidCommand::List(args) => list(args, json),
        OpenidCommand::Info(args) => info(args, json),
        OpenidCommand::Enable(args) => enable(args, json),
        OpenidCommand::Disable(args) => disable(args, json),
        OpenidCommand::Accesskey(args) => accesskey(args, json),
    }
}

fn add(args: OpenidAddArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid add")
}

fn update(args: OpenidUpdateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid update")
}

fn remove(args: OpenidRemoveArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid remove")
}

fn list(args: OpenidListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid list")
}

fn info(args: OpenidInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid info")
}

fn enable(args: OpenidEnableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid enable")
}

fn disable(args: OpenidDisableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid disable")
}

fn accesskey(args: OpenidAccesskeyArgs, json: bool) -> Result<()> {
    match args.command {
        OpenidAccesskeyCommand::List(args) => accesskey_list(args, json),
        OpenidAccesskeyCommand::Remove(args) => accesskey_remove(args, json),
        OpenidAccesskeyCommand::Info(args) => accesskey_info(args, json),
        OpenidAccesskeyCommand::Edit(args) => accesskey_edit(args, json),
        OpenidAccesskeyCommand::Enable(args) => accesskey_enable(args, json),
        OpenidAccesskeyCommand::Disable(args) => accesskey_disable(args, json),
    }
}

fn accesskey_list(args: OpenidAccesskeyListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid accesskey list")
}

fn accesskey_remove(args: OpenidAccesskeyRemoveArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid accesskey remove")
}

fn accesskey_info(args: OpenidAccesskeyInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid accesskey info")
}

fn accesskey_edit(args: OpenidAccesskeyEditArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid accesskey edit")
}

fn accesskey_enable(args: OpenidAccesskeyEnableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid accesskey enable")
}

fn accesskey_disable(args: OpenidAccesskeyDisableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp openid accesskey disable")
}
