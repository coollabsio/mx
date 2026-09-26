//! `mx admin heal` (mc `admin heal`): background heal status (`ALIAS`, `-v`), or a heal
//! sequence on a bucket/prefix (`-r`, `--scan`, `--dry-run`, `--force-start|stop`, ...)
//! followed until it finishes. Non-terminal output is mc's quiet mode (one line per item and
//! a `Healed:` summary); `--json` prints one document per item plus a summary.

use crate::commands::admin::trace::{admin_client, emit, help_exit, interrupted, quiet_pipe};
use crate::error::McError;
use crate::output::{self, Exit};
use crate::s3::admin::{AdminClient, comma, ibytes};
use crate::s3::admin_stream::{
    self as stream, BgHealState, Disk, HealDriveInfo, HealOpts, HealReply, HealResultItem,
    HealTaskStatus, ordinal, parse_time, relative_time,
};
use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;

#[derive(Debug, Args)]
pub struct HealArgs {
    #[arg(value_name = "TARGET")]
    pub target: Vec<String>,
    #[arg(long = "force", help = "avoid showing a warning prompt")]
    pub force: bool,
    #[arg(long = "verbose", short = 'v', help = "show verbose information")]
    pub verbose: bool,
    #[arg(
        long = "all-drives",
        short = 'a',
        help = "select all drives for verbose printing"
    )]
    pub all_drives: bool,
    #[arg(
        long = "pool",
        hide = true,
        value_name = "VALUE",
        help = "heal only the given pool"
    )]
    pub pool: Option<i64>,
    #[arg(
        long = "set",
        hide = true,
        value_name = "VALUE",
        help = "heal only the given set"
    )]
    pub set: Option<i64>,
    #[arg(
        long = "scan",
        hide = true,
        default_value = "normal",
        value_name = "VALUE",
        help = "select the healing scan mode (normal/deep)"
    )]
    pub scan: String,
    #[arg(
        long = "recursive",
        short = 'r',
        hide = true,
        help = "heal recursively"
    )]
    pub recursive: bool,
    #[arg(
        long = "dry-run",
        short = 'n',
        hide = true,
        help = "only inspect data, but do not mutate"
    )]
    pub dry_run: bool,
    #[arg(
        long = "force-start",
        short = 'f',
        hide = true,
        help = "force start a new heal sequence"
    )]
    pub force_start: bool,
    #[arg(
        long = "force-stop",
        short = 's',
        hide = true,
        help = "force stop a running heal sequence"
    )]
    pub force_stop: bool,
    #[arg(
        long = "remove",
        hide = true,
        help = "remove dangling objects in heal sequence"
    )]
    pub remove: bool,
    #[arg(
        long = "storage-class",
        hide = true,
        value_name = "VALUE",
        help = "show server/drives failure tolerance with the given storage class"
    )]
    pub storage_class: Option<String>,
    #[arg(
        long = "rewrite",
        hide = true,
        help = "rewrite objects from older to newer format"
    )]
    pub rewrite: bool,
}

pub fn run(args: HealArgs, json: bool) -> Result<()> {
    if args.target.len() != 1 {
        return help_exit(&["admin", "heal"]);
    }
    let scan = args.scan.to_lowercase();
    if scan != "normal" && scan != "deep" {
        return help_exit(&["admin", "heal"]);
    }
    let aliased_url = args.target[0].replace('\\', "/");
    let client = admin_client(&aliased_url)?;
    let mut splits = aliased_url.splitn(3, '/');
    let _alias = splits.next();
    let bucket = splits.next().unwrap_or_default().to_string();
    let prefix = splits.next().unwrap_or_default().to_string();
    let rt = crate::commands::runtime()?;

    if bucket.is_empty() && !args.recursive {
        let state = rt
            .block_on(stream::background_heal_status(&client))
            .context("Unable to get background heal status.")?;
        let message = BgHealMessage {
            status: "success",
            heal_info: &state,
        };
        let text = if json {
            output::json_string(&message)?
        } else if args.verbose {
            let sc = args.storage_class.as_deref().unwrap_or_default();
            verbose_status(&state, args.all_drives, &sc.to_uppercase())
        } else {
            short_status(&state)
        };
        return quiet_pipe(emit(text.strip_suffix('\n').unwrap_or(&text)));
    }

    let mut opts = HealOpts {
        scan_mode: if scan == "deep" { 2 } else { 1 },
        remove: args.remove,
        recursive: args.recursive,
        dry_run: args.dry_run,
        recreate: args.rewrite,
        ..Default::default()
    };
    if let Some(pool) = args.pool {
        if pool < 1 {
            return Err(anyhow::Error::new(McError::invalid_argument())
                .context("--pool takes a non zero positive number."));
        }
        opts.pool = Some(pool - 1);
    }
    if let Some(set) = args.set {
        if set < 1 {
            return Err(anyhow::Error::new(McError::invalid_argument())
                .context("--set takes a non zero positive number."));
        }
        opts.set = Some(set - 1);
    }
    if args.force_stop {
        rt.block_on(stream::heal(
            &client,
            &bucket,
            &prefix,
            &opts,
            "",
            args.force_start,
            true,
        ))
        .context("Unable to stop healing.")?;
        let text = if json {
            output::json_string(&StopMessage {
                status: "success",
                alias: &aliased_url,
            })?
        } else {
            format!("Heal stopped successfully at `{aliased_url}`.")
        };
        return quiet_pipe(emit(&text));
    }
    if opts.recursive
        && opts.pool.is_none()
        && opts.set.is_none()
        && output::is_terminal()
        && !args.force
    {
        print!(
            "You are about to scan and heal the whole namespace in all pools and sets, please confirm [y/N]: "
        );
        let _ = std::io::stdout().flush();
        let answer = output::read_answer().context("Unable to parse user input.")?;
        if answer != "y" && answer != "yes" {
            println!("Heal aborted!");
            return Ok(());
        }
    }
    let start = rt
        .block_on(stream::heal(
            &client,
            &bucket,
            &prefix,
            &opts,
            "",
            args.force_start,
            false,
        ))
        .context("Unable to start healing.")?;
    let start = match start {
        HealReply::Started(start) if !start.client_token.is_empty() => start,
        // Following needs the sequence token; without it every poll would start a new heal.
        _ => {
            return Err(
                anyhow::Error::new(McError::new("server returned no heal client token"))
                    .context("Unable to start healing."),
            );
        }
    };
    let mut ui = Ui {
        bucket,
        prefix,
        token: start.client_token,
        force_start: args.force_start,
        opts,
        json,
        quiet: output::quiet() || !output::stdout_is_terminal(),
        ..Default::default()
    };
    rt.block_on(ui.follow(&client))
        .context("Unable to display heal status.")
}

