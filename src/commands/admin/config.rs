//! `mx admin config` (mc `admin config`): get/set/reset config keys (with mc's help output
//! when no `key=value` is given), history/restore, and full export/import.
//!
//! Owner: SERVER.

use crate::commands::runtime;
use crate::s3::admin::AdminClient;
use crate::s3::admin_server::{self as api, Help};
use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::io::Read;

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    #[command(name = "get", about = "interactively retrieve a config key parameters")]
    Get(ConfigGetArgs),
    #[command(name = "set", about = "interactively set a config key parameters")]
    Set(ConfigSetArgs),
    #[command(name = "reset", about = "interactively reset a config key parameters")]
    Reset(ConfigResetArgs),
    #[command(name = "history", about = "show all historic configuration changes")]
    History(ConfigHistoryArgs),
    #[command(
        name = "restore",
        about = "rollback back changes to a specific config history"
    )]
    Restore(ConfigRestoreArgs),
    #[command(name = "export", about = "export all config keys to STDOUT")]
    Export(ConfigExportArgs),
    #[command(name = "import", about = "import multiple config keys from STDIN")]
    Import(ConfigImportArgs),
}

#[derive(Debug, Args)]
pub struct ConfigGetArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "KEYS")]
    pub keys: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ConfigSetArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "ARGS")]
    pub args: Vec<String>,
    #[arg(long = "env", help = "list all the env only help")]
    pub env: bool,
}

#[derive(Debug, Args)]
pub struct ConfigResetArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "CONFIG-KEYS")]
    pub config_keys: Vec<String>,
    #[arg(long = "env", help = "list all the env only help")]
    pub env: bool,
}

#[derive(Debug, Args)]
pub struct ConfigHistoryArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(
        long = "count",
        short = 'n',
        default_value_t = 10,
        value_name = "VALUE",
        help = "list only last 'n' entries"
    )]
    pub count: i64,
    #[arg(long = "clear", short = 'c', help = "clear all history")]
    pub clear: bool,
}

#[derive(Debug, Args)]
pub struct ConfigRestoreArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(value_name = "RESTOREID")]
    pub restoreid: Option<String>,
}

#[derive(Debug, Args)]
pub struct ConfigExportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ConfigImportArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
}

pub fn run(args: ConfigArgs, json: bool) -> Result<()> {
    match args.command {
        ConfigCommand::Get(args) => get(args, json),
        ConfigCommand::Set(args) => set(args, json),
        ConfigCommand::Reset(args) => reset(args, json),
        ConfigCommand::History(args) => history(args, json),
        ConfigCommand::Restore(args) => restore(args, json),
        ConfigCommand::Export(args) => export(args, json),
        ConfigCommand::Import(args) => import(args, json),
    }
}

/// mc `fatalIf(err, format, args...)` formats the message for text output only; the JSON
/// error keeps the raw format string.
fn go_msg(json: bool, format: &str, formatted: String) -> String {
    if json { format.to_string() } else { formatted }
}

fn client(target: &str) -> Result<AdminClient> {
    api::admin_client(target, "Unable to initialize admin connection.")
}

#[derive(Serialize)]
struct StatusMessage {
    status: &'static str,
}

fn print_success(json: bool, text: &str) -> Result<()> {
    if json {
        return crate::output::print_json(&StatusMessage { status: "success" });
    }
    println!("{text}");
    Ok(())
}

/// Prints text like mc `printMsg`: one trailing newline is replaced by `Println`'s.
fn print_msg(text: &str) {
    println!("{}", text.strip_suffix('\n').unwrap_or(text));
}

// ---------------------------------------------------------------------------
// help
// ---------------------------------------------------------------------------

/// mc `configHelpMessage`.
#[derive(Serialize)]
struct HelpMessage<'a> {
    status: &'static str,
    help: &'a Help,
}

