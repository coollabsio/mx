use crate::commands::stat::{print_date, rfc3339};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::{RewindFlag, TimeFilterFlags, VersionIdFlag};
use crate::s3::{DeleteOutcome, DeleteTarget, ListOptions, ObjectInfo, full_key};
use crate::target::TargetRef;
use anyhow::{Result, anyhow, bail};
use aws_sdk_s3::Client;
use clap::Args;
use std::io::BufRead;
use std::time::SystemTime;

const FORCE_REQUIRED: &str = "Removal requires --force flag. This operation is *IRREVERSIBLE*. Please review carefully before performing this *DANGEROUS* operation.";
const RECURSIVE_REQUIRED: &str = "Removal requires --recursive flag. This operation is *IRREVERSIBLE*. Please review carefully before performing this *DANGEROUS* operation.";
const DANGEROUS_REQUIRED: &str = "This operation results in site-wide removal of objects. If you are really sure, retry this command with ‘--dangerous’ and ‘--force’ flags.";

#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// remove object(s) and all its versions
    #[arg(long)]
    pub versions: bool,
    /// remove recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
    /// allow a recursive remove operation
    #[arg(long)]
    pub force: bool,
    /// allow site-wide removal of objects
    #[arg(long)]
    pub dangerous: bool,
    #[command(flatten)]
    pub rewind: RewindFlag,
    #[command(flatten)]
    pub version_id: VersionIdFlag,
    /// remove incomplete uploads
    #[arg(short = 'I', long)]
    pub incomplete: bool,
    /// perform a fake remove operation
    #[arg(long, alias = "fake")]
    pub dry_run: bool,
    /// read object names from STDIN
    #[arg(long)]
    pub stdin: bool,
    #[command(flatten)]
    pub time: TimeFilterFlags,
    /// bypass governance
    #[arg(long)]
    pub bypass: bool,
    /// remove object(s) versions that are non-current
    #[arg(long)]
    pub non_current: bool,
    /// attempt a prefix purge (MinIO force delete); requires --force
    #[arg(long, hide = true)]
    pub purge: bool,
    #[arg(value_name = "TARGET", required_unless_present = "stdin")]
    pub targets: Vec<String>,
}

/// Validates mc's flag combination rules for `rm` (targets from `--stdin` are checked later).
pub fn validate(args: &RemoveArgs) -> Result<()> {
    let has_rewind = args.rewind.rewind.is_some();
    if args.version_id.version_id.is_some() && (args.recursive || args.versions || has_rewind) {
        bail!(
            "You cannot specify --version-id with any of --versions, --rewind and --recursive flags."
        );
    }
    if args.non_current && (!args.versions || !args.recursive) {
        bail!(
            "You cannot specify --non-current without --versions --recursive, please use --non-current --versions --recursive."
        );
    }
    if args.purge && !args.force {
        bail!("You cannot specify --purge without --force.");
    }
    if args.purge && args.recursive {
        bail!("You cannot specify --purge with --recursive.");
    }
    if args.purge
        && (args.non_current
            || args.versions
            || args.time.is_set()
            || args.version_id.version_id.is_some())
    {
        bail!("You cannot specify --purge flag with any flag(s) other than --force.");
    }
    if args.incomplete
        && (args.versions
            || has_rewind
            || args.version_id.version_id.is_some()
            || args.purge
            || args.bypass)
    {
        bail!(
            "You cannot specify --incomplete with --versions, --rewind, --version-id, --purge or --bypass."
        );
    }
    if has_rewind && !args.recursive && !args.versions {
        bail!("You cannot specify --rewind without --recursive or --versions.");
    }
    if (args.versions || args.recursive || args.stdin) && !args.force {
        bail!(FORCE_REQUIRED);
    }
    args.time.parsed()?;
    for target in &args.targets {
        check_target(args, &TargetRef::parse(target)?)?;
    }
    Ok(())
}

/// Per-target rules: alias-wide removal needs `--dangerous --force`; buckets need `-r`.
fn check_target(args: &RemoveArgs, target: &TargetRef) -> Result<()> {
    if target.is_alias_root() {
        if !args.dangerous || !args.force {
            bail!(DANGEROUS_REQUIRED);
        }
        if !args.recursive {
            bail!(RECURSIVE_REQUIRED);
        }
    } else if target.key.is_none() && !args.recursive {
        bail!(RECURSIVE_REQUIRED);
    }
    Ok(())
}

