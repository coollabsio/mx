//! mc parity for `idp ldap` and `idp openid` (see tests/common/parity.rs). Runs against the
//! dedicated LDAP / OpenID MinIO servers of `tests/services/{openldap,oidc}.sh`:
//! `MX_MC_PARITY=1 sh tests/live_minio.sh live_mc_parity_idp`.
//!
//! IDP configuration is server-global, so commands that change it run per side
//! ([`Parity::run_side`]) with the configuration restored in between.

mod common;
mod idp_support;

use common::parity::{Outcome, Parity, Tool};
use idp_support::{LdapEnv, OidcEnv};

const DN_FAHIM: &str = "uid=fahim,ou=people,dc=min,dc=io";
const GROUP_B: &str = "cn=projectb,ou=groups,dc=min,dc=io";
const POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::idp-parity/*"]}]}"#;

fn ldap_parity() -> Option<(LdapEnv, Parity)> {
    let env = idp_support::ldap()?;
    common::parity::mc_bin()?;
    idp_support::ensure_ldap(&env);
    env.server.use_for_parity();
    Some((env, Parity::bare()?))
}

fn oidc_parity() -> Option<(OidcEnv, String, Parity)> {
    let env = idp_support::oidc()?;
    common::parity::mc_bin()?;
    let arn = idp_support::ensure_openid(&env);
    env.server.use_for_parity();
    Some((env, arn, Parity::bare()?))
}

/// Text and `--json` parity of `args`.
fn both(p: &Parity, args: &[&str]) {
    p.assert_parity(args, None);
    let mut json = vec!["--json"];
    json.extend_from_slice(args);
    p.assert_json_parity(&json, None);
}

/// Runs `args` for mc, then `reset()`, then for mx, then `reset()`, and compares.
fn per_side(p: &Parity, args: &[&str], json: bool, reset: &dyn Fn()) {
    let mc: Outcome = p.run_side(Tool::Mc, args, None);
    reset();
    let mx = p.run_side(Tool::Mx, args, None);
    reset();
    p.assert_outcomes(args, &mc, &mx, json);
}

fn owned<'a>(prefix: &[&'a str], rest: &'a [String]) -> Vec<&'a str> {
    prefix
        .iter()
        .copied()
        .chain(rest.iter().map(String::as_str))
        .collect()
}

// ---------------------------------------------------------------------------
// LDAP configuration
// ---------------------------------------------------------------------------

#[test]
fn ldap_config_read() {
    let _guard = idp_support::serial();
    let Some((_env, p)) = ldap_parity() else {
        return;
    };
    both(&p, &["idp", "ldap", "list", "{alias}"]);
    both(&p, &["idp", "ldap", "ls", "{alias}/"]);
    both(&p, &["idp", "ldap", "info", "{alias}"]);
    both(&p, &["idp", "ldap", "info", "nosuchalias"]);
}

#[test]
fn ldap_config_write() {
    let _guard = idp_support::serial();
    let Some((env, p)) = ldap_parity() else {
        return;
    };
    let cfg = idp_support::ldap_cfg_args(&env);
    // The active configuration already exists.
    both(&p, &owned(&["idp", "ldap", "add", "{alias}"], &cfg));
    both(&p, &["idp", "ldap", "add", "{alias}", "named", "a=b"]);
    both(&p, &owned(&["idp", "ldap", "update", "{alias}"], &cfg));
    both(&p, &["idp", "ldap", "update", "{alias}", "named", "a=b"]);
    both(&p, &["idp", "ldap", "update", "{alias}", "nosuchkey=1"]);
    both(&p, &["idp", "ldap", "disable", "{alias}"]);
    both(&p, &["idp", "ldap", "enable", "{alias}"]);
    // remove drops the stored config; put it back between the runs.
    let restore = || idp_support::ensure_ldap(&env);
    per_side(&p, &["idp", "ldap", "remove", "{alias}"], false, &restore);
    per_side(
        &p,
        &["--json", "idp", "ldap", "rm", "{alias}"],
        true,
        &restore,
    );
    // Without a stored config, `update` fails and `add` succeeds.
    let remove = || {
        let client = env.server.client();
        let _ = idp_support::block_on(mx::s3::admin_idp::delete_idp_config(&client, "ldap", "_"));
    };
    remove();
    both(
        &p,
        &["idp", "ldap", "update", "{alias}", "server_insecure=on"],
    );
    per_side(
        &p,
        &owned(&["idp", "ldap", "add", "{alias}"], &cfg),
        false,
        &remove,
    );
    restore();
}

// ---------------------------------------------------------------------------
// LDAP policy mappings
// ---------------------------------------------------------------------------

#[test]
fn ldap_policy_attach_detach_entities() {
    let _guard = idp_support::serial();
    let Some((_env, mut p)) = ldap_parity() else {
        return;
    };
    p.iam_cleanup();
    p.file("policy.json", POLICY);
    p.setup(&[
        "admin",
        "policy",
        "create",
        "{alias}",
        "{uniq}p",
        "policy.json",
    ]);
    p.cleanup_on_drop(&[
        "idp", "ldap", "policy", "detach", "{alias}", "{uniq}p", "-u", "liza",
    ]);
    p.cleanup_on_drop(&[
        "idp", "ldap", "policy", "detach", "{alias}", "{uniq}p", "-g", GROUP_B,
    ]);

    p.assert_parity(
        &[
            "idp", "ldap", "policy", "attach", "{alias}", "{uniq}p", "--user", "liza",
        ],
        None,
    );
    p.assert_json_parity(
        &[
            "--json", "idp", "ldap", "policy", "attach", "{alias}", "{uniq}p", "-g", GROUP_B,
        ],
        None,
    );
    // Already attached.
    both(
        &p,
        &[
            "idp", "ldap", "policy", "attach", "{alias}", "{uniq}p", "-u", "liza",
        ],
    );

    both(
        &p,
        &[
            "idp", "ldap", "policy", "entities", "{alias}", "-p", "{uniq}p",
        ],
    );
    p.assert_parity(
        &[
            "idp", "ldap", "policy", "entities", "{alias}", "-p", "{uniq}p", "-u", "fahim",
        ],
        None,
    );

    p.assert_parity(
        &[
            "idp", "ldap", "policy", "detach", "{alias}", "{uniq}p", "-u", "liza",
        ],
        None,
    );
    p.assert_json_parity(
        &[
            "--json", "idp", "ldap", "policy", "detach", "{alias}", "{uniq}p", "--group", GROUP_B,
        ],
        None,
    );
    both(
        &p,
        &[
            "idp", "ldap", "policy", "detach", "{alias}", "{uniq}p", "-u", "liza",
        ],
    );

    // Validation and server errors.
    both(
        &p,
        &["idp", "ldap", "policy", "attach", "{alias}", "readwrite"],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "policy",
            "attach",
            "{alias}",
            "readwrite",
            "-u",
            "liza",
            "-g",
            GROUP_B,
        ],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "policy",
            "attach",
            "{alias}",
            "nosuchpolicy",
            "-u",
            "liza",
        ],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "policy",
            "attach",
            "{alias}",
            "readwrite",
            "-u",
            "nosuchuser",
        ],
    );
    both(
        &p,
        &["idp", "ldap", "policy", "detach", "{alias}", "readwrite"],
    );
    both(&p, &["idp", "ldap", "policy", "entities", "nosuchalias"]);
}

// ---------------------------------------------------------------------------
// LDAP access keys
// ---------------------------------------------------------------------------

#[test]
fn ldap_accesskey_create_info_edit_remove() {
    let _guard = idp_support::serial();
    let Some((_env, mut p)) = ldap_parity() else {
        return;
    };
    // fahim needs a policy to own access keys (readwrite only: a deterministic policy dump).
    p.mc_json(&[
        "idp",
        "ldap",
        "policy",
        "attach",
        &common::live::alias_name(),
        "readwrite",
        "-u",
        "fahim",
    ]);
    for key in ["{uniq}k", "{uniq}j"] {
        p.cleanup_on_drop(&["idp", "ldap", "accesskey", "rm", "{alias}", key]);
    }

    p.assert_parity(
        &[
            "idp",
            "ldap",
            "accesskey",
            "create",
            "{alias}",
            "fahim",
            "--access-key",
            "{uniq}k",
            "--secret-key",
            "{uniq}secret",
            "--name",
            "nm",
            "--description",
            "first key",
        ],
        None,
    );
    p.assert_json_parity(
        &[
            "--json",
            "idp",
            "ldap",
            "accesskey",
            "create",
            "{alias}",
            "fahim",
            "--access-key",
            "{uniq}j",
            "--secret-key",
            "{uniq}secret",
            "--expiry-duration",
            "24h",
        ],
        None,
    );
    both(
        &p,
        &["idp", "ldap", "accesskey", "info", "{alias}", "{uniq}k"],
    );
    p.assert_parity(
        &[
            "idp",
            "ldap",
            "accesskey",
            "info",
            "{alias}",
            "{uniq}j",
            "{uniq}k",
        ],
        None,
    );
    both(
        &p,
        &["idp", "ldap", "accesskey", "info", "{alias}", "nosuchkey"],
    );

    p.assert_parity(
        &[
            "idp",
            "ldap",
            "accesskey",
            "edit",
            "{alias}",
            "{uniq}k",
            "--name",
            "nm2",
        ],
        None,
    );
    p.assert_json_parity(
        &[
            "--json",
            "idp",
            "ldap",
            "accesskey",
            "edit",
            "{alias}",
            "{uniq}k",
            "--description",
            "edited",
            "--expiry-duration",
            "48h",
        ],
        None,
    );
    p.assert_parity(
        &["idp", "ldap", "accesskey", "info", "{alias}", "{uniq}k"],
        None,
    );
    both(
        &p,
        &["idp", "ldap", "accesskey", "edit", "{alias}", "{uniq}k"],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "edit",
            "{alias}",
            "{uniq}k",
            "--expiry",
            "2030-01-01",
            "--expiry-duration",
            "1h",
        ],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "edit",
            "{alias}",
            "{uniq}k",
            "--expiry",
            "2030-13-01",
        ],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "edit",
            "{alias}",
            "{uniq}k",
            "--expiry",
            "2001-01-01",
        ],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "edit",
            "{alias}",
            "{uniq}k",
            "--name",
            "1bad",
        ],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "edit",
            "{alias}",
            "nosuchkey",
            "--name",
            "x",
        ],
    );

    p.assert_parity(
        &["idp", "ldap", "accesskey", "disable", "{alias}", "{uniq}k"],
        None,
    );
    p.assert_json_parity(
        &[
            "--json",
            "idp",
            "ldap",
            "accesskey",
            "enable",
            "{alias}",
            "{uniq}k",
        ],
        None,
    );
    both(&p, &["idp", "ldap", "accesskey", "enable", "{alias}"]);

    // The server lists keys in random order: compare text lines as sets, JSON only for
    // documents without key lists.
    p.unordered = true;
    p.assert_parity(
        &[
            "idp",
            "ldap",
            "accesskey",
            "list",
            "{alias}",
            "fahim",
            "--svcacc-only",
        ],
        None,
    );
    p.assert_parity(
        &[
            "idp",
            "ldap",
            "accesskey",
            "ls",
            "{alias}",
            DN_FAHIM,
            "liza",
        ],
        None,
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "list",
            "{alias}",
            "--users-only",
        ],
    );
    p.unordered = false;

    p.assert_parity(
        &["idp", "ldap", "accesskey", "remove", "{alias}", "{uniq}k"],
        None,
    );
    p.assert_json_parity(
        &[
            "--json",
            "idp",
            "ldap",
            "accesskey",
            "rm",
            "{alias}",
            "{uniq}j",
        ],
        None,
    );
    both(
        &p,
        &["idp", "ldap", "accesskey", "rm", "{alias}", "{uniq}k"],
    );
}

#[test]
fn ldap_accesskey_errors() {
    let _guard = idp_support::serial();
    let Some((_env, p)) = ldap_parity() else {
        return;
    };
    p.file("empty.json", r#"{"Version":"2012-10-17","Statement":[]}"#);
    p.file("bad.json", "xx");
    let create = ["idp", "ldap", "accesskey", "create", "{alias}"];
    for extra in [
        &["--login"][..],
        &["nosuchuser"],
        &[DN_FAHIM],
        &["fahim", "--expiry", "2027-13-01"],
        &["fahim", "--expiry-duration", "1h", "--expiry", "2027-01-01"],
        &["fahim", "--policy", "nosuch.json"],
        &["fahim", "--policy", "empty.json"],
        &["fahim", "--policy", "bad.json"],
        &["fahim", "--name", "9lives"],
    ] {
        both(&p, &[&create[..], extra].concat());
    }
    both(
        &p,
        &["idp", "ldap", "accesskey", "create-with-login", "{url}"],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "list",
            "{alias}",
            "--users-only",
            "--temp-only",
        ],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "list",
            "{alias}",
            "--self",
            "--all",
        ],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "list",
            "{alias}",
            "fahim",
            "--all",
        ],
    );
    both(&p, &["idp", "ldap", "accesskey", "list", "nosuchalias"]);
    both(
        &p,
        &["idp", "ldap", "accesskey", "rm", "{alias}", "nosuchkey"],
    );
}

#[test]
fn ldap_accesskey_sts_revoke() {
    let _guard = idp_support::serial();
    let Some((env, mut p)) = ldap_parity() else {
        return;
    };
    p.mc_json(&[
        "idp",
        "ldap",
        "policy",
        "attach",
        &common::live::alias_name(),
        "readwrite",
        "-u",
        "fahim",
    ]);
    idp_support::ldap_sts(&env, "fahim", "fahim123");
    p.unordered = true;
    p.assert_parity(
        &[
            "idp",
            "ldap",
            "accesskey",
            "list",
            "{alias}",
            "fahim",
            "--temp-only",
        ],
        None,
    );
    p.unordered = false;
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "sts-revoke",
            "{alias}",
            "fahim",
            "--all",
        ],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "sts-revoke",
            "{alias}",
            DN_FAHIM,
            "--token-type",
            "app-1",
        ],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "sts-revoke",
            "{alias}",
            "--self",
            "--all",
        ],
    );
    both(
        &p,
        &["idp", "ldap", "accesskey", "sts-revoke", "{alias}", "fahim"],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "sts-revoke",
            "{alias}",
            "fahim",
            "--all",
            "--token-type",
            "x",
        ],
    );
    both(
        &p,
        &["idp", "ldap", "accesskey", "sts-revoke", "{alias}", "--all"],
    );
    both(
        &p,
        &[
            "idp",
            "ldap",
            "accesskey",
            "sts-revoke",
            "{alias}",
            "fahim",
            "--self",
        ],
    );
}

// ---------------------------------------------------------------------------
// OpenID
// ---------------------------------------------------------------------------

#[test]
fn openid_config_read() {
    let _guard = idp_support::serial();
    let Some((_env, _arn, p)) = oidc_parity() else {
        return;
    };
    both(&p, &["idp", "openid", "list", "{alias}"]);
    both(&p, &["idp", "openid", "info", "{alias}"]);
    both(&p, &["idp", "openid", "info", "{alias}", "_"]);
    both(&p, &["idp", "openid", "info", "{alias}", "nosuchcfg"]);
    both(&p, &["idp", "openid", "disable", "{alias}", "nosuchcfg"]);
    both(&p, &["idp", "openid", "remove", "{alias}", "nosuchcfg"]);
    both(
        &p,
        &[
            "idp",
            "openid",
            "update",
            "{alias}",
            "nosuchcfg",
            "scopes=openid",
        ],
    );
    both(&p, &["idp", "openid", "list", "nosuchalias"]);
}

#[test]
fn openid_config_write() {
    let _guard = idp_support::serial();
    let Some((env, _arn, mut p)) = oidc_parity() else {
        return;
    };
    // Role ARNs are derived from each side's own client ID.
    p.normalizer.rule(
        r"arn:minio:iam:::role/[A-Za-z0-9_-]{27}",
        "arn:minio:iam:::role/<ROLE-ID-27-CHARACTERS-LONG>",
    );
    // Each side adds its own named configuration (names and client IDs differ per side).
    let args = |client: &str| {
        vec![
            format!("client_id={client}"),
            format!("client_secret={}", env.client_secret),
            format!("config_url={}", env.config_url),
            "role_policy=readonly".to_string(),
        ]
    };
    let mc_args = args("{uniq}cl");
    p.assert_parity(
        &owned(&["idp", "openid", "add", "{alias}", "{uniq}c"], &mc_args),
        None,
    );
    // Same client ID as the default config.
    both(
        &p,
        &owned(
            &["idp", "openid", "add", "{alias}", "{uniq}d"],
            &args(&env.client_id),
        ),
    );
    both(
        &p,
        &["idp", "openid", "add", "{alias}", "{uniq}d", "client_id=x"],
    );
    // Pending (not yet active) configurations cannot be changed yet.
    both(&p, &["idp", "openid", "info", "{alias}", "{uniq}c"]);
    idp_support::restart(&env.server);

    both(&p, &["idp", "openid", "info", "{alias}", "{uniq}c"]);
    both(&p, &["idp", "openid", "list", "{alias}"]);
    p.assert_parity(
        &[
            "idp",
            "openid",
            "update",
            "{alias}",
            "{uniq}c",
            "scopes=openid,email",
        ],
        None,
    );
    p.assert_json_parity(
        &[
            "--json",
            "idp",
            "openid",
            "update",
            "{alias}",
            "{uniq}c",
            "scopes=openid",
        ],
        None,
    );
    both(&p, &["idp", "openid", "disable", "{alias}", "{uniq}c"]);
    both(&p, &["idp", "openid", "enable", "{alias}", "{uniq}c"]);
    p.assert_parity(&["idp", "openid", "rm", "{alias}", "{uniq}c"], None);
    idp_support::restart(&env.server);
    both(&p, &["idp", "openid", "list", "{alias}"]);
}

#[test]
fn openid_accesskeys() {
    let _guard = idp_support::serial();
    let Some((env, arn, mut p)) = oidc_parity() else {
        return;
    };
    let sts = idp_support::openid_sts(&env, &arn);
    for side in [&p.mc, &p.mx] {
        idp_support::add_service_account_as(&env.server, &sts, &format!("{}o", side.uniq));
    }
    p.cleanup_on_drop(&["idp", "openid", "accesskey", "rm", "{alias}", "{uniq}o"]);

    both(
        &p,
        &["idp", "openid", "accesskey", "info", "{alias}", "{uniq}o"],
    );
    both(
        &p,
        &["idp", "openid", "accesskey", "info", "{alias}", "nosuchkey"],
    );
    p.assert_parity(
        &[
            "idp",
            "openid",
            "accesskey",
            "edit",
            "{alias}",
            "{uniq}o",
            "--name",
            "renamed",
        ],
        None,
    );
    both(
        &p,
        &["idp", "openid", "accesskey", "info", "{alias}", "{uniq}o"],
    );
    both(
        &p,
        &["idp", "openid", "accesskey", "edit", "{alias}", "{uniq}o"],
    );
    p.assert_parity(
        &[
            "idp",
            "openid",
            "accesskey",
            "disable",
            "{alias}",
            "{uniq}o",
        ],
        None,
    );
    p.assert_json_parity(
        &[
            "--json",
            "idp",
            "openid",
            "accesskey",
            "enable",
            "{alias}",
            "{uniq}o",
        ],
        None,
    );

    p.unordered = true;
    p.assert_parity(&["idp", "openid", "accesskey", "list", "{alias}"], None);
    p.assert_parity(
        &[
            "idp",
            "openid",
            "accesskey",
            "ls",
            "{alias}",
            "--svcacc-only",
        ],
        None,
    );
    both(
        &p,
        &[
            "idp",
            "openid",
            "accesskey",
            "list",
            "{alias}",
            "--users-only",
            "--all-configs",
        ],
    );
    p.unordered = false;
    both(
        &p,
        &["idp", "openid", "accesskey", "list", "{alias}:nosuchcfg"],
    );
    both(
        &p,
        &[
            "idp",
            "openid",
            "accesskey",
            "list",
            "{alias}:x",
            "--all-configs",
        ],
    );
    both(
        &p,
        &[
            "idp",
            "openid",
            "accesskey",
            "list",
            "{alias}",
            "--svcacc-only",
            "--temp-only",
        ],
    );

    p.assert_parity(
        &["idp", "openid", "accesskey", "rm", "{alias}", "{uniq}o"],
        None,
    );
    both(
        &p,
        &["idp", "openid", "accesskey", "rm", "{alias}", "{uniq}o"],
    );
}
