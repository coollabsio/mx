//! Live tests for share, tag, version, anonymous, ilm rule, ping, ready, cors, encrypt.
//! Skipped unless MX_LIVE_TESTS=1 (see tests/live_minio.sh).

mod common;

use common::live::{BucketOpts, Live};
use predicates::prelude::*;
use serde_json::Value;
use std::process::Command as Process;

fn stdout_of(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).to_string()
}

fn json_lines(assert: &assert_cmd::assert::Assert) -> Vec<Value> {
    stdout_of(assert)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json line"))
        .collect()
}

/// HTTP status of an anonymous GET.
fn http_status(url: &str) -> String {
    let output = Process::new("curl")
        .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", url])
        .output()
        .expect("curl");
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn http_body(url: &str) -> String {
    let output = Process::new("curl")
        .args(["-sf", url])
        .output()
        .expect("curl");
    assert!(output.status.success(), "curl {url} failed");
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn put(live: &Live, key: &str, contents: &str) {
    let file = live.local_file("upload.tmp", contents);
    live.cmd()
        .args(["put", file.to_str().unwrap(), &live.url(key)])
        .assert()
        .success();
}

#[test]
fn live_share_download_upload_and_list() {
    let Some(live) = Live::new() else { return };
    put(&live, "docs/a.txt", "alpha");
    put(&live, "docs/sub/b.txt", "beta");

    // download: single object, text output, URL works
    live.cmd()
        .args(["share", "download", "-E", "1h", &live.url("docs/a.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Expire: 1 hours 0 minutes 0 seconds",
        ))
        .stdout(predicate::str::contains("Share: http"));
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "share", "download", &live.url("docs/a.txt")])
            .assert()
            .success(),
    );
    assert_eq!(lines.len(), 1);
    assert!(lines[0]["url"].as_str().unwrap().ends_with("/docs/a.txt"));
    assert_eq!(lines[0]["timeLeft"], 604_800_000_000_000u64);
    assert_eq!(http_body(lines[0]["share"].as_str().unwrap()), "alpha");

    // download: prefix without / (non-recursive) and recursive bucket
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "share", "download", &live.url("docs")])
            .assert()
            .success(),
    );
    assert_eq!(lines.len(), 1, "{lines:?}");
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "share", "download", "-r", &live.bucket_target()])
            .assert()
            .success(),
    );
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(http_body(lines[1]["share"].as_str().unwrap()), "beta");
    live.cmd()
        .args(["share", "download", &live.url("missing.txt")])
        .assert()
        .failure();

    // upload: run the generated curl command
    let lines = json_lines(
        &live
            .cmd()
            .args([
                "--json",
                "share",
                "upload",
                "-T",
                "text/plain",
                &live.url("up/one.txt"),
            ])
            .assert()
            .success(),
    );
    let curl = lines[0]["share"].as_str().unwrap().to_string();
    assert!(curl.starts_with("curl http"), "{curl}");
    assert!(curl.contains("-F key=up/one.txt -F file=@<FILE>"), "{curl}");
    assert_eq!(lines[0]["contentType"], "text/plain");
    let payload = live.local_file("payload.txt", "uploaded via POST policy");
    let command = format!(
        "{} -F Content-Type=text/plain -sf -o /dev/null",
        curl.replace("<FILE>", payload.to_str().unwrap())
    );
    let status = Process::new("sh").args(["-c", &command]).status().unwrap();
    assert!(status.success(), "curl upload failed: {command}");
    live.cmd()
        .args(["cat", &live.url("up/one.txt")])
        .assert()
        .success()
        .stdout("uploaded via POST policy");

    // recursive upload: any key under the prefix
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "share", "upload", "-r", &live.url("drop/")])
            .assert()
            .success(),
    );
    let curl = lines[0]["share"].as_str().unwrap().to_string();
    assert!(curl.contains("-F key=drop/<NAME>"), "{curl}");
    let command = format!(
        "{} -sf -o /dev/null",
        curl.replace("<FILE>", payload.to_str().unwrap())
            .replace("<NAME>", "x.bin")
    );
    assert!(
        Process::new("sh")
            .args(["-c", &command])
            .status()
            .unwrap()
            .success()
    );
    live.cmd()
        .args(["stat", &live.url("drop/x.bin")])
        .assert()
        .success();
    // a key outside the prefix is rejected by the policy
    let command = format!(
        "{} -s -o /dev/null -w '%{{http_code}}'",
        curl.replace("<FILE>", payload.to_str().unwrap())
            .replace("key=drop/<NAME>", "key=other/x.bin")
    );
    let output = Process::new("sh").args(["-c", &command]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "403");

    // share list
    let uploads = json_lines(
        &live
            .cmd()
            .args(["--json", "share", "list", "upload"])
            .assert()
            .success(),
    );
    assert_eq!(uploads.len(), 2, "{uploads:?}");
    assert!(
        uploads
            .iter()
            .any(|item| item["contentType"] == "text/plain")
    );
    let downloads = json_lines(
        &live
            .cmd()
            .args(["--json", "share", "ls", "download"])
            .assert()
            .success(),
    );
    assert!(downloads.len() >= 3, "{downloads:?}");
    assert!(live.home.path().join(".mx/share/downloads.json").exists());
}

