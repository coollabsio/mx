use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn local_inventory_has_normalized_relative_file_names() {
    let source = tempfile::tempdir().expect("source");
    std::fs::create_dir(source.path().join("nested")).expect("nested dir");
    std::fs::write(source.path().join("root.txt"), b"root").expect("root file");
    std::fs::write(source.path().join("nested/item.txt"), b"item").expect("nested file");

    let entries = mx::transfer::local_inventory(source.path()).expect("inventory");
    let names: Vec<_> = entries
        .iter()
        .map(|entry| entry.relative.as_str())
        .collect();
    assert_eq!(names, ["nested/item.txt", "root.txt"]);
    assert_eq!(entries[0].size, 4);
}

#[test]
fn joins_s3_prefix_without_duplicate_slashes() {
    assert_eq!(
        mx::transfer::join_key(Some("backup/"), "a/b.txt"),
        "backup/a/b.txt"
    );
    assert_eq!(mx::transfer::join_key(None, "a.txt"), "a.txt");
}

#[test]
fn cp_requires_recursive_for_a_local_directory() {
    let home = tempfile::tempdir().expect("home");
    let source = tempfile::tempdir().expect("source");

    Command::new(assert_cmd::cargo::cargo_bin!("mx"))
        .env("HOME", home.path())
        .args([
            "cp",
            source.path().to_str().expect("utf8 path"),
            "play/test/",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--recursive"));
}

#[test]
fn recursive_flag_is_accepted_by_cp_mv_and_rm() {
    for command in ["cp", "mv"] {
        Command::new(assert_cmd::cargo::cargo_bin!("mx"))
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--recursive"));
    }

    Command::new(assert_cmd::cargo::cargo_bin!("mx"))
        .args(["rm", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--recursive"));
}