pub fn run(args: RemoveArgs, json: bool) -> Result<()> {
    validate(&args)?;
    let now = SystemTime::now();
    let remover = Remover {
        rewind: args.rewind.at(now)?,
        now,
        json,
        store: ConfigStore::load_or_create()?,
        rt: runtime()?,
        args: &args,
    };

    let mut errors = Vec::new();
    for target in &args.targets {
        if let Err(error) = remover.remove(target) {
            errors.push(error);
        }
    }
    if args.stdin {
        for line in std::io::stdin().lock().lines() {
            let line = line?;
            let target = line.trim();
            if target.is_empty() {
                continue;
            }
            if let Err(error) = remover.remove(target) {
                errors.push(error);
            }
        }
    }
    let Some(last) = errors.pop() else {
        return Ok(());
    };
    for error in errors {
        report(&error);
    }
    Err(last)
}

/// Reports a failure that does not stop the command (mc `errorIf`).
fn report(error: &anyhow::Error) {
    crate::output::print_error(error);
}

struct Remover<'a> {
    args: &'a RemoveArgs,
    rewind: Option<SystemTime>,
    now: SystemTime,
    json: bool,
    store: ConfigStore,
    rt: tokio::runtime::Runtime,
}

impl Remover<'_> {
    fn remove(&self, input: &str) -> Result<()> {
        let target = TargetRef::parse(input)?;
        check_target(self.args, &target)?;
        let alias = alias_config(&self.store, &target.alias)?;
        self.rt.block_on(async {
            let client = crate::s3::build_client(&alias).await?;
            let Some(bucket) = &target.bucket else {
                let response = client.list_buckets().send().await?;
                let mut result = Ok(());
                for entry in response.buckets() {
                    let bucket = entry.name().unwrap_or_default();
                    let removed = self
                        .list_and_remove(&client, &target.alias, bucket, None)
                        .await
                        .map(|_| ());
                    if let Err(error) = removed {
                        report(&error);
                        result = Err(anyhow!("Failed to remove `{input}` recursively."));
                    }
                }
                return result;
            };
            if self.args.recursive {
                let prefix = target.key_with_trailing_slash();
                let found = self
                    .list_and_remove(&client, &target.alias, bucket, prefix.as_deref())
                    .await?;
                match prefix.filter(|key| !found && !key.ends_with('/')) {
                    // `rm -r alias/bucket/file`: nothing under `file/`, remove the object itself.
                    Some(key) if self.args.versions || self.rewind.is_some() => {
                        let versions = crate::s3::list_key_versions(&client, bucket, &key).await?;
                        if versions.is_empty() {
                            bail!("Failed to remove `{input}`: object does not exist.");
                        }
                        let selected = select_versions(versions, self.args, self.rewind);
                        self.remove_entries(&client, &target.alias, bucket, selected)
                            .await
                    }
                    Some(key) => {
                        self.remove_single(&client, &target.alias, bucket, &key, input)
                            .await
                    }
                    None => Ok(()),
                }
            } else {
                let key = target.require_object_key()?;
                if self.args.versions {
                    self.remove_all_versions(&client, &target.alias, bucket, &key)
                        .await
                } else {
                    self.remove_single(&client, &target.alias, bucket, &key, input)
                        .await
                }
            }
        })
    }

    /// `rm [--version-id V] [--purge] [-I] ALIAS/BUCKET/KEY`
    async fn remove_single(
        &self,
        client: &Client,
        alias: &str,
        bucket: &str,
        key: &str,
        input: &str,
    ) -> Result<()> {
        let args = self.args;
        if args.incomplete {
            let uploads = crate::s3::list_key_uploads(client, bucket, key).await?;
            if uploads.is_empty() {
                bail!("Failed to remove `{input}`: no incomplete upload found.");
            }
            return self.remove_entries(client, alias, bucket, uploads).await;
        }
        let version_id = args.version_id.version_id.as_deref();
        if !args.purge {
            let head = client
                .head_object()
                .bucket(bucket)
                .key(key)
                .set_version_id(version_id.map(str::to_string))
                .send()
                .await;
            let (modified, head_version) = match head {
                Ok(head) => (
                    head.last_modified().and_then(crate::s3::to_system_time),
                    head.version_id().map(str::to_string),
                ),
                // 400: SSE-C object, 405: delete marker version. Remove it anyway.
                Err(error)
                    if error
                        .raw_response()
                        .is_some_and(|raw| matches!(raw.status().as_u16(), 400 | 405)) =>
                {
                    if args.time.is_set() {
                        bail!("Unable to stat `{input}`.");
                    }
                    (None, version_id.map(str::to_string))
                }
                Err(error) => {
                    return Err(
                        anyhow::Error::from(error).context(format!("Failed to remove `{input}`."))
                    );
                }
            };
            if !args.time.matches(modified, self.now) {
                return Ok(());
            }
            if args.dry_run {
                self.print(&RemoveMessage {
                    key: format!("{alias}/{bucket}/{key}"),
                    version_id: head_version,
                    dry_run: true,
                    ..Default::default()
                });
                return Ok(());
            }
        }
        let outcome =
            crate::s3::delete_object_with(client, bucket, key, version_id, args.bypass, args.purge)
                .await
                .map_err(|error| error.context(format!("Failed to remove `{input}`.")))?;
        self.print(&RemoveMessage::from_outcome(alias, bucket, &outcome));
        Ok(())
    }

    /// `rm --versions --force ALIAS/BUCKET/KEY` (non-recursive): every version of one key.
    async fn remove_all_versions(
        &self,
        client: &Client,
        alias: &str,
        bucket: &str,
        key: &str,
    ) -> Result<()> {
        let versions = crate::s3::list_key_versions(client, bucket, key).await?;
        let selected = select_versions(versions, self.args, self.rewind);
        self.remove_entries(client, alias, bucket, selected).await
    }

    /// Recursive removal under `prefix` (whole bucket when `None`). Returns whether anything was
    /// listed.
    async fn list_and_remove(
        &self,
        client: &Client,
        alias: &str,
        bucket: &str,
        prefix: Option<&str>,
    ) -> Result<bool> {
        let args = self.args;
        let base = crate::s3::normalize_prefix(prefix).unwrap_or_default();
        let options = ListOptions {
            recursive: true,
            incomplete: args.incomplete,
            versions: args.versions || self.rewind.is_some(),
            ..Default::default()
        };
        let mut items = crate::s3::list_objects_with(client, bucket, prefix, &options).await?;
        if options.versions {
            items = select_versions(items, args, self.rewind);
        }
        for item in &mut items {
            item.key = full_key(&base, &item.key);
        }
        let found = !items.is_empty();
        self.remove_entries(client, alias, bucket, items).await?;
        Ok(found)
    }

    /// Applies time filters, then prints (dry run), aborts (incomplete) or bulk-deletes.
    async fn remove_entries(
        &self,
        client: &Client,
        alias: &str,
        bucket: &str,
        items: Vec<ObjectInfo>,
    ) -> Result<()> {
        let args = self.args;
        let items: Vec<_> = items
            .into_iter()
            .filter(|item| !item.is_prefix && item.last_modified.is_some())
            .filter(|item| args.time.matches(item.last_modified, self.now))
            .collect();
        let with_mod_time = args.versions;
        if args.dry_run {
            for item in &items {
                self.print(&RemoveMessage {
                    key: format!("{alias}/{bucket}/{}", item.key),
                    version_id: item.version_id.clone(),
                    mod_time: item.last_modified.filter(|_| with_mod_time),
                    dry_run: true,
                    ..Default::default()
                });
            }
            return Ok(());
        }
        let mut failed = 0;
        if args.incomplete {
            for item in &items {
                let upload_id = item.upload_id.as_deref().unwrap_or_default();
                match crate::s3::abort_upload(client, bucket, &item.key, upload_id).await {
                    Ok(()) => self.print(&RemoveMessage {
                        key: format!("{alias}/{bucket}/{}", item.key),
                        ..Default::default()
                    }),
                    Err(error) => {
                        failed += 1;
                        report(
                            &error.context(format!(
                                "Failed to remove `{alias}/{bucket}/{}`.",
                                item.key
                            )),
                        );
                    }
                }
            }
        } else {
            let targets: Vec<_> = items
                .iter()
                .map(|item| DeleteTarget {
                    key: item.key.clone(),
                    version_id: item.version_id.clone(),
                })
                .collect();
            let outcomes =
                crate::s3::delete_versions(client, bucket, &targets, args.bypass).await?;
            for outcome in &outcomes {
                match &outcome.error {
                    None => self.print(&RemoveMessage::from_outcome(alias, bucket, outcome)),
                    Some(error) => {
                        failed += 1;
                        report(&anyhow!(
                            "Failed to remove `{alias}/{bucket}/{}`: {error}",
                            outcome.key
                        ));
                    }
                }
            }
        }
        if failed > 0 {
            bail!("Failed to remove {failed} object(s) in `{alias}/{bucket}`.");
        }
        Ok(())
    }

    fn print(&self, message: &RemoveMessage) {
        if self.json {
            println!("{}", message.json());
        } else {
            crate::output::print_plain(&message.text());
        }
    }
}

