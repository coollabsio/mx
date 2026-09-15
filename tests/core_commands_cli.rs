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

#[test]
fn accepts_json_after_stat_subcommand() {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("mx").unwrap();
    cmd.env("HOME", home.path())
        .args(["stat", "--json", "missing/example/file.txt"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument").not());
}

#[test]
fn accepts_resolve_after_commands_that_coolify_uses() {
    for args in [
        vec![
            "alias",
            "set",
            "--resolve",
            "s3.internal:9000=127.0.0.1",
            "demo",
            "http://s3.internal:9000",
            "key",
            "secret",
        ],
        vec![
            "stat",
            "--resolve",
            "s3.internal:9000=127.0.0.1",
            "missing/example/file.txt",
        ],
    ] {
        let home = tempfile::tempdir().unwrap();
        let mut cmd = Command::cargo_bin("mx").unwrap();
        cmd.env("HOME", home.path()).args(args);
        cmd.assert()
            .stderr(predicate::str::contains("unexpected argument").not());
    }
}

#[test]
fn pipe_command_accepts_coolify_options() {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("mx").unwrap();
    cmd.env("HOME", home.path()).args([
        "pipe",
        "--quiet",
        "--resolve",
        "s3.internal:9000=127.0.0.1",
        "missing/example/file.txt",
    ]);
    cmd.write_stdin("archive");
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand").not())
        .stderr(predicate::str::contains("unexpected argument").not());
}

#[test]
fn mb_accepts_ignore_existing() {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("mx").unwrap();
    cmd.env("HOME", home.path())
        .args(["mb", "--ignore-existing", "missing/example"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument").not());
}

#[test]
fn rb_rejects_ignore_existing() {
    let mut cmd = Command::cargo_bin("mx").unwrap();
    cmd.args(["rb", "--ignore-existing", "missing/example"]);
    cmd.assert().failure().stderr(predicate::str::contains(
        "unexpected argument '--ignore-existing'",
    ));
}

#[test]
fn resolve_option_is_repeatable() {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("mx").unwrap();
    cmd.env("HOME", home.path()).args([
        "stat",
        "--resolve",
        "first.internal:9000=127.0.0.1",
        "--resolve",
        "second.internal:9000=127.0.0.2",
        "missing/example/file.txt",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument").not());
}
