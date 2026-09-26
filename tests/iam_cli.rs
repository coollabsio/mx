//! Offline tests for `admin user|group|policy|accesskey`: argument checks, and requests /
//! output against a tiny in-process HTTP server with canned (optionally madmin-encrypted)
//! responses.

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

const SECRET: &str = "skey1234";

fn mx(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("mx").expect("binary");
    cmd.env("HOME", home).current_dir(home);
    cmd
}

/// Recorded request: request line and body.
#[derive(Debug)]
struct Recorded {
    line: String,
    body: Vec<u8>,
}

impl Recorded {
    /// The request body decrypted like madmin `DecryptData`.
    fn decrypted(&self) -> String {
        let plain = mx::s3::admin::decrypt_response(SECRET, &self.body).expect("decrypt");
        String::from_utf8(plain).unwrap()
    }
}

/// A canned response; `encrypt` wraps the body with madmin `EncryptData`.
struct Reply {
    status: u16,
    body: Vec<u8>,
}

fn plain(status: u16, body: &str) -> Reply {
    Reply {
        status,
        body: body.as_bytes().to_vec(),
    }
}

fn encrypted(body: &str) -> Reply {
    Reply {
        status: 200,
        body: mx::s3::admin::encrypt_data(SECRET, body.as_bytes()).unwrap(),
    }
}

/// Serves one reply per connection, in order, and returns the recorded requests.
fn fake_server(replies: Vec<Reply>) -> (String, JoinHandle<Vec<Recorded>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut recorded = Vec::new();
        for reply in replies {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                let (name, value) = header.split_once(':').unwrap();
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();
            let mut stream = reader.into_inner();
            write!(
                stream,
                "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.status,
                reply.body.len()
            )
            .unwrap();
            stream.write_all(&reply.body).unwrap();
            stream.flush().unwrap();
            recorded.push(Recorded {
                line: line.trim_end().to_string(),
                body,
            });
        }
        recorded
    });
    (url, handle)
}

fn home_with_alias(url: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args(["alias", "set", "--api", "S3v4", "fake", url, "akey", SECRET])
        .assert()
        .success();
    home
}

/// An alias whose server is never contacted (validation errors come first).
fn offline_home() -> tempfile::TempDir {
    home_with_alias("http://127.0.0.1:1")
}

