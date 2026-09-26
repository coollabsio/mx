//! Live tests for the SERVER area (`admin info|config|kms|prometheus|scanner|cluster|
//! service|update`) against MinIO (MX_LIVE_TESTS=1, `sh tests/live_minio.sh live_server`).
//! Disruptive commands (restart, freeze, IAM import, update) use the dedicated server from
//! tests/services/minio_server.sh (`MX_TEST_SERVER_URL`) and skip without it.

mod common;

use common::live::Live;
use serde_json::Value;
use std::time::{Duration, Instant};

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json_docs(text: &str) -> Vec<Value> {
    serde_json::Deserializer::from_str(text)
        .into_iter::<Value>()
        .map(|doc| doc.expect("json document"))
        .collect()
}

/// Adds alias `srv` for the dedicated server; None (skip) when it is not running.
fn dedicated(live: &Live) -> Option<String> {
    let url = std::env::var("MX_TEST_SERVER_URL").ok()?;
    let access = std::env::var("MX_TEST_SERVER_ACCESS_KEY").ok()?;
    let secret = std::env::var("MX_TEST_SERVER_SECRET_KEY").ok()?;
    live.cmd()
        .args(["alias", "set", "srv", &url, &access, &secret])
        .assert()
        .success();
    Some("srv".to_string())
}

fn wait_ready(live: &Live, alias: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if live
            .cmd()
            .args(["ready", alias])
            .output()
            .is_ok_and(|o| o.status.success())
            && live
                .cmd()
                .args(["admin", "info", alias])
                .output()
                .is_ok_and(|o| o.status.success())
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("{alias} did not come back");
}

#[test]
fn live_admin_info() {
    let Some(live) = Live::new() else { return };
    let out = live
        .cmd()
        .args(["admin", "info", &live.alias])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("●  "), "{text}");
    assert!(text.contains("   Uptime: "), "{text}");
    assert!(text.contains("   Drives: 1/1 OK \n"), "{text}");
    assert!(text.contains("│ Pool │ Drives Usage"), "{text}");
    assert!(
        text.ends_with("1 drive online, 0 drives offline, EC:0\n"),
        "{text}"
    );

    let out = live
        .cmd()
        .args(["--json", "admin", "info", &live.alias])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["status"], "success");
    assert_eq!(doc["info"]["mode"], "online");
    assert_eq!(doc["info"]["backend"]["backendType"], "Erasure");
    assert!(doc["info"]["servers"][0]["drives"].is_array());
    assert!(doc.get("error").is_none());
}

#[test]
fn live_config_get_set_reset_history() {
    let Some(live) = Live::new() else { return };
    let alias = live.alias.clone();
    let out = live
        .cmd()
        .args(["admin", "config", "get", &alias, "scanner"])
        .output()
        .unwrap();
    assert!(
        stdout(&out).starts_with("scanner speed="),
        "{}",
        stdout(&out)
    );
    let out = live
        .cmd()
        .args(["--json", "admin", "config", "get", &alias, "scanner"])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["config"][0]["subSystem"], "scanner");
    assert_eq!(doc["config"][0]["kv"][0]["key"], "speed");

    // Help output when no `key=value` is given.
    let out = live
        .cmd()
        .args(["admin", "config", "set", &alias, "scanner"])
        .output()
        .unwrap();
    assert!(
        stdout(&out).starts_with("KEY:\nscanner  "),
        "{}",
        stdout(&out)
    );
    let out = live
        .cmd()
        .args(["admin", "config", "get", &alias])
        .output()
        .unwrap();
    assert!(stdout(&out).starts_with("KEYS:\n"), "{}", stdout(&out));

    live.cmd()
        .args(["admin", "config", "set", &alias, "scanner", "speed=default"])
        .assert()
        .success()
        .stdout(predicates::str::starts_with(
            "Successfully applied new settings.",
        ));
    let out = live
        .cmd()
        .args(["--json", "admin", "config", "history", &alias, "-n", "100"])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let entry = doc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["targets"] == "scanner speed=default")
        .expect("history entry of the set");
    assert!(entry["createTime"].as_str().unwrap().ends_with(" GMT"));
    let restore_id = entry["restoreId"].as_str().unwrap().to_string();
    live.cmd()
        .args(["admin", "config", "restore", &alias, &restore_id])
        .assert()
        .success()
        .stdout(format!(
            "Please restart your server with `mc admin service restart {alias}`.\nRestored {restore_id} kv successfully.\n"
        ));
    live.cmd()
        .args(["admin", "config", "reset", &alias, "scanner", "speed"])
        .assert()
        .success()
        .stdout(predicates::str::starts_with(
            "'scanner speed' is successfully reset.",
        ));
    live.cmd()
        .args(["admin", "config", "get", &alias, "bogus"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> Unable to get server '[bogus]' config: unknown subsystem: bogus.\n");
}

#[test]
fn live_config_export_import() {
    let Some(live) = Live::new() else { return };
    let out = live
        .cmd()
        .args(["admin", "config", "export", &live.alias])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("\napi "), "{}", stdout(&out));
    let out = live
        .cmd()
        .args(["--json", "admin", "config", "export", &live.alias])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["status"], "success");
    // The shared server's env-configured notify target makes MinIO reject its own export
    // (`sub-system 'notify_webhook' cannot have empty keys`), for mc too: round-trip the
    // dedicated server's config instead.
    let Some(srv) = dedicated(&live) else {
        eprintln!("skipping import; dedicated server (MX_TEST_SERVER_URL) not running");
        return;
    };
    let out = live
        .cmd()
        .args(["admin", "config", "export", &srv])
        .output()
        .unwrap();
    live.cmd()
        .args(["admin", "config", "import", &srv])
        .write_stdin(out.stdout)
        .assert()
        .success()
        .stdout(format!(
            "Setting new key has been successful.\nPlease restart your server with `mc admin service restart {srv}`.\n"
        ));
    live.cmd()
        .args(["--json", "admin", "config", "import", &srv])
        .write_stdin("bogus x=1\n")
        .assert()
        .code(1)
        .stdout(predicates::str::contains("unknown sub-system bogus x=1"));
}

