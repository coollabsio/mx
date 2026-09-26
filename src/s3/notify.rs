//! Bucket notification configuration helpers (area G, `mx event`).
//!
//! The S3 configuration is flattened into [`EventConfig`] entries (topic, queue and lambda
//! configurations alike), edited with pure functions, and converted back.

use anyhow::{Result, anyhow, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::types::{
    Event, FilterRule, FilterRuleName, LambdaFunctionConfiguration, NotificationConfiguration,
    NotificationConfigurationFilter, QueueConfiguration, S3KeyFilter, TopicConfiguration,
};

/// Notification target type, from the ARN service (`sns`, `sqs`, `lambda`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArnKind {
    Topic,
    Queue,
    Lambda,
}

impl ArnKind {
    /// Validates `arn:<partition>:<service>:<region>:<account>:<resource>`.
    pub fn from_arn(arn: &str) -> Result<Self> {
        let parts: Vec<&str> = arn.split(':').collect();
        if parts.len() != 6 {
            bail!(
                "invalid ARN `{arn}`: must be 'arn:<partition>:<service>:<region>:<accountID>:<resource>'"
            );
        }
        if parts[0] != "arn" {
            bail!("invalid ARN `{arn}`: must start with 'arn:'");
        }
        match parts[2] {
            "sns" => Ok(ArnKind::Topic),
            "sqs" => Ok(ArnKind::Queue),
            "lambda" => Ok(ArnKind::Lambda),
            other => bail!("invalid ARN `{arn}`: unsupported service `{other}`"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            ArnKind::Topic => "Topic",
            ArnKind::Queue => "Queue",
            ArnKind::Lambda => "lambda",
        }
    }
}

/// One notification configuration entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventConfig {
    pub id: Option<String>,
    pub kind: ArnKind,
    pub arn: String,
    pub events: Vec<String>,
    pub prefix: String,
    pub suffix: String,
}

impl EventConfig {
    fn same_filter(&self, prefix: &str, suffix: &str) -> bool {
        self.prefix == prefix && self.suffix == suffix
    }

    fn same_events(&self, events: &[String]) -> bool {
        self.events.iter().all(|event| events.contains(event))
            && events.iter().all(|event| self.events.contains(event))
    }
}

/// Maps mc event names (`put,delete,get,replica,ilm,scanner`) to S3 event types.
pub fn parse_events(input: &str) -> Result<Vec<String>> {
    let mut events: Vec<String> = Vec::new();
    for name in input.split(',').map(str::trim) {
        let mapped: &[&str] = match name {
            "put" => &["s3:ObjectCreated:*"],
            "delete" => &["s3:ObjectRemoved:*"],
            "get" => &["s3:ObjectAccessed:*"],
            "replica" => &["s3:Replication:*"],
            "ilm" => &["s3:ObjectRestore:*", "s3:ObjectTransition:*"],
            "scanner" => &["s3:Scanner:ManyVersions", "s3:Scanner:BigPrefix"],
            _ => bail!(
                "invalid event `{name}`: use a comma-separated list of put, delete, get, replica, ilm, scanner"
            ),
        };
        for event in mapped {
            if !events.iter().any(|existing| existing == event) {
                events.push(event.to_string());
            }
        }
    }
    Ok(events)
}

/// Flattens an S3 notification configuration (event bridge settings are not represented).
pub fn to_entries(config: &NotificationConfiguration) -> Vec<EventConfig> {
    let mut entries = Vec::new();
    for topic in config.topic_configurations() {
        entries.push(entry(
            ArnKind::Topic,
            topic.id(),
            topic.topic_arn(),
            topic.events(),
            topic.filter(),
        ));
    }
    for queue in config.queue_configurations() {
        entries.push(entry(
            ArnKind::Queue,
            queue.id(),
            queue.queue_arn(),
            queue.events(),
            queue.filter(),
        ));
    }
    for lambda in config.lambda_function_configurations() {
        entries.push(entry(
            ArnKind::Lambda,
            lambda.id(),
            lambda.lambda_function_arn(),
            lambda.events(),
            lambda.filter(),
        ));
    }
    entries
}

fn entry(
    kind: ArnKind,
    id: Option<&str>,
    arn: &str,
    events: &[Event],
    filter: Option<&NotificationConfigurationFilter>,
) -> EventConfig {
    let mut prefix = String::new();
    let mut suffix = String::new();
    let rules = filter
        .and_then(|filter| filter.key())
        .map(|key| key.filter_rules())
        .unwrap_or_default();
    for rule in rules {
        let name = rule.name().map(|name| name.as_str().to_ascii_lowercase());
        let value = rule.value().unwrap_or_default().to_string();
        match name.as_deref() {
            Some("prefix") => prefix = value,
            Some("suffix") => suffix = value,
            _ => {}
        }
    }
    EventConfig {
        id: id.map(str::to_string),
        kind,
        arn: arn.to_string(),
        events: events
            .iter()
            .map(|event| event.as_str().to_string())
            .collect(),
        prefix,
        suffix,
    }
}

