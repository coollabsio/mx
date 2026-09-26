//! `mx admin group` (mc `admin group`).
//!
//! Owner: IAM. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct GroupArgs {
    #[command(subcommand)]
    pub command: GroupCommand,
}

#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    #[command(name = "add", about = "add users to a new or existing group")]
    Add(GroupAddArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "remove group or members from a group"
    )]
    Remove(GroupRemoveArgs),
    #[command(name = "info", about = "display group info")]
    Info(GroupInfoArgs),
    #[command(name = "list", visible_alias = "ls", about = "display list of groups")]
    List(GroupListArgs),
    #[command(name = "enable", about = "enable a group")]
    Enable(GroupEnableArgs),
    #[command(name = "disable", about = "disable a group")]
    Disable(GroupDisableArgs),
}

#[derive(Debug, Args)]
pub struct GroupAddArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "GROUPNAME")]
    pub groupname: String,
    #[arg(value_name = "MEMBERS", required = true)]
    pub members: Vec<String>,
}

#[derive(Debug, Args)]
pub struct GroupRemoveArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "GROUPNAME")]
    pub groupname: String,
    #[arg(value_name = "USERNAMES")]
    pub usernames: Vec<String>,
}

#[derive(Debug, Args)]
pub struct GroupInfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "GROUPNAME")]
    pub groupname: String,
}

#[derive(Debug, Args)]
pub struct GroupListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct GroupEnableArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "GROUPNAME")]
    pub groupname: String,
}

#[derive(Debug, Args)]
pub struct GroupDisableArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "GROUPNAME")]
    pub groupname: String,
}

pub fn run(args: GroupArgs, json: bool) -> Result<()> {
    match args.command {
        GroupCommand::Add(args) => add(args, json),
        GroupCommand::Remove(args) => remove(args, json),
        GroupCommand::Info(args) => info(args, json),
        GroupCommand::List(args) => list(args, json),
        GroupCommand::Enable(args) => enable(args, json),
        GroupCommand::Disable(args) => disable(args, json),
    }
}

fn add(args: GroupAddArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin group add")
}

fn remove(args: GroupRemoveArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin group remove")
}

fn info(args: GroupInfoArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin group info")
}

fn list(args: GroupListArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin group list")
}

fn enable(args: GroupEnableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin group enable")
}

fn disable(args: GroupDisableArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("admin group disable")
}