#[derive(Serialize)]
struct StopMessage<'a> {
    status: &'static str,
    alias: &'a str,
}

/// mc `shortBackgroundHealStatusMessage` / `verboseBackgroundHealStatusMessage` JSON.
#[derive(Serialize)]
struct BgHealMessage<'a> {
    status: &'static str,
    #[serde(rename = "HealInfo")]
    heal_info: &'a BgHealState,
}

// ---------------------------------------------------------------------------
// background heal status
// ---------------------------------------------------------------------------

fn all_disks(state: &BgHealState) -> Vec<&Disk> {
    state
        .sets
        .iter()
        .flatten()
        .flat_map(|set| set.disks.iter().flatten())
        .collect()
}

#[derive(Debug, Default, Clone, Copy)]
struct SetInfo {
    total: i64,
    ready: i64,
    ready_used: u64,
    incapable: i64,
}

fn sets_status<'a>(disks: impl IntoIterator<Item = &'a Disk>) -> BTreeMap<(i64, i64), SetInfo> {
    let mut m: BTreeMap<(i64, i64), SetInfo> = BTreeMap::new();
    for d in disks {
        let set = m.entry((d.pool_index, d.set_index)).or_default();
        set.total += 1;
        if d.state != "ok" || d.healing {
            set.incapable += 1;
        } else {
            set.ready += 1;
            set.ready_used += d.used_space;
        }
    }
    m
}

/// Host of a drive endpoint URL (`local-pool1st` for local drives).
fn endpoint_host(d: &Disk) -> Option<String> {
    let endpoint = &d.endpoint;
    let host = match endpoint.split_once("://") {
        Some((_, rest)) => rest.split('/').next().unwrap_or_default().to_string(),
        None if endpoint.contains(' ') => return None,
        None => String::new(),
    };
    Some(if host.is_empty() {
        format!("local-pool{}", ordinal(d.pool_index + 1))
    } else {
        host
    })
}

fn drive_path(d: &Disk) -> String {
    if !d.drive_path.is_empty() {
        return d.drive_path.clone();
    }
    match d.endpoint.split_once("://") {
        Some((_, rest)) => rest
            .find('/')
            .map(|i| rest[i..].to_string())
            .unwrap_or_default(),
        None => d.endpoint.clone(),
    }
}

struct Server<'a> {
    pool: i64,
    disks: Vec<&'a Disk>,
}

fn servers_status<'a>(disks: &[&'a Disk]) -> BTreeMap<String, Server<'a>> {
    let mut m: BTreeMap<String, Server> = BTreeMap::new();
    for d in disks {
        let Some(host) = endpoint_host(d) else {
            continue;
        };
        m.entry(host)
            .or_insert_with(|| Server {
                pool: d.pool_index,
                disks: Vec::new(),
            })
            .disks
            .push(d);
    }
    m
}

