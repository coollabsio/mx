//! Offline tests for `admin replicate`, `admin decommission` and `admin rebalance`: argument
//! checks (mc's messages and help/exit status) and output rendering against a tiny in-process
//! HTTP server with canned admin API replies.

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

fn mx(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("mx").expect("binary");
    cmd.env("HOME", home)
        .env_remove("MC_JSON")
        .env_remove("MC_CONFIG_DIR");
    cmd
}

/// `mx` with alias `fake` pointing at `url` (`MC_HOST_fake`).
fn mx_fake(home: &std::path::Path, url: &str) -> Command {
    let mut cmd = mx(home);
    let host = url.trim_start_matches("http://");
    cmd.env("MC_HOST_fake", format!("http://akey:skey1234@{host}"));
    cmd
}

/// Recorded request: request line and body.
#[derive(Debug)]
struct Recorded {
    line: String,
    body: Vec<u8>,
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
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                let (name, value) = header.split_once(':').unwrap();
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body_in = vec![0u8; length];
            reader.read_exact(&mut body_in).unwrap();
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
                body: body_in,
            });
        }
        recorded
    });
    (url, handle)
}

const INVALID: &str =
    "Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.";

// ---------------------------------------------------------------------------
// argument checks
// ---------------------------------------------------------------------------

#[test]
fn wrong_argument_counts_show_help_with_status_1() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["admin", "decommission", "start", "a"][..],
        &["admin", "decom", "start", "a", "b", "c"],
        &["admin", "decommission", "status"],
        &["admin", "decommission", "cancel", "a", "b", "c"],
        &["admin", "rebalance", "start"],
        &["admin", "rebalance", "status", "a", "b"],
        &["admin", "rebalance", "stop"],
        &["admin", "replicate", "update"],
        &["admin", "replicate", "resync", "start", "a"],
        &["admin", "replicate", "resync", "status", "a", "b", "c"],
        &["admin", "replicate", "resync", "cancel"],
    ] {
        mx(home.path())
            .args(args)
            .assert()
            .code(1)
            .stdout(predicate::str::contains("Usage:"))
            .stderr("");
    }
}

#[test]
fn replicate_argument_errors_match_mc() {
    let home = tempfile::tempdir().unwrap();
    let cases: &[(&[&str], String)] = &[
        (
            &["admin", "replicate", "add", "a"],
            format!("Need at least two arguments to add command. {INVALID}"),
        ),
        (
            &["admin", "replicate", "info"],
            format!("Need exactly one alias argument. {INVALID}"),
        ),
        (
            &["admin", "replicate", "status", "a", "b"],
            format!("Need exactly one alias argument. {INVALID}"),
        ),
        (
            &["admin", "replicate", "status", "a", "--bucket", "x", "--users"],
            format!(
                "Cannot specify both (bucket|group|policy|user|ilm-expiry-rule) flag and one or more of buckets|groups|policies|users|ilm-expiry-rules) flag(s). {INVALID}"
            ),
        ),
        (
            &["admin", "replicate", "status", "a", "--bucket", "x", "--user", "y"],
            format!(
                "Cannot specify more than one of --bucket, --policy, --user, --group, --ilm-expiry-rule  flags at the same time. {INVALID}"
            ),
        ),
        (
            &["admin", "replicate", "rm", "a"],
            format!("Need at least two arguments to remove command. {INVALID}"),
        ),
        (&["admin", "replicate", "rm", "a", "b", "--all"], format!(" {INVALID}")),
        (
            &["admin", "replicate", "remove", "a", "b"],
            "Site removal requires --force flag. This operation is *IRREVERSIBLE*. Please review carefully before performing this *DANGEROUS* operation. ".to_string(),
        ),
        (
            &["admin", "replicate", "update", "a", "b"],
            format!("Invalid arguments specified for edit command. {INVALID}"),
        ),
    ];
    for (args, message) in cases {
        mx(home.path())
            .args(*args)
            .assert()
            .code(1)
            .stdout("")
            .stderr(format!("mx: <ERROR> {message}\n"));
    }
    mx(home.path())
        .args(["--json", "admin", "replicate", "info"])
        .assert()
        .code(1)
        .stdout(
            r#"{"status":"error","error":{"message":"Need exactly one alias argument.","cause":{"message":"Invalid arguments provided, please refer `mc \u003ccommand\u003e -h` for relevant documentation.","error":{}},"type":"fatal"}}
"#,
        );
}

