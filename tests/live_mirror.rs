//! Live `mx mirror` tests (MX_LIVE_TESTS=1, see tests/live_minio.sh).

mod common;

use common::live::{BucketOpts, Live, alias_config_from_home};
use predicates::prelude::*;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Local source tree: `a.txt`, `nested/b.txt`, `skip.tmp`.
fn source_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("a.txt"), "alpha");
    write(&dir.path().join("nested/b.txt"), "bravo");
    write(&dir.path().join("skip.tmp"), "temp");
    dir
}

fn path_str(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

fn cat(live: &Live, target: &str) -> Option<String> {
    let output = live.cmd().args(["cat", target]).output().unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

fn stdout(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).to_string()
}

/// HeadObject through the library: (user metadata, server side encryption).
fn head(
    live: &Live,
    alias: &str,
    bucket: &str,
    key: &str,
) -> (std::collections::HashMap<String, String>, Option<String>) {
    let config = alias_config_from_home(live.home.path(), alias);
    mx::commands::runtime().unwrap().block_on(async {
        let client = mx::s3::build_client(&config).await.unwrap();
        let response = client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .unwrap();
        (
            response.metadata().cloned().unwrap_or_default(),
            response
                .server_side_encryption()
                .map(|v| v.as_str().to_string()),
        )
    })
}

fn wait_for(mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

fn spawn_mx(live: &Live, args: &[&str]) -> std::process::Child {
    std::process::Command::new(assert_cmd::cargo::cargo_bin!("mx"))
        .env("HOME", live.home.path())
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn live_mirror_local_to_s3_overwrite_remove_dry_run() {
    let Some(live) = Live::new() else { return };
    let src = source_tree();
    let src_path = path_str(src.path());
    let target = live.url("backup");

    live.cmd()
        .args(["mirror", "--dry-run", &src_path, &target])
        .assert()
        .success()
        // Like mc: no per-object lines, only the summary (dry-run doubles `Transferred`).
        .stdout(predicate::str::contains("a.txt").not())
        .stdout(predicate::str::contains("│ Total │ Transferred │"));
    assert!(cat(&live, &live.url("backup/a.txt")).is_none());

    live.cmd()
        .args(["mirror", "--exclude", "*.tmp", &src_path, &target])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "` -> `{}`",
            live.url("backup/nested/b.txt")
        )));
    assert_eq!(
        cat(&live, &live.url("backup/a.txt")).as_deref(),
        Some("alpha")
    );
    assert_eq!(
        cat(&live, &live.url("backup/nested/b.txt")).as_deref(),
        Some("bravo")
    );
    assert!(cat(&live, &live.url("backup/skip.tmp")).is_none());

    // Unchanged: nothing to do (only the summary table).
    live.cmd()
        .args(["mirror", "--exclude", "*.tmp", &src_path, &target])
        .assert()
        .success()
        .stdout(predicate::str::contains(" -> ").not())
        .stdout(predicate::str::contains("│ 0 B   │ 0 B         │"));

    // Changed size without --overwrite: reported, target untouched.
    write(&src.path().join("a.txt"), "alpha-v2");
    live.cmd()
        .args(["mirror", &src_path, &target])
        .assert()
        .success()
        .stderr(predicate::str::contains("Overwrite not allowed for `"));
    assert_eq!(
        cat(&live, &live.url("backup/a.txt")).as_deref(),
        Some("alpha")
    );
    live.cmd()
        .args(["mirror", "--overwrite", &src_path, &target])
        .assert()
        .success();
    assert_eq!(
        cat(&live, &live.url("backup/a.txt")).as_deref(),
        Some("alpha-v2")
    );

    // --remove deletes extraneous objects.
    std::fs::remove_file(src.path().join("nested/b.txt")).unwrap();
    live.cmd()
        .args(["mirror", &src_path, &target])
        .assert()
        .success();
    assert!(cat(&live, &live.url("backup/nested/b.txt")).is_some());
    live.cmd()
        .args(["mirror", "--remove", "--dry-run", &src_path, &target])
        .assert()
        .success()
        .stdout(predicate::str::contains(" -> ").not());
    assert!(cat(&live, &live.url("backup/nested/b.txt")).is_some());
    live.cmd()
        .args(["mirror", "--remove", &src_path, &target])
        .assert()
        .success()
        // mc prints initial-pass removals without an event type: `` -> `TARGET`.
        .stdout(predicate::str::contains(format!(
            "`` -> `{}`",
            live.url("backup/nested/b.txt")
        )));
    assert!(cat(&live, &live.url("backup/nested/b.txt")).is_none());
}

