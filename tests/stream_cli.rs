//! Offline CLI tests for the streaming commands: `admin trace`, `admin scanner trace`,
//! `admin logs`, `admin heal`, `watch` (argument checks, mc error texts, local `watch`).

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Read;
use std::time::Duration;

/// `mx` with an empty HOME and an alias `dead` pointing at a closed port.
fn mx(home: &tempfile::TempDir) -> Command {
    let mut cmd = Command::cargo_bin("mx").expect("binary");
    cmd.env("HOME", home.path())
        .env("MC_HOST_dead", "http://minio:minio123@127.0.0.1:1")
        .env_remove("MC_JSON");
    cmd
}

fn home() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn missing_targets_print_help_and_exit_1() {
    let h = home();
    for args in [
        &["admin", "trace"][..],
        &["admin", "trace", "dead", "extra"],
        &["admin", "trace", "--filter-request", "dead"],
        &[
            "admin",
            "scanner",
            "trace",
            "dead",
            "--response-duration",
            "5ms",
        ],
        &["admin", "logs"],
        &["admin", "logs", "dead", "a", "b", "c"],
        &["admin", "heal"],
        &["admin", "heal", "--scan", "bogus", "dead"],
        &["watch"],
        &["watch", "a", "b"],
    ] {
        mx(&h)
            .args(args)
            .assert()
            .code(1)
            .stdout(predicate::str::contains("Usage"));
    }
}