#[test]
fn replicate_update_flag_errors_match_mc() {
    let home = tempfile::tempdir().unwrap();
    let url = "http://127.0.0.1:9";
    let cases: &[(&[&str], &str)] = &[
        (&[], "--deployment-id is a required flag"),
        (
            &["--deployment-id", "d"],
            "--endpoint, --mode, --bucket-bandwidth, --disable-ilm-expiry-replication or --enable-ilm-expiry-replication is a required flag",
        ),
        (
            &["--deployment-id", "d", "--mode", "sync", "--sync", "enable"],
            "either --sync or --mode flag should be specified",
        ),
        (
            &["--deployment-id", "d", "--mode", "foo"],
            "--mode can be either [sync|async]",
        ),
        (
            &["--deployment-id", "d", "--sync", "maybe"],
            "--sync can be either [enable|disable]",
        ),
        (
            &[
                "--enable-ilm-expiry-replication",
                "--disable-ilm-expiry-replication",
            ],
            "either --disable-ilm-expiry-replication or --enable-ilm-expiry-replication flag should be specified",
        ),
        (
            &["--enable-ilm-expiry-replication", "--deployment-id", "d"],
            "--deployment-id should not be set with --disable-ilm-expiry-replication or --enable-ilm-expiry-replication",
        ),
        (
            &["--deployment-id", "d", "--endpoint", "%zz"],
            r#"Unsupported URL format parse "%zz": invalid URL escape "%zz""#,
        ),
    ];
    for (flags, message) in cases {
        mx_fake(home.path(), url)
            .args(["admin", "replicate", "update", "fake"])
            .args(*flags)
            .assert()
            .code(1)
            .stderr(format!("mx: <ERROR> {message}. {INVALID}\n"));
    }
    mx_fake(home.path(), url)
        .args([
            "admin",
            "replicate",
            "update",
            "fake",
            "--deployment-id",
            "d",
            "--bucket-bandwidth",
            "2X",
        ])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> invalid bandwidth value: unhandled size name: x.\n");
}

#[test]
fn unknown_alias_errors_match_mc() {
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args(["admin", "decommission", "status", "nosuch"])
        .assert()
        .code(1)
        .stderr(
            "mx: <ERROR> Unable to initialize admin connection. No valid configuration found for 'nosuch' host alias.\n",
        );
    mx(home.path())
        .args(["admin", "rebalance", "status", "nosuch"])
        .assert()
        .code(1)
        .stderr(
            "mx: <ERROR> Unable to initialize admin client. No valid configuration found for 'nosuch' host alias.\n",
        );
    let (url, _server) = fake_server(vec![]);
    mx_fake(home.path(), &url)
        .args(["admin", "replicate", "add", "fake", "nosuch"])
        .assert()
        .code(1)
        .stderr(
            "mx: <ERROR> unable to initialize admin connection. No valid configuration found for 'nosuch' host alias.\n",
        );
}

// ---------------------------------------------------------------------------
// decommission
// ---------------------------------------------------------------------------

const POOLS: &str = r#"[{"id":0,"cmdline":"/data{1...4}","lastUpdate":"2026-09-26T19:25:31.177815646Z","decommissionInfo":{"startTime":"0001-01-01T00:00:00Z","startSize":0,"totalSize":882070388736,"currentSize":434307284992,"complete":false,"failed":false,"canceled":false,"objectsDecommissioned":0,"objectsDecommissionedFailed":0,"bytesDecommissioned":0,"bytesDecommissionedFailed":0}},{"id":1,"cmdline":"/data{5...8}","lastUpdate":"2026-09-26T19:25:31.177815949Z","decommissionInfo":{"startTime":"2026-09-26T19:25:31Z","startSize":0,"totalSize":882070388736,"currentSize":434307284992,"complete":false,"failed":false,"canceled":false,"objectsDecommissioned":0,"objectsDecommissionedFailed":0,"bytesDecommissioned":0,"bytesDecommissionedFailed":0}}]"#;

