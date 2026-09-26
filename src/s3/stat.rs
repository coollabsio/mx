//! HeadBucket / HeadObject (area C).

use super::{GetOptions, build_client, debug_timestamp};
use crate::config::model::AliasConfig;
use crate::target::TargetRef;
use anyhow::Result;
use aws_sdk_s3::Client;
use aws_sdk_s3::operation::head_object::HeadObjectOutput;

#[derive(Debug, Clone)]
pub enum S3Stat {
    Bucket {
        bucket: String,
    },
    Object {
        bucket: String,
        key: String,
        size: Option<i64>,
        last_modified: Option<String>,
        etag: Option<String>,
        content_type: Option<String>,
        storage_class: Option<String>,
    },
}

pub async fn stat_target(alias: &AliasConfig, target: &TargetRef) -> Result<S3Stat> {
    let client = build_client(alias).await?;
    let bucket = target.require_bucket()?;

    if target.key.is_none() {
        client.head_bucket().bucket(bucket).send().await?;
        return Ok(S3Stat::Bucket {
            bucket: bucket.to_string(),
        });
    }

    let key = target.require_object_key()?;
    let response = head_object_with(&client, bucket, &key, &GetOptions::default()).await?;
    Ok(S3Stat::Object {
        bucket: bucket.to_string(),
        key,
        size: response.content_length(),
        last_modified: response.last_modified().map(debug_timestamp),
        etag: response.e_tag().map(str::to_string),
        content_type: response.content_type().map(str::to_string),
        storage_class: response
            .storage_class()
            .map(|value| value.as_str().to_string()),
    })
}

/// HeadObject honoring `version_id`, `sse_c`, `part_number` and `zip_extract` from
/// [`GetOptions`] (`range` is ignored).
pub async fn head_object_with(
    client: &Client,
    bucket: &str,
    key: &str,
    options: &GetOptions,
) -> Result<HeadObjectOutput> {
    let mut request = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(options.version_id.clone())
        .set_part_number(options.part_number);
    if let Some(key) = &options.sse_c {
        let (algorithm, encoded, md5) = super::objects::sse_c_headers(key);
        request = request
            .sse_customer_algorithm(algorithm)
            .sse_customer_key(encoded)
            .sse_customer_key_md5(md5);
    }
    if options.zip_extract {
        Ok(request
            .customize()
            .mutate_request(super::objects::add_zip_extract_header)
            .send()
            .await?)
    } else {
        Ok(request.send().await?)
    }
}
