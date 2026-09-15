use crate::cli::TreeArgs;
use crate::commands::util::{key_depth, object_infos};
use crate::config::ConfigStore;
use anyhow::Result;
use std::collections::BTreeSet;

pub fn run(args: TreeArgs, _json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (_, items) = object_infos(&store, &args.target)?;
    let mut nodes = BTreeSet::new();
    for item in items {
        let parts: Vec<_> = item
            .key
            .trim_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        let limit = args.depth.unwrap_or(usize::MAX);
        let max = parts.len().min(limit);
        for index in 1..=max {
            let is_file = index == parts.len();
            if is_file && !args.files {
                continue;
            }
            if key_depth(&parts[..index].join("/")) > limit {
                continue;
            }
            nodes.insert((index, parts[..index].join("/"), is_file));
        }
    }

    println!("{}", args.target.trim_end_matches('/'));
    for (depth, path, is_file) in nodes {
        let name = path.rsplit('/').next().unwrap_or(&path);
        let indent = "  ".repeat(depth);
        if is_file {
            println!("{indent}{name}");
        } else {
            println!("{indent}{name}/");
        }
    }
    Ok(())
}
