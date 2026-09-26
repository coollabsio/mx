//! Offline CLI tests for rm/ls/stat/du/tree/rb flag handling (area C).

use assert_cmd::Command;
use predicates::prelude::*;

fn mx(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("mx").expect("binary");
    cmd.env("HOME", home).env("NO_COLOR", "1");
    cmd
}

/// Runs `mx ARGS` with an alias `local` pointing to an unreachable server and expects a
/// validation failure containing `message` (no network access happens before validation).
fn fails_with(args: &[&str], message: &str) {
    let home = tempfile::tempdir().expect("tempdir");
    mx(home.path())
        .args([
            "alias",
            "set",
            "--api",
            "S3v4",
            "local",
            "http://127.0.0.1:1",
            "access",
            "secret12",
        ])
        .assert()
        .success();
    mx(home.path())
        .args(args)
        .assert()
        .failure()
        .stderr(predicate::str::contains(message));
}

#[test]
fn rm_enforces_mc_flag_rules() {
    fails_with(
        &["rm", "--vid", "v1", "--versions", "--force", "local/b/k"],
        "You cannot specify --version-id with any of --versions, --rewind and --recursive flags.",
    );
    fails_with(
        &["rm", "-vid=v1", "-r", "--force", "local/b/k"],
        "--version-id with any of",
    );
    fails_with(
        &["rm", "--non-current", "--versions", "--force", "local/b/k"],
        "You cannot specify --non-current without --versions --recursive",
    );
    fails_with(
        &["rm", "--purge", "local/b/k"],
        "You cannot specify --purge without --force.",
    );
    fails_with(
        &["rm", "--purge", "--force", "-r", "local/b/k"],
        "You cannot specify --purge with --recursive.",
    );
    fails_with(
        &["rm", "--purge", "--force", "--versions", "local/b/k"],
        "You cannot specify --purge flag with any flag(s) other than --force.",
    );
    fails_with(
        &["rm", "-r", "local/b/p/"],
        "Removal requires --force flag.",
    );
    fails_with(
        &["rm", "--versions", "local/b/k"],
        "Removal requires --force flag.",
    );
    fails_with(&["rm", "--stdin"], "Removal requires --force flag.");
    fails_with(
        &["rm", "-r", "--force", "local"],
        "retry this command with ‘--dangerous’ and ‘--force’ flags.",
    );
    fails_with(
        &["rm", "--force", "local/b"],
        "Removal requires --recursive flag.",
    );
    fails_with(
        &["rm", "--rewind", "1d", "local/b/k"],
        "You cannot specify --rewind without --recursive or --versions.",
    );
    fails_with(
        &["rm", "-I", "--versions", "--force", "local/b/k"],
        "You cannot specify --incomplete",
    );
    fails_with(
        &["rm", "-r", "--force", "--older-than", "soon", "local/b/"],
        "invalid duration",
    );
    fails_with(
        &["rm", "-r", "--force", "--rewind", "yesterday", "local/b/"],
        "invalid time",
    );
}

#[test]
fn rm_requires_a_target_unless_stdin() {
    let home = tempfile::tempdir().expect("tempdir");
    mx(home.path())
        .args(["rm"])
        .assert()
        .code(1)
        // Like mc: the command help on stdout.
        .stdout(predicate::str::contains("TARGET"));
}

#[test]
fn rm_help_lists_mc_flags() {
    let home = tempfile::tempdir().expect("tempdir");
    let assert = mx(home.path()).args(["rm", "--help"]).assert().success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for flag in [
        "--versions",
        "--recursive",
        "--force",
        "--dangerous",
        "--rewind",
        "--version-id",
        "--incomplete",
        "--dry-run",
        "--stdin",
        "--older-than",
        "--newer-than",
        "--bypass",
        "--non-current",
    ] {
        assert!(out.contains(flag), "rm --help is missing {flag}");
    }
    assert!(!out.contains("--purge"), "--purge is hidden like in mc");
}

