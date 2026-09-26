//! Offline tests for `batch` and `sql`. Requests go to a tiny in-process HTTP server that
//! records them and replies with canned responses (S3 Select replies are hand-built AWS
//! event-stream frames).

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

/// Recorded request: request line and body.
#[derive(Debug)]
struct Recorded {
    line: String,
    body: Vec<u8>,
}

/// One canned response.
struct Reply {
    status: u16,
    headers: Vec<(&'static str, &'static str)>,
    body: Vec<u8>,
}

fn reply(status: u16, body: &str) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    }
}

/// Serves one reply per connection, in order.
fn fake_server(replies: Vec<Reply>) -> (String, JoinHandle<Vec<Recorded>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut recorded = Vec::new();
        for reply in replies {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut length = 0;
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
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();
            let mut stream = reader.into_inner();
            let mut head = format!(
                "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                reply.status,
                reply.body.len()
            );
            for (name, value) in &reply.headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str("\r\n");
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&reply.body).unwrap();
            stream.flush().unwrap();
            recorded.push(Recorded {
                line: line.trim_end().to_string(),
                body,
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
            "alias", "set", "--api", "S3v4", "--path", "on", "fake", url, "akey", "skey1234",
        ])
        .assert()
        .success();
    home
}

// ---------------------------------------------------------------------------
// batch
// ---------------------------------------------------------------------------

#[test]
fn batch_generate_falls_back_to_static_templates() {
    let (url, server) = fake_server(vec![
        reply(426, ""),
        reply(426, ""),
        reply(404, ""),
        reply(426, ""),
    ]);
    let home = home_with_alias(&url);
    let out = mx(home.path())
        .args(["batch", "generate", "fake", "replicate"])
        .assert()
        .success();
    let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(text.starts_with("replicate:\n  apiVersion: v1\n  # source of the objects"));
    assert!(
        text.ends_with("      delay: \"500ms\" # least amount of delay between each retry\n\n")
    );
    mx(home.path())
        .args(["batch", "generate", "fake", "foo"])
        .assert()
        .failure()
        .stderr(
            "mx: <ERROR> Unable to generate a job template for the specified job type. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n",
        );
    mx(home.path())
        .args(["batch", "generate", "fake", "list"])
        .assert()
        .success()
        .stdout("replicate\nkeyrotate\nexpire\n");
    mx(home.path())
        .args(["--json", "batch", "generate", "fake", "list"])
        .assert()
        .success()
        .stdout("[\"replicate\",\"keyrotate\",\"expire\"]\n");
    let recorded = server.join().unwrap();
    assert!(
        recorded[0]
            .line
            .starts_with("GET /minio/admin/v3/generate-job?jobType=replicate "),
        "{:?}",
        recorded[0]
    );
    assert!(
        recorded[2]
            .line
            .starts_with("GET /minio/admin/v3/list-supported-job-types ")
    );
}

#[test]
fn batch_generate_prints_server_templates() {
    let (url, server) = fake_server(vec![reply(200, "custom:\n  x: 1\n")]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["batch", "generate", "fake", "custom"])
        .assert()
        .success()
        .stdout("custom:\n  x: 1\n\n");
    server.join().unwrap();
}

#[test]
fn batch_start_posts_the_job_file() {
    let result = r#"{"id":"replicate-abc:-1","type":"replicate","user":"minioadmin","started":"2026-09-26T19:24:35.1691Z"}"#;
    let (url, server) = fake_server(vec![reply(200, result), reply(200, result)]);
    let home = home_with_alias(&url);
    let job = home.path().join("job.yaml");
    std::fs::write(&job, "replicate:\n  apiVersion: v1\n").unwrap();
    let job = job.to_str().unwrap();
    mx(home.path())
        .args(["batch", "start", "fake", job])
        .assert()
        .success()
        .stdout(
            "Successfully started 'replicate' job `replicate-abc:-1` on '2026-09-26 19:24:35.1691 +0000 UTC'\n",
        );
    mx(home.path())
        .args(["--json", "batch", "start", "fake", job])
        .assert()
        .success()
        .stdout(format!("{{\"status\":\"success\",\"result\":{result}}}\n"));
    let recorded = server.join().unwrap();
    assert!(
        recorded[0]
            .line
            .starts_with("POST /minio/admin/v3/start-job ")
    );
    assert_eq!(recorded[0].body, b"replicate:\n  apiVersion: v1\n");
}

