use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn mirror_is_listed_and_accepts_remove() {
    Command::new(assert_cmd::cargo::cargo_bin!("mx"))
        .args(["mirror", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--remove"));
}

#[test]
fn mirror_rejects_local_to_local() {
    let home = tempfile::tempdir().expect("home");
    let source = tempfile::tempdir().expect("source");
    let target = tempfile::tempdir().expect("target");
    Command::new(assert_cmd::cargo::cargo_bin!("mx"))
        .env("HOME", home.path())
        .args([
            "mirror",
            source.path().to_str().expect("source path"),
            target.path().to_str().expect("target path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Local-to-local mirror is not supported",
        ));
}