/// mc `HelpTmpl` rendered through a Go `tabwriter` (minwidth 1, tabwidth 8, padding 2).
pub(crate) fn render_help(help: &Help) -> String {
    let keys = help.keys_help.as_deref().unwrap_or_default();
    let mut text = String::new();
    if !help.sub_sys.is_empty() {
        text.push_str("KEY:\n");
        if help.multiple_targets {
            text.push_str(&format!("{}[:name]\t", help.sub_sys));
        } else {
            text.push_str(&format!("{}\t", help.sub_sys));
        }
        text.push_str(&help.description);
        text.push_str("\n\nARGS:");
        for key in keys {
            let name = if key.optional {
                key.key.clone()
            } else {
                format!("{}*", key.key)
            };
            text.push_str(&format!("\n{name}\t({})\t{}", key.kind, key.description));
        }
    } else {
        text.push_str("KEYS:");
        for key in keys {
            text.push_str(&format!("\n{}\t{}", key.key, key.description));
        }
    }
    tabwrite(&text, 1, 2)
}

fn print_help(help: &Help, json: bool) -> Result<()> {
    if json {
        return crate::output::print_json(&HelpMessage {
            status: "success",
            help,
        });
    }
    print_msg(&render_help(help));
    Ok(())
}

fn show_help(client: &AdminClient, sub_sys: &str, key: &str, env: bool, json: bool) -> Result<()> {
    let help = runtime()?
        .block_on(api::help_config_kv(client, sub_sys, key, env))
        .context("Unable to get help for the sub-system")?;
    print_help(&help, json)
}

/// Go `text/tabwriter` (no flags, space padding): cells are tab-terminated, the text after
/// the last tab of a line is not aligned, and a column spans consecutive lines having it.
pub(crate) fn tabwrite(text: &str, minwidth: usize, padding: usize) -> String {
    let lines: Vec<Vec<&str>> = text.split('\n').map(|l| l.split('\t').collect()).collect();
    let mut out = String::new();
    let mut widths = Vec::new();
    format_block(
        &lines,
        0,
        lines.len(),
        &mut widths,
        minwidth,
        padding,
        &mut out,
    );
    out
}

fn format_block(
    lines: &[Vec<&str>],
    mut line0: usize,
    line1: usize,
    widths: &mut Vec<usize>,
    minwidth: usize,
    padding: usize,
    out: &mut String,
) {
    let column = widths.len();
    let mut this = line0;
    while this < line1 {
        if column + 1 >= lines[this].len() {
            this += 1;
            continue;
        }
        write_lines(lines, line0, this, widths, out);
        line0 = this;
        let mut width = minwidth;
        while this < line1 && column + 1 < lines[this].len() {
            width = width.max(lines[this][column].chars().count() + padding);
            this += 1;
        }
        widths.push(width);
        format_block(lines, line0, this, widths, minwidth, padding, out);
        widths.pop();
        line0 = this;
    }
    write_lines(lines, line0, line1, widths, out);
}

fn write_lines(
    lines: &[Vec<&str>],
    line0: usize,
    line1: usize,
    widths: &[usize],
    out: &mut String,
) {
    for (index, line) in lines.iter().enumerate().take(line1).skip(line0) {
        for (j, cell) in line.iter().enumerate() {
            out.push_str(cell);
            if j < widths.len() {
                let pad = widths[j].saturating_sub(cell.chars().count());
                out.push_str(&" ".repeat(pad));
            }
        }
        if index + 1 < lines.len() {
            out.push('\n');
        }
    }
}

// ---------------------------------------------------------------------------
// get / set / reset
// ---------------------------------------------------------------------------

/// mc `configGetMessage`.
#[derive(Serialize)]
struct GetMessage {
    status: &'static str,
    config: Vec<api::SubsysConfig>,
}

