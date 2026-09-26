//! Hidden, deprecated mc commands answer with mc's `deprecatedError` (owner: SERVER for
//! `admin` level, IAM for `admin policy`, STREAM for `admin top`).

use assert_cmd::Command;

fn mx(args: &[&str]) -> std::process::Output {
    let home = tempfile::tempdir().expect("tempdir");
    Command::cargo_bin("mx")
        .expect("binary")
        .env("HOME", home.path())
        .args(args)
        .output()
        .expect("run mx")
}

#[test]
fn deprecated_commands_point_to_replacements() {
    let cases: &[(&[&str], &str)] = &[
        (&["admin", "inspect"], "mc support inspect"),
        (&["admin", "idp", "x"], "mc idp ldap|openid"),
        (&["admin", "speedtest", "play/"], "mc support perf"),
        (
            &["admin", "console", "--limit", "5", "-t", "MINIO", "x"],
            "mc admin logs --last 5 --type minio x",
        ),
        (
            &["admin", "policy", "add", "a", "b", "c"],
            "mc admin policy create",
        ),
        (&["admin", "policy", "set", "a"], "mc admin policy attach"),
        (&["admin", "policy", "unset", "a"], "mc admin policy detach"),
        (
            &["admin", "policy", "update", "a"],
            "mc admin policy attach",
        ),
        (&["admin", "top", "api", "a"], "mc support top api"),
        (
            &["admin", "top", "locks", "--stale", "a"],
            "mc support top locks",
        ),
    ];
    for (args, replacement) in cases {
        let output = mx(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!("mx: <ERROR> Deprecated command. Please use '{replacement}' instead.\n"),
            "{args:?}"
        );
    }
}

#[test]
fn deprecated_json_error() {
    let output = mx(&["--json", "admin", "policy", "add", "a", "b", "c"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"status\":\"error\",\"error\":{\"message\":\"Deprecated command\",\"cause\":{\"message\":\"Please use 'mc admin policy create' instead\",\"error\":{}},\"type\":\"fatal\"}}\n"
    );
}
