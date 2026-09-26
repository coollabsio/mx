//! Live tests for the shared output layer: `MC_HOST_<alias>` against a real server and
//! JSON lines output. Needs MX_LIVE_TESTS=1 and MX_TEST_URL/ACCESS_KEY/SECRET_KEY.

mod common;

use common::live::Live;

/// `MC_HOST_<alias>` value for the test server (`scheme://KEY:SECRET@host:port`).
fn mc_host_value() -> Option<String> {
    let url = std::env::var("MX_TEST_URL").ok()?;
    let access = std::env::var("MX_TEST_ACCESS_KEY").ok()?;
    let secret = std::env::var("MX_TEST_SECRET_KEY").ok()?;
    let (scheme, host) = url.split_once("://")?;
    Some(format!(
        "{scheme}://{access}:{secret}@{}",
        host.trim_end_matches('/')
    ))
}

#[test]
fn live_mc_host_alias_works_for_commands() {
    let Some(live) = Live::new() else { return };
    let Some(value) = mc_host_value() else {
        eprintln!("skipping: MX_TEST_URL/MX_TEST_ACCESS_KEY/MX_TEST_SECRET_KEY not set");
        return;
    };
    let file = live.local_file("env.txt", "from env alias");
    live.cmd()
        .args(["cp", file.to_str().unwrap(), &live.url("env.txt")])
        .assert()
        .success();

    // `envhost` exists only in the environment.
    let target = format!("envhost/{}/", live.bucket);
    let output = live
        .cmd()
        .env("MC_HOST_envhost", &value)
        .args(["--json", "ls", &target])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "{text}");
    let doc: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(doc["status"], "success");
    assert!(lines[0].contains("env.txt"), "{text}");

    live.cmd()
        .env("MC_HOST_envhost", &value)
        .args(["cat", &format!("envhost/{}/env.txt", live.bucket)])
        .assert()
        .success()
        .stdout("from env alias");

    live.cmd()
        .env("MC_HOST_envhost", &value)
        .args(["alias", "list", "envhost"])
        .assert()
        .success()
        .stdout(predicates::str::contains("env"));

    // Wrong credentials in MC_HOST win over nothing and fail with an mc error line.
    let bad = value.replacen("://", "://wrong", 1);
    live.cmd()
        .env("MC_HOST_envhost", &bad)
        .args(["ls", &target])
        .assert()
        .code(1)
        .stderr(predicates::str::starts_with("mx: <ERROR> "));
}
