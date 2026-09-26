//! Live SSE-C tests over TLS (MinIO only accepts SSE-C keys on TLS connections): `put`, `get`,
//! `cat`, `cp` (upload, download, S3->S3 re-encryption), multipart `put -s`, and `stat`.
//! Needs MX_TEST_TLS_URL / MX_TEST_TLS_CA (set by tests/live_minio.sh).

mod common;

use base64::Engine;
use common::live::Tls;

fn key(byte: u8) -> String {
    base64::engine::general_purpose::STANDARD.encode([byte; 32])
}

/// `--enc-c` value for `tls/BUCKET/<prefix>`.
fn enc_c(tls: &Tls, prefix: &str, byte: u8) -> String {
    format!("{}={}", tls.target(prefix), key(byte))
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn live_ssec_put_get_cat_stat() {
    let Some(tls) = Tls::new() else { return };
    tls.trust_ca();
    let file = tls.home.path().join("secret.txt");
    std::fs::write(&file, "sse-c payload").unwrap();
    let enc = enc_c(&tls, "enc/", 1);

    tls.cmd()
        .args([
            "put",
            "--enc-c",
            &enc,
            file.to_str().unwrap(),
            &tls.target("enc/secret.txt"),
        ])
        .assert()
        .success();

    tls.cmd()
        .args(["cat", "--enc-c", &enc, &tls.target("enc/secret.txt")])
        .assert()
        .success()
        .stdout("sse-c payload");
    // Without the key, or with the wrong key, reads fail.
    tls.cmd()
        .args(["cat", &tls.target("enc/secret.txt")])
        .assert()
        .failure();
    tls.cmd()
        .args([
            "cat",
            "--enc-c",
            &enc_c(&tls, "enc/", 2),
            &tls.target("enc/secret.txt"),
        ])
        .assert()
        .failure();

    let downloaded = tls.home.path().join("downloaded.txt");
    tls.cmd()
        .args([
            "get",
            "--enc-c",
            &enc,
            &tls.target("enc/secret.txt"),
            downloaded.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(&downloaded).unwrap(),
        "sse-c payload"
    );

    // stat needs the key too (HEAD of an SSE-C object).
    let output = tls
        .cmd()
        .args([
            "--json",
            "stat",
            "--enc-c",
            &enc,
            &tls.target("enc/secret.txt"),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["size"], 13, "{value}");
    let output = tls
        .cmd()
        .args(["stat", "--enc-c", &enc, &tls.target("enc/secret.txt")])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("SSE-C"), "{}", stdout(&output));
    tls.cmd()
        .args(["stat", "--no-list", &tls.target("enc/secret.txt")])
        .assert()
        .failure();
}

#[test]
fn live_ssec_cp_upload_download_and_reencrypt() {
    let Some(tls) = Tls::new() else { return };
    let file = tls.home.path().join("cp.txt");
    std::fs::write(&file, "copy me").unwrap();
    let src_enc = enc_c(&tls, "src/", 3);
    let dst_enc = enc_c(&tls, "dst/", 4);

    // --insecure here (the other tests use the CA dir).
    tls.cmd()
        .args([
            "--insecure",
            "cp",
            "--enc-c",
            &src_enc,
            file.to_str().unwrap(),
            &tls.target("src/cp.txt"),
        ])
        .assert()
        .success();

    let downloaded = tls.home.path().join("cp-down.txt");
    tls.cmd()
        .args([
            "--insecure",
            "cp",
            "--enc-c",
            &src_enc,
            &tls.target("src/cp.txt"),
            downloaded.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert_eq!(std::fs::read_to_string(&downloaded).unwrap(), "copy me");

    // S3 -> S3, decrypting with the source key and re-encrypting with a different key.
    tls.cmd()
        .args([
            "--insecure",
            "cp",
            "--enc-c",
            &format!("{src_enc},{dst_enc}"),
            &tls.target("src/cp.txt"),
            &tls.target("dst/cp.txt"),
        ])
        .assert()
        .success();
    tls.cmd()
        .args([
            "--insecure",
            "cat",
            "--enc-c",
            &dst_enc,
            &tls.target("dst/cp.txt"),
        ])
        .assert()
        .success()
        .stdout("copy me");
    tls.cmd()
        .args([
            "--insecure",
            "cat",
            "--enc-c",
            &enc_c(&tls, "dst/", 3),
            &tls.target("dst/cp.txt"),
        ])
        .assert()
        .failure();
}

#[test]
fn live_ssec_multipart_put() {
    let Some(tls) = Tls::new() else { return };
    tls.trust_ca();
    // 11 MiB with 5 MiB parts -> 3 parts.
    let data: Vec<u8> = (0..11 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let file = tls.home.path().join("big.bin");
    std::fs::write(&file, &data).unwrap();
    let enc = enc_c(&tls, "mp/", 5);

    tls.cmd()
        .args([
            "put",
            "-s",
            "5MiB",
            "--enc-c",
            &enc,
            file.to_str().unwrap(),
            &tls.target("mp/big.bin"),
        ])
        .assert()
        .success();
    let output = tls
        .cmd()
        .args(["cat", "--enc-c", &enc, &tls.target("mp/big.bin")])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout == data, "multipart SSE-C content mismatch");

    let output = tls
        .cmd()
        .args(["--json", "stat", "--enc-c", &enc, &tls.target("mp/big.bin")])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["size"], data.len(), "{value}");
    assert!(
        value["etag"].as_str().unwrap_or_default().ends_with("-3"),
        "{value}"
    );
}

/// `ilm restore --enc-c`: the key is accepted, and like mc a non-transitioned object fails the
/// restore request (RestoreObject carries no SSE-C headers; only the status HEAD does).
#[test]
fn live_ssec_ilm_restore_non_transitioned() {
    let Some(tls) = Tls::new() else { return };
    tls.trust_ca();
    let file = tls.home.path().join("warm.txt");
    std::fs::write(&file, "warm").unwrap();
    let enc = enc_c(&tls, "", 6);
    tls.cmd()
        .args([
            "put",
            "--enc-c",
            &enc,
            file.to_str().unwrap(),
            &tls.target("warm.txt"),
        ])
        .assert()
        .success();
    // Like mc, the status HEAD runs even though the request failed; with the key it succeeds
    // and reports no restore, without it MinIO rejects the HEAD of the SSE-C object.
    let output = tls
        .cmd()
        .args(["ilm", "restore", "--enc-c", &enc, &tls.target("warm.txt")])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("Sent restore requests to 0 object(s)"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not valid for the current state of the object"),
        "{stderr}"
    );
    assert!(
        stderr.contains("warm.txt` did not receive restore request"),
        "{stderr}"
    );
    let output = tls
        .cmd()
        .args(["ilm", "restore", &tls.target("warm.txt")])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unable to check for restore status"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("did not receive restore request"),
        "{stderr}"
    );
}

/// Polls `check` every second for up to `secs`; returns whether it became true.
fn wait_until(secs: u64, mut check: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed().as_secs() < secs {
        if check() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    check()
}

/// End to end: an SSE-C object transitioned to a tier on server 2 is restored with `--enc-c`
/// (the status HEAD needs the key). Best effort: skipped if MinIO does not transition it.
#[test]
fn live_ssec_ilm_restore_transitioned() {
    let Some(tls) = Tls::new() else { return };
    tls.trust_ca();
    let (Ok(internal), Some(alias2)) = (
        std::env::var("MX_TEST_URL2_INTERNAL"),
        common::live::configure_second_alias(tls.home.path()),
    ) else {
        eprintln!("skipping; second server not configured");
        return;
    };
    let remote_bucket = format!("{}-tier", tls.bucket);
    tls.cmd()
        .args(["mb", &format!("{alias2}/{remote_bucket}")])
        .assert()
        .success();
    let tier = format!("X{}", tls.bucket.replace('-', "")).to_uppercase();
    struct Cleanup<'a>(&'a Tls, Vec<String>);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = self.0.cmd().args(&self.1).output();
        }
    }
    let _remote = Cleanup(
        &tls,
        vec![
            "rb".into(),
            "--force".into(),
            format!("{alias2}/{remote_bucket}"),
        ],
    );
    let _tier = Cleanup(
        &tls,
        ["ilm", "tier", "rm", "--force", "--dangerous", "tls", &tier]
            .map(String::from)
            .to_vec(),
    );
    tls.cmd()
        .args([
            "ilm",
            "tier",
            "add",
            "minio",
            "tls",
            &tier,
            "--endpoint",
            internal.trim_end_matches('/'),
            "--access-key",
            &std::env::var("MX_TEST_ACCESS_KEY2").unwrap(),
            "--secret-key",
            &std::env::var("MX_TEST_SECRET_KEY2").unwrap(),
            "--bucket",
            &remote_bucket,
        ])
        .assert()
        .success();
    tls.cmd()
        .args([
            "ilm",
            "rule",
            "add",
            &format!("tls/{}", tls.bucket),
            "--transition-days",
            "0",
            "--transition-tier",
            &tier,
        ])
        .assert()
        .success();

    let file = tls.home.path().join("cold.txt");
    std::fs::write(&file, "cold sse-c data").unwrap();
    let enc = enc_c(&tls, "", 7);
    let target = tls.target("cold.txt");
    tls.cmd()
        .args(["put", "--enc-c", &enc, file.to_str().unwrap(), &target])
        .assert()
        .success();
    let stat = || -> serde_json::Value {
        let output = tls
            .cmd()
            .args(["--json", "stat", "--enc-c", &enc, &target])
            .output()
            .unwrap();
        serde_json::from_slice(&output.stdout).unwrap_or_default()
    };
    if !wait_until(90, || stat()["storageClass"] == tier.as_str()) {
        eprintln!("skipping restore part: MinIO did not transition the SSE-C object in 90s");
        return;
    }

    tls.cmd()
        .args(["ilm", "restore", "--enc-c", &enc, &target])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "1/1 object(s) successfully restored",
        ));
    tls.cmd()
        .args(["cat", "--enc-c", &enc, &target])
        .assert()
        .success()
        .stdout("cold sse-c data");

    // Without the key the restore request is sent, but the status HEAD fails.
    let output = tls
        .cmd()
        .args(["ilm", "restore", &target])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Unable to check for restore status"),
        "{output:?}"
    );
}
