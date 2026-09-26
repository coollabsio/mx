use assert_cmd::Command;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static BUCKET_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    std::env::var("MX_LIVE_TESTS").ok().as_deref() == Some("1")
}

pub fn alias_name() -> String {
    std::env::var("MX_TEST_ALIAS").unwrap_or_else(|_| "play".to_string())
}

pub fn bucket_prefix() -> String {
    std::env::var("MX_TEST_BUCKET_PREFIX").unwrap_or_else(|_| "mx-it".to_string())
}

pub fn unique_bucket_name() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = BUCKET_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}-{now}-{seq}", bucket_prefix())
}

pub fn temp_home() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

pub fn mx() -> Command {
    Command::cargo_bin("mx").expect("binary")
}

pub fn configure_alias(home: &Path) {
    let alias = alias_name();
    let url = std::env::var("MX_TEST_URL").ok();
    let access_key = std::env::var("MX_TEST_ACCESS_KEY").ok();
    let secret_key = std::env::var("MX_TEST_SECRET_KEY").ok();
    let api = std::env::var("MX_TEST_API").unwrap_or_else(|_| "S3v4".to_string());
    let path = std::env::var("MX_TEST_PATH").unwrap_or_else(|_| "auto".to_string());

    if let (Some(url), Some(access_key), Some(secret_key)) = (url, access_key, secret_key) {
        mx().env("HOME", home)
            .args([
                "alias",
                "set",
                &alias,
                &url,
                &access_key,
                &secret_key,
                "--api",
                &api,
                "--path",
                &path,
            ])
            .assert()
            .success();
    } else if alias != "play" {
        panic!("custom MX_TEST_ALIAS requires MX_TEST_URL, MX_TEST_ACCESS_KEY, MX_TEST_SECRET_KEY");
    }
}

pub fn local_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write file");
    path
}

// ---------------------------------------------------------------------------
// Second server (MX_TEST_URL2 / MX_TEST_ACCESS_KEY2 / MX_TEST_SECRET_KEY2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub alias: String,
    pub url: String,
    pub access_key: String,
    pub secret_key: String,
}

/// Second S3 server for cross-server tests (None when not configured).
#[allow(dead_code)]
pub fn second_server() -> Option<ServerConfig> {
    Some(ServerConfig {
        alias: std::env::var("MX_TEST_ALIAS2").unwrap_or_else(|_| "local2".to_string()),
        url: std::env::var("MX_TEST_URL2").ok()?,
        access_key: std::env::var("MX_TEST_ACCESS_KEY2").ok()?,
        secret_key: std::env::var("MX_TEST_SECRET_KEY2").ok()?,
    })
}

/// Registers the second server alias in `home`; returns its alias name, or None if unset.
#[allow(dead_code)]
pub fn configure_second_alias(home: &Path) -> Option<String> {
    let server = second_server()?;
    let path = std::env::var("MX_TEST_PATH").unwrap_or_else(|_| "auto".to_string());
    mx().env("HOME", home)
        .args([
            "alias",
            "set",
            &server.alias,
            &server.url,
            &server.access_key,
            &server.secret_key,
            "--path",
            &path,
        ])
        .assert()
        .success();
    Some(server.alias)
}

/// Bucket creation options for [`Live::make_bucket`] / [`make_bucket`].
#[derive(Debug, Clone, Copy, Default)]
pub struct BucketOpts {
    pub versioning: bool,
    /// Object lock (MinIO enables versioning automatically).
    pub lock: bool,
}

/// Creates `alias/bucket` with `mx mb` (`--with-lock` / `--with-versioning`).
#[allow(dead_code)]
pub fn make_bucket(home: &Path, alias: &str, bucket: &str, opts: BucketOpts) {
    let target = format!("{alias}/{bucket}");
    let mut args = vec!["mb".to_string()];
    if opts.lock {
        args.push("--with-lock".into());
    }
    if opts.versioning {
        args.push("--with-versioning".into());
    }
    args.push(target);
    mx().env("HOME", home).args(&args).assert().success();
}

/// Live-test fixture: temp HOME with the test alias configured and one fresh bucket.
/// Buckets created through it are force-removed on drop (best effort).
///
/// ```ignore
/// let Some(live) = live::Live::new() else { return };
/// live.cmd().args(["cp", &file, &live.url("dir/")]).assert().success();
/// ```
#[allow(dead_code)]
pub struct Live {
    pub home: tempfile::TempDir,
    pub alias: String,
    pub bucket: String,
    /// Second server alias when MX_TEST_URL2 etc. are set.
    pub alias2: Option<String>,
    created: std::cell::RefCell<Vec<String>>,
}

#[allow(dead_code)]
impl Live {
    /// Returns None (and prints a skip note) unless MX_LIVE_TESTS=1.
    pub fn new() -> Option<Self> {
        Self::with_bucket(BucketOpts::default())
    }

    pub fn with_bucket(opts: BucketOpts) -> Option<Self> {
        if !enabled() {
            eprintln!("skipping live test; set MX_LIVE_TESTS=1");
            return None;
        }
        let home = temp_home();
        configure_alias(home.path());
        let alias2 = configure_second_alias(home.path());
        let alias = alias_name();
        let bucket = unique_bucket_name();
        make_bucket(home.path(), &alias, &bucket, opts);
        Some(Self {
            created: std::cell::RefCell::new(vec![format!("{alias}/{bucket}")]),
            alias,
            bucket,
            alias2,
            home,
        })
    }