/// mc `computePoolTolerance`.
fn pool_tolerance(
    pool: i64,
    parity: i64,
    sets: &BTreeMap<(i64, i64), SetInfo>,
    servers: &BTreeMap<String, Server>,
) -> i64 {
    let mut tolerance_per_set = Vec::new();
    for (&(set_pool, set_index), status) in sets {
        if set_pool != pool {
            continue;
        }
        let mut online = status.total - status.incapable;
        let mut tolerance = 0;
        for server in servers.values().filter(|s| s.pool == pool) {
            let in_set: Vec<_> = server
                .disks
                .iter()
                .filter(|d| d.pool_index == set_pool && d.set_index == set_index)
                .collect();
            if in_set.is_empty() {
                continue;
            }
            let count = in_set
                .iter()
                .filter(|d| d.state == "ok" && !d.healing)
                .count() as i64;
            if online - count < status.total - parity {
                break;
            }
            tolerance += 1;
            online -= count;
        }
        tolerance_per_set.push(tolerance);
    }
    tolerance_per_set
        .into_iter()
        .fold(servers.len() as i64, i64::min)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// Healing progress of a drive relative to the set's ready drives (mc `refUsedSpace`).
fn ref_used_space(set: &SetInfo, used: u64) -> u64 {
    let reference = if set.ready > 0 {
        set.ready_used / set.ready as u64
    } else {
        u64::MAX
    };
    reference.max(used)
}

/// mc `verboseBackgroundHealStatusMessage.String()`.
fn verbose_status(state: &BgHealState, all_drives: bool, storage_class: &str) -> String {
    let sc_parity = state.sc_parity.clone().unwrap_or_default();
    let parity = sc_parity.get(storage_class).copied();
    let show_tolerance = parity.is_some();
    let parity = parity.unwrap_or_default();
    let offline: Vec<&String> = state.offline_endpoints.iter().flatten().collect();
    let disks = all_disks(state);
    let mut pools: Vec<i64> = disks.iter().map(|d| d.pool_index).collect();
    pools.sort_unstable();
    pools.dedup();
    let sets = sets_status(disks.iter().copied());
    let servers = servers_status(&disks);
    let tolerance: BTreeMap<i64, i64> = pools
        .iter()
        .map(|&p| (p, pool_tolerance(p, parity, &sets, &servers)))
        .collect();

    let mut msg = String::new();
    let plural = if servers.len() > 1 { "s" } else { "" };
    msg.push_str(&format!("Server{plural} status:\n"));
    msg.push_str("==============\n");
    for &pool in &pools {
        msg.push_str(&format!("Pool {}:\n", ordinal(pool + 1)));
        for (endpoint, server) in servers.iter().filter(|(_, s)| s.pool == pool) {
            if offline.contains(&endpoint) {
                msg.push_str(&format!("  {endpoint}: OFFLINE\n"));
                continue;
            }
            let header = if show_tolerance {
                format!(
                    "  {endpoint}: (Tolerance: {} server(s))\n",
                    tolerance.get(&server.pool).copied().unwrap_or_default()
                )
            } else {
                format!("  {endpoint}:\n")
            };
            let mut printed = false;
            for d in server.disks.iter().filter(|d| d.pool_index == pool) {
                let state_text = match (d.state.as_str(), d.healing) {
                    ("ok", true) => "HEALING".to_string(),
                    ("ok", false) if !all_drives => continue,
                    ("ok", false) => "OK".to_string(),
                    (other, _) => other.to_string(),
                };
                if !printed {
                    printed = true;
                    msg.push_str(&header);
                }
                msg.push_str(&format!("  +  {} : {state_text}\n", drive_path(d)));
                let this_set = sets
                    .get(&(d.pool_index, d.set_index))
                    .copied()
                    .unwrap_or_default();
                if d.healing
                    && let Some(info) = d.heal_info.as_ref().filter(|i| !i.finished)
                {
                    let reference = ref_used_space(&this_set, d.used_space);
                    let percent = if reference == 0 {
                        0
                    } else {
                        (100u128 * d.used_space as u128 / reference as u128) as u64
                    };
                    msg.push_str(&format!("  |__   Progress: {percent}%\n"));
                    let started = parse_time(&info.started).map(|t| t.0).unwrap_or_default();
                    msg.push_str(&format!(
                        "  |__    Started: {}\n",
                        relative_time(started, now_secs())
                    ));
                    if info.retry_attempts > 0 {
                        msg.push_str(&format!("  |__    Retries: {}\n", info.retry_attempts));
                    }
                }
                msg.push_str(&format!(
                    "  |__   Capacity: {}/{}\n",
                    ibytes(d.used_space),
                    ibytes(d.total_space)
                ));
                if show_tolerance {
                    msg.push_str(&format!(
                        "  |__  Tolerance: {} drive(s)\n",
                        parity - this_set.incapable
                    ));
                }
            }
            if printed {
                msg.push('\n');
            }
        }
    }
    if show_tolerance {
        msg.push('\n');
        msg.push_str("Server Failure Tolerance:\n");
        msg.push_str("========================\n");
        for (i, &pool) in pools.iter().enumerate() {
            msg.push_str(&format!("Pool {}:\n", ordinal(i as i64 + 1)));
            msg.push_str(&format!(
                "   Tolerance : {} server(s)\n",
                tolerance.get(&pool).copied().unwrap_or_default()
            ));
            msg.push_str("       Nodes :");
            for (endpoint, _) in servers.iter().filter(|(_, s)| s.pool == pool) {
                msg.push_str(&format!(" {endpoint}"));
            }
            msg.push('\n');
        }
    }
    msg.push('\n');
    msg.push_str("Summary:\n");
    msg.push_str("=======\n");
    msg.push_str(&short_status(state));
    msg.push('\n');
    msg
}

/// go-humanize `CommafWithDigits(v, 1)`.
fn commaf1(value: f64) -> String {
    // Shortest representation, truncated to one decimal.
    let text = format!("{value}");
    let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
    let int: i64 = int.parse().unwrap_or_default();
    match frac.chars().next() {
        Some(digit) => format!("{}.{digit}", comma(int)),
        None => comma(int),
    }
}

/// mc `shortBackgroundHealStatusMessage.String()`.
fn short_status(state: &BgHealState) -> String {
    let (mut items_healed, mut bytes_healed, mut items_failed) = (0u64, 0u64, 0u64);
    let (mut items_per_sec, mut bytes_per_sec) = (0f64, 0f64);
    let mut started_at: Option<(i64, u32)> = None;
    let (mut exceeds_std, mut exceeds_rr) = (0, 0);
    let mut accumulated = 0i128;
    let mut problematic = 0;
    let mut least_pct = 100.0f64;
    let sc_parity = state.sc_parity.clone().unwrap_or_default();
    let sets: Vec<_> = state.sets.iter().flatten().collect();
    for set in &sets {
        let disks: Vec<&Disk> = set.disks.iter().flatten().collect();
        let status = sets_status(disks.iter().copied());
        let mut furthest: Option<&Disk> = None;
        let mut missing = 0;
        for disk in &disks {
            if disk.state != "ok" {
                if disk.state != "unformatted" {
                    missing += 1;
                    problematic += 1;
                }
                continue;
            }
            let Some(info) = disk.heal_info.as_ref().filter(|i| !i.finished) else {
                continue;
            };
            missing += 1;
            let this_set = status
                .get(&(disk.pool_index, disk.set_index))
                .copied()
                .unwrap_or_default();
            let reference = ref_used_space(&this_set, disk.used_space);
            if reference > 0 {
                least_pct = least_pct.min(disk.used_space as f64 / reference as f64);
            } else {
                least_pct = 0.0;
            }
            let progress = |d: &Disk| {
                d.heal_info
                    .as_ref()
                    .map(|i| i.items_healed + i.items_failed)
                    .unwrap_or_default()
            };
            if furthest.is_none_or(|f| info.items_healed + info.items_failed > progress(f)) {
                furthest = Some(disk);
            }
        }
        let Some(info) = furthest.and_then(|d| d.heal_info.as_ref()) else {
            continue;
        };
        items_healed += info.items_healed;
        bytes_healed += info.bytes_done;
        items_failed += info.items_failed;
        if let Some(started) = parse_time(&info.started).filter(|t| t.0 > -62_135_596_800) {
            if started_at.is_none_or(|s| started >= s) {
                started_at = Some(started);
            }
            let last = parse_time(&info.last_update).filter(|t| t.0 > -62_135_596_800);
            let elapsed = last
                .map(|l| {
                    (l.0 as i128 - started.0 as i128) * 1_000_000_000 + l.1 as i128
                        - started.1 as i128
                })
                .unwrap_or_default();
            if last.is_some() {
                accumulated += elapsed;
            }
            if elapsed != 0 {
                bytes_per_sec += 1e9 * info.bytes_done as f64 / elapsed as f64;
                items_per_sec +=
                    1e9 * (info.items_healed + info.items_failed) as f64 / elapsed as f64;
            }
        }
        if sc_parity.get("STANDARD").is_some_and(|n| missing > *n) {
            exceeds_std += 1;
        }
        if sc_parity
            .get("REDUCED_REDUNDANCY")
            .is_some_and(|n| missing > *n)
        {
            exceeds_rr += 1;
        }
    }
    if started_at.is_none() && items_healed == 0 {
        let mut msg = "No active healing is detected for new disks".to_string();
        if problematic > 0 {
            msg.push_str(&format!(", though {problematic} offline disk(s) found."));
        } else {
            msg.push('.');
        }
        return msg;
    }
    let mut msg = format!(
        "Objects Healed: {}, {} ({}%)\n",
        comma(items_healed as i64),
        ibytes(bytes_healed),
        commaf1(least_pct * 100.0)
    );
    msg.push_str(&format!("Objects Failed: {}\n", comma(items_failed as i64)));
    if accumulated > 0 {
        msg.push_str(&format!(
            "Heal rate: {} obj/s, {}/s\n",
            items_per_sec as i64,
            ibytes(bytes_per_sec as u64)
        ));
    }
    if problematic > 0 {
        msg.push_str(&format!("\n{problematic} offline disk(s) found."));
    }
    if exceeds_std > 0 {
        msg.push_str(&format!(
            "\n{exceeds_std} of {} sets exceeds standard parity count EC:{} lost/offline disks",
            sets.len(),
            sc_parity.get("STANDARD").copied().unwrap_or_default()
        ));
    }
    if exceeds_rr > 0 {
        msg.push_str(&format!(
            "\n{exceeds_rr} of {} sets exceeds reduced parity count EC:{} lost/offline disks",
            sets.len(),
            sc_parity
                .get("REDUCED_REDUNDANCY")
                .copied()
                .unwrap_or_default()
        ));
    }
    msg
}

// ---------------------------------------------------------------------------
// heal sequence
// ---------------------------------------------------------------------------

/// mc health color codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Col {
    Grey,
    Red,
    Yellow,
    Green,
}

