//! `mx admin accesskey` (mc `admin accesskey`): access keys (service accounts and STS keys)
//! of builtin users.

use super::user::{
    admin_client, block_on, fmt_message, parse_expiry, print_msg, read_policy_file, show_help,
};
use crate::error::McError;
use crate::s3::admin_iam::{self as iam, GoJson, GoTime, ServiceAccountInfo};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

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
    Enable(AccesskeyTargetArgs),
    #[command(name = "disable", about = "disable an access key")]
    Disable(AccesskeyTargetArgs),
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
    #[arg(value_name = "USER")]
    pub users: Vec<String>,
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
    /// Extra arguments: mc shows the command help.
    #[arg(hide = true)]
    pub extra: Vec<String>,
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
    #[arg(hide = true)]
    pub extra: Vec<String>,
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
    #[arg(hide = true)]
    pub extra: Vec<String>,
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
pub struct AccesskeyTargetArgs {
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Option<String>,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct AccesskeyStsRevokeArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "USER")]
    pub user: Option<String>,
    #[arg(hide = true)]
    pub extra: Vec<String>,
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
        AccesskeyCommand::Enable(args) => set_status(args, json, true),
        AccesskeyCommand::Disable(args) => set_status(args, json, false),
        AccesskeyCommand::StsRevoke(args) => sts_revoke(args, json),
    }
}

/// mc `nilExpiry`: the server's "no expiry" (`timeSentinel`) as no expiration.
fn nil_expiry(expiration: Option<String>) -> Option<String> {
    expiration.filter(|exp| !GoTime::parse(exp).is_some_and(|time| time.is_sentinel()))
}

fn humanize_or(expiration: Option<&str>, default: &str) -> String {
    expiration
        .and_then(GoTime::parse)
        .map(|time| time.humanize())
        .unwrap_or_else(|| default.to_string())
}

/// mc `userAccesskeyList`.
#[derive(Debug, Serialize)]
struct UserAccesskeyList {
    status: &'static str,
    user: String,
    #[serde(rename = "stsKeys")]
    sts_keys: Option<Vec<ServiceAccountInfo>>,
    svcaccs: Option<Vec<ServiceAccountInfo>>,
}

impl UserAccesskeyList {
    fn text(&self) -> String {
        let mut out = format!("User: {}\n", self.user);
        let sts = self.sts_keys.as_deref().unwrap_or_default();
        let svc = self.svcaccs.as_deref().unwrap_or_default();
        if !sts.is_empty() || !svc.is_empty() {
            out.push_str("  Access Keys:\n");
        }
        for (keys, is_sts) in [(sts, true), (svc, false)] {
            for key in keys {
                let expiration = nil_expiry(key.expiration.clone());
                out.push_str(&format!(
                    "    {}, expires: {}, sts: {is_sts}\n",
                    key.access_key,
                    humanize_or(expiration.as_deref(), "never")
                ));
            }
        }
        out
    }
}

/// mc `commonAccesskeyList` flag checks: `Invalid flags.` errors.
fn list_flags_error(args: &AccesskeyListArgs) -> Option<&'static str> {
    let exclusive = [args.users_only, args.temp_only, args.svcacc_only]
        .iter()
        .filter(|set| **set)
        .count();
    if exclusive > 1 {
        Some("only one of --users-only, --temp-only, or --permanent-only can be specified")
    } else if args.self_ && args.all {
        Some("only one of --self or --all can be specified")
    } else if (args.self_ || args.all) && !args.users.is_empty() {
        Some("users cannot be specified with --self or --all")
    } else {
        None
    }
}

