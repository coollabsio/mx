//! `mx watch` (mc `watch`): print object notification events of a bucket / all buckets
//! (MinIO listen API, `crate::s3::listen`) or of a local directory (inotify on Linux, the
//! `notify` crate elsewhere, like mc's `rjeczalik/notify` backends).

use crate::commands::admin::trace::{emit, help_exit, interrupted, path_error, quiet_pipe};
use crate::config::ConfigStore;
use crate::error::McError;
use crate::location::{Location, parse_location};
use crate::output::{self, Exit};
use crate::s3::admin::{self, ibytes};
use crate::s3::listen::{self, NotificationEvent, NotificationInfo};
use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;

#[derive(Debug, Args)]
pub struct WatchArgs {
    #[arg(value_name = "TARGET")]
    pub target: Vec<String>,
    #[arg(
        long = "events",
        default_value = "put,delete,get",
        value_name = "VALUE",
        help = "filter specific types of events; defaults to all events by default"
    )]
    pub events: String,
    #[arg(
        long = "prefix",
        value_name = "VALUE",
        help = "filter events for a prefix"
    )]
    pub prefix: Option<String>,
    #[arg(
        long = "suffix",
        value_name = "VALUE",
        help = "filter events for a suffix"
    )]
    pub suffix: Option<String>,
    #[arg(long = "recursive", help = "recursively watch for events")]
    pub recursive: bool,
}

/// mc `EventInfo`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventInfo {
    pub time: String,
    pub size: i64,
    pub path: String,
    pub event_type: String,
    pub host: String,
    pub port: String,
    pub user_agent: String,
}

#[derive(Serialize)]
struct WatchEvent<'a> {
    time: &'a str,
    size: i64,
    path: &'a str,
    #[serde(rename = "type")]
    event_type: &'a str,
}

#[derive(Serialize)]
struct WatchSource<'a> {
    #[serde(skip_serializing_if = "str::is_empty")]
    host: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    port: &'a str,
    #[serde(rename = "userAgent", skip_serializing_if = "str::is_empty")]
    user_agent: &'a str,
}

/// mc `watchMessage`.
#[derive(Serialize)]
struct WatchMessage<'a> {
    status: &'static str,
    events: WatchEvent<'a>,
    source: WatchSource<'a>,
}

impl EventInfo {
    /// mc `watchMessage.String()` (without colors).
    fn text(&self) -> String {
        let size = if self.event_type.starts_with("s3:ObjectCreated:") {
            ibytes(self.size.max(0) as u64)
        } else {
            String::new()
        };
        format!(
            "[{}] {size:>6} {} {}",
            self.time, self.event_type, self.path
        )
    }

    fn json(&self) -> Result<String> {
        output::json_string(&WatchMessage {
            status: "success",
            events: WatchEvent {
                time: &self.time,
                size: self.size,
                path: &self.path,
                event_type: &self.event_type,
            },
            source: WatchSource {
                host: &self.host,
                port: &self.port,
                user_agent: &self.user_agent,
            },
        })
    }

    fn print(&self, json: bool) -> Result<()> {
        let text = if json { self.json()? } else { self.text() };
        quiet_pipe(emit(&text))
    }
}

fn watch_error(cause: McError) -> anyhow::Error {
    anyhow::Error::new(cause).context("Unable to watch on the specified bucket.")
}

pub fn run(args: WatchArgs, json: bool) -> Result<()> {
    if args.target.len() != 1 {
        return help_exit(&["watch"]);
    }
    let target = &args.target[0];
    let events: Vec<&str> = args.events.split(',').collect();
    let store = ConfigStore::load_or_create()?;
    match parse_location(target, store.config()) {
        Location::S3(parsed) => {
            let alias = store.alias(&parsed.alias)?;
            let bucket = parsed.bucket.clone().unwrap_or_default();
            let object = parsed.key_with_trailing_slash().unwrap_or_default();
            let mut prefix = args.prefix.clone().unwrap_or_default();
            if !object.is_empty() && !prefix.is_empty() {
                return Err(watch_error(McError::invalid_argument()));
            }
            let mut names = Vec::new();
            for event in &events {
                let Some(mapped) = listen::event_names(event) else {
                    return Err(watch_error(McError::invalid_argument()));
                };
                names.extend_from_slice(mapped);
            }
            if !object.is_empty() {
                prefix = object;
            }
            let client = admin::admin_client_for(&store, target)
                .context("Unable to parse the provided url.")?;
            let base = alias.url.trim_end_matches('/').to_string();
            let suffix = args.suffix.clone().unwrap_or_default();
            watch_s3(&client, &base, &bucket, &prefix, &suffix, &names, json)
        }
        Location::Local(_) => watch_local(target, &events, args.recursive, json),
    }
}

