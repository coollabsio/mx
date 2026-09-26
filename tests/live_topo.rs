//! Live tests for `admin replicate` (site replication), `admin decommission` and
//! `admin rebalance` (MX_LIVE_TESTS=1). Servers come from tests/services/topo.sh (site
//! replication: MX_TEST_TOPO_SR5_URL, MX_TEST_TOPO_SR6_URL) and tests/services/minio_pools.sh
//! (two pools: MX_TEST_POOLS_*); the tests skip when those are not available.

mod common;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::time::{Duration, Instant};

/// Temporary HOME with aliases passed as `MC_HOST_<alias>` variables.
struct Fixture {
    home: tempfile::TempDir,
    hosts: Vec<(String, String)>,
}

impl Fixture {
    fn new(aliases: &[(&str, &str)], access_key: &str, secret_key: &str) -> Self {
        let hosts = aliases
            .iter()
            .map(|(alias, url)| {
                let (scheme, rest) = url.split_once("://").expect("URL with scheme");
                (
                    format!("MC_HOST_{alias}"),
                    format!("{scheme}://{access_key}:{secret_key}@{rest}"),
                )
            })
            .collect();
        Self {
            home: common::live::temp_home(),
            hosts,
        }
    }

    fn cmd(&self) -> Command {
        let mut cmd = common::live::mx();
        cmd.env("HOME", self.home.path())
            .env_remove("MC_JSON")
            .envs(self.hosts.iter().map(|(k, v)| (k, v)));
        cmd
    }

    /// Runs `args`, asserts success and returns stdout.
    fn ok(&self, args: &[&str]) -> String {
        let output = self.cmd().args(args).output().expect("run mx");
        assert!(
            output.status.success(),
            "mx {} failed: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf-8")
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        serde_json::from_str(self.ok(&full).trim()).expect("JSON output")
    }

    /// Best effort (cleanup).
    fn run_quietly(&self, args: &[&str]) {
        let _ = self.cmd().args(args).output();
    }
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

// ---------------------------------------------------------------------------
// site replication
// ---------------------------------------------------------------------------

/// Removes site replication and the test bucket from every site, even on panic.
struct SiteCleanup<'a> {
    fixture: &'a Fixture,
    bucket: String,
}

impl Drop for SiteCleanup<'_> {
    fn drop(&mut self) {
        self.fixture
            .run_quietly(&["admin", "replicate", "resync", "cancel", "site1", "site2"]);
        self.fixture
            .run_quietly(&["admin", "replicate", "rm", "site1", "--all", "--force"]);
        for site in ["site1", "site2"] {
            self.fixture
                .run_quietly(&["rb", "--force", &format!("{site}/{}", self.bucket)]);
        }
    }
}

