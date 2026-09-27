//! Live `cp`/`mv` tests against S3/MinIO. Skipped unless MX_LIVE_TESTS=1
//! (run with `tests/live_minio.sh live_cp`).

mod common;

use aws_sdk_s3::types::{ObjectLockLegalHoldStatus, ServerSideEncryption};
use common::live::{self, BucketOpts, Live};
use mx::s3::{self, GetOptions, ObjectRef, PutOptions};
use predicates::prelude::*;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("runtime")
}

fn s(path: &Path) -> String {
    path.to_str().expect("utf8 path").to_string()
}

fn write(path: &Path, contents: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn client(live: &Live) -> aws_sdk_s3::Client {
    rt().block_on(s3::build_client(&live.alias_config()))
        .expect("client")
}

/// Sorted absolute keys under `prefix` in `bucket`.
fn keys(client: &aws_sdk_s3::Client, bucket: &str, prefix: &str) -> Vec<String> {
    let rt = rt();
    let mut keys: Vec<String> = rt
        .block_on(s3::list_objects_with(
            client,
            bucket,
            (!prefix.is_empty()).then_some(prefix),
            &s3::ListOptions {
                recursive: true,
                ..Default::default()
            },
        ))
        .expect("list")
        .into_iter()
        .map(|info| s3::full_key(prefix, &info.key))
        .collect();
    keys.sort();
    keys
}

fn body(client: &aws_sdk_s3::Client, bucket: &str, key: &str) -> Vec<u8> {
    let rt = rt();
    rt.block_on(async {
        s3::get_object(client, bucket, key, &GetOptions::default())
            .await
            .expect("get")
            .body
            .collect()
            .await
            .expect("body")
            .into_bytes()
            .to_vec()
    })
}

fn head(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> aws_sdk_s3::operation::head_object::HeadObjectOutput {
    rt().block_on(s3::head_object_with(
        client,
        bucket,
        key,
        &GetOptions::default(),
    ))
    .expect("head")
}

fn local_tree(root: &Path) {
    write(&root.join("a.txt"), b"alpha");
    write(&root.join("sub/b.txt"), b"bravo");
    write(&root.join("sub/deep/c.txt"), b"charlie");
}

#[test]
fn live_cp_recursive_in_every_direction() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("src");
    local_tree(&src);

    // local -> S3: `src` (no slash) keeps the folder name.
    live.cmd()
        .args(["cp", "-r", &s(&src), &live.url("up/")])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "-> `{}`",
            live.url("up/src/a.txt")
        )));
    assert_eq!(
        keys(&client, &live.bucket, "up/"),
        ["up/src/a.txt", "up/src/sub/b.txt", "up/src/sub/deep/c.txt"]
    );

    // S3 -> local: trailing slash copies the contents.
    let down = work.path().join("down");
    live.cmd()
        .args(["cp", "-r", &live.url("up/src/"), &s(&down)])
        .assert()
        .success();
    assert_eq!(std::fs::read(down.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(
        std::fs::read(down.join("sub/deep/c.txt")).unwrap(),
        b"charlie"
    );

    // S3 -> S3 on the same server (server-side copy), with --json.
    let other = live.make_bucket(BucketOpts::default());
    let assert = live
        .cmd()
        .args([
            "--json",
            "cp",
            "-r",
            "--max-workers",
            "2",
            &live.url("up/src"),
            &format!("{}/{other}/copy/", live.alias),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let messages: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(messages.len(), 4);
    assert!(messages[..3].iter().all(|m| m["totalCount"] == 3));
    assert!(
        messages[..3]
            .iter()
            .any(|m| m["target"] == format!("{}/{other}/copy/src/sub/b.txt", live.alias))
    );
    assert_eq!(messages[3]["transferred"], 17);
    assert_eq!(
        keys(&client, &other, "copy/"),
        [
            "copy/src/a.txt",
            "copy/src/sub/b.txt",
            "copy/src/sub/deep/c.txt"
        ]
    );

    // Recursive copy of a whole bucket without trailing slash keeps the bucket name.
    let whole = work.path().join("whole");
    live.cmd()
        .args(["cp", "-r", &format!("{}/{other}", live.alias), &s(&whole)])
        .assert()
        .success();
    assert!(whole.join(format!("{other}/copy/src/a.txt")).is_file());

    // Copying a folder into itself is refused.
    live.cmd()
        .args(["cp", "-r", &live.url("up/"), &live.url("up/nested/")])
        .assert()
        .failure()
        .stderr(predicate::str::contains("into itself"));

    // S3 -> S3 across servers (streamed).
    if let Some(bucket2) = live.make_bucket2(BucketOpts::default()) {
        let alias2 = live.alias2.clone().unwrap();
        live.cmd()
            .args(["cp", "-r", &live.url("up/"), &format!("{alias2}/{bucket2}")])
            .assert()
            .success();
        let client2 = rt()
            .block_on(s3::build_client(&live::alias_config_from_home(
                live.home.path(),
                &alias2,
            )))
            .unwrap();
        assert_eq!(
            keys(&client2, &bucket2, ""),
            ["src/a.txt", "src/sub/b.txt", "src/sub/deep/c.txt"]
        );
        assert_eq!(body(&client2, &bucket2, "src/sub/b.txt"), b"bravo");
    }

    // Multiple S3 sources into a local folder.
    let multi = work.path().join("multi/");
    live.cmd()
        .args([
            "cp",
            &live.url("up/src/a.txt"),
            &live.url("up/src/sub/b.txt"),
            &format!("{}/", s(&multi)),
        ])
        .assert()
        .success();
    assert!(multi.join("a.txt").is_file() && multi.join("b.txt").is_file());

    // A prefix without -r is refused; a missing object is reported.
    live.cmd()
        .args(["cp", &live.url("up/src"), &s(&work.path().join("x"))])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--recursive flag is required"));
    live.cmd()
        .args(["cp", &live.url("missing.txt"), &s(&work.path().join("x"))])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
}

#[test]
fn live_cp_single_object_target_rules() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let file = live.local_file("one.txt", "one");
    live.cmd()
        .args(["cp", &s(&file), &live.bucket_target()])
        .assert()
        .success();
    live.cmd()
        .args(["cp", &s(&file), &live.url("named.txt")])
        .assert()
        .success();
    live.cmd()
        .args(["cp", &s(&file), &live.url("dir/")])
        .assert()
        .success();
    // An existing prefix without trailing slash is treated as a folder (mc isAliasURLDir).
    live.cmd()
        .args(["cp", &live.url("one.txt"), &live.url("dir")])
        .assert()
        .success();
    assert_eq!(
        keys(&client, &live.bucket, ""),
        ["dir/one.txt", "named.txt", "one.txt"]
    );
}

#[test]
fn live_cp_metadata_flags() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let file = live.local_file("meta.txt", "metadata body");
    live.cmd()
        .args([
            "cp",
            "--attr",
            "Cache-Control=max-age=60;Content-Type=text/x-mx;owner=alice",
            "--tags",
            "env=prod&team=core",
            "-sc",
            "REDUCED_REDUNDANCY",
            "--checksum",
            "SHA256",
            &s(&file),
            &live.url("meta.txt"),
        ])
        .assert()
        .success();
    let object = head(&client, &live.bucket, "meta.txt");
    assert_eq!(object.cache_control(), Some("max-age=60"));
    assert_eq!(object.content_type(), Some("text/x-mx"));
    assert_eq!(
        object.metadata().unwrap().get("owner").map(String::as_str),
        Some("alice")
    );
    assert_eq!(
        object.storage_class().map(|c| c.as_str()),
        Some("REDUCED_REDUNDANCY")
    );
    let rt = rt();
    let tags = rt
        .block_on(
            client
                .get_object_tagging()
                .bucket(&live.bucket)
                .key("meta.txt")
                .send(),
        )
        .unwrap();
    let mut tags: Vec<_> = tags
        .tag_set()
        .iter()
        .map(|t| format!("{}={}", t.key(), t.value()))
        .collect();
    tags.sort();
    assert_eq!(tags, ["env=prod", "team=core"]);
    let checksum = rt
        .block_on(
            client
                .head_object()
                .bucket(&live.bucket)
                .key("meta.txt")
                .checksum_mode(aws_sdk_s3::types::ChecksumMode::Enabled)
                .send(),
        )
        .unwrap();
    assert!(checksum.checksum_sha256().is_some());

    // S3 -> S3 with --attr merges into the source metadata.
    live.cmd()
        .args([
            "cp",
            "--attr",
            "team=storage",
            &live.url("meta.txt"),
            &live.url("merged.txt"),
        ])
        .assert()
        .success();
    let merged = head(&client, &live.bucket, "merged.txt");
    let metadata = merged.metadata().unwrap();
    assert_eq!(metadata.get("owner").map(String::as_str), Some("alice"));
    assert_eq!(metadata.get("team").map(String::as_str), Some("storage"));
    assert_eq!(merged.content_type(), Some("text/x-mx"));

    // --disable-multipart uploads a 9 MiB file with a single PUT (no `-N` ETag suffix).
    let big: Vec<u8> = (0..9 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let big_path = live.home.path().join("big.bin");
    std::fs::write(&big_path, &big).unwrap();
    live.cmd()
        .args(["cp", &s(&big_path), &live.url("multi.bin")])
        .assert()
        .success();
    live.cmd()
        .args([
            "cp",
            "--disable-multipart",
            &s(&big_path),
            &live.url("single.bin"),
        ])
        .assert()
        .success();
    let multi_etag = head(&client, &live.bucket, "multi.bin")
        .e_tag()
        .unwrap()
        .to_string();
    let single_etag = head(&client, &live.bucket, "single.bin")
        .e_tag()
        .unwrap()
        .to_string();
    assert!(multi_etag.contains('-'), "{multi_etag}");
    assert!(!single_etag.contains('-'), "{single_etag}");
    assert_eq!(body(&client, &live.bucket, "single.bin"), big);
}

#[cfg(unix)]
#[test]
fn live_cp_preserve_roundtrip() {
    use std::os::unix::fs::PermissionsExt;
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let file = live.local_file("keep.sh", "#!/bin/sh\n");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o750)).unwrap();
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(old)
                .set_accessed(old),
        )
        .unwrap();
    live.cmd()
        .args(["cp", "-a", &s(&file), &live.url("keep.sh")])
        .assert()
        .success();
    let object = head(&client, &live.bucket, "keep.sh");
    let attrs = object.metadata().unwrap().get("mc-attrs").unwrap().clone();
    assert!(attrs.contains("mode:33256"), "{attrs}");
    assert!(attrs.contains("mtime:1600000000#0"), "{attrs}");

    let out = live.home.path().join("restored.sh");
    live.cmd()
        .args(["cp", "--preserve", &live.url("keep.sh"), &s(&out)])
        .assert()
        .success();
    let meta = std::fs::metadata(&out).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o750);
    assert_eq!(meta.modified().unwrap(), old);
}