#[test]
fn batch_start_reports_unreadable_files_like_mc() {
    let home = home_with_alias("http://127.0.0.1:1");
    mx(home.path())
        .args(["batch", "start", "fake", "nofile.yml"])
        .assert()
        .failure()
        .stderr(
            "mx: <ERROR> Unable to read nofile.yml: open nofile.yml: no such file or directory.\n",
        );
    mx(home.path())
        .args(["--json", "batch", "start", "fake", "nofile.yml"])
        .assert()
        .failure()
        .stdout(
            r#"{"status":"error","error":{"message":"Unable to read %s","cause":{"message":"open nofile.yml: no such file or directory","error":{"Op":"open","Path":"nofile.yml","Err":2}},"type":"fatal"}}
"#,
        );
    mx(home.path())
        .args(["batch", "list", "nosuch"])
        .assert()
        .failure()
        .stderr(
            "mx: <ERROR> Unable to initialize admin connection. No valid configuration found for 'nosuch' host alias.\n",
        );
}

#[test]
fn batch_list_shows_job_states() {
    let jobs = r#"{"jobs":[{"id":"replicate-a:-1","type":"replicate","user":"minioadmin","started":"2020-01-01T00:00:00Z"},{"id":"expire-b:-1","type":"expire","user":"u","started":"2020-01-01T00:00:00Z"}]}"#;
    let done = r#"{"LastMetric":{"jobID":"x","complete":true}}"#;
    let failed = r#"{"LastMetric":{"jobID":"x","complete":true,"failed":true}}"#;
    let (url, server) = fake_server(vec![
        reply(200, jobs),
        reply(200, done),
        reply(200, failed),
        reply(200, jobs),
        reply(200, done),
        reply(
            404,
            r#"{"Code":"XMinioAdminNoSuchJob","Message":"The specified job does not exist."}"#,
        ),
        reply(200, r#"{"jobs":null}"#),
    ]);
    let home = home_with_alias(&url);
    let out = mx(home.path())
        .args(["batch", "list", "fake", "--type", "replicate"])
        .assert()
        .success();
    let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "ID            \tTYPE     \tUSER      \tSTARTED    \tSTATUS    "
    );
    assert!(
        lines[1].starts_with("replicate-a:-1\treplicate\tminioadmin\t")
            && lines[1].ends_with(" years ago\tcompleted\t"),
        "{text}"
    );
    assert!(lines[2].ends_with("\tfailed   \t"), "{text}");
    mx(home.path())
        .args(["--json", "batch", "list", "fake"])
        .assert()
        .success()
        .stdout(
            r#"{"jobs":[{"id":"replicate-a:-1","started":"2020-01-01T00:00:00Z","status":"completed","type":"replicate","user":"minioadmin"},{"id":"expire-b:-1","started":"2020-01-01T00:00:00Z","status":"unknown","type":"expire","user":"u"}],"status":"success"}
"#,
        )
        .stderr(
            "Failed to fetch job status for Job ID: expire-b:-1 Error: The specified job does not exist.\n",
        );
    mx(home.path())
        .args(["batch", "ls", "fake"])
        .assert()
        .success()
        .stdout("currently no jobs are running\n");
    let recorded = server.join().unwrap();
    assert!(
        recorded[0]
            .line
            .starts_with("GET /minio/admin/v3/list-jobs?jobType=replicate ")
    );
    assert!(
        recorded[1]
            .line
            .starts_with("GET /minio/admin/v3/status-job?jobId=replicate-a%3A-1 ")
    );
}

