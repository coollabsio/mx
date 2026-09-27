//! `mx ilm tier add|edit|update|ls|info|check|verify|rm` (MinIO admin API
//! `/minio/admin/v3/tier`). Wired from `ilm.rs` as `IlmCommand::Tier`.

use crate::commands::runtime;
use crate::config::ConfigStore;
use crate::error::McError;
use crate::output;
use crate::s3::admin::{
    self, AdminClient, ServicePrincipalAuth, TierAzure, TierConfig, TierCreds, TierGCS, TierInfo,
    TierMinIO, TierS3,
};
use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Args)]
pub struct IlmTierArgs {
    #[command(subcommand)]
    pub command: IlmTierCommand,
}

#[derive(Debug, Subcommand)]
pub enum IlmTierCommand {
    #[command(about = "add a new remote tier target")]
    Add(Box<IlmTierAddArgs>),
    #[command(hide = true, about = "update an existing remote tier configuration")]
    Edit(IlmTierEditArgs),
    #[command(about = "update an existing remote tier configuration")]
    Update(IlmTierEditArgs),
    #[command(visible_alias = "list", about = "list configured remote tier targets")]
    Ls(IlmTierAliasArgs),
    #[command(about = "display tier statistics")]
    Info(IlmTierInfoArgs),
    #[command(about = "validate remote tier configuration")]
    Check(IlmTierNameArgs),
    #[command(hide = true, about = "verifies if remote tier configuration is valid")]
    Verify(IlmTierNameArgs),
    #[command(visible_alias = "remove", about = "remove an empty remote tier")]
    Rm(IlmTierRmArgs),
}

#[derive(Debug, Default, Args)]
pub struct IlmTierAddArgs {
    /// tier type: minio, s3, azure or gcs
    pub tier_type: String,
    pub alias: String,
    /// name of the remote tier target, e.g. WARM-TIER
    pub name: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
    /// remote tier endpoint. e.g https://s3.amazonaws.com
    #[arg(long)]
    pub endpoint: Option<String>,
    /// remote tier region. e.g us-west-2
    #[arg(long)]
    pub region: Option<String>,
    /// AWS S3 or compatible object storage access-key
    #[arg(long)]
    pub access_key: Option<String>,
    /// AWS S3 or compatible object storage secret-key
    #[arg(long)]
    pub secret_key: Option<String>,
    /// use AWS S3 role
    #[arg(long)]
    pub use_aws_role: bool,
    /// use AWS S3 role name
    #[arg(long)]
    pub aws_role_arn: Option<String>,
    /// use AWS S3 web identity file
    #[arg(long)]
    pub aws_web_identity_file: Option<String>,
    /// Azure Blob Storage account name
    #[arg(long)]
    pub account_name: Option<String>,
    /// Azure Blob Storage account key
    #[arg(long)]
    pub account_key: Option<String>,
    /// Directory ID for the Azure service principal account
    #[arg(long)]
    pub az_sp_tenant_id: Option<String>,
    /// The client ID of the Azure service principal account
    #[arg(long)]
    pub az_sp_client_id: Option<String>,
    /// The client secret of the Azure service principal account
    #[arg(long)]
    pub az_sp_client_secret: Option<String>,
    /// path to Google Cloud Storage credentials file
    #[arg(long)]
    pub credentials_file: Option<String>,
    /// remote tier bucket
    #[arg(long)]
    pub bucket: Option<String>,
    /// remote tier prefix
    #[arg(long)]
    pub prefix: Option<String>,
    /// remote tier storage-class
    #[arg(long)]
    pub storage_class: Option<String>,
    /// ignores in-use check for remote tier bucket/prefix
    #[arg(long, hide = true)]
    pub force: bool,
}

#[derive(Debug, Default, Args)]
pub struct IlmTierEditArgs {
    pub alias: String,
    pub name: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
    /// AWS S3 or compatible object storage access-key
    #[arg(long)]
    pub access_key: Option<String>,
    /// AWS S3 or compatible object storage secret-key
    #[arg(long)]
    pub secret_key: Option<String>,
    /// use AWS S3 role
    #[arg(long)]
    pub use_aws_role: bool,
    /// Azure Blob Storage account key
    #[arg(long)]
    pub account_key: Option<String>,
    /// Directory ID for the Azure service principal account
    #[arg(long)]
    pub az_sp_tenant_id: Option<String>,
    /// The client ID of the Azure service principal account
    #[arg(long)]
    pub az_sp_client_id: Option<String>,
    /// The client secret of the Azure service principal account
    #[arg(long)]
    pub az_sp_client_secret: Option<String>,
    /// path to Google Cloud Storage credentials file
    #[arg(long)]
    pub credentials_file: Option<String>,
}

