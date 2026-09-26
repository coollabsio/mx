//! `mx ilm tier add|edit|ls|info|check|verify|rm` (MinIO admin API `/minio/admin/v3/tier`).
//! Wired from `ilm.rs` as `IlmCommand::Tier`.

use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::output;
use crate::s3::admin::{self, AdminClient, TierConfig, TierCreds};
use crate::target::TargetRef;
use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Args)]
pub struct IlmTierArgs {
    #[command(subcommand)]
    pub command: IlmTierCommand,
}

#[derive(Debug, Subcommand)]
pub enum IlmTierCommand {
    #[command(about = "add a new remote tier target")]
    Add(Box<IlmTierAddArgs>),
    #[command(about = "update an existing remote tier configuration")]
    Edit(IlmTierEditArgs),
    #[command(visible_alias = "list", about = "list remote tier targets")]
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

#[derive(Debug, Args)]
pub struct IlmTierAddArgs {
    /// tier type: minio or s3 (azure and gcs are not supported by mx)
    pub tier_type: String,
    pub alias: String,
    /// name of the remote tier target, e.g. WARM-TIER
    pub name: String,
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

#[derive(Debug, Args)]
pub struct IlmTierEditArgs {
    pub alias: String,
    pub name: String,
    /// AWS S3 or compatible object storage access-key
    #[arg(long)]
    pub access_key: Option<String>,
    /// AWS S3 or compatible object storage secret-key
    #[arg(long)]
    pub secret_key: Option<String>,
    /// use AWS S3 role
    #[arg(long)]
    pub use_aws_role: bool,
}

#[derive(Debug, Args)]
pub struct IlmTierNameArgs {
    pub alias: String,
    pub name: String,
}

#[derive(Debug, Args)]
pub struct IlmTierRmArgs {
    pub alias: String,
    pub name: String,
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
}

#[derive(Debug, Args)]
pub struct IlmTierInfoArgs {
    pub alias: String,
    pub name: Option<String>,
}

/// mc `tierMessage`.
#[derive(Debug, Default, Serialize)]
struct TierMessage {
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
    tier_params: Option<Value>,
}

#[derive(Serialize)]
struct TierList<'a> {
    status: &'static str,
    tiers: &'a [TierConfig],
}

const SUPPORTED_AWS_TIER_SC: [&str; 3] = ["STANDARD", "REDUCED_REDUNDANCY", "STANDARD_IA"];

pub fn run(args: IlmTierArgs, json: bool) -> Result<()> {
    match args.command {
        IlmTierCommand::Add(args) => add(*args, json),
        IlmTierCommand::Edit(args) => edit(args, json),
        IlmTierCommand::Ls(args) => list(args, json),
        IlmTierCommand::Info(args) => info(args, json),
        IlmTierCommand::Check(args) => verify(args, "check", json),
        IlmTierCommand::Verify(args) => verify(args, "verify", json),
        IlmTierCommand::Rm(args) => remove(args, json),
    }
}

fn admin_client(alias_arg: &str) -> Result<AdminClient> {
    let target = TargetRef::parse(alias_arg)?;
    let store = ConfigStore::load_or_create()?;
    AdminClient::new(&alias_config(&store, &target.alias)?)
}

fn require_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Tier name can't be empty");
    }
    Ok(())
}

fn print_message(message: &TierMessage, text: String, json: bool) -> Result<()> {
    if json {
        crate::output::print_json(message)?;
    } else {
        output::print_plain(&text);
    }
    Ok(())
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}

