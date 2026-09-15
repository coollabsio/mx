mod common;

use common::live;
use predicates::prelude::*;

#[test]
fn live_workflow_covers_core_s3_commands() {
    if !live::enabled() {
        eprintln!("skipping live S3 workflow; set MX_LIVE_TESTS=1");
        return;
    }

    let home = live::temp_home();
    live::configure_alias(home.path());
    let alias = live::alias_name();
    let bucket = live::unique_bucket_name();
    let bucket_target = format!("{alias}/{bucket}");
    let bucket_root = format!("{alias}/{bucket}/");
    let object_a = format!("{alias}/{bucket}/hello.txt");
    let object_b = format!("{alias}/{bucket}/copy.txt");
    let object_c = format!("{alias}/{bucket}/moved.txt");
    let download_dir = home.path().join("downloads");
    std::fs::create_dir_all(&download_dir).expect("download dir");

    live::mx()
        .env("HOME", home.path())
        .args(["mb", &bucket_target])
        .assert()
        .success();

    live::mx()
        .env("HOME", home.path())
        .args(["stat", &bucket_target])
        .assert()
        .success()
        .stdout(predicate::str::contains("bucket"));

    let source = live::local_file(home.path(), "hello.txt", "hello from mx\n");
    let source_str = source.to_string_lossy().to_string();
    live::mx()
        .env("HOME", home.path())
        .args(["cp", &source_str, &bucket_root])
        .assert()
        .success();

    live::mx()
        .env("HOME", home.path())
        .args(["stat", &object_a])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello.txt"));

    live::mx()
        .env("HOME", home.path())
        .args(["cat", &object_a])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello from mx"));

    let downloaded = download_dir.join("downloaded.txt");
    let downloaded_str = downloaded.to_string_lossy().to_string();
    live::mx()
        .env("HOME", home.path())
        .args(["cp", &object_a, &downloaded_str])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(&downloaded).expect("downloaded file"),
        "hello from mx\n"
    );

    live::mx()
        .env("HOME", home.path())
        .args(["cp", &object_a, &object_b])
        .assert()
        .success();

    live::mx()
        .env("HOME", home.path())
        .args(["mv", &object_b, &object_c])
        .assert()
        .success();

    live::mx()
        .env("HOME", home.path())
        .args(["rm", &object_c])
        .assert()
        .success();

    live::mx()
        .env("HOME", home.path())
        .args(["rm", &object_a])
        .assert()
        .success();

    live::mx()
        .env("HOME", home.path())
        .args(["rb", &bucket_target])
        .assert()
        .success();
}

#[test]
fn live_put_and_out_alias_work() {
    if !live::enabled() {
        eprintln!("skipping live put/out test; set MX_LIVE_TESTS=1");
        return;
    }

    let home = live::temp_home();
    live::configure_alias(home.path());
    let alias = live::alias_name();
    let bucket = live::unique_bucket_name();
    let bucket_target = format!("{alias}/{bucket}");
    let object_put = format!("{alias}/{bucket}/put.txt");
    let object_out = format!("{alias}/{bucket}/out.txt");

    live::mx()
        .env("HOME", home.path())
        .args(["mb", &bucket_target])
        .assert()
        .success();

    let source = live::local_file(home.path(), "put.txt", "put object\n");
    let source_str = source.to_string_lossy().to_string();
    live::mx()
        .env("HOME", home.path())
        .args(["put", &source_str, &object_put])
        .assert()
        .success();

    let source2 = live::local_file(home.path(), "out.txt", "out alias object\n");
    let source2_str = source2.to_string_lossy().to_string();
    live::mx()
        .env("HOME", home.path())
        .args(["out", &source2_str, &object_out])
        .assert()
        .success();

    live::mx()
        .env("HOME", home.path())
        .args(["rm", &object_put])
        .assert()
        .success();
    live::mx()
        .env("HOME", home.path())
        .args(["rm", &object_out])
        .assert()
        .success();
    live::mx()
        .env("HOME", home.path())
        .args(["rb", &bucket_target])
        .assert()
        .success();
}

#[test]
fn live_coolify_pipe_json_and_ignore_existing_work() {
    if !live::enabled() {
        eprintln!("skipping live Coolify compatibility test; set MX_LIVE_TESTS=1");
        return;
    }

    let home = live::temp_home();
    live::configure_alias(home.path());
    let alias = live::alias_name();
    let bucket = live::unique_bucket_name();
    let bucket_target = format!("{alias}/{bucket}");
    let object = format!("{alias}/{bucket}/archive.tar.gz");

    live::mx()
        .env("HOME", home.path())
        .args(["mb", &bucket_target])
        .assert()
        .success();
    live::mx()
        .env("HOME", home.path())
        .args(["mb", "--ignore-existing", &bucket_target])
        .assert()
        .success();
    live::mx()
        .env("HOME", home.path())
        .args(["pipe", "--quiet", &object])
        .write_stdin("streamed archive")
        .assert()
        .success()
        .stdout("");
    live::mx()
        .env("HOME", home.path())
        .args(["stat", "--json", &object])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"size\":16"));
    live::mx()
        .env("HOME", home.path())
        .args(["rm", &object])
        .assert()
        .success();
    live::mx()
        .env("HOME", home.path())
        .args(["rb", &bucket_target])
        .assert()
        .success();
}