#[test]
fn live_kms_keys() {
    let Some(live) = Live::new() else { return };
    let key = std::env::var("MX_TEST_KMS_KEY_ID").unwrap_or_else(|_| "mx-test-key".into());
    let out = live
        .cmd()
        .args(["admin", "kms", "key", "list", &live.alias])
        .output()
        .unwrap();
    assert!(
        stdout(&out).contains(&format!("│   1 │ {key} │")),
        "{}",
        stdout(&out)
    );
    live.cmd()
        .args(["admin", "kms", "key", "status", &live.alias])
        .assert()
        .success()
        .stdout(format!(
            "Key: {key}\n   - Encryption ✔\n   - Decryption ✔\n"
        ));
    live.cmd()
        .args(["--json", "admin", "kms", "key", "status", &live.alias, "nokey"])
        .assert()
        .success()
        .stdout(
            "{\"keyId\":\"nokey\",\"encryptionError\":\"key with given key ID does not exist\",\"status\":\"success\"}\n",
        );
    // The static-key KMS of the test server cannot create keys.
    live.cmd()
        .args(["admin", "kms", "key", "create", &live.alias, "newkey"])
        .assert()
        .code(1)
        .stderr(predicates::str::starts_with(
            "mx: <ERROR> Failed to create master key.",
        ));
}

#[test]
fn live_prometheus_metrics() {
    let Some(live) = Live::new() else { return };
    let out = live
        .cmd()
        .args(["admin", "prometheus", "metrics", &live.alias])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("# HELP "), "{}", stdout(&out));
    let out = live
        .cmd()
        .args([
            "--json",
            "admin",
            "prometheus",
            "metrics",
            &live.alias,
            "node",
        ])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let families = doc.as_array().unwrap();
    assert!(families.iter().any(|f| f["name"] == "go_threads"));
    let out = live
        .cmd()
        .args([
            "admin",
            "prometheus",
            "metrics",
            &live.alias,
            "system",
            "--api-version",
            "v3",
        ])
        .output()
        .unwrap();
    assert!(stdout(&out).contains("minio_system_"), "{}", stdout(&out));
    // A token for another access key is rejected.
    live.cmd()
        .env(
            "MC_HOST_badkey",
            std::env::var("MX_TEST_URL")
                .unwrap()
                .replacen("://", "://nouser:nopassword1@", 1),
        )
        .args(["admin", "prometheus", "metrics", "badkey"])
        .assert()
        .code(1)
        .stderr(
            "mx: <ERROR> Unable to list prometheus metrics with api-version v2. 403 Forbidden.\n",
        );
}

