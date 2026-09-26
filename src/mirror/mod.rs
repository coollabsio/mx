//! `mx mirror` engine (area D): lists source and target, compares them like mc's
//! `difference.go`, and copies/removes objects in parallel. `--watch` re-runs the comparison
//! periodically (mc uses server notifications; a rescan loop gives the same end result).

pub mod diff;

use crate::config::model::AliasConfig;
use crate::flags::{ChecksumAlgo, Sse, resolve_sse};
use crate::s3::{GetOptions, ListOptions, MakeBucketOptions, ObjectRef, PutOptions};
use anyhow::{Context, Result, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::{DateTime, DateTimeFormat};
use diff::{Action, Entry, FileAttrs, Listing, PlanOptions};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::task::JoinSet;

/// Largest object a single server-side CopyObject can handle.
const MAX_COPY_OBJECT_SIZE: i64 = 5 * 1024 * 1024 * 1024;

/// One side of a mirror.
#[derive(Clone)]
pub enum Endpoint {
    /// Local directory.
    Local(PathBuf),
    S3(Box<S3Endpoint>),
}

#[derive(Clone)]
pub struct S3Endpoint {
    pub alias_name: String,
    pub alias: AliasConfig,
    pub client: Client,
    /// `None` for an alias root (relative paths are then `BUCKET/KEY`).
    pub bucket: Option<String>,
    /// Key prefix: empty or ending with `/`.
    pub prefix: String,
}

impl S3Endpoint {
    fn locate(&self, rel: &str) -> Result<(String, String)> {
        match &self.bucket {
            Some(bucket) => Ok((bucket.clone(), format!("{}{rel}", self.prefix))),
            None => match rel.split_once('/') {
                Some((bucket, key)) if !bucket.is_empty() && !key.is_empty() => {
                    Ok((bucket.to_string(), key.to_string()))
                }
                _ => bail!(
                    "`{rel}` cannot be stored at the alias root `{}`; only folders map to buckets.",
                    self.alias_name
                ),
            },
        }
    }

    fn display(&self, rel: &str) -> String {
        match &self.bucket {
            Some(bucket) => format!("{}/{bucket}/{}{rel}", self.alias_name, self.prefix),
            None => format!("{}/{rel}", self.alias_name),
        }
    }
}

impl Endpoint {
    pub fn display(&self, rel: &str) -> String {
        match self {
            Endpoint::Local(root) => root.join(rel).display().to_string(),
            Endpoint::S3(s3) => s3.display(rel),
        }
    }

    fn is_alias_root(&self) -> bool {
        matches!(self, Endpoint::S3(s3) if s3.bucket.is_none())
    }
}

/// Builds an endpoint for `alias/bucket/prefix`.
pub async fn s3_endpoint(
    alias_name: &str,
    alias: AliasConfig,
    bucket: Option<String>,
    key: Option<&str>,
) -> Result<S3Endpoint> {
    let client = crate::s3::build_client(&alias).await?;
    let prefix = key
        .map(|key| key.trim_matches('/'))
        .filter(|key| !key.is_empty())
        .map(|key| format!("{key}/"))
        .unwrap_or_default();
    Ok(S3Endpoint {
        alias_name: alias_name.to_string(),
        alias,
        client,
        bucket: bucket.filter(|b| !b.is_empty()),
        prefix,
    })
}

/// Mirror settings (see `mx mirror --help`).
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub plan: PlanOptions,
    pub watch: bool,
    pub watch_interval: Duration,
    pub region: String,
    pub preserve: bool,
    pub attr: Vec<(String, String)>,
    pub storage_class: Option<String>,
    pub disable_multipart: bool,
    pub checksum: Option<ChecksumAlgo>,
    pub enc: Vec<(String, Sse)>,
    pub retry: bool,
    pub summary: bool,
    pub skip_errors: bool,
    pub max_workers: usize,
    pub json: bool,
}

impl Options {
    /// mc compares metadata when attributes are preserved or added.
    fn compare_metadata(&self) -> bool {
        self.preserve || !self.attr.is_empty()
    }

    fn keep_running(&self) -> bool {
        self.watch || self.plan.active_active
    }
}

