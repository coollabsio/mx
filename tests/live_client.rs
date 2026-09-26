//! Live tests for the client layer: S3 Signature V2 aliases, `alias set` signature probing,
//! the TLS trust prompt, connection deadlines and `-H`/`--debug` on admin requests.
//!
//! Needs MX_LIVE_TESTS=1 and the test server (tests/live_minio.sh). TLS cases also need
//! MX_TEST_TLS_URL (CA-signed server) and MX_TEST_SELFSIGNED_URL (tests/services/
//! client_selfsigned.sh); the trust prompt needs `script` (util-linux).

mod common;

use common::live::{self, Live};
use predicates::prelude::*;
use std::path::Path;

fn creds() -> (String, String, String) {
    (
        std::env::var("MX_TEST_URL").expect("MX_TEST_URL"),
        std::env::var("MX_TEST_ACCESS_KEY").expect("MX_TEST_ACCESS_KEY"),
        std::env::var("MX_TEST_SECRET_KEY").expect("MX_TEST_SECRET_KEY"),
    )
}

/// Adds alias `v2` (`--api S3v2`) for the test server to the fixture home.
fn set_v2_alias(live: &Live) {
    let (url, access, secret) = creds();
    live.cmd()
        .args([
            "alias", "set", "v2", &url, &access, &secret, "--api", "S3v2",
        ])
        .assert()
        .success();
}

fn stdout(out: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&out.get_output().stdout).into_owned()
}

/// Plain `curl URL` body (None when curl is missing).
fn curl(url: &str) -> Option<(String, String)> {
    let out = std::process::Command::new("curl")
        .args(["-s", "-w", "\n%{http_code}", url])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let (body, code) = text.rsplit_once('\n')?;
    Some((body.to_string(), code.to_string()))
}

#[test]
fn live_s3v2_alias_runs_core_commands() {
    let Some(live) = Live::new() else { return };
    set_v2_alias(&live);
    let bucket = format!("v2/{}", live.make_bucket(Default::default()));
    let file = live.local_file("hello.txt", "hello v2");

    live.cmd()
        .args([
            "put",
            file.to_str().unwrap(),
            &format!("{bucket}/dir/a b+c.txt"),
        ])
        .assert()
        .success();
    live.cmd()
        .args(["cp", file.to_str().unwrap(), &format!("{bucket}/plain.txt")])
        .assert()
        .success();
    let out = live.cmd().args(["ls", "-r", &bucket]).assert().success();
    let listing = stdout(&out);
    assert!(listing.contains("dir/a b+c.txt"), "{listing}");
    assert!(listing.contains("plain.txt"), "{listing}");
    live.cmd()
        .args(["cat", &format!("{bucket}/dir/a b+c.txt")])
        .assert()
        .success()
        .stdout("hello v2");
    live.cmd()
        .args(["stat", &format!("{bucket}/plain.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains("Size      : 8 B"));
    // Server-side copy and a multipart upload are signed with V2 as well.
    live.cmd()
        .args([
            "cp",
            &format!("{bucket}/plain.txt"),
            &format!("{bucket}/copy.txt"),
        ])
        .assert()
        .success();
    let big = live.home.path().join("big.bin");
    std::fs::write(&big, vec![5u8; 17 << 20]).unwrap();
    live.cmd()
        .args(["put", big.to_str().unwrap(), &format!("{bucket}/big.bin")])
        .assert()
        .success();
    live.cmd()
        .args(["stat", &format!("{bucket}/big.bin")])
        .assert()
        .success()
        .stdout(predicate::str::contains("Size      : 17 MiB"));

    // Presigned download (query auth V2) works without credentials.
    let out = live
        .cmd()
        .args([
            "--json",
            "share",
            "download",
            &format!("{bucket}/dir/a b+c.txt"),
        ])
        .assert()
        .success();
    let doc: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    let share = doc["share"].as_str().unwrap().to_string();
    assert!(share.contains("?AWSAccessKeyId="), "{share}");
    assert!(share.contains("&Signature="), "{share}");
    if let Some((body, code)) = curl(&share) {
        assert_eq!((body.as_str(), code.as_str()), ("hello v2", "200"));
        let tampered = share.replace("AWSAccessKeyId=", "AWSAccessKeyId=x");
        assert_eq!(curl(&tampered).unwrap().1, "403");
    }

    // Presigned upload (POST policy V2).
    let out = live
        .cmd()
        .args(["share", "upload", &format!("{bucket}/uploaded.txt")])
        .assert()
        .success();
    let text = stdout(&out);
    let command = text
        .lines()
        .find_map(|line| line.strip_prefix("Share: "))
        .unwrap()
        .replace("<FILE>", file.to_str().unwrap());
    assert!(command.contains("-F signature="), "{command}");
    if curl(&share).is_some() {
        let status = std::process::Command::new("sh")
            .args([
                "-c",
                &format!("{command} -s -o /dev/null -w '%{{http_code}}'"),
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout), "204");
        live.cmd()
            .args(["cat", &format!("{bucket}/uploaded.txt")])
            .assert()
            .success()
            .stdout("hello v2");
    }

    live.cmd()
        .args(["rm", "-r", "--force", &bucket])
        .assert()
        .success();
    live.cmd().args(["rb", &bucket]).assert().success();
    live.cmd()
        .args(["ls", &bucket])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
}

#[test]
fn live_s3v2_debug_trace_redacts_signature() {
    let Some(live) = Live::new() else { return };
    set_v2_alias(&live);
    let out = live
        .cmd()
        .args(["--debug", "ls", &format!("v2/{}", live.bucket)])
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        stderr.contains("Authorization: AWS **REDACTED**:**REDACTED**"),
        "{stderr}"
    );
    let (_, access, _) = creds();
    assert!(!stderr.contains(&format!("AWS {access}:")));
}

#[test]
fn live_alias_set_probes_s3v4() {
    if !live::enabled() {
        return;
    }
    let (url, access, secret) = creds();
    let home = live::temp_home();
    live::mx()
        .env("HOME", home.path())
        .args(["--json", "alias", "set", "probed", &url, &access, &secret])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"api\":\"s3v4\""));
    let config = live::alias_config_from_home(home.path(), "probed");
    assert_eq!(config.api, "s3v4");
}

