//! Live checks for the shared S3 building blocks (PutOptions/GetOptions/ListOptions, checksum,
//! SSE, object lock, cross-server copy). Skipped unless MX_LIVE_TESTS=1.

mod common;

use aws_sdk_s3::types::{ChecksumMode, ServerSideEncryption};
use common::live::{self, BucketOpts, Live};
use mx::flags::{ChecksumAlgo, Sse};
use mx::s3::{self, GetOptions, ListOptions, ObjectRef, PutOptions};
use predicates::prelude::*;
use std::time::{Duration, SystemTime};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("runtime")
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

const MIB: usize = 1024 * 1024;

#[test]
fn live_put_paths_and_cli_roundtrip() {
    let Some(live) = Live::new() else { return };
    let small = live.local_file("small.txt", "small body\n");
    live.cmd()
        .args(["put", small.to_str().unwrap(), &live.url("small.txt")])
        .assert()
        .success();
    live.cmd()
        .args(["cat", &live.url("small.txt")])
        .assert()
        .success()
        .stdout("small body\n");

    // > 8 MiB forces the multipart path through `cp`.
    let big_path = live.home.path().join("big.bin");
    let big = pattern(9 * MIB + 17);
    std::fs::write(&big_path, &big).unwrap();
    live.cmd()
        .args(["cp", big_path.to_str().unwrap(), &live.url("big.bin")])
        .assert()
        .success();
    let alias = live.alias_config();
    let rt = rt();
    let client = rt.block_on(s3::build_client(&alias)).unwrap();
    let head = rt
        .block_on(s3::head_object_with(
            &client,
            &live.bucket,
            "big.bin",
            &GetOptions::default(),
        ))
        .unwrap();
    assert_eq!(head.content_length(), Some(big.len() as i64));
    assert!(head.e_tag().unwrap().contains('-'), "multipart etag");
    let small_head = rt
        .block_on(s3::head_object_with(
            &client,
            &live.bucket,
            "small.txt",
            &GetOptions::default(),
        ))
        .unwrap();
    assert!(
        !small_head.e_tag().unwrap().contains('-'),
        "single put etag"
    );

    let out = live.home.path().join("big.out");
    live.cmd()
        .args(["cp", &live.url("big.bin"), out.to_str().unwrap()])
        .assert()
        .success();
    assert_eq!(std::fs::read(out).unwrap(), big);

    // pipe from stdin (unknown size).
    live.cmd()
        .args(["pipe", &live.url("piped.txt")])
        .write_stdin("from stdin")
        .assert()
        .success();
    live.cmd()
        .args(["cat", &live.url("piped.txt")])
        .assert()
        .success()
        .stdout(predicate::eq("from stdin"));
}

