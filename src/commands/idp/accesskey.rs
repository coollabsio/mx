//! Access key commands shared by `idp ldap accesskey` and `idp openid accesskey` (mc
//! `admin-accesskey-*.go` / `idp-*-accesskey-*.go` common functions).

use super::{admin_client, print_msg, show_help_and_exit};
use crate::commands::runtime;
use crate::error::McError;
use crate::s3::admin::AdminClient;
use crate::s3::admin_idp::{
    self, AddServiceAccountReq, InfoAccessKeyResp, ListAccessKeysOpts, RevokeTokensReq,
    ServiceAccountInfo, UpdateServiceAccountReq,
};
use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use serde_json::value::RawValue;

/// mc `accessKeyEditOpts` flags (`edit`).
#[derive(Debug, Args)]
pub struct EditFlags {
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

/// mc `idpLdapAccesskeyCreateFlags` (`create`, `create-with-login`).
#[derive(Debug, Args)]
pub struct CreateFlags {
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

/// mc `sts-revoke` flags.
#[derive(Debug, Args)]
pub struct StsRevokeFlags {
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

/// `accesskey list` flags (`all_configs` is OpenID only).
#[derive(Debug, Default)]
pub struct ListFlags {
    pub users_only: bool,
    pub temp_only: bool,
    pub svcacc_only: bool,
    pub self_: bool,
    pub all: bool,
    pub all_configs: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

// ---------------------------------------------------------------------------
// messages
// ---------------------------------------------------------------------------

/// mc `ldapAccessKeyInfo` / `openIDAccessKeyInfo`.
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
                let config = if config_name == admin_idp::DEFAULT_NAME {
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
    #[serde(skip)]
    op: &'static str,
    status: &'static str,
    #[serde(rename = "accessKey")]
    access_key: String,
    #[serde(rename = "secretKey", skip_serializing_if = "String::is_empty")]
    secret_key: String,
    #[serde(skip_serializing_if = "is_false")]
    sts: bool,
    #[serde(rename = "parentUser", skip_serializing_if = "String::is_empty")]
    parent_user: String,
    #[serde(rename = "accountStatus", skip_serializing_if = "String::is_empty")]
    account_status: String,
    #[serde(rename = "impliedPolicy", skip_serializing_if = "is_false")]
    implied_policy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<Box<RawValue>>,
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
    fn simple(op: &'static str, access_key: &str) -> Self {
        Self {
            op,
            status: "success",
            access_key: access_key.to_string(),
            ..Default::default()
        }
    }

    fn text(&self) -> String {
        let expiration = self
            .expiration
            .as_deref()
            .filter(|time| !admin_idp::is_no_expiry(time));
        match self.op {
            "info" => {
                let mut out = String::new();
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
                let expiration = expiration.map_or("NONE".to_string(), humanize_time);
                out.push_str(&format!("Access Key: {}\n", self.access_key));
                out.push_str(&format!("Parent User: {}\n", self.parent_user));
                out.push_str(&format!("Status: {status}\n"));
                out.push_str(&format!("Policy: {policy}\n"));
                out.push_str(&format!("Name: {}\n", self.name));
                out.push_str(&format!("Description: {}\n", self.description));
                out.push_str(&format!("Expiration: {expiration}\n"));
                out.push_str(&format!("STS: {}\n", self.sts));
                out.push_str(&format!("Provider: {}\n", self.provider));
                if let Some(info) = &self.provider_info {
                    out.push_str("Provider Specific Info:\n");
                    out.push_str(&info.text());
                }
                out
            }
            "create" => {
                let expiration = expiration.map_or("NONE".to_string(), go_time_string);
                format!(
                    "Access Key: {}\nSecret Key: {}\nExpiration: {expiration}\nName: {}\nDescription: {}\n",
                    self.access_key, self.secret_key, self.name, self.description
                )
            }
            "remove" => format!("Successfully removed access key `{}`.", self.access_key),
            "edit" => format!("Successfully edited access key `{}`.", self.access_key),
            "enable" => format!("Successfully enabled access key `{}`.", self.access_key),
            "disable" => format!("Successfully disabled access key `{}`.", self.access_key),
            _ => String::new(),
        }
    }

    fn print(&self, json: bool) -> Result<()> {
        print_msg(json, self, &self.text(), 1)
    }
}

/// `key, expires: ..., sts: ...` lines of mc's access key lists.
fn key_lines(
    out: &mut String,
    indent: usize,
    sts: &[ServiceAccountInfo],
    svc: &[ServiceAccountInfo],
) {
    for (keys, is_sts) in [(sts, true), (svc, false)] {
        for key in keys {
            let expiration = match key.expiration.as_deref() {
                Some(time) if admin_idp::unix_seconds(time) != Some(0) => humanize_time(time),
                _ => "never".to_string(),
            };
            out.push_str(&format!(
                "{}{}, expires: {expiration}, sts: {is_sts}\n",
                " ".repeat(indent),
                key.access_key
            ));
        }
    }
}

/// mc `userAccesskeyList`.
#[derive(Debug, Serialize)]
struct UserAccesskeyList<'a> {
    status: &'static str,
    user: &'a str,
    #[serde(rename = "stsKeys")]
    sts_keys: &'a Option<Vec<ServiceAccountInfo>>,
    #[serde(rename = "svcaccs")]
    service_accounts: &'a Option<Vec<ServiceAccountInfo>>,
    #[serde(skip_serializing_if = "is_false")]
    ldap: bool,
}

impl UserAccesskeyList<'_> {
    fn text(&self) -> String {
        let label = if self.ldap { "DN" } else { "User" };
        let mut out = format!("{label}: {}\n", self.user);
        let sts = self.sts_keys.as_deref().unwrap_or_default();
        let svc = self.service_accounts.as_deref().unwrap_or_default();
        if !sts.is_empty() || !svc.is_empty() {
            out.push_str("  Access Keys:\n");
        }
        key_lines(&mut out, 4, sts, svc);
        out
    }
}

/// mc `openIDAccesskeyList`.
#[derive(Debug, Serialize)]
struct OpenIdAccesskeyList<'a> {
    status: &'static str,
    #[serde(rename = "configName")]
    config_name: &'a str,
    users: &'a Option<Vec<admin_idp::OpenIdUserAccessKeys>>,
}

impl OpenIdAccesskeyList<'_> {
    fn text(&self) -> String {
        let mut out = format!("Config Name: {}\n", self.config_name);
        for user in self.users.as_deref().unwrap_or_default() {
            out.push_str(&format!("  User ID: {}\n", user.minio_access_key));
            out.push_str(&format!("  ID: {}\n", user.id));
            if !user.readable_name.is_empty() {
                out.push_str(&format!("  Readable Name: {}\n", user.readable_name));
            }
            let sts = user.sts_keys.as_deref().unwrap_or_default();
            let svc = user.service_accounts.as_deref().unwrap_or_default();
            if !sts.is_empty() || !svc.is_empty() {
                out.push_str("    Access Keys:\n");
            }
            key_lines(&mut out, 6, sts, svc);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

/// mc `commonAccesskeyList`: (alias, tentative `--all`, users, options).
fn list_options(
    path: &[&str],
    args: &[String],
    flags: &ListFlags,
) -> Result<(String, bool, Vec<String>, ListAccessKeysOpts)> {
    let Some(target) = args.first() else {
        show_help_and_exit(path)
    };
    let users = args[1..].to_vec();
    let (alias, cfg_name) = target.split_once(':').unwrap_or((target, ""));
    let mut opts = ListAccessKeysOpts {
        all: flags.all,
        all_configs: flags.all_configs,
        config_name: cfg_name.to_string(),
        ..Default::default()
    };
    let only = [flags.users_only, flags.temp_only, flags.svcacc_only]
        .iter()
        .filter(|set| **set)
        .count();
    let invalid = if only > 1 {
        Some("only one of --users-only, --temp-only, or --permanent-only can be specified")
    } else if flags.self_ && opts.all {
        Some("only one of --self or --all can be specified")
    } else if (flags.self_ || opts.all) && !users.is_empty() {
        Some("users cannot be specified with --self or --all")
    } else {
        None
    };
    if let Some(message) = invalid {
        return Err(anyhow::Error::new(McError::new(message)).context("Invalid flags."));
    }
    let tentative = !flags.self_ && !opts.all && users.is_empty();
    if tentative {
        opts.all = true;
    }
    opts.list_type = if flags.users_only {
        admin_idp::LIST_USERS_ONLY
    } else if flags.temp_only {
        admin_idp::LIST_STS_ONLY
    } else if flags.svcacc_only {
        admin_idp::LIST_SVCACC_ONLY
    } else {
        admin_idp::LIST_ALL
    }
    .to_string();
    Ok((alias.to_string(), tentative, users, opts))
}

fn access_denied(err: &anyhow::Error) -> bool {
    crate::error::mc_error(err).is_some_and(|e| e.message == "Access Denied.")
}

/// mc `mainIDPLdapAccesskeyList`.
pub(crate) fn ldap_list(
    path: &[&str],
    args: &[String],
    flags: &ListFlags,
    json: bool,
) -> Result<()> {
    let (alias, tentative, users, mut opts) = list_options(path, args, flags)?;
    let client = admin_client(&alias)?;
    let rt = runtime()?;
    let mut result = rt.block_on(admin_idp::list_access_keys_ldap_bulk(
        &client, &users, &opts,
    ));
    if let Err(err) = &result
        && tentative
        && access_denied(err)
    {
        opts.all = false;
        result = rt.block_on(admin_idp::list_access_keys_ldap_bulk(
            &client, &users, &opts,
        ));
    }
    let keys = result.context("Unable to list access keys.")?;
    for (dn, keys) in &keys {
        let message = UserAccesskeyList {
            status: "success",
            user: dn,
            sts_keys: &keys.sts_keys,
            service_accounts: &keys.service_accounts,
            ldap: true,
        };
        print_msg(json, &message, &message.text(), 1)?;
    }
    Ok(())
}

/// mc `mainIDPOpenIDAccesskeyList`.
pub(crate) fn openid_list(
    path: &[&str],
    args: &[String],
    flags: &ListFlags,
    json: bool,
) -> Result<()> {
    let (alias, tentative, users, mut opts) = list_options(path, args, flags)?;
    let client = admin_client(&alias)?;
    let rt = runtime()?;
    let mut result = rt.block_on(admin_idp::list_access_keys_openid_bulk(
        &client, &users, &opts,
    ));
    if let Err(err) = &result
        && tentative
        && access_denied(err)
    {
        opts.all = false;
        result = rt.block_on(admin_idp::list_access_keys_openid_bulk(
            &client, &users, &opts,
        ));
    }
    let configs = result.context("Unable to list access keys.")?;
    for config in &configs {
        let message = OpenIdAccesskeyList {
            status: "success",
            config_name: &config.config_name,
            users: &config.users,
        };
        print_msg(json, &message, &message.text(), 1)?;
    }
    Ok(())
}

/// mc `commonAccesskeyInfo`.
pub(crate) fn info(path: &[&str], args: &[String], json: bool) -> Result<()> {
    if args.len() < 2 {
        show_help_and_exit(path)
    }
    let client = admin_client(&args[0])?;
    let rt = runtime()?;
    for access_key in &args[1..] {
        let res = rt
            .block_on(admin_idp::info_access_key(&client, access_key))
            .context("Unable to get info for access key.")?;
        info_message(access_key, res).print(json)?;
    }
    Ok(())
}

fn info_message(access_key: &str, res: InfoAccessKeyResp) -> AccesskeyMessage {
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
    AccesskeyMessage {
        op: "info",
        status: "success",
        access_key: access_key.to_string(),
        parent_user: res.parent_user,
        account_status: res.account_status,
        implied_policy: res.implied_policy,
        policy: raw_json(&res.policy),
        name: res.name,
        description: res.description,
        // mc `nilExpiry`: the Unix epoch sentinel means "no expiry".
        expiration: res
            .expiration
            .filter(|time| admin_idp::unix_seconds(time) != Some(0)),
        provider: res.user_provider,
        provider_info,
        ..Default::default()
    }
}

/// Go `json.RawMessage` as marshaled (compacted); `None` when empty.
fn raw_json(text: &str) -> Option<Box<RawValue>> {
    if text.is_empty() {
        return None;
    }
    RawValue::from_string(compact_json(text)).ok()
}

/// Removes insignificant whitespace from a JSON text (Go `json.Compact`), keeping key order.
pub(crate) fn compact_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in text.chars() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
            out.push(ch);
        } else if !ch.is_ascii_whitespace() {
            out.push(ch);
        }
    }
    out
}