// xattrs are uploaded on Linux only, like mc.
#[cfg(target_os = "linux")]
#[test]
fn live_cp_preserve_xattrs_and_stream_copy_tags() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let file = live.local_file("attrs.txt", "x\n");
    // mc uploads extended attributes (except `system.*`) as user metadata with `-a`.
    if xattr::set(&file, "user.color", b"blue").is_ok() {
        live.cmd()
            .args(["cp", "-a", &s(&file), &live.url("attrs.txt")])
            .assert()
            .success();
        let object = head(&client, &live.bucket, "attrs.txt");
        assert_eq!(
            object
                .metadata()
                .unwrap()
                .get("user.color")
                .map(String::as_str),
            Some("blue")
        );
        // Binary (non-UTF-8) values are hex-encoded like mc.
        let binary = live.local_file("binattrs.txt", "x\n");
        xattr::set(&binary, "user.bin", &[0x00, 0xff, 0x0a, 0x41]).unwrap();
        live.cmd()
            .args(["cp", "-a", &s(&binary), &live.url("binattrs.txt")])
            .assert()
            .success();
        let object = head(&client, &live.bucket, "binattrs.txt");
        assert_eq!(
            object
                .metadata()
                .unwrap()
                .get("user.bin")
                .map(String::as_str),
            Some("00ff0a41")
        );
        live.cmd()
            .args(["cp", &s(&file), &live.url("plain.txt")])
            .assert()
            .success();
        let object = head(&client, &live.bucket, "plain.txt");
        assert!(
            object
                .metadata()
                .is_none_or(|m| !m.contains_key("user.color"))
        );
    } else {
        eprintln!("skipping xattr check: filesystem without user xattrs");
    }

    // Cross-server (streamed) copies keep the source tags only with `-a`, like mc.
    let Some(bucket2) = live.make_bucket2(BucketOpts::default()) else {
        eprintln!("skipping stream copy tags; set MX_TEST_URL2");
        return;
    };
    let alias2 = live.alias2.clone().unwrap();
    live.cmd()
        .args([
            "cp",
            "--tags",
            "a=1&b=2",
            &s(&file),
            &live.url("tagged.txt"),
        ])
        .assert()
        .success();
    let dst = |key: &str| format!("{alias2}/{bucket2}/{key}");
    live.cmd()
        .args(["cp", &live.url("tagged.txt"), &dst("plain.txt")])
        .assert()
        .success();
    live.cmd()
        .args(["cp", "-a", &live.url("tagged.txt"), &dst("kept.txt")])
        .assert()
        .success();
    let client2 = rt()
        .block_on(s3::build_client(&live::alias_config_from_home(
            live.home.path(),
            &alias2,
        )))
        .expect("client2");
    let tags = |key: &str| {
        let mut tags: Vec<(String, String)> = rt()
            .block_on(s3::object_tags(&client2, &bucket2, key, None))
            .expect("tags")
            .into_iter()
            .collect();
        tags.sort();
        tags
    };
    assert!(tags("plain.txt").is_empty());
    assert_eq!(
        tags("kept.txt"),
        vec![("a".into(), "1".into()), ("b".into(), "2".into())]
    );
}