#[test]
fn trace_argument_errors_match_mc() {
    let h = home();
    mx(&h)
        .args(["admin", "trace", "--all", "--call", "s3", "dead"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> You cannot specify both --all and --call flags at the same time. \n");
    mx(&h)
        .args(["admin", "trace", "--call", "bogus", "dead"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> Unable to start tracing: unknown call name: `bogus`.\n");
    mx(&h)
        .args(["--json", "admin", "trace", "--call", "s3,bogus", "dead"])
        .assert()
        .code(1)
        .stdout("{\"status\":\"error\",\"error\":{\"message\":\"Unable to start tracing\",\"cause\":{\"message\":\"unknown call name: `bogus`\",\"error\":{}},\"type\":\"fatal\"}}\n");
    mx(&h)
        .args(["admin", "trace", "nosuch"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> Unable to initialize admin client. No valid configuration found for 'nosuch' host alias.\n");
    mx(&h)
        .args([
            "admin",
            "trace",
            "--filter-request",
            "--filter-size",
            "1ZB",
            "dead",
        ])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> Unable to parse input bytes. unhandled size name: zb.\n");
    mx(&h)
        .args(["admin", "trace", "--status-code", "abc", "dead"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Invalid command usage, invalid value \"abc\" for flag -status-code: strconv.Atoi: parsing \"abc\": invalid syntax\n",
        ));
    mx(&h)
        .args(["admin", "trace", "--response-duration", "bogus", "dead"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Invalid command usage, invalid value \"bogus\" for flag -response-duration: parse error\n",
        ));
    mx(&h)
        .args(["admin", "trace", "--in", "/nonexistent-mx-trace"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> Unable to open input: open /nonexistent-mx-trace: no such file or directory.\n");
}

#[test]
fn trace_reports_unreachable_server() {
    let h = home();
    mx(&h)
        .args(["admin", "trace", "dead"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to listen to http trace",
        ));
    mx(&h)
        .args(["admin", "scanner", "trace", "dead"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to listen to http trace",
        ));
}

#[test]
fn logs_argument_errors_match_mc() {
    let h = home();
    mx(&h)
        .args(["admin", "logs", "--last", "0", "dead"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> please set a proper limit, for example: '--last 5' to display last 5 logs, omit this flag to display all available logs. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n");
    mx(&h)
        .args(["admin", "logs", "--type", "bogus", "dead"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> Invalid value for --type flag. Valid options are [minio, application, all]. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n");
    // madmin `GetLogs` ends quietly when the server cannot be reached.
    mx(&h)
        .args(["admin", "logs", "dead"])
        .assert()
        .success()
        .stdout("")
        .stderr("");
}

#[test]
fn heal_argument_errors_match_mc() {
    let h = home();
    mx(&h)
        .args(["admin", "heal", "--pool", "0", "-r", "dead/b"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> --pool takes a non zero positive number. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n");
    mx(&h)
        .args(["admin", "heal", "--set", "0", "-r", "dead/b"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> --set takes a non zero positive number.",
        ));
    mx(&h)
        .args(["admin", "heal", "nosuch"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> Unable to initialize admin client. No valid configuration found for 'nosuch' host alias.\n");
    mx(&h)
        .args(["admin", "heal", "dead"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to get background heal status.",
        ));
    mx(&h)
        .args(["admin", "heal", "-r", "dead/bucket"])
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to start healing.",
        ));
}

#[test]
fn watch_argument_errors_match_mc() {
    let h = home();
    let invalid = "mx: <ERROR> Unable to watch on the specified bucket. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n";
    mx(&h)
        .args(["watch", "--events", "put,bogus", "dead/bucket"])
        .assert()
        .code(1)
        .stderr(invalid);
    mx(&h)
        .args(["watch", "--prefix", "x", "dead/bucket/obj"])
        .assert()
        .code(1)
        .stderr(invalid);
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope/deeper");
    mx(&h)
        .args(["watch", &missing.to_string_lossy()])
        .assert()
        .code(1)
        .stderr(format!(
            "mx: <ERROR> Unable to watch on the specified bucket. lstat {}: no such file or directory.\n",
            dir.path().join("nope").display()
        ));
    // A failed listen request is reported with mc `errorIf`; mc exits 0 afterwards.
    mx(&h)
        .args(["watch", "dead/bucket"])
        .assert()
        .success()
        .stderr(predicate::str::starts_with(
            "mx: <ERROR> Unable to watch for events.",
        ));
}

/// Spawns `mx ARGS`, runs `action` once it is listening, then stops it with SIGTERM.
fn stream_output(args: &[&str], action: impl FnOnce()) -> (String, Option<i32>) {
    let h = home();
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("mx"))
        .args(args)
        .env("HOME", h.path())
        .env("TZ", "UTC")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("spawn mx");
    std::thread::sleep(Duration::from_millis(800));
    action();
    std::thread::sleep(Duration::from_millis(800));
    std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    let status = child.wait().expect("wait");
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    (out, status.code())
}

#[cfg(target_os = "linux")]
#[test]
fn watch_local_directory_prints_mc_events() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().into_owned();
    let (out, code) = stream_output(&["watch", "--events", "put,delete", &root], || {
        std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        // The put event stats the file (like mc); give it a moment before removing it.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::remove_file(dir.path().join("a.txt")).unwrap();
    });
    assert_eq!(code, Some(143), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(
        lines[0].ends_with(&format!("]    5 B s3:ObjectCreated:Put {root}/a.txt")),
        "{out}"
    );
    assert!(
        lines[1].ends_with(&format!("]        s3:ObjectRemoved:Delete {root}/a.txt")),
        "{out}"
    );

    let (out, _) = stream_output(&["--json", "watch", "--recursive", &root], || {
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(dir.path().join("sub/b.txt"), b"hi").unwrap();
    });
    let put = out
        .lines()
        .find(|line| line.contains("ObjectCreated"))
        .unwrap_or_else(|| panic!("{out}"));
    let doc: serde_json::Value = serde_json::from_str(put).unwrap();
    assert_eq!(doc["status"], "success");
    assert_eq!(doc["events"]["size"], 2);
    assert_eq!(doc["events"]["path"], format!("{root}/sub/b.txt"));
    assert_eq!(doc["events"]["type"], "s3:ObjectCreated:Put");
    assert_eq!(doc["source"], serde_json::json!({}));
}