/// mc `commonAccesskeyRemove`.
pub(crate) fn remove(path: &[&str], args: &[String], json: bool) -> Result<()> {
    if args.len() != 2 {
        show_help_and_exit(path)
    }
    let client = admin_client(&args[0])?;
    runtime()?
        .block_on(admin_idp::delete_service_account(&client, &args[1]))
        .context("Unable to remove service account.")?;
    AccesskeyMessage::simple("remove", &args[1]).print(json)
}

/// mc `enableDisableAccesskey`.
pub(crate) fn enable_disable(
    path: &[&str],
    args: &[String],
    enable: bool,
    json: bool,
) -> Result<()> {
    if args.is_empty() || args.len() > 2 {
        show_help_and_exit(path)
    }
    let access_key = args.get(1).map(String::as_str).unwrap_or_default();
    let client = admin_client(&args[0])?;
    let (op, status) = if enable {
        ("enable", "on")
    } else {
        ("disable", "off")
    };
    let req = UpdateServiceAccountReq {
        new_status: status.to_string(),
        ..Default::default()
    };
    runtime()?
        .block_on(admin_idp::update_service_account(&client, access_key, &req))
        .context("Unable to add service account.")?;
    AccesskeyMessage::simple(op, access_key).print(json)
}

/// mc `commonAccesskeyEdit`.
pub(crate) fn edit(path: &[&str], args: &[String], flags: &EditFlags, json: bool) -> Result<()> {
    if args.is_empty() || args.len() > 2 {
        show_help_and_exit(path)
    }
    let access_key = args.get(1).map(String::as_str).unwrap_or_default();
    let req = edit_request(flags)?;
    let client = admin_client(&args[0])?;
    runtime()?
        .block_on(admin_idp::update_service_account(&client, access_key, &req))
        .context("Unable to edit service account.")?;
    AccesskeyMessage::simple("edit", access_key).print(json)
}

