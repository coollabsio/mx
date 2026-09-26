//! `mx admin user` (mc `admin user`): builtin IAM users, their service accounts and STS
//! accounts (MinIO admin API, `crate::s3::admin_iam`).
//!
//! Also holds the helpers shared by the IAM commands (`group`, `policy`, `accesskey`).

use crate::commands::runtime;
use crate::config::ConfigStore;
use crate::error::McError;
use crate::output;
use crate::s3::admin::{self, AdminClient};
use crate::s3::admin_iam::{self as iam, GoJson, GoTime, IamPolicy};
use anyhow::{Context, Result};
use clap::{Args, CommandFactory, Subcommand};
use serde::Serialize;
use std::io::{BufRead, IsTerminal, Write};

#[derive(Debug, Args)]
pub struct UserArgs {
    #[command(subcommand)]
    pub command: UserCommand,
}

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    #[command(name = "add", about = "add a new user")]
    Add(UserAddArgs),
    #[command(name = "disable", about = "disable user")]
    Disable(UserTargetArgs),
    #[command(name = "enable", about = "enable user")]
    Enable(UserTargetArgs),
    #[command(name = "remove", visible_alias = "rm", about = "remove user")]
    Remove(UserTargetArgs),
    #[command(name = "list", visible_alias = "ls", about = "list all users")]
    List(UserListArgs),
    #[command(name = "info", about = "display info of a user")]
    Info(UserTargetArgs),
    #[command(name = "policy", about = "export user policies in JSON format")]
    Policy(UserTargetArgs),
    #[command(name = "svcacct", about = "manage service accounts")]
    Svcacct(UserSvcacctArgs),
    #[command(name = "sts", about = "manage STS accounts")]
    Sts(UserStsArgs),
}

#[derive(Debug, Args)]
pub struct UserAddArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "ACCESSKEY")]
    pub accesskey: Option<String>,
    #[arg(value_name = "SECRETKEY")]
    pub secretkey: Option<String>,
    /// Extra arguments: mc shows the command help.
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct UserTargetArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "USERNAME")]
    pub username: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct UserListArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct UserSvcacctArgs {
    #[command(subcommand)]
    pub command: UserSvcacctCommand,
}

#[derive(Debug, Subcommand)]
pub enum UserSvcacctCommand {
    #[command(name = "add", about = "add a new service account")]
    Add(UserSvcacctAddArgs),
    #[command(name = "list", visible_alias = "ls", about = "list services accounts")]
    List(UserSvcacctListArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "remove a service account"
    )]
    Remove(UserSvcacctTargetArgs),
    #[command(name = "info", about = "display service account info")]
    Info(UserSvcacctInfoArgs),
    #[command(
        name = "edit",
        visible_alias = "set",
        about = "edit an existing service account"
    )]
    Edit(UserSvcacctEditArgs),
    #[command(name = "enable", about = "enable a service account")]
    Enable(UserSvcacctTargetArgs),
    #[command(name = "disable", about = "disable a service account")]
    Disable(UserSvcacctTargetArgs),
}

#[derive(Debug, Args)]
pub struct UserSvcacctAddArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "ACCOUNT")]
    pub account: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
    #[arg(
        long = "access-key",
        value_name = "VALUE",
        help = "set an access key for the service account"
    )]
    pub access_key: Option<String>,
    #[arg(
        long = "secret-key",
        value_name = "VALUE",
        help = "set a secret key for the service account"
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
        help = "friendly name for the service account"
    )]
    pub name: Option<String>,
    #[arg(
        long = "description",
        value_name = "VALUE",
        help = "description for the service account"
    )]
    pub description: Option<String>,
    #[arg(
        long = "expiry",
        value_name = "VALUE",
        help = "time of expiration for the service account"
    )]
    pub expiry: Option<String>,
    #[arg(
        long = "comment",
        hide = true,
        value_name = "VALUE",
        help = "description for the service account (DEPRECATED: use --description instead)"
    )]
    pub comment: Option<String>,
}

