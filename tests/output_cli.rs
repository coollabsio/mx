//! Shared output layer: JSON lines on a non-TTY, mc error format (text + JSON), exit codes,
//! `MC_*` environment flags, `MC_HOST_<alias>` aliases, and `-v/--version`.

use assert_cmd::Command;
use predicates::prelude::*;

fn mx(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("mx").unwrap();
    cmd.env("HOME", home).current_dir(home);
    for (key, _) in std::env::vars() {
        if key.starts_with("MC_") {
            cmd.env_remove(key);
        }
    }
    cmd
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn json_is_one_compact_document_per_line_on_non_tty() {
    let home = tempfile::tempdir().unwrap();
    let output = mx(home.path())
        .args(["--json", "alias", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    // Default config: local, s3, gcs, play.
    assert_eq!(lines.len(), 4, "{text}");
    for line in lines {
        let doc: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(doc["status"], "success");
        assert!(!line.contains(": "), "not compact: {line}");
    }
}

#[test]
fn text_errors_use_mc_prefix_and_exit_one() {
    let home = tempfile::tempdir().unwrap();
    // Like mc, an unknown alias is a local path.
    let path = home
        .path()
        .canonicalize()
        .unwrap()
        .join("missing/bucket/key");
    mx(home.path())
        .args(["cat", "missing/bucket/key"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!(
            "mx: <ERROR> Unable to read from `missing/bucket/key`. Requested path `{}` not found.\n",
            path.display()
        ));
}

#[test]
fn json_errors_are_mc_error_documents_on_stdout() {
    let home = tempfile::tempdir().unwrap();
    let output = mx(home.path())
        .args(["--json", "rb", "missing/bucket"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    // Compact on a non-TTY, like mc.
    assert_eq!(
        stdout(&output),
        "{\"status\":\"error\",\"error\":{\"message\":\"No valid configuration found for 'missing' host alias.\",\"cause\":{\"message\":\"\",\"error\":{}},\"type\":\"fatal\"}}\n"
    );
}

#[test]
fn usage_errors_exit_one_with_mc_prefix() {
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args(["ls", "--no-such-flag"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Invalid command usage, flag provided but not defined: -no-such-flag\n\nSUPPORTED FLAGS:\n",
        ));
    // No command: app help, status 1 (mc `showAppHelpAndExit`).
    mx(home.path())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("USAGE:"));
}

#[test]
fn version_flags_print_mc_shaped_version() {
    let home = tempfile::tempdir().unwrap();
    for flag in ["-v", "--version", "-V"] {
        mx(home.path()).arg(flag).assert().success().stdout(
            predicate::str::is_match(
                r"^mx version RELEASE\.\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}Z \(commit-id=[0-9a-z]+\)\nRuntime: rustc\S* \S+/\S+\nCopyright \(c\) \d{4} .+\nLicense .+\n$",
            )
            .unwrap(),
        );
    }
    // `stat -v` stays the verbose flag.
    mx(home.path())
        .args(["stat", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--verbose, -v"));
}

#[test]
fn mc_env_flags_are_read() {
    let home = tempfile::tempdir().unwrap();
    // MC_JSON=1 behaves like --json (error document on stdout).
    mx(home.path())
        .env("MC_JSON", "1")
        .args(["stat", "missing/bucket"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"type\":\"fatal\""));
    // MC_JSON=false keeps text output.
    mx(home.path())
        .env("MC_JSON", "false")
        .args(["stat", "missing/bucket"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::contains("<ERROR>"));
    // Invalid boolean values are usage errors, like urfave/cli.
    mx(home.path())
        .env("MC_QUIET", "maybe")
        .args(["alias", "list"])
        .assert()
        .code(1);
    // MC_CONFIG_DIR is the config folder.
    let dir = home.path().join("cfg");
    mx(home.path())
        .env("MC_CONFIG_DIR", &dir)
        .args(["alias", "list", "local"])
        .assert()
        .success();
    assert!(dir.join("config.json").exists());
    // MC_RESOLVE is comma separated; MC_LIMIT_UPLOAD takes a size.
    mx(home.path())
        .env(
            "MC_RESOLVE",
            "a.example:9000=127.0.0.1,b.example:9000=127.0.0.2",
        )
        .env("MC_LIMIT_UPLOAD", "1MiB")
        .args(["alias", "list", "local"])
        .assert()
        .success();
}

#[test]
fn mc_host_env_aliases_are_listed_and_not_saved() {
    let home = tempfile::tempdir().unwrap();
    let output = mx(home.path())
        .env("MC_HOST_envx", "https://AKIA:se/cr+et:tok@env.example:9000")
        .args(["--json", "alias", "list", "envx"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let doc: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(doc["URL"], "https://env.example:9000");
    assert_eq!(doc["accessKey"], "AKIA");
    assert_eq!(doc["secretKey"], "se/cr+et");
    assert_eq!(doc["api"], "S3v4");
    assert_eq!(doc["src"], "env");

    // Also listed with all aliases, and never written to the config file.
    mx(home.path())
        .env("MC_HOST_envx", "https://AKIA:secret@env.example")
        .args(["alias", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("envx"));
    let config = std::fs::read_to_string(home.path().join(".mx/config.json")).unwrap();
    assert!(!config.contains("envx"));
}

#[test]
fn mc_host_env_alias_overrides_config_and_rejects_invalid_values() {
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .env("MC_HOST_local", "http://a:b@override.example:1234")
        .args(["alias", "list", "local"])
        .assert()
        .success()
        .stdout(predicate::str::contains("http://override.example:1234"));
    mx(home.path())
        .env("MC_HOST_bad", "https://a:b@host.example/bucket")
        .args(["stat", "bad/bucket"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Invalid arguments provided"));
}

#[test]
fn mc_config_env_file_aliases() {
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("aliases.env");
    std::fs::write(&file, "MC_HOST_fromfile=https://a:b@file.example\n").unwrap();
    mx(home.path())
        .env("MC_CONFIG_ENV_FILE", &file)
        .args(["alias", "list", "fromfile"])
        .assert()
        .success()
        .stdout(predicate::str::contains("https://file.example"));
    std::fs::write(&file, "garbage\n").unwrap();
    mx(home.path())
        .env("MC_CONFIG_ENV_FILE", &file)
        .args(["alias", "list"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Unable to parse"));
}

#[test]
fn alias_import_export_short_names() {
    let home = tempfile::tempdir().unwrap();
    let exported = mx(home.path())
        .args(["alias", "e", "play"])
        .output()
        .unwrap();
    assert!(exported.status.success());
    mx(home.path())
        .args(["alias", "i", "copy"])
        .write_stdin(exported.stdout)
        .assert()
        .success();
    mx(home.path())
        .args(["alias", "ls", "copy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("https://play.min.io"));
}