fn value(flag: &Option<String>) -> &str {
    flag.as_deref().unwrap_or_default()
}

/// urfave/cli `ctx.Duration` on a string flag: Go `time.ParseDuration`, 0 when invalid.
fn flag_duration(flag: &Option<String>) -> std::time::Duration {
    flag.as_deref()
        .and_then(|text| crate::cli::parse_go_duration(text).ok())
        .unwrap_or_default()
}

/// mc `accessKeyEditOpts`.
fn edit_request(flags: &EditFlags) -> Result<UpdateServiceAccountReq> {
    let duration = flag_duration(&flags.expiry_duration);
    let expiry = value(&flags.expiry);
    let policy_path = value(&flags.policy);
    if value(&flags.name).is_empty()
        && expiry.is_empty()
        && duration.is_zero()
        && policy_path.is_empty()
        && value(&flags.secret_key).is_empty()
        && value(&flags.description).is_empty()
    {
        return Err(
            anyhow::Error::new(McError::new("At least one property must be edited"))
                .context("invalid flags"),
        );
    }
    check_expiry_flags(expiry, duration)?;
    Ok(UpdateServiceAccountReq {
        new_name: value(&flags.name).to_string(),
        new_secret_key: value(&flags.secret_key).to_string(),
        new_description: value(&flags.description).to_string(),
        new_policy: read_policy(policy_path)?,
        new_expiration: expiration(expiry, duration)?,
        ..Default::default()
    })
}

