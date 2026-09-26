//! Live checks for retention, legalhold, event, undo, od, and ilm restore (area G).
//! Skipped unless MX_LIVE_TESTS=1 (see tests/live_minio.sh).

mod common;

use common::live::{BucketOpts, Live};
use predicates::prelude::*;
use serde_json::Value;

const LOCK: BucketOpts = BucketOpts {
    versioning: false,
    lock: true,
};
const VERSIONED: BucketOpts = BucketOpts {
    versioning: true,
    lock: false,
};

fn json_lines(output: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(output)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json line"))
        .collect()
}

fn run_json(live: &Live, args: &[&str]) -> Vec<Value> {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let output = live.cmd().args(&full).output().unwrap();
    assert!(
        output.status.success(),
        "mx {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    json_lines(&output.stdout)
}

fn put(live: &Live, key: &str, contents: &str) {
    let file = live.local_file("upload.txt", contents);
    live.cmd()
        .args(["put", file.to_str().unwrap(), &live.url(key)])
        .assert()
        .success();
}

/// Clears retention and legal hold on every version so the fixture can remove the bucket.
fn unlock_all(live: &Live) {
    live.cmd()
        .args(["retention", "clear", "-r", "--versions", &live.url("")])
        .output()
        .unwrap();
    live.cmd()
        .args(["legalhold", "clear", "-r", "--versions", &live.url("")])
        .output()
        .unwrap();
}

#[test]
fn live_retention_object_and_bypass() {
    let Some(live) = Live::with_bucket(LOCK) else {
        return;
    };
    put(&live, "dir/a.txt", "a1");
    put(&live, "dir/a.txt", "a2");
    put(&live, "dir/b.txt", "b");

    live.cmd()
        .args([
            "retention",
            "set",
            "GOVERNANCE",
            "2d",
            &live.url("dir/a.txt"),
        ])
        .assert()
        .success()
        .stdout(format!(
            "Object retention successfully set for `{}`.\n",
            live.url("dir/a.txt")
        ));
    live.cmd()
        .args(["retention", "info", &live.url("dir/a.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Name    : {}",
            live.url("dir/a.txt")
        )))
        .stdout(predicate::str::contains(
            "Mode    : GOVERNANCE, expiring in 1 days, 23 hours",
        ));

    // Shortening GOVERNANCE retention needs --bypass.
    live.cmd()
        .args([
            "retention",
            "set",
            "governance",
            "1d",
            &live.url("dir/a.txt"),
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("Unable to set object retention"));
    live.cmd()
        .args([
            "retention",
            "set",
            "governance",
            "1d",
            "--bypass",
            &live.url("dir/a.txt"),
        ])
        .assert()
        .success();
    let info = run_json(&live, &["retention", "info", &live.url("dir/a.txt")]);
    assert_eq!(info[0]["mode"], "GOVERNANCE");
    assert_eq!(info[0]["status"], "success");
    assert!(info[0]["until"].as_str().unwrap().ends_with('Z'));

    // --versions covers every version of the object; -r covers the prefix.
    let set = run_json(
        &live,
        &[
            "retention",
            "set",
            "governance",
            "1d",
            "--versions",
            &live.url("dir/a.txt"),
        ],
    );
    assert_eq!(set.len(), 2);
    assert!(set.iter().all(|m| m["op"] == "set" && m["versionID"] != ""));
    let listed = run_json(&live, &["retention", "info", "-r", &live.url("dir/")]);
    let modes: Vec<(&str, &str)> = listed
        .iter()
        .map(|m| (m["urlpath"].as_str().unwrap(), m["mode"].as_str().unwrap()))
        .collect();
    let (a, b) = (live.url("dir/a.txt"), live.url("dir/b.txt"));
    assert_eq!(modes, [(a.as_str(), "GOVERNANCE"), (b.as_str(), "")]);
    live.cmd()
        .args(["retention", "info", "-r", &live.url("dir/")])
        .assert()
        .success()
        .stdout(predicate::str::contains("[    GOVERNANCE      ]  "))
        .stdout(predicate::str::contains("[    NO RETENTION    ]  "));

    let versions = run_json(
        &live,
        &["retention", "info", "--versions", &live.url("dir/a.txt")],
    );
    assert_eq!(versions.len(), 2);

    // Clearing always bypasses governance.
    let cleared = run_json(
        &live,
        &["retention", "clear", "-r", "--versions", &live.url("dir/")],
    );
    assert_eq!(cleared.len(), 3);
    assert!(
        cleared
            .iter()
            .all(|m| m["op"] == "clear" && m["status"] == "success")
    );
    let after = run_json(
        &live,
        &["retention", "info", "-r", "--versions", &live.url("")],
    );
    assert!(after.iter().all(|m| m["mode"] == ""));
    live.cmd()
        .args(["retention", "info", "-r", &live.url("missing/")])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Unable to find any object/version",
        ));
    unlock_all(&live);
}

