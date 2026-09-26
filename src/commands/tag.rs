//! `tag set|list|remove` (mc tag) for buckets, objects, versions, and prefixes.

use crate::commands::runtime;
use crate::commands::util::{parse_tags, require_s3};
use crate::config::ConfigStore;
use crate::config::model::AliasConfig;
use crate::flags::{RewindFlag, VersionIdFlag, VersionsFlag};
use crate::output;
use crate::s3::ListOptions;
use anyhow::{Context, Result, anyhow, bail};
use aws_sdk_s3::Client;
use clap::{Args, Subcommand};
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::SystemTime;

#[derive(Debug, Args)]
pub struct TagArgs {
    #[command(subcommand)]
    pub command: TagCommand,
}

#[derive(Debug, Subcommand)]
pub enum TagCommand {
    #[command(about = "set tags for a bucket and object(s)")]
    Set(TagSetArgs),
    #[command(about = "list tags of a bucket or an object")]
    List(TagSelectArgs),
    #[command(about = "remove tags assigned to a bucket or an object")]
    Remove(TagSelectArgs),
}

/// Object selection flags shared by all tag subcommands.
#[derive(Debug, Args)]
pub struct TagSelection {
    #[command(flatten)]
    pub version: VersionIdFlag,
    #[command(flatten)]
    pub rewind: RewindFlag,
    #[command(flatten)]
    pub versions: VersionsFlag,
    /// apply to all objects under the prefix recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
}

#[derive(Debug, Args)]
pub struct TagSetArgs {
    #[command(flatten)]
    pub select: TagSelection,
    /// exclude setting tags on folder objects (requires --recursive)
    #[arg(long)]
    pub exclude_folders: bool,
    pub target: String,
    /// tags, e.g. "key1=value1&key2=value2"
    pub tags: String,
}

#[derive(Debug, Args)]
pub struct TagSelectArgs {
    #[command(flatten)]
    pub select: TagSelection,
    pub target: String,
}

pub fn run(command: TagCommand, json: bool) -> Result<()> {
    match command {
        TagCommand::Set(args) => {
            validate(&args.select, true)?;
            if args.exclude_folders && !args.select.recursive {
                bail!("'--exclude-folders' must be used with --recursive only");
            }
            let tags = parse_tags(&args.tags)?;
            for_each_target(&args.target, &args.select, args.exclude_folders, |ctx| {
                ctx.rt.block_on(crate::s3::put_tags(
                    ctx.client,
                    ctx.bucket,
                    ctx.key,
                    ctx.version_id,
                    &tags,
                ))?;
                print_result("Tags set for", &ctx.name, ctx.version_id, json)
            })
        }
        TagCommand::List(args) => {
            validate(&args.select, false)?;
            for_each_target(&args.target, &args.select, false, |ctx| {
                let tags = ctx
                    .rt
                    .block_on(crate::s3::get_tags(
                        ctx.client,
                        ctx.bucket,
                        ctx.key,
                        ctx.version_id,
                    ))?
                    .ok_or_else(|| {
                        anyhow!(
                            "No tags found for {}: check 'mx tag set --help' on how to set tags",
                            display_name(&ctx.name, ctx.version_id)
                        )
                    })?;
                print_tags(&ctx.name, ctx.version_id, tags, json)
            })
        }
        TagCommand::Remove(args) => {
            validate(&args.select, true)?;
            for_each_target(&args.target, &args.select, false, |ctx| {
                ctx.rt.block_on(crate::s3::delete_tags(
                    ctx.client,
                    ctx.bucket,
                    ctx.key,
                    ctx.version_id,
                ))?;
                print_result("Tags removed for", &ctx.name, ctx.version_id, json)
            })
        }
    }
}

/// mc rejects `--version-id` together with `--rewind` (and, for set/remove, `--versions`).
fn validate(select: &TagSelection, reject_versions: bool) -> Result<()> {
    if select.version.version_id.is_some() {
        if reject_versions && (select.rewind.rewind.is_some() || select.versions.versions) {
            bail!(
                "You cannot specify both --version-id and --rewind or --versions flags at the same time"
            );
        }
        if select.rewind.rewind.is_some() {
            bail!("You cannot specify both --version-id and --rewind flags at the same time");
        }
    }
    Ok(())
}

/// One object (version) or bucket to operate on.
struct TagTarget<'a> {
    rt: &'a tokio::runtime::Runtime,
    client: &'a Client,
    bucket: &'a str,
    key: Option<&'a str>,
    version_id: Option<&'a str>,
    /// Endpoint URL of the bucket/object (mc prints this).
    name: String,
}

fn object_name(alias: &AliasConfig, bucket: &str, key: Option<&str>) -> Result<String> {
    let url = crate::s3::bucket_url(alias, bucket)?;
    Ok(match key {
        Some(key) => format!("{url}{key}"),
        None => url.trim_end_matches('/').to_string(),
    })
}

