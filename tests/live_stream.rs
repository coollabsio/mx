//! Live tests for the streaming commands against MinIO (MX_LIVE_TESTS=1): `admin trace`,
//! `admin scanner trace`, `admin logs`, `admin heal`, `watch`.
//!
//!   sh tests/live_minio.sh live_stream
//!
//! Streaming commands never end by themselves: they are spawned, traffic is generated, and
//! they are stopped with SIGTERM (exit status 143 like mc). Heal sequences run against the
//! erasure-coded `minio_ec` service (`MX_TEST_EC_URL`) when available.

mod common;

use common::live::Live;
use predicates::prelude::*;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Spawns `mx ARGS` with the fixture HOME, runs `traffic` after it connected, then stops it
/// with SIGTERM; returns (stdout, stderr, exit code).
fn stream(live: &Live, args: &[&str], traffic: impl FnOnce()) -> (String, String, Option<i32>) {
    let mut child = Command::new(assert_cmd::cargo::cargo_bin("mx"))
        .args(args)
        .env("HOME", live.home.path())
        .env("TZ", "UTC")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mx");
    std::thread::sleep(Duration::from_millis(1500));
    traffic();
    std::thread::sleep(Duration::from_millis(1500));
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    let status = child.wait().expect("wait");
    let (mut out, mut err) = (String::new(), String::new());
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    (out, err, status.code())
}

fn put(live: &Live, key: &str, body: &str) {
    live.cmd()
        .args(["pipe", &live.url(key)])
        .write_stdin(body.to_string())
        .assert()
        .success();
}

#[test]
fn live_admin_trace_shows_bucket_calls() {
    let Some(live) = Live::new() else { return };
    let path = format!("{}/*", live.bucket);
    let (out, err, code) = stream(
        &live,
        &["admin", "trace", "--path", &path, &live.alias],
        || {
            put(&live, "a.txt", "hello");
            live.cmd()
                .args(["cat", &live.url("a.txt")])
                .assert()
                .success();
            let _ = live.cmd().args(["cat", &live.url("missing")]).output();
        },
    );
    assert_eq!(code, Some(143), "{out}{err}");
    assert!(out.contains("[200 OK] s3.PutObject "), "{out}");
    assert!(
        out.contains("[200 OK] s3.GetObject ") && out.contains(&format!("/{}/a.txt", live.bucket)),
        "{out}"
    );
    assert!(out.contains("[404 Not Found] "), "{out}");
    assert!(out.lines().all(|l| l.contains(&live.bucket)), "{out}");

    // Only failed calls, as JSON lines.
    let (out, _, _) = stream(
        &live,
        &[
            "admin",
            "trace",
            "--json",
            "-e",
            "--path",
            &path,
            &live.alias,
        ],
        || {
            put(&live, "b.txt", "x");
            let _ = live.cmd().args(["cat", &live.url("missing")]).output();
        },
    );
    let docs: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).expect("JSON line"))
        .collect();
    assert!(!docs.is_empty(), "{out}");
    for doc in &docs {
        assert_eq!(doc["status"], "success");
        assert_eq!(doc["type"], "S3");
        assert!(doc["statusCode"].as_i64().unwrap() >= 400, "{doc}");
    }
}

#[test]
fn live_admin_trace_verbose() {
    let Some(live) = Live::new() else { return };
    let path = format!("{}/v.txt", live.bucket);
    let (out, _, _) = stream(
        &live,
        &["admin", "trace", "-v", "--path", &path, &live.alias],
        || put(&live, "v.txt", "hello"),
    );
    assert!(out.contains("[REQUEST s3.PutObject] "), "{out}");
    assert!(
        out.contains(&format!("PUT /{}/v.txt", live.bucket)),
        "{out}"
    );
    assert!(out.contains("[RESPONSE] "), "{out}");
    assert!(out.contains(" 200 OK\n"), "{out}");
}

#[test]
fn live_admin_scanner_trace_runs_until_stopped() {
    let Some(live) = Live::new() else { return };
    let (_, err, code) = stream(&live, &["admin", "scanner", "trace", &live.alias], || {});
    assert_eq!(code, Some(143), "{err}");
}

