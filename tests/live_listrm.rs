//! Live tests for rm/ls/stat/du/tree/rb (area C). Skipped unless MX_LIVE_TESTS=1.

mod common;

use aws_sdk_s3::primitives::{DateTime, DateTimeFormat};
use common::live::{BucketOpts, Live};
use mx::s3::{self, PutOptions};
use predicates::prelude::*;
use serde_json::Value;
use std::time::{Duration, SystemTime};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("runtime")
}

fn pause() {
    std::thread::sleep(Duration::from_millis(1100));
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::from(time)
        .fmt(DateTimeFormat::DateTime)
        .expect("format time")
}

/// Parses a stream of (possibly pretty-printed) JSON documents.
fn json_docs(output: &[u8]) -> Vec<Value> {
    serde_json::Deserializer::from_slice(output)
        .into_iter::<Value>()
        .map(|value| value.expect("json"))
        .collect()
}

fn stdout(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).expect("utf8")
}

struct Fixture {
    live: Live,
    alias: mx::config::model::AliasConfig,
    rt: tokio::runtime::Runtime,
    client: aws_sdk_s3::Client,
}

impl Fixture {
    fn new(opts: BucketOpts) -> Option<Self> {
        let live = Live::with_bucket(opts)?;
        let alias = live.alias_config();
        let rt = rt();
        let client = rt.block_on(s3::build_client(&alias)).expect("client");
        Some(Self {
            live,
            alias,
            rt,
            client,
        })
    }

    fn put(&self, key: &str, body: &str) -> Option<String> {
        self.put_with(key, body, &PutOptions::default())
    }

    fn put_with(&self, key: &str, body: &str, options: &PutOptions) -> Option<String> {
        self.rt
            .block_on(s3::put_object_reader_with(
                &self.alias,
                &self.live.bucket,
                key,
                std::io::Cursor::new(body.as_bytes().to_vec()),
                None,
                options,
            ))
            .unwrap_or_else(|error| panic!("put {key}: {error:?}"))
            .version_id
    }

    fn versions(&self, key: &str) -> Vec<s3::ObjectInfo> {
        self.rt
            .block_on(s3::list_key_versions(&self.client, &self.live.bucket, key))
            .expect("versions")
    }

    fn exists(&self, key: &str) -> bool {
        self.rt
            .block_on(
                self.client
                    .head_object()
                    .bucket(&self.live.bucket)
                    .key(key)
                    .send(),
            )
            .is_ok()
    }

    fn url(&self, path: &str) -> String {
        self.live.url(path)
    }
}

