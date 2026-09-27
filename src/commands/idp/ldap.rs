//! `mx idp ldap` (mc `idp ldap`): LDAP IDP configuration, policy mappings and access keys.
//!
//! Owner: IDP. Positional arguments are collected loosely and counted like mc (wrong counts
//! print the command help and exit with status 1).

use super::accesskey::{self, CreateFlags, EditFlags, ListFlags, StsRevokeFlags};
use super::{admin_client, print_msg, show_help_and_exit};
use crate::commands::runtime;
use crate::error::McError;
use crate::s3::admin_idp::{self, PolicyAssociationReq, PolicyEntitiesResult};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct LdapArgs {
    #[command(subcommand)]
    pub command: LdapCommand,
}

#[derive(Debug, Subcommand)]
pub enum LdapCommand {
    #[command(name = "add", about = "Create an LDAP IDP server configuration")]
    Add(CfgParamsArgs),
    #[command(name = "update", about = "Update an LDAP IDP configuration")]
    Update(CfgParamsArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "remove LDAP IDP server configuration"
    )]
    Remove(TargetArgs),
    #[command(
        name = "list",
        visible_alias = "ls",
        about = "list LDAP IDP server configuration(s)"
    )]
    List(TargetArgs),
    #[command(name = "info", about = "get LDAP IDP server configuration info")]
    Info(TargetArgs),
    #[command(name = "enable", about = "manage LDAP IDP server configuration")]
    Enable(TargetArgs),
    #[command(name = "disable", about = "Disable an LDAP IDP server configuration")]
    Disable(TargetArgs),
    #[command(name = "policy", about = "manage policy assignments for LDAP")]
    Policy(LdapPolicyArgs),
    #[command(name = "accesskey", about = "manage LDAP access key pairs")]
    Accesskey(LdapAccesskeyArgs),
}

/// `TARGET [CFG_PARAMS...]`.
#[derive(Debug, Args)]
pub struct CfgParamsArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "CFG_PARAMS")]
    pub cfg_params: Vec<String>,
}

impl CfgParamsArgs {
    fn args(&self) -> Vec<String> {
        self.target
            .iter()
            .chain(&self.cfg_params)
            .cloned()
            .collect()
    }
}

/// `TARGET` (exactly one).
#[derive(Debug, Args)]
pub struct TargetArgs {
    #[arg(value_name = "TARGET")]
    pub args: Vec<String>,
}

impl TargetArgs {
    fn target(&self, path: &[&str]) -> &str {
        match self.args.as_slice() {
            [target] => target,
            _ => show_help_and_exit(path),
        }
    }
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
    Detach(LdapPolicyAttachArgs),
    #[command(name = "entities", about = "list policy association entities")]
    Entities(LdapPolicyEntitiesArgs),
}

#[derive(Debug, Args)]
pub struct LdapPolicyAttachArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "POLICY")]
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
    pub args: Vec<String>,
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
    Remove(AccesskeyArgs),
    #[command(name = "info", about = "info about given access key pairs for LDAP")]
    Info(AccesskeyArgs),
    #[command(name = "create", about = "create access key pairs for LDAP")]
    Create(LdapAccesskeyCreateArgs),
    #[command(
        name = "create-with-login",
        about = "login using LDAP credentials to generate access key pair"
    )]
    CreateWithLogin(LdapAccesskeyCreateWithLoginArgs),
    #[command(name = "edit", about = "edit existing access keys for LDAP")]
    Edit(AccesskeyEditArgs),
    #[command(name = "enable", about = "enable an access key")]
    Enable(AccesskeyArgs),
    #[command(name = "disable", about = "disable an access key")]
    Disable(AccesskeyArgs),
    #[command(
        name = "sts-revoke",
        about = "revokes all STS accounts or specified types for the specified user"
    )]
    StsRevoke(StsRevokeArgs),
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyListArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
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

/// `TARGET ACCESSKEY...` (counts checked per command).
#[derive(Debug, Args)]
pub struct AccesskeyArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Vec<String>,
}

