//! mc parity for the SERVER area: `admin info|config|kms|prometheus|scanner status|cluster|
//! service|update`, the hidden `admin tier|bucket|profile|subnet|health` and `update`.
//!
//!   MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity_server
//!
//! Commands that would disturb the shared servers (freeze) use the dedicated server from
//! tests/services/minio_server.sh (alias `srv`) and skip without it. Text-mode `service
//! restart` and `scanner status` are bubbletea UIs in mc; only their JSON output is compared.

mod common;

use common::parity::{Parity, Tool, json_docs};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn bare() -> Option<Parity> {
    Parity::bare()
}

fn empty() -> Option<Parity> {
    Parity::new()
}

fn with_setup(p: Option<Parity>, args: &[&str]) -> Option<Parity> {
    let p = p?;
    p.setup(args);
    Some(p)
}

/// Alias `dead` (closed port) for both sides.
fn dead(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.env.push((
        "MC_HOST_dead".to_string(),
        "http://minioadmin:minioadmin@127.0.0.1:1".to_string(),
    ));
    Some(p)
}

/// Alias `bad` (server 1 with unknown credentials) for both sides.
fn bad_credentials(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    let url = std::env::var("MX_TEST_URL").ok()?;
    p.env.push((
        "MC_HOST_bad".to_string(),
        url.replacen("://", "://nouser:nopassword1@", 1),
    ));
    Some(p)
}

/// Alias `srv`: the dedicated server (compared commands and setup/cleanup).
fn dedicated(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    let (Ok(url), Ok(access), Ok(secret)) = (
        std::env::var("MX_TEST_SERVER_URL"),
        std::env::var("MX_TEST_SERVER_ACCESS_KEY"),
        std::env::var("MX_TEST_SERVER_SECRET_KEY"),
    ) else {
        eprintln!("skipping parity test; dedicated server (MX_TEST_SERVER_URL) not running");
        return None;
    };
    p.env.push((
        "MC_HOST_srv".to_string(),
        url.replacen("://", &format!("://{access}:{secret}@"), 1),
    ));
    p.setup_once(&["alias", "set", "srv", &url, &access, &secret]);
    Some(p)
}