/// Checks that the source is an existing folder/bucket/prefix (mc `checkMirrorSyntax`) and the
/// target is usable.
pub async fn validate(source: &Endpoint, target: &Endpoint, options: &Options) -> Result<()> {
    if target.is_alias_root() && !matches!(source, Endpoint::Local(_)) && !source.is_alias_root() {
        bail!("Target must include a bucket unless the source is an alias or a local folder.");
    }
    if !options.keep_running() {
        match source {
            Endpoint::Local(path) => {
                let meta = std::fs::metadata(path)
                    .with_context(|| format!("Unable to stat source `{}`.", path.display()))?;
                if !meta.is_dir() {
                    bail!(
                        "Source `{}` is not a folder. Only folders are supported by mirror command.",
                        path.display()
                    );
                }
            }
            Endpoint::S3(s3) => {
                if let Some(bucket) = &s3.bucket {
                    let display = s3.display("");
                    s3.client
                        .head_bucket()
                        .bucket(bucket)
                        .send()
                        .await
                        .with_context(|| format!("Unable to stat source `{display}`."))?;
                    let key = s3.prefix.trim_end_matches('/');
                    if !key.is_empty()
                        && s3
                            .client
                            .head_object()
                            .bucket(bucket)
                            .key(key)
                            .send()
                            .await
                            .is_ok()
                    {
                        bail!(
                            "Source `{}/{bucket}/{key}` is not a folder. Only folders are supported by mirror command.",
                            s3.alias_name
                        );
                    }
                }
            }
        }
    }
    match target {
        Endpoint::Local(path) => {
            if path.exists() && !path.is_dir() {
                bail!("Target `{}` is not a folder.", path.display());
            }
        }
        Endpoint::S3(s3) => {
            if let Some(bucket) = &s3.bucket
                && s3.client.head_bucket().bucket(bucket).send().await.is_err()
            {
                bail!(
                    "Target bucket `{}/{bucket}` does not exist or is not accessible.",
                    s3.alias_name
                );
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct Stats {
    count: AtomicI64,
    total: AtomicI64,
    transferred: AtomicI64,
}

struct Job {
    source: Endpoint,
    target: Endpoint,
    options: Options,
    stats: Stats,
    /// Target buckets created with object lock (uploads need a checksum).
    locked_buckets: Mutex<HashSet<String>>,
    /// Overwrite conflicts already reported (watch mode reports each once).
    reported: Mutex<HashSet<String>>,
}

/// Event info for watch-mode messages (empty for the initial pass, like mc).
#[derive(Clone, Default)]
struct Event {
    time: String,
    kind: &'static str,
}

impl Event {
    fn now(kind: &'static str) -> Self {
        Self {
            time: format_time(SystemTime::now()),
            kind,
        }
    }
}

/// mc `mirrorMessage`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MirrorMessage<'a> {
    status: &'static str,
    source: &'a str,
    target: &'a str,
    size: i64,
    total_count: i64,
    total_size: i64,
    event_time: &'a str,
    event_type: &'a str,
}

/// Runs the mirror; returns an error when any copy/remove failed.
pub async fn run(source: Endpoint, target: Endpoint, options: Options) -> Result<()> {
    let job = Arc::new(Job {
        source,
        target,
        options,
        stats: Stats::default(),
        locked_buckets: Mutex::new(HashSet::new()),
        reported: Mutex::new(HashSet::new()),
    });
    let started = Instant::now();
    let mut previous: Option<BTreeSet<String>> = None;
    let mut failed = false;
    loop {
        match job.clone().pass(previous.as_ref()).await {
            Ok((keys, pass_failed)) => {
                failed |= pass_failed;
                previous = Some(keys);
            }
            Err(error) => {
                job.report_error("Failed to perform mirroring", &format!("{error:#}"));
                failed = true;
            }
        }
        if !job.options.keep_running() {
            break;
        }
        tokio::time::sleep(job.options.watch_interval).await;
    }
    if job.options.summary {
        job.print_summary(started.elapsed())?;
    }
    if failed {
        bail!("mirror finished with errors");
    }
    Ok(())
}

impl Job {
    /// One full comparison + transfer. Returns the source keys and whether anything failed.
    async fn pass(
        self: Arc<Self>,
        previous: Option<&BTreeSet<String>>,
    ) -> Result<(BTreeSet<String>, bool)> {
        let initial = previous.is_none();
        let options = &self.options;
        let mut failed = self.sync_buckets().await?;

        let (mut source, source_errors) = list(&self.source, options).await?;
        let (mut target, target_errors) = list(&self.target, options).await?;
        // An incomplete scan must not turn unreadable entries into deletions.
        let incomplete = !source_errors.is_empty() || !target_errors.is_empty();
        for error in source_errors.iter().chain(&target_errors) {
            self.report_error("Failed to perform mirroring", error);
            failed = true;
        }

        let compare_metadata = options.compare_metadata()
            && initial
            && matches!(
                (&self.source, &self.target),
                (Endpoint::S3(_), Endpoint::S3(_))
            );
        if compare_metadata || options.plan.active_active {
            let same_size: Vec<String> = source
                .iter()
                .filter(|(key, entry)| target.get(*key).is_some_and(|t| t.size == entry.size))
                .map(|(key, _)| key.clone())
                .collect();
            load_metadata(&self.source, &mut source, &same_size).await?;
            load_metadata(&self.target, &mut target, &same_size).await?;
        }

        let diffs = diff::difference(
            &source,
            &target,
            options.plan.remove || options.plan.active_active,
            compare_metadata,
        );
        let mut actions = diff::plan(&diffs, &source, &options.plan, SystemTime::now());

        if let Some(previous) = previous {
            // Objects deleted from the source while watching are removed from the target
            // (mc removes on source delete events even without --remove).
            for rel in previous.iter().filter(|rel| !source.contains_key(*rel)) {
                let removal = Action::Remove { rel: rel.clone() };
                if target.contains_key(rel)
                    && !options.plan.excluded(rel)
                    && !actions.contains(&removal)
                {
                    actions.push(removal);
                }
            }
            if options.plan.active_active {
                // Objects written by another mirror carry the source-mtime marker; copying
                // them back would loop (mc skips such events).
                let candidates: Vec<String> = actions
                    .iter()
                    .filter_map(|action| match action {
                        Action::Copy { rel, .. } => Some(rel.clone()),
                        _ => None,
                    })
                    .collect();
                load_metadata(&self.source, &mut source, &candidates).await?;
                actions.retain(|action| match action {
                    Action::Copy { rel, .. } => diff::source_mtime_meta(
                        source.get(rel).and_then(|e| e.user_metadata.as_ref()),
                    )
                    .is_none(),
                    _ => true,
                });
            }
        }
        if incomplete {
            actions.retain(|action| !matches!(action, Action::Remove { .. }));
        }

        let source = Arc::new(source);
        let workers = match options.max_workers {
            0 => std::thread::available_parallelism().map_or(4, |n| n.get()),
            n => n,
        };
        let mut tasks: JoinSet<std::result::Result<(), (String, String)>> = JoinSet::new();
        let mut stop = false;
        for action in actions {
            if stop {
                break;
            }
            let event = if initial {
                Event::default()
            } else if matches!(action, Action::Remove { .. }) {
                Event::now("s3:ObjectRemoved:Delete")
            } else {
                Event::now("s3:ObjectCreated:Put")
            };
            match action {
                Action::OverwriteNotAllowed { rel, cond } => {
                    let target = self.target.display(&rel);
                    if self.reported.lock().unwrap().insert(rel) {
                        self.report_error(
                            &format!(
                                "Failed to perform mirroring, with error condition ({})",
                                cond.as_str()
                            ),
                            &format!(
                                "Overwrite not allowed for `{target}`. Use `--overwrite` to override this behavior."
                            ),
                        );
                    }
                    continue;
                }
                Action::Copy { rel, size } => {
                    let count = self.stats.count.fetch_add(1, Ordering::Relaxed) + 1;
                    let total = self.stats.total.fetch_add(size, Ordering::Relaxed) + size;
                    let entry = source.get(&rel).cloned().unwrap_or_default();
                    let job = self.clone();
                    tasks.spawn(async move {
                        job.copy_task(&rel, &entry, (count, total), &event).await
                    });
                }
                Action::Remove { rel } => {
                    let job = self.clone();
                    tasks.spawn(async move { job.remove_task(&rel, &event).await });
                }
            }
            while tasks.len() >= workers {
                if let Some(result) = tasks.join_next().await {
                    stop |= self.handle_result(result, &mut failed);
                }
            }
        }
        while let Some(result) = tasks.join_next().await {
            self.handle_result(result, &mut failed);
        }
        let mut keys: BTreeSet<String> = source.keys().cloned().collect();
        if incomplete && let Some(previous) = previous {
            // Keep tracking entries that could not be read in this pass.
            keys.extend(previous.iter().cloned());
        }
        Ok((keys, failed))
    }

    /// Records a task result; returns true when mirroring should stop.
    fn handle_result(
        &self,
        result: std::result::Result<
            std::result::Result<(), (String, String)>,
            tokio::task::JoinError,
        >,
        failed: &mut bool,
    ) -> bool {
        let error = match result {
            Ok(Ok(())) => return false,
            Ok(Err(error)) => error,
            Err(join) => ("Failed to perform mirroring".to_string(), join.to_string()),
        };
        self.report_error(&error.0, &error.1);
        *failed = true;
        !self.options.skip_errors && !self.options.keep_running()
    }

    /// Creates target buckets missing for alias-root mirrors. Extraneous target buckets are never
    /// dropped: like mc, `--remove` only deletes their objects (one by one, via the normal diff).
    /// Returns whether anything failed.
    async fn sync_buckets(&self) -> Result<bool> {
        let Endpoint::S3(target) = &self.target else {
            return Ok(false);
        };
        if target.bucket.is_some() {
            return Ok(false);
        }
        let options = &self.options;
        let source_buckets: BTreeSet<String> = match &self.source {
            Endpoint::S3(source) => list_buckets(&source.client).await?.into_iter().collect(),
            Endpoint::Local(root) => local_dirs(root)?,
        };
        let target_buckets: BTreeSet<String> =
            list_buckets(&target.client).await?.into_iter().collect();
        let mut failed = false;
        for bucket in source_buckets.difference(&target_buckets) {
            if options.plan.dry_run
                || diff::match_exclude_bucket(&options.plan.exclude_bucket, bucket)
            {
                continue;
            }
            self.print_message(
                &self.source.display(bucket),
                &target.display(bucket),
                0,
                (0, 0),
                &Event::default(),
            );
            if let Err(error) = self.create_bucket(target, bucket).await {
                self.report_error(
                    &format!("Unable to create bucket at `{}`.", target.display(bucket)),
                    &format!("{error:#}"),
                );
                failed = true;
            }
        }
        Ok(failed)
    }

    async fn create_bucket(&self, target: &S3Endpoint, bucket: &str) -> Result<()> {
        // `--preserve` between alias roots copies object lock configuration and policy.
        let source = match &self.source {
            Endpoint::S3(source) if self.options.preserve => Some(source),
            _ => None,
        };
        let mut lock = None;
        if let Some(source) = source
            && let Ok(response) = source
                .client
                .get_object_lock_configuration()
                .bucket(bucket)
                .send()
                .await
        {
            lock = response.object_lock_configuration().cloned();
        }
        crate::s3::make_bucket_with(
            &target.alias,
            bucket,
            &MakeBucketOptions {
                region: Some(self.options.region.clone()),
                with_lock: lock.is_some(),
                ..Default::default()
            },
        )
        .await?;
        if let Some(config) = lock {
            self.locked_buckets
                .lock()
                .unwrap()
                .insert(bucket.to_string());
            if config.rule().is_some() {
                target
                    .client
                    .put_object_lock_configuration()
                    .bucket(bucket)
                    .object_lock_configuration(config)
                    .send()
                    .await
                    .context("Unable to set object lock config")?;
            }
        }
        if let Some(source) = source
            && let Some(policy) = crate::s3::get_bucket_policy(&source.alias, bucket)
                .await
                .ok()
                .flatten()
        {
            crate::s3::put_bucket_policy(&target.alias, bucket, &policy)
                .await
                .context("Unable to copy bucket policies")?;
        }
        Ok(())
    }

    async fn copy_task(
        &self,
        rel: &str,
        entry: &Entry,
        totals: (i64, i64),
        event: &Event,
    ) -> std::result::Result<(), (String, String)> {
        let source_display = self.source.display(rel);
        let target_display = self.target.display(rel);
        if !self.options.summary {
            self.print_message(&source_display, &target_display, entry.size, totals, event);
        }
        if self.options.plan.dry_run {
            self.stats
                .transferred
                .fetch_add(entry.size, Ordering::Relaxed);
            return Ok(());
        }
        let mut attempt = 0;
        loop {
            match self.copy_one(rel, entry).await {
                Ok(()) => {
                    self.stats
                        .transferred
                        .fetch_add(entry.size, Ordering::Relaxed);
                    return Ok(());
                }
                Err(_) if self.options.retry && attempt < 3 => {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_secs(attempt)).await;
                    self.print_retry(&source_display, &target_display, attempt);
                }
                Err(error) => {
                    return Err((
                        format!("Failed to copy `{source_display}`."),
                        format!("{error:#}"),
                    ));
                }
            }
        }
    }

    async fn remove_task(
        &self,
        rel: &str,
        event: &Event,
    ) -> std::result::Result<(), (String, String)> {
        let display = self.target.display(rel);
        let result = if self.options.plan.dry_run {
            Ok(())
        } else {
            match &self.target {
                Endpoint::Local(root) => safe_join(root, rel).and_then(|path| {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("Unable to remove `{}`.", path.display()))
                }),
                Endpoint::S3(s3) => match s3.locate(rel) {
                    Ok((bucket, key)) => s3
                        .client
                        .delete_object()
                        .bucket(bucket)
                        .key(key)
                        .send()
                        .await
                        .map(|_| ())
                        .map_err(Into::into),
                    Err(error) => Err(error),
                },
            }
        };
        match result {
            Ok(()) => {
                self.print_removed(&display, event);
                Ok(())
            }
            Err(error) => Err((
                format!("Failed to remove `{display}`."),
                format!("{error:#}"),
            )),
        }
    }

    fn put_options(&self, bucket: &str, target_display: &str) -> PutOptions {
        let mut checksum = self.options.checksum;
        if checksum.is_none() && self.locked_buckets.lock().unwrap().contains(bucket) {
            checksum = Some(ChecksumAlgo::Crc32);
        }
        PutOptions {
            storage_class: self.options.storage_class.clone(),
            sse: resolve_sse(&self.options.enc, target_display),
            checksum,
            disable_multipart: self.options.disable_multipart,
            ..Default::default()
        }
    }

    fn source_get_options(&self, source_display: &str) -> GetOptions {
        GetOptions {
            sse_c: resolve_sse(&self.options.enc, source_display).and_then(|s| s.customer_key()),
            ..Default::default()
        }
    }

    /// Extra metadata written on the target: `--attr` and the active-active source mtime.
    fn extra_metadata(
        &self,
        entry: &Entry,
        source_meta: Option<&BTreeMap<String, String>>,
    ) -> Vec<(String, String)> {
        let mut pairs = self.options.attr.clone();
        if self.options.plan.active_active {
            let origin = diff::source_mtime_meta(source_meta)
                .map(str::to_string)
                .or_else(|| entry.mtime.map(format_time));
            if let Some(origin) = origin {
                pairs.push((diff::SOURCE_MTIME_KEY.to_string(), origin));
            }
        }
        pairs
    }

    async fn copy_one(&self, rel: &str, entry: &Entry) -> Result<()> {
        let source_display = self.source.display(rel);
        let target_display = self.target.display(rel);
        match (&self.source, &self.target) {
            (Endpoint::Local(root), Endpoint::S3(target)) => {
                let path = root.join(rel);
                let (bucket, key) = target.locate(rel)?;
                let mut put = self.put_options(&bucket, &target_display);
                put.metadata = self.extra_metadata(entry, None);
                if self.options.preserve {
                    let meta = std::fs::metadata(&path)
                        .with_context(|| format!("Unable to stat `{}`.", path.display()))?;
                    put.metadata
                        .push((diff::ATTRS_KEY.to_string(), local_attrs(&meta).encode()));
                }
                let file = tokio::fs::File::open(&path)
                    .await
                    .with_context(|| format!("Unable to read local file `{}`.", path.display()))?;
                let size = u64::try_from(entry.size).ok();
                crate::s3::upload_stream(&target.client, &bucket, &key, file, size, &put).await?;
            }
            (Endpoint::S3(source), Endpoint::Local(root)) => {
                let (bucket, key) = source.locate(rel)?;
                let dest = safe_join(root, rel)?;
                let get = self.source_get_options(&source_display);
                let response = crate::s3::get_object(&source.client, &bucket, &key, &get).await?;
                let attrs = self
                    .options
                    .preserve
                    .then(|| {
                        response
                            .metadata()
                            .and_then(|m| m.get(diff::ATTRS_KEY))
                            .map(|value| FileAttrs::parse(value))
                    })
                    .flatten();
                let mut reader = response.body.into_async_read();
                write_atomically(&dest, async |file| {
                    tokio::io::copy(&mut reader, file).await?;
                    Ok(())
                })
                .await?;
                if let Some(attrs) = attrs {
                    apply_attrs(&dest, &attrs);
                }
            }
            (Endpoint::S3(source), Endpoint::S3(target)) => {
                self.copy_s3(source, target, rel, entry, &source_display, &target_display)
                    .await?;
            }
            (Endpoint::Local(src_root), Endpoint::Local(dst_root)) => {
                let src = src_root.join(rel);
                let dest = safe_join(dst_root, rel)?;
                let mut input = tokio::fs::File::open(&src)
                    .await
                    .with_context(|| format!("Unable to read local file `{}`.", src.display()))?;
                write_atomically(&dest, async |file| {
                    tokio::io::copy(&mut input, file).await?;
                    Ok(())
                })
                .await?;
                if self.options.preserve {
                    apply_attrs(&dest, &local_attrs(&std::fs::metadata(&src)?));
                }
            }
        }
        Ok(())
    }

    async fn copy_s3(
        &self,
        source: &S3Endpoint,
        target: &S3Endpoint,
        rel: &str,
        entry: &Entry,
        source_display: &str,
        target_display: &str,
    ) -> Result<()> {
        let (src_bucket, src_key) = source.locate(rel)?;
        let (dst_bucket, dst_key) = target.locate(rel)?;
        let get = self.source_get_options(source_display);
        let mut put = self.put_options(&dst_bucket, target_display);
        let src_ref = ObjectRef {
            alias: &source.alias,
            bucket: &src_bucket,
            key: &src_key,
        };
        let dst_ref = ObjectRef {
            alias: &target.alias,
            bucket: &dst_bucket,
            key: &dst_key,
        };
        let same_server =
            crate::s3::client::same_endpoint_and_credentials(&source.alias, &target.alias);
        if same_server && entry.size <= MAX_COPY_OBJECT_SIZE {
            if !self.options.attr.is_empty() || self.options.plan.active_active {
                // Metadata is replaced: carry the source's metadata over explicitly.
                let head = crate::s3::head_object_with(&source.client, &src_bucket, &src_key, &get)
                    .await?;
                let user = head.metadata().map(to_btree);
                put.metadata = inherited_metadata(
                    [
                        ("Content-Type", head.content_type()),
                        ("Cache-Control", head.cache_control()),
                        ("Content-Encoding", head.content_encoding()),
                        ("Content-Disposition", head.content_disposition()),
                        ("Content-Language", head.content_language()),
                    ],
                    user.as_ref(),
                );
                put.metadata
                    .extend(self.extra_metadata(entry, user.as_ref()));
            }
            crate::s3::server_side_copy(&target.client, src_ref, dst_ref, &get, &put).await?;
            return Ok(());
        }
        let response = crate::s3::get_object(&source.client, &src_bucket, &src_key, &get).await?;
        let user = response.metadata().map(to_btree);
        put.metadata = inherited_metadata(
            [
                ("Content-Type", response.content_type()),
                ("Cache-Control", response.cache_control()),
                ("Content-Encoding", response.content_encoding()),
                ("Content-Disposition", response.content_disposition()),
                ("Content-Language", response.content_language()),
            ],
            user.as_ref(),
        );
        put.metadata
            .extend(self.extra_metadata(entry, user.as_ref()));
        let size = response
            .content_length()
            .and_then(|v| u64::try_from(v).ok());
        let reader = response.body.into_async_read();
        crate::s3::upload_stream(&target.client, &dst_bucket, &dst_key, reader, size, &put).await?;
        Ok(())
    }

    // ------------------------------------------------------------------------------------
    // output
    // ------------------------------------------------------------------------------------

    fn print_message(
        &self,
        source: &str,
        target: &str,
        size: i64,
        totals: (i64, i64),
        event: &Event,
    ) {
        if self.options.json {
            let message = MirrorMessage {
                status: "success",
                source,
                target,
                size,
                total_count: totals.0,
                total_size: totals.1,
                event_time: &event.time,
                event_type: event.kind,
            };
            let _ = crate::output::print_json(&message);
        } else if event.time.is_empty() {
            crate::output::print_plain(&format!("`{source}` -> `{target}`"));
        } else {
            crate::output::print_plain(&format!(
                "[{}] {:>6} `{source}` -> `{target}`",
                event.time,
                diff::humanize_ibytes(size.max(0) as u64)
            ));
        }
    }

    fn print_removed(&self, target: &str, event: &Event) {
        if self.options.json {
            let message = MirrorMessage {
                status: "success",
                source: "",
                target,
                size: 0,
                total_count: self.stats.count.load(Ordering::Relaxed),
                total_size: self.stats.total.load(Ordering::Relaxed),
                event_time: &event.time,
                event_type: "s3:ObjectRemoved:Delete",
            };
            let _ = crate::output::print_json(&message);
        } else if event.time.is_empty() {
            crate::output::print_plain(&format!("Removed `{target}`"));
        } else {
            crate::output::print_plain(&format!("[{}] Removed `{target}`", event.time));
        }
    }

    fn print_retry(&self, source: &str, target: &str, retries: u64) {
        if self.options.json {
            let message = serde_json::json!({
                "sourceURL": source,
                "targetURL": target,
                "retries": retries,
            });
            let _ = crate::output::print_json(&message);
        } else {
            crate::output::print_plain(&format!(
                "<INFO> Retries {retries}: source `{source}` >> target `{target}`"
            ));
        }
    }

    /// mc `errorIf`: JSON error document on stdout, or `<ERROR>` line on stderr.
    fn report_error(&self, message: &str, cause: &str) {
        crate::output::error_if(message, cause);
    }

    fn print_summary(&self, elapsed: Duration) -> Result<()> {
        let total = self.stats.total.load(Ordering::Relaxed);
        let transferred = self.stats.transferred.load(Ordering::Relaxed);
        let speed = if elapsed.as_secs_f64() > 0.0 && transferred > 0 {
            transferred as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };
        if self.options.json {
            let summary = serde_json::json!({
                "status": "success",
                "total": total,
                "transferred": transferred,
                "duration": elapsed.as_nanos() as u64,
                "speed": speed,
            });
            crate::output::print_json(&summary)?;
            return Ok(());
        }
        use std::io::Write;
        let mut table = tabwriter::TabWriter::new(Vec::new()).padding(2);
        writeln!(table, "Total\tTransferred\tDuration\tSpeed")?;
        writeln!(
            table,
            "{}\t{}\t{:.3}s\t{}/s",
            format_bytes(total),
            format_bytes(transferred),
            elapsed.as_secs_f64(),
            format_bytes(speed as i64)
        )?;
        table.flush()?;
        print!("{}", String::from_utf8(table.into_inner()?)?);
        Ok(())
    }
}

// ----------------------------------------------------------------------------------------
// listing
// ----------------------------------------------------------------------------------------

/// Lists an endpoint. The second value holds per-entry errors (local walks only): the listing is
/// then incomplete and must not be used to propagate deletions.
async fn list(endpoint: &Endpoint, options: &Options) -> Result<(Listing, Vec<String>)> {
    match endpoint {
        Endpoint::Local(root) => list_local(root),
        Endpoint::S3(s3) => match &s3.bucket {
            Some(bucket) => list_bucket(&s3.client, bucket, &s3.prefix, "")
                .await
                .map(|listing| (listing, Vec::new()))
                .with_context(|| format!("Unable to list `{}`.", s3.display(""))),
            None => {
                let mut listing = Listing::new();
                for bucket in list_buckets(&s3.client).await? {
                    if diff::match_exclude_bucket(&options.plan.exclude_bucket, &bucket) {
                        continue;
                    }
                    listing.extend(
                        list_bucket(&s3.client, &bucket, "", &format!("{bucket}/"))
                            .await
                            .with_context(|| {
                                format!("Unable to list `{}`.", s3.display(&bucket))
                            })?,
                    );
                }
                Ok((listing, Vec::new()))
            }
        },
    }
}

async fn list_buckets(client: &Client) -> Result<Vec<String>> {
    let response = client.list_buckets().send().await?;
    Ok(response
        .buckets()
        .iter()
        .filter_map(|bucket| bucket.name().map(str::to_string))
        .collect())
}

async fn list_bucket(
    client: &Client,
    bucket: &str,
    prefix: &str,
    rel_prefix: &str,
) -> Result<Listing> {
    let items = crate::s3::list_objects_with(
        client,
        bucket,
        Some(prefix).filter(|p| !p.is_empty()),
        &ListOptions {
            recursive: true,
            ..Default::default()
        },
    )
    .await?;
    Ok(items
        .into_iter()
        .filter(|item| !item.is_prefix && !item.key.is_empty() && !item.key.ends_with('/'))
        .map(|item| {
            (
                format!("{rel_prefix}{}", item.key),
                Entry {
                    size: item.size,
                    mtime: item.last_modified,
                    storage_class: item.storage_class,
                    user_metadata: None,
                },
            )
        })
        .collect())
}

fn list_local(root: &Path) -> Result<(Listing, Vec<String>)> {
    let mut listing = Listing::new();
    let mut errors = Vec::new();
    if root.is_dir() {
        walk_local(root, root, &mut listing, &mut errors)?;
    }
    Ok((listing, errors))
}

/// Unreadable entries are recorded in `errors` (never silently skipped: a missing entry would be
/// treated as deleted).
fn walk_local(
    root: &Path,
    dir: &Path,
    listing: &mut Listing,
    errors: &mut Vec<String>,
) -> Result<()> {
    for item in std::fs::read_dir(dir)
        .with_context(|| format!("Unable to read directory `{}`.", dir.display()))?
    {
        let item = item?;
        let path = item.path();
        let file_type = item.file_type()?;
        if file_type.is_dir() {
            walk_local(root, &path, listing, errors)?;
            continue;
        }
        // Symlinks to files are followed; dangling symlinks and anything else are skipped.
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(error)
                if file_type.is_symlink() && error.kind() == std::io::ErrorKind::NotFound =>
            {
                continue;
            }
            Err(error) => {
                errors.push(format!("Unable to stat `{}`: {error}", path.display()));
                continue;
            }
        };
        if !meta.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(root)?
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        listing.insert(
            rel,
            Entry {
                size: meta.len() as i64,
                mtime: meta.modified().ok(),
                storage_class: None,
                user_metadata: None,
            },
        );
    }
    Ok(())
}

