//! Live tests for `admin user|group|policy|accesskey` against MinIO (MX_LIVE_TESTS=1): IAM
//! changes made with mx take effect (new credentials work, policies grant access, disabled
//! accounts are refused).
//!
//!   sh tests/live_minio.sh live_iam

mod common;

use common::live::Live;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// Unique IAM names for one test (`mxi<digits><suffix>`), removed on drop.
struct Iam<'a> {
    live: &'a Live,
    prefix: String,
}

impl<'a> Iam<'a> {
    fn new(live: &'a Live) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            live,
            prefix: format!("mxi{:010}", nanos % 10_000_000_000),
        }
    }

    fn name(&self, suffix: &str) -> String {
        format!("{}{suffix}", self.prefix)
    }

    fn mx(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        self.live.cmd().args(args).assert()
    }

    fn json(&self, args: &[&str]) -> Vec<Value> {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.mx(&all).success().get_output().stdout.clone();
        serde_json::Deserializer::from_slice(&out)
            .into_iter::<Value>()
            .map(|doc| doc.expect("JSON document"))
            .collect()
    }

    /// Policy granting `actions` on the fixture bucket.
    fn policy(&self, suffix: &str, actions: &[&str]) -> String {
        let name = self.name(suffix);
        let document = serde_json::json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": actions,
                "Resource": [
                    format!("arn:aws:s3:::{}", self.live.bucket),
                    format!("arn:aws:s3:::{}/*", self.live.bucket),
                ],
            }],
        });
        let path = self
            .live
            .local_file(&format!("{name}.json"), &document.to_string());
        self.mx(&[
            "admin",
            "policy",
            "create",
            &self.live.alias,
            &name,
            path.to_str().unwrap(),
        ])
        .success();
        name
    }

    /// Configures an alias `name` with the given credentials.
    fn alias(&self, name: &str, access_key: &str, secret_key: &str) {
        let url = std::env::var("MX_TEST_URL").expect("MX_TEST_URL");
        self.mx(&[
            "alias", "set", name, &url, access_key, secret_key, "--api", "S3v4",
        ])
        .success();
    }

    /// True when `alias` can read the fixture object.
    fn can_read(&self, alias: &str) -> bool {
        self.live
            .cmd()
            .args(["cat", &format!("{alias}/{}/obj.txt", self.live.bucket)])
            .output()
            .expect("run mx")
            .status
            .success()
    }
}

impl Drop for Iam<'_> {
    fn drop(&mut self) {
        let alias = self.live.alias.clone();
        let ours = |name: &str| name.starts_with(&self.prefix);
        let run = |args: &[&str]| {
            let _ = self.live.cmd().args(args).output();
        };
        let json = |args: &[&str]| -> Vec<Value> {
            let mut all = vec!["--json"];
            all.extend_from_slice(args);
            let out = self.live.cmd().args(&all).output().expect("run mx").stdout;
            serde_json::Deserializer::from_slice(&out)
                .into_iter::<Value>()
                .filter_map(Result::ok)
                .collect()
        };
        for doc in json(&["admin", "user", "list", &alias]) {
            if let Some(user) = doc["accessKey"].as_str().filter(|u| ours(u)) {
                run(&["admin", "user", "remove", &alias, user]);
            }
        }
        for doc in json(&["admin", "group", "list", &alias]) {
            for group in doc["groups"].as_array().into_iter().flatten() {
                if let Some(group) = group.as_str().filter(|g| ours(g)) {
                    run(&["admin", "group", "remove", &alias, group]);
                }
            }
        }
        for doc in json(&["admin", "policy", "list", &alias]) {
            if let Some(policy) = doc["policy"].as_str().filter(|p| ours(p)) {
                run(&["admin", "policy", "remove", &alias, policy]);
            }
        }
    }
}

