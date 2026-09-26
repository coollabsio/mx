//! `mx admin policy` (mc `admin policy`): canned IAM policies and their user/group
//! associations. `add`, `set`, `unset` and `update` are mc's deprecated names.

use super::user::{
    admin_client, admin_client_with, block_on, path_error, print_msg, read_file, show_help,
};
use crate::commands::admin::deprecated;
use crate::output;
use crate::s3::admin_iam::{self as iam, GoTime, PolicyEntitiesResult, PolicyInfo};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

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
    /// Extra arguments: mc shows the command help.
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct PolicyRemoveArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POLICYNAME")]
    pub policyname: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct PolicyListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct PolicyInfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "POLICYNAME")]
    pub policyname: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
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
    #[arg(hide = true)]
    pub extra: Vec<String>,
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

/// mc `userPolicyMessage` (`policyInfo` is a struct, so Go always marshals it).
#[derive(Debug, Default, Serialize)]
struct UserPolicyMessage {
    status: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    policy: String,
    #[serde(rename = "policyInfo")]
    policy_info: PolicyInfo,
    #[serde(rename = "isGroup")]
    is_group: bool,
}

impl UserPolicyMessage {
    fn new(policy: &str) -> Self {
        Self {
            status: "success",
            policy: policy.to_string(),
            ..Default::default()
        }
    }
}

pub fn run(args: PolicyArgs, json: bool) -> Result<()> {
    match args.command {
        PolicyCommand::Create(args) => create(args, json),
        PolicyCommand::Remove(args) => remove(args, json),
        PolicyCommand::List(args) => list(args, json),
        PolicyCommand::Info(args) => info(args, json),
        PolicyCommand::Attach(args) => associate(
            json,
            true,
            &args.target,
            args.policies,
            args.user,
            args.group,
        ),
        PolicyCommand::Detach(args) => associate(
            json,
            false,
            &args.target,
            args.policies,
            args.user,
            args.group,
        ),
        PolicyCommand::Entities(args) => entities(args, json),
        PolicyCommand::Add(_) => deprecated("admin policy create"),
        PolicyCommand::Set(_) => deprecated("admin policy attach"),
        PolicyCommand::Unset(_) => deprecated("admin policy detach"),
        PolicyCommand::Update(_) => deprecated("admin policy attach"),
    }
}

fn create(args: PolicyCreateArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "policy", "create"]);
    }
    let policy = read_file(&args.policyfile).context("Unable to get policy")?;
    let client = admin_client(&args.target)?;
    block_on(iam::add_canned_policy(&client, &args.policyname, policy))
        .context("Unable to create new policy")?;
    print_msg(
        json,
        &UserPolicyMessage::new(&args.policyname),
        &format!("Created policy `{}` successfully.", args.policyname),
    )
}

fn remove(args: PolicyRemoveArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "policy", "remove"]);
    }
    let client = admin_client(&args.target)?;
    block_on(iam::remove_canned_policy(&client, &args.policyname))
        .context("Unable to remove policy")?;
    print_msg(
        json,
        &UserPolicyMessage::new(&args.policyname),
        &format!("Removed policy `{}` successfully.", args.policyname),
    )
}

fn list(args: PolicyListArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "policy", "list"]);
    }
    let client = admin_client(&args.target)?;
    let policies = block_on(iam::list_canned_policies(&client)).context("Unable to list policy")?;
    for policy in policies {
        print_msg(json, &UserPolicyMessage::new(&policy), &policy)?;
    }
    Ok(())
}

fn info(args: PolicyInfoArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "policy", "info"]);
    }
    let client = admin_client_with(&args.target, "Unable to initialize admin connection")?;
    let info =
        block_on(iam::policy_info(&client, &args.policyname)).context("Unable to fetch policy")?;
    if let Some(path) = args.policy_file.filter(|path| !path.is_empty()) {
        std::fs::write(&path, &info.policy_raw)
            .map_err(|err| path_error("open", &path, &err))
            .context("Could not open given policy file")?;
    }
    // mc marshals `PolicyInfo` (a json.Marshaler) compactly, even in text mode.
    let text = output::format_json(&info, true)?;
    let message = UserPolicyMessage {
        policy_info: info,
        ..UserPolicyMessage::new(&args.policyname)
    };
    print_msg(json, &message, &text)
}

/// mc `policyAssociationMessage`.
#[derive(Debug, Serialize)]
struct PolicyAssociationMessage {
    status: &'static str,
    #[serde(rename = "policiesAttached", skip_serializing_if = "Vec::is_empty")]
    policies_attached: Vec<String>,
    #[serde(rename = "policiesDetached", skip_serializing_if = "Vec::is_empty")]
    policies_detached: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    user: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    group: String,
}