#[test]
fn live_ls_stat_du_tree_versions_and_rewind() {
    let Some(fx) = Fixture::new(BucketOpts {
        versioning: true,
        lock: false,
    }) else {
        return;
    };
    let live = &fx.live;
    let a1 = fx.put("a.txt", "one").unwrap();
    pause();
    let t1 = SystemTime::now();
    pause();
    let a2 = fx.put("a.txt", "two!").unwrap();
    fx.put("dir/b.txt", "bee");
    pause();
    let t2 = SystemTime::now();
    pause();
    live.cmd()
        .args(["rm", &fx.url("dir/b.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Created delete marker `{}` (versionId=",
            fx.url("dir/b.txt")
        )));

    // ls --versions: ordinals, PUT/DEL, JSON fields.
    let out = stdout(
        live.cmd()
            .args(["ls", "-r", "--versions", &live.bucket_target()])
            .assert()
            .success(),
    );
    assert!(out.contains(&format!("{a2} v2 PUT")), "{out}");
    assert!(out.contains(&format!("{a1} v1 PUT")), "{out}");
    assert!(out.contains(" v2 DEL"), "{out}");
    let docs = json_docs(
        &live
            .cmd()
            .args(["--json", "ls", "-r", "--versions", &live.bucket_target()])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    let a_versions: Vec<_> = docs.iter().filter(|doc| doc["name"] == "a.txt").collect();
    assert_eq!(a_versions.len(), 2);
    assert_eq!(a_versions[0]["versionId"], a2.as_str());
    assert_eq!(a_versions[0]["versionOrdinal"], 2);
    assert_eq!(a_versions[1]["versionOrdinal"], 1);
    assert!(
        docs.iter()
            .any(|doc| doc["name"] == "dir/b.txt" && doc["isDeleteMarker"] == true)
    );

    // ls --rewind: state at t1 (only a.txt v1) and t2 (a.txt v2 + dir/b.txt).
    let docs = json_docs(
        &live
            .cmd()
            .args([
                "--json",
                "ls",
                "-r",
                "--rewind",
                &rfc3339(t1),
                &live.bucket_target(),
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(docs.len(), 1, "{docs:?}");
    assert_eq!(docs[0]["versionId"], a1.as_str());
    assert_eq!(docs[0]["size"], 3);
    let out = stdout(
        live.cmd()
            .args(["ls", "-r", "--rewind", &rfc3339(t2), &live.bucket_target()])
            .assert()
            .success(),
    );
    assert!(out.contains("a.txt") && out.contains("dir/b.txt"), "{out}");

    // ls --summarize and --storage-class filter.
    live.cmd()
        .args(["ls", "-r", "--summarize", &live.bucket_target()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Total Size: 4 B\nTotal Objects: 1",
        ));
    let docs = json_docs(
        &live
            .cmd()
            .args(["--json", "ls", "-r", "--summarize", &live.bucket_target()])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    let summary = docs.last().unwrap();
    assert_eq!(summary["totalObjects"], 1);
    assert_eq!(summary["totalSize"], 4);
    live.cmd()
        .args(["ls", "-r", "-sc", "GLACIER", &live.bucket_target()])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.txt").not());
    live.cmd()
        .args([
            "ls",
            "-r",
            "--storage-class",
            "STANDARD",
            &live.bucket_target(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.txt"));

    // stat variants.
    let out = stdout(
        live.cmd()
            .args(["stat", &fx.url("a.txt")])
            .assert()
            .success(),
    );
    assert!(out.contains("Name      : a.txt\n"), "{out}");
    assert!(out.contains("Size      : 4 B    \n"), "{out}");
    assert!(out.contains(&format!("VersionID : {a2} \n")), "{out}");
    assert!(out.contains("Type      : file \n"), "{out}");
    assert!(out.contains("Metadata  :\n  Content-Type: "), "{out}");
    let doc: Value = serde_json::from_slice(
        &live
            .cmd()
            .args(["--json", "stat", "--vid", &a1, &fx.url("a.txt")])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(doc["size"], 3);
    assert_eq!(doc["versionID"], a1.as_str());
    assert_eq!(doc["name"], "a.txt");
    let out = stdout(
        live.cmd()
            .args(["stat", "--versions", &fx.url("a.txt")])
            .assert()
            .success(),
    );
    assert_eq!(out.matches("VersionID :").count(), 2, "{out}");
    let doc: Value = serde_json::from_slice(
        &live
            .cmd()
            .args(["--json", "stat", "--rewind", &rfc3339(t1), &fx.url("a.txt")])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(doc["size"], 3);
    live.cmd()
        .args(["stat", "--no-list", &fx.url("a.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains("Size      : 4 B"));
    let out = stdout(
        live.cmd()
            .args(["stat", "-r", &format!("{}/", live.bucket_target())])
            .assert()
            .success(),
    );
    assert!(
        out.contains("Name      : a.txt") && !out.contains("b.txt"),
        "{out}"
    );
    let out = stdout(
        live.cmd()
            .args(["stat", &live.bucket_target()])
            .assert()
            .success(),
    );
    assert!(
        out.contains(&format!("Name      : {}\n", live.bucket)),
        "{out}"
    );
    assert!(out.contains("  Versioning: Enabled\n"), "{out}");
    assert!(out.contains("  Location: "), "{out}");
    let doc: Value = serde_json::from_slice(
        &live
            .cmd()
            .args(["--json", "stat", &live.bucket_target()])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(doc["Versioning"]["status"], "Enabled");
    live.cmd()
        .args(["stat", &fx.url("missing.txt")])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unable to stat"));

    // du --versions / --rewind (delete markers never count).
    live.cmd()
        .args(["du", "-r", "--versions", &live.bucket_target()])
        .assert()
        .success()
        .stdout(predicate::str::contains("10  3 versions"));
    live.cmd()
        .args(["du", "-r", "--rewind", &rfc3339(t1), &live.bucket_target()])
        .assert()
        .success()
        .stdout(predicate::str::contains("3  1 objects"));
    let doc: Value = serde_json::from_slice(
        &live
            .cmd()
            .args(["--json", "du", "-r", "--versions", &live.bucket_target()])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .unwrap();
    assert_eq!(doc["isVersions"], true);

    // tree --rewind shows the deleted object as it was at t2.
    live.cmd()
        .args([
            "tree",
            "--files",
            "--rewind",
            &rfc3339(t2),
            &live.bucket_target(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("b.txt"));
    live.cmd()
        .args(["tree", "--files", &live.bucket_target()])
        .assert()
        .success()
        .stdout(predicate::str::contains("b.txt").not());
}

#[test]
fn live_rm_versions_non_current_and_rewind() {
    let Some(fx) = Fixture::new(BucketOpts {
        versioning: true,
        lock: false,
    }) else {
        return;
    };
    let live = &fx.live;
    let v1 = fx.put("k.txt", "1").unwrap();
    let v2 = fx.put("k.txt", "22").unwrap();
    let v3 = fx.put("k.txt", "333").unwrap();

    live.cmd()
        .args(["rm", "--version-id", &v1, &fx.url("k.txt")])
        .assert()
        .success()
        .stdout(format!("Removed `{}` (versionId={v1}).\n", fx.url("k.txt")));
    assert_eq!(fx.versions("k.txt").len(), 2);

    // Dry run leaves everything in place and prints modTime for version removals.
    live.cmd()
        .args(["rm", "--versions", "--force", "--dry-run", &fx.url("k.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "DRYRUN: Removing `{}` (versionId={v3}) (modTime=",
            fx.url("k.txt")
        )));
    assert_eq!(fx.versions("k.txt").len(), 2);

    // --non-current keeps only the latest version.
    let docs = json_docs(
        &live
            .cmd()
            .args([
                "--json",
                "rm",
                "--versions",
                "--non-current",
                "-r",
                "--force",
                &live.bucket_target(),
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(docs.len(), 1, "{docs:?}");
    assert_eq!(docs[0]["status"], "success");
    assert_eq!(docs[0]["key"], fx.url("k.txt"));
    assert_eq!(docs[0]["versionID"], v2.as_str());
    assert_eq!(docs[0]["deleteMarker"], false);
    assert_eq!(docs[0]["dryRun"], false);
    let left = fx.versions("k.txt");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].version_id.as_deref(), Some(v3.as_str()));

    // rm --versions --force KEY removes every version.
    live.cmd()
        .args(["rm", "--versions", "--force", &fx.url("k.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("(versionId={v3})")));
    assert!(fx.versions("k.txt").is_empty());

    // --rewind: removes the version that was current at that time.
    let r1 = fx.put("rw/r.txt", "old").unwrap();
    pause();
    let at = rfc3339(SystemTime::now());
    pause();
    let r2 = fx.put("rw/r.txt", "new").unwrap();
    live.cmd()
        .args(["rm", "-r", "--force", "--rewind", &at, &fx.url("rw/")])
        .assert()
        .success()
        .stdout(format!(
            "Removed `{}` (versionId={r1}).\n",
            fx.url("rw/r.txt")
        ));
    let left = fx.versions("rw/r.txt");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].version_id.as_deref(), Some(r2.as_str()));

    // --versions --rewind: every version at or before the time.
    let o1 = fx.put("old/o.txt", "a").unwrap();
    let o2 = fx.put("old/o.txt", "b").unwrap();
    pause();
    let at = rfc3339(SystemTime::now());
    pause();
    let o3 = fx.put("old/o.txt", "c").unwrap();
    let out = stdout(
        live.cmd()
            .args([
                "rm",
                "-r",
                "--versions",
                "--force",
                "--rewind",
                &at,
                &fx.url("old/"),
            ])
            .assert()
            .success(),
    );
    assert!(
        out.contains(&o1) && out.contains(&o2) && !out.contains(&o3),
        "{out}"
    );
    assert_eq!(fx.versions("old/o.txt").len(), 1);

    // Plain recursive rm on a versioned bucket creates delete markers.
    live.cmd()
        .args(["rm", "-r", "--force", &fx.url("old/")])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Created delete marker `{}` (versionId=",
            fx.url("old/o.txt")
        )));
    assert!(!fx.exists("old/o.txt"));
}

#[test]
fn live_rm_filters_stdin_dry_run_and_errors() {
    let Some(fx) = Fixture::new(BucketOpts::default()) else {
        return;
    };
    let live = &fx.live;
    fx.put("f/old.txt", "old");
    std::thread::sleep(Duration::from_secs(3));
    fx.put("f/new.txt", "new");

    let out = stdout(
        live.cmd()
            .args(["rm", "-r", "--force", "--dry-run", &fx.url("f/")])
            .assert()
            .success(),
    );
    assert!(
        out.contains(&format!("DRYRUN: Removing `{}`.", fx.url("f/old.txt")))
            && out.contains(&format!("DRYRUN: Removing `{}`.", fx.url("f/new.txt"))),
        "{out}"
    );
    assert!(fx.exists("f/old.txt") && fx.exists("f/new.txt"));

    // Single-object time filters skip silently.
    live.cmd()
        .args(["rm", "--older-than", "1h", &fx.url("f/old.txt")])
        .assert()
        .success()
        .stdout("");
    assert!(fx.exists("f/old.txt"));

    live.cmd()
        .args(["rm", "-r", "--force", "--newer-than", "2s", &fx.url("f/")])
        .assert()
        .success()
        .stdout(format!("Removed `{}`.\n", fx.url("f/new.txt")));
    assert!(fx.exists("f/old.txt") && !fx.exists("f/new.txt"));
    live.cmd()
        .args(["rm", "-r", "--force", "--older-than", "2s", &fx.url("f/")])
        .assert()
        .success()
        .stdout(format!("Removed `{}`.\n", fx.url("f/old.txt")));
    assert!(!fx.exists("f/old.txt"));

    // --stdin and multiple targets.
    for key in ["s1", "s2", "m1", "m2"] {
        fx.put(key, key);
    }
    live.cmd()
        .args(["rm", "--force", "--stdin"])
        .write_stdin(format!("{}\n\n{}\n", fx.url("s1"), fx.url("s2")))
        .assert()
        .success()
        .stdout(format!(
            "Removed `{}`.\nRemoved `{}`.\n",
            fx.url("s1"),
            fx.url("s2")
        ));
    live.cmd()
        .args(["rm", &fx.url("m1"), &fx.url("m2")])
        .assert()
        .success();
    for key in ["s1", "s2", "m1", "m2"] {
        assert!(!fx.exists(key), "{key} still exists");
    }

    live.cmd()
        .args(["rm", &fx.url("missing.txt")])
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "Failed to remove `{}`.",
            fx.url("missing.txt")
        )));
}

#[test]
fn live_rm_and_ls_incomplete_uploads() {
    let Some(fx) = Fixture::new(BucketOpts::default()) else {
        return;
    };
    let live = &fx.live;
    let create = |key: &str| {
        fx.rt
            .block_on(
                fx.client
                    .create_multipart_upload()
                    .bucket(&live.bucket)
                    .key(key)
                    .send(),
            )
            .expect("create upload");
    };
    create("mp/big.bin");
    create("single.bin");
    let uploads = || {
        fx.rt
            .block_on(s3::list_incomplete_uploads(
                &fx.client,
                &live.bucket,
                None,
                true,
            ))
            .expect("uploads")
    };
    assert_eq!(uploads().len(), 2);

    live.cmd()
        .args(["ls", "-I", "-r", &live.bucket_target()])
        .assert()
        .success()
        .stdout(predicate::str::contains("mp/big.bin"));

    live.cmd()
        .args(["rm", "-I", "-r", "--force", "--dry-run", &fx.url("mp/")])
        .assert()
        .success()
        .stdout(format!("DRYRUN: Removing `{}`.\n", fx.url("mp/big.bin")));
    assert_eq!(uploads().len(), 2);
    live.cmd()
        .args(["rm", "-I", "-r", "--force", &fx.url("mp/")])
        .assert()
        .success()
        .stdout(format!("Removed `{}`.\n", fx.url("mp/big.bin")));
    live.cmd()
        .args(["rm", "-I", &fx.url("single.bin")])
        .assert()
        .success()
        .stdout(format!("Removed `{}`.\n", fx.url("single.bin")));
    assert!(uploads().is_empty());
}

/// `rm -r` on folder markers (`pre/`, `dir/`) never touches the sibling key; `rm -r` on a plain
/// object removes it, and a missing target is an error.
#[test]
fn live_rm_recursive_folder_markers_and_objects() {
    let Some(fx) = Fixture::new(BucketOpts::default()) else {
        return;
    };
    let live = &fx.live;
    // MinIO cannot list `pre/...` children while the object `pre` exists: marker + sibling only.
    for key in ["pre", "pre/", "dir/", "dir/a", "file"] {
        fx.put(key, "x");
    }
    live.cmd()
        .args(["rm", "-r", "--force", &fx.url("pre/")])
        .assert()
        .success();
    assert!(fx.exists("pre"));
    assert!(!fx.exists("pre/"));

    live.cmd()
        .args(["rm", "-r", "--force", &fx.url("dir")])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Removed `{}`.",
            fx.url("dir/")
        )));
    assert!(!fx.exists("dir/") && !fx.exists("dir/a"));

    live.cmd()
        .args(["rm", "-r", "--force", &fx.url("file")])
        .assert()
        .success()
        .stdout(format!("Removed `{}`.\n", fx.url("file")));
    assert!(!fx.exists("file"));
    live.cmd()
        .args(["rm", "-r", "--force", &fx.url("missing")])
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "Failed to remove `{}`",
            fx.url("missing")
        )));
    assert!(fx.exists("pre"));
}

#[test]
fn live_rm_bypass_governance_and_rb_force() {
    let Some(fx) = Fixture::new(BucketOpts {
        versioning: false,
        lock: true,
    }) else {
        return;
    };
    let live = &fx.live;
    let until = SystemTime::now() + Duration::from_secs(3600);
    let version = fx
        .put_with(
            "locked.txt",
            "locked",
            &PutOptions {
                retention: Some(("governance".into(), until)),
                ..Default::default()
            },
        )
        .expect("version id");

    live.cmd()
        .args(["rm", "--version-id", &version, &fx.url("locked.txt")])
        .assert()
        .failure();
    assert_eq!(fx.versions("locked.txt").len(), 1);
    // rb --force reports the per-object failure and keeps the bucket (no MinIO force-delete
    // fallback unless the bucket is merely not empty).
    live.cmd()
        .args(["rb", "--force", &live.bucket_target()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("`locked.txt` (versionId="));
    assert_eq!(fx.versions("locked.txt").len(), 1);
    live.cmd()
        .args([
            "rm",
            "--version-id",
            &version,
            "--bypass",
            &fx.url("locked.txt"),
        ])
        .assert()
        .success()
        .stdout(format!(
            "Removed `{}` (versionId={version}).\n",
            fx.url("locked.txt")
        ));
    assert!(fx.versions("locked.txt").is_empty());

    // rb --force on the (now empty) object-lock bucket.
    live.cmd()
        .args(["rb", "--force", &live.bucket_target()])
        .assert()
        .success()
        .stdout(format!("Bucket `{}` removed successfully.\n", live.bucket));
}

#[test]
fn live_rb_rules_and_site_wide_removal() {
    let Some(fx) = Fixture::new(BucketOpts {
        versioning: true,
        lock: false,
    }) else {
        return;
    };
    let live = &fx.live;
    fx.put("a.txt", "a");
    fx.put("a.txt", "b");
    live.cmd().args(["rm", &fx.url("a.txt")]).assert().success();

    live.cmd()
        .args(["rb", &live.bucket_target()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "`{}` is not empty. Retry this command with ‘--force’ flag",
            live.bucket_target()
        )));
    let empty = live.make_bucket(BucketOpts::default());
    live.cmd()
        .args([
            "rb",
            "--force",
            &live.bucket_target(),
            &format!("{}/{empty}", live.alias),
        ])
        .assert()
        .success()
        .stdout(format!(
            "Bucket `{}` removed successfully.\nBucket `{empty}` removed successfully.\n",
            live.bucket
        ));
    // Missing bucket with --force is a no-op.
    live.cmd()
        .args(["rb", "--force", &live.bucket_target()])
        .assert()
        .success()
        .stdout("");

    // Site-wide operations run against the second server only.
    let Some(alias2) = live.alias2.clone() else {
        return;
    };
    let b1 = live.make_bucket2(BucketOpts::default()).unwrap();
    let b2 = live.make_bucket2(BucketOpts::default()).unwrap();
    let file = live.local_file("x.txt", "x");
    for bucket in [&b1, &b2] {
        live.cmd()
            .args([
                "put",
                file.to_str().unwrap(),
                &format!("{alias2}/{bucket}/x.txt"),
            ])
            .assert()
            .success();
    }
    live.cmd()
        .args(["rm", "-r", "--force", &alias2])
        .assert()
        .failure()
        .stderr(predicate::str::contains("‘--dangerous’"));
    let out = stdout(
        live.cmd()
            .args(["rm", "-r", "--force", "--dangerous", &alias2])
            .assert()
            .success(),
    );
    assert!(
        out.contains(&format!("Removed `{alias2}/{b1}/x.txt`."))
            && out.contains(&format!("Removed `{alias2}/{b2}/x.txt`.")),
        "{out}"
    );
    live.cmd()
        .args(["rb", "--force", "--dangerous", &alias2])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("Bucket `{b1}` removed")))
        .stdout(predicate::str::contains(format!("Bucket `{b2}` removed")));
    live.cmd()
        .args(["ls", &alias2])
        .assert()
        .success()
        .stdout(predicate::str::contains(&b1).not());
}

/// A zip archive with one stored (uncompressed) file.
fn stored_zip(name: &str, body: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in data {
            crc ^= *byte as u32;
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
    let crc = crc32(body);
    let mut local = Vec::new();
    local.extend(0x0403_4b50u32.to_le_bytes());
    local.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // version, flags, method, time, date
    local.extend(crc.to_le_bytes());
    local.extend((body.len() as u32).to_le_bytes());
    local.extend((body.len() as u32).to_le_bytes());
    local.extend((name.len() as u16).to_le_bytes());
    local.extend(0u16.to_le_bytes());
    local.extend(name.as_bytes());
    local.extend(body);
    let mut central = Vec::new();
    central.extend(0x0201_4b50u32.to_le_bytes());
    central.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // made by, needed, flags, method, time, date
    central.extend(crc.to_le_bytes());
    central.extend((body.len() as u32).to_le_bytes());
    central.extend((body.len() as u32).to_le_bytes());
    central.extend((name.len() as u16).to_le_bytes());
    central.extend([0u8; 12]); // extra, comment, disk, internal attr (2+2+2+2) + external attr (4)
    central.extend(0u32.to_le_bytes()); // local header offset
    central.extend(name.as_bytes());
    let mut out = local.clone();
    let central_offset = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]); // disk numbers
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(central_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

#[test]
fn live_ls_zip_lists_archive_contents() {
    let Some(fx) = Fixture::new(BucketOpts::default()) else {
        return;
    };
    let live = &fx.live;
    let path = live.home.path().join("archive.zip");
    std::fs::write(&path, stored_zip("inner.txt", b"hello zip")).unwrap();
    live.cmd()
        .args(["put", path.to_str().unwrap(), &fx.url("archive.zip")])
        .assert()
        .success();
    live.cmd()
        .args(["ls", "-r", "--zip", &fx.url("archive.zip")])
        .assert()
        .success()
        .stdout(predicate::str::contains("inner.txt"));
    live.cmd()
        .args(["ls", "-r", &fx.url("archive.zip")])
        .assert()
        .success()
        .stdout(predicate::str::contains("inner.txt").not());
}