fn list(args: AccesskeyListArgs, json: bool) -> Result<()> {
    let (alias, config_name) = args
        .target
        .split_once(':')
        .unwrap_or((args.target.as_str(), ""));
    if let Some(cause) = list_flags_error(&args) {
        return Err(anyhow::Error::new(McError::new(cause)).context("Invalid flags."));
    }
    // Without users, --self or --all mc tries --all and falls back to --self when denied.
    let tentative_all = !args.self_ && !args.all && args.users.is_empty();
    let list_type = if args.users_only {
        "users-only"
    } else if args.temp_only {
        "sts-only"
    } else if args.svcacc_only {
        "svcacc-only"
    } else {
        "all"
    };
    let mut opts = iam::ListAccessKeysOpts {
        list_type: list_type.to_string(),
        all: args.all || tentative_all,
        config_name: config_name.to_string(),
    };
    let client = admin_client(alias)?;
    let mut result = block_on(iam::list_access_keys_bulk(&client, &args.users, &opts));
    if let Err(err) = &result
        && tentative_all
        && crate::error::mc_error(err).is_some_and(|e| e.message == "Access Denied.")
    {
        opts.all = false;
        result = block_on(iam::list_access_keys_bulk(&client, &args.users, &opts));
    }
    let keys = result.context("Unable to list access keys.")?;
    for (user, keys) in keys {
        let message = UserAccesskeyList {
            status: "success",
            user,
            sts_keys: keys.sts_keys,
            svcaccs: keys.service_accounts,
        };
        print_msg(json, &message, &message.text())?;
    }
    Ok(())
}

/// Provider specific access key info (mc `ldapAccessKeyInfo` / `openIDAccessKeyInfo`).
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ProviderInfo {
    Ldap {
        #[serde(skip_serializing_if = "String::is_empty")]
        username: String,
    },
    OpenId {
        #[serde(rename = "configName")]
        config_name: String,
        #[serde(rename = "userID")]
        user_id: String,
        #[serde(rename = "userIDClaim")]
        user_id_claim: String,
        #[serde(rename = "displayName", skip_serializing_if = "String::is_empty")]
        display_name: String,
        #[serde(rename = "displayNameClaim", skip_serializing_if = "String::is_empty")]
        display_name_claim: String,
    },
}

impl ProviderInfo {
    fn text(&self) -> String {
        match self {
            ProviderInfo::Ldap { username } => format!("Username: {username}"),
            ProviderInfo::OpenId {
                config_name,
                user_id,
                user_id_claim,
                display_name,
                display_name_claim,
            } => {
                let config = if config_name == "_" {
                    "_ (default)"
                } else {
                    config_name
                };
                let mut out = format!("  Config: {config}\n");
                if !display_name_claim.is_empty() {
                    out.push_str(&format!(
                        "  Display Name ({display_name_claim}): {display_name}\n"
                    ));
                }
                out.push_str(&format!("  User ID ({user_id_claim}): {user_id}\n"));
                out
            }
        }
    }
}

/// mc `accesskeyMessage`.
#[derive(Debug, Default, Serialize)]
struct AccesskeyMessage {
    status: &'static str,
    #[serde(rename = "accessKey")]
    access_key: String,
    #[serde(rename = "secretKey", skip_serializing_if = "String::is_empty")]
    secret_key: String,
    #[serde(rename = "parentUser", skip_serializing_if = "String::is_empty")]
    parent_user: String,
    #[serde(rename = "accountStatus", skip_serializing_if = "String::is_empty")]
    account_status: String,
    #[serde(rename = "impliedPolicy", skip_serializing_if = "std::ops::Not::not")]
    implied_policy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<GoJson>,
    #[serde(skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expiration: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    provider: String,
    #[serde(rename = "providerInfo", skip_serializing_if = "Option::is_none")]
    provider_info: Option<ProviderInfo>,
}

impl AccesskeyMessage {
    fn new(access_key: &str) -> Self {
        Self {
            status: "success",
            access_key: access_key.to_string(),
            ..Default::default()
        }
    }

    /// Expiration text: `NONE` unless set (not zero, not the sentinel).
    fn expiration_text(&self, format: fn(&GoTime) -> String) -> String {
        self.expiration
            .as_deref()
            .and_then(GoTime::parse)
            .filter(|time| !time.is_zero() && !time.is_sentinel())
            .map(|time| format(&time))
            .unwrap_or_else(|| "NONE".to_string())
    }

