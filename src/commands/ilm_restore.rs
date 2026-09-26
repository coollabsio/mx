//! `mx ilm restore` (area G): RestoreObject for transitioned objects, then waits until the
//! restored copies are available (like `mc ilm restore`).
//! Wired from `ilm.rs` as `IlmCommand::Restore`; area G edits only this file.

use crate::commands::cat::EncCFlag;
use crate::commands::retention::{LockTarget, print_json};
use crate::commands::runtime;
use crate::flags::{Sse, VersionIdFlag, VersionsFlag, resolve_sse};
use crate::s3::lock::{self, Selection};
use anyhow::{Context, Result, bail};
use clap::Args;
use std::io::Write;
use std::time::{Duration, Instant};

/// Poll interval while waiting for restores to finish (mc uses 5s).
const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Args)]
#[command(mut_args(|a| match a.get_id().as_str() {
    "versions" => a.help("apply on versions"),
    "version_id" => a.help("select a specific version id"),
    _ => a,
}))]
pub struct IlmRestoreArgs {
    pub target: String,
    /// keep the restored copy for N days
    #[arg(long, default_value_t = 1)]
    pub days: i32,
    /// apply recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub versions: VersionsFlag,
    #[command(flatten)]
    pub version_id: VersionIdFlag,
    #[command(flatten)]
    pub enc: EncCFlag,
}

/// SSE-C key for `ALIAS/BUCKET/KEY` (longest `--enc-c` prefix, like mc `getSSE`).
fn customer_key(enc: &[(String, Sse)], alias_path: &str) -> Option<[u8; 32]> {
    resolve_sse(enc, alias_path).and_then(|sse| sse.customer_key())
}

/// mc `checkILMRestoreSyntax`. mc's `--version-id` combination check calls `ctx.Bool` on a
/// string flag and never fires, so `--version-id`/`--versions` are not checked against `-r`.
fn validate(args: &IlmRestoreArgs) -> Result<()> {
    if args.days <= 0 {
        bail!("--days should be equal or greater than 1");
    }
    Ok(())
}

/// mc `printStatus`: redraws one status line (`\n` + cursor up + clear line) with 1-3 cycling
/// dots, on every event and every second. Nothing in JSON mode.
struct Status {
    json: bool,
    dot_cycle: usize,
    last_tick: Instant,
}

impl Status {
    fn print(&mut self, message: &str) {
        if self.json {
            return;
        }
        self.dot_cycle += 1;
        print!(
            "\n\x1b[1A\x1b[K{message}{}",
            ".".repeat(self.dot_cycle % 3 + 1)
        );
        let _ = std::io::stdout().flush();
    }

    /// Prints the 1s ticker updates due since the last one.
    fn ticks(&mut self, message: &str) {
        while self.last_tick.elapsed() >= TICK {
            self.last_tick += TICK;
            self.print(message);
        }
    }

    fn end_line(&self) {
        if !self.json {
            println!();
        }
    }
}

const TICK: Duration = Duration::from_secs(1);