/// Server info changes between the two runs (uptime, memory, usage).
fn info(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.normalizer
        .rule(r"Uptime: [^\n]*", "Uptime: <UPTIME>")
        .rule(r"\d+\.\d% \(total: [^)]*\)", "<USAGE>")
        .rule(r"(?m)^\S+ \S+ Used, [^\n]*$", "<USED>")
        .rule(
            r#""(uptime|Alloc|TotalAlloc|Mallocs|Frees|HeapAlloc|num_gc|pause_total|usedspace|availspace|used_inodes|free_inodes|rawUsage|rawCapacity|usage|size|count|objectsCount|versionsCount|deleteMarkersCount)":\d+"#,
            r#""$1":0"#,
        )
        .rule(r#""(pause|pause_end)":\[[^\]]*\]"#, r#""$1":[]"#)
        .rule(r#""(totalspace)":\d+"#, r#""$1":0"#);
    Some(p)
}

/// Bearer tokens embed the expiry second.
fn jwt(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.normalizer.rule(
        r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
        "<JWT>",
    );
    Some(p)
}

/// Durations of `service restart`.
fn timed(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.normalizer.rule(
        r#""(restartDuration|waitingDuration|timeTaken)":\d+"#,
        r#""$1":0"#,
    );
    Some(p)
}

/// Scanner metrics: counters may move between the runs.
fn scanner(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.normalizer
        .rule(
            r#""cycle_complete_times":\s*\[[^\]]*\]"#,
            r#""cycle_complete_times":[]"#,
        )
        .rule(r#""life_time_ops":\s*\{[^}]*\}"#, r#""life_time_ops":{}"#)
        .rule(r#""(current_cycle|ongoing_buckets)":\s*\d+"#, r#""$1":0"#);
    Some(p)
}

/// `name`: one parity case (`text`: normalized bytes; `json`: JSON documents, then bytes).
macro_rules! case {
    ($(#[$meta:meta])* $name:ident, text, $fixture:expr, [$($arg:expr),* $(,)?] $(, $stdin:expr)?) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            let Some(p) = $fixture else { return };
            let stdin: Option<&[u8]> = None $(.or(Some($stdin)))?;
            p.assert_parity(&[$($arg),*], stdin);
        }
    };
    ($(#[$meta:meta])* $name:ident, json, $fixture:expr, [$($arg:expr),* $(,)?] $(, $stdin:expr)?) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            let Some(p) = $fixture else { return };
            let stdin: Option<&[u8]> = None $(.or(Some($stdin)))?;
            p.assert_json_parity(&["--json", $($arg),*], stdin);
        }
    };
}

// ---------------------------------------------------------------------------
// admin info
// ---------------------------------------------------------------------------

case!(info_text, text, info(bare()), ["admin", "info", "{alias}"]);
case!(info_json, json, info(bare()), ["admin", "info", "{alias}"]);
case!(
    info_offline_text,
    text,
    info(bare()),
    ["admin", "info", "{alias}", "--offline"]
);
case!(
    info_unknown_alias_text,
    text,
    bare(),
    ["admin", "info", "nosuch"]
);
case!(
    info_unknown_alias_json,
    json,
    bare(),
    ["admin", "info", "nosuch"]
);
case!(
    info_unreachable_text,
    text,
    dead(bare()),
    ["admin", "info", "dead"]
);
case!(
    info_unreachable_json,
    json,
    dead(bare()),
    ["admin", "info", "dead"]
);
case!(
    info_bad_credentials_json,
    json,
    bad_credentials(bare()),
    ["admin", "info", "bad"]
);

// ---------------------------------------------------------------------------
// admin config
// ---------------------------------------------------------------------------

fn config_set() -> Option<Parity> {
    with_setup(
        bare(),
        &[
            "admin",
            "config",
            "set",
            "{alias}",
            "scanner",
            "speed=default",
        ],
    )
}

case!(
    config_get_text,
    text,
    bare(),
    ["admin", "config", "get", "{alias}", "scanner"]
);
case!(
    config_get_json,
    json,
    bare(),
    ["admin", "config", "get", "{alias}", "scanner"]
);
case!(
    config_get_env_override_json,
    json,
    bare(),
    ["admin", "config", "get", "{alias}", "notify_webhook"]
);
case!(
    config_get_keys_text,
    text,
    bare(),
    ["admin", "config", "get", "{alias}"]
);
case!(
    config_get_keys_json,
    json,
    bare(),
    ["admin", "config", "get", "{alias}"]
);
case!(
    config_get_unknown_text,
    text,
    bare(),
    ["admin", "config", "get", "{alias}", "bogus"]
);
case!(
    config_get_unknown_json,
    json,
    bare(),
    ["admin", "config", "get", "{alias}", "bogus"]
);
case!(
    config_set_help_text,
    text,
    bare(),
    ["admin", "config", "set", "{alias}", "notify_webhook"]
);
case!(
    config_set_help_json,
    json,
    bare(),
    ["admin", "config", "set", "{alias}", "scanner"]
);
case!(
    config_set_help_key_text,
    text,
    bare(),
    ["admin", "config", "set", "{alias}", "scanner", "speed"]
);
case!(
    config_set_help_env_text,
    text,
    bare(),
    ["admin", "config", "set", "{alias}", "--env", "scanner"]
);
case!(
    config_set_text,
    text,
    bare(),
    [
        "admin",
        "config",
        "set",
        "{alias}",
        "scanner",
        "speed=default"
    ]
);
case!(
    config_set_json,
    json,
    bare(),
    [
        "admin",
        "config",
        "set",
        "{alias}",
        "scanner",
        "speed=default"
    ]
);
case!(
    config_set_unknown_text,
    text,
    bare(),
    ["admin", "config", "set", "{alias}", "bogus", "x=y"]
);
case!(
    config_set_unknown_json,
    json,
    bare(),
    ["admin", "config", "set", "{alias}", "bogus", "x=y"]
);
case!(
    config_set_bad_key_text,
    text,
    bare(),
    ["admin", "config", "set", "{alias}", "scanner", "bogus=1"]
);
case!(
    config_reset_text,
    text,
    config_set(),
    ["admin", "config", "reset", "{alias}", "scanner", "speed"]
);
case!(
    config_reset_json,
    json,
    config_set(),
    ["admin", "config", "reset", "{alias}", "scanner", "speed"]
);
case!(
    config_reset_help_text,
    text,
    bare(),
    ["admin", "config", "reset", "{alias}"]
);
case!(
    config_reset_value_text,
    text,
    bare(),
    ["admin", "config", "reset", "{alias}", "scanner", "speed=x"]
);
case!(
    config_reset_value_json,
    json,
    bare(),
    ["admin", "config", "reset", "{alias}", "scanner", "speed=x"]
);
case!(
    config_history_text,
    text,
    config_set(),
    ["admin", "config", "history", "{alias}", "-n", "2"]
);
case!(
    config_history_json,
    json,
    config_set(),
    ["admin", "config", "history", "{alias}", "-n", "2"]
);
case!(
    config_history_clear_text,
    text,
    bare(),
    ["admin", "config", "history", "{alias}", "--clear"]
);
case!(
    config_history_clear_json,
    json,
    bare(),
    ["admin", "config", "history", "{alias}", "--clear"]
);
case!(
    config_restore_missing_text,
    text,
    bare(),
    ["admin", "config", "restore", "{alias}", "nonexistent"]
);
case!(
    config_restore_missing_json,
    json,
    bare(),
    ["admin", "config", "restore", "{alias}", "nonexistent"]
);
case!(
    config_export_text,
    text,
    bare(),
    ["admin", "config", "export", "{alias}"]
);
case!(
    config_export_json,
    json,
    bare(),
    ["admin", "config", "export", "{alias}"]
);
case!(
    config_import_invalid_text,
    text,
    bare(),
    ["admin", "config", "import", "{alias}"],
    b"bogus x=1\n"
);
case!(
    config_import_invalid_json,
    json,
    bare(),
    ["admin", "config", "import", "{alias}"],
    b"bogus x=1\n"
);

/// Imports the server's own export (a partial config would reset the rest).
#[test]
fn config_import_text_and_json() {
    let Some(p) = bare() else { return };
    let (export, _) = p.run(&["admin", "config", "export", "{alias}"], None);
    let config = export.stdout.into_bytes();
    p.assert_parity(&["admin", "config", "import", "{alias}"], Some(&config));
    p.assert_json_parity(
        &["--json", "admin", "config", "import", "{alias}"],
        Some(&config),
    );
}

// ---------------------------------------------------------------------------
// admin kms key
// ---------------------------------------------------------------------------

case!(
    kms_list_text,
    text,
    bare(),
    ["admin", "kms", "key", "list", "{alias}"]
);
case!(
    kms_list_json,
    json,
    bare(),
    ["admin", "kms", "key", "list", "{alias}"]
);
case!(
    kms_status_text,
    text,
    bare(),
    ["admin", "kms", "key", "status", "{alias}"]
);
case!(
    kms_status_json,
    json,
    bare(),
    ["admin", "kms", "key", "status", "{alias}"]
);
case!(
    kms_status_missing_text,
    text,
    bare(),
    ["admin", "kms", "key", "status", "{alias}", "nokey"]
);
case!(
    kms_status_missing_json,
    json,
    bare(),
    ["admin", "kms", "key", "status", "{alias}", "nokey"]
);
// The static-key KMS of the test server cannot create keys; mc and mx report the error.
case!(
    kms_create_text,
    text,
    bare(),
    ["admin", "kms", "key", "create", "{alias}", "{uniq}k"]
);
case!(
    kms_create_json,
    json,
    bare(),
    ["admin", "kms", "key", "create", "{alias}", "{uniq}k"]
);
case!(
    kms_not_configured_text,
    text,
    second_server(),
    ["admin", "kms", "key", "list", "{alias2}"]
);

fn second_server() -> Option<Parity> {
    let mut p = bare()?;
    p.second_server().then_some(p)
}

// ---------------------------------------------------------------------------
// admin prometheus
// ---------------------------------------------------------------------------

case!(
    prometheus_generate_text,
    text,
    jwt(bare()),
    ["admin", "prometheus", "generate", "{alias}"]
);
case!(
    prometheus_generate_json,
    json,
    jwt(bare()),
    ["admin", "prometheus", "generate", "{alias}"]
);
case!(
    prometheus_generate_node_public_text,
    text,
    bare(),
    [
        "admin",
        "prometheus",
        "generate",
        "{alias}",
        "node",
        "--public"
    ]
);
case!(
    prometheus_generate_v3_text,
    text,
    jwt(bare()),
    [
        "admin",
        "prometheus",
        "generate",
        "{alias}/",
        "api",
        "--api-version",
        "v3",
        "--bucket",
        "b1"
    ]
);
case!(
    prometheus_generate_v3_json,
    json,
    bare(),
    [
        "admin",
        "prometheus",
        "generate",
        "{alias}",
        "--api-version",
        "v3",
        "--public"
    ]
);
case!(
    prometheus_generate_bad_type_text,
    text,
    bare(),
    ["admin", "prometheus", "generate", "{alias}", "bogus"]
);
case!(
    prometheus_generate_bad_type_json,
    json,
    bare(),
    ["admin", "prometheus", "generate", "{alias}", "bogus"]
);
case!(
    prometheus_generate_v2_bucket_text,
    text,
    bare(),
    [
        "admin",
        "prometheus",
        "generate",
        "{alias}",
        "--bucket",
        "b"
    ]
);
case!(
    prometheus_generate_bad_version_text,
    text,
    bare(),
    [
        "admin",
        "prometheus",
        "generate",
        "{alias}",
        "--api-version",
        "v9"
    ]
);
case!(
    prometheus_generate_v3_bucket_type_text,
    text,
    bare(),
    [
        "admin",
        "prometheus",
        "generate",
        "{alias}",
        "system",
        "--api-version",
        "v3",
        "--bucket",
        "b"
    ]
);
case!(
    prometheus_generate_bad_alias_text,
    text,
    bare(),
    ["admin", "prometheus", "generate", "{alias}/bucket"]
);
case!(
    prometheus_generate_unknown_alias_json,
    json,
    bare(),
    ["admin", "prometheus", "generate", "nosuch"]
);
case!(
    prometheus_metrics_forbidden_text,
    text,
    bad_credentials(bare()),
    ["admin", "prometheus", "metrics", "bad"]
);
case!(
    prometheus_metrics_forbidden_json,
    json,
    bad_credentials(bare()),
    ["admin", "prometheus", "metrics", "bad"]
);
case!(
    prometheus_metrics_empty_bucket_text,
    text,
    bare(),
    [
        "admin",
        "prometheus",
        "metrics",
        "{alias}",
        "api",
        "--api-version",
        "v3",
        "--bucket",
        "{uniq}"
    ]
);

/// Metric values move between the runs and mc's family order is random (Go map):
/// compare the families' names, help, types and label sets.
#[test]
fn prometheus_metrics_json_families() {
    let Some(p) = bare() else { return };
    for subsystem in ["cluster", "node", "resource"] {
        let (mc, mx) = p.run(
            &[
                "--json",
                "admin",
                "prometheus",
                "metrics",
                "{alias}",
                subsystem,
            ],
            None,
        );
        assert_eq!(mc.code, mx.code, "{subsystem}");
        let shape = |text: &str| -> Vec<String> {
            let doc: Value = serde_json::from_str(text).expect("json array");
            let mut out: Vec<String> = doc
                .as_array()
                .unwrap()
                .iter()
                .map(|family| {
                    let mut labels: Vec<String> = family["metrics"]
                        .as_array()
                        .map(|m| {
                            m.iter()
                                .map(|metric| {
                                    let keys: Vec<&String> =
                                        metric.as_object().unwrap().keys().collect();
                                    format!("{}{keys:?}", metric["labels"])
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    labels.sort();
                    format!(
                        "{} {} {} {labels:?}",
                        family["name"], family["type"], family["help"]
                    )
                })
                .collect();
            out.sort();
            out
        };
        let (a, b) = (shape(&mc.stdout), shape(&mx.stdout));
        let only_mc: Vec<&String> = a.iter().filter(|x| !b.contains(x)).collect();
        let only_mx: Vec<&String> = b.iter().filter(|x| !a.contains(x)).collect();
        // Counters can appear once the server served its first request of a kind.
        assert!(
            only_mc.len() <= 2 && only_mx.len() <= 2,
            "{subsystem}: only mc {only_mc:#?}\nonly mx {only_mx:#?}"
        );
    }
}

// ---------------------------------------------------------------------------
// admin scanner status
// ---------------------------------------------------------------------------

case!(
    scanner_status_json,
    json,
    scanner(bare()),
    ["admin", "scanner", "status", "{alias}", "-n", "1"]
);

/// `--json` after the command: mc compacts the encoder output only then.
#[test]
fn scanner_status_trailing_json() {
    let Some(p) = scanner(bare()) else { return };
    p.assert_parity(
        &["admin", "scanner", "status", "{alias}", "-n", "1", "--json"],
        None,
    );
}

case!(
    scanner_bucket_text,
    text,
    empty(),
    [
        "admin", "scanner", "status", "{alias}", "--bucket", "{bucket}"
    ]
);
case!(
    scanner_bucket_json,
    json,
    empty(),
    [
        "admin", "scanner", "status", "{alias}", "--bucket", "{bucket}"
    ]
);
case!(
    scanner_unknown_alias_text,
    text,
    bare(),
    ["admin", "scanner", "status", "nosuch"]
);

// ---------------------------------------------------------------------------
// admin service / update
// ---------------------------------------------------------------------------

case!(
    service_restart_dry_run_json,
    json,
    timed(bare()),
    ["admin", "service", "restart", "{alias}", "--dry-run"]
);
case!(
    service_restart_unknown_alias_text,
    text,
    bare(),
    ["admin", "service", "restart", "nosuch"]
);
// Transport errors: the text matches; mc's JSON also carries Go's `*url.Error` value, which
// the shared admin client does not model yet.
case!(
    service_restart_unreachable_text,
    text,
    dead(bare()),
    ["admin", "service", "restart", "dead", "--dry-run"]
);
case!(
    #[ignore = "parity: transport error detail (*url.Error) not modeled"]
    service_restart_unreachable_json,
    json,
    dead(bare()),
    ["admin", "service", "restart", "dead", "--dry-run"]
);
case!(
    service_unfreeze_text,
    text,
    dedicated(bare()),
    ["admin", "service", "unfreeze", "srv"]
);
case!(
    service_unfreeze_json,
    json,
    dedicated(bare()),
    ["admin", "service", "unfreeze", "srv"]
);
case!(
    service_unfreeze_unreachable_text,
    text,
    dead(bare()),
    ["admin", "service", "unfreeze", "dead"]
);

/// Freeze, then unfreeze (also on drop) the dedicated server.
#[test]
fn service_freeze_text_and_json() {
    let Some(mut p) = dedicated(bare()) else {
        return;
    };
    // MinIO counts freezes: every freeze needs its unfreeze (4 here).
    p.cleanup_on_drop(&["admin", "service", "unfreeze", "srv"]);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        p.assert_parity(&["admin", "service", "freeze", "srv"], None);
        p.assert_json_parity(&["--json", "admin", "service", "freeze", "srv"], None);
    }));
    p.setup(&["admin", "service", "unfreeze", "srv"]);
    p.setup(&["admin", "service", "unfreeze", "srv"]);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

case!(
    update_error_text,
    text,
    bare(),
    [
        "admin",
        "update",
        "{alias}",
        "http://127.0.0.1:9000/minio.sha256sum",
        "-y"
    ]
);
case!(
    update_error_json,
    json,
    bare(),
    [
        "admin",
        "update",
        "{alias}",
        "http://127.0.0.1:9000/minio.sha256sum"
    ]
);
case!(
    update_unknown_alias_text,
    text,
    bare(),
    ["admin", "update", "nosuch"]
);

// ---------------------------------------------------------------------------
// admin cluster
// ---------------------------------------------------------------------------

fn exported() -> Option<Parity> {
    with_setup(
        empty(),
        &["admin", "cluster", "bucket", "export", "{target}"],
    )
}

case!(
    cluster_bucket_export_text,
    text,
    empty(),
    ["admin", "cluster", "bucket", "export", "{target}"]
);
case!(
    cluster_bucket_export_json,
    json,
    empty(),
    ["admin", "cluster", "bucket", "export", "{target}"]
);
case!(
    cluster_bucket_import_text,
    text,
    exported(),
    [
        "admin",
        "cluster",
        "bucket",
        "import",
        "{target}",
        "{alias}/{bucket}-{bucket}-metadata.zip"
    ]
);
case!(
    cluster_bucket_import_json,
    json,
    exported(),
    [
        "admin",
        "cluster",
        "bucket",
        "import",
        "{target}",
        "{alias}/{bucket}-{bucket}-metadata.zip"
    ]
);

/// A trailing `--json` compacts the report.
#[test]
fn cluster_bucket_import_trailing_json() {
    let Some(p) = exported() else { return };
    p.assert_parity(
        &[
            "admin",
            "cluster",
            "bucket",
            "import",
            "{target}",
            "{alias}/{bucket}-{bucket}-metadata.zip",
            "--json",
        ],
        None,
    );
}

case!(
    cluster_bucket_import_missing_text,
    text,
    bare(),
    [
        "admin",
        "cluster",
        "bucket",
        "import",
        "{alias}",
        "missing.zip"
    ]
);
case!(
    cluster_bucket_import_missing_json,
    json,
    bare(),
    [
        "admin",
        "cluster",
        "bucket",
        "import",
        "{alias}",
        "missing.zip"
    ]
);

fn not_a_zip() -> Option<Parity> {
    let p = bare()?;
    p.file("bad.zip", "not a zip archive");
    Some(p)
}

case!(
    cluster_iam_import_not_zip_text,
    text,
    not_a_zip(),
    ["admin", "cluster", "iam", "import", "{alias}", "bad.zip"]
);
case!(
    cluster_iam_import_not_zip_json,
    json,
    not_a_zip(),
    ["admin", "cluster", "iam", "import", "{alias}", "bad.zip"]
);
case!(
    cluster_iam_export_text,
    text,
    bare(),
    ["admin", "cluster", "iam", "export", "{alias}"]
);
case!(
    cluster_iam_export_json,
    json,
    bare(),
    [
        "admin", "cluster", "iam", "export", "{alias}", "-o", "out.zip"
    ]
);
case!(
    cluster_iam_export_unknown_alias_text,
    text,
    bare(),
    ["admin", "cluster", "iam", "export", "nosuch"]
);

/// IAM import on the dedicated server; the server lists entities in random (Go map) order,
/// so lists are compared sorted.
#[test]
fn cluster_iam_import_sorted() {
    let Some(p) = dedicated(bare()) else { return };
    p.setup(&["admin", "cluster", "iam", "export", "srv", "-o", "iam.zip"]);
    let (mc, mx) = p.run(
        &[
            "--json", "admin", "cluster", "iam", "import", "srv", "iam.zip",
        ],
        None,
    );
    assert_eq!(mc.code, mx.code, "mc: {mc:?}\nmx: {mx:?}");
    fn sorted(value: &mut Value) {
        match value {
            Value::Array(items) => {
                items.iter_mut().for_each(sorted);
                items.sort_by_key(|v| v.to_string());
            }
            Value::Object(map) => map.values_mut().for_each(sorted),
            _ => {}
        }
    }
    let docs = |tool: Tool, text: &str| -> Vec<Value> {
        let mut docs = json_docs(&p.normalize(tool, text)).expect("json");
        docs.iter_mut().for_each(sorted);
        docs
    };
    assert_eq!(docs(Tool::Mc, &mc.stdout), docs(Tool::Mx, &mx.stdout));
    let (mc, mx) = p.run(
        &["admin", "cluster", "iam", "import", "srv", "iam.zip"],
        None,
    );
    // `Added policies: a, b, c`: compare the names sorted.
    let lines = |text: &str| -> Vec<(String, Vec<String>)> {
        text.lines()
            .map(|line| {
                let (head, names) = line.split_once(": ").unwrap_or((line, ""));
                let mut names: Vec<String> = names.split(", ").map(str::to_string).collect();
                names.sort();
                (head.to_string(), names)
            })
            .collect()
    };
    assert_eq!(lines(&mc.stdout), lines(&mx.stdout));
    assert_eq!(mc.stderr, mx.stderr);
}

// ---------------------------------------------------------------------------
// hidden, deprecated commands
// ---------------------------------------------------------------------------

case!(tier_text, text, bare(), ["admin", "tier"]);
case!(
    tier_ls_text,
    text,
    bare(),
    ["admin", "tier", "ls", "{alias}"]
);
// mc prints its "no tiers" note as text even with --json.
case!(
    tier_ls_json_flag_text,
    text,
    bare(),
    ["--json", "admin", "tier", "ls", "{alias}"]
);
case!(
    tier_info_text,
    text,
    bare(),
    ["admin", "tier", "info", "{alias}"]
);
case!(
    tier_add_text,
    text,
    bare(),
    ["admin", "tier", "add", "minio", "{alias}", "X"]
);
case!(
    tier_verify_text,
    text,
    bare(),
    ["admin", "tier", "verify", "{alias}", "NOTIER"]
);
case!(bucket_help_text, text, bare(), ["admin", "bucket", "bogus"]);
case!(
    bucket_remote_add_text,
    text,
    bare(),
    ["admin", "bucket", "remote", "add", "{alias}/b"]
);
case!(
    bucket_remote_rm_json,
    json,
    bare(),
    ["admin", "bucket", "remote", "rm", "{alias}/b"]
);
case!(
    bucket_quota_text,
    text,
    bare(),
    ["admin", "bucket", "quota", "{alias}/b", "--hard", "1GB"]
);
case!(
    bucket_info_text,
    text,
    bare(),
    ["admin", "bucket", "info", "{alias}/b"]
);
case!(profile_text, text, bare(), ["admin", "profile"]);
case!(
    profile_start_json,
    json,
    bare(),
    ["admin", "profile", "start", "{alias}"]
);
case!(subnet_text, text, bare(), ["admin", "subnet"]);
case!(
    subnet_health_text,
    text,
    bare(),
    ["admin", "subnet", "health", "{alias}"]
);
case!(
    subnet_register_text,
    text,
    bare(),
    ["admin", "subnet", "register", "{alias}"]
);
case!(health_text, text, bare(), ["admin", "health", "{alias}"]);
case!(health_json, json, bare(), ["admin", "health"]);

// mx does not replace its own binary; mc downloads a release (or fails like here).
case!(
    #[ignore = "parity: mx does not self-update"]
    update_self_text,
    text,
    bare(),
    ["update"]
);