#[test]
fn live_mirror_upload_flags_attr_preserve_enc_checksum() {
    let Some(live) = Live::new() else { return };
    let src = source_tree();
    let src_path = path_str(src.path());
    live.cmd()
        .args([
            "mirror",
            "-a",
            "--attr",
            "Owner=mx;Cache-Control=max-age=60",
            "--enc-s3",
            &live.url("enc"),
            "--checksum",
            "CRC32C",
            "--disable-multipart",
            "-sc",
            "STANDARD",
            "--max-workers",
            "2",
            &src_path,
            &live.url("enc"),
        ])
        .assert()
        .success();
    let (meta, sse) = head(&live, &live.alias, &live.bucket, "enc/a.txt");
    assert_eq!(meta.get("owner").map(String::as_str), Some("mx"));
    assert!(
        meta.get("mc-attrs").is_some_and(|v| v.contains("mtime:")),
        "{meta:?}"
    );
    assert_eq!(sse.as_deref(), Some("AES256"));
}

#[test]
fn live_mirror_s3_to_local_with_preserve() {
    let Some(live) = Live::new() else { return };
    let src = source_tree();
    let old = SystemTime::now() - Duration::from_secs(5 * 86400);
    std::fs::File::options()
        .write(true)
        .open(src.path().join("a.txt"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    live.cmd()
        .args(["mirror", "-a", &path_str(src.path()), &live.bucket_target()])
        .assert()
        .success();

    let dst = tempfile::tempdir().unwrap();
    let dst_path = path_str(&dst.path().join("restore"));
    live.cmd()
        .args(["mirror", "-a", &live.bucket_target(), &dst_path])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "`{}` -> `",
            live.url("a.txt")
        )));
    let restored = dst.path().join("restore/a.txt");
    assert_eq!(std::fs::read_to_string(&restored).unwrap(), "alpha");
    assert_eq!(
        std::fs::read_to_string(dst.path().join("restore/nested/b.txt")).unwrap(),
        "bravo"
    );
    let restored_mtime = std::fs::metadata(&restored).unwrap().modified().unwrap();
    let delta = restored_mtime
        .duration_since(old)
        .unwrap_or_else(|e| e.duration());
    assert!(delta < Duration::from_secs(1), "mtime not preserved");

    // A prefix source mirrors only that folder; --remove cleans local extras.
    let prefix_dst = dst.path().join("prefix");
    write(&prefix_dst.join("extra.txt"), "x");
    live.cmd()
        .args([
            "mirror",
            "--remove",
            &live.url("nested"),
            &path_str(&prefix_dst),
        ])
        .assert()
        .success();
    assert!(prefix_dst.join("b.txt").is_file());
    assert!(!prefix_dst.join("extra.txt").exists());
    assert!(!prefix_dst.join("a.txt").exists());

    // An object (not a folder) as source is rejected.
    live.cmd()
        .args(["mirror", &live.url("a.txt"), &dst_path])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not a folder"));
}

#[test]
fn live_mirror_s3_to_s3_same_and_cross_server() {
    let Some(live) = Live::new() else { return };
    let src = source_tree();
    live.cmd()
        .args(["mirror", &path_str(src.path()), &live.bucket_target()])
        .assert()
        .success();

    // Same server, with metadata added.
    let other = live.make_bucket(BucketOpts::default());
    let other_target = format!("{}/{other}", live.alias);
    live.cmd()
        .args([
            "mirror",
            "--attr",
            "Team=storage",
            "--exclude",
            "*.tmp",
            &live.bucket_target(),
            &other_target,
        ])
        .assert()
        .success();
    assert_eq!(
        cat(&live, &format!("{other_target}/nested/b.txt")).as_deref(),
        Some("bravo")
    );
    assert!(cat(&live, &format!("{other_target}/skip.tmp")).is_none());
    let (meta, _) = head(&live, &live.alias, &other, "a.txt");
    assert_eq!(meta.get("team").map(String::as_str), Some("storage"));

    // Storage class and time filters.
    let filtered = live.make_bucket(BucketOpts::default());
    let filtered_target = format!("{}/{filtered}", live.alias);
    for flags in [
        ["--older-than", "1d"],
        ["--exclude-storageclass", "STANDARD"],
    ] {
        live.cmd()
            .arg("mirror")
            .args(flags)
            .args([&live.bucket_target(), &filtered_target])
            .assert()
            .success()
            .stdout(predicate::str::contains(" -> ").not());
    }
    live.cmd()
        .args([
            "mirror",
            "--newer-than",
            "1d",
            &live.bucket_target(),
            &filtered_target,
        ])
        .assert()
        .success();
    assert!(cat(&live, &format!("{filtered_target}/a.txt")).is_some());

    // Cross server (streamed copy).
    let Some(bucket2) = live.make_bucket2(BucketOpts::default()) else {
        return;
    };
    let alias2 = live.alias2.clone().unwrap();
    let target2 = format!("{alias2}/{bucket2}/copy");
    let assert = live
        .cmd()
        .args([
            "--json",
            "mirror",
            "--summary",
            &live.bucket_target(),
            &target2,
        ])
        .assert()
        .success();
    let summary: serde_json::Value = serde_json::from_str(stdout(&assert).trim()).unwrap();
    assert_eq!(summary["total"], 14);
    assert_eq!(summary["transferred"], 14);
    assert_eq!(
        cat(&live, &format!("{target2}/nested/b.txt")).as_deref(),
        Some("bravo")
    );

    // JSON per-object output and --remove across servers.
    live.cmd()
        .args(["rm", &live.url("skip.tmp")])
        .assert()
        .success();
    let assert = live
        .cmd()
        .args([
            "mirror",
            "--json",
            "--remove",
            &live.bucket_target(),
            &target2,
        ])
        .assert()
        .success();
    let out = stdout(&assert);
    let docs: Vec<serde_json::Value> = out
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(docs.len(), 2, "{out}");
    let line = &docs[0];
    assert_eq!(line["status"], "success");
    assert_eq!(line["source"], "");
    assert_eq!(line["target"], format!("{target2}/skip.tmp"));
    assert_eq!(line["eventType"], "");
    assert_eq!(docs[1]["total"], 0);
    assert!(cat(&live, &format!("{target2}/skip.tmp")).is_none());
}

