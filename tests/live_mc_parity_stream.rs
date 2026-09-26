//! Parity against the reference mc for the streaming commands: `admin trace`,
//! `admin scanner trace`, `admin logs`, `admin heal`, `watch`.
//!
//!   MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity_stream
//!
//! Streaming cases run mc and mx at the same time ([`Parity::assert_stream_parity`]):
//! traffic is generated with the reference mc on each side's bucket, then both are stopped
//! with SIGTERM. Traces are filtered to the side's own bucket (`--path {bucket}/*`).
//! `admin heal` sequences need an erasure-coded server (`MX_TEST_EC_URL`, service
//! `minio_ec`); the main servers are single-drive, where mc reports the server's error.

mod common;

use common::parity::Parity;

fn alias() -> String {
    std::env::var("MX_TEST_ALIAS").unwrap_or_else(|_| "local".to_string())
}

/// Parity fixture with trace-friendly normalization: run-length spaces collapsed (column
/// padding depends on durations), multipart upload IDs, signing dates, rate-limit counters.
fn trace_fixture() -> Option<Parity> {
    let mut p = Parity::new()?;
    p.normalizer
        .rule(r#"("timeToFirstByte":\s*)\d+"#, "${1}0")
        .rule(r"uploadId=[A-Za-z0-9_=-]+", "uploadId=<ID>")
        .rule(r"\b\d{8}T\d{6}Z\b", "<AMZ_DATE>")
        .rule(r"(X-Ratelimit-(?:Limit|Remaining): )\d+", "${1}<N>")
        .rule(r"Signature=[0-9a-f<>A-Z_]+", "Signature=<SIG>")
        .rule(r"[ \t]{2,}", " ");
    p.object("a.txt", "hello");
    Some(p)
}

/// Same requests (with the reference mc) against each side's bucket.
fn s3_traffic(p: &Parity) {
    p.setup(&["cat", "{target}/a.txt"]);
    p.setup(&["stat", "{target}/a.txt"]);
    p.setup(&["cp", "-q", "{target}/a.txt", "{target}/b.txt"]);
    p.setup(&["rm", "{target}/b.txt"]);
    // A failing request (404) through the reference mc, per side.
    for side in [&p.mc, &p.mx] {
        p.mc_json(&["cat", &format!("{}/{}/missing.txt", alias(), side.bucket)]);
    }
}

// ---------------------------------------------------------------------------
// admin trace
// ---------------------------------------------------------------------------

#[test]
fn parity_admin_trace_short() {
    let Some(p) = trace_fixture() else { return };
    p.assert_stream_parity(
        &["admin", "trace", "--path", "{bucket}/*", "{alias}"],
        s3_traffic,
    );
}

#[test]
fn parity_admin_trace_json_indented_and_lines() {
    let Some(p) = trace_fixture() else { return };
    // `--json` before `admin`: mc keeps its indented encoder output.
    p.assert_stream_json_parity(
        &[
            "--json",
            "admin",
            "trace",
            "--path",
            "{bucket}/*",
            "{alias}",
        ],
        s3_traffic,
    );
    // `--json` on the trace command: JSON lines.
    p.assert_stream_json_parity(
        &[
            "admin",
            "trace",
            "--json",
            "--path",
            "{bucket}/*",
            "{alias}",
        ],
        s3_traffic,
    );
}

#[test]
fn parity_admin_trace_filters() {
    let Some(p) = trace_fixture() else { return };
    p.assert_stream_parity(
        &["admin", "trace", "-e", "--path", "{bucket}/*", "{alias}"],
        s3_traffic,
    );
    p.assert_stream_parity(
        &[
            "admin",
            "trace",
            "--status-code",
            "404",
            "--status-code",
            "200",
            "--method",
            "HEAD",
            "--path",
            "{bucket}/*",
            "{alias}",
        ],
        s3_traffic,
    );
    p.assert_stream_parity(
        &[
            "admin",
            "trace",
            "--funcname",
            "s3.GetObject",
            "--filter-response",
            "--filter-size",
            "1B",
            "--path",
            "{bucket}/*",
            "{alias}",
        ],
        s3_traffic,
    );
}

#[test]
fn parity_admin_trace_verbose() {
    let Some(mut p) = trace_fixture() else { return };
    // mc prints headers in Go map order; compare lines as sets.
    p.unordered = true;
    p.assert_stream_parity(
        &[
            "admin",
            "trace",
            "-v",
            "--path",
            "{bucket}/a.txt",
            "{alias}",
        ],
        s3_traffic,
    );
    p.unordered = false;
    p.assert_stream_json_parity(
        &[
            "admin",
            "trace",
            "-v",
            "--json",
            "--path",
            "{bucket}/a.txt",
            "{alias}",
        ],
        s3_traffic,
    );
}

#[test]
fn parity_admin_trace_errors() {
    let Some(p) = Parity::bare() else { return };
    for json in [false, true] {
        for args in [
            &["admin", "trace", "--all", "--call", "s3", "{alias}"][..],
            &["admin", "trace", "--call", "bogus", "{alias}"],
            &["admin", "trace", "nosuchalias"],
            &[
                "admin",
                "trace",
                "--filter-request",
                "--filter-size",
                "1ZB",
                "{alias}",
            ],
            &["admin", "scanner", "trace", "nosuchalias"],
        ] {
            let mut args = args.to_vec();
            if json {
                args.insert(0, "--json");
                p.assert_json_parity(&args, None);
            } else {
                p.assert_parity(&args, None);
            }
        }
    }
    // Flag value errors: the reason line matches; the SUPPORTED FLAGS block differs in
    // `--response-duration 5ms` (urfave backtick placeholder) and its unquoted default.
    for args in [
        &["admin", "trace", "--status-code", "abc", "{alias}"][..],
        &["admin", "trace", "--response-duration", "bogus", "{alias}"],
    ] {
        let (mc, mx) = p.run(args, None);
        assert_eq!(mc.code, mx.code, "exit code of {args:?}");
        assert_eq!(
            mc.stderr.lines().next(),
            mx.stderr.lines().next(),
            "first stderr line of {args:?}"
        );
    }
    // `--in` errors are not compared: mc races the file reader and its terminal UI.
    // Usage errors print the command help (mx's help text differs): compare exit codes.
    for args in [
        &["admin", "trace"][..],
        &["admin", "trace", "{alias}", "extra"],
        &["admin", "trace", "--filter-request", "{alias}"],
        &["admin", "scanner", "trace"],
        &[
            "admin",
            "scanner",
            "trace",
            "--response-duration",
            "5ms",
            "{alias}",
        ],
    ] {
        let (mc, mx) = p.run(args, None);
        assert_eq!(mc.code, mx.code, "exit code of {args:?}");
    }
}

// ---------------------------------------------------------------------------
// admin logs
// ---------------------------------------------------------------------------

/// Makes the server log an error: an event target that cannot be reached.
fn log_something(p: &Parity) -> bool {
    let Ok(arn) = std::env::var("MX_TEST_NOTIFY_ARN") else {
        eprintln!("skipping logs parity; MX_TEST_NOTIFY_ARN not set");
        return false;
    };
    p.setup(&["event", "add", "{target}", &arn, "--event", "put"]);
    p.object("logged.txt", "x");
    std::thread::sleep(std::time::Duration::from_secs(1));
    true
}

#[test]
fn parity_admin_logs_last() {
    let Some(p) = Parity::new() else { return };
    if !log_something(&p) {
        return;
    }
    p.assert_stream_parity(&["admin", "logs", "--last", "1", "{alias}"], |_| {});
    p.assert_stream_json_parity(
        &["--json", "admin", "logs", "--last", "1", "{alias}"],
        |_| {},
    );
    p.assert_stream_parity(
        &[
            "admin",
            "logs",
            "--last",
            "1",
            "--type",
            "application",
            "{alias}",
            "nosuchnode",
        ],
        |_| {},
    );
}

#[test]
fn parity_admin_logs_errors() {
    let Some(p) = Parity::bare() else { return };
    for args in [
        &["admin", "logs", "--last", "0", "{alias}"][..],
        &["admin", "logs", "--type", "bogus", "{alias}"],
        &["admin", "logs", "nosuchalias"],
    ] {
        p.assert_parity(args, None);
        let mut json = args.to_vec();
        json.insert(0, "--json");
        p.assert_json_parity(&json, None);
    }
    for args in [
        &["admin", "logs"][..],
        &["admin", "logs", "{alias}", "a", "b", "c"],
    ] {
        let (mc, mx) = p.run(args, None);
        assert_eq!(mc.code, mx.code, "exit code of {args:?}");
    }
}

// ---------------------------------------------------------------------------
// admin heal
// ---------------------------------------------------------------------------

#[test]
fn parity_admin_heal_single_drive_errors() {
    let Some(p) = Parity::new() else { return };
    for args in [
        &["admin", "heal", "{alias}"][..],
        &["admin", "heal", "-v", "{alias}"],
        &["admin", "heal", "-r", "{target}"],
        &["admin", "heal", "--force-stop", "{target}"],
        &["admin", "heal", "--force-start", "--force-stop", "{target}"],
        &[
            "admin",
            "heal",
            "-r",
            "--force-start",
            "--force-stop",
            "{alias}",
        ],
        &["admin", "heal", "--pool", "0", "-r", "{target}"],
        &["admin", "heal", "nosuchalias"],
    ] {
        p.assert_parity(args, None);
        let mut json = args.to_vec();
        json.insert(0, "--json");
        p.assert_json_parity(&json, None);
    }
    for args in [
        &["admin", "heal"][..],
        &["admin", "heal", "--scan", "bogus", "{alias}"],
    ] {
        let (mc, mx) = p.run(args, None);
        assert_eq!(mc.code, mx.code, "exit code of {args:?}");
    }
}

/// Parity fixture with alias `ec` (erasure-coded server) and `{bucket}` created on it with
/// objects `dir/x1`, `dir/x2` by each tool.
fn ec_fixture() -> Option<Parity> {
    let url = std::env::var("MX_TEST_EC_URL").ok();
    let Some(url) = url else {
        eprintln!("skipping heal parity; MX_TEST_EC_URL not set (service minio_ec)");
        return None;
    };
    let mut p = Parity::bare()?;
    let user = std::env::var("MX_TEST_EC_ACCESS_KEY").expect("MX_TEST_EC_ACCESS_KEY");
    let pass = std::env::var("MX_TEST_EC_SECRET_KEY").expect("MX_TEST_EC_SECRET_KEY");
    let host = url.replacen("://", &format!("://{user}:{pass}@"), 1);
    p.env.push(("MC_HOST_ec".into(), host));
    // Drives are listed in server order; the heal colors carry the comparison.
    p.normalizer.rule(r"/data\d", "/dataN");
    let (mc, mx) = p.run(&["mb", "ec/{bucket}"], None);
    assert_eq!((mc.code, mx.code), (Some(0), Some(0)), "mb: {mc:?} {mx:?}");
    for key in ["dir/x1", "dir/x2"] {
        let (mc, mx) = p.run(&["pipe", &format!("ec/{{bucket}}/{key}")], Some(b"hello"));
        assert_eq!(
            (mc.code, mx.code),
            (Some(0), Some(0)),
            "pipe: {mc:?} {mx:?}"
        );
    }
    Some(p)
}

#[test]
fn parity_admin_heal_erasure() {
    let Some(p) = ec_fixture() else { return };
    p.assert_parity(&["admin", "heal", "ec"], None);
    p.assert_parity(&["admin", "heal", "-v", "ec"], None);
    p.assert_parity(&["admin", "heal", "-r", "ec/{bucket}"], None);
    p.assert_parity(
        &["admin", "heal", "-r", "--dry-run", "ec/{bucket}/dir"],
        None,
    );
    p.assert_json_parity(&["--json", "admin", "heal", "-r", "ec/{bucket}"], None);
    p.assert_parity(&["admin", "heal", "--force-stop", "ec/{bucket}"], None);
    p.assert_json_parity(
        &["--json", "admin", "heal", "--force-stop", "ec/{bucket}"],
        None,
    );
}

// ---------------------------------------------------------------------------
// watch
// ---------------------------------------------------------------------------

fn watch_traffic(p: &Parity) {
    p.object("dir/f.txt", "abc");
    p.setup(&["cat", "{target}/dir/f.txt"]);
    p.setup(&["cp", "-q", "{target}/dir/f.txt", "{target}/dir/g.jpg"]);
    p.setup(&["rm", "{target}/dir/f.txt"]);
}

#[test]
fn parity_watch_bucket() {
    let Some(p) = Parity::new() else { return };
    p.assert_stream_parity(&["watch", "{target}"], watch_traffic);
    p.assert_stream_json_parity(&["--json", "watch", "{target}"], watch_traffic);
    p.assert_stream_parity(
        &[
            "watch",
            "--events",
            "put,delete",
            "--suffix",
            ".jpg",
            "{target}",
        ],
        watch_traffic,
    );
    p.assert_stream_parity(&["watch", "{target}/dir/"], watch_traffic);
}

#[test]
fn parity_watch_errors() {
    let Some(p) = Parity::bare() else { return };
    for args in [
        &["watch", "{alias}/nosuchbucket-mx-parity"][..],
        &["watch", "--events", "put,bogus", "{alias}/somebucket"],
        &["watch", "--prefix", "x", "{alias}/somebucket/obj"],
        &["watch", "{work}/nope/deeper"],
    ] {
        p.assert_parity(args, None);
        let mut json = args.to_vec();
        json.insert(0, "--json");
        p.assert_json_parity(&json, None);
    }
    for args in [&["watch"][..], &["watch", "a", "b"]] {
        let (mc, mx) = p.run(args, None);
        assert_eq!(mc.code, mx.code, "exit code of {args:?}");
    }
}

#[test]
fn parity_watch_local_directory() {
    let Some(mut p) = Parity::bare() else { return };
    // inotify delivers some events in a different order to both tools.
    p.unordered = true;
    p.file("wdir/.keep", "");
    let traffic = |p: &Parity| {
        p.file("wdir/a.txt", "hello");
        std::thread::sleep(std::time::Duration::from_millis(300));
        for side in [&p.mc, &p.mx] {
            let _ = std::fs::read(side.path("wdir/a.txt"));
            let _ = std::fs::rename(side.path("wdir/a.txt"), side.path("wdir/c.txt"));
            let _ = std::fs::remove_file(side.path("wdir/c.txt"));
        }
    };
    p.assert_stream_parity(&["watch", "{work}/wdir"], traffic);
    p.assert_stream_json_parity(
        &["--json", "watch", "--events", "put,delete", "{work}/wdir"],
        traffic,
    );
}