fn check_expiry_flags(expiry: &str, duration: std::time::Duration) -> Result<()> {
    if !expiry.is_empty() && !duration.is_zero() {
        return Err(anyhow::Error::new(McError::new(
            "Only one of --expiry or --expiry-duration can be specified",
        ))
        .context("invalid flags"));
    }
    Ok(())
}

/// mc `accessKeyCreateOpts`.
pub(crate) fn create_request(
    flags: &CreateFlags,
    target_user: &str,
) -> Result<AddServiceAccountReq> {
    let mut access_key = value(&flags.access_key).to_string();
    let mut secret_key = value(&flags.secret_key).to_string();
    if access_key.is_empty() || secret_key.is_empty() {
        let (random_access, random_secret) =
            generate_credentials().context("unable to generate randomized access credentials")?;
        if access_key.is_empty() {
            access_key = random_access;
        }
        if secret_key.is_empty() {
            secret_key = random_secret;
        }
    }
    let duration = flag_duration(&flags.expiry_duration);
    let expiry = value(&flags.expiry);
    check_expiry_flags(expiry, duration)?;
    Ok(AddServiceAccountReq {
        target_user: target_user.to_string(),
        access_key,
        secret_key,
        name: value(&flags.name).to_string(),
        description: value(&flags.description).to_string(),
        policy: read_policy(value(&flags.policy))?,
        expiration: expiration(expiry, duration)?,
    })
}

/// mc `commonAccesskeyCreate` for LDAP.
pub(crate) fn ldap_create(
    path: &[&str],
    args: &[String],
    flags: &CreateFlags,
    json: bool,
) -> Result<()> {
    if args.is_empty() || args.len() > 2 {
        show_help_and_exit(path)
    }
    if flags.login {
        return Err(anyhow::Error::new(McError::new(
            "Please use 'mc idp ldap accesskey create-with-login' instead",
        ))
        .context("Deprecated command"));
    }
    let target_user = args.get(1).map(String::as_str).unwrap_or_default();
    let req = create_request(flags, target_user)?;
    let client = admin_client(&args[0])?;
    let creds = runtime()?
        .block_on(admin_idp::add_service_account_ldap(&client, &req))
        .context("Unable to add service account.")?;
    create_message(&req, creds).print(json)
}

fn create_message(req: &AddServiceAccountReq, creds: admin_idp::Credentials) -> AccesskeyMessage {
    AccesskeyMessage {
        op: "create",
        status: "success",
        access_key: creds.access_key,
        secret_key: creds.secret_key,
        // Go `&res.Expiration`: always marshaled, the zero time when unset.
        expiration: Some(
            creds
                .expiration
                .unwrap_or_else(|| crate::s3::admin::GO_ZERO_TIME.to_string()),
        ),
        name: req.name.clone(),
        description: req.description.clone(),
        ..Default::default()
    }
}