impl Col {
    fn name(self) -> &'static str {
        match self {
            Col::Grey => "Grey",
            Col::Red => "Red",
            Col::Yellow => "Yellow",
            Col::Green => "Green",
        }
    }
}

/// mc `getHColCode`.
fn hcol_code(surplus: i64, parity: i64) -> std::result::Result<Col, String> {
    if !(1..=8).contains(&parity) || surplus > parity {
        return Err("Invalid parity shard count/surplus shard count given".into());
    }
    if surplus < 0 {
        return Ok(Col::Grey);
    }
    let row: [i64; 3] = match parity {
        1 => [0, -1, 1],
        2 => [0, 1, 2],
        3 => [1, 2, 3],
        4 => [1, 2, 4],
        5 => [1, 3, 5],
        6 => [2, 4, 6],
        7 => [2, 4, 7],
        _ => [2, 5, 8],
    };
    let order = [Col::Red, Col::Yellow, Col::Green];
    for (index, val) in row.iter().enumerate() {
        if *val != -1 && surplus <= *val {
            return Ok(order[index]);
        }
    }
    Err("cannot get a heal color code".into())
}

type ColChange = std::result::Result<(Col, Col), String>;

fn object_cols(h: &HealResultItem) -> ColChange {
    let (before, after) = (h.before.count("ok") as i64, h.after.count("ok") as i64);
    let b = hcol_code(before - h.data_blocks, h.parity_blocks).map_err(|e| {
        format!(
            "{e}: surplusShardsBeforeHeal: {}, parityShards: {}",
            before - h.data_blocks,
            h.parity_blocks
        )
    })?;
    let a = hcol_code(after - h.data_blocks, h.parity_blocks).map_err(|e| {
        format!(
            "{e}: surplusShardsAfterHeal: {}, parityShards: {}",
            after - h.data_blocks,
            h.parity_blocks
        )
    })?;
    Ok((b, a))
}