#[test]
fn live_scanner_status() {
    let Some(live) = Live::new() else { return };
    let out = live
        .cmd()
        .args([
            "--json",
            "admin",
            "scanner",
            "status",
            &live.alias,
            "-n",
            "1",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let docs = json_docs(&stdout(&out));
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["final"], true);
    assert!(docs[0]["aggregated"]["scanner"]["collected"].is_string());
    // Leading `--json`: mc prints this encoder output indented.
    assert!(stdout(&out).starts_with("{\n \"hosts\": ["));
    let out = live
        .cmd()
        .args([
            "admin",
            "scanner",
            "status",
            &live.alias,
            "-n",
            "1",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(stdout(&out).starts_with("{\"hosts\":["), "{}", stdout(&out));
    // Per-bucket scan stats are not available on this MinIO build.
    live.cmd()
        .args([
            "admin",
            "scanner",
            "status",
            &live.alias,
            "--bucket",
            &live.bucket,
        ])
        .assert()
        .code(1)
        .stderr(predicates::str::starts_with(
            "mx: <ERROR> Unable to get bucket stats. This 'admin' API is not supported by server",
        ));
}

#[test]
fn live_cluster_bucket_export_import() {
    let Some(live) = Live::new() else { return };
    live.cmd()
        .args(["version", "enable", &live.bucket_target()])
        .assert()
        .success();
    let work = tempfile::tempdir().unwrap();
    let target = live.bucket_target();
    let file = format!(
        "{}/{}-{}-metadata.zip",
        live.alias, live.bucket, live.bucket
    );
    live.cmd()
        .current_dir(work.path())
        .args(["admin", "cluster", "bucket", "export", &target])
        .assert()
        .success()
        .stdout(format!(
            "mx: Bucket metadata successfully downloaded as {file}\n"
        ));
    let saved = work.path().join(&file);
    assert!(saved.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&saved).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // A second export keeps the first one as a timestamped backup.
    live.cmd()
        .current_dir(work.path())
        .args(["--json", "admin", "cluster", "bucket", "export", &target])
        .assert()
        .success()
        .stdout(format!("{{\"file\":\"{file}\"}}\n"));
    let backups = std::fs::read_dir(saved.parent().unwrap())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("-metadata.zip.")
        })
        .count();
    assert_eq!(backups, 1);

    let out = live
        .cmd()
        .current_dir(work.path())
        .args(["admin", "cluster", "bucket", "import", &target, &file])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "\n1/1 buckets were imported successfully.\n");
    let out = live
        .cmd()
        .current_dir(work.path())
        .args([
            "admin", "cluster", "bucket", "import", &target, &file, "--json",
        ])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc[&live.bucket]["versioning"]["isSet"], true);
}

#[test]
fn live_cluster_iam_export_import() {
    let Some(live) = Live::new() else { return };
    let Some(srv) = dedicated(&live) else {
        eprintln!("skipping; dedicated server (MX_TEST_SERVER_URL) not running");
        return;
    };
    let work = tempfile::tempdir().unwrap();
    live.cmd()
        .current_dir(work.path())
        .args(["admin", "cluster", "iam", "export", &srv])
        .assert()
        .success()
        .stdout(format!(
            "mx: IAM info successfully downloaded as {srv}-iam-info.zip\n"
        ));
    live.cmd()
        .current_dir(work.path())
        .args([
            "admin",
            "cluster",
            "iam",
            "export",
            &srv,
            "-o",
            "custom.zip",
        ])
        .assert()
        .success()
        .stdout("mx: IAM info successfully downloaded as custom.zip\n");
    let out = live
        .cmd()
        .current_dir(work.path())
        .args([
            "--json",
            "admin",
            "cluster",
            "iam",
            "import",
            &srv,
            "custom.zip",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        doc["added"].is_object() && doc["failed"].is_object(),
        "{doc}"
    );
}

#[test]
fn live_service_freeze_restart_update() {
    let Some(live) = Live::new() else { return };
    let Some(srv) = dedicated(&live) else {
        eprintln!("skipping; dedicated server (MX_TEST_SERVER_URL) not running");
        return;
    };
    live.cmd()
        .args(["admin", "service", "freeze", &srv])
        .assert()
        .success()
        .stdout(format!("Freeze command successfully sent to `{srv}`.\n"));
    live.cmd()
        .args(["--json", "admin", "service", "unfreeze", &srv])
        .assert()
        .success()
        .stdout(format!(
            "{{\"status\":\"success\",\"serverURL\":\"{srv}\"}}\n"
        ));

    let out = live
        .cmd()
        .args(["--json", "admin", "service", "restart", &srv, "--dry-run"])
        .output()
        .unwrap();
    let docs = json_docs(&stdout(&out));
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["result"]["dryRun"], true);
    assert_eq!(docs[0]["state"], 2);

    let out = live
        .cmd()
        .args(["--json", "admin", "service", "restart", &srv, "--wait"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let docs = json_docs(&stdout(&out));
    assert_eq!(docs.first().unwrap()["state"], 0);
    assert_eq!(docs.last().unwrap()["state"], 2);
    assert_eq!(docs[0]["result"]["action"], "restart");
    wait_ready(&live, &srv);

    // The server downloads the release itself; pointing it at itself fails fast.
    live.cmd()
        .args([
            "admin",
            "update",
            &srv,
            "http://127.0.0.1:9000/minio.sha256sum",
            "-y",
        ])
        .assert()
        .code(1)
        .stderr(predicates::str::starts_with(
            "mx: <ERROR> Unable to update the server. Error downloading URL http://127.0.0.1:9000/minio.sha256sum.",
        ));
}
