//! Bucket lifecycle (ILM) rules (area F; H/G may add tier/restore helpers in their own modules).

use super::build_client;
use crate::config::model::AliasConfig;
use anyhow::Result;
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::types::{
    BucketLifecycleConfiguration, LifecycleExpiration, LifecycleRule, LifecycleRuleFilter,
};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct LifecycleRuleView {
    pub id: String,
    pub prefix: Option<String>,
    pub expire_days: Option<i32>,
    pub status: String,
}

pub async fn add_lifecycle_rule(
    alias: &AliasConfig,
    bucket: &str,
    id: &str,
    prefix: Option<&str>,
    expire_days: i32,
) -> Result<()> {
    let client = build_client(alias).await?;
    let mut rules = get_lifecycle_rules(alias, bucket).await.unwrap_or_default();
    let mut builder = LifecycleRule::builder()
        .id(id)
        .status(aws_sdk_s3::types::ExpirationStatus::Enabled)
        .expiration(LifecycleExpiration::builder().days(expire_days).build());
    if let Some(prefix) = prefix {
        builder = builder.filter(LifecycleRuleFilter::builder().prefix(prefix).build());
    }
    rules.push(builder.build()?);
    client
        .put_bucket_lifecycle_configuration()
        .bucket(bucket)
        .lifecycle_configuration(
            BucketLifecycleConfiguration::builder()
                .set_rules(Some(rules))
                .build()?,
        )
        .send()
        .await?;
    Ok(())
}

pub async fn list_lifecycle_rules(
    alias: &AliasConfig,
    bucket: &str,
) -> Result<Vec<LifecycleRuleView>> {
    let client = build_client(alias).await?;
    let rules = match client
        .get_bucket_lifecycle_configuration()
        .bucket(bucket)
        .send()
        .await
    {
        Ok(response) => response.rules.unwrap_or_default(),
        Err(error)
            if error
                .as_service_error()
                .and_then(ProvideErrorMetadata::code)
                .is_some_and(|code| {
                    matches!(code, "NoSuchLifecycleConfiguration" | "NoSuchBucket")
                }) =>
        {
            Vec::new()
        }
        Err(error) => return Err(error.into()),
    };
    Ok(rules
        .into_iter()
        .map(|rule| LifecycleRuleView {
            id: rule.id.clone().unwrap_or_default(),
            prefix: rule.filter.and_then(|filter| filter.prefix),
            expire_days: rule.expiration.and_then(|expiration| expiration.days),
            status: rule.status.as_str().to_string(),
        })
        .collect())
}

pub async fn remove_lifecycle_rule(alias: &AliasConfig, bucket: &str, id: &str) -> Result<()> {
    let client = build_client(alias).await?;
    let rules = get_lifecycle_rules(alias, bucket)
        .await?
        .into_iter()
        .filter(|rule| rule.id.as_deref() != Some(id))
        .collect::<Vec<_>>();
    if rules.is_empty() {
        client
            .delete_bucket_lifecycle()
            .bucket(bucket)
            .send()
            .await?;
        return Ok(());
    }
    client
        .put_bucket_lifecycle_configuration()
        .bucket(bucket)
        .lifecycle_configuration(
            BucketLifecycleConfiguration::builder()
                .set_rules(Some(rules))
                .build()?,
        )
        .send()
        .await?;
    Ok(())
}

async fn get_lifecycle_rules(alias: &AliasConfig, bucket: &str) -> Result<Vec<LifecycleRule>> {
    let client = build_client(alias).await?;
    match client
        .get_bucket_lifecycle_configuration()
        .bucket(bucket)
        .send()
        .await
    {
        Ok(response) => Ok(response.rules.unwrap_or_default()),
        Err(error)
            if error
                .as_service_error()
                .and_then(ProvideErrorMetadata::code)
                .is_some_and(|code| code == "NoSuchLifecycleConfiguration") =>
        {
            Ok(Vec::new())
        }
        Err(error) => Err(error.into()),
    }
}