#[derive(Debug, Args)]
pub struct UserSvcacctListArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "TARGET-ACCOUNT")]
    pub target_account: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct UserSvcacctTargetArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "SERVICE-ACCOUNT")]
    pub service_account: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct UserSvcacctInfoArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "SERVICE-ACCOUNT")]
    pub service_account: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
    #[arg(long = "policy", help = "print policy in JSON format")]
    pub policy: bool,
}

#[derive(Debug, Args)]
pub struct UserSvcacctEditArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "SERVICE-ACCOUNT")]
    pub service_account: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
    #[arg(
        long = "secret-key",
        value_name = "VALUE",
        help = "set a secret key for the service account"
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
        help = "name for the service account"
    )]
    pub name: Option<String>,
    #[arg(
        long = "description",
        value_name = "VALUE",
        help = "description for the service account"
    )]
    pub description: Option<String>,
    #[arg(
        long = "expiry",
        value_name = "VALUE",
        help = "time of expiration for the service account"
    )]
    pub expiry: Option<String>,
}

#[derive(Debug, Args)]
pub struct UserStsArgs {
    #[command(subcommand)]
    pub command: UserStsCommand,
}

#[derive(Debug, Subcommand)]
pub enum UserStsCommand {
    #[command(name = "info", about = "display temporary account info")]
    Info(UserStsInfoArgs),
}

#[derive(Debug, Args)]
pub struct UserStsInfoArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "STS-ACCOUNT")]
    pub sts_account: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
    #[arg(long = "policy", help = "print policy in JSON format")]
    pub policy: bool,
}

// ---------------------------------------------------------------------------
// Helpers shared by the IAM commands
// ---------------------------------------------------------------------------

/// mc `newAdminClient(aliasedURL)` + `fatalIf(err, "Unable to initialize admin connection.")`.
pub(crate) fn admin_client(target: &str) -> Result<AdminClient> {
    admin::admin_client_for(&ConfigStore::load_or_create()?, target)
}

/// [`admin_client`] with another fatal message (mc varies the text per command).
pub(crate) fn admin_client_with(target: &str, message: &'static str) -> Result<AdminClient> {
    admin_client(target).map_err(|err| {
        let cause = match crate::error::mc_error(&err) {
            Some(cause) => anyhow::Error::new(cause.clone()),
            None => anyhow::anyhow!("{}", output::split_error(&err).1),
        };
        cause.context(message)
    })
}

pub(crate) fn block_on<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    runtime()?.block_on(future)
}

/// mc `printMsg`: the JSON document with `--json`, else `text` (one trailing newline trimmed).
pub(crate) fn print_msg<T: Serialize>(json: bool, message: &T, text: &str) -> Result<()> {
    if json {
        return output::print_json(message);
    }
    println!("{}", text.strip_suffix('\n').unwrap_or(text));
    Ok(())
}

/// mc `showCommandHelpAndExit(ctx, 1)`: the command help on stdout, exit status 1.
pub(crate) fn show_help(path: &[&str]) -> ! {
    let mut cmd = crate::cli::Cli::command();
    for name in path {
        match cmd.find_subcommand(name) {
            Some(sub) => cmd = sub.clone(),
            None => break,
        }
    }
    let _ = cmd.print_help();
    std::process::exit(1);
}

/// A fatal message with a `%s` verb: mc formats it for text, but its JSON error keeps the
/// format string verbatim.
pub(crate) fn fmt_message(template: &str, arg: &str) -> String {
    if crate::globals::json() {
        template.to_string()
    } else {
        template.replacen("%s", arg, 1)
    }
}

/// mc `errDummy()`: a fatal error with an empty cause.
pub(crate) fn dummy_error() -> McError {
    McError::new("")
}

