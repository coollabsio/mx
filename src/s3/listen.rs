//! MinIO bucket notification listening (`GET /bucket?events=...`, minio-go
//! `ListenBucketNotification` / `ListenNotification`), used by `mx watch`.
//!
//! The server keeps the response open and writes one `notification.Info` JSON document per
//! line (`{"Records":[...]}`), plus empty documents/whitespace as keep-alive pings.

use super::admin::{AdminClient, JsonStream, check_s3_status, encode_component};
use anyhow::Result;
use serde::Deserialize;

/// Event names for mc `watch --events` values (`put`, `delete`, `get`, `replica`, `ilm`,
/// `bucket-creation`, `bucket-removal`, `scanner`); `None` for an unknown value.
pub fn event_names(event: &str) -> Option<&'static [&'static str]> {
    Some(match event {
        "put" => &["s3:ObjectCreated:*"],
        "delete" => &["s3:ObjectRemoved:*"],
        "get" => &["s3:ObjectAccessed:*"],
        "replica" => &["s3:Replication:*"],
        "ilm" => &["s3:ObjectRestore:*", "s3:ObjectTransition:*"],
        "bucket-creation" => &["s3:BucketCreated:*"],
        "bucket-removal" => &["s3:BucketRemoved:*"],
        "scanner" => &["s3:Scanner:ManyVersions", "s3:Scanner:BigPrefix"],
        _ => return None,
    })
}

/// One line of the listen stream (`notification.Info`); `records` is empty for pings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NotificationInfo {
    #[serde(rename = "Records", default)]
    pub records: Option<Vec<NotificationEvent>>,
}

/// `notification.Event` (the fields mc `watch` prints; the full JSON is kept in `raw`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationEvent {
    #[serde(default)]
    pub event_time: String,
    #[serde(default)]
    pub event_name: String,
    #[serde(default)]
    pub s3: EventS3,
    #[serde(default)]
    pub source: EventSource,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventS3 {
    #[serde(default)]
    pub bucket: EventBucket,
    #[serde(default)]
    pub object: EventObject,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventBucket {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventObject {
    /// Object key as the server sends it (path-escaped by MinIO, `/` kept).
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub e_tag: String,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub version_id: String,
    #[serde(default)]
    pub user_metadata: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventSource {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: String,
    #[serde(default)]
    pub user_agent: String,
}

/// Opens a listen stream. `bucket` empty listens on all buckets (`ListenNotification`, no
/// prefix/suffix); `events` are S3 event names (see [`event_names`]). Read it with
/// `stream.next::<NotificationInfo>()` / `for_each`. Like minio-go, callers should reconnect
/// when the server closes the stream.
pub async fn listen(
    client: &AdminClient,
    bucket: &str,
    prefix: &str,
    suffix: &str,
    events: &[&str],
) -> Result<JsonStream> {
    let path = if bucket.is_empty() {
        "/".to_string()
    } else {
        format!("/{}/", encode_component(bucket))
    };
    let mut query = vec![("ping", "10")];
    if !bucket.is_empty() {
        query.push(("prefix", prefix));
        query.push(("suffix", suffix));
    }
    // MinIO signs the query with Go `url.Values.Encode` (values in request order) while SigV4
    // signers sort values: send them sorted so both agree.
    let mut events = events.to_vec();
    events.sort_unstable();
    query.extend(events.iter().map(|event| ("events", *event)));
    let (mut response, body) = client
        .send_streaming("GET", &path, &query, &[], Vec::new())
        .await?;
    if !(200..300).contains(&response.status) {
        response.body = body.collect().await?.into_bytes().to_vec();
        return Err(check_s3_status(response, bucket).expect_err("non-2xx status"));
    }
    Ok(JsonStream::new(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3::admin::mock;

    #[test]
    fn maps_watch_event_types() {
        assert_eq!(event_names("put"), Some(&["s3:ObjectCreated:*"][..]));
        assert_eq!(event_names("ilm").unwrap().len(), 2);
        assert!(event_names("bogus").is_none());
    }

    #[test]
    fn listens_and_skips_pings() {
        let server = mock::serve(vec![mock::chunked(
            200,
            &[
                "{\"Records\":null}\n",
                " ",
                r#"{"Records":[{"eventTime":"2026-01-01T00:00:00.000Z","eventName":"s3:ObjectCreated:Put","s3":{"bucket":{"name":"b"},"object":{"key":"a%2Fb.txt","size":5}},"source":{"host":"127.0.0.1","port":"","userAgent":"mc"}}]}"#,
                "\n",
            ],
        )]);
        let client = mock::client(&server, "s");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let events = rt
            .block_on(async {
                let mut stream = listen(
                    &client,
                    "b",
                    "a/",
                    "",
                    &["s3:ObjectCreated:*", "s3:ObjectRemoved:*"],
                )
                .await?;
                let mut events = Vec::new();
                while let Some(info) = stream.next::<NotificationInfo>().await? {
                    events.extend(info.records.unwrap_or_default());
                }
                anyhow::Ok(events)
            })
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].s3.object.key, "a%2Fb.txt");
        assert_eq!(events[0].s3.object.size, 5);
        let head = server.requests().remove(0).head;
        assert!(
            head.starts_with(
                "GET /b/?events=s3%3AObjectCreated%3A%2A&events=s3%3AObjectRemoved%3A%2A&ping=10&prefix=a%2F&suffix= "
            ),
            "{head}"
        );
    }

    #[test]
    fn maps_listen_errors_like_s3() {
        let server = mock::serve(vec![mock::reply(
            404,
            b"<Error><Code>NoSuchBucket</Code><Message>The specified bucket does not exist</Message></Error>",
        )]);
        let client = mock::client(&server, "s");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(listen(&client, "nope", "", "", &[]))
            .err()
            .unwrap();
        let mapped = crate::error::mc_error(&err).unwrap();
        assert_eq!(mapped.message, "The specified bucket does not exist");
        assert!(
            server.requests()[0]
                .head
                .starts_with("GET /nope/?ping=10&prefix=&suffix= ")
        );
    }
}
