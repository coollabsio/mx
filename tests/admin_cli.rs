//! Offline tests for `quota`, `ilm tier` and `replicate` (MinIO admin API). Requests are sent
//! to a tiny in-process HTTP server that records them and replies with canned responses.

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

fn mx(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("mx").expect("binary");
    cmd.env("HOME", home);
    cmd
}

/// Recorded request: request line, lowercase headers, body.
#[derive(Debug)]
struct Recorded {
    line: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Serves one canned `(status, body)` response per connection, in order.
fn fake_server(responses: Vec<(u16, &'static str)>) -> (String, JoinHandle<Vec<Recorded>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut recorded = Vec::new();
        for (status, body) in responses {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut headers = Vec::new();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                let (name, value) = header.split_once(':').unwrap();
                headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
            let length: usize = headers
                .iter()
                .find(|(n, _)| n == "content-length")
                .map(|(_, v)| v.parse().unwrap())
                .unwrap_or(0);
            let mut request_body = vec![0u8; length];
            reader.read_exact(&mut request_body).unwrap();
            let mut stream = reader.into_inner();
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            stream.flush().unwrap();
            recorded.push(Recorded {
                line: line.trim_end().to_string(),
                headers,
                body: request_body,
            });
        }
        recorded
    });
    (url, handle)
}

fn home_with_alias(url: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias", "set", "--api", "S3v4", "fake", url, "akey", "skey1234",
        ])
        .assert()
        .success();
    home
}

fn assert_signed(request: &Recorded) {
    let auth = request.header("authorization").unwrap_or_default();
    assert!(
        auth.starts_with("AWS4-HMAC-SHA256 Credential=akey/")
            && auth.contains("/us-east-1/s3/aws4_request"),
        "{auth}"
    );
    assert!(request.header("x-amz-content-sha256").is_some());
    assert!(request.header("x-amz-date").is_some());
}

// ---------------------------------------------------------------------------
// quota
// ---------------------------------------------------------------------------