/// mc `mainIDPLdapAccesskeyCreateWithLogin`: prompts for LDAP credentials, gets STS
/// credentials for them and creates an access key as that user.
pub(crate) fn create_with_login(
    path: &[&str],
    url: Option<&str>,
    flags: &CreateFlags,
    json: bool,
) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    let Some(url) = url else {
        show_help_and_exit(path)
    };
    if !std::io::stdin().is_terminal() {
        return Err(anyhow::Error::new(McError::new(
            "login flag cannot be used with a non-interactive terminal",
        ))
        .context("unable to read from STDIN"));
    }
    let parsed = url::Url::parse(url)
        .map_err(|err| McError::new(format!("parse \"{url}\": {err}")))
        .context("unable to parse server URL")?;
    print!("Enter LDAP Username: ");
    std::io::stdout().flush()?;
    let mut username = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut username)
        .context("unable to read username")?;
    let username = username.trim_end_matches(['\r', '\n']).to_string();
    print!("Enter LDAP Password: ");
    std::io::stdout().flush()?;
    // rpassword echoes the newline (ECHONL) where mc prints it after Go's `term.ReadPassword`.
    let password = rpassword::read_password().context("unable to read password")?;

    let alias = crate::config::model::AliasConfig {
        url: url.to_string(),
        ..Default::default()
    };
    let anonymous = AdminClient::new(&alias).context("unable to initialize LDAP identity")?;
    let rt = runtime()?;
    let sts = rt
        .block_on(admin_idp::assume_role_with_ldap_identity(
            &anonymous,
            parsed.path(),
            &username,
            &password,
        ))
        .context("unable to create a temporary account from LDAP identity")?;
    let alias = crate::config::model::AliasConfig {
        url: format!(
            "{}://{}",
            parsed.scheme(),
            parsed.host_str().unwrap_or_default()
        ) + &parsed.port().map(|p| format!(":{p}")).unwrap_or_default(),
        access_key: sts.access_key.clone(),
        secret_key: sts.secret_key.clone(),
        session_token: Some(sts.session_token.clone()),
        ..Default::default()
    };
    let client = AdminClient::new(&alias).context("unable to initialize admin connection")?;
    let req = create_request(flags, &sts.access_key)?;
    let creds = rt
        .block_on(admin_idp::add_service_account_ldap(&client, &req))
        .context("unable to add service account")?;
    create_message(&req, creds).print(json)
}

/// mc `checkSTSRevokeSyntax` + `mainAdminAccesskeySTSRevoke` (mc uses the builtin provider
/// endpoint for `idp ldap accesskey sts-revoke` too).
pub(crate) fn sts_revoke(
    path: &[&str],
    args: &[String],
    flags: &StsRevokeFlags,
    json: bool,
) -> Result<()> {
    if args.is_empty() || args.len() > 2 {
        show_help_and_exit(path)
    }
    let user = args.get(1).map(String::as_str).unwrap_or_default();
    let token_type = value(&flags.token_type);
    if !flags.self_ && user.is_empty() {
        return Err(anyhow::Error::new(McError::invalid_argument())
            .context("Must specify user or use --self flag."));
    }
    if flags.self_ && !user.is_empty() {
        return Err(anyhow::Error::new(McError::invalid_argument())
            .context("Cannot specify user with --self flag."));
    }
    if flags.all == !token_type.is_empty() {
        return Err(anyhow::Error::new(McError::new(""))
            .context("Exactly one of --all or --token-type must be specified."));
    }
    let client = admin_client(&args[0])?;
    let req = RevokeTokensReq {
        user: user.to_string(),
        token_revoke_type: token_type.to_string(),
        full_revoke: flags.all,
    };
    runtime()?
        .block_on(admin_idp::revoke_tokens(&client, "builtin", &req))
        .with_context(|| super::fatal_msg("Unable to revoke tokens for %s", &[user]))?;

    #[derive(Serialize)]
    struct StsRevokeMessage<'a> {
        status: &'static str,
        user: &'a str,
        #[serde(rename = "tokenRevokeType", skip_serializing_if = "str::is_empty")]
        token_revoke_type: &'a str,
    }
    let who = if user.is_empty() {
        "authenticated user".to_string()
    } else {
        format!("user {user}")
    };
    let text = if token_type.is_empty() {
        format!("Successfully revoked all STS accounts for {who}")
    } else {
        format!("Successfully revoked all STS accounts of type {token_type} for {who}")
    };
    let message = StsRevokeMessage {
        status: "success",
        user,
        token_revoke_type: token_type,
    };
    print_msg(json, &message, &text, 1)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// mc `generateCredentials`: 20 characters from `[0-9A-Z]` and a 40 character base64 secret
