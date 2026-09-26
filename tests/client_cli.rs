//! Offline tests for the client layer: S3 Signature V2, `alias set` signature probing, the TLS
//! trust prompt, connection deadlines, `-H`/`--debug` on admin requests and the interactive
//! `replicate backlog` view. mx talks to tiny in-process HTTP(S) servers.

mod common;

use assert_cmd::Command;
use common::tty::have;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn mx(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("mx").expect("binary");
    cmd.env("HOME", home);
    cmd
}

fn mx_path() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("mx")
}

/// Recorded request: method, path, query, headers (lowercase names, in order).
#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// `(status, extra headers, body)`.
type Reply = (u16, Vec<(&'static str, String)>, String);
type Responder = Arc<dyn Fn(&Recorded) -> Reply + Send + Sync>;

/// Reads one HTTP/1.1 request (headers and body) from `stream`.
fn read_request<S: Read>(stream: S) -> Option<Recorded> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let length: usize = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(Recorded {
        method,
        path: path.to_string(),
        query: query.to_string(),
        headers,
    })
}

fn write_reply<S: Write>(stream: &mut S, (status, headers, body): Reply) {
    let mut out = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n");
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
    {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(&body);
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.flush();
}

/// Plain HTTP server answering every request with `responder` (one request per connection).
/// Returns the URL and the recorded requests.
fn fake_server(responder: Responder) -> (String, Arc<Mutex<Vec<Recorded>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let log = recorded.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let responder = responder.clone();
            let log = log.clone();
            std::thread::spawn(move || {
                let Some(request) = read_request(&mut stream) else {
                    return;
                };
                let reply = responder(&request);
                log.lock().unwrap().push(request);
                write_reply(&mut stream, reply);
            });
        }
    });
    (url, recorded)
}

fn xml_error(code: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{code}</Code><Message>{code} message</Message><Resource>/</Resource><RequestId>1</RequestId></Error>"
    )
}

const LIST_OBJECTS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>bucket</Name><Prefix></Prefix><KeyCount>0</KeyCount><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated></ListBucketResult>";
const LIST_BUCKETS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ListAllMyBucketsResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Owner><ID>1</ID><DisplayName>x</DisplayName></Owner><Buckets><Bucket><Name>fakebucket</Name><CreationDate>2024-01-01T00:00:00.000Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>";

fn ok_xml(body: &str) -> Reply {
    (
        200,
        vec![("Content-Type", "application/xml".to_string())],
        body.to_string(),
    )
}

fn alias_config(home: &Path, alias: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(home.join(".mx/config.json")).unwrap();
    let config: serde_json::Value = serde_json::from_str(&text).unwrap();
    config["aliases"][alias].clone()
}

// ---------------------------------------------------------------------------
// S3 Signature V2
// ---------------------------------------------------------------------------

#[test]
fn s3v2_alias_signs_requests_with_signature_v2() {
    let (url, recorded) = fake_server(Arc::new(|_| ok_xml(LIST_OBJECTS)));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias", "set", "v2", &url, "minio", "minio123", "--api", "S3v2",
        ])
        .assert()
        .success();
    let out = mx(home.path())
        .args([
            "--debug",
            "-H",
            "X-Amz-Meta-Test: yes",
            "ls",
            "v2/bucket/dir with space/",
        ])
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        stderr.contains("Authorization: AWS **REDACTED**:**REDACTED**"),
        "{stderr}"
    );
    assert!(!stderr.contains("minio:"), "{stderr}");

    let requests = recorded.lock().unwrap().clone();
    assert!(!requests.is_empty());
    for request in requests {
        let auth = request.header("authorization").unwrap();
        assert!(auth.starts_with("AWS minio:"), "{auth}");
        assert!(request.header("date").is_some());
        assert!(request.header("x-amz-content-sha256").is_none());
        assert!(request.header("x-amz-date").is_none());
        assert_eq!(request.header("x-amz-meta-test"), Some("yes"));
        let host = request.header("host").unwrap().to_string();
        let signable = mx::net::sigv2::SignableV2 {
            method: &request.method,
            host: &host,
            path: &request.path,
            query: &request.query,
            headers: request
                .headers
                .iter()
                .filter(|(n, _)| n != "authorization")
                .map(|(n, v)| (n.as_str(), v.as_str()))
                .collect(),
            virtual_host: false,
        };
        assert_eq!(
            auth,
            mx::net::sigv2::authorization(&signable, "minio", "minio123"),
            "{request:?}"
        );
    }
}