fn local_dirs(root: &Path) -> Result<BTreeSet<String>> {
    let mut dirs = BTreeSet::new();
    if root.is_dir() {
        for item in std::fs::read_dir(root)? {
            let item = item?;
            if item.file_type()?.is_dir() {
                dirs.insert(item.file_name().to_string_lossy().to_string());
            }
        }
    }
    Ok(dirs)
}

/// Fetches user metadata (HeadObject) for `keys` on S3 endpoints.
async fn load_metadata(endpoint: &Endpoint, listing: &mut Listing, keys: &[String]) -> Result<()> {
    let Endpoint::S3(s3) = endpoint else {
        return Ok(());
    };
    for key in keys {
        let Some(entry) = listing.get_mut(key) else {
            continue;
        };
        if entry.user_metadata.is_some() {
            continue;
        }
        let (bucket, object) = s3.locate(key)?;
        let response = s3
            .client
            .head_object()
            .bucket(bucket)
            .key(object)
            .send()
            .await
            .with_context(|| format!("Unable to stat `{}`.", s3.display(key)))?;
        entry.user_metadata = Some(response.metadata().map(to_btree).unwrap_or_default());
    }
    Ok(())
}

// ----------------------------------------------------------------------------------------
// helpers
// ----------------------------------------------------------------------------------------