fn bucket_cols(h: &HealResultItem) -> ColChange {
    let code = |drives: &[HealDriveInfo]| {
        if drives.is_empty() {
            return Col::Grey;
        }
        let missing = drives.iter().filter(|d| d.state == "missing").count();
        let unavailable = drives
            .iter()
            .filter(|d| d.state != "ok" && d.state != "missing")
            .count();
        if unavailable > 0 {
            Col::Red
        } else if missing > 0 {
            Col::Yellow
        } else {
            Col::Green
        }
    };
    Ok((code(h.before.list()), code(h.after.list())))
}

fn replicated_cols(h: &HealResultItem) -> ColChange {
    let code = |available: i64| {
        let (quorum, surplus, parity) = if h.set_count > 0 {
            let quorum = h.disk_count / h.set_count / 2 + 1;
            (
                quorum,
                available / h.set_count - quorum,
                h.disk_count / h.set_count - quorum,
            )
        } else {
            let quorum = h.disk_count / 2 + 1;
            (quorum, available - quorum, h.disk_count - quorum)
        };
        let _ = quorum;
        hcol_code(surplus, parity)
    };
    Ok((
        code(h.before.count("ok") as i64)?,
        code(h.after.count("ok") as i64)?,
    ))
}

fn item_cols(h: &HealResultItem) -> ColChange {
    match h.item_type.as_str() {
        "bucket" => bucket_cols(h),
        "metadata" | "bucket-metadata" => replicated_cols(h),
        _ => object_cols(h),
    }
}

/// mc `getHRTypeAndName`.
fn type_and_name(h: &HealResultItem) -> (String, String) {
    let name = format!("{}/{}", h.bucket, h.object);
    match h.item_type.as_str() {
        "metadata" => ("system".into(), h.detail.clone()),
        "bucket-metadata" => ("system".into(), format!("bucket-metadata:{name}")),
        "bucket" => ("bucket".into(), name),
        "object" => ("object".into(), name),
        other => {
            let typ = format!("!! Unknown heal result record {other} !!");
            (typ.clone(), typ)
        }
    }
}

/// mc `getHealResultStr`.
fn result_str(h: &HealResultItem) -> String {
    let (typ, name) = type_and_name(h);
    match h.item_type.as_str() {
        "metadata" | "bucket-metadata" => format!("{typ}:{name}"),
        _ => name,
    }
}

/// mc `makeHealEntityString`.
fn entity_str(h: &HealResultItem) -> String {
    match h.item_type.as_str() {
        "object" => format!("{}/{}", h.bucket, h.object),
        "bucket" => h.bucket.clone(),
        "metadata" => "[disk-format]".into(),
        "bucket-metadata" => format!("[bucket-metadata]{}/{}", h.bucket, h.object),
        _ => "** unexpected **".into(),
    }
}

#[derive(Serialize)]
struct HealSide<'a> {
    color: String,
    offline: usize,
    online: usize,
    missing: usize,
    corrupted: usize,
    drives: &'a Option<Vec<HealDriveInfo>>,
}

/// mc `healRec` (`--json` per item).
#[derive(Serialize)]
struct HealRec<'a> {
    status: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    error: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    detail: String,
    #[serde(rename = "type")]
    item_type: String,
    name: String,
    before: HealSide<'a>,
    after: HealSide<'a>,
    size: i64,
}