#[test]
fn ls_rejects_invalid_combinations() {
    fails_with(
        &["ls", "--zip", "--versions", "local/b/archive.zip"],
        "Zip file listing can only be performed on the latest version",
    );
    fails_with(
        &["ls", "--zip", "--rewind", "1d", "local/b/archive.zip"],
        "Zip file listing can only be performed on the latest version",
    );
    fails_with(
        &["ls", "-I", "--versions", "local/b/"],
        "You cannot specify --incomplete",
    );
    fails_with(&["ls", "--versions", "local"], "require a bucket target");
    fails_with(&["ls", "--rewind", "nope", "local/b/"], "invalid time");
}

#[test]
fn ls_help_lists_mc_flags() {
    let home = tempfile::tempdir().expect("tempdir");
    let assert = mx(home.path()).args(["ls", "--help"]).assert().success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for flag in [
        "--rewind",
        "--versions",
        "--recursive",
        "--incomplete",
        "--summarize",
        "--storage-class",
        "--zip",
    ] {
        assert!(out.contains(flag), "ls --help is missing {flag}");
    }
}

#[test]
fn stat_rejects_invalid_combinations() {
    fails_with(
        &["stat", "--vid", "v1", "local/b/k", "local/b/j"],
        "You cannot specify --version-id with multiple arguments.",
    );
    fails_with(
        &["stat", "-vid", "v1", "--versions", "local/b/k"],
        "You cannot specify --version-id with either --rewind, --versions or --recursive.",
    );
    fails_with(
        &["stat", "--no-list", "-r", "local/b/"],
        "You cannot specify --no-list with either --versions or --recursive.",
    );
}

#[test]
fn stat_help_lists_mc_flags() {
    let home = tempfile::tempdir().expect("tempdir");
    let assert = mx(home.path()).args(["stat", "--help"]).assert().success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for flag in [
        "--rewind",
        "--versions",
        "--version-id",
        "--recursive",
        "--verbose",
        "--no-list",
    ] {
        assert!(out.contains(flag), "stat --help is missing {flag}");
    }
}

#[test]
fn du_and_tree_ignore_versions_on_local_paths() {
    // Like mc, local listings ignore --versions / --rewind.
    let home = tempfile::tempdir().expect("tempdir");
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/a.txt"), "abc").unwrap();
    let path = dir.path().to_str().unwrap();
    let label = path.trim_matches('/');
    mx(home.path())
        .args(["du", "--versions", path])
        .assert()
        .success()
        .stdout(format!("3B\t1 version\t{label}\n"));
    mx(home.path())
        .args(["du", "--rewind", "1d", "-r", path])
        .assert()
        .success()
        .stdout(format!(
            "3B\t1 object\t{label}/sub\n3B\t1 object\t{label}\n"
        ));
    mx(home.path())
        .args(["tree", "--rewind", "1d", "-f", path])
        .assert()
        .success()
        .stdout(format!("{path}\n└─ sub\n   └─ a.txt\n"));
    fails_with(&["du", "--rewind", "later", "local/b/"], "invalid time");
}

#[test]
fn ls_lists_local_folders_like_mc() {
    let home = tempfile::tempdir().expect("tempdir");
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("a.txt"), "abc").unwrap();
    std::fs::write(dir.path().join("sub/b.txt"), "b").unwrap();
    let path = dir.path().to_str().unwrap();
    let out = mx(home.path())
        .args(["ls", "-r", "--summarize", path])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 5, "{text}");
    assert!(
        lines[0].starts_with('[') && lines[0].ends_with("]     3B a.txt"),
        "{text}"
    );
    assert!(lines[1].ends_with("]     1B sub/b.txt"), "{text}");
    assert_eq!(lines[2..], ["", "Total Size: 4 B", "Total Objects: 2"]);
    let out = mx(home.path())
        .args(["--json", "ls", path])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let docs: Vec<serde_json::Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0]["key"], "a.txt");
    assert_eq!(docs[0]["type"], "file");
    assert_eq!(docs[1]["key"], "sub/");
    assert_eq!(docs[1]["type"], "folder");
    assert_eq!(docs[1]["url"], format!("{path}/"));
}

#[test]
fn rb_requires_force_and_dangerous_for_alias_root() {
    fails_with(
        &["rb", "local"],
        "This operation results in **site-wide** removal of buckets.",
    );
    fails_with(&["rb", "--force", "local"], "‘--force’ and ‘--dangerous’");
    fails_with(&["rb", "local/b/key"], "`rb` requires a bucket target");
}
