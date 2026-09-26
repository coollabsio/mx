//! Offline checks for foundation wiring: command registration, argv rewriting, new mb flags.

use assert_cmd::Command;
use predicates::prelude::*;

fn mx() -> Command {
    Command::cargo_bin("mx").expect("binary")
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
        .args(["ilm", "restore", "-vid", "v1", "-r", "local/b/o"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("combine --version-id"));
    mx().env("HOME", home.path())
        .args([
            "legalhold",
            "info",
            "--vid",
            "v1",
            "--versions",
            "local/b/o",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("You cannot pass --version-id"));
    // A command without the flag reports the rewritten long form.
    mx().env("HOME", home.path())
        .args(["ping", "-vid", "v1", "local"])
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
