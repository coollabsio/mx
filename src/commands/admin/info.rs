//! `mx admin info` (mc `admin info`): per-server summary, pool table and cluster totals, or
//! the full `madmin.InfoMessage` with `--json`.
//!
//! Owner: SERVER.

use crate::commands::runtime;
use crate::s3::admin::ibytes;
use crate::s3::admin_server::{self as api, InfoMessage};
use anyhow::{Result, anyhow};
use clap::Args;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Args)]
pub struct InfoArgs {
    #[arg(value_name = "TARGET")]
    pub target: String,
    #[arg(long = "offline", help = "show only offline nodes/drives")]
    pub offline: bool,
}

/// mc `clusterStruct`.
#[derive(Serialize)]
struct ClusterMessage<'a> {
    status: &'static str,
    #[serde(skip_serializing_if = "str::is_empty")]
    error: &'a str,
    info: &'a InfoMessage,
}

pub fn run(args: InfoArgs, json: bool) -> Result<()> {
    let client = api::admin_client(&args.target, "Unable to initialize admin connection.")?;
    let (info, error) = match runtime()?.block_on(api::server_info(&client)) {
        Ok(info) => (info, String::new()),
        Err(err) => (InfoMessage::default(), error_text(&err)),
    };
    if json {
        // mc prints the error inside the document and exits 0.
        let message = ClusterMessage {
            status: if error.is_empty() { "success" } else { "error" },
            error: &error,
            info: &info,
        };
        println!("{}", json_4(&message)?);
        return Ok(());
    }
    if !error.is_empty() {
        return Err(anyhow!(error).context("Unable to get service info"));
    }
    if info.servers.is_empty() {
        return Err(anyhow!("Unable to get service info").context(""));
    }
    println!("{}", render(info, args.offline));
    Ok(())
}

/// The Go error text (`e.Error()`) of a failed request.
pub(crate) fn error_text(err: &anyhow::Error) -> String {
    match crate::error::mc_error(err) {
        Some(cause) => cause.message.clone(),
        None => err.to_string(),
    }
}

/// mc `json.MarshalIndent(v, "", "    ")`, compacted to one line when stdout is not a
/// terminal (`printMsg`).
pub(crate) fn json_4<T: Serialize>(value: &T) -> Result<String> {
    if !crate::output::stdout_is_terminal() {
        return crate::output::format_json(value, true);
    }
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut serializer)?;
    let text = String::from_utf8(buf)?;
    Ok(text
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026"))
}

/// minio/pkg `console.Table.PopulateTable` (left-aligned cells, no header separator).
pub(crate) fn console_table(rows: &[Vec<String>], indent: usize) -> String {
    let columns = rows.first().map(Vec::len).unwrap_or_default();
    let mut widths = vec![0; columns];
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let pad = " ".repeat(indent);
    let border = |left: &str, mid: &str, right: &str| {
        let segments: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        format!("{pad}{left}{}{right}\n", segments.join(mid))
    };
    let mut out = border("┌", "┬", "┐");
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        out.push_str(&format!("{pad}│ {} │\n", cells.join(" │ ")));
    }
    out.push_str(&border("└", "┴", "┘"));
    out
}

/// mc `poolSummary`.
#[derive(Default)]
struct PoolSummary {
    sets: i64,
    drives_per_set: i64,
    free: u64,
    usable: u64,
    endpoints: BTreeSet<String>,
}

/// mc `clusterSummaryInfo`.
fn cluster_summary(info: &InfoMessage) -> BTreeMap<i64, PoolSummary> {
    let mut summary: BTreeMap<i64, PoolSummary> = BTreeMap::new();
    let drives_per_set = info.backend.drives_per_set.clone().unwrap_or_default();
    for server in &info.servers {
        for disk in &server.disks {
            if disk.pool_index < 0 {
                continue;
            }
            let pool = summary.entry(disk.pool_index).or_default();
            if let Some(per_set) = drives_per_set.get(disk.pool_index as usize)
                && disk.disk_index < per_set - info.backend.standard_sc_parity
            {
                pool.free += disk.available_space;
                pool.usable += disk.total_space;
            }
            pool.endpoints.insert(server.endpoint.clone());
        }
    }
    let total_sets = info.backend.total_sets.clone().unwrap_or_default();
    for (index, sets) in total_sets.iter().enumerate() {
        if let Some(pool) = summary.get_mut(&(index as i64)) {
            pool.sets = *sets;
            pool.drives_per_set = drives_per_set.get(index).copied().unwrap_or_default();
        }
    }
    summary
}

