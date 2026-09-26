//! Live tests for `idp ldap` and `idp openid` against the dedicated LDAP / OpenID MinIO
//! servers of `tests/services/{openldap,oidc}.sh` (MX_LIVE_TESTS=1; skipped when the
//! services are not running).

mod idp_support;

use idp_support::{LdapEnv, OidcEnv, Server};
use serde_json::Value;
use std::process::Output;

/// `mx` with a temp HOME and alias `idp` pointing at `server`.
struct Mx {
    home: tempfile::TempDir,
}

impl Mx {
    fn new(server: &Server) -> Self {
        let mx = Self {
            home: tempfile::tempdir().expect("tempdir"),
        };
        let out = mx.run(&[
            "alias",
            "set",
            "idp",
            &server.url,
            &server.access_key,
            &server.secret_key,
        ]);
        assert!(out.status.success(), "alias set: {out:?}");
        mx
    }

    fn run(&self, args: &[&str]) -> Output {
        assert_cmd::Command::cargo_bin("mx")
            .expect("binary")
            .env("HOME", self.home.path())
            .env("TZ", "UTC")
            .args(args)
            .output()
            .expect("run mx")
    }

    /// Runs `args`, asserts success and returns stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "mx {}: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Runs `args`, asserts exit status 1 and returns stderr.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert_eq!(out.status.code(), Some(1), "mx {}: {out:?}", args.join(" "));
        String::from_utf8(out.stderr).unwrap()
    }

    fn json(&self, args: &[&str]) -> Vec<Value> {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let text = self.ok(&full);
        text.lines()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect()
    }
}

fn ldap_setup() -> Option<(LdapEnv, Mx)> {
    let env = idp_support::ldap()?;
    idp_support::ensure_ldap(&env);
    let mx = Mx::new(&env.server);
    Some((env, mx))
}