/// (`/` replaced by `+`).
fn generate_credentials() -> Result<(String, String)> {
    use base64::Engine;
    const TABLE: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut key = [0u8; 20];
    let mut secret = [0u8; 40];
    aws_lc_rs::rand::fill(&mut key).map_err(|_| anyhow::anyhow!("random generator failed"))?;
    aws_lc_rs::rand::fill(&mut secret).map_err(|_| anyhow::anyhow!("random generator failed"))?;
    let access: String = key
        .iter()
        .map(|b| TABLE[(*b % TABLE.len() as u8) as usize] as char)
        .collect();
    let secret = base64::engine::general_purpose::STANDARD.encode(secret)[..40].replace('/', "+");
    Ok((access, secret))
}

/// Reads and checks a policy file like mc (`policy.ParseConfig`, no statements rejected).
fn read_policy(path: &str) -> Result<Option<serde_json::Value>> {
    if path.is_empty() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)
        .map_err(|err| go_read_error(&err, path))
        .context("unable to read the policy document")?;
    let policy: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|err| go_json_error(&err, &bytes))
        .context("unable to parse the policy document")?;
    let empty = policy
        .get("Statement")
        .and_then(serde_json::Value::as_array)
        .is_none_or(Vec::is_empty);
    if empty {
        return Err(anyhow::Error::new(McError::invalid_argument())
            .context("empty policies are not allowed"));
    }
    Ok(Some(policy))
}

/// Go `os.ReadFile` error (`*fs.PathError`: `open PATH: no such file or directory`,
/// marshaled as `{"Op","Path","Err":errno}`).
fn go_read_error(err: &std::io::Error, path: &str) -> McError {
    let (op, reason) = match err.kind() {
        std::io::ErrorKind::NotFound => ("open", "no such file or directory".to_string()),
        std::io::ErrorKind::PermissionDenied => ("open", "permission denied".to_string()),
        std::io::ErrorKind::IsADirectory => ("read", "is a directory".to_string()),
        _ => ("open", err.to_string()),
    };
    McError::with_detail(
        format!("{op} {path}: {reason}"),
        path_error_detail(op, path, err.raw_os_error().unwrap_or_default()),
    )
}

fn path_error_detail(op: &str, path: &str, errno: i32) -> crate::error::Detail {
    crate::detail![("Op", op), ("Path", path), ("Err", errno)]
}

/// Go `encoding/json` syntax error text for the common cases (mc wraps it in minio/pkg's
/// policy error, marshaled as `{}`); other decode errors keep serde's text.
fn go_json_error(err: &serde_json::Error, bytes: &[u8]) -> McError {
    if err.is_eof() {
        return McError::new("unexpected end of JSON input");
    }
    match bytes.iter().find(|b| !b.is_ascii_whitespace()) {
        Some(&b)
            if !matches!(
                b,
                b'{' | b'[' | b'"' | b't' | b'f' | b'n' | b'-' | b'0'..=b'9'
            ) =>
        {
            McError::new(format!(
                "invalid character '{}' looking for beginning of value",
                b as char
            ))
        }
        _ => McError::new(err.to_string()),
    }
}

/// `--expiry` (mc `supportedTimeFormats`, local time taken as UTC) or `--expiry-duration`
/// as an RFC 3339 time for the request.
fn expiration(expiry: &str, duration: std::time::Duration) -> Result<Option<String>> {
    use aws_smithy_types::DateTime;
    use aws_smithy_types::date_time::Format;
    if !expiry.is_empty() {
        let Some(time) = parse_expiry(expiry) else {
            return Err(anyhow::Error::new(McError::new(format!(
                "invalid expiry date format '{expiry}'"
            )))
            .context("unable to parse the expiry argument"));
        };
        return Ok(Some(time));
    }
    if !duration.is_zero() {
        let time = std::time::SystemTime::now() + duration;
        return Ok(Some(DateTime::from(time).fmt(Format::DateTime)?));
    }
    Ok(None)
}

/// Parses `2006-01-02`, `2006-01-02T15:04`, `2006-01-02T15:04:05` (local time, like Go's
/// `time.ParseInLocation(..., Local)`) or RFC 3339; returns the UTC RFC 3339 time.
fn parse_expiry(text: &str) -> Option<String> {
    use aws_smithy_types::DateTime;
    use aws_smithy_types::date_time::Format;
    let short =
        regex::Regex::new(r"^(\d{4})-(\d{2})-(\d{2})(?:T(\d{2}):(\d{2})(?::(\d{2}))?)?$").ok()?;
    let secs = match short.captures(text) {
        Some(caps) => {
            let num = |i: usize| -> i64 {
                caps.get(i)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0)
            };
            let (year, month, day) = (num(1), num(2), num(3));
            let (hour, minute, second) = (num(4), num(5), num(6));
            if day < 1
                || day > days_in_month(year, month)?
                || hour > 23
                || minute > 59
                || second > 59
            {
                return None;
            }
            local_epoch(year, month, day, hour, minute, second)?
        }
        None => DateTime::from_str(text, Format::DateTimeWithOffset)
            .ok()?
            .secs(),
    };
    DateTime::from_secs(secs).fmt(Format::DateTime).ok()
}