// ---------------------------------------------------------------------------
// Connection deadlines
// ---------------------------------------------------------------------------

#[test]
fn live_conn_deadlines_are_per_read_and_write() {
    let Some(live) = Live::new() else { return };
    for flag in ["--conn-read-deadline", "--conn-write-deadline"] {
        live.cmd()
            .args(["ls", flag, "1ns", &live.bucket_target()])
            .assert()
            .failure()
            .stderr(
                predicate::str::is_match(r"(read|write) tcp [0-9.:]+->[0-9.:]+: i/o timeout")
                    .unwrap(),
            );
    }
    let file = live.local_file("deadline.txt", "deadline");
    live.cmd()
        .args([
            "--conn-read-deadline",
            "1m",
            "--conn-write-deadline",
            "1m",
            "cp",
            file.to_str().unwrap(),
            &live.url("deadline.txt"),
        ])
        .assert()
        .success();
    live.cmd()
        .args([
            "cat",
            "--conn-read-deadline",
            "1m",
            &live.url("deadline.txt"),
        ])
        .assert()
        .success()
        .stdout("deadline");
}

#[test]
fn live_conn_deadlines_apply_to_tls_and_admin() {
    let Some(tls) = live::Tls::new() else { return };
    tls.trust_ca();
    tls.cmd()
        .args([
            "ls",
            "--conn-read-deadline",
            "1m",
            &format!("tls/{}", tls.bucket),
        ])
        .assert()
        .success();
    tls.cmd()
        .args([
            "ls",
            "--conn-read-deadline",
            "1ns",
            &format!("tls/{}", tls.bucket),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("i/o timeout"));
    tls.cmd()
        .args([
            "quota",
            "info",
            "--conn-read-deadline",
            "1ns",
            &format!("tls/{}", tls.bucket),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("i/o timeout"));
    tls.cmd()
        .args([
            "quota",
            "info",
            "--conn-read-deadline",
            "1m",
            &format!("tls/{}", tls.bucket),
        ])
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// Admin requests: -H / --debug
// ---------------------------------------------------------------------------

#[test]
fn live_admin_requests_honor_custom_headers_and_debug() {
    let Some(live) = Live::new() else { return };
    let out = live
        .cmd()
        .args([
            "--debug",
            "-H",
            "X-Mx-Live: admin",
            "quota",
            "info",
            &live.bucket_target(),
        ])
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        stderr.contains("mx: <DEBUG> GET /minio/admin/v3/get-bucket-quota?bucket="),
        "{stderr}"
    );
    assert!(stderr.contains("X-Mx-Live: admin"), "{stderr}");
    assert!(stderr.contains("Signature=**REDACTED**"), "{stderr}");
    assert!(stderr.contains("mx: <DEBUG> HTTP/1.1 200 OK"), "{stderr}");
}

// ---------------------------------------------------------------------------
// TLS trust prompt
// ---------------------------------------------------------------------------

fn selfsigned_url() -> Option<String> {
    if !live::enabled() {
        return None;
    }
    let url = std::env::var("MX_TEST_SELFSIGNED_URL").ok();
    if url.is_none() {
        eprintln!("skipping; MX_TEST_SELFSIGNED_URL not set (tests/services/client_selfsigned.sh)");
    }
    url
}

fn tty_alias_set(home: &Path, url: &str, answer: &str) -> (String, bool) {
    let (_, access, secret) = creds();
    common::tty::run(
        &assert_cmd::cargo::cargo_bin("mx"),
        home,
        &format!("alias set tofu {url} {access} {secret}"),
        answer,
    )
}

#[test]
fn live_alias_set_trusts_confirmed_self_signed_certificate() {
    let Some(url) = selfsigned_url() else { return };
    if !common::tty::have("script") {
        eprintln!("skipping; needs `script`");
        return;
    }
    let home = live::temp_home();
    let saved = home.path().join(".mx/certs/CAs/tofu.crt");

    let (output, success) = tty_alias_set(home.path(), &url, "no\n");
    assert!(!success, "{output}");
    assert!(output.contains("Confirm public key y/N: "), "{output}");
    assert!(
        output.contains(&format!(
            "Unable to initialize new alias from the provided credentials. Get \"{url}\": tls: failed to verify certificate: x509: certificate signed by unknown authority."
        )),
        "{output}"
    );
    assert!(!saved.exists());

    let (output, success) = tty_alias_set(home.path(), &url, "yes\n");
    assert!(success, "{output}");
    assert!(output.contains("Added `tofu` successfully."), "{output}");
    let cert = std::fs::read_to_string(std::env::var("MX_TEST_SELFSIGNED_CERT").unwrap()).unwrap();
    let saved_pem = std::fs::read_to_string(&saved).unwrap();
    assert_eq!(saved_pem.trim(), cert.trim());
    assert_eq!(
        live::alias_config_from_home(home.path(), "tofu").api,
        "s3v4"
    );

    // Later commands (S3 and admin) trust the saved certificate without --insecure.
    let bucket = format!("tofu/{}", live::unique_bucket_name());
    live::mx()
        .env("HOME", home.path())
        .args(["mb", &bucket])
        .assert()
        .success();
    live::mx()
        .env("HOME", home.path())
        .args(["quota", "info", &bucket])
        .assert()
        .success();
    live::mx()
        .env("HOME", home.path())
        .args(["rb", &bucket])
        .assert()
        .success();
    // A second `alias set` on the terminal no longer prompts.
    let (output, success) = tty_alias_set(home.path(), &url, "");
    assert!(success, "{output}");
    assert!(!output.contains("Fingerprint"), "{output}");
}

#[test]
fn live_alias_set_does_not_prompt_without_tty_or_for_ca_issued_certs() {
    let Some(url) = selfsigned_url() else { return };
    let (_, access, secret) = creds();
    let home = live::temp_home();
    live::mx()
        .env("HOME", home.path())
        .args(["alias", "set", "tofu", &url, &access, &secret])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "x509: certificate signed by unknown authority",
        ));
    assert!(!home.path().join(".mx/certs/CAs").join("tofu.crt").exists());

    // The CA-issued TLS server: the leaf is not self-signed, so mc reports the error on a
    // terminal as well, without prompting.
    let Ok(tls_url) = std::env::var("MX_TEST_TLS_URL") else {
        return;
    };
    if !common::tty::have("script") {
        return;
    }
    let (output, success) = tty_alias_set(home.path(), &tls_url, "y\n");
    assert!(!success, "{output}");
    assert!(!output.contains("Fingerprint"), "{output}");
    assert!(
        output.contains(&format!(
            "Get \"{tls_url}\": tls: failed to verify certificate: x509: certificate signed by unknown authority."
        )),
        "{output}"
    );
}