/// Go `*fs.PathError` (`open /x: no such file or directory`, `{"Op","Path","Err":errno}`).
pub(crate) fn path_error(op: &str, path: &str, err: &std::io::Error) -> McError {
    let text = err.to_string();
    let text = text.split(" (os error").next().unwrap_or_default();
    let mut chars = text.chars();
    let text = chars
        .next()
        .map(|first| first.to_lowercase().collect::<String>() + chars.as_str())
        .unwrap_or_default();
    McError::with_detail(
        format!("{op} {path}: {text}"),
        crate::detail![
            ("Op", op),
            ("Path", path),
            ("Err", err.raw_os_error().unwrap_or_default()),
        ],
    )
}

/// Go `os.ReadFile(path)` with its error values.
pub(crate) fn read_file(path: &str) -> std::result::Result<Vec<u8>, McError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|err| path_error("open", path, &err))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|err| path_error("read", path, &err))?;
    Ok(data)
}

/// mc `--expiry` parsing (`supportedTimeFormats`); `cause` is mc's error text on failure.
pub(crate) fn parse_expiry(value: &str, cause: String, message: &'static str) -> Result<String> {
    match iam::parse_expiry(value) {
        Some(time) => Ok(time.rfc3339_nano()),
        None => Err(anyhow::Error::new(McError::new(cause)).context(message)),
    }
}

/// A local policy file checked like mc (`policy.ParseConfig`, no empty policies).
pub(crate) fn read_policy_file(path: &str) -> Result<GoJson> {
    let data = read_file(path).context("unable to read the policy document")?;
    let policy = IamPolicy::parse_config(&data).context("unable to parse the policy document")?;
    if policy.is_empty() {
        return Err(anyhow::Error::new(McError::invalid_argument())
            .context("empty policies are not allowed"));
    }
    Ok(GoJson::parse(&data)?)
}

/// Pretty-prints a policy like mc's `json.Encoder` with `SetIndent("", " ")`.
fn print_policy_document(policy: &str) -> Result<()> {
    let policy = IamPolicy::parse_config(policy.as_bytes()).context("Unable to parse policy.")?;
    println!("{}", output::json_indent(&policy)?);
    Ok(())
}