#[test]
fn ldap_config_commands() {
    let _guard = idp_support::serial();
    let Some((env, mx)) = ldap_setup() else {
        return;
    };
    let list = mx.ok(&["idp", "ldap", "list", "idp"]);
    assert!(list.contains("🟢   (default)"), "{list}");
    let docs = mx.json(&["idp", "ldap", "ls", "idp/"]);
    assert_eq!(docs[0][0]["type"], "ldap");
    assert_eq!(docs[0][0]["enabled"], true);

    let info = mx.ok(&["idp", "ldap", "info", "idp"]);
    assert!(info.starts_with('╭'), "{info}");
    assert!(
        info.contains(&format!("server_addr: {}", env.ldap_addr)),
        "{info}"
    );
    let docs = mx.json(&["idp", "ldap", "info", "idp"]);
    let keys: Vec<&str> = docs[0]["info"]
        .as_array()
        .unwrap()
        .iter()
        .map(|kv| kv["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"user_dn_search_filter"), "{keys:?}");

    // Changes need a restart; the config stays active until then.
    let cfg = idp_support::ldap_cfg_args(&env);
    let mut add: Vec<&str> = vec!["idp", "ldap", "add", "idp"];
    add.extend(cfg.iter().map(String::as_str));
    let err = mx.fails(&add);
    assert!(
        err.contains("Unable to add LDAP IDP config to server. An IDP configuration with the given name already exists."),
        "{err}"
    );
    add[2] = "update";
    let out = mx.ok(&add);
    assert_eq!(
        out,
        "Successfully applied new settings.\nPlease restart your server 'mc admin service restart idp'.\n"
    );
    let update = [
        "idp",
        "ldap",
        "update",
        "idp",
        &format!("lookup_bind_password={}", env.bind_password),
    ];
    assert_eq!(
        mx.json(&update)[0],
        serde_json::json!({"status": "success"})
    );
    mx.ok(&["idp", "ldap", "disable", "idp"]);
    mx.ok(&["idp", "ldap", "enable", "idp"]);
    mx.ok(&["idp", "ldap", "rm", "idp"]);
    let err = mx.fails(&["idp", "ldap", "add", "idp", "named", "a=b"]);
    assert!(
        err.contains(
            "Bad LDAP IDP configuration: all config parameters must be of the form \"key=value\"."
        ),
        "{err}"
    );
    // Put the configuration back (pending) so a later restart keeps LDAP enabled.
    idp_support::ensure_ldap(&env);
    assert!(
        mx.ok(&["idp", "ldap", "info", "idp"])
            .contains("enable: on")
    );
}

#[test]
fn ldap_policy_commands() {
    let _guard = idp_support::serial();
    let Some((_env, mx)) = ldap_setup() else {
        return;
    };
    let _ = mx.run(&[
        "idp",
        "ldap",
        "policy",
        "detach",
        "idp",
        "consoleAdmin",
        "-u",
        "liza",
    ]);
    let out = mx.ok(&[
        "idp",
        "ldap",
        "policy",
        "attach",
        "idp",
        "consoleAdmin",
        "--user",
        "liza",
    ]);
    assert_eq!(out, "Attached Policies: [consoleAdmin]\nTo User: liza\n");
    let err = mx.fails(&[
        "idp",
        "ldap",
        "policy",
        "attach",
        "idp",
        "consoleAdmin",
        "-u",
        "liza",
    ]);
    assert!(err.contains("policy change is already in effect"), "{err}");

    let docs = mx.json(&[
        "idp",
        "ldap",
        "policy",
        "entities",
        "idp",
        "-p",
        "consoleAdmin",
    ]);
    assert_eq!(
        docs[0]["result"]["policyMappings"][0]["users"],
        serde_json::json!(["uid=liza,ou=people,dc=min,dc=io"])
    );
    let text = mx.ok(&["idp", "ldap", "policy", "entities", "idp", "-u", "liza"]);
    assert!(text.starts_with("Query time: "), "{text}");
    assert!(
        text.contains("User -> Policy Mappings:\n  User: uid=liza,ou=people,dc=min,dc=io\n    Policies:\n      consoleAdmin"),
        "{text}"
    );

    let docs = mx.json(&[
        "idp",
        "ldap",
        "policy",
        "detach",
        "idp",
        "consoleAdmin",
        "-u",
        "liza",
    ]);
    assert_eq!(
        docs[0],
        serde_json::json!({"status": "success", "policiesDetached": ["consoleAdmin"], "user": "liza"})
    );
    let err = mx.fails(&["idp", "ldap", "policy", "attach", "idp", "readwrite"]);
    assert!(
        err.contains("Invalid policy attach arguments. no user or group association was given."),
        "{err}"
    );
    let err = mx.fails(&["idp", "ldap", "policy", "detach", "idp", "readwrite"]);
    assert!(
        err.contains("Missing flag in command: at least one of --user or --group is required."),
        "{err}"
    );
}

#[test]
fn ldap_accesskey_commands() {
    let _guard = idp_support::serial();
    let Some((env, mx)) = ldap_setup() else {
        return;
    };
    let _ = mx.run(&[
        "idp",
        "ldap",
        "policy",
        "attach",
        "idp",
        "readwrite",
        "-u",
        "fahim",
    ]);
    let key = format!("mxidp{}", std::process::id());
    let _ = mx.run(&["idp", "ldap", "accesskey", "rm", "idp", &key]);

    let out = mx.ok(&[
        "idp",
        "ldap",
        "accesskey",
        "create",
        "idp",
        "fahim",
        "--access-key",
        &key,
        "--secret-key",
        "fahimsecret1",
        "--name",
        "n1",
        "--description",
        "first key",
    ]);
    assert_eq!(
        out,
        format!(
            "Access Key: {key}\nSecret Key: fahimsecret1\nExpiration: NONE\nName: n1\nDescription: first key\n"
        )
    );
    let docs = mx.json(&["idp", "ldap", "accesskey", "list", "idp", "fahim"]);
    assert_eq!(docs[0]["user"], "uid=fahim,ou=people,dc=min,dc=io");
    assert_eq!(docs[0]["ldap"], true);
    assert_eq!(docs[0]["svcaccs"][0]["accessKey"], key.as_str());
    let text = mx.ok(&[
        "idp",
        "ldap",
        "accesskey",
        "ls",
        "idp",
        "fahim",
        "--svcacc-only",
    ]);
    assert!(
        text.contains("  Access Keys:\n")
            && text.contains(&format!("\n    {key}, expires: never, sts: false\n")),
        "{text}"
    );

    let info = mx.ok(&["idp", "ldap", "accesskey", "info", "idp", &key]);
    assert!(
        info.contains("Parent User: uid=fahim,ou=people,dc=min,dc=io\n"),
        "{info}"
    );
    assert!(
        info.ends_with("Provider Specific Info:\nUsername: uid=fahim,ou=people,dc=min,dc=io\n"),
        "{info}"
    );
    let docs = mx.json(&["idp", "ldap", "accesskey", "info", "idp", &key]);
    assert_eq!(docs[0]["provider"], "ldap");
    assert_eq!(docs[0]["policy"]["Version"], "2012-10-17");

    let out = mx.ok(&[
        "idp",
        "ldap",
        "accesskey",
        "edit",
        "idp",
        &key,
        "--name",
        "n2",
        "--expiry-duration",
        "48h",
    ]);
    assert_eq!(out, format!("Successfully edited access key `{key}`.\n"));
    let info = mx.ok(&["idp", "ldap", "accesskey", "info", "idp", &key]);
    assert!(
        info.contains("Name: n2\n") && info.contains("Expiration: 1 day from now\n"),
        "{info}"
    );
    let out = mx.ok(&["idp", "ldap", "accesskey", "disable", "idp", &key]);
    assert_eq!(out, format!("Successfully disabled access key `{key}`.\n"));
    // MinIO does not report disabled access keys.
    let err = mx.fails(&["idp", "ldap", "accesskey", "info", "idp", &key]);
    assert!(
        err.contains("The specified access key does not exist."),
        "{err}"
    );
    let docs = mx.json(&["idp", "ldap", "accesskey", "enable", "idp", &key]);
    assert_eq!(
        docs[0],
        serde_json::json!({"status": "success", "accessKey": key})
    );

    // STS keys of an LDAP user are listed with --temp-only.
    let sts = idp_support::ldap_sts(&env, "fahim", "fahim123");
    let text = mx.ok(&[
        "idp",
        "ldap",
        "accesskey",
        "list",
        "idp",
        "fahim",
        "--temp-only",
    ]);
    assert!(
        text.contains(&format!("{}, expires: ", sts.access_key)),
        "{text}"
    );
    let out = mx.ok(&[
        "idp",
        "ldap",
        "accesskey",
        "sts-revoke",
        "idp",
        "fahim",
        "--all",
    ]);
    assert_eq!(
        out,
        "Successfully revoked all STS accounts for user fahim\n"
    );
    let err = mx.fails(&["idp", "ldap", "accesskey", "sts-revoke", "idp", "fahim"]);
    assert!(
        err.contains("Exactly one of --all or --token-type must be specified."),
        "{err}"
    );

    let out = mx.ok(&["idp", "ldap", "accesskey", "rm", "idp", &key]);
    assert_eq!(out, format!("Successfully removed access key `{key}`.\n"));
    let err = mx.fails(&["idp", "ldap", "accesskey", "info", "idp", &key]);
    assert!(
        err.contains("Unable to get info for access key. The specified access key does not exist."),
        "{err}"
    );

    // Generated credentials, JSON output.
    let docs = mx.json(&[
        "idp",
        "ldap",
        "accesskey",
        "create",
        "idp",
        "fahim",
        "--expiry-duration",
        "2h",
    ]);
    let generated = docs[0]["accessKey"].as_str().unwrap().to_string();
    assert_eq!(generated.len(), 20);
    assert_eq!(docs[0]["secretKey"].as_str().unwrap().len(), 40);
    assert!(docs[0]["expiration"].as_str().is_some());
    mx.ok(&["idp", "ldap", "accesskey", "remove", "idp", &generated]);
    let _ = mx.run(&[
        "idp",
        "ldap",
        "policy",
        "detach",
        "idp",
        "readwrite",
        "-u",
        "fahim",
    ]);
}

/// `create-with-login` needs a terminal: run it under `script` (skipped without it).
#[test]
fn ldap_accesskey_create_with_login() {
    let _guard = idp_support::serial();
    let Some((env, mx)) = ldap_setup() else {
        return;
    };
    if std::process::Command::new("script")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping: `script` not available");
        return;
    }
    let _ = mx.run(&[
        "idp",
        "ldap",
        "policy",
        "attach",
        "idp",
        "readwrite",
        "-u",
        "fahim",
    ]);
    let key = format!("mxlogin{}", std::process::id());
    let command = format!(
        "{} idp ldap accesskey create-with-login {} --access-key {key} --secret-key loginsecret1",
        env!("CARGO_BIN_EXE_mx"),
        env.server.url
    );
    let mut child = std::process::Command::new("script")
        .args(["-qec", &command, "/dev/null"])
        .env("HOME", mx.home.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("script");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        stdin.write_all(b"fahim\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        stdin.write_all(b"fahim123\n").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    assert!(text.contains("Enter LDAP Username: "), "{text}");
    assert!(text.contains("Enter LDAP Password: "), "{text}");
    assert!(
        text.contains(&format!("Access Key: {key}\nSecret Key: loginsecret1\n")),
        "{text}"
    );
    let info = mx.ok(&["idp", "ldap", "accesskey", "info", "idp", &key]);
    assert!(
        info.contains("Parent User: uid=fahim,ou=people,dc=min,dc=io"),
        "{info}"
    );
    mx.ok(&["idp", "ldap", "accesskey", "rm", "idp", &key]);
    let _ = mx.run(&[
        "idp",
        "ldap",
        "policy",
        "detach",
        "idp",
        "readwrite",
        "-u",
        "fahim",
    ]);
}

fn oidc_setup() -> Option<(OidcEnv, String, Mx)> {
    let env = idp_support::oidc()?;
    let arn = idp_support::ensure_openid(&env);
    let mx = Mx::new(&env.server);
    Some((env, arn, mx))
}

#[test]
fn openid_config_commands() {
    let _guard = idp_support::serial();
    let Some((env, arn, mx)) = oidc_setup() else {
        return;
    };
    let list = mx.ok(&["idp", "openid", "list", "idp"]);
    assert!(list.contains(&format!("🟢   (default)  {arn} │")), "{list}");
    let docs = mx.json(&["idp", "openid", "list", "idp"]);
    assert_eq!(docs[0][0]["roleARN"], arn.as_str());
    let info = mx.ok(&["idp", "openid", "info", "idp"]);
    assert!(info.contains(&format!("   roleARN: {arn}")), "{info}");
    assert!(info.contains("role_policy: readwrite"), "{info}");

    // A second, named configuration: add, restart, disable/enable, remove.
    let name = "mxsecond";
    let cfg = idp_support::openid_cfg_args(&env, "mx-second-client", "readonly");
    let mut add: Vec<&str> = vec!["idp", "openid", "add", "idp", name];
    add.extend(cfg.iter().map(String::as_str));
    assert!(
        mx.ok(&add)
            .starts_with("Successfully applied new settings.\nPlease restart")
    );
    idp_support::restart(&env.server);
    let info = mx.ok(&["idp", "openid", "info", "idp", name]);
    assert!(info.contains("client_id: mx-second-client"), "{info}");
    mx.ok(&["idp", "openid", "update", "idp", name, "scopes=openid"]);
    mx.ok(&["idp", "openid", "disable", "idp", name]);
    mx.ok(&["idp", "openid", "enable", "idp", name]);
    let docs = mx.json(&["idp", "openid", "remove", "idp", name]);
    assert_eq!(docs[0], serde_json::json!({"status": "success"}));
    idp_support::restart(&env.server);
    let err = mx.fails(&["idp", "openid", "info", "idp", name]);
    assert!(
        err.contains("Unable to get openid IDP config from server. No such named configuration target exists."),
        "{err}"
    );
    let err = mx.fails(&["idp", "openid", "add", "idp", "other", "client_id=x"]);
    assert!(
        err.contains("Unable to add OpenID IDP config to server"),
        "{err}"
    );
    idp_support::ensure_openid(&env);
}

#[test]
fn openid_accesskey_commands() {
    let _guard = idp_support::serial();
    let Some((env, arn, mx)) = oidc_setup() else {
        return;
    };
    let sts = idp_support::openid_sts(&env, &arn);
    let key = format!("mxoidc{}", std::process::id());
    idp_support::add_service_account_as(&env.server, &sts, &key);

    let docs = mx.json(&["idp", "openid", "accesskey", "list", "idp"]);
    assert_eq!(docs[0]["configName"], "_");
    let users = docs[0]["users"].as_array().unwrap();
    let user = users
        .iter()
        .find(|u| {
            u["serviceAccounts"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| k["accessKey"] == key.as_str())
        })
        .expect("user with the key");
    assert!(
        user["stsKeys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["accessKey"] == sts.access_key.as_str())
    );
    let text = mx.ok(&["idp", "openid", "accesskey", "ls", "idp", "--svcacc-only"]);
    assert!(text.starts_with("Config Name: _\n  User ID: "), "{text}");
    assert!(
        text.contains(&format!("      {key}, expires: never, sts: false\n")),
        "{text}"
    );
    let err = mx.fails(&["idp", "openid", "accesskey", "list", "idp:nosuch"]);
    assert!(
        err.contains("Unable to list access keys. No such named configuration target exists."),
        "{err}"
    );
    let err = mx.fails(&[
        "idp",
        "openid",
        "accesskey",
        "list",
        "idp:x",
        "--all-configs",
    ]);
    assert!(
        err.contains(
            "Unable to list access keys. configName and allConfigs are mutually exclusive."
        ),
        "{err}"
    );

    let info = mx.ok(&["idp", "openid", "accesskey", "info", "idp", &key]);
    assert!(
        info.contains(
            "Provider: openid\nProvider Specific Info:\n  Config: _ (default)\n  User ID (sub): "
        ),
        "{info}"
    );
    let docs = mx.json(&["idp", "openid", "accesskey", "info", "idp", &key]);
    assert_eq!(docs[0]["providerInfo"]["configName"], "_");
    assert_eq!(docs[0]["providerInfo"]["userIDClaim"], "sub");

    mx.ok(&[
        "idp",
        "openid",
        "accesskey",
        "edit",
        "idp",
        &key,
        "--description",
        "edited",
    ]);
    assert!(
        mx.ok(&["idp", "openid", "accesskey", "info", "idp", &key])
            .contains("Description: edited\n")
    );
    mx.ok(&["idp", "openid", "accesskey", "disable", "idp", &key]);
    mx.ok(&["idp", "openid", "accesskey", "enable", "idp", &key]);
    let out = mx.ok(&["idp", "openid", "accesskey", "rm", "idp", &key]);
    assert_eq!(out, format!("Successfully removed access key `{key}`.\n"));
}
