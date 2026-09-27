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

/// Runs `mx ARGS` so that mode-000 folders are really unreadable: directly as a normal user,
/// via `setpriv` as `nobody` (with a world-readable copy of the binary) when root. `None` when
/// that is not possible.
#[cfg(unix)]
fn run_unprivileged(work: &std::path::Path, args: &[&str]) -> Option<std::process::Output> {
    use std::os::unix::fs::PermissionsExt;
    let bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_mx"));
    let config = work.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o777)).unwrap();
    let uid = std::process::Command::new("id").arg("-u").output().ok()?;
    let mut cmd = if String::from_utf8_lossy(&uid.stdout).trim() == "0" {
        let copy = work.join("mx");
        std::fs::copy(&bin, &copy).unwrap();
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut cmd = std::process::Command::new("setpriv");
        cmd.args(["--reuid=65534", "--regid=65534", "--clear-groups"])
            .arg(copy);
        cmd
    } else {
        std::process::Command::new(bin)
    };
    let out = cmd
        .env("NO_COLOR", "1")
        .arg("-C")
        .arg(&config)
        .args(args)
        .output()
        .ok()?;
    // setpriv missing or not permitted.
    (!String::from_utf8_lossy(&out.stderr).contains("setpriv")).then_some(out)
}

#[cfg(unix)]
#[test]
fn local_walks_continue_past_unreadable_folders_like_mc() {
    use std::os::unix::fs::PermissionsExt;
    let work = tempfile::tempdir().expect("tempdir");
    std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let root = work.path().join("tree");
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::create_dir_all(root.join("bad")).unwrap();
    std::fs::write(root.join("a/f1"), "hi\n").unwrap();
    std::fs::write(root.join("bad/secret"), "x\n").unwrap();
    std::fs::write(root.join("top"), "z\n").unwrap();
    std::fs::set_permissions(root.join("bad"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let path = root.to_str().unwrap();
    let bad = format!("{path}/bad");
    let denied = format!(
        "<ERROR> Unable to list folder. Insufficient permissions to access this path `{bad}`"
    );
    let Some(ls) = run_unprivileged(work.path(), &["ls", "-r", path]) else {
        eprintln!("skipping: cannot drop privileges");
        return;
    };
    // mc `ls -r`: reports the folder, lists the rest, exits 1.
    let stdout = String::from_utf8_lossy(&ls.stdout);
    assert_eq!(ls.status.code(), Some(1), "{ls:?}");
    assert!(
        String::from_utf8_lossy(&ls.stderr).contains(&denied),
        "{ls:?}"
    );
    assert!(
        stdout.contains(" a/f1\n") && stdout.contains(" top\n"),
        "{stdout}"
    );
    // An unreadable walk root is skipped silently.
    let ls = run_unprivileged(work.path(), &["ls", "-r", &bad]).unwrap();
    assert_eq!(ls.status.code(), Some(0), "{ls:?}");
    assert!(ls.stdout.is_empty() && ls.stderr.is_empty(), "{ls:?}");
    // A plain listing reports mc's raw `open` error.
    let ls = run_unprivileged(work.path(), &["ls", &bad]).unwrap();
    assert_eq!(ls.status.code(), Some(1), "{ls:?}");
    assert!(
        String::from_utf8_lossy(&ls.stderr).contains(&format!(
            "Unable to list folder. open {bad}/: permission denied"
        )),
        "{ls:?}"
    );
    // mc `du`: reports the folder, still prints the total, exits 0.
    let du = run_unprivileged(work.path(), &["du", path]).unwrap();
    assert_eq!(du.status.code(), Some(0), "{du:?}");
    assert!(
        String::from_utf8_lossy(&du.stderr).contains(&denied),
        "{du:?}"
    );
    assert_eq!(
        String::from_utf8_lossy(&du.stdout),
        format!("5B\t2 objects\t{}\n", path.trim_matches('/'))
    );
    let du = run_unprivileged(work.path(), &["du", "-d", "2", path]).unwrap();
    assert_eq!(du.status.code(), Some(0), "{du:?}");
    let trimmed = path.trim_matches('/');
    assert_eq!(
        String::from_utf8_lossy(&du.stdout),
        format!(
            "3B\t1 object\t{trimmed}/a\n0B\t0 objects\t{trimmed}/bad\n5B\t2 objects\t{trimmed}\n"
        )
    );
    // `du -r` lists every level plainly, so the unreadable folder is fatal like in mc.
    let du = run_unprivileged(work.path(), &["du", "-r", path]).unwrap();
    assert_eq!(du.status.code(), Some(1), "{du:?}");
    assert!(
        String::from_utf8_lossy(&du.stderr).contains(&format!(
            "Failed to find disk usage of `{bad}` recursively. open {bad}/: permission denied"
        )),
        "{du:?}"
    );
    std::fs::set_permissions(root.join("bad"), std::fs::Permissions::from_mode(0o755)).unwrap();
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

/// Runs `mx ARGS` in `work` and returns (success, stdout, stderr).
fn run_in(home: &std::path::Path, work: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    let out = mx(home).current_dir(work).args(args).output().expect("run");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn stat_and_rm_handle_local_paths_like_mc() {
    let home = tempfile::tempdir().expect("tempdir");
    let work = tempfile::tempdir().expect("tempdir");
    let w = work.path();
    std::fs::create_dir_all(w.join("sub/d")).unwrap();
    std::fs::write(w.join("top.txt"), "t\n").unwrap();
    std::fs::write(w.join("sub/a.txt"), "a\n").unwrap();
    std::fs::write(w.join("sub/d/b.txt"), "b\n").unwrap();

    let (ok, out, _) = run_in(home.path(), w, &["stat", "top.txt"]);
    assert!(ok);
    assert!(
        out.starts_with("Name      : top.txt\nDate      : "),
        "{out}"
    );
    assert!(
        out.contains("\nSize      : 2 B    \nType      : file \nMetadata  :\n"),
        "{out}"
    );
    assert!(
        out.contains("  Content-Type       : text/plain \n"),
        "{out}"
    );

    let (ok, out, _) = run_in(home.path(), w, &["--json", "stat", "sub/"]);
    assert!(ok);
    let docs: Vec<serde_json::Value> = out
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(docs.len(), 2, "{out}");
    assert_eq!(docs[0]["name"], "a.txt");
    assert_eq!(docs[0]["type"], "file");
    assert_eq!(docs[1]["name"], "d/");
    assert_eq!(docs[1]["type"], "folder");
    assert_eq!(
        docs[1]["metadata"]["Content-Type"],
        "application/octet-stream"
    );

    let (ok, _, err) = run_in(home.path(), w, &["stat", "nope"]);
    assert!(!ok);
    assert!(
        err.contains("Unable to stat `nope`. Object does not exist."),
        "{err}"
    );

    let (ok, _, err) = run_in(home.path(), w, &["rm", "-r", "sub/d"]);
    assert!(!ok);
    assert!(err.contains("Removal requires --force flag."), "{err}");
    let (ok, _, err) = run_in(home.path(), w, &["rm", "-r", "--force", "sub"]);
    assert!(!ok);
    assert!(err.contains("‘--dangerous’"), "{err}");

    let (ok, out, _) = run_in(
        home.path(),
        w,
        &["rm", "-r", "--force", "--dry-run", "sub/d"],
    );
    assert!(ok);
    assert!(out.starts_with("DRYRUN: Removing `"), "{out}");
    assert!(out.trim_end().ends_with("/sub/d/b.txt`."), "{out}");
    assert!(w.join("sub/d/b.txt").exists());

    let (ok, out, _) = run_in(home.path(), w, &["rm", "-r", "--force", "sub/d"]);
    assert!(ok);
    assert!(out.trim_end().ends_with("/sub/d/b.txt`."), "{out}");
    assert!(!w.join("sub/d").exists());
    assert!(w.join("sub/a.txt").exists());

    let (ok, out, _) = run_in(home.path(), w, &["rm", "top.txt"]);
    assert!(ok);
    assert_eq!(out, "Removed `top.txt`.\n");
    assert!(!w.join("top.txt").exists());

    let (ok, out, err) = run_in(home.path(), w, &["rm", "nope", "sub/a.txt"]);
    assert!(!ok);
    assert_eq!(out, "Removed `sub/a.txt`.\n");
    assert!(
        err.contains("Failed to remove `nope`. Requested path `"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn rm_never_follows_local_symlinks() {
    let home = tempfile::tempdir().expect("tempdir");
    let work = tempfile::tempdir().expect("tempdir");
    let w = work.path();
    std::fs::create_dir_all(w.join("keep")).unwrap();
    std::fs::create_dir_all(w.join("x/d")).unwrap();
    std::fs::write(w.join("keep/k"), "k").unwrap();
    std::os::unix::fs::symlink(w.join("keep"), w.join("x/d/link")).unwrap();
    std::os::unix::fs::symlink(w.join("keep"), w.join("x/top")).unwrap();

    let (ok, out, _) = run_in(home.path(), w, &["rm", "-r", "--force", "x/d"]);
    assert!(ok);
    assert!(out.trim_end().ends_with("/x/d/link`."), "{out}");
    assert!(!w.join("x/d").exists());
    // A symlinked target is removed as a link (mc would walk into it).
    let (ok, out, _) = run_in(home.path(), w, &["rm", "-r", "--force", "x/top/"]);
    assert!(ok);
    assert!(out.trim_end().ends_with("/x/top`."), "{out}");
    assert!(std::fs::symlink_metadata(w.join("x/top")).is_err());
    assert!(w.join("keep/k").exists());
}
