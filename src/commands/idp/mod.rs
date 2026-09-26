//! `mx idp` (mc `idp`): identity provider configuration.
//!
//! Owner: IDP. `openid` and `ldap` share the config commands here (mc `idp-openid-subcommands.go`)
//! and the access key commands in [`accesskey`].

pub mod accesskey;
pub mod ldap;
pub mod openid;

use crate::commands::runtime;
use crate::config::ConfigStore;
use crate::s3::admin::{self, AdminClient};
use crate::s3::admin_idp::{self, IdpConfig, IdpListItem};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct IdpArgs {
    #[command(subcommand)]
    pub command: IdpCommand,
}

#[derive(Debug, Subcommand)]
pub enum IdpCommand {
    #[command(about = "manage OpenID IDP server configuration")]
    Openid(openid::OpenidArgs),
    #[command(about = "manage Ldap IDP server configuration")]
    Ldap(ldap::LdapArgs),
}

pub fn run(args: IdpArgs, json: bool) -> Result<()> {
    match args.command {
        IdpCommand::Openid(args) => openid::run(args, json),
        IdpCommand::Ldap(args) => ldap::run(args, json),
    }
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

/// mc `showCommandHelpAndExit(ctx, 1)`: the help of `mx <path...>` on stdout, exit status 1.
pub(crate) fn show_help_and_exit(path: &[&str]) -> ! {
    crate::help::show_help_and_exit(path, 1)
}

/// mc `newAdminClient(aliasedURL)`.
pub(crate) fn admin_client(target: &str) -> Result<AdminClient> {
    admin::admin_client_for(&ConfigStore::load_or_create()?, target)
}

/// mc `printMsg`: `--json` prints `value` (compact on non-TTY, `indent`-space indented like the
/// message's `JSON()` on a TTY), else `text` without one trailing newline.
pub(crate) fn print_msg<T: Serialize>(
    json: bool,
    value: &T,
    text: &str,
    indent: usize,
) -> Result<()> {
    if !json {
        println!("{}", text.strip_suffix('\n').unwrap_or(text));
        return Ok(());
    }
    if indent == 1 || !crate::output::stdout_is_terminal() {
        return crate::output::print_json(value);
    }
    let mut buf = Vec::new();
    let indent = " ".repeat(indent);
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut serializer)?;
    println!("{}", String::from_utf8(buf)?);
    Ok(())
}

/// mc `fatalIf(err, msg, args...)` message: `--json` reports the unformatted template, text
/// mode substitutes each `%s`.
pub(crate) fn fatal_msg(template: &str, args: &[&str]) -> String {
    if crate::globals::json() {
        return template.to_string();
    }
    let mut out = template.to_string();
    for arg in args {
        out = out.replacen("%s", arg, 1);
    }
    out
}

/// mc `configSetMessage`.
pub(crate) fn print_config_set(json: bool, target: &str, restart: bool) -> Result<()> {
    #[derive(Serialize)]
    struct ConfigSetMessage {
        status: &'static str,
    }
    let mut text = "Successfully applied new settings.".to_string();
    if restart {
        text.push_str(&format!(
            "\nPlease restart your server 'mc admin service restart {target}'."
        ));
    }
    print_msg(json, &ConfigSetMessage { status: "success" }, &text, 1)
}

/// Terminal cell width (lipgloss/go-runewidth): emoji take two cells.
pub(crate) fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| if c as u32 >= 0x1F000 { 2 } else { 1 })
        .sum()
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

/// lipgloss `Style.Width(width).Padding(0, 1).Align(align).Render(text)` for one line.
fn cell(text: &str, width: usize, align: Align) -> String {
    let inner = width.saturating_sub(2);
    let gap = inner.saturating_sub(display_width(text));
    let (left, right) = match align {
        Align::Left => (0, gap),
        Align::Right => (gap, 0),
        Align::Center => (gap / 2, gap - gap / 2),
    };
    format!(" {}{text}{} ", " ".repeat(left), " ".repeat(right))
}

/// lipgloss `RoundedBorder` box around `lines` (padded to the widest line).
pub(crate) fn rounded_box(lines: &[String]) -> String {
    let width = lines.iter().map(|l| display_width(l)).max().unwrap_or(0);
    let mut out = format!("╭{}╮\n", "─".repeat(width));
    for line in lines {
        let pad = width - display_width(line);
        out.push_str(&format!("│{line}{}│\n", " ".repeat(pad)));
    }
    out.push_str(&format!("╰{}╯", "─".repeat(width)));
    out
}