fn to_btree(map: &HashMap<String, String>) -> BTreeMap<String, String> {
    map.iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
        .collect()
}

/// Standard headers and user metadata of the source object as `--attr`-style pairs.
fn inherited_metadata(
    headers: [(&str, Option<&str>); 5],
    user: Option<&BTreeMap<String, String>>,
) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(name, value)| value.map(|v| (name.to_string(), v.to_string())))
        .collect();
    if let Some(user) = user {
        pairs.extend(
            user.iter()
                .filter(|(key, _)| key.as_str() != diff::SOURCE_MTIME_KEY)
                .map(|(k, v)| (k.clone(), v.clone())),
        );
    }
    pairs
}

/// RFC3339 with fractional seconds (Go `time.RFC3339Nano` style).
fn format_time(time: SystemTime) -> String {
    DateTime::from(time)
        .fmt(DateTimeFormat::DateTime)
        .unwrap_or_default()
}

/// cheggaaa/pb byte formatting used by mc's accounting summary.
fn format_bytes(size: i64) -> String {
    const KIB: f64 = 1024.0;
    let value = size as f64;
    for (unit, scale) in [
        ("TiB", KIB.powi(4)),
        ("GiB", KIB.powi(3)),
        ("MiB", KIB.powi(2)),
        ("KiB", KIB),
    ] {
        if value >= scale {
            return format!("{:.2} {unit}", value / scale);
        }
    }
    format!("{size} B")
}

