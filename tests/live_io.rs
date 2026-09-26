//! Live tests for cat, head, get, put, pipe and find. Skipped unless MX_LIVE_TESTS=1
//! (run with `tests/live_minio.sh live_io`).

mod common;

use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::{DateTime, DateTimeFormat};
use aws_sdk_s3::types::{ChecksumMode, ServerSideEncryption};
use common::live::{BucketOpts, Live};
use predicates::prelude::*;
use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant, SystemTime};

const MIB: usize = 1024 * 1024;

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

struct Sdk {
    rt: tokio::runtime::Runtime,
    client: Client,
}

impl Sdk {
    fn new(live: &Live) -> Self {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = rt
            .block_on(mx::s3::build_client(&live.alias_config()))
            .unwrap();
        Self { rt, client }
    }

    fn head(
        &self,
        bucket: &str,
        key: &str,
    ) -> aws_sdk_s3::operation::head_object::HeadObjectOutput {
        self.rt
            .block_on(
                self.client
                    .head_object()
                    .bucket(bucket)
                    .key(key)
                    .checksum_mode(ChecksumMode::Enabled)
                    .send(),
            )
            .unwrap()
    }

    /// Version IDs of `key`, oldest first.
    fn versions(&self, bucket: &str, key: &str) -> Vec<String> {
        let response = self
            .rt
            .block_on(
                self.client
                    .list_object_versions()
                    .bucket(bucket)
                    .prefix(key)
                    .send(),
            )
            .unwrap();
        let mut versions: Vec<_> = response
            .versions()
            .iter()
            .filter(|v| v.key() == Some(key))
            .map(|v| {
                (
                    *v.last_modified().unwrap(),
                    v.version_id().unwrap().to_string(),
                )
            })
            .collect();
        versions.sort_by_key(|(time, _)| time.as_nanos());
        versions.into_iter().map(|(_, id)| id).collect()
    }

    fn tags(&self, bucket: &str, key: &str) -> Vec<(String, String)> {
        let response = self
            .rt
            .block_on(
                self.client
                    .get_object_tagging()
                    .bucket(bucket)
                    .key(key)
                    .send(),
            )
            .unwrap();
        let mut tags: Vec<_> = response
            .tag_set()
            .iter()
            .map(|t| (t.key().to_string(), t.value().to_string()))
            .collect();
        tags.sort();
        tags
    }
}

fn pipe(live: &Live, args: &[&str], target: &str, body: impl Into<Vec<u8>>) {
    live.cmd()
        .arg("pipe")
        .args(args)
        .arg(target)
        .write_stdin(body.into())
        .assert()
        .success();
}

fn stdout(live: &Live, args: &[&str]) -> Vec<u8> {
    let output = live.cmd().args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn sorted_lines(live: &Live, args: &[&str]) -> Vec<String> {
    let mut lines: Vec<String> = String::from_utf8(stdout(live, args))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::from(time).fmt(DateTimeFormat::DateTime).unwrap()
}

#[test]
fn live_cat_head_get_versions_and_ranges() {
    let Some(live) = Live::with_bucket(BucketOpts {
        versioning: true,
        lock: false,
    }) else {
        return;
    };
    let sdk = Sdk::new(&live);
    let before = SystemTime::now() - Duration::from_secs(60);
    let object = live.url("doc.txt");
    pipe(&live, &[], &object, "version-one\nline2\nline3\n");
    std::thread::sleep(Duration::from_millis(1500));
    let middle = rfc3339(SystemTime::now());
    std::thread::sleep(Duration::from_millis(1500));
    pipe(&live, &[], &object, "version-two\nsecond\n");
    let versions = sdk.versions(&live.bucket, "doc.txt");
    assert_eq!(versions.len(), 2);
    let v1 = versions[0].as_str();

    // cat: offset, tail, version, rewind, multiple targets.
    assert_eq!(
        stdout(&live, &["cat", "--offset", "8", &object]),
        b"two\nsecond\n"
    );
    assert_eq!(stdout(&live, &["cat", "--tail", "7", &object]), b"second\n");
    assert_eq!(
        stdout(&live, &["cat", "--tail", "1000", &object]),
        b"version-two\nsecond\n"
    );
    assert_eq!(
        stdout(&live, &["cat", "--vid", v1, &object]),
        b"version-one\nline2\nline3\n"
    );
    assert_eq!(
        stdout(&live, &["cat", "-vid", v1, "--tail", "6", &object]),
        b"line3\n"
    );
    assert_eq!(
        stdout(&live, &["cat", "--rewind", &middle, &object]),
        b"version-one\nline2\nline3\n"
    );
    live.cmd()
        .args(["cat", "--rewind", &rfc3339(before), &object])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
    live.cmd()
        .args(["cat", "--offset", "100", &object])
        .assert()
        .failure()
        .stderr(predicate::str::contains("bigger than file"));
    let local = live.local_file("local.txt", "local\n");
    assert_eq!(
        stdout(&live, &["cat", &object, local.to_str().unwrap(), &object]),
        b"version-two\nsecond\nlocal\nversion-two\nsecond\n"
    );

    // head: lines, version, rewind.
    assert_eq!(
        stdout(&live, &["head", "-n", "1", &object]),
        b"version-two\n"
    );
    assert_eq!(
        stdout(&live, &["head", "-n", "2", "--version-id", v1, &object]),
        b"version-one\nline2\n"
    );
    assert_eq!(
        stdout(
            &live,
            &["head", "--lines", "1", "--rewind", &middle, &object]
        ),
        b"version-one\n"
    );

    // get: version.
    let out = live.home.path().join("v1.txt");
    live.cmd()
        .args(["get", "--vid", v1, &object, out.to_str().unwrap()])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "version-one\nline2\nline3\n"
    );
    live.cmd()
        .args(["get", &object, out.to_str().unwrap()])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "version-two\nsecond\n"
    );
}

