//! Byte-level parity against the reference MinIO `mc` (see tests/common/parity.rs).
//!
//! Skipped unless MX_LIVE_TESTS=1 and MX_MC_BIN points to the reference mc:
//!
//!   MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity
//!
//! Known gaps are `#[ignore = "parity: ..."]`. Run them (diffs are printed per case):
//!
//!   MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity -- --ignored
//!
//! Against an already running server: set MX_LIVE_TESTS=1, MX_TEST_URL, MX_TEST_ACCESS_KEY,
//! MX_TEST_SECRET_KEY, MX_TEST_ALIAS, MX_MC_BIN="$(sh tests/mc_ref.sh)" and run
//! `cargo test --test live_mc_parity -- --ignored --test-threads=1`.
//! When a fix lands, remove the case's `#[ignore]`.

mod common;

use common::parity::{Opts, Parity};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// No buckets.
fn bare() -> Option<Parity> {
    Parity::bare()
}

/// One empty bucket per side.
fn empty() -> Option<Parity> {
    Parity::new()
}

/// Objects `a.txt`, `dir/b.txt`, `dir/sub/c.txt`; local `src/one.txt`, `src/nested/two.txt`.
fn seeded() -> Option<Parity> {
    let p = Parity::new()?;
    seed(&p);
    Some(p)
}

fn seed(p: &Parity) {
    p.object("a.txt", "alpha\n");
    p.object("dir/b.txt", "bravo 1\nbravo 2\nbravo 3\n");
    p.object("dir/sub/c.txt", "charlie\n");
    p.file("src/one.txt", "one\n");
    p.file("src/nested/two.txt", "two two\n");
}

/// Versioned bucket: `a.txt` with two versions, `dir/b.txt` with a delete marker.
fn versioned() -> Option<Parity> {
    let p = Parity::with(Opts {
        bucket: true,
        versioning: true,
        ..Opts::default()
    })?;
    p.object("a.txt", "alpha\n");
    p.object("a.txt", "alpha v2\n");
    p.object("dir/b.txt", "bravo\n");
    p.setup(&["rm", "{target}/dir/b.txt"]);
    Some(p)
}

/// Object-lock bucket with `a.txt`.
fn locked() -> Option<Parity> {
    let p = Parity::with(Opts {
        bucket: true,
        lock: true,
        ..Opts::default()
    })?;
    p.object("a.txt", "alpha\n");
    Some(p)
}

/// Transfer commands print an mc summary table whose column widths depend on the speed;
/// collapse padding and borders so only the content is compared.
fn transfer(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.normalizer.rule("[─]+", "─").rule(" +│", " │");
    Some(p)
}

/// `pipe` draws a progress counter that mc refreshes on a timer (` 6 B / ?  <SPEED>` frames
/// after the initial ` 0 B / ? `); drop the timing-dependent refresh frames.
fn piping(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.normalizer.rule(r"\r [^\r]* / \?  <SPEED>", "");
    // mc's progress ticker may redraw the ` 0 B / ?` residue more than once.
    p.normalizer.rule(r"(\r? 0 B / \? )+", "\r 0 B / ? ");
    Some(p)
}

/// Parallel transfers (`cp -r`, `mirror`): mc output order is not deterministic.
fn parallel(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.unordered = true;
    Some(p)
}

