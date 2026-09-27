//! mc `fsClient` behavior for local paths used by `stat` and `rm`: listings in mc's order,
//! mc's typed errors, and removal (`deleteFile`).
//!
//! Paths are absolute and `/`-separated ([`crate::error::abs_path`]); folders never get a
//! trailing `/` except where mc keeps the one typed by the user.

use crate::error::McError;
use std::fs::Metadata;
use std::io;

/// Suffix of partially downloaded files (`rm -I` removes them, listings skip them).
pub const PART_SUFFIX: &str = ".part.minio";

/// A listed local entry: absolute path and its metadata.
#[derive(Debug)]
pub struct Entry {
    pub path: String,
    pub meta: Metadata,
}

impl Entry {
    /// mc `getKey`: the path, with a trailing `/` for folders.
    pub fn key(&self) -> String {
        if self.meta.is_dir() && !self.path.ends_with('/') {
            format!("{}/", self.path)
        } else {
            self.path.clone()
        }
    }
}

/// Kind of a listing error: mc's typed path errors, or a raw OS error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    Permission,
    SymlinkLoop,
    Other,
}

/// One item of a local listing (mc sends errors through the same channel as entries).
#[derive(Debug)]
pub enum Listed {
    Entry(Entry),
    Error { error: McError, kind: ErrorKind },
}

/// Go `filepath.Clean` on a `/`-separated path.
pub fn clean_path(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let rooted = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// Go `path.Dir`.
pub fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(index) => clean_path(&path[..=index]),
        None => ".".to_string(),
    }
}

/// `path` with `\` separators turned into `/` on Windows (mc accepts both there).
pub fn slash_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_string()
    }
}

