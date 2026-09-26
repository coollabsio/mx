//! Self-tests of the parity harness (`tests/common/parity.rs`): `{uniq}` IAM names and
//! [`Parity::iam_cleanup`]. Runs with `MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity_harness`.

mod common;

use common::parity::Parity;
use serde_json::Value;

const POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::*"]}]}"#;

fn names(p: &Parity, args: &[&str], field: &str) -> Vec<String> {
    let mut out = Vec::new();
    for doc in p.mc_json(args) {
        match doc.get(field) {
            Some(Value::String(name)) => out.push(name.clone()),
            Some(Value::Array(items)) => {
                out.extend(items.iter().filter_map(Value::as_str).map(str::to_string))
            }
            _ => {}
        }
    }
    out
}

#[test]
fn uniq_iam_entities_are_normalized_and_cleaned_up() {
    let Some(mut p) = Parity::bare() else { return };
    p.iam_cleanup();
    let alias = common::live::alias_name();
    p.file("policy.json", POLICY);
    p.setup(&["admin", "user", "add", "{alias}", "{uniq}u1", "secret12345"]);
    p.setup(&["admin", "group", "add", "{alias}", "{uniq}g1", "{uniq}u1"]);
    p.setup(&[
        "admin",
        "policy",
        "create",
        "{alias}",
        "{uniq}p1",
        "policy.json",
    ]);
    p.setup(&[
        "admin", "policy", "attach", "{alias}", "{uniq}p1", "--group", "{uniq}g1",
    ]);
    p.setup(&[
        "admin",
        "accesskey",
        "create",
        "{alias}/",
        "--access-key",
        "{uniq}k1",
        "--secret-key",
        "secret12345",
    ]);
    let (mc, mx) = (p.mc.uniq.clone(), p.mx.uniq.clone());
    assert_eq!(mc.len(), mx.len());
    assert_eq!(
        p.normalize(common::parity::Tool::Mc, &format!("user {mc}u1")),
        "user <UNIQ>u1"
    );
    let users = names(
        &p,
        &["--json", "admin", "user", "list", &alias],
        "accessKey",
    );
    assert!(users.contains(&format!("{mc}u1")) && users.contains(&format!("{mx}u1")));

    // A second fixture inspects the server after the first one is dropped.
    let check = Parity::bare().expect("parity enabled");
    drop(p);
    let users = names(
        &check,
        &["--json", "admin", "user", "list", &alias],
        "accessKey",
    );
    let groups = names(
        &check,
        &["--json", "admin", "group", "list", &alias],
        "groups",
    );
    let policies = names(
        &check,
        &["--json", "admin", "policy", "list", &alias],
        "policy",
    );
    let keys: Vec<String> = check
        .mc_json(&[
            "--json",
            "admin",
            "accesskey",
            "list",
            &format!("{alias}/"),
            "--all",
        ])
        .iter()
        .flat_map(|doc| {
            doc.get("svcaccs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|key| {
            key.get("accessKey")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    for name in users.iter().chain(&groups).chain(&policies).chain(&keys) {
        assert!(
            !name.starts_with(&mc) && !name.starts_with(&mx),
            "{name} left behind"
        );
    }
}
