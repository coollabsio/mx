//! Live tests for `quota`, `ilm tier` and `replicate` against MinIO (MX_LIVE_TESTS=1).
//! Tier and replication tests need a second server reachable from the first one
//! (`MX_TEST_URL2_INTERNAL`, exported by tests/live_minio.sh).

mod common;

use common::live::{BucketOpts, Live};
use predicates::prelude::*;
use std::time::{Duration, Instant};

fn internal_url2() -> Option<url::Url> {
    let value = std::env::var("MX_TEST_URL2_INTERNAL").ok()?;
    Some(url::Url::parse(&value).expect("MX_TEST_URL2_INTERNAL"))
}

fn credentials2() -> (String, String) {
    (
        std::env::var("MX_TEST_ACCESS_KEY2").expect("MX_TEST_ACCESS_KEY2"),
        std::env::var("MX_TEST_SECRET_KEY2").expect("MX_TEST_SECRET_KEY2"),
    )
}

fn stdout(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

/// Runs a cleanup command on drop (best effort), even if the test panics.
struct Cleanup<'a> {
    live: &'a Live,
    args: Vec<String>,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = self.live.cmd().args(&self.args).output();
    }
}

#[test]
fn live_quota_set_info_clear() {
    let Some(live) = Live::new() else { return };
    let target = live.bucket_target();
    live.cmd()
        .args(["quota", "set", &target, "--size", "64MiB"])
        .assert()
        .success()
        .stdout(format!(
            "Successfully set bucket quota of 64 MiB on `{}`\n",
            live.bucket
        ));
    live.cmd()
        .args(["quota", "info", &target])
        .assert()
        .success()
        .stdout(format!(
            "Bucket `{}` has hard quota of 64 MiB\n",
            live.bucket
        ));
    let out = stdout(
        live.cmd()
            .args(["--json", "quota", "info", &target])
            .assert()
            .success(),
    );
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["quota"], 64 * 1024 * 1024);
    assert_eq!(value["type"], "hard");
    live.cmd()
        .args(["quota", "clear", &target])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Successfully cleared bucket quota",
        ));
    let out = stdout(
        live.cmd()
            .args(["--json", "quota", "info", &target])
            .assert()
            .success(),
    );
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(value.get("quota").is_none(), "{out}");
}