/// mc `PrettyTable.buildRow`: `%-N.Ns` columns (cut with `...`) joined by `separator`.
pub(crate) fn pretty_row(separator: &str, widths: &[usize], contents: &[&str]) -> String {
    let columns = widths.len().min(contents.len());
    (0..columns)
        .map(|i| {
            let width = widths[i];
            let content = contents[i];
            let cell = if content.len() > width {
                format!("{}...", &content[..content.floor_char_boundary(width - 3)])
            } else {
                content.to_string()
            };
            let cell: String = cell.chars().take(width).collect();
            let pad = width - cell.chars().count();
            let sep = if i + 1 < columns { separator } else { "" };
            format!("{cell}{}{sep}", " ".repeat(pad))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// admin user add|disable|enable|remove|list|info|policy
// ---------------------------------------------------------------------------

/// mc `userGroup`.
#[derive(Debug, Serialize)]
struct UserGroup {
    #[serde(skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    policies: Vec<String>,
}

/// mc `userMessage`.
#[derive(Debug, Default, Serialize)]
struct UserMessage {
    status: &'static str,
    #[serde(rename = "accessKey", skip_serializing_if = "String::is_empty")]
    access_key: String,
    #[serde(rename = "secretKey", skip_serializing_if = "String::is_empty")]
    secret_key: String,
    #[serde(rename = "policyName", skip_serializing_if = "String::is_empty")]
    policy_name: String,
    #[serde(rename = "userStatus", skip_serializing_if = "String::is_empty")]
    user_status: String,
    #[serde(rename = "memberOf", skip_serializing_if = "Vec::is_empty")]
    member_of: Vec<UserGroup>,
    #[serde(skip_serializing_if = "String::is_empty")]
    authentication: String,
}

impl UserMessage {
    fn new(access_key: &str) -> Self {
        Self {
            status: "success",
            access_key: access_key.to_string(),
            ..Default::default()
        }
    }

    fn list_row(&self) -> String {
        pretty_row(
            "  ",
            &[9, 20, 20],
            &[&self.user_status, &self.access_key, &self.policy_name],
        )
    }

    fn info_text(&self) -> String {
        let groups: Vec<&str> = self.member_of.iter().map(|g| g.name.as_str()).collect();
        let mut lines = vec![
            format!("AccessKey: {}", self.access_key),
            format!("Status: {}", self.user_status),
            format!("PolicyName: {}", self.policy_name),
            format!("MemberOf: [{}]", groups.join(" ")),
        ];
        if !self.authentication.is_empty() {
            lines.push(format!("Authentication: {}", self.authentication));
        }
        lines.join("\n")
    }
}

pub fn run(args: UserArgs, json: bool) -> Result<()> {
    match args.command {
        UserCommand::Add(args) => add(args, json),
        UserCommand::Disable(args) => set_status(args, json, false),
        UserCommand::Enable(args) => set_status(args, json, true),
        UserCommand::Remove(args) => remove(args, json),
        UserCommand::List(args) => list(args, json),
        UserCommand::Info(args) => info(args, json),
        UserCommand::Policy(args) => policy(args),
        UserCommand::Svcacct(args) => svcacct(args, json),
        UserCommand::Sts(args) => sts(args, json),
    }
}

/// Go `bufio.Reader.ReadLine`: one line without `\n` / `\r\n` (empty at EOF).
fn read_line(reader: &mut dyn BufRead) -> Result<String> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let line = line.strip_suffix('\n').unwrap_or(&line);
    Ok(line.strip_suffix('\r').unwrap_or(line).to_string())
}

/// mc `fetchUserKeys`: missing keys are prompted for on a terminal, else read from stdin.
fn fetch_user_keys(args: &UserAddArgs) -> Result<(String, String)> {
    let stdin = std::io::stdin();
    let terminal = stdin.is_terminal();
    let mut reader = stdin.lock();
    let access_key = match &args.accesskey {
        Some(key) => key.clone(),
        None => {
            if terminal {
                print!("Enter Access Key: ");
                std::io::stdout().flush()?;
            }
            read_line(&mut reader)?
        }
    };
    let secret_key = match &args.secretkey {
        Some(key) => key.clone(),
        None if terminal => {
            print!("Enter Secret Key: ");
            std::io::stdout().flush()?;
            let secret = rpassword::read_password().unwrap_or_default();
            println!();
            secret
        }
        None => read_line(&mut reader)?,
    };
    Ok((access_key, secret_key))
}

fn add(args: UserAddArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "add"]);
    }
    let (access_key, secret_key) = fetch_user_keys(&args)?;
    let client = admin_client(&args.target)?;
    block_on(iam::add_user(&client, &access_key, &secret_key)).context("Unable to add new user")?;
    let message = UserMessage {
        secret_key,
        user_status: "enabled".into(),
        ..UserMessage::new(&access_key)
    };
    print_msg(
        json,
        &message,
        &format!("Added user `{access_key}` successfully."),
    )
}

fn set_status(args: UserTargetArgs, json: bool, enable: bool) -> Result<()> {
    let (name, status, message) = if enable {
        ("enable", "enabled", "Unable to enable user")
    } else {
        ("disable", "disabled", "Unable to disable user")
    };
    if !args.extra.is_empty() {
        show_help(&["admin", "user", name]);
    }
    let client = admin_client(&args.target)?;
    block_on(iam::set_user_status(&client, &args.username, status)).context(message)?;
    let text = if enable {
        format!("Enabled user `{}` successfully.", args.username)
    } else {
        format!("Disabled user `{}` successfully.", args.username)
    };
    print_msg(json, &UserMessage::new(&args.username), &text)
}

fn remove(args: UserTargetArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "remove"]);
    }
    let client = admin_client(&args.target)?;
    block_on(iam::remove_user(&client, &args.username))
        .with_context(|| fmt_message("Unable to remove %s", &args.username))?;
    print_msg(
        json,
        &UserMessage::new(&args.username),
        &format!("Removed user `{}` successfully.", args.username),
    )
}

