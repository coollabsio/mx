//! Offline CLI checks for retention, legalhold, event, undo, od, and ilm restore (area G).
//! Validation errors must happen before any network access; the alias points at a closed port.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

fn home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    mx(&home)
        .args([
            "alias",
            "set",
            "--api",
            "S3v4",
            "dead",
            "http://127.0.0.1:1",
            "access",
            "secret12",
        ])
        .assert()
        .success();
    home
}

fn mx(home: &tempfile::TempDir) -> Command {
    let mut command = Command::cargo_bin("mx").expect("binary");
    command.env("HOME", home.path());
    command
}

fn fails_with(home: &tempfile::TempDir, args: &[&str], message: &str) {
    mx(home)
        .args(args)
        .assert()
        .failure()
        .stderr(predicate::str::contains(message));
}

#[test]
fn help_lists_lock_commands() {
    let home = home();
    mx(&home)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("retention"))
        .stdout(predicate::str::contains("legalhold"))
        .stdout(predicate::str::contains("event"))
        .stdout(predicate::str::contains("undo"))
        .stdout(predicate::str::contains("od"));
    mx(&home)
        .args(["ilm", "restore", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--days"))
        .stdout(predicate::str::contains("--versions"));
    mx(&home)
        .args(["retention", "set", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--bypass"))
        .stdout(predicate::str::contains("--default"))
        .stdout(predicate::str::contains("--rewind"));
}

#[test]
fn retention_rejects_bad_arguments() {
    let home = home();
    fails_with(
        &home,
        &["retention", "set", "legal", "30d", "dead/b/o"],
        "invalid retention mode",
    );
    fails_with(
        &home,
        &["retention", "set", "governance", "30w", "dead/b/o"],
        "invalid validity",
    );
    fails_with(
        &home,
        &["retention", "set", "governance", "0d", "dead/b/o"],
        "greater than 0",
    );
    fails_with(
        &home,
        &[
            "retention",
            "set",
            "--default",
            "-r",
            "governance",
            "1d",
            "dead/b",
        ],
        "--default cannot be specified",
    );
    fails_with(
        &home,
        &[
            "retention",
            "set",
            "--default",
            "--bypass",
            "governance",
            "1d",
            "dead/b",
        ],
        "--default cannot be specified",
    );
    fails_with(
        &home,
        &["retention", "clear", "--default", "--versions", "dead/b"],
        "--default cannot be specified",
    );
    fails_with(
        &home,
        &["retention", "info", "-vid", "v1", "-r", "dead/b/o"],
        "--version-id",
    );
    fails_with(
        &home,
        &["retention", "info", "--rewind", "yesterday", "dead/b/o"],
        "invalid time",
    );
    fails_with(
        &home,
        &["retention", "info", "local-dir/file"],
        "No valid configuration found for 'local-dir' host alias.",
    );
    // `retention clear` has no --bypass (clear always bypasses governance, like mc).
    mx(&home)
        .args(["retention", "clear", "--bypass", "dead/b/o"])
        .assert()
        .failure();
}

#[test]
fn legalhold_rejects_bad_arguments() {
    let home = home();
    fails_with(
        &home,
        &[
            "legalhold",
            "set",
            "--version-id",
            "v1",
            "--versions",
            "dead/b/o",
        ],
        "You cannot pass --version-id",
    );
    fails_with(
        &home,
        &[
            "legalhold",
            "info",
            "--vid",
            "v1",
            "--rewind",
            "1d",
            "dead/b/o",
        ],
        "You cannot pass --version-id",
    );
    fails_with(
        &home,
        &["legalhold", "clear", "local-dir/file"],
        "No valid configuration found for 'local-dir' host alias.",
    );
}

#[test]
fn event_rejects_bad_arguments() {
    let home = home();
    let arn = "arn:minio:sqs::MXTEST:webhook";
    fails_with(
        &home,
        &["event", "add", "dead/b", arn, "--event", "put,copy"],
        "invalid event `copy`",
    );
    fails_with(
        &home,
        &["event", "add", "dead/b", "arn:minio:s3::x:y"],
        "unsupported service",
    );
    fails_with(
        &home,
        &["event", "add", "dead/b", "not-an-arn"],
        "invalid ARN",
    );
    fails_with(
        &home,
        &["event", "add", "dead/b/object", arn],
        "must be a bucket",
    );
    fails_with(
        &home,
        &["event", "rm", "dead/b"],
        "--force flag needs to be passed",
    );
    fails_with(
        &home,
        &["event", "remove", "dead/b", "--force", "--prefix", "x/"],
        "require an ARN",
    );
    mx(&home)
        .args(["event", "list", "--help"])
        .assert()
        .success();
}

#[test]
fn undo_rejects_bad_arguments() {
    let home = home();
    fails_with(
        &home,
        &["undo", "-r", "dead/b/prefix/"],
        "provide --force flag",
    );
    fails_with(
        &home,
        &["undo", "--last", "0", "dead/b/o"],
        "positive integer",
    );
    fails_with(
        &home,
        &["undo", "--action", "copy", "dead/b/o"],
        "unsupported action",
    );
    fails_with(
        &home,
        &["undo", "--action", "PUT", "--last", "2", "dead/b/o"],
        "--last=1",
    );
}

#[test]
fn ilm_restore_rejects_bad_arguments() {
    let home = home();
    fails_with(
        &home,
        &["ilm", "restore", "--days", "0", "dead/b/o"],
        "--days should be equal or greater than 1",
    );
    fails_with(
        &home,
        &["ilm", "restore", "-r", "--vid", "v1", "dead/b/"],
        "You cannot combine --version-id",
    );
    fails_with(
        &home,
        &["ilm", "restore", "--versions", "dead/b/o"],
        "--versions requires --recursive",
    );
}

#[test]
fn od_rejects_bad_operands() {
    let home = home();
    mx(&home).arg("od").assert().failure();
    fails_with(&home, &["od", "if=a"], "both if= and of=");
    fails_with(
        &home,
        &["od", "if=a", "of=b", "bs=1"],
        "unknown operand `bs`",
    );
    fails_with(&home, &["od", "if=a", "of=b", "size=10XB"], "invalid size");
    fails_with(
        &home,
        &["od", "if=a", "of=b", "parts=x"],
        "invalid value for `parts`",
    );
    fails_with(
        &home,
        &["od", "if=dead/b/o", "of=out.bin", "size=1MiB"],
        "size cannot be specified getting from server",
    );
    let dir = home.path().to_str().unwrap().to_string();
    fails_with(
        &home,
        &["od", &format!("if={dir}"), "of=dead/b/o"],
        "source cannot be a directory",
    );
}

#[test]
fn od_copies_local_files_in_parts() {
    let home = home();
    let source = home.path().join("source.bin");
    let data: Vec<u8> = (0..3 * 1024 * 1024 + 10).map(|i| (i % 251) as u8).collect();
    std::fs::write(&source, &data).unwrap();

    let target = home.path().join("full.bin");
    mx(&home)
        .args([
            "od",
            &format!("if={}", source.display()),
            &format!("of={}", target.display()),
        ])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "Transferred: 3.0 MiB, Parts: 1, Time: ",
        ))
        .stdout(predicate::str::contains("Speed: "));
    assert_eq!(std::fs::read(&target).unwrap(), data);

    let target = home.path().join("parts.bin");
    let output = mx(&home)
        .args([
            "--json",
            "od",
            &format!("if={}", source.display()),
            &format!("of={}", target.display()),
            "size=1MiB",
            "parts=2",
            "skip=1",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["status"], "success");
    assert_eq!(json["type"], "FStoFS");
    assert_eq!(json["partSize"], 1024 * 1024);
    assert_eq!(json["totalSize"], 2 * 1024 * 1024);
    assert_eq!(json["parts"], 2);
    assert_eq!(json["skip"], 1);
    assert_eq!(
        std::fs::read(&target).unwrap(),
        &data[1024 * 1024..3 * 1024 * 1024]
    );
}