#[test]
fn live_user_policy_grants_access() {
    let Some(live) = Live::new() else { return };
    let iam = Iam::new(&live);
    let obj = live.local_file("obj.txt", "hello\n");
    iam.mx(&["cp", obj.to_str().unwrap(), &live.url("obj.txt")])
        .success();
    let user = iam.name("u1");
    iam.mx(&["admin", "user", "add", &live.alias, &user, "secret12345"])
        .success()
        .stdout(format!("Added user `{user}` successfully.\n"));
    iam.alias("iamuser", &user, "secret12345");
    assert!(!iam.can_read("iamuser"), "no policy attached yet");

    let policy = iam.policy("p1", &["s3:GetObject"]);
    iam.mx(&[
        "admin",
        "policy",
        "attach",
        &live.alias,
        &policy,
        "--user",
        &user,
    ])
    .success()
    .stdout(format!("Attached Policies: [{policy}]\nTo User: {user}\n"));
    assert!(iam.can_read("iamuser"));

    let info = iam.json(&["admin", "user", "info", &live.alias, &user]);
    assert_eq!(info[0]["policyName"], policy.as_str());
    assert_eq!(info[0]["userStatus"], "enabled");
    let merged = iam
        .mx(&["admin", "user", "policy", &live.alias, &user])
        .success();
    let merged: Value = serde_json::from_slice(&merged.get_output().stdout).unwrap();
    assert_eq!(merged["Statement"][0]["Action"][0], "s3:GetObject");

    iam.mx(&["admin", "user", "disable", &live.alias, &user])
        .success();
    assert!(!iam.can_read("iamuser"), "disabled user");
    iam.mx(&["admin", "user", "enable", &live.alias, &user])
        .success();
    assert!(iam.can_read("iamuser"));

    iam.mx(&[
        "admin",
        "policy",
        "detach",
        &live.alias,
        &policy,
        "--user",
        &user,
    ])
    .success();
    assert!(!iam.can_read("iamuser"), "policy detached");
    iam.mx(&["admin", "user", "remove", &live.alias, &user])
        .success();
    iam.mx(&["admin", "user", "info", &live.alias, &user])
        .failure()
        .stderr(predicates::str::contains("Unable to get user info"));
}

#[test]
fn live_group_policy_applies_to_members() {
    let Some(live) = Live::new() else { return };
    let iam = Iam::new(&live);
    let obj = live.local_file("obj.txt", "hello\n");
    iam.mx(&["cp", obj.to_str().unwrap(), &live.url("obj.txt")])
        .success();
    let user = iam.name("u1");
    let group = iam.name("g1");
    iam.mx(&["admin", "user", "add", &live.alias, &user, "secret12345"])
        .success();
    iam.mx(&["admin", "group", "add", &live.alias, &group, &user])
        .success();
    let policy = iam.policy("p1", &["s3:GetObject"]);
    iam.mx(&[
        "admin",
        "policy",
        "attach",
        &live.alias,
        &policy,
        "--group",
        &group,
    ])
    .success();
    iam.alias("iamgroupuser", &user, "secret12345");
    assert!(iam.can_read("iamgroupuser"));

    let info = iam.json(&["admin", "group", "info", &live.alias, &group]);
    assert_eq!(info[0]["groupPolicy"], policy.as_str());
    assert_eq!(info[0]["members"][0], user.as_str());
    let entities = iam.json(&[
        "admin",
        "policy",
        "entities",
        &live.alias,
        "--group",
        &group,
    ]);
    assert_eq!(
        entities[0]["result"]["groupMappings"][0]["policies"][0],
        policy.as_str()
    );
    let groups = iam.json(&["admin", "group", "list", &live.alias]);
    assert!(
        groups[0]["groups"]
            .as_array()
            .unwrap()
            .contains(&Value::from(group.clone()))
    );

    iam.mx(&["admin", "group", "remove", &live.alias, &group, &user])
        .success()
        .stdout(format!(
            "Removed members {{{user}}} from group {group} successfully.\n"
        ));
    assert!(!iam.can_read("iamgroupuser"), "user left the group");
    iam.mx(&["admin", "group", "remove", &live.alias, &group])
        .success()
        .stdout(format!("Removed group {group} successfully.\n"));
}

