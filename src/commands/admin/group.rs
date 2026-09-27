//! `mx admin group` (mc `admin group`): builtin IAM groups.

use super::user::{admin_client, block_on, print_msg, show_help};
use crate::s3::admin_iam as iam;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

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
    Info(GroupTargetArgs),
    #[command(name = "list", visible_alias = "ls", about = "display list of groups")]
    List(GroupListArgs),
    #[command(name = "enable", about = "enable a group")]
    Enable(GroupTargetArgs),
    #[command(name = "disable", about = "disable a group")]
    Disable(GroupTargetArgs),
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
pub struct GroupTargetArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "GROUPNAME")]
    pub groupname: String,
    /// Extra arguments: mc shows the command help.
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct GroupListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

/// mc `groupMessage`.
#[derive(Debug, Default, Serialize)]
struct GroupMessage {
    status: &'static str,
    #[serde(rename = "groupName", skip_serializing_if = "String::is_empty")]
    group_name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    groups: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    members: Vec<String>,
    #[serde(rename = "groupStatus", skip_serializing_if = "String::is_empty")]
    group_status: String,
    #[serde(rename = "groupPolicy", skip_serializing_if = "String::is_empty")]
    group_policy: String,
}

impl GroupMessage {
    fn new(group: &str) -> Self {
        Self {
            status: "success",
            group_name: group.to_string(),
            ..Default::default()
        }
    }
}

pub fn run(args: GroupArgs, json: bool) -> Result<()> {
    match args.command {
        GroupCommand::Add(args) => add(args, json),
        GroupCommand::Remove(args) => remove(args, json),
        GroupCommand::Info(args) => info(args, json),
        GroupCommand::List(args) => list(args, json),
        GroupCommand::Enable(args) => set_status(args, json, true),
        GroupCommand::Disable(args) => set_status(args, json, false),
    }
}

fn add(args: GroupAddArgs, json: bool) -> Result<()> {
    let client = admin_client(&args.target)?;
    block_on(iam::update_group_members(
        &client,
        &args.groupname,
        &args.members,
        false,
    ))
    .context("Unable to add new group")?;
    let text = format!(
        "Added members `{}` to group `{}` successfully.",
        args.members.join(","),
        args.groupname
    );
    let message = GroupMessage {
        members: args.members,
        ..GroupMessage::new(&args.groupname)
    };
    print_msg(json, &message, &text)
}

fn remove(args: GroupRemoveArgs, json: bool) -> Result<()> {
    let client = admin_client(&args.target)?;
    block_on(iam::update_group_members(
        &client,
        &args.groupname,
        &args.usernames,
        true,
    ))
    .context("Could not perform remove operation")?;
    let text = if args.usernames.is_empty() {
        format!("Removed group {} successfully.", args.groupname)
    } else {
        format!(
            "Removed members {{{}}} from group {} successfully.",
            args.usernames.join(","),
            args.groupname
        )
    };
    let message = GroupMessage {
        members: args.usernames,
        ..GroupMessage::new(&args.groupname)
    };
    print_msg(json, &message, &text)
}

fn info(args: GroupTargetArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "group", "info"]);
    }
    let client = admin_client(&args.target)?;
    let desc = block_on(iam::group_description(&client, &args.groupname))
        .context("Unable to fetch group info")?;
    let text = [
        format!("Group: {}", args.groupname),
        format!("Status: {}", desc.status),
        format!("Policy: {}", desc.policy),
        format!("Members: {}", desc.members.join(",")),
    ]
    .join("\n");
    let message = GroupMessage {
        group_status: desc.status,
        group_policy: desc.policy,
        members: desc.members,
        ..GroupMessage::new(&args.groupname)
    };
    print_msg(json, &message, &text)
}

fn list(args: GroupListArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "group", "list"]);
    }
    let client = admin_client(&args.target)?;
    let groups = block_on(iam::list_groups(&client)).context("Unable to list groups")?;
    let text = groups.join("\n");
    let message = GroupMessage {
        groups,
        ..GroupMessage::new("")
    };
    print_msg(json, &message, &text)
}

fn set_status(args: GroupTargetArgs, json: bool, enable: bool) -> Result<()> {
    let (name, status) = if enable {
        ("enable", "enabled")
    } else {
        ("disable", "disabled")
    };
    if !args.extra.is_empty() {
        show_help(&["admin", "group", name]);
    }
    let client = admin_client(&args.target)?;
    block_on(iam::set_group_status(&client, &args.groupname, status))
        .context("Unable set group status")?;
    let text = if enable {
        format!("Enabled group `{}` successfully.", args.groupname)
    } else {
        format!("Disabled group `{}` successfully.", args.groupname)
    };
    let message = GroupMessage {
        group_status: status.to_string(),
        ..GroupMessage::new(&args.groupname)
    };
    print_msg(json, &message, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_message_json_omits_empty_fields() {
        let message = GroupMessage {
            members: vec!["u1".into()],
            ..GroupMessage::new("g1")
        };
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"status":"success","groupName":"g1","members":["u1"]}"#
        );
        assert_eq!(
            serde_json::to_string(&GroupMessage::new("")).unwrap(),
            r#"{"status":"success"}"#
        );
    }
}
