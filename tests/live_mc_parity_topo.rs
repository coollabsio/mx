//! mc parity for `admin replicate`, `admin decommission` and `admin rebalance`
//! (see tests/common/parity.rs):
//!
//!   MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity_topo
//!
//! These commands change server topology state, so each side gets its own servers from
//! tests/services/topo.sh under the same alias names: `pools` (two pools; mc side
//! MX_TEST_TOPO_POOLS_MC_URL, mx side MX_TEST_TOPO_POOLS_MX_URL) and `site1`/`site2` (fresh
//! single-drive servers; mc side SR1/SR2, mx side SR3/SR4). Both sides run the same command
//! sequence, so they see the same history. Compared: stdout, stderr and exit status after
//! normalizing server-specific values (deployment IDs, endpoint IPs, disk usage, uptime, ids).

mod common;

use common::parity::{Parity, Tool};
use serde_json::Value;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Captured result of one command.
#[derive(Debug)]
struct Run {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

/// Runs `args` with one side's program and HOME (like the parity harness).
fn side_exec(p: &Parity, tool: Tool, args: &[&str]) -> Run {
    let side = p.side(tool);
    let mut command = Command::new(&side.program);
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy().into_owned();
        if key.starts_with("MC_") || key.starts_with("MX_") {
            command.env_remove(key);
        }
    }
    let output = command
        .args(args)
        .current_dir(&side.work)
        .env("HOME", side.home.path())
        .env("TZ", "UTC")
        .env("LANG", "C")
        .env_remove("NO_COLOR")
        .stdin(Stdio::null())
        .output()
        .expect("run command");
    Run {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code(),
    }
}

/// Runs different arguments per side (values that differ per server, e.g. deployment IDs)
/// and compares the normalized results.
fn assert_side_parity(p: &Parity, mc_args: &[&str], mx_args: &[&str]) {
    let mc = side_exec(p, Tool::Mc, mc_args);
    let mx = side_exec(p, Tool::Mx, mx_args);
    let norm = |tool, text: &str| p.normalize(tool, text);
    assert_eq!(
        (
            norm(Tool::Mc, &mc.stdout),
            norm(Tool::Mc, &mc.stderr),
            mc.code
        ),
        (
            norm(Tool::Mx, &mx.stdout),
            norm(Tool::Mx, &mx.stderr),
            mx.code
        ),
        "parity mismatch for `{}` (left: mc, right: mx)",
        mc_args.join(" ")
    );
}

/// Wrong argument counts: both print the command help on stdout and exit with status 1 (the
/// help text itself is clap's, not byte-identical).
fn assert_help(p: &Parity, args: &[&str]) {
    for tool in [Tool::Mc, Tool::Mx] {
        let out = side_exec(p, tool, args);
        assert_eq!(out.code, Some(1), "{tool:?} {args:?}: {out:?}");
        assert!(
            out.stdout.contains("USAGE") || out.stdout.contains("Usage"),
            "{out:?}"
        );
        assert_eq!(out.stderr, "", "{tool:?} {args:?}");
    }
}

/// `alias set` on one side.
fn set_alias(p: &Parity, tool: Tool, alias: &str, url: &str, access: &str, secret: &str) {
    let out = side_exec(p, tool, &["alias", "set", alias, url, access, secret]);
    assert_eq!(out.code, Some(0), "alias set {alias}: {out:?}");
}

fn credentials() -> Option<(String, String)> {
    Some((
        std::env::var("MX_TEST_TOPO_ACCESS_KEY").ok()?,
        std::env::var("MX_TEST_TOPO_SECRET_KEY").ok()?,
    ))
}

