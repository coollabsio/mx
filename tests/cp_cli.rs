//! Offline `cp`/`mv` tests: flag validation and local-to-local copies (no S3 needed).

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn mx(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("mx").expect("binary");
    cmd.env("HOME", home).env("NO_COLOR", "1");
    cmd
}

fn s(path: &Path) -> String {
    path.to_str().expect("utf8 path").to_string()
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn set_mtime(path: &Path, age: Duration) {
    let time = SystemTime::now() - age;
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(time)
                .set_accessed(time),
        )
        .unwrap();
}

#[test]
fn cp_help_lists_mc_flags() {
    let home = tempfile::tempdir().unwrap();
    let assert = mx(home.path()).args(["cp", "--help"]).assert().success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for flag in [
        "--recursive",
        "--older-than",
        "--newer-than",
        "--storage-class",
        "--attr",
        "--tags",
        "--preserve",
        "--disable-multipart",
        "--checksum",
        "--enc-c",
        "--enc-s3",
        "--enc-kms",
        "--rewind",
        "--version-id",
        "--legal-hold",
        "--retention-mode",
        "--retention-duration",
        "--zip",
        "--max-workers",
    ] {
        assert!(out.contains(flag), "missing {flag}");
    }
    assert!(out.contains("--preserve, -a"));
    assert!(out.contains("--recursive, -r"));
}

#[test]
fn mv_help_lists_only_mv_flags() {
    let home = tempfile::tempdir().unwrap();
    let assert = mx(home.path()).args(["mv", "--help"]).assert().success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for flag in [
        "--recursive",
        "--older-than",
        "--newer-than",
        "--storage-class",
        "--attr",
        "--tags",
        "--preserve",
        "--disable-multipart",
        "--checksum",
        "--enc-s3",
    ] {
        assert!(out.contains(flag), "missing {flag}");
    }
    for flag in ["--rewind", "--version-id", "--zip", "--legal-hold"] {
        assert!(!out.contains(flag), "unexpected {flag}");
    }
}

#[test]
fn cp_rejects_invalid_flag_values() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    write(&file, "a");
    let file = s(&file);
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec!["--legal-hold", "maybe"], "invalid legal hold"),
        (
            vec!["--retention-mode", "governance"],
            "Both object retention flags",
        ),
        (vec!["--retention-duration", "1d"], "Both object retention"),
        (
            vec!["--retention-mode", "strict", "--retention-duration", "1d"],
            "invalid retention mode",
        ),
        (
            vec![
                "--retention-mode",
                "governance",
                "--retention-duration",
                "1w",
            ],
            "invalid retention duration",
        ),
        (vec!["--older-than", "abc"], "invalid duration"),
        (vec!["--rewind", "yesterday"], "invalid time"),
        (vec!["--attr", "novalue"], "key=value"),
        (vec!["--max-workers", "0"], "--max-workers"),
        (vec!["--enc-c", "play/b=short"], "SSE-C key"),
    ];
    for (flags, message) in cases {
        let mut args = vec!["cp"];
        args.extend(flags.iter());
        args.extend([file.as_str(), "play/bucket/"]);
        mx(home.path())
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(message));
    }
    mx(home.path())
        .args(["cp", "--checksum", "md5", &file, "play/bucket/"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid checksum"));
}

#[test]
fn cp_rejects_unsupported_combinations() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    write(&file, "a");
    let file = s(&file);
    let out = s(&dir.path().join("out/"));
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (
            vec!["--attr", "k=v", &file, &out],
            "`--attr` requires an S3 target",
        ),
        (
            vec!["-sc", "STANDARD", &file, &out],
            "`--storage-class` requires an S3 target",
        ),
        (
            vec!["--tags", "a=b", &file, &out],
            "`--tags` requires an S3 target",
        ),
        (
            vec!["--legal-hold", "on", &file, &out],
            "`--legal-hold` requires an S3 target",
        ),
        (
            vec!["--checksum", "crc32", &file, &out],
            "`--checksum` requires an S3 target",
        ),
        (
            vec!["--disable-multipart", &file, &out],
            "`--disable-multipart` requires an S3 target",
        ),
        (
            vec!["-vid", "v1", &file, "play/b/"],
            "`--version-id` requires an S3 source",
        ),
        (
            vec!["--rewind", "1d", &file, "play/b/"],
            "`--rewind` requires an S3 source",
        ),
        (
            vec!["--zip", &file, "play/b/"],
            "`--zip` requires an S3 source",
        ),
        (
            vec!["--vid", "v1", "play/b/x", "play/b/y", &out],
            "multiple copy sources",
        ),
        (
            vec!["--version-id", "v1", "-r", "play/b/x", &out],
            "cannot be used with `--recursive`",
        ),
        (
            vec!["--zip", "--rewind", "1d", "play/b/x.zip/a", &out],
            "--zip and --rewind cannot be used together",
        ),
        (vec![&file, "play"], "does not contain bucket name"),
    ];
    for (args, message) in cases {
        let mut argv = vec!["cp"];
        argv.extend(args.iter());
        mx(home.path())
            .args(&argv)
            .assert()
            .failure()
            .stderr(predicate::str::contains(message));
    }
    // Too few arguments: the help, exit status 1 (like mc).
    mx(home.path())
        .args(["cp", &file])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("USAGE:\n  mx cp [FLAGS] SOURCE"));
}

