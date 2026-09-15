use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;

fn mx() -> Command {
    Command::cargo_bin("mx").expect("binary")
}

fn temp_home() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn top_level_help_lists_parity_commands() {
    mx().arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("head"))
        .stdout(predicate::str::contains("du"))
        .stdout(predicate::str::contains("find"))
        .stdout(predicate::str::contains("tree"))
        .stdout(predicate::str::contains("diff"))
        .stdout(predicate::str::contains("share"))
        .stdout(predicate::str::contains("ready"))
        .stdout(predicate::str::contains("ping"))
        .stdout(predicate::str::contains("tag"))
        .stdout(predicate::str::contains("version"))
        .stdout(predicate::str::contains("cors"))
        .stdout(predicate::str::contains("encrypt"))
        .stdout(predicate::str::contains("anonymous"))
        .stdout(predicate::str::contains("ilm"));
}

#[test]
fn global_help_lists_config_quiet_insecure_and_version() {
    mx().arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--config-dir"))
        .stdout(predicate::str::contains("--quiet"))
        .stdout(predicate::str::contains("--insecure"))
        .stdout(predicate::str::contains("--version"));
}

#[test]
fn version_flag_prints_package_version() {
    mx().arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn command_help_is_available_for_parity_commands() {
    for command in [
        "get",
        "head",
        "du",
        "find",
        "tree",
        "diff",
        "share",
        "ready",
        "ping",
        "tag",
        "version",
        "cors",
        "encrypt",
        "anonymous",
        "ilm",
    ] {
        mx().env("NO_COLOR", "1")
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains(command));
    }
}

#[test]
fn alias_help_lists_import_and_export() {
    mx().args(["alias", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("import"))
        .stdout(predicate::str::contains("export"));
}

#[test]
fn alias_export_and_import_round_trip() {
    let home = temp_home();
    mx().env("HOME", home.path())
        .args([
            "alias",
            "set",
            "demo",
            "http://localhost:9000",
            "minio",
            "minio123",
        ])
        .assert()
        .success();

    let exported = mx()
        .env("HOME", home.path())
        .args(["alias", "export", "demo"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&exported).expect("export json");
    assert_eq!(json["url"], "http://localhost:9000");
    assert_eq!(json["accessKey"], "minio");
    assert_eq!(json["secretKey"], "minio123");

    let other = temp_home();
    mx().env("HOME", other.path())
        .args(["alias", "import", "copied"])
        .write_stdin(exported)
        .assert()
        .success();

    mx().env("HOME", other.path())
        .args(["--json", "alias", "list", "copied"])
        .assert()
        .success()
        .stdout(predicate::str::contains("http://localhost:9000"));
}

#[test]
fn config_dir_flag_writes_alias_outside_home() {
    let home = temp_home();
    let config_dir = temp_home();
    mx().env("HOME", home.path())
        .args([
            "--config-dir",
            config_dir.path().to_str().expect("utf8"),
            "alias",
            "set",
            "demo",
            "http://localhost:9000",
            "minio",
            "minio123",
        ])
        .assert()
        .success();

    assert!(config_dir.path().join("config.json").exists());
    assert!(!home.path().join(".mx/config.json").exists());
}

#[test]
fn ls_and_rm_help_include_recursive_and_force() {
    mx().args(["ls", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--recursive"));
    mx().args(["rm", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--force"));
    mx().args(["rb", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--force"));
}

#[test]
fn share_download_help_includes_expire() {
    mx().args(["share", "download", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--expire"));
}

#[test]
fn head_help_includes_lines() {
    mx().args(["head", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--lines"));
}

#[test]
fn ilm_rule_add_help_includes_expire_days() {
    mx().args(["ilm", "rule", "add", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--expire-days"));
}

#[test]
fn anonymous_set_help_describes_policy() {
    mx().args(["anonymous", "set", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("download"));
}

#[test]
fn local_head_prints_first_lines() {
    let dir = temp_home();
    let path = dir.path().join("sample.txt");
    fs::write(&path, "one\ntwo\nthree\nfour\n").expect("write");
    mx().args(["head", "--lines", "2", path.to_str().expect("utf8")])
        .assert()
        .success()
        .stdout("one\ntwo\n");
}

#[test]
fn local_du_summarizes_directory() {
    let dir = temp_home();
    fs::write(dir.path().join("a.txt"), "abcd").expect("write");
    mx().args(["du", dir.path().to_str().expect("utf8")])
        .assert()
        .success()
        .stdout(predicate::str::contains("4"));
}
