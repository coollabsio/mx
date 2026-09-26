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
        .stderr("mx: <ERROR> Unable to parse quota: unhandled size name: xb.\n");
    mx(home.path())
        .args(["quota", "info", "nosuch/b"])
        .assert()
        .failure()
        .stderr(
            "mx: <ERROR> Unable to initialize admin connection. No valid configuration found for 'nosuch/b' host alias.\n",
        );
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
    // Like mc (madmin `BucketQuota{Quota, Type}`).
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"quota": 1073741824u64, "size": 0, "rate": 0, "requests": 0, "quotatype": "hard"})
    );
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
        "azure remote tier requires the storage account name",
    );
    fail(
        &[
            "ilm",
            "tier",
            "add",
            "azure",
            "local",
            "T",
            "--account-name",
            "acct",
            "--az-sp-tenant-id",
            "t",
            "--bucket",
            "b",
        ],
        "requires static credentials OR service principal credentials",
    );
    fail(
        &[
            "ilm",
            "tier",
            "add",
            "gcs",
            "local",
            "T",
            "--credentials-file",
            "/nonexistent.json",
            "--bucket",
            "b",
        ],
        "Failed to read credentials file: open /nonexistent.json: no such file or directory.",
    );
    fail(
        &["ilm", "tier", "add", "bogus", "local", "T"],
        "Unsupported tier type: unsupported tier type.",
    );
    fail(
        &["ilm", "tier", "add", "minio", "local", "T", "extra"],
        "Incorrect number of arguments for tier add command. Invalid arguments provided",
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
        "retry this command with ‘--force’ and ‘--dangerous’ flags.",
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
    let tiers = r#"[{"Version":"v1","Type":"minio","Name":"WARM","MinIO":{"Prefix":"p/","Bucket":"tier","Endpoint":"http://remote:9000","AccessKey":"ra","SecretKey":"REDACTED"}}]"#;
    let (url, server) = fake_server(vec![(200, tiers), (200, tiers), (200, "[]")]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["ilm", "tier", "ls", "fake"])
        .assert()
        .success()
        .stdout(concat!(
            "┌────┬─────┬──────────────────┬──────┬──────┬──────┬─────────────┐\n",
            "│Name│Type │     Endpoint     │Bucket│Prefix│Region│Storage-Class│\n",
            "├────┼─────┼──────────────────┼──────┼──────┼──────┼─────────────┤\n",
            "│WARM│minio│http://remote:9000│ tier │  p/  │  -   │      -      │\n",
            "└────┴─────┴──────────────────┴──────┴──────┴──────┴─────────────┘\n",
        ));
    let out = mx(home.path())
        .args(["--json", "ilm", "tier", "ls", "fake"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    // madmin field order; the key order of the server response does not matter.
    assert_eq!(
        String::from_utf8(out).unwrap(),
        concat!(
            r#"{"status":"success","tiers":[{"Version":"v1","Type":"minio","Name":"WARM","#,
            r#""MinIO":{"Endpoint":"http://remote:9000","AccessKey":"ra","SecretKey":"REDACTED","Bucket":"tier","Prefix":"p/"}}]}"#,
            "\n"
        )
    );
    mx(home.path())
        .args(["ilm", "tier", "ls", "fake"])
        .assert()
        .success()
        .stdout(
            "mx: No remote tier targets found for alias 'fake'. Use `mx ilm tier add` to configure one.\n",
        );
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
    // mc's command is named `remove`, which has no message: an empty line.
    mx(home.path())
        .args(["ilm", "tier", "rm", "fake", "WARM"])
        .assert()
        .success()
        .stdout("\n");
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
        &["replicate", "backlog", "local"],
        "bucket not specified in `local`. Invalid arguments provided",
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
    // Compact without a terminal (mc's colorjson), indented on a terminal.
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with(r#"{"Rules":[{"ID":"r1""#), "{text}");
    let config: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(config["Rules"][0]["Filter"]["And"]["Prefix"], "logs/");
    assert_eq!(config["Rules"][0]["DeleteReplication"]["Status"], "Enabled");

    let requests = server.join().unwrap();
    assert_eq!(requests[0].line, "GET /b1/?replication= HTTP/1.1");
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
    assert_eq!(requests[2].line, "PUT /b1/?replication= HTTP/1.1");
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
        .stdout(predicate::str::contains(r#""op":"remove""#));
    let requests = server.join().unwrap();
    assert_eq!(requests[1].line, "DELETE /b1/?replication= HTTP/1.1");
}

/// madmin `EncryptData` body -> JSON (the test server knows the alias secret key).
fn decrypt_json(body: &[u8]) -> serde_json::Value {
    let plain = mx::s3::admin::decrypt_data("skey1234", body, None).unwrap();
    serde_json::from_slice(&plain).unwrap()
}

#[test]
fn tier_add_azure_and_gcs_send_madmin_config() {
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("creds.json");
    std::fs::write(&creds, r#"{"type":"service_account"}"#).unwrap();
    let (url, server) = fake_server(vec![(204, ""), (204, ""), (204, "")]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args([
            "ilm",
            "tier",
            "add",
            "azure",
            "fake",
            "aztier",
            "--account-name",
            "acct",
            "--account-key",
            "a2V5",
            "--bucket",
            "container",
            "--prefix",
            "p/",
            "--region",
            "westeurope",
        ])
        .assert()
        .success()
        .stdout("Added remote tier AZTIER of type azure\n");
    let out = mx(home.path())
        .args([
            "--json",
            "ilm",
            "tier",
            "add",
            "gcs",
            "fake",
            "gcstier",
            "--credentials-file",
            creds.to_str().unwrap(),
            "--bucket",
            "gb",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        r#"{"status":"success","tierName":"GCSTIER","tierType":"gcs","tierEndpoint":"https://storage.googleapis.com/","bucket":"gb"}"#.to_string() + "\n"
    );
    mx(home.path())
        .args([
            "ilm",
            "tier",
            "edit",
            "fake",
            "GCSTIER",
            "--credentials-file",
            creds.to_str().unwrap(),
        ])
        .assert()
        .success();
    let requests = server.join().unwrap();
    assert_eq!(
        decrypt_json(&requests[0].body),
        serde_json::json!({"Version": "v1", "Type": "azure", "Name": "AZTIER", "Azure": {
            "AccountName": "acct", "AccountKey": "a2V5", "Bucket": "container",
            "Prefix": "p/", "Region": "westeurope", "SPAuth": {}}})
    );
    // GCS credentials: URL-safe base64 of the file (madmin `NewTierGCS`).
    assert_eq!(
        decrypt_json(&requests[1].body),
        serde_json::json!({"Version": "v1", "Type": "gcs", "Name": "GCSTIER", "GCS": {
            "Endpoint": "https://storage.googleapis.com/",
            "Creds": "eyJ0eXBlIjoic2VydmljZV9hY2NvdW50In0=", "Bucket": "gb"}})
    );
    // Edit: madmin `TierCreds.CredsJSON` ([]byte, standard base64).
    assert_eq!(
        requests[2].line,
        "POST /minio/admin/v3/tier/GCSTIER HTTP/1.1"
    );
    assert_eq!(
        decrypt_json(&requests[2].body),
        serde_json::json!({"awsrole": false, "azSP": {}, "creds": "eyJ0eXBlIjoic2VydmljZV9hY2NvdW50In0="})
    );
}

#[test]
fn tier_edit_azure_service_principal() {
    let (url, server) = fake_server(vec![(204, "")]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args([
            "ilm",
            "tier",
            "update",
            "fake",
            "AZ",
            "--az-sp-tenant-id",
            "t",
            "--az-sp-client-id",
            "c",
            "--az-sp-client-secret",
            "s",
        ])
        .assert()
        .success()
        .stdout("\n");
    let requests = server.join().unwrap();
    assert_eq!(
        decrypt_json(&requests[0].body),
        serde_json::json!({"awsrole": false, "azSP": {"TenantID": "t", "ClientID": "c", "ClientSecret": "s"}})
    );
}

#[test]
fn admin_errors_carry_madmin_detail() {
    let (url, server) = fake_server(vec![(
        404,
        r#"{"Code":"XMinioAdminTierNotFound","Message":"Specified remote tier was not found","Resource":"/minio/admin/v3/tier/X","RequestId":"R1","HostId":"H1"}"#,
    )]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["--json", "ilm", "tier", "check", "fake", "X"])
        .assert()
        .code(1)
        .stdout(concat!(
            r#"{"status":"error","error":{"message":"Unable to verify remote tier target","cause":{"message":"Specified remote tier was not found","#,
            r#""error":{"Code":"XMinioAdminTierNotFound","Message":"Specified remote tier was not found","BucketName":"","Key":"","RequestID":"R1","HostID":"H1","Region":""}},"type":"fatal"}}"#,
            "\n"
        ));
    server.join().unwrap();
}

const METRICS_JSON: &str = r#"{"currStats":{"Stats":{"arn:minio:replication::id1:dst":{"completedReplicationSize":3,"failed":{"lastHour":{"bytes":0,"count":2},"lastMinute":{"bytes":0,"count":1},"totals":{"bytes":0,"count":3}},"replicationCount":1}},"completedReplicationSize":3,"queued":{"avg":{"bytes":0,"count":0},"curr":{"bytes":2048,"count":4},"max":{"bytes":0,"count":0}},"replicationCount":1},"queueStats":{"nodes":[{"activeWorkers":{"avg":2.5,"curr":3,"max":5},"nodeName":"node1:9000","transferSummary":{"Large":{"avgRate":0,"currRate":0,"peakRate":0},"Small":{"avgRate":1500,"currRate":2000,"peakRate":3000}},"uptime":7300}],"uptime":7300},"uptime":7300}"#;
const ONLINE_TARGETS_JSON: &str = r#"[{"sourcebucket":"b1","endpoint":"remote:9000","targetbucket":"dst","arn":"arn:minio:replication::id1:dst","type":"replication","isOnline":true,"totalDowntime":61000000000,"latency":{"curr":1000000,"avg":2400000,"max":1500000000}}]"#;

#[test]
fn replicate_status_renders_mc_table() {
    let (url, server) = fake_server(vec![
        (200, METRICS_JSON),
        (200, ONLINE_TARGETS_JSON),
        (200, REPLICATION_XML),
        (200, METRICS_JSON),
        (200, ONLINE_TARGETS_JSON),
        (200, REPLICATION_XML),
    ]);
    let home = home_with_alias(&url);
    let out = mx(home.path())
        .args(["replicate", "status", "fake/b1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    assert_eq!(
        lines,
        [
            "  Replication status since 2 hours",
            "  remote:9000",
            "  Replicated:                   1 objects (3 B)",
            "  Queued:                       ● 4 objects, 2.0 KiB (avg: 0 objects, 0 B ; max: 0 objects, 0 B)",
            "  Workers:                      3 (avg: 2; max: 5)",
            "  Transfer Rate:                0 B/s (avg: 0 B/s; max: 0 B/s",
            "  Latency:                      1ms (avg: 2ms; max: 1.5s)",
            "  Link:                         ● online (total downtime: 1 minutes 1 seconds)",
            "  Errors:                       1 in last 1 minute; 2 in last 1hr; 3 since uptime",
        ],
        "{text}"
    );
    let out = mx(home.path())
        .args(["replicate", "status", "--nodes", "fake/b1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains(
            "node1:9000    | 2 hours         | Small Objects (<128 MiB)  | 1.5 kB/s     | 3.0 kB/s     | 2.0 kB/s     | 2         \n"
        ),
        "{text}"
    );
    let requests = server.join().unwrap();
    assert_eq!(requests[0].line, "GET /b1/?replication-metrics=2 HTTP/1.1");
}

#[test]
fn replicate_backlog_json_documents() {
    let mrf = r#"{"nodeName":"n1","bucket":"b1","object":"o1","versionId":"v1","retryCount":2}
{"nodeName":"n1","bucket":"b1","object":"o2","versionId":"","retryCount":0}"#;
    let diff = r#"{"object":"o1","versionId":"v1","rStatus":"FAILED","lastModified":"2026-09-26T10:00:00Z","targets":{"arn1":{"rStatus":"FAILED"}},"deletemarker":false}"#;
    let (url, server) = fake_server(vec![(200, mrf), (200, diff)]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["--json", "replicate", "backlog", "fake/b1"])
        .assert()
        .success()
        .stdout(concat!(
            r#"{"op":"mrf","status":"success","nodeName":"n1","bucket":"b1","object":"o1","versionId":"v1","retryCount":2}"#,
            "\n",
            r#"{"op":"mrf","status":"success","nodeName":"n1","bucket":"b1","object":"o2","versionId":"","retryCount":0}"#,
            "\n"
        ));
    mx(home.path())
        .args(["--json", "replicate", "backlog", "--full", "fake/b1/pre"])
        .assert()
        .success()
        .stdout(concat!(
            r#"{"object":"o1","versionId":"v1","targets":{"arn1":{"rStatus":"FAILED"}},"rStatus":"FAILED","replTimestamp":"0001-01-01T00:00:00Z","lastModified":"2026-09-26T10:00:00Z","deletemarker":false}"#,
            "\n"
        ));
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "GET /minio/admin/v3/replication/mrf?bucket=b1&node=all HTTP/1.1"
    );
    assert_eq!(
        requests[1].line,
        "POST /minio/admin/v3/replication/diff?bucket=b1&prefix=pre HTTP/1.1"
    );
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
