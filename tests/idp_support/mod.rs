//! Shared setup for the IDP live suites (`live_idp`, `live_mc_parity_idp`): the dedicated
//! LDAP / OpenID MinIO servers started by `tests/services/{openldap,oidc}.sh`, IDP config
//! bootstrap (config + restart) and STS credentials for LDAP and OpenID users.
#![allow(dead_code)]

use mx::config::model::AliasConfig;
use mx::s3::admin::AdminClient;
use mx::s3::admin_idp::{self, Credentials};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// IDP server state is global: tests in one binary run one at a time.
pub fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// A MinIO server with root credentials.
#[derive(Debug, Clone)]
pub struct Server {
    pub url: String,
    pub access_key: String,
    pub secret_key: String,
}

impl Server {
    pub fn alias(&self) -> AliasConfig {
        AliasConfig {
            url: self.url.clone(),
            access_key: self.access_key.clone(),
            secret_key: self.secret_key.clone(),
            ..Default::default()
        }
    }

    pub fn client(&self) -> AdminClient {
        AdminClient::new(&self.alias()).expect("admin client")
    }

    /// Points the parity harness (`MX_TEST_URL` & co.) at this server.
    pub fn use_for_parity(&self) {
        // SAFETY: IDP tests hold `serial()`, so no other test thread reads the environment.
        unsafe {
            std::env::set_var("MX_TEST_URL", &self.url);
            std::env::set_var("MX_TEST_ACCESS_KEY", &self.access_key);
            std::env::set_var("MX_TEST_SECRET_KEY", &self.secret_key);
        }
    }
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn live_enabled() -> bool {
    std::env::var("MX_LIVE_TESTS").ok().as_deref() == Some("1")
}

/// LDAP test environment (`tests/services/openldap.sh`).
#[derive(Debug, Clone)]
pub struct LdapEnv {
    pub server: Server,
    pub ldap_addr: String,
    pub bind_dn: String,
    pub bind_password: String,
}

pub fn ldap() -> Option<LdapEnv> {
    if !live_enabled() {
        return None;
    }
    let Some(url) = env("MX_TEST_LDAP_URL") else {
        eprintln!("skipping: LDAP service not running (MX_TEST_LDAP_URL)");
        return None;
    };
    Some(LdapEnv {
        server: Server {
            url,
            access_key: env("MX_TEST_LDAP_ACCESS_KEY")?,
            secret_key: env("MX_TEST_LDAP_SECRET_KEY")?,
        },
        ldap_addr: env("MX_TEST_LDAP_SERVER")?,
        bind_dn: env("MX_TEST_LDAP_BIND_DN")?,
        bind_password: env("MX_TEST_LDAP_BIND_PASSWORD")?,
    })
}

/// OpenID test environment (`tests/services/oidc.sh`).
#[derive(Debug, Clone)]
pub struct OidcEnv {
    pub server: Server,
    pub config_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
}

pub fn oidc() -> Option<OidcEnv> {
    if !live_enabled() {
        return None;
    }
    let Some(url) = env("MX_TEST_OIDC_MINIO_URL") else {
        eprintln!("skipping: OpenID service not running (MX_TEST_OIDC_MINIO_URL)");
        return None;
    };
    Some(OidcEnv {
        server: Server {
            url,
            access_key: env("MX_TEST_OIDC_ACCESS_KEY")?,
            secret_key: env("MX_TEST_OIDC_SECRET_KEY")?,
        },
        config_url: env("MX_TEST_OIDC_CONFIG_URL")?,
        token_url: env("MX_TEST_OIDC_TOKEN_URL")?,
        client_id: env("MX_TEST_OIDC_CLIENT_ID")?,
        client_secret: env("MX_TEST_OIDC_CLIENT_SECRET")?,
    })
}

pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(future)
}