#[test]
fn live_put_options_metadata_tags_checksum() {
    let Some(live) = Live::new() else { return };
    let alias = live.alias_config();
    let rt = rt();
    let client = rt.block_on(s3::build_client(&alias)).unwrap();
    let options = PutOptions {
        metadata: vec![
            ("Cache-Control".into(), "max-age=90".into()),
            ("Content-Type".into(), "text/x-mx".into()),
            ("Content-Disposition".into(), "attachment".into()),
            ("x-amz-meta-owner".into(), "alice".into()),
            ("project".into(), "mx".into()),
        ],
        tags: vec![
            ("env".into(), "dev test".into()),
            ("team".into(), "a+b/c".into()),
        ],
        storage_class: Some("REDUCED_REDUNDANCY".into()),
        checksum: Some(ChecksumAlgo::Crc32c),
        ..Default::default()
    };

    for (key, len, parallel) in [("single.txt", 1024, None), ("multi.bin", 12 * MIB, Some(3))] {
        let data = pattern(len);
        let opts = PutOptions {
            part_size: Some(5 * MIB as u64),
            parallel,
            ..options.clone()
        };
        let outcome = rt
            .block_on(s3::put_object_reader_with(
                &alias,
                &live.bucket,
                key,
                std::io::Cursor::new(data.clone()),
                None,
                &opts,
            ))
            .unwrap();
        assert_eq!(outcome.size, Some(len as i64));

        let head = rt
            .block_on(
                client
                    .head_object()
                    .bucket(&live.bucket)
                    .key(key)
                    .checksum_mode(ChecksumMode::Enabled)
                    .send(),
            )
            .unwrap();
        assert_eq!(head.content_type(), Some("text/x-mx"), "{key}");
        assert_eq!(head.cache_control(), Some("max-age=90"), "{key}");
        assert_eq!(head.content_disposition(), Some("attachment"), "{key}");
        let meta = head.metadata().unwrap();
        assert_eq!(
            meta.get("owner").map(String::as_str),
            Some("alice"),
            "{key}"
        );
        assert_eq!(meta.get("project").map(String::as_str), Some("mx"), "{key}");
        assert_eq!(
            head.storage_class().map(|c| c.as_str()),
            Some("REDUCED_REDUNDANCY"),
            "{key}"
        );
        assert!(
            head.checksum_crc32_c().is_some(),
            "{key} checksum: {head:?}"
        );

        let tags = rt
            .block_on(s3::get_object_tags(&alias, &live.bucket, Some(key)))
            .unwrap();
        assert!(
            tags.contains(&("env".into(), "dev test".into())),
            "{tags:?}"
        );
        assert!(tags.contains(&("team".into(), "a+b/c".into())), "{tags:?}");

        let body = rt
            .block_on(s3::get_object_bytes(&alias, &live.bucket, key))
            .unwrap();
        assert_eq!(body, data, "{key}");
    }

    for algo in [
        ChecksumAlgo::Crc32,
        ChecksumAlgo::Crc64Nvme,
        ChecksumAlgo::Sha1,
        ChecksumAlgo::Sha256,
    ] {
        for (len, suffix) in [(10, "s"), (11 * MIB, "m")] {
            let key = format!("sum-{}-{suffix}", algo.as_str());
            rt.block_on(s3::put_object_reader_with(
                &alias,
                &live.bucket,
                &key,
                std::io::Cursor::new(pattern(len)),
                None,
                &PutOptions {
                    checksum: Some(algo),
                    ..Default::default()
                },
            ))
            .unwrap_or_else(|e| panic!("{key}: {e:?}"));
            let head = rt
                .block_on(
                    client
                        .head_object()
                        .bucket(&live.bucket)
                        .key(&key)
                        .checksum_mode(ChecksumMode::Enabled)
                        .send(),
                )
                .unwrap();
            let present = match algo {
                ChecksumAlgo::Crc32 => head.checksum_crc32().is_some(),
                ChecksumAlgo::Crc64Nvme => head.checksum_crc64_nvme().is_some(),
                ChecksumAlgo::Sha1 => head.checksum_sha1().is_some(),
                ChecksumAlgo::Sha256 => head.checksum_sha256().is_some(),
                ChecksumAlgo::Crc32c => head.checksum_crc32_c().is_some(),
            };
            assert!(present, "{key}: {head:?}");
        }
    }

    // disable_multipart keeps a large upload in one PUT.
    rt.block_on(s3::put_object_reader_with(
        &alias,
        &live.bucket,
        "one-put.bin",
        std::io::Cursor::new(pattern(9 * MIB)),
        None,
        &PutOptions {
            disable_multipart: true,
            ..Default::default()
        },
    ))
    .unwrap();
    let head = rt
        .block_on(s3::head_object_with(
            &client,
            &live.bucket,
            "one-put.bin",
            &GetOptions::default(),
        ))
        .unwrap();
    assert!(!head.e_tag().unwrap().contains('-'));
}

