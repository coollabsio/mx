use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use crate::error::McError;
use crate::output;
use crate::target::is_valid_alias;
use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use url::Url;

#[derive(Debug, Args)]
pub struct AliasArgs {
    #[command(subcommand)]
    pub command: AliasCommand,
}

#[derive(Debug, Subcommand)]
pub enum AliasCommand {
    #[command(visible_alias = "s", about = "set a new alias to configuration file")]
    Set(AliasSetArgs),
    #[command(visible_alias = "ls", about = "list aliases in configuration file")]
    List(AliasListArgs),
    #[command(
        visible_alias = "rm",
        about = "remove an alias from configuration file"
    )]
    Remove(AliasRemoveArgs),
    #[command(visible_alias = "i", about = "import an alias from JSON")]
    Import(AliasImportArgs),
    #[command(visible_alias = "e", about = "export an alias as JSON")]
    Export(AliasExportArgs),
}

#[derive(Debug, Args)]
pub struct AliasSetArgs {
    pub alias: String,
    pub url: String,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    /// bucket path lookup supported by the server. Valid options are '[auto, on, off]'
    #[arg(long, default_value = "auto")]
    pub path: String,
    /// API signature. Valid options are '[S3v4, S3v2]'
    #[arg(long)]
    pub api: Option<String>,
}

#[derive(Debug, Args)]
pub struct AliasListArgs {
    pub alias: Option<String>,
}