/// mc version selection for `rm --versions` / `--rewind` / `--non-current`.
///
/// * `--versions`: every version and delete marker (modified at or before `--rewind` if set)
/// * `--rewind` alone: per key, the version or delete marker that was current at that time
/// * `--non-current`: drops each key's latest version unless it is a delete marker
pub fn select_versions(
    versions: Vec<ObjectInfo>,
    args: &RemoveArgs,
    rewind: Option<SystemTime>,
) -> Vec<ObjectInfo> {
    let mut selected = match (args.versions, rewind) {
        (true, Some(at)) => crate::s3::versions_before(versions, at),
        (true, None) => versions,
        (false, Some(at)) => current_at(versions, at),
        (false, None) => versions,
    };
    if args.non_current {
        selected.retain(|item| !item.is_latest || item.is_delete_marker);
    }
    selected
}

/// Per key, the newest version or delete marker modified at or before `at`.
fn current_at(versions: Vec<ObjectInfo>, at: SystemTime) -> Vec<ObjectInfo> {
    let mut picked: Vec<ObjectInfo> = Vec::new();
    for item in crate::s3::versions_before(versions, at) {
        match picked.last_mut() {
            Some(last) if last.key == item.key => {
                if item.last_modified > last.last_modified {
                    *last = item;
                }
            }
            _ => picked.push(item),
        }
    }
    picked
}