fn rfc3339(time: SystemTime) -> String {
    aws_sdk_s3::primitives::DateTime::from(time)
        .fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
        .unwrap()
}

#[test]
fn live_cp_version_id_and_rewind() {
    let Some(live) = Live::with_bucket(BucketOpts {
        versioning: true,
        lock: false,
    }) else {
        return;
    };
    let client = client(&live);
    let v1 = live.local_file("v1.txt", "version one");
    let v2 = live.local_file("v2.txt", "version two");
    live.cmd()
        .args(["cp", &s(&v1), &live.url("doc.txt")])
        .assert()
        .success();
    std::thread::sleep(Duration::from_millis(1500));
    let between = SystemTime::now();
    std::thread::sleep(Duration::from_millis(1500));
    live.cmd()
        .args(["cp", &s(&v2), &live.url("doc.txt")])
        .assert()
        .success();
    live.cmd()
        .args(["cp", &s(&v2), &live.url("later.txt")])
        .assert()
        .success();

    let versions = rt()
        .block_on(s3::list_object_versions(&client, &live.bucket, None, true))
        .unwrap();
    let first = versions
        .iter()
        .find(|v| v.key == "doc.txt" && !v.is_latest)
        .and_then(|v| v.version_id.clone())
        .expect("old version");

    let out = live.home.path().join("by-vid.txt");
    live.cmd()
        .args(["cp", "--vid", &first, &live.url("doc.txt"), &s(&out)])
        .assert()
        .success();
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "version one");

    // S3 -> S3 copy of an old version.
    live.cmd()
        .args([
            "cp",
            "--version-id",
            &first,
            &live.url("doc.txt"),
            &live.url("restored.txt"),
        ])
        .assert()
        .success();
    assert_eq!(body(&client, &live.bucket, "restored.txt"), b"version one");

    // Single-object rewind.
    let out = live.home.path().join("rewound.txt");
    live.cmd()
        .args([
            "cp",
            "--rewind",
            &rfc3339(between),
            &live.url("doc.txt"),
            &s(&out),
        ])
        .assert()
        .success();
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "version one");

    // Recursive rewind: `later.txt` did not exist yet.
    let dir = live.home.path().join("rewound-dir");
    live.cmd()
        .args([
            "cp",
            "-r",
            "--rewind",
            &rfc3339(between),
            &format!("{}/", live.bucket_target()),
            &s(&dir),
        ])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dir.join("doc.txt")).unwrap(),
        "version one"
    );
    assert!(!dir.join("later.txt").exists());
}