/// Polls `check` for up to `secs` seconds.
fn wait_for(secs: u64, what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// JSON output of a successful command on one side.
fn side_json(p: &Parity, tool: Tool, args: &[&str]) -> Value {
    let out = side_exec(p, tool, args);
    serde_json::from_str(out.stdout.trim()).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// rebalance + decommission
// ---------------------------------------------------------------------------

#[test]
fn parity_pools_rebalance_then_decommission() {
    let Some(mut p) = Parity::bare() else { return };
    let (Ok(mc_url), Ok(mx_url), Some((access, secret))) = (
        std::env::var("MX_TEST_TOPO_POOLS_MC_URL"),
        std::env::var("MX_TEST_TOPO_POOLS_MX_URL"),
        credentials(),
    ) else {
        eprintln!("skipping: pools servers (tests/services/topo.sh) not available");
        return;
    };
    set_alias(&p, Tool::Mc, "pools", &mc_url, &access, &secret);
    set_alias(&p, Tool::Mx, "pools", &mx_url, &access, &secret);
    p.normalizer
        .rule(r"\d+(?:\.\d)?% \(total: [0-9.]+ ?[KMGTPE]?i?B\)", "<USAGE>")
        .rule(r"\d+\.\d\d%", "<PCT>")
        // After the standard rules (long float fractions look like request IDs).
        .rule(r#""used":[^,}]+"#, r#""used":0"#)
        .rule(
            r#""(startSize|totalSize|currentSize|(?:objects|bytes)Decommissioned(?:Failed)?)":\d+"#,
            r#""$1":0"#,
        )
        .rule(r#""(ID|id)":"[0-9A-Za-z]{16,}""#, r#""$1":"<RID>""#)
        // A stopped rebalance of (almost) empty pools may or may not have run to completion.
        .rule(
            r#""status":"(?:None|Started|Completed|Stopped|Failed)""#,
            r#""status":"<REBALANCE>""#,
        )
        .rule(r#""(elapsed|eta)":\d+"#, r#""$1":0"#);
    let pool1 = "/data{1...4}";
    let pool2 = "/data{5...8}";

    // rebalance: not started yet, stop (always succeeds), start/stop, status after stop.
    p.assert_parity(&["admin", "rebalance", "status", "pools"], None);
    p.assert_json_parity(&["--json", "admin", "rebalance", "status", "pools"], None);
    p.assert_parity(&["admin", "rebalance", "stop", "pools"], None);
    p.assert_json_parity(&["--json", "admin", "rebalance", "stop", "pools"], None);
    p.assert_parity(&["admin", "rebalance", "start", "pools"], None);
    p.assert_parity(&["admin", "rebalance", "stop", "pools"], None);
    p.assert_json_parity(&["--json", "admin", "rebalance", "start", "pools"], None);
    p.assert_parity(&["admin", "rebalance", "stop", "pools"], None);
    p.assert_parity(&["admin", "rebalance", "status", "pools"], None);
    p.assert_json_parity(&["--json", "admin", "rebalance", "status", "pools"], None);
    p.assert_parity(&["admin", "rebalance", "status", "nosuch"], None);
    assert_help(&p, &["admin", "rebalance", "start"]);

    // decommission: status, errors, then drain the second pool.
    p.assert_parity(&["admin", "decommission", "status", "pools"], None);
    p.assert_json_parity(
        &["--json", "admin", "decommission", "status", "pools"],
        None,
    );
    p.assert_parity(&["admin", "decom", "status", "pools", pool2], None);
    p.assert_json_parity(
        &["--json", "admin", "decom", "status", "pools", pool2],
        None,
    );
    p.assert_parity(&["admin", "decom", "status", "pools", "/nope"], None);
    p.assert_json_parity(
        &["--json", "admin", "decom", "status", "pools", "/nope"],
        None,
    );
    p.assert_parity(&["admin", "decom", "start", "pools", "/nope"], None);
    p.assert_json_parity(
        &["--json", "admin", "decom", "start", "pools", "/nope"],
        None,
    );
    p.assert_parity(&["admin", "decom", "cancel", "pools", pool1], None);
    p.assert_parity(&["admin", "decom", "cancel", "pools"], None);
    p.assert_parity(&["admin", "decom", "status", "nosuch"], None);
    assert_help(&p, &["admin", "decom", "start", "pools"]);
    p.assert_parity(&["admin", "decom", "start", "pools/", pool2], None);
    for tool in [Tool::Mc, Tool::Mx] {
        wait_for(120, "decommission to complete", || {
            let status = side_json(
                &p,
                tool,
                &["--json", "admin", "decom", "status", "pools", pool2],
            );
            status["decommissionInfo"]["complete"] == true
        });
    }
    p.assert_parity(&["admin", "decom", "status", "pools", pool2], None);
    p.assert_json_parity(
        &["--json", "admin", "decom", "status", "pools", pool2],
        None,
    );
    p.assert_parity(&["admin", "decom", "status", "pools"], None);
    p.assert_json_parity(&["--json", "admin", "decom", "status", "pools"], None);
    p.assert_parity(&["admin", "decom", "cancel", "pools"], None);
}

// ---------------------------------------------------------------------------
// site replication
// ---------------------------------------------------------------------------

/// Deployment ID of the server at `url`.
fn deployment_id(url: &str, access: &str, secret: &str) -> String {
    let alias = mx::config::model::AliasConfig {
        url: url.to_string(),
        access_key: access.to_string(),
        secret_key: secret.to_string(),
        api: "S3v4".into(),
        path: "auto".into(),
        ..Default::default()
    };
    let client = mx::s3::admin::AdminClient::new(&alias).expect("admin client");
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(mx::s3::admin_topo::deployment_id(&client))
        .expect("deployment id")
}

/// Removes site replication on both sides when dropped (also on panic).
struct SiteCleanup<'a>(&'a Parity);

impl Drop for SiteCleanup<'_> {
    fn drop(&mut self) {
        for tool in [Tool::Mc, Tool::Mx] {
            // A resync of an empty deployment stays "in progress"; later runs need a fresh one.
            side_exec(
                self.0,
                tool,
                &["admin", "replicate", "resync", "cancel", "site1", "site2"],
            );
            side_exec(
                self.0,
                tool,
                &["admin", "replicate", "rm", "site1", "--all", "--force"],
            );
        }
    }
}

/// Waits until site replication is enabled and every entity is in sync on `tool`'s sites.
fn wait_in_sync(p: &Parity, tool: Tool) {
    wait_for(60, "site replication sync", || {
        let status = side_json(
            p,
            tool,
            &["--json", "admin", "replicate", "status", "site1"],
        );
        let empty = |name: &str| {
            status[name].is_null() || status[name].as_object().is_some_and(|m| m.is_empty())
        };
        status["Enabled"] == true
            && status["MaxPolicies"].as_i64().unwrap_or(0) > 0
            && ["BucketStats", "PolicyStats", "UserStats", "GroupStats"]
                .iter()
                .all(|name| empty(name))
    });
}

#[test]
fn parity_site_replication() {
    let Some(mut p) = Parity::bare() else { return };
    let urls: Option<Vec<String>> = (1..=4)
        .map(|i| std::env::var(format!("MX_TEST_TOPO_SR{i}_URL")).ok())
        .collect();
    let (Some(urls), Some((access, secret))) = (urls, credentials()) else {
        eprintln!("skipping: site replication servers (tests/services/topo.sh) not available");
        return;
    };
    // Go orders deployment-ID keyed maps by ID; pick the mx side's site1/site2 so that their
    // IDs sort like the mc side's and the normalized JSON keeps the same order.
    let ids: Vec<String> = urls
        .iter()
        .map(|url| deployment_id(url, &access, &secret))
        .collect();
    let mc_sites = [(&urls[0], &ids[0]), (&urls[1], &ids[1])];
    let mut mx_sites = [(&urls[2], &ids[2]), (&urls[3], &ids[3])];
    if (ids[0] < ids[1]) != (ids[2] < ids[3]) {
        mx_sites.swap(0, 1);
    }
    for (tool, sites) in [(Tool::Mc, mc_sites), (Tool::Mx, mx_sites)] {
        set_alias(&p, tool, "site1", sites[0].0, &access, &secret);
        set_alias(&p, tool, "site2", sites[1].0, &access, &secret);
    }
    p.normalizer
        .rule(
            r"(?:https?://)?\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}:9000 *",
            "<EP>",
        )
        .rule(
            r"Replication status since [^\n]*",
            "Replication status since <T>",
        )
        .rule(r#""uptime":\d+"#, r#""uptime":0"#);
    let _cleanup = SiteCleanup(&p);
    for tool in [Tool::Mc, Tool::Mx] {
        side_exec(
            &p,
            tool,
            &["admin", "replicate", "rm", "site1", "--all", "--force"],
        );
    }

    // Site replication disabled, argument errors.
    for args in [
        &["admin", "replicate", "info", "site1"][..],
        &["admin", "replicate", "status", "site1"],
        &["admin", "replicate", "status", "site1", "--users"],
        &["admin", "replicate", "info"],
        &["admin", "replicate", "status", "site1", "site2"],
        &[
            "admin",
            "replicate",
            "status",
            "site1",
            "--bucket",
            "b",
            "--users",
        ],
        &[
            "admin",
            "replicate",
            "status",
            "site1",
            "--bucket",
            "b",
            "--user",
            "u",
        ],
        &["admin", "replicate", "add", "site1"],
        &["admin", "replicate", "add", "site1", "nosuch"],
        &["admin", "replicate", "rm", "site1"],
        &["admin", "replicate", "rm", "site1", "site2"],
        &["admin", "replicate", "rm", "site1", "site2", "--all"],
        &["admin", "replicate", "rm", "site1", "--all", "--force"],
        &["admin", "replicate", "update", "site1"],
        &[
            "admin",
            "replicate",
            "update",
            "site1",
            "site2",
            "--mode",
            "sync",
        ],
        &[
            "admin",
            "replicate",
            "update",
            "site1",
            "--deployment-id",
            "x",
        ],
        &[
            "admin",
            "replicate",
            "update",
            "site1",
            "--deployment-id",
            "x",
            "--mode",
            "foo",
        ],
        &[
            "admin",
            "replicate",
            "update",
            "site1",
            "--deployment-id",
            "x",
            "--sync",
            "no",
        ],
        &[
            "admin",
            "replicate",
            "update",
            "site1",
            "--deployment-id",
            "x",
            "--bucket-bandwidth",
            "2X",
        ],
        &[
            "admin",
            "replicate",
            "update",
            "site1",
            "--deployment-id",
            "x",
            "--endpoint",
            "%zz",
        ],
        &[
            "admin",
            "replicate",
            "update",
            "site1",
            "--enable-ilm-expiry-replication",
            "--disable-ilm-expiry-replication",
        ],
        &[
            "admin",
            "replicate",
            "update",
            "site1",
            "--enable-ilm-expiry-replication",
        ],
        &["admin", "replicate", "resync", "status", "site1", "site2"],
        &["admin", "replicate", "resync", "start", "site1", "site2"],
    ] {
        p.assert_parity(args, None);
    }
    for args in [
        &["--json", "admin", "replicate", "info", "site1"][..],
        &["--json", "admin", "replicate", "status", "site1"],
        &["--json", "admin", "replicate", "info"],
        &["--json", "admin", "replicate", "rm", "site1", "site2"],
        &[
            "--json",
            "admin",
            "replicate",
            "update",
            "site1",
            "--deployment-id",
            "x",
            "--mode",
            "foo",
        ],
        &[
            "--json",
            "admin",
            "replicate",
            "resync",
            "start",
            "site1",
            "site2",
        ],
    ] {
        p.assert_json_parity(args, None);
    }

    assert_help(&p, &["admin", "replicate", "resync", "start", "site1"]);
    assert_help(&p, &["admin", "replicate", "update"]);

    // Enable site replication.
    p.assert_parity(&["admin", "replicate", "add", "site1", "site2"], None);
    wait_in_sync(&p, Tool::Mc);
    wait_in_sync(&p, Tool::Mx);
    for args in [
        &["admin", "replicate", "info", "site1"][..],
        &["admin", "replicate", "info", "site2"],
        &["admin", "replicate", "status", "site1"],
        &["admin", "replicate", "status", "site1", "--all"],
        &["admin", "replicate", "status", "site1", "--buckets"],
        &[
            "admin",
            "replicate",
            "status",
            "site1",
            "--policies",
            "--users",
        ],
        &[
            "admin",
            "replicate",
            "status",
            "site1",
            "--groups",
            "--ilm-expiry-rules",
        ],
        &[
            "admin",
            "replicate",
            "status",
            "site1",
            "--policy",
            "readwrite",
        ],
        &["admin", "replicate", "status", "site1", "--policy", "nope"],
        &["admin", "replicate", "status", "site1", "--bucket", "nope"],
        &["admin", "replicate", "status", "site1", "--user", "nope"],
        &["admin", "replicate", "status", "site1", "--group", "nope"],
        &[
            "admin",
            "replicate",
            "status",
            "site1",
            "--ilm-expiry-rule",
            "nope",
        ],
        &["admin", "replicate", "add", "site1", "site2"],
        &["admin", "replicate", "resync", "start", "site1", "site1"],
        &["admin", "replicate", "resync", "start", "site1", "nosuch"],
        &[
            "admin",
            "replicate",
            "update",
            "site1",
            "--deployment-id",
            "nope",
            "--mode",
            "sync",
        ],
    ] {
        p.assert_parity(args, None);
    }
    for args in [
        &["--json", "admin", "replicate", "info", "site1"][..],
        &["--json", "admin", "replicate", "status", "site1"],
        &[
            "--json",
            "admin",
            "replicate",
            "status",
            "site1",
            "--policy",
            "readwrite",
        ],
        &["--json", "admin", "replicate", "add", "site1", "site2"],
        &[
            "--json",
            "admin",
            "replicate",
            "resync",
            "start",
            "site1",
            "site1",
        ],
        &[
            "--json",
            "admin",
            "replicate",
            "resync",
            "status",
            "site1",
            "site2",
        ],
    ] {
        p.assert_json_parity(args, None);
    }

    // update: per-side deployment IDs of site2.
    let update = |json: bool, id: &str| -> Vec<String> {
        let mut args: Vec<String> = if json { vec!["--json".into()] } else { vec![] };
        for arg in [
            "admin",
            "replicate",
            "update",
            "site1",
            "--deployment-id",
            id,
        ] {
            args.push(arg.to_string());
        }
        for arg in ["--mode", "sync", "--bucket-bandwidth", "2G"] {
            args.push(arg.to_string());
        }
        args
    };
    for json in [false, true] {
        let mc_args = update(json, mc_sites[1].1);
        let mx_args = update(json, mx_sites[1].1);
        let mc_args: Vec<&str> = mc_args.iter().map(String::as_str).collect();
        let mx_args: Vec<&str> = mx_args.iter().map(String::as_str).collect();
        assert_side_parity(&p, &mc_args, &mx_args);
    }
    p.assert_parity(&["admin", "replicate", "info", "site1"], None);
    p.assert_parity(
        &[
            "admin",
            "replicate",
            "update",
            "site1",
            "--enable-ilm-expiry-replication",
        ],
        None,
    );
    p.assert_json_parity(
        &[
            "--json",
            "admin",
            "replicate",
            "update",
            "site1",
            "--disable-ilm-expiry-replication",
        ],
        None,
    );
    p.assert_json_parity(&["--json", "admin", "replicate", "info", "site2"], None);

    // resync (an empty deployment finishes right away, so only the start is compared).
    p.assert_parity(
        &["admin", "replicate", "resync", "start", "site1", "site2"],
        None,
    );
    for tool in [Tool::Mc, Tool::Mx] {
        side_exec(
            &p,
            tool,
            &["admin", "replicate", "resync", "cancel", "site1", "site2"],
        );
    }

    // Remove, add again (JSON), remove (JSON).
    p.assert_parity(
        &["admin", "replicate", "rm", "site1", "--all", "--force"],
        None,
    );
    p.assert_parity(&["admin", "replicate", "info", "site2"], None);
    p.assert_json_parity(
        &["--json", "admin", "replicate", "add", "site1", "site2"],
        None,
    );
    wait_in_sync(&p, Tool::Mc);
    wait_in_sync(&p, Tool::Mx);
    p.assert_parity(
        &["admin", "replicate", "rm", "site2", "site1", "--force"],
        None,
    );
    p.assert_json_parity(
        &["--json", "admin", "replicate", "add", "site1", "site2"],
        None,
    );
    wait_in_sync(&p, Tool::Mc);
    wait_in_sync(&p, Tool::Mx);
    p.assert_json_parity(
        &[
            "--json",
            "admin",
            "replicate",
            "rm",
            "site1",
            "--all",
            "--force",
        ],
        None,
    );
    p.assert_json_parity(&["--json", "admin", "replicate", "info", "site1"], None);
}