fn get(args: ConfigGetArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    if args.keys.is_empty() {
        return show_help(&client, "", "", false, json);
    }
    let body = runtime()?
        .block_on(api::get_config_kv(&client, &args.keys.join(" ")))
        .with_context(|| {
            go_msg(
                json,
                "Unable to get server '%s' config",
                format!("Unable to get server '[{}]' config", args.keys.join(" ")),
            )
        })?;
    let text = String::from_utf8_lossy(&body);
    if json {
        let config =
            api::parse_server_config_output(&text).context("Unable to marshal into JSON.")?;
        return crate::output::print_json(&GetMessage {
            status: "success",
            config,
        });
    }
    print_msg(&text);
    Ok(())
}

fn set(args: ConfigSetArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    let input = args.args.join(" ");
    if !input.contains('=') {
        let sub_sys = args.args.first().map(String::as_str).unwrap_or_default();
        let key = args.args.get(1).map(String::as_str).unwrap_or_default();
        return show_help(&client, sub_sys, key, args.env, json);
    }
    let restart = runtime()?
        .block_on(api::set_config_kv(&client, &input))
        .with_context(|| {
            go_msg(
                json,
                "Unable to set '%s' to server",
                format!("Unable to set '{input}' to server"),
            )
        })?;
    let mut text = "Successfully applied new settings.".to_string();
    if restart {
        text.push_str(&format!(
            "\nPlease restart your server 'mc admin service restart {}'.",
            args.target
        ));
    }
    print_success(json, &text)
}

fn reset(args: ConfigResetArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    if args.config_keys.is_empty() {
        return show_help(&client, "", "", args.env, json);
    }
    if args.config_keys.iter().any(|k| k.contains('=')) {
        return Err(
            anyhow!("new settings may not be provided for sub-system keys").context(go_msg(
                json,
                "Unable to reset '%s' on the server",
                format!("Unable to reset '{}' on the server", args.config_keys[0]),
            )),
        );
    }
    let input = args.config_keys.join(" ");
    let restart = runtime()?
        .block_on(api::del_config_kv(&client, &input))
        .with_context(|| {
            go_msg(
                json,
                "Unable to reset '%s' on the server",
                format!("Unable to reset '{input}' on the server"),
            )
        })?;
    let mut text = format!("'{input}' is successfully reset.");
    if restart {
        text.push_str(&format!(
            "\nPlease restart your server with `mc admin service restart {}`.",
            args.target
        ));
    }
    print_success(json, &text)
}

// ---------------------------------------------------------------------------
// history / restore
// ---------------------------------------------------------------------------

/// mc `historyEntry`.
#[derive(Serialize)]
struct HistoryEntry {
    #[serde(rename = "restoreId")]
    restore_id: String,
    #[serde(rename = "createTime")]
    create_time: String,
    targets: String,
}

/// mc `configHistoryMessage` (`entries` is `null` after `--clear`).
#[derive(Serialize)]
struct HistoryMessage {
    status: &'static str,
    entries: Option<Vec<HistoryEntry>>,
}

fn render_history(entries: &[HistoryEntry]) -> String {
    let mut text = String::new();
    for entry in entries {
        text.push_str(&format!(
            "RestoreId: {}\nDate: {}\n\n{}\n\n",
            entry.restore_id, entry.create_time, entry.targets
        ));
    }
    tabwrite(&text, 1, 2)
}

fn history(args: ConfigHistoryArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    let entries = if args.clear {
        runtime()?
            .block_on(api::clear_config_history(&client, "all"))
            .context("Unable to clear server configuration.")?;
        None
    } else {
        let entries = runtime()?
            .block_on(api::list_config_history(&client, args.count))
            .context("Unable to list server history configuration.")?;
        Some(
            entries
                .into_iter()
                .map(|entry| HistoryEntry {
                    restore_id: entry.restore_id,
                    create_time: api::http_time(&entry.create_time),
                    targets: entry.data,
                })
                .collect::<Vec<_>>(),
        )
    };
    if json {
        return crate::output::print_json(&HistoryMessage {
            status: "success",
            entries,
        });
    }
    print_msg(&render_history(entries.as_deref().unwrap_or_default()));
    Ok(())
}