#[test]
fn live_cp_object_lock_flags() {
    let Some(live) = Live::with_bucket(BucketOpts {
        versioning: false,
        lock: true,
    }) else {
        return;
    };
    let client = client(&live);
    let file = live.local_file("locked.txt", "locked");
    live.cmd()
        .args(["cp", "--legal-hold", "on", &s(&file), &live.url("held.txt")])
        .assert()
        .success();
    live.cmd()
        .args([
            "cp",
            "--retention-mode",
            "governance",
            "--retention-duration",
            "1d",
            &s(&file),
            &live.url("retained.txt"),
        ])
        .assert()
        .success();
    let rt = rt();
    let hold = rt
        .block_on(
            client
                .get_object_legal_hold()
                .bucket(&live.bucket)
                .key("held.txt")
                .send(),
        )
        .unwrap();
    assert_eq!(
        hold.legal_hold().and_then(|h| h.status()),
        Some(&ObjectLockLegalHoldStatus::On)
    );
    let retention = rt
        .block_on(
            client
                .get_object_retention()
                .bucket(&live.bucket)
                .key("retained.txt")
                .send(),
        )
        .unwrap();
    let retention = retention.retention().unwrap();
    assert_eq!(retention.mode().map(|m| m.as_str()), Some("GOVERNANCE"));
    let until = SystemTime::try_from(*retention.retain_until_date().unwrap()).unwrap();
    let delta = until
        .duration_since(SystemTime::now())
        .unwrap_or_default()
        .as_secs();
    assert!((86_000..=86_400).contains(&delta), "{delta}");

    // Release the hold so the fixture can clean up.
    rt.block_on(
        client
            .put_object_legal_hold()
            .bucket(&live.bucket)
            .key("held.txt")
            .legal_hold(
                aws_sdk_s3::types::ObjectLockLegalHold::builder()
                    .status(ObjectLockLegalHoldStatus::Off)
                    .build(),
            )
            .customize()
            .config_override(s3::checksum_override())
            .send(),
    )
    .ok();
}

