use crate::cli::DiffArgs;
use crate::commands::util::object_infos;
use crate::config::ConfigStore;
use anyhow::Result;
use std::collections::BTreeMap;

pub fn run(args: DiffArgs, _json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
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