/// Restarts the server (madmin `ServiceRestartV2`) and waits until it serves again.
pub fn restart(server: &Server) {
    let client = server.client();
    block_on(
        client
            .request("POST", "service")
            .query("action", "restart")
            .query("dry-run", "false")
            .query("type", "2")
            .send(),
    )
    .expect("service restart");
    std::thread::sleep(Duration::from_secs(2));
    wait_ready(server);
}

fn wait_ready(server: &Server) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let ok = Command::new("curl")
            .args(["-sf", "-o", "/dev/null"])
            .arg(format!("{}/minio/health/ready", server.url))
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return;
        }
        assert!(Instant::now() < deadline, "{} not ready", server.url);
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// `KEY=VALUE` arguments of the test LDAP configuration.
pub fn ldap_cfg_args(env: &LdapEnv) -> Vec<String> {
    vec![
        format!("server_addr={}", env.ldap_addr),
        format!("lookup_bind_dn={}", env.bind_dn),
        format!("lookup_bind_password={}", env.bind_password),
        "user_dn_search_base_dn=ou=people,dc=min,dc=io".into(),
        "user_dn_search_filter=(uid=%s)".into(),
        "group_search_base_dn=ou=groups,dc=min,dc=io".into(),
        "group_search_filter=(&(objectclass=groupOfNames)(member=%d))".into(),
        "server_insecure=on".into(),
    ]
}

fn cfg_value(config: &admin_idp::IdpConfig, key: &str) -> Option<String> {
    config
        .info
        .iter()
        .flatten()
        .find(|kv| kv.key == key)
        .map(|kv| kv.value.clone())
}

/// Sets the pending default config of `cfg_type` (update, or add when none is stored), so
/// leftovers of earlier tests (disable, remove) never reach the next restart.
fn put_config(client: &AdminClient, cfg_type: &str, params: &str) {
    let updated = block_on(admin_idp::add_or_update_idp_config(
        client, cfg_type, "_", params, true,
    ));
    if updated.is_err() {
        block_on(admin_idp::add_or_update_idp_config(
            client, cfg_type, "_", params, false,
        ))
        .unwrap_or_else(|err| panic!("add {cfg_type} config: {err:#}"));
    }
}