/// `mirror` JSON carries running totals that depend on completion order.
fn mirrored(p: Option<Parity>) -> Option<Parity> {
    let mut p = parallel(p)?;
    p.normalizer
        .rule(r#"("(?:totalCount|totalSize)":\s*)\d+"#, "${1}0");
    Some(p)
}

/// `alias set` probes a random `probe-bsign-*` bucket.
fn probing(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.normalizer
        .rule(r"probe-bsign-[0-9a-z]+", "probe-bsign-<RANDOM>");
    Some(p)
}

fn with_setup(p: Option<Parity>, args: &[&str]) -> Option<Parity> {
    let p = p?;
    p.setup(args);
    Some(p)
}

/// `name`: one parity case. `text` compares normalized bytes; `json` compares JSON documents
/// field by field (then the bytes). Optional trailing stdin bytes.
macro_rules! case {
    ($(#[$meta:meta])* $name:ident, text, $fixture:expr, [$($arg:expr),* $(,)?] $(, $stdin:expr)?) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            let Some(p) = $fixture else { return };
            let stdin: Option<&[u8]> = None $(.or(Some($stdin)))?;
            p.assert_parity(&[$($arg),*], stdin);
        }
    };
    ($(#[$meta:meta])* $name:ident, json, $fixture:expr, [$($arg:expr),* $(,)?] $(, $stdin:expr)?) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            let Some(p) = $fixture else { return };
            let stdin: Option<&[u8]> = None $(.or(Some($stdin)))?;
            p.assert_json_parity(&["--json", $($arg),*], stdin);
        }
    };
}

// ---------------------------------------------------------------------------
// ls
// ---------------------------------------------------------------------------

case!(ls_alias_root_text, text, seeded(), ["ls", "{alias}/"]);
case!(ls_alias_root_json, json, seeded(), ["ls", "{alias}/"]);
case!(ls_bucket_text, text, seeded(), ["ls", "{target}"]);
case!(ls_bucket_json, json, seeded(), ["ls", "{target}"]);
case!(ls_prefix_text, text, seeded(), ["ls", "{target}/dir/"]);
case!(ls_prefix_json, json, seeded(), ["ls", "{target}/dir/"]);
case!(ls_recursive_text, text, seeded(), ["ls", "-r", "{target}"]);
case!(ls_recursive_json, json, seeded(), ["ls", "-r", "{target}"]);
case!(
    ls_versions_text,
    text,
    versioned(),
    ["ls", "-r", "--versions", "{target}"]
);
case!(
    ls_versions_json,
    json,
    versioned(),
    ["ls", "-r", "--versions", "{target}"]
);
case!(
    ls_summarize_text,
    text,
    seeded(),
    ["ls", "-r", "--summarize", "{target}"]
);
case!(
    ls_summarize_json,
    json,
    seeded(),
    ["ls", "-r", "--summarize", "{target}"]
);
case!(ls_object_text, text, seeded(), ["ls", "{target}/a.txt"]);
case!(
    ls_partial_prefix_json,
    json,
    seeded(),
    ["ls", "{target}/di"]
);
case!(ls_local_text, text, seeded(), ["ls", "src"]);
case!(ls_local_json, json, seeded(), ["ls", "src"]);
case!(
    ls_local_recursive_text,
    text,
    seeded(),
    ["ls", "-r", "src/"]
);
case!(ls_local_missing_text, text, seeded(), ["ls", "src/nope/x"]);

// ---------------------------------------------------------------------------
// stat
// ---------------------------------------------------------------------------

/// Bucket usage comes from MinIO's background scanner, so its numbers depend on scan timing.
fn scanned(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.normalizer
        .rule(r"(Total size|Objects count|Versions count): .*", "$1: <N>")
        .rule(
            r"Object sizes histogram:\n(?: +\d+ object\(s\) \S+\n?)*",
            "",
        )
        .rule(
            r#""Usage":\{(?:[^{}]|\{[^{}]*\})*\}"#,
            r#""Usage":"<USAGE>""#,
        );
    Some(p)
}

case!(
    stat_bucket_text,
    text,
    scanned(seeded()),
    ["stat", "{target}"]
);
case!(
    stat_bucket_json,
    json,
    scanned(seeded()),
    ["stat", "{target}"]
);
case!(
    stat_bucket_versioned_json,
    json,
    scanned(versioned()),
    ["stat", "{target}"]
);
case!(stat_folder_text, text, seeded(), ["stat", "{target}/dir"]);
case!(stat_folder_json, json, seeded(), ["stat", "{target}/dir"]);
case!(stat_prefix_text, text, seeded(), ["stat", "{target}/dir/"]);
case!(
    stat_recursive_json,
    json,
    seeded(),
    ["stat", "-r", "{target}/dir"]
);
case!(stat_object_text, text, seeded(), ["stat", "{target}/a.txt"]);
case!(stat_object_json, json, seeded(), ["stat", "{target}/a.txt"]);

// ---------------------------------------------------------------------------
// mb / rb
// ---------------------------------------------------------------------------

case!(mb_text, text, empty(), ["mb", "{target}-new"]);
case!(mb_json, json, empty(), ["mb", "{target}-new"]);
case!(mb_existing_p_text, text, empty(), ["mb", "-p", "{target}"]);
case!(mb_existing_p_json, json, empty(), ["mb", "-p", "{target}"]);
case!(mb_existing_error_text, text, empty(), ["mb", "{target}"]);
case!(mb_existing_error_json, json, empty(), ["mb", "{target}"]);
case!(rb_text, text, empty(), ["rb", "{target}"]);
case!(rb_json, json, empty(), ["rb", "{target}"]);
case!(rb_not_empty_text, text, seeded(), ["rb", "{target}"]);
case!(rb_not_empty_json, json, seeded(), ["rb", "{target}"]);
case!(rb_force_text, text, seeded(), ["rb", "--force", "{target}"]);
case!(rb_missing_text, text, bare(), ["rb", "{target}"]);
case!(rb_missing_json, json, bare(), ["rb", "{target}"]);
case!(
    rb_missing_force_text,
    text,
    bare(),
    ["rb", "--force", "{target}"]
);
case!(mb_folder_text, text, empty(), ["mb", "{target}/newdir"]);
case!(
    mb_multiple_json,
    json,
    bare(),
    ["mb", "{target}", "{target}-2"]
);

// ---------------------------------------------------------------------------
// rm
// ---------------------------------------------------------------------------

case!(rm_single_text, text, seeded(), ["rm", "{target}/a.txt"]);
case!(rm_single_json, json, seeded(), ["rm", "{target}/a.txt"]);
case!(
    rm_recursive_text,
    text,
    seeded(),
    ["rm", "-r", "--force", "{target}/dir/"]
);
case!(
    rm_recursive_json,
    json,
    seeded(),
    ["rm", "-r", "--force", "{target}/dir/"]
);
case!(
    rm_versions_text,
    text,
    versioned(),
    ["rm", "-r", "--force", "--versions", "{target}"]
);
case!(
    rm_versions_json,
    json,
    versioned(),
    ["rm", "-r", "--force", "--versions", "{target}"]
);

// ---------------------------------------------------------------------------
// cp / mv / put / pipe
// ---------------------------------------------------------------------------

case!(
    cp_upload_text,
    text,
    transfer(seeded()),
    ["cp", "src/one.txt", "{target}/up/one.txt"]
);
case!(
    cp_upload_json,
    json,
    seeded(),
    ["cp", "src/one.txt", "{target}/up/one.txt"]
);
case!(
    cp_upload_quiet,
    text,
    transfer(seeded()),
    ["cp", "-q", "src/one.txt", "{target}/up/one.txt"]
);
case!(
    cp_download_text,
    text,
    transfer(seeded()),
    ["cp", "{target}/a.txt", "out.txt"]
);
case!(
    cp_download_json,
    json,
    seeded(),
    ["cp", "{target}/a.txt", "out.txt"]
);
case!(
    cp_recursive_upload_text,
    text,
    parallel(transfer(seeded())),
    ["cp", "-r", "src/", "{target}/up/"]
);
case!(
    cp_recursive_upload_json,
    json,
    parallel(seeded()),
    ["cp", "-r", "src/", "{target}/up/"]
);
case!(
    cp_recursive_download_text,
    text,
    parallel(transfer(seeded())),
    ["cp", "-r", "{target}/dir/", "down/"]
);
case!(
    cp_recursive_download_json,
    json,
    parallel(seeded()),
    ["cp", "-r", "{target}/dir/", "down/"]
);
case!(
    cp_server_side_text,
    text,
    transfer(seeded()),
    ["cp", "{target}/a.txt", "{target}/copy.txt"]
);
case!(
    cp_server_side_json,
    json,
    seeded(),
    ["cp", "{target}/a.txt", "{target}/copy.txt"]
);
case!(
    mv_upload_text,
    text,
    transfer(seeded()),
    ["mv", "src/one.txt", "{target}/moved.txt"]
);
case!(
    mv_upload_json,
    json,
    seeded(),
    ["mv", "src/one.txt", "{target}/moved.txt"]
);
case!(
    mv_server_side_text,
    text,
    transfer(seeded()),
    ["mv", "{target}/a.txt", "{target}/moved.txt"]
);
case!(
    mv_server_side_json,
    json,
    seeded(),
    ["mv", "{target}/a.txt", "{target}/moved.txt"]
);
case!(
    put_text,
    text,
    transfer(seeded()),
    ["put", "src/one.txt", "{target}/put.txt"]
);
case!(
    put_json,
    json,
    seeded(),
    ["put", "src/one.txt", "{target}/put.txt"]
);
case!(
    pipe_text,
    text,
    piping(seeded()),
    ["pipe", "{target}/piped.txt"],
    b"piped\n"
);
case!(
    pipe_quiet,
    text,
    piping(seeded()),
    ["pipe", "-q", "{target}/piped.txt"],
    b"piped\n"
);
// `mc --json pipe` also draws the progress residue before the JSON document (the command reads
// its own `--json` flag); mx never corrupts JSON output, so compare the `pipe --json` form.
case!(
    pipe_json,
    text,
    seeded(),
    ["pipe", "--json", "{target}/piped.txt"],
    b"piped\n"
);
case!(
    put_quiet,
    text,
    transfer(seeded()),
    ["put", "-q", "src/one.txt", "{target}/put.txt"]
);
case!(
    put_into_folder_json,
    json,
    seeded(),
    ["put", "src/nested/two.txt", "{target}/up/"]
);
case!(
    put_missing_text,
    text,
    seeded(),
    ["put", "nope.txt", "{target}/put.txt"]
);
case!(
    put_missing_json,
    json,
    seeded(),
    ["put", "nope.txt", "{target}/put.txt"]
);
case!(put_folder_text, text, seeded(), ["put", "src", "{target}/"]);
case!(
    put_missing_bucket_text,
    text,
    seeded(),
    ["put", "src/one.txt", "{target}-none/put.txt"]
);
case!(
    get_text,
    text,
    transfer(seeded()),
    ["get", "{target}/a.txt", "got.txt"]
);
case!(
    get_json,
    json,
    seeded(),
    ["get", "{target}/dir/b.txt", "./"]
);
case!(
    get_missing_text,
    text,
    seeded(),
    ["get", "{target}/nope.txt", "got.txt"]
);
case!(
    get_missing_json,
    json,
    seeded(),
    ["get", "{target}/nope.txt", "got.txt"]
);
case!(
    get_not_s3_text,
    text,
    seeded(),
    ["get", "src/one.txt", "got.txt"]
);
// mc reads `totalCount` while its URL producer is still counting (racy); zero the totals.
case!(
    cp_multi_json,
    json,
    mirrored(seeded()),
    ["cp", "{target}/a.txt", "{target}/dir/b.txt", "multi/"]
);
case!(
    cp_parent_relative_text,
    text,
    transfer(seeded()),
    ["cp", "src/../src/./one.txt", "{target}/rel.txt"]
);
case!(
    rm_dry_run_json,
    json,
    seeded(),
    ["rm", "-r", "--force", "--dry-run", "{target}/dir/"]
);

// ---------------------------------------------------------------------------
// cat / head
// ---------------------------------------------------------------------------

case!(cat_text, text, seeded(), ["cat", "{target}/a.txt"]);
case!(
    cat_missing_text,
    text,
    seeded(),
    ["cat", "{target}/nope.txt"]
);
case!(
    cat_missing_json,
    json,
    seeded(),
    ["cat", "{target}/nope.txt"]
);
case!(
    head_text,
    text,
    seeded(),
    ["head", "-n", "2", "{target}/dir/b.txt"]
);

// ---------------------------------------------------------------------------
// du / tree / find / diff
// ---------------------------------------------------------------------------

case!(du_text, text, seeded(), ["du", "{target}"]);
case!(du_json, json, seeded(), ["du", "{target}"]);
case!(du_depth_text, text, seeded(), ["du", "-d", "2", "{target}"]);
case!(tree_text, text, seeded(), ["tree", "{target}"]);
case!(tree_files_text, text, seeded(), ["tree", "-f", "{target}"]);
case!(tree_json, json, seeded(), ["tree", "-f", "{target}"]);
case!(du_recursive_text, text, seeded(), ["du", "-r", "{target}"]);
case!(
    du_versions_json,
    json,
    versioned(),
    ["du", "-r", "--versions", "{target}"]
);
case!(du_local_text, text, seeded(), ["du", "src"]);
case!(du_object_text, text, seeded(), ["du", "{target}/a.txt"]);
case!(
    tree_depth_text,
    text,
    seeded(),
    ["tree", "-d", "1", "{target}"]
);
case!(tree_local_text, text, seeded(), ["tree", "-f", "src"]);
case!(tree_missing_text, text, bare(), ["tree", "{target}"]);
case!(
    find_name_text,
    text,
    seeded(),
    ["find", "{target}", "--name", "*.txt"]
);
case!(
    find_name_json,
    json,
    seeded(),
    ["find", "{target}", "--name", "*.txt"]
);
case!(
    find_local_text,
    text,
    seeded(),
    ["find", "src", "--name", "*.txt"]
);
case!(find_local_all_text, text, seeded(), ["find", "src"]);
case!(
    find_local_json,
    json,
    seeded(),
    ["find", "src", "--name", "*.txt"]
);
case!(
    find_path_text,
    text,
    seeded(),
    ["find", "{target}", "--path", "dir/*"]
);
case!(
    find_maxdepth_text,
    text,
    seeded(),
    ["find", "{target}", "--maxdepth", "2"]
);
case!(
    find_print_text,
    text,
    seeded(),
    ["find", "{target}/dir", "--print", "{base} {dir} {size}"]
);
case!(
    find_versions_text,
    text,
    versioned(),
    ["find", "{target}", "--versions"]
);
case!(
    find_versions_json,
    json,
    versioned(),
    ["find", "{target}", "--versions"]
);

fn diffed() -> Option<Parity> {
    let p = seeded()?;
    p.object("left/same.txt", "same\n");
    p.object("left/changed.txt", "left\n");
    p.object("left/only-left.txt", "l\n");
    p.object("right/same.txt", "same\n");
    p.object("right/changed.txt", "right side\n");
    p.object("right/only-right.txt", "r\n");
    Some(p)
}

case!(
    diff_text,
    text,
    diffed(),
    ["diff", "{target}/left", "{target}/right"]
);
case!(
    diff_json,
    json,
    diffed(),
    ["diff", "{target}/left", "{target}/right"]
);
case!(
    diff_local_s3_text,
    text,
    seeded(),
    ["diff", "src", "{target}/dir"]
);
case!(
    diff_local_s3_json,
    json,
    seeded(),
    ["diff", "src", "{target}/dir"]
);
case!(
    diff_not_folder_text,
    text,
    seeded(),
    ["diff", "src/one.txt", "{target}"]
);

// ---------------------------------------------------------------------------
// share
// ---------------------------------------------------------------------------

case!(
    share_download_json,
    json,
    seeded(),
    ["share", "download", "{target}/a.txt"]
);
case!(
    share_download_text,
    text,
    seeded(),
    ["share", "download", "{target}/a.txt"]
);

case!(
    share_download_expire_text,
    text,
    seeded(),
    ["share", "download", "--expire", "2h", "{target}/a.txt"]
);
case!(
    share_download_recursive_json,
    json,
    seeded(),
    ["share", "download", "-r", "{target}/dir/"]
);

// ---------------------------------------------------------------------------
// tag / version / anonymous / ilm
// ---------------------------------------------------------------------------

fn tagged() -> Option<Parity> {
    with_setup(seeded(), &["tag", "set", "{target}/a.txt", "k1=v1&k2=v2"])
}

case!(
    tag_set_text,
    text,
    seeded(),
    ["tag", "set", "{target}/a.txt", "k1=v1&k2=v2"]
);
case!(
    tag_set_json,
    json,
    seeded(),
    ["tag", "set", "{target}/a.txt", "k1=v1&k2=v2"]
);
case!(
    tag_list_text,
    text,
    tagged(),
    ["tag", "list", "{target}/a.txt"]
);
case!(
    tag_list_json,
    json,
    tagged(),
    ["tag", "list", "{target}/a.txt"]
);
case!(
    tag_remove_text,
    text,
    tagged(),
    ["tag", "remove", "{target}/a.txt"]
);
case!(
    tag_remove_json,
    json,
    tagged(),
    ["tag", "remove", "{target}/a.txt"]
);

case!(
    tag_list_missing_object_text,
    text,
    seeded(),
    ["tag", "list", "{target}/nope.txt"]
);
case!(
    tag_list_missing_object_json,
    json,
    seeded(),
    ["tag", "list", "{target}/nope.txt"]
);
case!(
    tag_list_bucket_text,
    text,
    empty(),
    ["tag", "list", "{target}"]
);
case!(
    tag_list_missing_bucket_text,
    text,
    bare(),
    ["tag", "list", "{target}"]
);
case!(
    tag_remove_missing_bucket_text,
    text,
    bare(),
    ["tag", "remove", "{target}"]
);
case!(
    version_enable_missing_bucket_text,
    text,
    bare(),
    ["version", "enable", "{target}"]
);
case!(
    version_enable_text,
    text,
    empty(),
    ["version", "enable", "{target}"]
);
case!(
    version_enable_json,
    json,
    empty(),
    ["version", "enable", "{target}"]
);
case!(
    version_info_text,
    text,
    versioned(),
    ["version", "info", "{target}"]
);
case!(
    version_info_json,
    json,
    versioned(),
    ["version", "info", "{target}"]
);
case!(
    version_suspend_text,
    text,
    versioned(),
    ["version", "suspend", "{target}"]
);
case!(
    version_suspend_json,
    json,
    versioned(),
    ["version", "suspend", "{target}"]
);
case!(
    version_info_unversioned_text,
    text,
    empty(),
    ["version", "info", "{target}"]
);

/// Bucket with a `download` policy. MinIO returns policy `Action` sets in random order, so
/// action lists are masked (both tools echo the server document).
fn public() -> Option<Parity> {
    let mut p = with_setup(empty(), &["anonymous", "set", "download", "{target}"])?;
    p.normalizer.rule(
        r#""Action":(\s*)\[[^\]]*\]"#,
        r#""Action":${1}["<ACTIONS>"]"#,
    );
    Some(p)
}

case!(
    anonymous_set_text,
    text,
    empty(),
    ["anonymous", "set", "download", "{target}"]
);
case!(
    anonymous_set_json,
    json,
    empty(),
    ["anonymous", "set", "download", "{target}"]
);
case!(
    anonymous_get_text,
    text,
    public(),
    ["anonymous", "get", "{target}"]
);
case!(
    anonymous_get_json,
    json,
    public(),
    ["anonymous", "get", "{target}"]
);
case!(
    anonymous_get_private_text,
    text,
    empty(),
    ["anonymous", "get", "{target}"]
);

fn public_prefix() -> Option<Parity> {
    with_setup(empty(), &["anonymous", "set", "download", "{target}/pub/"])
}

case!(
    anonymous_list_text,
    text,
    public_prefix(),
    ["anonymous", "list", "{target}"]
);
case!(
    anonymous_list_json,
    json,
    public_prefix(),
    ["anonymous", "list", "{target}"]
);
case!(
    anonymous_list_missing_bucket_text,
    text,
    bare(),
    ["anonymous", "list", "{target}"]
);
case!(
    anonymous_set_prefix_text,
    text,
    empty(),
    ["anonymous", "set", "upload", "{target}/drop/"]
);

// ---------------------------------------------------------------------------
// cors / encrypt / event
// ---------------------------------------------------------------------------

case!(
    cors_get_missing_text,
    text,
    empty(),
    ["cors", "get", "{target}"]
);
case!(
    cors_get_missing_json,
    json,
    empty(),
    ["cors", "get", "{target}"]
);
case!(
    encrypt_info_missing_text,
    text,
    empty(),
    ["encrypt", "info", "{target}"]
);
case!(
    encrypt_info_missing_json,
    json,
    empty(),
    ["encrypt", "info", "{target}"]
);

fn encrypted() -> Option<Parity> {
    with_setup(empty(), &["encrypt", "set", "sse-s3", "{target}"])
}

case!(
    encrypt_set_text,
    text,
    empty(),
    ["encrypt", "set", "sse-s3", "{target}"]
);
case!(
    encrypt_set_json,
    json,
    empty(),
    ["encrypt", "set", "sse-s3", "{target}"]
);
case!(
    encrypt_info_text,
    text,
    encrypted(),
    ["encrypt", "info", "{target}"]
);
case!(
    encrypt_info_json,
    json,
    encrypted(),
    ["encrypt", "info", "{target}"]
);
case!(
    event_ls_empty_text,
    text,
    empty(),
    ["event", "ls", "{target}"]
);
case!(
    event_ls_empty_json,
    json,
    empty(),
    ["event", "ls", "{target}"]
);

/// Bucket with a `put` event on the webhook target of tests/minio.env (MX_TEST_NOTIFY_ARN).
fn with_event() -> Option<Parity> {
    let Ok(arn) = std::env::var("MX_TEST_NOTIFY_ARN") else {
        eprintln!("skipping; set MX_TEST_NOTIFY_ARN");
        return None;
    };
    with_setup(
        empty(),
        &[
            "event", "add", "{target}", &arn, "--event", "put", "--suffix", ".jpg",
        ],
    )
}

case!(
    event_ls_text,
    text,
    with_event(),
    ["event", "ls", "{target}"]
);
case!(
    event_ls_json,
    json,
    with_event(),
    ["event", "ls", "{target}"]
);

fn with_rule() -> Option<Parity> {
    with_setup(
        empty(),
        &[
            "ilm",
            "rule",
            "add",
            "--expire-days",
            "30",
            "--prefix",
            "logs/",
            "{target}",
        ],
    )
}

case!(
    ilm_rule_add_text,
    text,
    empty(),
    ["ilm", "rule", "add", "--expire-days", "30", "{target}"]
);
case!(
    ilm_rule_add_json,
    json,
    empty(),
    ["ilm", "rule", "add", "--expire-days", "30", "{target}"]
);
case!(
    ilm_rule_ls_text,
    text,
    with_rule(),
    ["ilm", "rule", "ls", "{target}"]
);
case!(
    ilm_rule_ls_json,
    json,
    with_rule(),
    ["ilm", "rule", "ls", "{target}"]
);

fn with_noncurrent_rule() -> Option<Parity> {
    with_setup(
        with_rule(),
        &[
            "ilm",
            "rule",
            "add",
            "--noncurrent-expire-days",
            "5",
            "--noncurrent-expire-newer",
            "3",
            "{target}",
        ],
    )
}

case!(
    ilm_rule_ls_noncurrent_text,
    text,
    with_noncurrent_rule(),
    ["ilm", "rule", "ls", "{target}"]
);
case!(
    ilm_rule_ls_noncurrent_json,
    json,
    with_noncurrent_rule(),
    ["ilm", "rule", "ls", "{target}"]
);
case!(
    ilm_rule_ls_missing_text,
    text,
    empty(),
    ["ilm", "rule", "ls", "{target}"]
);

// ---------------------------------------------------------------------------
// retention / legalhold (object lock)
// ---------------------------------------------------------------------------

case!(
    retention_set_default_text,
    text,
    locked(),
    [
        "retention",
        "set",
        "--default",
        "GOVERNANCE",
        "30d",
        "{target}"
    ]
);
case!(
    retention_set_default_json,
    json,
    locked(),
    [
        "retention",
        "set",
        "--default",
        "GOVERNANCE",
        "30d",
        "{target}"
    ]
);
case!(
    retention_info_default_text,
    text,
    locked(),
    ["retention", "info", "--default", "{target}"]
);
case!(
    retention_info_default_json,
    json,
    locked(),
    ["retention", "info", "--default", "{target}"]
);
case!(
    retention_set_object_text,
    text,
    locked(),
    ["retention", "set", "GOVERNANCE", "1d", "{target}/a.txt"]
);
case!(
    retention_set_object_json,
    json,
    locked(),
    ["retention", "set", "GOVERNANCE", "1d", "{target}/a.txt"]
);

fn retained() -> Option<Parity> {
    with_setup(
        locked(),
        &["retention", "set", "GOVERNANCE", "1d", "{target}/a.txt"],
    )
}

case!(
    retention_info_object_text,
    text,
    retained(),
    ["retention", "info", "{target}/a.txt"]
);
case!(
    retention_info_object_json,
    json,
    retained(),
    ["retention", "info", "{target}/a.txt"]
);
case!(
    retention_info_recursive_text,
    text,
    retained(),
    ["retention", "info", "-r", "{target}"]
);
case!(
    retention_info_recursive_json,
    json,
    retained(),
    ["retention", "info", "-r", "{target}"]
);
case!(
    retention_clear_object_text,
    text,
    retained(),
    ["retention", "clear", "{target}/a.txt"]
);
case!(
    retention_info_missing_text,
    text,
    locked(),
    ["retention", "info", "{target}/nope.txt"]
);
case!(
    retention_info_missing_json,
    json,
    locked(),
    ["retention", "info", "{target}/nope.txt"]
);
case!(
    retention_set_default_missing_bucket_text,
    text,
    bare(),
    [
        "retention",
        "set",
        "--default",
        "GOVERNANCE",
        "30d",
        "{target}"
    ]
);
case!(
    legalhold_info_missing_text,
    text,
    locked(),
    ["legalhold", "info", "{target}/nope.txt"]
);
case!(
    legalhold_info_missing_json,
    json,
    locked(),
    ["legalhold", "info", "{target}/nope.txt"]
);
case!(
    legalhold_set_missing_text,
    text,
    locked(),
    ["legalhold", "set", "{target}/nope.txt"]
);
case!(
    legalhold_info_missing_bucket_text,
    text,
    bare(),
    ["legalhold", "info", "{target}/a.txt"]
);
case!(
    legalhold_set_text,
    text,
    locked(),
    ["legalhold", "set", "{target}/a.txt"]
);
case!(
    legalhold_set_json,
    json,
    locked(),
    ["legalhold", "set", "{target}/a.txt"]
);
case!(
    legalhold_info_text,
    text,
    locked(),
    ["legalhold", "info", "{target}/a.txt"]
);
case!(
    legalhold_info_json,
    json,
    locked(),
    ["legalhold", "info", "{target}/a.txt"]
);

// ---------------------------------------------------------------------------
// alias
// ---------------------------------------------------------------------------

case!(alias_list_text, text, bare(), ["alias", "list"]);
case!(alias_list_json, json, bare(), ["alias", "list"]);
case!(
    alias_list_one_text,
    text,
    bare(),
    ["alias", "list", "{alias}"]
);
case!(
    alias_list_one_json,
    json,
    bare(),
    ["alias", "list", "{alias}"]
);
case!(
    alias_list_missing_text,
    text,
    bare(),
    ["alias", "list", "nosuch"]
);
case!(
    alias_set_text,
    text,
    bare(),
    [
        "alias",
        "set",
        "extra",
        "{url}",
        "{access_key}",
        "{secret_key}"
    ]
);
case!(
    alias_set_json,
    json,
    bare(),
    [
        "alias",
        "set",
        "extra",
        "{url}",
        "{access_key}",
        "{secret_key}"
    ]
);
case!(
    alias_set_unreachable_text,
    text,
    probing(bare()),
    [
        "alias",
        "set",
        "extra",
        "http://127.0.0.1:1",
        "akey12",
        "sk12345678"
    ]
);
case!(alias_remove_text, text, bare(), ["alias", "remove", "gcs"]);
case!(
    alias_set_invalid_key_text,
    text,
    bare(),
    [
        "alias",
        "set",
        "extra",
        "http://127.0.0.1:1",
        "ak",
        "sk12345678"
    ]
);
case!(alias_remove_json, json, bare(), ["alias", "remove", "gcs"]);
case!(
    alias_remove_missing_text,
    text,
    bare(),
    ["alias", "remove", "nosuch"]
);

case!(
    alias_export_text,
    text,
    bare(),
    ["alias", "export", "{alias}"]
);
case!(
    alias_export_json,
    json,
    bare(),
    ["alias", "export", "{alias}"]
);
case!(
    alias_export_missing_text,
    text,
    bare(),
    ["alias", "export", "nosuch"]
);
case!(
    alias_import_empty_text,
    text,
    bare(),
    ["alias", "import", "extra"],
    b""
);

/// Credentials document as `mc alias export` prints it, for `alias import` on stdin.
fn import_doc() -> Vec<u8> {
    format!(
        r#"{{"url":"{}","accessKey":"{}","secretKey":"{}","api":"s3v4","path":"auto"}}"#,
        std::env::var("MX_TEST_URL").unwrap_or_default(),
        std::env::var("MX_TEST_ACCESS_KEY").unwrap_or_default(),
        std::env::var("MX_TEST_SECRET_KEY").unwrap_or_default(),
    )
    .into_bytes()
}

#[test]
fn alias_import_text() {
    let Some(p) = bare() else { return };
    p.assert_parity(&["alias", "import", "extra"], Some(&import_doc()));
    p.assert_parity(&["alias", "list", "extra"], None);
}

#[test]
fn alias_import_json() {
    let Some(p) = bare() else { return };
    p.assert_json_parity(&["--json", "alias", "import", "extra"], Some(&import_doc()));
}

// ---------------------------------------------------------------------------
// ping / ready
// ---------------------------------------------------------------------------

/// `ping` prints response times padded to 8 columns (`0.51ms  `, `25.16ms `) and nanosecond
/// stats; the standard rules have already turned the times into `<DURATION>`.
fn pinged(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.normalizer
        .rule(r"time=<DURATION> *", "time=<PING>")
        .rule(r"<DURATION> +│", "<PING> │")
        .rule(r#""time":\s*"[^"]*""#, r#""time":"<PING>""#)
        .rule(r#"("(?:min|max|sum|avg|dns)":\s*)\d+"#, "${1}0");
    Some(p)
}

case!(
    ping_text,
    text,
    pinged(bare()),
    ["ping", "-c", "1", "{alias}"]
);
case!(
    ping_json,
    json,
    pinged(bare()),
    ["ping", "-c", "1", "{alias}"]
);
case!(
    ping_distributed_text,
    text,
    pinged(bare()),
    ["ping", "-c", "1", "-a", "{alias}"]
);
case!(
    ping_distributed_json,
    json,
    pinged(bare()),
    ["ping", "-c", "1", "-a", "{alias}"]
);
case!(
    ping_node_missing_text,
    text,
    pinged(bare()),
    ["ping", "-c", "1", "--node", "nosuch:9000", "{alias}"]
);
case!(ready_text, text, bare(), ["ready", "{alias}"]);
case!(ready_json, json, bare(), ["ready", "{alias}"]);

// ---------------------------------------------------------------------------
// mirror
// ---------------------------------------------------------------------------

case!(
    mirror_upload_text,
    text,
    mirrored(transfer(seeded())),
    ["mirror", "src", "{target}/mirror"]
);
case!(
    mirror_upload_json,
    json,
    mirrored(seeded()),
    ["mirror", "src", "{target}/mirror"]
);
case!(
    mirror_download_json,
    json,
    mirrored(seeded()),
    ["mirror", "{target}/dir", "mirrored"]
);

case!(
    mirror_dry_run_text,
    text,
    transfer(seeded()),
    ["mirror", "--dry-run", "src", "{target}/mirror"]
);
case!(
    mirror_dry_run_json,
    json,
    seeded(),
    ["mirror", "--dry-run", "src", "{target}/mirror"]
);
case!(
    mirror_summary_text,
    text,
    transfer(seeded()),
    ["mirror", "--summary", "src", "{target}/mirror"]
);

/// `src/` mirrored to `{target}/mirror`, then `src/one.txt` removed and `src/nested/two.txt`
/// changed locally.
fn mirrored_then_changed() -> Option<Parity> {
    let p = with_setup(seeded(), &["mirror", "src", "{target}/mirror"])?;
    for side in [&p.mc, &p.mx] {
        std::fs::remove_file(side.path("src/one.txt")).expect("rm");
    }
    p.file("src/nested/two.txt", "two two two\n");
    Some(p)
}

case!(
    mirror_remove_text,
    text,
    transfer(mirrored_then_changed()),
    [
        "mirror",
        "--remove",
        "--overwrite",
        "src",
        "{target}/mirror"
    ]
);
case!(
    mirror_remove_json,
    json,
    mirrored(mirrored_then_changed()),
    [
        "mirror",
        "--remove",
        "--overwrite",
        "src",
        "{target}/mirror"
    ]
);
case!(
    mirror_overwrite_conflict_text,
    text,
    transfer(mirrored_then_changed()),
    ["mirror", "src", "{target}/mirror"]
);
case!(
    mirror_missing_bucket_text,
    text,
    transfer(seeded()),
    ["mirror", "src", "{target}-nope/mirror"]
);
case!(
    mirror_not_folder_text,
    text,
    seeded(),
    ["mirror", "{target}/a.txt", "out"]
);

// ---------------------------------------------------------------------------
// od / undo
// ---------------------------------------------------------------------------

/// `od` reports the elapsed time (text `Time:`/`Speed:` use the standard rules).
fn timed(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.normalizer.rule(r#"("elapsed":\s*)\d+"#, "${1}0");
    Some(p)
}

case!(
    od_upload_text,
    text,
    timed(seeded()),
    ["od", "if=src/one.txt", "of={target}/od.txt"]
);
case!(
    od_upload_json,
    json,
    timed(seeded()),
    ["od", "if=src/one.txt", "of={target}/od.txt"]
);
case!(
    od_download_text,
    text,
    timed(seeded()),
    ["od", "if={target}/dir/b.txt", "of=od.txt"]
);
case!(
    od_download_json,
    json,
    timed(seeded()),
    ["od", "if={target}/dir/b.txt", "of=od.txt"]
);
case!(
    od_missing_source_text,
    text,
    seeded(),
    ["od", "if=nope.txt", "of={target}/od.txt"]
);
case!(
    undo_upload_text,
    text,
    versioned(),
    ["undo", "{target}/a.txt"]
);
case!(
    undo_upload_json,
    json,
    versioned(),
    ["undo", "{target}/a.txt"]
);
case!(
    undo_delete_text,
    text,
    versioned(),
    ["undo", "{target}/dir/b.txt"]
);
case!(
    undo_recursive_json,
    json,
    versioned(),
    [
        "undo",
        "-r",
        "--force",
        "--dry-run",
        "--last",
        "5",
        "{target}"
    ]
);
case!(
    undo_missing_text,
    text,
    versioned(),
    ["undo", "{target}/nope.txt"]
);
case!(
    undo_usage_error_json,
    json,
    versioned(),
    ["undo", "-r", "{target}"]
);

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

case!(
    err_missing_alias_text,
    text,
    bare(),
    ["ls", "nosuch/bucket"]
);
case!(
    err_missing_alias_json,
    json,
    bare(),
    ["ls", "nosuch/bucket"]
);
case!(err_missing_bucket_text, text, bare(), ["ls", "{target}"]);
case!(err_missing_bucket_json, json, bare(), ["ls", "{target}"]);
case!(
    err_missing_object_stat_text,
    text,
    empty(),
    ["stat", "{target}/nope.txt"]
);
case!(
    err_missing_object_stat_json,
    json,
    empty(),
    ["stat", "{target}/nope.txt"]
);
case!(
    err_missing_object_rm_text,
    text,
    empty(),
    ["rm", "{target}/nope.txt"]
);
case!(
    err_missing_local_file_text,
    text,
    empty(),
    ["cp", "nope.txt", "{target}/"]
);
case!(
    err_invalid_flag_text,
    text,
    bare(),
    ["ls", "--bogus", "{alias}/"]
);
case!(
    err_invalid_flag_json,
    json,
    bare(),
    ["ls", "--bogus", "{alias}/"]
);
case!(err_unknown_command_text, text, bare(), ["bogus"]);

// ---------------------------------------------------------------------------
// quota / ilm tier / replicate (MinIO admin API)
// ---------------------------------------------------------------------------

fn quota_set() -> Option<Parity> {
    with_setup(empty(), &["quota", "set", "{target}", "--size", "64MiB"])
}

case!(
    quota_set_text,
    text,
    empty(),
    ["quota", "set", "{target}", "--size", "1GB"]
);
case!(
    quota_set_json,
    json,
    empty(),
    ["quota", "set", "{target}", "--size", "64MiB"]
);
case!(
    quota_info_text,
    text,
    quota_set(),
    ["quota", "info", "{target}"]
);
case!(
    quota_info_json,
    json,
    quota_set(),
    ["quota", "info", "{target}"]
);
case!(
    quota_info_unset_text,
    text,
    empty(),
    ["quota", "info", "{target}"]
);
case!(
    quota_info_unset_json,
    json,
    empty(),
    ["quota", "info", "{target}"]
);
case!(
    quota_clear_text,
    text,
    quota_set(),
    ["quota", "clear", "{target}"]
);
case!(
    quota_clear_json,
    json,
    quota_set(),
    ["quota", "clear", "{target}"]
);
case!(
    quota_set_missing_size_text,
    text,
    empty(),
    ["quota", "set", "{target}"]
);
case!(
    quota_set_bad_size_json,
    json,
    empty(),
    ["quota", "set", "{target}", "--size", "abc"]
);
case!(
    quota_missing_bucket_text,
    text,
    bare(),
    ["quota", "info", "{target}"]
);
case!(
    quota_missing_bucket_json,
    json,
    bare(),
    ["quota", "info", "{target}"]
);
case!(
    quota_missing_alias_text,
    text,
    bare(),
    ["quota", "info", "nosuch/bucket"]
);

/// Configures the second server (`{alias2}`); None when it is not available.
fn second(p: Option<Parity>) -> Option<Parity> {
    let mut p = p?;
    p.second_server().then_some(p)
}

/// Upper-cased fixture names (tier names) and shared `{base}` names.
fn shared_names(p: &mut Parity) {
    let prefix = regex::escape(&common::live::bucket_prefix());
    p.normalizer
        .rule(&format!(r"(?i)\b{prefix}-p\d+(?:-m[cx])?\b"), "<BASE>");
}

/// Minio tier `{BASE}` on server 1 (shared by both sides), backed by bucket `{base}` on
/// server 2.
fn tiered() -> Option<Parity> {
    let mut p = second(bare())?;
    p.setup_once(&["mb", "{alias2}/{base}"]);
    p.setup_once(&[
        "ilm",
        "tier",
        "add",
        "minio",
        "{alias}",
        "{BASE}",
        "--endpoint",
        "{endpoint2}",
        "--access-key",
        "{access_key2}",
        "--secret-key",
        "{secret_key2}",
        "--bucket",
        "{base}",
        "--prefix",
        "p/",
    ]);
    p.cleanup_on_drop(&["ilm", "tier", "rm", "{alias}", "{BASE}"]);
    shared_names(&mut p);
    Some(p)
}

/// Bucket `{bucket}` on server 2 per side for `ilm tier add`; the tiers are removed on drop.
fn tier_target() -> Option<Parity> {
    let mut p = second(bare())?;
    p.setup(&["mb", "{alias2}/{bucket}"]);
    p.cleanup_on_drop(&["ilm", "tier", "rm", "{alias}", "{BUCKET}"]);
    shared_names(&mut p);
    Some(p)
}

/// A (fake) GCS credentials file `creds.json` in each work dir.
fn gcs_creds() -> Option<Parity> {
    let p = bare()?;
    p.file(
        "creds.json",
        r#"{"type":"service_account","project_id":"mx-parity"}"#,
    );
    Some(p)
}

macro_rules! tier_add_minio {
    ($name:ident, $mode:ident) => {
        case!(
            $name,
            $mode,
            tier_target(),
            [
                "ilm",
                "tier",
                "add",
                "minio",
                "{alias}",
                "{bucket}",
                "--endpoint",
                "{endpoint2}",
                "--access-key",
                "{access_key2}",
                "--secret-key",
                "{secret_key2}",
                "--bucket",
                "{bucket}",
                "--prefix",
                "p/"
            ]
        );
    };
}

tier_add_minio!(ilm_tier_add_minio_text, text);
tier_add_minio!(ilm_tier_add_minio_json, json);
case!(
    ilm_tier_add_s3_json,
    json,
    tier_target(),
    [
        "ilm",
        "tier",
        "add",
        "s3",
        "{alias}",
        "{bucket}",
        "--endpoint",
        "{endpoint2}",
        "--access-key",
        "{access_key2}",
        "--secret-key",
        "{secret_key2}",
        "--bucket",
        "{bucket}",
        "--storage-class",
        "STANDARD",
        "--region",
        "us-east-1"
    ]
);
// Azure and GCS need real cloud accounts: these check that MinIO receives, decrypts and
// validates the request and that the errors match mc's.
case!(
    ilm_tier_add_gcs_text,
    text,
    gcs_creds(),
    [
        "ilm",
        "tier",
        "add",
        "gcs",
        "{alias}",
        "gcstier",
        "--credentials-file",
        "creds.json",
        "--bucket",
        "gcsbucket",
        "--prefix",
        "p/"
    ]
);
case!(
    ilm_tier_add_gcs_json,
    json,
    gcs_creds(),
    [
        "ilm",
        "tier",
        "add",
        "gcs",
        "{alias}",
        "gcstier",
        "--credentials-file",
        "creds.json",
        "--bucket",
        "gcsbucket"
    ]
);
case!(
    ilm_tier_add_azure_unreachable_text,
    text,
    bare(),
    [
        "ilm",
        "tier",
        "add",
        "azure",
        "{alias}",
        "aztier",
        "--account-name",
        "account",
        "--account-key",
        "a2V5",
        "--bucket",
        "container",
        "--endpoint",
        "http://127.0.0.1:1"
    ]
);
case!(
    ilm_tier_add_azure_no_account_text,
    text,
    bare(),
    [
        "ilm",
        "tier",
        "add",
        "azure",
        "{alias}",
        "aztier",
        "--account-key",
        "a2V5",
        "--bucket",
        "container"
    ]
);
case!(
    ilm_tier_add_azure_no_credentials_json,
    json,
    bare(),
    [
        "ilm",
        "tier",
        "add",
        "azure",
        "{alias}",
        "aztier",
        "--account-name",
        "account",
        "--az-sp-tenant-id",
        "tenant",
        "--bucket",
        "container"
    ]
);
case!(
    ilm_tier_add_gcs_missing_file_text,
    text,
    bare(),
    [
        "ilm",
        "tier",
        "add",
        "gcs",
        "{alias}",
        "gcstier",
        "--credentials-file",
        "nope.json",
        "--bucket",
        "gcsbucket"
    ]
);
case!(
    ilm_tier_add_bad_type_text,
    text,
    bare(),
    ["ilm", "tier", "add", "bogus", "{alias}", "t1"]
);
case!(
    ilm_tier_add_minio_no_creds_json,
    json,
    bare(),
    [
        "ilm",
        "tier",
        "add",
        "minio",
        "{alias}",
        "t1",
        "--endpoint",
        "http://127.0.0.1:1",
        "--bucket",
        "b1"
    ]
);
case!(
    ilm_tier_add_extra_arg_text,
    text,
    bare(),
    ["ilm", "tier", "add", "minio", "{alias}", "t1", "extra"]
);
case!(
    ilm_tier_edit_azure_missing_text,
    text,
    bare(),
    [
        "ilm",
        "tier",
        "edit",
        "{alias}",
        "NOSUCHTIER",
        "--account-key",
        "a2V5"
    ]
);
case!(
    ilm_tier_edit_gcs_missing_json,
    json,
    gcs_creds(),
    [
        "ilm",
        "tier",
        "edit",
        "{alias}",
        "NOSUCHTIER",
        "--credentials-file",
        "creds.json"
    ]
);
case!(
    ilm_tier_edit_no_creds_text,
    text,
    bare(),
    ["ilm", "tier", "edit", "{alias}", "NOSUCHTIER"]
);
case!(
    ilm_tier_ls_text,
    text,
    tiered(),
    ["ilm", "tier", "ls", "{alias}"]
);
case!(
    ilm_tier_ls_json,
    json,
    tiered(),
    ["ilm", "tier", "ls", "{alias}"]
);
case!(
    ilm_tier_ls_empty_text,
    text,
    second(bare()),
    ["ilm", "tier", "ls", "{alias2}"]
);
/// mc prints the "no tiers" note as text even with --json.
#[test]
fn ilm_tier_ls_empty_json() {
    let Some(p) = second(bare()) else { return };
    p.assert_parity(&["--json", "ilm", "tier", "ls", "{alias2}"], None);
}
case!(
    ilm_tier_info_text,
    text,
    tiered(),
    ["ilm", "tier", "info", "{alias}", "{BASE}"]
);
case!(
    ilm_tier_info_json,
    json,
    tiered(),
    ["ilm", "tier", "info", "{alias}"]
);
case!(
    ilm_tier_info_no_match_text,
    text,
    tiered(),
    ["ilm", "tier", "info", "{alias}", "NOSUCHTIER"]
);
case!(
    ilm_tier_info_name_json,
    json,
    tiered(),
    ["ilm", "tier", "info", "{alias}", "{BASE}"]
);
case!(
    ilm_tier_check_text,
    text,
    tiered(),
    ["ilm", "tier", "check", "{alias}", "{BASE}"]
);
case!(
    ilm_tier_check_json,
    json,
    tiered(),
    ["ilm", "tier", "check", "{alias}", "{BASE}"]
);
case!(
    ilm_tier_verify_text,
    text,
    tiered(),
    ["ilm", "tier", "verify", "{alias}", "{BASE}"]
);
case!(
    ilm_tier_edit_text,
    text,
    tiered(),
    [
        "ilm",
        "tier",
        "edit",
        "{alias}",
        "{BASE}",
        "--access-key",
        "{access_key2}",
        "--secret-key",
        "{secret_key2}"
    ]
);
case!(
    ilm_tier_update_json,
    json,
    tiered(),
    [
        "ilm",
        "tier",
        "update",
        "{alias}",
        "{BASE}",
        "--access-key",
        "{access_key2}",
        "--secret-key",
        "{secret_key2}"
    ]
);
case!(
    ilm_tier_check_missing_text,
    text,
    bare(),
    ["ilm", "tier", "check", "{alias}", "NOSUCHTIER"]
);
case!(
    ilm_tier_check_missing_json,
    json,
    bare(),
    ["ilm", "tier", "check", "{alias}", "NOSUCHTIER"]
);
case!(
    ilm_tier_rm_text,
    text,
    tiered(),
    ["ilm", "tier", "rm", "{alias}", "{BASE}"]
);
case!(
    ilm_tier_rm_json,
    json,
    tiered(),
    ["ilm", "tier", "rm", "{alias}", "{BASE}"]
);
case!(
    ilm_tier_rm_force_text,
    text,
    bare(),
    ["ilm", "tier", "rm", "--force", "{alias}", "NOSUCHTIER"]
);
case!(
    ilm_tier_missing_alias_text,
    text,
    bare(),
    ["ilm", "tier", "ls", "nosuch"]
);

/// Replication `{target}` -> `{alias2}/{bucket}` (rule `r1`). Uptime, latencies and rates
/// vary between runs; the status table's padding follows the widest line, so trailing
/// spaces are dropped.
fn replicated_with(extra: &[&str]) -> Option<Parity> {
    let mut p = second(Parity::with(Opts {
        bucket: true,
        versioning: true,
        ..Opts::default()
    }))?;
    p.setup(&["mb", "--with-versioning", "{alias2}/{bucket}"]);
    let mut args = vec![
        "replicate",
        "add",
        "{target}",
        "--remote-bucket",
        "{remote2}/{bucket}",
        "--priority",
        "1",
        "--id",
        "r1",
    ];
    args.extend_from_slice(extra);
    p.setup(&args);
    p.normalizer
        .rule(r"(?m)^(  Replication status since ).*$", "${1}<UPTIME>")
        .rule(r"\| (?:now|a long while|\d+ [a-z]+) *\|", "| <UPTIME> |")
        .rule(
            r#""(uptime|curr|avg|max|avgRate|peakRate|currRate|totalDowntime|currentBandwidth)":-?[0-9][0-9.e+-]*"#,
            r#""$1":0"#,
        )
        .rule(r"(?m) +$", "");
    Some(p)
}

fn replicated() -> Option<Parity> {
    replicated_with(&[])
}

/// Synchronous replication with one replicated object (single-target status view).
fn replicated_object() -> Option<Parity> {
    let p = replicated_with(&["--sync"])?;
    p.object("a.txt", "alpha\n");
    Some(p)
}

case!(
    replicate_ls_text,
    text,
    replicated(),
    ["replicate", "ls", "{target}"]
);
case!(
    replicate_ls_json,
    json,
    replicated(),
    ["replicate", "ls", "{target}"]
);
case!(
    replicate_ls_unset_text,
    text,
    empty(),
    ["replicate", "ls", "{target}"]
);
case!(
    replicate_ls_unset_json,
    json,
    empty(),
    ["replicate", "ls", "{target}"]
);
case!(
    replicate_export_text,
    text,
    replicated(),
    ["replicate", "export", "{target}"]
);
case!(
    replicate_export_json,
    json,
    replicated(),
    ["replicate", "export", "{target}"]
);
case!(
    replicate_export_unset_text,
    text,
    empty(),
    ["replicate", "export", "{target}"]
);
case!(
    replicate_export_unset_json,
    json,
    empty(),
    ["replicate", "export", "{target}"]
);
case!(
    replicate_status_text,
    text,
    replicated(),
    ["replicate", "status", "{target}"]
);
case!(
    replicate_status_json,
    json,
    replicated(),
    ["replicate", "status", "{target}"]
);
case!(
    replicate_status_object_text,
    text,
    replicated_object(),
    ["replicate", "status", "{target}"]
);
case!(
    replicate_status_object_json,
    json,
    replicated_object(),
    ["replicate", "status", "{target}"]
);
case!(
    replicate_status_nodes_text,
    text,
    replicated(),
    ["replicate", "status", "--nodes", "{target}"]
);
case!(
    replicate_status_nodes_json,
    json,
    replicated(),
    ["replicate", "status", "--nodes", "{target}"]
);
case!(
    replicate_status_unset_text,
    text,
    empty(),
    ["replicate", "status", "{target}"]
);
case!(
    replicate_status_unset_json,
    json,
    empty(),
    ["replicate", "status", "{target}"]
);
// `replicate backlog` text is an interactive bubbletea view in mc (see COMPATIBILITY.md);
// only JSON is compared.
case!(
    replicate_backlog_json,
    json,
    replicated(),
    ["replicate", "backlog", "{target}"]
);
case!(
    replicate_backlog_full_json,
    json,
    replicated_object(),
    ["replicate", "backlog", "--full", "{target}"]
);
case!(
    replicate_backlog_missing_bucket_json,
    json,
    bare(),
    ["replicate", "backlog", "{target}"]
);
case!(
    replicate_backlog_full_missing_bucket_json,
    json,
    bare(),
    ["replicate", "backlog", "--full", "{target}"]
);
case!(
    replicate_backlog_no_bucket_text,
    text,
    bare(),
    ["replicate", "backlog", "{alias}"]
);

// ---------------------------------------------------------------------------
// --version
// ---------------------------------------------------------------------------

#[test]
fn version_shape() {
    let Some(mut p) = bare() else { return };
    // Compare the shape only: release tag, commit, runtime, copyright holder and license are
    // mx's own (intentional difference, see COMPATIBILITY.md).
    p.normalizer
        .rule(r"RELEASE\.[0-9TZ-]+", "<RELEASE>")
        .rule(r"commit-id=[0-9a-f]+", "commit-id=<COMMIT>")
        .rule(r"Runtime: .*", "Runtime: <RUNTIME>")
        .rule(r"Copyright \(c\) .*", "Copyright (c) <HOLDER>")
        .rule(r"License .*", "License <LICENSE>");
    p.assert_parity(&["--version"], None);
}
