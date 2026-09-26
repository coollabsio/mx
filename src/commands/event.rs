//! `mx event add|rm|ls` (area G): bucket notifications.

use crate::commands::retention::print_json;
use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use crate::s3::notify::{self, EventConfig};
use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;

const DEFAULT_EVENTS: &str = "put,delete,get";

#[derive(Debug, Args)]
pub struct EventArgs {
    #[command(subcommand)]
    pub command: EventCommand,
}

#[derive(Debug, Subcommand)]
pub enum EventCommand {
    #[command(about = "add a new bucket notification")]
    Add(EventAddArgs),
    #[command(
        visible_alias = "remove",
        about = "remove a bucket notification; '--force' removes all bucket notifications"
    )]
    Rm(EventRmArgs),
    #[command(visible_alias = "list", about = "list bucket notifications")]
    Ls(EventLsArgs),
}

#[derive(Debug, Args)]
pub struct EventAddArgs {
    pub target: String,
    pub arn: String,
    /// filter specific type of events: put, delete, get, replica, ilm, scanner
    #[arg(long, default_value = DEFAULT_EVENTS)]
    pub event: String,
    /// filter event associated to the specified prefix
    #[arg(long)]
    pub prefix: Option<String>,
    /// filter event associated to the specified suffix
    #[arg(long)]
    pub suffix: Option<String>,
    /// ignore if event already exists
    #[arg(short = 'p', long)]
    pub ignore_existing: bool,
}

#[derive(Debug, Args)]
pub struct EventRmArgs {
    pub target: String,
    pub arn: Option<String>,
    /// force removing all bucket notifications
    #[arg(long)]
    pub force: bool,
    /// only remove the configuration with these events (default put,delete,get when --prefix
    /// or --suffix is given)
    #[arg(long)]
    pub event: Option<String>,
    #[arg(long)]
    pub prefix: Option<String>,
    #[arg(long)]
    pub suffix: Option<String>,
}

#[derive(Debug, Args)]
pub struct EventLsArgs {
    pub target: String,
    pub arn: Option<String>,
}

pub fn run(args: EventArgs, json: bool) -> Result<()> {
    match args.command {
        EventCommand::Add(args) => add(args, json),
        EventCommand::Rm(args) => remove(args, json),
        EventCommand::Ls(args) => list(args, json),
    }
}

fn bucket_target(input: &str) -> Result<(AliasConfig, String)> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, input)?;
    let bucket = target.require_bucket()?.to_string();
    if target.key.is_some() {
        bail!("Target `{input}` must be a bucket (ALIAS/BUCKET).");
    }
    Ok((alias, bucket))
}

#[derive(Serialize)]
struct EventAddMessage<'a> {
    arn: &'a str,
    event: Vec<&'a str>,
    prefix: &'a str,
    suffix: &'a str,
    status: &'static str,
}

fn add(args: EventAddArgs, json: bool) -> Result<()> {
    let events = notify::parse_events(&args.event)?;
    notify::ArnKind::from_arn(&args.arn)?;
    let (alias, bucket) = bucket_target(&args.target)?;
    let prefix = args.prefix.as_deref().unwrap_or_default();
    let suffix = args.suffix.as_deref().unwrap_or_default();
    runtime()?
        .block_on(async {
            let client = crate::s3::build_client(&alias).await?;
            let mut entries = notify::get_bucket_notification(&client, &bucket).await?;
            let changed = notify::add_entry(
                &mut entries,
                &args.arn,
                events,
                prefix,
                suffix,
                args.ignore_existing,
            )?;
            if !changed {
                return Ok(());
            }
            match notify::put_bucket_notification(&client, &bucket, &entries).await {
                Err(error)
                    if args.ignore_existing
                        && format!("{error:#}")
                            .to_ascii_lowercase()
                            .contains("overlapping") =>
                {
                    Ok(())
                }
                result => result,
            }
        })
        .map_err(|error| error.context("Unable to enable notification on the specified bucket"))?;

    if json {
        print_json(&EventAddMessage {
            arn: &args.arn,
            event: args.event.split(',').map(str::trim).collect(),
            prefix,
            suffix,
            status: "success",
        })
    } else {
        println!("Successfully added {}", args.arn);
        Ok(())
    }
}

