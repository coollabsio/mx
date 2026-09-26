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

case!(
    #[ignore = "parity: mc ls line format `[DATE]  SIZE NAME/`"]
    ls_alias_root_text,
    text,
    seeded(),
    ["ls", "{alias}/"]
);
case!(
    #[ignore = "parity: JSON fields key/url/type=folder/versionOrdinal"]
    ls_alias_root_json,
    json,
    seeded(),
    ["ls", "{alias}/"]
);
case!(
    #[ignore = "parity: mc ls line format `[DATE]  SIZE CLASS NAME`"]
    ls_bucket_text,
    text,
    seeded(),
    ["ls", "{target}"]
);
case!(
    #[ignore = "parity: JSON fields key/url/type=file|folder/versionOrdinal, order"]
    ls_bucket_json,
    json,
    seeded(),
    ["ls", "{target}"]
);
case!(
    #[ignore = "parity: mc ls line format `[DATE]  SIZE CLASS NAME`"]
    ls_prefix_text,
    text,
    seeded(),
    ["ls", "{target}/dir/"]
);
case!(
    #[ignore = "parity: JSON fields key/url/type=file|folder/versionOrdinal, order"]
    ls_prefix_json,
    json,
    seeded(),
    ["ls", "{target}/dir/"]
);
case!(
    #[ignore = "parity: mc ls line format `[DATE]  SIZE CLASS NAME`"]
    ls_recursive_text,
    text,
    seeded(),
    ["ls", "-r", "{target}"]
);
case!(
    #[ignore = "parity: JSON fields key/url/type/versionOrdinal, unquoted etag"]
    ls_recursive_json,
    json,
    seeded(),
    ["ls", "-r", "{target}"]
);
case!(
    #[ignore = "parity: mc ls line format with version id"]
    ls_versions_text,
    text,
    versioned(),
    ["ls", "-r", "--versions", "{target}"]
);
case!(
    #[ignore = "parity: JSON fields key/url/type, unquoted etag"]
    ls_versions_json,
    json,
    versioned(),
    ["ls", "-r", "--versions", "{target}"]
);
case!(
    #[ignore = "parity: mc ls line format `[DATE]  SIZE CLASS NAME`"]
    ls_summarize_text,
    text,
    seeded(),
    ["ls", "-r", "--summarize", "{target}"]
);
case!(
    #[ignore = "parity: JSON fields key/url/type/versionOrdinal, unquoted etag"]
    ls_summarize_json,
    json,
    seeded(),
    ["ls", "-r", "--summarize", "{target}"]
);

// ---------------------------------------------------------------------------
// stat
// ---------------------------------------------------------------------------

case!(
    #[ignore = "parity: missing `Usage:` section"]
    stat_bucket_text,
    text,
    seeded(),
    ["stat", "{target}"]
);
case!(
    #[ignore = "parity: mc bucket stat JSON shape (Usage, ilm, notification, name=`b/`)"]
    stat_bucket_json,
    json,
    seeded(),
    ["stat", "{target}"]
);
case!(stat_object_text, text, seeded(), ["stat", "{target}/a.txt"]);
case!(
    #[ignore = "parity: extra fields bucket/contentType/eTag/key/target"]
    stat_object_json,
    json,
    seeded(),
    ["stat", "{target}/a.txt"]
);

// ---------------------------------------------------------------------------
// mb / rb
// ---------------------------------------------------------------------------

case!(
    #[ignore = "parity: message `Bucket created successfully `alias/bucket`.`"]
    mb_text,
    text,
    empty(),
    ["mb", "{target}-new"]
);
case!(
    #[ignore = "parity: JSON bucket=`alias/bucket`, region field, no target"]
    mb_json,
    json,
    empty(),
    ["mb", "{target}-new"]
);
case!(
    #[ignore = "parity: message `Bucket created successfully `alias/bucket`.`"]
    mb_existing_p_text,
    text,
    empty(),
    ["mb", "-p", "{target}"]
);
case!(
    #[ignore = "parity: JSON bucket=`alias/bucket`, region field, no target"]
    mb_existing_p_json,
    json,
    empty(),
    ["mb", "-p", "{target}"]
);
case!(mb_existing_error_text, text, empty(), ["mb", "{target}"]);
case!(mb_existing_error_json, json, empty(), ["mb", "{target}"]);
case!(
    #[ignore = "parity: message `Removed `alias/bucket` successfully.`"]
    rb_text,
    text,
    empty(),
    ["rb", "{target}"]
);
case!(
    #[ignore = "parity: JSON bucket=`alias/bucket`, no target"]
    rb_json,
    json,
    empty(),
    ["rb", "{target}"]
);
case!(rb_not_empty_text, text, seeded(), ["rb", "{target}"]);
case!(rb_not_empty_json, json, seeded(), ["rb", "{target}"]);

