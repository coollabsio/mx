use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEntry {
    pub path: PathBuf,
    pub relative: String,
    pub size: u64,
}

pub fn local_inventory(root: &Path) -> Result<Vec<LocalEntry>> {
    let mut entries = Vec::new();
    collect_local(root, root, &mut entries)?;
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(entries)
}

pub fn join_key(prefix: Option<&str>, relative: &str) -> String {
    let prefix = prefix.unwrap_or_default().trim_matches('/');
    if prefix.is_empty() {
        relative.to_string()
    } else {
        format!("{prefix}/{}", relative.trim_start_matches('/'))
    }
}

fn collect_local(root: &Path, directory: &Path, entries: &mut Vec<LocalEntry>) -> Result<()> {
    for item in std::fs::read_dir(directory)
        .with_context(|| format!("Unable to read directory `{}`.", directory.display()))?
    {
        let item = item?;
        let file_type = item.file_type()?;
        let path = item.path();
        if file_type.is_dir() {
            collect_local(root, &path, entries)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            entries.push(LocalEntry {
                size: item.metadata()?.len(),
                path,
                relative,
            });
        }
    }
    Ok(())
}
