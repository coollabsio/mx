use crate::commands::ls::{ListOpts, list, print_listing};
use crate::config::ConfigStore;
use crate::error::nonfatal;
use crate::flags::RewindFlag;
use anyhow::{Context, Result};
use clap::Args;
use std::time::SystemTime;

const TREE_ENTRY: &str = "├─ ";
const TREE_LAST_ENTRY: &str = "└─ ";
const TREE_NEXT: &str = "│";
const TREE_LEVEL: &str = "  ";

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
    /// sets the depth threshold
    #[arg(
        short = 'd',
        long,
        default_value_t = -1,
        allow_negative_numbers = true,
        value_name = "value"
    )]
    pub depth: i64,
    #[command(flatten)]
    pub rewind: RewindFlag,
    /// targets to show (default: current folder)
    #[arg(value_name = "TARGET")]
    pub targets: Vec<String>,
}

pub fn run(args: TreeArgs, json: bool) -> Result<()> {
    if args.depth < -1 || args.depth == 0 {
        return Err(anyhow::Error::new(crate::error::McError::invalid_argument()).context(
            "please set a proper depth, for example: '--depth 1' to limit the tree output, default (-1) output displays everything",
        ));
    }
    let store = ConfigStore::load_or_create()?;
    let rewind = args.rewind.at(SystemTime::now())?;
    for target in &args.targets {
        super::util::stat_target(&store, target)
            .with_context(|| format!("Unable to tree `{target}`."))?;
    }
    let targets = if args.targets.is_empty() {
        vec![".".to_string()]
    } else {
        args.targets.clone()
    };
    let rt = super::runtime()?;
    let mut failed = false;
    for target in &targets {
        if json {
            // mc prints the recursive `ls` JSON of the folder.
            let opts = ListOpts {
                recursive: true,
                rewind,
                ..Default::default()
            };
            match list(&store, &rt, &with_slash(target), &opts) {
                Ok(listing) => {
                    print_listing(&listing, Some("*"), true)?;
                }
                Err(error) => {
                    crate::output::print_error(&error.context(nonfatal("Unable to list folder.")));
                    failed = true;
                }
            }
            continue;
        }
        let tree = Tree {
            store: &store,
            rt: &rt,
            rewind,
            depth: args.depth,
            files: args.files,
        };
        tree.show(target, 1, "")?;
    }
    if failed {
        return Err(crate::output::Exit(1).into());
    }
    Ok(())
}

fn with_slash(target: &str) -> String {
    if target.ends_with('/') {
        target.to_string()
    } else {
        format!("{target}/")
    }
}

struct Tree<'a> {
    store: &'a ConfigStore,
    rt: &'a tokio::runtime::Runtime,
    rewind: Option<SystemTime>,
    depth: i64,
    files: bool,
}

impl Tree<'_> {
    /// mc `doTree`: prints the entries of `url` (the target itself first at level 1).
    fn show(&self, url: &str, level: i64, branch: &str) -> Result<()> {
        let dir = with_slash(url);
        let opts = ListOpts {
            rewind: self.rewind,
            ..Default::default()
        };
        let listing = match list(self.store, self.rt, &dir, &opts) {
            Ok(listing) => listing,
            Err(error) => {
                crate::output::print_error(&error.context(nonfatal("Unable to tree.")));
                return Ok(());
            }
        };
        let entries: Vec<_> = listing
            .contents
            .iter()
            .filter(|content| self.files || content.is_dir)
            .collect();
        for (index, content) in entries.iter().enumerate() {
            if level == 1 && index == 0 {
                println!("{branch}{url}");
            }
            let current = branch_string(branch, level, index + 1 == entries.len());
            if !content.is_dir {
                println!("{current}{}", content.key);
                continue;
            }
            let next = format!("{dir}{}", content.key);
            if next == url {
                continue;
            }
            println!("{current}{}", content.key.trim_end_matches('/'));
            if self.depth == -1 || level <= self.depth {
                self.show(&next, level + 1, &current)?;
            }
        }
        Ok(())
    }
}

/// Branch prefix of one entry, derived from the parent's prefix like mc.
fn branch_string(parent: &str, level: i64, end: bool) -> String {
    let closed = parent.ends_with(TREE_LAST_ENTRY);
    let mut current = parent
        .strip_suffix(if closed { TREE_LAST_ENTRY } else { TREE_ENTRY })
        .unwrap_or(parent)
        .to_string();
    if level != 1 {
        if closed {
            current.push(' ');
        } else {
            current.push_str(TREE_NEXT);
        }
        current.push_str(TREE_LEVEL);
    }
    current.push_str(if end { TREE_LAST_ENTRY } else { TREE_ENTRY });
    current
}

#[cfg(test)]
mod tests {
    use super::branch_string;

    #[test]
    fn branches_match_mc() {
        assert_eq!(branch_string("", 1, false), "├─ ");
        assert_eq!(branch_string("", 1, true), "└─ ");
        assert_eq!(branch_string("├─ ", 2, true), "│  └─ ");
        assert_eq!(branch_string("└─ ", 2, false), "   ├─ ");
        assert_eq!(branch_string("│  └─ ", 3, true), "│     └─ ");
    }
}