impl AccesskeyArgs {
    pub(crate) fn args(&self) -> Vec<String> {
        self.target.iter().chain(&self.accesskey).cloned().collect()
    }
}

#[derive(Debug, Args)]
pub struct AccesskeyEditArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Vec<String>,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyCreateArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "DN")]
    pub dn: Vec<String>,
    #[command(flatten)]
    pub flags: CreateFlags,
}

#[derive(Debug, Args)]
pub struct LdapAccesskeyCreateWithLoginArgs {
    #[arg(value_name = "URL")]
    pub url: Option<String>,
    #[arg(value_name = "ARGS", hide = true)]
    pub rest: Vec<String>,
    #[command(flatten)]
    pub flags: CreateFlags,
}

#[derive(Debug, Args)]
pub struct StsRevokeArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: Option<String>,
    #[arg(value_name = "USER")]
    pub user: Vec<String>,
    #[command(flatten)]
    pub flags: StsRevokeFlags,
}

const LDAP: &[&str] = &["idp", "ldap"];

fn path(names: &[&'static str]) -> Vec<&'static str> {
    LDAP.iter().chain(names).copied().collect()
}

pub fn run(args: LdapArgs, json: bool) -> Result<()> {
    match args.command {
        LdapCommand::Add(args) => add_or_update(&args, false, json),
        LdapCommand::Update(args) => add_or_update(&args, true, json),
        LdapCommand::Remove(args) => {
            let target = args.target(&path(&["remove"]));
            super::remove(target, false, admin_idp::DEFAULT_NAME, json)
        }
        LdapCommand::List(args) => super::list(args.target(&path(&["list"])), false, json),
        LdapCommand::Info(args) => super::info(
            args.target(&path(&["info"])),
            false,
            admin_idp::DEFAULT_NAME,
            json,
        ),
        LdapCommand::Enable(args) => super::enable_disable(
            args.target(&path(&["enable"])),
            false,
            admin_idp::DEFAULT_NAME,
            true,
            json,
        ),
        LdapCommand::Disable(args) => super::enable_disable(
            args.target(&path(&["disable"])),
            false,
            admin_idp::DEFAULT_NAME,
            false,
            json,
        ),
        LdapCommand::Policy(args) => match args.command {
            LdapPolicyCommand::Attach(args) => policy_attach_detach(&args, true, json),
            LdapPolicyCommand::Detach(args) => policy_attach_detach(&args, false, json),
            LdapPolicyCommand::Entities(args) => policy_entities(&args, json),
        },
        LdapCommand::Accesskey(args) => accesskey_run(args, json),
    }
}

fn accesskey_run(args: LdapAccesskeyArgs, json: bool) -> Result<()> {
    match args.command {
        LdapAccesskeyCommand::List(args) => {
            let list: Vec<String> = args.target.iter().chain(&args.dn).cloned().collect();
            let flags = ListFlags {
                users_only: args.users_only,
                temp_only: args.temp_only,
                svcacc_only: args.svcacc_only,
                self_: args.self_,
                all: args.all,
                all_configs: false,
            };
            accesskey::ldap_list(&path(&["accesskey", "list"]), &list, &flags, json)
        }
        LdapAccesskeyCommand::Remove(args) => {
            accesskey::remove(&path(&["accesskey", "remove"]), &args.args(), json)
        }
        LdapAccesskeyCommand::Info(args) => {
            accesskey::info(&path(&["accesskey", "info"]), &args.args(), json)
        }
        LdapAccesskeyCommand::Create(args) => {
            let list: Vec<String> = args.target.iter().chain(&args.dn).cloned().collect();
            accesskey::ldap_create(&path(&["accesskey", "create"]), &list, &args.flags, json)
        }
        LdapAccesskeyCommand::CreateWithLogin(args) => accesskey::create_with_login(
            &path(&["accesskey", "create-with-login"]),
            args.url.as_deref(),
            &args.flags,
            json,
        ),
        LdapAccesskeyCommand::Edit(args) => {
            let list: Vec<String> = args.target.iter().chain(&args.accesskey).cloned().collect();
            accesskey::edit(&path(&["accesskey", "edit"]), &list, &args.flags, json)
        }
        LdapAccesskeyCommand::Enable(args) => {
            accesskey::enable_disable(&path(&["accesskey", "enable"]), &args.args(), true, json)
        }
        LdapAccesskeyCommand::Disable(args) => {
            accesskey::enable_disable(&path(&["accesskey", "disable"]), &args.args(), false, json)
        }
        LdapAccesskeyCommand::StsRevoke(args) => {
            let list: Vec<String> = args.alias.iter().chain(&args.user).cloned().collect();
            accesskey::sts_revoke(
                &path(&["accesskey", "sts-revoke"]),
                &list,
                &args.flags,
                json,
            )
        }
    }
}

/// mc `mainIDPLDAPAdd` / `mainIDPLDAPUpdate`.
fn add_or_update(args: &CfgParamsArgs, update: bool, json: bool) -> Result<()> {
    let list = args.args();
    if list.len() < 2 {
        show_help_and_exit(&path(&[if update { "update" } else { "add" }]))
    }
    let target = &list[0];
    // Only the admin client is created before the config check, like mc.
    super::admin_client(target)?;
    let (name, params) = super::split_cfg_args(&list);
    if name != admin_idp::DEFAULT_NAME {
        return Err(anyhow::Error::new(McError::new(
            "all config parameters must be of the form \"key=value\"",
        ))
        .context("Bad LDAP IDP configuration"));
    }
    let context = if update {
        "Unable to update LDAP IDP configuration"
    } else {
        "Unable to add LDAP IDP config to server"
    };
    super::add_or_update(target, false, &name, &params, update, context, json)
}

/// mc `policyAssociationMessage`.
#[derive(Debug, Serialize)]
struct PolicyAssociationMessage<'a> {
    #[serde(skip)]
    attach: bool,
    status: &'static str,
    #[serde(
        rename = "policiesAttached",
        skip_serializing_if = "<[String]>::is_empty"
    )]
    policies_attached: &'a [String],
    #[serde(
        rename = "policiesDetached",
        skip_serializing_if = "<[String]>::is_empty"
    )]
    policies_detached: &'a [String],
    #[serde(skip_serializing_if = "str::is_empty")]
    user: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    group: &'a str,
}

