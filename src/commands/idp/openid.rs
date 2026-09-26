//! `mx idp openid` (mc `idp openid`): OpenID IDP configuration and access keys.
//!
//! Owner: IDP. Positional arguments are collected loosely and counted like mc (wrong counts
//! print the command help and exit with status 1).

use super::accesskey::{self, EditFlags, ListFlags};
use super::ldap::{AccesskeyArgs, AccesskeyEditArgs};
use super::show_help_and_exit;
use crate::s3::admin_idp;
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
    Add(OpenidCfgArgs),
    #[command(name = "update", about = "Update an OpenID IDP configuration")]
    Update(OpenidCfgArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "remove OpenID IDP server configuration"
    )]
    Remove(OpenidNameArgs),
    #[command(
        name = "list",
        visible_alias = "ls",
        about = "list OpenID IDP server configuration(s)"
    )]
    List(OpenidTargetArgs),
    #[command(name = "info", about = "get OpenID IDP server configuration info")]
    Info(OpenidNameArgs),
    #[command(name = "enable", about = "enable an OpenID IDP server configuration")]
    Enable(OpenidNameArgs),
    #[command(name = "disable", about = "Disable an OpenID IDP server configuration")]
    Disable(OpenidNameArgs),
    #[command(name = "accesskey", about = "manage OpenID access key pairs")]
    Accesskey(OpenidAccesskeyArgs),
}

/// `TARGET [CFG_NAME] [CFG_PARAMS...]`.
#[derive(Debug, Args)]
pub struct OpenidCfgArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "CFG_NAME")]
    pub cfg_name: Option<String>,
    #[arg(value_name = "CFG_PARAMS")]
    pub cfg_params: Vec<String>,
}

/// `TARGET [CFG_NAME]`.
#[derive(Debug, Args)]
pub struct OpenidNameArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "CFG_NAME")]
    pub cfg_name: Vec<String>,
}

impl OpenidNameArgs {
    /// (target, config name as given) for 1 or 2 arguments, else the command help.
    fn split(&self, path: &[&str]) -> (&str, Option<&str>) {
        match (self.target.as_deref(), self.cfg_name.as_slice()) {
            (Some(target), []) => (target, None),
            (Some(target), [name]) => (target, Some(name.as_str())),
            _ => show_help_and_exit(path),
        }
    }
}

/// `TARGET`.
#[derive(Debug, Args)]
pub struct OpenidTargetArgs {
    #[arg(value_name = "TARGET")]
    pub args: Vec<String>,
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
    Remove(AccesskeyArgs),
    #[command(name = "info", about = "info about given access key pairs for OpenID")]
    Info(AccesskeyArgs),
    #[command(name = "edit", about = "edit existing access keys for OpenID")]
    Edit(AccesskeyEditArgs),
    #[command(name = "enable", about = "enable an access key")]
    Enable(AccesskeyArgs),
    #[command(name = "disable", about = "disable an access key")]
    Disable(AccesskeyArgs),
}

#[derive(Debug, Args)]
pub struct OpenidAccesskeyListArgs {
    #[arg(value_name = "TARGET[:CFGNAME]")]
    pub target: Option<String>,
    #[arg(value_name = "USER/ID")]
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

const OPENID: &[&str] = &["idp", "openid"];

fn path(names: &[&'static str]) -> Vec<&'static str> {
    OPENID.iter().chain(names).copied().collect()
}

pub fn run(args: OpenidArgs, json: bool) -> Result<()> {
    match args.command {
        OpenidCommand::Add(args) => add_or_update(&args, false, json),
        OpenidCommand::Update(args) => add_or_update(&args, true, json),
        OpenidCommand::Remove(args) => {
            // mc passes the name as given (empty for the default config).
            let (target, name) = args.split(&path(&["remove"]));
            super::remove(target, true, name.unwrap_or_default(), json)
        }
        OpenidCommand::List(args) => match args.args.as_slice() {
            [target] => super::list(target, true, json),
            _ => show_help_and_exit(&path(&["list"])),
        },
        OpenidCommand::Info(args) => {
            let (target, name) = args.split(&path(&["info"]));
            super::info(target, true, name.unwrap_or_default(), json)
        }
        OpenidCommand::Enable(args) => {
            let (target, name) = args.split(&path(&["enable"]));
            let name = name.unwrap_or(admin_idp::DEFAULT_NAME);
            super::enable_disable(target, true, name, true, json)
        }
        OpenidCommand::Disable(args) => {
            let (target, name) = args.split(&path(&["disable"]));
            let name = name.unwrap_or(admin_idp::DEFAULT_NAME);
            super::enable_disable(target, true, name, false, json)
        }
        OpenidCommand::Accesskey(args) => accesskey_run(args, json),
    }
}

fn accesskey_run(args: OpenidAccesskeyArgs, json: bool) -> Result<()> {
    match args.command {
        OpenidAccesskeyCommand::List(args) => {
            let list: Vec<String> = args.target.iter().chain(&args.users).cloned().collect();
            let flags = ListFlags {
                users_only: args.users_only,
                temp_only: args.temp_only,
                svcacc_only: args.svcacc_only,
                self_: args.self_,
                all: args.all,
                all_configs: args.all_configs,
            };
            accesskey::openid_list(&path(&["accesskey", "list"]), &list, &flags, json)
        }
        OpenidAccesskeyCommand::Remove(args) => {
            accesskey::remove(&path(&["accesskey", "remove"]), &args.args(), json)
        }
        OpenidAccesskeyCommand::Info(args) => {
            accesskey::info(&path(&["accesskey", "info"]), &args.args(), json)
        }
        OpenidAccesskeyCommand::Edit(args) => {
            let list: Vec<String> = args.target.iter().chain(&args.accesskey).cloned().collect();
            edit(&list, &args.flags, json)
        }
        OpenidAccesskeyCommand::Enable(args) => {
            accesskey::enable_disable(&path(&["accesskey", "enable"]), &args.args(), true, json)
        }
        OpenidAccesskeyCommand::Disable(args) => {
            accesskey::enable_disable(&path(&["accesskey", "disable"]), &args.args(), false, json)
        }
    }
}

fn edit(list: &[String], flags: &EditFlags, json: bool) -> Result<()> {
    accesskey::edit(&path(&["accesskey", "edit"]), list, flags, json)
}

/// mc `mainIDPOpenIDAddOrUpdate`.
fn add_or_update(args: &OpenidCfgArgs, update: bool, json: bool) -> Result<()> {
    let list: Vec<String> = args
        .target
        .iter()
        .chain(&args.cfg_name)
        .chain(&args.cfg_params)
        .cloned()
        .collect();
    if list.len() < 2 {
        show_help_and_exit(&path(&[if update { "update" } else { "add" }]))
    }
    let (name, params) = super::split_cfg_args(&list);
    super::add_or_update(
        &list[0],
        true,
        &name,
        &params,
        update,
        "Unable to add OpenID IDP config to server",
        json,
    )
}
