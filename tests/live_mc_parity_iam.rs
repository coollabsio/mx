//! mc parity for the IAM admin commands: `admin user` (incl. `svcacct`, `sts`), `admin group`,
//! `admin policy`, `admin accesskey` (see tests/common/parity.rs).
//!
//!   MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity_iam
//!
//! IAM state is server-wide, so the cases run one at a time and name everything `{uniq}*`.
//! Listings that mc prints in Go map order are compared order-insensitively.

mod common;

use common::parity::{Parity, Tool, json_docs};
use serde_json::Value;
use std::sync::{Mutex, MutexGuard};

const POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::mxbucket/*"]}]}"#;
const POLICY2: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:PutObject"],"Resource":["arn:aws:s3:::mxbucket/*"]}]}"#;

static LOCK: Mutex<()> = Mutex::new(());

/// One IAM fixture at a time; entities named `{uniq}*` are removed on drop.
fn fixture() -> Option<(MutexGuard<'static, ()>, Parity)> {
    let guard = LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let mut p = Parity::bare()?;
    p.iam_cleanup();
    p.file("p.json", POLICY);
    p.file("p2.json", POLICY2);
    p.file("empty.json", r#"{"Version":"2012-10-17","Statement":[]}"#);
    p.file("bad.json", "xx");
    Some((guard, p))
}

/// Text and `--json` parity for a read-only command.
fn check(p: &Parity, args: &[&str]) {
    p.assert_parity(args, None);
    let mut json = vec!["--json"];
    json.extend_from_slice(args);
    p.assert_json_parity(&json, None);
}

/// Text parity for `text`, `--json` parity for `json` (mutating commands need two targets).
fn check_pair(p: &Parity, text: &[&str], json: &[&str]) {
    p.assert_parity(text, None);
    let mut args = vec!["--json"];
    args.extend_from_slice(json);
    p.assert_json_parity(&args, None);
}

/// Sorts every array (and the document list): the server returns access keys in varying
/// order, which mc prints as received.
fn canonical(value: &mut Value) {
    match value {
        Value::Array(items) => {
            items.iter_mut().for_each(canonical);
            items.sort_by_key(|item| item.to_string());
        }
        Value::Object(map) => map.values_mut().for_each(canonical),
        _ => {}
    }
}

/// Text parity (lines sorted) and `--json` parity with all arrays sorted.
fn check_any_order(p: &mut Parity, args: &[&str]) {
    p.unordered = true;
    p.assert_parity(args, None);
    p.unordered = false;
    let mut json = vec!["--json"];
    json.extend_from_slice(args);
    let (mc, mx) = p.run(&json, None);
    assert_eq!(mc.code, mx.code, "exit codes for {args:?}");
    let docs = |tool: Tool, text: &str| {
        let mut docs = Value::Array(json_docs(&p.normalize(tool, text)).expect("JSON output"));
        canonical(&mut docs);
        docs
    };
    assert_eq!(
        docs(Tool::Mc, &mc.stdout),
        docs(Tool::Mx, &mx.stdout),
        "{args:?}"
    );
}

/// Both tools print their usage help and exit with status 1 (help texts differ).
fn check_help_exit(p: &Parity, args: &[&str]) {
    let (mc, mx) = p.run(args, None);
    assert_eq!(mc.code, Some(1), "mc {args:?}: {mc:?}");
    assert_eq!(mx.code, Some(1), "mx {args:?}: {mx:?}");
    assert!(
        mx.stdout.contains("Usage") || mx.stdout.contains("USAGE"),
        "{mx:?}"
    );
}

fn user(p: &Parity, name: &str) {
    p.setup(&["admin", "user", "add", "{alias}", name, "secret12345"]);
}

// ---------------------------------------------------------------------------
// admin user
// ---------------------------------------------------------------------------

#[test]
fn user_add_enable_disable_remove() {
    let Some((_lock, p)) = fixture() else { return };
    check_pair(
        &p,
        &["admin", "user", "add", "{alias}", "{uniq}u1", "secret12345"],
        &["admin", "user", "add", "{alias}", "{uniq}u2", "secret12345"],
    );
    check(
        &p,
        &["admin", "user", "add", "{alias}", "u1", "secret12345"],
    );
    check(&p, &["admin", "user", "disable", "{alias}", "{uniq}u1"]);
    check(&p, &["admin", "user", "info", "{alias}", "{uniq}u1"]);
    check(&p, &["admin", "user", "enable", "{alias}", "{uniq}u1"]);
    check(&p, &["admin", "user", "disable", "{alias}", "{uniq}nouser"]);
    check_pair(
        &p,
        &["admin", "user", "remove", "{alias}", "{uniq}u1"],
        &["admin", "user", "rm", "{alias}", "{uniq}u2"],
    );
    check(&p, &["admin", "user", "remove", "{alias}", "{uniq}nouser"]);
    check_help_exit(&p, &["admin", "user", "info", "{alias}"]);
    check_help_exit(&p, &["admin", "user", "info", "{alias}", "a", "b"]);
}

#[test]
fn user_add_reads_keys_from_stdin() {
    let Some((_lock, p)) = fixture() else { return };
    // Same name on both sides (stdin is not templated); prefixed for cleanup.
    let name = format!("{}s1", p.mc.uniq.trim_end_matches("mc"));
    let input = format!("{name}\nsecret12345\n");
    p.assert_parity(&["admin", "user", "add", "{alias}"], Some(input.as_bytes()));
    p.assert_json_parity(
        &["--json", "admin", "user", "add", "{alias}", &name],
        Some(b"secret12345\n"),
    );
}

#[test]
fn user_list_info_policy() {
    let Some((_lock, mut p)) = fixture() else {
        return;
    };
    user(&p, "{uniq}u1");
    user(&p, "{uniq}u2");
    p.setup(&["admin", "policy", "create", "{alias}", "{uniq}p1", "p.json"]);
    p.setup(&[
        "admin", "policy", "create", "{alias}", "{uniq}p2", "p2.json",
    ]);
    p.setup(&[
        "admin", "policy", "attach", "{alias}", "{uniq}p1", "--user", "{uniq}u1",
    ]);
    p.setup(&[
        "admin", "policy", "attach", "{alias}", "{uniq}p2", "--user", "{uniq}u1",
    ]);
    p.setup(&["admin", "group", "add", "{alias}", "{uniq}g1", "{uniq}u1"]);
    p.setup(&[
        "admin", "policy", "attach", "{alias}", "{uniq}p2", "--group", "{uniq}g1",
    ]);
    check(&p, &["admin", "user", "info", "{alias}", "{uniq}u1"]);
    check(&p, &["admin", "user", "info", "{alias}", "{uniq}u2"]);
    check(&p, &["admin", "user", "info", "{alias}", "{uniq}nouser"]);
    check(&p, &["admin", "user", "policy", "{alias}", "{uniq}u1"]);
    check(&p, &["admin", "user", "policy", "{alias}", "{uniq}u2"]);
    check(&p, &["admin", "user", "list", "nosuchalias"]);
    p.unordered = true;
    check(&p, &["admin", "user", "list", "{alias}"]);
    check(&p, &["admin", "user", "ls", "{alias}/"]);
}

// ---------------------------------------------------------------------------
// admin user svcacct / sts
// ---------------------------------------------------------------------------

#[test]
fn svcacct_lifecycle() {
    let Some((_lock, mut p)) = fixture() else {
        return;
    };
    user(&p, "{uniq}u1");
    user(&p, "{uniq}u2");
    p.setup(&["admin", "policy", "create", "{alias}", "{uniq}p1", "p.json"]);
    p.setup(&[
        "admin", "policy", "attach", "{alias}", "{uniq}p1", "--user", "{uniq}u1",
    ]);
    let add = |key: &'static str| {
        [
            "admin",
            "user",
            "svcacct",
            "add",
            "{alias}",
            "{uniq}u1",
            "--access-key",
            key,
            "--secret-key",
            "secret12345",
        ]
    };
    check_pair(&p, &add("{uniq}k1"), &add("{uniq}k2"));
    let mut embedded = add("{uniq}k3").to_vec();
    embedded.extend(["--policy", "p2.json", "--name", "n1", "--description", "d1"]);
    let mut embedded_json = add("{uniq}k4").to_vec();
    embedded_json.extend(["--policy", "p2.json", "--name", "n1", "--comment", "c1"]);
    check_pair(&p, &embedded, &embedded_json);
    check(&p, &add("{uniq}k1"));
    for (flag, value) in [
        ("--policy", "missing.json"),
        ("--policy", "bad.json"),
        ("--policy", "empty.json"),
        ("--expiry", "tomorrow"),
    ] {
        let mut args = add("{uniq}k9").to_vec();
        args.extend([flag, value]);
        check(&p, &args);
    }
    // The server lists keys in varying order.
    check_any_order(
        &mut p,
        &["admin", "user", "svcacct", "list", "{alias}", "{uniq}u1"],
    );
    check(
        &p,
        &["admin", "user", "svcacct", "ls", "{alias}", "{uniq}u2"],
    );
    check(
        &p,
        &["admin", "user", "svcacct", "info", "{alias}", "{uniq}k1"],
    );
    check(
        &p,
        &["admin", "user", "svcacct", "info", "{alias}", "{uniq}k3"],
    );
    check(
        &p,
        &[
            "admin", "user", "svcacct", "info", "{alias}", "{uniq}k1", "--policy",
        ],
    );
    check(
        &p,
        &[
            "admin", "user", "svcacct", "info", "{alias}", "{uniq}k3", "--policy",
        ],
    );
    check(
        &p,
        &["admin", "user", "svcacct", "info", "{alias}", "{uniq}nokey"],
    );
    check(
        &p,
        &[
            "admin", "user", "svcacct", "edit", "{alias}", "{uniq}k1", "--name", "n2",
        ],
    );
    check(
        &p,
        &["admin", "user", "svcacct", "info", "{alias}", "{uniq}k1"],
    );
    check(
        &p,
        &[
            "admin", "user", "svcacct", "set", "{alias}", "{uniq}k1", "--policy", "bad.json",
        ],
    );
    check(
        &p,
        &[
            "admin", "user", "svcacct", "edit", "{alias}", "{uniq}k1", "--expiry", "x",
        ],
    );
    check(
        &p,
        &[
            "admin", "user", "svcacct", "edit", "{alias}", "{uniq}k1", "--name", "1bad",
        ],
    );
    check(
        &p,
        &["admin", "user", "svcacct", "disable", "{alias}", "{uniq}k1"],
    );
    check(
        &p,
        &["admin", "user", "svcacct", "info", "{alias}", "{uniq}k1"],
    );
    check(
        &p,
        &["admin", "user", "svcacct", "enable", "{alias}", "{uniq}k1"],
    );
    check_pair(
        &p,
        &["admin", "user", "svcacct", "remove", "{alias}", "{uniq}k1"],
        &["admin", "user", "svcacct", "rm", "{alias}", "{uniq}k2"],
    );
    check(
        &p,
        &["admin", "user", "svcacct", "remove", "{alias}", "{uniq}k1"],
    );
    check(
        &p,
        &["admin", "user", "sts", "info", "{alias}", "{uniq}nokey"],
    );
    check(
        &p,
        &[
            "admin",
            "user",
            "sts",
            "info",
            "{alias}",
            "{uniq}nokey",
            "--policy",
        ],
    );
    check_help_exit(&p, &["admin", "user", "svcacct", "add", "{alias}"]);
}

// ---------------------------------------------------------------------------
// admin group
// ---------------------------------------------------------------------------

#[test]
fn group_lifecycle() {
    let Some((_lock, mut p)) = fixture() else {
        return;
    };
    user(&p, "{uniq}u1");
    user(&p, "{uniq}u2");
    p.setup(&["admin", "policy", "create", "{alias}", "{uniq}p1", "p.json"]);
    check_pair(
        &p,
        &[
            "admin", "group", "add", "{alias}", "{uniq}g1", "{uniq}u1", "{uniq}u2",
        ],
        &["admin", "group", "add", "{alias}", "{uniq}g2", "{uniq}u1"],
    );
    check(
        &p,
        &[
            "admin",
            "group",
            "add",
            "{alias}",
            "{uniq}g3",
            "{uniq}nouser",
        ],
    );
    p.setup(&[
        "admin", "policy", "attach", "{alias}", "{uniq}p1", "--group", "{uniq}g1",
    ]);
    check(&p, &["admin", "group", "info", "{alias}", "{uniq}g1"]);
    check(&p, &["admin", "group", "info", "{alias}", "{uniq}nogroup"]);
    check(&p, &["admin", "group", "disable", "{alias}", "{uniq}g1"]);
    check(&p, &["admin", "group", "info", "{alias}", "{uniq}g1"]);
    check(&p, &["admin", "group", "enable", "{alias}", "{uniq}g1"]);
    check(
        &p,
        &["admin", "group", "enable", "{alias}", "{uniq}nogroup"],
    );
    check_pair(
        &p,
        &[
            "admin", "group", "remove", "{alias}", "{uniq}g1", "{uniq}u2",
        ],
        &["admin", "group", "rm", "{alias}", "{uniq}g2", "{uniq}u1"],
    );
    check(&p, &["admin", "group", "info", "{alias}", "{uniq}g2"]);
    check(
        &p,
        &["admin", "group", "remove", "{alias}", "{uniq}nogroup"],
    );
    check_help_exit(&p, &["admin", "group", "add", "{alias}", "{uniq}g1"]);
    p.unordered = true;
    check(&p, &["admin", "group", "list", "{alias}"]);
    p.unordered = false;
    check_pair(
        &p,
        &["admin", "group", "remove", "{alias}", "{uniq}g2"],
        &[
            "admin", "group", "remove", "{alias}", "{uniq}g1", "{uniq}u1",
        ],
    );
}

// ---------------------------------------------------------------------------
// admin policy
// ---------------------------------------------------------------------------

#[test]
fn policy_create_info_list_remove() {
    let Some((_lock, mut p)) = fixture() else {
        return;
    };
    check_pair(
        &p,
        &["admin", "policy", "create", "{alias}", "{uniq}p1", "p.json"],
        &[
            "admin", "policy", "create", "{alias}", "{uniq}p2", "p2.json",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "policy",
            "create",
            "{alias}",
            "{uniq}p3",
            "missing.json",
        ],
    );
    check(
        &p,
        &[
            "admin", "policy", "create", "{alias}", "{uniq}p3", "bad.json",
        ],
    );
    check(&p, &["admin", "policy", "info", "{alias}", "{uniq}p1"]);
    check(
        &p,
        &["admin", "policy", "info", "{alias}", "{uniq}nopolicy"],
    );
    check(
        &p,
        &[
            "admin", "policy", "info", "{alias}", "{uniq}p1", "-f", "out.json",
        ],
    );
    let written = |side: &common::parity::Side| {
        std::fs::read_to_string(side.path("out.json")).expect("policy file")
    };
    assert_eq!(written(&p.mc), written(&p.mx));
    check(
        &p,
        &[
            "admin",
            "policy",
            "info",
            "{alias}",
            "{uniq}p1",
            "--policy-file",
            "/nonexistent/x",
        ],
    );
    check(&p, &["admin", "policy", "info", "nosuchalias", "{uniq}p1"]);
    check(&p, &["admin", "policy", "add", "{alias}", "x", "y"]);
    check(&p, &["admin", "policy", "set", "{alias}", "x", "user=y"]);
    p.unordered = true;
    check(&p, &["admin", "policy", "list", "{alias}"]);
    p.unordered = false;
    check_pair(
        &p,
        &["admin", "policy", "remove", "{alias}", "{uniq}p1"],
        &["admin", "policy", "rm", "{alias}", "{uniq}p2"],
    );
    check(
        &p,
        &["admin", "policy", "remove", "{alias}", "{uniq}nopolicy"],
    );
}

#[test]
fn policy_attach_detach_entities() {
    let Some((_lock, p)) = fixture() else { return };
    user(&p, "{uniq}u1");
    p.setup(&["admin", "group", "add", "{alias}", "{uniq}g1", "{uniq}u1"]);
    p.setup(&["admin", "policy", "create", "{alias}", "{uniq}p1", "p.json"]);
    p.setup(&[
        "admin", "policy", "create", "{alias}", "{uniq}p2", "p2.json",
    ]);
    check_pair(
        &p,
        &[
            "admin", "policy", "attach", "{alias}", "{uniq}p1", "--user", "{uniq}u1",
        ],
        &[
            "admin", "policy", "attach", "{alias}", "{uniq}p2", "-u", "{uniq}u1",
        ],
    );
    // Already attached: mc reports success with the requested policies.
    check(
        &p,
        &[
            "admin", "policy", "attach", "{alias}", "{uniq}p1", "--user", "{uniq}u1",
        ],
    );
    check(
        &p,
        &[
            "admin", "policy", "attach", "{alias}", "{uniq}p1", "{uniq}p2", "--group", "{uniq}g1",
        ],
    );
    check(&p, &["admin", "policy", "attach", "{alias}", "{uniq}p1"]);
    check(
        &p,
        &[
            "admin", "policy", "attach", "{alias}", "{uniq}p1", "-u", "{uniq}u1", "-g", "{uniq}g1",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "policy",
            "attach",
            "{alias}",
            "{uniq}nopolicy",
            "--user",
            "{uniq}u1",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "policy",
            "attach",
            "{alias}",
            "{uniq}p1",
            "--user",
            "{uniq}nouser",
        ],
    );
    check(
        &p,
        &[
            "admin", "policy", "entities", "{alias}", "--user", "{uniq}u1",
        ],
    );
    check(
        &p,
        &[
            "admin", "policy", "entities", "{alias}", "-g", "{uniq}g1", "-p", "{uniq}p1",
        ],
    );
    check(
        &p,
        &[
            "admin", "policy", "entities", "{alias}", "--policy", "{uniq}p2", "--user", "{uniq}u1",
            "--group", "{uniq}g1",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "policy",
            "entities",
            "{alias}",
            "--user",
            "{uniq}nouser",
        ],
    );
    check_pair(
        &p,
        &[
            "admin", "policy", "detach", "{alias}", "{uniq}p1", "--user", "{uniq}u1",
        ],
        &[
            "admin", "policy", "detach", "{alias}", "{uniq}p2", "--user", "{uniq}u1",
        ],
    );
    check(
        &p,
        &[
            "admin", "policy", "detach", "{alias}", "{uniq}p1", "--user", "{uniq}u1",
        ],
    );
    check_pair(
        &p,
        &[
            "admin", "policy", "detach", "{alias}", "{uniq}p1", "--group", "{uniq}g1",
        ],
        &[
            "admin", "policy", "detach", "{alias}", "{uniq}p2", "-g", "{uniq}g1",
        ],
    );
    check_help_exit(&p, &["admin", "policy", "attach", "{alias}"]);
}

// ---------------------------------------------------------------------------
// admin accesskey
// ---------------------------------------------------------------------------

#[test]
fn accesskey_lifecycle() {
    let Some((_lock, mut p)) = fixture() else {
        return;
    };
    user(&p, "{uniq}u1");
    user(&p, "{uniq}u2");
    p.setup(&["admin", "policy", "create", "{alias}", "{uniq}p1", "p.json"]);
    p.setup(&[
        "admin", "policy", "attach", "{alias}", "{uniq}p1", "--user", "{uniq}u1",
    ]);
    let create = |key: &'static str| {
        [
            "admin",
            "accesskey",
            "create",
            "{alias}/",
            "{uniq}u1",
            "--access-key",
            key,
            "--secret-key",
            "secret12345",
        ]
    };
    check_pair(&p, &create("{uniq}k1"), &create("{uniq}k2"));
    let mut full = create("{uniq}k3").to_vec();
    full.extend([
        "--name",
        "n1",
        "--description",
        "d1",
        "--expiry-duration",
        "48h",
        "--policy",
        "p2.json",
    ]);
    let mut full_json = create("{uniq}k4").to_vec();
    full_json.extend([
        "--name",
        "n1",
        "--description",
        "d1",
        "--expiry-duration",
        "48h",
    ]);
    check_pair(&p, &full, &full_json);
    check(&p, &create("{uniq}k1"));
    for extra in [
        &["--expiry", "2030-01-01", "--expiry-duration", "1h"][..],
        &["--expiry", "soon"],
        &["--policy", "empty.json"],
        &["--policy", "bad.json"],
        &["--name", "1bad"],
    ] {
        let mut args = create("{uniq}k9").to_vec();
        args.extend_from_slice(extra);
        check(&p, &args);
    }
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "create",
            "{alias}/",
            "{uniq}nouser",
            "--access-key",
            "{uniq}k9",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "info",
            "{alias}/",
            "{uniq}k1",
            "{uniq}k3",
        ],
    );
    check(
        &p,
        &["admin", "accesskey", "info", "{alias}/", "{uniq}nokey"],
    );
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "edit",
            "{alias}/",
            "{uniq}k1",
            "--description",
            "d2",
            "--expiry-duration",
            "72h",
        ],
    );
    check(&p, &["admin", "accesskey", "info", "{alias}/", "{uniq}k1"]);
    check(&p, &["admin", "accesskey", "edit", "{alias}/", "{uniq}k1"]);
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "edit",
            "{alias}/",
            "{uniq}k1",
            "--expiry-duration",
            "bogus",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "edit",
            "{alias}/",
            "{uniq}nokey",
            "--name",
            "n3",
        ],
    );
    check(
        &p,
        &["admin", "accesskey", "disable", "{alias}/", "{uniq}k1"],
    );
    check(&p, &["admin", "accesskey", "info", "{alias}/", "{uniq}k1"]);
    check(
        &p,
        &["admin", "accesskey", "enable", "{alias}/", "{uniq}k1"],
    );
    // The server lists keys in varying order.
    check_any_order(
        &mut p,
        &["admin", "accesskey", "list", "{alias}/", "{uniq}u1"],
    );
    check_any_order(
        &mut p,
        &[
            "admin",
            "accesskey",
            "ls",
            "{alias}/",
            "{uniq}u1",
            "{uniq}u2",
            "--svcacc-only",
        ],
    );
    check_any_order(
        &mut p,
        &[
            "admin",
            "accesskey",
            "list",
            "{alias}/",
            "{uniq}u1",
            "--temp-only",
        ],
    );
    check(
        &p,
        &["admin", "accesskey", "list", "{alias}/", "--self", "--all"],
    );
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "list",
            "{alias}/",
            "--all",
            "{uniq}u1",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "list",
            "{alias}/",
            "--users-only",
            "--temp-only",
        ],
    );
    check(&p, &["admin", "accesskey", "list", "{alias}:cfg"]);
    check_pair(
        &p,
        &["admin", "accesskey", "remove", "{alias}/", "{uniq}k1"],
        &["admin", "accesskey", "rm", "{alias}/", "{uniq}k2"],
    );
    check(
        &p,
        &["admin", "accesskey", "remove", "{alias}/", "{uniq}k1"],
    );
    check_help_exit(&p, &["admin", "accesskey", "create"]);
    check_help_exit(&p, &["admin", "accesskey", "list"]);
    check_any_order(&mut p, &["admin", "accesskey", "list", "{alias}/"]);
    check_any_order(
        &mut p,
        &[
            "admin",
            "accesskey",
            "list",
            "{alias}/",
            "--all",
            "--svcacc-only",
        ],
    );
}

#[test]
fn accesskey_sts_revoke() {
    let Some((_lock, p)) = fixture() else { return };
    user(&p, "{uniq}u1");
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "sts-revoke",
            "{alias}/",
            "{uniq}u1",
            "--all",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "sts-revoke",
            "{alias}/",
            "{uniq}u1",
            "--token-type",
            "x",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "sts-revoke",
            "{alias}/",
            "--self",
            "--all",
        ],
    );
    check(
        &p,
        &["admin", "accesskey", "sts-revoke", "{alias}/", "{uniq}u1"],
    );
    check(
        &p,
        &["admin", "accesskey", "sts-revoke", "{alias}/", "--all"],
    );
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "sts-revoke",
            "{alias}/",
            "{uniq}u1",
            "--self",
            "--all",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "sts-revoke",
            "{alias}/",
            "{uniq}u1",
            "--all",
            "--token-type",
            "x",
        ],
    );
    check(
        &p,
        &[
            "admin",
            "accesskey",
            "sts-revoke",
            "{alias}/",
            "{uniq}nouser",
            "--all",
        ],
    );
    check_help_exit(
        &p,
        &["admin", "accesskey", "sts-revoke", "{alias}/", "a", "b"],
    );
}
