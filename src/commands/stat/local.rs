//! `stat` on local paths (mc `statURL` with an `fsClient`): mc's listing rules, names
//! relative to the target's folder, and the metadata mc reports for files (`Content-Type`,
//! `X-Amz-Meta-Mc-Attrs`, extended attributes). S3-only flags (`--versions`, `--rewind`,
//! `--version-id`, `--enc-c`) are accepted and have no effect, like mc.

use super::{StatArgs, StatJson, human_bytes, print_date};
use crate::error::{McError, nonfatal};
use crate::local_fs::{self, ErrorKind, Listed, PART_SUFFIX};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

/// One stat'ed local file or folder.
struct LocalStat {
    name: String,
    is_dir: bool,
    size: i64,
    time: SystemTime,
    metadata: BTreeMap<String, String>,
}

impl LocalStat {
    /// Stats `path` (symlinks followed); `name` is what mc prints.
    fn new(path: &str, mut name: String) -> std::result::Result<Self, McError> {
        let meta = std::fs::metadata(path)
            .map_err(|error| local_fs::client_error("stat", path, &error).0)?;
        if meta.is_dir() && !name.ends_with('/') {
            name.push('/');
        }
        Ok(Self {
            name,
            is_dir: meta.is_dir(),
            size: meta.len() as i64,
            time: meta.modified().unwrap_or(UNIX_EPOCH),
            metadata: local_fs::stat_metadata(path),
        })
    }

    /// mc `statMessage.String()` (folders have no `Size` line).
    fn text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{:<10}: {}", "Name", self.name);
        let _ = writeln!(out, "{:<10}: {} ", "Date", print_date(self.time));
        if !self.is_dir {
            let size = human_bytes(self.size.max(0) as u64);
            let _ = writeln!(out, "{:<10}: {size:<6} ", "Size");
        }
        let _ = writeln!(out, "{:<10}: {} ", "Type", self.kind());
        if !self.metadata.is_empty() {
            let width = self.metadata.keys().map(String::len).max().unwrap_or(0);
            let _ = writeln!(out, "{:<10}:", "Metadata");
            for (key, value) in &self.metadata {
                let _ = writeln!(out, "  {key:<width$}: {value} ");
            }
        }
        out
    }

    fn kind(&self) -> &'static str {
        if self.is_dir { "folder" } else { "file" }
    }

    fn print(&self, json: bool) -> Result<()> {
        if !json {
            println!("{}", self.text());
            return Ok(());
        }
        crate::output::print_json(&StatJson {
            status: "success",
            name: self.name.clone(),
            last_modified: crate::commands::ls::go_time(self.time),
            size: self.size,
            kind: self.kind(),
            metadata: Some(self.metadata.clone()),
            ..Default::default()
        })
    }
}

/// mc `parseAndCheckStatSyntax` with `--verbose`: a target without a path after the "alias"
/// (`dir`, `dir/`) is repeated once per sub-folder (mc joins an empty bucket name).
fn expand_verbose(input: &str) -> Vec<String> {
    let slashed = local_fs::slash_path(input);
    if slashed
        .split_once('/')
        .is_some_and(|(_, path)| !path.is_empty())
    {
        return vec![input.to_string()];
    }
    let dir = local_fs::client_path(input);
    let folders = crate::commands::ls::read_dir_entries(&dir)
        .map(|entries| entries.iter().filter(|(_, meta)| meta.is_dir()).count())
        .unwrap_or(0);
    if folders == 0 {
        return vec![input.to_string()];
    }
    vec![local_fs::clean_path(&slashed); folders]
}

pub(super) fn stat(input: &str, args: &StatArgs, json: bool) -> Result<()> {
    let targets = if args.verbose {
        expand_verbose(input)
    } else {
        vec![input.to_string()]
    };
    for target in targets {
        stat_target(&target, args, json).with_context(|| format!("Unable to stat `{target}`."))?;
    }
    Ok(())
}

