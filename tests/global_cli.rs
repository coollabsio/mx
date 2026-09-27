//! Offline checks for global flags: parsing, validation, CA loading and the `--debug` trace.

use assert_cmd::Command;
use predicates::prelude::*;

fn mx(home: &std::path::Path) -> Command {
    let mut command = Command::cargo_bin("mx").expect("binary");
    command.env("HOME", home);
    command
}

/// Alias `dead` pointing at a closed local port.
fn dead_alias(home: &std::path::Path) {
    mx(home)
        .args([
            "alias",
            "set",
            "--api",
            "S3v4",
            "dead",
            "http://127.0.0.1:1",
            "akey",
            "skey1234",
        ])
        .assert()
        .success();
}

#[test]
fn accepts_output_flags_anywhere() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["--dp", "alias", "list"][..],
        &["--disable-pager", "alias", "list"],
        &["--no-color", "alias", "list"],
        &["alias", "list", "--dp", "--no-color"],
        &["--debug", "alias", "list"],
        &["--insecure", "alias", "list"],
        &[
            "--limit-upload",
            "1MiB",
            "--limit-download",
            "500 KiB",
            "alias",
            "list",
        ],
        &["-H", "x-a:1", "--custom-header", "x-b: 2", "alias", "list"],
    ] {
        mx(home.path())
            .args(args)
            .assert()
            .success()
            .stdout(predicate::str::contains("play"));
    }
}

#[test]
fn rejects_invalid_flag_values() {
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args(["--limit-upload", "fast", "alias", "list"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--limit-upload"));
    mx(home.path())
        .args(["--limit-download", "10XB", "alias", "list"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unhandled size name"));
    mx(home.path())
        .args(["-H", "no-colon", "alias", "list"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid custom header entry"));
    mx(home.path())
        .args(["-H", "bad name:v", "alias", "list"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid custom header entry"));
}

#[test]
fn debug_traces_requests_with_custom_headers() {
    let home = tempfile::tempdir().unwrap();
    dead_alias(home.path());
    mx(home.path())
        .args(["--debug", "-H", "X-Mx-Test: hello", "ls", "dead/bucket/"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("mx: <DEBUG> GET /bucket"))
        .stderr(predicate::str::contains("Host: 127.0.0.1:1"))
        .stderr(predicate::str::contains("X-Mx-Test: hello"))
        .stderr(predicate::str::contains("Credential=**REDACTED**/"))
        .stderr(predicate::str::contains("Signature=**REDACTED**"))
        .stderr(predicate::str::contains("Credential=ak/").not());
    mx(home.path())
        .args(["ls", "dead/bucket/"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("<DEBUG>").not());
}

#[test]
fn invalid_ca_file_is_reported() {
    let home = tempfile::tempdir().unwrap();
    dead_alias(home.path());
    let cas = home.path().join(".mx/certs/CAs");
    std::fs::create_dir_all(&cas).unwrap();
    std::fs::write(
        cas.join("broken.pem"),
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    )
    .unwrap();
    mx(home.path())
        .args(["ls", "dead/bucket/"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("broken.pem"));
}

#[test]
fn valid_ca_files_and_other_files_are_accepted() {
    let home = tempfile::tempdir().unwrap();
    dead_alias(home.path());
    let cas = home.path().join(".mx/certs/CAs");
    std::fs::create_dir_all(&cas).unwrap();
    std::fs::write(cas.join("ca.crt"), include_str!("fixtures/test-ca.pem")).unwrap();
    std::fs::write(cas.join("notes.txt"), "not a certificate").unwrap();
    // Fails only because nothing listens on the port, not because of the CA directory.
    mx(home.path())
        .args(["ls", "dead/bucket/"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("CA").not());
}

#[test]
fn insecure_client_reaches_the_network_layer() {
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias",
            "set",
            "--api",
            "S3v4",
            "deadtls",
            "https://127.0.0.1:1",
            "akey",
            "skey1234",
        ])
        .assert()
        .success();
    mx(home.path())
        .args(["--insecure", "--debug", "ls", "deadtls/bucket/"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("mx: <DEBUG> GET /bucket"));
}
