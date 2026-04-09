use crate::cli::{AliasCommand, AliasListArgs, AliasRemoveArgs, AliasSetArgs};
use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use anyhow::{Result, bail};
use serde::Serialize;
use std::io::{self, BufRead, IsTerminal, Write};
use tabwriter::TabWriter;
use url::Url;

pub fn run(command: AliasCommand, json: bool) -> Result<()> {
    match command {
        AliasCommand::Set(args) => set(args, json),
        AliasCommand::List(args) => list(args, json),
        AliasCommand::Remove(args) => remove(args, json),
    }
}

fn set(args: AliasSetArgs, json: bool) -> Result<()> {
    let alias = normalize_alias(&args.alias)?;
    let url = normalize_url(&args.url)?;
    let api = normalize_api(&args.api)?;
    let path = normalize_path(&args.path)?;
    let (access_key, secret_key) = collect_credentials(args.access_key, args.secret_key)?;

    if access_key.is_empty() {
        bail!("Invalid access key `{access_key}`.");
    }
    if secret_key.is_empty() {
        bail!("Invalid secret key `{secret_key}`.");
    }

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
            url: Some(url.as_str()),
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

    let mut rows = Vec::new();
    for (alias, cfg) in &store.config().aliases {
        rows.push(DisplayAlias::from_parts(
            alias.clone(),
            cfg.clone(),
            src.clone(),
        ));
    }

    if let Some(alias) = args.alias {
        let alias = normalize_alias(&alias)?;
        let row = rows
            .into_iter()
            .find(|row| row.alias == alias)
            .ok_or_else(|| anyhow::anyhow!("No such alias `{alias}` found."))?;
        print_rows(&[row], json)?;
        return Ok(());
    }

    print_rows(&rows, json)
}

fn remove(args: AliasRemoveArgs, json: bool) -> Result<()> {
    let alias = normalize_alias(&args.alias)?;
    let mut store = ConfigStore::load_or_create()?;

    if store.config_mut().aliases.remove(&alias).is_none() {
        bail!("No such alias `{alias}` found.");
    }

    store.save()?;
    print_message(
        &AliasMessage {
            status: "success",
            alias: &alias,
            url: None,
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

fn print_rows(rows: &[DisplayAlias], json: bool) -> Result<()> {
    if json {
        for row in rows {
            print_message(&row.as_message(), "", true)?;
        }
        return Ok(());
    }

    let mut writer = TabWriter::new(io::stdout()).padding(2);
    writeln!(writer, "Alias\tURL\tAccessKey\tSecretKey\tAPI\tPath\tSrc")?;
    for row in rows {
        writeln!(
            writer,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.alias, row.url, row.access_key, row.secret_key, row.api, row.path, row.src
        )?;
    }
    writer.flush()?;
    Ok(())
}

fn print_message(message: &AliasMessage<'_>, plain: &str, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(message)?);
    } else if !plain.is_empty() {
        println!("{plain}");
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

fn normalize_alias(input: &str) -> Result<String> {
    let alias = input.trim_end_matches(['/', '\\']).to_string();
    let valid = !alias.is_empty()
        && alias.chars().enumerate().all(|(idx, ch)| match idx {
            0 => ch.is_ascii_alphabetic(),
            _ => ch.is_ascii_alphanumeric() || ch == '-' || ch == '_',
        });
    if !valid {
        bail!("Invalid alias `{alias}`.");
    }
    Ok(alias)
}

fn normalize_url(input: &str) -> Result<String> {
    let trimmed = input.trim_end_matches('/');
    let parsed = Url::parse(trimmed)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("Invalid URL.");
    }
    if parsed.host_str().is_none() || parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("Invalid URL.");
    }
    if parsed.path() != "/" && !parsed.path().is_empty() {
        bail!("Invalid URL.");
    }
    Ok(trimmed.to_string())
}

fn normalize_api(input: &str) -> Result<String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "s3v4" => Ok("S3v4".to_string()),
        "s3v2" => Ok("S3v2".to_string()),
        _ => bail!("Unrecognized API signature. Valid options are `[S3v4, S3v2]`."),
    }
}

fn normalize_path(input: &str) -> Result<String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok("auto".to_string()),
        "on" => Ok("on".to_string()),
        "off" => Ok("off".to_string()),
        _ => bail!("Unrecognized path value. Valid options are `[auto, on, off]`."),
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
            url: Some(self.url.as_str()),
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
    #[serde(rename = "URL", skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
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
