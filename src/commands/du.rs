use crate::commands::ls::{ListOpts, list};
use crate::commands::stat::human_bytes;
use crate::config::ConfigStore;
use crate::flags::RewindFlag;
use crate::location::{Location, parse_location};
use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use std::time::SystemTime;

#[derive(Debug, Args)]
#[command(mut_args(|a| if a.get_id().as_str() == "rewind" {
    a.help("include all object versions no later than specified date")
} else {
    a
}))]
pub struct DuArgs {
    /// print the total for a folder prefix only if it is N or fewer levels below the command line argument (default: 0)
    #[arg(short = 'd', long, allow_negative_numbers = true)]
    pub depth: Option<i64>,
    /// recursively print the total for a folder prefix
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub rewind: RewindFlag,
    /// include all object versions
    #[arg(long)]
    pub versions: bool,
    #[arg(required = true, value_name = "TARGET")]
    pub targets: Vec<String>,
}

pub fn run(args: DuArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    // mc: `-d 0` (the default) means 1 level, or everything with `-r` unless `-d` was given.
    let depth = match args.depth {
        Some(0) | None if !args.recursive => 1,
        None => -1,
        Some(depth) => depth,
    };
    let rewind = args.rewind.at(SystemTime::now())?;
    let rt = super::runtime()?;
    let mut failed = false;
    for input in &args.targets {
        // mc `isAliasURLDir`: only folders (and paths ending in `/`) can be summarized.
        let is_dir = input.ends_with('/')
            || matches!(
                super::util::stat_target(&store, input),
                Ok(super::util::TargetKind::Folder)
            )
            || matches!(parse_location(input, store.config()), Location::S3(t) if t.key.is_none());
        if !is_dir {
            return Err(crate::error::McError::invalid_argument()).with_context(|| {
                format!(
                    "Source `{input}` is not a folder. Only folders are supported by 'du' command."
                )
            });
        }
        let du = Du {
            store: &store,
            rt: &rt,
            opts: ListOpts {
                versions: args.versions,
                rewind,
                ..Default::default()
            },
            local: matches!(parse_location(input, store.config()), Location::Local(_)),
            json,
        };
        if let Err(error) = du.total(input, depth) {
            crate::output::print_error(&error);
            failed = true;
        }
    }
    if failed {
        return Err(crate::output::Exit(1).into());
    }
    Ok(())
}

struct Du<'a> {
    store: &'a ConfigStore,
    rt: &'a tokio::runtime::Runtime,
    opts: ListOpts,
    local: bool,
    json: bool,
}

impl Du<'_> {
    /// mc `du`: (size, objects) below `url`; prints the total unless `depth` is 0. Level 1
    /// sums a recursive listing, deeper levels recurse into each folder first.
    fn total(&self, url: &str, depth: i64) -> Result<(i64, i64)> {
        let dir = if url.ends_with('/') {
            url.to_string()
        } else {
            format!("{url}/")
        };
        let recursive = depth == 1;
        let opts = ListOpts {
            recursive,
            ..self.opts.clone()
        };
        let listing = list(self.store, self.rt, &dir, &opts).with_context(|| {
            crate::error::nonfatal(format!("Failed to find disk usage of `{url}` recursively."))
        })?;
        // mc reports unreadable folders of the recursive listing and still prints the total.
        for denied in &listing.denied {
            crate::output::print_error(&denied.error());
        }
        let (mut size, mut objects) = (0, 0);
        for content in &listing.contents {
            if content.is_dir && !recursive {
                // mc descends with the absolute path of local folders (no trailing `/`).
                let sub = if self.local {
                    format!("{}{}", listing.url, content.key.trim_end_matches('/'))
                } else {
                    format!("{dir}{}", content.key)
                };
                let (sub_size, sub_objects) =
                    self.total(&sub, if depth > 0 { depth - 1 } else { depth })?;
                size += sub_size;
                objects += sub_objects;
            } else if !content.is_delete_marker && !content.is_dir {
                size += content.size;
                objects += 1;
            }
        }
        if depth != 0 {
            let prefix = if self.local {
                dir.trim_matches('/')
            } else {
                // `ALIAS/BUCKET/PREFIX/` -> `BUCKET/PREFIX`
                dir.split_once('/')
                    .map_or("", |(_, path)| path)
                    .trim_matches('/')
            };
            let message = DuMessage {
                prefix,
                size,
                objects,
                status: "success",
                is_versions: self.opts.versions,
            };
            if self.json {
                crate::output::print_json(&message)?;
            } else {
                println!("{}", message.text());
            }
        }
        Ok((size, objects))
    }
}

/// mc `duMessage`.
#[derive(Debug, Serialize)]
struct DuMessage<'a> {
    prefix: &'a str,
    size: i64,
    objects: i64,
    status: &'static str,
    #[serde(rename = "isVersions")]
    is_versions: bool,
}

impl DuMessage<'_> {
    fn text(&self) -> String {
        let unit = if self.is_versions {
            "version"
        } else {
            "object"
        };
        let plural = if self.objects == 1 { "" } else { "s" };
        format!(
            "{}\t{} {unit}{plural}\t{}",
            human_bytes(self.size.max(0) as u64).replace(' ', ""),
            self.objects,
            self.prefix
        )
    }
}

#[cfg(test)]
mod tests {
    use super::DuMessage;

    #[test]
    fn text_matches_mc() {
        let mut message = DuMessage {
            prefix: "b/dir",
            size: 2048,
            objects: 1,
            status: "success",
            is_versions: false,
        };
        assert_eq!(message.text(), "2.0KiB\t1 object\tb/dir");
        message.objects = 3;
        message.is_versions = true;
        assert_eq!(message.text(), "2.0KiB\t3 versions\tb/dir");
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"prefix":"b/dir","size":2048,"objects":3,"status":"success","isVersions":true}"#
        );
    }
}
