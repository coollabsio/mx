//! `mx admin policy` (mc `admin policy`).
//!
//! Owner: IAM. Stubs return "not implemented yet" until implemented.

use crate::commands::admin::deprecated;
use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct PolicyArgs {
    #[command(subcommand)]
    pub command: PolicyCommand,
}

#[derive(Debug, Subcommand)]
pub enum PolicyCommand {
    #[command(name = "create", about = "create a new IAM policy")]
    Create(PolicyCreateArgs),
    #[command(name = "remove", visible_alias = "rm", about = "remove an IAM policy")]
    Remove(PolicyRemoveArgs),
    #[command(name = "list", visible_alias = "ls", about = "list all IAM policies")]
    List(PolicyListArgs),
    #[command(name = "info", about = "show info on an IAM policy")]
    Info(PolicyInfoArgs),
    #[command(name = "attach", about = "attach an IAM policy to a user or group")]
    Attach(PolicyAttachArgs),
    #[command(name = "detach", about = "detach an IAM policy from a user or group")]
    Detach(PolicyDetachArgs),
    #[command(name = "entities", about = "list policy association entities")]
    Entities(PolicyEntitiesArgs),
    #[command(name = "add", hide = true, about = "add an IAM policy")]
    Add(PolicyAddArgs),
    #[command(name = "set", hide = true, about = "set IAM policy on a user or group")]
    Set(PolicySetArgs),
    #[command(
        name = "unset",
        hide = true,
        about = "unset an IAM policy for a user or group"
    )]
    Unset(PolicyUnsetArgs),
    #[command(
        name = "update",
        hide = true,
        about = "attach a new IAM policy to user or group"
    )]
    Update(PolicyUpdateArgs),
}

#[derive(Debug, Args)]
pub struct PolicyCreateArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POLICYNAME")]
    pub policyname: String,
    #[arg(value_name = "POLICYFILE")]
    pub policyfile: String,
}

#[derive(Debug, Args)]
pub struct PolicyRemoveArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POLICYNAME")]
    pub policyname: String,
}

#[derive(Debug, Args)]
pub struct PolicyListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct PolicyInfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POLICYNAME")]
    pub policyname: String,
    #[arg(
        long = "policy-file",
        short = 'f',
        value_name = "VALUE",
        help = "additionally (over-)write policy JSON to given file"
    )]
    pub policy_file: Option<String>,
}

#[derive(Debug, Args)]
pub struct PolicyAttachArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POLICIES", required = true)]
    pub policies: Vec<String>,
    #[arg(
        long = "user",
        short = 'u',
        value_name = "VALUE",
        help = "attach policy to user"
    )]
    pub user: Option<String>,
    #[arg(
        long = "group",
        short = 'g',
        value_name = "VALUE",
        help = "attach policy to group"
    )]
    pub group: Option<String>,
}

#[derive(Debug, Args)]
pub struct PolicyDetachArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POLICIES", required = true)]
    pub policies: Vec<String>,
    #[arg(
        long = "user",
        short = 'u',
        value_name = "VALUE",
        help = "detach policy from user"
    )]
    pub user: Option<String>,
    #[arg(
        long = "group",
        short = 'g',
        value_name = "VALUE",
        help = "detach policy from group"
    )]
    pub group: Option<String>,
}

#[derive(Debug, Args)]
pub struct PolicyEntitiesArgs {
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
pub struct PolicyAddArgs {
    /// Accepted and ignored (deprecated command).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct PolicySetArgs {
    /// Accepted and ignored (deprecated command).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct PolicyUnsetArgs {
    /// Accepted and ignored (deprecated command).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct PolicyUpdateArgs {
    /// Accepted and ignored (deprecated command).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub args: Vec<String>,
}

pub fn run(args: PolicyArgs, json: bool) -> Result<()> {
    match args.command {
        PolicyCommand::Create(args) => create(args, json),
        PolicyCommand::Remove(args) => remove(args, json),
        PolicyCommand::List(args) => list(args, json),
        PolicyCommand::Info(args) => info(args, json),
        PolicyCommand::Attach(args) => attach(args, json),
        PolicyCommand::Detach(args) => detach(args, json),
        PolicyCommand::Entities(args) => entities(args, json),
        PolicyCommand::Add(args) => add(args, json),
        PolicyCommand::Set(args) => set(args, json),
        PolicyCommand::Unset(args) => unset(args, json),
        PolicyCommand::Update(args) => update(args, json),
    }
}

fn create(args: PolicyCreateArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin policy create")
}

fn remove(args: PolicyRemoveArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin policy remove")
}

fn list(args: PolicyListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin policy list")
}

fn info(args: PolicyInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin policy info")
}

fn attach(args: PolicyAttachArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin policy attach")
}

fn detach(args: PolicyDetachArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin policy detach")
}

fn entities(args: PolicyEntitiesArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin policy entities")
}

fn add(_args: PolicyAddArgs, _json: bool) -> Result<()> {
    deprecated("admin policy create")
}

fn set(_args: PolicySetArgs, _json: bool) -> Result<()> {
    deprecated("admin policy attach")
}

fn unset(_args: PolicyUnsetArgs, _json: bool) -> Result<()> {
    deprecated("admin policy detach")
}

fn update(_args: PolicyUpdateArgs, _json: bool) -> Result<()> {
    deprecated("admin policy attach")
}