/// mc `fsNew`: the absolute path, keeping a typed trailing separator.
pub fn client_path(input: &str) -> String {
    let input = slash_path(input);
    let abs = crate::error::abs_path(&input);
    if input.ends_with('/') && !abs.ends_with('/') {
        format!("{abs}/")
    } else {
        abs
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// mc `isIgnoredFile`.
pub fn is_ignored(name: &str) -> bool {
    name == "lost+found" || (cfg!(target_os = "macos") && name.ends_with(".DS_Store"))
}

/// Go's text for an OS error (`permission denied`, `not a directory`, ...).
pub fn errno_text(error: &io::Error) -> String {
    if error.raw_os_error().is_some() {
        let text = error.to_string();
        let text = text
            .rfind(" (os error ")
            .map_or(text.as_str(), |index| &text[..index]);
        // Go's unix errno texts are lowercase; its Windows texts match Rust's as is.
        if cfg!(windows) {
            return text.to_string();
        }
        let mut chars = text.chars();
        return match chars.next() {
            Some(first) => first.to_lowercase().chain(chars).collect(),
            None => String::new(),
        };
    }
    error.to_string()
}

/// Go `*PathError` text: `OP PATH: ERRNO`.
pub fn path_error(op: &str, path: &str, error: &io::Error) -> McError {
    McError::new(format!("{op} {path}: {}", errno_text(error)))
}

fn is_symlink_loop(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

/// mc `toClientError`: typed errors for missing paths, permissions and symlink loops, else
/// the raw `OP PATH: ERRNO` error.
pub fn client_error(op: &str, path: &str, error: &io::Error) -> (McError, ErrorKind) {
    match error.kind() {
        io::ErrorKind::PermissionDenied => (
            McError::path_insufficient_permission(path),
            ErrorKind::Permission,
        ),
        io::ErrorKind::NotFound => (McError::path_not_found(path), ErrorKind::NotFound),
        _ if is_symlink_loop(error) => (McError::too_many_symlinks(path), ErrorKind::SymlinkLoop),
        _ => (path_error(op, path, error), ErrorKind::Other),
    }
}

/// mc `fsStat`: folders as is; files (or `PATH.part.minio` with `incomplete`) followed through
/// symlinks. Errors carry the path that failed.
pub fn fs_stat(path: &str, incomplete: bool) -> Result<Metadata, (io::Error, String)> {
    if let Ok(meta) = std::fs::metadata(path)
        && meta.is_dir()
    {
        return Ok(meta);
    }
    let path = if incomplete {
        format!("{path}{PART_SUFFIX}")
    } else {
        path.to_string()
    };
    std::fs::metadata(&path).map_err(|error| (error, path))
}

/// mc `readDir`: entries of `dir` (not followed through symlinks) sorted like mc (folders
/// compare with a trailing `/`).
pub fn read_dir(dir: &str) -> io::Result<Vec<(String, Metadata)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = join(dir, &name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        out.push((path, meta));
    }
    out.sort_by_cached_key(|(path, meta)| {
        if meta.is_dir() {
            format!("{path}/")
        } else {
            path.clone()
        }
    });
    Ok(out)
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// mc `listPrefixes`: entries of the parent folder whose path starts with `prefix`.
fn list_prefixes(prefix: &str, out: &mut Vec<Listed>) {
    let dir = parent_dir(prefix);
    let entries = match read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => {
            let (error, kind) = client_error("open", &dir, &error);
            out.push(Listed::Error { error, kind });
            return;
        }
    };
    for (path, meta) in entries {
        if is_ignored(base_name(&path)) || !path.starts_with(prefix) {
            continue;
        }
        let meta = if meta.file_type().is_symlink() {
            match std::fs::metadata(&path) {
                Ok(meta) => meta,
                Err(_) => continue,
            }
        } else {
            meta
        };
        out.push(Listed::Entry(Entry { path, meta }));
    }
}

/// mc `List` filter: `incomplete` keeps only `.part.minio` entries (suffix removed), else they
/// are skipped.
fn part_filter(items: Vec<Listed>, incomplete: bool) -> Vec<Listed> {
    items
        .into_iter()
        .filter_map(|item| match item {
            Listed::Entry(mut entry) => {
                let part = entry.path.ends_with(PART_SUFFIX);
                if part != incomplete {
                    return None;
                }
                if part {
                    entry.path.truncate(entry.path.len() - PART_SUFFIX.len());
                }
                Some(Listed::Entry(entry))
            }
            error => Some(error),
        })
        .collect()
}

/// mc `listInRoutine` (non-recursive `List`) of the client path `fpath`: a folder with a
/// trailing `/` lists its files and folders, a file itself; anything else lists the entries of
/// its parent starting with the path.
pub fn list(fpath: &str, incomplete: bool) -> Vec<Listed> {
    let mut out = Vec::new();
    match fs_stat(fpath, incomplete) {
        Err((error, _)) if error.kind() == io::ErrorKind::NotFound => {
            list_prefixes(fpath, &mut out);
        }
        Err((error, path)) => {
            let (error, kind) = client_error("stat", &path, &error);
            out.push(Listed::Error { error, kind });
        }
        Ok(meta) if meta.is_dir() && !fpath.ends_with('/') => list_prefixes(fpath, &mut out),
        Ok(meta) if meta.is_dir() => match read_dir(fpath) {
            Err(error) => out.push(Listed::Error {
                error: path_error("open", fpath, &error),
                kind: ErrorKind::Other,
            }),
            Ok(entries) => {
                for (path, meta) in entries {
                    let meta = if meta.file_type().is_symlink() {
                        match std::fs::metadata(&path) {
                            Ok(meta) => meta,
                            Err(_) => continue,
                        }
                    } else {
                        meta
                    };
                    if (meta.is_file() || meta.is_dir()) && !is_ignored(base_name(&path)) {
                        out.push(Listed::Entry(Entry { path, meta }));
                    }
                }
            }
        },
        Ok(meta) => out.push(Listed::Entry(Entry {
            path: fpath.to_string(),
            meta,
        })),
    }
    part_filter(out, incomplete)
}

/// mc `listDirOpt` with `DirLast` (recursive `rm`): every entry below `fpath` depth first,
/// folders after their contents. Entries are not followed through symlinks, so symlinked
/// folders are listed (and removed) as links. The walked folder itself is not listed.
pub fn list_dirs_last(fpath: &str, incomplete: bool) -> Vec<Listed> {
    fn walk(dir: &str, incomplete: bool, out: &mut Vec<Listed>) -> bool {
        let entries = match read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                let (error, kind) = match error.kind() {
                    io::ErrorKind::NotFound => (McError::path_not_found(dir), ErrorKind::NotFound),
                    io::ErrorKind::PermissionDenied => (
                        McError::path_insufficient_permission(dir),
                        ErrorKind::Permission,
                    ),
                    io::ErrorKind::NotADirectory => (
                        McError::new(format!("readdirent {dir}: not a directory")),
                        ErrorKind::Other,
                    ),
                    _ => (path_error("open", dir, &error), ErrorKind::Other),
                };
                let stop = kind == ErrorKind::Other;
                out.push(Listed::Error { error, kind });
                return stop;
            }
        };
        for (path, meta) in entries {
            if meta.is_dir() {
                if walk(&path, incomplete, out) {
                    return true;
                }
                if !incomplete {
                    out.push(Listed::Entry(Entry { path, meta }));
                }
            } else {
                out.push(Listed::Entry(Entry { path, meta }));
            }
        }
        false
    }
    let dir = fpath.strip_suffix('/').unwrap_or(fpath);
    let mut out = Vec::new();
    walk(dir, incomplete, &mut out);
    part_filter(out, incomplete)
}