/// Rebuilds an S3 notification configuration from entries.
pub fn from_entries(entries: &[EventConfig]) -> Result<NotificationConfiguration> {
    let mut config = NotificationConfiguration::builder();
    for item in entries {
        let events: Vec<Event> = item
            .events
            .iter()
            .map(|e| Event::from(e.as_str()))
            .collect();
        let filter = filter_for(&item.prefix, &item.suffix);
        config = match item.kind {
            ArnKind::Topic => config.topic_configurations(
                TopicConfiguration::builder()
                    .set_id(item.id.clone())
                    .topic_arn(&item.arn)
                    .set_events(Some(events))
                    .set_filter(filter)
                    .build()?,
            ),
            ArnKind::Queue => config.queue_configurations(
                QueueConfiguration::builder()
                    .set_id(item.id.clone())
                    .queue_arn(&item.arn)
                    .set_events(Some(events))
                    .set_filter(filter)
                    .build()?,
            ),
            ArnKind::Lambda => config.lambda_function_configurations(
                LambdaFunctionConfiguration::builder()
                    .set_id(item.id.clone())
                    .lambda_function_arn(&item.arn)
                    .set_events(Some(events))
                    .set_filter(filter)
                    .build()?,
            ),
        };
    }
    Ok(config.build())
}

fn filter_for(prefix: &str, suffix: &str) -> Option<NotificationConfigurationFilter> {
    let mut rules = Vec::new();
    if !prefix.is_empty() {
        rules.push(
            FilterRule::builder()
                .name(FilterRuleName::Prefix)
                .value(prefix)
                .build(),
        );
    }
    if !suffix.is_empty() {
        rules.push(
            FilterRule::builder()
                .name(FilterRuleName::Suffix)
                .value(suffix)
                .build(),
        );
    }
    if rules.is_empty() {
        return None;
    }
    Some(
        NotificationConfigurationFilter::builder()
            .key(S3KeyFilter::builder().set_filter_rules(Some(rules)).build())
            .build(),
    )
}

/// Appends a new entry. Returns `Ok(false)` without changes when an entry for the same ARN and
/// filter already covers one of the events and `ignore_existing` is set; errors otherwise.
pub fn add_entry(
    entries: &mut Vec<EventConfig>,
    arn: &str,
    events: Vec<String>,
    prefix: &str,
    suffix: &str,
    ignore_existing: bool,
) -> Result<bool> {
    let kind = ArnKind::from_arn(arn)?;
    let overlapping = entries.iter().any(|item| {
        item.arn == arn
            && item.same_filter(prefix, suffix)
            && item.events.iter().any(|event| events.contains(event))
    });
    if overlapping {
        if ignore_existing {
            return Ok(false);
        }
        bail!("Overlapping {} configs", kind.label());
    }
    entries.push(EventConfig {
        id: None,
        kind,
        arn: arn.to_string(),
        events,
        prefix: prefix.to_string(),
        suffix: suffix.to_string(),
    });
    Ok(true)
}

/// Removes entries for `arn`. With `filter = Some((events, prefix, suffix))` only the first
/// entry with exactly those events and filters is removed (error when none matches);
/// otherwise every entry for the ARN is removed.
pub fn remove_entries(
    entries: &mut Vec<EventConfig>,
    arn: &str,
    filter: Option<(&[String], &str, &str)>,
) -> Result<()> {
    ArnKind::from_arn(arn)?;
    match filter {
        Some((events, prefix, suffix)) => {
            let index = entries
                .iter()
                .position(|item| {
                    item.arn == arn && item.same_events(events) && item.same_filter(prefix, suffix)
                })
                .ok_or_else(|| anyhow!("no notification configuration matched"))?;
            entries.remove(index);
        }
        None => entries.retain(|item| item.arn != arn),
    }
    Ok(())
}

pub async fn get_bucket_notification(client: &Client, bucket: &str) -> Result<Vec<EventConfig>> {
    let output = client
        .get_bucket_notification_configuration()
        .bucket(bucket)
        .send()
        .await?;
    let config = NotificationConfiguration::builder()
        .set_topic_configurations(Some(output.topic_configurations().to_vec()))
        .set_queue_configurations(Some(output.queue_configurations().to_vec()))
        .set_lambda_function_configurations(Some(output.lambda_function_configurations().to_vec()))
        .build();
    Ok(to_entries(&config))
}

