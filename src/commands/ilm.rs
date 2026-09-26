use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::commands::{ilm_restore, ilm_tier};
use crate::config::ConfigStore;
use crate::flags::TargetArg;
use crate::output;
use anyhow::{Result, bail};
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct IlmArgs {
    #[command(subcommand)]
    pub command: IlmCommand,
}

#[derive(Debug, Subcommand)]
pub enum IlmCommand {
    #[command(about = "manage lifecycle rules")]
    Rule(IlmRuleArgs),
    /// Implemented in `ilm_tier.rs` (area H).
    #[command(about = "manage remote tier targets for ILM transition")]
    Tier(ilm_tier::IlmTierArgs),
    /// Implemented in `ilm_restore.rs` (area G).
    #[command(about = "restore archived objects")]
    Restore(ilm_restore::IlmRestoreArgs),
}

#[derive(Debug, Args)]
pub struct IlmRuleArgs {
    #[command(subcommand)]
    pub command: IlmRuleCommand,
}

#[derive(Debug, Subcommand)]
pub enum IlmRuleCommand {
    #[command(about = "add a lifecycle rule")]
    Add(IlmRuleAddArgs),
    #[command(about = "list lifecycle rules")]
    List(TargetArg),
    #[command(about = "remove a lifecycle rule")]
    Remove(IlmRuleRemoveArgs),
}

#[derive(Debug, Args)]
pub struct IlmRuleAddArgs {
    #[arg(long)]
    pub expire_days: Option<i32>,
    #[arg(long)]
    pub prefix: Option<String>,
    #[arg(long)]
    pub id: Option<String>,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct IlmRuleRemoveArgs {
    #[arg(long)]
    pub id: String,
    pub target: String,
}

pub fn run(command: IlmCommand, json: bool) -> Result<()> {
    match command {
        IlmCommand::Rule(args) => match args.command {
            IlmRuleCommand::Add(args) => add(args, json),
            IlmRuleCommand::List(args) => list(args, json),
            IlmRuleCommand::Remove(args) => remove(args, json),
        },
        IlmCommand::Tier(args) => ilm_tier::run(args, json),
        IlmCommand::Restore(args) => ilm_restore::run(args, json),
    }
}

fn add(args: IlmRuleAddArgs, json: bool) -> Result<()> {
    let days = args
        .expire_days
        .ok_or_else(|| anyhow::anyhow!("`--expire-days` is required"))?;
    if days <= 0 {
        bail!("`--expire-days` must be greater than 0");
    }
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let id = args.id.unwrap_or_else(|| format!("expire-{days}"));
    runtime()?.block_on(crate::s3::add_lifecycle_rule(
        &alias,
        &bucket,
        &id,
        args.prefix.as_deref(),
        days,
    ))?;
    if json {
        println!(r#"{{"status":"success","id":"{id}"}}"#);
    } else {
        output::print_plain(&format!("Lifecycle rule `{id}` added."));
    }
    Ok(())
}

fn list(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let rules = runtime()?.block_on(crate::s3::list_lifecycle_rules(&alias, &bucket))?;
    if json {
        println!("{}", serde_json::to_string(&rules)?);
    } else if rules.is_empty() {
        println!("No lifecycle rules");
    } else {
        for rule in rules {
            println!("{}\t{}\t{:?}", rule.id, rule.status, rule.expire_days);
        }
    }
    Ok(())
}

fn remove(args: IlmRuleRemoveArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?.block_on(crate::s3::remove_lifecycle_rule(&alias, &bucket, &args.id))?;
    if json {
        println!(r#"{{"status":"success"}}"#);
    } else {
        output::print_plain(&format!("Lifecycle rule `{}` removed.", args.id));
    }
    Ok(())
}