#[derive(Debug, Args)]
pub struct IlmTierNameArgs {
    pub alias: String,
    pub name: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct IlmTierRmArgs {
    pub alias: String,
    pub name: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
    /// forcefully remove the specified tier
    #[arg(long, hide = true)]
    pub force: bool,
    /// additional flag to be required in addition to force flag
    #[arg(long, hide = true)]
    pub dangerous: bool,
}

#[derive(Debug, Args)]
pub struct IlmTierAliasArgs {
    pub alias: String,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

#[derive(Debug, Args)]
pub struct IlmTierInfoArgs {
    pub alias: String,
    pub name: Option<String>,
    #[arg(hide = true)]
    pub extra: Vec<String>,
}

/// mc `tierMessage`.
#[derive(Debug, Default, Serialize)]
struct TierMessage {
    #[serde(skip)]
    op: &'static str,
    status: &'static str,
    #[serde(rename = "tierName")]
    tier_name: String,
    #[serde(rename = "tierType")]
    tier_type: String,
    #[serde(rename = "tierEndpoint")]
    endpoint: String,
    bucket: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    prefix: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    region: String,
    #[serde(rename = "tierParams", skip_serializing_if = "Option::is_none")]
    tier_params: Option<BTreeMap<&'static str, String>>,
}

impl TierMessage {
    fn new(op: &'static str, name: &str) -> Self {
        Self {
            op,
            status: "success",
            tier_name: name.to_string(),
            ..Default::default()
        }
    }

    /// mc `tierMessage.String()`: ops without a text (mc's `remove`, `update`) print an
    /// empty line.
    fn text(&self) -> String {
        let name = &self.tier_name;
        match self.op {
            "add" => format!("Added remote tier {name} of type {}", self.tier_type),
            "verify" => format!("Verified remote tier {name}"),
            "check" => format!("Remote tier connectivity check for {name} was successful"),
            "edit" => format!("Updated remote tier {name}"),
            _ => String::new(),
        }
    }

    fn print(&self, json: bool) -> Result<()> {
        if json {
            output::print_json(self)
        } else {
            output::print_plain(&self.text());
            Ok(())
        }
    }
}

#[derive(Serialize)]
struct TierList<'a> {
    status: &'static str,
    tiers: &'a [TierConfig],
}

/// mc `tierInfoMessage` with its `tierInfos` JSON shape.
#[derive(Serialize)]
struct TierInfoMessage<'a> {
    status: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tiers: Vec<TierInfoJson<'a>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    error: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct TierInfoJson<'a> {
    name: &'a str,
    #[serde(rename = "API")]
    api: &'a str,
    #[serde(rename = "Type")]
    kind: &'static str,
    stats: &'a admin::TierStats,
    daily_stats: &'a admin::DailyTierStats,
}

const SUPPORTED_AWS_TIER_SC: [&str; 3] = ["STANDARD", "REDUCED_REDUNDANCY", "STANDARD_IA"];

pub fn run(args: IlmTierArgs, json: bool) -> Result<()> {
    match args.command {
        IlmTierCommand::Add(args) => add(*args, json),
        IlmTierCommand::Edit(args) => edit(args, "edit", json),
        IlmTierCommand::Update(args) => edit(args, "update", json),
        IlmTierCommand::Ls(args) => list(args, json),
        IlmTierCommand::Info(args) => info(args, json),
        IlmTierCommand::Check(args) => verify(args, "check", json),
        IlmTierCommand::Verify(args) => verify(args, "verify", json),
        IlmTierCommand::Rm(args) => remove(args, json),
    }
}

/// mc `fatalIf(errInvalidArgument(), message)`.
fn invalid(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(McError::invalid_argument()).context(message.into())
}

fn check_extra(extra: &[String], message: &str) -> Result<()> {
    if extra.is_empty() {
        Ok(())
    } else {
        Err(invalid(message))
    }
}

fn admin_client(alias_arg: &str) -> Result<AdminClient> {
    admin::admin_client_for(&ConfigStore::load_or_create()?, alias_arg)
}

fn require_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(invalid("Tier name can't be empty"));
    }
    Ok(())
}