#[test]
fn live_site_replication_lifecycle() {
    if !common::live::enabled() {
        eprintln!("skipping live test; set MX_LIVE_TESTS=1");
        return;
    }
    let urls: Option<Vec<String>> = (5..=6)
        .map(|i| std::env::var(format!("MX_TEST_TOPO_SR{i}_URL")).ok())
        .collect();
    let (Some(urls), Some((access, secret))) = (urls, credentials()) else {
        eprintln!("skipping: site replication servers (tests/services/topo.sh) not available");
        return;
    };
    let fixture = Fixture::new(
        &[("site1", &urls[0]), ("site2", &urls[1])],
        &access,
        &secret,
    );
    let bucket = common::live::unique_bucket_name();
    let _cleanup = SiteCleanup {
        fixture: &fixture,
        bucket: bucket.clone(),
    };
    fixture.run_quietly(&["admin", "replicate", "rm", "site1", "--all", "--force"]);

    fixture
        .cmd()
        .args(["admin", "replicate", "info", "site1"])
        .assert()
        .success()
        .stdout("SiteReplication is not enabled\n");
    assert_eq!(
        fixture.json(&["admin", "replicate", "info", "site1"]),
        serde_json::json!({"enabled": false})
    );

    fixture.ok(&["mb", &format!("site1/{bucket}")]);
    fixture
        .cmd()
        .args(["admin", "replicate", "add", "site1", "site2"])
        .assert()
        .success()
        .stdout("Requested sites were configured for replication successfully.\n");
    // Adding again fails: the site is already configured.
    fixture
        .cmd()
        .args(["admin", "replicate", "add", "site1", "site2"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to add sites for replication. Invalid site-replication request",
        ));

    let info = fixture.json(&["admin", "replicate", "info", "site1"]);
    assert_eq!(info["enabled"], true);
    assert_eq!(info["name"], "site1");
    let sites = info["sites"].as_array().expect("sites");
    let names: Vec<&str> = sites.iter().filter_map(|s| s["name"].as_str()).collect();
    assert_eq!(names.len(), 2, "{info}");
    assert!(
        names.contains(&"site1") && names.contains(&"site2"),
        "{info}"
    );
    let site2_id = sites
        .iter()
        .find(|s| s["name"] == "site2")
        .and_then(|s| s["deploymentID"].as_str())
        .expect("site2 deployment id")
        .to_string();
    let text = fixture.ok(&["admin", "replicate", "info", "site1"]);
    assert!(
        text.starts_with("SiteReplication enabled for:\n\nDeployment ID "),
        "{text}"
    );
    assert!(text.contains(&format!("{site2_id} | site2 ")), "{text}");

    // The bucket reaches site2.
    wait_for(60, "bucket replication", || {
        let status = fixture.json(&["admin", "replicate", "status", "site1", "--buckets"]);
        status["MaxBuckets"] == 1
            && status["BucketStats"]
                .as_object()
                .is_none_or(|stats| stats.is_empty())
    });
    fixture
        .cmd()
        .args(["admin", "replicate", "status", "site1", "--buckets"])
        .assert()
        .success()
        .stdout("Bucket replication status:\n●  1/1 Buckets in sync\n");
    let summary = fixture.ok(&["admin", "replicate", "status", "site1", "--bucket", &bucket]);
    assert!(
        summary.starts_with(&format!(
            "●  Bucket config replication summary for: {bucket}\n\nBucket          | SITE1           | SITE2          \n"
        )),
        "{summary}"
    );
    assert!(summary.contains("\nReplication     | ✔               | ✔              "));
    fixture
        .cmd()
        .args(["admin", "replicate", "status", "site1", "--user", "nobody"])
        .assert()
        .success()
        .stdout("User nobody not found\n");
    let all = fixture.ok(&["admin", "replicate", "status", "site1"]);
    for section in [
        "Bucket replication status:",
        "Policy replication status:",
        "User replication status:",
        "Group replication status:",
        "ILM Expiry Rules replication status:",
        "Object replication status:",
    ] {
        assert!(all.contains(section), "{section} missing:\n{all}");
    }
    let status = fixture.json(&["admin", "replicate", "status", "site1"]);
    assert_eq!(status["Enabled"], true);
    assert_eq!(status["Sites"].as_object().map(|s| s.len()), Some(2));

    // update: sync mode, bandwidth, ILM expiry replication.
    let updated = fixture.ok(&[
        "admin",
        "replicate",
        "update",
        "site1",
        "--deployment-id",
        &site2_id,
        "--mode",
        "sync",
        "--bucket-bandwidth",
        "1G",
    ]);
    assert!(
        updated.starts_with("Cluster replication configuration updated successfully"),
        "{updated}"
    );
    let text = fixture.ok(&["admin", "replicate", "info", "site1"]);
    let site2_line = text
        .lines()
        .find(|line| line.starts_with(&site2_id))
        .expect("site2 row");
    assert!(site2_line.contains("| ✔    | 1.0 GB/s   |"), "{site2_line}");
    let result = fixture.json(&[
        "admin",
        "replicate",
        "update",
        "site1",
        "--enable-ilm-expiry-replication",
    ]);
    assert_eq!(result["success"], true, "{result}");
    let info = fixture.json(&["admin", "replicate", "info", "site1"]);
    assert!(
        info["sites"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["replicate-ilm-expiry"] == true),
        "{info}"
    );
    fixture
        .cmd()
        .args([
            "admin",
            "replicate",
            "update",
            "site1",
            "--deployment-id",
            "no-such-id",
            "--mode",
            "async",
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to edit cluster replication site endpoint.",
        ));

    // resync
    let started = fixture.ok(&["admin", "replicate", "resync", "start", "site1", "site2"]);
    assert!(
        started.starts_with("Site resync started with ID "),
        "{started}"
    );
    fixture
        .cmd()
        .args([
            "--json",
            "admin",
            "replicate",
            "resync",
            "status",
            "site1",
            "site2",
        ])
        .assert()
        .success()
        .stdout("");
    let cancel = fixture
        .cmd()
        .args(["admin", "replicate", "resync", "cancel", "site1", "site2"])
        .output()
        .unwrap();
    let cancel_text = String::from_utf8_lossy(&cancel.stdout).into_owned()
        + &String::from_utf8_lossy(&cancel.stderr);
    // The resync of a small deployment may already be done.
    assert!(
        cancel_text.contains("canceled successfully") || cancel_text.contains("no resync"),
        "{cancel_text}"
    );
    fixture
        .cmd()
        .args(["admin", "replicate", "resync", "start", "site1", "nosuch"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "No valid configuration found for 'nosuch' host alias.",
        ));

    // Removal: --force is required; removing one of two sites ends site replication.
    fixture
        .cmd()
        .args(["admin", "replicate", "rm", "site1", "site2"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "Site removal requires --force flag.",
        ));
    fixture
        .cmd()
        .args(["admin", "replicate", "rm", "site1", "site2", "--force"])
        .assert()
        .success()
        .stdout("Following site(s) [site2] were removed successfully\n");
    fixture
        .cmd()
        .args(["admin", "replicate", "info", "site1"])
        .assert()
        .success()
        .stdout("SiteReplication is not enabled\n");
    // Buckets now exist on both sites: they cannot be joined again.
    fixture
        .cmd()
        .args(["admin", "replicate", "add", "site1", "site2"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "only one cluster may have data when configuring site replication",
        ));
    fixture.ok(&["rb", "--force", &format!("site2/{bucket}")]);
    fixture
        .cmd()
        .args(["admin", "replicate", "add", "site1", "site2"])
        .assert()
        .success();
    wait_for(30, "site replication", || {
        fixture.json(&["admin", "replicate", "info", "site2"])["enabled"] == true
    });
    fixture
        .cmd()
        .args(["admin", "replicate", "rm", "site1", "--all", "--force"])
        .assert()
        .success()
        .stdout("All site(s) were removed successfully\n");
    fixture
        .cmd()
        .args(["admin", "replicate", "info", "site2"])
        .assert()
        .success()
        .stdout("SiteReplication is not enabled\n");
    fixture
        .cmd()
        .args([
            "--json",
            "admin",
            "replicate",
            "rm",
            "site1",
            "--all",
            "--force",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            r#""message":"Unable to remove cluster replication""#,
        ));
}