impl PolicyAssociationMessage {
    fn text(&self, attach: bool) -> String {
        let (mut label, mut policies) = ("Attached", &self.policies_attached);
        let (mut entity_label, mut entity) = ("To User:", &self.user);
        if !self.user.is_empty() {
            if !attach {
                (label, policies) = ("Detached", &self.policies_detached);
                entity_label = "From User:";
            }
        } else if !self.group.is_empty() {
            entity = &self.group;
            if attach {
                entity_label = "To Group:";
            } else {
                (label, policies) = ("Detached", &self.policies_detached);
                entity_label = "From Group:";
            }
        }
        format!(
            "{label} Policies: [{}]\n{entity_label} {entity}",
            policies.join(" ")
        )
    }
}

/// mc `userAttachOrDetachPolicy`.
fn associate(
    json: bool,
    attach: bool,
    target: &str,
    policies: Vec<String>,
    user: Option<String>,
    group: Option<String>,
) -> Result<()> {
    let req = iam::PolicyAssociationReq {
        policies,
        user: user.unwrap_or_default(),
        group: group.unwrap_or_default(),
    };
    let client = admin_client(target)?;
    let mut res = match block_on(iam::attach_detach_policy(&client, attach, &req)) {
        Ok(res) => res,
        Err(err)
            if crate::error::error_code(&err) == Some("XMinioAdminPolicyChangeAlreadyApplied") =>
        {
            iam::PolicyAssociationResp::default()
        }
        Err(err) => return Err(err.context("Unable to make user/group policy association")),
    };
    if res.is_empty() {
        if attach {
            res.policies_attached = req.policies.clone();
        } else {
            res.policies_detached = req.policies.clone();
        }
    }
    let message = PolicyAssociationMessage {
        status: "success",
        policies_attached: res.policies_attached,
        policies_detached: res.policies_detached,
        user: req.user,
        group: req.group,
    };
    print_msg(json, &message, &message.text(attach))
}

/// mc `policyEntities`.
#[derive(Debug, Serialize)]
pub(crate) struct PolicyEntitiesMessage {
    status: &'static str,
    result: PolicyEntitiesResult,
}

/// mc `builderWrapper`: comma-separated values wrapped at `max_len` columns.
fn wrap_list(values: &[String], out: &mut String, indent: usize, max_len: usize) {
    let mut len = 0;
    for value in values {
        if len + value.len() > max_len && len > 0 {
            out.push('\n');
            len = 0;
        }
        if len == 0 {
            out.push_str(&" ".repeat(indent));
            len = indent;
        } else {
            out.push_str(", ");
            len += 2;
        }
        let value = if value.contains(',') {
            format!("\"{value}\"")
        } else {
            value.clone()
        };
        out.push_str(&value);
        len += value.len();
    }
    out.push('\n');
}

impl PolicyEntitiesMessage {
    pub(crate) fn new(result: PolicyEntitiesResult) -> Self {
        Self {
            status: "success",
            result,
        }
    }

    /// mc `policyEntities.String()`.
    pub(crate) fn text(&self) -> String {
        let result = &self.result;
        let time = GoTime::parse(&result.timestamp)
            .map(|t| t.rfc3339())
            .unwrap_or_else(|| result.timestamp.clone());
        let mut out = format!("Query time: {time}\n");
        if !result.user_mappings.is_empty() {
            out.push_str("User -> Policy Mappings:\n");
            for user in &result.user_mappings {
                let policies = user.policies.clone().unwrap_or_default();
                out.push_str(&format!("  User: {}\n", user.user));
                out.push_str("    Policies:\n");
                wrap_list(&policies, &mut out, 6, 80);
                if !user.member_of_mappings.is_empty() {
                    let mut effective: std::collections::BTreeSet<String> =
                        policies.iter().cloned().collect();
                    out.push_str("    Group Memberships:\n");
                    let groups: Vec<String> = user
                        .member_of_mappings
                        .iter()
                        .map(|g| {
                            effective.extend(g.policies.clone().unwrap_or_default());
                            g.group.clone()
                        })
                        .collect();
                    wrap_list(&groups, &mut out, 6, 80);
                    out.push_str("    Effective Policies:\n");
                    let effective: Vec<String> = effective.into_iter().collect();
                    wrap_list(&effective, &mut out, 6, 80);
                }
            }
        }
        if !result.group_mappings.is_empty() {
            out.push_str("Group -> Policy Mappings:\n");
            for group in &result.group_mappings {
                out.push_str(&format!("  Group: {}\n", group.group));
                out.push_str("    Policies:\n");
                for policy in group.policies.iter().flatten() {
                    out.push_str(&format!("      {policy}\n"));
                }
            }
        }
        if !result.policy_mappings.is_empty() {
            out.push_str("Policy -> Entity Mappings:\n");
            for policy in &result.policy_mappings {
                out.push_str(&format!("  Policy: {}\n", policy.policy));
                let users = policy.users.clone().unwrap_or_default();
                if !users.is_empty() {
                    out.push_str("    User Mappings:\n");
                    for user in users {
                        out.push_str(&format!("      {user}\n"));
                    }
                }
                let groups = policy.groups.clone().unwrap_or_default();
                if !groups.is_empty() {
                    out.push_str("    Group Mappings:\n");
                    for group in groups {
                        out.push_str(&format!("      {group}\n"));
                    }
                }
            }
        }
        out
    }

