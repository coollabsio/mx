//! Pure mirror logic: mc's `difference.go` comparison, action planning (mirror-url.go),
//! exclude matching and small formatting helpers.

use crate::flags::TimeFilterFlags;
use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

/// mc's active-active source modification time metadata (`X-Amz-Meta-Mm-Source-Mtime`), as
/// user metadata key.
pub const SOURCE_MTIME_KEY: &str = "mm-source-mtime";

/// Local file attributes metadata (`X-Amz-Meta-Mc-Attrs`), as user metadata key.
pub const ATTRS_KEY: &str = "mc-attrs";

/// One listed file/object, keyed by its path relative to the mirror root.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub size: i64,
    pub mtime: Option<SystemTime>,
    pub storage_class: Option<String>,
    /// User metadata (lowercase keys, no `x-amz-meta-` prefix), when it was fetched.
    pub user_metadata: Option<BTreeMap<String, String>>,
}

pub type Listing = BTreeMap<String, Entry>;

/// mc `differType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Diff {
    OnlyInSource,
    OnlyInTarget,
    Size,
    Metadata,
    /// Source is newer (mc `differInAASourceMTime`, uses the active-active mtime metadata).
    SourceNewer,
}

impl Diff {
    /// mc's error condition name.
    pub fn as_str(self) -> &'static str {
        match self {
            Diff::OnlyInSource => "only-in-first",
            Diff::OnlyInTarget => "only-in-second",
            Diff::Size => "size",
            Diff::Metadata => "metadata",
            Diff::SourceNewer => "mm-source-mtime",
        }
    }
}

/// Compares two listings like mc's `differenceInternal`. With `include_target_only` false
/// (mc `sourceListingOnly`) objects only present on the target are not reported.
pub fn difference(
    source: &Listing,
    target: &Listing,
    include_target_only: bool,
    compare_metadata: bool,
) -> Vec<(String, Diff)> {
    let mut keys: BTreeSet<&String> = source.keys().collect();
    if include_target_only {
        keys.extend(target.keys());
    }
    let mut out = Vec::new();
    for key in keys {
        let diff = match (source.get(key), target.get(key)) {
            (Some(_), None) => Some(Diff::OnlyInSource),
            (None, Some(_)) => Some(Diff::OnlyInTarget),
            (Some(src), Some(dst)) => {
                if src.size != dst.size {
                    Some(Diff::Size)
                } else if active_active_mtime_updated(src, dst) {
                    Some(Diff::SourceNewer)
                } else if compare_metadata
                    && let (Some(a), Some(b)) = (&src.user_metadata, &dst.user_metadata)
                    && !metadata_equal(a, b)
                {
                    Some(Diff::Metadata)
                } else {
                    None
                }
            }
            (None, None) => None,
        };
        if let Some(diff) = diff {
            out.push((key.clone(), diff));
        }
    }
    out
}

/// mc `activeActiveModTimeUpdated`: is the source newer than the target, preferring the
/// origin mtime recorded by an active-active mirror when it is later than the object time.
pub fn active_active_mtime_updated(src: &Entry, dst: &Entry) -> bool {
    let (Some(src_time), Some(dst_time)) = (src.mtime, dst.mtime) else {
        return false;
    };
    let src_meta = source_mtime_meta(src.user_metadata.as_ref());
    let dst_meta = source_mtime_meta(dst.user_metadata.as_ref());
    if src_meta.is_none() && dst_meta.is_none() {
        return src_time > dst_time;
    }
    let parse = |value: Option<&str>| -> Result<Option<SystemTime>, ()> {
        value
            .map(|value| crate::flags::parse_timestamp(value).map_err(|_| ()))
            .transpose()
    };
    let (Ok(src_origin), Ok(dst_origin)) = (parse(src_meta), parse(dst_meta)) else {
        return false;
    };
    let src_actual = src_origin.filter(|t| *t > src_time).unwrap_or(src_time);
    let dst_actual = dst_origin.filter(|t| *t > dst_time).unwrap_or(dst_time);
    src_actual > dst_actual
}

pub fn source_mtime_meta(metadata: Option<&BTreeMap<String, String>>) -> Option<&str> {
    metadata?
        .iter()
        .find(|(key, value)| is_source_mtime_key(key) && !value.is_empty())
        .map(|(_, value)| value.as_str())
}

fn is_source_mtime_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key == SOURCE_MTIME_KEY || key == "x-amz-meta-mm-source-mtime"
}