#[test]
fn live_cp_server_side_encryption() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let file = live.local_file("secret.txt", "secret body");
    live.cmd()
        .args([
            "cp",
            "--enc-s3",
            &live.url("s3enc/"),
            &s(&file),
            &live.url("s3enc/secret.txt"),
        ])
        .assert()
        .success();
    assert_eq!(
        head(&client, &live.bucket, "s3enc/secret.txt").server_side_encryption(),
        Some(&ServerSideEncryption::Aes256)
    );

    let key_id = std::env::var("MX_TEST_KMS_KEY_ID").unwrap_or_else(|_| "mx-test-key".into());
    live.cmd()
        .args([
            "cp",
            "-r",
            "--enc-kms",
            &format!("{}={key_id}", live.url("kms")),
            &live.url("s3enc/"),
            &live.url("kms/"),
        ])
        .assert()
        .success();
    let object = head(&client, &live.bucket, "kms/secret.txt");
    assert_eq!(
        object.server_side_encryption(),
        Some(&ServerSideEncryption::AwsKms)
    );
    assert!(object.ssekms_key_id().unwrap_or_default().contains(&key_id));

    let out = live.home.path().join("decrypted.txt");
    live.cmd()
        .args(["cp", &live.url("kms/secret.txt"), &s(&out)])
        .assert()
        .success();
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "secret body");
}