fn non_empty(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or_default()
}

/// Go `os.ReadFile` error text: `open PATH: no such file or directory`.
fn read_file(path: &str) -> std::result::Result<Vec<u8>, McError> {
    std::fs::read(path).map_err(|err| {
        let text = err.to_string();
        let text = text.split(" (os error").next().unwrap_or(&text);
        let mut chars = text.chars();
        let text = chars
            .next()
            .map(|c| c.to_ascii_lowercase().to_string() + chars.as_str())
            .unwrap_or_default();
        McError::new(format!("open {path}: {text}"))
    })
}

/// Builds the madmin `TierConfig` for `ilm tier add` (mc `fetchTierConfig`).
fn tier_config(args: &IlmTierAddArgs, name: &str) -> Result<TierConfig> {
    let mut config = TierConfig {
        version: "v1".into(),
        tier_type: args.tier_type.clone(),
        name: name.to_string(),
        ..Default::default()
    };
    let tier_type = args.tier_type.as_str();
    let prefix = non_empty(&args.prefix).to_string();
    let region = non_empty(&args.region).to_string();
    let bucket = non_empty(&args.bucket).to_string();
    match tier_type {
        "minio" => {
            let (access_key, secret_key) =
                (non_empty(&args.access_key), non_empty(&args.secret_key));
            if access_key.is_empty() || secret_key.is_empty() {
                return Err(invalid(format!(
                    "{tier_type} remote tier requires access credentials"
                )));
            }
            if bucket.is_empty() {
                return Err(invalid(format!(
                    "{tier_type} remote tier requires target bucket"
                )));
            }
            let endpoint = non_empty(&args.endpoint);
            if endpoint.is_empty() {
                return Err(invalid(format!(
                    "{tier_type} remote tier requires target endpoint"
                )));
            }
            config.minio = Some(TierMinIO {
                endpoint: endpoint.to_string(),
                access_key: access_key.to_string(),
                secret_key: secret_key.to_string(),
                bucket,
                prefix,
                region,
            });
        }
        "s3" => {
            let access = args.access_key.is_some();
            let secret = args.secret_key.is_some();
            let role = args.use_aws_role;
            let role_arn = args.aws_role_arn.is_some();
            let web_identity = args.aws_web_identity_file.is_some();
            let problem = if !access && !secret && !role && !role_arn && !web_identity {
                Some("No authentication mechanism was provided")
            } else if (access || secret) && (role || role_arn || web_identity) {
                Some("Static credentials cannot be combined with AWS role authentication")
            } else if role && (role_arn || web_identity) {
                Some(
                    "--use-aws-role cannot be combined with --aws-role-arn or --aws-web-identity-file",
                )
            } else if role_arn != web_identity {
                Some(
                    "Both --use-aws-role and --aws-web-identity-file are required to enable web identity token based authentication",
                )
            } else if access != secret {
                Some(
                    "Both --access-key and --secret-key are required to enable static credentials authentication",
                )
            } else {
                None
            };
            if let Some(problem) = problem {
                return Err(invalid(format!("{tier_type}: {problem}")));
            }
            if bucket.is_empty() {
                return Err(invalid(format!(
                    "{tier_type} remote tier requires target bucket"
                )));
            }
            let storage_class = non_empty(&args.storage_class);
            if !storage_class.is_empty() && !SUPPORTED_AWS_TIER_SC.contains(&storage_class) {
                return Err(invalid(format!(
                    "unsupported storage-class type {storage_class}"
                )));
            }
            let endpoint = non_empty(&args.endpoint);
            config.s3 = Some(TierS3 {
                endpoint: if endpoint.is_empty() {
                    "https://s3.amazonaws.com".into()
                } else {
                    endpoint.to_string()
                },
                access_key: non_empty(&args.access_key).to_string(),
                secret_key: non_empty(&args.secret_key).to_string(),
                bucket,
                prefix,
                region,
                storage_class: storage_class.to_string(),
                aws_role: role,
                aws_role_web_identity_token_file: non_empty(&args.aws_web_identity_file)
                    .to_string(),
                aws_role_arn: non_empty(&args.aws_role_arn).to_string(),
                ..Default::default()
            });
        }
        "azure" => {
            let account_name = non_empty(&args.account_name);
            let account_key = non_empty(&args.account_key);
            let (tenant, client, secret) = (
                non_empty(&args.az_sp_tenant_id),
                non_empty(&args.az_sp_client_id),
                non_empty(&args.az_sp_client_secret),
            );
            // mc uses `errDummy` here: the message is printed without a cause.
            if account_name.is_empty() {
                bail!("{tier_type} remote tier requires the storage account name");
            }
            if account_key.is_empty()
                && (tenant.is_empty() || client.is_empty() || secret.is_empty())
            {
                bail!(
                    "{tier_type} remote tier requires static credentials OR service principal credentials"
                );
            }
            if bucket.is_empty() {
                bail!("{tier_type} remote tier requires target bucket");
            }
            let mut sp_auth = ServicePrincipalAuth::default();
            if !tenant.is_empty() || !client.is_empty() || !secret.is_empty() {
                let missing = if tenant.is_empty() {
                    Some("empty tenant ID unsupported")
                } else if client.is_empty() {
                    Some("empty client ID unsupported")
                } else if secret.is_empty() {
                    Some("empty client secret unsupported")
                } else {
                    None
                };
                if let Some(missing) = missing {
                    return Err(anyhow::Error::new(McError::new(missing))
                        .context("Invalid configuration for Azure Blob Storage remote tier"));
                }
                sp_auth = ServicePrincipalAuth {
                    tenant_id: tenant.to_string(),
                    client_id: client.to_string(),
                    client_secret: secret.to_string(),
                };
            }
            config.azure = Some(TierAzure {
                endpoint: non_empty(&args.endpoint).to_string(),
                account_name: account_name.to_string(),
                account_key: account_key.to_string(),
                bucket,
                prefix,
                region,
                storage_class: String::new(),
                sp_auth,
            });
        }
        "gcs" => {
            if bucket.is_empty() {
                return Err(invalid(format!(
                    "{tier_type} remote requires target bucket"
                )));
            }
            let path = non_empty(&args.credentials_file);
            let creds = read_file(path).context("Failed to read credentials file")?;
            config.gcs = Some(TierGCS {
                // madmin: the endpoint is only for client-side display.
                endpoint: "https://storage.googleapis.com/".into(),
                creds: gcs_creds(&creds),
                bucket,
                prefix,
                region,
                storage_class: String::new(),
            });
        }
        _ => {
            return Err(anyhow::Error::new(McError::new("unsupported tier type"))
                .context("Unsupported tier type"));
        }
    }
    Ok(config)
}

