//! mc parity for `batch` and `sql` (see tests/common/parity.rs).
//!
//!   MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity_jobs
//!
//! Batch jobs are server-global, so every batch case runs in one test, in order. Job IDs
//! (`expire-<22 chars>:-1`) and humanized ages are normalized.
//!
//! The reference mc (minio-go v7.0.90) sometimes panics while closing a successful select
//! response (`zstd.(*Decoder).Reset` nil pointer); sql cases retry mc until it does not.

mod common;

use common::parity::{Parity, Tool, line_diff};

fn batch_fixture() -> Option<Parity> {
    let mut p = Parity::bare()?;
    p.normalizer
        .rule(
            r"\b(replicate|keyrotate|expire)-[0-9A-Za-z]{16,32}:-?\d+",
            "$1-<JOBID>",
        )
        .rule(r"\b(?:\d+ (?:seconds?|minutes?) ago|now)\b", "<AGO>")
        // Table padding depends on the humanized ages.
        .rule(r" +\t", "\t")
        .rule(r"(?m) +$", "");
    Some(p)
}

/// Starts `yaml` (written to the mc work dir) with the reference mc; returns the job ID.
fn start_with_mc(p: &Parity, yaml: &str) -> String {
    let path = p.mc.path("setup-job.yaml");
    std::fs::write(&path, yaml).expect("write job");
    let alias = common::live::alias_name();
    let docs = p.mc_json(&["--json", "batch", "start", &alias, path.to_str().unwrap()]);
    let id = docs
        .first()
        .and_then(|doc| doc["result"]["id"].as_str())
        .unwrap_or_else(|| panic!("mc batch start failed: {docs:?}"))
        .to_string();
    // Wait for the job to finish.
    let status = p.mc_json(&["--json", "batch", "status", &alias, &id]);
    assert!(
        status.iter().any(|doc| doc["status"] == "complete"),
        "{status:?}"
    );
    id
}

#[test]
fn parity_batch() {
    let Some(p) = batch_fixture() else { return };
    let base = p.mc.expand("{base}");
    p.setup_once(&["mb", "{alias}/{base}"]);
    for key in ["old/a.txt", "old/b.txt", "keep/c.txt"] {
        p.file(&format!(".seed/{key}"), "data");
    }
    p.setup(&["cp", "-q", "-r", ".seed/", "{alias}/{base}/"]);
    let expire = |prefix: &str| {
        format!(
            "expire:\n  apiVersion: v1\n  bucket: {base}\n  prefix: {prefix}\n  rules:\n    - type: object\n      olderThan: 0s\n"
        )
    };
    let id = start_with_mc(&p, &expire("old/"));

    // generate
    for job_type in ["replicate", "keyrotate", "expire", "list", "nosuchtype"] {
        p.assert_parity(&["batch", "generate", "{alias}", job_type], None);
        p.assert_parity(&["--json", "batch", "generate", "{alias}", job_type], None);
    }

    // start: each side starts its own job.
    p.file("job.yaml", &expire("none/"));
    p.file("bad.yaml", "garbage: [");
    p.file("empty.yaml", "foo: 1\n");
    p.assert_parity(&["batch", "start", "{alias}", "job.yaml"], None);
    p.assert_json_parity(&["--json", "batch", "start", "{alias}", "job.yaml"], None);
    for file in ["nofile.yaml", "bad.yaml", "empty.yaml"] {
        p.assert_parity(&["batch", "start", "{alias}", file], None);
        p.assert_json_parity(&["--json", "batch", "start", "{alias}", file], None);
    }
    p.assert_parity(&["batch", "start", "nosuchalias", "job.yaml"], None);

    // Let both sides' jobs finish before listing.
    std::thread::sleep(std::time::Duration::from_secs(2));

    // list (all jobs of the type; job IDs and ages are normalized)
    p.assert_parity(&["batch", "list", "{alias}", "--type", "expire"], None);
    p.assert_json_parity(
        &["--json", "batch", "list", "{alias}", "--type", "expire"],
        None,
    );
    p.assert_parity(&["batch", "ls", "{alias}", "--type", "catalog"], None);
    p.assert_json_parity(
        &["--json", "batch", "ls", "{alias}", "--type", "catalog"],
        None,
    );

    // describe / status / cancel on the same job
    p.assert_parity(&["batch", "describe", "{alias}", &id], None);
    p.assert_parity(&["--json", "batch", "describe", "{alias}", &id], None);
    p.assert_parity(&["batch", "describe", "{alias}", "nosuchjob"], None);
    p.assert_json_parity(
        &["--json", "batch", "describe", "{alias}", "nosuchjob"],
        None,
    );
    p.assert_json_parity(&["--json", "batch", "status", "{alias}", &id], None);
    p.assert_json_parity(&["--json", "batch", "status", "{alias}", "nosuchjob"], None);
    p.assert_parity(&["batch", "status", "nosuchalias", &id], None);
    // Without a terminal, mc's live view fails to open /dev/tty (skip when one is available).
    if std::fs::File::open("/dev/tty").is_err() {
        p.assert_parity(&["batch", "status", "{alias}", &id], None);
    }
    p.assert_parity(&["batch", "cancel", "{alias}", &id], None);
    p.assert_json_parity(&["--json", "batch", "cancel", "{alias}", &id], None);
}