#[test]
fn s3v2_alias_signs_bucket_subresources_but_not_admin_api() {
    let (url, recorded) = fake_server(Arc::new(|_| {
        (
            404,
            vec![],
            xml_error("ReplicationConfigurationNotFoundError"),
        )
    }));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias", "set", "v2", &url, "minio", "minio123", "--api", "S3v2",
        ])
        .assert()
        .success();
    let _ = mx(home.path())
        .args(["replicate", "ls", "v2/bucket"])
        .output()
        .unwrap();
    let _ = mx(home.path())
        .args(["quota", "info", "v2/bucket"])
        .output()
        .unwrap();
    let requests = recorded.lock().unwrap().clone();
    let replication = requests
        .iter()
        .find(|r| r.path == "/bucket/" && r.query.starts_with("replication"))
        .expect("replication config request");
    assert!(
        replication
            .header("authorization")
            .unwrap()
            .starts_with("AWS minio:")
    );
    let quota = requests
        .iter()
        .find(|r| r.path.starts_with("/minio/admin/"))
        .expect("admin request");
    assert!(
        quota
            .header("authorization")
            .unwrap()
            .starts_with("AWS4-HMAC-SHA256 ")
    );
}

#[test]
fn s3v2_anonymous_alias_sends_unsigned_requests() {
    let (url, recorded) = fake_server(Arc::new(|_| ok_xml(LIST_OBJECTS)));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args(["alias", "set", "anon", &url, "", "", "--api", "S3v2"])
        .assert()
        .success();
    mx(home.path())
        .args(["ls", "anon/bucket"])
        .assert()
        .success();
    let requests = recorded.lock().unwrap().clone();
    assert!(!requests.is_empty());
    assert!(requests.iter().all(|r| r.header("authorization").is_none()));
}

#[test]
fn s3v2_share_download_presigns_with_query_signature_v2() {
    let (url, _) = fake_server(Arc::new(|request: &Recorded| {
        if request.method == "HEAD" {
            (
                200,
                vec![
                    ("Content-Length", "5".to_string()),
                    ("ETag", "\"5d41402abc4b2a76b9719d911017c592\"".to_string()),
                    ("Last-Modified", "Mon, 01 Jan 2024 00:00:00 GMT".to_string()),
                ],
                String::new(),
            )
        } else {
            ok_xml(LIST_OBJECTS)
        }
    }));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias", "set", "v2", &url, "minio", "minio123", "--api", "S3v2",
        ])
        .assert()
        .success();
    let out = mx(home.path())
        .args(["--json", "share", "download", "v2/bucket/my file.txt"])
        .assert()
        .success();
    let doc: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let share = doc["share"].as_str().unwrap();
    let prefix = format!("{url}/bucket/my%20file.txt?AWSAccessKeyId=minio&Expires=");
    assert!(share.starts_with(&prefix), "{share}");
    let (expires, signature) = share[prefix.len()..].split_once("&Signature=").unwrap();
    let expires: u64 = expires.parse().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(expires.abs_diff(now + 7 * 24 * 3600) < 60);
    let expected = mx::net::sigv2::signature(
        "minio123",
        &format!("GET\n\n\n{expires}\n/bucket/my%20file.txt"),
    );
    assert_eq!(signature, mx::net::sigv2::encode_path(&expected));
}

// ---------------------------------------------------------------------------
// alias set: signature probing
// ---------------------------------------------------------------------------

/// Rejects SigV4 (`SignatureDoesNotMatch`) and accepts SigV2 (`NoSuchBucket` probe answer).
fn v2_only_server() -> (String, Arc<Mutex<Vec<Recorded>>>) {
    fake_server(Arc::new(|request: &Recorded| {
        let v4 = request
            .header("authorization")
            .is_some_and(|auth| auth.starts_with("AWS4-"));
        if v4 {
            (403, vec![], xml_error("SignatureDoesNotMatch"))
        } else {
            (404, vec![], xml_error("NoSuchBucket"))
        }
    }))
}