    /// Prints like mc: its `JSON()` indents with two spaces (compacted when not a terminal).
    pub(crate) fn print(&self, json: bool) -> Result<()> {
        if !json {
            return print_msg(false, self, &self.text());
        }
        if !output::stdout_is_terminal() {
            return output::print_json(self);
        }
        let mut buf = Vec::new();
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"  ");
        let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
        self.serialize(&mut serializer)?;
        println!("{}", String::from_utf8(buf)?);
        Ok(())
    }
}

fn entities(args: PolicyEntitiesArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "policy", "entities"]);
    }
    let client = admin_client(&args.target)?;
    let result = block_on(iam::policy_entities(
        &client,
        &args.user,
        &args.group,
        &args.policy,
    ))
    .context("Unable to fetch policy entities")?;
    PolicyEntitiesMessage::new(result).print(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn association_text_like_mc() {
        let message = PolicyAssociationMessage {
            status: "success",
            policies_attached: vec!["readonly".into(), "diag".into()],
            policies_detached: vec![],
            user: "u1".into(),
            group: String::new(),
        };
        assert_eq!(
            message.text(true),
            "Attached Policies: [readonly diag]\nTo User: u1"
        );
        let message = PolicyAssociationMessage {
            status: "success",
            policies_attached: vec![],
            policies_detached: vec!["p".into()],
            user: String::new(),
            group: "g1".into(),
        };
        assert_eq!(
            message.text(false),
            "Detached Policies: [p]\nFrom Group: g1"
        );
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"status":"success","policiesDetached":["p"],"group":"g1"}"#
        );
    }

    #[test]
    fn entities_text_like_mc() {
        let result: PolicyEntitiesResult = serde_json::from_str(
            r#"{"timestamp":"2026-09-26T19:25:42.431787438Z","userMappings":[{"user":"user1","policies":["readonly"],"memberOfMappings":[{"group":"grp1","policies":["writeonly"]}]}],"groupMappings":[{"group":"grp1","policies":["writeonly"]}],"policyMappings":[{"policy":"readonly","users":["user1"],"groups":null}]}"#,
        )
        .unwrap();
        let message = PolicyEntitiesMessage::new(result);
        assert_eq!(
            message.text(),
            "Query time: 2026-09-26T19:25:42Z
User -> Policy Mappings:
  User: user1
    Policies:
      readonly
    Group Memberships:
      grp1
    Effective Policies:
      readonly, writeonly
Group -> Policy Mappings:
  Group: grp1
    Policies:
      writeonly
Policy -> Entity Mappings:
  Policy: readonly
    User Mappings:
      user1
"
        );
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"status":"success","result":{"timestamp":"2026-09-26T19:25:42.431787438Z","userMappings":[{"user":"user1","policies":["readonly"],"memberOfMappings":[{"group":"grp1","policies":["writeonly"]}]}],"groupMappings":[{"group":"grp1","policies":["writeonly"]}],"policyMappings":[{"policy":"readonly","users":["user1"],"groups":null}]}}"#
        );
    }

    #[test]
    fn wrap_list_wraps_and_quotes() {
        let mut out = String::new();
        let values: Vec<String> = vec!["a".repeat(40), "b".repeat(40), "c,d".into()];
        wrap_list(&values, &mut out, 6, 80);
        assert_eq!(
            out,
            format!(
                "      {}\n      {}, \"c,d\"\n",
                "a".repeat(40),
                "b".repeat(40)
            )
        );
    }

    #[test]
    fn policy_message_always_has_policy_info() {
        assert_eq!(
            serde_json::to_string(&UserPolicyMessage::new("p1")).unwrap(),
            r#"{"status":"success","policy":"p1","policyInfo":{"PolicyName":"","Policy":null},"isGroup":false}"#
        );
    }
}