/// mc `idpCfgList.String()`.
pub(crate) fn list_text(items: &[IdpListItem]) -> String {
    let display_name = |item: &IdpListItem| -> String {
        if item.name == admin_idp::DEFAULT_NAME {
            "(default)".to_string()
        } else {
            item.name.clone()
        }
    };
    let name_width = items
        .iter()
        .map(|item| display_name(item).len())
        .fold("Name".len(), usize::max)
        + 2;
    let arn_width = items
        .iter()
        .map(|item| item.role_arn.len())
        .fold("RoleArn".len(), usize::max)
        + 2;
    let mut lines = vec![format!(
        "{}{}{}",
        cell("On?", 5, Align::Center),
        cell("Name", name_width, Align::Center),
        cell("RoleARN", arn_width, Align::Center)
    )];
    for item in items {
        let enabled = if item.enabled { "🟢" } else { "🔴" };
        lines.push(format!(
            "{}{}{}",
            cell(enabled, 5, Align::Center),
            cell(&display_name(item), name_width, Align::Right),
            cell(&item.role_arn, arn_width, Align::Left)
        ));
    }
    rounded_box(&lines)
}

/// mc `idpConfig.String()`.
pub(crate) fn info_text(config: &IdpConfig) -> String {
    let info = config.info.as_deref().unwrap_or_default();
    if info.is_empty() {
        return "Not configured.".to_string();
    }
    let key_width = info.iter().map(|kv| kv.key.len()).max().unwrap_or(0) + 1;
    let enable = info
        .iter()
        .rev()
        .find(|kv| kv.key == "enable")
        .map_or("on", |kv| kv.value.as_str());
    let field = |key: &str| format!("{:>key_width$}", format!("{key}:"));
    let mut lines = vec![format!("{} {enable}", field("enable"))];
    for kv in info.iter().filter(|kv| kv.key != "enable") {
        let env = if kv.is_cfg && kv.is_env {
            " (environment)"
        } else {
            ""
        };
        lines.push(format!("{} {} {env}", field(&kv.key), kv.value));
    }
    rounded_box(&lines)
}

fn idp_type(openid: bool) -> &'static str {
    if openid {
        admin_idp::OPENID
    } else {
        admin_idp::LDAP
    }
}

/// mc `idpListCommon`.
pub(crate) fn list(target: &str, openid: bool, json: bool) -> Result<()> {
    let client = admin_client(target)?;
    let cfg_type = idp_type(openid);
    let items = runtime()?
        .block_on(admin_idp::list_idp_config(&client, cfg_type))
        .with_context(|| fatal_msg("Unable to list IDP config for '%s'", &[cfg_type]))?;
    print_msg(
        json,
        &items,
        &list_text(items.as_deref().unwrap_or_default()),
        2,
    )
}

/// mc `idpInfo`.
pub(crate) fn info(target: &str, openid: bool, name: &str, json: bool) -> Result<()> {
    let client = admin_client(target)?;
    let cfg_type = idp_type(openid);
    let config = runtime()?
        .block_on(admin_idp::get_idp_config(&client, cfg_type, name))
        .with_context(|| fatal_msg("Unable to get %s IDP config from server", &[cfg_type]))?;
    print_msg(json, &config, &info_text(&config), 2)
}

/// mc `idpRemove`.
pub(crate) fn remove(target: &str, openid: bool, name: &str, json: bool) -> Result<()> {
    let client = admin_client(target)?;
    let cfg_type = idp_type(openid);
    let restart = runtime()?
        .block_on(admin_idp::delete_idp_config(&client, cfg_type, name))
        .with_context(|| fatal_msg("Unable to remove %s IDP config '%s'", &[cfg_type, name]))?;
    print_config_set(json, target, restart)
}