/// madmin `NewTierGCS`: URL-safe base64 (with padding) of the credentials JSON.
fn gcs_creds(json: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE.encode(json)
}

fn add(args: IlmTierAddArgs, json: bool) -> Result<()> {
    check_extra(
        &args.extra,
        "Incorrect number of arguments for tier add command.",
    )?;
    if !matches!(args.tier_type.as_str(), "minio" | "s3" | "azure" | "gcs") {
        return Err(anyhow::Error::new(McError::new("unsupported tier type"))
            .context("Unsupported tier type"));
    }
    require_name(&args.name)?;
    let client = admin_client(&args.alias)?;
    let config = tier_config(&args, &args.name.to_uppercase())?;
    runtime()?
        .block_on(admin::add_tier(&client, &config, args.force))
        .context("Unable to configure remote tier target")?;
    let message = TierMessage {
        tier_type: config.tier_type.clone(),
        endpoint: config.endpoint().to_string(),
        bucket: config.bucket().to_string(),
        prefix: config.prefix().to_string(),
        region: config.region().to_string(),
        tier_params: config
            .s3
            .as_ref()
            .map(|s3| BTreeMap::from([("storageClass", s3.storage_class.clone())])),
        ..TierMessage::new("add", &config.name)
    };
    message.print(json)
}