#[test]
fn decommission_status_and_cancel_tables() {
    let home = tempfile::tempdir().unwrap();
    let (url, server) = fake_server(vec![(200, POOLS), (200, POOLS), (200, POOLS)]);
    mx_fake(home.path(), &url)
        .args(["admin", "decom", "status", "fake/"])
        .assert()
        .success()
        .stdout(
            "┌─────┬──────────────┬────────────────────────┬──────────┐\n\
             │ ID  │ Pools        │ Drives Usage           │ Status   │\n\
             │ 1st │ /data{1...4} │ 50.8% (total: 822 GiB) │ Active   │\n\
             │ 2nd │ /data{5...8} │ 50.8% (total: 822 GiB) │ Draining │\n\
             └─────┴──────────────┴────────────────────────┴──────────┘\n",
        );
    // Not a terminal: compact JSON (Go prints this document with 4-space indent on a TTY).
    mx_fake(home.path(), &url)
        .args(["--json", "admin", "decom", "status", "fake"])
        .assert()
        .success()
        .stdout(format!("{POOLS}\n"));
    // Without a pool, cancel only lists the pools being drained.
    let capacity = "417 GiB (used) / 822 GiB (total)";
    let dashes = "─".repeat(capacity.len() + 2);
    mx_fake(home.path(), &url)
        .args(["admin", "decom", "cancel", "fake"])
        .assert()
        .success()
        .stdout(format!(
            "┌─────┬──────────────┬{dashes}┬──────────┐\n\
             │ ID  │ Pools        │ {:<w$} │ Status   │\n\
             │ 2nd │ /data{{5...8}} │ {capacity} │ Draining │\n\
             └─────┴──────────────┴{dashes}┴──────────┘\n",
            "Capacity",
            w = capacity.len()
        ));
    let requests = server.join().unwrap();
    assert!(
        requests
            .iter()
            .all(|r| r.line.starts_with("GET /minio/admin/v3/pools/list "))
    );
}

#[test]
fn decommission_start_status_cancel_pool() {
    let home = tempfile::tempdir().unwrap();
    let pool_status = r#"{"id":1,"cmdline":"/data{5...8}","lastUpdate":"2026-09-26T19:29:40Z","decommissionInfo":{"startTime":"0001-01-01T00:00:00Z","startSize":0,"totalSize":10,"currentSize":5,"complete":false,"failed":false,"canceled":false,"objectsDecommissioned":0,"objectsDecommissionedFailed":0,"bytesDecommissioned":0,"bytesDecommissionedFailed":0}}"#;
    let (url, server) = fake_server(vec![
        (200, ""),
        (200, ""),
        (200, pool_status),
        (200, ""),
        (
            400,
            r#"{"Code":"XMinioAdminInvalidArgument","Message":"Invalid arguments specified.","Resource":"/minio/admin/v3/pools/decommission","RequestId":"R1","HostId":"H1"}"#,
        ),
    ]);
    mx_fake(home.path(), &url)
        .args(["admin", "decommission", "start", "fake", "/data{5...8}"])
        .assert()
        .success()
        .stdout("Decommission started successfully for `/data{5...8}`.\n");
    mx_fake(home.path(), &url)
        .args([
            "--json",
            "admin",
            "decommission",
            "start",
            "fake",
            "/data{5...8}",
        ])
        .assert()
        .success()
        .stdout("{\"status\":\"success\",\"pool\":\"/data{5...8}\"}\n");
    // Not being decommissioned: a non-fatal error, exit status 0.
    mx_fake(home.path(), &url)
        .args(["admin", "decommission", "status", "fake", "/data{5...8}"])
        .assert()
        .success()
        .stdout("")
        .stderr("mx: <ERROR> This pool is currently not scheduled for decomissioning \n");
    // A successful cancel prints nothing.
    mx_fake(home.path(), &url)
        .args(["admin", "decommission", "cancel", "fake", "/data{5...8}"])
        .assert()
        .success()
        .stdout("");
    mx_fake(home.path(), &url)
        .args(["admin", "decommission", "start", "fake", "/x"])
        .assert()
        .code(1)
        .stderr(
            "mx: <ERROR> Unable to start decommission on the specified pool. Invalid arguments specified.\n",
        );
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "POST /minio/admin/v3/pools/decommission?pool=%2Fdata%7B5...8%7D HTTP/1.1"
    );
    assert_eq!(
        requests[2].line,
        "GET /minio/admin/v3/pools/status?pool=%2Fdata%7B5...8%7D HTTP/1.1"
    );
    assert_eq!(
        requests[3].line,
        "POST /minio/admin/v3/pools/cancel?pool=%2Fdata%7B5...8%7D HTTP/1.1"
    );
}

// ---------------------------------------------------------------------------
// rebalance
// ---------------------------------------------------------------------------

