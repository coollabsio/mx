//! `diff FIRST SECOND`: lists both folders and reports differences like mc (`difference.go`
//! without metadata comparison): `< FIRST_URL` only in first, `> SECOND_URL` only in second,
//! `! SECOND_URL` differing size or newer first. URLs are endpoint URLs / absolute paths.

use crate::commands::runtime;
use crate::commands::util::{TargetKind, stat_target};
use crate::config::ConfigStore;
use crate::error::{McError, nonfatal};
use crate::mirror::diff::{Diff, difference};
use crate::mirror::{Endpoint, endpoint, list_all};
use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;

#[derive(Debug, Args)]
pub struct DiffArgs {
    pub source: String,
    pub target: String,
}

/// mc `diffMessage` (`diff` is mc's numeric `differType`).
#[derive(Debug, Serialize, PartialEq)]
struct DiffMessage {
    status: &'static str,
    first: String,
    second: String,
    diff: u8,
}

impl DiffMessage {
    fn new(diff: Diff, rel: &str, first: &Endpoint, second: &Endpoint) -> Self {
        let (first, second) = match diff {
            Diff::OnlyInSource => (first.url(rel), String::new()),
            Diff::OnlyInTarget => (String::new(), second.url(rel)),
            _ => (first.url(rel), second.url(rel)),
        };
        Self {
            status: "success",
            first,
            second,
            diff: match diff {
                Diff::Size => 2,
                Diff::Metadata => 3,
                Diff::OnlyInSource => 5,
                Diff::OnlyInTarget => 6,
                Diff::SourceNewer => 7,
            },
        }
    }

    /// mc `diffMessage.String()`.
    fn text(&self) -> String {
        match self.diff {
            5 => format!("< {}", self.first),
            6 => format!("> {}", self.second),
            _ => format!("! {}", self.second),
        }
    }
}

/// mc `checkDiffSyntax`: FIRST must be an existing folder; SECOND may be missing.
fn check(store: &ConfigStore, args: &DiffArgs) -> Result<()> {
    let not_folder = |input: &str| {
        Err(McError::invalid_argument()).with_context(|| format!("`{input}` is not a folder."))
    };
    let first = stat_target(store, &args.source)
        .with_context(|| format!("Unable to stat '{}'.", args.source))?;
    if first != TargetKind::Folder {
        return not_folder(&args.source);
    }
    match stat_target(store, &args.target) {
        Ok(TargetKind::Folder) => Ok(()),
        Ok(TargetKind::File) => not_folder(&args.target),
        Err(error) if crate::error::error_code(&error) == Some("NoSuchKey") => Ok(()),
        Err(error) => Err(error).with_context(|| format!("Unable to stat '{}'.", args.target)),
    }
}

pub fn run(args: DiffArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    check(&store, &args)?;
    runtime()?.block_on(async {
        let first = endpoint(&store, &args.source, true).await?;
        let second = endpoint(&store, &args.target, true).await?;
        let listings = async {
            let (first_list, _) = list_all(&first).await?;
            let (second_list, _) = list_all(&second).await?;
            anyhow::Ok((first_list, second_list))
        };
        let (first_list, second_list) = match listings.await {
            Ok(listings) => listings,
            Err(error) => {
                crate::output::print_error(
                    &error.context(nonfatal("Unable to calculate objects difference.")),
                );
                return Ok(());
            }
        };
        for (rel, diff) in difference(&first_list, &second_list, true, false) {
            let message = DiffMessage::new(diff, &rel, &first, &second);
            if json {
                crate::output::print_json(&message)?;
            } else {
                println!("{}", message.text());
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn formats_messages_like_mc() {
        let first = Endpoint::Local(PathBuf::from("/w/l"));
        let second = Endpoint::Local(PathBuf::from("/w/r"));
        let only_first = DiffMessage::new(Diff::OnlyInSource, "a", &first, &second);
        assert_eq!(only_first.text(), "< /w/l/a");
        assert_eq!(
            serde_json::to_string(&only_first).unwrap(),
            r#"{"status":"success","first":"/w/l/a","second":"","diff":5}"#
        );
        let only_second = DiffMessage::new(Diff::OnlyInTarget, "b", &first, &second);
        assert_eq!(only_second.text(), "> /w/r/b");
        assert_eq!(only_second.diff, 6);
        let size = DiffMessage::new(Diff::Size, "c", &first, &second);
        assert_eq!(size.text(), "! /w/r/c");
        assert_eq!((size.first.as_str(), size.diff), ("/w/l/c", 2));
        assert_eq!(
            DiffMessage::new(Diff::SourceNewer, "c", &first, &second).diff,
            7
        );
    }
}