/// Go `os.Remove`: unlink, else rmdir (a symlink is removed, never its target). The error of
/// rmdir wins unless it is `ENOTDIR`.
pub fn remove(path: &str) -> io::Result<()> {
    let unlink = match std::fs::remove_file(path) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() != io::ErrorKind::NotADirectory => Err(error),
        Err(_) => Err(unlink),
    }
}

/// mc `deleteFile`: removes `path` (a non-empty folder or a missing path is not an error),
/// then its parent folders while they are empty and inside `base`.
pub fn delete_file(base: &str, path: &str) -> io::Result<()> {
    let mut path = path.to_string();
    loop {
        if let Err(error) = remove(&path) {
            return match error.kind() {
                io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::NotFound => Ok(()),
                _ => Err(error),
            };
        }
        let parent = parent_dir(path.strip_suffix('/').unwrap_or(&path));
        if !parent.starts_with(base) || parent == "." || parent == path {
            return Ok(());
        }
        path = parent;
    }
}

/// Local file metadata mc reports with `stat` (`fileAttr`): the guessed `Content-Type`, the
/// `X-Amz-Meta-Mc-Attrs` attributes and extended attributes (Linux).
pub fn stat_metadata(path: &str) -> std::collections::BTreeMap<String, String> {
    let mut metadata = std::collections::BTreeMap::new();
    // Go's `filepath.Ext` of a folder key (`dir.txt/`) is empty.
    let content_type = if path.ends_with('/') {
        "application/octet-stream".to_string()
    } else {
        crate::s3::guess_content_type(path)
    };
    metadata.insert("Content-Type".to_string(), content_type);
    let file = std::path::Path::new(path);
    if let Ok(attrs) = crate::transfer::file_attrs(file) {
        metadata.extend(crate::transfer::file_xattrs(file));
        metadata.insert(crate::transfer::ATTRS_METADATA_KEY.to_string(), attrs);
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_like_go() {
        assert_eq!(clean_path("./a.txt"), "a.txt");
        assert_eq!(clean_path("sub//b/../c/"), "sub/c");
        assert_eq!(clean_path("../x"), "../x");
        assert_eq!(clean_path("/../x"), "/x");
        assert_eq!(clean_path(""), ".");
        assert_eq!(clean_path("/tmp/x/../y/./z"), "/tmp/y/z");
        assert_eq!(clean_path("./"), ".");
        assert_eq!(clean_path("/.."), "/");
        assert_eq!(clean_path("e//b/./c/../"), "e/b");
        assert_eq!(parent_dir("/a"), "/");
        assert_eq!(parent_dir("a"), ".");
        assert_eq!(parent_dir("/a/b/"), "/a/b");
    }

    #[test]
    fn errno_text_is_go_style() {
        let error = io::Error::from_raw_os_error(2);
        let expected = if cfg!(windows) {
            "The system cannot find the file specified."
        } else {
            "no such file or directory"
        };
        assert_eq!(errno_text(&error), expected);
    }

    fn keys(items: &[Listed], root: &str) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                Listed::Entry(entry) => entry.key().replacen(root, "", 1),
                Listed::Error { error, .. } => format!("error: {error}"),
            })
            .collect()
    }

    #[test]
    fn lists_like_mc() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::error::abs_path(&dir.path().to_string_lossy().replace('\\', "/"));
        for rel in ["d/s/e", "d/s.x", "d/t"] {
            std::fs::create_dir_all(format!("{root}/{rel}")).unwrap();
        }
        for rel in ["d/a", "d/s/b", "d/s.x/c", "d/p.part.minio", "d/f"] {
            std::fs::write(format!("{root}/{rel}"), rel).unwrap();
        }
        let all = list_dirs_last(&format!("{root}/d/"), false);
        assert_eq!(
            keys(&all, &root),
            [
                "/d/a", "/d/f", "/d/s.x/c", "/d/s.x/", "/d/s/b", "/d/s/e/", "/d/s/", "/d/t/"
            ]
        );
        let parts = list_dirs_last(&format!("{root}/d"), true);
        assert_eq!(keys(&parts, &root), ["/d/p"]);
        assert_eq!(
            keys(&list(&format!("{root}/d/s"), false), &root),
            ["/d/s.x/", "/d/s/"]
        );
        assert_eq!(
            keys(&list(&format!("{root}/d/"), false), &root),
            ["/d/a", "/d/f", "/d/s.x/", "/d/s/", "/d/t/"]
        );
        assert_eq!(keys(&list(&format!("{root}/d/a"), false), &root), ["/d/a"]);
        let missing = list(&format!("{root}/nope/x"), false);
        assert!(matches!(
            &missing[..],
            [Listed::Error {
                kind: ErrorKind::NotFound,
                ..
            }]
        ));
    }

    #[test]
    fn deletes_empty_parents_inside_base_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::error::abs_path(&dir.path().to_string_lossy().replace('\\', "/"));
        std::fs::create_dir_all(format!("{root}/d/s/e")).unwrap();
        std::fs::write(format!("{root}/d/s/e/f"), "x").unwrap();
        std::fs::write(format!("{root}/keep"), "x").unwrap();
        // A non-empty folder is left alone without an error.
        delete_file(&format!("{root}/d"), &format!("{root}/d/s")).unwrap();
        assert!(std::path::Path::new(&format!("{root}/d/s/e/f")).exists());
        delete_file(&format!("{root}/d"), &format!("{root}/d/s/e/f")).unwrap();
        assert!(!std::path::Path::new(&format!("{root}/d")).exists());
        assert!(std::path::Path::new(&format!("{root}/keep")).exists());
        // A trailing `/` on the base keeps the base folder itself.
        std::fs::create_dir_all(format!("{root}/k/s")).unwrap();
        delete_file(&format!("{root}/k/"), &format!("{root}/k/s")).unwrap();
        assert!(std::path::Path::new(&format!("{root}/k")).exists());
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::error::abs_path(&dir.path().to_string_lossy());
        std::fs::create_dir_all(format!("{root}/d")).unwrap();
        std::fs::create_dir_all(format!("{root}/other")).unwrap();
        std::fs::write(format!("{root}/other/k"), "x").unwrap();
        std::os::unix::fs::symlink(format!("{root}/other"), format!("{root}/d/link")).unwrap();
        let items = list_dirs_last(&format!("{root}/d"), false);
        assert_eq!(keys(&items, &root), ["/d/link"]);
        delete_file(&format!("{root}/d"), &format!("{root}/d/link")).unwrap();
        assert!(!std::path::Path::new(&format!("{root}/d")).exists());
        assert!(std::path::Path::new(&format!("{root}/other/k")).exists());
    }
}
