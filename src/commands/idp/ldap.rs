//! `mx idp ldap` (mc `idp ldap`).
//!
//! Owner: IDP. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct LdapArgs {
    #[command(subcommand)]
    pub command: LdapCommand,
}

#[derive(Debug, Subcommand)]
pub enum LdapCommand {
    #[command(name = "add", about = "Create an LDAP IDP server configuration")]
    Add(LdapAddArgs),
    #[command(name = "update", about = "Update an LDAP IDP configuration")]
    Update(LdapUpdateArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "remove LDAP IDP server configuration"
    )]
    Remove(LdapRemoveArgs),
    #[command(
        name = "list",
        visible_alias = "ls",
        about = "list LDAP IDP server configuration(s)"
    )]
    List(LdapListArgs),
    #[command(name = "info", about = "get LDAP IDP server configuration info")]
    Info(LdapInfoArgs),
    #[command(name = "enable", about = "manage LDAP IDP server configuration")]
    Enable(LdapEnableArgs),
    #[command(name = "disable", about = "Disable an LDAP IDP server configuration")]
    Disable(LdapDisableArgs),
    #[command(name = "policy", about = "manage policy assignments for LDAP")]
    Policy(LdapPolicyArgs),
    #[command(name = "accesskey", about = "manage LDAP access key pairs")]
    Accesskey(LdapAccesskeyArgs),
}

#[derive(Debug, Args)]
pub struct LdapAddArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "CFG-PARAMS")]
    pub cfg_params: Vec<String>,
}

#[derive(Debug, Args)]
pub struct LdapUpdateArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "CFG-PARAMS")]
    pub cfg_params: Vec<String>,
}

#[derive(Debug, Args)]
pub struct LdapRemoveArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct LdapListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct LdapInfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct LdapEnableArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct LdapDisableArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct LdapPolicyArgs {
    #[command(subcommand)]
    pub command: LdapPolicyCommand,
}

#[derive(Debug, Subcommand)]
pub enum LdapPolicyCommand {
    #[command(name = "attach", about = "attach a policy to an entity")]
    Attach(LdapPolicyAttachArgs),
    #[command(name = "detach", about = "detach a policy from an entity")]
    Detach(LdapPolicyDetachArgs),
    #[command(name = "entities", about = "list policy association entities")]
    Entities(LdapPolicyEntitiesArgs),
}

#[derive(Debug, Args)]
pub struct LdapPolicyAttachArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POLICIES", required = true)]
    pub policies: Vec<String>,
    #[arg(
        long = "user",
        short = 'u',
        value_name = "VALUE",
        help = "attach policy to user by DN or by login name"
    )]
    pub user: Option<String>,
    #[arg(
        long = "group",
        short = 'g',
        value_name = "VALUE",
        help = "attach policy to LDAP Group DN"
    )]
    pub group: Option<String>,
}

#[derive(Debug, Args)]
pub struct LdapPolicyDetachArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POLICIES", required = true)]
    pub policies: Vec<String>,
    #[arg(
        long = "user",
        short = 'u',
        value_name = "VALUE",
        help = "attach policy to user by DN or by login name"
    )]
    pub user: Option<String>,
    #[arg(
        long = "group",
        short = 'g',
        value_name = "VALUE",
        help = "attach policy to LDAP Group DN"
    )]
    pub group: Option<String>,
}

#[derive(Debug, Args)]
pub struct LdapPolicyEntitiesArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(
        long = "user",
        short = 'u',
        value_name = "VALUE",
        help = "list policies associated with user(s)"
    )]
    pub user: Vec<String>,
    #[arg(
        long = "group",
        short = 'g',
        value_name = "VALUE",
        help = "list policies associated with group(s)"
    )]
    pub group: Vec<String>,
    #[arg(
        long = "policy",
        short = 'p',
        value_name = "VALUE",
        help = "list users or groups associated with policy"
    )]
    pub policy: Vec<String>,
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyArgs {
    #[command(subcommand)]
    pub command: LdapAccesskeyCommand,
}