#[test]
fn live_put_multipart_storage_checksum_encryption() {
    let Some(live) = Live::new() else { return };
    let sdk = Sdk::new(&live);
    let big = pattern(11 * MIB + 123);
    let big_path = live.home.path().join("big.bin");
    std::fs::write(&big_path, &big).unwrap();
    let big_str = big_path.to_str().unwrap();

    // -P / -s: 5 MiB parts -> 3 parts.
    live.cmd()
        .args(["put", "-P", "2", "-s", "5MiB", big_str, &live.url("mp.bin")])
        .assert()
        .success();
    let head = sdk.head(&live.bucket, "mp.bin");
    assert_eq!(head.content_length(), Some(big.len() as i64));
    assert!(
        head.e_tag().unwrap().ends_with("-3\""),
        "{:?}",
        head.e_tag()
    );
    assert_eq!(stdout(&live, &["cat", &live.url("mp.bin")]), big);
    // cat --part-number reads a single part.
    assert_eq!(
        stdout(&live, &["cat", "--part-number", "3", &live.url("mp.bin")]),
        &big[10 * MIB..]
    );

    // Default part size (16MiB) -> single PUT for an 11 MiB file; --disable-multipart too.
    live.cmd()
        .args(["put", big_str, &live.url("default.bin")])
        .assert()
        .success();
    assert!(
        !sdk.head(&live.bucket, "default.bin")
            .e_tag()
            .unwrap()
            .contains('-')
    );
    live.cmd()
        .args([
            "put",
            "--disable-multipart",
            "-s",
            "5MiB",
            big_str,
            &live.url("single.bin"),
        ])
        .assert()
        .success();
    let head = sdk.head(&live.bucket, "single.bin");
    assert!(!head.e_tag().unwrap().contains('-'), "{:?}", head.e_tag());
    assert_eq!(head.content_length(), Some(big.len() as i64));

    // -sc
    let small = live.local_file("small.txt", "small\n");
    let small_str = small.to_str().unwrap();
    live.cmd()
        .args([
            "put",
            "-sc",
            "REDUCED_REDUNDANCY",
            small_str,
            &live.url("rrs.txt"),
        ])
        .assert()
        .success();
    assert_eq!(
        sdk.head(&live.bucket, "rrs.txt")
            .storage_class()
            .map(|c| c.as_str().to_string())
            .as_deref(),
        Some("REDUCED_REDUNDANCY")
    );

    // --checksum (single and multipart)
    live.cmd()
        .args([
            "put",
            "--checksum",
            "CRC32C",
            small_str,
            &live.url("crc.txt"),
        ])
        .assert()
        .success();
    assert!(
        sdk.head(&live.bucket, "crc.txt")
            .checksum_crc32_c()
            .is_some()
    );
    live.cmd()
        .args([
            "put",
            "--checksum",
            "SHA256",
            "-s",
            "5MiB",
            big_str,
            &live.url("crc.bin"),
        ])
        .assert()
        .success();
    assert!(
        sdk.head(&live.bucket, "crc.bin")
            .checksum_sha256()
            .is_some()
    );

    // --enc-s3 / --enc-kms
    live.cmd()
        .args([
            "put",
            "--enc-s3",
            &live.bucket_target(),
            small_str,
            &live.url("sse-s3.txt"),
        ])
        .assert()
        .success();
    assert_eq!(
        sdk.head(&live.bucket, "sse-s3.txt")
            .server_side_encryption(),
        Some(&ServerSideEncryption::Aes256)
    );
    if let Ok(key_id) = std::env::var("MX_TEST_KMS_KEY_ID") {
        live.cmd()
            .args([
                "put",
                "--enc-kms",
                &format!("{}={key_id}", live.bucket_target()),
                small_str,
                &live.url("sse-kms.txt"),
            ])
            .assert()
            .success();
        let head = sdk.head(&live.bucket, "sse-kms.txt");
        assert_eq!(
            head.server_side_encryption(),
            Some(&ServerSideEncryption::AwsKms)
        );
        assert_eq!(
            stdout(&live, &["cat", &live.url("sse-kms.txt")]),
            b"small\n"
        );
    }

    // --if-not-exists (single PUT and multipart)
    live.cmd()
        .args(["put", "--if-not-exists", small_str, &live.url("once.txt")])
        .assert()
        .success();
    live.cmd()
        .args(["put", "--if-not-exists", small_str, &live.url("once.txt")])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "pre-conditions you specified did not hold",
        ));
    live.cmd()
        .args([
            "put",
            "--if-not-exists",
            "-s",
            "5MiB",
            big_str,
            &live.url("mp.bin"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "pre-conditions you specified did not hold",
        ));
    live.cmd()
        .args([
            "put",
            "--if-not-exists",
            "-s",
            "5MiB",
            big_str,
            &live.url("mp-new.bin"),
        ])
        .assert()
        .success();
    assert!(
        sdk.head(&live.bucket, "mp-new.bin")
            .e_tag()
            .unwrap()
            .contains('-')
    );

    // multiple sources into a prefix; stdin source.
    let other = live.local_file("other.txt", "other\n");
    live.cmd()
        .args([
            "put",
            small_str,
            other.to_str().unwrap(),
            &live.url("multi/"),
        ])
        .assert()
        .success();
    assert_eq!(
        stdout(&live, &["cat", &live.url("multi/other.txt")]),
        b"other\n"
    );
    assert_eq!(
        stdout(&live, &["cat", &live.url("multi/small.txt")]),
        b"small\n"
    );
    live.cmd()
        .args(["put", "-", &live.url("stdin.txt")])
        .write_stdin("from stdin")
        .assert()
        .success();
    assert_eq!(
        stdout(&live, &["cat", &live.url("stdin.txt")]),
        b"from stdin"
    );
}