/// mc `idpEnableDisable`.
pub(crate) fn enable_disable(
    target: &str,
    openid: bool,
    name: &str,
    enable: bool,
    json: bool,
) -> Result<()> {
    let client = admin_client(target)?;
    let cfg_type = idp_type(openid);
    let body = if enable { "enable=" } else { "enable=off" };
    let restart = runtime()?
        .block_on(admin_idp::add_or_update_idp_config(
            &client, cfg_type, name, body, true,
        ))
        // mc reuses the remove message here.
        .with_context(|| fatal_msg("Unable to remove %s IDP config '%s'", &[cfg_type, name]))?;
    print_config_set(json, target, restart)
}

/// Splits mc's `TARGET [CFG_NAME] [CFG_PARAMS...]`: the second argument is a config name
/// unless it contains `=`. Returns (name, params joined with spaces).
pub(crate) fn split_cfg_args(args: &[String]) -> (String, String) {
    match args.get(1) {
        Some(first) if !first.contains('=') => (first.clone(), args[2..].join(" ")),
        _ => (
            admin_idp::DEFAULT_NAME.to_string(),
            args.get(1..).unwrap_or_default().join(" "),
        ),
    }
}

/// `AddOrUpdateIDPConfig` + `configSetMessage`; `context` is mc's failure message.
pub(crate) fn add_or_update(
    target: &str,
    openid: bool,
    name: &str,
    params: &str,
    update: bool,
    context: &'static str,
    json: bool,
) -> Result<()> {
    let client = admin_client(target)?;
    let restart = runtime()?
        .block_on(admin_idp::add_or_update_idp_config(
            &client,
            idp_type(openid),
            name,
            params,
            update,
        ))
        .context(context)?;
    print_config_set(json, target, restart)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::admin_idp::IdpCfgInfo;

    fn item(name: &str, enabled: bool, arn: &str) -> IdpListItem {
        IdpListItem {
            cfg_type: "openid".into(),
            name: name.into(),
            enabled,
            role_arn: arn.into(),
        }
    }

    #[test]
    fn list_box_matches_mc() {
        assert_eq!(
            list_text(&[item("_", false, "")]),
            "╭─────────────────────────╮\n\
             │ On?    Name     RoleARN │\n\
             │ 🔴   (default)          │\n\
             ╰─────────────────────────╯"
        );
        let text = list_text(&[
            item(
                "_",
                true,
                "arn:minio:iam:::role/nOybJqMNzNmroqEKq5D0EUsRZw0",
            ),
            item(
                "dex2",
                true,
                "arn:minio:iam:::role/0JQeaNqPOBUf-Gph_Fn3xc-fyqI",
            ),
        ]);
        assert_eq!(
            text,
            "╭──────────────────────────────────────────────────────────────────╮\n\
             │ On?    Name                         RoleARN                      │\n\
             │ 🟢   (default)  arn:minio:iam:::role/nOybJqMNzNmroqEKq5D0EUsRZw0 │\n\
             │ 🟢        dex2  arn:minio:iam:::role/0JQeaNqPOBUf-Gph_Fn3xc-fyqI │\n\
             ╰──────────────────────────────────────────────────────────────────╯"
        );
        assert_eq!(list_text(&[]).lines().count(), 3);
    }

    #[test]
    fn info_box_matches_mc() {
        let kv = |key: &str, value: &str, is_env: bool| IdpCfgInfo {
            key: key.into(),
            value: value.into(),
            is_cfg: true,
            is_env,
        };
        let config = IdpConfig {
            cfg_type: "openid".into(),
            name: "_".into(),
            info: Some(vec![
                kv("client_id", "other", false),
                kv("enable", "on", false),
                kv("scopes", "openid", true),
            ]),
        };
        assert_eq!(
            info_text(&config),
            "╭────────────────────────────────╮\n\
             │   enable: on                   │\n\
             │client_id: other                │\n\
             │   scopes: openid  (environment)│\n\
             ╰────────────────────────────────╯"
        );
        assert_eq!(info_text(&IdpConfig::default()), "Not configured.");
    }

    #[test]
    fn cfg_args_split_like_mc() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            split_cfg_args(&args(&["a/", "k=v", "x=y"])),
            ("_".to_string(), "k=v x=y".to_string())
        );
        assert_eq!(
            split_cfg_args(&args(&["a/", "dex", "k=v"])),
            ("dex".to_string(), "k=v".to_string())
        );
        assert_eq!(
            split_cfg_args(&args(&["a/", "dex"])),
            ("dex".to_string(), String::new())
        );
    }
}