fn stat_target(input: &str, args: &StatArgs, json: bool) -> Result<()> {
    let fpath = local_fs::client_path(input);
    let prefix_path = if fpath.ends_with('/') {
        fpath.clone()
    } else {
        fpath[..fpath.rfind('/').map_or(0, |index| index + 1)].to_string()
    };
    let relative = |path: &str| path.strip_prefix(&prefix_path).unwrap_or(path).to_string();

    if args.no_list || args.version_id.version_id.is_some() {
        if let Err((error, path)) = local_fs::fs_stat(&fpath, false) {
            return Err(local_fs::client_error("stat", &path, &error).0.into());
        }
        return LocalStat::new(&fpath, relative(&fpath))?.print(json);
    }

    let items = if args.recursive {
        list_recursive(&fpath)
    } else {
        local_fs::list(&fpath, false)
    };
    let mut found = 0;
    let mut failed = false;
    for item in items {
        match item {
            Listed::Error { error, kind } => {
                let label = match kind {
                    ErrorKind::SymlinkLoop => "Unable to list too many levels link.",
                    _ => "Unable to list folder.",
                };
                crate::output::print_error(&anyhow::Error::new(error).context(nonfatal(label)));
                failed |= kind == ErrorKind::Other;
            }
            Listed::Entry(entry) => {
                found += 1;
                let key = entry.key();
                LocalStat::new(&key, relative(&key))?.print(json)?;
            }
        }
    }
    if found == 0 {
        return Err(McError::object_missing().into());
    }
    if failed {
        return Err(crate::output::Exit(1).into());
    }
    Ok(())
}

/// mc `listRecursiveInRoutine`: regular files below the folder (a path without a trailing `/`
/// walks its parent and keeps paths starting with it). Symlinked folders are not followed.
fn list_recursive(fpath: &str) -> Vec<Listed> {
    let (dir, file_prefix) = if fpath.ends_with('/') {
        (fpath.to_string(), String::new())
    } else {
        let parent = &fpath[..fpath.rfind('/').map_or(0, |index| index + 1)];
        (parent.to_string(), fpath.to_string())
    };
    let mut files = Vec::new();
    let mut denied = Vec::new();
    let walked = crate::commands::ls::walk(
        &dir,
        &file_prefix,
        true,
        &mut files,
        &mut denied,
        &crate::commands::ls::read_dir_entries,
    );
    let mut out: Vec<Listed> = denied
        .into_iter()
        .map(|denied| Listed::Error {
            error: McError::path_insufficient_permission(&denied.path),
            kind: ErrorKind::Permission,
        })
        .collect();
    if let Err(error) = walked {
        // A missing folder lists nothing (mc's walk ignores it).
        let missing = crate::error::mc_error(&error)
            .is_some_and(|error| error.message.starts_with("Requested path"));
        if !missing {
            out.push(Listed::Error {
                error: McError::new(error.to_string()),
                kind: ErrorKind::Other,
            });
        }
    }
    out.extend(
        files
            .into_iter()
            .filter(|(path, _)| !path.ends_with(PART_SUFFIX))
            .map(|(path, meta)| Listed::Entry(local_fs::Entry { path, meta })),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_matches_mc_layout() {
        let stat = LocalStat {
            name: "sub/".into(),
            is_dir: true,
            size: 4096,
            time: UNIX_EPOCH,
            metadata: BTreeMap::from([
                ("Content-Type".into(), "application/octet-stream".into()),
                ("X-Amz-Meta-Mc-Attrs".into(), "mode:16877".into()),
            ]),
        };
        assert_eq!(
            stat.text(),
            "Name      : sub/\n\
             Date      : 1970-01-01 00:00:00 UTC \n\
             Type      : folder \n\
             Metadata  :\n  \
             Content-Type       : application/octet-stream \n  \
             X-Amz-Meta-Mc-Attrs: mode:16877 \n"
        );
        let file = LocalStat {
            name: "a.txt".into(),
            is_dir: false,
            size: 6,
            metadata: BTreeMap::new(),
            ..stat
        };
        assert_eq!(
            file.text(),
            "Name      : a.txt\n\
             Date      : 1970-01-01 00:00:00 UTC \n\
             Size      : 6 B    \n\
             Type      : file \n"
        );
    }
}
