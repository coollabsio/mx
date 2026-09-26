//! Offline tests for cat, head, get, put, pipe and find (local paths and flag validation).

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::{Duration, Instant};

fn mx(home: &Path) -> Command {
    let mut command = Command::cargo_bin("mx").unwrap();
    command.env("HOME", home);
    command
}

fn help(home: &Path, command: &str) -> String {
    let output = mx(home).args([command, "--help"]).output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

// ---------------------------------------------------------------------------
// cat
// ---------------------------------------------------------------------------

#[test]
fn cat_concatenates_local_files_and_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "alpha\n").unwrap();
    fs::write(&b, "beta\n").unwrap();
    mx(dir.path())
        .args(["cat", path(&a), "-", path(&b)])
        .write_stdin("middle\n")
        .assert()
        .success()
        .stdout("alpha\nmiddle\nbeta\n");
    mx(dir.path())
        .arg("cat")
        .write_stdin("only stdin")
        .assert()
        .success()
        .stdout("only stdin");
}

#[test]
fn cat_offset_and_tail_on_local_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("digits.txt");
    fs::write(&file, "0123456789").unwrap();
    mx(dir.path())
        .args(["cat", "--offset", "4", path(&file)])
        .assert()
        .success()
        .stdout("456789");
    mx(dir.path())
        .args(["cat", "--tail", "3", path(&file)])
        .assert()
        .success()
        .stdout("789");
    mx(dir.path())
        .args(["cat", "--tail", "30", path(&file)])
        .assert()
        .success()
        .stdout("0123456789");
    mx(dir.path())
        .args(["cat", "--offset", "11", path(&file)])
        .assert()
        .failure()
        .stderr(predicate::str::contains("bigger than file"));
}

#[test]
fn cat_rejects_invalid_flag_combinations() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    fs::write(&file, "x").unwrap();
    let f = path(&file);
    for (args, message) in [
        (
            vec!["--tail", "1", "--offset", "1", f],
            "both --tail and --offset",
        ),
        (vec!["--tail", "-1", f], "negative"),
        (
            vec!["--zip", "--offset", "1", f],
            "--zip with --tail or --offset",
        ),
        (
            vec!["--part-number", "1", "--offset", "1", f],
            "--part-number with",
        ),
        (
            vec!["--vid", "v1", "--rewind", "1d", f],
            "--version-id and --rewind",
        ),
        (vec!["--vid", "v1", f, f], "exactly one argument"),
        (vec!["--offset", "1"], "stdin"),
        (vec!["--offset", "1", "-"], "stdin"),
        (vec!["--vid", "v1", f], "only supported for S3"),
        (vec!["--zip", f], "only supported for S3"),
        (vec!["--rewind", "bogus", f], "invalid time"),
    ] {
        mx(dir.path())
            .arg("cat")
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(message));
    }
}

#[test]
fn cat_help_lists_mc_flags() {
    let dir = tempfile::tempdir().unwrap();
    let text = help(dir.path(), "cat");
    for flag in [
        "--rewind",
        "--version-id",
        "--zip",
        "--offset",
        "--tail",
        "--part-number",
        "--enc-c",
    ] {
        assert!(text.contains(flag), "missing {flag}");
    }
}

// ---------------------------------------------------------------------------
// head
// ---------------------------------------------------------------------------

#[test]
fn head_prints_lines_from_files_and_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "1\n2\n3\n").unwrap();
    fs::write(&b, "x\r\ny\nz").unwrap();
    mx(dir.path())
        .args(["head", "-n", "2", path(&a), path(&b)])
        .assert()
        .success()
        .stdout("1\n2\nx\ny\n");
    mx(dir.path())
        .args(["head", "-n", "1"])
        .write_stdin("first\nsecond\n")
        .assert()
        .success()
        .stdout("first\n");
    let many: String = (1..=20).map(|i| format!("{i}\n")).collect();
    let expected: String = (1..=10).map(|i| format!("{i}\n")).collect();
    mx(dir.path())
        .args(["head", "-n", "-5", "-"])
        .write_stdin(many)
        .assert()
        .success()
        .stdout(expected);
}

