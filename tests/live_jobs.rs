//! Live tests for `batch` (replicate to server 2, keyrotate with the server 1 KMS, expire,
//! list/describe/status/cancel) and `sql` (S3 Select) against MinIO (MX_LIVE_TESTS=1).
//! Batch replication needs server 2 reachable from server 1 (`MX_TEST_URL2_INTERNAL`); SSE-C
//! select needs the TLS server (MX_TEST_TLS_URL / MX_TEST_TLS_CA).

mod common;

use base64::Engine;
use common::live::{BucketOpts, Live, Tls};
use serde_json::Value;
use std::io::Write;
use std::time::Duration;

fn stdout(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

fn json(text: &str) -> Value {
    serde_json::from_str(text.lines().last().unwrap_or_default())
        .unwrap_or_else(|err| panic!("not JSON ({err}): {text}"))
}

/// Starts `yaml` as a batch job; returns the job ID.
fn start_job(live: &Live, yaml: &str) -> String {
    let file = live.local_file("job.yaml", yaml);
    let out = stdout(
        live.cmd()
            .args(["--json", "batch", "start", &live.alias])
            .arg(&file)
            .assert()
            .success(),
    );
    let doc = json(&out);
    assert_eq!(doc["status"], "success", "{out}");
    doc["result"]["id"].as_str().expect("job id").to_string()
}

/// Waits for the job to finish (`batch status --json` follows it); returns the last metric.
fn wait_job(live: &Live, id: &str) -> Value {
    let out = stdout(
        live.cmd()
            .args(["--json", "batch", "status", &live.alias, id])
            .timeout(Duration::from_secs(120))
            .assert()
            .success(),
    );
    let doc = json(&out);
    assert_eq!(doc["status"], "complete", "{out}");
    doc["metric"].clone()
}

fn put(live: &Live, target: &str, contents: &str) {
    live.cmd()
        .args(["pipe", target])
        .write_stdin(contents.to_string())
        .assert()
        .success();
}

#[test]
fn live_batch_generate_templates() {
    let Some(live) = Live::new() else { return };
    let out = stdout(
        live.cmd()
            .args(["batch", "generate", &live.alias, "expire"])
            .assert()
            .success(),
    );
    assert!(out.starts_with("expire:\n  apiVersion: v1\n"), "{out}");
    live.cmd()
        .args(["batch", "generate", &live.alias, "list"])
        .assert()
        .success()
        .stdout(predicates::str::contains("replicate"));
    live.cmd()
        .args(["batch", "generate", &live.alias, "nosuchtype"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "Unable to generate a job template for the specified job type",
        ));
}

#[test]
fn live_batch_expire_list_describe_cancel() {
    let Some(live) = Live::new() else { return };
    for key in ["old/a.txt", "old/b.txt", "keep/c.txt"] {
        put(&live, &live.url(key), "data");
    }
    let yaml = format!(
        "expire:\n  apiVersion: v1\n  bucket: {}\n  prefix: old/\n  rules:\n    - type: object\n      olderThan: 0s\n",
        live.bucket
    );
    let id = start_job(&live, &yaml);
    assert!(id.starts_with("expire-"), "{id}");
    let metric = wait_job(&live, &id);
    assert_eq!(metric["jobType"], "expire");
    assert_eq!(metric["expired"]["objects"], 2, "{metric}");

    let listing = stdout(
        live.cmd()
            .args(["ls", "-r", &live.url("")])
            .assert()
            .success(),
    );
    assert!(
        listing.contains("keep/c.txt") && !listing.contains("old/"),
        "{listing}"
    );

    let list = json(&stdout(
        live.cmd()
            .args(["--json", "batch", "list", &live.alias, "--type", "expire"])
            .assert()
            .success(),
    ));
    let job = list["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["id"] == id.as_str())
        .unwrap_or_else(|| panic!("{id} not listed: {list}"))
        .clone();
    assert_eq!(job["status"], "completed");
    assert_eq!(job["type"], "expire");
    let table = stdout(
        live.cmd()
            .args(["batch", "list", &live.alias])
            .assert()
            .success(),
    );
    assert!(table.starts_with("ID "), "{table}");
    assert!(
        table
            .lines()
            .any(|line| line.starts_with(&format!("{id}\t")) && line.contains("\tcompleted")),
        "{table}"
    );

    let describe = stdout(
        live.cmd()
            .args(["batch", "describe", &live.alias, &id])
            .assert()
            .success(),
    );
    assert!(
        describe.contains("expire:") && describe.contains(&live.bucket),
        "{describe}"
    );
    live.cmd()
        .args(["batch", "cancel", &live.alias, &id])
        .assert()
        .success()
        .stdout(format!("Successfully canceled batch job `{id}`\n"));
    live.cmd()
        .args(["batch", "describe", &live.alias, "nosuchjob"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "Unable to fetch the job definition. The specified job does not exist.",
        ));
}

#[test]
fn live_batch_replicate_to_second_server() {
    let Some(live) = Live::new() else { return };
    let Some(target_bucket) = live.make_bucket2(BucketOpts::default()) else {
        eprintln!("skipping: second server not configured");
        return;
    };
    let Ok(endpoint) = std::env::var("MX_TEST_URL2_INTERNAL") else {
        eprintln!("skipping: MX_TEST_URL2_INTERNAL not set");
        return;
    };
    let access = std::env::var("MX_TEST_ACCESS_KEY2").unwrap();
    let secret = std::env::var("MX_TEST_SECRET_KEY2").unwrap();
    put(&live, &live.url("r/one.txt"), "one");
    put(&live, &live.url("r/two/three.txt"), "three");
    let yaml = format!(
        "replicate:\n  apiVersion: v1\n  source:\n    type: minio\n    bucket: {}\n  target:\n    type: minio\n    bucket: {target_bucket}\n    endpoint: \"{endpoint}\"\n    credentials:\n      accessKey: {access}\n      secretKey: {secret}\n",
        live.bucket
    );
    let id = start_job(&live, &yaml);
    let metric = wait_job(&live, &id);
    assert_eq!(metric["replicate"]["objects"], 2, "{metric}");
    assert_eq!(metric["replicate"]["bytesTransferred"], 8, "{metric}");
    let alias2 = live.alias2.clone().unwrap();
    live.cmd()
        .args(["cat", &format!("{alias2}/{target_bucket}/r/two/three.txt")])
        .assert()
        .success()
        .stdout("three");
    // Secrets are redacted in the stored definition.
    let describe = stdout(
        live.cmd()
            .args(["batch", "describe", &live.alias, &id])
            .assert()
            .success(),
    );
    assert!(
        describe.contains("**REDACTED**") && !describe.contains(&format!("secretKey: {secret}\n")),
        "{describe}"
    );
}

#[test]
fn live_batch_keyrotate_with_kms() {
    let Some(live) = Live::new() else { return };
    live.cmd()
        .args([
            "pipe",
            "--enc-s3",
            &live.bucket_target(),
            &live.url("k/x.txt"),
        ])
        .write_stdin("secret")
        .assert()
        .success();
    let yaml = format!(
        "keyrotate:\n  apiVersion: v1\n  bucket: {}\n  prefix: k/\n  encryption:\n    type: sse-kms\n    key: mx-test-key\n",
        live.bucket
    );
    let id = start_job(&live, &yaml);
    assert!(id.starts_with("keyrotate-"), "{id}");
    let metric = wait_job(&live, &id);
    assert_eq!(metric["rotation"]["objects"], 1, "{metric}");
    assert_eq!(metric["rotation"]["objectsFailed"], 0, "{metric}");
    live.cmd()
        .args(["cat", &live.url("k/x.txt")])
        .assert()
        .success()
        .stdout("secret");
}

#[test]
fn live_batch_errors() {
    let Some(live) = Live::new() else { return };
    let bad = live.local_file("bad.yaml", "garbage: [");
    live.cmd()
        .args(["batch", "start", &live.alias])
        .arg(&bad)
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "mx: <ERROR> Unable to start job.",
        ));
    live.cmd()
        .args(["--json", "batch", "status", &live.alias, "nosuchjob"])
        .assert()
        .failure()
        .stdout(predicates::str::contains("Unable to lookup job status"));
}