#[test]
fn live_tags_versions_and_recursive() {
    let Some(live) = Live::with_bucket(BucketOpts {
        versioning: true,
        lock: false,
    }) else {
        return;
    };
    for version in ["v1", "v2", "v3"] {
        put(&live, "obj.txt", version);
    }
    put(&live, "top.txt", "x");
    put(&live, "dir/nested.txt", "x");

    live.cmd()
        .args(["tag", "set", &live.url("obj.txt"), "a=1&b=2"])
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"^Tags set for http.*/obj\.txt\.\n$").unwrap());
    live.cmd()
        .args(["tag", "list", &live.url("obj.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains("Name : http"))
        .stdout(predicate::str::contains("a    : 1\nb    : 2"));
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "tag", "list", &live.url("obj.txt")])
            .assert()
            .success(),
    );
    assert_eq!(lines[0]["tagset"], serde_json::json!({"a": "1", "b": "2"}));

    // --versions applies to every version
    let lines = json_lines(
        &live
            .cmd()
            .args([
                "--json",
                "tag",
                "set",
                "--versions",
                &live.url("obj.txt"),
                "v=all",
            ])
            .assert()
            .success(),
    );
    assert_eq!(lines.len(), 3);
    let oldest = lines[2]["versionID"].as_str().unwrap().to_string();
    assert!(!oldest.is_empty());
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "tag", "list", "--versions", &live.url("obj.txt")])
            .assert()
            .success(),
    );
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().all(|line| line["tagset"]["v"] == "all"));

    // --version-id targets one version
    live.cmd()
        .args([
            "tag",
            "set",
            "-vid",
            &oldest,
            &live.url("obj.txt"),
            "only=old",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("({oldest})")));
    live.cmd()
        .args(["tag", "list", "--version-id", &oldest, &live.url("obj.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains("only : old"));
    live.cmd()
        .args(["tag", "list", &live.url("obj.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains("v    : all"));

    // --rewind to now picks the latest version
    let lines = json_lines(
        &live
            .cmd()
            .args([
                "--json",
                "tag",
                "list",
                "--rewind",
                "0s",
                &live.url("obj.txt"),
            ])
            .assert()
            .success(),
    );
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["tagset"]["v"], "all");

    // recursive with --exclude-folders skips keys inside "folders"
    let lines = json_lines(
        &live
            .cmd()
            .args([
                "--json",
                "tag",
                "set",
                "-r",
                "--exclude-folders",
                &live.bucket_target(),
                "r=1",
            ])
            .assert()
            .success(),
    );
    let names = lines
        .iter()
        .map(|line| line["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names.iter().all(|name| !name.contains("/dir/")),
        "{names:?}"
    );
    live.cmd()
        .args(["tag", "list", &live.url("dir/nested.txt")])
        .assert()
        .success()
        .stdout(predicate::str::contains("No tags found"));
    live.cmd()
        .args(["tag", "remove", "-r", &live.bucket_target()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Tags removed for").count(3));

    // bucket tags
    live.cmd()
        .args(["tag", "list", &live.bucket_target()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No tags found"));
    live.cmd()
        .args(["tag", "set", &live.bucket_target(), "team=x"])
        .assert()
        .success();
    live.cmd()
        .args(["tag", "list", &live.bucket_target()])
        .assert()
        .success()
        .stdout(predicate::str::contains("team : x"));
    live.cmd()
        .args(["--json", "tag", "remove", &live.bucket_target()])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""status":"success""#));
}

#[test]
fn live_version_excluded_prefixes() {
    let Some(live) = Live::new() else { return };
    let target = live.bucket_target();
    live.cmd()
        .args(["version", "info", &target])
        .assert()
        .success()
        .stdout(format!("{target} is un-versioned\n"));
    live.cmd()
        .args([
            "version",
            "enable",
            "--excluded-prefixes",
            "tmp/,cache/",
            "--excluded-prefixes",
            "scratch/",
            "--exclude-folders",
            &target,
        ])
        .assert()
        .success()
        .stdout(format!("{target} versioning is enabled\n"));
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "version", "info", &target])
            .assert()
            .success(),
    );
    assert_eq!(lines[0]["Op"], "info");
    assert_eq!(lines[0]["url"], target.as_str());
    assert_eq!(lines[0]["versioning"]["status"], "Enabled");
    assert_eq!(
        lines[0]["versioning"]["ExcludedPrefixes"],
        serde_json::json!(["tmp/", "cache/", "scratch/"])
    );
    assert_eq!(lines[0]["versioning"]["ExcludeFolders"], true);

    // excluded prefixes are not versioned
    put(&live, "tmp/a.txt", "1");
    put(&live, "tmp/a.txt", "2");
    put(&live, "keep/a.txt", "1");
    put(&live, "keep/a.txt", "2");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = rt
        .block_on(mx::s3::build_client(&live.alias_config()))
        .unwrap();
    let versions = |key: &str| {
        rt.block_on(mx::s3::list_key_versions(&client, &live.bucket, key))
            .unwrap()
            .len()
    };
    assert_eq!(versions("tmp/a.txt"), 1);
    assert_eq!(versions("keep/a.txt"), 2);

    live.cmd()
        .args(["version", "suspend", &target])
        .assert()
        .success()
        .stdout(format!("{target} versioning is suspended\n"));
    live.cmd()
        .args(["version", "info", &target])
        .assert()
        .success()
        .stdout(format!("{target} versioning is suspended\n"));
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "version", "enable", &target])
            .assert()
            .success(),
    );
    assert_eq!(lines[0]["Op"], "enable");
    // mc leaves the `versioning` object of enable/suspend messages empty.
    assert_eq!(lines[0]["versioning"]["status"], "");
}

#[test]
fn live_anonymous_policies_and_links() {
    let Some(live) = Live::new() else { return };
    put(&live, "public/a.txt", "public");
    put(&live, "public/deep/b.txt", "deep");
    put(&live, "private.txt", "secret");
    let target = live.bucket_target();
    let object_url = |key: &str| {
        let assert = live
            .cmd()
            .args(["--json", "share", "download", &live.url(key)])
            .assert()
            .success();
        json_lines(&assert)[0]["url"].as_str().unwrap().to_string()
    };
    let public_url = object_url("public/a.txt");
    let private_url = object_url("private.txt");

    live.cmd()
        .args(["anonymous", "get", &target])
        .assert()
        .success()
        .stdout(format!("Access permission for `{target}` is `private`\n"));
    assert_eq!(http_status(&public_url), "403");

    live.cmd()
        .args(["anonymous", "set", "download", &live.url("public/")])
        .assert()
        .success()
        .stdout(predicate::str::contains("is set to `download`"));
    assert_eq!(http_status(&public_url), "200");
    assert_eq!(http_status(&private_url), "403");
    live.cmd()
        .args(["anonymous", "get", &live.url("public/")])
        .assert()
        .success()
        .stdout(predicate::str::contains("is `download`"));
    live.cmd()
        .args(["anonymous", "get", &target])
        .assert()
        .success()
        .stdout(predicate::str::contains("is `custom`"));

    live.cmd()
        .args(["anonymous", "set", "upload", &live.url("inbox/")])
        .assert()
        .success();
    let rules = json_lines(
        &live
            .cmd()
            .args(["--json", "anonymous", "list", &target])
            .assert()
            .success(),
    );
    let bucket = &live.bucket;
    assert_eq!(
        rules,
        vec![
            serde_json::json!({"resource": format!("{bucket}/inbox/*"), "allow": "writeonly"}),
            serde_json::json!({"resource": format!("{bucket}/public/*"), "allow": "readonly"}),
        ]
    );
    live.cmd()
        .args(["anonymous", "list", &target])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "{bucket}/public/* => readonly"
        )));

    live.cmd()
        .args(["anonymous", "links", &target])
        .assert()
        .success()
        .stdout(predicate::str::contains(public_url.as_str()))
        .stdout(predicate::str::contains("public/deep/"))
        .stdout(predicate::str::contains("private.txt").not());
    let links = json_lines(
        &live
            .cmd()
            .args(["--json", "anonymous", "links", "-r", &target])
            .assert()
            .success(),
    );
    let urls = links
        .iter()
        .map(|link| link["url"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(urls.len(), 2, "{urls:?}");
    assert!(urls.iter().any(|url| url.ends_with("public/deep/b.txt")));
    assert!(!http_body(&urls[0]).is_empty());

    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "anonymous", "get", &live.url("public/")])
            .assert()
            .success(),
    );
    assert_eq!(lines[0]["operation"], "get");
    assert_eq!(lines[0]["permission"], "download");
    assert!(lines[0]["anonymous"]["Statement"].is_array());
    let policy = stdout_of(
        &live
            .cmd()
            .args(["anonymous", "get-json", &target])
            .assert()
            .success(),
    );
    let policy: Value = serde_json::from_str(&policy).expect("policy json");
    assert!(policy["Statement"].is_array());

    // removing both prefixes leaves no policy at all (while inbox/ is still shared, mc
    // reports the remaining policy as `custom`)
    live.cmd()
        .args(["anonymous", "set", "none", &live.url("public/")])
        .assert()
        .success()
        .stdout(predicate::str::contains("is set to `custom`"));
    live.cmd()
        .args(["anonymous", "set", "private", &live.url("inbox/")])
        .assert()
        .success()
        .stdout(predicate::str::contains("is set to `private`"));
    live.cmd()
        .args(["anonymous", "get-json", &target])
        .assert()
        .success()
        .stdout("{}\n");
    assert_eq!(http_status(&public_url), "403");

    // bucket-wide public, then set-json
    live.cmd()
        .args(["anonymous", "set", "public", &target])
        .assert()
        .success()
        .stdout(predicate::str::contains("is set to `public`"));
    assert_eq!(http_status(&private_url), "200");
    let file = live.local_file(
        "policy.json",
        &format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":["*"]}},"Action":["s3:GetObject"],"Resource":["arn:aws:s3:::{bucket}/private.txt"]}}]}}"#
        ),
    );
    live.cmd()
        .args(["anonymous", "set-json", file.to_str().unwrap(), &target])
        .assert()
        .success()
        .stdout(predicate::str::contains("is set from"));
    assert_eq!(http_status(&private_url), "200");
    assert_eq!(http_status(&public_url), "403");
    live.cmd()
        .args(["anonymous", "set", "private", &target])
        .assert()
        .success();
}