#[derive(Debug, Default)]
struct RemoveMessage {
    key: String,
    delete_marker: bool,
    version_id: Option<String>,
    mod_time: Option<SystemTime>,
    dry_run: bool,
}

impl RemoveMessage {
    fn from_outcome(alias: &str, bucket: &str, outcome: &DeleteOutcome) -> Self {
        let created_marker = outcome.delete_marker && outcome.version_id.is_none();
        Self {
            key: format!("{alias}/{bucket}/{}", outcome.key),
            delete_marker: created_marker,
            version_id: if created_marker {
                outcome.delete_marker_version_id.clone()
            } else {
                outcome.version_id.clone()
            },
            ..Default::default()
        }
    }

    /// mc text: ``Removed `k`.``, ``DRYRUN: Removing `k`.``, ``Created delete marker `k` (versionId=v).``
    fn text(&self) -> String {
        let mut message = if self.delete_marker {
            "Created delete marker ".to_string()
        } else if self.dry_run {
            "DRYRUN: Removing ".to_string()
        } else {
            "Removed ".to_string()
        };
        message.push_str(&format!("`{}`", self.key));
        if let Some(version_id) = self.version_id.as_deref().filter(|v| !v.is_empty()) {
            message.push_str(&format!(" (versionId={version_id})"));
            if let Some(mod_time) = self.mod_time {
                message.push_str(&format!(" (modTime={})", print_date(mod_time)));
            }
        }
        message.push('.');
        message
    }

