//! mc parity for the client layer: S3 Signature V2 aliases (`--api S3v2`), `alias set`
//! signature probing and TLS verification errors, connection deadlines.
//!
//!   MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity_client
//!
//! Known gaps are `#[ignore = "parity: ..."]`.

mod common;

use common::parity::Parity;

/// Adds alias `v2` (`--api S3v2`, test server) on both sides, comparing the output.
fn with_v2_alias(p: &Parity) {
    p.assert_parity(
        &[
            "alias",
            "set",
            "v2",
            "{url}",
            "{access_key}",
            "{secret_key}",
            "--api",
            "S3v2",
        ],
        None,
    );
}

/// Presigned V2 query values and the random `alias set` probe bucket vary per run.
fn normalize_client(p: &mut Parity) {
    p.normalizer
        .rule(
            r"(AWSAccessKeyId|Expires|Signature)=[^&\s\x22\\]+",
            "$1=<X>",
        )
        .rule(r"probe-bsign-[a-z0-9]+", "probe-bsign-<X>");
}

fn seeded() -> Option<Parity> {
    let mut p = Parity::new()?;
    normalize_client(&mut p);
    // Transfer summary tables: column widths follow the (normalized) speed.
    p.normalizer.rule("[─]+", "─").rule(" +│", " │");
    p.object("a.txt", "alpha");
    p.object("dir/b.txt", "bravo");
    with_v2_alias(&p);
    Some(p)
}

#[test]
fn s3v2_alias_set_and_list() {
    let Some(p) = Parity::bare() else { return };
    with_v2_alias(&p);
    p.assert_parity(&["alias", "list", "v2"], None);
    p.assert_json_parity(&["--json", "alias", "list", "v2"], None);
}

#[test]
fn s3v2_listing_and_reads() {
    let Some(p) = seeded() else { return };
    for args in [
        &["ls", "v2/{bucket}"][..],
        &["ls", "-r", "v2/{bucket}"],
        &["cat", "v2/{bucket}/a.txt"],
        &["stat", "v2/{bucket}/dir/b.txt"],
        &["du", "v2/{bucket}"],
        &["tree", "v2/{bucket}"],
        &["find", "v2/{bucket}", "--name", "*.txt"],
        &["head", "v2/{bucket}/a.txt"],
    ] {
        p.assert_parity(args, None);
    }
    p.assert_json_parity(&["--json", "ls", "-r", "v2/{bucket}"], None);
    p.assert_json_parity(&["--json", "stat", "v2/{bucket}/a.txt"], None);
}

#[test]
fn s3v2_writes_and_removal() {
    let Some(p) = seeded() else { return };
    p.file("up.txt", "uploaded");
    p.assert_parity(&["put", "up.txt", "v2/{bucket}/up.txt"], None);
    p.assert_json_parity(&["--json", "put", "up.txt", "v2/{bucket}/up2.txt"], None);
    p.assert_parity(
        &["cp", "-q", "v2/{bucket}/up.txt", "v2/{bucket}/copy.txt"],
        None,
    );
    p.assert_parity(&["cat", "v2/{bucket}/copy.txt"], None);
    p.assert_parity(&["pipe", "-q", "v2/{bucket}/piped.txt"], Some(b"piped"));
    p.assert_parity(&["rm", "v2/{bucket}/up.txt"], None);
    p.assert_json_parity(&["--json", "rm", "v2/{bucket}/copy.txt"], None);
    p.assert_parity(&["rm", "-r", "--force", "v2/{bucket}/dir"], None);
    p.assert_parity(&["ls", "-r", "v2/{bucket}"], None);
}

#[test]
fn s3v2_make_and_remove_bucket() {
    let Some(p) = Parity::bare() else { return };
    with_v2_alias(&p);
    p.assert_parity(&["mb", "v2/{bucket}"], None);
    p.assert_parity(&["mb", "v2/{bucket}"], None);
    p.assert_parity(&["rb", "v2/{bucket}"], None);
    p.assert_parity(&["rb", "v2/{bucket}"], None);
}

#[test]
fn s3v2_share_download() {
    let Some(p) = seeded() else { return };
    p.assert_parity(&["share", "download", "v2/{bucket}/a.txt"], None);
    p.assert_json_parity(
        &[
            "--json",
            "share",
            "download",
            "--expire",
            "1h",
            "v2/{bucket}/dir/b.txt",
        ],
        None,
    );
}

