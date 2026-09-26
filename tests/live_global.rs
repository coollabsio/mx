//! Live tests for global flags: TLS (`--insecure`, `certs/CAs`), `--resolve` over TLS,
//! `-H`, `--limit-upload/--limit-download`, `--debug`, and an SSE-C round trip over TLS.
//! TLS tests need MX_TEST_TLS_URL / MX_TEST_TLS_CA (set by tests/live_minio.sh).

mod common;

use common::live::{Live, enabled, mx, unique_bucket_name};
use std::path::Path;
use std::time::{Duration, Instant};

/// Temp HOME with alias `tls` for the TLS server and one bucket (made with `--insecure`).
struct Tls {
    home: tempfile::TempDir,
    url: String,
    ca: String,
    bucket: String,
}

impl Tls {
    fn new() -> Option<Self> {
        if !enabled() {
            eprintln!("skipping live test; set MX_LIVE_TESTS=1");
            return None;
        }
        let (Ok(url), Ok(ca)) = (
            std::env::var("MX_TEST_TLS_URL"),
            std::env::var("MX_TEST_TLS_CA"),
        ) else {
            eprintln!("skipping TLS live test; set MX_TEST_TLS_URL and MX_TEST_TLS_CA");
            return None;
        };
        let home = tempfile::tempdir().unwrap();
        set_alias(home.path(), "tls", &url);
        let bucket = unique_bucket_name();
        mx().env("HOME", home.path())
            .args(["--insecure", "mb", &format!("tls/{bucket}")])
            .assert()
            .success();
        Some(Self {
            home,
            url,
            ca,
            bucket,
        })
    }

    fn cmd(&self) -> assert_cmd::Command {
        let mut command = mx();
        command.env("HOME", self.home.path());
        command
    }

    fn target(&self, path: &str) -> String {
        format!("tls/{}/{path}", self.bucket)
    }
}

impl Drop for Tls {
    fn drop(&mut self) {
        let _ = self
            .cmd()
            .args([
                "--insecure",
                "rb",
                "--force",
                &format!("tls/{}", self.bucket),
            ])
            .output();
    }
}

fn set_alias(home: &Path, alias: &str, url: &str) {
    let access_key = std::env::var("MX_TEST_ACCESS_KEY").unwrap();
    let secret_key = std::env::var("MX_TEST_SECRET_KEY").unwrap();
    mx().env("HOME", home)
        .args(["alias", "set", alias, url, &access_key, &secret_key])
        .assert()
        .success();
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn live_tls_requires_trust_or_insecure() {
    let Some(tls) = Tls::new() else { return };
    let file = tls.home.path().join("hello.txt");
    std::fs::write(&file, "hello tls").unwrap();

    // Self-signed CA: rejected by default.
    let output = tls.cmd().args(["ls", "tls"]).output().unwrap();
    assert!(!output.status.success());
    assert!(
        stderr(&output).to_lowercase().contains("certificate")
            || stderr(&output).contains("dispatch failure"),
        "{}",
        stderr(&output)
    );

    // --insecure skips verification.
    tls.cmd()
        .args([
            "--insecure",
            "put",
            file.to_str().unwrap(),
            &tls.target("hello.txt"),
        ])
        .assert()
        .success();
    tls.cmd()
        .args(["--insecure", "cat", &tls.target("hello.txt")])
        .assert()
        .success()
        .stdout("hello tls");

    // CA in ~/.mx/certs/CAs is trusted without --insecure.
    let cas = tls.home.path().join(".mx/certs/CAs");
    std::fs::create_dir_all(&cas).unwrap();
    std::fs::copy(&tls.ca, cas.join("ca.crt")).unwrap();
    tls.cmd()
        .args(["cat", &tls.target("hello.txt")])
        .assert()
        .success()
        .stdout("hello tls");

    // --config-dir selects the CA directory too.
    let config_dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        tls.home.path().join(".mx/config.json"),
        config_dir.path().join("config.json"),
    )
    .unwrap();
    let output = tls
        .cmd()
        .args(["-C", config_dir.path().to_str().unwrap(), "ls", "tls"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "no CA in the config dir yet");
    let cas = config_dir.path().join("certs/CAs");
    std::fs::create_dir_all(&cas).unwrap();
    std::fs::copy(&tls.ca, cas.join("ca.pem")).unwrap();
    tls.cmd()
        .args([
            "-C",
            config_dir.path().to_str().unwrap(),
            "ls",
            &tls.target(""),
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("hello.txt"));
}

#[test]
fn live_tls_insecure_with_resolve() {
    let Some(tls) = Tls::new() else { return };
    let port = url::Url::parse(&tls.url).unwrap().port().unwrap();
    // The certificate does not cover this name, and it does not resolve without --resolve.
    set_alias(
        tls.home.path(),
        "pinned",
        &format!("https://mx-pinned.invalid:{port}"),
    );
    let resolve = format!("mx-pinned.invalid:{port}=127.0.0.1");
    tls.cmd()
        .args(["--insecure", "--resolve", &resolve, "ls", "pinned"])
        .assert()
        .success()
        .stdout(predicates::str::contains(&tls.bucket));
    tls.cmd()
        .args(["--insecure", "ls", "pinned"])
        .assert()
        .failure();
}

#[test]
fn live_sse_c_round_trip_over_tls() {
    let Some(tls) = Tls::new() else { return };
    mx::globals::init(mx::globals::Globals {
        insecure: true,
        ..Default::default()
    });
    let alias = common::live::alias_config_from_home(tls.home.path(), "tls");
    let key = [7u8; 32];
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let client = mx::s3::build_client(&alias).await.unwrap();
        let put = mx::s3::PutOptions {
            sse: Some(mx::s3::Sse::C { key }),
            ..Default::default()
        };
        mx::s3::put_object_with(
            &client,
            &tls.bucket,
            "secret.txt",
            b"sse-c data".to_vec(),
            &put,
        )
        .await
        .unwrap();
        let get = mx::s3::GetOptions {
            sse_c: Some(key),
            ..Default::default()
        };
        let body = mx::s3::get_object_bytes_with(&alias, &tls.bucket, "secret.txt", &get)
            .await
            .unwrap();
        assert_eq!(body, b"sse-c data");
        let without_key = mx::s3::get_object_bytes_with(
            &alias,
            &tls.bucket,
            "secret.txt",
            &mx::s3::GetOptions::default(),
        )
        .await;
        assert!(without_key.is_err());
    });
    // The SSE-C key is redacted in the debug trace.
    let output = tls
        .cmd()
        .args(["--insecure", "--debug", "stat", &tls.target("secret.txt")])
        .output()
        .unwrap();
    assert!(!stderr(&output).contains(&base64_key(&key)));
}

