use assert_cmd::Command;
use predicates::prelude::*;

fn copied_binary(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = assert_cmd::cargo::cargo_bin!("mx");
    let target = dir.path().join(name);
    #[cfg(unix)]
    std::os::unix::fs::symlink(source, &target).expect("link test binary");
    #[cfg(not(unix))]
    std::fs::copy(source, &target).expect("copy test binary");
    (dir, target)
}

#[test]
fn mc_name_runs_the_same_command_set() {
    let (_dir, binary) = copied_binary("mc");

    Command::new(binary)
        .env("NO_COLOR", "1")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("alias"));
}

#[test]
fn mc_name_prefixes_runtime_errors_with_mc() {
    let (_dir, binary) = copied_binary("mc");
    let home = tempfile::tempdir().expect("home");

    Command::new(binary)
        .env("HOME", home.path())
        .args(["ls", "missing"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with("mc:"));
}

#[test]
fn invalid_command_use_returns_status_two() {
    Command::new(assert_cmd::cargo::cargo_bin!("mx"))
        .arg("--not-supported")
        .assert()
        .code(2);
}
