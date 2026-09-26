//! Offline CLI tests for share, tag, version, anonymous, ilm rule, ping, ready, cors, encrypt.
//! `ping`/`ready` run against a tiny local HTTP stub that emulates MinIO health endpoints.

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

fn mx(home: &Path) -> Command {
    let mut command = Command::cargo_bin("mx").expect("binary");
    command.env("HOME", home);
    command
}

fn home() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn fails_with(home: &Path, args: &[&str], message: &str) {
    mx(home)
        .args(args)
        .assert()
        .failure()
        .stderr(predicate::str::contains(message));
}

#[test]
fn help_lists_new_subcommands_and_flags() {
    let home = home();
    let cases: &[(&[&str], &[&str])] = &[
        (&["share", "--help"], &["download", "upload", "list"]),
        (
            &["share", "download", "--help"],
            &["--recursive", "--version-id", "--expire"],
        ),
        (
            &["share", "upload", "--help"],
            &["--recursive", "--expire", "--content-type"],
        ),
        (
            &["tag", "set", "--help"],
            &[
                "--version-id",
                "--rewind",
                "--versions",
                "--recursive",
                "--exclude-folders",
            ],
        ),
        (
            &["tag", "list", "--help"],
            &["--version-id", "--rewind", "--versions", "--recursive"],
        ),
        (
            &["version", "enable", "--help"],
            &["--excluded-prefixes", "--exclude-folders"],
        ),
        (
            &["anonymous", "--help"],
            &["set", "set-json", "get", "get-json", "list", "links"],
        ),
        (&["anonymous", "--help"], &["--recursive, -r"]),
        (&["anonymous", "links", "--help"], &["--recursive"]),
        (
            &["ilm", "rule", "--help"],
            &["add", "edit", "list", "remove", "export", "import"],
        ),
        (
            &["ilm", "rule", "add", "--help"],
            &[
                "--prefix",
                "--tags",
                "--size-lt",
                "--size-gt",
                "--expire-days",
                "--expire-delete-marker",
                "--transition-days",
                "--transition-tier",
                "--noncurrent-expire-days",
                "--noncurrent-expire-newer",
                "--noncurrent-transition-days",
                "--noncurrent-transition-tier",
                "--expire-all-object-versions",
            ],
        ),
        (
            &["ilm", "rule", "edit", "--help"],
            &["--id", "--disable", "--enable", "--expire-days"],
        ),
        (
            &["ilm", "rule", "ls", "--help"],
            &["--expiry", "--transition"],
        ),
        (
            &["ilm", "rule", "rm", "--help"],
            &["--id", "--all", "--force"],
        ),
        (
            &["ping", "--help"],
            &[
                "--count",
                "--error-count",
                "--exit",
                "--interval",
                "--distributed",
                "--node",
            ],
        ),
        (&["ready", "--help"], &["--cluster-read", "--maintenance"]),
        (&["encrypt", "--help"], &["set", "clear", "info"]),
        (&["cors", "--help"], &["set", "get", "remove"]),
    ];
    for (args, expected) in cases {
        let assert = mx(home.path()).args(*args).assert().success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
        for needle in *expected {
            assert!(
                stdout.contains(needle),
                "{args:?} help lacks {needle}:\n{stdout}"
            );
        }
    }
}

#[test]
fn share_validates_expiry_and_flags() {
    let home = home();
    fails_with(
        home.path(),
        &["share", "download", "--expire", "169h", "local/b/o"],
        "Expiry cannot be larger than 7 days.",
    );
    fails_with(
        home.path(),
        &["share", "download", "-E", "500ms", "local/b/o"],
        "Expiry cannot be lesser than 1 second.",
    );
    fails_with(
        home.path(),
        &["share", "download", "-E", "10", "local/b/o"],
        "Unable to parse expire",
    );
    fails_with(
        home.path(),
        &["share", "download", "-r", "-vid", "v1", "local/b/"],
        "--version-id cannot be specified with --recursive flag.",
    );
    fails_with(
        home.path(),
        &["share", "upload", "local/b/prefix/"],
        "Use --recursive flag to generate curl command for prefixes.",
    );
    mx(home.path())
        .args(["share", "list", "everything"])
        .assert()
        .failure();
}

