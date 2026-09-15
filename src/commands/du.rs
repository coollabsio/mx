use crate::cli::DuArgs;
use crate::commands::util::{key_depth, object_infos};
use crate::config::ConfigStore;
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;

pub fn run(args: DuArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (_, items) = object_infos(&store, &args.target)?;
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
            println!(
                "{}",
                serde_json::to_string(&DuMessage {
                    status: "success",
                    prefix: &name,
                    size,
                    objects: count,
                })?
            );
        } else {
            let label = if name.is_empty() { "." } else { name.as_str() };
            println!("{size}  {count} objects  {label}");
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
}