#[test]
fn alias_set_falls_back_to_s3v2_when_s3v4_is_rejected() {
    let (url, recorded) = v2_only_server();
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args(["--json", "alias", "set", "old", &url, "minio", "minio123"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"api\":\"s3v2\""));
    assert_eq!(alias_config(home.path(), "old")["api"], "s3v2");
    let requests = recorded.lock().unwrap().clone();
    let auth: Vec<_> = requests
        .iter()
        .map(|r| r.header("authorization").unwrap_or_default().to_string())
        .collect();
    assert_eq!(auth.len(), 2, "{auth:?}");
    assert!(auth[0].starts_with("AWS4-HMAC-SHA256 "));
    assert!(auth[1].starts_with("AWS minio:"));
    assert!(requests[1].path.starts_with("/probe-bsign-"));
    assert!(requests[1].query.starts_with("location"));
}

#[test]
fn alias_set_keeps_s3v4_when_accepted() {
    let (url, recorded) = fake_server(Arc::new(|_| (404, vec![], xml_error("NoSuchBucket"))));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args(["alias", "set", "new", &url, "minio", "minio123"])
        .assert()
        .success();
    assert_eq!(alias_config(home.path(), "new")["api"], "s3v4");
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[test]
fn alias_set_reports_the_s3v2_error_when_both_signatures_fail() {
    let (url, _) = fake_server(Arc::new(|_| (403, vec![], xml_error("InvalidAccessKeyId"))));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args(["alias", "set", "bad", &url, "minio", "minio123"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "Unable to initialize new alias from the provided credentials. InvalidAccessKeyId message",
        ));
    assert!(
        !home.path().join(".mx/config.json").exists() || alias_config(home.path(), "bad").is_null()
    );
}

// ---------------------------------------------------------------------------
// Connection deadlines
// ---------------------------------------------------------------------------

/// Accepts connections and never answers (nor reads).
fn silent_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut held: Vec<TcpStream> = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    });
    url
}

#[test]
fn conn_read_deadline_fails_a_silent_server() {
    let url = silent_server();
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias", "set", "slow", &url, "minio", "minio123", "--api", "S3v4",
        ])
        .assert()
        .success();
    let started = Instant::now();
    mx(home.path())
        .args(["ls", "--conn-read-deadline", "300ms", "slow/bucket"])
        .timeout(Duration::from_secs(60))
        .assert()
        .failure()
        .stderr(
            predicates::str::is_match(r"read tcp 127\.0\.0\.1:\d+->127\.0\.0\.1:\d+: i/o timeout")
                .unwrap(),
        );
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[test]
fn conn_write_deadline_fails_a_server_that_stops_reading() {
    let url = silent_server();
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias", "set", "slow", &url, "minio", "minio123", "--api", "S3v4",
        ])
        .assert()
        .success();
    let file = home.path().join("big.bin");
    std::fs::write(&file, vec![7u8; 32 << 20]).unwrap();
    mx(home.path())
        .args([
            "--conn-write-deadline",
            "300ms",
            "--conn-read-deadline",
            "30s",
            "put",
            "--disable-multipart",
            file.to_str().unwrap(),
            "slow/bucket/big.bin",
        ])
        .timeout(Duration::from_secs(90))
        .assert()
        .failure()
        .stderr(predicates::str::contains("i/o timeout"));
}

// ---------------------------------------------------------------------------
// Admin requests: -H and --debug
// ---------------------------------------------------------------------------

#[test]
fn admin_requests_send_custom_headers_and_are_traced() {
    let (url, recorded) = fake_server(Arc::new(|_| {
        (
            200,
            vec![("Content-Type", "application/json".to_string())],
            r#"{"quota":0,"quotatype":"hard"}"#.to_string(),
        )
    }));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias", "set", "adm", &url, "minio", "minio123", "--api", "S3v4",
        ])
        .assert()
        .success();
    let out = mx(home.path())
        .args([
            "--debug",
            "-H",
            "X-Mx-Test: yes",
            "quota",
            "info",
            "adm/bucket",
        ])
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        stderr.contains("mx: <DEBUG> GET /minio/admin/v3/get-bucket-quota?bucket=bucket HTTP/1.1"),
        "{stderr}"
    );
    assert!(stderr.contains("X-Mx-Test: yes"), "{stderr}");
    assert!(stderr.contains("Signature=**REDACTED**"), "{stderr}");
    assert!(stderr.contains("mx: <DEBUG> HTTP/1.1 200 OK"), "{stderr}");
    assert!(!stderr.contains("minio123"));
    let requests = recorded.lock().unwrap().clone();
    let request = requests
        .iter()
        .find(|r| r.path == "/minio/admin/v3/get-bucket-quota")
        .expect("quota request");
    assert_eq!(request.header("x-mx-test"), Some("yes"));
    // Added after signing, like mc's header transport.
    assert!(
        !request
            .header("authorization")
            .unwrap()
            .contains("x-mx-test")
    );
}