/// Go `url.URL.EscapedPath` for a plain path.
fn escape_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b'$'
                    | b'&'
                    | b'+'
                    | b','
                    | b'/'
                    | b':'
                    | b';'
                    | b'='
                    | b'@'
            );
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Go `path.Join("/", parts...)`.
fn join_clean(parts: &[&str]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in parts.iter().flat_map(|p| p.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    format!("/{}", out.join("/"))
}

/// mc `notificationToEventsInfo` for one record.
fn event_info(base: &str, record: &NotificationEvent) -> EventInfo {
    let raw_key = &record.s3.object.key;
    let key = if raw_key.contains("%2F") {
        crate::commands::admin::trace::query_unescape(raw_key).unwrap_or_else(|| raw_key.clone())
    } else {
        raw_key.clone()
    };
    let path = format!(
        "{base}{}",
        escape_path(&join_clean(&[&record.s3.bucket.name, &key]))
    );
    let name = &record.event_name;
    let event_type = if let Some(rest) = name.strip_prefix("s3:ObjectCreated:") {
        if rest.starts_with("Copy") {
            "s3:ObjectCreated:Copy"
        } else if rest.starts_with("PutRetention") {
            "s3:ObjectCreated:PutRetention"
        } else if rest.starts_with("PutLegalHold") {
            "s3:ObjectCreated:PutLegalHold"
        } else {
            "s3:ObjectCreated:Put"
        }
        .to_string()
    } else {
        name.clone()
    };
    EventInfo {
        time: record.event_time.clone(),
        size: record.s3.object.size,
        path,
        event_type,
        host: record.source.host.clone(),
        port: record.source.port.clone(),
        user_agent: record.source.user_agent.clone(),
    }
}

/// Listens until interrupted; the server closing the stream reconnects (minio-go), a failed
/// request is reported with mc `errorIf` and ends the command with status 0 like mc.
fn watch_s3(
    client: &admin::AdminClient,
    base: &str,
    bucket: &str,
    prefix: &str,
    suffix: &str,
    events: &[&str],
    json: bool,
) -> Result<()> {
    let result: Result<()> = crate::commands::runtime()?.block_on(async {
        let signal = interrupted();
        tokio::pin!(signal);
        loop {
            let mut stream = tokio::select! {
                code = &mut signal => return Err(Exit(code).into()),
                stream = listen::listen(client, bucket, prefix, suffix, events) => stream?,
            };
            loop {
                let next = tokio::select! {
                    code = &mut signal => return Err(Exit(code).into()),
                    next = stream.next::<NotificationInfo>() => next,
                };
                let Ok(Some(info)) = next else {
                    break;
                };
                for record in info.records.iter().flatten() {
                    event_info(base, record).print(json)?;
                }
            }
            tokio::select! {
                code = &mut signal => return Err(Exit(code).into()),
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
        }
    });
    match result {
        Err(err) if err.downcast_ref::<Exit>().is_none() => {
            let err = if crate::error::error_code(&err) == Some("NotImplemented") {
                anyhow::Error::new(McError::new(format!(
                    "`Watch` is not supported for `{base}/{bucket}`."
                )))
            } else {
                err
            };
            output::print_error(
                &err.context(crate::error::nonfatal("Unable to watch for events.")),
            );
            Ok(())
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// local directories
// ---------------------------------------------------------------------------

/// mc's fs `Watch` needs the path to exist: notify `lstat`s it component by component.
/// Unix paths are `/a/b`; Windows paths `C:/a/b` (see `abs_path`), where the volume is the root.
fn check_local(path: &str) -> Result<String> {
    let abs = crate::error::abs_path(path);
    let (root, rest) = match abs.find('/') {
        Some(0) | None => ("", abs.as_str()),
        Some(index) => abs.split_at(index),
    };
    let mut current = root.to_string();
    for part in rest.split('/').filter(|p| !p.is_empty()) {
        current.push('/');
        current.push_str(part);
        if let Err(err) = std::fs::symlink_metadata(&current) {
            return Err(watch_error(path_error("lstat", &current, &err)));
        }
    }
    Ok(if current == root {
        format!("{root}/")
    } else {
        current
    })
}

fn now_fs() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}Z",
        crate::s3::admin_stream::format_millis(now.as_secs() as i64, now.subsec_nanos())
    )
}

#[cfg(target_os = "linux")]
mod inotify {
    //! Minimal inotify watcher (masks as mc's `client-fs_linux.go`).

    use std::collections::HashMap;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    pub const PUT: u32 = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO;
    pub const DELETE: u32 = libc::IN_DELETE | libc::IN_DELETE_SELF | libc::IN_MOVED_FROM;
    pub const GET: u32 = libc::IN_ACCESS | libc::IN_OPEN;

    pub struct Watcher {
        fd: i32,
        mask: u32,
        recursive: bool,
        dirs: HashMap<i32, String>,
    }

    impl Watcher {
        pub fn new(mask: u32, recursive: bool) -> std::io::Result<Self> {
            // SAFETY: plain syscall without pointers.
            let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self {
                fd,
                mask,
                recursive,
                dirs: HashMap::new(),
            })
        }

        /// Watches `path` (and, when recursive, every directory below it).
        pub fn add(&mut self, path: &str) -> std::io::Result<()> {
            let mut mask = self.mask;
            if self.recursive {
                mask |= libc::IN_CREATE | libc::IN_MOVED_TO;
            }
            let c_path = CString::new(path.as_bytes())?;
            // SAFETY: `c_path` is a valid NUL-terminated string for the call's duration.
            let wd = unsafe { libc::inotify_add_watch(self.fd, c_path.as_ptr(), mask) };
            if wd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            self.dirs.insert(wd, path.to_string());
            if self.recursive
                && let Ok(entries) = std::fs::read_dir(path)
            {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_dir()) {
                        let child = format!(
                            "{}/{}",
                            path.trim_end_matches('/'),
                            std::ffi::OsStr::new(&entry.file_name()).to_string_lossy()
                        );
                        let _ = self.add(&child);
                    }
                }
            }
            Ok(())
        }

        /// Blocks for the next batch of `(path, mask)` events (only the requested masks).
        pub fn read(&mut self) -> std::io::Result<Vec<(String, u32)>> {
            let mut buf = vec![0u8; 64 * 1024];
            // SAFETY: `buf` is valid for writes of its length.
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let header = std::mem::size_of::<libc::inotify_event>();
            let mut events = Vec::new();
            let mut offset = 0usize;
            while offset + header <= n as usize {
                // SAFETY: the kernel wrote a complete `inotify_event` header at `offset`.
                let event: libc::inotify_event =
                    unsafe { std::ptr::read_unaligned(buf[offset..].as_ptr().cast()) };
                let name_bytes = &buf[offset + header..offset + header + event.len as usize];
                let name_end = name_bytes
                    .iter()
                    .position(|b| *b == 0)
                    .unwrap_or(name_bytes.len());
                let name = std::ffi::OsStr::from_bytes(&name_bytes[..name_end])
                    .to_string_lossy()
                    .into_owned();
                offset += header + event.len as usize;
                let Some(dir) = self.dirs.get(&event.wd).cloned() else {
                    continue;
                };
                let path = if name.is_empty() {
                    dir
                } else {
                    format!("{}/{name}", dir.trim_end_matches('/'))
                };
                let is_dir = event.mask & libc::IN_ISDIR != 0;
                if self.recursive
                    && is_dir
                    && event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0
                {
                    let _ = self.add(&path);
                }
                if event.mask & libc::IN_IGNORED != 0 {
                    self.dirs.remove(&event.wd);
                }
                let wanted = event.mask & self.mask;
                if wanted != 0 {
                    events.push((path, wanted));
                }
            }
            Ok(events)
        }
    }

    impl Drop for Watcher {
        fn drop(&mut self) {
            // SAFETY: `fd` is owned by this watcher.
            unsafe { libc::close(self.fd) };
        }
    }
}

