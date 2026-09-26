//! `mx undo` (area G): reverts the last PUT/DELETE operations on a versioned bucket by
//! removing the newest object versions / delete markers.

use crate::commands::retention::{LockTarget, print_json};
use crate::commands::runtime;
use crate::s3::ObjectInfo;
use crate::s3::lock;
use anyhow::{Result, bail};
use clap::Args;
use serde::Serialize;

#[derive(Debug, Args)]
pub struct UndoArgs {
    pub target: String,
    /// undo last S3 PUT/DELETE operations recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
    /// force recursive operation
    #[arg(long)]
    pub force: bool,
    /// undo N last changes
    #[arg(long, default_value_t = 1)]
    pub last: usize,
    /// undo only if the latest version is of the following type: PUT or DELETE
    #[arg(long)]
    pub action: Option<String>,
    /// fake an undo operation
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Serialize)]
struct UndoMessage<'a> {
    status: &'static str,
    url: String,
    key: String,
    #[serde(rename = "versionId", skip_serializing_if = "str::is_empty")]
    version_id: &'a str,
    #[serde(rename = "isDeleteMarker", skip_serializing_if = "std::ops::Not::not")]
    is_delete_marker: bool,
}

impl UndoMessage<'_> {
    fn text(&self) -> String {
        if self.is_delete_marker {
            format!("\u{2713} Last delete of `{}` is reverted.", self.key)
        } else {
            format!(
                "\u{2713} Last upload of `{}` (vid={}) is reverted.",
                self.key, self.version_id
            )
        }
    }
}

fn parse_action(args: &UndoArgs) -> Result<Option<String>> {
    if args.last < 1 {
        bail!("--last value should be a positive integer");
    }
    if args.recursive && !args.force {
        bail!("This is a dangerous operation, you need to provide --force flag as well");
    }
    let action = args.action.as_deref().map(str::to_ascii_uppercase);
    match action.as_deref() {
        None | Some("PUT") | Some("DELETE") => {}
        Some(_) => bail!(
            "unsupported action specified, supported actions are PUT, DELETE or empty (default)"
        ),
    }
    if action.is_some() && args.last != 1 {
        bail!("--action if specified requires that you must specify --last=1");
    }
    Ok(action)
}

pub fn run(args: UndoArgs, json: bool) -> Result<()> {
    let action = parse_action(&args)?;
    let target = LockTarget::resolve(&args.target)?;
    if !args.recursive {
        target.require_key()?;
    }
    let rt = runtime()?;
    let status = rt.block_on(crate::s3::get_versioning(&target.alias, &target.bucket))?;
    if status != "Enabled" {
        bail!("Undo command works only with S3 versioned-enabled buckets.");
    }
    let client = rt.block_on(crate::s3::build_client(&target.alias))?;
    let items: Vec<ObjectInfo> = rt
        .block_on(lock::list_versions_under(
            &client,
            &target.bucket,
            &target.key,
        ))?
        .into_iter()
        .filter(|item| item.storage_class.as_deref() != Some("GLACIER"))
        .filter(|item| args.recursive || item.key == target.key)
        .collect();

    let mut found = false;
    let mut failed = 0;
    for group in items.chunk_by(|left, right| left.key == right.key) {
        let candidates = lock::undo_candidates(group, args.last, action.as_deref());
        found |= !candidates.is_empty();
        for item in candidates {
            let version_id = item.version_id.as_deref().unwrap_or_default();
            if !args.dry_run
                && let Err(error) = rt.block_on(lock::delete_object_version(
                    &client,
                    &target.bucket,
                    &item.key,
                    version_id,
                ))
            {
                eprintln!("mx: Unable to undo `{}`: {error:#}", item.key);
                failed += 1;
                continue;
            }
            let message = UndoMessage {
                status: "success",
                url: target.object_url(&item.key),
                key: target.relative_key(&item.key),
                version_id,
                is_delete_marker: item.is_delete_marker,
            };
            if json {
                print_json(&message)?;
            } else {
                println!("{}", message.text());
            }
        }
    }
    if !found {
        bail!("Unable to find any object version to undo.");
    }
    if failed > 0 {
        bail!("Unable to undo {failed} object version(s).");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(recursive: bool, force: bool, last: usize, action: Option<&str>) -> UndoArgs {
        UndoArgs {
            target: "local/b/k".into(),
            recursive,
            force,
            last,
            action: action.map(str::to_string),
            dry_run: false,
        }
    }

    #[test]
    fn validates_flags_like_mc() {
        assert!(parse_action(&args(false, false, 0, None)).is_err());
        assert!(parse_action(&args(true, false, 1, None)).is_err());
        assert!(parse_action(&args(true, true, 1, None)).is_ok());
        assert_eq!(
            parse_action(&args(false, false, 1, Some("put"))).unwrap(),
            Some("PUT".into())
        );
        assert!(parse_action(&args(false, false, 2, Some("DELETE"))).is_err());
        assert!(parse_action(&args(false, false, 1, Some("COPY"))).is_err());
    }

    #[test]
    fn formats_messages() {
        let message = UndoMessage {
            status: "success",
            url: "http://h/b/k".into(),
            key: "k".into(),
            version_id: "v1",
            is_delete_marker: false,
        };
        assert_eq!(
            message.text(),
            "\u{2713} Last upload of `k` (vid=v1) is reverted."
        );
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["versionId"], "v1");
        assert!(json.get("isDeleteMarker").is_none());
        let marker = UndoMessage {
            is_delete_marker: true,
            ..message
        };
        assert_eq!(marker.text(), "\u{2713} Last delete of `k` is reverted.");
    }
}
