use crate::commands::util::{key_depth, object_infos};
use crate::config::ConfigStore;
use crate::flags::RewindFlag;
use crate::location::{Location, parse_location};
use crate::s3::{ListOptions, ObjectInfo};
use anyhow::{Context, Result, bail};
use clap::Args;
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::SystemTime;

#[derive(Debug, Args)]
#[command(mut_args(|a| if a.get_id().as_str() == "rewind" {
    a.help("include all object versions no later than specified date")
} else {
    a
}))]
pub struct DuArgs {
    /// print the total for a folder prefix only if it is N or fewer levels below the command line argument (default: 0)
    #[arg(short = 'd', long)]
    pub depth: Option<usize>,
    /// recursively print the total for a folder prefix
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub rewind: RewindFlag,
    /// include all object versions
    #[arg(long)]
    pub versions: bool,
    pub target: String,
}

/// Recursive listing for du/tree. With `--versions` / `--rewind` (S3 only) the result holds
/// every version (at or before the rewind time) or the objects as of the rewind time; delete
/// markers are never included.
pub(crate) fn listing(
    store: &ConfigStore,
    input: &str,
    versions: bool,
    rewind: &RewindFlag,
) -> Result<Vec<ObjectInfo>> {
    let rewind = rewind.at(SystemTime::now())?;
    if !versions && rewind.is_none() {
        return Ok(object_infos(store, input)?.1);
    }
    let Location::S3(target) = parse_location(input, store.config()) else {
        bail!("--versions and --rewind are only supported for S3 targets, not `{input}`.");
    };
    let alias = super::alias_config(store, &target.alias)?;
    let bucket = target.require_bucket()?.to_string();
    let prefix = target.key_with_trailing_slash();
    let options = ListOptions {
        recursive: true,
        versions,
        rewind,
        ..Default::default()
    };
    let items = super::runtime()?.block_on(async {
        let client = crate::s3::build_client(&alias).await?;
        crate::s3::list_objects_with(&client, &bucket, prefix.as_deref(), &options).await
    })?;
    Ok(items
        .into_iter()
        .filter(|item| !item.is_delete_marker && !item.is_prefix)
        .collect())
}

pub fn run(args: DuArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let input = &args.target;
    // mc `isAliasURLDir`: only folders (and paths ending in `/`) can be summarized.
    let is_dir = input.ends_with('/')
        || matches!(
            super::util::stat_target(&store, input),
            Ok(super::util::TargetKind::Folder)
        )
        || matches!(parse_location(input, store.config()), Location::S3(t) if t.key.is_none());
    if !is_dir {
        return Err(crate::error::McError::invalid_argument()).with_context(|| {
            format!("Source `{input}` is not a folder. Only folders are supported by 'du' command.")
        });
    }
    let items = listing(&store, input, args.versions, &args.rewind).with_context(|| {
        crate::error::nonfatal(format!(
            "Failed to find disk usage of `{input}` recursively."
        ))
    })?;
    let depth = args
        .depth
        .unwrap_or(if args.recursive { usize::MAX } else { 1 });
    let mut totals: BTreeMap<String, (i64, u64)> = BTreeMap::new();

    for item in items {
        let prefix = prefix_for_depth(&item.key, depth);
        let entry = totals.entry(prefix).or_insert((0, 0));
        entry.0 += item.size;
        entry.1 += 1;
    }

    if totals.is_empty() {
        totals.insert(args.target.clone(), (0, 0));
    }

    for (name, (size, count)) in totals {
        if json {
            crate::output::print_json(&DuMessage {
                status: "success",
                prefix: &name,
                size,
                objects: count,
                is_versions: args.versions,
            })?;
        } else {
            let label = if name.is_empty() { "." } else { name.as_str() };
            let unit = if args.versions { "versions" } else { "objects" };
            println!("{size}  {count} {unit}  {label}");
        }
    }
    Ok(())
}

fn prefix_for_depth(key: &str, depth: usize) -> String {
    if depth == usize::MAX {
        return String::new();
    }
    let parts: Vec<_> = key
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    let take = depth
        .min(key_depth(key).saturating_sub(1).max(1))
        .min(parts.len());
    parts[..take].join("/")
}

#[derive(Debug, Serialize)]
struct DuMessage<'a> {
    status: &'static str,
    prefix: &'a str,
    size: i64,
    objects: u64,
    #[serde(rename = "isVersions")]
    is_versions: bool,
}
