use assert_cmd::Command;
use predicates::prelude::*;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn mx(home: &Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("mx"));
    command.env("HOME", home);
    command
}

struct Dirs {
    home: tempfile::TempDir,
    src: tempfile::TempDir,
    dst: tempfile::TempDir,
}

impl Dirs {
    fn new() -> Self {
        let dirs = Self {
            home: tempfile::tempdir().unwrap(),
            src: tempfile::tempdir().unwrap(),
            dst: tempfile::tempdir().unwrap(),
        };
        write(&dirs.src.path().join("a.txt"), "alpha");
        write(&dirs.src.path().join("nested/deep/b.txt"), "bravo");
        dirs
    }

    fn mirror(&self, flags: &[&str]) -> Command {
        let mut command = mx(self.home.path());
        command
            .arg("mirror")
            .args(flags)
            .args([self.src(), self.dst()]);
        command
    }

    fn src(&self) -> &str {
        self.src.path().to_str().unwrap()
    }

    fn dst(&self) -> &str {
        self.dst.path().to_str().unwrap()
    }

    fn read_dst(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.dst.path().join(rel)).ok()
    }
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn set_mtime(path: &Path, age: Duration) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - age).unwrap();
}

fn stdout(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).to_string()
}

#[test]
fn mirror_help_lists_mc_flags() {
    let home = tempfile::tempdir().unwrap();
    let assert = mx(home.path())
        .args(["mirror", "--help"])
        .assert()
        .success();
    let out = stdout(&assert);
    for flag in [
        "--overwrite",
        "--remove",
        "--dry-run",
        "--watch",
        "--region",
        "--preserve",
        "--active-active",
        "--disable-multipart",
        "--exclude",
        "--exclude-bucket",
        "--exclude-storageclass",
        "--older-than",
        "--newer-than",
        "--storage-class",
        "--attr",
        "--retry",
        "--summary",
        "--skip-errors",
        "--max-workers",
        "--checksum",
        "--enc-c",
        "--enc-s3",
        "--enc-kms",
    ] {
        assert!(out.contains(flag), "missing {flag} in help");
    }
}

#[test]
fn mirror_local_to_local_copies_tree_and_is_idempotent() {
    let dirs = Dirs::new();
    dirs.mirror(&[])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.txt` -> `"))
        .stdout(predicate::str::contains("nested/deep/b.txt`"));
    assert_eq!(dirs.read_dst("a.txt").as_deref(), Some("alpha"));
    assert_eq!(dirs.read_dst("nested/deep/b.txt").as_deref(), Some("bravo"));
    // Nothing left to do on a second run: only mc's summary table.
    dirs.mirror(&[])
        .assert()
        .success()
        .stdout(predicate::str::contains("->").not())
        .stdout(predicate::str::contains(
            "│ 0 B   │ 0 B         │ 00m00s   │ 0 B/s │",
        ))
        .stderr(predicate::str::is_empty());
}

#[test]
fn mirror_creates_missing_target_folder() {
    let dirs = Dirs::new();
    let target = dirs.dst.path().join("new/sub");
    mx(dirs.home.path())
        .args(["mirror", dirs.src(), target.to_str().unwrap()])
        .assert()
        .success();
    assert!(target.join("nested/deep/b.txt").is_file());
}

