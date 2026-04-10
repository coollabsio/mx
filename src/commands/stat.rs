use crate::cli::TargetArg;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::s3::S3Stat;
use crate::target::TargetRef;
use anyhow::Result;
use serde::Serialize;
use std::io::{self, Write};
use tabwriter::TabWriter;

pub fn run(args: TargetArg, json: bool) -> Result<()> {
    let target = TargetRef::parse(&args.target)?;
    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &target.alias)?;

    let stat = runtime()?.block_on(crate::s3::stat_target(&alias, &target))?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&StatMessage::from_stat(&args.target, &stat))?
        );
    } else {
        print_plain(&stat)?;
    }

    Ok(())
}

fn print_plain(stat: &S3Stat) -> Result<()> {
    let mut writer = TabWriter::new(io::stdout()).padding(2);
    match stat {
        S3Stat::Bucket { bucket } => {
            writeln!(writer, "Field\tValue")?;
            writeln!(writer, "Type\tbucket")?;
            writeln!(writer, "Bucket\t{bucket}")?;
        }
        S3Stat::Object {
            bucket,
            key,
            size,
            last_modified,
            etag,
            content_type,
            storage_class,
        } => {
            writeln!(writer, "Field\tValue")?;
            writeln!(writer, "Type\tobject")?;
            writeln!(writer, "Bucket\t{bucket}")?;
            writeln!(writer, "Key\t{key}")?;
            writeln!(
                writer,
                "Size\t{}",
                size.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
            )?;
            writeln!(
                writer,
                "LastModified\t{}",
                last_modified.clone().unwrap_or_else(|| "-".into())
            )?;
            writeln!(
                writer,
                "ETag\t{}",
                etag.clone().unwrap_or_else(|| "-".into())
            )?;
            writeln!(
                writer,
                "ContentType\t{}",
                content_type.clone().unwrap_or_else(|| "-".into())
            )?;
            writeln!(
                writer,
                "StorageClass\t{}",
                storage_class.clone().unwrap_or_else(|| "-".into())
            )?;
        }
    }
    writer.flush()?;
    Ok(())
}

#[derive(Debug, Serialize)]
struct StatMessage<'a> {
    status: &'static str,
    target: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    bucket: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<i64>,
    #[serde(rename = "lastModified", skip_serializing_if = "Option::is_none")]
    last_modified: Option<&'a str>,
    #[serde(rename = "eTag", skip_serializing_if = "Option::is_none")]
    etag: Option<&'a str>,
    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    content_type: Option<&'a str>,
    #[serde(rename = "storageClass", skip_serializing_if = "Option::is_none")]
    storage_class: Option<&'a str>,
}

impl<'a> StatMessage<'a> {
    fn from_stat(target: &'a str, stat: &'a S3Stat) -> Self {
        match stat {
            S3Stat::Bucket { bucket } => Self {
                status: "success",
                target,
                kind: "bucket",
                bucket: Some(bucket),
                key: None,
                size: None,
                last_modified: None,
                etag: None,
                content_type: None,
                storage_class: None,
            },
            S3Stat::Object {
                bucket,
                key,
                size,
                last_modified,
                etag,
                content_type,
                storage_class,
            } => Self {
                status: "success",
                target,
                kind: "object",
                bucket: Some(bucket),
                key: Some(key),
                size: *size,
                last_modified: last_modified.as_deref(),
                etag: etag.as_deref(),
                content_type: content_type.as_deref(),
                storage_class: storage_class.as_deref(),
            },
        }
    }
}