// ---------------------------------------------------------------------------
// TLS trust prompt (pseudo-terminal via `script`)
// ---------------------------------------------------------------------------

/// Self-signed `CA:TRUE` certificate for 127.0.0.1 (openssl), as `(cert PEM, key PEM)`.
fn self_signed_cert(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    let status = std::process::Command::new("openssl")
        .current_dir(dir)
        .args([
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:prime256v1",
            "-nodes",
            "-keyout",
            "key.pem",
            "-out",
            "cert.pem",
            "-days",
            "2",
            "-subj",
            "/CN=127.0.0.1",
            "-addext",
            "subjectAltName=IP:127.0.0.1",
        ])
        .output()
        .unwrap();
    assert!(status.status.success(), "{status:?}");
    (
        std::fs::read(dir.join("cert.pem")).unwrap(),
        std::fs::read(dir.join("key.pem")).unwrap(),
    )
}

/// HTTPS server with `cert`/`key` answering every request with `responder`.
fn tls_server(cert: &[u8], key: &[u8], responder: Responder) -> String {
    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::{CertificateDer, PrivateKeyDer};
    let certs = vec![CertificateDer::from_pem_slice(cert).unwrap()];
    let key = PrivateKeyDer::from_pem_slice(key).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("https://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let config = config.clone();
            let responder = responder.clone();
            std::thread::spawn(move || {
                let Ok(conn) = rustls::ServerConnection::new(config) else {
                    return;
                };
                let mut tls = rustls::StreamOwned::new(conn, stream);
                let Some(request) = read_request(&mut tls) else {
                    return;
                };
                write_reply(&mut tls, responder(&request));
                tls.conn.send_close_notify();
                let _ = tls.flush();
            });
        }
    });
    url
}

fn run_on_tty(home: &Path, args: &str, answer: &str) -> (String, bool) {
    common::tty::run(&mx_path(), home, args, answer)
}