#[test]
fn batch_describe_cancel_and_status() {
    let metric = r#"{"LastMetric":{"jobID":"expire-b:-1","jobType":"expire","startTime":"2026-01-01T00:00:00Z","lastUpdate":"2026-01-01T00:00:01Z","retryAttempts":1,"complete":true,"failed":false,"expired":{"lastBucket":"b","lastObject":"o","objects":3,"objectsFailed":0,"deleteMarkers":0,"deleteMarkersFailed":0},"extra":1}}"#;
    let (url, server) = fake_server(vec![
        reply(200, "expire:\n    apiVersion: v1\n"),
        reply(204, ""),
        reply(204, ""),
        reply(
            404,
            r#"{"Code":"XMinioAdminNoSuchJob","Message":"The specified job does not exist."}"#,
        ),
        reply(200, metric),
    ]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["batch", "describe", "fake", "expire-b:-1"])
        .assert()
        .success()
        .stdout("expire:\n    apiVersion: v1\n\n");
    mx(home.path())
        .args(["batch", "cancel", "fake", "expire-b:-1"])
        .assert()
        .success()
        .stdout("Successfully canceled batch job `expire-b:-1`\n");
    mx(home.path())
        .args(["--json", "batch", "cancel", "fake", "expire-b:-1"])
        .assert()
        .success()
        .stdout("{\"status\":\"success\",\"job-id\":\"expire-b:-1\"}\n");
    mx(home.path())
        .args(["--json", "batch", "status", "fake", "expire-b:-1"])
        .assert()
        .success()
        .stdout(
            r#"{"status":"success","metric":{"jobID":"expire-b:-1","jobType":"expire","startTime":"2026-01-01T00:00:00Z","lastUpdate":"2026-01-01T00:00:01Z","retryAttempts":1,"complete":true,"failed":false,"expired":{"lastBucket":"b","lastObject":"o","objects":3,"objectsFailed":0,"deleteMarkers":0,"deleteMarkersFailed":0}}}
"#,
        );
    let recorded = server.join().unwrap();
    assert!(
        recorded[1]
            .line
            .starts_with("DELETE /minio/admin/v3/cancel-job?id=expire-b%3A-1 ")
    );
    assert!(
        recorded[3]
            .line
            .starts_with("GET /minio/admin/v3/describe-job?jobId=")
    );
    assert!(
        recorded[4]
            .line
            .starts_with("GET /minio/admin/v3/status-job?jobId=")
    );
}

#[test]
fn batch_status_streams_realtime_metrics() {
    let running = r#"{"hosts":["h"],"aggregated":{"batchJobs":{"collected":"2026-01-01T00:00:00Z","Jobs":{"r-1":{"jobID":"r-1","jobType":"replicate","startTime":"2026-01-01T00:00:00Z","lastUpdate":"2026-01-01T00:00:00Z","retryAttempts":0,"complete":false,"failed":false,"replicate":{"lastBucket":"","lastObject":"","objects":0,"objectsFailed":0,"deleteMarkers":0,"deleteMarkersFailed":0,"bytesTransferred":0,"bytesFailed":0}}}}},"final":false}"#;
    let done = running.replace("\"complete\":false", "\"complete\":true");
    let body = format!("{running}\n{done}\n");
    let (url, server) = fake_server(vec![
        reply(200, "replicate:\n"),
        Reply {
            status: 200,
            headers: Vec::new(),
            body: body.into_bytes(),
        },
    ]);
    let home = home_with_alias(&url);
    let out = mx(home.path())
        .args(["--json", "batch", "status", "fake", "r-1"])
        .assert()
        .success();
    let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].starts_with(r#"{"status":"in-progress","metric":{"jobID":"r-1","#));
    assert!(lines[1].starts_with(r#"{"status":"complete","metric":{"jobID":"r-1","#));
    let recorded = server.join().unwrap();
    assert!(
        recorded[1].line.starts_with(
            "GET /minio/admin/v3/metrics?types=8&n=0&interval=1s&hosts=&disks=&by-jobID=r-1 "
        ),
        "{:?}",
        recorded[1]
    );
}

#[test]
fn batch_requires_its_arguments() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["batch", "generate", "a"][..],
        &["batch", "start", "a"],
        &["batch", "status", "a"],
        &["batch", "describe", "a"],
        &["batch", "cancel", "a"],
    ] {
        mx(home.path()).args(args).assert().failure();
    }
}

// ---------------------------------------------------------------------------
// sql
// ---------------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
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

/// One AWS event-stream message with string headers.
fn event(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut head = Vec::new();
    for (name, value) in headers {
        head.push(name.len() as u8);
        head.extend_from_slice(name.as_bytes());
        head.push(7);
        head.extend_from_slice(&(value.len() as u16).to_be_bytes());
        head.extend_from_slice(value.as_bytes());
    }
    let total = 12 + head.len() + payload.len() + 4;
    let mut msg = Vec::new();
    msg.extend_from_slice(&(total as u32).to_be_bytes());
    msg.extend_from_slice(&(head.len() as u32).to_be_bytes());
    let prelude_crc = crc32(&msg);
    msg.extend_from_slice(&prelude_crc.to_be_bytes());
    msg.extend_from_slice(&head);
    msg.extend_from_slice(payload);
    let crc = crc32(&msg);
    msg.extend_from_slice(&crc.to_be_bytes());
    msg
}

fn records(payload: &str) -> Vec<u8> {
    event(
        &[
            (":message-type", "event"),
            (":event-type", "Records"),
            (":content-type", "application/octet-stream"),
        ],
        payload.as_bytes(),
    )
}