pub async fn put_bucket_notification(
    client: &Client,
    bucket: &str,
    entries: &[EventConfig],
) -> Result<()> {
    client
        .put_bucket_notification_configuration()
        .bucket(bucket)
        .notification_configuration(from_entries(entries)?)
        .send()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUEUE: &str = "arn:minio:sqs::MXTEST:webhook";

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn maps_mc_event_names() {
        assert_eq!(
            parse_events("put,delete,get").unwrap(),
            strings(&[
                "s3:ObjectCreated:*",
                "s3:ObjectRemoved:*",
                "s3:ObjectAccessed:*"
            ])
        );
        assert_eq!(
            parse_events("ilm,scanner,replica,put,put").unwrap(),
            strings(&[
                "s3:ObjectRestore:*",
                "s3:ObjectTransition:*",
                "s3:Scanner:ManyVersions",
                "s3:Scanner:BigPrefix",
                "s3:Replication:*",
                "s3:ObjectCreated:*"
            ])
        );
        assert!(parse_events("put,copy").is_err());
        assert!(parse_events("").is_err());
    }

    #[test]
    fn validates_arns() {
        assert_eq!(ArnKind::from_arn(QUEUE).unwrap(), ArnKind::Queue);
        assert_eq!(
            ArnKind::from_arn("arn:aws:sns:us-east-1:1:topic").unwrap(),
            ArnKind::Topic
        );
        assert_eq!(
            ArnKind::from_arn("arn:aws:lambda:us-east-1:1:fn").unwrap(),
            ArnKind::Lambda
        );
        assert!(ArnKind::from_arn("arn:aws:s3:::bucket").is_err());
        assert!(ArnKind::from_arn("nope:minio:sqs::1:webhook").is_err());
        assert!(ArnKind::from_arn("arn:minio:sqs:1:webhook").is_err());
    }

    #[test]
    fn roundtrips_configuration() {
        let mut entries = Vec::new();
        add_entry(
            &mut entries,
            QUEUE,
            parse_events("put").unwrap(),
            "photos/",
            ".jpg",
            false,
        )
        .unwrap();
        add_entry(
            &mut entries,
            "arn:aws:sns:us-east-1:1:topic",
            parse_events("delete").unwrap(),
            "",
            "",
            false,
        )
        .unwrap();
        let config = from_entries(&entries).unwrap();
        assert_eq!(config.queue_configurations().len(), 1);
        assert_eq!(config.topic_configurations().len(), 1);
        let rules = config.queue_configurations()[0]
            .filter()
            .unwrap()
            .key()
            .unwrap()
            .filter_rules();
        assert_eq!(rules.len(), 2);
        assert!(config.topic_configurations()[0].filter().is_none());
        let mut back = to_entries(&config);
        back.sort_by_key(|item| item.kind == ArnKind::Topic);
        assert_eq!(back, entries);
    }

    #[test]
    fn detects_overlapping_entries() {
        let mut entries = Vec::new();
        assert!(
            add_entry(
                &mut entries,
                QUEUE,
                parse_events("put,get").unwrap(),
                "",
                "",
                false
            )
            .unwrap()
        );
        let error = add_entry(
            &mut entries,
            QUEUE,
            parse_events("put").unwrap(),
            "",
            "",
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Overlapping Queue configs"));
        assert!(
            !add_entry(
                &mut entries,
                QUEUE,
                parse_events("put").unwrap(),
                "",
                "",
                true
            )
            .unwrap()
        );
        // Different filter or events do not overlap.
        assert!(
            add_entry(
                &mut entries,
                QUEUE,
                parse_events("put").unwrap(),
                "a/",
                "",
                false
            )
            .unwrap()
        );
        assert!(
            add_entry(
                &mut entries,
                QUEUE,
                parse_events("delete").unwrap(),
                "",
                "",
                false
            )
            .unwrap()
        );
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn removes_entries() {
        let mut entries = Vec::new();
        add_entry(
            &mut entries,
            QUEUE,
            parse_events("put").unwrap(),
            "a/",
            "",
            false,
        )
        .unwrap();
        add_entry(
            &mut entries,
            QUEUE,
            parse_events("put,get").unwrap(),
            "",
            "",
            false,
        )
        .unwrap();
        add_entry(
            &mut entries,
            "arn:minio:sqs::OTHER:webhook",
            parse_events("put").unwrap(),
            "",
            "",
            false,
        )
        .unwrap();

        let events = parse_events("get,put").unwrap();
        remove_entries(&mut entries, QUEUE, Some((&events, "", ""))).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(remove_entries(&mut entries, QUEUE, Some((&events, "", ""))).is_err());

        remove_entries(&mut entries, QUEUE, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].arn, "arn:minio:sqs::OTHER:webhook");
        assert!(remove_entries(&mut entries, "bad", None).is_err());
    }
}
