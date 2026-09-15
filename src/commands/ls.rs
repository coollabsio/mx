use crate::cli::LsArgs;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::s3::S3ListItem;
use crate::target::TargetRef;
use anyhow::Result;
use serde::Serialize;
use std::cmp::Ordering;
use std::io::{self, Write};
use tabwriter::TabWriter;

pub fn run(args: LsArgs, json: bool) -> Result<()> {
    let target = TargetRef::parse(&args.target)?;
    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &target.alias)?;

    let runtime = runtime()?;
    let mut items = runtime.block_on(crate::s3::list_target(&alias, &target, args.recursive))?;
    items.sort_by(compare_items);

    if json {
        for item in &items {
            println!(
                "{}",
                serde_json::to_string_pretty(&ListMessage::from_item(&args.target, item))?
            );
        }
        return Ok(());
    }

    print_plain(&items)
}

fn print_plain(items: &[S3ListItem]) -> Result<()> {
    let mut writer = TabWriter::new(io::stdout()).padding(2);
    writeln!(writer, "Type\tModified\tSize\tName")?;

    for item in items {
        let (kind, modified, size, name) = plain_row(item);
        writeln!(writer, "{kind}\t{modified}\t{size}\t{name}")?;
    }

    writer.flush()?;
    Ok(())
}

fn plain_row(item: &S3ListItem) -> (&'static str, String, String, String) {
    match item {
        S3ListItem::Bucket {
            name,
            last_modified,
        } => (
            "BUCKET",
            last_modified.clone().unwrap_or_else(|| "-".into()),
            "-".into(),
            format!("{name}/"),
        ),
        S3ListItem::Prefix { name } => ("PREFIX", "-".into(), "-".into(), name.clone()),
        S3ListItem::Object {
            name,
            size,
            last_modified,
            ..
        } => (
            "OBJECT",
            last_modified.clone().unwrap_or_else(|| "-".into()),
            size.map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            name.clone(),
        ),
    }
}

fn compare_items(left: &S3ListItem, right: &S3ListItem) -> Ordering {
    item_rank(left)
        .cmp(&item_rank(right))
        .then_with(|| item_name(left).cmp(item_name(right)))
}

fn item_rank(item: &S3ListItem) -> u8 {
    match item {
        S3ListItem::Bucket { .. } => 0,
        S3ListItem::Prefix { .. } => 1,
        S3ListItem::Object { .. } => 2,
    }
}

fn item_name(item: &S3ListItem) -> &str {
    match item {
        S3ListItem::Bucket { name, .. } => name,
        S3ListItem::Prefix { name } => name,
        S3ListItem::Object { name, .. } => name,
    }
}

#[derive(Debug, Serialize)]
struct ListMessage<'a> {
    status: &'static str,
    target: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    name: &'a str,
    #[serde(rename = "lastModified", skip_serializing_if = "Option::is_none")]
    last_modified: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<&'a str>,
    #[serde(rename = "storageClass", skip_serializing_if = "Option::is_none")]
    storage_class: Option<&'a str>,
}

impl<'a> ListMessage<'a> {
    fn from_item(target: &'a str, item: &'a S3ListItem) -> Self {
        match item {
            S3ListItem::Bucket {
                name,
                last_modified,
            } => Self {
                status: "success",
                target,
                kind: "bucket",
                name,
                last_modified: last_modified.as_deref(),
                size: None,
                etag: None,
                storage_class: None,
            },
            S3ListItem::Prefix { name } => Self {
                status: "success",
                target,
                kind: "prefix",
                name,
                last_modified: None,
                size: None,
                etag: None,
                storage_class: None,
            },
            S3ListItem::Object {
                name,
                size,
                last_modified,
                etag,
                storage_class,
            } => Self {
                status: "success",
                target,
                kind: "object",
                name,
                last_modified: last_modified.as_deref(),
                size: *size,
                etag: etag.as_deref(),
                storage_class: storage_class.as_deref(),
            },
        }
    }
}