    fn info_text(&self) -> String {
        let policy = if self.implied_policy {
            "implied"
        } else {
            "embedded"
        };
        let status = if self.account_status == "off" {
            "disabled"
        } else {
            "enabled"
        };
        let mut out = [
            format!("Access Key: {}", self.access_key),
            format!("Parent User: {}", self.parent_user),
            format!("Status: {status}"),
            format!("Policy: {policy}"),
            format!("Name: {}", self.name),
            format!("Description: {}", self.description),
            format!("Expiration: {}", self.expiration_text(GoTime::humanize)),
            "STS: false".to_string(),
            format!("Provider: {}", self.provider),
        ]
        .join("\n");
        out.push('\n');
        if let Some(info) = &self.provider_info {
            out.push_str("Provider Specific Info:\n");
            out.push_str(&info.text());
        }
        out
    }

    fn create_text(&self) -> String {
        [
            format!("Access Key: {}", self.access_key),
            format!("Secret Key: {}", self.secret_key),
            format!("Expiration: {}", self.expiration_text(GoTime::go_string)),
            format!("Name: {}", self.name),
            format!("Description: {}", self.description),
        ]
        .join("\n")
    }
}

fn remove(args: AccesskeyRemoveArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "accesskey", "remove"]);
    }
    let client = admin_client(&args.target)?;
    block_on(iam::delete_service_account(&client, &args.accesskey))
        .context("Unable to remove service account.")?;
    print_msg(
        json,
        &AccesskeyMessage::new(&args.accesskey),
        &format!("Successfully removed access key `{}`.", args.accesskey),
    )
}

fn info(args: AccesskeyInfoArgs, json: bool) -> Result<()> {
    let client = admin_client(&args.target)?;
    for access_key in &args.accesskey {
        let res = block_on(iam::info_access_key(&client, access_key))
            .context("Unable to get info for access key.")?;
        let provider_info = match res.user_provider.as_str() {
            "ldap" => Some(ProviderInfo::Ldap {
                username: res.ldap_specific_info.username,
            }),
            "openid" => {
                let info = res.openid_specific_info;
                Some(ProviderInfo::OpenId {
                    config_name: info.config_name,
                    user_id: info.user_id,
                    user_id_claim: info.user_id_claim,
                    display_name: info.display_name,
                    display_name_claim: info.display_name_claim,
                })
            }
            _ => None,
        };
        let info = res.info;
        let message = AccesskeyMessage {
            parent_user: info.parent_user,
            account_status: info.account_status,
            implied_policy: info.implied_policy,
            policy: iam::raw_policy(&info.policy),
            name: info.name,
            description: info.description,
            expiration: nil_expiry(info.expiration),
            provider: res.user_provider,
            provider_info,
            ..AccesskeyMessage::new(access_key)
        };
        print_msg(json, &message, &message.info_text())?;
    }
    Ok(())
}

/// `--expiry-duration` like mc's `ctx.Duration`: invalid values count as unset (0).
fn expiry_duration(value: Option<&str>) -> i128 {
    value.and_then(iam::parse_go_duration).unwrap_or(0)
}

/// mc: `--expiry` and `--expiry-duration` are exclusive.
fn check_expiry_flags(expiry: &str, duration: i128) -> Result<()> {
    if !expiry.is_empty() && duration != 0 {
        return Err(anyhow::Error::new(McError::new(
            "Only one of --expiry or --expiry-duration can be specified",
        ))
        .context("invalid flags"));
    }
    Ok(())
}

/// `--expiry` / `--expiry-duration` as the request's expiration.
fn expiration(expiry: &str, duration: i128) -> Result<Option<String>> {
    if !expiry.is_empty() {
        return parse_expiry(
            expiry,
            format!("invalid expiry date format '{expiry}'"),
            "unable to parse the expiry argument",
        )
        .map(Some);
    }
    Ok((duration != 0).then(|| GoTime::after(duration).rfc3339_nano()))
}

