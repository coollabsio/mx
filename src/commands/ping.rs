use crate::cli::PingArgs;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use anyhow::Result;

pub fn run(args: PingArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &args.target)?;
    let runtime = runtime()?;
    for attempt in 1..=args.count.max(1) {
        let elapsed = runtime.block_on(crate::s3::ping(&alias))?;
        let micros = elapsed.as_micros();
        if json {
            println!(r#"{{"status":"success","count":{attempt},"timeUs":{micros}}}"#);
        } else {
            println!("pong {} {} {}us", args.target, attempt, micros);
        }
    }
    Ok(())
}