#[test]
fn live_server_side_encryption() {
    let Some(live) = Live::new() else { return };
    let Ok(kms_key) = std::env::var("MX_TEST_KMS_KEY_ID") else {
        eprintln!("skipping SSE test; set MX_TEST_KMS_KEY_ID (server needs KMS)");
        return;
    };
    let alias = live.alias_config();
    let rt = rt();
    let client = rt.block_on(s3::build_client(&alias)).unwrap();
    for (sse, expected) in [
        (Sse::S3, ServerSideEncryption::Aes256),
        (
            Sse::Kms {
                key_id: kms_key.clone(),
            },
            ServerSideEncryption::AwsKms,
        ),
    ] {
        for len in [100, 11 * MIB] {
            let key = format!("enc-{}-{len}", expected.as_str());
            let data = pattern(len);
            rt.block_on(s3::put_object_reader_with(
                &alias,
                &live.bucket,
                &key,
                std::io::Cursor::new(data.clone()),
                None,
                &PutOptions {
                    sse: Some(sse.clone()),
                    ..Default::default()
                },
            ))
            .unwrap_or_else(|e| panic!("{key}: {e:?}"));
            let head = rt
                .block_on(s3::head_object_with(
                    &client,
                    &live.bucket,
                    &key,
                    &GetOptions::default(),
                ))
                .unwrap();
            assert_eq!(head.server_side_encryption(), Some(&expected), "{key}");
            let body = rt
                .block_on(s3::get_object_bytes(&alias, &live.bucket, &key))
                .unwrap();
            assert_eq!(body, data);
        }
    }
    // Server-side copy re-encrypting with SSE-S3 and replacing metadata.
    rt.block_on(s3::copy_object_with(
        ObjectRef {
            alias: &alias,
            bucket: &live.bucket,
            key: "enc-aws:kms-100",
        },
        ObjectRef {
            alias: &alias,
            bucket: &live.bucket,
            key: "enc-copy",
        },
        &GetOptions::default(),
        &PutOptions {
            sse: Some(Sse::S3),
            metadata: vec![("color".into(), "blue".into())],
            ..Default::default()
        },
    ))
    .unwrap();
    let head = rt
        .block_on(s3::head_object_with(
            &client,
            &live.bucket,
            "enc-copy",
            &GetOptions::default(),
        ))
        .unwrap();
    assert_eq!(
        head.server_side_encryption(),
        Some(&ServerSideEncryption::Aes256)
    );
    assert_eq!(
        head.metadata().unwrap().get("color").map(String::as_str),
        Some("blue")
    );
}