fn stdout(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

// ---------------------------------------------------------------------------
// Arguments and local validation
// ---------------------------------------------------------------------------

#[test]
fn missing_or_extra_arguments_show_help() {
    let home = offline_home();
    for args in [
        &["admin", "user", "info", "fake"][..],
        &["admin", "user", "info", "fake", "a", "b"],
        &["admin", "user", "add", "fake", "a", "b", "c"],
        &["admin", "user", "list"],
        &["admin", "user", "svcacct", "add", "fake"],
        &["admin", "group", "add", "fake", "g"],
        &["admin", "group", "list", "fake", "x"],
        &["admin", "policy", "attach", "fake"],
        &["admin", "policy", "create", "fake", "p"],
        &["admin", "accesskey", "create"],
        &["admin", "accesskey", "edit", "fake", "a", "b"],
        &["admin", "accesskey", "enable"],
        &["admin", "accesskey", "sts-revoke", "fake", "a", "b"],
    ] {
        mx(home.path())
            .args(args)
            .assert()
            .code(1)
            .stdout(predicate::str::contains("Usage"));
    }
}

#[test]
fn unknown_alias_fails_like_mc() {
    let home = offline_home();
    mx(home.path())
        .args(["admin", "user", "list", "nosuch"])
        .assert()
        .code(1)
        .stderr(
            "mx: <ERROR> Unable to initialize admin connection. No valid configuration found for 'nosuch' host alias.\n",
        );
    // `admin policy info` has no period in its message.
    mx(home.path())
        .args(["admin", "policy", "info", "nosuch", "p"])
        .assert()
        .code(1)
        .stderr(
            "mx: <ERROR> Unable to initialize admin connection. No valid configuration found for 'nosuch' host alias.\n",
        );
    let out = stdout(
        mx(home.path())
            .args(["--json", "admin", "policy", "info", "nosuch", "p"])
            .assert()
            .code(1),
    );
    assert!(
        out.contains(r#""message":"Unable to initialize admin connection""#),
        "{out}"
    );
}

#[test]
fn accesskey_flag_errors() {
    let home = offline_home();
    let cases: &[(&[&str], &str)] = &[
        (
            &["admin", "accesskey", "list", "fake", "--self", "--all"],
            "mx: <ERROR> Invalid flags. only one of --self or --all can be specified.\n",
        ),
        (
            &[
                "admin",
                "accesskey",
                "list",
                "fake",
                "--users-only",
                "--temp-only",
            ],
            "mx: <ERROR> Invalid flags. only one of --users-only, --temp-only, or --permanent-only can be specified.\n",
        ),
        (
            &["admin", "accesskey", "list", "fake", "--all", "u1"],
            "mx: <ERROR> Invalid flags. users cannot be specified with --self or --all.\n",
        ),
        (
            &["admin", "accesskey", "edit", "fake", "k1"],
            "mx: <ERROR> invalid flags. At least one property must be edited.\n",
        ),
        (
            &[
                "admin",
                "accesskey",
                "edit",
                "fake",
                "k1",
                "--expiry-duration",
                "bogus",
            ],
            "mx: <ERROR> invalid flags. At least one property must be edited.\n",
        ),
        (
            &[
                "admin",
                "accesskey",
                "create",
                "fake",
                "--expiry",
                "2030-01-01",
                "--expiry-duration",
                "1h",
            ],
            "mx: <ERROR> invalid flags. Only one of --expiry or --expiry-duration can be specified.\n",
        ),
        (
            &["admin", "accesskey", "create", "fake", "--expiry", "soon"],
            "mx: <ERROR> unable to parse the expiry argument: invalid expiry date format 'soon'.\n",
        ),
        (
            &["admin", "accesskey", "create", "fake", "--name", "1bad"],
            "mx: <ERROR> Unable to add service account. name must contain only ASCII letters, digits, underscores and hyphens and must start with a letter.\n",
        ),
        (
            &["admin", "accesskey", "sts-revoke", "fake"],
            "mx: <ERROR> Must specify user or use --self flag. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n",
        ),
        (
            &["admin", "accesskey", "sts-revoke", "fake", "u1", "--self"],
            "mx: <ERROR> Cannot specify user with --self flag. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n",
        ),
        (
            &["admin", "accesskey", "sts-revoke", "fake", "u1"],
            "mx: <ERROR> Exactly one of --all or --token-type must be specified. \n",
        ),
        (
            &[
                "admin", "user", "svcacct", "add", "fake", "u1", "--expiry", "x",
            ],
            "mx: <ERROR> unable to parse the expiry argument: expiry argument is not matching any of the supported patterns.\n",
        ),
        (
            &["admin", "policy", "attach", "fake", "p1"],
            "mx: <ERROR> Unable to make user/group policy association: no user or group association was given.\n",
        ),
    ];
    for (args, stderr) in cases {
        mx(home.path()).args(*args).assert().code(1).stderr(*stderr);
    }
}

#[test]
fn local_policy_files_are_checked() {
    let home = offline_home();
    std::fs::write(home.path().join("bad.json"), "xx").unwrap();
    std::fs::write(
        home.path().join("empty.json"),
        r#"{"Version":"2012-10-17","Statement":[]}"#,
    )
    .unwrap();
    mx(home.path())
        .args(["admin", "policy", "create", "fake", "p1", "/nonexistent.json"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> Unable to get policy: open /nonexistent.json: no such file or directory.\n");
    let out = stdout(
        mx(home.path())
            .args([
                "--json",
                "admin",
                "policy",
                "create",
                "fake",
                "p1",
                "/nonexistent.json",
            ])
            .assert()
            .code(1),
    );
    assert!(
        out.contains(r#""error":{"Op":"open","Path":"/nonexistent.json","Err":2}"#),
        "{out}"
    );
    mx(home.path())
        .args(["admin", "user", "svcacct", "add", "fake", "u1", "--policy", "bad.json"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> unable to parse the policy document: invalid character 'x' looking for beginning of value.\n");
    mx(home.path())
        .args(["admin", "accesskey", "create", "fake", "--policy", "empty.json"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> empty policies are not allowed. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n");
}

// ---------------------------------------------------------------------------
// Requests and output
// ---------------------------------------------------------------------------

#[test]
fn user_add_reads_stdin_and_encrypts_request() {
    let (url, server) = fake_server(vec![plain(200, "")]);
    let home = home_with_alias(&url);
    let out = stdout(
        mx(home.path())
            .args(["--json", "admin", "user", "add", "fake"])
            .write_stdin("user1\r\nsecret12345\n")
            .assert()
            .success(),
    );
    assert_eq!(
        out,
        "{\"status\":\"success\",\"accessKey\":\"user1\",\"secretKey\":\"secret12345\",\"userStatus\":\"enabled\"}\n"
    );
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "PUT /minio/admin/v3/add-user?accessKey=user1 HTTP/1.1"
    );
    assert_eq!(
        requests[0].decrypted(),
        r#"{"secretKey":"secret12345","status":"enabled"}"#
    );
}

#[test]
fn user_list_and_info_resolve_groups() {
    let (url, server) = fake_server(vec![
        encrypted(
            r#"{"u2":{"status":"enabled"},"u1":{"policyName":"readonly","status":"disabled","memberOf":["g1"]}}"#,
        ),
        plain(
            200,
            r#"{"name":"g1","status":"enabled","members":["u1"],"policy":"p1,p2"}"#,
        ),
        plain(
            200,
            r#"{"policyName":"readonly","status":"enabled","memberOf":["g1"],"userAuthInfo":{"type":"builtin"}}"#,
        ),
        plain(
            200,
            r#"{"name":"g1","status":"enabled","members":["u1"],"policy":""}"#,
        ),
    ]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["admin", "user", "list", "fake"])
        .assert()
        .success()
        .stdout(
            "disabled   u1                    readonly            \nenabled    u2                                        \n",
        );
    mx(home.path())
        .args(["admin", "user", "info", "fake", "u1"])
        .assert()
        .success()
        .stdout(
            "AccessKey: u1\nStatus: enabled\nPolicyName: readonly\nMemberOf: [g1]\nAuthentication: builtin ()\n",
        );
    let requests = server.join().unwrap();
    assert_eq!(requests[0].line, "GET /minio/admin/v3/list-users HTTP/1.1");
    assert_eq!(
        requests[1].line,
        "GET /minio/admin/v3/group?group=g1 HTTP/1.1"
    );
    assert_eq!(
        requests[2].line,
        "GET /minio/admin/v3/user-info?accessKey=u1 HTTP/1.1"
    );
}

#[test]
fn user_remove_error_keeps_mc_format_verb_in_json() {
    let error = r#"{"Code":"XMinioAdminNoSuchUser","Message":"The specified user does not exist.","RequestID":"ABC"}"#;
    let (url, _server) = fake_server(vec![plain(404, error), plain(404, error)]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["admin", "user", "remove", "fake", "nouser"])
        .assert()
        .code(1)
        .stderr("mx: <ERROR> Unable to remove nouser. The specified user does not exist.\n");
    let out = stdout(
        mx(home.path())
            .args(["--json", "admin", "user", "remove", "fake", "nouser"])
            .assert()
            .code(1),
    );
    assert!(out.contains(r#""message":"Unable to remove %s""#), "{out}");
}

#[test]
fn group_add_sends_plain_json() {
    let (url, server) = fake_server(vec![plain(200, "")]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["admin", "group", "add", "fake", "g1", "u1", "u2"])
        .assert()
        .success()
        .stdout("Added members `u1,u2` to group `g1` successfully.\n");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "PUT /minio/admin/v3/update-group-members HTTP/1.1"
    );
    assert_eq!(
        String::from_utf8(requests[0].body.clone()).unwrap(),
        r#"{"group":"g1","members":["u1","u2"],"groupStatus":"","isRemove":false}"#
    );
}

#[test]
fn policy_create_info_and_old_server_attach() {
    let (url, server) = fake_server(vec![
        plain(200, ""),
        plain(
            200,
            r#"{"PolicyName":"p1","Policy":{"Version":"2012-10-17","Statement":[]},"CreateDate":"2026-01-02T03:04:05Z","UpdateDate":"2026-01-02T03:04:05Z"}"#,
        ),
        plain(204, ""),
    ]);
    let home = home_with_alias(&url);
    let policy = r#"{"Version":"2012-10-17","Statement":[]}"#;
    std::fs::write(home.path().join("p.json"), policy).unwrap();
    mx(home.path())
        .args(["admin", "policy", "create", "fake", "p1", "p.json"])
        .assert()
        .success()
        .stdout("Created policy `p1` successfully.\n");
    mx(home.path())
        .args(["admin", "policy", "info", "fake", "p1"])
        .assert()
        .success()
        .stdout(
            "{\"PolicyName\":\"p1\",\"Policy\":{\"Version\":\"2012-10-17\",\"Statement\":[]},\"CreateDate\":\"2026-01-02T03:04:05Z\",\"UpdateDate\":\"2026-01-02T03:04:05Z\"}\n",
        );
    // Older servers answer 204 without a result: mc reports the requested policies.
    mx(home.path())
        .args([
            "admin", "policy", "attach", "fake", "p1", "p2", "--group", "g1",
        ])
        .assert()
        .success()
        .stdout("Attached Policies: [p1 p2]\nTo Group: g1\n");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "PUT /minio/admin/v3/add-canned-policy?name=p1 HTTP/1.1"
    );
    assert_eq!(String::from_utf8(requests[0].body.clone()).unwrap(), policy);
    assert_eq!(
        requests[1].line,
        "GET /minio/admin/v3/info-canned-policy?name=p1&v=2 HTTP/1.1"
    );
    assert_eq!(
        requests[2].line,
        "POST /minio/admin/v3/idp/builtin/policy/attach HTTP/1.1"
    );
    assert_eq!(
        requests[2].decrypted(),
        r#"{"policies":["p1","p2"],"group":"g1"}"#
    );
}

#[test]
fn accesskey_list_and_sts_revoke() {
    let (url, server) = fake_server(vec![
        encrypted(
            r#"{"u1":{"serviceAccounts":[{"parentUser":"u1","accountStatus":"on","impliedPolicy":true,"accessKey":"k1","expiration":"1970-01-01T00:00:00Z"}],"stsKeys":null}}"#,
        ),
        plain(204, ""),
    ]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["admin", "accesskey", "list", "fake", "u1", "--svcacc-only"])
        .assert()
        .success()
        .stdout("User: u1\n  Access Keys:\n    k1, expires: never, sts: false\n");
    mx(home.path())
        .args(["admin", "accesskey", "sts-revoke", "fake", "u1", "--all"])
        .assert()
        .success()
        .stdout("Successfully revoked all STS accounts for user u1\n");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "GET /minio/admin/v3/list-access-keys-bulk?listType=svcacc-only&users=u1 HTTP/1.1"
    );
    assert_eq!(
        requests[1].line,
        "POST /minio/admin/v3/revoke-tokens/builtin?tokenRevokeType=&user=u1&fullRevoke=true HTTP/1.1"
    );
}

#[test]
fn svcacct_add_sends_policy_and_prints_credentials() {
    let (url, server) = fake_server(vec![encrypted(
        r#"{"credentials":{"accessKey":"k1","secretKey":"s1","expiration":"1970-01-01T00:00:00Z"}}"#,
    )]);
    let home = home_with_alias(&url);
    std::fs::write(
        home.path().join("p.json"),
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"arn:aws:s3:::*"}]}"#,
    )
    .unwrap();
    mx(home.path())
        .args([
            "admin",
            "user",
            "svcacct",
            "add",
            "fake",
            "u1",
            "--access-key",
            "k1",
            "--secret-key",
            "s1",
            "--policy",
            "p.json",
            "--comment",
            "c1",
        ])
        .assert()
        .success()
        .stdout("Access Key: k1\nSecret Key: s1\nExpiration: no-expiry\n");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0].line,
        "PUT /minio/admin/v3/add-service-account HTTP/1.1"
    );
    assert_eq!(
        requests[0].decrypted(),
        r#"{"policy":{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"arn:aws:s3:::*"}]},"targetUser":"u1","accessKey":"k1","secretKey":"s1","description":"c1"}"#
    );
}