impl PolicyAssociationMessage<'_> {
    fn text(&self) -> String {
        let (mut label, mut policies) = ("Attached Policies:", self.policies_attached);
        let (mut entity_label, mut entity) = ("To User:", self.user);
        if !self.user.is_empty() && !self.attach {
            (label, policies) = ("Detached Policies:", self.policies_detached);
            entity_label = "From User:";
        } else if self.user.is_empty() && !self.group.is_empty() {
            entity = self.group;
            if self.attach {
                entity_label = "To Group:";
            } else {
                (label, policies) = ("Detached Policies:", self.policies_detached);
                entity_label = "From Group:";
            }
        }
        format!(
            "{label} [{}]\n{entity_label} {entity}\n",
            policies.join(" ")
        )
    }
}

/// mc `mainIDPLdapPolicyAttach` / `mainIDPLdapPolicyDetach`.
fn policy_attach_detach(args: &LdapPolicyAttachArgs, attach: bool, json: bool) -> Result<()> {
    let list: Vec<String> = args.target.iter().chain(&args.policies).cloned().collect();
    if list.len() < 2 {
        show_help_and_exit(&path(&["policy", if attach { "attach" } else { "detach" }]))
    }
    let user = args.user.clone().unwrap_or_default();
    let group = args.group.clone().unwrap_or_default();
    let req = PolicyAssociationReq {
        policies: list[1..].to_vec(),
        user: user.clone(),
        group: group.clone(),
    };
    if attach {
        req.validate().context("Invalid policy attach arguments.")?;
    } else if user.is_empty() && group.is_empty() {
        return Err(anyhow::Error::new(McError::new(
            "at least one of --user or --group is required.",
        ))
        .context("Missing flag in command"));
    }
    let client = admin_client(&list[0])?;
    let res = runtime()?
        .block_on(admin_idp::ldap_policy_association(&client, attach, &req))
        .context("Unable to make LDAP policy association")?;
    let attached = res.policies_attached.unwrap_or_default();
    let detached = res.policies_detached.unwrap_or_default();
    let message = PolicyAssociationMessage {
        attach,
        status: "success",
        policies_attached: if attach { &attached } else { &[] },
        policies_detached: if attach { &[] } else { &detached },
        user: &user,
        group: &group,
    };
    print_msg(json, &message, &message.text(), 1)
}