/// `memberOf` groups with their policies (mc fetches every group description).
fn member_of(client: &AdminClient, groups: &[String]) -> Result<Vec<UserGroup>> {
    groups
        .iter()
        .map(|group| {
            let desc = block_on(iam::group_description(client, group))
                .context("Unable to fetch group info")?;
            let policies = if desc.policy.is_empty() {
                Vec::new()
            } else {
                desc.policy.split(',').map(str::to_string).collect()
            };
            Ok(UserGroup {
                name: desc.name,
                policies,
            })
        })
        .collect()
}

fn list(args: UserListArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "list"]);
    }
    let client = admin_client(&args.target)?;
    let users = block_on(iam::list_users(&client)).context("Unable to list user")?;
    for (access_key, user) in users {
        let message = UserMessage {
            policy_name: user.policy_name,
            member_of: member_of(&client, &user.member_of)?,
            user_status: user.status,
            ..UserMessage::new(&access_key)
        };
        print_msg(json, &message, &message.list_row())?;
    }
    Ok(())
}

fn info(args: UserTargetArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "info"]);
    }
    let client = admin_client(&args.target)?;
    let user =
        block_on(iam::user_info(&client, &args.username)).context("Unable to get user info")?;
    // mc `authInfoToUserMessage`.
    let authentication = user
        .auth_info
        .as_ref()
        .map(|auth| {
            let server = if auth.auth_type == "builtin" {
                String::new()
            } else {
                format!("/{}", auth.auth_server)
            };
            format!("{}{server} ({})", auth.auth_type, auth.auth_server_user_id)
        })
        .unwrap_or_default();
    let message = UserMessage {
        policy_name: user.policy_name,
        user_status: user.status,
        member_of: member_of(&client, &user.member_of)?,
        authentication,
        ..UserMessage::new(&args.username)
    };
    print_msg(json, &message, &message.info_text())
}