/// `GET /metrics` from the `--monitoring-address` endpoint (None while it is not up).
fn scrape(address: &str) -> Option<String> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(address).ok()?;
    stream
        .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    Some(response)
}

#[test]
fn live_mirror_monitoring_address_serves_metrics() {
    let Some(live) = Live::new() else { return };
    let src = source_tree();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let address = format!("127.0.0.1:{port}");
    let mut child = spawn_mx(
        &live,
        &[
            "mirror",
            "--watch",
            "--watch-interval",
            "1s",
            "--monitoring-address",
            &address,
            &path_str(src.path()),
            &live.bucket_target(),
        ],
    );
    let mut metrics = String::new();
    let scraped = wait_for(|| {
        metrics = scrape(&address).unwrap_or_default();
        metrics.contains("mc_mirror_total_s3uploaded_bytes 14")
    });
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(scraped, "{metrics}");
    assert!(metrics.starts_with("HTTP/1.1 200 OK"), "{metrics}");
    for name in [
        "mc_mirror_total_s3ops",
        "mc_mirror_failed_s3ops 0",
        "mc_mirror_total_restarts 0",
        "mc_mirror_replication_duration_count{object_size=\"LESS_THAN_1_KiB\"}",
    ] {
        assert!(metrics.contains(name), "{name}: {metrics}");
    }
}