// ---------------------------------------------------------------------------
// rm
// ---------------------------------------------------------------------------

case!(rm_single_text, text, seeded(), ["rm", "{target}/a.txt"]);
case!(
    #[ignore = "parity: JSON pretty-printed / key order"]
    rm_single_json,
    json,
    seeded(),
    ["rm", "{target}/a.txt"]
);
case!(
    rm_recursive_text,
    text,
    seeded(),
    ["rm", "-r", "--force", "{target}/dir/"]
);
case!(
    #[ignore = "parity: JSON pretty-printed / key order"]
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
    #[ignore = "parity: JSON pretty-printed / key order"]
    rm_versions_json,
    json,
    versioned(),
    ["rm", "-r", "--force", "--versions", "{target}"]
);

// ---------------------------------------------------------------------------
// cp / mv / put / pipe
// ---------------------------------------------------------------------------

case!(
    #[ignore = "parity: local source printed as absolute path"]
    cp_upload_text,
    text,
    transfer(seeded()),
    ["cp", "src/one.txt", "{target}/up/one.txt"]
);
case!(
    #[ignore = "parity: local source is absolute path; totalSize 0"]
    cp_upload_json,
    json,
    seeded(),
    ["cp", "src/one.txt", "{target}/up/one.txt"]
);
case!(
    #[ignore = "parity: local source printed as absolute path"]
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
    #[ignore = "parity: JSON totalSize is 0 in mc"]
    cp_download_json,
    json,
    seeded(),
    ["cp", "{target}/a.txt", "out.txt"]
);
case!(
    #[ignore = "parity: local source printed as absolute path"]
    cp_recursive_upload_text,
    text,
    parallel(transfer(seeded())),
    ["cp", "-r", "src/", "{target}/up/"]
);
case!(
    #[ignore = "parity: local source is absolute path; totalSize 0"]
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
    #[ignore = "parity: JSON totalSize is 0 in mc"]
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
    #[ignore = "parity: JSON totalSize is 0 in mc"]
    cp_server_side_json,
    json,
    seeded(),
    ["cp", "{target}/a.txt", "{target}/copy.txt"]
);
case!(
    #[ignore = "parity: local source printed as absolute path"]
    mv_upload_text,
    text,
    transfer(seeded()),
    ["mv", "src/one.txt", "{target}/moved.txt"]
);
case!(
    #[ignore = "parity: local source is absolute path; totalSize 0"]
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
    #[ignore = "parity: JSON totalSize is 0 in mc"]
    mv_server_side_json,
    json,
    seeded(),
    ["mv", "{target}/a.txt", "{target}/moved.txt"]
);
case!(
    #[ignore = "parity: mc cp-style output (`SRC -> TGT` + summary table)"]
    put_text,
    text,
    transfer(seeded()),
    ["put", "src/one.txt", "{target}/put.txt"]
);
case!(
    #[ignore = "parity: mc cp-style JSON (size/totalCount/totalSize) + summary doc"]
    put_json,
    json,
    seeded(),
    ["put", "src/one.txt", "{target}/put.txt"]
);
case!(
    #[ignore = "parity: mc emits progress residue ` 0 B / ? ` before result"]
    pipe_text,
    text,
    transfer(seeded()),
    ["pipe", "{target}/piped.txt"],
    b"piped\n"
);
case!(
    #[ignore = "parity: key order; mc emits progress residue ` 0 B / ? `"]
    pipe_json,
    json,
    seeded(),
    ["pipe", "{target}/piped.txt"],
    b"piped\n"
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

case!(
    #[ignore = "parity: mc prints one total line `38B<TAB>3 objects<TAB>BUCKET`"]
    du_text,
    text,
    seeded(),
    ["du", "{target}"]
);
case!(
    #[ignore = "parity: mc prints one total doc with prefix=BUCKET"]
    du_json,
    json,
    seeded(),
    ["du", "{target}"]
);
case!(
    #[ignore = "parity: mc -d format/ordering (`SIZE<TAB>N objects<TAB>BUCKET/dir`)"]
    du_depth_text,
    text,
    seeded(),
    ["du", "-d", "2", "{target}"]
);
case!(
    #[ignore = "parity: mc tree glyphs `└─ dir`"]
    tree_text,
    text,
    seeded(),
    ["tree", "{target}"]
);
case!(
    #[ignore = "parity: mc tree glyphs `├─ a.txt`"]
    tree_files_text,
    text,
    seeded(),
    ["tree", "-f", "{target}"]
);
case!(
    #[ignore = "parity: --json unsupported; mc prints ls-style JSON docs"]
    tree_json,
    json,
    seeded(),
    ["tree", "-f", "{target}"]
);
case!(
    #[ignore = "parity: mc prints `alias/bucket/key`, mx relative keys"]
    find_name_text,
    text,
    seeded(),
    ["find", "{target}", "--name", "*.txt"]
);
case!(
    #[ignore = "parity: key is `alias/bucket/key`; missing etag/type"]
    find_name_json,
    json,
    seeded(),
    ["find", "{target}", "--name", "*.txt"]
);
case!(
    #[ignore = "parity: mc prints full paths (`<WORK>/src/one.txt`)"]
    find_local_text,
    text,
    seeded(),
    ["find", "src", "--name", "*.txt"]
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
    #[ignore = "parity: mc format `! URL` / `< URL` / `> URL` with full URLs"]
    diff_text,
    text,
    diffed(),
    ["diff", "{target}/left", "{target}/right"]
);
case!(
    #[ignore = "parity: --json not supported (text output)"]
    diff_json,
    json,
    diffed(),
    ["diff", "{target}/left", "{target}/right"]
);

// ---------------------------------------------------------------------------
// share
// ---------------------------------------------------------------------------

case!(
    #[ignore = "parity: presigned URL has extra `x-id=GetObject`"]
    share_download_json,
    json,
    seeded(),
    ["share", "download", "{target}/a.txt"]
);
case!(
    #[ignore = "parity: presigned URL has extra `x-id=GetObject`; trailing blank line"]
    share_download_text,
    text,
    seeded(),
    ["share", "download", "{target}/a.txt"]
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
    version_enable_text,
    text,
    empty(),
    ["version", "enable", "{target}"]
);
case!(
    #[ignore = "parity: JSON versioning.status is empty in mc"]
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
    #[ignore = "parity: table cell alignment (mc left-aligns ID, right-aligns days)"]
    ilm_rule_ls_text,
    text,
    with_rule(),
    ["ilm", "rule", "ls", "{target}"]
);
case!(
    #[ignore = "parity: JSON missing `updatedAt`"]
    ilm_rule_ls_json,
    json,
    with_rule(),
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
    #[ignore = "parity: JSON missing `error:null`; validity empty in mc"]
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
    #[ignore = "parity: extra trailing blank line"]
    retention_info_object_text,
    text,
    retained(),
    ["retention", "info", "{target}/a.txt"]
);
case!(
    #[ignore = "parity: JSON missing `error:null`"]
    retention_info_object_json,
    json,
    retained(),
    ["retention", "info", "{target}/a.txt"]
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
    #[ignore = "parity: status padding `[  Not set ]`"]
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

case!(
    #[ignore = "parity: table layout, mc prints per-alias block `name<NL>  URL       : ...`"]
    alias_list_text,
    text,
    bare(),
    ["alias", "list"]
);
case!(
    #[ignore = "parity: JSON pretty-printed, mc prints compact one-line docs"]
    alias_list_json,
    json,
    bare(),
    ["alias", "list"]
);
case!(
    #[ignore = "parity: table layout, mc prints per-alias block `name<NL>  URL       : ...`"]
    alias_list_one_text,
    text,
    bare(),
    ["alias", "list", "{alias}"]
);
case!(
    #[ignore = "parity: JSON pretty-printed, mc prints compact one-line docs"]
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
case!(
    #[ignore = "parity: JSON missing empty `URL` field"]
    alias_remove_json,
    json,
    bare(),
    ["alias", "remove", "gcs"]
);
case!(
    alias_remove_missing_text,
    text,
    bare(),
    ["alias", "remove", "nosuch"]
);

// ---------------------------------------------------------------------------
// mirror
// ---------------------------------------------------------------------------

case!(
    #[ignore = "parity: missing mc summary table"]
    mirror_upload_text,
    text,
    mirrored(transfer(seeded())),
    ["mirror", "src", "{target}/mirror"]
);
case!(
    #[ignore = "parity: missing summary doc {total,transferred,duration,speed}"]
    mirror_upload_json,
    json,
    mirrored(seeded()),
    ["mirror", "src", "{target}/mirror"]
);
case!(
    #[ignore = "parity: missing summary doc {total,transferred,duration,speed}"]
    mirror_download_json,
    json,
    mirrored(seeded()),
    ["mirror", "{target}/dir", "mirrored"]
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

#[ignore = "parity: mc prints 4-line version block"]
#[test]
fn version_shape() {
    let Some(mut p) = bare() else { return };
    // Compare the shape only: release tag, commit, runtime and copyright year vary.
    p.normalizer
        .rule(r"RELEASE\.[0-9TZ-]+", "<RELEASE>")
        .rule(r"commit-id=[0-9a-f]+", "commit-id=<COMMIT>")
        .rule(r"Runtime: .*", "Runtime: <RUNTIME>")
        .rule(r"2015-\d{4}", "2015-<YEAR>");
    p.assert_parity(&["--version"], None);
}
