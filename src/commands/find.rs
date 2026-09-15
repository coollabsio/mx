use crate::cli::FindArgs;
use crate::commands::util::{glob_match, key_depth, object_infos};
use crate::config::ConfigStore;
use anyhow::Result;
use serde::Serialize;

pub fn run(args: FindArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (_, items) = object_infos(&store, &args.target)?;
    for item in items {
        if let Some(maxdepth) = args.maxdepth
            && key_depth(&item.key) > maxdepth
        {
            continue;
        }
        let name = item.key.rsplit('/').next().unwrap_or(&item.key);
        if let Some(pattern) = &args.name
            && !glob_match(pattern, name)
        {
            continue;
        }
        if let Some(pattern) = &args.regex
            && !glob_match(pattern, &item.key)
            && !item.key.contains(pattern)
        {
            continue;
        }
        if json {
            println!(
                "{}",
                serde_json::to_string(&FindMessage {
                    status: "success",
                    key: &item.key,
                    size: item.size,
                })?
            );
        } else {
            println!("{}", item.key);
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct FindMessage<'a> {
    status: &'static str,
    key: &'a str,
    size: i64,
}
