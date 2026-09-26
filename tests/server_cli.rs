//! Offline CLI tests for the SERVER area: `admin service|update|info|config|prometheus|kms|
//! scanner status|cluster`, the hidden `admin tier|bucket|profile|subnet|health` and `update`.
//! Nothing here needs a server (unreachable aliases, argument validation, local-only output).

use assert_cmd::Command;

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    /// Adds `alias` without probing the server (config file only).
    fn with_alias(alias: &str, url: &str) -> Self {
        let home = Self::new();
        let dir = home.dir.path().join(".mx");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            format!(
                r#"{{"version":"10","aliases":{{"{alias}":{{"url":"{url}","accessKey":"minioadmin","secretKey":"minioadmin","api":"s3v4","path":"auto"}}}}}}"#
            ),
        )
        .unwrap();
        home
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::cargo_bin("mx")
            .expect("binary")
            .env("HOME", self.dir.path())
            .current_dir(self.dir.path())
            .env_remove("MC_HOST_dead")
            .args(args)
            .output()
            .expect("run mx")
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Closed local port: every request fails with `connection refused`.
const DEAD: &str = "http://127.0.0.1:1";

#[test]
fn unknown_alias_is_an_admin_connection_error() {
    let home = Home::new();
    for (args, message) in [
        (
            vec!["admin", "info", "nosuch"],
            "Unable to initialize admin connection.",
        ),
        (
            vec!["admin", "service", "unfreeze", "nosuch"],
            "Unable to initialize admin connection.",
        ),
        (
            vec!["admin", "config", "get", "nosuch", "region"],
            "Unable to initialize admin connection.",
        ),
        (
            vec!["admin", "kms", "key", "create", "nosuch", "k"],
            "Cannot get a configured admin connection.",
        ),
        (
            vec!["admin", "kms", "key", "status", "nosuch"],
            "Unable to get a configured admin connection.",
        ),
        (
            vec!["admin", "scanner", "status", "nosuch"],
            "Unable to initialize admin client.",
        ),
        (
            vec!["admin", "cluster", "iam", "export", "nosuch"],
            "Unable to initialize admin client.",
        ),
    ] {
        let out = home.run(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(
            text(&out.stderr),
            format!(
                "mx: <ERROR> {message} No valid configuration found for 'nosuch' host alias.\n"
            ),
            "{args:?}"
        );
    }
}

#[test]
fn admin_info_json_reports_errors_inside_the_document() {
    let home = Home::with_alias("dead", DEAD);
    let out = home.run(&["--json", "admin", "info", "dead"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout),
        "{\"status\":\"error\",\"error\":\"Get \\\"http://127.0.0.1:1/minio/admin/v3/info?metrics=false\\\": dial tcp 127.0.0.1:1: connect: connection refused\",\"info\":{\"buckets\":{\"count\":0},\"objects\":{\"count\":0},\"versions\":{\"count\":0},\"deletemarkers\":{\"count\":0},\"usage\":{\"size\":0},\"services\":{\"kms\":{},\"ldap\":{}},\"backend\":{\"backendType\":\"\",\"onlineDisks\":0,\"offlineDisks\":0,\"standardSCParity\":0,\"rrSCParity\":0,\"totalSets\":null,\"totalDrivesPerSet\":null}}}\n"
    );
    let out = home.run(&["admin", "info", "dead"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        "mx: <ERROR> Unable to get service info. Get \"http://127.0.0.1:1/minio/admin/v3/info?metrics=false\": dial tcp 127.0.0.1:1: connect: connection refused.\n"
    );
}

#[test]
fn request_failures_use_mc_messages() {
    let home = Home::with_alias("dead", DEAD);
    for (args, message) in [
        (
            vec!["admin", "service", "unfreeze", "dead"],
            "Unable to unfreeze the server.",
        ),
        (
            vec!["admin", "service", "freeze", "dead"],
            "Unable to freeze the server.",
        ),
        (
            vec!["admin", "service", "stop", "dead"],
            "Unable to stop the server.",
        ),
        (
            vec!["--json", "admin", "service", "restart", "dead", "--dry-run"],
            "Unable to restart the server.",
        ),
        (
            vec!["admin", "update", "dead", "-y"],
            "Unable to update the server.",
        ),
        (
            vec!["admin", "config", "export", "dead"],
            "Unable to get server config",
        ),
        (
            vec!["admin", "config", "history", "dead"],
            "Unable to list server history configuration.",
        ),
        (
            vec!["admin", "config", "restore", "dead", "x"],
            "Unable to restore server configuration.",
        ),
        (
            vec!["admin", "kms", "key", "list", "dead"],
            "Unable to list KMS keys",
        ),
        (
            vec!["admin", "cluster", "bucket", "export", "dead"],
            "Unable to export bucket metadata.",
        ),
    ] {
        let out = home.run(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let all = text(&out.stderr) + &text(&out.stdout);
        assert!(all.contains(message), "{args:?}: {all}");
        assert!(all.contains("connection refused"), "{args:?}: {all}");
    }
}

#[test]
fn config_messages_keep_go_format_in_json() {
    let home = Home::new();
    let out = home.run(&["admin", "config", "reset", "nosuch", "region", "name=x"]);
    // The alias fails first, like mc.
    assert!(text(&out.stderr).contains("Unable to initialize admin connection."));
    let home = Home::with_alias("dead", DEAD);
    let out = home.run(&["admin", "config", "reset", "dead", "region", "name=x"]);
    assert_eq!(
        text(&out.stderr),
        "mx: <ERROR> Unable to reset 'region' on the server: new settings may not be provided for sub-system keys.\n"
    );
    let out = home.run(&[
        "--json", "admin", "config", "reset", "dead", "region", "name=x",
    ]);
    assert_eq!(
        text(&out.stdout),
        "{\"status\":\"error\",\"error\":{\"message\":\"Unable to reset '%s' on the server\",\"cause\":{\"message\":\"new settings may not be provided for sub-system keys\",\"error\":{}},\"type\":\"fatal\"}}\n"
    );
}

#[test]
fn restart_and_scanner_text_need_a_terminal() {
    let home = Home::with_alias("dead", DEAD);
    for (args, message) in [
        (
            vec!["admin", "service", "restart", "dead"],
            "Unable to initialize service restart UI",
        ),
        (
            vec!["admin", "scanner", "status", "dead"],
            "Unable to fetch scanner metrics",
        ),
    ] {
        let out = Command::cargo_bin("mx")
            .unwrap()
            .env("HOME", home.dir.path())
            .args(&args)
            .output()
            .unwrap();
        let stderr = text(&out.stderr);
        // Without a controlling terminal mc's bubbletea UI cannot start; with one, the
        // request fails instead (unreachable server).
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.contains(message) || stderr.contains("connection refused"),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn prometheus_generate_is_local() {
    let home = Home::with_alias("prom", "http://127.0.0.1:9000");
    let out = home.run(&[
        "admin",
        "prometheus",
        "generate",
        "prom",
        "node",
        "--public",
    ]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout),
        "scrape_configs:\n- job_name: minio-job-node\n  metrics_path: /minio/v2/metrics/node\n  scheme: http\n  static_configs:\n  - targets: ['127.0.0.1:9000']\n"
    );
    let out = home.run(&["--json", "admin", "prometheus", "generate", "prom/"]);
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["jobName"], "minio-job");
    assert_eq!(doc["metricsPath"], "/minio/v2/metrics/cluster");
    let token = doc["bearerToken"].as_str().unwrap();
    assert!(token.starts_with("eyJhbGciOiJIUzUxMiIsInR5cCI6IkpXVCJ9."));
    let out = home.run(&[
        "admin",
        "prometheus",
        "generate",
        "prom",
        "api",
        "--api-version",
        "v3",
        "--bucket",
        "b",
        "--public",
    ]);
    assert!(text(&out.stdout).contains("  metrics_path: /minio/metrics/v3/bucket/api/b\n"));
}

#[test]
fn prometheus_validation_errors() {
    let home = Home::with_alias("prom", "http://127.0.0.1:9000");
    let invalid =
        "Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.";
    for (args, message) in [
        (
            vec!["admin", "prometheus", "generate", "prom", "bogus"],
            format!("invalid metric type `bogus`. valid values are `bucket, cluster, node, resource`. {invalid}"),
        ),
        (
            vec!["admin", "prometheus", "metrics", "prom", "--bucket", "b"],
            format!("Flag `bucket` is not supported with v2 metrics. {invalid}"),
        ),
        (
            vec!["admin", "prometheus", "generate", "prom", "--api-version", "v4"],
            format!("Invalid api version `v4`. {invalid}"),
        ),
        (
            vec!["admin", "prometheus", "generate", "prom", "--api-version", "v3", "--bucket", "x"],
            format!("metric type must be passed with --bucket. valid values are `api, replication`. {invalid}"),
        ),
        (
            vec!["admin", "prometheus", "generate", "prom/b"],
            "Invalid alias. Alias `prom/b` should have alphanumeric characters such as [helloWorld0, hello_World0, ...] and begin with a letter.".to_string(),
        ),
        (
            vec!["admin", "prometheus", "generate", "nosuch"],
            "No such alias `nosuch` found. Use `mc alias set mycloud nosuch ...` to add an alias. Use the alias for S3 operations.".to_string(),
        ),
    ] {
        let out = home.run(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(text(&out.stderr), format!("mx: <ERROR> {message}\n"), "{args:?}");
    }
}

#[test]
fn cluster_import_checks_the_archive_first() {
    let home = Home::new();
    let out = home.run(&[
        "admin",
        "cluster",
        "bucket",
        "import",
        "nosuch",
        "missing.zip",
    ]);
    assert_eq!(
        text(&out.stderr),
        "mx: <ERROR> Unable to get bucket metadata: open missing.zip: no such file or directory.\n"
    );
    let out = home.run(&[
        "--json",
        "admin",
        "cluster",
        "iam",
        "import",
        "nosuch",
        "missing.zip",
    ]);
    assert_eq!(
        text(&out.stdout),
        "{\"status\":\"error\",\"error\":{\"message\":\"Unable to get IAM info\",\"cause\":{\"message\":\"open missing.zip: no such file or directory\",\"error\":{\"Op\":\"open\",\"Path\":\"missing.zip\",\"Err\":2}},\"type\":\"fatal\"}}\n"
    );
    std::fs::write(home.dir.path().join("bad.zip"), "not a zip").unwrap();
    let out = home.run(&["admin", "cluster", "iam", "import", "nosuch", "bad.zip"]);
    assert_eq!(
        text(&out.stderr),
        "mx: <ERROR> Unable to read zip file bad.zip: zip: not a valid zip file.\n"
    );
}

#[test]
fn missing_arguments_print_help() {
    let home = Home::new();
    for args in [
        vec!["admin", "info"],
        vec!["admin", "service", "restart"],
        vec!["admin", "kms", "key", "create", "a"],
        vec!["admin", "cluster", "bucket", "import", "a"],
        vec!["admin", "update"],
    ] {
        let out = home.run(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(text(&out.stdout).contains("Usage"), "{args:?}");
    }
}

#[test]
fn hidden_deprecated_commands_match_mc() {
    let home = Home::new();
    let cases: &[(&[&str], &str)] = &[
        (&["admin", "tier"], "mx ilm tier"),
        (&["admin", "tier", "bogus"], "mx ilm tier"),
        (&["admin", "profile"], "mx support profile"),
        (
            &["admin", "profile", "start", "a"],
            "mx support profile start",
        ),
        (&["admin", "subnet"], "mx support"),
        (&["admin", "subnet", "register", "a"], "mx support register"),
        (&["admin", "subnet", "health", "a"], "mx support diag a"),
        (&["admin", "health"], "mx support diag"),
        (
            &["admin", "health", "a", "--offline"],
            "mx support diag a --offline",
        ),
        (&["admin", "bucket", "quota", "a/b"], "mx quota"),
        (&["admin", "bucket", "info", "a/b"], "mx stat"),
        (
            &["admin", "bucket", "remote", "add", "a"],
            "mx replicate add",
        ),
        (
            &["admin", "bucket", "remote", "edit", "a"],
            "mx replicate update",
        ),
        (&["admin", "bucket", "remote", "rm", "a"], "mx replicate rm"),
    ];
    for (args, replacement) in cases {
        let out = home.run(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(
            text(&out.stderr),
            format!("mx: <ERROR> Deprecated command. Please use '{replacement}' instead.\n"),
            "{args:?}"
        );
    }
    let out = home.run(&["admin", "bucket", "bogus"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).starts_with(
        "mx: <ERROR> `bogus` is not a recognized command. Get help using `--help` flag."
    ));
    let out = home.run(&["admin", "bucket"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).contains("COMMANDS:\n  remote  configure remote target buckets\n"));
    let out = home.run(&["admin", "health", "a", "--json"]);
    assert_eq!(
        text(&out.stdout),
        "{\"status\":\"error\",\"error\":{\"message\":\"Deprecated command\",\"cause\":{\"message\":\"Please use 'mx support diag a --json' instead\",\"error\":{}},\"type\":\"fatal\"}}\n"
    );
}

#[test]
fn admin_tier_runs_ilm_tier() {
    let home = Home::with_alias("dead", DEAD);
    let out = home.run(&["admin", "tier", "add", "minio", "dead", "X"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        "mx: <ERROR> minio remote tier requires access credentials. Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.\n"
    );
}

#[test]
fn update_is_not_supported() {
    let home = Home::new();
    let out = home.run(&["update"]);
    assert_eq!(out.status.code(), Some(255));
    assert_eq!(
        text(&out.stderr),
        "mx: <ERROR> Unable to update ‘mx’. self-update is not supported; install a newer release manually\n"
    );
    let out = home.run(&["--json", "update"]);
    assert_eq!(out.status.code(), Some(255));
    assert_eq!(
        text(&out.stdout),
        "{\"status\":\"error\",\"error\":{\"message\":\"Unable to update ‘mx’.\",\"cause\":{\"message\":\"self-update is not supported; install a newer release manually\",\"error\":{}},\"type\":\"error\"}}\n"
    );
}