/// mc `metadataEqual` (ignores the active-active mtime key).
pub fn metadata_equal(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> bool {
    let strip = |m: &BTreeMap<String, String>| -> BTreeMap<String, String> {
        m.iter()
            .filter(|(key, _)| !is_source_mtime_key(key))
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect()
    };
    strip(a) == strip(b)
}

/// What to do for one relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Copy {
        rel: String,
        size: i64,
    },
    Remove {
        rel: String,
    },
    /// Target differs but `--overwrite` is not set (reported, not fatal).
    OverwriteNotAllowed {
        rel: String,
        cond: Diff,
    },
}

/// Filters applied while planning.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    pub overwrite: bool,
    pub dry_run: bool,
    pub active_active: bool,
    pub remove: bool,
    pub exclude: Vec<String>,
    pub exclude_bucket: Vec<String>,
    pub exclude_storage_class: Vec<String>,
    pub time: TimeFilterFlags,
}

impl PlanOptions {
    pub fn excluded(&self, rel: &str) -> bool {
        match_exclude(&self.exclude, rel) || match_exclude_bucket(&self.exclude_bucket, rel)
    }
}

/// Turns differences into actions (mc `deltaSourceTarget` + `startMirror` filters).
pub fn plan(
    diffs: &[(String, Diff)],
    source: &Listing,
    options: &PlanOptions,
    now: SystemTime,
) -> Vec<Action> {
    let mut actions = Vec::new();
    for (rel, diff) in diffs {
        if options.excluded(rel) {
            continue;
        }
        let entry = source.get(rel);
        if let Some(entry) = entry
            && let Some(class) = &entry.storage_class
            && options.exclude_storage_class.iter().any(|c| c == class)
        {
            continue;
        }
        match diff {
            Diff::OnlyInTarget => {
                if options.remove {
                    actions.push(Action::Remove { rel: rel.clone() });
                }
            }
            Diff::Size | Diff::Metadata | Diff::SourceNewer
                if !options.overwrite && !options.dry_run && !options.active_active =>
            {
                actions.push(Action::OverwriteNotAllowed {
                    rel: rel.clone(),
                    cond: *diff,
                });
            }
            _ => {
                let Some(entry) = entry else { continue };
                if !options.time.matches(entry.mtime, now) {
                    continue;
                }
                actions.push(Action::Copy {
                    rel: rel.clone(),
                    size: entry.size,
                });
            }
        }
    }
    actions
}

/// minio `wildcard.Match`: `*` matches any sequence (including `/`), `?` one character.
pub fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

pub fn match_exclude(patterns: &[String], rel: &str) -> bool {
    let rel = rel.trim_start_matches('/');
    patterns.iter().any(|pattern| wildcard_match(pattern, rel))
}

/// Matches the first path component (the bucket for alias-root mirrors).
pub fn match_exclude_bucket(patterns: &[String], rel: &str) -> bool {
    let bucket = rel.trim_start_matches('/').split('/').next().unwrap_or("");
    patterns
        .iter()
        .any(|pattern| wildcard_match(pattern, bucket))
}

/// go-humanize `IBytes`.
pub fn humanize_ibytes(size: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if size < 10 {
        return format!("{size} B");
    }
    let exp = ((size as f64).ln() / 1024f64.ln()).floor() as usize;
    let exp = exp.min(UNITS.len() - 1);
    let value = ((size as f64) / 1024f64.powi(exp as i32) * 10.0 + 0.5).floor() / 10.0;
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[exp])
    } else {
        format!("{value:.0} {}", UNITS[exp])
    }
}

/// Local file attributes captured for `--preserve` (mc `X-Amz-Meta-Mc-Attrs`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileAttrs {
    pub atime: Option<(i64, i64)>,
    pub mtime: Option<(i64, i64)>,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

impl FileAttrs {
    /// `atime:S#N/gid:G/mode:M/mtime:S#N/uid:U` (mc order).
    pub fn encode(&self) -> String {
        let mut parts = Vec::new();
        if let Some((s, n)) = self.atime {
            parts.push(format!("atime:{s}#{n}"));
        }
        if let Some(gid) = self.gid {
            parts.push(format!("gid:{gid}"));
        }
        if let Some(mode) = self.mode {
            parts.push(format!("mode:{mode}"));
        }
        if let Some((s, n)) = self.mtime {
            parts.push(format!("mtime:{s}#{n}"));
        }
        if let Some(uid) = self.uid {
            parts.push(format!("uid:{uid}"));
        }
        parts.join("/")
    }