pub fn run(args: IlmRestoreArgs, json: bool) -> Result<()> {
    validate(&args)?;
    let enc = args
        .enc
        .entries()
        .context("Unable to parse encryption keys.")?;
    let target = LockTarget::resolve(&args.target)?;
    if !args.recursive {
        target.require_key()?;
    }
    let rt = runtime()?;
    let client = rt.block_on(crate::s3::build_client(&target.alias))?;
    let objects: Vec<(String, Option<String>)> = if args.recursive {
        let selection = Selection {
            recursive: true,
            versions: args.versions.versions,
            rewind: None,
        };
        rt.block_on(lock::select_objects(
            &client,
            &target.bucket,
            &target.key,
            &selection,
        ))?
        .into_iter()
        .map(|item| (item.key, item.version_id))
        .collect()
    } else {
        vec![(target.key.clone(), args.version_id.version_id.clone())]
    };
    let mut status = Status {
        json,
        dot_cycle: 0,
        last_tick: Instant::now(),
    };

    // mc `sendRestoreRequests`: versions of one object count once.
    let mut sent = 0;
    let mut prev = None;
    for (key, version_id) in &objects {
        status.ticks(&format!("Sent restore requests to {sent} object(s)"));
        match rt.block_on(lock::restore_object(
            &client,
            &target.bucket,
            key,
            version_id.as_deref(),
            args.days,
        )) {
            Ok(()) => {
                if prev != Some(key) {
                    prev = Some(key);
                    sent += 1;
                }
            }
            Err(error) => {
                crate::output::print_error(&error.context("Unable to send restore request."))
            }
        }
        status.print(&format!("Sent restore requests to {sent} object(s)"));
    }
    status.print(&format!("Sent restore requests to {sent} object(s)"));
    status.end_line();

    // mc `checkRestoreStatus`: waits on every selected object, even if its request failed.
    let mut finished = 0;
    let mut prev = None;
    for (key, version_id) in &objects {
        let progress =
            |finished: usize| format!("{finished}/{sent} object(s) successfully restored");
        let sse_c = customer_key(&enc, &target.alias_path(key));
        let result = loop {
            status.ticks(&progress(finished));
            match rt.block_on(lock::restore_ongoing(
                &client,
                &target.bucket,
                key,
                version_id.as_deref(),
                sse_c.as_ref(),
            )) {
                Ok(Some(true)) => {
                    let started = Instant::now();
                    while started.elapsed() < POLL_INTERVAL {
                        std::thread::sleep(TICK.min(POLL_INTERVAL - started.elapsed()));
                        status.ticks(&progress(finished));
                    }
                }
                Ok(Some(false)) => break Ok(()),
                Ok(None) => {
                    break Err(anyhow::anyhow!(
                        "`{}` did not receive restore request",
                        object_url(&target, key)
                    ));
                }
                Err(error) => break Err(error),
            }
        };
        match result {
            Ok(()) => {
                if prev != Some(key) {
                    prev = Some(key);
                    finished += 1;
                }
            }
            Err(error) => {
                crate::output::print_error(&error.context("Unable to check for restore status"))
            }
        }
        status.print(&progress(finished));
    }
    status.print(&format!(
        "{finished}/{sent} object(s) successfully restored"
    ));
    if json {
        #[derive(serde::Serialize)]
        struct IlmRestoreMessage {
            status: &'static str,
            restored: usize,
        }
        // mc reports success (and exits 0) even when requests failed.
        print_json(&IlmRestoreMessage {
            status: "success",
            restored: sent,
        })?;
    } else {
        status.end_line();
    }
    Ok(())
}

/// mc's expanded object URL (`urlJoinPath(alias URL, bucket/key)`).
fn object_url(target: &LockTarget, key: &str) -> String {
    format!(
        "{}/{}/{key}",
        target.alias.url.trim_end_matches('/'),
        target.bucket
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(days: i32, recursive: bool, versions: bool, vid: Option<&str>) -> IlmRestoreArgs {
        IlmRestoreArgs {
            target: "local/b/k".into(),
            days,
            recursive,
            version_id: VersionIdFlag {
                version_id: vid.map(str::to_string),
            },
            versions: VersionsFlag { versions },
            enc: EncCFlag::default(),
        }
    }

    #[test]
    fn picks_longest_enc_c_prefix() {
        let hex = |byte: u8| format!("{byte:02x}").repeat(32);
        let enc = EncCFlag {
            enc_c: vec![
                format!("local/b/={}", hex(1)),
                format!("local/b/secret/={}", hex(2)),
            ],
        }
        .entries()
        .unwrap();
        assert_eq!(customer_key(&enc, "local/b/k"), Some([1; 32]));
        assert_eq!(customer_key(&enc, "local/b/secret/k"), Some([2; 32]));
        assert_eq!(customer_key(&enc, "local/other/k"), None);
        assert_eq!(customer_key(&[], "local/b/k"), None);
    }

    #[test]
    fn validates_flags() {
        assert!(validate(&args(1, false, false, None)).is_ok());
        assert!(validate(&args(0, false, false, None)).is_err());
        // Accepted like mc (its combination check never fires).
        assert!(validate(&args(1, true, false, Some("v"))).is_ok());
        assert!(validate(&args(1, false, true, None)).is_ok());
        assert!(validate(&args(1, true, true, None)).is_ok());
        assert!(validate(&args(3, false, false, Some("v"))).is_ok());
    }
}