/// Makes the LDAP configuration pending (always) and active (restart when needed).
pub fn ensure_ldap(env: &LdapEnv) {
    let client = env.server.client();
    let params = ldap_cfg_args(env).join(" ") + " enable=on";
    put_config(&client, admin_idp::LDAP, &params);
    let active = |client: &AdminClient| {
        block_on(admin_idp::get_idp_config(client, admin_idp::LDAP, "_"))
            .ok()
            .is_some_and(|config| {
                cfg_value(&config, "server_addr").as_deref() == Some(env.ldap_addr.as_str())
                    && cfg_value(&config, "enable").as_deref() == Some("on")
            })
    };
    if active(&client) {
        return;
    }
    restart(&env.server);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !active(&env.server.client()) {
        assert!(Instant::now() < deadline, "LDAP config not active");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// `KEY=VALUE` arguments of an OpenID configuration for the test Dex.
pub fn openid_cfg_args(env: &OidcEnv, client_id: &str, role_policy: &str) -> Vec<String> {
    vec![
        format!("client_id={client_id}"),
        format!("client_secret={}", env.client_secret),
        format!("config_url={}", env.config_url),
        "scopes=openid,email,profile".into(),
        format!("role_policy={role_policy}"),
    ]
}

/// Makes the default OpenID configuration (role policy `readwrite`) pending and active;
/// returns its role ARN.
pub fn ensure_openid(env: &OidcEnv) -> String {
    let client = env.server.client();
    let params = openid_cfg_args(env, &env.client_id, "readwrite").join(" ") + " enable=on";
    put_config(&client, admin_idp::OPENID, &params);
    let role_arn = |client: &AdminClient| {
        block_on(admin_idp::list_idp_config(client, admin_idp::OPENID))
            .ok()
            .flatten()
            .into_iter()
            .flatten()
            .find(|item| item.name == "_" && item.enabled && !item.role_arn.is_empty())
            .map(|item| item.role_arn)
    };
    if let Some(arn) = role_arn(&client) {
        return arn;
    }
    restart(&env.server);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(arn) = role_arn(&env.server.client()) {
            return arn;
        }
        assert!(Instant::now() < deadline, "OpenID config not active");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// LDAP STS credentials (`AssumeRoleWithLDAPIdentity`).
pub fn ldap_sts(env: &LdapEnv, username: &str, password: &str) -> Credentials {
    let anonymous = AdminClient::new(&AliasConfig {
        url: env.server.url.clone(),
        ..Default::default()
    })
    .expect("client");
    block_on(admin_idp::assume_role_with_ldap_identity(
        &anonymous, "/", username, password,
    ))
    .expect("LDAP STS")
}

fn xml_text(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open).map(|i| i + open.len()).unwrap_or(0);
    let end = text[start..]
        .find(&close)
        .map(|i| start + i)
        .unwrap_or(start);
    text[start..end].to_string()
}

/// OpenID STS credentials for Dex user `dillon@example.com` (password grant +
/// `AssumeRoleWithWebIdentity`).
pub fn openid_sts(env: &OidcEnv, role_arn: &str) -> Credentials {
    let output = Command::new("curl")
        .args(["-sf", "-u"])
        .arg(format!("{}:{}", env.client_id, env.client_secret))
        .args([
            "-d",
            "grant_type=password",
            "-d",
            "username=dillon@example.com",
            "-d",
            "password=password",
            "--data-urlencode",
            "scope=openid email profile",
        ])
        .arg(&env.token_url)
        .output()
        .expect("curl");
    let token: serde_json::Value = serde_json::from_slice(&output.stdout).expect("token JSON");
    let id_token = token["id_token"].as_str().expect("id_token");
    let form = [
        ("Action", "AssumeRoleWithWebIdentity"),
        ("RoleArn", role_arn),
        ("Version", "2011-06-15"),
        ("WebIdentityToken", id_token),
    ]
    .iter()
    .map(|(k, v)| {
        format!(
            "{}={}",
            mx::s3::admin::encode_component(k),
            mx::s3::admin::encode_component(v)
        )
    })
    .collect::<Vec<_>>()
    .join("&");
    let anonymous = AdminClient::new(&AliasConfig {
        url: env.server.url.clone(),
        ..Default::default()
    })
    .expect("client");
    let response = block_on(anonymous.send_anonymous(
        "POST",
        "/",
        &[(
            "content-type",
            "application/x-www-form-urlencoded".to_string(),
        )],
        form.into_bytes(),
    ))
    .expect("OpenID STS");
    let text = response.text();
    assert_eq!(response.status, 200, "AssumeRoleWithWebIdentity: {text}");
    Credentials {
        access_key: xml_text(&text, "AccessKeyId"),
        secret_key: xml_text(&text, "SecretAccessKey"),
        session_token: xml_text(&text, "SessionToken"),
        expiration: Some(xml_text(&text, "Expiration")),
    }
}

/// Creates a service account for the owner of STS `creds` (madmin `AddServiceAccount`).
pub fn add_service_account_as(server: &Server, creds: &Credentials, access_key: &str) {
    let client = AdminClient::new(&AliasConfig {
        url: server.url.clone(),
        access_key: creds.access_key.clone(),
        secret_key: creds.secret_key.clone(),
        session_token: Some(creds.session_token.clone()),
        ..Default::default()
    })
    .expect("client");
    block_on(async {
        client
            .request("PUT", "add-service-account")
            .encrypted_json(&serde_json::json!({
                "accessKey": access_key,
                "secretKey": format!("{access_key}-secret"),
            }))?
            .send()
            .await
    })
    .expect("add service account");
}