fn create(args: AccesskeyCreateArgs, json: bool) -> Result<()> {
    let Some(target) = args.target.filter(|_| args.extra.is_empty()) else {
        show_help(&["admin", "accesskey", "create"]);
    };
    let expiry = args.expiry.unwrap_or_default();
    let duration = expiry_duration(args.expiry_duration.as_deref());
    let mut access_key = args.access_key.unwrap_or_default();
    let mut secret_key = args.secret_key.unwrap_or_default();
    if access_key.is_empty() || secret_key.is_empty() {
        let (random_access, random_secret) = iam::generate_credentials()
            .context("unable to generate randomized access credentials")?;
        if access_key.is_empty() {
            access_key = random_access;
        }
        if secret_key.is_empty() {
            secret_key = random_secret;
        }
    }
    check_expiry_flags(&expiry, duration)?;
    let mut req = iam::AddServiceAccountReq {
        target_user: args.user.unwrap_or_default(),
        access_key,
        secret_key,
        name: args.name.unwrap_or_default(),
        description: args.description.unwrap_or_default(),
        ..Default::default()
    };
    if let Some(path) = args.policy.filter(|path| !path.is_empty()) {
        req.policy = Some(read_policy_file(&path)?);
    }
    req.expiration = expiration(&expiry, duration)?;
    let client = admin_client(&target)?;
    let creds = block_on(iam::add_service_account(&client, &req))
        .context("Unable to add service account.")?;
    let message = AccesskeyMessage {
        secret_key: creds.secret_key,
        expiration: Some(
            creds
                .expiration
                .unwrap_or_else(|| iam::ZERO_TIME.to_string()),
        ),
        name: req.name,
        description: req.description,
        ..AccesskeyMessage::new(&creds.access_key)
    };
    print_msg(json, &message, &message.create_text())
}

fn edit(args: AccesskeyEditArgs, json: bool) -> Result<()> {
    let Some(target) = args.target.filter(|_| args.extra.is_empty()) else {
        show_help(&["admin", "accesskey", "edit"]);
    };
    let access_key = args.accesskey.unwrap_or_default();
    let expiry = args.expiry.unwrap_or_default();
    let duration = expiry_duration(args.expiry_duration.as_deref());
    let policy = args.policy.unwrap_or_default();
    let mut req = iam::UpdateServiceAccountReq {
        new_secret_key: args.secret_key.unwrap_or_default(),
        new_name: args.name.unwrap_or_default(),
        new_description: args.description.unwrap_or_default(),
        ..Default::default()
    };
    if req.new_name.is_empty()
        && expiry.is_empty()
        && duration == 0
        && policy.is_empty()
        && req.new_secret_key.is_empty()
        && req.new_description.is_empty()
    {
        return Err(
            anyhow::Error::new(McError::new("At least one property must be edited"))
                .context("invalid flags"),
        );
    }
    check_expiry_flags(&expiry, duration)?;
    if !policy.is_empty() {
        req.new_policy = Some(read_policy_file(&policy)?);
    }
    req.new_expiration = expiration(&expiry, duration)?;
    let client = admin_client(&target)?;
    block_on(iam::update_service_account(&client, &access_key, &req))
        .context("Unable to edit service account.")?;
    print_msg(
        json,
        &AccesskeyMessage::new(&access_key),
        &format!("Successfully edited access key `{access_key}`."),
    )
}

fn set_status(args: AccesskeyTargetArgs, json: bool, enable: bool) -> Result<()> {
    let (name, status, done) = if enable {
        ("enable", "on", "enabled")
    } else {
        ("disable", "off", "disabled")
    };
    let Some(target) = args.target.filter(|_| args.extra.is_empty()) else {
        show_help(&["admin", "accesskey", name]);
    };
    let access_key = args.accesskey.unwrap_or_default();
    let client = admin_client(&target)?;
    let req = iam::UpdateServiceAccountReq {
        new_status: status.into(),
        ..Default::default()
    };
    block_on(iam::update_service_account(&client, &access_key, &req))
        .context("Unable to add service account.")?;
    print_msg(
        json,
        &AccesskeyMessage::new(&access_key),
        &format!("Successfully {done} access key `{access_key}`."),
    )
}