fn edit(args: IlmTierEditArgs, op: &'static str, json: bool) -> Result<()> {
    check_extra(
        &args.extra,
        "Incorrect number of arguments for tier-edit subcommand.",
    )?;
    let client = admin_client(&args.alias)?;
    let (access_key, secret_key) = (non_empty(&args.access_key), non_empty(&args.secret_key));
    let (tenant, client_id, secret) = (
        non_empty(&args.az_sp_tenant_id),
        non_empty(&args.az_sp_client_id),
        non_empty(&args.az_sp_client_secret),
    );
    let account_key = non_empty(&args.account_key);
    let creds_path = non_empty(&args.credentials_file);
    let mut creds = TierCreds::default();
    if !access_key.is_empty() && !secret_key.is_empty() && !args.use_aws_role {
        creds.access_key = access_key.to_string();
        creds.secret_key = secret_key.to_string();
    } else if args.use_aws_role {
        creds.aws_role = true;
    } else if !account_key.is_empty() {
        creds.secret_key = account_key.to_string();
    } else if !tenant.is_empty() || !client_id.is_empty() || !secret.is_empty() {
        creds.az_sp = ServicePrincipalAuth {
            tenant_id: tenant.to_string(),
            client_id: client_id.to_string(),
            client_secret: secret.to_string(),
        };
    } else if !creds_path.is_empty() {
        creds.creds_json = read_file(creds_path)
            .with_context(|| format!("Unable to read credentials file at {creds_path}"))?;
    } else {
        return Err(invalid(
            "Insufficient credential information supplied to update remote tier target credentials",
        ));
    }
    runtime()?
        .block_on(admin::edit_tier(&client, &args.name, &creds))
        .context("Unable to edit remote tier")?;
    TierMessage::new(op, &args.name).print(json)
}

fn list(args: IlmTierAliasArgs, json: bool) -> Result<()> {
    check_extra(
        &args.extra,
        "Incorrect number of arguments for tier-ls subcommand.",
    )?;
    let client = admin_client(&args.alias)?;
    let mut tiers = runtime()?
        .block_on(admin::list_tiers(&client))
        .context("Unable to list configured remote tier targets")?;
    if tiers.is_empty() {
        // mc `console.Infoln` (also with --json).
        let prog = output::prog_name();
        println!(
            "{prog}: No remote tier targets found for alias '{}'. Use `{prog} ilm tier add` to configure one.",
            args.alias
        );
        return Ok(());
    }
    if json {
        return output::print_json(&TierList {
            status: "success",
            tiers: &tiers,
        });
    }
    tiers.sort_by(|a, b| a.name.cmp(&b.name));
    let dash = |value: &str| {
        if value.is_empty() {
            "-".to_string()
        } else {
            value.to_string()
        }
    };
    let rows: Vec<Vec<String>> = tiers
        .iter()
        .map(|tier| {
            vec![
                dash(&tier.name),
                dash(&tier.tier_type),
                dash(tier.endpoint()),
                dash(tier.bucket()),
                dash(tier.prefix()),
                dash(tier.region()),
                dash(tier.storage_class()),
            ]
        })
        .collect();
    println!(
        "{}",
        render_table(
            &[
                "Name",
                "Type",
                "Endpoint",
                "Bucket",
                "Prefix",
                "Region",
                "Storage-Class"
            ],
            &rows,
            |_, _| Align::Center,
        )
    );
    Ok(())
}

fn tier_info_type(api: &str) -> &'static str {
    if api == "internal" { "hot" } else { "warm" }
}