// ---------------------------------------------------------------------------
// sql
// ---------------------------------------------------------------------------

const CSV: &str = "name,age\nalice,30\nbob,40\n";

fn sql_fixture() -> Option<Parity> {
    let p = Parity::new()?;
    p.object("d.csv", CSV);
    p.object("d.json", "{\"a\":1}\n{\"a\":2}\n");
    p.object("a.txt", "text");
    p.file("local.csv", CSV);
    Some(p)
}

/// Like [`Parity::assert_parity`] (normalized stdout, stderr, exit code), retrying while the
/// reference mc panics.
fn assert_sql(p: &Parity, args: &[&str]) {
    for _ in 0..8 {
        let (mc, mx) = p.run(args, None);
        if mc.stderr.contains("panic: ") {
            continue;
        }
        let mut report = String::new();
        for (stream, a, b) in [
            ("stdout", &mc.stdout, &mx.stdout),
            ("stderr", &mc.stderr, &mx.stderr),
        ] {
            let (a, b) = (p.normalize(Tool::Mc, a), p.normalize(Tool::Mx, b));
            if a != b {
                report.push_str(&format!(
                    "{stream} differs (-mc +mx):\n{}",
                    line_diff(&a, &b)
                ));
            }
        }
        if mc.code != mx.code {
            report.push_str(&format!("exit code: mc={:?} mx={:?}\n", mc.code, mx.code));
        }
        assert!(
            report.is_empty(),
            "parity mismatch for `mc {}`:\n{report}",
            args.join(" ")
        );
        return;
    }
    panic!("reference mc kept panicking for `mc {}`", args.join(" "));
}

#[test]
fn parity_sql_results() {
    let Some(p) = sql_fixture() else { return };
    for args in [
        &["sql", "{target}/d.csv"][..],
        &["--json", "sql", "{target}/d.csv"],
        &[
            "sql",
            "-e",
            "select s.name from S3Object s",
            "{target}/d.csv",
        ],
        &[
            "sql",
            "--query",
            "select count(*) from S3Object",
            "{target}/d.csv",
        ],
        &["sql", "--csv-output", "fd=;", "{target}/d.csv"],
        &["sql", "--csv-output-header", "", "{target}/d.csv"],
        &["sql", "--csv-output-header", "x,y", "{target}/d.csv"],
        &[
            "--json",
            "sql",
            "--csv-output-header",
            "x",
            "{target}/d.csv",
        ],
        &["sql", "--csv-input", "fh=NONE", "{target}/d.csv"],
        &["sql", "--json-output", "rd=\\n\\n", "{target}/d.csv"],
        &["sql", "{target}/d.json"],
        &["sql", "--json-input", "type=lines", "{target}/d.json"],
        &["sql", "{target}"],
    ] {
        assert_sql(&p, args);
    }
}

#[test]
fn parity_sql_errors() {
    let Some(p) = sql_fixture() else { return };
    for args in [
        &["sql", "{target}/nosuch.csv"][..],
        &["--json", "sql", "{target}/nosuch.csv"],
        &["sql", "{alias}/nosuchbucket-mx-parity/x.csv"],
        &["sql", "{target}/a.txt"],
        &["--json", "sql", "{target}/a.txt"],
        &["sql", "-e", "selec bad", "{target}/d.csv"],
        &["--json", "sql", "-e", "selec bad", "{target}/d.csv"],
        &["sql", "--compression", "GZIP", "{target}/d.csv"],
        &["sql", "--csv-input", "rd", "{target}/d.csv"],
        &["sql", "--csv-input", "rd=a,rd=b", "{target}/d.csv"],
        &["sql", "--json-output", "fd=1", "{target}/d.csv"],
        &["--json", "sql", "--json-output", "fd=1", "{target}/d.csv"],
        &["sql", "--json-input", "t=lines", "{target}/d.json"],
        &[
            "sql",
            "--csv-input",
            "fh=USE",
            "--json-input",
            "type=lines",
            "{target}/d.csv",
        ],
        &[
            "sql",
            "--csv-output",
            "",
            "--json-output",
            "",
            "{target}/d.csv",
        ],
        &[
            "sql",
            "--json-output",
            "",
            "--csv-output-header",
            "a",
            "{target}/d.csv",
        ],
        &["sql", "local.csv"],
        &["--json", "sql", "local.csv"],
        &["sql", "missing.csv"],
    ] {
        assert_sql(&p, args);
    }
}