/// mc `stsRevokeMessage`.
#[derive(Debug, Serialize)]
struct StsRevokeMessage {
    status: &'static str,
    user: String,
    #[serde(rename = "tokenRevokeType", skip_serializing_if = "String::is_empty")]
    token_revoke_type: String,
}

impl StsRevokeMessage {
    fn text(&self) -> String {
        let user = if self.user.is_empty() {
            "authenticated user".to_string()
        } else {
            format!("user {}", self.user)
        };
        if self.token_revoke_type.is_empty() {
            format!("Successfully revoked all STS accounts for {user}")
        } else {
            format!(
                "Successfully revoked all STS accounts of type {} for {user}",
                self.token_revoke_type
            )
        }
    }
}

fn sts_revoke(args: AccesskeyStsRevokeArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "accesskey", "sts-revoke"]);
    }
    let user = args.user.unwrap_or_default();
    let token_type = args.token_type.unwrap_or_default();
    if !args.self_ && user.is_empty() {
        return Err(anyhow::Error::new(McError::invalid_argument())
            .context("Must specify user or use --self flag."));
    }
    if args.self_ && !user.is_empty() {
        return Err(anyhow::Error::new(McError::invalid_argument())
            .context("Cannot specify user with --self flag."));
    }
    if args.all == !token_type.is_empty() {
        return Err(anyhow::Error::new(super::user::dummy_error())
            .context("Exactly one of --all or --token-type must be specified."));
    }
    let client = admin_client(&args.alias)?;
    block_on(iam::revoke_tokens(&client, &user, &token_type, args.all))
        .with_context(|| fmt_message("Unable to revoke tokens for %s", &user))?;
    let message = StsRevokeMessage {
        status: "success",
        user,
        token_revoke_type: token_type,
    };
    print_msg(json, &message, &message.text())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_text_like_mc() {
        let message = UserAccesskeyList {
            status: "success",
            user: "user1".into(),
            sts_keys: None,
            svcaccs: Some(vec![ServiceAccountInfo {
                access_key: "svc1key".into(),
                expiration: Some("1970-01-01T00:00:00Z".into()),
                ..Default::default()
            }]),
        };
        assert_eq!(
            message.text(),
            "User: user1\n  Access Keys:\n    svc1key, expires: never, sts: false\n"
        );
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"status":"success","user":"user1","stsKeys":null,"svcaccs":[{"parentUser":"","accountStatus":"","impliedPolicy":false,"accessKey":"svc1key","expiration":"1970-01-01T00:00:00Z"}]}"#
        );
    }

    #[test]
    fn info_and_create_text_like_mc() {
        let message = AccesskeyMessage {
            parent_user: "user1".into(),
            account_status: "on".into(),
            implied_policy: true,
            name: "n1".into(),
            provider: "builtin".into(),
            ..AccesskeyMessage::new("ak1")
        };
        assert_eq!(
            message.info_text(),
            "Access Key: ak1\nParent User: user1\nStatus: enabled\nPolicy: implied\nName: n1\nDescription: \nExpiration: NONE\nSTS: false\nProvider: builtin\n"
        );
        let message = AccesskeyMessage {
            secret_key: "secret".into(),
            expiration: Some("2026-09-27T19:25:55Z".into()),
            ..AccesskeyMessage::new("ak1")
        };
        assert_eq!(
            message.create_text(),
            "Access Key: ak1\nSecret Key: secret\nExpiration: 2026-09-27 19:25:55 +0000 UTC\nName: \nDescription: "
        );
    }

    #[test]
    fn provider_info_json() {
        let info = ProviderInfo::Ldap {
            username: "bob".into(),
        };
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"username":"bob"}"#
        );
        assert_eq!(info.text(), "Username: bob");
    }

    #[test]
    fn sts_revoke_text() {
        let message = StsRevokeMessage {
            status: "success",
            user: String::new(),
            token_revoke_type: "x".into(),
        };
        assert_eq!(
            message.text(),
            "Successfully revoked all STS accounts of type x for authenticated user"
        );
    }
}