fn info(args: IlmTierInfoArgs, json: bool) -> Result<()> {
    if json && args.name.is_some() {
        return Err(invalid(
            "Incorrect number of arguments for tier-info subcommand with json output.",
        ));
    }
    check_extra(
        &args.extra,
        "Incorrect number of arguments for tier-info subcommand.",
    )?;
    let client = admin_client(&args.alias)?;
    let rt = runtime()?;
    let stats = rt.block_on(admin::tier_stats(&client));
    if json {
        let message = match &stats {
            Ok(infos) => TierInfoMessage {
                status: "success",
                tiers: infos
                    .iter()
                    .map(|info| TierInfoJson {
                        name: &info.name,
                        api: &info.tier_type,
                        kind: tier_info_type(&info.tier_type),
                        stats: &info.stats,
                        daily_stats: &info.daily_stats,
                    })
                    .collect(),
                error: String::new(),
            },
            Err(err) => TierInfoMessage {
                status: "error",
                tiers: Vec::new(),
                error: err.to_string(),
            },
        };
        return output::print_json(&message);
    }
    // mc ignores a stats error in text mode (the table is just empty).
    let infos = stats.unwrap_or_default();
    let name = args.name.as_deref().unwrap_or_default();
    let mut shown: Vec<TierInfo> = infos
        .into_iter()
        .filter(|info| name.is_empty() || info.name == name)
        .collect();
    if shown.is_empty() {
        // A valid tier without stats yet is shown with empty usage.
        let tiers = rt
            .block_on(admin::list_tiers(&client))
            .context("Unable to list configured remote tier targets")?;
        if let Some(tier) = tiers.iter().find(|t| t.name == name) {
            shown.push(TierInfo {
                name: name.to_string(),
                tier_type: tier.tier_type.clone(),
                ..Default::default()
            });
        }
    }
    if shown.is_empty() {
        if name.is_empty() {
            println!("No remote tiers configured");
        } else {
            println!("No remote tiers' name match {name}");
        }
        return Ok(());
    }
    let rows: Vec<Vec<String>> = shown
        .iter()
        .map(|info| {
            vec![
                info.name.clone(),
                info.tier_type.clone(),
                tier_info_type(&info.tier_type).to_string(),
                admin::ibytes(info.stats.total_size),
                info.stats.num_objects.to_string(),
                info.stats.num_versions.to_string(),
            ]
        })
        .collect();
    // mc's style function: numbers (Usage/Objects/Versions) are right-aligned, except in
    // the first data row, which lipgloss v1 styles with mc's "header" style (row index 0).
    println!(
        "{}",
        render_table(
            &["Tier Name", "API", "Type", "Usage", "Objects", "Versions"],
            &rows,
            |row, col| match (row, col) {
                (Some(0), _) => Align::Center,
                (_, 3..=5) => Align::Right,
                _ => Align::Center,
            },
        )
    );
    Ok(())
}

fn verify(args: IlmTierNameArgs, op: &'static str, json: bool) -> Result<()> {
    check_extra(
        &args.extra,
        "Incorrect number of arguments for tier verify command.",
    )?;
    require_name(&args.name)?;
    let client = admin_client(&args.alias)?;
    runtime()?
        .block_on(admin::verify_tier(&client, &args.name))
        .context("Unable to verify remote tier target")?;
    TierMessage::new(op, &args.name).print(json)
}

fn remove(args: IlmTierRmArgs, json: bool) -> Result<()> {
    check_extra(
        &args.extra,
        "Incorrect number of arguments for tier remove command.",
    )?;
    require_name(&args.name)?;
    if args.force && !args.dangerous {
        return Err(invalid(
            "This operation results in an irreversible disconnection from the specified remote tier. If you are really sure, retry this command with ‘--force’ and ‘--dangerous’ flags.",
        ));
    }
    let client = admin_client(&args.alias)?;
    runtime()?
        .block_on(admin::remove_tier(&client, &args.name, args.force))
        .context("Unable to remove remote tier target")?;
    // mc's command is named `remove`, which has no text message: it prints an empty line.
    TierMessage::new("remove", &args.name).print(json)
}

/// Cell alignment for [`render_table`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Center,
    Right,
}