fn end() -> Vec<u8> {
    event(&[(":message-type", "event"), (":event-type", "End")], b"")
}

fn head_ok() -> Reply {
    Reply {
        status: 200,
        headers: vec![("Content-Type", "text/csv"), ("ETag", "\"abc\"")],
        body: Vec::new(),
    }
}

fn select_reply(body: Vec<u8>) -> Reply {
    Reply {
        status: 200,
        headers: vec![("Content-Type", "application/octet-stream")],
        body,
    }
}

#[test]
fn sql_streams_records_and_builds_the_request() {
    let mut body = records("alice,30\n");
    body.extend(event(
        &[(":message-type", "event"), (":event-type", "Stats")],
        b"<Stats></Stats>",
    ));
    body.extend(records("bob,40\n"));
    body.extend(end());
    let (url, server) = fake_server(vec![head_ok(), select_reply(body)]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args([
            "sql",
            "--csv-output-header",
            "name,age",
            "--csv-output",
            "fd=;",
            "fake/b/data.csv",
        ])
        .assert()
        .success()
        .stdout("name,age\nalice,30\nbob,40\n");
    let recorded = server.join().unwrap();
    assert!(
        recorded[1].line.starts_with("POST /b/data.csv?select"),
        "{:?}",
        recorded[1]
    );
    let xml = String::from_utf8(recorded[1].body.clone()).unwrap();
    for part in [
        "<Expression>select * from s3object</Expression>",
        "<ExpressionType>SQL</ExpressionType>",
        "<CompressionType>NONE</CompressionType>",
        "<FileHeaderInfo>USE</FileHeaderInfo>",
        "<FieldDelimiter>;</FieldDelimiter>",
    ] {
        assert!(xml.contains(part), "{part} missing in {xml}");
    }
}

#[test]
fn sql_reports_error_events_and_keeps_exit_zero() {
    let body = event(
        &[
            (":message-type", "error"),
            (":error-code", "CSVParsingError"),
            (":error-message", "bad csv"),
        ],
        b"",
    );
    let (url, server) = fake_server(vec![head_ok(), select_reply(body)]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["sql", "fake/b/data.csv"])
        .assert()
        .success()
        .stderr("mx: <ERROR> Unable to run sql CSVParsingError:\"bad csv\"\n");
    server.join().unwrap();
}

#[test]
fn sql_option_errors_are_fatal() {
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("d.csv");
    std::fs::write(&file, "a,b\n1,2\n").unwrap();
    let file = file.to_str().unwrap();
    mx(home.path())
        .args(["sql", "--csv-input", "rd", file])
        .assert()
        .failure()
        .stderr("mx: <ERROR> Invalid serialization option(s) specified for --csv-input flag. Arguments should be of the form key=value,...\n");
    mx(home.path())
        .args(["sql", "--json-output", "fd=1", file])
        .assert()
        .failure()
        .stderr("mx: <ERROR> Invalid value(s) specified for --json-output flag. Options should be key-value pairs in the form key=value,... where valid key(s) are RecordDelimiter(rd).\n");
    mx(home.path())
        .args(["sql", "--csv-input", "fh=USE", "--json-input", "t=x", file])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Only one of --csv-input or --json-input can be specified as input serialization option. Invalid arguments provided",
        ));
    mx(home.path())
        .args(["sql", "--json-output", "", "--csv-output-header", "a", file])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--csv-output-header incompatible with --json-output option",
        ));
}

#[test]
fn sql_on_local_files_reports_like_mc() {
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("d.csv");
    std::fs::write(&file, "a,b\n1,2\n").unwrap();
    let file = file.to_str().unwrap();
    mx(home.path())
        .args(["sql", file])
        .assert()
        .success()
        .stderr("mx: <ERROR> Unable to run sql `Select` is not supported for `filesystem`.\n");
    mx(home.path())
        .args(["--json", "sql", file])
        .assert()
        .success()
        .stdout(r#"{"status":"error","error":{"message":"Unable to run sql","cause":{"message":"`Select` is not supported for `filesystem`.","error":{"API":"Select","APIType":"filesystem"}},"type":"error"}}
"#);
    let missing = home.path().join("nosuch.csv");
    let missing = missing.to_str().unwrap();
    mx(home.path())
        .args(["sql", missing])
        .assert()
        .success()
        .stderr(format!(
            "mx: <ERROR> Unable to run sql for {missing}. Requested path `{missing}` not found\n"
        ));
    mx(home.path()).args(["sql"]).assert().failure();
}