#[test]
fn s3v2_bucket_config() {
    let Some(p) = seeded() else { return };
    p.assert_parity(&["version", "enable", "v2/{bucket}"], None);
    p.assert_parity(&["version", "info", "v2/{bucket}"], None);
    p.assert_parity(&["anonymous", "set", "download", "v2/{bucket}"], None);
    p.assert_parity(&["anonymous", "get", "v2/{bucket}"], None);
    // MinIO rejects the V2 signature of tagging and replication-config requests (its V2
    // sub-resource list differs from minio-go's); mc reports the server error.
    p.assert_parity(&["tag", "set", "v2/{bucket}/a.txt", "k=v"], None);
    p.assert_parity(&["replicate", "ls", "v2/{bucket}"], None);
    p.assert_json_parity(&["--json", "replicate", "export", "v2/{bucket}"], None);
}

// ---------------------------------------------------------------------------
// alias set: probing and TLS verification
// ---------------------------------------------------------------------------

#[test]
fn alias_set_probes_the_signature() {
    let Some(p) = Parity::bare() else { return };
    p.assert_json_parity(
        &[
            "--json",
            "alias",
            "set",
            "probed",
            "{url}",
            "{access_key}",
            "{secret_key}",
        ],
        None,
    );
    p.assert_parity(&["alias", "list", "probed"], None);
}

const ARGS_DOWN: [&str; 6] = [
    "alias",
    "set",
    "down",
    "http://127.0.0.1:1",
    "minio",
    "minio123",
];

#[test]
fn alias_set_unreachable_server() {
    let Some(mut p) = Parity::bare() else { return };
    normalize_client(&mut p);
    p.assert_parity(&ARGS_DOWN, None);
}

#[test]
#[ignore = "parity: mc's JSON error embeds Go's url.Error struct (`Op`, `URL`, nested net.OpError) under `cause.error`"]
fn alias_set_unreachable_server_json() {
    let Some(mut p) = Parity::bare() else { return };
    normalize_client(&mut p);
    let mut json = vec!["--json"];
    json.extend(ARGS_DOWN);
    p.assert_json_parity(&json, None);
}

/// Without a terminal mc never prompts: an untrusted certificate fails the probe.
fn untrusted_tls(env: &str) {
    let Some(mut p) = Parity::bare() else { return };
    let Ok(url) = std::env::var(env) else {
        eprintln!("skipping parity test; {env} not set");
        return;
    };
    normalize_client(&mut p);
    let args = [
        "alias",
        "set",
        "tls",
        url.as_str(),
        "{access_key}",
        "{secret_key}",
    ];
    p.assert_parity(&args, None);
    // `--api` skips the probe.
    let mut with_api = args.to_vec();
    with_api.extend(["--api", "S3v4"]);
    p.assert_parity(&with_api, None);
}

#[test]
fn alias_set_ca_issued_certificate_is_untrusted() {
    untrusted_tls("MX_TEST_TLS_URL");
}

#[test]
fn alias_set_self_signed_certificate_is_untrusted_without_tty() {
    untrusted_tls("MX_TEST_SELFSIGNED_URL");
}

#[test]
#[ignore = "parity: transport errors of S3 commands lack mc's `Get \"URL\": ` prefix (the SDK error carries no request URL)"]
fn untrusted_certificate_fails_later_commands() {
    let Some(p) = Parity::bare() else { return };
    let Ok(url) = std::env::var("MX_TEST_TLS_URL") else {
        return;
    };
    p.assert_parity(
        &[
            "alias",
            "set",
            "tls",
            url.as_str(),
            "{access_key}",
            "{secret_key}",
            "--api",
            "S3v4",
        ],
        None,
    );
    p.assert_parity(&["ls", "tls"], None);
}

#[test]
#[ignore = "parity: mc's JSON error embeds Go's url.Error with the whole x509 certificate (`cause.error.Err.Cert`)"]
fn alias_set_untrusted_certificate_json() {
    let Some(mut p) = Parity::bare() else { return };
    let Ok(url) = std::env::var("MX_TEST_TLS_URL") else {
        return;
    };
    normalize_client(&mut p);
    p.assert_json_parity(
        &[
            "--json",
            "alias",
            "set",
            "tls",
            url.as_str(),
            "{access_key}",
            "{secret_key}",
        ],
        None,
    );
}

// ---------------------------------------------------------------------------
// Connection deadlines
// ---------------------------------------------------------------------------

#[test]
fn conn_deadlines_allow_normal_requests() {
    let Some(p) = seeded() else { return };
    p.assert_parity(
        &[
            "ls",
            "--conn-read-deadline",
            "1m",
            "--conn-write-deadline",
            "1m",
            "{target}",
        ],
        None,
    );
}

#[test]
#[ignore = "parity: mc's first request is minio-go's GetBucketLocation, so its timeout error names `Get \"URL/?location=\"`; mx reports `read tcp A->B: i/o timeout` alone"]
fn conn_read_deadline_timeout_error() {
    let Some(mut p) = Parity::new() else { return };
    p.normalizer.rule(r"\d+\.\d+\.\d+\.\d+:\d+", "<ADDR>");
    p.assert_parity(&["ls", "--conn-read-deadline", "1ns", "{target}"], None);
}