#[test]
fn live_ilm_tier_lifecycle() {
    let Some(live) = Live::new() else { return };
    let Some(internal) = internal_url2() else {
        eprintln!("skipping tier test; MX_TEST_URL2_INTERNAL not set");
        return;
    };
    let Some(remote_bucket) = live.make_bucket2(BucketOpts::default()) else {
        eprintln!("skipping tier test; second server not configured");
        return;
    };
    let (access_key, secret_key) = credentials2();
    let name = format!("T{}", live.bucket.replace('-', "")).to_uppercase();
    let _cleanup = Cleanup {
        live: &live,
        args: vec![
            "ilm".into(),
            "tier".into(),
            "rm".into(),
            live.alias.clone(),
            name.clone(),
        ],
    };
    let endpoint = internal.as_str().trim_end_matches('/').to_string();
    let add = |expect_success: bool| {
        let assert = live
            .cmd()
            .args([
                "ilm",
                "tier",
                "add",
                "minio",
                &live.alias,
                &name.to_lowercase(),
                "--endpoint",
                &endpoint,
                "--access-key",
                &access_key,
                "--secret-key",
                &secret_key,
                "--bucket",
                &remote_bucket,
                "--prefix",
                "tier/",
            ])
            .assert();
        if expect_success {
            assert
                .success()
                .stdout(format!("Added remote tier {name} of type minio\n"));
        } else {
            assert.failure();
        }
    };
    add(true);
    add(false);

    live.cmd()
        .args(["ilm", "tier", "ls", &live.alias])
        .assert()
        .success()
        .stdout(predicate::str::contains(&name))
        .stdout(predicate::str::contains(&remote_bucket));
    let out = stdout(
        live.cmd()
            .args(["--json", "ilm", "tier", "ls", &live.alias])
            .assert()
            .success(),
    );
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    let tier = value["tiers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["Name"] == name.as_str())
        .expect("tier listed");
    assert_eq!(tier["MinIO"]["Bucket"], remote_bucket.as_str());
    assert_eq!(tier["MinIO"]["Prefix"], "tier/");

    live.cmd()
        .args(["ilm", "tier", "check", &live.alias, &name])
        .assert()
        .success()
        .stdout(predicate::str::contains("was successful"));
    live.cmd()
        .args(["ilm", "tier", "verify", &live.alias, &name])
        .assert()
        .success();
    live.cmd()
        .args([
            "ilm",
            "tier",
            "edit",
            &live.alias,
            &name,
            "--access-key",
            &access_key,
            "--secret-key",
            &secret_key,
        ])
        .assert()
        .success()
        .stdout(format!("Updated remote tier {name}\n"));
    live.cmd()
        .args(["ilm", "tier", "info", &live.alias, &name])
        .assert()
        .success()
        .stdout(predicate::str::contains(&name));
    live.cmd()
        .args(["--json", "ilm", "tier", "info", &live.alias])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""status":"success""#));

    live.cmd()
        .args(["ilm", "tier", "rm", &live.alias, &name])
        .assert()
        .success()
        .stdout(format!("Removed remote tier {name}\n"));
    live.cmd()
        .args(["ilm", "tier", "check", &live.alias, &name])
        .assert()
        .failure();
}

#[test]
fn live_replicate_workflow() {
    let Some(live) = Live::with_bucket(BucketOpts {
        versioning: true,
        lock: false,
    }) else {
        return;
    };
    let Some(internal) = internal_url2() else {
        eprintln!("skipping replication test; MX_TEST_URL2_INTERNAL not set");
        return;
    };
    let Some(remote_bucket) = live.make_bucket2(BucketOpts {
        versioning: true,
        lock: false,
    }) else {
        eprintln!("skipping replication test; second server not configured");
        return;
    };
    let alias2 = live.alias2.clone().unwrap();
    let (access_key, secret_key) = credentials2();
    let source = live.bucket_target();
    let _cleanup = Cleanup {
        live: &live,
        args: vec![
            "replicate".into(),
            "rm".into(),
            source.clone(),
            "--all".into(),
            "--force".into(),
        ],
    };
    let remote = format!(
        "{}://{access_key}:{secret_key}@{}:{}/{remote_bucket}",
        internal.scheme(),
        internal.host_str().unwrap(),
        internal.port_or_known_default().unwrap()
    );

    live.cmd()
        .args([
            "replicate",
            "add",
            &source,
            "--remote-bucket",
            &remote,
            "--priority",
            "1",
            "--id",
            "r1",
        ])
        .assert()
        .success()
        .stdout(format!(
            "Replication configuration rule with ID `r1` applied to {source}.\n"
        ));

    // Replicates new objects to the second server.
    let file = live.local_file("replicated.txt", "replicate me");
    live.cmd()
        .args(["cp", file.to_str().unwrap(), &live.url("replicated.txt")])
        .assert()
        .success();
    let replica = format!("{alias2}/{remote_bucket}/replicated.txt");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if live
            .cmd()
            .args(["stat", &replica])
            .output()
            .unwrap()
            .status
            .success()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "object was not replicated in 60s"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    live.cmd()
        .args(["cat", &replica])
        .assert()
        .success()
        .stdout("replicate me");

    live.cmd()
        .args(["replicate", "ls", &source])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rule ID: r1"))
        .stdout(predicate::str::contains(format!("/{remote_bucket}\n")));
    let out = stdout(
        live.cmd()
            .args(["--json", "replicate", "ls", &source])
            .assert()
            .success(),
    );
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["rule"]["ID"], "r1");
    assert_eq!(value["rule"]["DeleteReplication"]["Status"], "Enabled");
    let arn = value["rule"]["Destination"]["Bucket"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(arn.starts_with("arn:minio:replication:"), "{arn}");

    live.cmd()
        .args(["replicate", "status", &source])
        .assert()
        .success()
        .stdout(predicate::str::contains("Replication status since"));
    let out = stdout(
        live.cmd()
            .args(["--json", "replicate", "status", &source])
            .assert()
            .success(),
    );
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(value["replicationstats"].is_object());
    assert_eq!(value["remoteTargets"][0]["arn"], arn.as_str());

    // update: priority and state
    live.cmd()
        .args([
            "replicate",
            "update",
            &source,
            "--id",
            "r1",
            "--priority",
            "5",
            "--state",
            "disable",
        ])
        .assert()
        .success();
    live.cmd()
        .args(["replicate", "ls", &source, "--status", "disabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Priority: 5"));
    live.cmd()
        .args([
            "replicate",
            "update",
            &source,
            "--id",
            "r1",
            "--state",
            "enable",
            "--remote-bucket",
            &remote,
            "--sync",
            "enable",
            "--bandwidth",
            "100MiB",
        ])
        .assert()
        .success();

    // export / import round trip
    let exported = stdout(
        live.cmd()
            .args(["replicate", "export", &source])
            .assert()
            .success(),
    );
    let config: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(config["Rules"][0]["Priority"], 5);
    assert_eq!(config["Rules"][0]["Status"], "Enabled");
    live.cmd()
        .args(["replicate", "import", &source])
        .write_stdin(exported.clone())
        .assert()
        .success()
        .stdout(format!(
            "Replication configuration successfully set on `{source}`.\n"
        ));
    let reexported = stdout(
        live.cmd()
            .args(["replicate", "export", &source])
            .assert()
            .success(),
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&reexported).unwrap(),
        config
    );

    // backlog and resync
    live.cmd()
        .args(["replicate", "backlog", &source])
        .assert()
        .success();
    live.cmd()
        .args(["--json", "replicate", "backlog", &source, "--full"])
        .assert()
        .success();
    live.cmd()
        .args([
            "replicate",
            "resync",
            "start",
            &source,
            "--remote-bucket",
            &arn,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Replication reset started for"));
    let out = stdout(
        live.cmd()
            .args([
                "--json",
                "replicate",
                "resync",
                "status",
                &source,
                "--remote-bucket",
                &arn,
            ])
            .assert()
            .success(),
    );
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["resyncInfo"]["target"][0]["arn"], arn.as_str());

    // rm: the last rule can only go with --all --force
    live.cmd()
        .args(["replicate", "rm", &source, "--id", "r1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("at least one rule"));
    live.cmd()
        .args(["replicate", "rm", &source, "--all", "--force"])
        .assert()
        .success()
        .stdout(format!(
            "Replication configuration removed from {source} successfully.\n"
        ));
    live.cmd()
        .args(["replicate", "ls", &source])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "replication configuration not set",
        ));
}