#[derive(Serialize)]
struct EventRemoveMessage<'a> {
    arn: &'a str,
    status: &'static str,
}

fn remove(args: EventRmArgs, json: bool) -> Result<()> {
    if args.arn.is_none() && !args.force {
        bail!("--force flag needs to be passed to remove all bucket notifications.");
    }
    if args.arn.is_none()
        && (args.event.is_some() || args.prefix.is_some() || args.suffix.is_some())
    {
        bail!("--event, --prefix and --suffix require an ARN.");
    }
    let filter_events = if args.event.is_some() || args.prefix.is_some() || args.suffix.is_some() {
        Some(notify::parse_events(
            args.event.as_deref().unwrap_or(DEFAULT_EVENTS),
        )?)
    } else {
        None
    };
    if let Some(arn) = &args.arn {
        notify::ArnKind::from_arn(arn)?;
    }
    let (alias, bucket) = bucket_target(&args.target)?;
    let prefix = args.prefix.as_deref().unwrap_or_default();
    let suffix = args.suffix.as_deref().unwrap_or_default();
    runtime()?
        .block_on(async {
            let client = crate::s3::build_client(&alias).await?;
            let entries = match &args.arn {
                None => Vec::new(),
                Some(arn) => {
                    let mut entries = notify::get_bucket_notification(&client, &bucket).await?;
                    let filter = filter_events
                        .as_deref()
                        .map(|events| (events, prefix, suffix));
                    notify::remove_entries(&mut entries, arn, filter)?;
                    entries
                }
            };
            notify::put_bucket_notification(&client, &bucket, &entries).await
        })
        .map_err(|error| error.context("Unable to disable notification on the specified bucket"))?;

    let arn = args.arn.as_deref().unwrap_or_default();
    if json {
        print_json(&EventRemoveMessage {
            arn,
            status: "success",
        })
    } else {
        println!("Successfully removed {arn}");
        Ok(())
    }
}

#[derive(Serialize)]
struct EventListMessage<'a> {
    status: &'static str,
    id: &'a str,
    event: &'a [String],
    prefix: &'a str,
    suffix: &'a str,
    arn: &'a str,
}

fn list_line(item: &EventConfig) -> String {
    let mut line = format!("{}   {}   Filter: ", item.arn, item.events.join(","));
    if !item.prefix.is_empty() {
        line.push_str(&format!("prefix=\"{}\"", item.prefix));
    }
    if !item.suffix.is_empty() {
        line.push_str(&format!("suffix=\"{}\"", item.suffix));
    }
    line
}

fn list(args: EventLsArgs, json: bool) -> Result<()> {
    let (alias, bucket) = bucket_target(&args.target)?;
    let entries = runtime()?
        .block_on(async {
            let client = crate::s3::build_client(&alias).await?;
            notify::get_bucket_notification(&client, &bucket).await
        })
        .map_err(|error| error.context("Unable to list notifications on the specified bucket"))?;
    for item in entries
        .iter()
        .filter(|item| args.arn.as_ref().is_none_or(|arn| &item.arn == arn))
    {
        if json {
            print_json(&EventListMessage {
                status: "success",
                id: item.id.as_deref().unwrap_or_default(),
                event: &item.events,
                prefix: &item.prefix,
                suffix: &item.suffix,
                arn: &item.arn,
            })?;
        } else {
            println!("{}", list_line(item));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::notify::ArnKind;

    #[test]
    fn formats_list_lines_like_mc() {
        let item = EventConfig {
            id: Some("1".into()),
            kind: ArnKind::Queue,
            arn: "arn:minio:sqs::MXTEST:webhook".into(),
            events: vec!["s3:ObjectCreated:*".into(), "s3:ObjectRemoved:*".into()],
            prefix: "photos/".into(),
            suffix: ".jpg".into(),
        };
        assert_eq!(
            list_line(&item),
            "arn:minio:sqs::MXTEST:webhook   s3:ObjectCreated:*,s3:ObjectRemoved:*   Filter: prefix=\"photos/\"suffix=\".jpg\""
        );
    }
}
