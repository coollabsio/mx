//! `mx admin update` (mc `admin update`): asks every MinIO server to update itself
//! (optionally from a custom release URL).
//!
//! Owner: SERVER.

use crate::commands::runtime;
use crate::s3::admin_server::{self as api, ServerUpdateStatus};
use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use std::io::{BufRead, IsTerminal, Write};

#[derive(Debug, Args)]
pub struct UpdateArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    /// Optional update URL (mc's second argument).
    #[arg(value_name = "URL", hide = true)]
    pub update_url: Option<String>,
    #[arg(long = "yes", short = 'y', help = "Confirms the server update")]
    pub yes: bool,
}

/// mc `serverUpdateMessage`.
#[derive(Serialize)]
struct UpdateMessage<'a> {
    status: &'static str,
    #[serde(rename = "serverURL")]
    server_url: &'a str,
    #[serde(rename = "serverUpdateStatus")]
    server_update_status: &'a ServerUpdateStatus,
}

pub fn run(args: UpdateArgs, json: bool) -> Result<()> {
    let client = api::admin_client(&args.target, "Unable to initialize admin connection.")?;
    if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() && !args.yes {
        print!("You are about to upgrade *MinIO Server*, please confirm [y/N]: ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .context("Unable to parse user input.")?;
        let answer = answer.trim().to_lowercase();
        if answer != "y" && answer != "yes" {
            println!("Upgrade aborted!");
            return Ok(());
        }
    }
    let url = args.update_url.as_deref().unwrap_or_default();
    let status = runtime()?
        .block_on(api::server_update(&client, url, false))
        .context("Unable to update the server.")?;
    if json {
        return crate::output::print_json(&UpdateMessage {
            status: "success",
            server_url: &args.target,
            server_update_status: &status,
        });
    }
    print!("{}", render(&args.target, &status));
    Ok(())
}

/// mc `serverUpdateMessage.String()`: a go-pretty `StyleLight` table of per-host results.
fn render(target: &str, status: &ServerUpdateStatus) -> String {
    let rows: Vec<Vec<String>> = status
        .results
        .iter()
        .map(|peer| {
            let text = if !peer.err.is_empty() {
                peer.err.clone()
            } else if !peer.waiting_drives.is_empty() {
                format!(
                    "{} drives are hung, process was upgraded. However OS reboot is recommended.",
                    peer.waiting_drives.len()
                )
            } else {
                format!(
                    "upgraded server from {} to {}: ✔ ",
                    peer.current_version, peer.updated_version
                )
            };
            vec![peer.host.clone(), text]
        })
        .collect();
    format!(
        "Server update request sent successfully `{target}`\n{}",
        crate::commands::ilm::render_table(None, &["Host", "Status"], &rows, &[])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_update_table() {
        let status: ServerUpdateStatus = serde_json::from_str(
            r#"{"dryRun":false,"results":[{"host":"h:9000","currentVersion":"a","updatedVersion":"b"},{"host":"x:9000","err":"boom"}]}"#,
        )
        .unwrap();
        assert_eq!(
            render("e", &status),
            "Server update request sent successfully `e`\n┌────────┬─────────────────────────────────┐\n│ HOST   │ STATUS                          │\n├────────┼─────────────────────────────────┤\n│ h:9000 │ upgraded server from a to b: ✔  │\n│ x:9000 │ boom                            │\n└────────┴─────────────────────────────────┘\n"
        );
    }
}
