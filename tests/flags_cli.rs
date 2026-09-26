//! Offline checks for foundation wiring: stub commands, argv rewriting, new mb flags.

use assert_cmd::Command;
use predicates::prelude::*;

fn mx() -> Command {
    Command::cargo_bin("mx").expect("binary")
}

#[test]
fn stub_commands_are_registered_and_fail_cleanly() {
    let home = tempfile::tempdir().unwrap();
    let cases: &[&[&str]] = &[
        &["retention", "info", "local/b/o"],
        &["retention", "set", "GOVERNANCE", "1d", "local/b/o"],
        &["retention", "clear", "local/b/o"],
        &["legalhold", "set", "local/b/o"],
        &["legalhold", "info", "local/b/o"],
        &["event", "add", "local/b", "arn:minio:sqs::1:webhook"],
        &["event", "remove", "local/b", "--force"],
        &["event", "list", "local/b"],
        &["undo", "local/b/o"],
        &["od", "if=/dev/null", "of=local/b/o"],
        &["replicate", "ls", "local/b"],
        &["replicate", "backlog", "local/b"],
        &["quota", "info", "local/b"],
        &["ilm", "tier", "ls", "local"],
        &["ilm", "restore", "local/b/o"],
    ];
    for args in cases {
        mx().env("HOME", home.path())
            .args(*args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("not implemented yet"));
    }
}

#[test]
fn help_lists_new_commands() {
    let assert = mx().arg("--help").assert().success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for name in [
        "retention",
        "legalhold",
        "event",
        "undo",
        "od",
        "replicate",
        "quota",
    ] {
        assert!(out.contains(name), "missing {name} in help");
    }
    mx().args(["ilm", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tier"))
        .stdout(predicate::str::contains("restore"));
}

#[test]
fn multichar_short_flags_are_rewritten() {
    let home = tempfile::tempdir().unwrap();
    mx().env("HOME", home.path())
        .args(["ilm", "restore", "-vid", "v1", "local/b/o"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not implemented yet"));
    mx().env("HOME", home.path())
        .args(["legalhold", "info", "--vid", "v1", "local/b/o"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not implemented yet"));
    // A command without the flag reports the rewritten long form.
    mx().env("HOME", home.path())
        .args(["cat", "-vid", "v1", "local/b/o"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--version-id"));
    mx().env("HOME", home.path())
        .args(["cat", "-sc", "STANDARD", "local/b/o"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--storage-class"));
}

#[test]
fn mb_accepts_lock_and_versioning_flags() {
    mx().args(["mb", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--with-lock"))
        .stdout(predicate::str::contains("--with-versioning"))
        .stdout(predicate::str::contains("--region"))
        .stdout(predicate::str::contains("--ignore-existing"));
}