/// Joins a relative object path under a local root, rejecting `..`/absolute components.
fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel_path = Path::new(rel);
    if rel.is_empty()
        || !rel_path
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        bail!("Refusing to write object `{rel}` outside of the target folder.");
    }
    Ok(root.join(rel_path))
}

/// Writes `dest` via a temporary `.part.minio` file and renames it into place.
async fn write_atomically<F>(dest: &Path, write: F) -> Result<()>
where
    F: AsyncFnOnce(&mut tokio::fs::File) -> Result<()>,
{
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Unable to create folder `{}`.", parent.display()))?;
    }
    let mut part = dest.as_os_str().to_owned();
    part.push(".part.minio");
    let part = PathBuf::from(part);
    let mut file = tokio::fs::File::create(&part)
        .await
        .with_context(|| format!("Unable to write local file `{}`.", part.display()))?;
    let result = async {
        write(&mut file).await?;
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);
        tokio::fs::rename(&part, dest)
            .await
            .with_context(|| format!("Unable to write local file `{}`.", dest.display()))
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
}

#[cfg(unix)]
fn local_attrs(meta: &std::fs::Metadata) -> FileAttrs {
    use std::os::unix::fs::MetadataExt;
    FileAttrs {
        atime: Some((meta.atime(), meta.atime_nsec())),
        mtime: Some((meta.mtime(), meta.mtime_nsec())),
        mode: Some(meta.mode()),
        uid: Some(meta.uid()),
        gid: Some(meta.gid()),
    }
}