#[derive(Debug, Subcommand)]
pub enum LdapAccesskeyCommand {
    #[command(
        name = "list",
        visible_alias = "ls",
        about = "list access key pairs for LDAP"
    )]
    List(LdapAccesskeyListArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "delete access key pairs for LDAP"
    )]
    Remove(LdapAccesskeyRemoveArgs),
    #[command(name = "info", about = "info about given access key pairs for LDAP")]
    Info(LdapAccesskeyInfoArgs),
    #[command(name = "create", about = "create access key pairs for LDAP")]
    Create(LdapAccesskeyCreateArgs),
    #[command(
        name = "create-with-login",
        about = "login using LDAP credentials to generate access key pair"
    )]
    CreateWithLogin(LdapAccesskeyCreateWithLoginArgs),
    #[command(name = "edit", about = "edit existing access keys for LDAP")]
    Edit(LdapAccesskeyEditArgs),
    #[command(name = "enable", about = "enable an access key")]
    Enable(LdapAccesskeyEnableArgs),
    #[command(name = "disable", about = "disable an access key")]
    Disable(LdapAccesskeyDisableArgs),
    #[command(
        name = "sts-revoke",
        about = "revokes all STS accounts or specified types for the specified user"
    )]
    StsRevoke(LdapAccesskeyStsRevokeArgs),
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyListArgs {
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
    #[arg(long = "all", help = "list all access keys for all LDAP users")]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyRemoveArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: String,
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyInfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "ACCESSKEY", required = true)]
    pub accesskey: Vec<String>,
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyCreateArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "DN")]
    pub dn: Option<String>,
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
    #[arg(
        long = "login",
        hide = true,
        help = "log in using ldap credentials to generate access key pair for future use"
    )]
    pub login: bool,
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyCreateWithLoginArgs {
    #[arg(value_name = "URL")]
    pub url: String,
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
pub struct LdapAccesskeyEditArgs {
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
pub struct LdapAccesskeyEnableArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Option<String>,
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyDisableArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Option<String>,
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyStsRevokeArgs {
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

pub fn run(args: LdapArgs, json: bool) -> Result<()> {
    match args.command {
        LdapCommand::Add(args) => add(args, json),
        LdapCommand::Update(args) => update(args, json),
        LdapCommand::Remove(args) => remove(args, json),
        LdapCommand::List(args) => list(args, json),
        LdapCommand::Info(args) => info(args, json),
        LdapCommand::Enable(args) => enable(args, json),
        LdapCommand::Disable(args) => disable(args, json),
        LdapCommand::Policy(args) => policy(args, json),
        LdapCommand::Accesskey(args) => accesskey(args, json),
    }
}

fn add(args: LdapAddArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap add")
}

fn update(args: LdapUpdateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap update")
}

fn remove(args: LdapRemoveArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap remove")
}

fn list(args: LdapListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap list")
}

fn info(args: LdapInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap info")
}

fn enable(args: LdapEnableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap enable")
}

fn disable(args: LdapDisableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap disable")
}

fn policy(args: LdapPolicyArgs, json: bool) -> Result<()> {
    match args.command {
        LdapPolicyCommand::Attach(args) => policy_attach(args, json),
        LdapPolicyCommand::Detach(args) => policy_detach(args, json),
        LdapPolicyCommand::Entities(args) => policy_entities(args, json),
    }
}

fn policy_attach(args: LdapPolicyAttachArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap policy attach")
}

fn policy_detach(args: LdapPolicyDetachArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap policy detach")
}

fn policy_entities(args: LdapPolicyEntitiesArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap policy entities")
}

fn accesskey(args: LdapAccesskeyArgs, json: bool) -> Result<()> {
    match args.command {
        LdapAccesskeyCommand::List(args) => accesskey_list(args, json),
        LdapAccesskeyCommand::Remove(args) => accesskey_remove(args, json),
        LdapAccesskeyCommand::Info(args) => accesskey_info(args, json),
        LdapAccesskeyCommand::Create(args) => accesskey_create(args, json),
        LdapAccesskeyCommand::CreateWithLogin(args) => accesskey_create_with_login(args, json),
        LdapAccesskeyCommand::Edit(args) => accesskey_edit(args, json),
        LdapAccesskeyCommand::Enable(args) => accesskey_enable(args, json),
        LdapAccesskeyCommand::Disable(args) => accesskey_disable(args, json),
        LdapAccesskeyCommand::StsRevoke(args) => accesskey_sts_revoke(args, json),
    }
}

fn accesskey_list(args: LdapAccesskeyListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap accesskey list")
}

fn accesskey_remove(args: LdapAccesskeyRemoveArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap accesskey remove")
}

fn accesskey_info(args: LdapAccesskeyInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap accesskey info")
}

fn accesskey_create(args: LdapAccesskeyCreateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap accesskey create")
}

fn accesskey_create_with_login(args: LdapAccesskeyCreateWithLoginArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap accesskey create-with-login")
}

fn accesskey_edit(args: LdapAccesskeyEditArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap accesskey edit")
}

fn accesskey_enable(args: LdapAccesskeyEnableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap accesskey enable")
}

fn accesskey_disable(args: LdapAccesskeyDisableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap accesskey disable")
}

fn accesskey_sts_revoke(args: LdapAccesskeyStsRevokeArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("idp ldap accesskey sts-revoke")
}