#[test]
fn live_access_keys_authenticate() {
    let Some(live) = Live::new() else { return };
    let iam = Iam::new(&live);
    let obj = live.local_file("obj.txt", "hello\n");
    iam.mx(&["cp", obj.to_str().unwrap(), &live.url("obj.txt")])
        .success();
    let user = iam.name("u1");
    iam.mx(&["admin", "user", "add", &live.alias, &user, "secret12345"])
        .success();
    let policy = iam.policy("p1", &["s3:GetObject"]);
    iam.mx(&[
        "admin",
        "policy",
        "attach",
        &live.alias,
        &policy,
        "--user",
        &user,
    ])
    .success();

    // accesskey create with generated credentials.
    let created = iam.json(&[
        "admin",
        "accesskey",
        "create",
        &format!("{}/", live.alias),
        &user,
        "--name",
        "key1",
        "--expiry-duration",
        "48h",
    ]);
    let key = created[0]["accessKey"].as_str().unwrap().to_string();
    let secret = created[0]["secretKey"].as_str().unwrap().to_string();
    assert_eq!(key.len(), 20);
    iam.alias("iamkey", &key, &secret);
    assert!(iam.can_read("iamkey"));
    let info = iam.json(&["admin", "accesskey", "info", &live.alias, &key]);
    assert_eq!(info[0]["parentUser"], user.as_str());
    assert_eq!(info[0]["name"], "key1");
    assert_eq!(info[0]["impliedPolicy"], true);
    assert!(info[0]["expiration"].is_string());

    iam.mx(&["admin", "accesskey", "disable", &live.alias, &key])
        .success();
    assert!(!iam.can_read("iamkey"), "disabled key");
    iam.mx(&["admin", "accesskey", "enable", &live.alias, &key])
        .success();
    iam.mx(&[
        "admin",
        "accesskey",
        "edit",
        &live.alias,
        &key,
        "--description",
        "d2",
    ])
    .success()
    .stdout(format!("Successfully edited access key `{key}`.\n"));
    let listed = iam.json(&["admin", "accesskey", "list", &live.alias, &user]);
    let keys: Vec<&str> = listed[0]["svcaccs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|k| k["accessKey"].as_str())
        .collect();
    assert!(keys.contains(&key.as_str()), "{listed:?}");

    // svcacct with an embedded policy that denies reading.
    let svc = iam.name("k2");
    let denied = live.local_file(
        "denied.json",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:ListBucket"],"Resource":["arn:aws:s3:::*"]}]}"#,
    );
    iam.mx(&[
        "admin",
        "user",
        "svcacct",
        "add",
        &live.alias,
        &user,
        "--access-key",
        &svc,
        "--secret-key",
        "secret12345",
        "--policy",
        denied.to_str().unwrap(),
    ])
    .success()
    .stdout(format!(
        "Access Key: {svc}\nSecret Key: secret12345\nExpiration: no-expiry\n"
    ));
    iam.alias("iamsvc", &svc, "secret12345");
    assert!(!iam.can_read("iamsvc"), "embedded policy restricts the key");
    let info = iam.json(&["admin", "user", "svcacct", "info", &live.alias, &svc]);
    assert_eq!(
        info[0]["policy"]["Statement"][0]["Action"][0],
        "s3:ListBucket"
    );
    iam.mx(&[
        "admin",
        "user",
        "svcacct",
        "info",
        &live.alias,
        &svc,
        "--policy",
    ])
    .success()
    .stdout(predicates::str::contains("\"s3:ListBucket\""));

    for access_key in [&key, &svc] {
        iam.mx(&["admin", "accesskey", "remove", &live.alias, access_key])
            .success();
    }
    assert!(!iam.can_read("iamkey"), "removed key");
    iam.mx(&[
        "admin",
        "accesskey",
        "sts-revoke",
        &live.alias,
        &user,
        "--all",
    ])
    .success()
    .stdout(format!(
        "Successfully revoked all STS accounts for user {user}\n"
    ));
}

#[test]
fn live_policy_info_writes_file() {
    let Some(live) = Live::new() else { return };
    let iam = Iam::new(&live);
    let policy = iam.policy("p1", &["s3:GetObject"]);
    let out = live.home.path().join("out.json");
    let info = iam.json(&[
        "admin",
        "policy",
        "info",
        &live.alias,
        &policy,
        "-f",
        out.to_str().unwrap(),
    ]);
    assert_eq!(info[0]["policyInfo"]["PolicyName"], policy.as_str());
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(written, info[0]["policyInfo"]["Policy"]);
    let names: Vec<Value> = iam
        .json(&["admin", "policy", "list", &live.alias])
        .into_iter()
        .map(|doc| doc["policy"].clone())
        .collect();
    assert!(names.contains(&Value::from(policy.clone())));
    iam.mx(&["admin", "policy", "remove", &live.alias, &policy])
        .success()
        .stdout(format!("Removed policy `{policy}` successfully.\n"));
}
