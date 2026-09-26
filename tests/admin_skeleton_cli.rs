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
// IAM
// ---------------------------------------------------------------------------

const IAM: &[&[&str]] = &[
    &["admin", "user", "add", "a"],
    &["admin", "user", "disable", "a", "a"],
    &["admin", "user", "enable", "a", "a"],
    &["admin", "user", "remove", "a", "a"],
    &["admin", "user", "list", "a"],
    &["admin", "user", "info", "a", "a"],
    &["admin", "user", "policy", "a", "a"],
    &["admin", "user", "svcacct", "add", "a", "a"],
    &["admin", "user", "svcacct", "list", "a", "a"],
    &["admin", "user", "svcacct", "remove", "a", "a"],
    &["admin", "user", "svcacct", "info", "a", "a"],
    &["admin", "user", "svcacct", "edit", "a", "a"],
    &["admin", "user", "svcacct", "enable", "a", "a"],
    &["admin", "user", "svcacct", "disable", "a", "a"],
    &["admin", "user", "sts", "info", "a", "a"],
    &["admin", "group", "add", "a", "a", "a"],
    &["admin", "group", "remove", "a", "a"],
    &["admin", "group", "info", "a", "a"],
    &["admin", "group", "list", "a"],
    &["admin", "group", "enable", "a", "a"],
    &["admin", "group", "disable", "a", "a"],
    &["admin", "policy", "create", "a", "a", "a"],
    &["admin", "policy", "remove", "a", "a"],
    &["admin", "policy", "list", "a"],
    &["admin", "policy", "info", "a", "a"],
    &["admin", "policy", "attach", "a", "a"],
    &["admin", "policy", "detach", "a", "a"],
    &["admin", "policy", "entities", "a"],
    &["admin", "accesskey", "list", "a"],
    &["admin", "accesskey", "remove", "a", "a"],
    &["admin", "accesskey", "info", "a", "a"],
    &["admin", "accesskey", "create"],
    &["admin", "accesskey", "edit"],
    &["admin", "accesskey", "enable"],
    &["admin", "accesskey", "disable"],
    &["admin", "accesskey", "sts-revoke", "a"],
];

#[test]
fn iam_stubs() {
    assert_stubs(IAM);
}

// ---------------------------------------------------------------------------
// IDP
// ---------------------------------------------------------------------------

const IDP: &[&[&str]] = &[
    &["idp", "openid", "add", "a"],
    &["idp", "openid", "update", "a"],
    &["idp", "openid", "remove", "a"],
    &["idp", "openid", "list", "a"],
    &["idp", "openid", "info", "a"],
    &["idp", "openid", "enable", "a"],
    &["idp", "openid", "disable", "a"],
    &["idp", "openid", "accesskey", "list", "a"],
    &["idp", "openid", "accesskey", "remove", "a", "a"],
    &["idp", "openid", "accesskey", "info", "a", "a"],
    &["idp", "openid", "accesskey", "edit"],
    &["idp", "openid", "accesskey", "enable"],
    &["idp", "openid", "accesskey", "disable"],
    &["idp", "ldap", "add", "a"],
    &["idp", "ldap", "update", "a"],
    &["idp", "ldap", "remove", "a"],
    &["idp", "ldap", "list", "a"],
    &["idp", "ldap", "info", "a"],
    &["idp", "ldap", "enable", "a"],
    &["idp", "ldap", "disable", "a"],
    &["idp", "ldap", "policy", "attach", "a", "a"],
    &["idp", "ldap", "policy", "detach", "a", "a"],
    &["idp", "ldap", "policy", "entities", "a"],
    &["idp", "ldap", "accesskey", "list", "a"],
    &["idp", "ldap", "accesskey", "remove", "a", "a"],
    &["idp", "ldap", "accesskey", "info", "a", "a"],
    &["idp", "ldap", "accesskey", "create"],
    &["idp", "ldap", "accesskey", "create-with-login", "a"],
    &["idp", "ldap", "accesskey", "edit"],
    &["idp", "ldap", "accesskey", "enable"],
    &["idp", "ldap", "accesskey", "disable"],
    &["idp", "ldap", "accesskey", "sts-revoke", "a"],
];

#[test]
fn idp_stubs() {
    assert_stubs(IDP);
}

// ---------------------------------------------------------------------------
// STREAM
// ---------------------------------------------------------------------------

const STREAM: &[&[&str]] = &[
    &["admin", "scanner", "trace", "a"],
    &["admin", "trace", "a"],
    &["admin", "logs", "a"],
    &["admin", "heal", "a"],
    &["watch", "a"],
];

#[test]
fn stream_stubs() {
    assert_stubs(STREAM);
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