/// `admin user policy`: the user's policies merged into one document (always JSON).
fn policy(args: UserTargetArgs) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "policy"]);
    }
    let client = admin_client(&args.target)?;
    let user =
        block_on(iam::user_info(&client, &args.username)).context("Unable to get user info")?;
    if user.policy_name.is_empty() {
        return Err(anyhow::Error::new(McError::new(format!(
            "policy not found for user {}",
            args.username
        )))
        .context("Unable to fetch user policy document"));
    }
    let mut policies = Vec::new();
    for name in user.policy_name.split(',').filter(|name| !name.is_empty()) {
        let info = block_on(iam::policy_info(&client, name))
            .with_context(|| format!("Unable to fetch user policy document for policy {name}"))?;
        let policy =
            IamPolicy::from_json(&info.policy, false).context("Unable to unmarshal policy")?;
        policies.push(policy);
    }
    println!(
        "{}",
        output::format_json(&IamPolicy::merge(&policies), true)?
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// admin user svcacct / sts
// ---------------------------------------------------------------------------

/// mc `acctMessage`.
#[derive(Debug, Default, Serialize)]
struct AcctMessage {
    status: &'static str,
    #[serde(rename = "accessKey", skip_serializing_if = "String::is_empty")]
    access_key: String,
    #[serde(rename = "secretKey", skip_serializing_if = "String::is_empty")]
    secret_key: String,
    #[serde(rename = "parentUser", skip_serializing_if = "String::is_empty")]
    parent_user: String,
    #[serde(rename = "impliedPolicy", skip_serializing_if = "std::ops::Not::not")]
    implied_policy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<GoJson>,
    #[serde(skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(rename = "accountStatus", skip_serializing_if = "String::is_empty")]
    account_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expiration: Option<String>,
}

impl AcctMessage {
    fn new(access_key: &str) -> Self {
        Self {
            status: "success",
            access_key: access_key.to_string(),
            ..Default::default()
        }
    }

    /// mc `acctMessage.String()` for `svcAccOpInfo` / `stsAccOpInfo`.
    fn info_text(&self) -> String {
        let policy = if self.implied_policy {
            "implied"
        } else {
            "embedded"
        };
        let expiration = match self.expiration.as_deref().and_then(GoTime::parse) {
            Some(time) => time.humanize(),
            None => "no-expiry".to_string(),
        };
        [
            format!("AccessKey: {}", self.access_key),
            format!("ParentUser: {}", self.parent_user),
            format!("Status: {}", self.account_status),
            format!("Name: {}", self.name),
            format!("Description: {}", self.description),
            format!("Policy: {policy}"),
            format!("Expiration: {expiration}"),
        ]
        .join("\n")
    }
}

/// A set expiration (not Go's zero time nor mc's `timeSentinel`).
fn set_expiration(expiration: Option<&str>) -> Option<GoTime> {
    expiration
        .and_then(GoTime::parse)
        .filter(|time| !time.is_zero() && !time.is_sentinel())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn svcacct(args: UserSvcacctArgs, json: bool) -> Result<()> {
    match args.command {
        UserSvcacctCommand::Add(args) => svcacct_add(args, json),
        UserSvcacctCommand::List(args) => svcacct_list(args, json),
        UserSvcacctCommand::Remove(args) => svcacct_remove(args, json),
        UserSvcacctCommand::Info(args) => svcacct_info(args, json),
        UserSvcacctCommand::Edit(args) => svcacct_edit(args, json),
        UserSvcacctCommand::Enable(args) => svcacct_status(args, json, true),
        UserSvcacctCommand::Disable(args) => svcacct_status(args, json, false),
    }
}

fn svcacct_add(args: UserSvcacctAddArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "svcacct", "add"]);
    }
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
    let client = admin_client(&args.alias)?;
    let mut req = iam::AddServiceAccountReq {
        target_user: args.account,
        access_key,
        secret_key,
        name: args.name.unwrap_or_default(),
        description: non_empty(args.description)
            .or(args.comment)
            .unwrap_or_default(),
        ..Default::default()
    };
    if let Some(path) = non_empty(args.policy) {
        req.policy = Some(read_policy_file(&path)?);
    }
    if let Some(expiry) = non_empty(args.expiry) {
        req.expiration = Some(parse_expiry(
            &expiry,
            "expiry argument is not matching any of the supported patterns".into(),
            "unable to parse the expiry argument",
        )?);
    }
    let creds = block_on(iam::add_service_account(&client, &req))
        .context("Unable to add a new service account.")?;
    let expiration = creds
        .expiration
        .unwrap_or_else(|| iam::ZERO_TIME.to_string());
    let expiry_text = set_expiration(Some(&expiration))
        .map(|time| time.go_string())
        .unwrap_or_else(|| "no-expiry".to_string());
    let text = format!(
        "Access Key: {}\nSecret Key: {}\nExpiration: {expiry_text}",
        creds.access_key, creds.secret_key
    );
    let message = AcctMessage {
        secret_key: creds.secret_key,
        expiration: Some(expiration),
        account_status: "enabled".into(),
        ..AcctMessage::new(&creds.access_key)
    };
    print_msg(json, &message, &text)
}

fn svcacct_list(args: UserSvcacctListArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "svcacct", "list"]);
    }
    let client = admin_client(&args.alias)?;
    let accounts = block_on(iam::list_service_accounts(&client, &args.target_account))
        .context("Unable to list service accounts")?;
    if accounts.is_empty() {
        if !json {
            println!("No service accounts found");
        }
        return Ok(());
    }
    let widths = [20, 29];
    if !json {
        println!(
            "{}",
            pretty_row(" | ", &widths, &["   Access Key", "Expiry"])
        );
    }
    for account in accounts {
        let expiration = account
            .expiration
            .filter(|exp| !GoTime::parse(exp).is_some_and(|time| time.is_sentinel()));
        let expiry_text = expiration
            .as_deref()
            .and_then(GoTime::parse)
            .filter(|time| !time.is_zero())
            .map(|time| time.go_string())
            .unwrap_or_else(|| "no-expiry".to_string());
        let text = pretty_row(" | ", &widths, &[&account.access_key, &expiry_text]);
        let message = AcctMessage {
            expiration,
            ..AcctMessage::new(&account.access_key)
        };
        print_msg(json, &message, &text)?;
    }
    Ok(())
}