/// Builds the madmin `TierConfig` for `ilm tier add` (mc `fetchTierConfig`).
fn tier_config(args: &IlmTierAddArgs) -> Result<TierConfig> {
    let name = args.name.to_uppercase();
    let mut section = Map::new();
    let mut put = |key: &str, value: Value| {
        section.insert(key.to_string(), value);
    };
    match args.tier_type.as_str() {
        "minio" => {
            let (Some(access_key), Some(secret_key)) =
                (non_empty(&args.access_key), non_empty(&args.secret_key))
            else {
                bail!("minio remote tier requires access credentials");
            };
            let Some(bucket) = non_empty(&args.bucket) else {
                bail!("minio remote tier requires target bucket");
            };
            let Some(endpoint) = non_empty(&args.endpoint) else {
                bail!("minio remote tier requires target endpoint");
            };
            if args.use_aws_role
                || args.aws_role_arn.is_some()
                || args.aws_web_identity_file.is_some()
            {
                bail!("minio remote tier does not support AWS role authentication");
            }
            if args.storage_class.is_some() {
                bail!("minio remote tier does not support --storage-class");
            }
            put("Endpoint", json!(endpoint));
            put("AccessKey", json!(access_key));
            put("SecretKey", json!(secret_key));
            put("Bucket", json!(bucket));
            if let Some(prefix) = non_empty(&args.prefix) {
                put("Prefix", json!(prefix));
            }
            if let Some(region) = non_empty(&args.region) {
                put("Region", json!(region));
            }
            Ok(TierConfig {
                version: "v1".into(),
                tier_type: "minio".into(),
                name,
                minio: Some(section),
                ..Default::default()
            })
        }
        "s3" => {
            let access = args.access_key.is_some();
            let secret = args.secret_key.is_some();
            let role = args.use_aws_role;
            let role_arn = args.aws_role_arn.is_some();
            let web_identity = args.aws_web_identity_file.is_some();
            if !access && !secret && !role && !role_arn && !web_identity {
                bail!("s3: No authentication mechanism was provided");
            }
            if (access || secret) && (role || role_arn || web_identity) {
                bail!("s3: Static credentials cannot be combined with AWS role authentication");
            }
            if role && (role_arn || web_identity) {
                bail!(
                    "s3: --use-aws-role cannot be combined with --aws-role-arn or --aws-web-identity-file"
                );
            }
            if role_arn != web_identity {
                bail!(
                    "s3: Both --aws-role-arn and --aws-web-identity-file are required to enable web identity token based authentication"
                );
            }
            if access != secret {
                bail!(
                    "s3: Both --access-key and --secret-key are required to enable static credentials authentication"
                );
            }
            let Some(bucket) = non_empty(&args.bucket) else {
                bail!("s3 remote tier requires target bucket");
            };
            put(
                "Endpoint",
                json!(non_empty(&args.endpoint).unwrap_or("https://s3.amazonaws.com")),
            );
            if let Some(access_key) = non_empty(&args.access_key) {
                put("AccessKey", json!(access_key));
            }
            if let Some(secret_key) = non_empty(&args.secret_key) {
                put("SecretKey", json!(secret_key));
            }
            put("Bucket", json!(bucket));
            if let Some(prefix) = non_empty(&args.prefix) {
                put("Prefix", json!(prefix));
            }
            if let Some(region) = non_empty(&args.region) {
                put("Region", json!(region));
            }
            if let Some(class) = non_empty(&args.storage_class) {
                if !SUPPORTED_AWS_TIER_SC.contains(&class) {
                    bail!("unsupported storage-class type {class}");
                }
                put("StorageClass", json!(class));
            }
            if role {
                put("AWSRole", json!(true));
            }
            if let Some(arn) = non_empty(&args.aws_role_arn) {
                put("AWSRoleARN", json!(arn));
            }
            if let Some(file) = non_empty(&args.aws_web_identity_file) {
                put("AWSRoleWebIdentityTokenFile", json!(file));
            }
            Ok(TierConfig {
                version: "v1".into(),
                tier_type: "s3".into(),
                name,
                s3: Some(section),
                ..Default::default()
            })
        }
        "azure" | "gcs" => bail!(
            "tier type `{}` is not supported by mx yet (supported: minio, s3)",
            args.tier_type
        ),
        other => bail!("Unsupported tier type `{other}` (supported: minio, s3)"),
    }
}

fn add(args: IlmTierAddArgs, json: bool) -> Result<()> {
    require_name(&args.name)?;
    let config = tier_config(&args)?;
    let client = admin_client(&args.alias)?;
    runtime()?
        .block_on(admin::add_tier(&client, &config, args.force))
        .context("Unable to configure remote tier target")?;
    let message = TierMessage {
        status: "success",
        tier_name: config.name.clone(),
        tier_type: config.tier_type.clone(),
        endpoint: config.field("Endpoint"),
        bucket: config.field("Bucket"),
        prefix: config.field("Prefix"),
        region: config.field("Region"),
        tier_params: (config.tier_type == "s3")
            .then(|| json!({ "storageClass": config.field("StorageClass") })),
    };
    let text = format!(
        "Added remote tier {} of type {}",
        config.name, config.tier_type
    );
    print_message(&message, text, json)
}

fn edit(args: IlmTierEditArgs, json: bool) -> Result<()> {
    let creds = match (
        non_empty(&args.access_key),
        non_empty(&args.secret_key),
        args.use_aws_role,
    ) {
        (_, _, true) => TierCreds {
            aws_role: true,
            ..Default::default()
        },
        (Some(access_key), Some(secret_key), false) => TierCreds {
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            aws_role: false,
        },
        _ => bail!(
            "Insufficient credential information supplied to update remote tier target credentials (use --access-key and --secret-key, or --use-aws-role)"
        ),
    };
    let client = admin_client(&args.alias)?;
    runtime()?
        .block_on(admin::edit_tier(&client, &args.name, &creds))
        .context("Unable to edit remote tier")?;
    let message = TierMessage {
        status: "success",
        tier_name: args.name.clone(),
        ..Default::default()
    };
    print_message(&message, format!("Updated remote tier {}", args.name), json)
}