#[test]
fn mirror_reports_differences_without_overwrite() {
    let dirs = Dirs::new();
    dirs.mirror(&[]).assert().success();
    write(&dirs.src.path().join("a.txt"), "alpha-changed");
    dirs.mirror(&[])
        .assert()
        .success()
        .stdout(predicate::str::contains("->").not())
        .stderr(predicate::str::contains(
            "Failed to perform mirroring, with error condition (size)",
        ))
        .stderr(predicate::str::contains("Overwrite not allowed for `"))
        .stderr(predicate::str::contains(
            "Use `--overwrite` to override this behavior.",
        ));
    assert_eq!(dirs.read_dst("a.txt").as_deref(), Some("alpha"));

    dirs.mirror(&["--overwrite"])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.txt`"));
    assert_eq!(dirs.read_dst("a.txt").as_deref(), Some("alpha-changed"));
}

#[test]
fn mirror_detects_newer_source_with_same_size() {
    let dirs = Dirs::new();
    dirs.mirror(&[]).assert().success();
    // Make the target older than the source.
    set_mtime(&dirs.dst.path().join("a.txt"), Duration::from_secs(3600));
    write(&dirs.src.path().join("a.txt"), "ALPHA");
    dirs.mirror(&[])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "error condition (mm-source-mtime)",
        ));
    dirs.mirror(&["--overwrite"]).assert().success();
    assert_eq!(dirs.read_dst("a.txt").as_deref(), Some("ALPHA"));
}

#[test]
fn mirror_remove_deletes_extraneous_target_files() {
    let dirs = Dirs::new();
    write(&dirs.dst.path().join("extra.txt"), "x");
    dirs.mirror(&[]).assert().success();
    assert!(dirs.read_dst("extra.txt").is_some());

    // mc's dry run prints no per-object lines.
    dirs.mirror(&["--remove", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("->").not());
    assert!(dirs.read_dst("extra.txt").is_some());

    // Initial-pass removals have no event type: mc prints `` -> `TARGET`.
    dirs.mirror(&["--remove"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "`` -> `{}/extra.txt`",
            dirs.dst()
        )));
    assert!(dirs.read_dst("extra.txt").is_none());
    assert!(dirs.read_dst("a.txt").is_some());
}

#[test]
fn mirror_dry_run_changes_nothing() {
    let dirs = Dirs::new();
    // Like mc: only the summary, with `Transferred` counted twice.
    dirs.mirror(&["--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.txt").not())
        .stdout(predicate::str::contains("│ 10 B  │ 20 B        │"));
    assert!(dirs.read_dst("a.txt").is_none());
    let assert = dirs.mirror(&["--dry-run"]).arg("--json").assert().success();
    let summary: serde_json::Value = serde_json::from_str(stdout(&assert).trim()).unwrap();
    assert_eq!(
        (summary["total"].as_u64(), summary["transferred"].as_u64()),
        (Some(10), Some(20))
    );
}

#[test]
fn mirror_exclude_patterns() {
    let dirs = Dirs::new();
    write(&dirs.src.path().join(".hidden"), "h");
    write(&dirs.src.path().join("tmp/x.temp"), "t");
    dirs.mirror(&["--exclude", ".*", "--exclude", "*.temp"])
        .assert()
        .success();
    assert!(dirs.read_dst(".hidden").is_none());
    assert!(dirs.read_dst("tmp/x.temp").is_none());
    assert!(dirs.read_dst("a.txt").is_some());
    // Excluded target files are not removed either.
    write(&dirs.dst.path().join("keep.temp"), "k");
    dirs.mirror(&["--remove", "--exclude", "*.temp"])
        .assert()
        .success();
    assert!(dirs.read_dst("keep.temp").is_some());
}

#[test]
fn mirror_time_filters() {
    let dirs = Dirs::new();
    set_mtime(
        &dirs.src.path().join("a.txt"),
        Duration::from_secs(3 * 86400),
    );
    dirs.mirror(&["--older-than", "1d"]).assert().success();
    assert!(dirs.read_dst("a.txt").is_some());
    assert!(dirs.read_dst("nested/deep/b.txt").is_none());

    let dirs = Dirs::new();
    set_mtime(
        &dirs.src.path().join("a.txt"),
        Duration::from_secs(3 * 86400),
    );
    dirs.mirror(&["--newer-than", "1d"]).assert().success();
    assert!(dirs.read_dst("a.txt").is_none());
    assert!(dirs.read_dst("nested/deep/b.txt").is_some());
}

#[test]
fn mirror_json_output_uses_mc_fields() {
    let dirs = Dirs::new();
    let assert = dirs.mirror(&[]).arg("--json").assert().success();
    let mut lines: Vec<serde_json::Value> = stdout(&assert)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    // The last document is mc's accounting summary.
    let summary = lines.pop().unwrap();
    assert_eq!(summary["total"], 10);
    assert_eq!(summary["transferred"], 10);
    for line in &lines {
        assert_eq!(line["status"], "success");
        for field in [
            "source",
            "target",
            "size",
            "totalCount",
            "totalSize",
            "eventTime",
            "eventType",
        ] {
            assert!(line.get(field).is_some(), "missing {field}: {line}");
        }
    }
    let a = lines
        .iter()
        .find(|l| l["source"].as_str().unwrap().ends_with("a.txt"))
        .unwrap();
    assert_eq!(a["size"], 5);
    assert!(a["target"].as_str().unwrap().ends_with("a.txt"));

    write(&dirs.src.path().join("a.txt"), "alpha-changed");
    let assert = dirs.mirror(&[]).arg("--json").assert().success();
    let out = stdout(&assert);
    let error: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(error["status"], "error");
    assert!(
        error["error"]["cause"]["message"]
            .as_str()
            .unwrap()
            .contains("Overwrite not allowed")
    );
}

#[test]
fn mirror_summary_replaces_per_object_lines() {
    let dirs = Dirs::new();
    dirs.mirror(&["--summary"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Total"))
        .stdout(predicate::str::contains("Transferred"))
        .stdout(predicate::str::contains("10 B"))
        .stdout(predicate::str::contains("->").not());

    let dirs = Dirs::new();
    let assert = dirs.mirror(&["--summary"]).arg("--json").assert().success();
    let summary: serde_json::Value = serde_json::from_str(stdout(&assert).trim()).unwrap();
    assert_eq!(summary["total"], 10);
    assert_eq!(summary["transferred"], 10);
    assert!(summary.get("duration").is_some() && summary.get("speed").is_some());
}

#[test]
fn mirror_max_workers_retry_and_skip_errors_flags() {
    let dirs = Dirs::new();
    for i in 0..12 {
        write(&dirs.src.path().join(format!("many/{i}.txt")), "x");
    }
    dirs.mirror(&["--max-workers", "1", "--skip-errors", "--retry"])
        .assert()
        .success();
    assert!(dirs.read_dst("many/11.txt").is_some());
}

#[test]
fn mirror_copy_failures_stop_unless_skip_errors() {
    // The temporary download path is blocked by a directory, so this copy fails (works even
    // when running as root, unlike permission-based failures).
    let dirs = Dirs::new();
    write(&dirs.src.path().join("0-conflict"), "file");
    std::fs::create_dir_all(dirs.dst.path().join("0-conflict.part.minio")).unwrap();
    dirs.mirror(&["--max-workers", "1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Failed to copy `"));
    // Without --skip-errors mirroring stops after the first failure.
    assert!(dirs.read_dst("nested/deep/b.txt").is_none());
    dirs.mirror(&["--max-workers", "1", "--skip-errors"])
        .assert()
        .failure();
    assert!(dirs.read_dst("nested/deep/b.txt").is_some());
}

