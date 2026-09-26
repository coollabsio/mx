//! Bucket-level operations: mb/rb, versioning, CORS, encryption, policy, tags, presign, ping
//! (area F, except mb/rb).

use super::{build_client, delete_all_versions, delete_keys, list_object_infos};
use crate::config::model::AliasConfig;
use anyhow::Result;
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::types::{
    BucketLocationConstraint, BucketVersioningStatus, CorsConfiguration, CorsRule,
    CreateBucketConfiguration, ServerSideEncryption, ServerSideEncryptionByDefault,
    ServerSideEncryptionConfiguration, ServerSideEncryptionRule, Tag, Tagging,
    VersioningConfiguration,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Options for [`make_bucket_with`].
#[derive(Debug, Clone, Default)]
pub struct MakeBucketOptions {
    pub ignore_existing: bool,
    pub with_versioning: bool,
    /// Enables object lock (S3 implies versioning).
    pub with_lock: bool,
    pub region: Option<String>,
}

pub async fn make_bucket(alias: &AliasConfig, bucket: &str, ignore_existing: bool) -> Result<()> {
    make_bucket_with(
        alias,
        bucket,
        &MakeBucketOptions {
            ignore_existing,
            ..Default::default()
        },
    )
    .await
}

pub async fn make_bucket_with(
    alias: &AliasConfig,
    bucket: &str,
    options: &MakeBucketOptions,
) -> Result<()> {
    let client = build_client(alias).await?;
    let mut request = client.create_bucket().bucket(bucket);
    if options.with_lock {
        request = request.object_lock_enabled_for_bucket(true);
    }
    if let Some(region) = options
        .region
        .as_deref()
        .filter(|region| !region.is_empty() && *region != "us-east-1")
    {
        request = request.create_bucket_configuration(
            CreateBucketConfiguration::builder()
                .location_constraint(BucketLocationConstraint::from(region))
                .build(),
        );
    }
    let mut created = true;
    if let Err(error) = request.send().await {
        let already_exists = error
            .as_service_error()
            .and_then(ProvideErrorMetadata::code)
            .is_some_and(|code| matches!(code, "BucketAlreadyExists" | "BucketAlreadyOwnedByYou"));
        if !(options.ignore_existing && already_exists) {
            return Err(error.into());
        }
        created = false;
    }
    if created && options.with_versioning && !options.with_lock {
        set_versioning_client(&client, bucket, true).await?;
    }
    Ok(())
}

pub async fn remove_bucket(alias: &AliasConfig, bucket: &str, force: bool) -> Result<()> {
    let client = build_client(alias).await?;
    if force {
        delete_all_versions(&client, bucket).await?;
        let keys = list_object_infos(alias, bucket, None)
            .await?
            .into_iter()
            .map(|item| item.key)
            .collect::<Vec<_>>();
        delete_keys(&client, bucket, &keys).await?;
    }
    client.delete_bucket().bucket(bucket).send().await?;
    Ok(())
}

pub async fn ping(alias: &AliasConfig) -> Result<Duration> {
    let client = build_client(alias).await?;
    let started = std::time::Instant::now();
    client.list_buckets().send().await?;
    Ok(started.elapsed())
}

pub async fn presign_get(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    expire: Duration,
) -> Result<String> {
    let client = build_client(alias).await?;
    let request = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .presigned(PresigningConfig::expires_in(expire)?)
        .await?;
    Ok(request.uri().to_string())
}

pub async fn presign_put(
    alias: &AliasConfig,
    bucket: &str,
    key: &str,
    expire: Duration,
) -> Result<String> {
    let client = build_client(alias).await?;
    let request = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .presigned(PresigningConfig::expires_in(expire)?)
        .await?;
    Ok(request.uri().to_string())
}

pub async fn put_object_tags(
    alias: &AliasConfig,
    bucket: &str,
    key: Option<&str>,
    tags: Vec<(String, String)>,
) -> Result<()> {
    let client = build_client(alias).await?;
    let tag_set = tags
        .into_iter()
        .map(|(key, value)| Tag::builder().key(key).value(value).build())
        .collect::<Result<Vec<_>, _>>()?;
    let tagging = Tagging::builder().set_tag_set(Some(tag_set)).build()?;
    if let Some(key) = key {
        client
            .put_object_tagging()
            .bucket(bucket)
            .key(key)
            .tagging(tagging)
            .send()
            .await?;
    } else {
        client
            .put_bucket_tagging()
            .bucket(bucket)
            .tagging(tagging)
            .send()
            .await?;
    }
    Ok(())
}

pub async fn get_object_tags(
    alias: &AliasConfig,
    bucket: &str,
    key: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let client = build_client(alias).await?;
    let tags = if let Some(key) = key {
        client
            .get_object_tagging()
            .bucket(bucket)
            .key(key)
            .send()
            .await?
            .tag_set
    } else {
        client
            .get_bucket_tagging()
            .bucket(bucket)
            .send()
            .await?
            .tag_set
    };
    Ok(tags.into_iter().map(|tag| (tag.key, tag.value)).collect())
}

pub async fn delete_object_tags(
    alias: &AliasConfig,
    bucket: &str,
    key: Option<&str>,
) -> Result<()> {
    let client = build_client(alias).await?;
    if let Some(key) = key {
        client
            .delete_object_tagging()
            .bucket(bucket)
            .key(key)
            .send()
            .await?;
    } else {
        client.delete_bucket_tagging().bucket(bucket).send().await?;
    }
    Ok(())
}

pub async fn set_versioning(alias: &AliasConfig, bucket: &str, enabled: bool) -> Result<()> {
    let client = build_client(alias).await?;
    set_versioning_client(&client, bucket, enabled).await
}

pub async fn set_versioning_client(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    enabled: bool,
) -> Result<()> {
    let status = if enabled {
        BucketVersioningStatus::Enabled
    } else {
        BucketVersioningStatus::Suspended
    };
    client
        .put_bucket_versioning()
        .bucket(bucket)
        .versioning_configuration(VersioningConfiguration::builder().status(status).build())
        .send()
        .await?;
    Ok(())
}

pub async fn get_versioning(alias: &AliasConfig, bucket: &str) -> Result<String> {
    let client = build_client(alias).await?;
    let response = client.get_bucket_versioning().bucket(bucket).send().await?;
    Ok(response
        .status()
        .map(|status| status.as_str().to_string())
        .unwrap_or_else(|| "Off".to_string()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsDocument {
    #[serde(rename = "CORSRules")]
    pub rules: Vec<CorsDocumentRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsDocumentRule {
    #[serde(rename = "AllowedOrigins", default)]
    pub allowed_origins: Vec<String>,
    #[serde(rename = "AllowedMethods", default)]
    pub allowed_methods: Vec<String>,
    #[serde(rename = "AllowedHeaders", default)]
    pub allowed_headers: Vec<String>,
    #[serde(rename = "ExposeHeaders", default)]
    pub expose_headers: Vec<String>,
    #[serde(rename = "MaxAgeSeconds")]
    pub max_age_seconds: Option<i32>,
}

pub async fn put_cors(alias: &AliasConfig, bucket: &str, document: CorsDocument) -> Result<()> {
    let client = build_client(alias).await?;
    let mut rules = Vec::new();
    for rule in document.rules {
        let mut builder = CorsRule::builder()
            .set_allowed_origins(Some(rule.allowed_origins))
            .set_allowed_methods(Some(rule.allowed_methods))
            .set_allowed_headers(Some(rule.allowed_headers))
            .set_expose_headers(Some(rule.expose_headers));
        if let Some(max_age) = rule.max_age_seconds {
            builder = builder.max_age_seconds(max_age);
        }
        rules.push(builder.build()?);
    }
    client
        .put_bucket_cors()
        .bucket(bucket)
        .cors_configuration(
            CorsConfiguration::builder()
                .set_cors_rules(Some(rules))
                .build()?,
        )
        .send()
        .await?;
    Ok(())
}

pub async fn get_cors(alias: &AliasConfig, bucket: &str) -> Result<CorsDocument> {
    let client = build_client(alias).await?;
    let response = client.get_bucket_cors().bucket(bucket).send().await?;
    Ok(CorsDocument {
        rules: response
            .cors_rules
            .unwrap_or_default()
            .into_iter()
            .map(|rule| CorsDocumentRule {
                allowed_origins: rule.allowed_origins,
                allowed_methods: rule.allowed_methods,
                allowed_headers: rule.allowed_headers.unwrap_or_default(),
                expose_headers: rule.expose_headers.unwrap_or_default(),
                max_age_seconds: rule.max_age_seconds,
            })
            .collect(),
    })
}

pub async fn delete_cors(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client.delete_bucket_cors().bucket(bucket).send().await?;
    Ok(())
}

pub async fn put_encryption_s3(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    let default = ServerSideEncryptionByDefault::builder()
        .sse_algorithm(ServerSideEncryption::Aes256)
        .build()?;
    client
        .put_bucket_encryption()
        .bucket(bucket)
        .server_side_encryption_configuration(
            ServerSideEncryptionConfiguration::builder()
                .rules(
                    ServerSideEncryptionRule::builder()
                        .apply_server_side_encryption_by_default(default)
                        .build(),
                )
                .build()?,
        )
        .send()
        .await?;
    Ok(())
}

pub async fn get_encryption(alias: &AliasConfig, bucket: &str) -> Result<String> {
    let client = build_client(alias).await?;
    let response = client.get_bucket_encryption().bucket(bucket).send().await?;
    let algorithm = response
        .server_side_encryption_configuration
        .and_then(|config| config.rules.into_iter().next())
        .and_then(|rule| rule.apply_server_side_encryption_by_default)
        .map(|default| default.sse_algorithm.as_str().to_string())
        .unwrap_or_else(|| "none".to_string());
    Ok(algorithm)
}

pub async fn delete_encryption(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client
        .delete_bucket_encryption()
        .bucket(bucket)
        .send()
        .await?;
    Ok(())
}

pub async fn put_bucket_policy(alias: &AliasConfig, bucket: &str, policy: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client
        .put_bucket_policy()
        .bucket(bucket)
        .policy(policy)
        .send()
        .await?;
    Ok(())
}

pub async fn get_bucket_policy(alias: &AliasConfig, bucket: &str) -> Result<Option<String>> {
    let client = build_client(alias).await?;
    match client.get_bucket_policy().bucket(bucket).send().await {
        Ok(response) => Ok(response.policy),
        Err(error)
            if error
                .as_service_error()
                .and_then(ProvideErrorMetadata::code)
                .is_some_and(|code| code == "NoSuchBucketPolicy") =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn delete_bucket_policy(alias: &AliasConfig, bucket: &str) -> Result<()> {
    let client = build_client(alias).await?;
    client.delete_bucket_policy().bucket(bucket).send().await?;
    Ok(())
}