#[test]
fn live_versions_rewind_range_and_incomplete() {
    let Some(live) = Live::with_bucket(BucketOpts {
        versioning: true,
        lock: false,
    }) else {
        return;
    };
    let alias = live.alias_config();
    let rt = rt();
    let client = rt.block_on(s3::build_client(&alias)).unwrap();
    let put = |key: &str, body: &str| {
        rt.block_on(s3::put_object_reader_with(
            &alias,
            &live.bucket,
            key,
            std::io::Cursor::new(body.as_bytes().to_vec()),
            None,
            &PutOptions::default(),
        ))
        .unwrap()
    };
    let v1 = put("dir/a.txt", "version-one");
    std::thread::sleep(Duration::from_millis(1100));
    let between = SystemTime::now();
    std::thread::sleep(Duration::from_millis(1100));
    let v2 = put("dir/a.txt", "version-two!");
    put("dir/b.txt", "bee");
    live.cmd()
        .args(["rm", &live.url("dir/b.txt")])
        .assert()
        .success();
    assert!(v1.version_id.is_some() && v1.version_id != v2.version_id);

    let versions = rt
        .block_on(s3::list_object_versions(
            &client,
            &live.bucket,
            Some("dir"),
            true,
        ))
        .unwrap();
    let a: Vec<_> = versions.iter().filter(|v| v.key == "a.txt").collect();
    assert_eq!(a.len(), 2);
    assert!(a[0].is_latest && a[0].version_id == v2.version_id);
    let b: Vec<_> = versions.iter().filter(|v| v.key == "b.txt").collect();
    assert_eq!(b.len(), 2);
    assert!(b[0].is_delete_marker && b[0].is_latest);

    let now_view = rt
        .block_on(s3::list_objects_with(
            &client,
            &live.bucket,
            Some("dir"),
            &ListOptions {
                recursive: true,
                rewind: Some(SystemTime::now() + Duration::from_secs(5)),
                ..Default::default()
            },
        ))
        .unwrap();
    assert_eq!(
        now_view.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(),
        ["a.txt"]
    );
    let past_view = rt
        .block_on(s3::list_objects_with(
            &client,
            &live.bucket,
            Some("dir"),
            &ListOptions {
                recursive: true,
                rewind: Some(between),
                ..Default::default()
            },
        ))
        .unwrap();
    assert_eq!(past_view.len(), 1);
    assert_eq!(past_view[0].version_id, v1.version_id);

    let old = rt
        .block_on(s3::get_object_bytes_with(
            &alias,
            &live.bucket,
            "dir/a.txt",
            &GetOptions {
                version_id: v1.version_id.clone(),
                ..Default::default()
            },
        ))
        .unwrap();
    assert_eq!(old, b"version-one");
    let ranged = rt
        .block_on(s3::get_object_bytes_with(
            &alias,
            &live.bucket,
            "dir/a.txt",
            &GetOptions {
                range: Some((8, Some(10))),
                ..Default::default()
            },
        ))
        .unwrap();
    assert_eq!(ranged, b"two");
    let tail = rt
        .block_on(s3::get_object_bytes_with(
            &alias,
            &live.bucket,
            "dir/a.txt",
            &GetOptions {
                range: Some((8, None)),
                ..Default::default()
            },
        ))
        .unwrap();
    assert_eq!(tail, b"two!");

    // Plain listing still returns only current objects, with timestamps.
    let current = rt
        .block_on(s3::list_object_infos(&alias, &live.bucket, Some("dir")))
        .unwrap();
    assert_eq!(current.len(), 1);
    assert!(current[0].last_modified.is_some() && current[0].is_latest);

    // Incomplete uploads.
    let created = rt
        .block_on(
            client
                .create_multipart_upload()
                .bucket(&live.bucket)
                .key("partial/obj")
                .send(),
        )
        .unwrap();
    let uploads = rt
        .block_on(s3::list_objects_with(
            &client,
            &live.bucket,
            None,
            &ListOptions {
                recursive: true,
                incomplete: true,
                ..Default::default()
            },
        ))
        .unwrap();
    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0].key, "partial/obj");
    assert_eq!(uploads[0].upload_id.as_deref(), created.upload_id());
    rt.block_on(
        client
            .abort_multipart_upload()
            .bucket(&live.bucket)
            .key("partial/obj")
            .upload_id(created.upload_id().unwrap())
            .send(),
    )
    .unwrap();

    // Non-recursive version listing yields prefixes.
    let top = rt
        .block_on(s3::list_objects_with(
            &client,
            &live.bucket,
            None,
            &ListOptions {
                versions: true,
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(
        top.iter().any(|v| v.is_prefix && v.key == "dir/"),
        "{top:?}"
    );
}

#[test]
fn live_object_lock_put_options() {
    let Some(live) = Live::with_bucket(BucketOpts {
        versioning: false,
        lock: true,
    }) else {
        return;
    };
    let alias = live.alias_config();
    let rt = rt();
    let client = rt.block_on(s3::build_client(&alias)).unwrap();
    let until = SystemTime::now() + Duration::from_secs(3600);
    for (key, len) in [("locked-small", 10), ("locked-big", 11 * MIB)] {
        let outcome = rt
            .block_on(s3::put_object_reader_with(
                &alias,
                &live.bucket,
                key,
                std::io::Cursor::new(pattern(len)),
                None,
                &PutOptions {
                    legal_hold: Some(true),
                    retention: Some(("governance".into(), until)),
                    ..Default::default()
                },
            ))
            .unwrap_or_else(|e| panic!("{key}: {e:?}"));
        let head = rt
            .block_on(s3::head_object_with(
                &client,
                &live.bucket,
                key,
                &GetOptions::default(),
            ))
            .unwrap();
        assert_eq!(
            head.object_lock_legal_hold_status().map(|s| s.as_str()),
            Some("ON")
        );
        assert_eq!(
            head.object_lock_mode().map(|s| s.as_str()),
            Some("GOVERNANCE")
        );
        // Release so the fixture can clean up.
        rt.block_on(
            client
                .put_object_legal_hold()
                .bucket(&live.bucket)
                .key(key)
                .set_version_id(outcome.version_id.clone())
                .legal_hold(
                    aws_sdk_s3::types::ObjectLockLegalHold::builder()
                        .status(aws_sdk_s3::types::ObjectLockLegalHoldStatus::Off)
                        .build(),
                )
                .customize()
                .config_override(s3::checksum_override())
                .send(),
        )
        .unwrap();
        rt.block_on(
            client
                .delete_object()
                .bucket(&live.bucket)
                .key(key)
                .set_version_id(outcome.version_id.clone())
                .bypass_governance_retention(true)
                .send(),
        )
        .unwrap();
    }
}

#[test]
fn live_cross_server_copy_streams_and_keeps_metadata() {
    let Some(live) = Live::new() else { return };
    let Some(bucket2) = live.make_bucket2(BucketOpts::default()) else {
        eprintln!("skipping cross-server copy; set MX_TEST_URL2 etc.");
        return;
    };
    let alias2 = live.alias2.clone().unwrap();
    let alias = live.alias_config();
    let rt = rt();
    let data = pattern(9 * MIB + 3);
    rt.block_on(s3::put_object_reader_with(
        &alias,
        &live.bucket,
        "src.bin",
        std::io::Cursor::new(data.clone()),
        None,
        &PutOptions {
            content_type: Some("application/x-mx".into()),
            metadata: vec![("origin".into(), "one".into())],
            ..Default::default()
        },
    ))
    .unwrap();
    live.cmd()
        .args([
            "cp",
            &live.url("src.bin"),
            &format!("{alias2}/{bucket2}/copied.bin"),
        ])
        .assert()
        .success();
    let alias2_config = live::alias_config_from_home(live.home.path(), &alias2);
    let client2 = rt.block_on(s3::build_client(&alias2_config)).unwrap();
    let head = rt
        .block_on(s3::head_object_with(
            &client2,
            &bucket2,
            "copied.bin",
            &GetOptions::default(),
        ))
        .unwrap();
    assert_eq!(head.content_type(), Some("application/x-mx"));
    assert_eq!(
        head.metadata().unwrap().get("origin").map(String::as_str),
        Some("one")
    );
    let body = rt
        .block_on(s3::get_object_bytes(&alias2_config, &bucket2, "copied.bin"))
        .unwrap();
    assert_eq!(body.len(), data.len());
    assert!(body == data);
}

#[test]
fn live_zip_extract_header() {
    let Some(live) = Live::new() else { return };
    let alias = live.alias_config();
    let rt = rt();
    let zip = stored_zip("inner.txt", b"inside the zip");
    rt.block_on(s3::put_object_bytes(
        &alias,
        &live.bucket,
        "archive.zip",
        zip,
        Some("application/zip"),
    ))
    .unwrap();
    let body = rt
        .block_on(s3::get_object_bytes_with(
            &alias,
            &live.bucket,
            "archive.zip/inner.txt",
            &GetOptions {
                zip_extract: true,
                ..Default::default()
            },
        ))
        .unwrap();
    assert_eq!(body, b"inside the zip");
}

/// Minimal single-entry, uncompressed zip archive.
fn stored_zip(name: &str, data: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let crc = crc32(data);
    let mut out = Vec::new();
    let le16 = |v: u16| v.to_le_bytes();
    let le32 = |v: u32| v.to_le_bytes();
    // local file header
    out.extend(le32(0x0403_4b50));
    out.extend(le16(20));
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le32(crc));
    out.extend(le32(data.len() as u32));
    out.extend(le32(data.len() as u32));
    out.extend(le16(name.len() as u16));
    out.extend(le16(0));
    out.extend(name.as_bytes());
    out.extend(data);
    let central_offset = out.len() as u32;
    // central directory
    out.extend(le32(0x0201_4b50));
    out.extend(le16(20));
    out.extend(le16(20));
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le32(crc));
    out.extend(le32(data.len() as u32));
    out.extend(le32(data.len() as u32));
    out.extend(le16(name.len() as u16));
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le32(0));
    out.extend(le32(0));
    out.extend(name.as_bytes());
    let central_size = out.len() as u32 - central_offset;
    // end of central directory
    out.extend(le32(0x0605_4b50));
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le16(1));
    out.extend(le16(1));
    out.extend(le32(central_size));
    out.extend(le32(central_offset));
    out.extend(le16(0));
    out
}
