use crate::commands::util::{TargetKind, object_infos, stat_target};
use crate::config::ConfigStore;
use crate::error::McError;
use anyhow::{Context, Result};
use clap::Args;
use std::collections::BTreeMap;

#[derive(Debug, Args)]
pub struct DiffArgs {
    pub source: String,
    pub target: String,
}

pub fn run(args: DiffArgs, _json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    for input in [&args.source, &args.target] {
        let kind =
            stat_target(&store, input).with_context(|| format!("Unable to stat '{input}'."))?;
        if kind != TargetKind::Folder {
            return Err(McError::invalid_argument())
                .with_context(|| format!("`{input}` is not a folder."));
        }
    }
    let (_, source) = object_infos(&store, &args.source)?;
    let (_, target) = object_infos(&store, &args.target)?;
    let source_map: BTreeMap<_, _> = source
        .into_iter()
        .map(|item| (item.key, item.size))
        .collect();
    let target_map: BTreeMap<_, _> = target
        .into_iter()
        .map(|item| (item.key, item.size))
        .collect();

    let mut names = source_map.keys().cloned().collect::<Vec<_>>();
    names.extend(target_map.keys().cloned());
    names.sort();
    names.dedup();

    for name in names {
        match (source_map.get(&name), target_map.get(&name)) {
            (Some(_), None) => println!("only in {}: {name}", args.source),
            (None, Some(_)) => println!("only in {}: {name}", args.target),
            (Some(left), Some(right)) if left != right => {
                println!("size differs: {name} ({left} vs {right})")
            }
            _ => {}
        }
    }
    Ok(())
}
