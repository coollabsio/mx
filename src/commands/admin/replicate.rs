//! `mx admin replicate` (mc `admin replicate`): MinIO site replication (`site-replication/*`
//! admin API): add, update, remove, info, status and resync start|status|cancel.

use super::decommission::show_help;
use crate::commands::replicate::{go_duration_string, humanized_duration, pretty_row, rel_time};
use crate::commands::runtime;
use crate::config::ConfigStore;
use crate::error::McError;
use crate::output;
use crate::s3::admin::{self, AdminClient};
use crate::s3::admin_topo::{
    self, BucketBandwidth, PeerInfo, PeerSite, REPLICATE_REMOVE_STATUS_SUCCESS,
    SiteReplicationInfo, SiteResyncMetrics, SrEntity, SrRemoveReq, SrStatusInfo, SrStatusOptions,
};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};

/// mc `dot` / `check` and the status cell markers.
const DOT: &str = "●";
const CHECK: &str = "✔";
const TICK_CELL: &str = "✔ ";
const CROSS_TICK_CELL: &str = "✗ ";
const BLANK_CELL: &str = " ";
/// Width of every `admin replicate status` table column.
const FIELD_LEN: usize = 15;

#[derive(Debug, Args)]
pub struct ReplicateArgs {
    #[command(subcommand)]
    pub command: ReplicateCommand,
}

#[derive(Debug, Subcommand)]
pub enum ReplicateCommand {
    #[command(name = "add", about = "add one or more sites for replication")]
    Add(ReplicateAddArgs),
    #[command(
        name = "update",
        alias = "edit",
        about = "modify endpoint of site participating in site replication"
    )]
    Update(ReplicateUpdateArgs),
    #[command(
        name = "remove",
        visible_alias = "rm",
        about = "remove one or more sites from site replication"
    )]
    Remove(ReplicateRemoveArgs),
    #[command(name = "info", about = "get site replication information")]
    Info(ReplicateInfoArgs),
    #[command(name = "status", about = "display site replication status")]
    Status(ReplicateStatusArgs),
    #[command(name = "resync", about = "resync content to site")]
    Resync(ReplicateResyncArgs),
}

#[derive(Debug, Args)]
pub struct ReplicateAddArgs {
    #[arg(value_name = "ALIAS1 ALIAS2 [ALIAS3...]")]
    pub aliases: Vec<String>,
    #[arg(long = "replicate-ilm-expiry", help = "replicate ILM expiry rules")]
    pub replicate_ilm_expiry: bool,
}

#[derive(Debug, Args)]
pub struct ReplicateUpdateArgs {
    #[arg(value_name = "ALIAS")]
    pub args: Vec<String>,
    #[arg(
        long = "deployment-id",
        value_name = "VALUE",
        help = "deployment id of the site, should be a unique value"
    )]
    pub deployment_id: Option<String>,
    #[arg(
        long = "endpoint",
        value_name = "VALUE",
        help = "endpoint for the site"
    )]
    pub endpoint: Option<String>,
    #[arg(
        long = "mode",
        value_name = "VALUE",
        help = "change mode of replication for this target, valid values are ['sync', 'async']."
    )]
    pub mode: Option<String>,
    #[arg(
        long = "bucket-bandwidth",
        value_name = "VALUE",
        help = "Set default bandwidth limit for bucket in bytes per second (K,B,G,T for metric and Ki,Bi,Gi,Ti for IEC units)"
    )]
    pub bucket_bandwidth: Option<String>,
    #[arg(
        long = "disable-ilm-expiry-replication",
        help = "disable ILM expiry rules replication"
    )]
    pub disable_ilm_expiry_replication: bool,
    #[arg(
        long = "enable-ilm-expiry-replication",
        help = "enable ILM expiry rules replication"
    )]
    pub enable_ilm_expiry_replication: bool,
    /// Deprecated (Jul 2023) in favor of `--mode`; mc's default value is `disable`.
    #[arg(
        long = "sync",
        hide = true,
        value_name = "VALUE",
        help = "enable synchronous replication for this target, valid values are ['enable', 'disable']."
    )]
    pub sync: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReplicateRemoveArgs {
    #[arg(value_name = "TARGET [SITES...]")]
    pub args: Vec<String>,
    #[arg(
        long = "all",
        help = "remove site replication from all participating sites"
    )]
    pub all: bool,
    #[arg(
        long = "force",
        help = "force removal of site(s) from site replication configuration"
    )]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ReplicateInfoArgs {
    #[arg(value_name = "ALIAS")]
    pub args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ReplicateStatusArgs {
    #[arg(value_name = "TARGET")]
    pub args: Vec<String>,
    #[arg(long = "buckets", help = "display only buckets")]
    pub buckets: bool,
    #[arg(long = "policies", help = "display only policies")]
    pub policies: bool,
    #[arg(long = "users", help = "display only users")]
    pub users: bool,
    #[arg(long = "groups", help = "display only groups")]
    pub groups: bool,
    #[arg(long = "ilm-expiry-rules", help = "display only ilm expiry rules")]
    pub ilm_expiry_rules: bool,
    #[arg(long = "all", help = "display all available site replication status")]
    pub all: bool,
    #[arg(
        long = "bucket",
        value_name = "VALUE",
        help = "display bucket sync status"
    )]
    pub bucket: Option<String>,
    #[arg(
        long = "policy",
        value_name = "VALUE",
        help = "display policy sync status"
    )]
    pub policy: Option<String>,
    #[arg(long = "user", value_name = "VALUE", help = "display user sync status")]
    pub user: Option<String>,
    #[arg(
        long = "group",
        value_name = "VALUE",
        help = "display group sync status"
    )]
    pub group: Option<String>,
    #[arg(
        long = "ilm-expiry-rule",
        value_name = "VALUE",
        help = "display ILM expiry rule sync status"
    )]
    pub ilm_expiry_rule: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReplicateResyncArgs {
    #[command(subcommand)]
    pub command: ReplicateResyncCommand,
}

#[derive(Debug, Subcommand)]
pub enum ReplicateResyncCommand {
    #[command(name = "start", about = "start resync to site")]
    Start(ReplicateResyncTargetArgs),
    #[command(name = "status", about = "show site replication resync status")]
    Status(ReplicateResyncTargetArgs),
    #[command(name = "cancel", about = "cancel ongoing resync operation")]
    Cancel(ReplicateResyncTargetArgs),
}

#[derive(Debug, Args)]
pub struct ReplicateResyncTargetArgs {
    #[arg(value_name = "ALIAS1 ALIAS2")]
    pub args: Vec<String>,
}

pub fn run(args: ReplicateArgs, json: bool) -> Result<()> {
    match args.command {
        ReplicateCommand::Add(args) => add(args, json),
        ReplicateCommand::Update(args) => update(args, json),
        ReplicateCommand::Remove(args) => remove(args, json),
        ReplicateCommand::Info(args) => info(args, json),
        ReplicateCommand::Status(args) => status(args, json),
        ReplicateCommand::Resync(args) => match args.command {
            ReplicateResyncCommand::Start(args) => resync_op(args, "start", json),
            ReplicateResyncCommand::Status(args) => resync_status(args, json),
            ReplicateResyncCommand::Cancel(args) => resync_op(args, "cancel", json),
        },
    }
}

