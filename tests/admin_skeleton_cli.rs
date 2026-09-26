//! Commands registered for mc parity but not implemented yet: each must parse and fail with
//! "not implemented yet". Area agents delete their rows as they implement commands (and add
//! real tests); delete a table (and its test) once it is empty.

use assert_cmd::Command;

/// The new command groups are listed in help like mc's (visible names and aliases).
#[test]
fn help_lists_new_commands() {
    let help = |args: &[&str]| {
        let output = Command::cargo_bin("mx")
            .expect("binary")
            .args(args)
            .arg("--help")
            .output()
            .expect("run mx");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let top = help(&[]);
    for name in ["admin", "idp", "batch", "sql", "watch", "update"] {
        assert!(
            top.contains(&format!("  {name} ")),
            "{name} missing:\n{top}"
        );
    }
    let admin = help(&["admin"]);
    for name in [
        "service",
        "update",
        "info",
        "user",
        "group",
        "policy",
        "replicate",
        "config",
        "decommission",
        "heal",
        "prometheus",
        "kms",
        "scanner",
        "top",
        "trace",
        "cluster",
        "rebalance",
        "logs",
        "accesskey",
        "decom",
    ] {
        assert!(admin.contains(name), "admin {name} missing:\n{admin}");
    }
    for hidden in ["inspect", "speedtest", "console", "subnet", "profile"] {
        assert!(!admin.contains(hidden), "admin {hidden} should be hidden");
    }
}

fn assert_stubs(rows: &[&[&str]]) {
    let home = tempfile::tempdir().expect("tempdir");
    for args in rows {
        let output = Command::cargo_bin("mx")
            .expect("binary")
            .env("HOME", home.path())
            .args(*args)
            .output()
            .expect("run mx");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(1),
            "mx {}: {stderr}",
            args.join(" ")
        );
        assert!(
            stderr.contains("is not implemented yet"),
            "mx {}: {stderr}",
            args.join(" ")
        );
    }
}

// ---------------------------------------------------------------------------
// TOPO
// ---------------------------------------------------------------------------

const TOPO: &[&[&str]] = &[
    &["admin", "replicate", "add", "a"],
    &["admin", "replicate", "update", "a"],
    &["admin", "replicate", "remove", "a"],
    &["admin", "replicate", "info", "a"],
    &["admin", "replicate", "status", "a"],
    &["admin", "replicate", "resync", "start", "a", "a"],
    &["admin", "replicate", "resync", "status", "a", "a"],
    &["admin", "replicate", "resync", "cancel", "a", "a"],
    &["admin", "decommission", "start", "a", "a"],
    &["admin", "decommission", "status", "a"],
    &["admin", "decommission", "cancel", "a"],
    &["admin", "rebalance", "start", "a"],
    &["admin", "rebalance", "status", "a"],
    &["admin", "rebalance", "stop", "a"],
];

#[test]
fn topo_stubs() {
    assert_stubs(TOPO);
}

// ---------------------------------------------------------------------------
// JOBS
// ---------------------------------------------------------------------------

const JOBS: &[&[&str]] = &[
    &["batch", "generate", "a", "a"],
    &["batch", "start", "a", "a"],
    &["batch", "list", "a"],
    &["batch", "status", "a", "a"],
    &["batch", "describe", "a", "a"],
    &["batch", "cancel", "a"],
    &["sql", "a"],
];

#[test]
fn jobs_stubs() {
    assert_stubs(JOBS);
}