#[test]
fn head_rejects_invalid_flags() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "1\n").unwrap();
    let f = path(&file);
    for (args, message) in [
        (
            vec!["--vid", "v1", "--rewind", "1d", f],
            "--version-id and --rewind",
        ),
        (vec!["--vid", "v1", f], "only supported for S3"),
        (vec!["--zip", f], "only supported for S3"),
        (vec!["--vid", "v1", f, f], "exactly one argument"),
    ] {
        mx(dir.path())
            .arg("head")
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(message));
    }
}

// ---------------------------------------------------------------------------
// get / put / pipe
// ---------------------------------------------------------------------------

#[test]
fn get_help_lists_version_and_encryption() {
    let dir = tempfile::tempdir().unwrap();
    mx(dir.path())
        .args(["get", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--version-id"))
        .stdout(predicate::str::contains("--enc-c"));
}

#[test]
fn put_help_and_validation() {
    let dir = tempfile::tempdir().unwrap();
    let text = help(dir.path(), "put");
    for flag in [
        "--parallel",
        "--part-size",
        "--disable-multipart",
        "--storage-class",
        "--checksum",
        "--enc-c",
        "--enc-s3",
        "--enc-kms",
    ] {
        assert!(text.contains(flag), "missing {flag}");
    }
    assert!(!text.contains("--if-not-exists"));

    let file = dir.path().join("f.txt");
    fs::write(&file, "x").unwrap();
    for (args, message) in [
        (vec!["-P", "0"], "Invalid number of threads"),
        (vec!["-s", "lots"], "Unable to parse part size"),
        (vec!["--checksum", "md5"], "invalid checksum"),
    ] {
        mx(dir.path())
            .arg("put")
            .args(&args)
            .args([path(&file), "play/bucket/"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(message));
    }
    // -sc is rewritten to --storage-class and accepted by the parser. Like mc, problems found
    // while preparing the upload are reported with exit status 0.
    mx(dir.path())
        .args(["put", "-sc", "STANDARD", path(&file), "missing/bucket/"])
        .assert()
        .success()
        .stderr("mx: <ERROR> Unable to upload. Target is not s3.\n");
    // Multiple sources need a folder target; folders cannot be uploaded.
    mx(dir.path())
        .args(["put", path(&file), path(&file), "play/bucket/obj"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must be a folder"));
    mx(dir.path())
        .args(["put", path(dir.path()), "play/bucket/"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Unable to upload. Invalid arguments provided",
        ));
}

#[test]
fn pipe_writes_local_files_and_accepts_both_quiet_spellings() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    mx(dir.path())
        .args(["pipe", path(&out)])
        .write_stdin("hello")
        .assert()
        .success()
        // mc's progress residue (also when stdout is not a terminal).
        .stdout(format!("\r 0 B / ? 5 bytes -> `{}`\n", path(&out)));
    assert_eq!(fs::read_to_string(&out).unwrap(), "hello");
    for flag in ["-q", "--quiet"] {
        mx(dir.path())
            .args(["pipe", flag, path(&out)])
            .write_stdin("again")
            .assert()
            .success()
            .stdout(format!("5 bytes -> `{}`\n", path(&out)));
        mx(dir.path())
            .args([flag, "pipe", path(&out)])
            .write_stdin("again")
            .assert()
            .success()
            .stdout(format!("5 bytes -> `{}`\n", path(&out)));
    }
    mx(dir.path())
        .args(["pipe", "--attr", "a=b", path(&out)])
        .write_stdin("x")
        .assert()
        .failure()
        .stderr(predicate::str::contains("only supported for S3 targets"));
    for (args, message) in [
        (vec!["--concurrent", "0"], "--concurrent"),
        (vec!["--part-size", "big"], "--part-size"),
    ] {
        mx(dir.path())
            .arg("pipe")
            .args(&args)
            .arg(path(&out))
            .write_stdin("x")
            .assert()
            .failure()
            .stderr(predicate::str::contains(message));
    }
    let text = help(dir.path(), "pipe");
    for flag in [
        "--storage-class",
        "--attr",
        "--tags",
        "--concurrent",
        "--part-size",
        "--checksum",
        "--enc-kms",
    ] {
        assert!(text.contains(flag), "missing {flag}");
    }
}

// ---------------------------------------------------------------------------
// find
// ---------------------------------------------------------------------------

fn find_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    fs::create_dir_all(root.join("logs/old")).unwrap();
    fs::write(root.join("a.txt"), "a").unwrap();
    fs::write(root.join("big.bin"), vec![0u8; 2048]).unwrap();
    fs::write(root.join("logs/app.log"), "log line").unwrap();
    fs::write(root.join("logs/old/app.txt"), "old").unwrap();
    dir
}

fn find_lines(home: &Path, args: &[&str]) -> Vec<String> {
    let output = mx(home).arg("find").args(args).output().unwrap();
    assert!(
        output.status.success(),
        "find {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut lines: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

#[test]
fn find_filters_local_trees() {
    let dir = find_tree();
    let home = dir.path();
    let root = dir.path().join("data");
    let r = path(&root);
    assert_eq!(
        find_lines(home, &[r, "--name", "*.txt"]),
        ["a.txt", "logs/old/app.txt"]
    );
    assert_eq!(
        find_lines(home, &[r, "--name", "logs"]),
        ["logs/app.log", "logs/old/app.txt"]
    );
    assert_eq!(
        find_lines(home, &[r, "--path", "logs/*"]),
        ["logs/app.log", "logs/old/app.txt"]
    );
    assert_eq!(
        find_lines(home, &[r, "--ignore", "*.txt"]),
        ["big.bin", "logs/app.log"]
    );
    assert_eq!(
        find_lines(home, &[r, "--regex", r"^logs/.*\.log$"]),
        ["logs/app.log"]
    );
    assert_eq!(find_lines(home, &[r, "--larger", "1KiB"]), ["big.bin"]);
    assert_eq!(find_lines(home, &[r, "--smaller", "2"]), ["a.txt"]);
    assert_eq!(
        find_lines(home, &[r, "--maxdepth", "1"]),
        ["a.txt", "big.bin"]
    );
    assert_eq!(find_lines(home, &[r, "--maxdepth", "0"]).len(), 4);
    assert_eq!(find_lines(home, &[r, "--newer-than", "1d"]).len(), 4);
    assert!(find_lines(home, &[r, "--older-than", "1d"]).is_empty());
}

#[test]
fn find_print_and_exec_on_local_trees() {
    let dir = find_tree();
    let home = dir.path();
    let root = dir.path().join("data");
    let r = path(&root);
    assert_eq!(
        find_lines(
            home,
            &[r, "--name", "big.bin", "--print", "{base}|{size}|{dir}"]
        ),
        [format!("big.bin|2.0 KiB|{r}")]
    );
    assert_eq!(
        find_lines(home, &[r, "--name", "a.txt", "--print", "{}"]),
        [format!("{r}/a.txt")]
    );
    assert_eq!(
        find_lines(
            home,
            &[r, "--name", "a.txt", "--exec", "echo found {base} '{size}'"]
        ),
        ["found a.txt 1 B"]
    );
    mx(home)
        .args([
            "find",
            r,
            "--name",
            "a.txt",
            "--exec",
            "sh -c 'echo boom >&2; exit 3'",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("boom"));
    mx(home)
        .args(["find", r, "--print", "{url}"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("only supported for S3"));
    let output = mx(home)
        .args(["--json", "find", r, "--name", "a.txt"])
        .output()
        .unwrap();
    let message: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(message["status"], "success");
    assert_eq!(message["key"], "a.txt");
    assert_eq!(message["size"], 1);
}

#[test]
fn find_rejects_invalid_inputs() {
    let dir = find_tree();
    let root = dir.path().join("data");
    let r = path(&root);
    for (args, message) in [
        (vec![r, "--regex", "("], "invalid --regex"),
        (vec![r, "--larger", "huge"], "Unable to parse input bytes"),
        (vec![r, "--older-than", "5y"], "invalid duration"),
        (vec![r, "--versions"], "only supported for S3"),
        (vec![r, "--metadata", "a=b"], "only supported for S3"),
        (vec![r, "--tags", "a=b"], "only supported for S3"),
        (vec!["/definitely/missing/path"], "Unable to stat"),
    ] {
        mx(dir.path())
            .arg("find")
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(message));
    }
}

#[test]
fn find_watch_reports_new_local_files() {
    let dir = find_tree();
    let root = dir.path().join("data");
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("mx"))
        .env("HOME", dir.path())
        .args(["find", path(&root), "--name", "*.new", "--watch"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    std::thread::sleep(Duration::from_millis(500));
    fs::write(root.join("fresh.new"), "new").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seen = None;
    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            seen = Some(line);
            break;
        }
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(seen.as_deref(), Some("fresh.new"));
}