// ---------------------------------------------------------------------------
// sql
// ---------------------------------------------------------------------------

const CSV: &str = "name,age\nalice,30\nbob,40\n";

#[test]
fn live_sql_csv_and_json() {
    let Some(live) = Live::new() else { return };
    put(&live, &live.url("d.csv"), CSV);
    put(&live, &live.url("d.json"), "{\"a\":1}\n{\"a\":2}\n");
    live.cmd()
        .args(["sql", &live.url("d.csv")])
        .assert()
        .success()
        .stdout("alice,30\nbob,40\n");
    live.cmd()
        .args([
            "sql",
            "-e",
            "select s.name from S3Object s",
            &live.url("d.csv"),
        ])
        .assert()
        .success()
        .stdout("alice\nbob\n");
    live.cmd()
        .args(["--json", "sql", &live.url("d.csv")])
        .assert()
        .success()
        .stdout("{\"name\":\"alice\",\"age\":\"30\"}\n{\"name\":\"bob\",\"age\":\"40\"}\n");
    live.cmd()
        .args(["sql", "--csv-output-header", "", &live.url("d.csv")])
        .assert()
        .success()
        .stdout(format!("name,age\n{}", "alice,30\nbob,40\n"));
    live.cmd()
        .args(["sql", "--csv-output", "fd=;", &live.url("d.csv")])
        .assert()
        .success()
        .stdout("alice;30\nbob;40\n");
    live.cmd()
        .args(["sql", "--csv-input", "fh=NONE", &live.url("d.csv")])
        .assert()
        .success()
        .stdout(CSV);
    live.cmd()
        .args([
            "sql",
            "-e",
            "select s.a from S3Object s",
            &live.url("d.json"),
        ])
        .assert()
        .success()
        .stdout("{\"a\":1}\n{\"a\":2}\n");
    // Server-side errors are reported, exit status stays 0 like mc.
    live.cmd()
        .args(["sql", "-e", "selec bad", &live.url("d.csv")])
        .assert()
        .success()
        .stderr("mx: <ERROR> Unable to run sql 1:1: unexpected token \"selec\"\n");
    live.cmd()
        .args(["sql", &live.url("missing.csv")])
        .assert()
        .success()
        .stderr(format!(
            "mx: <ERROR> Unable to run sql for {}. Object does not exist\n",
            live.url("missing.csv")
        ));
}

