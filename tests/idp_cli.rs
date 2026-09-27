//! Offline tests for `idp ldap` / `idp openid`: argument checks and mc's client-side errors,
//! plus the admin API requests (checked against a tiny in-process HTTP server).

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

const SECRET: &str = "skey1234";

fn mx(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("mx").expect("binary");
    cmd.env("HOME", home);
    cmd
}

fn run(args: &[&str]) -> assert_cmd::assert::Assert {
    let home = tempfile::tempdir().unwrap();
    mx(home.path()).args(args).assert()
}

// ---------------------------------------------------------------------------
// arguments
// ---------------------------------------------------------------------------

#[test]
fn wrong_argument_counts_print_help() {
    for args in [
        &["idp", "ldap", "add", "a"][..],
        &["idp", "ldap", "update", "a"],
        &["idp", "ldap", "list"],
        &["idp", "ldap", "info", "a", "b"],
        &["idp", "ldap", "enable", "a", "b"],
        &["idp", "ldap", "rm"],
        &["idp", "ldap", "policy", "attach", "a"],
        &["idp", "ldap", "policy", "detach", "a"],
        &["idp", "ldap", "policy", "entities", "a", "b"],
        &["idp", "ldap", "accesskey", "list"],
        &["idp", "ldap", "accesskey", "rm", "a"],
        &["idp", "ldap", "accesskey", "rm", "a", "b", "c"],
        &["idp", "ldap", "accesskey", "info", "a"],
        &["idp", "ldap", "accesskey", "create"],
        &["idp", "ldap", "accesskey", "create", "a", "b", "c"],
        &["idp", "ldap", "accesskey", "create-with-login"],
        &["idp", "ldap", "accesskey", "edit"],
        &["idp", "ldap", "accesskey", "enable", "a", "b", "c"],
        &["idp", "ldap", "accesskey", "disable"],
        &["idp", "ldap", "accesskey", "sts-revoke"],
        &["idp", "ldap", "accesskey", "sts-revoke", "a", "b", "c"],
        &["idp", "openid", "add", "a"],
        &["idp", "openid", "update"],
        &["idp", "openid", "remove", "a", "b", "c"],
        &["idp", "openid", "list", "a", "b"],
        &["idp", "openid", "info"],
        &["idp", "openid", "enable", "a", "b", "c"],
        &["idp", "openid", "disable"],
        &["idp", "openid", "accesskey", "ls"],
        &["idp", "openid", "accesskey", "info", "a"],
        &["idp", "openid", "accesskey", "edit"],
    ] {
        run(args)
            .code(1)
            .stdout(predicate::str::contains("USAGE:\n  mx idp "))
            .stderr("");
    }
}