#[derive(Debug, Args)]
pub struct AliasRemoveArgs {
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct AliasImportArgs {
    pub alias: String,
    /// credentials JSON file (default: stdin)
    pub file: Option<String>,
}

#[derive(Debug, Args)]
pub struct AliasExportArgs {
    pub alias: String,
}

pub fn run(command: AliasCommand, json: bool) -> Result<()> {
    match command {
        AliasCommand::Set(args) => set(args, json),
        AliasCommand::List(args) => list(args, json),
        AliasCommand::Remove(args) => remove(args, json),
        AliasCommand::Import(args) => import(args, json),
        AliasCommand::Export(args) => export(args),
    }
}

fn set(args: AliasSetArgs, json: bool) -> Result<()> {
    let (access_key, secret_key) = collect_credentials(args.access_key, args.secret_key)?;
    let alias = args.alias.trim_end_matches(['/', '\\']).to_string();
    if !is_valid_alias(&alias) {
        return Err(McError::new(format!(
            "Alias `{alias}` should have alphanumeric characters such as [helloWorld0, hello_World0, ...] and begin with a letter"
        )))
        .context("Invalid alias.");
    }
    let url = normalize_url(&args.url)?;
    // mc: empty keys make an anonymous alias; set keys have minimum lengths.
    if !access_key.is_empty() && access_key.len() < 3 {
        return Err(McError::invalid_argument())
            .context(format!("Invalid access key `{access_key}`."));
    }
    if !secret_key.is_empty() && secret_key.len() < 8 {
        return Err(McError::invalid_argument())
            .context(format!("Invalid secret key `{secret_key}`."));
    }
    let api = args.api.as_deref().map(normalize_api).transpose()?;
    let path = normalize_path(&args.path)?;
    // Without `--api`, mc probes the server for the signature version (and fails when the
    // server cannot be reached).
    let api = match api {
        Some(api) => api,
        None => {
            let probe = AliasConfig {
                url: url.clone(),
                access_key: access_key.clone(),
                secret_key: secret_key.clone(),
                api: "S3v4".to_string(),
                path: path.clone(),
                ..Default::default()
            };
            crate::commands::runtime()?
                .block_on(crate::s3::probe_signature(&probe))
                .context("Unable to initialize new alias from the provided credentials.")?;
            "s3v4".to_string()
        }
    };

    let mut store = ConfigStore::load_or_create()?;
    store.config_mut().aliases.insert(
        alias.clone(),
        AliasConfig {
            url: url.clone(),
            access_key: access_key.clone(),
            secret_key: secret_key.clone(),
            api: api.clone(),
            path: path.clone(),
            ..Default::default()
        },
    );
    store.save()?;

    print_message(
        &AliasMessage {
            status: "success",
            alias: &alias,
            url: &url,
            access_key: Some(access_key.as_str()),
            secret_key: Some(secret_key.as_str()),
            api: Some(api.as_str()),
            path: Some(path.as_str()),
            src: None,
        },
        &format!("Added `{alias}` successfully."),
        json,
    )?;
    Ok(())
}

fn list(args: AliasListArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let src = store.path().display().to_string();

    // Includes `MC_HOST_*` / `MC_CONFIG_ENV_FILE` aliases (src `env` / the env file).
    let mut rows = Vec::new();
    for (alias, cfg) in &store.config().aliases {
        if store.is_invalid_env_alias(alias) {
            continue;
        }
        let row_src = cfg.src.clone().unwrap_or_else(|| src.clone());
        rows.push(DisplayAlias::from_parts(
            alias.clone(),
            cfg.clone(),
            row_src,
        ));
    }

    if let Some(alias) = args.alias {
        let alias = normalize_alias(&alias)?;
        let row = rows
            .into_iter()
            .find(|row| row.alias == alias)
            .ok_or_else(|| {
                anyhow::Error::new(McError::invalid_aliased_url(&alias))
                    .context(format!("No such alias `{alias}` found."))
            })?;
        print_rows(&[row], json)?;
        return Ok(());
    }

    print_rows(&rows, json)
}

fn remove(args: AliasRemoveArgs, json: bool) -> Result<()> {
    let alias = normalize_alias(&args.alias)?;
    let mut store = ConfigStore::load_or_create()?;

    if store.config_mut().aliases.remove(&alias).is_none() {
        return Err(McError::invalid_aliased_url(&alias))
            .context(format!("No such alias `{alias}` found."));
    }

    store.save()?;
    print_message(
        &AliasMessage {
            status: "success",
            alias: &alias,
            url: "",
            access_key: None,
            secret_key: None,
            api: None,
            path: None,
            src: None,
        },
        &format!("Removed `{alias}` successfully."),
        json,
    )?;
    Ok(())
}

/// mc `alias import`: stores the JSON document as-is (no normalization or server probe).
fn import(args: AliasImportArgs, json: bool) -> Result<()> {
    let alias = args.alias.trim_end_matches(['/', '\\']).to_string();
    if !is_valid_alias(&alias) {
        return Err(McError::new(format!(
            "Alias `{alias}` should have alphanumeric characters such as [helloWorld0, hello_World0, ...] and begin with a letter"
        )))
        .context("Invalid alias.");
    }
    let raw = match args
        .file
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
    {
        Some(file) => std::fs::read(file).context("Unable to parse credentials file")?,
        None => {
            let mut raw = Vec::new();
            io::stdin()
                .read_to_end(&mut raw)
                .context("Unable to parse credentials file")?;
            raw
        }
    };
    let mut cfg: AliasConfig = serde_json::from_slice(&raw).map_err(|err| {
        anyhow::Error::new(McError::new(go_json_error(&err)))
            .context("Unable to parse input credentials")
    })?;
    normalize_url(&cfg.url)?;
    if !cfg.access_key.is_empty() && cfg.access_key.len() < 3 {
        return Err(McError::invalid_argument())
            .context(format!("Invalid access key `{}`.", cfg.access_key));
    }
    if !cfg.secret_key.is_empty() && cfg.secret_key.len() < 8 {
        return Err(McError::invalid_argument()).context("Invalid secret key.");
    }
    if !cfg.api.is_empty() {
        normalize_api(&cfg.api)?;
    }
    normalize_path(&cfg.path)?;
    cfg.src = None;

    let mut store = ConfigStore::load_or_create()?;
    store
        .config_mut()
        .aliases
        .insert(alias.clone(), cfg.clone());
    store.save()?;
    print_message(
        &AliasMessage {
            status: "success",
            alias: &alias,
            url: &cfg.url,
            access_key: Some(cfg.access_key.as_str()),
            secret_key: Some(cfg.secret_key.as_str()),
            api: Some(cfg.api.as_str()),
            path: Some(cfg.path.as_str()),
            src: None,
        },
        &format!("Imported `{alias}` successfully."),
        json,
    )
}

/// Go `encoding/json` wording for the common "no input" case.
fn go_json_error(err: &serde_json::Error) -> String {
    if err.is_eof() {
        "unexpected end of JSON input".to_string()
    } else {
        err.to_string()
    }
}

fn export(args: AliasExportArgs) -> Result<()> {
    let alias = normalize_alias(&args.alias)?;
    let store = ConfigStore::load_or_create()?;
    let config = store.config().aliases.get(&alias).ok_or_else(|| {
        anyhow::Error::new(McError::invalid_argument()).context("Unable to export credentials")
    })?;
    // mc marshals the stored alias config (known fields only), compact, regardless of TTY.
    let document = AliasConfig {
        src: None,
        extra: Default::default(),
        ..config.clone()
    };
    println!("{}", serde_json::to_string(&document)?);
    Ok(())
}

fn print_rows(rows: &[DisplayAlias], json: bool) -> Result<()> {
    if json {
        for row in rows {
            print_message(&row.as_message(), "", true)?;
        }
        return Ok(());
    }

    // mc: one block per alias, names padded to the longest alias.
    let width = rows.iter().map(|row| row.alias.len()).max().unwrap_or(0);
    let mut out = io::stdout().lock();
    for row in rows {
        writeln!(out, "{:<width$}", row.alias)?;
        for (label, value) in [
            ("URL", &row.url),
            ("AccessKey", &row.access_key),
            ("SecretKey", &row.secret_key),
            ("API", &row.api),
            ("Path", &row.path),
            ("Src", &row.src),
        ] {
            writeln!(out, "  {label:<9} : {value}")?;
        }
    }
    out.flush()?;
    Ok(())
}

fn print_message(message: &AliasMessage<'_>, plain: &str, json: bool) -> Result<()> {
    if json {
        output::print_json(message)?;
    } else if !plain.is_empty() {
        output::print_plain(plain);
    }
    Ok(())
}

fn collect_credentials(
    access_key: Option<String>,
    secret_key: Option<String>,
) -> Result<(String, String)> {
    if access_key.is_some() && secret_key.is_some() {
        return Ok((
            access_key.unwrap_or_default(),
            secret_key.unwrap_or_default(),
        ));
    }

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdin_is_terminal = io::stdin().is_terminal();

    let access_key = match access_key {
        Some(value) => value,
        None if stdin_is_terminal => prompt_line("Enter Access Key: ")?,
        None => read_line(&mut reader)?,
    };

    let secret_key = match secret_key {
        Some(value) => value,
        None if stdin_is_terminal => rpassword::prompt_password("Enter Secret Key: ")?,
        None => read_line(&mut reader)?,
    };

    Ok((access_key.trim().to_string(), secret_key.trim().to_string()))
}

fn prompt_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim().to_string())
}