/// `Drives: ON/TOTAL OK ` (mc counts `ok` and `unformatted` drives as online).
fn drives_line(disks: &[api::Disk]) -> String {
    let online = disks
        .iter()
        .filter(|d| d.state == "ok" || d.state == "unformatted")
        .count();
    format!("   Drives: {online}/{} OK \n", disks.len())
}

/// mc `clusterStruct.String()`.
fn render(mut info: InfoMessage, only_offline: bool) -> String {
    let erasure = info.backend.backend_type == "Erasure";
    info.servers.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
    let summary = cluster_summary(&info);
    let mut msg = String::new();
    let mut offline_nodes = 0;
    for server in &info.servers {
        if server.state != "online" {
            offline_nodes += 1;
            msg.push_str(&format!("●  {}\n", server.endpoint));
            msg.push_str(&format!("   Uptime: {}\n", server.state));
            if erasure {
                msg.push_str(&drives_line(&server.disks));
            }
            msg.push('\n');
            continue;
        }
        if only_offline {
            continue;
        }
        msg.push_str(&format!("●  {}\n", server.endpoint));
        msg.push_str(&format!(
            "   Uptime: {}\n",
            api::rel_time(server.uptime, "", "")
        ));
        let version = if server.version.contains("DEVELOPMENT") {
            "<development>"
        } else {
            &server.version
        };
        msg.push_str(&format!("   Version: {version}\n"));
        if !server.network.is_empty() && erasure {
            let alive = server.network.values().filter(|v| *v == "online").count();
            msg.push_str(&format!(
                "   Network: {alive}/{} OK \n",
                server.network.len()
            ));
        }
        if erasure {
            msg.push_str(&drives_line(&server.disks));
            let pools: Vec<String> = summary
                .iter()
                .filter(|(_, pool)| pool.endpoints.contains(&server.endpoint))
                .map(|(index, _)| (index + 1).to_string())
                .collect();
            msg.push_str(&format!("   Pool: {}\n", pools.join(", ")));
        }
        msg.push('\n');
    }
    if erasure {
        let header = [
            "Pool",
            "Drives Usage",
            "Erasure stripe size",
            "Erasure sets",
        ];
        let mut rows = vec![header.map(String::from).to_vec()];
        let mut print_summary = false;
        for index in 0..summary.len() as i64 {
            let Some(pool) = summary.get(&index) else {
                break;
            };
            let capacity = if pool.usable > 0 {
                let used = pool.usable.saturating_sub(pool.free);
                format!(
                    "{:.1}% (total: {})",
                    100.0 * used as f64 / pool.usable as f64,
                    ibytes(pool.usable)
                )
            } else {
                String::new()
            };
            if pool.drives_per_set > 0 {
                print_summary = true;
            }
            rows.push(vec![
                api::ordinal(index + 1),
                capacity,
                pool.drives_per_set.to_string(),
                pool.sets.to_string(),
            ]);
        }
        if print_summary {
            msg.push_str(&console_table(&rows, 0));
            msg.push('\n');
        }
    }
    if info.buckets.count > 0 {
        msg.push_str(&format!(
            "{} Used, {}, {}",
            ibytes(info.usage.size),
            api::plural(info.buckets.count as i64, "Bucket"),
            api::plural(info.objects.count as i64, "Object")
        ));
        if info.versions.count > 0 {
            msg.push_str(&format!(
                ", {}",
                api::plural(info.versions.count as i64, "Version")
            ));
        }
        if info.delete_markers.count > 0 {
            msg.push_str(&format!(
                ", {}",
                api::plural(info.delete_markers.count as i64, "Delete Marker")
            ));
        }
        msg.push('\n');
    }
    if erasure {
        if offline_nodes != 0 {
            msg.push_str(&format!("{} offline, ", api::plural(offline_nodes, "node")));
        }
        msg.push_str(&format!(
            "{} online, {} offline, EC:{}\n",
            api::plural(info.backend.online_disks, "drive"),
            api::plural(info.backend.offline_disks, "drive"),
            info.backend.standard_sc_parity
        ));
    }
    msg.strip_suffix('\n').unwrap_or(&msg).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const POOLS: &str = r#"{"mode":"online","buckets":{"count":2},"objects":{"count":1},"versions":{"count":1},"deletemarkers":{"count":0},"usage":{"size":3},"services":{},"backend":{"backendType":"Erasure","onlineDisks":8,"offlineDisks":0,"standardSCParity":2,"rrSCParity":1,"totalSets":[1,1],"totalDrivesPerSet":[4,4]},"servers":[{"state":"online","endpoint":"127.0.0.1:33040","uptime":6,"version":"2026-08-04T00:00:00Z","network":{"127.0.0.1:33040":"online"},"drives":[
        {"state":"ok","totalspace":441035194368,"availspace":214171586560,"pool_index":0,"set_index":0,"disk_index":0},
        {"state":"ok","totalspace":441035194368,"availspace":214171586560,"pool_index":0,"set_index":0,"disk_index":1},
        {"state":"ok","totalspace":441035194368,"availspace":214171586560,"pool_index":0,"set_index":0,"disk_index":2},
        {"state":"ok","totalspace":441035194368,"availspace":214171586560,"pool_index":0,"set_index":0,"disk_index":3},
        {"state":"ok","totalspace":441035194368,"availspace":214171586560,"pool_index":1,"set_index":0,"disk_index":0},
        {"state":"ok","totalspace":441035194368,"availspace":214171586560,"pool_index":1,"set_index":0,"disk_index":1},
        {"state":"ok","totalspace":441035194368,"availspace":214171586560,"pool_index":1,"set_index":0,"disk_index":2},
        {"state":"ok","totalspace":441035194368,"availspace":214171586560,"pool_index":1,"set_index":0,"disk_index":3}]}]}"#;

    #[test]
    fn renders_pools_like_mc() {
        let info: InfoMessage = serde_json::from_str(POOLS).unwrap();
        assert_eq!(
            render(info, false),
            "●  127.0.0.1:33040\n   Uptime: 6 seconds \n   Version: 2026-08-04T00:00:00Z\n   Network: 1/1 OK \n   Drives: 8/8 OK \n   Pool: 1, 2\n\n\
             ┌──────┬────────────────────────┬─────────────────────┬──────────────┐\n\
             │ Pool │ Drives Usage           │ Erasure stripe size │ Erasure sets │\n\
             │ 1st  │ 51.4% (total: 822 GiB) │ 4                   │ 1            │\n\
             │ 2nd  │ 51.4% (total: 822 GiB) │ 4                   │ 1            │\n\
             └──────┴────────────────────────┴─────────────────────┴──────────────┘\n\n\
             3 B Used, 2 Buckets, 1 Object, 1 Version\n\
             8 drives online, 0 drives offline, EC:2"
        );
    }

    #[test]
    fn offline_servers_and_filter() {
        let info: InfoMessage = serde_json::from_str(
            r#"{"backend":{"backendType":"Erasure","onlineDisks":1,"offlineDisks":1,"standardSCParity":0},"servers":[
            {"state":"offline","endpoint":"b:9000","drives":[{"state":"offline","pool_index":0}]},
            {"state":"online","endpoint":"a:9000","uptime":3700,"version":"DEVELOPMENT.x","drives":[{"state":"ok","pool_index":0}]}]}"#,
        )
        .unwrap();
        assert_eq!(
            render(info.clone(), true),
            "●  b:9000\n   Uptime: offline\n   Drives: 0/1 OK \n\n1 node offline, 1 drive online, 1 drive offline, EC:0"
        );
        assert!(render(info, false).starts_with(
            "●  a:9000\n   Uptime: 1 hour \n   Version: <development>\n   Drives: 1/1 OK \n   Pool: 1\n\n●  b:9000"
        ));
    }

    #[test]
    fn console_table_indents() {
        let rows = vec![vec!["a".to_string(), "bb".to_string()]];
        assert_eq!(
            console_table(&rows, 2),
            "  ┌───┬────┐\n  │ a │ bb │\n  └───┴────┘\n"
        );
    }
}