#[test]
fn client_side_errors_match_mc() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["idp", "ldap", "accesskey", "create", "a", "--login"],
            "mx: <ERROR> Deprecated command. Please use 'mc idp ldap accesskey create-with-login' instead.\n",
        ),
        (
            &[
                "idp",
                "ldap",
                "accesskey",
                "create-with-login",
                "http://localhost:9000",
            ],
            "mx: <ERROR> unable to read from STDIN: login flag cannot be used with a non-interactive terminal.\n",
        ),
        (
            &["idp", "ldap", "accesskey", "sts-revoke", "a", "--all"],
            "mx: <ERROR> Must specify user or use --self flag. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n",
        ),
        (
            &["idp", "ldap", "accesskey", "sts-revoke", "a", "u", "--self"],
            "mx: <ERROR> Cannot specify user with --self flag. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n",
        ),
        (
            &["idp", "ldap", "accesskey", "sts-revoke", "a", "u"],
            "mx: <ERROR> Exactly one of --all or --token-type must be specified. \n",
        ),
        (
            &[
                "idp",
                "ldap",
                "accesskey",
                "sts-revoke",
                "a",
                "u",
                "--all",
                "--token-type",
                "t",
            ],
            "mx: <ERROR> Exactly one of --all or --token-type must be specified. \n",
        ),
        (
            &[
                "idp",
                "ldap",
                "accesskey",
                "list",
                "a",
                "--users-only",
                "--svcacc-only",
            ],
            "mx: <ERROR> Invalid flags. only one of --users-only, --temp-only, or --permanent-only can be specified.\n",
        ),
        (
            &["idp", "openid", "accesskey", "list", "a", "--self", "--all"],
            "mx: <ERROR> Invalid flags. only one of --self or --all can be specified.\n",
        ),
        (
            &["idp", "ldap", "accesskey", "list", "a", "u", "--all"],
            "mx: <ERROR> Invalid flags. users cannot be specified with --self or --all.\n",
        ),
        (
            &["idp", "ldap", "policy", "attach", "a", "p"],
            "mx: <ERROR> Invalid policy attach arguments. no user or group association was given.\n",
        ),
        (
            &[
                "idp", "ldap", "policy", "attach", "a", "p", "-u", "x", "-g", "y",
            ],
            "mx: <ERROR> Invalid policy attach arguments. either a group or a user association must be given, not both.\n",
        ),
        (
            &["idp", "ldap", "policy", "detach", "a", "p"],
            "mx: <ERROR> Missing flag in command: at least one of --user or --group is required.\n",
        ),
        (
            &["idp", "ldap", "accesskey", "edit", "a", "k"],
            "mx: <ERROR> invalid flags. At least one property must be edited.\n",
        ),
        (
            &[
                "idp",
                "openid",
                "accesskey",
                "edit",
                "a",
                "k",
                "--expiry-duration",
                "bogus",
            ],
            "mx: <ERROR> invalid flags. At least one property must be edited.\n",
        ),
        (
            &[
                "idp",
                "ldap",
                "accesskey",
                "edit",
                "a",
                "k",
                "--expiry",
                "2030-01-01",
                "--expiry-duration",
                "1h",
            ],
            "mx: <ERROR> invalid flags. Only one of --expiry or --expiry-duration can be specified.\n",
        ),
        (
            &[
                "idp",
                "ldap",
                "accesskey",
                "create",
                "a",
                "--expiry",
                "01/02/2030",
            ],
            "mx: <ERROR> unable to parse the expiry argument: invalid expiry date format '01/02/2030'.\n",
        ),
        (
            &[
                "idp",
                "ldap",
                "accesskey",
                "create",
                "a",
                "--policy",
                "/nonexistent/p.json",
            ],
            "mx: <ERROR> unable to read the policy document: open /nonexistent/p.json: no such file or directory.\n",
        ),
        (
            &["idp", "ldap", "info", "nosuch"],
            "mx: <ERROR> Unable to initialize admin connection. No valid configuration found for 'nosuch' host alias.\n",
        ),
    ];
    for (args, stderr) in cases {
        run(args).code(1).stdout("").stderr(*stderr);
    }
}

#[test]
fn json_errors_use_mc_templates() {
    run(&["--json", "idp", "ldap", "accesskey", "sts-revoke", "a", "u"])
        .code(1)
        .stdout(
            "{\"status\":\"error\",\"error\":{\"message\":\"Exactly one of --all or --token-type must be specified.\",\"cause\":{\"message\":\"\",\"error\":{}},\"type\":\"fatal\"}}\n",
        );
    run(&[
        "--json",
        "idp",
        "ldap",
        "accesskey",
        "create",
        "a",
        "--policy",
        "/nonexistent/p.json",
    ])
    .code(1)
    .stdout(predicate::str::contains(
        r#""error":{"Op":"open","Path":"/nonexistent/p.json","Err":2}"#,
    ));
}

// ---------------------------------------------------------------------------
// admin API requests
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Recorded {
    line: String,
    body: Vec<u8>,
}

impl Recorded {
    /// The request body decrypted like the server does (`madmin.EncryptData`).
    fn plain(&self) -> String {
        let plain = mx::s3::admin::decrypt_data(SECRET, &self.body, None).expect("encrypted");
        String::from_utf8(plain).unwrap()
    }
}