/// mc `configRestoreMessage`.
#[derive(Serialize)]
struct RestoreMessage<'a> {
    status: &'static str,
    #[serde(rename = "restoreID")]
    restore_id: &'a str,
}

fn restore(args: ConfigRestoreArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    let restore_id = args.restoreid.as_deref().unwrap_or_default();
    runtime()?
        .block_on(api::restore_config_history(&client, restore_id))
        .context("Unable to restore server configuration.")?;
    if json {
        return crate::output::print_json(&RestoreMessage {
            status: "success",
            restore_id,
        });
    }
    println!(
        "Please restart your server with `mc admin service restart {}`.\nRestored {restore_id} kv successfully.",
        args.target
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// export / import
// ---------------------------------------------------------------------------

/// mc `configExportMessage` (`value` is Go `[]byte`: base64 in JSON).
#[derive(Serialize)]
struct ExportMessage {
    status: &'static str,
    value: String,
}

fn export(args: ConfigExportArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    let body = runtime()?
        .block_on(api::get_config(&client))
        .context("Unable to get server config")?;
    if json {
        use base64::Engine;
        return crate::output::print_json(&ExportMessage {
            status: "success",
            value: base64::engine::general_purpose::STANDARD.encode(&body),
        });
    }
    print_msg(&String::from_utf8_lossy(&body));
    Ok(())
}

fn import(args: ConfigImportArgs, json: bool) -> Result<()> {
    let client = client(&args.target)?;
    let mut config = Vec::new();
    std::io::stdin()
        .take(api::MAX_CONFIG_SIZE as u64 + 1)
        .read_to_end(&mut config)
        .context("Unable to set server config")?;
    runtime()?
        .block_on(api::set_config(&client, &config))
        .context("Unable to set server config")?;
    let text = format!(
        "Setting new key has been successful.\nPlease restart your server with `mc admin service restart {}`.",
        args.target
    );
    print_success(json, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabwriter_matches_go() {
        assert_eq!(
            tabwrite(
                "KEY:\nregion\tdesc\n\nARGS:\nname\t(string)\tx\ncomment\t(sentence)\ty",
                1,
                2
            ),
            "KEY:\nregion  desc\n\nARGS:\nname     (string)    x\ncomment  (sentence)  y"
        );
        assert_eq!(
            tabwrite("a\tb\nlonger\tc\n", 1, 2),
            "a       b\nlonger  c\n"
        );
        assert_eq!(tabwrite("", 1, 2), "");
    }

    #[test]
    fn help_renders_like_mc() {
        let help: Help = serde_json::from_str(
            r#"{"subSys":"notify_webhook","description":"publish","multipleTargets":true,"keysHelp":[{"key":"endpoint","description":"url","optional":false,"type":"url"},{"key":"comment","description":"c","optional":true,"type":"sentence"}]}"#,
        )
        .unwrap();
        assert_eq!(
            render_help(&help),
            "KEY:\nnotify_webhook[:name]  publish\n\nARGS:\nendpoint*  (url)       url\ncomment    (sentence)  c"
        );
        let keys: Help = serde_json::from_str(
            r#"{"subSys":"","keysHelp":[{"key":"site","description":"label"},{"key":"compression","description":"zip"}]}"#,
        )
        .unwrap();
        assert_eq!(
            render_help(&keys),
            "KEYS:\nsite         label\ncompression  zip"
        );
    }

    #[test]
    fn history_renders_entries() {
        let entries = vec![HistoryEntry {
            restore_id: "id".into(),
            create_time: "Sat, 26 Sep 2026 19:27:36 GMT".into(),
            targets: "region name=x".into(),
        }];
        assert_eq!(
            render_history(&entries),
            "RestoreId: id\nDate: Sat, 26 Sep 2026 19:27:36 GMT\n\nregion name=x\n\n"
        );
        assert_eq!(
            serde_json::to_string(&HistoryMessage {
                status: "success",
                entries: None
            })
            .unwrap(),
            r#"{"status":"success","entries":null}"#
        );
    }
}