#[test]
fn alias_set_on_a_tty_trusts_a_confirmed_self_signed_certificate() {
    if !have("script") || !have("openssl") {
        eprintln!("skipping: needs `script` and `openssl`");
        return;
    }
    let certs = tempfile::tempdir().unwrap();
    let (cert, key) = self_signed_cert(certs.path());
    let url = tls_server(&cert, &key, Arc::new(|_| ok_xml(LIST_BUCKETS)));
    let home = tempfile::tempdir().unwrap();

    // Declined: nothing is saved and mc's verification error is reported.
    let (output, success) = run_on_tty(
        home.path(),
        &format!("alias set tofu {url} minio minio123 --api S3v4"),
        "n\n",
    );
    assert!(!success, "{output}");
    assert!(output.contains("Fingerprint of"), "{output}");
    assert!(output.contains("Confirm public key y/N: "), "{output}");
    assert!(
        output.contains("x509: certificate signed by unknown authority"),
        "{output}"
    );
    let saved = home.path().join(".mx/certs/CAs/tofu.crt");
    assert!(!saved.exists());

    // Confirmed: the certificate is saved and trusted by later commands.
    let (output, success) = run_on_tty(
        home.path(),
        &format!("alias set tofu {url} minio minio123 --api S3v4"),
        "y\n",
    );
    assert!(success, "{output}");
    assert!(output.contains("Added `tofu` successfully."), "{output}");
    let pem = std::fs::read(&saved).unwrap();
    use rustls_pki_types::pem::PemObject;
    assert_eq!(
        rustls_pki_types::CertificateDer::from_pem_slice(&pem).unwrap(),
        rustls_pki_types::CertificateDer::from_pem_slice(&cert).unwrap()
    );
    // The fingerprint is the SHA-256 of the public key info.
    let info = mx::net::x509::parse(
        rustls_pki_types::CertificateDer::from_pem_slice(&cert)
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    assert!(output.contains(&info.fingerprint()), "{output}");
    mx(home.path())
        .args(["ls", "tofu"])
        .assert()
        .success()
        .stdout(predicates::str::contains("fakebucket/"));
}

#[test]
fn alias_set_without_a_tty_does_not_prompt() {
    if !have("openssl") {
        eprintln!("skipping: needs `openssl`");
        return;
    }
    let certs = tempfile::tempdir().unwrap();
    let (cert, key) = self_signed_cert(certs.path());
    let url = tls_server(&cert, &key, Arc::new(|_| ok_xml(LIST_BUCKETS)));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args(["alias", "set", "tofu", &url, "minio", "minio123"])
        .assert()
        .failure()
        .stdout(predicates::str::is_empty())
        .stderr(predicates::str::contains(
            "Unable to initialize new alias from the provided credentials. Get \"",
        ))
        .stderr(predicates::str::contains(
            "/?location=\": tls: failed to verify certificate: x509: certificate signed by unknown authority.",
        ));
    assert!(!home.path().join(".mx/certs/CAs/tofu.crt").exists());
    // `--api` skips the probe; later commands fail verification without `--insecure`.
    mx(home.path())
        .args([
            "alias", "set", "tofu", &url, "minio", "minio123", "--api", "S3v4",
        ])
        .assert()
        .success();
    mx(home.path())
        .args(["ls", "tofu"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "certificate signed by unknown authority",
        ));
    mx(home.path())
        .args(["--insecure", "ls", "tofu"])
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// replicate backlog view
// ---------------------------------------------------------------------------

#[test]
fn replicate_backlog_view_runs_on_a_tty_and_quits_with_q() {
    if !have("script") {
        eprintln!("skipping: needs `script`");
        return;
    }
    let (url, _) = fake_server(Arc::new(|_| (200, vec![], String::new())));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias", "set", "rep", &url, "minio", "minio123", "--api", "S3v4",
        ])
        .assert()
        .success();
    let (output, success) = run_on_tty(home.path(), "replicate backlog rep/bucket", "q");
    assert!(success, "{output}");
    assert!(
        output.contains("enter/spacebar  • ↓/j Move down • ↑/k Move up • q quit"),
        "{output}"
    );
}

#[test]
fn replicate_backlog_view_quits_while_the_fetch_is_loading() {
    if !have("script") {
        eprintln!("skipping: needs `script`");
        return;
    }
    // The backlog request stalls; `q` must not wait for it.
    let (url, _) = fake_server(Arc::new(|request| {
        if request.path.starts_with("/minio/admin/") {
            std::thread::sleep(Duration::from_secs(30));
        }
        (200, vec![], String::new())
    }));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias", "set", "rep", &url, "minio", "minio123", "--api", "S3v4",
        ])
        .assert()
        .success();
    let started = Instant::now();
    let (output, success) = run_on_tty(home.path(), "replicate backlog rep/bucket", "q");
    assert!(success, "{output}");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "quit waited for the fetch: {output}"
    );
}

#[test]
fn replicate_backlog_without_a_tty_keeps_mc_error() {
    let (url, _) = fake_server(Arc::new(|_| (200, vec![], String::new())));
    let home = tempfile::tempdir().unwrap();
    mx(home.path())
        .args([
            "alias", "set", "rep", &url, "minio", "minio123", "--api", "S3v4",
        ])
        .assert()
        .success();
    let mut cmd = std::process::Command::new(mx_path());
    cmd.env("HOME", home.path())
        .args(["replicate", "backlog", "rep/bucket"])
        .stdin(std::process::Stdio::null());
    // Detach from any controlling terminal so /dev/tty cannot be opened, like CI.
    let out = if have("setsid") {
        std::process::Command::new("setsid")
            .env("HOME", home.path())
            .arg(mx_path())
            .args(["replicate", "backlog", "rep/bucket"])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    } else {
        cmd.output().unwrap()
    };
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.is_empty() {
        eprintln!("skipping: a controlling terminal is available");
        return;
    }
    assert!(!out.status.success());
    assert!(
        stderr.contains(
            "Unable to fetch replication backlog: could not open a new TTY: open /dev/tty:"
        ),
        "{stderr}"
    );
}