#[test]
fn live_cp_time_filters() {
    let Some(live) = Live::new() else { return };
    let work = tempfile::tempdir().unwrap();
    local_tree(&work.path().join("tree"));
    live.cmd()
        .args([
            "cp",
            "-r",
            &format!("{}/", s(&work.path().join("tree"))),
            &live.url("t/"),
        ])
        .assert()
        .success();

    let older = work.path().join("older");
    live.cmd()
        .args([
            "cp",
            "-r",
            "--older-than",
            "1d",
            &live.url("t/"),
            &s(&older),
        ])
        .assert()
        .success();
    assert!(!older.exists());

    let newer = work.path().join("newer");
    live.cmd()
        .args([
            "cp",
            "-r",
            "--newer-than",
            "1d",
            &live.url("t/"),
            &s(&newer),
        ])
        .assert()
        .success();
    assert!(newer.join("a.txt").is_file());
    assert!(newer.join("sub/deep/c.txt").is_file());
}

#[test]
fn live_mv_recursive() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let work = tempfile::tempdir().unwrap();
    let tree = work.path().join("tree");
    local_tree(&tree);

    live.cmd()
        .args(["mv", "-r", &s(&tree), &live.url("moved/")])
        .assert()
        .success();
    assert!(!tree.exists());
    assert_eq!(
        keys(&client, &live.bucket, "moved/"),
        [
            "moved/tree/a.txt",
            "moved/tree/sub/b.txt",
            "moved/tree/sub/deep/c.txt"
        ]
    );

    let other = live.make_bucket(BucketOpts::default());
    live.cmd()
        .args([
            "mv",
            "--recursive",
            &live.url("moved/tree/"),
            &format!("{}/{other}/", live.alias),
        ])
        .assert()
        .success();
    assert!(keys(&client, &live.bucket, "moved/").is_empty());
    assert_eq!(
        keys(&client, &other, ""),
        ["a.txt", "sub/b.txt", "sub/deep/c.txt"]
    );

    let back = work.path().join("back");
    live.cmd()
        .args([
            "mv",
            "-r",
            &format!("{}/{other}/sub/", live.alias),
            &s(&back),
        ])
        .assert()
        .success();
    assert_eq!(keys(&client, &other, ""), ["a.txt"]);
    assert_eq!(std::fs::read(back.join("deep/c.txt")).unwrap(), b"charlie");

    // Single object move with a new name.
    live.cmd()
        .args([
            "mv",
            &format!("{}/{other}/a.txt", live.alias),
            &live.url("renamed.txt"),
        ])
        .assert()
        .success();
    assert!(keys(&client, &other, "").is_empty());
    assert_eq!(body(&client, &live.bucket, "renamed.txt"), b"alpha");
}

#[test]
fn live_cp_zip_extract() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let zip = stored_zip(&[("inner.txt", b"inside"), ("dir/nested.txt", b"nested")]);
    rt().block_on(s3::put_object_with(
        &client,
        &live.bucket,
        "archive.zip",
        zip,
        &PutOptions::default(),
    ))
    .unwrap();

    let out = live.home.path().join("inner.txt");
    live.cmd()
        .args(["cp", "--zip", &live.url("archive.zip/inner.txt"), &s(&out)])
        .assert()
        .success();
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "inside");

    let dir = live.home.path().join("unzipped");
    live.cmd()
        .args(["cp", "-r", "--zip", &live.url("archive.zip/"), &s(&dir)])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dir.join("dir/nested.txt")).unwrap(),
        "nested"
    );

    // S3 -> S3 with --zip streams the extracted file.
    live.cmd()
        .args([
            "cp",
            "--zip",
            &live.url("archive.zip/dir/nested.txt"),
            &live.url("extracted/"),
        ])
        .assert()
        .success();
    assert_eq!(
        body(&client, &live.bucket, "extracted/nested.txt"),
        b"nested"
    );
}