/// One canned reply: status, extra headers, body.
type Reply = (u16, &'static str, Vec<u8>);

fn encrypted(json: &str) -> Vec<u8> {
    mx::s3::admin::encrypt_data(SECRET, json.as_bytes()).unwrap()
}

/// Serves one canned reply per connection, in order, recording the requests.
fn fake_server(replies: Vec<Reply>) -> (String, JoinHandle<Vec<Recorded>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut recorded = Vec::new();
        for (status, headers, body) in replies {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut length = 0usize;
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
            let mut request_body = vec![0u8; length];
            reader.read_exact(&mut request_body).unwrap();
            let mut stream = reader.into_inner();
            write!(
                stream,
                "HTTP/1.1 {status} X\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
            stream.flush().unwrap();
            recorded.push(Recorded {
                line: line.trim_end().to_string(),
                body: request_body,
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

#[test]
fn config_add_update_send_encrypted_kv_and_report_restart() {
    let (url, server) = fake_server(vec![
        (200, "", Vec::new()),
        (200, "x-minio-config-applied: true\r\n", Vec::new()),
        (426, "", Vec::new()),
        (200, "", Vec::new()),
    ]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["idp", "ldap", "add", "fake/", "server_addr=ldap:389", "lookup_bind_dn=cn=admin,dc=min,dc=io"])
        .assert()
        .success()
        .stdout("Successfully applied new settings.\nPlease restart your server 'mc admin service restart fake/'.\n");
    mx(home.path())
        .args([
            "--json",
            "idp",
            "openid",
            "update",
            "fake",
            "dex",
            "scopes=openid",
        ])
        .assert()
        .success()
        .stdout("{\"status\":\"success\"}\n");
    // Older servers answer 426: madmin retries with the query form.
    mx(home.path())
        .args(["idp", "openid", "enable", "fake"])
        .assert()
        .success();
    let requests = server.join().unwrap();
    assert!(
        requests[0]
            .line
            .starts_with("PUT /minio/admin/v3/idp-config/ldap/_ "),
        "{}",
        requests[0].line
    );
    assert_eq!(
        requests[0].plain(),
        "server_addr=ldap:389 lookup_bind_dn=cn=admin,dc=min,dc=io"
    );
    assert!(
        requests[1]
            .line
            .starts_with("POST /minio/admin/v3/idp-config/openid/dex ")
    );
    assert_eq!(requests[1].plain(), "scopes=openid");
    assert!(
        requests[2]
            .line
            .starts_with("POST /minio/admin/v3/idp-config/openid/_ ")
    );
    assert!(
        requests[3]
            .line
            .starts_with("POST /minio/admin/v3/idp-config?name=_&type=openid "),
        "{}",
        requests[3].line
    );
    assert_eq!(requests[3].plain(), "enable=");
}

#[test]
fn config_list_and_info_render_mc_boxes() {
    let (url, server) = fake_server(vec![
        (
            200,
            "",
            encrypted(
                r#"[{"type":"openid","name":"_","enabled":true,"roleARN":"arn:minio:iam:::role/abc"},{"type":"openid","name":"dex","enabled":false}]"#,
            ),
        ),
        (
            200,
            "",
            encrypted(
                r#"{"type":"openid","name":"dex","info":[{"key":"client_id","value":"app","isCfg":true,"isEnv":true},{"key":"enable","value":"off","isCfg":true,"isEnv":false}]}"#,
            ),
        ),
        (
            200,
            "",
            encrypted(r#"{"type":"ldap","name":"_","info":[]}"#),
        ),
    ]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args(["idp", "openid", "ls", "fake"])
        .assert()
        .success()
        .stdout(
            "╭──────────────────────────────────────────╮\n\
             │ On?    Name             RoleARN          │\n\
             │ 🟢   (default)  arn:minio:iam:::role/abc │\n\
             │ 🔴         dex                           │\n\
             ╰──────────────────────────────────────────╯\n",
        );
    mx(home.path())
        .args(["idp", "openid", "info", "fake", "dex"])
        .assert()
        .success()
        .stdout(
            "╭─────────────────────────────╮\n\
             │   enable: off               │\n\
             │client_id: app  (environment)│\n\
             ╰─────────────────────────────╯\n",
        );
    mx(home.path())
        .args(["idp", "ldap", "info", "fake"])
        .assert()
        .success()
        .stdout("Not configured.\n");
    let requests = server.join().unwrap();
    assert!(
        requests[0]
            .line
            .starts_with("GET /minio/admin/v3/idp-config/openid ")
    );
    assert!(
        requests[1]
            .line
            .starts_with("GET /minio/admin/v3/idp-config/openid/dex ")
    );
    assert!(
        requests[2]
            .line
            .starts_with("GET /minio/admin/v3/idp-config/ldap/_ ")
    );
}

#[test]
fn accesskey_and_policy_requests() {
    let (url, server) = fake_server(vec![
        (204, "", Vec::new()),
        (204, "", Vec::new()),
        (
            200,
            "",
            encrypted(
                r#"{"timestamp":"2026-01-02T03:04:05.123Z","groupMappings":[{"group":"cn=g","policies":["p1","p2"]}]}"#,
            ),
        ),
        (
            200,
            "",
            encrypted(
                r#"{"credentials":{"accessKey":"AK","secretKey":"SK","expiration":"1970-01-01T00:00:00Z"}}"#,
            ),
        ),
    ]);
    let home = home_with_alias(&url);
    mx(home.path())
        .args([
            "idp",
            "ldap",
            "accesskey",
            "sts-revoke",
            "fake",
            "bob",
            "--all",
        ])
        .assert()
        .success()
        .stdout("Successfully revoked all STS accounts for user bob\n");
    mx(home.path())
        .args([
            "--json",
            "idp",
            "openid",
            "accesskey",
            "disable",
            "fake",
            "k1",
        ])
        .assert()
        .success()
        .stdout("{\"status\":\"success\",\"accessKey\":\"k1\"}\n");
    mx(home.path())
        .args(["idp", "ldap", "policy", "entities", "fake", "-u", "zed", "-u", "amy", "-g", "cn=g"])
        .assert()
        .success()
        .stdout(
            "Query time: 2026-01-02T03:04:05Z\nGroup -> Policy Mappings:\n  Group: cn=g\n    Policies:\n      p1\n      p2\n",
        );
    mx(home.path())
        .args([
            "idp",
            "ldap",
            "accesskey",
            "create",
            "fake",
            "bob",
            "--access-key",
            "AK",
            "--secret-key",
            "SK",
            "--name",
            "n",
        ])
        .assert()
        .success()
        .stdout("Access Key: AK\nSecret Key: SK\nExpiration: NONE\nName: n\nDescription: \n");
    let requests = server.join().unwrap();
    // mc revokes through the builtin provider endpoint.
    assert!(
        requests[0].line.starts_with(
            "POST /minio/admin/v3/revoke-tokens/builtin?fullRevoke=true&tokenRevokeType=&user=bob "
        ),
        "{}",
        requests[0].line
    );
    assert!(
        requests[1]
            .line
            .starts_with("POST /minio/admin/v3/update-service-account?accessKey=k1 ")
    );
    assert_eq!(requests[1].plain(), r#"{"newStatus":"off"}"#);
    // Repeated query values are sent sorted (SigV4 canonical order).
    assert!(
        requests[2].line.starts_with(
            "GET /minio/admin/v3/idp/ldap/policy-entities?group=cn%3Dg&user=amy&user=zed "
        ),
        "{}",
        requests[2].line
    );
    assert!(
        requests[3]
            .line
            .starts_with("PUT /minio/admin/v3/idp/ldap/add-service-account ")
    );
    let body: serde_json::Value = serde_json::from_str(&requests[3].plain()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"targetUser": "bob", "accessKey": "AK", "secretKey": "SK", "name": "n"})
    );
}
