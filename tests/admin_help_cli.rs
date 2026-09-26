//! Help output lists the admin, idp, batch, sql and watch command groups like mc.

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