fn heal_side(drives: &crate::s3::admin_stream::HealDrives, color: String) -> HealSide<'_> {
    HealSide {
        color,
        offline: drives.count("offline"),
        online: drives.count("ok"),
        missing: drives.count("missing"),
        corrupted: drives.count("corrupt"),
        drives: &drives.drives,
    }
}

fn heal_rec(h: &HealResultItem) -> HealRec<'_> {
    let (item_type, name) = type_and_name(h);
    let (cols, error) = match item_cols(h) {
        Ok(cols) => (cols, String::new()),
        Err(err) => ((Col::Grey, Col::Grey), err),
    };
    let color = |col: Col| {
        if error.is_empty() {
            col.name().to_lowercase()
        } else {
            String::new()
        }
    };
    let (before, after) = (
        heal_side(&h.before, color(cols.0)),
        heal_side(&h.after, color(cols.1)),
    );
    HealRec {
        status: "success",
        detail: h.detail.clone(),
        item_type,
        name,
        before,
        after,
        size: if h.item_type == "object" {
            h.object_size
        } else {
            0
        },
        error,
    }
}

#[derive(Serialize)]
struct HealSummary {
    status: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    objects_scanned: i64,
    objects_healed: i64,
    items_scanned: i64,
    items_healed: i64,
    size: i64,
    duration: i64,
}

/// mc `uiData`.
#[derive(Default)]
struct Ui {
    bucket: String,
    prefix: String,
    token: String,
    force_start: bool,
    opts: HealOpts,
    json: bool,
    quiet: bool,
    last_item: Option<HealResultItem>,
    /// Nanoseconds since the sequence started.
    duration: i64,
    bytes_scanned: i64,
    objects_scanned: i64,
    items_scanned: i64,
    objects_healed: i64,
    items_healed: i64,
    health_cols: BTreeMap<Col, i64>,
    cursor: usize,
    drawn: bool,
}

impl Ui {
    fn update_stats(&mut self, item: &HealResultItem) {
        if item.item_type == "object" {
            if item.object_size >= 0 {
                self.bytes_scanned += item.object_size;
            }
            self.objects_scanned += 1;
        }
        self.items_scanned += 1;
        if item.after.count("ok") > item.before.count("ok") {
            if item.item_type == "object" {
                self.objects_healed += 1;
            }
            self.items_healed += 1;
        }
        if let Ok((_, after)) = item_cols(item) {
            *self.health_cols.entry(after).or_default() += 1;
        }
    }

    /// mc `getProgress`: (objects, size, duration).
    fn progress(&self) -> (String, String, String) {
        let bytes = self.bytes_scanned as f64;
        let magnitudes = [1u64 << 10, 1 << 20, 1 << 30, 1 << 40, 1 << 50, 1 << 60];
        let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
        let i = magnitudes
            .iter()
            .position(|m| bytes <= *m as f64)
            .unwrap_or(magnitudes.len() - 1);
        let num = (bytes * 1024.0 / magnitudes[i] as f64) as i64;
        let duration = stream::go_duration(stream::round_duration(self.duration, 1_000_000_000));
        (
            comma(self.objects_scanned),
            format!("{num} {}", units[i]),
            duration,
        )
    }

    fn print_items_quietly(&self, status: &HealTaskStatus) -> Result<()> {
        for item in &status.items {
            let text = result_str(item);
            let line = match item_cols(item) {
                Err(err) => {
                    let err = if item.detail.is_empty() {
                        err
                    } else {
                        item.detail.clone()
                    };
                    format!("[ERROR] ** {text} **: {err}")
                }
                Ok((b, a)) => {
                    let cols = format!("[{:<6} -> {:>6}] ", b.name(), a.name());
                    match item.item_type.as_str() {
                        "metadata" | "bucket-metadata" => format!("{cols}** {text} **"),
                        _ => format!("{cols}{text}"),
                    }
                }
            };
            emit(&line)?;
        }
        Ok(())
    }

    fn print_items_json(&self, status: &HealTaskStatus) -> Result<()> {
        for item in &status.items {
            emit(&output::json_string(&heal_rec(item))?)?;
        }
        Ok(())
    }

    fn summary(&self) -> HealSummary {
        HealSummary {
            status: "success",
            kind: "summary",
            objects_scanned: self.objects_scanned,
            objects_healed: self.objects_healed,
            items_scanned: self.items_scanned,
            items_healed: self.items_healed,
            size: self.bytes_scanned,
            duration: stream::round_duration(self.duration, 1_000_000_000) / 1_000_000_000,
        }
    }