/// Hidden global `--conn-read-deadline` / `--conn-write-deadline` bound S3 requests.
#[test]
fn live_conn_deadlines_apply() {
    let Some(live) = Live::new() else { return };
    let src = source_tree();
    let target = live.url("deadline");
    live.cmd()
        .args([
            "--conn-read-deadline",
            "1ns",
            "mirror",
            &path_str(src.path()),
            &target,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("i/o timeout"));
    live.cmd()
        .args([
            "--conn-read-deadline",
            "10m",
            "--conn-write-deadline",
            "10m",
            "mirror",
            &path_str(src.path()),
            &target,
        ])
        .assert()
        .success();
    assert_eq!(
        cat(&live, &format!("{target}/a.txt")).as_deref(),
        Some("alpha")
    );
}

#[test]
fn live_mirror_watch_uploads_new_files() {
    let Some(live) = Live::new() else { return };
    let src = source_tree();
    let mut child = spawn_mx(
        &live,
        &[
            "mirror",
            "--watch",
            "--watch-interval",
            "1s",
            &path_str(src.path()),
            &live.bucket_target(),
        ],
    );
    let initial = wait_for(|| cat(&live, &live.url("a.txt")).is_some());
    write(&src.path().join("later/new.txt"), "fresh");
    let added = wait_for(|| cat(&live, &live.url("later/new.txt")).as_deref() == Some("fresh"));
    std::fs::remove_file(src.path().join("a.txt")).unwrap();
    let removed = wait_for(|| cat(&live, &live.url("a.txt")).is_none());
    let running = child.try_wait().unwrap().is_none();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(initial && added && removed && running);
}

#[test]
fn live_mirror_active_active_sets_source_mtime() {
    let Some(live) = Live::new() else { return };
    let src = source_tree();
    live.cmd()
        .args(["mirror", &path_str(src.path()), &live.bucket_target()])
        .assert()
        .success();
    let other = live.make_bucket(BucketOpts::default());
    let other_target = format!("{}/{other}", live.alias);
    let mut child = spawn_mx(
        &live,
        &[
            "mirror",
            "--active-active",
            "--watch-interval",
            "1s",
            &live.bucket_target(),
            &other_target,
        ],
    );
    let copied = wait_for(|| cat(&live, &format!("{other_target}/nested/b.txt")).is_some());
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(copied);
    let (meta, _) = head(&live, &live.alias, &other, "a.txt");
    assert!(meta.contains_key("mm-source-mtime"), "{meta:?}");
}

/// Alias root -> alias root. Only against the throwaway servers of tests/live_minio.sh: it
/// mirrors every bucket (and with --remove deletes extraneous objects, never buckets or versions).
#[test]
fn live_mirror_alias_root_to_second_server() {
    let Some(live) = Live::new() else { return };
    let url = std::env::var("MX_TEST_URL").unwrap_or_default();
    if !(url.contains("127.0.0.1") || url.contains("localhost")) {
        eprintln!("skipping alias-root mirror test: not a local test server");
        return;
    }
    let Some(alias2) = live.alias2.clone() else {
        return;
    };
    let src = source_tree();
    live.cmd()
        .args(["mirror", &path_str(src.path()), &live.bucket_target()])
        .assert()
        .success();
    let locked = live.make_bucket(BucketOpts {
        versioning: false,
        lock: true,
    });
    let excluded = live.make_bucket(BucketOpts::default());
    live.cmd()
        .args([
            "cp",
            &path_str(&src.path().join("a.txt")),
            &format!("{}/{excluded}/x.txt", live.alias),
        ])
        .assert()
        .success();
    let extra2 = live
        .make_bucket2(BucketOpts {
            versioning: true,
            lock: false,
        })
        .unwrap();
    live.cmd()
        .args([
            "cp",
            &path_str(&src.path().join("a.txt")),
            &format!("{alias2}/{extra2}/keep.txt"),
        ])
        .assert()
        .success();

    live.cmd()
        .args([
            "mirror",
            "-a",
            "--remove",
            "--exclude-bucket",
            &excluded,
            &live.alias,
            &alias2,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "`{}/{}` -> `{alias2}/{}`",
            live.alias, live.bucket, live.bucket
        )))
        .stdout(predicate::str::contains(format!(
            "`` -> `{alias2}/{extra2}/keep.txt`"
        )));

    assert_eq!(
        cat(&live, &format!("{alias2}/{}/nested/b.txt", live.bucket)).as_deref(),
        Some("bravo")
    );
    let buckets2 = live.cmd().args(["ls", &alias2]).output().unwrap();
    let buckets2 = String::from_utf8_lossy(&buckets2.stdout).to_string();
    assert!(buckets2.contains(&locked), "{buckets2}");
    assert!(!buckets2.contains(&excluded), "{buckets2}");
    // Like mc, a bucket missing on the source is kept; only its objects are removed (a delete
    // marker on top of the preserved version).
    assert!(buckets2.contains(&extra2), "{buckets2}");
    let versions = live
        .cmd()
        .args(["ls", "--versions", &format!("{alias2}/{extra2}")])
        .output()
        .unwrap();
    let versions = String::from_utf8_lossy(&versions.stdout).to_string();
    assert!(versions.contains(" v1 PUT "), "{versions}");
    assert!(versions.contains(" v2 DEL "), "{versions}");

    // -a copied the object lock configuration.
    let config2 = alias_config_from_home(live.home.path(), &alias2);
    let lock_enabled = mx::commands::runtime().unwrap().block_on(async {
        let client = mx::s3::build_client(&config2).await.unwrap();
        client
            .get_object_lock_configuration()
            .bucket(&locked)
            .send()
            .await
            .is_ok()
    });
    assert!(lock_enabled);

    // Alias root -> local folder.
    let dst = tempfile::tempdir().unwrap();
    live.cmd()
        .args([
            "mirror",
            "--exclude-bucket",
            &excluded,
            &alias2,
            &path_str(dst.path()),
        ])
        .assert()
        .success();
    assert!(dst.path().join(&live.bucket).join("a.txt").is_file());

    for bucket in [&live.bucket, &locked, &extra2] {
        let _ = live
            .cmd()
            .args(["rb", "--force", &format!("{alias2}/{bucket}")])
            .output();
    }
}