#[cfg(target_os = "linux")]
fn watch_local(target: &str, events: &[&str], recursive: bool, json: bool) -> Result<()> {
    let path = check_local(target)?;
    let mut mask = 0;
    for event in events {
        match *event {
            "put" => mask |= inotify::PUT,
            "delete" => mask |= inotify::DELETE,
            "get" => mask |= inotify::GET,
            // Other event types are not supported locally and ignored, like mc.
            _ => {}
        }
    }
    let mut watcher = inotify::Watcher::new(mask, recursive).map_err(|err| {
        watch_error(McError::new(crate::commands::admin::trace::go_os_error(
            &err,
        )))
    })?;
    watcher.add(&path).map_err(|err| {
        watch_error(McError::new(crate::commands::admin::trace::go_os_error(
            &err,
        )))
    })?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(batch) = watcher.read() {
            if tx.send(batch).is_err() {
                break;
            }
        }
    });
    crate::commands::runtime()?.block_on(async {
        let signal = interrupted();
        tokio::pin!(signal);
        loop {
            let batch = tokio::select! {
                code = &mut signal => return Err(Exit(code).into()),
                batch = rx.recv() => batch,
            };
            let Some(batch) = batch else {
                return Ok(());
            };
            for (path, event_mask) in batch {
                let base = path.rsplit('/').next().unwrap_or_default();
                if base == "lost+found" {
                    continue;
                }
                let info = if event_mask & inotify::PUT != 0 {
                    match std::fs::metadata(&path) {
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(err) => {
                            output::print_error(
                                &anyhow::Error::new(path_error("stat", &path, &err))
                                    .context(crate::error::nonfatal("Unable to watch for events.")),
                            );
                            return Ok(());
                        }
                        Ok(meta) if meta.is_dir() => continue,
                        Ok(meta) => EventInfo {
                            time: now_fs(),
                            size: meta.len() as i64,
                            path,
                            event_type: "s3:ObjectCreated:Put".into(),
                            ..Default::default()
                        },
                    }
                } else if event_mask & inotify::DELETE != 0 {
                    EventInfo {
                        time: now_fs(),
                        path,
                        event_type: "s3:ObjectRemoved:Delete".into(),
                        ..Default::default()
                    }
                } else {
                    EventInfo {
                        time: now_fs(),
                        path,
                        event_type: "s3:ObjectAccessed:Get".into(),
                        ..Default::default()
                    }
                };
                info.print(json)?;
            }
        }
    })
}