    /// mc `updateUI` (terminal): spinner, last item, totals and the health color table.
    fn update_ui(&mut self, status: &HealTaskStatus) -> Result<()> {
        if let Some(item) = status.items.last() {
            self.last_item = Some(item.clone());
        }
        let scanned = match &self.last_item {
            Some(item) => line_trunc(&entity_str(item), 80 - "Scanned: ".len()),
            None => "** waiting for status from server **".to_string(),
        };
        let (objects, size, duration) = self.progress();
        let cursors = ["◐", "◓", "◑", "◒"];
        let cursor = cursors[self.cursor % cursors.len()];
        self.cursor += 1;
        let mut out = String::new();
        if self.drawn {
            out.push_str(&"\x1b[1A\x1b[2K".repeat(8));
        }
        self.drawn = true;
        out.push_str(&format!(" {cursor}  {scanned}\n"));
        out.push_str(&format!(
            "    {}/{objects} objects; {size} in {duration}\n",
            comma(self.objects_healed)
        ));
        let rows: Vec<[String; 3]> = [Col::Green, Col::Yellow, Col::Red, Col::Grey]
            .iter()
            .map(|col| {
                let count = self.health_cols.get(col).copied().unwrap_or_default();
                let (pct, filled) = if self.items_scanned == 0 {
                    (0.0, 0)
                } else {
                    (
                        count as f64 * 100.0 / self.items_scanned as f64,
                        (12.0 * count as f64 / self.items_scanned as f64).ceil() as usize,
                    )
                };
                let bar = format!("{}{}", "█".repeat(filled), " ".repeat(12 - filled.min(12)));
                [
                    col.name().to_string(),
                    comma(count),
                    format!("{pct:5.1}% {bar}"),
                ]
            })
            .collect();
        let widths: Vec<usize> = (0..3)
            .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
            .collect();
        let border = |l: &str, m: &str, r: &str| {
            let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            format!("    {l}{}{r}\n", parts.join(m))
        };
        out.push_str(&border("┌", "┬", "┐"));
        for row in &rows {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(c, cell)| {
                    let pad = " ".repeat(widths[c] - cell.chars().count());
                    if c == 0 {
                        format!(" {cell}{pad} ")
                    } else {
                        format!(" {pad}{cell} ")
                    }
                })
                .collect();
            out.push_str(&format!("    │{}│\n", cells.join("│")));
        }
        out.push_str(&border("└", "┴", "┘"));
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(out.as_bytes())?;
        stdout.flush()?;
        Ok(())
    }

    fn update_display(&mut self, status: &HealTaskStatus) -> Result<()> {
        if let Some((secs, nanos)) = parse_time(&status.start_time) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            // Go `Time.Sub` saturates.
            self.duration = (now.as_secs() as i64 - secs)
                .saturating_mul(1_000_000_000)
                .saturating_add(now.subsec_nanos() as i64 - nanos as i64);
        }
        for item in &status.items {
            self.update_stats(item);
        }
        if self.json {
            self.print_items_json(status)
        } else if self.quiet {
            self.print_items_quietly(status)
        } else {
            self.update_ui(status)
        }
    }

    /// mc `DisplayAndFollowHealStatus`: polls the sequence status every second.
    async fn follow(&mut self, client: &AdminClient) -> Result<()> {
        let signal = interrupted();
        tokio::pin!(signal);
        loop {
            let reply = tokio::select! {
                code = &mut signal => return Err(Exit(code).into()),
                reply = stream::heal(client, &self.bucket, &self.prefix, &self.opts, &self.token, self.force_start, false) => reply?,
            };
            let HealReply::Status(status) = reply else {
                return Err(McError::new("unexpected heal start reply to a status request").into());
            };
            quiet_pipe(self.update_display(&status))?;
            if status.summary == "finished" {
                if self.json {
                    quiet_pipe(emit(&output::json_string(&self.summary())?))?;
                } else if self.quiet {
                    let (objects, size, duration) = self.progress();
                    quiet_pipe(emit(&format!(
                        "Healed:\t{}/{objects} objects; {size} in {duration}",
                        comma(self.objects_healed)
                    )))?;
                }
                return Ok(());
            }
            if status.summary == "stopped" {
                return Err(
                    McError::new(format!("Heal had an error - {}", status.failure_detail)).into(),
                );
            }
            tokio::select! {
                code = &mut signal => return Err(Exit(code).into()),
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
            }
        }
    }
}

