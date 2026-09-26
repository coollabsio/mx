//! Live checks of the shared admin API helpers (`mx::s3::admin`, `mx::s3::listen`) against a
//! real server: encrypted responses (argon2id), trace streaming, bucket notification listening.

mod common;

use common::live::Live;
use mx::s3::admin::AdminClient;
use mx::s3::listen::{NotificationInfo, listen};
use std::time::Duration;

fn client(live: &Live) -> AdminClient {
    AdminClient::new(&live.alias_config()).expect("admin client")
}

#[test]
fn decrypts_encrypted_admin_responses() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let user = format!("mxapi{}", std::process::id());
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        client
            .request("PUT", "add-user")
            .query("accessKey", &user)
            .encrypted_json(&serde_json::json!({"secretKey": "secret12345", "status": "enabled"}))?
            .send()
            .await?;
        // `list-users` answers with madmin.EncryptData output (argon2id on MinIO).
        let users: serde_json::Map<String, serde_json::Value> = client
            .request("GET", "list-users")
            .decrypt()
            .send_json()
            .await?;
        assert_eq!(users[&user]["status"], "enabled");
        client
            .delete("remove-user", &[("accessKey", user.as_str())])
            .await?;
        let err = client
            .get_json::<serde_json::Value>("user-info", &[("accessKey", user.as_str())])
            .await
            .unwrap_err();
        assert_eq!(
            mx::error::mc_error(&err).unwrap().code.as_deref(),
            Some("XMinioAdminNoSuchUser")
        );
        anyhow::Ok(())
    })
    .unwrap();
}

#[test]
fn streams_trace_entries() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut stream = rt
        .block_on(
            client
                .request("GET", "trace")
                .query("s3", "true")
                .query("err", "false")
                .query("threshold", "0s")
                .stream(),
        )
        .unwrap();
    // Generate S3 traffic until an entry shows up.
    let target = live.bucket_target();
    let entry = rt
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let traffic = live.cmd().args(["ls", &target]).output();
                    assert!(traffic.is_ok());
                    let next = tokio::time::timeout(
                        Duration::from_secs(2),
                        stream.next::<serde_json::Value>(),
                    )
                    .await;
                    if let Ok(entry) = next {
                        return entry;
                    }
                }
            })
            .await
        })
        .expect("trace entry within 30s")
        .unwrap()
        .expect("stream open");
    assert!(entry.get("funcname").is_some(), "{entry}");
}

#[test]
fn listens_for_bucket_notifications() {
    let Some(live) = Live::new() else { return };
    let client = client(&live);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut stream = rt
        .block_on(listen(
            &client,
            &live.bucket,
            "dir/",
            "",
            &["s3:ObjectCreated:*"],
        ))
        .unwrap();
    let file = live.local_file("hello.txt", "hello");
    live.cmd()
        .args(["cp", file.to_str().unwrap(), &live.url("dir/hello.txt")])
        .assert()
        .success();
    let event = rt
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    match stream.next::<NotificationInfo>().await? {
                        Some(info) => {
                            if let Some(event) = info.records.unwrap_or_default().into_iter().next()
                            {
                                return anyhow::Ok(event);
                            }
                        }
                        None => anyhow::bail!("stream closed"),
                    }
                }
            })
            .await
        })
        .expect("event within 30s")
        .unwrap();
    assert_eq!(event.s3.bucket.name, live.bucket);
    assert_eq!(event.s3.object.key, "dir/hello.txt");
    assert_eq!(event.s3.object.size, 5);
    assert!(event.event_name.starts_with("s3:ObjectCreated:"));

    let err = rt
        .block_on(listen(&client, "mx-no-such-bucket-xyz", "", "", &[]))
        .err()
        .unwrap();
    assert_eq!(
        mx::error::mc_error(&err).unwrap().message,
        "The specified bucket does not exist"
    );
}