#[cfg(not(unix))]
fn local_attrs(meta: &std::fs::Metadata) -> FileAttrs {
    let secs = |t: std::io::Result<SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
    };
    FileAttrs {
        atime: secs(meta.accessed()),
        mtime: secs(meta.modified()),
        ..Default::default()
    }
}

/// Restores preserved attributes (best effort, like mc: failures do not fail the copy).
fn apply_attrs(path: &Path, attrs: &FileAttrs) {
    let to_time = |(secs, nanos): (i64, i64)| -> Option<SystemTime> {
        Some(UNIX_EPOCH + Duration::new(u64::try_from(secs).ok()?, u32::try_from(nanos).ok()?))
    };
    let mut times = std::fs::FileTimes::new();
    let mut set = false;
    if let Some(time) = attrs.mtime.and_then(to_time) {
        times = times.set_modified(time);
        set = true;
    }
    if let Some(time) = attrs.atime.and_then(to_time) {
        times = times.set_accessed(time);
        set = true;
    }
    if set && let Ok(file) = std::fs::File::open(path) {
        let _ = file.set_times(times);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if attrs.uid.is_some() || attrs.gid.is_some() {
            let _ = std::os::unix::fs::chown(path, attrs.uid, attrs.gid);
        }
        if let Some(mode) = attrs.mode {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_escapes() {
        let root = Path::new("/tmp/root");
        assert_eq!(safe_join(root, "a/b.txt").unwrap(), root.join("a/b.txt"));
        assert!(safe_join(root, "../x").is_err());
        assert!(safe_join(root, "a/../../x").is_err());
        assert!(safe_join(root, "/etc/passwd").is_err());
        assert!(safe_join(root, "").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn local_walk_reports_unreadable_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/ok.txt"), "ok").unwrap();
        // Dangling symlink: not a file, skipped without error.
        std::os::unix::fs::symlink("missing", dir.path().join("dangling")).unwrap();
        let (listing, errors) = list_local(dir.path()).unwrap();
        assert_eq!(listing.keys().collect::<Vec<_>>(), ["sub/ok.txt"]);
        assert!(errors.is_empty(), "{errors:?}");

        // Symlink loop: stat fails with ELOOP; the scan must be flagged incomplete.
        std::os::unix::fs::symlink("loop", dir.path().join("loop")).unwrap();
        let (listing, errors) = list_local(dir.path()).unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("loop"), "{errors:?}");
    }

    #[test]
    fn formats_summary_bytes() {
        assert_eq!(format_bytes(12), "12 B");
        assert_eq!(format_bytes(2048), "2.00 KiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 + 512 * 1024), "3.50 MiB");
    }

    #[test]
    fn inherits_source_metadata() {
        let user = BTreeMap::from([
            ("owner".to_string(), "alice".to_string()),
            (diff::SOURCE_MTIME_KEY.to_string(), "t".to_string()),
        ]);
        let pairs = inherited_metadata(
            [
                ("Content-Type", Some("text/plain")),
                ("Cache-Control", None),
                ("Content-Encoding", None),
                ("Content-Disposition", None),
                ("Content-Language", None),
            ],
            Some(&user),
        );
        assert_eq!(
            pairs,
            vec![
                ("Content-Type".to_string(), "text/plain".to_string()),
                ("owner".to_string(), "alice".to_string()),
            ]
        );
    }

    #[test]
    fn alias_root_locate_splits_bucket() {
        let endpoint = S3Endpoint {
            alias_name: "a".into(),
            alias: AliasConfig::default(),
            client: Client::from_conf(
                aws_sdk_s3::Config::builder()
                    .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
                    .build(),
            ),
            bucket: None,
            prefix: String::new(),
        };
        assert_eq!(
            endpoint.locate("b/k/x").unwrap(),
            ("b".to_string(), "k/x".to_string())
        );
        assert!(endpoint.locate("toplevel.txt").is_err());
        assert_eq!(endpoint.display("b/k"), "a/b/k");
        let bucket = S3Endpoint {
            bucket: Some("bkt".into()),
            prefix: "p/".into(),
            ..endpoint
        };
        assert_eq!(bucket.locate("x").unwrap(), ("bkt".into(), "p/x".into()));
        assert_eq!(bucket.display("x"), "a/bkt/p/x");
    }
}