    fn json(&self) -> String {
        let value = serde_json::json!({
            "status": "success",
            "key": self.key,
            "deleteMarker": self.delete_marker,
            "versionID": self.version_id.clone().unwrap_or_default(),
            "modTime": self.mod_time.map(rfc3339),
            "dryRun": self.dry_run,
        });
        crate::output::json_string(&value).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::time::{Duration, UNIX_EPOCH};

    #[derive(Debug, Parser)]
    struct Harness {
        #[command(flatten)]
        args: RemoveArgs,
    }

    fn parse(args: &[&str]) -> RemoveArgs {
        let argv = crate::flags::rewrite_argv(std::iter::once("rm").chain(args.iter().copied()));
        Harness::try_parse_from(argv).unwrap().args
    }

    fn check(args: &[&str]) -> std::result::Result<(), String> {
        validate(&parse(args)).map_err(|error| error.to_string())
    }

    #[test]
    fn enforces_mc_flag_rules() {
        assert!(check(&["a/b/k"]).is_ok());
        assert!(
            check(&["--vid", "v1", "--versions", "--force", "a/b/k"])
                .unwrap_err()
                .contains("--version-id with any of")
        );
        assert!(
            check(&["--non-current", "--versions", "--force", "a/b/k"])
                .unwrap_err()
                .contains("--non-current without --versions --recursive")
        );
        assert!(check(&["--non-current", "--versions", "-r", "--force", "a/b/"]).is_ok());
        assert!(
            check(&["--purge", "a/b/k"])
                .unwrap_err()
                .contains("without --force")
        );
        assert!(
            check(&["--purge", "--force", "-r", "a/b/k"])
                .unwrap_err()
                .contains("--purge with --recursive")
        );
        assert!(
            check(&["--purge", "--force", "--older-than", "1d", "a/b/k"])
                .unwrap_err()
                .contains("other than --force")
        );
        assert!(check(&["--purge", "--force", "a/b/k"]).is_ok());
        assert!(
            check(&["-r", "a/b/"])
                .unwrap_err()
                .contains("requires --force")
        );
        assert!(
            check(&["--versions", "a/b/k"])
                .unwrap_err()
                .contains("requires --force")
        );
        assert!(check(&["--stdin", "--force"]).is_ok());
        assert!(
            check(&["--stdin"])
                .unwrap_err()
                .contains("requires --force")
        );
        assert!(
            check(&["-r", "--force", "a"])
                .unwrap_err()
                .contains("‘--dangerous’")
        );
        assert!(check(&["-r", "--force", "--dangerous", "a"]).is_ok());
        assert!(
            check(&["--force", "a/b"])
                .unwrap_err()
                .contains("requires --recursive")
        );
        assert!(
            check(&["--rewind", "1d", "a/b/k"])
                .unwrap_err()
                .contains("--rewind without")
        );
        assert!(
            check(&["-I", "--versions", "--force", "a/b/k"])
                .unwrap_err()
                .contains("--incomplete")
        );
        assert!(check(&["-r", "--force", "--older-than", "bogus", "a/b/"]).is_err());
    }

    fn version(key: &str, secs: u64, id: &str, latest: bool, marker: bool) -> ObjectInfo {
        ObjectInfo {
            key: key.into(),
            last_modified: Some(UNIX_EPOCH + Duration::from_secs(secs)),
            version_id: Some(id.into()),
            is_latest: latest,
            is_delete_marker: marker,
            ..Default::default()
        }
    }

    fn ids(items: &[ObjectInfo]) -> Vec<&str> {
        items
            .iter()
            .map(|item| item.version_id.as_deref().unwrap())
            .collect()
    }

    fn sample() -> Vec<ObjectInfo> {
        vec![
            version("a", 30, "a3", true, false),
            version("a", 20, "a2", false, false),
            version("a", 10, "a1", false, false),
            version("b", 25, "b2", true, true),
            version("b", 5, "b1", false, false),
        ]
    }

    #[test]
    fn selects_versions_like_mc() {
        let at = Some(UNIX_EPOCH + Duration::from_secs(22));
        let all = parse(&["--versions", "-r", "--force", "a/b/"]);
        assert_eq!(ids(&select_versions(sample(), &all, None)).len(), 5);
        assert_eq!(
            ids(&select_versions(sample(), &all, at)),
            ["a2", "a1", "b1"]
        );
        let rewind = parse(&["-r", "--force", "--rewind", "1d", "a/b/"]);
        assert_eq!(ids(&select_versions(sample(), &rewind, at)), ["a2", "b1"]);
        let non_current = parse(&["--versions", "--non-current", "-r", "--force", "a/b/"]);
        assert_eq!(
            ids(&select_versions(sample(), &non_current, None)),
            ["a2", "a1", "b2", "b1"]
        );
    }

    #[test]
    fn formats_mc_messages() {
        let message = RemoveMessage {
            key: "a/b/k".into(),
            ..Default::default()
        };
        assert_eq!(message.text(), "Removed `a/b/k`.");
        let message = RemoveMessage {
            key: "a/b/k".into(),
            version_id: Some("v1".into()),
            ..Default::default()
        };
        assert_eq!(message.text(), "Removed `a/b/k` (versionId=v1).");
        let message = RemoveMessage {
            key: "a/b/k".into(),
            version_id: Some("v1".into()),
            mod_time: Some(UNIX_EPOCH),
            dry_run: true,
            ..Default::default()
        };
        assert_eq!(
            message.text(),
            "DRYRUN: Removing `a/b/k` (versionId=v1) (modTime=1970-01-01 00:00:00 UTC)."
        );
        let outcome = DeleteOutcome {
            key: "k".into(),
            delete_marker: true,
            delete_marker_version_id: Some("dm".into()),
            ..Default::default()
        };
        let message = RemoveMessage::from_outcome("a", "b", &outcome);
        assert_eq!(
            message.text(),
            "Created delete marker `a/b/k` (versionId=dm)."
        );
        let json: serde_json::Value = serde_json::from_str(&message.json()).unwrap();
        assert_eq!(json["status"], "success");
        assert_eq!(json["deleteMarker"], true);
        assert_eq!(json["versionID"], "dm");
        assert_eq!(json["dryRun"], false);
        assert!(json["modTime"].is_null());
    }
}