#[test]
fn live_multipart_copy_api() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let alias = live.alias_config();
    let data: Vec<u8> = (0..1024 * 1024 + 7).map(|i| (i % 253) as u8).collect();
    let rt = rt();
    rt.block_on(s3::put_object_with(
        &client,
        &live.bucket,
        "src.bin",
        data.clone(),
        &PutOptions {
            content_type: Some("application/x-mx".into()),
            metadata: vec![("origin".into(), "one".into())],
            tags: vec![("k".into(), "v".into())],
            ..Default::default()
        },
    ))
    .unwrap();
    let outcome = rt
        .block_on(s3::multipart_copy(
            &client,
            ObjectRef {
                alias: &alias,
                bucket: &live.bucket,
                key: "src.bin",
            },
            ObjectRef {
                alias: &alias,
                bucket: &live.bucket,
                key: "dst.bin",
            },
            &GetOptions::default(),
            &PutOptions::default(),
            data.len() as u64,
        ))
        .unwrap();
    assert!(outcome.etag.unwrap().contains('-'));
    assert_eq!(body(&client, &live.bucket, "dst.bin"), data);
    let copied = head(&client, &live.bucket, "dst.bin");
    assert_eq!(copied.content_type(), Some("application/x-mx"));
    assert_eq!(
        copied.metadata().unwrap().get("origin").map(String::as_str),
        Some("one")
    );
    let tags = rt
        .block_on(
            client
                .get_object_tagging()
                .bucket(&live.bucket)
                .key("dst.bin")
                .send(),
        )
        .unwrap();
    assert_eq!(tags.tag_set().len(), 1);
}

/// Minimal uncompressed (stored) zip archive.
fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let le16 = |v: u16| v.to_le_bytes();
    let le32 = |v: u32| v.to_le_bytes();
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let crc = crc32(data);
        let offset = out.len() as u32;
        out.extend(le32(0x0403_4b50));
        out.extend(le16(20));
        out.extend([0u8; 8]); // flags, method, time, date
        out.extend(le32(crc));
        out.extend(le32(data.len() as u32));
        out.extend(le32(data.len() as u32));
        out.extend(le16(name.len() as u16));
        out.extend(le16(0));
        out.extend(name.as_bytes());
        out.extend(*data);

        central.extend(le32(0x0201_4b50));
        central.extend(le16(20));
        central.extend(le16(20));
        central.extend([0u8; 8]); // flags, method, time, date
        central.extend(le32(crc));
        central.extend(le32(data.len() as u32));
        central.extend(le32(data.len() as u32));
        central.extend(le16(name.len() as u16));
        central.extend([0u8; 12]); // extra, comment, disk, int attrs, ext attrs (4)
        central.extend(le32(offset));
        central.extend(name.as_bytes());
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend(central);
    out.extend(le32(0x0605_4b50));
    out.extend([0u8; 4]);
    out.extend(le16(entries.len() as u16));
    out.extend(le16(entries.len() as u16));
    out.extend(le32(central_size));
    out.extend(le32(central_offset));
    out.extend(le16(0));
    out
}

fn put_raw(live: &Live, key: &str, contents: &str) {
    rt().block_on(s3::put_object_reader_with(
        &live.alias_config(),
        &live.bucket,
        key,
        std::io::Cursor::new(contents.as_bytes().to_vec()),
        None,
        &PutOptions::default(),
    ))
    .unwrap_or_else(|error| panic!("put {key}: {error:?}"));
}

/// Folder-marker objects (`dir/`) never turn into their sibling key (`dir`). (Colliding keys such
/// as `a//b` vs `a/b` are covered by the `colliding_targets_are_rejected` unit test: MinIO
/// rejects such key names.)
#[test]
fn live_cp_mv_folder_markers() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let work = tempfile::tempdir().unwrap();
    for (key, contents) in [
        ("pre", "sibling"),
        ("pre/", ""),
        ("dir/", ""),
        ("dir/a", "alpha"),
    ] {
        put_raw(&live, key, contents);
    }

    // Console-style folder: `dir/` + `dir/a`.
    let out = work.path().join("cpout");
    live.cmd()
        .args(["cp", "-r", &live.url("dir/"), &format!("{}/", s(&out))])
        .assert()
        .success();
    assert_eq!(std::fs::read(out.join("a")).unwrap(), b"alpha");

    // mv of the marker folder `pre/` must not touch the sibling object `pre` (MinIO cannot list
    // children of `pre/` while the object `pre` exists, so the marker is the only entry).
    let moved = work.path().join("mvout");
    live.cmd()
        .args(["mv", "-r", &live.url("pre/"), &format!("{}/", s(&moved))])
        .assert()
        .success();
    assert!(!moved.join("pre").exists());
    assert_eq!(body(&client, &live.bucket, "pre"), b"sibling");
}
