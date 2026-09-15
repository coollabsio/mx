use crate::cli::{ShareCommand, ShareUrlArgs};
use crate::commands::runtime;
use crate::commands::util::{parse_expire, require_s3};
use crate::config::ConfigStore;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub fn run(command: ShareCommand, json: bool) -> Result<()> {
    match command {
        ShareCommand::Download(args) => create(args, "GET", json),
        ShareCommand::Upload(args) => create(args, "PUT", json),
        ShareCommand::List => list(json),
    }
}

fn create(args: ShareUrlArgs, method: &str, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let key = target.require_object_key()?;
    let expire = parse_expire(&args.expire)?;
    let url = runtime()?.block_on(async {
        if method == "GET" {
            crate::s3::presign_get(&alias, &bucket, &key, expire).await
        } else {
            crate::s3::presign_put(&alias, &bucket, &key, expire).await
        }
    })?;
    append_share(ShareRecord {
        url: url.clone(),
        method: method.to_string(),
        target: args.target.clone(),
        expire: args.expire.clone(),
    })?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&ShareRecord {
                url: url.clone(),
                method: method.to_string(),
                target: args.target,
                expire: args.expire,
            })?
        );
    } else {
        println!("{url}");
    }
    Ok(())
}

fn list(json: bool) -> Result<()> {
    let records = load_shares()?;
    if json {
        for record in records {
            println!("{}", serde_json::to_string(&record)?);
        }
    } else {
        for record in records {
            println!("{}\t{}\t{}", record.method, record.expire, record.url);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShareRecord {
    url: String,
    method: String,
    target: String,
    expire: String,
}

fn share_path() -> Result<PathBuf> {
    let store = ConfigStore::load_or_create()?;
    Ok(store
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("share.json"))
}

fn load_shares() -> Result<Vec<ShareRecord>> {
    let path = share_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn append_share(record: ShareRecord) -> Result<()> {
    let mut records = load_shares()?;
    records.push(record);
    let path = share_path()?;
    fs::write(path, serde_json::to_string_pretty(&records)?)?;
    Ok(())
}