// ---------------------------------------------------------------------------
// rebalance and decommission (two pools)
// ---------------------------------------------------------------------------

#[test]
fn live_pools_rebalance_then_decommission() {
    if !common::live::enabled() {
        eprintln!("skipping live test; set MX_LIVE_TESTS=1");
        return;
    }
    let vars = [
        "MX_TEST_POOLS_URL",
        "MX_TEST_POOLS_ACCESS_KEY",
        "MX_TEST_POOLS_SECRET_KEY",
        "MX_TEST_POOLS_POOL1",
        "MX_TEST_POOLS_POOL2",
    ];
    let Some(env): Option<Vec<String>> = vars.iter().map(|v| std::env::var(v).ok()).collect()
    else {
        eprintln!("skipping: pools server (tests/services/minio_pools.sh) not available");
        return;
    };
    let (pool1, pool2) = (env[3].as_str(), env[4].as_str());
    let fixture = Fixture::new(&[("pools", &env[0])], &env[1], &env[2]);
    let bucket = common::live::unique_bucket_name();
    fixture.ok(&["mb", &format!("pools/{bucket}")]);
    let data = fixture.home.path().join("obj.txt");
    std::fs::write(&data, "topo data").unwrap();
    for i in 0..5 {
        fixture.ok(&[
            "cp",
            "-q",
            data.to_str().unwrap(),
            &format!("pools/{bucket}/o{i}"),
        ]);
    }

    // rebalance
    fixture
        .cmd()
        .args(["admin", "rebalance", "start", "pools"])
        .assert()
        .success()
        .stdout("Rebalance started for pools\n");
    let status = fixture.ok(&["admin", "rebalance", "status", "pools"]);
    assert!(status.starts_with("Per-pool usage:\n┌"), "{status}");
    assert!(status.contains("│ Pool-0 │ Pool-1 │") || status.contains("Pool-0"));
    assert!(status.contains("\nSummary: \nData: "), "{status}");
    let json = fixture.json(&["admin", "rebalance", "status", "pools"]);
    assert_eq!(json["pools"].as_array().map(Vec::len), Some(2), "{json}");
    assert!(json["ID"].as_str().is_some_and(|id| !id.is_empty()));
    fixture
        .cmd()
        .args(["admin", "rebalance", "stop", "pools"])
        .assert()
        .success()
        .stdout("Rebalance stopped for pools\n");
    assert_eq!(
        fixture.json(&["admin", "rebalance", "stop", "pools"]),
        serde_json::json!({"status": "success", "url": "pools"})
    );

    // decommission
    let table = fixture.ok(&["admin", "decommission", "status", "pools"]);
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines.len(), 5, "{table}");
    assert!(
        lines[1].starts_with("│ ID  │ Pools        │ Drives Usage"),
        "{table}"
    );
    assert!(
        lines[2].starts_with(&format!("│ 1st │ {pool1} │")),
        "{table}"
    );
    assert!(
        lines[3].starts_with(&format!("│ 2nd │ {pool2} │")),
        "{table}"
    );
    assert!(lines[3].ends_with("│ Active │"), "{table}");
    let pools = fixture.json(&["admin", "decommission", "status", "pools"]);
    assert_eq!(pools[1]["cmdline"], pool2);
    fixture
        .cmd()
        .args(["admin", "decommission", "status", "pools", pool2])
        .assert()
        .success()
        .stdout("")
        .stderr("mx: <ERROR> This pool is currently not scheduled for decomissioning \n");
    fixture
        .cmd()
        .args(["admin", "decommission", "start", "pools", "/nope{1...4}"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to start decommission on the specified pool.",
        ));
    fixture
        .cmd()
        .args(["admin", "decommission", "cancel", "pools", pool1])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to cancel decommissioning, please try again.",
        ));

    fixture
        .cmd()
        .args(["admin", "decom", "start", "pools/", pool2])
        .assert()
        .success()
        .stdout(format!(
            "Decommission started successfully for `{pool2}`.\n"
        ));
    wait_for(120, "decommission to complete", || {
        let status = fixture.json(&["admin", "decommission", "status", "pools", pool2]);
        status["decommissionInfo"]["complete"] == true
    });
    fixture
        .cmd()
        .args(["admin", "decommission", "status", "pools", pool2])
        .assert()
        .success()
        .stdout(format!(
            "Decommission of pool {pool2} is complete, you may now remove it from server command line\n"
        ));
    let table = fixture.ok(&["admin", "decommission", "status", "pools"]);
    assert!(
        table
            .lines()
            .nth(3)
            .is_some_and(|l| l.ends_with("│ Complete │")),
        "{table}"
    );
    // Without a pool, cancel lists the pools being drained (none).
    fixture
        .cmd()
        .args(["admin", "decommission", "cancel", "pools"])
        .assert()
        .success()
        .stdout(
            "┌────┬───────┬──────────┬────────┐\n│ ID │ Pools │ Capacity │ Status │\n└────┴───────┴──────────┴────────┘\n",
        );
    // The data moved to the first pool.
    let listing = fixture.ok(&["ls", &format!("pools/{bucket}/")]);
    assert_eq!(listing.lines().count(), 5, "{listing}");
    fixture.run_quietly(&["rb", "--force", &format!("pools/{bucket}")]);
}