    /// Parses mc (`S#N`) and s3cmd (`S`) attribute strings; unknown fields are ignored.
    pub fn parse(value: &str) -> Self {
        let mut attrs = FileAttrs::default();
        let time = |v: &str| -> Option<(i64, i64)> {
            let (s, n) = v.split_once('#').unwrap_or((v, "0"));
            Some((s.trim().parse().ok()?, n.trim().parse().ok()?))
        };
        for part in value.split('/') {
            let Some((key, val)) = part.trim().split_once(':') else {
                continue;
            };
            let val = val.trim();
            match key.trim() {
                "atime" => attrs.atime = time(val),
                "mtime" => attrs.mtime = time(val),
                "mode" => attrs.mode = val.parse().ok(),
                "uid" => attrs.uid = val.parse().ok(),
                "gid" => attrs.gid = val.parse().ok(),
                _ => {}
            }
        }
        attrs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(secs: u64) -> Option<SystemTime> {
        Some(UNIX_EPOCH + Duration::from_secs(secs))
    }

    fn entry(size: i64, secs: u64) -> Entry {
        Entry {
            size,
            mtime: at(secs),
            ..Default::default()
        }
    }

    fn listing(items: &[(&str, Entry)]) -> Listing {
        items
            .iter()
            .map(|(k, e)| (k.to_string(), e.clone()))
            .collect()
    }

    #[test]
    fn difference_classifies_like_mc() {
        let source = listing(&[
            ("a", entry(1, 10)),
            ("b", entry(2, 10)),
            ("c", entry(3, 30)),
            ("d", entry(4, 10)),
        ]);
        let target = listing(&[
            ("b", entry(5, 20)),
            ("c", entry(3, 20)),
            ("d", entry(4, 20)),
            ("e", entry(1, 20)),
        ]);
        assert_eq!(
            difference(&source, &target, true, false),
            vec![
                ("a".into(), Diff::OnlyInSource),
                ("b".into(), Diff::Size),
                ("c".into(), Diff::SourceNewer),
                ("e".into(), Diff::OnlyInTarget),
            ]
        );
        assert_eq!(difference(&source, &target, false, false).len(), 3);
    }

    #[test]
    fn active_active_prefers_origin_mtime() {
        let meta = |value: &str| {
            Some(BTreeMap::from([(
                SOURCE_MTIME_KEY.to_string(),
                value.to_string(),
            )]))
        };
        // Target copy made at t=100 of a source modified at t=50: source not newer.
        let src = entry(1, 50);
        let mut dst = entry(1, 100);
        dst.user_metadata = meta("1970-01-01T00:00:50Z");
        assert!(!active_active_mtime_updated(&src, &dst));
        // Reverse direction: the origin (50) is not later than the copy's own time (100), so mc
        // compares object times and the copy counts as newer (loops are avoided in watch mode).
        let mut copy = entry(1, 100);
        copy.user_metadata = meta("1970-01-01T00:00:50Z");
        let mut original = entry(1, 50);
        original.user_metadata = Some(BTreeMap::new());
        assert!(active_active_mtime_updated(&copy, &original));
        // Origin later than object time wins.
        let mut newer = entry(1, 10);
        newer.user_metadata = meta("1970-01-01T00:03:20Z");
        assert!(active_active_mtime_updated(&newer, &entry(1, 150)));
        // Tampered metadata is ignored.
        let mut bad = entry(1, 500);
        bad.user_metadata = meta("garbage");
        assert!(!active_active_mtime_updated(&bad, &entry(1, 10)));
        assert!(!active_active_mtime_updated(
            &Entry::default(),
            &entry(1, 1)
        ));
    }

    #[test]
    fn metadata_difference_ignores_source_mtime() {
        let a = BTreeMap::from([
            ("k".to_string(), "v".to_string()),
            (SOURCE_MTIME_KEY.to_string(), "x".to_string()),
        ]);
        let b = BTreeMap::from([("K".to_string(), "v".to_string())]);
        assert!(metadata_equal(&a, &b));
        let c = BTreeMap::from([("k".to_string(), "other".to_string())]);
        assert!(!metadata_equal(&a, &c));

        let mut src = entry(1, 10);
        src.user_metadata = Some(c);
        let mut dst = entry(1, 20);
        dst.user_metadata = Some(b);
        let source = listing(&[("x", src)]);
        let target = listing(&[("x", dst)]);
        assert_eq!(
            difference(&source, &target, false, true),
            vec![("x".into(), Diff::Metadata)]
        );
        assert!(difference(&source, &target, false, false).is_empty());
    }

    #[test]
    fn plan_applies_overwrite_remove_and_filters() {
        let now = UNIX_EPOCH + Duration::from_secs(10 * 86400);
        let old = (now - Duration::from_secs(3 * 86400))
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut glacier = entry(1, old);
        glacier.storage_class = Some("GLACIER".into());
        let source = listing(&[
            ("new.txt", entry(1, old)),
            ("changed.txt", entry(2, old)),
            ("skip.tmp", entry(1, old)),
            ("cold.bin", glacier),
        ]);
        let diffs = vec![
            ("changed.txt".to_string(), Diff::Size),
            ("cold.bin".to_string(), Diff::OnlyInSource),
            ("extra.txt".to_string(), Diff::OnlyInTarget),
            ("new.txt".to_string(), Diff::OnlyInSource),
            ("skip.tmp".to_string(), Diff::OnlyInSource),
        ];
        let mut options = PlanOptions {
            exclude: vec!["*.tmp".into()],
            exclude_storage_class: vec!["GLACIER".into()],
            ..Default::default()
        };
        assert_eq!(
            plan(&diffs, &source, &options, now),
            vec![
                Action::OverwriteNotAllowed {
                    rel: "changed.txt".into(),
                    cond: Diff::Size
                },
                Action::Copy {
                    rel: "new.txt".into(),
                    size: 1
                },
            ]
        );
        options.overwrite = true;
        options.remove = true;
        options.time.newer_than = Some("1d".into());
        assert_eq!(
            plan(&diffs, &source, &options, now),
            vec![Action::Remove {
                rel: "extra.txt".into()
            }]
        );
        options.time = TimeFilterFlags {
            older_than: Some("1d".into()),
            newer_than: None,
        };
        options.remove = false;
        assert_eq!(
            plan(&diffs, &source, &options, now),
            vec![
                Action::Copy {
                    rel: "changed.txt".into(),
                    size: 2
                },
                Action::Copy {
                    rel: "new.txt".into(),
                    size: 1
                },
            ]
        );
        // dry-run and active-active copy differing objects without --overwrite
        for flags in [(true, false), (false, true)] {
            let options = PlanOptions {
                dry_run: flags.0,
                active_active: flags.1,
                ..Default::default()
            };
            assert!(
                plan(&diffs[..1], &source, &options, now).contains(&Action::Copy {
                    rel: "changed.txt".into(),
                    size: 2
                })
            );
        }
    }

    #[test]
    fn wildcard_matching() {
        assert!(wildcard_match("*.temp", "a/b/c.temp"));
        assert!(wildcard_match(".*", ".hidden"));
        assert!(!wildcard_match(".*", "dir/.hidden"));
        assert!(wildcard_match("*/.*", "dir/.hidden"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("a?c", "ac"));
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("dir/*", "dir/x/y"));
        assert!(!wildcard_match("dir/*", "other/x"));
        assert!(wildcard_match("exact", "exact"));
        assert!(!wildcard_match("exact", "exactly"));
        assert!(match_exclude(&["*.log".into()], "/logs/a.log"));
        assert!(match_exclude_bucket(&["test*".into()], "test-1/obj"));
        assert!(!match_exclude_bucket(&["test*".into()], "prod/test-1"));
    }

    #[test]
    fn humanizes_sizes_like_go_humanize() {
        assert_eq!(humanize_ibytes(0), "0 B");
        assert_eq!(humanize_ibytes(9), "9 B");
        assert_eq!(humanize_ibytes(10), "10 B");
        assert_eq!(humanize_ibytes(1024), "1.0 KiB");
        assert_eq!(humanize_ibytes(1536), "1.5 KiB");
        assert_eq!(humanize_ibytes(20 * 1024 * 1024), "20 MiB");
    }

    #[test]
    fn encodes_and_parses_file_attrs() {
        let attrs = FileAttrs {
            atime: Some((1700000000, 5)),
            mtime: Some((1600000000, 0)),
            mode: Some(33188),
            uid: Some(1000),
            gid: Some(100),
        };
        let text = attrs.encode();
        assert_eq!(
            text,
            "atime:1700000000#5/gid:100/mode:33188/mtime:1600000000#0/uid:1000"
        );
        assert_eq!(FileAttrs::parse(&text), attrs);
        let s3cmd = FileAttrs::parse("mtime:1600000000/mode:420/uname:root");
        assert_eq!(s3cmd.mtime, Some((1600000000, 0)));
        assert_eq!(s3cmd.mode, Some(420));
    }
}