/// mc `mainIDPLdapPolicyEntities`.
fn policy_entities(args: &LdapPolicyEntitiesArgs, json: bool) -> Result<()> {
    let [target] = args.args.as_slice() else {
        show_help_and_exit(&path(&["policy", "entities"]))
    };
    let client = admin_client(target)?;
    let result = runtime()?
        .block_on(admin_idp::ldap_policy_entities(
            &client,
            &args.user,
            &args.group,
            &args.policy,
        ))
        .context("Unable to fetch LDAP policy entities")?;

    #[derive(Serialize)]
    struct PolicyEntitiesMessage<'a> {
        status: &'static str,
        result: &'a PolicyEntitiesResult,
    }
    let message = PolicyEntitiesMessage {
        status: "success",
        result: &result,
    };
    print_msg(json, &message, &entities_text(&result), 2)
}

/// mc `builderWrapper`: comma-separated items wrapped at `max_len`, quoted when they contain
/// a comma.
fn wrap_list(items: &[String], out: &mut String, indent: usize, max_len: usize) {
    let mut len = 0;
    for item in items {
        if len + item.len() > max_len && len > 0 {
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
        let item = if item.contains(',') {
            format!("\"{item}\"")
        } else {
            item.clone()
        };
        out.push_str(&item);
        len += item.len();
    }
    out.push('\n');
}

/// Go `time.Format(time.RFC3339)` of a marshaled time: fractional seconds dropped.
fn rfc3339_seconds(time: &str) -> String {
    match time.find('.') {
        Some(dot) => {
            let end = time[dot + 1..]
                .find(|c: char| !c.is_ascii_digit())
                .map_or(time.len(), |i| dot + 1 + i);
            format!("{}{}", &time[..dot], &time[end..])
        }
        None => time.to_string(),
    }
}

/// mc `policyEntities.String()`.
fn entities_text(result: &PolicyEntitiesResult) -> String {
    let mut out = format!("Query time: {}\n", rfc3339_seconds(&result.timestamp));
    let users = result.user_mappings.as_deref().unwrap_or_default();
    if !users.is_empty() {
        out.push_str("User -> Policy Mappings:\n");
        for user in users {
            out.push_str(&format!("  User: {}\n", user.user));
            out.push_str("    Policies:\n");
            let policies = user.policies.as_deref().unwrap_or_default();
            wrap_list(policies, &mut out, 6, 80);
            let groups = user.member_of_mappings.as_deref().unwrap_or_default();
            if !groups.is_empty() {
                let mut effective: std::collections::BTreeSet<String> =
                    policies.iter().cloned().collect();
                out.push_str("    Group Memberships:\n");
                let names: Vec<String> = groups.iter().map(|g| g.group.clone()).collect();
                for group in groups {
                    effective.extend(group.policies.iter().flatten().cloned());
                }
                wrap_list(&names, &mut out, 6, 80);
                out.push_str("    Effective Policies:\n");
                let effective: Vec<String> = effective.into_iter().collect();
                wrap_list(&effective, &mut out, 6, 80);
            }
        }
    }
    let groups = result.group_mappings.as_deref().unwrap_or_default();
    if !groups.is_empty() {
        out.push_str("Group -> Policy Mappings:\n");
        for group in groups {
            out.push_str(&format!("  Group: {}\n", group.group));
            out.push_str("    Policies:\n");
            for policy in group.policies.iter().flatten() {
                out.push_str(&format!("      {policy}\n"));
            }
        }
    }
    let policies = result.policy_mappings.as_deref().unwrap_or_default();
    if !policies.is_empty() {
        out.push_str("Policy -> Entity Mappings:\n");
        for policy in policies {
            out.push_str(&format!("  Policy: {}\n", policy.policy));
            let users = policy.users.as_deref().unwrap_or_default();
            if !users.is_empty() {
                out.push_str("    User Mappings:\n");
                for user in users {
                    out.push_str(&format!("      {user}\n"));
                }
            }
            let groups = policy.groups.as_deref().unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_text_matches_mc() {
        let result: PolicyEntitiesResult = serde_json::from_str(
            r#"{"timestamp":"2026-09-26T19:27:02.910404621Z",
            "userMappings":[{"user":"uid=dillon,dc=io","policies":["readwrite"],
              "memberOfMappings":[{"group":"cn=a,dc=io","policies":["readonly","diagnostics"]}]}],
            "groupMappings":[{"group":"cn=a,dc=io","policies":["diagnostics","readonly"]}],
            "policyMappings":[{"policy":"readwrite","users":["uid=dillon,dc=io"],"groups":null}]}"#,
        )
        .unwrap();
        assert_eq!(
            entities_text(&result),
            "Query time: 2026-09-26T19:27:02Z\n\
             User -> Policy Mappings:\n  User: uid=dillon,dc=io\n    Policies:\n      readwrite\n\
             \x20   Group Memberships:\n      \"cn=a,dc=io\"\n    Effective Policies:\n\
             \x20     diagnostics, readonly, readwrite\n\
             Group -> Policy Mappings:\n  Group: cn=a,dc=io\n    Policies:\n      diagnostics\n      readonly\n\
             Policy -> Entity Mappings:\n  Policy: readwrite\n    User Mappings:\n      uid=dillon,dc=io\n"
        );
    }

    #[test]
    fn wrap_list_breaks_at_max_len() {
        let items: Vec<String> = (0..6).map(|i| format!("policy-number-{i}")).collect();
        let mut out = String::new();
        wrap_list(&items, &mut out, 6, 40);
        assert_eq!(
            out,
            "      policy-number-0, policy-number-1\n      policy-number-2, policy-number-3\n      policy-number-4, policy-number-5\n"
        );
        let mut out = String::new();
        wrap_list(&[], &mut out, 6, 80);
        assert_eq!(out, "\n");
    }

    #[test]
    fn association_text_matches_mc() {
        let policies = vec!["readwrite".to_string()];
        let message = PolicyAssociationMessage {
            attach: true,
            status: "success",
            policies_attached: &policies,
            policies_detached: &[],
            user: "uid=x",
            group: "",
        };
        assert_eq!(
            message.text(),
            "Attached Policies: [readwrite]\nTo User: uid=x\n"
        );
        let message = PolicyAssociationMessage {
            attach: false,
            policies_attached: &[],
            policies_detached: &policies,
            user: "",
            group: "cn=g",
            ..message
        };
        assert_eq!(
            message.text(),
            "Detached Policies: [readwrite]\nFrom Group: cn=g\n"
        );
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"status":"success","policiesDetached":["readwrite"],"group":"cn=g"}"#
        );
    }

    #[test]
    fn rfc3339_drops_fraction() {
        assert_eq!(
            rfc3339_seconds("2026-09-26T19:27:02.910404621Z"),
            "2026-09-26T19:27:02Z"
        );
        assert_eq!(
            rfc3339_seconds("2026-09-26T19:27:02+02:00"),
            "2026-09-26T19:27:02+02:00"
        );
    }
}