#[test]
fn live_pipe_flags() {
    let Some(live) = Live::new() else { return };
    let sdk = Sdk::new(&live);
    pipe(
        &live,
        &[
            "--attr",
            "Cache-Control=max-age=90;Artist=Unknown",
            "--tags",
            "env=prod&team=io",
            "-sc",
            "REDUCED_REDUNDANCY",
            "--checksum",
            "SHA256",
        ],
        &live.url("meta.txt"),
        "meta body",
    );
    let head = sdk.head(&live.bucket, "meta.txt");
    assert_eq!(head.cache_control(), Some("max-age=90"));
    assert_eq!(
        head.metadata()
            .and_then(|m| m.get("artist"))
            .map(String::as_str),
        Some("Unknown")
    );
    assert_eq!(
        head.storage_class().map(|c| c.as_str()),
        Some("REDUCED_REDUNDANCY")
    );
    assert!(head.checksum_sha256().is_some());
    assert_eq!(
        sdk.tags(&live.bucket, "meta.txt"),
        vec![("env".into(), "prod".into()), ("team".into(), "io".into())]
    );

    let big = pattern(11 * MIB + 7);
    pipe(
        &live,
        &["--concurrent", "2", "--part-size", "5MiB"],
        &live.url("big.bin"),
        big.clone(),
    );
    assert!(
        sdk.head(&live.bucket, "big.bin")
            .e_tag()
            .unwrap()
            .ends_with("-3\"")
    );
    assert_eq!(stdout(&live, &["cat", &live.url("big.bin")]), big);

    pipe(
        &live,
        &["--enc-s3", &live.bucket_target()],
        &live.url("enc.txt"),
        "enc",
    );
    assert_eq!(
        sdk.head(&live.bucket, "enc.txt").server_side_encryption(),
        Some(&ServerSideEncryption::Aes256)
    );
    if let Ok(key_id) = std::env::var("MX_TEST_KMS_KEY_ID") {
        pipe(
            &live,
            &["--enc-kms", &format!("{}={key_id}", live.bucket_target())],
            &live.url("kms.txt"),
            "kms",
        );
        assert_eq!(
            sdk.head(&live.bucket, "kms.txt").server_side_encryption(),
            Some(&ServerSideEncryption::AwsKms)
        );
    }

    for quiet in [["pipe", "-q"], ["pipe", "--quiet"], ["-q", "pipe"]] {
        live.cmd()
            .args(quiet)
            .arg(live.url("quiet.txt"))
            .write_stdin("quiet")
            .assert()
            .success()
            .stdout("");
    }
    let output = live
        .cmd()
        .args(["--json", "pipe", &live.url("json.txt")])
        .write_stdin("json")
        .output()
        .unwrap();
    let message: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(message["status"], "success");
    assert_eq!(message["size"], 4);
}