fn svcacct_remove(args: UserSvcacctTargetArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "svcacct", "remove"]);
    }
    let client = admin_client(&args.alias)?;
    block_on(iam::delete_service_account(&client, &args.service_account))
        .context("Unable to remove the specified service account")?;
    print_msg(
        json,
        &AcctMessage::new(&args.service_account),
        &format!(
            "Removed service account `{}` successfully.",
            args.service_account
        ),
    )
}

/// `svcacct info` / `sts info` output (`--policy` prints the policy document only).
fn print_account_info(
    json: bool,
    access_key: &str,
    info: iam::InfoServiceAccountResp,
    policy_only: bool,
    with_names: bool,
) -> Result<()> {
    if policy_only {
        if info.policy.is_empty() {
            return Err(anyhow::Error::new(dummy_error()).context(
                "No policy found associated to the specified service account. Check the policy of its parent user.",
            ));
        }
        return print_policy_document(&info.policy);
    }
    let mut message = AcctMessage {
        parent_user: info.parent_user,
        account_status: info.account_status,
        implied_policy: info.implied_policy,
        policy: iam::raw_policy(&info.policy),
        expiration: info.expiration,
        ..AcctMessage::new(access_key)
    };
    if with_names {
        message.name = info.name;
        message.description = info.description;
    }
    print_msg(json, &message, &message.info_text())
}

fn svcacct_info(args: UserSvcacctInfoArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "svcacct", "info"]);
    }
    let client = admin_client(&args.alias)?;
    let info = block_on(iam::info_service_account(&client, &args.service_account))
        .context("Unable to get information of the specified service account")?;
    print_account_info(json, &args.service_account, info, args.policy, true)
}

/// A policy file for an update request: mc sends the raw bytes (`json.RawMessage`), which
/// fail to marshal when they are not JSON; an empty file sends no policy.
pub(crate) fn raw_policy_file(data: &[u8], message: &'static str) -> Result<Option<GoJson>> {
    if data.is_empty() {
        return Ok(None);
    }
    match GoJson::parse(data) {
        Ok(policy) => Ok(Some(policy)),
        Err(err) => {
            let cause = McError::with_detail(
                format!(
                    "json: error calling MarshalJSON for type json.RawMessage: {}",
                    iam::json_error_text(data, &err)
                ),
                crate::detail![
                    ("Type", serde_json::json!({})),
                    ("Err", serde_json::json!({"Offset": 0}))
                ],
            );
            Err(anyhow::Error::new(cause).context(message))
        }
    }
}

fn svcacct_edit(args: UserSvcacctEditArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "svcacct", "edit"]);
    }
    let client = admin_client(&args.alias)?;
    let mut policy = Vec::new();
    if let Some(path) = non_empty(args.policy) {
        policy = read_file(&path).context("Unable to open the policy document.")?;
    }
    let mut req = iam::UpdateServiceAccountReq {
        new_secret_key: args.secret_key.unwrap_or_default(),
        new_name: args.name.unwrap_or_default(),
        new_description: args.description.unwrap_or_default(),
        ..Default::default()
    };
    if let Some(expiry) = non_empty(args.expiry) {
        req.new_expiration = Some(parse_expiry(
            &expiry,
            "expiry argument is not matching any of the supported patterns".into(),
            "unable to parse the expiry argument.",
        )?);
    }
    const MESSAGE: &str = "Unable to edit the specified service account";
    req.new_policy = raw_policy_file(&policy, MESSAGE)?;
    block_on(iam::update_service_account(
        &client,
        &args.service_account,
        &req,
    ))
    .context(MESSAGE)?;
    print_msg(
        json,
        &AcctMessage::new(&args.service_account),
        &format!(
            "Edited service account `{}` successfully.",
            args.service_account
        ),
    )
}

