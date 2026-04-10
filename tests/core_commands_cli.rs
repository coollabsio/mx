use assert_cmd::Command;
use predicates::prelude::*;

fn mx() -> Command {
    Command::cargo_bin("mx").expect("binary")
}

#[test]
fn top_level_help_lists_core_commands() {
    let mut cmd = mx();
    cmd.args(["--help"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("mb"))
        .stdout(predicate::str::contains("rb"))
        .stdout(predicate::str::contains("stat"))
        .stdout(predicate::str::contains("cat"))
        .stdout(predicate::str::contains("rm"))
        .stdout(predicate::str::contains("cp"))
        .stdout(predicate::str::contains("mv"))
        .stdout(predicate::str::contains("put"));
}

#[test]
fn command_help_is_available_for_all_core_commands() {
    for command in ["mb", "rb", "stat", "cat", "rm", "cp", "mv", "put"] {
        let mut cmd = mx();
        cmd.args([command, "--help"]);
        cmd.assert()
            .success()
            .stdout(predicate::str::contains(format!("Usage: mx {command}")));
    }
}

#[test]
fn cat_rejects_json_output() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut cmd = mx();
    cmd.env("HOME", home.path())
        .args(["--json", "cat", "play/example/file.txt"]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("does not support `--json`"));
}
