use crate::cli::HealthArgs;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::output;
use anyhow::Result;

pub fn run(args: HealthArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, &args.target)?;
    runtime()?.block_on(crate::s3::ping(&alias))?;
    if json {
        println!(r#"{{"status":"success","ready":true}}"#);
    } else {
        output::print_plain(&format!("{} is ready", args.target));
    }
    Ok(())
}
