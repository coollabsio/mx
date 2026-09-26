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

// ---------------------------------------------------------------------------
// `--preserve` filesystem attributes (mc `X-Amz-Meta-Mc-Attrs`)
// ---------------------------------------------------------------------------

/// Metadata key mc uses to store filesystem attributes.
pub const ATTRS_METADATA_KEY: &str = "X-Amz-Meta-Mc-Attrs";

/// Encodes a file's attributes like mc (Linux):
/// `atime:SEC#NSEC/gid:N/gname:NAME/mode:N/mtime:SEC#NSEC/uid:N/uname:NAME`
/// (`mode` is the decimal `st_mode`; names are omitted when they cannot be resolved).
#[cfg(unix)]
pub fn file_attrs(path: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let meta =
        std::fs::metadata(path).with_context(|| format!("Unable to stat `{}`.", path.display()))?;
    let mut out = format!(
        "atime:{}#{}/gid:{}",
        meta.atime(),
        meta.atime_nsec(),
        meta.gid()
    );
    if let Some(name) = lookup_id_name("/etc/group", meta.gid()) {
        out.push_str(&format!("/gname:{name}"));
    }
    out.push_str(&format!(
        "/mode:{}/mtime:{}#{}/uid:{}",
        meta.mode(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.uid()
    ));
    if let Some(name) = lookup_id_name("/etc/passwd", meta.uid()) {
        out.push_str(&format!("/uname:{name}"));
    }
    Ok(out)
}

#[cfg(not(unix))]
pub fn file_attrs(_path: &Path) -> Result<String> {
    anyhow::bail!("`--preserve` is not supported on this platform.")
}

/// Extended attributes mc uploads with `--preserve` (`getAllXattrs`): every attribute except
/// `system.*`, sent as user metadata named after the attribute (`user.a` -> `X-Amz-Meta-User.a`).
/// Best effort like mc: unsupported filesystems or read errors yield none. mc never restores
/// xattrs on download.
#[cfg(unix)]
pub fn file_xattrs(path: &Path) -> Vec<(String, String)> {
    let Ok(names) = xattr::list(path) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = names
        .filter_map(|name| {
            let name = name.to_str()?.to_string();
            if name.starts_with("system.") {
                return None;
            }
            let value = xattr::get(path, &name).ok().flatten().unwrap_or_default();
            Some((name, String::from_utf8_lossy(&value).into_owned()))
        })
        .collect();
    out.sort();
    out
}

#[cfg(not(unix))]
pub fn file_xattrs(_path: &Path) -> Vec<(String, String)> {
    Vec::new()
}

/// Name for a numeric id from an `/etc/passwd`-style file (`name:x:id:...`).
fn lookup_id_name(file: &str, id: u32) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    parse_id_name(&text, id)
}

fn parse_id_name(text: &str, id: u32) -> Option<String> {
    text.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let found: u32 = fields.nth(1)?.parse().ok()?;
        (found == id && !name.is_empty()).then(|| name.to_string())
    })
}

/// Parses an mc attribute string (`key:value/key:value`).
pub fn parse_attrs(value: &str) -> std::collections::BTreeMap<String, String> {
    value
        .split('/')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let (key, value) = part.split_once(':').unwrap_or((part, ""));
            Some((key.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

/// `SEC#NSEC` (or `SEC`) -> SystemTime.
fn parse_attr_time(value: &str) -> Option<std::time::SystemTime> {
    let (secs, nanos) = value.split_once('#').unwrap_or((value, "0"));
    let secs: i64 = secs.parse().ok()?;
    let nanos: u32 = nanos.parse().ok()?;
    let secs = u64::try_from(secs).ok()?;
    Some(std::time::UNIX_EPOCH + std::time::Duration::new(secs, nanos))
}

/// Go `strconv.ParseUint(s, 0, 32)`: decimal, `0x` hex, or leading-`0` octal.
fn parse_mode(value: &str) -> Option<u32> {
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()
    } else if let Some(octal) = value.strip_prefix("0o") {
        u32::from_str_radix(octal, 8).ok()
    } else if value.len() > 1 && value.starts_with('0') {
        u32::from_str_radix(&value[1..], 8).ok()
    } else {
        value.parse().ok()
    }
}

/// Restores timestamps, ownership and permission bits from mc attributes. Ownership changes
/// that fail (e.g. when not root) produce a warning, like mc.
#[cfg(unix)]
pub fn apply_attrs(path: &Path, attrs: &std::collections::BTreeMap<String, String>) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let atime = attrs.get("atime").and_then(|v| parse_attr_time(v));
    let mtime = attrs.get("mtime").and_then(|v| parse_attr_time(v));
    if let (Some(atime), Some(mtime)) = (atime, mtime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|file| {
                file.set_times(
                    std::fs::FileTimes::new()
                        .set_accessed(atime)
                        .set_modified(mtime),
                )
            })
            .with_context(|| format!("Unable to set times on `{}`.", path.display()))?;
    }
    let meta = std::fs::metadata(path)?;
    let uid = attrs.get("uid").and_then(|v| v.parse::<u32>().ok());
    let gid = attrs.get("gid").and_then(|v| v.parse::<u32>().ok());
    let owner_differs =
        (uid.is_some() && uid != Some(meta.uid())) || (gid.is_some() && gid != Some(meta.gid()));
    if owner_differs && let Err(error) = std::os::unix::fs::chown(path, uid, gid) {
        crate::output::error_if(
            &format!("Unable to preserve ownership of `{}`.", path.display()),
            &error.to_string(),
        );
    }
    if let Some(mode) = attrs.get("mode").and_then(|v| parse_mode(v)) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777))
            .with_context(|| format!("Unable to set mode on `{}`.", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn apply_attrs(
    _path: &Path,
    _attrs: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    anyhow::bail!("`--preserve` is not supported on this platform.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mc_attribute_strings() {
        let attrs = parse_attrs(
            "atime:1700000000#5/gid:100/gname:users/mode:33188/mtime:1600000000#0/uid:1000/uname:me",
        );
        assert_eq!(attrs["mode"], "33188");
        assert_eq!(attrs["uname"], "me");
        assert_eq!(
            parse_attr_time(&attrs["atime"]),
            Some(std::time::UNIX_EPOCH + std::time::Duration::new(1_700_000_000, 5))
        );
        assert_eq!(parse_mode("33188"), Some(0o100644));
        assert_eq!(parse_mode("0644"), Some(0o644));
        assert_eq!(parse_mode("0x1a4"), Some(0o644));
        assert_eq!(
            parse_id_name(
                "root:x:0:0::/root:/bin/sh\nme:x:1000:1000::/h:/bin/sh\n",
                1000
            ),
            Some("me".to_string())
        );
        assert_eq!(parse_id_name("root:x:0:\n", 7), None);
    }

    #[cfg(unix)]
    #[test]
    fn round_trips_file_attributes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src.txt");
        std::fs::write(&source, "x").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(old)
                    .set_accessed(old),
            )
            .unwrap();
        let encoded = file_attrs(&source).unwrap();
        assert!(encoded.starts_with("atime:1600000000#0/gid:"));
        assert!(encoded.contains("/mode:33184/mtime:1600000000#0/uid:"));

        let target = dir.path().join("dst.txt");
        std::fs::write(&target, "x").unwrap();
        apply_attrs(&target, &parse_attrs(&encoded)).unwrap();
        let meta = std::fs::metadata(&target).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o640);
        assert_eq!(meta.modified().unwrap(), old);
    }
}