#[test]
fn live_find_filters_print_exec_and_watch() {
    let Some(live) = Live::new() else { return };
    let unique = format!("{}-unique.txt", live.bucket);
    pipe(
        &live,
        &["--attr", "Owner=alice", "--tags", "env=prod"],
        &live.url("a.txt"),
        "a",
    );
    pipe(&live, &[], &live.url("docs/b.log"), vec![b'x'; 2048]);
    pipe(&live, &[], &live.url("docs/deep/c.txt"), "0123456789");
    pipe(&live, &[], &live.url(&unique), "u");
    let bucket = live.bucket_target();
    // mc prints `alias/bucket/key`; compare the keys.
    let find = |args: &[&str]| -> Vec<String> {
        let mut all = vec!["find", bucket.as_str()];
        all.extend_from_slice(args);
        sorted_lines(&live, &all)
            .iter()
            .map(|line| {
                line.strip_prefix(&format!("{bucket}/"))
                    .unwrap_or(line)
                    .to_string()
            })
            .collect()
    };

    assert_eq!(
        find(&["--name", "*.txt"]),
        ["a.txt", "docs/deep/c.txt", unique.as_str()]
    );
    assert_eq!(
        find(&["--path", "docs/*"]),
        ["docs/b.log", "docs/deep/c.txt"]
    );
    assert_eq!(find(&["--ignore", "*.txt"]), ["docs/b.log"]);
    assert_eq!(find(&["--regex", r"^docs/.*\.txt$"]), ["docs/deep/c.txt"]);
    assert_eq!(find(&["--larger", "1KiB"]), ["docs/b.log"]);
    assert_eq!(find(&["--smaller", "5", "--name", "a*"]), ["a.txt"]);
    assert_eq!(
        find(&["--maxdepth", "2", "--name", "*.txt"]),
        ["a.txt", unique.as_str()]
    );
    assert_eq!(find(&["--newer-than", "1d"]).len(), 4);
    assert!(find(&["--older-than", "1d"]).is_empty());
    assert_eq!(find(&["--metadata", "Owner=^ali"]), ["a.txt"]);
    assert_eq!(
        find(&["--metadata", "X-Amz-Meta-Owner=bob"]),
        Vec::<String>::new()
    );
    assert_eq!(find(&["--tags", "env=^prod$"]), ["a.txt"]);
    assert!(find(&["--tags", "env=dev"]).is_empty());
    // `--maxdepth` truncates keys like mc instead of filtering.
    assert_eq!(
        find(&["--maxdepth", "2", "--name", "docs"]),
        ["docs/", "docs/"]
    );
    assert_eq!(
        find(&["--name", "c.txt", "--print", "{} {base} {dir} {size}"]),
        [format!("docs/deep/c.txt c.txt {bucket}/docs/deep 10 B")]
    );
    let url = find(&["--name", "b.log", "--print", "{url}"]);
    assert!(url[0].starts_with("http"), "{url:?}");
    assert!(url[0].contains("docs/b.log") && url[0].contains("X-Amz-Signature"));
    assert_eq!(
        find(&["--name", "a.txt", "--exec", "echo hit {}"]),
        [format!("hit {bucket}/a.txt")]
    );
    // Prefix target and alias-wide search.
    assert_eq!(
        sorted_lines(&live, &["find", &live.url("docs/"), "--name", "*.txt"]),
        [format!("{bucket}/docs/deep/c.txt")]
    );
    assert_eq!(
        sorted_lines(&live, &["find", &live.alias, "--name", &unique]),
        [format!("{bucket}/{unique}")]
    );

    // --versions
    let versioned = live.make_bucket(BucketOpts {
        versioning: true,
        lock: false,
    });
    let target = format!("{}/{versioned}/v.txt", live.alias);
    pipe(&live, &[], &target, "one");
    pipe(&live, &[], &target, "two");
    let base = format!("{}/{versioned}", live.alias);
    let lines = sorted_lines(&live, &["find", &base, "--versions"]);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines
            .iter()
            .all(|l| l.starts_with(&format!("{base}/v.txt (")))
    );
    let ids = sorted_lines(
        &live,
        &["find", &base, "--versions", "--print", "{version}"],
    );
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert_eq!(
        sorted_lines(&live, &["find", &base]),
        [format!("{base}/v.txt")]
    );

    // --watch
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("mx"))
        .env("HOME", live.home.path())
        .args(["find", &bucket, "--name", "*.new", "--watch"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    std::thread::sleep(Duration::from_secs(1));
    pipe(&live, &[], &live.url("later/x.new"), "new");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = None;
    while Instant::now() < deadline && seen.is_none() {
        seen = rx.recv_timeout(Duration::from_millis(200)).ok();
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(seen, Some(format!("{bucket}/later/x.new")));
}