#[test]
fn rebalance_start_status_stop() {
    let home = tempfile::tempdir().unwrap();
    let status = r#"{"ID":"n4Za","stoppedAt":"0001-01-01T00:00:00Z","pools":[{"id":0,"status":"Started","used":0.5086507767922406,"progress":{"objects":3,"versions":4,"bytes":2048,"bucket":"b","object":"o","elapsed":61000000000,"eta":0}},{"id":1,"status":"None","used":0.25,"progress":{"objects":0,"versions":0,"bytes":0,"bucket":"","object":"","elapsed":0,"eta":0}}]}"#;
    let (url, server) = fake_server(vec![
        (200, r#"{"id":"abc"}"#),
        (200, r#"{"id":"abc"}"#),
        (200, status),
        (200, status),
        (200, ""),
        (200, ""),
        (
            400,
            r#"{"Code":"XMinioAdminRebalanceNotStarted","Message":"Pool rebalance is not started","RequestId":"R1","HostId":"H1"}"#,
        ),
    ]);
    mx_fake(home.path(), &url)
        .args(["admin", "rebalance", "start", "fake"])
        .assert()
        .success()
        .stdout("Rebalance started for fake\n");
    mx_fake(home.path(), &url)
        .args(["--json", "admin", "rebalance", "start", "fake"])
        .assert()
        .success()
        .stdout("{\"status\":\"success\",\"url\":\"fake\",\"id\":\"abc\"}\n");
    mx_fake(home.path(), &url)
        .args(["admin", "rebalance", "status", "fake"])
        .assert()
        .success()
        .stdout(
            "Per-pool usage:\n\
             ┌──────────┬────────┐\n\
             │ Pool-0   │ Pool-1 │\n\
             │ 50.87% * │ 25.00% │\n\
             └──────────┴────────┘\n\
             Summary: \n\
             Data: 2.0 KiB (3 objects, 4 versions) \n\
             Time: 1m1s (0s to completion)\n",
        );
    mx_fake(home.path(), &url)
        .args(["--json", "admin", "rebalance", "status", "fake"])
        .assert()
        .success()
        .stdout(format!("{status}\n"));
    mx_fake(home.path(), &url)
        .args(["admin", "rebalance", "stop", "fake"])
        .assert()
        .success()
        .stdout("Rebalance stopped for fake\n");
    mx_fake(home.path(), &url)
        .args(["--json", "admin", "rebalance", "stop", "fake"])
        .assert()
        .success()
        .stdout("{\"status\":\"success\",\"url\":\"fake\"}\n");
    mx_fake(home.path(), &url)
        .args(["admin", "rebalance", "status", "fake"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> Unable to get rebalance status. Pool rebalance is not started.\n");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "POST /minio/admin/v3/rebalance/start HTTP/1.1"
    );
    assert_eq!(
        requests[2].line,
        "GET /minio/admin/v3/rebalance/status HTTP/1.1"
    );
    assert_eq!(
        requests[4].line,
        "POST /minio/admin/v3/rebalance/stop HTTP/1.1"
    );
}

// ---------------------------------------------------------------------------
// replicate
// ---------------------------------------------------------------------------

const SR_INFO: &str = r#"{"enabled":true,"name":"sr1","sites":[{"endpoint":"http://10.0.0.1:9000","name":"sr1","deploymentID":"72fcc4ed-8354-4f1b-ab7d-b92732817b94","sync":"","defaultbandwidth":{"bandwidthLimitPerBucket":0,"set":false,"updatedAt":"0001-01-01T00:00:00Z"},"replicate-ilm-expiry":false},{"endpoint":"http://10.0.0.2:9000","name":"sr2","deploymentID":"9ad5747f-0335-413e-8659-bda18fca271b","sync":"enable","defaultbandwidth":{"bandwidthLimitPerBucket":2000000000,"set":true,"updatedAt":"2026-09-26T19:20:00Z"},"replicate-ilm-expiry":true}],"serviceAccountAccessKey":"site-replicator-0"}"#;

#[test]
fn replicate_info_text_and_json() {
    let home = tempfile::tempdir().unwrap();
    let (url, server) = fake_server(vec![
        (200, SR_INFO),
        (200, SR_INFO),
        (200, r#"{"enabled":false}"#),
    ]);
    mx_fake(home.path(), &url)
        .args(["admin", "replicate", "info", "fake"])
        .assert()
        .success()
        .stdout(
            "SiteReplication enabled for:\n\n\
             Deployment ID                        | Site Name       | Endpoint                                       | Sync | Bandwidth  | ILM Expiry Replication   \n                                     |                 |                                                |      | Per Bucket |                          \n\
             72fcc4ed-8354-4f1b-ab7d-b92732817b94 | sr1             | http://10.0.0.1:9000                           |      | N/A        | false                    \n\
             9ad5747f-0335-413e-8659-bda18fca271b | sr2             | http://10.0.0.2:9000                           | ✔    | 2.0 GB/s   | true                     \n",
        );
    mx_fake(home.path(), &url)
        .args(["--json", "admin", "replicate", "info", "fake"])
        .assert()
        .success()
        .stdout(format!("{SR_INFO}\n"));
    mx_fake(home.path(), &url)
        .args(["admin", "replicate", "info", "fake"])
        .assert()
        .success()
        .stdout("SiteReplication is not enabled\n");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "GET /minio/admin/v3/site-replication/info?api-version=1 HTTP/1.1"
    );
}

#[test]
fn replicate_add_sends_encrypted_peer_sites() {
    let home = tempfile::tempdir().unwrap();
    let (url, server) = fake_server(vec![(
        200,
        r#"{"success":true,"status":"Requested sites were configured for replication successfully."}"#,
    )]);
    let host = url.trim_start_matches("http://").to_string();
    mx_fake(home.path(), &url)
        .env("MC_HOST_other", format!("http://ak2:sk2secret@{host}"))
        .args([
            "admin",
            "replicate",
            "add",
            "fake",
            "other",
            "--replicate-ilm-expiry",
        ])
        .assert()
        .success()
        .stdout("Requested sites were configured for replication successfully.\n");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "PUT /minio/admin/v3/site-replication/add?replicateILMExpiry=true&force=false&api-version=1 HTTP/1.1"
    );
    // madmin.EncryptData: the credentials never travel in clear text.
    let body = mx::s3::admin::decrypt_response("skey1234", &requests[0].body).expect("decrypt");
    let sites: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        sites,
        serde_json::json!([
            {"name": "fake", "endpoints": url, "accessKey": "akey", "secretKey": "skey1234"},
            {"name": "other", "endpoints": url, "accessKey": "ak2", "secretKey": "sk2secret"},
        ])
    );
}

#[test]
fn replicate_status_remove_and_update_output() {
    let home = tempfile::tempdir().unwrap();
    let status = r#"{"Enabled":false,"MaxBuckets":0,"MaxUsers":0,"MaxGroups":0,"MaxPolicies":0,"MaxILMExpiryRules":0,"Sites":null,"StatsSummary":null,"BucketStats":{},"PolicyStats":{},"UserStats":{},"GroupStats":{},"Metrics":{"activeWorkers":{"curr":0,"avg":0,"max":0},"replicaSize":0,"replicaCount":0,"queued":{"curr":{"count":0,"bytes":0},"avg":{"count":0,"bytes":0},"max":{"count":0,"bytes":0}},"proxied":{"putTaggingProxyTotal":0,"getTaggingProxyTotal":0,"removeTaggingProxyTotal":0,"getProxyTotal":0,"headProxyTotal":0,"putTaggingProxyFailed":0,"getTaggingProxyFailed":0,"removeTaggingProxyFailed":0,"getProxyFailed":0,"headProxyFailed":0},"replMetrics":null,"uptime":0,"retries":{"last1hr":0,"last1m":0,"total":0},"errors":{"last1hr":0,"last1m":0,"total":0}},"ILMExpiryStats":null}"#;
    let (url, server) = fake_server(vec![
        (200, status),
        (200, status),
        (
            200,
            r#"{"status":"Requested site(s) were removed from cluster replication successfully."}"#,
        ),
        (
            200,
            r#"{"status":"Requested site(s) were removed from cluster replication successfully."}"#,
        ),
        (
            200,
            r#"{"success":true,"status":"Cluster replication configuration updated successfully with:\n- sync state enable for peer "}"#,
        ),
    ]);
    mx_fake(home.path(), &url)
        .args(["admin", "replicate", "status", "fake", "--users"])
        .assert()
        .success()
        .stdout("SiteReplication is not enabled\n");
    mx_fake(home.path(), &url)
        .args(["--json", "admin", "replicate", "status", "fake"])
        .assert()
        .success()
        .stdout(format!("{status}\n"));
    mx_fake(home.path(), &url)
        .args(["admin", "replicate", "rm", "fake", "sr3", "--force"])
        .assert()
        .success()
        .stdout("Following site(s) [sr3] were removed successfully\n");
    mx_fake(home.path(), &url)
        .args(["admin", "replicate", "rm", "fake", "--all", "--force"])
        .assert()
        .success()
        .stdout("All site(s) were removed successfully\n");
    mx_fake(home.path(), &url)
        .args([
            "admin",
            "replicate",
            "edit",
            "fake",
            "--deployment-id",
            "d1",
            "--mode",
            "sync",
        ])
        .assert()
        .success()
        .stdout("Cluster replication configuration updated successfully with:\n- sync state enable for peer \n");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "GET /minio/admin/v3/site-replication/status?buckets=false&policies=false&users=true&groups=false&showDeleted=false&metrics=false&ilm-expiry-rules=false&peer-state=false&api-version=1 HTTP/1.1"
    );
    assert_eq!(
        requests[2].line,
        "PUT /minio/admin/v3/site-replication/remove?api-version=1 HTTP/1.1"
    );
    assert_eq!(
        String::from_utf8_lossy(&requests[2].body),
        r#"{"requestingDepID":"","sites":["sr3"],"all":false}"#
    );
    assert_eq!(
        String::from_utf8_lossy(&requests[3].body),
        r#"{"requestingDepID":"","sites":null,"all":true}"#
    );
    assert!(requests[4].line.starts_with(
        "PUT /minio/admin/v3/site-replication/edit?disableILMExpiryReplication=false&enableILMExpiryReplication=false&api-version=1 "
    ));
    let body = mx::s3::admin::decrypt_response("skey1234", &requests[4].body).expect("decrypt");
    let peer: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(peer["deploymentID"], "d1");
    assert_eq!(peer["sync"], "enable");
}