/// Seconds since the epoch of a local wall-clock time (`mktime`).
#[cfg(unix)]
fn local_epoch(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> Option<i64> {
    // SAFETY: `tm` is a plain C struct; all-zero is a valid value and mktime only reads and
    // normalizes it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = (year - 1900) as libc::c_int;
    tm.tm_mon = (month - 1) as libc::c_int;
    tm.tm_mday = day as libc::c_int;
    tm.tm_hour = hour as libc::c_int;
    tm.tm_min = minute as libc::c_int;
    tm.tm_sec = second as libc::c_int;
    tm.tm_isdst = -1;
    // SAFETY: `tm` is a valid, exclusively borrowed `libc::tm`.
    let time = unsafe { libc::mktime(&mut tm) };
    (time != -1).then_some(time as i64)
}

/// Seconds since the epoch of a UTC wall-clock time (no local time zone support).
#[cfg(not(unix))]
fn local_epoch(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> Option<i64> {
    use aws_smithy_types::DateTime;
    use aws_smithy_types::date_time::Format;
    let text = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z");
    Some(DateTime::from_str(&text, Format::DateTime).ok()?.secs())
}

fn days_in_month(year: i64, month: i64) -> Option<i64> {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    Some(match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    })
}

/// go-humanize `Time(then)`: `3 hours from now`, `2 days ago`, `now`.
pub(crate) fn humanize_time(time: &str) -> String {
    let Some(then) = admin_idp::unix_nanos(time) else {
        return time.to_string();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i128)
        .unwrap_or_default();
    rel_time(then - now)
}

/// go-humanize `RelTime(then, now, "ago", "from now")` for `then - now` nanoseconds.
fn rel_time(delta: i128) -> String {
    const SECOND: i128 = 1_000_000_000;
    const MINUTE: i128 = 60 * SECOND;
    const HOUR: i128 = 60 * MINUTE;
    const DAY: i128 = 24 * HOUR;
    const WEEK: i128 = 7 * DAY;
    const MONTH: i128 = 30 * DAY;
    const YEAR: i128 = 12 * MONTH;
    let label = if delta > 0 { "from now" } else { "ago" };
    let diff = delta.abs();
    let (format, divisor): (&str, i128) = match diff {
        d if d < SECOND => return "now".to_string(),
        d if d < 2 * SECOND => ("1 second", 0),
        d if d < MINUTE => ("{} seconds", SECOND),
        d if d < 2 * MINUTE => ("1 minute", 0),
        d if d < HOUR => ("{} minutes", MINUTE),
        d if d < 2 * HOUR => ("1 hour", 0),
        d if d < DAY => ("{} hours", HOUR),
        d if d < 2 * DAY => ("1 day", 0),
        d if d < WEEK => ("{} days", DAY),
        d if d < 2 * WEEK => ("1 week", 0),
        d if d < MONTH => ("{} weeks", WEEK),
        d if d < 2 * MONTH => ("1 month", 0),
        d if d < YEAR => ("{} months", MONTH),
        d if d < 18 * MONTH => ("1 year", 0),
        d if d < 2 * YEAR => ("2 years", 0),
        d if d < 37 * YEAR => ("{} years", YEAR),
        _ => ("a long while", 0),
    };
    let text = if divisor > 0 {
        format.replace("{}", &(diff / divisor).to_string())
    } else {
        format.to_string()
    };
    format!("{text} {label}")
}

/// Go `time.Time.String()` of an RFC 3339 time: `2026-09-27 19:27:08 +0000 UTC`.
pub(crate) fn go_time_string(time: &str) -> String {
    let Some((date, rest)) = time.split_once('T') else {
        return time.to_string();
    };
    let zone_start = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(zone_start);
    let (hms, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let fraction = fraction.trim_end_matches('0');
    let fraction = if fraction.is_empty() {
        String::new()
    } else {
        format!(".{fraction}")
    };
    let zone = match zone {
        "" | "Z" | "z" => "+0000 UTC".to_string(),
        offset => {
            let offset = offset.replace(':', "");
            format!("{offset} {offset}")
        }
    };
    format!("{date} {hms}{fraction} {zone}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_time_matches_go_humanize() {
        let s = |secs: i128| secs * 1_000_000_000;
        assert_eq!(rel_time(0), "now");
        assert_eq!(rel_time(s(1)), "1 second from now");
        assert_eq!(rel_time(-s(30)), "30 seconds ago");
        assert_eq!(rel_time(s(3599)), "59 minutes from now");
        assert_eq!(rel_time(s(86_400) - 1), "23 hours from now");
        assert_eq!(rel_time(s(2 * 86_400) - 1), "1 day from now");
        assert_eq!(rel_time(s(3 * 86_400)), "3 days from now");
        assert_eq!(rel_time(-s(400 * 86_400)), "1 year ago");
        assert_eq!(rel_time(s(40 * 360 * 86_400)), "a long while from now");
    }

    #[test]
    fn go_time_string_formats() {
        assert_eq!(
            go_time_string("2026-09-27T19:27:08Z"),
            "2026-09-27 19:27:08 +0000 UTC"
        );
        assert_eq!(
            go_time_string("2026-09-27T19:27:08.120Z"),
            "2026-09-27 19:27:08.12 +0000 UTC"
        );
        assert_eq!(
            go_time_string("2026-09-27T19:27:08+02:00"),
            "2026-09-27 19:27:08 +0200 +0200"
        );
    }

    #[test]
    fn expiry_formats_like_mc() {
        let secs = |text: &str| admin_idp::unix_seconds(&parse_expiry(text).unwrap()).unwrap();
        assert_eq!(
            secs("2030-01-02T03:04") - secs("2030-01-02"),
            3 * 3600 + 4 * 60
        );
        assert_eq!(secs("2030-01-02T03:04:05") - secs("2030-01-02T03:04"), 5);
        let local = secs("2030-01-02");
        assert!((local - 1_893_542_400).abs() <= 14 * 3600, "{local}");
        assert_eq!(
            parse_expiry("2030-01-02T03:04:05+01:00").as_deref(),
            Some("2030-01-02T02:04:05Z")
        );
        assert_eq!(parse_expiry("2027-13-01"), None);
        assert_eq!(parse_expiry("2027-02-30"), None);
        assert_eq!(parse_expiry("tomorrow"), None);
    }

    #[test]
    fn list_flags_validate_like_mc() {
        let args = vec!["a:cfg".to_string()];
        let (alias, tentative, users, opts) =
            list_options(&[], &args, &ListFlags::default()).unwrap();
        assert_eq!(alias, "a");
        assert!(tentative && users.is_empty() && opts.all);
        assert_eq!(opts.config_name, "cfg");
        assert_eq!(opts.list_type, "all");
        let flags = ListFlags {
            users_only: true,
            temp_only: true,
            ..Default::default()
        };
        let err = list_options(&[], &args, &flags).unwrap_err();
        assert_eq!(err.to_string(), "Invalid flags.");
        let flags = ListFlags {
            self_: true,
            ..Default::default()
        };
        let (_, tentative, _, opts) = list_options(&[], &args, &flags).unwrap();
        assert!(!tentative && !opts.all);
    }

    #[test]
    fn credentials_have_mc_shape() {
        let (access, secret) = generate_credentials().unwrap();
        assert_eq!(access.len(), 20);
        assert!(
            access
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        );
        assert_eq!(secret.len(), 40);
        assert!(!secret.contains('/'));
    }

    #[test]
    fn compact_json_keeps_strings_and_order() {
        assert_eq!(
            compact_json("{ \"b\" : [1, 2],\n \"a\": \"x y\\\" z\" }"),
            "{\"b\":[1,2],\"a\":\"x y\\\" z\"}"
        );
    }

    #[test]
    fn info_message_json_matches_mc() {
        let res: InfoAccessKeyResp = serde_json::from_str(
            r#"{"parentUser":"p","accountStatus":"on","impliedPolicy":true,"policy":"{\"Version\":\"2012-10-17\", \"Statement\":null}","userProvider":"ldap","ldapSpecificInfo":{"username":"u"},"expiration":"1970-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let message = info_message("k", res);
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"status":"success","accessKey":"k","parentUser":"p","accountStatus":"on","impliedPolicy":true,"policy":{"Version":"2012-10-17","Statement":null},"provider":"ldap","providerInfo":{"username":"u"}}"#
        );
        assert!(
            message
                .text()
                .ends_with("Provider Specific Info:\nUsername: u")
        );
        assert!(message.text().contains("Expiration: NONE\n"));
    }

    #[test]
    fn go_errors_for_policy_files() {
        let err = std::io::Error::from_raw_os_error(2);
        let mc = go_read_error(&err, "/x");
        assert_eq!(mc.message, "open /x: no such file or directory");
        assert_eq!(
            serde_json::to_string(&mc.detail).unwrap(),
            r#"{"Op":"open","Path":"/x","Err":2}"#
        );
        let err = serde_json::from_slice::<serde_json::Value>(b" xx").unwrap_err();
        let mc = go_json_error(&err, b" xx");
        assert_eq!(
            mc.message,
            "invalid character 'x' looking for beginning of value"
        );
        assert!(mc.detail.is_empty());
    }
}