fn for_each_target(
    target_arg: &str,
    select: &TagSelection,
    exclude_folders: bool,
    mut action: impl FnMut(TagTarget<'_>) -> Result<()>,
) -> Result<()> {
    let now = SystemTime::now();
    let rewind = select.rewind.at(now)?;
    let versions = select.versions.versions;
    let time_ref = rewind.or(versions.then_some(now));
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, target_arg)?;
    let bucket = target.require_bucket()?.to_string();
    let rt = runtime()?;
    let client = rt.block_on(crate::s3::build_client(&alias))?;
    let key = target.key_with_trailing_slash();

    if time_ref.is_none() && !select.recursive {
        return action(TagTarget {
            rt: &rt,
            client: &client,
            bucket: &bucket,
            key: key.as_deref(),
            version_id: select.version.version_id.as_deref(),
            name: object_name(&alias, &bucket, key.as_deref())?,
        });
    }

    // (key, version id) pairs selected by --rewind/--versions/--recursive.
    let mut selected: Vec<(String, Option<String>)> = Vec::new();
    if select.recursive {
        let items = rt.block_on(crate::s3::list_objects_with(
            &client,
            &bucket,
            key.as_deref(),
            &ListOptions {
                recursive: true,
                versions,
                rewind,
                ..Default::default()
            },
        ))?;
        let base = key.clone().unwrap_or_default();
        for item in items {
            if item.is_prefix || item.is_delete_marker {
                continue;
            }
            if versions && item.last_modified.zip(time_ref).is_some_and(|(m, t)| m > t) {
                continue;
            }
            let full = crate::s3::full_key(&base, &item.key);
            if exclude_folders && full.find('/').is_some_and(|index| index > 0) {
                continue;
            }
            selected.push((full, item.version_id));
        }
    } else {
        let key = key
            .clone()
            .ok_or_else(|| anyhow!("--rewind/--versions need an object target (or --recursive)"))?;
        let items = rt
            .block_on(crate::s3::list_key_versions(&client, &bucket, &key))
            .with_context(|| format!("Unable to list target {target_arg}"))?;
        let items = match rewind {
            Some(at) if !versions => crate::s3::resolve_rewind(items, at),
            _ => items,
        };
        for item in items {
            if item.is_delete_marker || item.last_modified.zip(time_ref).is_some_and(|(m, t)| m > t)
            {
                continue;
            }
            selected.push((item.key, item.version_id));
        }
    }
    for (key, version_id) in &selected {
        action(TagTarget {
            rt: &rt,
            client: &client,
            bucket: &bucket,
            key: Some(key),
            version_id: version_id.as_deref(),
            name: object_name(&alias, &bucket, Some(key))?,
        })?;
    }
    Ok(())
}

fn display_name(name: &str, version_id: Option<&str>) -> String {
    match version_id {
        Some(version) if !version.is_empty() => format!("{name} ({version})"),
        _ => name.to_string(),
    }
}

#[derive(Serialize)]
struct TagResultMessage<'a> {
    status: &'a str,
    name: &'a str,
    #[serde(rename = "versionID")]
    version_id: &'a str,
}

fn print_result(action: &str, name: &str, version_id: Option<&str>, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string(&TagResultMessage {
                status: "success",
                name,
                version_id: version_id.unwrap_or_default(),
            })?
        );
    } else {
        output::print_plain(&format!("{action} {}.", display_name(name, version_id)));
    }
    Ok(())
}

#[derive(Serialize)]
struct TagListMessage<'a> {
    #[serde(rename = "tagset", skip_serializing_if = "BTreeMap::is_empty")]
    tags: BTreeMap<String, String>,
    status: &'a str,
    url: &'a str,
    #[serde(rename = "versionID")]
    version_id: &'a str,
}

fn print_tags(
    name: &str,
    version_id: Option<&str>,
    tags: Vec<(String, String)>,
    json: bool,
) -> Result<()> {
    let tags = tags.into_iter().collect::<BTreeMap<_, _>>();
    if json {
        println!(
            "{}",
            serde_json::to_string(&TagListMessage {
                tags,
                status: "success",
                url: name,
                version_id: version_id.unwrap_or_default(),
            })?
        );
        return Ok(());
    }
    println!("{}", format_tags(&display_name(name, version_id), &tags));
    Ok(())
}

/// mc `tagListMessage.String()`: `Name : <url>` then aligned `key : value` lines.
pub fn format_tags(name: &str, tags: &BTreeMap<String, String>) -> String {
    let width = tags.keys().map(String::len).max().unwrap_or(0).max(4) + 2;
    let mut lines = vec![format!("Name{:>pad$} {name}", ":", pad = width - 4)];
    for (key, value) in tags {
        lines.push(format!(
            "{key}{:>pad$} {value}",
            ":",
            pad = width - key.len()
        ));
    }
    if tags.is_empty() {
        lines.push("No tags found".to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::format_tags;
    use std::collections::BTreeMap;

    #[test]
    fn formats_tags_like_mc() {
        let tags = BTreeMap::from([
            ("env".to_string(), "prod".to_string()),
            ("team-name".to_string(), "x".to_string()),
        ]);
        assert_eq!(
            format_tags("http://h/b/o", &tags),
            "Name      : http://h/b/o\nenv       : prod\nteam-name : x"
        );
        assert_eq!(
            format_tags("n", &BTreeMap::new()),
            "Name : n\nNo tags found"
        );
    }
}
