use crate::cli::PipeArgs;
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::location::{Location, parse_location};
use anyhow::{Result, bail};

pub fn run(args: PipeArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let target = parse_location(&args.target, store.config());
    let Location::S3(target) = target else {
        bail!("`pipe` target must be an S3 object target.");
    };
    let alias = alias_config(&store, &target.alias)?;
    let bucket = target.require_bucket()?.to_string();
    let key = target.require_object_key()?;
    let bytes = runtime()?.block_on(crate::s3::put_object_reader(
        &alias,
        &bucket,
        &key,
        std::io::stdin().lock(),
    ))?;

    if json {
        println!(
            "{{\"status\":\"success\",\"size\":{bytes},\"target\":{}}}",
            serde_json::to_string(&args.target)?
        );
    } else if !args.quiet {
        println!("{bytes} bytes -> `{}`", args.target);
    }
    Ok(())
}