#[test]
fn share_list_reads_mc_share_db_and_drops_expired() {
    let home = home();
    let share_dir = home.path().join(".mx").join("share");
    std::fs::create_dir_all(&share_dir).unwrap();
    std::fs::write(
        share_dir.join("downloads.json"),
        r#"{
	"version": "1",
	"shares": {
		"http://h/b/fresh?X-Amz-Signature=1": {
			"share": "http://h/b/fresh",
			"versionID": "",
			"date": "2999-01-01T00:00:00Z",
			"expiry": 3600000000000
		},
		"http://h/b/old?X-Amz-Signature=2": {
			"share": "http://h/b/old",
			"versionID": "",
			"date": "2001-01-01T00:00:00Z",
			"expiry": 3600000000000
		}
	}
}"#,
    )
    .unwrap();
    mx(home.path())
        .args(["share", "list", "download"])
        .assert()
        .success()
        .stdout(predicate::str::contains("URL: http://h/b/fresh"))
        .stdout(predicate::str::contains(
            "Expire: 1 hours 0 minutes 0 seconds",
        ))
        .stdout(predicate::str::contains(
            "Share: http://h/b/fresh?X-Amz-Signature=1",
        ))
        .stdout(predicate::str::contains("old").not());
    let assert = mx(home.path())
        .args(["--json", "share", "ls", "download"])
        .assert()
        .success();
    let line: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(line["status"], "success");
    assert_eq!(line["url"], "http://h/b/fresh");
    assert_eq!(line["timeLeft"], 3_600_000_000_000u64);
    let saved = std::fs::read_to_string(share_dir.join("downloads.json")).unwrap();
    assert!(!saved.contains("http://h/b/old"), "{saved}");
    mx(home.path())
        .args(["share", "list", "upload"])
        .assert()
        .success()
        .stdout("");
}

#[test]
fn tag_validates_flag_combinations() {
    let home = home();
    fails_with(
        home.path(),
        &["tag", "set", "--exclude-folders", "local/b", "a=1"],
        "'--exclude-folders' must be used with --recursive only",
    );
    fails_with(
        home.path(),
        &[
            "tag",
            "set",
            "--version-id",
            "v",
            "--versions",
            "local/b/o",
            "a=1",
        ],
        "You cannot specify both --version-id and --rewind or --versions",
    );
    fails_with(
        home.path(),
        &["tag", "remove", "-vid", "v", "--rewind", "1d", "local/b/o"],
        "You cannot specify both --version-id and --rewind or --versions",
    );
    fails_with(
        home.path(),
        &["tag", "list", "--vid", "v", "--rewind", "1d", "local/b/o"],
        "You cannot specify both --version-id and --rewind flags",
    );
    fails_with(
        home.path(),
        &["tag", "set", "local/b/o", "novalue"],
        "must use key=value",
    );
}

#[test]
fn anonymous_validates_permissions_and_files() {
    let home = home();
    fails_with(
        home.path(),
        &["anonymous", "set", "bogus", "local/b"],
        "Unrecognized permission `bogus`. Allowed values are [private, public, download, upload].",
    );
    fails_with(
        home.path(),
        &[
            "anonymous",
            "set-json",
            "/nonexistent/policy.json",
            "local/b",
        ],
        "Unable to set anonymous for `local/b`.",
    );
    let file = home.path().join("bad.json");
    std::fs::write(&file, "not json").unwrap();
    fails_with(
        home.path(),
        &["anonymous", "set-json", file.to_str().unwrap(), "local/b"],
        "is not valid JSON",
    );
}

/// mc's `-r/--recursive` belongs to `anonymous` itself: accepted before or after the operation
/// (only `links` uses it), never a usage error.
#[test]
fn anonymous_accepts_recursive_anywhere() {
    let home = home();
    for args in [
        &["anonymous", "-r", "links", "dead/b"][..],
        &["anonymous", "links", "dead/b", "--recursive"],
        &["anonymous", "--recursive", "list", "dead/b"],
    ] {
        fails_with(
            home.path(),
            args,
            "Unable to list policies of target `dead/b`.",
        );
    }
    for args in [
        &["anonymous", "-r", "get", "dead/b"][..],
        &["anonymous", "get-json", "-r", "dead/b"],
    ] {
        mx(home.path())
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("anonymous `` for `dead/b`."))
            .stderr(predicate::str::contains("Invalid command usage").not());
    }
    fails_with(
        home.path(),
        &["anonymous", "set", "-r", "public", "dead/b"],
        "Unable to set anonymous `public` for `dead/b`.",
    );
}

