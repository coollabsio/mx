use assert_cmd::Command;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static BUCKET_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    std::env::var("MX_LIVE_TESTS").ok().as_deref() == Some("1")
}

pub fn alias_name() -> String {
    std::env::var("MX_TEST_ALIAS").unwrap_or_else(|_| "play".to_string())
}

pub fn bucket_prefix() -> String {
    std::env::var("MX_TEST_BUCKET_PREFIX").unwrap_or_else(|_| "mx-it".to_string())
}

pub fn unique_bucket_name() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = BUCKET_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}-{now}-{seq}", bucket_prefix())
}

pub fn temp_home() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

pub fn mx() -> Command {
    Command::cargo_bin("mx").expect("binary")
}

pub fn configure_alias(home: &Path) {
    let alias = alias_name();
    let url = std::env::var("MX_TEST_URL").ok();
    let access_key = std::env::var("MX_TEST_ACCESS_KEY").ok();
    let secret_key = std::env::var("MX_TEST_SECRET_KEY").ok();
    let api = std::env::var("MX_TEST_API").unwrap_or_else(|_| "S3v4".to_string());
    let path = std::env::var("MX_TEST_PATH").unwrap_or_else(|_| "auto".to_string());

    if let (Some(url), Some(access_key), Some(secret_key)) = (url, access_key, secret_key) {
        mx().env("HOME", home)
            .args([
                "alias",
                "set",
                &alias,
                &url,
                &access_key,
                &secret_key,
                "--api",
                &api,
                "--path",
                &path,
            ])
            .assert()
            .success();
    } else if alias != "play" {
        panic!("custom MX_TEST_ALIAS requires MX_TEST_URL, MX_TEST_ACCESS_KEY, MX_TEST_SECRET_KEY");
    }
}

pub fn local_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write file");
    path
}