/// Renders a lipgloss `table` with `NormalBorder` and no cell padding, like mc's tier
/// tables without a terminal. `align(row, col)` gets `None` for the header row. No
/// trailing newline (mc prints it with `fmt.Println`).
pub(crate) fn render_table(
    headers: &[&str],
    rows: &[Vec<String>],
    align: impl Fn(Option<usize>, usize) -> Align,
) -> String {
    let width = |s: &str| s.chars().count();
    let mut widths: Vec<usize> = headers.iter().map(|h| width(h)).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(width(cell));
        }
    }
    let line = |left: &str, mid: &str, right: &str| {
        let parts: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
        format!("{left}{}{right}", parts.join(mid))
    };
    let row_text = |row: Option<usize>, cells: Vec<&str>| {
        let parts: Vec<String> = cells
            .iter()
            .zip(&widths)
            .enumerate()
            .map(|(col, (cell, width))| {
                let pad = width - cell.chars().count();
                match align(row, col) {
                    Align::Right => format!("{}{cell}", " ".repeat(pad)),
                    Align::Center => {
                        let left = pad / 2;
                        format!("{}{cell}{}", " ".repeat(left), " ".repeat(pad - left))
                    }
                }
            })
            .collect();
        format!("│{}│", parts.join("│"))
    };
    let mut out = vec![line("┌", "┬", "┐"), row_text(None, headers.to_vec())];
    out.push(line("├", "┼", "┤"));
    for (index, row) in rows.iter().enumerate() {
        out.push(row_text(
            Some(index),
            row.iter().map(String::as_str).collect(),
        ));
    }
    out.push(line("└", "┴", "┘"));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn add_args(tier_type: &str) -> IlmTierAddArgs {
        IlmTierAddArgs {
            tier_type: tier_type.into(),
            alias: "local".into(),
            name: "warm".into(),
            endpoint: Some("http://remote:9000".into()),
            access_key: Some("ak".into()),
            secret_key: Some("sk".into()),
            bucket: Some("b".into()),
            prefix: Some("p/".into()),
            ..Default::default()
        }
    }

    fn cause(err: anyhow::Error) -> String {
        let (message, cause) = output::split_error(&err);
        format!("{message} | {cause}")
    }

    #[test]
    fn builds_minio_tier_config() {
        let config = tier_config(&add_args("minio"), "WARM").unwrap();
        assert_eq!(
            serde_json::to_string(&config).unwrap(),
            r#"{"Version":"v1","Type":"minio","Name":"WARM","MinIO":{"Endpoint":"http://remote:9000","AccessKey":"ak","SecretKey":"sk","Bucket":"b","Prefix":"p/"}}"#
        );
    }

    #[test]
    fn builds_s3_tier_config_and_validates() {
        let mut args = add_args("s3");
        args.endpoint = None;
        args.storage_class = Some("STANDARD_IA".into());
        args.region = Some("us-west-2".into());
        let config = tier_config(&args, "WARM").unwrap();
        assert_eq!(
            serde_json::to_string(&config).unwrap(),
            r#"{"Version":"v1","Type":"s3","Name":"WARM","S3":{"Endpoint":"https://s3.amazonaws.com","AccessKey":"ak","SecretKey":"sk","Bucket":"b","Prefix":"p/","Region":"us-west-2","StorageClass":"STANDARD_IA"}}"#
        );

        args.storage_class = Some("GLACIER".into());
        assert_eq!(
            cause(tier_config(&args, "W").unwrap_err()),
            "unsupported storage-class type GLACIER | Invalid arguments provided, please refer `mc <command> -h` for relevant documentation."
        );
        args.storage_class = None;
        args.use_aws_role = true;
        assert!(
            cause(tier_config(&args, "W").unwrap_err())
                .starts_with("s3: Static credentials cannot be combined")
        );
        args.access_key = None;
        args.secret_key = None;
        assert!(tier_config(&args, "W").unwrap().s3.unwrap().aws_role);

        let mut minio = add_args("minio");
        minio.endpoint = None;
        assert!(
            cause(tier_config(&minio, "W").unwrap_err())
                .starts_with("minio remote tier requires target endpoint | Invalid arguments")
        );
        assert_eq!(
            cause(tier_config(&add_args("bogus"), "W").unwrap_err()),
            "Unsupported tier type | unsupported tier type"
        );
    }

    /// madmin-go `NewTierAzure`: `SPAuth` is a struct and always marshaled.
    #[test]
    fn builds_azure_tier_config_like_madmin() {
        let mut args = IlmTierAddArgs {
            tier_type: "azure".into(),
            account_name: Some("acct".into()),
            account_key: Some("a2V5".into()),
            bucket: Some("azb".into()),
            prefix: Some("p/".into()),
            ..Default::default()
        };
        let config = tier_config(&args, "AZ").unwrap();
        assert_eq!(
            serde_json::to_string(&config).unwrap(),
            r#"{"Version":"v1","Type":"azure","Name":"AZ","Azure":{"AccountName":"acct","AccountKey":"a2V5","Bucket":"azb","Prefix":"p/","SPAuth":{}}}"#
        );

        args.account_key = None;
        args.az_sp_tenant_id = Some("t".into());
        args.az_sp_client_id = Some("c".into());
        args.az_sp_client_secret = Some("s".into());
        args.endpoint = Some("https://acct.blob.core.windows.net".into());
        let config = tier_config(&args, "AZ").unwrap();
        assert_eq!(
            serde_json::to_value(&config).unwrap()["Azure"],
            json!({"Endpoint": "https://acct.blob.core.windows.net", "AccountName": "acct", "Bucket": "azb", "Prefix": "p/", "SPAuth": {"TenantID": "t", "ClientID": "c", "ClientSecret": "s"}})
        );
        assert_eq!(config.endpoint(), "https://acct.blob.core.windows.net");

        args.az_sp_client_secret = None;
        assert_eq!(
            cause(tier_config(&args, "AZ").unwrap_err()),
            "azure remote tier requires static credentials OR service principal credentials | "
        );
        args.account_name = None;
        assert_eq!(
            cause(tier_config(&args, "AZ").unwrap_err()),
            "azure remote tier requires the storage account name | "
        );
    }

    /// madmin-go `NewTierGCS`: creds are `base64.URLEncoding` of the file contents.
    #[cfg(not(windows))] // Windows OS error texts differ
    #[test]
    fn builds_gcs_tier_config_like_madmin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("creds.json");
        std::fs::write(&path, b"{\"type\":\"service_account\",\"k\":\"??>\"}").unwrap();
        let mut args = IlmTierAddArgs {
            tier_type: "gcs".into(),
            credentials_file: Some(path.to_string_lossy().into_owned()),
            bucket: Some("gb".into()),
            region: Some("us".into()),
            // Ignored for GCS like mc.
            endpoint: Some("http://ignored".into()),
            ..Default::default()
        };
        let config = tier_config(&args, "GT").unwrap();
        assert_eq!(
            serde_json::to_string(&config).unwrap(),
            r#"{"Version":"v1","Type":"gcs","Name":"GT","GCS":{"Endpoint":"https://storage.googleapis.com/","Creds":"eyJ0eXBlIjoic2VydmljZV9hY2NvdW50IiwiayI6Ij8_PiJ9","Bucket":"gb","Region":"us"}}"#
        );
        args.credentials_file = Some("/nonexistent/creds.json".into());
        assert_eq!(
            cause(tier_config(&args, "GT").unwrap_err()),
            "Failed to read credentials file | open /nonexistent/creds.json: no such file or directory"
        );
        args.bucket = None;
        assert!(
            cause(tier_config(&args, "GT").unwrap_err())
                .starts_with("gcs remote requires target bucket | Invalid arguments")
        );
    }

    #[test]
    fn tier_messages_follow_mc_ops() {
        let mut message = TierMessage::new("add", "WARM");
        message.tier_type = "minio".into();
        assert_eq!(message.text(), "Added remote tier WARM of type minio");
        assert_eq!(TierMessage::new("remove", "W").text(), "");
        assert_eq!(TierMessage::new("update", "W").text(), "");
        assert_eq!(
            TierMessage::new("edit", "W").text(),
            "Updated remote tier W"
        );
        assert_eq!(
            serde_json::to_string(&TierMessage::new("remove", "W")).unwrap(),
            r#"{"status":"success","tierName":"W","tierType":"","tierEndpoint":"","bucket":""}"#
        );
    }

    #[test]
    fn renders_lipgloss_table_without_padding() {
        let table = render_table(
            &["Name", "Bucket"],
            &[vec!["WARM1".into(), "rb2".into()]],
            |_, _| Align::Center,
        );
        assert_eq!(
            table,
            "┌─────┬──────┐\n│Name │Bucket│\n├─────┼──────┤\n│WARM1│ rb2  │\n└─────┴──────┘"
        );
        let rows = vec![
            vec!["a".into(), "1.0 MiB".into()],
            vec!["b".into(), "5 B".into()],
        ];
        let table = render_table(&["N", "Usage"], &rows, |row, col| match (row, col) {
            (Some(0), _) => Align::Center,
            (_, 1) => Align::Right,
            _ => Align::Center,
        });
        assert_eq!(
            table,
            "┌─┬───────┐\n│N│  Usage│\n├─┼───────┤\n│a│1.0 MiB│\n│b│    5 B│\n└─┴───────┘"
        );
    }
}