#[test]
fn ilm_rule_validates_before_contacting_the_server() {
    let home = home();
    fails_with(
        home.path(),
        &["ilm", "rule", "add", "local/b"],
        "at least one of Expiry, Transition",
    );
    fails_with(
        home.path(),
        &["ilm", "rule", "add", "--transition-tier", "WARM", "local/b"],
        "--transition-days must be set together with --transition-tier",
    );
    fails_with(
        home.path(),
        &["ilm", "rule", "add", "--transition-days", "3", "local/b"],
        "--transition-tier must be set together with --transition-days",
    );
    fails_with(
        home.path(),
        &[
            "ilm",
            "rule",
            "add",
            "--noncurrent-transition-tier",
            "WARM",
            "local/b",
        ],
        "--noncurrent-transition-days must be set",
    );
    fails_with(
        home.path(),
        &[
            "ilm",
            "rule",
            "add",
            "--expire-days",
            "3",
            "--expire-delete-marker",
            "local/b",
        ],
        "only one parameter under Expiration can be specified",
    );
    fails_with(
        home.path(),
        &["ilm", "rule", "add", "--expire-days", "0", "local/b"],
        "expiration days cannot be set to zero",
    );
    fails_with(
        home.path(),
        &[
            "ilm",
            "rule",
            "add",
            "--size-lt",
            "lots",
            "--expire-days",
            "1",
            "local/b",
        ],
        "size-lt value lots is invalid",
    );
    fails_with(
        home.path(),
        &["ilm", "rule", "rm", "local/b"],
        "ilm ID cannot be empty",
    );
    fails_with(
        home.path(),
        &["ilm", "rule", "rm", "--all", "local/b"],
        "--all and --force",
    );
    mx(home.path())
        .args(["ilm", "rule", "edit", "--expire-days", "3", "local/b"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--id"));
    mx(home.path())
        .args(["ilm", "rule", "ls", "--expiry", "--transition", "local/b"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
    mx(home.path())
        .args(["ilm", "rule", "import", "local/b"])
        .write_stdin(r#"{"Rules":[]}"#)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "does not contain any rule, aborting",
        ));
    mx(home.path())
        .args(["ilm", "rule", "import", "local/b"])
        .write_stdin("garbage")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unable to read ILM configuration"));
}

#[test]
fn encrypt_and_cors_validate_arguments() {
    let home = home();
    fails_with(
        home.path(),
        &["encrypt", "set", "sse-c", "local/b"],
        "Unknown argument `sse-c` passed",
    );
    fails_with(
        home.path(),
        &["encrypt", "set", "sse-s3", "key", "local/b"],
        "sse-s3 does not take a KMS key id",
    );
    mx(home.path())
        .args(["encrypt", "set", "sse-s3"])
        .assert()
        .failure();
    let file = home.path().join("cors.txt");
    std::fs::write(&file, "nonsense").unwrap();
    fails_with(
        home.path(),
        &["cors", "set", "local/b", file.to_str().unwrap()],
        "expected XML or JSON",
    );
    fails_with(
        home.path(),
        &["cors", "set", "local/b", "/nonexistent/cors.xml"],
        "Unable to open bucket CORS configuration file",
    );
}

#[test]
fn ping_validates_flags_and_server_info() {
    let home = home();
    // `--node` / `-a` read the node list from the admin ServerInfo API first.
    fails_with(
        home.path(),
        &["ping", "--node", "n1", "local"],
        "Unable to get server info",
    );
    fails_with(
        home.path(),
        &["ping", "--distributed", "local"],
        "Unable to get server info",
    );
    fails_with(
        home.path(),
        &["ping", "-c", "0", "local"],
        "ping count cannot be less than 1",
    );
}

// ---------------------------------------------------------------------------
// health endpoint stub
// ---------------------------------------------------------------------------

/// Serves `respond(path_and_query, request_index)` -> (status, extra headers) on a local port
/// and records the requested paths.
fn health_stub(
    respond: impl Fn(&str, usize) -> (u16, Vec<(&'static str, &'static str)>) + Send + 'static,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for (index, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            log.lock().unwrap().push(path.clone());
            let (status, headers) = respond(&path, index);
            let mut response =
                format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n");
            for (name, value) in headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (url, seen)
}

fn stub_home(url: &str) -> tempfile::TempDir {
    let home = home();
    mx(home.path())
        .args([
            "alias",
            "set",
            "--api",
            "S3v4",
            "stub",
            url,
            "access",
            "secret12345",
        ])
        .assert()
        .success();
    home
}

#[test]
fn ping_reports_status_and_summary() {
    let (url, seen) = health_stub(|path, index| {
        assert!(path.starts_with("/minio/health/live"), "{path}");
        if index == 1 {
            (503, vec![])
        } else {
            (200, vec![])
        }
    });
    let home = stub_home(&url);
    mx(home.path())
        .args(["ping", "-c", "3", "-i", "0", "stub"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(r"  1: http://127\.0\.0\.1:\d+   status=ok  time=").unwrap(),
        )
        .stdout(
            predicate::str::contains("  2: http").and(predicate::str::contains("status=failed ")),
        )
        .stdout(predicate::str::contains("  3: http"))
        .stdout(predicate::str::contains("│ Endpoint "))
        .stdout(predicate::str::contains("│ Count │"));
    assert_eq!(seen.lock().unwrap().len(), 3);

    let assert = mx(home.path())
        .args(["--json", "ping", "--exit", "stub"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "{stdout}");
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["counter"], "  1");
    assert_eq!(first["servers"][0]["status"], "ok ");
    let summary: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert!(summary["serverMap"].is_object());
}

#[test]
fn ping_stops_after_error_count() {
    // Non-200 responses are "failed" but only transport errors count as errors.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let home = home();
    mx(home.path())
        .args([
            "alias",
            "set",
            "--api",
            "S3v4",
            "dead",
            &dead,
            "access",
            "secret12345",
        ])
        .assert()
        .success();
    mx(home.path())
        .args(["ping", "-e", "2", "-i", "0", "dead"])
        .assert()
        .success()
        .stdout(predicate::str::contains("  2: http"))
        .stdout(predicate::str::contains("  3: http").not());
}

#[test]
fn ready_uses_cluster_endpoints() {
    let (url, seen) = health_stub(|path, _| match path {
        "/minio/health/cluster" => (
            200,
            vec![
                ("x-minio-write-quorum", "3"),
                ("x-minio-healing-drives", "1"),
            ],
        ),
        "/minio/health/cluster/read" => (200, vec![]),
        _ => (404, vec![]),
    });
    let home = stub_home(&url);
    mx(home.path())
        .args(["ready", "stub"])
        .assert()
        .success()
        .stdout("The cluster 'stub' is ready\n");
    let assert = mx(home.path())
        .args(["--json", "ready", "stub"])
        .assert()
        .success();
    let message: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(message["healthy"], true);
    assert_eq!(message["writeQuorum"], 3);
    assert_eq!(message["healingDrives"], 1);
    assert_eq!(message["error"], serde_json::Value::Null);
    mx(home.path())
        .args(["ready", "--cluster-read", "stub"])
        .assert()
        .success();
    let paths = seen.lock().unwrap().clone();
    assert_eq!(
        paths,
        vec![
            "/minio/health/cluster",
            "/minio/health/cluster",
            "/minio/health/cluster/read"
        ]
    );
}

#[test]
fn ready_waits_while_in_maintenance() {
    let (url, seen) = health_stub(|path, index| {
        assert_eq!(path, "/minio/health/cluster?maintenance=true");
        if index == 0 {
            (412, vec![])
        } else {
            (200, vec![])
        }
    });
    let home = stub_home(&url);
    let assert = mx(home.path())
        .args(["--json", "ready", "--maintenance", "stub"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let lines = stdout
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert_eq!(lines[0]["healthy"], false);
    assert_eq!(lines[0]["maintenanceMode"], true);
    assert_eq!(lines[1]["healthy"], true);
    assert_eq!(seen.lock().unwrap().len(), 2);
}