#[test]
fn replicate_resync_resolves_the_peer() {
    let home = tempfile::tempdir().unwrap();
    let peer_info = r#"{"deploymentID":"9ad5747f-0335-413e-8659-bda18fca271b"}"#;
    let other_info = r#"{"deploymentID":"00000000-0000-0000-0000-000000000000"}"#;
    let (url, server) = fake_server(vec![
        (200, SR_INFO),
        (200, peer_info),
        (
            200,
            r#"{"op":"start","id":"c2beb6fa","status":"success","buckets":null}"#,
        ),
        (200, SR_INFO),
        (200, peer_info),
        (
            200,
            r#"{"op":"cancel","id":"c2beb6fa","status":"success","buckets":[]}"#,
        ),
        (200, SR_INFO),
        (200, other_info),
        (200, r#"{"enabled":false}"#),
    ]);
    mx_fake(home.path(), &url)
        .args(["admin", "replicate", "resync", "start", "fake", "fake"])
        .assert()
        .success()
        .stdout("Site resync started with ID c2beb6fa\n");
    mx_fake(home.path(), &url)
        .args([
            "--json",
            "admin",
            "replicate",
            "resync",
            "cancel",
            "fake",
            "fake",
        ])
        .assert()
        .success()
        .stdout("{\"op\":\"cancel\",\"id\":\"c2beb6fa\",\"status\":\"success\",\"buckets\":[]}\n");
    mx_fake(home.path(), &url)
        .args(["admin", "replicate", "resync", "start", "fake", "fake"])
        .assert()
        .code(1)
        .stderr(format!(
            "mx: <ERROR> alias provided is not part of cluster replication. {INVALID}\n"
        ));
    // Site replication disabled: mc prints nothing.
    mx_fake(home.path(), &url)
        .args(["admin", "replicate", "resync", "status", "fake", "fake"])
        .assert()
        .success()
        .stdout("")
        .stderr("");
    let requests = server.join().unwrap();
    assert_eq!(requests[1].line, "GET /minio/admin/v3/info HTTP/1.1");
    assert_eq!(
        requests[2].line,
        "PUT /minio/admin/v3/site-replication/resync/op?operation=start&api-version=1 HTTP/1.1"
    );
    let peer: serde_json::Value = serde_json::from_slice(&requests[2].body).unwrap();
    assert_eq!(peer["name"], "sr2");
    assert_eq!(peer["deploymentID"], "9ad5747f-0335-413e-8659-bda18fca271b");
}