#[test]
fn cp_copies_a_local_file_with_mc_output() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    write(&file, "hello");
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let expected = out.join("a.txt");
    mx(home.path())
        .args(["cp", &s(&file), &s(&out)])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "`{}` -> `{}`",
            s(&file),
            s(&expected)
        )))
        .stdout(predicate::str::contains("Transferred"));
    assert_eq!(std::fs::read_to_string(&expected).unwrap(), "hello");
    assert!(!out.join("a.txt.part.minio").exists());

    // File to explicit file name.
    let renamed = dir.path().join("new/name.txt");
    mx(home.path())
        .args(["--quiet", "cp", &s(&file), &s(&renamed)])
        .assert()
        .success();
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "hello");
}

#[test]
fn cp_json_prints_mc_copy_messages() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("src/a.txt"), "aa");
    write(&dir.path().join("src/sub/b.txt"), "bbb");
    let out = dir.path().join("out");
    let assert = mx(home.path())
        .args(["--json", "cp", "-r", &s(&dir.path().join("src")), &s(&out)])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("json line"))
        .collect();
    assert_eq!(lines.len(), 3);
    let mut copies: Vec<_> = lines[..2].to_vec();
    copies.sort_by_key(|v| v["source"].as_str().unwrap().to_string());
    assert_eq!(copies[0]["status"], "success");
    assert_eq!(copies[0]["size"], 2);
    assert_eq!(copies[0]["totalCount"], 2);
    // mc never fills in totalSize (shadowed running total in cp-main.go).
    assert_eq!(copies[0]["totalSize"], 0);
    assert_eq!(copies[0]["target"], s(&out.join("src/a.txt")));
    assert_eq!(copies[1]["target"], s(&out.join("src/sub/b.txt")));
    let summary = &lines[2];
    assert_eq!(summary["status"], "success");
    assert_eq!(summary["total"], 5);
    assert_eq!(summary["transferred"], 5);
    assert!(summary["duration"].is_u64());
    assert!(summary["speed"].is_number());
}

#[test]
fn cp_recursive_trailing_slash_copies_contents() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("src/a.txt"), "a");
    write(&dir.path().join("src/deep/er/b.txt"), "b");

    let with_name = dir.path().join("with-name");
    mx(home.path())
        .args(["cp", "-r", &s(&dir.path().join("src")), &s(&with_name)])
        .assert()
        .success();
    assert!(with_name.join("src/a.txt").is_file());
    assert!(with_name.join("src/deep/er/b.txt").is_file());

    let contents = dir.path().join("contents");
    mx(home.path())
        .args([
            "cp",
            "--recursive",
            &format!("{}/", s(&dir.path().join("src"))),
            &s(&contents),
        ])
        .assert()
        .success();
    assert!(contents.join("a.txt").is_file());
    assert_eq!(
        std::fs::read_to_string(contents.join("deep/er/b.txt")).unwrap(),
        "b"
    );
}

#[test]
fn cp_requires_recursive_for_directories() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("src/a.txt"), "a");
    mx(home.path())
        .args(["cp", &s(&dir.path().join("src")), &s(&dir.path().join("o"))])
        .assert()
        .failure()
        .stderr(predicate::str::contains("the --recursive flag is required"));
    mx(home.path())
        .args([
            "cp",
            "-r",
            &s(&dir.path().join("src")),
            &s(&dir.path().join("src/inner")),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("into itself"));
}