#[test]
fn live_admin_logs_last_entries() {
    let Some(live) = Live::new() else { return };
    let Ok(arn) = std::env::var("MX_TEST_NOTIFY_ARN") else {
        eprintln!("skipping; MX_TEST_NOTIFY_ARN not set");
        return;
    };
    // An unreachable event target makes the server log an error.
    live.cmd()
        .args([
            "event",
            "add",
            &live.bucket_target(),
            &arn,
            "--event",
            "put",
        ])
        .assert()
        .success();
    put(&live, "logged.txt", "x");
    std::thread::sleep(Duration::from_secs(1));
    let (out, err, code) = stream(&live, &["admin", "logs", "--last", "1", &live.alias], || {});
    assert_eq!(code, Some(143), "{err}");
    assert!(out.contains(" DeploymentID: "), "{out}");
    let (out, _, _) = stream(
        &live,
        &["admin", "logs", "--json", "--last", "1", &live.alias],
        || {},
    );
    let doc: serde_json::Value =
        serde_json::from_str(out.lines().next().unwrap_or_default()).expect("JSON");
    assert_eq!(doc["status"], "success");
    assert!(doc["deploymentid"].is_string(), "{doc}");
    assert!(doc.get("ConsoleMsg").is_some(), "{doc}");
}

#[test]
fn live_watch_bucket_events() {
    let Some(live) = Live::new() else { return };
    let (out, err, code) = stream(&live, &["watch", &live.bucket_target()], || {
        put(&live, "dir/f.txt", "abc");
        live.cmd()
            .args(["rm", &live.url("dir/f.txt")])
            .assert()
            .success();
    });
    assert_eq!(code, Some(143), "{err}");
    let url = live.alias_config().url.trim_end_matches('/').to_string();
    let path = format!("{url}/{}/dir/f.txt", live.bucket);
    assert!(
        out.contains(&format!("    3 B s3:ObjectCreated:Put {path}\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!("        s3:ObjectRemoved:Delete {path}\n")),
        "{out}"
    );

    let (out, _, _) = stream(
        &live,
        &[
            "--json",
            "watch",
            "--events",
            "put",
            "--suffix",
            ".jpg",
            &live.bucket_target(),
        ],
        || {
            put(&live, "skip.txt", "x");
            put(&live, "pic.jpg", "xy");
        },
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "{out}");
    let doc: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(doc["events"]["size"], 2);
    assert_eq!(doc["events"]["type"], "s3:ObjectCreated:Put");
    assert!(doc["source"]["userAgent"].is_string(), "{doc}");
}

#[test]
fn live_watch_missing_bucket_reports_error() {
    let Some(live) = Live::new() else { return };
    live.cmd()
        .args(["watch", &format!("{}/{}-nope", live.alias, live.bucket)])
        .assert()
        .success()
        .stderr("mx: <ERROR> Unable to watch for events. The specified bucket does not exist\n");
}

#[test]
fn live_admin_heal_single_drive_is_unsupported() {
    let Some(live) = Live::new() else { return };
    live.cmd()
        .args(["admin", "heal", &live.alias])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to get background heal status. This 'admin' API is not supported by server in",
        ));
    live.cmd()
        .args(["admin", "heal", "-r", &live.bucket_target()])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to start healing.",
        ));
}

#[test]
fn live_admin_heal_erasure_coded() {
    let Some(live) = Live::new() else { return };
    let Ok(url) = std::env::var("MX_TEST_EC_URL") else {
        eprintln!("skipping; MX_TEST_EC_URL not set (service minio_ec)");
        return;
    };
    let user = std::env::var("MX_TEST_EC_ACCESS_KEY").unwrap();
    let pass = std::env::var("MX_TEST_EC_SECRET_KEY").unwrap();
    let host = url.replacen("://", &format!("://{user}:{pass}@"), 1);
    let cmd = || {
        let mut cmd = live.cmd();
        cmd.env("MC_HOST_ec", &host);
        cmd
    };
    let bucket = format!("ec/{}", live.bucket);
    cmd().args(["mb", &bucket]).assert().success();
    for key in ["dir/x1", "dir/x2"] {
        cmd()
            .args(["pipe", &format!("{bucket}/{key}")])
            .write_stdin("hello")
            .assert()
            .success();
    }
    cmd()
        .args(["admin", "heal", "ec"])
        .assert()
        .success()
        .stdout("No active healing is detected for new disks.\n");
    cmd()
        .args(["admin", "heal", "-v", "ec"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "Server status:\n==============\nPool 1st:\n",
        ));
    cmd()
        .args(["admin", "heal", "-r", &bucket])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "[Green  ->  Green] {}/dir/x1\n",
            live.bucket
        )))
        .stdout(predicate::str::contains("Healed:\t0/2 objects; 10 B in "));
    let out = cmd()
        .args(["--json", "admin", "heal", "-r", &bucket])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let last: serde_json::Value =
        serde_json::from_str(text.lines().last().unwrap_or_default()).unwrap();
    assert_eq!(last["type"], "summary", "{text}");
    assert_eq!(last["objects_scanned"], 2, "{text}");
    cmd()
        .args(["admin", "heal", "--force-stop", &bucket])
        .assert()
        .success()
        .stdout(format!("Heal stopped successfully at `{bucket}`.\n"));
    let _ = cmd().args(["rb", "--force", &bucket]).output();
}