/// mc `lineTrunc`.
fn line_trunc(content: &str, max: usize) -> String {
    let runes: Vec<char> = content.chars().collect();
    if runes.len() <= max {
        return content.to_string();
    }
    let half = max / 2;
    let first: String = runes[..half].iter().collect();
    let second: String = runes[runes.len() - half..].iter().collect();
    format!("{first}…{second}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::admin_stream::{HealDrives, HealingDisk, SetStatus};

    fn drives(states: &[&str]) -> HealDrives {
        HealDrives {
            drives: Some(
                states
                    .iter()
                    .enumerate()
                    .map(|(i, s)| HealDriveInfo {
                        uuid: String::new(),
                        endpoint: format!("/data{}", i + 1),
                        state: s.to_string(),
                    })
                    .collect(),
            ),
        }
    }

    fn item(kind: &str, before: &[&str], after: &[&str]) -> HealResultItem {
        HealResultItem {
            item_type: kind.into(),
            bucket: "bucket1".into(),
            object: if kind == "object" {
                "dir/x1".into()
            } else {
                String::new()
            },
            parity_blocks: 2,
            data_blocks: 2,
            disk_count: 4,
            set_count: 1,
            before: drives(before),
            after: drives(after),
            object_size: 4,
            ..Default::default()
        }
    }

    #[test]
    fn color_codes_follow_mc_table() {
        assert_eq!(hcol_code(2, 2), Ok(Col::Green));
        assert_eq!(hcol_code(1, 2), Ok(Col::Yellow));
        assert_eq!(hcol_code(0, 2), Ok(Col::Red));
        assert_eq!(hcol_code(-1, 2), Ok(Col::Grey));
        assert!(hcol_code(3, 2).is_err());
        let ok = ["ok"; 4];
        assert_eq!(
            item_cols(&item("object", &ok, &ok)),
            Ok((Col::Green, Col::Green))
        );
        assert_eq!(
            item_cols(&item("bucket", &["ok", "missing", "ok", "ok"], &ok)),
            Ok((Col::Yellow, Col::Green))
        );
        assert_eq!(
            item_cols(&item("bucket-metadata", &ok, &ok)),
            Ok((Col::Green, Col::Green))
        );
    }

    #[test]
    fn quiet_lines_and_json_records() {
        let ok = ["ok"; 4];
        let bucket = item("bucket", &ok, &ok);
        assert_eq!(result_str(&bucket), "bucket1/");
        let mut meta = item("bucket-metadata", &ok, &ok);
        meta.bucket = ".minio.sys".into();
        meta.object = "config/config.json".into();
        assert_eq!(
            result_str(&meta),
            "system:bucket-metadata:.minio.sys/config/config.json"
        );
        let rec = serde_json::to_string(&heal_rec(&item("object", &ok, &ok))).unwrap();
        assert_eq!(
            rec,
            r#"{"status":"success","type":"object","name":"bucket1/dir/x1","before":{"color":"green","offline":0,"online":4,"missing":0,"corrupted":0,"drives":[{"uuid":"","endpoint":"/data1","state":"ok"},{"uuid":"","endpoint":"/data2","state":"ok"},{"uuid":"","endpoint":"/data3","state":"ok"},{"uuid":"","endpoint":"/data4","state":"ok"}]},"after":{"color":"green","offline":0,"online":4,"missing":0,"corrupted":0,"drives":[{"uuid":"","endpoint":"/data1","state":"ok"},{"uuid":"","endpoint":"/data2","state":"ok"},{"uuid":"","endpoint":"/data3","state":"ok"},{"uuid":"","endpoint":"/data4","state":"ok"}]},"size":4}"#
        );
    }

    #[test]
    fn progress_matches_mc_units() {
        let mut ui = Ui {
            bytes_scanned: 12,
            objects_scanned: 3,
            duration: 1_200_000_000,
            ..Default::default()
        };
        assert_eq!(
            ui.progress(),
            ("3".to_string(), "12 B".to_string(), "1s".to_string())
        );
        ui.bytes_scanned = 5 << 20;
        assert_eq!(ui.progress().1, "5 MiB");
    }

    fn state(heal: Option<HealingDisk>, disk_state: &str) -> BgHealState {
        let disks = (0..4)
            .map(|i| Disk {
                endpoint: format!("/data{}", i + 1),
                drive_path: format!("/data{}", i + 1),
                state: if i == 3 {
                    disk_state.into()
                } else {
                    "ok".into()
                },
                healing: i == 3 && heal.is_some(),
                heal_info: if i == 3 { heal.clone() } else { None },
                total_space: 100 << 30,
                used_space: 50 << 30,
                disk_index: i,
                ..Default::default()
            })
            .collect();
        let mut parity = BTreeMap::new();
        parity.insert("STANDARD".to_string(), 2);
        parity.insert("REDUCED_REDUNDANCY".to_string(), 1);
        BgHealState {
            sets: Some(vec![SetStatus {
                id: "0-0".into(),
                disks: Some(disks),
                ..Default::default()
            }]),
            sc_parity: Some(parity),
            ..Default::default()
        }
    }

    #[test]
    fn background_status_texts() {
        let idle = state(None, "ok");
        assert_eq!(
            short_status(&idle),
            "No active healing is detected for new disks."
        );
        assert_eq!(
            verbose_status(&idle, false, ""),
            "Server status:\n==============\nPool 1st:\n\nSummary:\n=======\nNo active healing is detected for new disks.\n"
        );
        let all = verbose_status(&idle, true, "STANDARD");
        assert!(
            all.contains("  local-pool1st: (Tolerance: 0 server(s))\n  +  /data1 : OK\n  |__   Capacity: 50 GiB/100 GiB\n  |__  Tolerance: 2 drive(s)\n"),
            "{all}"
        );
        assert!(all.contains("Server Failure Tolerance:\n"), "{all}");
        let offline = state(None, "offline");
        assert_eq!(
            short_status(&offline),
            "No active healing is detected for new disks, though 1 offline disk(s) found."
        );
        let healing = state(
            Some(HealingDisk {
                items_healed: 1234,
                bytes_done: 2048,
                started: "2026-01-01T00:00:00Z".into(),
                last_update: "2026-01-01T00:00:10Z".into(),
                ..Default::default()
            }),
            "ok",
        );
        assert_eq!(
            short_status(&healing),
            "Objects Healed: 1,234, 2.0 KiB (100%)\nObjects Failed: 0\nHeal rate: 123 obj/s, 204 B/s\n"
        );
    }
}