fn svcacct_status(args: UserSvcacctTargetArgs, json: bool, enable: bool) -> Result<()> {
    let (name, status, message, done) = if enable {
        (
            "enable",
            "on",
            "Unable to enable the specified service account",
            "Enabled",
        )
    } else {
        (
            "disable",
            "off",
            "Unable to disable the specified service account",
            "Disabled",
        )
    };
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "svcacct", name]);
    }
    let client = admin_client(&args.alias)?;
    let req = iam::UpdateServiceAccountReq {
        new_status: status.into(),
        ..Default::default()
    };
    block_on(iam::update_service_account(
        &client,
        &args.service_account,
        &req,
    ))
    .context(message)?;
    print_msg(
        json,
        &AcctMessage::new(&args.service_account),
        &format!(
            "{done} service account `{}` successfully.",
            args.service_account
        ),
    )
}

fn sts(args: UserStsArgs, json: bool) -> Result<()> {
    match args.command {
        UserStsCommand::Info(args) => sts_info(args, json),
    }
}

fn sts_info(args: UserStsInfoArgs, json: bool) -> Result<()> {
    if !args.extra.is_empty() {
        show_help(&["admin", "user", "sts", "info"]);
    }
    let client = admin_client(&args.alias)?;
    let info = block_on(iam::temporary_account_info(&client, &args.sts_account))
        .context("Unable to get information of the specified service account")?;
    print_account_info(json, &args.sts_account, info, args.policy, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_row_pads_and_cuts_like_mc() {
        assert_eq!(
            pretty_row("  ", &[9, 20, 20], &["enabled", "user1", "readonly"]),
            "enabled    user1                 readonly            "
        );
        assert_eq!(
            pretty_row(" | ", &[20, 29], &["   Access Key", "Expiry"]),
            "   Access Key        | Expiry                       "
        );
        assert_eq!(pretty_row("  ", &[9], &["abcdefghijkl"]), "abcdef...");
    }

    #[test]
    fn user_info_text() {
        let message = UserMessage {
            policy_name: "readonly".into(),
            user_status: "enabled".into(),
            member_of: vec![
                UserGroup {
                    name: "g1".into(),
                    policies: vec![],
                },
                UserGroup {
                    name: "g2".into(),
                    policies: vec![],
                },
            ],
            ..UserMessage::new("u1")
        };
        assert_eq!(
            message.info_text(),
            "AccessKey: u1\nStatus: enabled\nPolicyName: readonly\nMemberOf: [g1 g2]"
        );
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"status":"success","accessKey":"u1","policyName":"readonly","userStatus":"enabled","memberOf":[{"name":"g1"},{"name":"g2"}]}"#
        );
    }

    #[test]
    fn acct_info_text() {
        let message = AcctMessage {
            parent_user: "u1".into(),
            account_status: "on".into(),
            implied_policy: true,
            ..AcctMessage::new("k1")
        };
        assert_eq!(
            message.info_text(),
            "AccessKey: k1\nParentUser: u1\nStatus: on\nName: \nDescription: \nPolicy: implied\nExpiration: no-expiry"
        );
    }

    #[test]
    fn path_errors_look_like_go() {
        let err = read_file("/nonexistent-mx-test").unwrap_err();
        assert_eq!(
            err.message,
            "open /nonexistent-mx-test: no such file or directory"
        );
        assert_eq!(
            serde_json::to_string(&err.detail).unwrap(),
            r#"{"Op":"open","Path":"/nonexistent-mx-test","Err":2}"#
        );
        let err = read_file("/").unwrap_err();
        assert_eq!(err.message, "read /: is a directory");
    }
}