#[test]
fn live_sql_compressed_and_recursive() {
    let Some(live) = Live::new() else { return };
    let gz = live.home.path().join("data.csv.gz");
    let mut encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(&gz).unwrap(),
        flate2::Compression::default(),
    );
    encoder.write_all(CSV.as_bytes()).unwrap();
    encoder.finish().unwrap();
    live.cmd()
        .arg("cp")
        .arg(&gz)
        .arg(live.url("dir/data.csv.gz"))
        .assert()
        .success();
    put(&live, &live.url("dir/sub/more.csv"), "name,age\ncarol,50\n");
    put(&live, &live.url("dir/notes.txt"), "not queried");
    live.cmd()
        .args(["sql", &live.url("dir/data.csv.gz")])
        .assert()
        .success()
        .stdout("alice,30\nbob,40\n");
    live.cmd()
        .args([
            "sql",
            "--csv-output-header",
            "",
            &live.url("dir/data.csv.gz"),
        ])
        .assert()
        .success()
        .stdout("name,age\nalice,30\nbob,40\n");
    // Non-recursive: only direct children; `.txt` objects are skipped.
    live.cmd()
        .args(["sql", &live.url("dir/")])
        .assert()
        .success()
        .stdout("alice,30\nbob,40\n");
    live.cmd()
        .args(["sql", "-r", &live.url("dir/")])
        .assert()
        .success()
        .stdout("alice,30\nbob,40\ncarol,50\n");
    // A bucket without a trailing slash lists as the bucket entry itself: nothing to query.
    live.cmd()
        .args(["sql", &live.bucket_target()])
        .assert()
        .success()
        .stdout("");
}

#[test]
fn live_sql_sse_c() {
    let Some(tls) = Tls::new() else { return };
    tls.trust_ca();
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let enc = format!("{}={key}", tls.target("enc/"));
    let file = tls.home.path().join("d.csv");
    std::fs::write(&file, CSV).unwrap();
    tls.cmd()
        .args(["put", "--enc-c", &enc])
        .arg(&file)
        .arg(tls.target("enc/d.csv"))
        .assert()
        .success();
    tls.cmd()
        .args(["sql", "--enc-c", &enc, &tls.target("enc/d.csv")])
        .assert()
        .success()
        .stdout("alice,30\nbob,40\n");
    // Without the key the object cannot even be stat'ed.
    tls.cmd()
        .args(["sql", &tls.target("enc/d.csv")])
        .assert()
        .success()
        .stderr(predicates::str::contains("Unable to run sql"));
}