#[cfg(unix)]
#[test]
fn mirror_preserve_keeps_mtime_and_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dirs = Dirs::new();
    let source = dirs.src.path().join("a.txt");
    set_mtime(&source, Duration::from_secs(7 * 86400));
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();
    dirs.mirror(&["-a"]).assert().success();
    let src_meta = std::fs::metadata(&source).unwrap();
    let dst_meta = std::fs::metadata(dirs.dst.path().join("a.txt")).unwrap();
    assert_eq!(src_meta.modified().unwrap(), dst_meta.modified().unwrap());
    assert_eq!(dst_meta.permissions().mode() & 0o777, 0o640);
}

#[test]
fn mirror_validates_source_and_flags() {
    let dirs = Dirs::new();
    let file = dirs.src.path().join("a.txt");
    mx(dirs.home.path())
        .args(["mirror", file.to_str().unwrap(), dirs.dst()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "is not a folder. Only folders are supported by mirror command.",
        ));
    mx(dirs.home.path())
        .args(["mirror", "/nonexistent/mx-mirror-src", dirs.dst()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unable to stat source"));
    dirs.mirror(&["--monitoring-address", "256.0.0.1:x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Unable to setup monitoring endpoint.",
        ));
    dirs.mirror(&["--older-than", "soon"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid duration"));
    dirs.mirror(&["--checksum", "md5"]).assert().failure();
    let target_file = dirs.dst.path().join("file");
    write(&target_file, "x");
    mx(dirs.home.path())
        .args(["mirror", dirs.src(), target_file.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not a folder"));
}

#[test]
fn mirror_watch_syncs_new_and_deleted_files() {
    let dirs = Dirs::new();
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin!("mx"))
        .env("HOME", dirs.home.path())
        .args([
            "mirror",
            "--watch",
            "--watch-interval",
            "1s",
            dirs.src(),
            dirs.dst(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let wait_for = |check: &dyn Fn() -> bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        false
    };
    let initial = wait_for(&|| dirs.read_dst("a.txt").is_some());
    write(&dirs.src.path().join("later.txt"), "later");
    let added = wait_for(&|| dirs.read_dst("later.txt").as_deref() == Some("later"));
    std::fs::remove_file(dirs.src.path().join("a.txt")).unwrap();
    let removed = wait_for(&|| dirs.read_dst("a.txt").is_none());
    let still_running = child.try_wait().unwrap().is_none();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(initial, "initial sync did not happen");
    assert!(added, "new file was not mirrored while watching");
    assert!(removed, "deleted file was not removed while watching");
    assert!(still_running, "watch mode exited early");
}