/// mc `fatalIf(errInvalidArgument(), message)`.
fn invalid_argument(message: &str) -> anyhow::Error {
    anyhow::Error::new(McError::invalid_argument()).context(message.to_string())
}

fn connect(target: &str) -> Result<AdminClient> {
    admin::admin_client_for(&ConfigStore::load_or_create()?, target)
}

/// mc `printMsg` text: one trailing newline is trimmed before printing the line.
fn print_text(text: &str) {
    println!("{}", text.strip_suffix('\n').unwrap_or(text));
}

// ---------------------------------------------------------------------------
// add
// ---------------------------------------------------------------------------

fn add(args: ReplicateAddArgs, json: bool) -> Result<()> {
    if args.aliases.len() < 2 {
        return Err(invalid_argument(
            "Need at least two arguments to add command.",
        ));
    }
    let store = ConfigStore::load_or_create()?;
    let client = admin::admin_client_for(&store, &args.aliases[0])?;
    let mut sites = Vec::with_capacity(args.aliases.len());
    for name in &args.aliases {
        sites.push(peer_site(&store, name)?);
    }
    let status = runtime()?
        .block_on(admin_topo::site_replication_add(
            &client,
            &sites,
            args.replicate_ilm_expiry,
        ))
        .context("Unable to add sites for replication")?;
    if json {
        return output::print_json(&status);
    }
    let mut messages = vec![status.status.as_str()];
    if !status.err_detail.is_empty() {
        messages.push(&status.err_detail);
    }
    if !status.initial_sync_error_message.is_empty() {
        messages.push(&status.initial_sync_error_message);
    }
    print_text(&messages.join("\n"));
    Ok(())
}

/// madmin `PeerSite` for an alias: its endpoint URL (`scheme://host[:port]`) and credentials.
fn peer_site(store: &ConfigStore, aliased_url: &str) -> Result<PeerSite> {
    const MESSAGE: &str = "unable to initialize admin connection";
    let rewrap = |err: anyhow::Error| match err.downcast_ref::<McError>() {
        Some(cause) => anyhow::Error::new(cause.clone()).context(MESSAGE),
        None => err.context(MESSAGE),
    };
    admin::admin_client_for(store, aliased_url).map_err(rewrap)?;
    let alias = aliased_url.split('/').next().unwrap_or_default();
    let config = store.alias(alias).map_err(rewrap)?;
    let endpoint = match url::Url::parse(&config.url) {
        Ok(url) => {
            let host = url.host_str().unwrap_or_default();
            match url.port() {
                Some(port) => format!("{}://{host}:{port}", url.scheme()),
                None => format!("{}://{host}", url.scheme()),
            }
        }
        Err(_) => config.url.clone(),
    };
    Ok(PeerSite {
        name: aliased_url.to_string(),
        endpoint,
        access_key: config.access_key,
        secret_key: config.secret_key,
    })
}

// ---------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------

fn update(args: ReplicateUpdateArgs, json: bool) -> Result<()> {
    if args.args.is_empty() {
        show_help(&["admin", "replicate", "update"]);
    }
    if args.args.len() != 1 {
        return Err(invalid_argument(
            "Invalid arguments specified for edit command.",
        ));
    }
    let client = connect(&args.args[0])?;
    let peer = update_peer(&args)?;
    let status = runtime()?
        .block_on(admin_topo::site_replication_edit(
            &client,
            &peer,
            args.disable_ilm_expiry_replication,
            args.enable_ilm_expiry_replication,
        ))
        .context("Unable to edit cluster replication site endpoint")?;
    if json {
        return output::print_json(&status);
    }
    let mut messages = vec![status.status.as_str()];
    if !status.err_detail.is_empty() {
        messages.push(&status.err_detail);
    }
    print_text(&messages.join("\n"));
    Ok(())
}

/// Validates the `update` flags like mc and builds the `PeerInfo` to send.
fn update_peer(args: &ReplicateUpdateArgs) -> Result<PeerInfo> {
    let disable = args.disable_ilm_expiry_replication;
    let enable = args.enable_ilm_expiry_replication;
    if args.deployment_id.is_none() && !disable && !enable {
        return Err(invalid_argument("--deployment-id is a required flag"));
    }
    if args.endpoint.is_none()
        && args.mode.is_none()
        && args.sync.is_none()
        && args.bucket_bandwidth.is_none()
        && !disable
        && !enable
    {
        return Err(invalid_argument(
            "--endpoint, --mode, --bucket-bandwidth, --disable-ilm-expiry-replication or --enable-ilm-expiry-replication is a required flag",
        ));
    }
    if args.mode.is_some() && args.sync.is_some() {
        return Err(invalid_argument(
            "either --sync or --mode flag should be specified",
        ));
    }
    if disable && enable {
        return Err(invalid_argument(
            "either --disable-ilm-expiry-replication or --enable-ilm-expiry-replication flag should be specified",
        ));
    }
    if (disable || enable) && args.deployment_id.is_some() {
        return Err(invalid_argument(
            "--deployment-id should not be set with --disable-ilm-expiry-replication or --enable-ilm-expiry-replication",
        ));
    }
    let mut sync_state = String::new();
    if let Some(sync) = &args.sync {
        sync_state = sync.to_lowercase();
        if sync_state != "enable" && sync_state != "disable" {
            return Err(invalid_argument("--sync can be either [enable|disable]"));
        }
    }
    if let Some(mode) = &args.mode {
        sync_state = match mode.to_lowercase().as_str() {
            "sync" => "enable".to_string(),
            "async" => "disable".to_string(),
            _ => return Err(invalid_argument("--mode can be either [sync|async]")),
        };
    }
    let mut bandwidth = BucketBandwidth::default();
    if let Some(value) = &args.bucket_bandwidth {
        // mc `getBandwidthInBytes`: an empty value means no limit.
        if !value.is_empty() {
            bandwidth.limit = admin::parse_bytes(value).context("invalid bandwidth value")?;
        }
        bandwidth.is_set = true;
    }
    let mut endpoint = String::new();
    if let Some(value) = &args.endpoint {
        if let Err(err) = check_go_url(value) {
            return Err(invalid_argument(&format!("Unsupported URL format {err}")));
        }
        endpoint = value.clone();
    }
    Ok(PeerInfo {
        deployment_id: args.deployment_id.clone().unwrap_or_default(),
        endpoint,
        sync: sync_state,
        default_bandwidth: bandwidth,
        ..Default::default()
    })
}