fn read_line(reader: &mut dyn BufRead) -> Result<String> {
    let mut value = String::new();
    reader.read_line(&mut value)?;
    Ok(value.trim_end_matches(['\r', '\n']).to_string())
}

/// Alias name for `alias remove` / `list` (mc `cleanAlias` + `isValidAlias`).
fn normalize_alias(input: &str) -> Result<String> {
    let alias = input.trim_end_matches(['/', '\\']).to_string();
    if !is_valid_alias(&alias) {
        bail!("Invalid alias `{alias}`.");
    }
    Ok(alias)
}

fn normalize_url(input: &str) -> Result<String> {
    let trimmed = input.trim_end_matches('/');
    let valid = Url::parse(trimmed).is_ok_and(|parsed| {
        matches!(parsed.scheme(), "http" | "https")
            && parsed.host_str().is_some()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && (parsed.path() == "/" || parsed.path().is_empty())
    });
    if !valid {
        return Err(McError::new(format!(
            "URL `{input}` for MinIO Client should be of the form scheme://host[:port]/ without resource component."
        )))
        .context("Invalid URL.");
    }
    Ok(trimmed.to_string())
}

fn normalize_api(input: &str) -> Result<String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "s3v4" => Ok("S3v4".to_string()),
        "s3v2" => Ok("S3v2".to_string()),
        _ => Err(McError::invalid_argument())
            .context("Unrecognized API signature. Valid options are `[S3v4, S3v2]`."),
    }
}

fn normalize_path(input: &str) -> Result<String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok("auto".to_string()),
        "on" => Ok("on".to_string()),
        "off" => Ok("off".to_string()),
        _ => Err(McError::invalid_argument())
            .context("Unrecognized path value. Valid options are `[auto, on, off]`."),
    }
}

#[derive(Debug, Clone)]
struct DisplayAlias {
    alias: String,
    url: String,
    access_key: String,
    secret_key: String,
    api: String,
    path: String,
    src: String,
}

impl DisplayAlias {
    fn from_parts(alias: String, cfg: AliasConfig, src: String) -> Self {
        let anonymous = cfg.access_key.is_empty() || cfg.secret_key.is_empty();
        Self {
            alias,
            url: cfg.url,
            access_key: if anonymous {
                String::new()
            } else {
                cfg.access_key
            },
            secret_key: if anonymous {
                String::new()
            } else {
                cfg.secret_key
            },
            api: if anonymous { String::new() } else { cfg.api },
            path: cfg.path,
            src,
        }
    }

    fn as_message(&self) -> AliasMessage<'_> {
        AliasMessage {
            status: "success",
            alias: &self.alias,
            url: &self.url,
            access_key: Some(self.access_key.as_str()),
            secret_key: Some(self.secret_key.as_str()),
            api: Some(self.api.as_str()),
            path: Some(self.path.as_str()),
            src: Some(self.src.as_str()),
        }
    }
}

#[derive(Debug, Serialize)]
struct AliasMessage<'a> {
    status: &'static str,
    alias: &'a str,
    #[serde(rename = "URL")]
    url: &'a str,
    #[serde(rename = "accessKey", skip_serializing_if = "Option::is_none")]
    access_key: Option<&'a str>,
    #[serde(rename = "secretKey", skip_serializing_if = "Option::is_none")]
    secret_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    src: Option<&'a str>,
}