fn list(args: IlmTierAliasArgs, json: bool) -> Result<()> {
    let client = admin_client(&args.alias)?;
    let mut tiers = runtime()?
        .block_on(admin::list_tiers(&client))
        .context("Unable to list configured remote tier targets")?;
    if tiers.is_empty() {
        output::print_plain(&format!(
            "No remote tier targets found for alias '{}'. Use `mx ilm tier add` to configure one.",
            args.alias
        ));
        return Ok(());
    }
    if json {
        crate::output::print_json(&TierList {
            status: "success",
            tiers: &tiers,
        })?;
        return Ok(());
    }
    tiers.sort_by(|a, b| a.name.cmp(&b.name));
    let dash = |value: String| if value.is_empty() { "-".into() } else { value };
    let rows: Vec<Vec<String>> = tiers
        .iter()
        .map(|tier| {
            let storage_class = match tier.tier_type.as_str() {
                "s3" | "azure" | "gcs" => tier.field("StorageClass"),
                _ => String::new(),
            };
            vec![
                dash(tier.name.clone()),
                dash(tier.tier_type.clone()),
                dash(tier.field("Endpoint")),
                dash(tier.field("Bucket")),
                dash(tier.field("Prefix")),
                dash(tier.field("Region")),
                dash(storage_class),
            ]
        })
        .collect();
    print!(
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
            &rows
        )
    );
    Ok(())
}

fn tier_info_type(api: &str) -> &'static str {
    if api == "internal" { "hot" } else { "warm" }
}

fn info(args: IlmTierInfoArgs, json: bool) -> Result<()> {
    if json && args.name.is_some() {
        bail!("Incorrect number of arguments for tier-info subcommand with json output.");
    }
    let client = admin_client(&args.alias)?;
    let rt = runtime()?;
    let stats = rt.block_on(admin::tier_stats(&client));
    if json {
        let value = match stats {
            Ok(infos) => {
                let tiers: Vec<Value> = infos
                    .iter()
                    .map(|info| {
                        let api = info["Type"].as_str().unwrap_or_default();
                        json!({
                            "Name": info["Name"],
                            "API": api,
                            "Type": tier_info_type(api),
                            "Stats": info["Stats"],
                            "DailyStats": info["DailyStats"],
                        })
                    })
                    .collect();
                if tiers.is_empty() {
                    json!({ "status": "success" })
                } else {
                    json!({ "status": "success", "tiers": tiers })
                }
            }
            Err(err) => json!({ "status": "error", "error": format!("{err:#}") }),
        };
        crate::output::print_json(&value)?;
        return Ok(());
    }
    let infos = stats.context("Unable to get tier statistics")?;
    let name = args.name.as_deref().unwrap_or_default();
    let mut rows: Vec<Vec<String>> = infos
        .iter()
        .filter(|info| name.is_empty() || info["Name"].as_str() == Some(name))
        .map(|info| {
            let api = info["Type"].as_str().unwrap_or_default();
            let stat = |key: &str| info["Stats"][key].as_u64().unwrap_or_default();
            vec![
                info["Name"].as_str().unwrap_or_default().to_string(),
                api.to_string(),
                tier_info_type(api).to_string(),
                admin::ibytes(stat("totalSize")),
                stat("numObjects").to_string(),
                stat("numVersions").to_string(),
            ]
        })
        .collect();
    if rows.is_empty() && !name.is_empty() {
        // Newly added tiers have no stats yet; show them with zero usage.
        let tiers = rt
            .block_on(admin::list_tiers(&client))
            .context("Unable to list configured remote tier targets")?;
        if let Some(tier) = tiers.iter().find(|t| t.name == name) {
            rows.push(vec![
                tier.name.clone(),
                tier.tier_type.clone(),
                tier_info_type(&tier.tier_type).to_string(),
                admin::ibytes(0),
                "0".into(),
                "0".into(),
            ]);
        }
    }
    if rows.is_empty() {
        if name.is_empty() {
            println!("No remote tiers configured");
        } else {
            println!("No remote tiers' name match {name}");
        }
        return Ok(());
    }
    print!(
        "{}",
        render_table(
            &["Tier Name", "API", "Type", "Usage", "Objects", "Versions"],
            &rows
        )
    );
    Ok(())
}