/// Non-Linux local watch through the `notify` crate, like mc's `client-fs_other.go`
/// (rjeczalik/notify: put = create/write/rename, delete = remove, no get events).
#[cfg(not(target_os = "linux"))]
fn watch_local(target: &str, events: &[&str], recursive: bool, json: bool) -> Result<()> {
    use notify::event::{EventKind, ModifyKind};
    use notify::{RecursiveMode, Watcher};
    let path = check_local(target)?;
    let want_put = events.contains(&"put");
    let want_delete = events.contains(&"delete");
    let os_error = |err: notify::Error| {
        let text = match err.kind {
            notify::ErrorKind::Io(io) => crate::commands::admin::trace::go_os_error(&io),
            _ => err.to_string(),
        };
        watch_error(McError::new(text))
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let _ = tx.send(event);
    })
    .map_err(os_error)?;
    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    watcher
        .watch(std::path::Path::new(&path), mode)
        .map_err(os_error)?;
    crate::commands::runtime()?.block_on(async {
        let signal = interrupted();
        tokio::pin!(signal);
        loop {
            let event = tokio::select! {
                code = &mut signal => return Err(Exit(code).into()),
                event = rx.recv() => event,
            };
            let Some(event) = event else {
                return Ok(());
            };
            let Ok(event) = event else {
                continue;
            };
            let put = matches!(
                event.kind,
                EventKind::Create(_)
                    | EventKind::Modify(
                        ModifyKind::Data(_)
                            | ModifyKind::Name(_)
                            | ModifyKind::Any
                            | ModifyKind::Other
                    )
            );
            let delete = matches!(event.kind, EventKind::Remove(_));
            for path in event.paths {
                let path = path.to_string_lossy().replace('\\', "/");
                let info = if put && want_put {
                    match std::fs::metadata(&path) {
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(err) => {
                            output::print_error(
                                &anyhow::Error::new(path_error("stat", &path, &err))
                                    .context(crate::error::nonfatal("Unable to watch for events.")),
                            );
                            return Ok(());
                        }
                        Ok(meta) if meta.is_dir() => continue,
                        Ok(meta) => EventInfo {
                            time: now_fs(),
                            size: meta.len() as i64,
                            path,
                            event_type: "s3:ObjectCreated:Put".into(),
                            ..Default::default()
                        },
                    }
                } else if delete && want_delete {
                    EventInfo {
                        time: now_fs(),
                        path,
                        event_type: "s3:ObjectRemoved:Delete".into(),
                        ..Default::default()
                    }
                } else {
                    continue;
                };
                info.print(json)?;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::listen::{EventBucket, EventObject, EventS3, EventSource};

    fn record(name: &str, key: &str, size: i64) -> NotificationEvent {
        NotificationEvent {
            event_time: "2026-09-26T19:31:12.644Z".into(),
            event_name: name.into(),
            s3: EventS3 {
                bucket: EventBucket {
                    name: "bucket2".into(),
                },
                object: EventObject {
                    key: key.into(),
                    size,
                    ..Default::default()
                },
            },
            source: EventSource {
                host: "172.17.0.1".into(),
                port: String::new(),
                user_agent: "MinIO".into(),
            },
        }
    }

    #[test]
    fn s3_events_print_like_mc() {
        let base = "http://127.0.0.1:9000";
        let put = event_info(
            base,
            &record("s3:ObjectCreated:CompleteMultipartUpload", "dir%2Ff.txt", 4),
        );
        assert_eq!(put.path, "http://127.0.0.1:9000/bucket2/dir/f.txt");
        assert_eq!(
            put.text(),
            "[2026-09-26T19:31:12.644Z]    4 B s3:ObjectCreated:Put http://127.0.0.1:9000/bucket2/dir/f.txt"
        );
        let get = event_info(base, &record("s3:ObjectAccessed:Head", "a b!.txt", 0));
        assert_eq!(
            get.text(),
            "[2026-09-26T19:31:12.644Z]        s3:ObjectAccessed:Head http://127.0.0.1:9000/bucket2/a%20b%21.txt"
        );
        let copy = event_info(base, &record("s3:ObjectCreated:Copy", "c", 1));
        assert_eq!(copy.event_type, "s3:ObjectCreated:Copy");
        assert_eq!(
            serde_json::to_string(&WatchMessage {
                status: "success",
                events: WatchEvent {
                    time: &put.time,
                    size: put.size,
                    path: &put.path,
                    event_type: &put.event_type,
                },
                source: WatchSource {
                    host: &put.host,
                    port: &put.port,
                    user_agent: &put.user_agent,
                },
            })
            .unwrap(),
            r#"{"status":"success","events":{"time":"2026-09-26T19:31:12.644Z","size":4,"path":"http://127.0.0.1:9000/bucket2/dir/f.txt","type":"s3:ObjectCreated:Put"},"source":{"host":"172.17.0.1","userAgent":"MinIO"}}"#
        );
    }

    #[cfg(not(windows))] // Windows OS error texts and `C:/` paths
    #[test]
    fn local_path_errors_name_the_first_missing_component() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope/deeper");
        let err = check_local(&missing.to_string_lossy()).unwrap_err();
        let (message, cause) = output::split_error(&err);
        assert_eq!(message, "Unable to watch on the specified bucket.");
        assert_eq!(
            cause,
            format!(
                "lstat {}: no such file or directory",
                dir.path().join("nope").display()
            )
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inotify_reports_put_get_delete() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut watcher = inotify::Watcher::new(inotify::PUT | inotify::DELETE, true).unwrap();
        watcher.add(&root).unwrap();
        std::fs::write(dir.path().join("sub/a.txt"), b"hi").unwrap();
        std::fs::remove_file(dir.path().join("sub/a.txt")).unwrap();
        let mut seen = Vec::new();
        while seen.len() < 2 {
            seen.extend(watcher.read().unwrap());
        }
        let path = format!("{root}/sub/a.txt");
        assert_eq!(seen[0], (path.clone(), libc::IN_CLOSE_WRITE));
        assert_eq!(seen[1], (path, libc::IN_DELETE));
    }
}