#[test]
fn cp_multiple_sources_into_a_folder() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    write(&a, "a");
    write(&b, "b");
    write(&dir.path().join("d/c.txt"), "c");
    let out = dir.path().join("out/");
    mx(home.path())
        .args([
            "cp",
            "-r",
            &s(&a),
            &s(&b),
            &s(&dir.path().join("d")),
            &format!("{}/", s(&out)),
        ])
        .assert()
        .success();
    assert!(out.join("a.txt").is_file());
    assert!(out.join("b.txt").is_file());
    assert!(out.join("d/c.txt").is_file());

    let file_target = dir.path().join("plain.txt");
    write(&file_target, "x");
    mx(home.path())
        .args(["cp", &s(&a), &s(&b), &s(&file_target)])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not a folder"));
    mx(home.path())
        .args([
            "cp",
            &s(&a),
            &s(&dir.path().join("d")),
            &format!("{}/", s(&out)),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--recursive"));
}

#[test]
fn cp_filters_by_age() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("src/old.txt");
    let new = dir.path().join("src/new.txt");
    write(&old, "old");
    write(&new, "new");
    set_mtime(&old, Duration::from_secs(3 * 86_400));

    let older = dir.path().join("older");
    mx(home.path())
        .args([
            "cp",
            "-r",
            "--older-than",
            "1d",
            &format!("{}/", s(&dir.path().join("src"))),
            &s(&older),
        ])
        .assert()
        .success();
    assert!(older.join("old.txt").is_file());
    assert!(!older.join("new.txt").exists());

    let newer = dir.path().join("newer");
    mx(home.path())
        .args([
            "cp",
            "-r",
            "--newer-than",
            "1d",
            &format!("{}/", s(&dir.path().join("src"))),
            &s(&newer),
        ])
        .assert()
        .success();
    assert!(newer.join("new.txt").is_file());
    assert!(!newer.join("old.txt").exists());
}

#[cfg(unix)]
#[test]
fn cp_preserve_keeps_mode_and_mtime() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("script.sh");
    write(&file, "#!/bin/sh\n");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o751)).unwrap();
    set_mtime(&file, Duration::from_secs(10 * 86_400));
    let source_mtime = std::fs::metadata(&file).unwrap().modified().unwrap();

    let plain = dir.path().join("plain.sh");
    mx(home.path())
        .args(["cp", &s(&file), &s(&plain)])
        .assert()
        .success();
    assert_ne!(
        std::fs::metadata(&plain).unwrap().modified().unwrap(),
        source_mtime
    );

    let kept = dir.path().join("kept.sh");
    mx(home.path())
        .args(["cp", "-a", &s(&file), &s(&kept)])
        .assert()
        .success();
    let meta = std::fs::metadata(&kept).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o751);
    assert_eq!(meta.modified().unwrap(), source_mtime);
}

#[test]
fn mv_moves_local_files_and_folders() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    write(&file, "a");
    let target = dir.path().join("moved.txt");
    mx(home.path())
        .args(["mv", &s(&file), &s(&target)])
        .assert()
        .success()
        .stdout(predicate::str::contains("->"));
    assert!(!file.exists());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "a");

    write(&dir.path().join("tree/x.txt"), "x");
    write(&dir.path().join("tree/sub/y.txt"), "y");
    let out = dir.path().join("out");
    mx(home.path())
        .args(["mv", &s(&dir.path().join("tree")), &s(&out)])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--recursive"));
    mx(home.path())
        .args(["mv", "-r", &s(&dir.path().join("tree")), &s(&out)])
        .assert()
        .success();
    assert!(out.join("tree/x.txt").is_file());
    assert!(out.join("tree/sub/y.txt").is_file());
    assert!(!dir.path().join("tree").exists());
}

#[test]
fn mv_rejects_overlapping_source_and_target() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    write(&file, "a");
    mx(home.path())
        .args(["mv", &s(&file), &format!("{}/", s(dir.path()))])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be subdirectories"));
    assert!(file.exists());
    mx(home.path())
        .args(["mv", "--rewind", "1d", &s(&file), "play/b/"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("flag provided but not defined"));
}