#[test]
fn live_retention_bucket_default() {
    let Some(live) = Live::with_bucket(LOCK) else {
        return;
    };
    let bucket = live.bucket_target();
    live.cmd()
        .args(["retention", "info", &bucket])
        .assert()
        .success()
        .stdout("Object locking is not enabled.\n");
    live.cmd()
        .args(["retention", "set", "--default", "GOVERNANCE", "1d", &bucket])
        .assert()
        .success()
        .stdout("Object locking 'GOVERNANCE' is configured for 1DAYS.\n");
    let info = run_json(&live, &["retention", "info", "--default", &bucket]);
    assert_eq!(info[0]["enabled"], "Enabled");
    assert_eq!(info[0]["mode"], "GOVERNANCE");
    assert_eq!(info[0]["validity"], "1DAYS");

    // New objects inherit the default retention.
    put(&live, "inherited.txt", "x");
    let object = run_json(&live, &["retention", "info", &live.url("inherited.txt")]);
    assert_eq!(object[0]["mode"], "GOVERNANCE");

    live.cmd()
        .args(["retention", "set", "--default", "compliance", "2y", &bucket])
        .assert()
        .success()
        .stdout("Object locking 'COMPLIANCE' is configured for 2YEARS.\n");
    live.cmd()
        .args(["retention", "clear", "--default", &bucket])
        .assert()
        .success()
        .stdout("Object lock configuration cleared successfully.\n");
    live.cmd()
        .args(["retention", "info", "--default", &bucket])
        .assert()
        .success()
        .stdout("Object locking is not enabled.\n");
    live.cmd()
        .args([
            "retention",
            "set",
            "--default",
            "governance",
            "1d",
            &live.url("obj"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires a bucket target"));
    unlock_all(&live);
}

#[test]
fn live_legalhold() {
    let Some(live) = Live::with_bucket(LOCK) else {
        return;
    };
    put(&live, "dir/a.txt", "a1");
    put(&live, "dir/a.txt", "a2");
    put(&live, "dir/b.txt", "b");

    live.cmd()
        .args(["legalhold", "info", &live.url("dir/a.txt")])
        .assert()
        .success()
        .stdout("[ Not set  ]  a.txt\n");
    live.cmd()
        .args(["legalhold", "set", &live.url("dir/a.txt")])
        .assert()
        .success()
        .stdout("Object legal hold successfully set for `a.txt`.\n");
    live.cmd()
        .args(["legalhold", "info", "-r", &live.url("dir/")])
        .assert()
        .success()
        .stdout("[    ON    ]  a.txt\n[ Not set  ]  b.txt\n");

    let versions = run_json(
        &live,
        &["legalhold", "set", "--versions", &live.url("dir/a.txt")],
    );
    assert_eq!(versions.len(), 2);
    assert!(
        versions
            .iter()
            .all(|m| m["legalhold"] == "ON" && m["versionID"] != "")
    );
    let first_version = versions[1]["versionID"].as_str().unwrap().to_string();
    let info = run_json(
        &live,
        &[
            "legalhold",
            "info",
            "--vid",
            &first_version,
            &live.url("dir/a.txt"),
        ],
    );
    assert_eq!(info[0]["legalhold"], "ON");
    assert_eq!(info[0]["versionID"], first_version.as_str());

    // A held version cannot be deleted.
    live.cmd()
        .args(["undo", &live.url("dir/a.txt")])
        .assert()
        .failure();

    let cleared = run_json(
        &live,
        &["legalhold", "clear", "-r", "--versions", &live.url("dir/")],
    );
    assert_eq!(cleared.len(), 3);
    assert!(cleared.iter().all(|m| m["legalhold"] == "OFF"));
    let info = run_json(
        &live,
        &["legalhold", "info", "-r", "--versions", &live.url("")],
    );
    assert!(info.iter().all(|m| m["legalhold"] == "OFF"));

    // Buckets without object lock are rejected.
    let plain = live.make_bucket(BucketOpts::default());
    live.cmd()
        .args(["legalhold", "set", &format!("{}/{plain}/x", live.alias)])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Bucket lock needs to be enabled"));
    live.cmd()
        .args(["retention", "info", &format!("{}/{plain}/x", live.alias)])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not support locking"));
}

#[test]
fn live_event_add_list_remove() {
    let Some(live) = Live::new() else { return };
    let Ok(arn) = std::env::var("MX_TEST_NOTIFY_ARN") else {
        eprintln!("skipping event test; set MX_TEST_NOTIFY_ARN");
        return;
    };
    let bucket = live.bucket_target();
    live.cmd()
        .args(["event", "ls", &bucket])
        .assert()
        .success()
        .stdout("");
    live.cmd()
        .args(["event", "add", &bucket, &arn])
        .assert()
        .success()
        .stdout(format!("Successfully added {arn}\n"));
    let added = run_json(
        &live,
        &[
            "event", "add", &bucket, &arn, "--event", "put", "--prefix", "photos/", "--suffix",
            ".jpg",
        ],
    );
    assert_eq!(added[0]["event"], serde_json::json!(["put"]));
    assert_eq!(added[0]["prefix"], "photos/");

    // Overlapping configuration fails unless -p is given.
    live.cmd()
        .args(["event", "add", &bucket, &arn, "--event", "get"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Overlapping Queue configs"));
    live.cmd()
        .args(["event", "add", "-p", &bucket, &arn, "--event", "get"])
        .assert()
        .success();

    let listed = run_json(&live, &["event", "list", &bucket, &arn]);
    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed[0]["event"],
        serde_json::json!([
            "s3:ObjectCreated:*",
            "s3:ObjectRemoved:*",
            "s3:ObjectAccessed:*"
        ])
    );
    assert_eq!(listed[1]["suffix"], ".jpg");
    live.cmd()
        .args(["event", "ls", &bucket])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "{arn}   s3:ObjectCreated:*   Filter: prefix=\"photos/\"suffix=\".jpg\""
        )));
    live.cmd()
        .args(["event", "ls", &bucket, "arn:minio:sqs::OTHER:webhook"])
        .assert()
        .success()
        .stdout("");

    // Unknown targets are rejected by the server.
    live.cmd()
        .args(["event", "add", &bucket, "arn:minio:sqs::MISSING:webhook"])
        .assert()
        .failure();

    // Remove only the filtered configuration, then everything.
    live.cmd()
        .args([
            "event", "rm", &bucket, &arn, "--event", "put", "--prefix", "photos/", "--suffix",
            ".jpg",
        ])
        .assert()
        .success()
        .stdout(format!("Successfully removed {arn}\n"));
    assert_eq!(run_json(&live, &["event", "ls", &bucket]).len(), 1);
    live.cmd()
        .args(["event", "rm", &bucket, &arn, "--event", "delete"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no notification configuration matched",
        ));
    live.cmd()
        .args([
            "event", "add", &bucket, &arn, "--event", "delete", "--prefix", "x/",
        ])
        .assert()
        .success();
    live.cmd()
        .args(["event", "remove", &bucket, &arn])
        .assert()
        .success();
    assert!(run_json(&live, &["event", "ls", &bucket]).is_empty());
    live.cmd()
        .args(["event", "add", &bucket, &arn])
        .assert()
        .success();
    live.cmd()
        .args(["event", "rm", "--force", &bucket])
        .assert()
        .success();
    assert!(run_json(&live, &["event", "ls", &bucket]).is_empty());
}

#[test]
fn live_undo_put_and_delete() {
    let Some(live) = Live::with_bucket(VERSIONED) else {
        return;
    };
    put(&live, "u.txt", "v1");
    put(&live, "u.txt", "v2");

    live.cmd()
        .args(["undo", "--dry-run", &live.url("u.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains("Last upload of `u.txt` (vid="));
    live.cmd()
        .args(["cat", &live.url("u.txt")])
        .assert()
        .success()
        .stdout("v2");

    live.cmd()
        .args(["undo", &live.url("u.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{2713} Last upload of `u.txt`"));
    live.cmd()
        .args(["cat", &live.url("u.txt")])
        .assert()
        .success()
        .stdout("v1");

    live.cmd()
        .args(["rm", &live.url("u.txt")])
        .assert()
        .success();
    live.cmd()
        .args(["undo", "--action", "PUT", &live.url("u.txt")])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Unable to find any object version to undo",
        ));
    let undone = run_json(&live, &["undo", "--action", "delete", &live.url("u.txt")]);
    assert_eq!(undone[0]["isDeleteMarker"], true);
    assert_eq!(undone[0]["key"], "u.txt");
    live.cmd()
        .args(["cat", &live.url("u.txt")])
        .assert()
        .success()
        .stdout("v1");

    // Recursive undo with --last removes several versions per key.
    put(&live, "p/a", "a1");
    put(&live, "p/a", "a2");
    put(&live, "p/b", "b1");
    let undone = run_json(
        &live,
        &["undo", "-r", "--force", "--last", "2", &live.url("p/")],
    );
    assert_eq!(undone.len(), 3);
    for key in ["p/a", "p/b"] {
        live.cmd().args(["cat", &live.url(key)]).assert().failure();
    }

    // Unversioned buckets are rejected.
    let plain = live.make_bucket(BucketOpts::default());
    live.cmd()
        .args(["undo", &format!("{}/{plain}/x", live.alias)])
        .assert()
        .failure()
        .stderr(predicate::str::contains("versioned-enabled buckets"));
}

#[test]
fn live_od_upload_download() {
    let Some(live) = Live::new() else { return };
    const MIB: usize = 1024 * 1024;
    let data: Vec<u8> = (0..12 * MIB + 123).map(|i| (i * 7 % 251) as u8).collect();
    let source = live.home.path().join("od-source.bin");
    std::fs::write(&source, &data).unwrap();
    let input = format!("if={}", source.display());

    live.cmd()
        .args(["od", &input, &format!("of={}", live.url("full.bin"))])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "Transferred: 12 MiB, Parts: 1, Time: ",
        ));

    let upload = run_json(
        &live,
        &[
            "od",
            &input,
            &format!("of={}", live.url("parts.bin")),
            "size=5MiB",
            "parts=2",
        ],
    );
    assert_eq!(upload[0]["type"], "FStoS3");
    assert_eq!(upload[0]["totalSize"], 10 * MIB);
    assert_eq!(upload[0]["parts"], 2);
    let skipped = run_json(
        &live,
        &[
            "od",
            &input,
            &format!("of={}", live.url("skip.bin")),
            "size=5MiB",
            "skip=1",
        ],
    );
    assert_eq!(skipped[0]["totalSize"], data.len() - 5 * MIB);
    assert_eq!(skipped[0]["skip"], 1);

    // Download the whole object and part by part.
    let full = live.home.path().join("full.out");
    live.cmd()
        .args([
            "od",
            &format!("if={}", live.url("full.bin")),
            &format!("of={}", full.display()),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Full file"));
    assert_eq!(std::fs::read(&full).unwrap(), data);
    let parts = live.home.path().join("parts.out");
    let download = run_json(
        &live,
        &[
            "od",
            &format!("if={}", live.url("parts.bin")),
            &format!("of={}", parts.display()),
            "parts=2",
        ],
    );
    assert_eq!(download[0]["type"], "S3toFS");
    assert_eq!(download[0]["parts"], 2);
    assert_eq!(std::fs::read(&parts).unwrap(), &data[..10 * MIB]);
    let second = live.home.path().join("second.out");
    live.cmd()
        .args([
            "od",
            &format!("if={}", live.url("parts.bin")),
            &format!("of={}", second.display()),
            "parts=2",
            "skip=1",
        ])
        .assert()
        .success();
    assert_eq!(std::fs::read(&second).unwrap(), &data[5 * MIB..10 * MIB]);

    // S3 -> S3 and a small single-PUT upload.
    let copy = run_json(
        &live,
        &[
            "od",
            &format!("if={}", live.url("skip.bin")),
            &format!("of={}", live.url("copy.bin")),
        ],
    );
    assert_eq!(copy[0]["type"], "S3toS3");
    assert_eq!(copy[0]["totalSize"], data.len() - 5 * MIB);
    live.cmd()
        .args([
            "od",
            &input,
            &format!("of={}", live.url("small.bin")),
            "size=1MiB",
            "parts=3",
        ])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "Transferred: 3.0 MiB, Parts: 3",
        ));
    let small = live.home.path().join("small.out");
    live.cmd()
        .args(["get", &live.url("small.bin"), small.to_str().unwrap()])
        .assert()
        .success();
    assert_eq!(std::fs::read(&small).unwrap(), &data[..3 * MIB]);
}

#[test]
fn live_ilm_restore_non_transitioned_object() {
    let Some(live) = Live::new() else { return };
    put(&live, "cold.txt", "not transitioned");
    live.cmd()
        .args(["ilm", "restore", "--days", "2", &live.url("cold.txt")])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "Sent restore requests to 0 object(s)",
        ))
        .stderr(predicate::str::contains("Unable to send restore request"))
        .stderr(predicate::str::contains("InvalidObjectState"));
    let output = live
        .cmd()
        .args(["--json", "ilm", "restore", "-r", &live.url("")])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let json = json_lines(&output.stdout);
    assert_eq!(json[0]["status"], "failure");
    assert_eq!(json[0]["restored"], 0);
}
