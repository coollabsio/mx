//! `rm` on local paths (mc `fsClient`): mc's `removeSingle` / `listAndRemove` flow, output and
//! errors. S3-only flags (`--version-id`, `--bypass`, `--rewind`, ...) are accepted and have no
//! effect, like mc.
//!
//! Safety: listings never descend into symlinked folders (links are removed, not their
//! targets), a target that is itself a symlink is removed as a link (mc walks into it), and
//! folders are only removed when empty (`rmdir`).

use super::{RemoveMessage, Remover, report};
use crate::error::{McError, nonfatal};
use crate::local_fs::{self, ErrorKind, Listed, PART_SUFFIX};
use anyhow::Result;

/// mc `isAliasURLDir` for a local target: an existing folder (symlinks followed).
pub(super) fn is_dir(input: &str) -> bool {
    let path = local_fs::clean_path(&local_fs::slash_path(input));
    std::fs::metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// mc `url2Alias` of the cleaned target has an empty path: a top-level folder (`dir`, `dir/`,
/// `.`, `/`), which mc treats like an alias root (`--dangerous` required).
pub(super) fn is_namespace(input: &str) -> bool {
    let cleaned = local_fs::clean_path(&local_fs::slash_path(input));
    let cleaned = if cfg!(windows) {
        cleaned.trim_start_matches('/')
    } else {
        cleaned.as_str()
    };
    cleaned
        .split_once('/')
        .is_none_or(|(_, path)| path.is_empty())
}

impl Remover<'_> {
    pub(super) fn remove_local(&self, input: &str) -> Result<()> {
        if self.args.recursive || self.args.versions {
            self.list_and_remove_local(input)
        } else {
            self.remove_single_local(input)
        }
    }

    /// mc `removeSingle`: stat, filters, then `deleteFile` (a non-empty folder is reported as
    /// removed and kept, like mc).
    fn remove_single_local(&self, input: &str) -> Result<()> {
        let args = self.args;
        let failed = || nonfatal(format!("Failed to remove `{input}`."));
        let fpath = local_fs::client_path(input);
        let mut is_dir = false;
        if !args.purge {
            let meta = match std::fs::metadata(&fpath) {
                Ok(meta) => meta,
                Err(error) => {
                    let (error, _) = local_fs::client_error("stat", &fpath, &error);
                    return Err(anyhow::Error::new(error).context(failed()));
                }
            };
            is_dir = meta.is_dir();
            if !args.time.matches(meta.modified().ok(), self.now) {
                return Ok(());
            }
            if args.dry_run {
                let key = if is_dir && !fpath.ends_with('/') {
                    format!("{fpath}/")
                } else {
                    fpath
                };
                self.print(&RemoveMessage {
                    key,
                    dry_run: true,
                    ..Default::default()
                });
                return Ok(());
            }
        }
        let mut target = local_fs::slash_path(input);
        if is_dir && !target.ends_with('/') {
            target.push('/');
        }
        let name = if args.incomplete {
            format!("{target}{PART_SUFFIX}")
        } else {
            target.clone()
        };
        match local_fs::delete_file(&fpath, &name) {
            Ok(()) => {
                self.print(&RemoveMessage {
                    key: local_fs::clean_path(&target),
                    ..Default::default()
                });
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                // mc reports permission errors and carries on.
                report(
                    &anyhow::Error::new(McError::path_insufficient_permission(&target))
                        .context(failed()),
                );
                Ok(())
            }
            Err(error) => Err(
                anyhow::Error::new(local_fs::path_error("remove", &name, &error)).context(failed()),
            ),
        }
    }

    /// mc `listAndRemove`: `-r` removes every entry below the target (folders after their
    /// contents; the target folder goes once it is empty, unless given with a trailing `/`).
    /// `--versions` without `-r` lists the target like `ls` and, like mc, only removes it when
    /// the target is given as an absolute path. (Nothing found is never an error: both need
    /// `--force`.)
    fn list_and_remove_local(&self, input: &str) -> Result<()> {
        let args = self.args;
        let fpath = local_fs::client_path(input);
        let trimmed = fpath.strip_suffix('/').unwrap_or(&fpath);
        let link = std::fs::symlink_metadata(trimmed)
            .ok()
            .filter(|meta| meta.file_type().is_symlink());
        let items = match link {
            // Never walk into a symlinked target (mc does): remove the link itself.
            Some(meta) if args.recursive => vec![Listed::Entry(local_fs::Entry {
                path: trimmed.to_string(),
                meta,
            })],
            _ if args.recursive => local_fs::list_dirs_last(&fpath, args.incomplete),
            _ => local_fs::list(&fpath, args.incomplete),
        };
        let url = local_fs::slash_path(input);
        for item in items {
            let entry = match item {
                Listed::Entry(entry) => entry,
                Listed::Error { error, kind } => {
                    report(
                        &anyhow::Error::new(error)
                            .context(nonfatal(format!("Failed to remove `{input}` recursively."))),
                    );
                    if kind == ErrorKind::Permission {
                        continue;
                    }
                    return Err(crate::output::Exit(1).into());
                }
            };
            if !args.recursive && !url.starts_with(&entry.key()) {
                break;
            }
            let modified = entry.meta.modified().ok();
            if !args.time.matches(modified, self.now) {
                continue;
            }
            if args.dry_run {
                self.print(&RemoveMessage {
                    key: entry.key(),
                    mod_time: modified.filter(|_| args.versions),
                    dry_run: true,
                    ..Default::default()
                });
                continue;
            }
            let name = if args.incomplete {
                format!("{}{PART_SUFFIX}", entry.path)
            } else {
                entry.path.clone()
            };
            match local_fs::delete_file(&fpath, &name) {
                Ok(()) => self.print(&RemoveMessage {
                    key: local_fs::clean_path(&entry.path),
                    ..Default::default()
                }),
                Err(error) => {
                    let failed = nonfatal(format!("Failed to remove `{}`.", entry.path));
                    if error.kind() == std::io::ErrorKind::PermissionDenied {
                        report(
                            &anyhow::Error::new(McError::path_insufficient_permission(&entry.path))
                                .context(failed),
                        );
                        continue;
                    }
                    report(
                        &anyhow::Error::new(local_fs::path_error("remove", &name, &error))
                            .context(failed),
                    );
                    return Err(crate::output::Exit(1).into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_targets_are_top_level_folders() {
        assert!(is_namespace("dir"));
        assert!(is_namespace("dir/"));
        assert!(is_namespace("./dir/"));
        assert!(is_namespace("."));
        assert!(is_namespace("/"));
        assert!(!is_namespace("dir/sub"));
        assert!(!is_namespace("/tmp"));
        assert!(!is_namespace("../dir"));
    }
}
