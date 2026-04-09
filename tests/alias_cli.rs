use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

fn temp_home() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn mx() -> Command {
    Command::cargo_bin("mx").expect("binary")
}

#[test]
fn list_creates_default_mx_config() {
    let home = temp_home();
    let mut cmd = mx();
    cmd.env("HOME", home.path()).args(["alias", "list"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("play"))
        .stdout(predicate::str::contains("Alias"));

    let config_path = home.path().join(".mx/config.json");
    assert!(
        config_path.exists(),
        "expected {} to exist",
        config_path.display()
    );
}

#[test]
fn set_writes_alias_to_mx_config() {
    let home = temp_home();
    let mut cmd = mx();
    cmd.env("HOME", home.path()).args([
        "alias",
        "set",
        "demo",
        "http://localhost:9000",
        "minio",
        "minio123",
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Added `demo` successfully."));

    let raw = fs::read_to_string(home.path().join(".mx/config.json")).unwrap();
    let json: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(json["aliases"]["demo"]["url"], "http://localhost:9000");
    assert_eq!(json["aliases"]["demo"]["accessKey"], "minio");
    assert_eq!(json["aliases"]["demo"]["secretKey"], "minio123");
    assert_eq!(json["aliases"]["demo"]["api"], "S3v4");
    assert_eq!(json["aliases"]["demo"]["path"], "auto");
}

#[test]
fn list_reads_mc_config_when_mx_missing() {
    let home = temp_home();
    let mc_dir = home.path().join(".mc");
    fs::create_dir_all(&mc_dir).unwrap();
    fs::write(
        mc_dir.join("config.json"),
        r#"{
  "version": "10",
  "aliases": {
    "legacy": {
      "url": "https://s3.example.com",
      "accessKey": "oldkey",
      "secretKey": "oldsecret",
      "api": "S3v4",
      "path": "auto"
    }
  }
}
"#,
    )
    .unwrap();

    let mut cmd = mx();
    cmd.env("HOME", home.path())
        .args(["alias", "list", "legacy"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("legacy"))
        .stdout(predicate::str::contains(".mc/config.json"));

    assert!(!home.path().join(".mx/config.json").exists());
}

#[test]
fn remove_updates_mc_config_when_selected() {
    let home = temp_home();
    let mc_dir = home.path().join(".mc");
    fs::create_dir_all(&mc_dir).unwrap();
    fs::write(
        mc_dir.join("config.json"),
        r#"{
  "version": "10",
  "aliases": {
    "legacy": {
      "url": "https://s3.example.com",
      "accessKey": "oldkey",
      "secretKey": "oldsecret",
      "api": "S3v4",
      "path": "auto"
    }
  }
}
"#,
    )
    .unwrap();

    let mut cmd = mx();
    cmd.env("HOME", home.path())
        .args(["alias", "remove", "legacy"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Removed `legacy` successfully."));

    let raw = fs::read_to_string(mc_dir.join("config.json")).unwrap();
    let json: Value = serde_json::from_str(&raw).unwrap();
    assert!(json["aliases"].get("legacy").is_none());
}

#[test]
fn mx_config_takes_precedence_over_mc_config() {
    let home = temp_home();
    let mc_dir = home.path().join(".mc");
    let mx_dir = home.path().join(".mx");
    fs::create_dir_all(&mc_dir).unwrap();
    fs::create_dir_all(&mx_dir).unwrap();
    fs::write(
        mc_dir.join("config.json"),
        r#"{"version":"10","aliases":{"frommc":{"url":"https://mc.example.com","accessKey":"a","secretKey":"b","api":"S3v4","path":"auto"}}}"#,
    )
    .unwrap();
    fs::write(
        mx_dir.join("config.json"),
        r#"{"version":"10","aliases":{"frommx":{"url":"https://mx.example.com","accessKey":"x","secretKey":"y","api":"S3v4","path":"auto"}}}"#,
    )
    .unwrap();

    let mut cmd = mx();
    cmd.env("HOME", home.path()).args(["alias", "list"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("frommx"))
        .stdout(predicate::str::contains("https://mx.example.com"))
        .stdout(predicate::str::contains("frommc").not());
}

#[test]
fn set_supports_json_output() {
    let home = temp_home();
    let mut cmd = mx();
    cmd.env("HOME", home.path()).args([
        "--json",
        "alias",
        "set",
        "demo",
        "http://localhost:9000",
        "minio",
        "minio123",
    ]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"success\""))
        .stdout(predicate::str::contains("\"alias\": \"demo\""))
        .stdout(predicate::str::contains(
            "\"URL\": \"http://localhost:9000\"",
        ))
        .stdout(predicate::str::contains("\"accessKey\": \"minio\""))
        .stdout(predicate::str::contains("\"secretKey\": \"minio123\""));
}

#[test]
fn list_supports_json_output() {
    let home = temp_home();
    let mut cmd = mx();
    cmd.env("HOME", home.path()).args([
        "alias",
        "set",
        "demo",
        "http://localhost:9000",
        "minio",
        "minio123",
    ]);
    cmd.assert().success();

    let mut cmd = mx();
    cmd.env("HOME", home.path())
        .args(["--json", "alias", "list", "demo"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"success\""))
        .stdout(predicate::str::contains("\"alias\": \"demo\""))
        .stdout(predicate::str::contains("\"src\": "))
        .stdout(predicate::str::contains("\"path\": \"auto\""));
}

#[test]
fn remove_supports_json_output() {
    let home = temp_home();
    let mut cmd = mx();
    cmd.env("HOME", home.path()).args([
        "alias",
        "set",
        "demo",
        "http://localhost:9000",
        "minio",
        "minio123",
    ]);
    cmd.assert().success();

    let mut cmd = mx();
    cmd.env("HOME", home.path())
        .args(["--json", "alias", "remove", "demo"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"success\""))
        .stdout(predicate::str::contains("\"alias\": \"demo\""))
        .stdout(predicate::str::contains("\"URL\"").not());
}