    /// `mx` command with HOME set to the fixture home.
    pub fn cmd(&self) -> Command {
        let mut command = mx();
        command.env("HOME", self.home.path());
        command
    }

    /// `ALIAS/BUCKET/<path>` on the primary bucket (`path` may be empty).
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}/{}", self.alias, self.bucket, path)
    }

    /// `ALIAS/BUCKET` of the primary bucket.
    pub fn bucket_target(&self) -> String {
        format!("{}/{}", self.alias, self.bucket)
    }

    /// Creates another fresh bucket on the primary alias; returns its name.
    pub fn make_bucket(&self, opts: BucketOpts) -> String {
        let bucket = unique_bucket_name();
        make_bucket(self.home.path(), &self.alias, &bucket, opts);
        self.created
            .borrow_mut()
            .push(format!("{}/{bucket}", self.alias));
        bucket
    }

    /// Creates a fresh bucket on the second server; None if it is not configured.
    pub fn make_bucket2(&self, opts: BucketOpts) -> Option<String> {
        let alias2 = self.alias2.clone()?;
        let bucket = unique_bucket_name();
        make_bucket(self.home.path(), &alias2, &bucket, opts);
        self.created.borrow_mut().push(format!("{alias2}/{bucket}"));
        Some(bucket)
    }

    /// Writes a local file under the fixture home.
    pub fn local_file(&self, name: &str, contents: &str) -> PathBuf {
        local_file(self.home.path(), name, contents)
    }

    /// Alias config for in-process library calls (`mx::s3::...`).
    pub fn alias_config(&self) -> mx::config::model::AliasConfig {
        alias_config_from_home(self.home.path(), &self.alias)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        for target in self.created.borrow().iter() {
            let _ = mx()
                .env("HOME", self.home.path())
                .args(["rb", "--force", target])
                .output();
        }
    }
}

/// Reads an alias from `<home>/.mx/config.json` (or `.mc`).
#[allow(dead_code)]
pub fn alias_config_from_home(home: &Path, alias: &str) -> mx::config::model::AliasConfig {
    for dir in [".mx", ".mc"] {
        let path = home.join(dir).join("config.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let config: mx::config::model::ConfigV10 =
                serde_json::from_str(&text).expect("parse config");
            if let Some(found) = config.aliases.get(alias) {
                return found.clone();
            }
        }
    }
    panic!("alias `{alias}` not found under {}", home.display());
}

// ---------------------------------------------------------------------------
// TLS server (MX_TEST_TLS_URL / MX_TEST_TLS_CA)
// ---------------------------------------------------------------------------

/// Temp HOME with alias `tls` for the TLS server and one bucket (made with `--insecure`).
#[allow(dead_code)]
pub struct Tls {
    pub home: tempfile::TempDir,
    pub url: String,
    /// CA PEM path (MX_TEST_TLS_CA).
    pub ca: String,
    pub bucket: String,
}

#[allow(dead_code)]
impl Tls {
    /// Returns None (with a skip note) unless live tests and the TLS server are configured.
    pub fn new() -> Option<Self> {
        if !enabled() {
            eprintln!("skipping live test; set MX_LIVE_TESTS=1");
            return None;
        }
        let (Ok(url), Ok(ca)) = (
            std::env::var("MX_TEST_TLS_URL"),
            std::env::var("MX_TEST_TLS_CA"),
        ) else {
            eprintln!("skipping TLS live test; set MX_TEST_TLS_URL and MX_TEST_TLS_CA");
            return None;
        };
        let home = tempfile::tempdir().unwrap();
        set_tls_alias(home.path(), "tls", &url);
        let bucket = unique_bucket_name();
        mx().env("HOME", home.path())
            .args(["--insecure", "mb", &format!("tls/{bucket}")])
            .assert()
            .success();
        Some(Self {
            home,
            url,
            ca,
            bucket,
        })
    }

    pub fn cmd(&self) -> assert_cmd::Command {
        let mut command = mx();
        command.env("HOME", self.home.path());
        command
    }

    /// Installs the test CA in `~/.mx/certs/CAs` so commands work without `--insecure`.
    pub fn trust_ca(&self) {
        let cas = self.home.path().join(".mx/certs/CAs");
        std::fs::create_dir_all(&cas).unwrap();
        std::fs::copy(&self.ca, cas.join("ca.crt")).unwrap();
    }

    /// `tls/BUCKET/<path>`.
    pub fn target(&self, path: &str) -> String {
        format!("tls/{}/{path}", self.bucket)
    }
}

impl Drop for Tls {
    fn drop(&mut self) {
        let _ = self
            .cmd()
            .args([
                "--insecure",
                "rb",
                "--force",
                &format!("tls/{}", self.bucket),
            ])
            .output();
    }
}

/// Registers `alias` for `url` with the primary server credentials.
#[allow(dead_code)]
pub fn set_tls_alias(home: &Path, alias: &str, url: &str) {
    let access_key = std::env::var("MX_TEST_ACCESS_KEY").unwrap();
    let secret_key = std::env::var("MX_TEST_SECRET_KEY").unwrap();
    mx().env("HOME", home)
        .args(["alias", "set", alias, url, &access_key, &secret_key])
        .assert()
        .success();
}