#[test]
fn quota_validates_arguments() {
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args(["quota", "set", "local/b"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--size flag needs to be set"));
    mx(home.path())
        .args(["quota", "set", "local/b", "--size", "1XB"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unable to parse quota"));
    mx(home.path())
        .args(["quota", "info", "local/b/key"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("bucket target"));
}

#[test]
fn quota_set_and_info_use_admin_api() {
    let (url, server) = fake_server(vec![
        (200, ""),
        (
            200,
            r#"{"quota":0,"size":1073741824,"rate":0,"requests":0,"quotatype":"hard"}"#,
        ),
        (200, r#"{"size":1073741824,"quotatype":"hard"}"#),
    ]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["quota", "set", "fake/b1", "--size", "1GiB"])
        .assert()
        .success()
        .stdout("Successfully set bucket quota of 1.0 GiB on `b1`\n");
    mx(home.path())
        .args(["quota", "info", "fake/b1"])
        .assert()
        .success()
        .stdout("Bucket `b1` has hard quota of 1.0 GiB\n");
    let out = mx(home.path())
        .args(["--json", "quota", "info", "fake/b1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"status": "success", "bucket": "b1", "quota": 1073741824u64, "type": "hard"})
    );

    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "PUT /minio/admin/v3/set-bucket-quota?bucket=b1 HTTP/1.1"
    );
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["size"], 1073741824u64);
    assert_eq!(body["quotatype"], "hard");
    assert_signed(&requests[0]);
    assert_eq!(
        requests[1].line,
        "GET /minio/admin/v3/get-bucket-quota?bucket=b1 HTTP/1.1"
    );
    assert_signed(&requests[1]);
}

#[test]
fn quota_reports_server_errors() {
    let (url, server) = fake_server(vec![(
        404,
        r#"{"Code":"XMinioAdminNoSuchQuotaConfiguration","Message":"The quota configuration does not exist"}"#,
    )]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["quota", "info", "fake/b1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "mx: <ERROR> Unable to get bucket quota. The quota configuration does not exist.",
        ));
    server.join().unwrap();
}

// ---------------------------------------------------------------------------
// ilm tier
// ---------------------------------------------------------------------------

#[test]
fn tier_validates_arguments() {
    let home = tempfile::tempdir().unwrap();
    let fail = |args: &[&str], message: &str| {
        mx(home.path())
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(message));
    };
    fail(
        &["ilm", "tier", "add", "azure", "local", "T", "--bucket", "b"],
        "not supported by mx",
    );
    fail(
        &["ilm", "tier", "add", "bogus", "local", "T"],
        "Unsupported tier type",
    );
    fail(
        &[
            "ilm",
            "tier",
            "add",
            "minio",
            "local",
            "T",
            "--bucket",
            "b",
            "--endpoint",
            "http://h",
        ],
        "requires access credentials",
    );
    fail(
        &[
            "ilm",
            "tier",
            "add",
            "s3",
            "local",
            "T",
            "--bucket",
            "b",
            "--access-key",
            "a",
            "--secret-key",
            "s",
            "--storage-class",
            "GLACIER",
        ],
        "unsupported storage-class",
    );
    fail(
        &["ilm", "tier", "edit", "local", "T", "--access-key", "a"],
        "Insufficient credential information",
    );
    fail(
        &["ilm", "tier", "rm", "local", "T", "--force"],
        "--dangerous",
    );
    // With --json, errors are an mc error document on stdout.
    mx(home.path())
        .args(["--json", "ilm", "tier", "info", "local", "T"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("Incorrect number of arguments"))
        .stdout(predicate::str::contains(r#""type":"fatal""#));
}

#[test]
fn tier_add_sends_encrypted_config() {
    let (url, server) = fake_server(vec![(204, "")]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args([
            "ilm",
            "tier",
            "add",
            "minio",
            "fake",
            "warm",
            "--endpoint",
            "http://remote:9000",
            "--access-key",
            "ra",
            "--secret-key",
            "rs",
            "--bucket",
            "tier",
            "--prefix",
            "p/",
        ])
        .assert()
        .success()
        .stdout("Added remote tier WARM of type minio\n");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "PUT /minio/admin/v3/tier?force=false HTTP/1.1"
    );
    assert_signed(&requests[0]);
    let body = &requests[0].body;
    // salt(32) | id(0x02 = PBKDF2/AES-GCM) | nonce(8) | ciphertext + tag
    assert_eq!(body[32], 0x02);
    assert!(body.len() > 41 + 16);
    assert!(!String::from_utf8_lossy(body).contains("remote:9000"));
}

#[test]
fn tier_ls_renders_table_and_json() {
    let tiers = r#"[{"Version":"v1","Type":"minio","Name":"WARM","MinIO":{"Endpoint":"http://remote:9000","AccessKey":"ra","SecretKey":"REDACTED","Bucket":"tier","Prefix":"p/"}}]"#;
    let (url, server) = fake_server(vec![(200, tiers), (200, tiers), (200, "[]")]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["ilm", "tier", "ls", "fake"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "│ WARM │ minio │ http://remote:9000 │  tier  │   p/   │   -    │       -       │",
        ));
    let out = mx(home.path())
        .args(["--json", "ilm", "tier", "ls", "fake"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["status"], "success");
    assert_eq!(value["tiers"][0]["Name"], "WARM");
    mx(home.path())
        .args(["ilm", "tier", "ls", "fake"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No remote tier targets found"));
    let requests = server.join().unwrap();
    assert_eq!(requests[0].line, "GET /minio/admin/v3/tier HTTP/1.1");
}

#[test]
fn tier_check_edit_and_remove_paths() {
    let (url, server) = fake_server(vec![(204, ""), (204, ""), (204, "")]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["ilm", "tier", "check", "fake", "WARM"])
        .assert()
        .success()
        .stdout("Remote tier connectivity check for WARM was successful\n");
    mx(home.path())
        .args([
            "ilm",
            "tier",
            "edit",
            "fake",
            "WARM",
            "--access-key",
            "a",
            "--secret-key",
            "s",
        ])
        .assert()
        .success()
        .stdout("Updated remote tier WARM\n");
    mx(home.path())
        .args(["ilm", "tier", "rm", "fake", "WARM"])
        .assert()
        .success()
        .stdout("Removed remote tier WARM\n");
    let requests = server.join().unwrap();
    assert_eq!(requests[0].line, "GET /minio/admin/v3/tier/WARM HTTP/1.1");
    assert_eq!(requests[1].line, "POST /minio/admin/v3/tier/WARM HTTP/1.1");
    assert_eq!(requests[1].body[32], 0x02);
    assert_eq!(
        requests[2].line,
        "DELETE /minio/admin/v3/tier/WARM?force=false HTTP/1.1"
    );
}

// ---------------------------------------------------------------------------
// replicate
// ---------------------------------------------------------------------------

#[test]
fn replicate_validates_arguments() {
    let home = tempfile::tempdir().unwrap();
    let fail = |args: &[&str], message: &str| {
        mx(home.path())
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(message));
    };
    fail(&["replicate", "add", "local/b"], "--remote-bucket");
    fail(
        &[
            "replicate",
            "add",
            "local/b",
            "--remote-bucket",
            "http://a:b@h/dst",
            "--replicate",
            "bogus",
        ],
        "invalid value for --replicate",
    );
    fail(
        &[
            "replicate",
            "add",
            "local/b",
            "--remote-bucket",
            "http://a:b@h/dst",
            "--path",
            "sideways",
        ],
        "unrecognized bucket path style",
    );
    fail(
        &["replicate", "update", "local/b"],
        "--id is a required flag",
    );
    fail(
        &[
            "replicate",
            "update",
            "local/b",
            "--id",
            "r",
            "--sync",
            "enable",
        ],
        "--remote-bucket is a required flag",
    );
    fail(
        &[
            "replicate",
            "update",
            "local/b",
            "--id",
            "r",
            "--state",
            "maybe",
        ],
        "--state can be either",
    );
    fail(
        &["replicate", "rm", "local/b", "--all"],
        "--all and --force",
    );
    fail(&["replicate", "rm", "local/b"], "rule ID cannot be empty");
    fail(
        &["replicate", "status", "local/b", "--nodes"],
        "not supported",
    );
    fail(
        &["replicate", "resync", "start", "local/b"],
        "--remote-bucket",
    );
    fail(
        &[
            "replicate",
            "resync",
            "start",
            "local/b",
            "--remote-bucket",
            "arn",
            "--older-than",
            "abc",
        ],
        "Unable to parse older-than",
    );
}

#[test]
fn replicate_add_rejects_temporary_tokens() {
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "replicate",
            "add",
            "local/b",
            "--remote-bucket",
            "https://a:b:token@h/dst",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("temporary tokens"));
}

const REPLICATION_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?><ReplicationConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Rule><ID>r1</ID><Status>Enabled</Status><Priority>1</Priority><DeleteMarkerReplication><Status>Enabled</Status></DeleteMarkerReplication><DeleteReplication><Status>Enabled</Status></DeleteReplication><Destination><Bucket>arn:minio:replication::id1:dst</Bucket></Destination><Filter><And><Prefix>logs/</Prefix><Tag><Key>a</Key><Value>1</Value></Tag></And></Filter></Rule></ReplicationConfiguration>"#;
const TARGETS_JSON: &str = r#"[{"sourcebucket":"b1","endpoint":"remote:9000","targetbucket":"dst","arn":"arn:minio:replication::id1:dst","type":"replication"}]"#;

#[test]
fn replicate_ls_and_export() {
    let (url, server) = fake_server(vec![
        (200, REPLICATION_XML),
        (200, TARGETS_JSON),
        (200, REPLICATION_XML),
    ]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["replicate", "ls", "fake/b1"])
        .assert()
        .success()
        .stdout(
            "Rules:\nRemote Bucket: remote:9000/dst\n  Rule ID: r1\n  Priority: 1\n  ARN: arn:minio:replication::id1:dst\n  Prefix: logs/\n  Tags: a=1\n\n",
        );
    let out = mx(home.path())
        .args(["replicate", "export", "fake/b1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.starts_with("{\n \"Rules\": [\n  {\n   \"ID\": \"r1\""),
        "{text}"
    );
    let config: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(config["Rules"][0]["Filter"]["And"]["Prefix"], "logs/");
    assert_eq!(config["Rules"][0]["DeleteReplication"]["Status"], "Enabled");

    let requests = server.join().unwrap();
    assert_eq!(requests[0].line, "GET /b1?replication= HTTP/1.1");
    assert_signed(&requests[0]);
    assert_eq!(
        requests[1].line,
        "GET /minio/admin/v3/list-remote-targets?bucket=b1&type= HTTP/1.1"
    );
}

#[test]
fn replicate_ls_without_config_fails() {
    let (url, server) = fake_server(vec![(
        404,
        "<Error><Code>ReplicationConfigurationNotFoundError</Code><Message>The replication configuration was not found</Message></Error>",
    )]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["replicate", "ls", "fake/b1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "replication configuration not set",
        ));
    server.join().unwrap();
}

#[test]
fn replicate_add_creates_target_then_rule() {
    let (url, server) = fake_server(vec![
        (
            404,
            "<Error><Code>ReplicationConfigurationNotFoundError</Code></Error>",
        ),
        (200, r#""arn:minio:replication::id9:dst""#),
        (200, ""),
    ]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args([
            "replicate",
            "add",
            "fake/b1/pre/",
            "--remote-bucket",
            "http://ra:rs@remote:9000/dst",
            "--priority",
            "4",
            "--id",
            "rule-x",
            "--replicate",
            "delete,existing-objects",
            "--tags",
            "k=v",
            "--sync",
            "--bandwidth",
            "1Gi",
        ])
        .assert()
        .success()
        .stdout("Replication configuration rule with ID `rule-x` applied to fake/b1/pre/.\n");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[1].line,
        "PUT /minio/admin/v3/set-remote-target?bucket=b1 HTTP/1.1"
    );
    assert_eq!(requests[1].body[32], 0x02);
    assert_eq!(requests[2].line, "PUT /b1?replication= HTTP/1.1");
    assert!(requests[2].header("content-md5").is_some());
    let xml = String::from_utf8(requests[2].body.clone()).unwrap();
    for part in [
        "<ID>rule-x</ID>",
        "<Priority>4</Priority>",
        "<DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication>",
        "<DeleteReplication><Status>Enabled</Status></DeleteReplication>",
        "<Bucket>arn:minio:replication::id9:dst</Bucket>",
        "<And><Prefix>pre/</Prefix><Tag><Key>k</Key><Value>v</Value></Tag></And>",
        "<ExistingObjectReplication><Status>Enabled</Status></ExistingObjectReplication>",
    ] {
        assert!(xml.contains(part), "missing {part} in {xml}");
    }
}

#[test]
fn replicate_rm_all_deletes_configuration() {
    let (url, server) = fake_server(vec![(200, REPLICATION_XML), (204, "")]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["--json", "replicate", "rm", "fake/b1", "--all", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""op":"rm""#));
    let requests = server.join().unwrap();
    assert_eq!(requests[1].line, "DELETE /b1?replication= HTTP/1.1");
}

#[test]
fn help_lists_admin_subcommands() {
    let home = tempfile::tempdir().unwrap();
    let out = mx(home.path())
        .args(["replicate", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    for name in [
        "add", "update", "ls", "status", "resync", "export", "import", "rm", "backlog",
    ] {
        assert!(text.contains(name), "missing {name}");
    }
    mx(home.path())
        .args(["ilm", "tier", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("check"));
    mx(home.path())
        .args(["replicate", "resync", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cancel"));
}