#[test]
fn live_ilm_rules() {
    let Some(live) = Live::with_bucket(BucketOpts {
        versioning: true,
        lock: false,
    }) else {
        return;
    };
    let target = live.bucket_target();
    live.cmd()
        .args(["ilm", "rule", "ls", &target])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Unable to get lifecycle. The lifecycle configuration does not exist.",
        ));
    live.cmd()
        .args([
            "ilm",
            "rule",
            "add",
            "--expire-days",
            "30",
            "--prefix",
            "logs/",
            &target,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Lifecycle configuration rule added with ID `",
        ));
    let lines = json_lines(
        &live
            .cmd()
            .args([
                "--json",
                "ilm",
                "rule",
                "add",
                "--expire-days",
                "10",
                "--expire-all-object-versions",
                "--tags",
                "a=1&b=2",
                "--size-gt",
                "1MiB",
                "--size-lt",
                "1GiB",
                &target,
            ])
            .assert()
            .success(),
    );
    let tagged_id = lines[0]["id"].as_str().unwrap().to_string();
    assert_eq!(tagged_id.len(), 20);
    live.cmd()
        .args([
            "ilm",
            "rule",
            "add",
            "--expire-delete-marker",
            "--noncurrent-expire-days",
            "5",
            "--noncurrent-expire-newer",
            "2",
            &live.url("pfx/"),
        ])
        .assert()
        .success();

    live.cmd()
        .args(["ilm", "rule", "ls", &target])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Expiration for latest version (Expiration)",
        ))
        .stdout(predicate::str::contains(
            "Expiration for older versions (NoncurrentVersionExpiration)",
        ))
        .stdout(predicate::str::contains("logs/"))
        .stdout(predicate::str::contains("a=1&b=2"));
    live.cmd()
        .args(["ilm", "rule", "ls", "--transition", &target])
        .assert()
        .success()
        .stdout("");
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "ilm", "rule", "list", &target])
            .assert()
            .success(),
    );
    let rules = lines[0]["config"]["Rules"].as_array().unwrap().clone();
    assert_eq!(rules.len(), 3);
    let tagged = rules
        .iter()
        .find(|rule| rule["ID"] == tagged_id.as_str())
        .unwrap();
    assert_eq!(tagged["Expiration"]["ExpiredObjectAllVersions"], true);
    assert_eq!(tagged["Filter"]["And"]["ObjectSizeGreaterThan"], 1_048_576);
    assert_eq!(
        tagged["Filter"]["And"]["ObjectSizeLessThan"],
        1_073_741_824u64
    );
    assert_eq!(tagged["Filter"]["And"]["Tags"][1]["Key"], "b");
    let noncurrent = rules
        .iter()
        .find(|rule| rule["Filter"]["Prefix"] == "pfx/")
        .unwrap();
    assert_eq!(noncurrent["Expiration"]["ExpiredObjectDeleteMarker"], true);
    assert_eq!(
        noncurrent["NoncurrentVersionExpiration"]["NewerNoncurrentVersions"],
        2
    );

    // edit
    live.cmd()
        .args([
            "ilm",
            "rule",
            "edit",
            "--id",
            &tagged_id,
            "--disable",
            "--expire-days",
            "45",
            &target,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("modified  to"));
    let export = stdout_of(
        &live
            .cmd()
            .args(["ilm", "rule", "export", &target])
            .assert()
            .success(),
    );
    let exported: Value = serde_json::from_str(&export).expect("export json");
    let edited = exported["Rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["ID"] == tagged_id.as_str())
        .unwrap()
        .clone();
    assert_eq!(edited["Status"], "Disabled");
    assert_eq!(edited["Expiration"]["Days"], 45);
    assert_eq!(edited["Expiration"]["ExpiredObjectAllVersions"], true);
    live.cmd()
        .args([
            "ilm", "rule", "edit", "--id", "missing", "--enable", &target,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unable to find rule id"));

    // rm by id, rm --all --force, import
    live.cmd()
        .args(["ilm", "rule", "rm", "--id", &tagged_id, &target])
        .assert()
        .success()
        .stdout(format!(
            "Rule ID `{tagged_id}` from target {target} removed.\n"
        ));
    live.cmd()
        .args(["ilm", "rule", "remove", "--id", &tagged_id, &target])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "ilm", "rule", "rm", "--all", "--force", &target])
            .assert()
            .success(),
    );
    assert_eq!(lines[0]["all"], true);
    live.cmd()
        .args(["ilm", "rule", "export", &target])
        .assert()
        .failure();
    live.cmd()
        .args(["ilm", "rule", "import", &target])
        .write_stdin(export)
        .assert()
        .success()
        .stdout(format!(
            "Lifecycle configuration imported successfully to `{target}`.\n"
        ));
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "ilm", "rule", "export", &target])
            .assert()
            .success(),
    );
    assert_eq!(lines[0]["config"], exported);
    live.cmd()
        .args(["ilm", "rule", "rm", "--all", "--force", &target])
        .assert()
        .success();
}

#[test]
fn live_ping_and_ready() {
    let Some(live) = Live::new() else { return };
    live.cmd()
        .args(["ping", "-c", "2", "-i", "0", &live.alias])
        .assert()
        .success()
        .stdout(predicate::str::contains("  1: http"))
        .stdout(predicate::str::contains("  2: http"))
        .stdout(predicate::str::contains("status=ok"));
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "ping", "--exit", &live.alias])
            .assert()
            .success(),
    );
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["servers"][0]["status"], "ok ");
    live.cmd()
        .args(["ready", &live.alias])
        .assert()
        .success()
        .stdout(format!("The cluster '{}' is ready\n", live.alias));
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "ready", "--cluster-read", &live.alias])
            .assert()
            .success(),
    );
    assert_eq!(lines[0]["healthy"], true);
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "ready", &live.alias])
            .assert()
            .success(),
    );
    assert!(lines[0]["writeQuorum"].as_i64().unwrap() >= 1);
}

#[test]
fn live_encrypt_sse_s3_and_kms() {
    let Some(live) = Live::new() else { return };
    let target = live.bucket_target();
    // mc: a bucket without auto encryption is the server's "not found" error.
    live.cmd()
        .args(["encrypt", "info", &target])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Unable to get encryption info. The server side encryption configuration was not found.",
        ));
    live.cmd()
        .args(["encrypt", "set", "sse-s3", &target])
        .assert()
        .success()
        .stdout(format!(
            "Auto encryption configuration has been set successfully for {target}\n"
        ));
    live.cmd()
        .args(["encrypt", "info", &target])
        .assert()
        .success()
        .stdout("Auto encryption 'sse-s3' is enabled\n");
    if let Ok(key) = std::env::var("MX_TEST_KMS_KEY_ID") {
        live.cmd()
            .args(["encrypt", "set", "sse-kms", &key, &target])
            .assert()
            .success();
        let lines = json_lines(
            &live
                .cmd()
                .args(["--json", "encrypt", "info", &target])
                .assert()
                .success(),
        );
        assert_eq!(lines[0]["op"], "info");
        assert_eq!(lines[0]["encryption"]["algorithm"], "aws:kms");
        assert_eq!(lines[0]["encryption"]["keyId"], key.as_str());
        live.cmd()
            .args(["encrypt", "info", &target])
            .assert()
            .success()
            .stdout(format!(
                "Auto encryption 'sse-kms' is enabled with KeyID: {key}\n"
            ));
    }
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "encrypt", "clear", &target])
            .assert()
            .success(),
    );
    assert_eq!(
        lines[0],
        serde_json::json!({"op":"clear","status":"success","url":target})
    );
    live.cmd()
        .args(["encrypt", "info", &target])
        .assert()
        .failure()
        .stderr(predicate::str::contains("configuration was not found"));
}

#[test]
fn live_cors() {
    let Some(live) = Live::new() else { return };
    let target = live.bucket_target();
    live.cmd()
        .args(["cors", "get", &target])
        .assert()
        .success()
        .stdout("No bucket CORS configuration found.\n");
    live.cmd()
        .args(["--json", "cors", "get", &target])
        .assert()
        .success()
        .stdout("{\"status\":\"not found\"}\n");
    let file = live.local_file(
        "cors.xml",
        "<CORSConfiguration><CORSRule><AllowedOrigin>https://example.com</AllowedOrigin><AllowedMethod>GET</AllowedMethod><MaxAgeSeconds>300</MaxAgeSeconds></CORSRule></CORSConfiguration>",
    );
    let output = live
        .cmd()
        .args(["cors", "set", &target, file.to_str().unwrap()])
        .output()
        .unwrap();
    if !output.status.success() {
        // MinIO community builds do not implement the bucket CORS API.
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("not implemented"), "{stderr}");
        eprintln!("server does not implement bucket CORS; skipping set/get/remove");
        return;
    }
    live.cmd()
        .args(["cors", "get", &target])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "<AllowedOrigin>https://example.com</AllowedOrigin>",
        ));
    let lines = json_lines(
        &live
            .cmd()
            .args(["--json", "cors", "get", &target])
            .assert()
            .success(),
    );
    assert_eq!(lines[0]["cors"]["CORSRules"][0]["MaxAgeSeconds"], 300);
    live.cmd()
        .args(["cors", "remove", &target])
        .assert()
        .success()
        .stdout("Removed bucket CORS config successfully.\n");
}