fn verify(args: IlmTierNameArgs, op: &str, json: bool) -> Result<()> {
    require_name(&args.name)?;
    let client = admin_client(&args.alias)?;
    runtime()?
        .block_on(admin::verify_tier(&client, &args.name))
        .context("Unable to verify remote tier target")?;
    let message = TierMessage {
        status: "success",
        tier_name: args.name.clone(),
        ..Default::default()
    };
    let text = if op == "check" {
        format!(
            "Remote tier connectivity check for {} was successful",
            args.name
        )
    } else {
        format!("Verified remote tier {}", args.name)
    };
    print_message(&message, text, json)
}

fn remove(args: IlmTierRmArgs, json: bool) -> Result<()> {
    require_name(&args.name)?;
    if args.force && !args.dangerous {
        bail!(
            "This operation results in an irreversible disconnection from the specified remote tier. If you are really sure, retry this command with '--force' and '--dangerous' flags."
        );
    }
    if args.dangerous && !args.force {
        bail!("--dangerous requires --force");
    }
    let client = admin_client(&args.alias)?;
    runtime()?
        .block_on(admin::remove_tier(&client, &args.name, args.force))
        .context("Unable to remove remote tier target")?;
    let message = TierMessage {
        status: "success",
        tier_name: args.name.clone(),
        ..Default::default()
    };
    print_message(&message, format!("Removed remote tier {}", args.name), json)
}

/// Renders a box-drawn table like mc's lipgloss tables (centered cells).
pub(crate) fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let line = |left: &str, mid: &str, right: &str| {
        let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        format!("{left}{}{right}\n", parts.join(mid))
    };
    let row_text = |cells: Vec<&str>| {
        let parts: Vec<String> = cells
            .iter()
            .zip(&widths)
            .map(|(cell, width)| {
                let pad = width - cell.chars().count();
                let left = pad / 2;
                format!(" {}{}{} ", " ".repeat(left), cell, " ".repeat(pad - left))
            })
            .collect();
        format!("│{}│\n", parts.join("│"))
    };
    let mut out = line("┌", "┬", "┐");
    out.push_str(&row_text(headers.to_vec()));
    out.push_str(&line("├", "┼", "┤"));
    for row in rows {
        out.push_str(&row_text(row.iter().map(String::as_str).collect()));
    }
    out.push_str(&line("└", "┴", "┘"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_args(tier_type: &str) -> IlmTierAddArgs {
        IlmTierAddArgs {
            tier_type: tier_type.into(),
            alias: "local".into(),
            name: "warm".into(),
            endpoint: Some("http://remote:9000".into()),
            region: None,
            access_key: Some("ak".into()),
            secret_key: Some("sk".into()),
            use_aws_role: false,
            aws_role_arn: None,
            aws_web_identity_file: None,
            bucket: Some("b".into()),
            prefix: Some("p/".into()),
            storage_class: None,
            force: false,
        }
    }

    #[test]
    fn builds_minio_tier_config() {
        let config = tier_config(&add_args("minio")).unwrap();
        assert_eq!(
            serde_json::to_value(&config).unwrap(),
            json!({
                "Version": "v1", "Type": "minio", "Name": "WARM",
                "MinIO": {"Endpoint": "http://remote:9000", "AccessKey": "ak", "SecretKey": "sk", "Bucket": "b", "Prefix": "p/"}
            })
        );
    }

    #[test]
    fn builds_s3_tier_config_and_validates() {
        let mut args = add_args("s3");
        args.endpoint = None;
        args.storage_class = Some("STANDARD_IA".into());
        args.region = Some("us-west-2".into());
        let config = tier_config(&args).unwrap();
        assert_eq!(config.field("Endpoint"), "https://s3.amazonaws.com");
        assert_eq!(config.field("StorageClass"), "STANDARD_IA");
        assert_eq!(config.field("Region"), "us-west-2");

        args.storage_class = Some("GLACIER".into());
        assert!(tier_config(&args).is_err());
        args.storage_class = None;
        args.use_aws_role = true;
        assert!(
            tier_config(&args)
                .unwrap_err()
                .to_string()
                .contains("cannot be combined")
        );
        args.access_key = None;
        args.secret_key = None;
        assert!(tier_config(&args).unwrap().s3.unwrap()["AWSRole"] == json!(true));

        let mut minio = add_args("minio");
        minio.endpoint = None;
        assert!(tier_config(&minio).is_err());
        assert!(tier_config(&add_args("azure")).is_err());
        assert!(tier_config(&add_args("bogus")).is_err());
    }

    #[test]
    fn renders_centered_table() {
        let table = render_table(&["Name", "Type"], &[vec!["WARM".into(), "minio".into()]]);
        assert_eq!(
            table,
            "┌──────┬───────┐\n│ Name │ Type  │\n├──────┼───────┤\n│ WARM │ minio │\n└──────┴───────┘\n"
        );
    }
}