fn base64_key(key: &[u8; 32]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(key)
}

#[test]
fn live_custom_header_reaches_server() {
    let Some(live) = Live::new() else { return };
    let file = live.local_file("meta.txt", "meta");
    live.cmd()
        .args([
            "-H",
            "x-amz-meta-mxheader: from-flag",
            "put",
            file.to_str().unwrap(),
            &live.url("meta.txt"),
        ])
        .assert()
        .success();
    // The server stored the header as user metadata: HEAD returns it.
    let output = live
        .cmd()
        .args(["--debug", "stat", &live.url("meta.txt")])
        .output()
        .unwrap();
    assert!(output.status.success());
    let err = stderr(&output);
    assert!(err.contains("X-Amz-Meta-Mxheader: from-flag"), "{err}");
}

#[test]
fn live_debug_prints_redacted_trace() {
    let Some(live) = Live::new() else { return };
    let output = live
        .cmd()
        .args(["--debug", "-H", "X-Mx-Trace: yes", "ls", &live.url("")])
        .output()
        .unwrap();
    assert!(output.status.success());
    let err = stderr(&output);
    assert!(err.contains("mx: <DEBUG> GET /"), "{err}");
    assert!(err.contains("X-Mx-Trace: yes"), "{err}");
    assert!(err.contains("Credential=**REDACTED**/"), "{err}");
    assert!(err.contains("Signature=**REDACTED**"), "{err}");
    assert!(err.contains("HTTP/1.1 200 OK"), "{err}");
    assert!(err.contains("Response Time:"), "{err}");
    let access_key = std::env::var("MX_TEST_ACCESS_KEY").unwrap();
    assert!(!err.contains(&format!("Credential={access_key}")), "{err}");
    // Without --debug nothing is traced.
    let output = live.cmd().args(["ls", &live.url("")]).output().unwrap();
    assert!(!stderr(&output).contains("<DEBUG>"));
}

#[test]
fn live_limit_upload_and_download_throttle() {
    let Some(live) = Live::new() else { return };
    let data = "x".repeat(512 * 1024);
    let file = live.local_file("big.bin", &data);

    let started = Instant::now();
    live.cmd()
        .args([
            "--limit-upload",
            "256KiB",
            "put",
            file.to_str().unwrap(),
            &live.url("big.bin"),
        ])
        .assert()
        .success();
    let upload = started.elapsed();
    assert!(
        upload >= Duration::from_millis(1500),
        "upload took {upload:?}"
    );

    let started = Instant::now();
    let output = live
        .cmd()
        .args(["--limit-download", "256KiB", "cat", &live.url("big.bin")])
        .output()
        .unwrap();
    let download = started.elapsed();
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), data.len());
    assert!(
        download >= Duration::from_millis(1500),
        "download took {download:?}"
    );

    // Unthrottled for comparison.
    let started = Instant::now();
    live.cmd()
        .args(["cat", &live.url("big.bin")])
        .assert()
        .success();
    assert!(started.elapsed() < Duration::from_millis(1500));
}