/// The Go `url.Parse` errors a user can hit: invalid `%` escapes and a missing scheme.
fn check_go_url(input: &str) -> std::result::Result<(), String> {
    if input.starts_with(':') {
        return Err(format!("parse {input:?}: missing protocol scheme"));
    }
    let bytes = input.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if *byte != b'%' {
            continue;
        }
        let valid = bytes.len() > i + 2
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit();
        if !valid {
            let end = input.len().min(i + 3);
            let escape = input.get(i..end).unwrap_or("%");
            return Err(format!("parse {input:?}: invalid URL escape {escape:?}"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// remove
// ---------------------------------------------------------------------------

fn remove(args: ReplicateRemoveArgs, json: bool) -> Result<()> {
    if args.all && args.args.len() > 1 {
        return Err(invalid_argument(""));
    }
    if args.args.len() < 2 && !args.all {
        return Err(invalid_argument(
            "Need at least two arguments to remove command.",
        ));
    }
    if !args.force {
        return Err(anyhow::Error::new(McError::new("")).context(
            "Site removal requires --force flag. This operation is *IRREVERSIBLE*. Please review carefully before performing this *DANGEROUS* operation.",
        ));
    }
    let target = args.args.first().cloned().unwrap_or_default();
    let sites: Vec<String> = args.args.iter().skip(1).cloned().collect();
    let request = SrRemoveReq {
        site_names: (!sites.is_empty()).then(|| sites.clone()),
        remove_all: args.all,
        ..Default::default()
    };
    let client = connect(&target)?;
    let status = runtime()?
        .block_on(admin_topo::site_replication_remove(&client, &request))
        .context("Unable to remove cluster replication")?;
    if json {
        return output::print_json(&status);
    }
    print_text(&remove_text(
        &status.status,
        &status.err_detail,
        &sites,
        args.all,
    ));
    Ok(())
}

/// mc `srRemoveStatus.String()`; sites print like Go's `%s` of a string slice (`[a b]`).
fn remove_text(status: &str, err_detail: &str, sites: &[String], all: bool) -> String {
    if all {
        return "All site(s) were removed successfully".to_string();
    }
    let list = format!("[{}]", sites.join(" "));
    if status == REPLICATE_REMOVE_STATUS_SUCCESS {
        return format!("Following site(s) {list} were removed successfully");
    }
    if sites.len() == 1 {
        return format!(
            "Following site {list} was removed partially, some operations failed:\nERROR: '{err_detail}'"
        );
    }
    format!(
        "Following site(s) {list} were removed partially, some operations failed: \nERROR: '{err_detail}'"
    )
}

// ---------------------------------------------------------------------------
// info
// ---------------------------------------------------------------------------

fn info(args: ReplicateInfoArgs, json: bool) -> Result<()> {
    if args.args.len() != 1 {
        return Err(invalid_argument("Need exactly one alias argument."));
    }
    let client = connect(&args.args[0])?;
    let info = runtime()?
        .block_on(admin_topo::site_replication_info(&client))
        .context("Unable to get cluster replication information")?;
    if json {
        return output::print_json(&info);
    }
    print_text(&info_text(&info));
    Ok(())
}

/// mc `srInfo.String()`.
fn info_text(info: &SiteReplicationInfo) -> String {
    if !info.enabled {
        return "SiteReplication is not enabled".to_string();
    }
    const WIDTHS: [usize; 6] = [36, 15, 46, 4, 10, 25];
    let mut messages = vec![
        "SiteReplication enabled for:\n".to_string(),
        pretty_row(
            &WIDTHS,
            &[
                "Deployment ID",
                "Site Name",
                "Endpoint",
                "Sync",
                "Bandwidth",
                "ILM Expiry Replication",
            ],
        ),
        pretty_row(&WIDTHS, &["", "", "", "", "Per Bucket", ""]),
    ];
    for peer in &info.sites {
        let sync = if peer.sync == "enable" { CHECK } else { "" };
        // N/A: no default bandwidth configured for the peer.
        let limit = if peer.default_bandwidth.limit > 0 {
            format!("{}/s", admin::si_bytes(peer.default_bandwidth.limit))
        } else {
            "N/A".to_string()
        };
        messages.push(pretty_row(
            &WIDTHS,
            &[
                &peer.deployment_id,
                &peer.name,
                &peer.endpoint,
                sync,
                &limit,
                &peer.replicate_ilm_expiry.to_string(),
            ],
        ));
    }
    messages.join("\n")
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

fn status(args: ReplicateStatusArgs, json: bool) -> Result<()> {
    if args.args.len() != 1 {
        return Err(invalid_argument("Need exactly one alias argument."));
    }
    let group = args.buckets || args.groups || args.users || args.policies || args.ilm_expiry_rules;
    let entities = [
        args.bucket.is_some(),
        args.user.is_some(),
        args.group.is_some(),
        args.policy.is_some(),
        args.ilm_expiry_rule.is_some(),
    ];
    if group && entities.contains(&true) {
        return Err(invalid_argument(
            "Cannot specify both (bucket|group|policy|user|ilm-expiry-rule) flag and one or more of buckets|groups|policies|users|ilm-expiry-rules) flag(s)",
        ));
    }
    if entities.iter().filter(|set| **set).count() > 1 {
        return Err(invalid_argument(
            "Cannot specify more than one of --bucket, --policy, --user, --group, --ilm-expiry-rule  flags at the same time",
        ));
    }
    let client = connect(&args.args[0])?;
    let opts = status_options(&args);
    let info = runtime()?
        .block_on(admin_topo::sr_status_info(&client, &opts))
        .context("Unable to get cluster replication status")?;
    if json {
        return output::print_json(&info);
    }
    print_text(&status_text(&info, &opts));
    Ok(())
}

/// mc `srStatusOpts`: everything (with metrics) unless a filter flag is given, or `--all`.
fn status_options(args: &ReplicateStatusArgs) -> SrStatusOptions {
    let entities = [
        (SrEntity::Bucket, &args.bucket),
        (SrEntity::User, &args.user),
        (SrEntity::Group, &args.group),
        (SrEntity::Policy, &args.policy),
        (SrEntity::IlmExpiryRule, &args.ilm_expiry_rule),
    ];
    let any_filter = args.buckets
        || args.users
        || args.groups
        || args.policies
        || args.ilm_expiry_rules
        || entities.iter().any(|(_, value)| value.is_some());
    if !any_filter || args.all {
        return SrStatusOptions {
            buckets: true,
            policies: true,
            users: true,
            groups: true,
            metrics: true,
            ilm_expiry_rules: true,
            ..Default::default()
        };
    }
    let mut opts = SrStatusOptions {
        buckets: args.buckets,
        policies: args.policies,
        users: args.users,
        groups: args.groups,
        ilm_expiry_rules: args.ilm_expiry_rules,
        ..Default::default()
    };
    if let Some((entity, Some(value))) = entities.iter().find(|(_, value)| value.is_some()) {
        opts.entity = *entity;
        opts.entity_value = value.clone();
    }
    opts
}

/// Row of `FIELD_LEN`-wide columns (mc `newPrettyTable(" | ", ...).buildRow`).
fn status_row(cells: &[String]) -> String {
    let widths = vec![FIELD_LEN; cells.len()];
    let cells: Vec<&str> = cells.iter().map(String::as_str).collect();
    pretty_row(&widths, &cells)
}

/// Site names (upper-cased, sorted) and their deployment IDs.
fn site_names(info: &SrStatusInfo) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = info
        .sites
        .iter()
        .flatten()
        .map(|(id, peer)| (peer.name.to_uppercase(), id.clone()))
        .collect();
    names.sort();
    names
}

/// mc `srStatus.siteHeader`.
fn site_header(sites: &[(String, String)], legend: &str) -> String {
    let mut cells = vec![legend.to_string()];
    cells.extend(sites.iter().map(|(name, _)| name.clone()));
    status_row(&cells)
}

/// mc `syncStatus`: blank when not set, cross on mismatch, tick otherwise.
fn sync_cell(mismatch: bool, set: bool) -> String {
    match (set, mismatch) {
        (false, _) => BLANK_CELL,
        (true, true) => CROSS_TICK_CELL,
        (true, false) => TICK_CELL,
    }
    .to_string()
}

/// Names of one `... replication status:` section.
struct Section<'a> {
    /// `Bucket` in `Bucket replication status:`.
    title: &'a str,
    /// `Buckets` in `No Buckets present` / `1/1 Buckets in sync`.
    plural: &'a str,
    /// First column header of the mismatch table.
    legend: &'a str,
    /// mc adds a blank line after every bucket / ILM rule row, but only once after the
    /// policy / user / group table.
    blank_line_per_row: bool,
}

/// One `... replication status:` section; `cell(stats)` renders one site's cell.
fn entity_section<T: Default>(
    messages: &mut Vec<String>,
    sites: &[(String, String)],
    section: Section,
    max: i64,
    stats: Option<&BTreeMap<String, BTreeMap<String, T>>>,
    cell: impl Fn(&T) -> String,
) {
    messages.push(format!("{} replication status:", section.title));
    if max == 0 {
        messages.push(format!("No {} present\n", section.plural));
        return;
    }
    let empty = BTreeMap::new();
    let stats = stats.unwrap_or(&empty);
    messages.push(format!(
        "{DOT}  {}/{max} {} in sync\n",
        max - stats.len() as i64,
        section.plural
    ));
    if !stats.is_empty() {
        messages.push(site_header(sites, section.legend));
    }
    let default = T::default();
    for (name, by_site) in stats {
        let mut cells = vec![name.clone()];
        for (_, id) in sites {
            cells.push(cell(by_site.get(id).unwrap_or(&default)));
        }
        messages.push(status_row(&cells));
        if section.blank_line_per_row {
            messages.push(String::new());
        }
    }
    if !section.blank_line_per_row && !stats.is_empty() {
        messages.push(String::new());
    }
}

/// Label of a summary row and its `(mismatch, set)` status for one site.
type RowStatus<'a, T> = (&'a str, &'a dyn Fn(&T) -> (bool, bool));

/// Summary table of one entity (`--bucket NAME`, `--user NAME`, ...).
fn entity_summary<T: Default>(
    sites: &[(String, String)],
    (kind, title, legend): (&str, &str, &str),
    value: &str,
    stats: Option<&BTreeMap<String, BTreeMap<String, T>>>,
    found: impl Fn(&T) -> bool,
    rows: &[RowStatus<T>],
) -> Vec<String> {
    let Some(by_site) = stats
        .and_then(|stats| stats.get(value))
        .filter(|by_site| by_site.values().any(&found))
    else {
        return vec![format!("{kind} {value} not found\n")];
    };
    let default = T::default();
    let mut messages = vec![
        format!("{DOT}  {title} replication summary for: {value}\n"),
        site_header(sites, legend),
    ];
    for (label, status) in rows {
        let mut cells = vec![label.to_string()];
        for (_, id) in sites {
            let (mismatch, set) = status(by_site.get(id).unwrap_or(&default));
            cells.push(sync_cell(mismatch, set));
        }
        messages.push(status_row(&cells));
    }
    messages
}

/// Cell of the policy / user / group / ILM rule tables.
fn in_sync(has: bool, mismatch: bool) -> String {
    if !has {
        BLANK_CELL.to_string()
    } else if mismatch {
        format!("{CROSS_TICK_CELL} in-sync")
    } else {
        format!("{TICK_CELL} in-sync")
    }
}

/// mc `srStatus.String()`.
fn status_text(info: &SrStatusInfo, opts: &SrStatusOptions) -> String {
    if !info.enabled {
        return "SiteReplication is not enabled".to_string();
    }
    let sites = site_names(info);
    let mut messages: Vec<String> = Vec::new();
    let section = |title, plural, legend, blank_line_per_row| Section {
        title,
        plural,
        legend,
        blank_line_per_row,
    };
    if opts.buckets {
        entity_section(
            &mut messages,
            &sites,
            section("Bucket", "Buckets", "Bucket", true),
            info.max_buckets,
            info.bucket_stats.as_ref(),
            |ss| {
                if !ss.has_bucket {
                    format!("{BLANK_CELL} ")
                } else if ss.olock_config_mismatch
                    || ss.policy_mismatch
                    || ss.quota_cfg_mismatch
                    || ss.replication_cfg_mismatch
                    || ss.tag_mismatch
                {
                    format!("{CROSS_TICK_CELL} in-sync")
                } else {
                    format!("{TICK_CELL} in-sync")
                }
            },
        );
    }
    if opts.policies {
        entity_section(
            &mut messages,
            &sites,
            section("Policy", "Policies", "Policy", false),
            info.max_policies,
            info.policy_stats.as_ref(),
            |ss| in_sync(ss.has_policy, ss.policy_mismatch),
        );
    }
    if opts.users {
        entity_section(
            &mut messages,
            &sites,
            section("User", "Users", "User", false),
            info.max_users,
            info.user_stats.as_ref(),
            |ss| in_sync(ss.has_user, ss.user_info_mismatch),
        );
    }
    if opts.groups {
        entity_section(
            &mut messages,
            &sites,
            section("Group", "Groups", "Group", false),
            info.max_groups,
            info.group_stats.as_ref(),
            |ss| in_sync(ss.has_group, ss.group_desc_mismatch),
        );
    }
    if opts.ilm_expiry_rules {
        if info.max_ilm_expiry_rules != 0 && info.ilm_expiry_stats.is_none() {
            messages.push("ILM Expiry Rules replication status:".to_string());
            messages.push("Replication of ILM Expiry is not enabled\n".to_string());
        } else {
            entity_section(
                &mut messages,
                &sites,
                section(
                    "ILM Expiry Rules",
                    "ILM Expiry Rules",
                    "ILM Expiry Rules",
                    true,
                ),
                info.max_ilm_expiry_rules,
                info.ilm_expiry_stats.as_ref(),
                |ss| in_sync(ss.has_ilm_expiry_rules, ss.ilm_expiry_rule_mismatch),
            );
        }
    }
    let value = opts.entity_value.as_str();
    match opts.entity {
        SrEntity::Unspecified => {}
        SrEntity::Bucket => messages.extend(entity_summary(
            &sites,
            ("Bucket", "Bucket config", "Bucket"),
            value,
            info.bucket_stats.as_ref(),
            |ss| ss.has_bucket,
            &[
                ("Tags", &|ss| (ss.tag_mismatch, ss.has_tags_set)),
                ("Policy", &|ss| (ss.policy_mismatch, ss.has_policy_set)),
                ("Quota", &|ss| (ss.quota_cfg_mismatch, ss.has_quota_cfg_set)),
                ("Retention", &|ss| {
                    (ss.olock_config_mismatch, ss.has_olock_config_set)
                }),
                ("Encryption", &|ss| {
                    (ss.sse_config_mismatch, ss.has_sse_cfg_set)
                }),
                ("Replication", &|ss| {
                    (ss.replication_cfg_mismatch, ss.has_replication_cfg)
                }),
            ],
        )),
        SrEntity::Policy => messages.extend(entity_summary(
            &sites,
            ("Policy", "Policy", "Policy"),
            value,
            info.policy_stats.as_ref(),
            |ss| ss.has_policy,
            &[("Policy", &|ss| (ss.policy_mismatch, ss.has_policy))],
        )),
        SrEntity::User => messages.extend(entity_summary(
            &sites,
            ("User", "User", "User"),
            value,
            info.user_stats.as_ref(),
            |ss| ss.has_user,
            &[
                ("Info", &|ss| (ss.user_info_mismatch, ss.has_user)),
                ("Policy mapping", &|ss| {
                    (ss.policy_mismatch, ss.has_policy_mapping)
                }),
            ],
        )),
        SrEntity::Group => messages.extend(entity_summary(
            &sites,
            ("Group", "Group", "Group"),
            value,
            info.group_stats.as_ref(),
            |ss| ss.has_group,
            &[
                ("Info", &|ss| (ss.group_desc_mismatch, ss.has_group)),
                ("Policy mapping", &|ss| {
                    (ss.policy_mismatch, ss.has_policy_mapping)
                }),
            ],
        )),
        SrEntity::IlmExpiryRule => messages.extend(entity_summary(
            &sites,
            ("ILM Expiry Rule", "ILM Expiry Rule", "ILMExpiryRule"),
            value,
            info.ilm_expiry_stats.as_ref(),
            |ss| ss.has_ilm_expiry_rules,
            &[("ILM Expiry Rule", &|ss| {
                (ss.ilm_expiry_rule_mismatch, ss.has_ilm_expiry_rules)
            })],
        )),
    }
    if opts.metrics {
        metrics_section(&mut messages, info);
    }
    messages.join("\n")
}

/// Go `time.Duration.Round(time.Millisecond)` (halves away from zero).
fn round_millis(nanos: i64) -> i64 {
    const MS: i64 = 1_000_000;
    if nanos < 0 {
        return -round_millis(-nanos);
    }
    let r = nanos % MS;
    if r + r < MS {
        nanos - r
    } else {
        nanos + MS - r
    }
}

/// `Object replication status:` section of `admin replicate status`.
fn metrics_section(messages: &mut Vec<String>, info: &SrStatusInfo) {
    let ms = &info.metrics;
    let (q, w) = (&ms.queued, &ms.active_workers);
    let metrics: Vec<_> = ms.metrics.iter().flatten().map(|(_, m)| m).collect();
    let single = metrics.len() == 1;
    let count = |value: f64| admin::comma(value as i64);
    let bytes = |value: f64| admin::ibytes(value as u64);
    let queued = format!(
        "Queued:        {DOT} {} objects, ({}) (avg: {} objects, {}; max: {} objects, {})",
        count(q.curr.count),
        bytes(q.curr.bytes),
        count(q.avg.count),
        bytes(q.avg.bytes),
        count(q.max.count),
        bytes(q.max.bytes)
    );
    let received = format!(
        "Received:      {} objects ({})",
        admin::comma(ms.replica_count),
        admin::ibytes(ms.replica_size as u64)
    );
    messages.push("Object replication status:".to_string());
    messages.push(format!("Replication status since {}", rel_time(ms.uptime)));
    let (mut replicated_count, mut replicated_size) = (0i64, 0i64);
    for m in &metrics {
        messages.push(m.endpoint.clone());
        messages.push(format!(
            "Replicated:    {} objects ({})",
            admin::comma(m.replicated_count),
            admin::ibytes(m.replicated_size as u64)
        ));
        if single {
            messages.push(received.clone());
            messages.push(queued.clone());
            messages.push(format!(
                "Workers:       {} (avg: {}; max {}) ",
                admin::comma(w.curr),
                admin::comma(w.avg as i64),
                admin::comma(w.max)
            ));
        } else {
            replicated_count += m.replicated_count;
            replicated_size += m.replicated_size;
        }
        if let Some(total) = m.xfer_stats.as_ref().and_then(|x| x.get("Total")) {
            let rate = |value: f64| admin::si_bytes(value as u64);
            messages.push(format!(
                "Transfer Rate: {}/s (avg: {}/s; max {}/s)",
                rate(total.curr_rate),
                rate(total.avg_rate),
                rate(total.peak_rate)
            ));
            let latency = |nanos: i64| go_duration_string(round_millis(nanos));
            messages.push(format!(
                "Latency:       {} (avg: {}; max {})",
                latency(m.latency.curr),
                latency(m.latency.avg),
                latency(m.latency.max)
            ));
        }
        let current_downtime = if !m.online && !m.last_online.is_zero() {
            m.last_online.elapsed_nanos()
        } else {
            0
        };
        // The server updates the total downtime at heartbeat intervals; it may lag behind.
        let total_downtime = m.total_downtime.max(current_downtime);
        let link = if m.online {
            format!(
                "{DOT} online (total downtime: {})",
                humanized_duration(total_downtime)
            )
        } else {
            format!(
                "{DOT} offline {} (total downtime: {})",
                humanized_duration(current_downtime),
                humanized_duration(total_downtime)
            )
        };
        messages.push(format!("Link:          {link}"));
        messages.push(format!(
            "Errors:        {} in last 1 minute; {} in last 1hr; {} since uptime",
            count(m.failed.last_minute.count),
            count(m.failed.last_hour.count),
            count(m.failed.totals.count)
        ));
        messages.push(String::new());
    }
    if !single {
        messages.push("Summary:".to_string());
        messages.push(format!(
            "Replicated:    {} objects ({})",
            admin::comma(replicated_count),
            admin::ibytes(replicated_size as u64)
        ));
        messages.push(queued);
        messages.push(received);
    }
}

// ---------------------------------------------------------------------------
// resync
// ---------------------------------------------------------------------------

/// mc's resync preamble: ALIAS1's site replication info and the entry of ALIAS2's deployment.
/// `None` when site replication is not enabled and `require_enabled` is false.
fn resync_peer(args: &[String], require_enabled: bool) -> Result<(AdminClient, Option<PeerInfo>)> {
    let client = connect(&args[0])?;
    let rt = runtime()?;
    let info = rt
        .block_on(admin_topo::site_replication_info(&client))
        .context("Unable to fetch site replication info.")?;
    if !require_enabled && !info.enabled {
        return Ok((client, None));
    }
    let peer_client = connect(&args[1])?;
    let peer_id = rt
        .block_on(admin_topo::deployment_id(&peer_client))
        .context("Unable to fetch server info of the peer.")?;
    let peer = info
        .sites
        .into_iter()
        .rfind(|site| site.deployment_id == peer_id)
        .filter(|peer| !peer.deployment_id.is_empty());
    match peer {
        Some(peer) => Ok((client, Some(peer))),
        None => Err(invalid_argument(
            "alias provided is not part of cluster replication.",
        )),
    }
}

fn resync_op(args: ReplicateResyncTargetArgs, operation: &str, json: bool) -> Result<()> {
    if args.args.len() != 2 {
        show_help(&["admin", "replicate", "resync", operation]);
    }
    let (client, peer) = resync_peer(&args.args, true)?;
    let peer = peer.unwrap_or_default();
    let message = if operation == "start" {
        "Unable to start replication resync"
    } else {
        "Unable to cancel replication resync"
    };
    let status = runtime()?
        .block_on(admin_topo::site_replication_resync_op(
            &client, &peer, operation,
        ))
        .context(message)?;
    if json {
        return output::print_json(&status);
    }
    let text = if !status.err_detail.is_empty() {
        status.err_detail.clone()
    } else if operation == "start" {
        format!("Site resync started with ID {}", status.resync_id)
    } else {
        format!(
            "Site resync with ID {} canceled successfully.",
            status.resync_id
        )
    };
    print_text(&text);
    Ok(())
}

fn resync_status(args: ReplicateResyncTargetArgs, json: bool) -> Result<()> {
    if args.args.len() != 2 {
        show_help(&["admin", "replicate", "resync", "status"]);
    }
    // mc prints nothing when site replication is not enabled.
    let (client, Some(peer)) = resync_peer(&args.args, false)? else {
        return Ok(());
    };
    if json {
        // mc starts the metrics stream in the background and returns right away with
        // `--json`, so it prints nothing.
        return Ok(());
    }
    // mc renders a terminal UI (bubbletea), which opens /dev/tty when stdin is no terminal.
    if !std::io::stdin().is_terminal()
        && let Err(err) = std::fs::File::open("/dev/tty")
    {
        return Err(anyhow::Error::new(McError::new(format!(
            "could not open a new TTY: open /dev/tty: {}",
            go_errno_text(&err)
        ))))
        .context("Unable to get resync status");
    }
    runtime()?
        .block_on(watch_resync(&client, &peer.deployment_id))
        .context("Unable to get resync status")
}

/// Go's `syscall.Errno` text (Rust capitalizes it and adds ` (os error N)`).
fn go_errno_text(err: &std::io::Error) -> String {
    match err.raw_os_error() {
        Some(6) => "no such device or address".to_string(),
        Some(2) => "no such file or directory".to_string(),
        Some(13) => "permission denied".to_string(),
        _ => {
            let text = err.to_string();
            let text = text.split(" (os error").next().unwrap_or_default();
            let mut chars = text.chars();
            match chars.next() {
                Some(first) => first.to_lowercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    }
}

/// Spinner frames (bubbles `spinner.Points`).
const SPINNER: [&str; 4] = ["∙∙∙", "●∙∙", "∙●∙", "∙∙●"];

/// Streams site resync metrics and redraws mc's resync view until the resync completes or
/// is canceled, the stream ends, or Ctrl-C is pressed.
async fn watch_resync(client: &AdminClient, deployment_id: &str) -> Result<()> {
    let mut stream = admin_topo::site_resync_metrics(client, deployment_id).await?;
    let mut current = SiteResyncMetrics::default();
    let mut frame = 0usize;
    let mut drawn = 0usize;
    let mut quitting = false;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(1000 / 7));
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    loop {
        tokio::select! {
            _ = &mut ctrl_c => quitting = true,
            _ = tick.tick() => frame += 1,
            item = stream.next::<admin_topo::ResyncRealtimeMetrics>() => match item? {
                None => quitting = true,
                Some(metrics) => {
                    if let Some(sr) = metrics.aggregated.site_resync {
                        quitting = sr.complete() || sr.resync_status == "Canceled";
                        current = sr;
                    }
                    quitting |= metrics.is_final;
                }
            },
        }
        let view = resync_view(&current, SPINNER[frame % SPINNER.len()], quitting);
        let mut out = std::io::stdout().lock();
        if drawn > 0 {
            // Back to the start of the previous view, then clear it.
            write!(out, "\x1b[{drawn}A\r\x1b[J")?;
        }
        write!(out, "{view}")?;
        out.flush()?;
        drawn = view.matches('\n').count();
        if quitting {
            return Ok(());
        }
    }
}

/// mc `resyncMetricsUI.View()` without colors (tablewriter: label and value columns padded,
/// each followed by a tab).
fn resync_view(current: &SiteResyncMetrics, spinner: &str, quitting: bool) -> String {
    let mut out = String::new();
    if !quitting {
        out.push_str(spinner);
    } else if current.complete() {
        let cell = if current.failed_count == 0 {
            TICK_CELL
        } else {
            CROSS_TICK_CELL
        };
        out.push_str(&cell.repeat(3));
    }
    out.push('\n');
    if !current.resync_id.is_empty() {
        let elapsed = current.start_time.nanos_until(&current.last_update);
        let mut lines: Vec<(&str, String)> = vec![
            ("ResyncID: ", current.resync_id.clone()),
            ("Status: ", current.resync_status.clone()),
            ("Objects: ", current.replicated_count.to_string()),
            ("Versions: ", current.replicated_count.to_string()),
            ("FailedObjects: ", current.failed_count.to_string()),
        ];
        if elapsed > 0 {
            let secs = elapsed as f64 / 1e9;
            lines.push((
                "Throughput: ",
                format!(
                    "{}/s",
                    admin::ibytes((current.replicated_size as f64 / secs) as u64)
                ),
            ));
            lines.push((
                "IOPs: ",
                format!("{:.2} objs/s", current.replicated_count as f64 / secs),
            ));
        }
        lines.push((
            "Transferred: ",
            admin::ibytes(current.replicated_size as u64),
        ));
        lines.push(("Elapsed: ", go_duration_string(elapsed)));
        lines.push((
            "CurrObjName: ",
            format!("{}/{}", current.bucket, current.object),
        ));
        let label_width = lines.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
        let value_width = lines
            .iter()
            .map(|(_, v)| v.chars().count())
            .max()
            .unwrap_or(0);
        for (label, value) in lines {
            let pad = " ".repeat(value_width - value.chars().count());
            out.push_str(&format!("{label:<label_width$}\t{value}{pad}\t\n"));
        }
    }
    if quitting {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::admin_topo::{
        SrBucketStatsSummary, SrMetric, SrPolicyStatsSummary, SrUserStatsSummary,
    };
    use crate::s3::replication::XferStats;

    fn peer(name: &str, id: &str) -> PeerInfo {
        PeerInfo {
            endpoint: format!("http://{name}:9000"),
            name: name.into(),
            deployment_id: id.into(),
            ..Default::default()
        }
    }

    fn enabled_status() -> SrStatusInfo {
        let mut sites = BTreeMap::new();
        sites.insert("id-2".to_string(), peer("sr2", "id-2"));
        sites.insert("id-1".to_string(), peer("sr1", "id-1"));
        SrStatusInfo {
            enabled: true,
            max_policies: 10,
            max_users: 1,
            sites: Some(sites),
            bucket_stats: Some(BTreeMap::new()),
            ..Default::default()
        }
    }

    fn all_options() -> SrStatusOptions {
        SrStatusOptions {
            buckets: true,
            policies: true,
            users: true,
            groups: true,
            metrics: true,
            ilm_expiry_rules: true,
            ..Default::default()
        }
    }

    #[test]
    fn info_text_matches_mc() {
        let mut second = peer("sr2", "9ad5747f-0335-413e-8659-bda18fca271b");
        second.sync = "enable".into();
        second.default_bandwidth.limit = 2_000_000_000;
        second.replicate_ilm_expiry = true;
        let info = SiteReplicationInfo {
            enabled: true,
            sites: vec![peer("sr1", "72fcc4ed-8354-4f1b-ab7d-b92732817b94"), second],
            ..Default::default()
        };
        let expected = [
            "SiteReplication enabled for:",
            "",
            "Deployment ID                        | Site Name       | Endpoint                                       | Sync | Bandwidth  | ILM Expiry Replication   ",
            "                                     |                 |                                                |      | Per Bucket |                          ",
            "72fcc4ed-8354-4f1b-ab7d-b92732817b94 | sr1             | http://sr1:9000                                |      | N/A        | false                    ",
            "9ad5747f-0335-413e-8659-bda18fca271b | sr2             | http://sr2:9000                                | ✔    | 2.0 GB/s   | true                     ",
        ];
        assert_eq!(info_text(&info), expected.join("\n"));
        assert_eq!(
            info_text(&SiteReplicationInfo::default()),
            "SiteReplication is not enabled"
        );
    }

    #[test]
    fn status_text_default_sections_match_mc() {
        let mut info = enabled_status();
        info.metrics.uptime = 263;
        info.metrics.metrics = Some(BTreeMap::new());
        let expected = [
            "Bucket replication status:",
            "No Buckets present\n",
            "Policy replication status:",
            "●  10/10 Policies in sync\n",
            "User replication status:",
            "●  1/1 Users in sync\n",
            "Group replication status:",
            "No Groups present\n",
            "ILM Expiry Rules replication status:",
            "No ILM Expiry Rules present\n",
            "Object replication status:",
            "Replication status since 4 minutes ",
            "Summary:",
            "Replicated:    0 objects (0 B)",
            "Queued:        ● 0 objects, (0 B) (avg: 0 objects, 0 B; max: 0 objects, 0 B)",
            "Received:      0 objects (0 B)",
        ];
        assert_eq!(status_text(&info, &all_options()), expected.join("\n"));
        assert_eq!(
            status_text(&SrStatusInfo::default(), &all_options()),
            "SiteReplication is not enabled"
        );
    }

    #[test]
    fn status_text_lists_mismatches() {
        let mut info = enabled_status();
        info.max_buckets = 2;
        let mut by_site = BTreeMap::new();
        by_site.insert(
            "id-1".to_string(),
            SrBucketStatsSummary {
                has_bucket: true,
                ..Default::default()
            },
        );
        by_site.insert(
            "id-2".to_string(),
            SrBucketStatsSummary {
                has_bucket: true,
                tag_mismatch: true,
                ..Default::default()
            },
        );
        info.bucket_stats = Some(BTreeMap::from([("b1".to_string(), by_site)]));
        let by_site = BTreeMap::from([(
            "id-1".to_string(),
            SrUserStatsSummary {
                has_user: true,
                ..Default::default()
            },
        )]);
        info.user_stats = Some(BTreeMap::from([("u1".to_string(), by_site)]));
        let opts = SrStatusOptions {
            buckets: true,
            users: true,
            ..Default::default()
        };
        let expected = [
            "Bucket replication status:",
            "●  1/2 Buckets in sync\n",
            "Bucket          | SR1             | SR2            ",
            "b1              | ✔  in-sync      | ✗  in-sync     ",
            "",
            "User replication status:",
            "●  0/1 Users in sync\n",
            "User            | SR1             | SR2            ",
            "u1              | ✔  in-sync      |                ",
            "",
        ];
        assert_eq!(status_text(&info, &opts), expected.join("\n"));
    }

    #[test]
    fn status_text_entity_summaries_match_mc() {
        let mut info = enabled_status();
        let by_site: BTreeMap<String, SrPolicyStatsSummary> = ["id-1", "id-2"]
            .into_iter()
            .map(|id| {
                let stats = SrPolicyStatsSummary {
                    has_policy: true,
                    ..Default::default()
                };
                (id.to_string(), stats)
            })
            .collect();
        info.policy_stats = Some(BTreeMap::from([("readwrite".to_string(), by_site)]));
        let opts = SrStatusOptions {
            entity: SrEntity::Policy,
            entity_value: "readwrite".into(),
            ..Default::default()
        };
        let expected = [
            "●  Policy replication summary for: readwrite\n",
            "Policy          | SR1             | SR2            ",
            "Policy          | ✔               | ✔              ",
        ];
        assert_eq!(status_text(&info, &opts), expected.join("\n"));
        let opts = SrStatusOptions {
            entity: SrEntity::Bucket,
            entity_value: "nope".into(),
            ..Default::default()
        };
        assert_eq!(status_text(&info, &opts), "Bucket nope not found\n");
        let by_site: BTreeMap<String, SrBucketStatsSummary> = ["id-1", "id-2"]
            .into_iter()
            .map(|id| {
                let stats = SrBucketStatsSummary {
                    has_bucket: true,
                    has_replication_cfg: true,
                    ..Default::default()
                };
                (id.to_string(), stats)
            })
            .collect();
        info.bucket_stats = Some(BTreeMap::from([("topobucket".to_string(), by_site)]));
        let opts = SrStatusOptions {
            entity: SrEntity::Bucket,
            entity_value: "topobucket".into(),
            ..Default::default()
        };
        let expected = [
            "●  Bucket config replication summary for: topobucket\n",
            "Bucket          | SR1             | SR2            ",
            "Tags            |                 |                ",
            "Policy          |                 |                ",
            "Quota           |                 |                ",
            "Retention       |                 |                ",
            "Encryption      |                 |                ",
            "Replication     | ✔               | ✔              ",
        ];
        assert_eq!(status_text(&info, &opts), expected.join("\n"));
    }

    #[test]
    fn status_text_single_target_metrics() {
        let mut info = enabled_status();
        info.metrics.uptime = 269;
        let xfer = BTreeMap::from([(
            "Total".to_string(),
            XferStats {
                avg_rate: 0.0,
                peak_rate: 1.1,
                curr_rate: 1.1,
            },
        )]);
        let metric = SrMetric {
            endpoint: "172.18.0.3:9000".into(),
            online: true,
            replicated_size: 3,
            replicated_count: 1,
            latency: admin_topo::LatencyStat {
                curr: 436_523,
                avg: 696_998,
                max: 1_862_281,
            },
            xfer_stats: Some(xfer),
            ..Default::default()
        };
        info.metrics.metrics = Some(BTreeMap::from([("id-2".to_string(), metric)]));
        let opts = SrStatusOptions {
            metrics: true,
            ..Default::default()
        };
        let expected = [
            "Object replication status:",
            "Replication status since 4 minutes ",
            "172.18.0.3:9000",
            "Replicated:    1 objects (3 B)",
            "Received:      0 objects (0 B)",
            "Queued:        ● 0 objects, (0 B) (avg: 0 objects, 0 B; max: 0 objects, 0 B)",
            "Workers:       0 (avg: 0; max 0) ",
            "Transfer Rate: 1 B/s (avg: 0 B/s; max 1 B/s)",
            "Latency:       0s (avg: 1ms; max 2ms)",
            "Link:          ● online (total downtime: 0 milliseconds)",
            "Errors:        0 in last 1 minute; 0 in last 1hr; 0 since uptime",
            "",
        ];
        assert_eq!(status_text(&info, &opts), expected.join("\n"));
    }

    fn status_args() -> ReplicateStatusArgs {
        ReplicateStatusArgs {
            args: vec!["a".into()],
            buckets: false,
            policies: false,
            users: false,
            groups: false,
            ilm_expiry_rules: false,
            all: false,
            bucket: None,
            policy: None,
            user: None,
            group: None,
            ilm_expiry_rule: None,
        }
    }

    #[test]
    fn status_options_follow_mc() {
        assert_eq!(status_options(&status_args()), all_options());
        let mut args = status_args();
        args.users = true;
        let users = status_options(&args);
        assert!(users.users && !users.buckets && !users.metrics);
        args.all = true;
        assert_eq!(status_options(&args), all_options());
        let mut args = status_args();
        args.bucket = Some("b".into());
        let bucket = status_options(&args);
        assert_eq!(bucket.entity, SrEntity::Bucket);
        assert_eq!(bucket.entity_value, "b");
        assert!(!bucket.buckets);
    }

    #[test]
    fn remove_text_matches_mc() {
        let sites = vec!["sr3".to_string()];
        assert_eq!(
            remove_text(REPLICATE_REMOVE_STATUS_SUCCESS, "", &sites, false),
            "Following site(s) [sr3] were removed successfully"
        );
        assert_eq!(
            remove_text("partial", "boom", &sites, false),
            "Following site [sr3] was removed partially, some operations failed:\nERROR: 'boom'"
        );
        let two = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            remove_text("partial", "boom", &two, false),
            "Following site(s) [a b] were removed partially, some operations failed: \nERROR: 'boom'"
        );
        assert_eq!(
            remove_text("x", "", &[], true),
            "All site(s) were removed successfully"
        );
    }

    fn update_args() -> ReplicateUpdateArgs {
        ReplicateUpdateArgs {
            args: vec!["a".into()],
            deployment_id: None,
            endpoint: None,
            mode: None,
            bucket_bandwidth: None,
            disable_ilm_expiry_replication: false,
            enable_ilm_expiry_replication: false,
            sync: None,
        }
    }

    fn update_error(args: &ReplicateUpdateArgs) -> String {
        output::split_error(&update_peer(args).unwrap_err()).0
    }

    #[test]
    fn update_flags_are_validated_like_mc() {
        let mut args = update_args();
        assert_eq!(update_error(&args), "--deployment-id is a required flag");
        args.deployment_id = Some("d".into());
        assert!(update_error(&args).starts_with("--endpoint, --mode"));
        args.mode = Some("foo".into());
        assert_eq!(update_error(&args), "--mode can be either [sync|async]");
        args.sync = Some("enable".into());
        assert_eq!(
            update_error(&args),
            "either --sync or --mode flag should be specified"
        );
        args.sync = None;
        args.mode = Some("SYNC".into());
        args.bucket_bandwidth = Some("2G".into());
        let peer = update_peer(&args).unwrap();
        assert_eq!(peer.sync, "enable");
        assert_eq!(peer.deployment_id, "d");
        assert_eq!(peer.default_bandwidth.limit, 2_000_000_000);
        assert!(peer.default_bandwidth.is_set);
        args.endpoint = Some("http://x/%zz".into());
        assert_eq!(
            update_error(&args),
            r#"Unsupported URL format parse "http://x/%zz": invalid URL escape "%zz""#
        );
        let mut args = update_args();
        args.enable_ilm_expiry_replication = true;
        args.disable_ilm_expiry_replication = true;
        assert!(update_error(&args).starts_with("either --disable-ilm"));
        args.disable_ilm_expiry_replication = false;
        args.deployment_id = Some("d".into());
        assert!(update_error(&args).starts_with("--deployment-id should not be set"));
        args.deployment_id = None;
        let peer = update_peer(&args).unwrap();
        assert_eq!(peer.sync, "");
        assert!(!peer.default_bandwidth.is_set);
    }

    #[test]
    fn duration_rounding_matches_go() {
        assert_eq!(round_millis(436_523), 0);
        assert_eq!(round_millis(696_998), 1_000_000);
        assert_eq!(round_millis(1_500_000), 2_000_000);
        assert_eq!(round_millis(-1_500_000), -2_000_000);
    }

    #[test]
    fn resync_view_matches_mc_layout() {
        let metrics = SiteResyncMetrics {
            resync_status: "Completed".into(),
            resync_id: "r1".into(),
            start_time: admin_topo::GoTime("2026-09-26T19:00:00Z".into()),
            last_update: admin_topo::GoTime("2026-09-26T19:00:02Z".into()),
            replicated_size: 2048,
            replicated_count: 4,
            bucket: "b".into(),
            object: "o".into(),
            ..Default::default()
        };
        let view = resync_view(&metrics, "∙∙∙", true);
        let lines: Vec<&str> = view.lines().collect();
        assert_eq!(lines[0], "✔ ✔ ✔ ");
        assert_eq!(lines[1], "ResyncID:      \tr1         \t");
        assert!(lines.contains(&"Throughput:    \t1.0 KiB/s  \t"));
        assert!(lines.contains(&"IOPs:          \t2.00 objs/s\t"));
        assert!(lines.contains(&"Elapsed:       \t2s         \t"));
        assert!(view.ends_with("\t\n\n"));
        assert_eq!(
            resync_view(&SiteResyncMetrics::default(), "●∙∙", false),
            "●∙∙\n"
        );
    }

    #[test]
    fn go_url_errors() {
        assert!(check_go_url("https://minio2:9000").is_ok());
        assert_eq!(
            check_go_url("%zz").unwrap_err(),
            r#"parse "%zz": invalid URL escape "%zz""#
        );
        assert_eq!(
            check_go_url("a%2").unwrap_err(),
            r#"parse "a%2": invalid URL escape "%2""#
        );
        assert_eq!(
            check_go_url(":x").unwrap_err(),
            r#"parse ":x": missing protocol scheme"#
        );
    }

    #[cfg(target_os = "linux")] // Linux errno numbers and texts
    #[test]
    fn errno_texts_follow_go() {
        let err = std::io::Error::from_raw_os_error(6);
        assert_eq!(go_errno_text(&err), "no such device or address");
        let err = std::io::Error::from_raw_os_error(1);
        assert_eq!(go_errno_text(&err), "operation not permitted");
    }
}
