use crate::commands::util::key_depth;
use crate::config::ConfigStore;
use crate::flags::RewindFlag;
use anyhow::{Context, Result};
use clap::Args;
use std::collections::BTreeSet;

#[derive(Debug, Args)]
#[command(mut_args(|a| if a.get_id().as_str() == "rewind" {
    a.help("display tree no later than specified date")
} else {
    a
}))]
pub struct TreeArgs {
    /// includes files in tree
    #[arg(short = 'f', long)]
    pub files: bool,
    /// sets the depth threshold (default: -1)
    #[arg(short = 'd', long)]
    pub depth: Option<usize>,
    #[command(flatten)]
    pub rewind: RewindFlag,
    pub target: String,
}

pub fn run(args: TreeArgs, _json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    super::util::stat_target(&store, &args.target)
        .with_context(|| format!("Unable to tree `{}`.", args.target))?;
    let items = super::du::listing(&store, &args.target, false, &args.rewind)
        .context(crate::error::nonfatal("Unable to tree."))?;
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
