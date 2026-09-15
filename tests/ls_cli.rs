use assert_cmd::Command;
use predicates::prelude::*;

fn mx() -> Command {
    let mut cmd = Command::cargo_bin("mx").expect("binary");
    cmd.env("NO_COLOR", "1");
    cmd
}

#[test]
fn ls_fails_for_unknown_alias() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut cmd = mx();
    cmd.env("HOME", home.path()).args(["ls", "missing"]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("No such alias `missing` found."));
}

#[test]
fn ls_help_mentions_buckets_and_objects() {
    let mut cmd = mx();
    cmd.args(["ls", "--help"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("list buckets and objects"))
        .stdout(predicate::str::contains("Usage: mx ls"));
}
