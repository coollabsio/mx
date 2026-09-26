//! `ilm rule add|edit|ls|rm|export|import` (mc ilm rule). `ilm tier` and `ilm restore` live in
//! their own modules.

use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::commands::{ilm_restore, ilm_tier};
use crate::config::ConfigStore;
use crate::error::McError;
use crate::flags::TargetArg;
use crate::output;
use crate::s3::lifecycle::{And, Filter, LifecycleConfig, LifecycleRule, Tag};
use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Args)]
pub struct IlmArgs {
    #[command(subcommand)]
    pub command: IlmCommand,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum IlmCommand {
    #[command(about = "manage bucket lifecycle rules")]
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
    #[command(about = "add a lifecycle configuration rule for a bucket")]
    Add(IlmRuleAddArgs),
    #[command(about = "modify a lifecycle configuration rule with given id")]
    Edit(IlmRuleEditArgs),
    #[command(
        visible_alias = "ls",
        about = "lists lifecycle configuration rules set on a bucket"
    )]
    List(IlmRuleListArgs),
    #[command(
        visible_alias = "rm",
        about = "remove (if any) existing lifecycle configuration rule"
    )]
    Remove(IlmRuleRemoveArgs),
    #[command(about = "export lifecycle configuration in JSON format")]
    Export(TargetArg),
    #[command(about = "import lifecycle configuration in JSON format (read from stdin)")]
    Import(TargetArg),
}

/// Rule fields shared by `add` and `edit`.
#[derive(Debug, Default, Args)]
pub struct RuleFlags {
    /// object prefix
    #[arg(long)]
    pub prefix: Option<String>,
    /// key value pairs of the form '<key1>=<value1>&<key2>=<value2>&<key3>=<value3>'
    #[arg(long)]
    pub tags: Option<String>,
    /// objects with size less than this value will be selected for the lifecycle action
    #[arg(long = "size-lt", value_name = "SIZE")]
    pub size_lt: Option<String>,
    /// objects with size greater than this value will be selected for the lifecycle action
    #[arg(long = "size-gt", value_name = "SIZE")]
    pub size_gt: Option<String>,
    /// number of days to expire
    #[arg(long = "expire-days", value_name = "DAYS")]
    pub expire_days: Option<String>,
    /// expire zombie delete markers
    #[arg(long = "expire-delete-marker")]
    pub expire_delete_marker: bool,
    /// number of days to transition
    #[arg(long = "transition-days", value_name = "DAYS")]
    pub transition_days: Option<String>,
    /// remote tier name to transition
    #[arg(long = "transition-tier", value_name = "TIER")]
    pub transition_tier: Option<String>,
    /// number of days to expire noncurrent versions
    #[arg(long = "noncurrent-expire-days", value_name = "DAYS")]
    pub noncurrent_expire_days: Option<String>,
    /// number of newer noncurrent versions to retain (default: 0)
    #[arg(long = "noncurrent-expire-newer", value_name = "COUNT")]
    pub noncurrent_expire_newer: Option<i64>,
    /// number of days to transition noncurrent versions (default: 0)
    #[arg(long = "noncurrent-transition-days", value_name = "DAYS")]
    pub noncurrent_transition_days: Option<i64>,
    /// remote tier name to transition
    #[arg(long = "noncurrent-transition-tier", value_name = "TIER")]
    pub noncurrent_transition_tier: Option<String>,
    /// expire all object versions
    #[arg(long = "expire-all-object-versions")]
    pub expire_all_object_versions: bool,
}

#[derive(Debug, Args)]
pub struct IlmRuleAddArgs {
    #[command(flatten)]
    pub rule: RuleFlags,
    /// rule id (generated when omitted)
    #[arg(long)]
    pub id: Option<String>,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct IlmRuleEditArgs {
    /// id of the rule to be modified
    #[arg(long, required = true)]
    pub id: String,
    /// disable the rule
    #[arg(long, conflicts_with = "enable")]
    pub disable: bool,
    /// enable the rule
    #[arg(long)]
    pub enable: bool,
    #[command(flatten)]
    pub rule: RuleFlags,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct IlmRuleListArgs {
    /// display only expiration fields
    #[arg(long, conflicts_with = "transition")]
    pub expiry: bool,
    /// display only transition fields
    #[arg(long)]
    pub transition: bool,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct IlmRuleRemoveArgs {
    /// id of the lifecycle rule
    #[arg(long)]
    pub id: Option<String>,
    /// force flag is to be used when deleting all lifecycle configuration rules for the bucket
    #[arg(long)]
    pub force: bool,
    /// delete all lifecycle configuration rules of the bucket, force flag enforced
    #[arg(long)]
    pub all: bool,
    pub target: String,
}

pub fn run(command: IlmCommand, json: bool) -> Result<()> {
    match command {
        IlmCommand::Rule(args) => match args.command {
            IlmRuleCommand::Add(args) => add(args, json),
            IlmRuleCommand::Edit(args) => edit(args, json),
            IlmRuleCommand::List(args) => list(args, json),
            IlmRuleCommand::Remove(args) => remove(args, json),
            IlmRuleCommand::Export(args) => export(&args.target, json),
            IlmRuleCommand::Import(args) => import(&args.target, json),
        },
        IlmCommand::Tier(args) => ilm_tier::run(args, json),
        IlmCommand::Restore(args) => ilm_restore::run(args, json),
    }
}

// ---------------------------------------------------------------------------
// rule options (port of mc cmd/ilm/options.go)
// ---------------------------------------------------------------------------

/// Parsed rule flags; `None` means "not set".
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuleOptions {
    pub id: String,
    pub status: Option<bool>,
    pub prefix: Option<String>,
    pub tags: Option<Vec<Tag>>,
    pub size_lt: Option<i64>,
    pub size_gt: Option<i64>,
    pub expiry_days: Option<i64>,
    pub expired_object_delete_marker: Option<bool>,
    pub expired_object_all_versions: Option<bool>,
    pub transition_days: Option<i64>,
    pub tier: Option<String>,
    pub noncurrent_expiration_days: Option<i64>,
    pub newer_noncurrent_expiration_versions: Option<i64>,
    pub noncurrent_transition_days: Option<i64>,
    pub noncurrent_tier: Option<String>,
}

/// [`crate::flags::parse_size`] as the `i64` lifecycle filters use.
fn parse_size(input: &str) -> Result<i64> {
    crate::flags::parse_size(input)
        .ok()
        .and_then(|bytes| i64::try_from(bytes).ok())
        .ok_or_else(|| anyhow!("size value {input} is invalid"))
}

fn parse_days(flag: &str, value: &str) -> Result<i64> {
    value
        .trim()
        .parse()
        .map_err(|_| anyhow!("failed to parse {flag}: invalid number `{value}`"))
}

/// mc `extractILMTags`.
fn parse_rule_tags(input: &str) -> Vec<Tag> {
    input
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Tag {
                key: key.to_string(),
                value: value.to_string(),
            }
        })
        .collect()
}

/// Random-ish 20 character id in the xid alphabet (mc uses `xid.New()`).
fn generate_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuv";
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(now.as_nanos());
    hasher.write_u32(std::process::id());
    let mut bits = (u128::from(now.as_secs() as u32) << 96)
        | (u128::from(hasher.finish()) << 32)
        | u128::from(now.subsec_nanos());
    let mut id = String::with_capacity(20);
    for _ in 0..20 {
        id.push(ALPHABET[(bits >> 95) as usize & 31] as char);
        bits <<= 5;
    }
    id
}

impl RuleOptions {
    /// mc `GetLifecycleOptions`. The prefix defaults to the object part of the target.
    pub fn parse(
        flags: &RuleFlags,
        target: &str,
        id: Option<&str>,
        status: Option<bool>,
    ) -> Result<Self> {
        let tier = flags
            .transition_tier
            .as_deref()
            .map(str::to_ascii_uppercase);
        let noncurrent_tier = flags
            .noncurrent_transition_tier
            .as_deref()
            .map(str::to_ascii_uppercase);
        if tier.is_some() && flags.transition_days.is_none() {
            bail!("--transition-days must be set together with --transition-tier");
        }
        if noncurrent_tier.is_some() && flags.noncurrent_transition_days.is_none() {
            bail!(
                "--noncurrent-transition-days must be set together with --noncurrent-transition-tier"
            );
        }
        let prefix = flags.prefix.clone().or_else(|| {
            let parts = target.splitn(3, '/').collect::<Vec<_>>();
            parts
                .get(2)
                .filter(|prefix| !prefix.is_empty())
                .map(|prefix| prefix.to_string())
        });
        let size = |flag: &str, value: &Option<String>| {
            value
                .as_deref()
                .map(|value| {
                    parse_size(value).map_err(|_| anyhow!("{flag} value {value} is invalid"))
                })
                .transpose()
        };
        Ok(Self {
            id: id
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .unwrap_or_else(generate_id),
            status,
            prefix,
            tags: flags.tags.as_deref().map(parse_rule_tags),
            size_lt: size("size-lt", &flags.size_lt)?,
            size_gt: size("size-gt", &flags.size_gt)?,
            expiry_days: flags
                .expire_days
                .as_deref()
                .map(|value| parse_days("expire-days", value))
                .transpose()?,
            expired_object_delete_marker: flags.expire_delete_marker.then_some(true),
            expired_object_all_versions: flags.expire_all_object_versions.then_some(true),
            transition_days: flags
                .transition_days
                .as_deref()
                .map(|value| parse_days("transition-days", value))
                .transpose()?,
            tier,
            noncurrent_expiration_days: flags
                .noncurrent_expire_days
                .as_deref()
                .map(|value| parse_days("noncurrent-expire-days", value))
                .transpose()?,
            newer_noncurrent_expiration_versions: flags.noncurrent_expire_newer,
            noncurrent_transition_days: flags.noncurrent_transition_days,
            noncurrent_tier,
        })
    }

    /// mc `LifecycleOptions.Filter()`.
    fn filter(&self) -> Filter {
        let tags = self.tags.clone().unwrap_or_default();
        let predicates = tags.len()
            + usize::from(self.prefix.is_some())
            + usize::from(self.size_lt.is_some())
            + usize::from(self.size_gt.is_some());
        let prefix = self.prefix.clone().unwrap_or_default();
        let size_lt = self.size_lt.unwrap_or_default();
        let size_gt = self.size_gt.unwrap_or_default();
        if predicates >= 2 {
            Filter {
                and: And {
                    prefix,
                    tags,
                    object_size_less_than: size_lt,
                    object_size_greater_than: size_gt,
                },
                ..Default::default()
            }
        } else {
            Filter {
                prefix,
                tag: tags.into_iter().next().unwrap_or_default(),
                object_size_less_than: size_lt,
                object_size_greater_than: size_gt,
                ..Default::default()
            }
        }
    }

    /// mc `ToILMRule` (validated).
    pub fn to_rule(&self) -> Result<LifecycleRule> {
        if self.expiry_days == Some(0) {
            bail!("expiration days cannot be set to zero");
        }
        let mut rule = LifecycleRule {
            id: self.id.clone(),
            status: if self.status == Some(false) {
                "Disabled"
            } else {
                "Enabled"
            }
            .to_string(),
            filter: self.filter(),
            ..Default::default()
        };
        rule.expiration.days = self.expiry_days.unwrap_or_default();
        rule.expiration.expired_object_delete_marker =
            self.expired_object_delete_marker.unwrap_or_default();
        rule.expiration.expired_object_all_versions =
            self.expired_object_all_versions.unwrap_or_default();
        rule.transition.days = self.transition_days.unwrap_or_default();
        rule.transition.storage_class = self.tier.clone().unwrap_or_default();
        rule.noncurrent_version_expiration.noncurrent_days =
            self.noncurrent_expiration_days.unwrap_or_default();
        rule.noncurrent_version_expiration.newer_noncurrent_versions = self
            .newer_noncurrent_expiration_versions
            .unwrap_or_default();
        rule.noncurrent_version_transition.noncurrent_days =
            self.noncurrent_transition_days.unwrap_or_default();
        rule.noncurrent_version_transition.storage_class =
            self.noncurrent_tier.clone().unwrap_or_default();
        validate_rule(&rule)?;
        Ok(rule)
    }

    /// mc `ApplyRuleFields` (plus size filters), then validation.
    pub fn apply(&self, rule: &mut LifecycleRule) -> Result<()> {
        if let Some(tags) = &self.tags {
            rule.filter.and.tags = tags.clone();
            if rule.filter.and.prefix.is_empty() {
                rule.filter.and.prefix = std::mem::take(&mut rule.filter.prefix);
            }
            if !rule.filter.tag.is_empty() {
                rule.filter.tag = Tag::default();
            }
        }
        let use_and = !rule.filter.and.tags.is_empty();
        if let Some(prefix) = &self.prefix {
            if use_and {
                rule.filter.and.prefix = prefix.clone();
            } else {
                rule.filter.prefix = prefix.clone();
            }
        }
        if let Some(size) = self.size_lt {
            if use_and {
                rule.filter.and.object_size_less_than = size;
            } else {
                rule.filter.object_size_less_than = size;
            }
        }
        if let Some(size) = self.size_gt {
            if use_and {
                rule.filter.and.object_size_greater_than = size;
            } else {
                rule.filter.object_size_greater_than = size;
            }
        }
        if let Some(days) = self.expiry_days {
            if days == 0 {
                bail!("expiration days cannot be set to zero");
            }
            rule.expiration.days = days;
            rule.expiration.date.clear();
        } else if let Some(marker) = self.expired_object_delete_marker {
            rule.expiration.expired_object_delete_marker = marker;
            rule.expiration.days = 0;
            rule.expiration.date.clear();
        }
        if let Some(all) = self.expired_object_all_versions {
            rule.expiration.expired_object_all_versions = all;
        }
        if let Some(days) = self.transition_days {
            rule.transition.days = days;
            rule.transition.date.clear();
        }
        if let Some(days) = self.noncurrent_expiration_days {
            rule.noncurrent_version_expiration.noncurrent_days = days;
        }
        if let Some(count) = self.newer_noncurrent_expiration_versions {
            rule.noncurrent_version_expiration.newer_noncurrent_versions = count;
        }
        if let Some(days) = self.noncurrent_transition_days {
            rule.noncurrent_version_transition.noncurrent_days = days;
        }
        if let Some(tier) = &self.noncurrent_tier {
            rule.noncurrent_version_transition.storage_class = tier.clone();
        }
        if let Some(tier) = &self.tier {
            rule.transition.storage_class = tier.clone();
        }
        if let Some(enabled) = self.status {
            rule.status = if enabled { "Enabled" } else { "Disabled" }.to_string();
        }
        validate_rule(rule)
    }
}

/// mc `validateILMRule` (date-based checks omitted: mx has no date flags).
pub fn validate_rule(rule: &LifecycleRule) -> Result<()> {
    let expiration = &rule.expiration;
    let transition_days_without_tier =
        rule.transition.storage_class.is_empty() && rule.transition.days != 0;
    if transition_days_without_tier {
        bail!("--transition-tier must be set together with --transition-days");
    }
    let noncurrent = &rule.noncurrent_version_transition;
    if noncurrent.noncurrent_days < 0 {
        bail!("NoncurrentVersionTransition.NoncurrentDays is not a positive integer");
    }
    if noncurrent.noncurrent_days > 0 && noncurrent.storage_class.is_empty() {
        bail!(
            "both NoncurrentVersionTransition NoncurrentDays and StorageClass need to be specified"
        );
    }
    if expiration.is_null()
        && rule.transition.is_null()
        && rule.noncurrent_version_expiration.noncurrent_days == 0
        && rule.noncurrent_version_transition.storage_class.is_empty()
        && rule.noncurrent_version_expiration.newer_noncurrent_versions == 0
        && rule.noncurrent_version_transition.newer_noncurrent_versions == 0
        && rule.del_marker_expiration.is_null()
        && rule.all_versions_expiration.is_null()
    {
        bail!(
            "at least one of Expiry, Transition, NoncurrentExpiry, NoncurrentVersionTransition actions should be specified in a rule"
        );
    }
    let expiration_params = usize::from(expiration.days != 0)
        + usize::from(!expiration.date.is_empty())
        + usize::from(expiration.expired_object_delete_marker);
    if expiration_params > 1 {
        bail!("only one parameter under Expiration can be specified");
    }
    if rule.transition.days != 0 && !rule.transition.date.is_empty() {
        bail!("only one parameter under Transition can be specified");
    }
    if rule.transition.days < 0 {
        bail!("number of days to transition can't be negative");
    }
    let noncurrent_expiration = rule.noncurrent_version_expiration.noncurrent_days;
    if noncurrent_expiration < 0 {
        bail!("NoncurrentVersionExpiration.NoncurrentDays is not a positive integer");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// subcommands
// ---------------------------------------------------------------------------

struct BucketTarget {
    alias: crate::config::model::AliasConfig,
    bucket: String,
}

fn bucket_target(target_arg: &str) -> Result<BucketTarget> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, target_arg)?;
    Ok(BucketTarget {
        bucket: target.require_bucket()?.to_string(),
        alias,
    })
}

#[derive(Serialize)]
struct RuleMessage<'a> {
    status: &'a str,
    target: &'a str,
    id: &'a str,
}

fn print_rule_message(text: String, target: &str, id: &str, json: bool) -> Result<()> {
    if json {
        crate::output::print_json(&RuleMessage {
            status: "success",
            target,
            id,
        })?;
    } else {
        output::print_plain(&text);
    }
    Ok(())
}

fn add(args: IlmRuleAddArgs, json: bool) -> Result<()> {
    let options = RuleOptions::parse(&args.rule, &args.target, args.id.as_deref(), None)?;
    let rule = options.to_rule()?;
    let target = bucket_target(&args.target)?;
    let rt = runtime()?;
    let mut config = rt
        .block_on(crate::s3::get_lifecycle(&target.alias, &target.bucket))
        .with_context(|| format!("Unable to fetch lifecycle rules for {}", args.target))?
        .map(|info| info.config)
        .unwrap_or_default();
    if config.rules.iter().any(|existing| existing.id == rule.id) {
        bail!("lifecycle rule with ID `{}` already exists", rule.id);
    }
    config.rules.push(rule);
    rt.block_on(crate::s3::put_lifecycle(
        &target.alias,
        &target.bucket,
        &config,
    ))
    .context("Unable to add this lifecycle rule")?;
    print_rule_message(
        format!(
            "Lifecycle configuration rule added with ID `{}` to {}.",
            options.id, args.target
        ),
        &args.target,
        &options.id,
        json,
    )
}

fn edit(args: IlmRuleEditArgs, json: bool) -> Result<()> {
    let status = if args.disable {
        Some(false)
    } else if args.enable {
        Some(true)
    } else {
        None
    };
    let options = RuleOptions::parse(&args.rule, &args.target, Some(&args.id), status)?;
    let target = bucket_target(&args.target)?;
    let rt = runtime()?;
    let mut config = rt
        .block_on(crate::s3::get_lifecycle(&target.alias, &target.bucket))
        .with_context(|| format!("Unable to fetch lifecycle rules for {}", args.target))?
        .map(|info| info.config)
        .unwrap_or_default();
    let rule = config
        .rules
        .iter_mut()
        .find(|rule| rule.id == args.id)
        .ok_or_else(|| anyhow!("Unable to find rule id `{}`", args.id))?;
    options.apply(rule)?;
    rt.block_on(crate::s3::put_lifecycle(
        &target.alias,
        &target.bucket,
        &config,
    ))
    .context("Unable to set new lifecycle rules")?;
    print_rule_message(
        format!(
            "Lifecycle configuration rule with ID `{}` modified  to {}.",
            args.id, args.target
        ),
        &args.target,
        &args.id,
        json,
    )
}

/// mc: server errors (including a missing configuration) are fatal with `get_context`; an
/// empty rule list is "lifecycle configuration not set".
fn fetch_existing(
    target_arg: &str,
    action: &str,
    get_context: &'static str,
) -> Result<crate::s3::lifecycle::LifecycleInfo> {
    let target = bucket_target(target_arg)?;
    let info = runtime()?
        .block_on(crate::s3::get_lifecycle_required(
            &target.alias,
            &target.bucket,
        ))
        .context(get_context)?;
    if info.config.rules.is_empty() {
        return Err(McError::new("lifecycle configuration not set"))
            .context(format!("Unable to {action} lifecycle configuration"));
    }
    Ok(info)
}

#[derive(Serialize)]
struct ConfigMessage<'a> {
    status: &'a str,
    target: &'a str,
    config: &'a LifecycleConfig,
    #[serde(rename = "updatedAt")]
    updated_at: &'a str,
}

fn list(args: IlmRuleListArgs, json: bool) -> Result<()> {
    let mut info = fetch_existing(&args.target, "ls", "Unable to get lifecycle")?;
    info.config.rules.retain(|rule| {
        if args.expiry {
            !rule.expiration.is_null()
                || rule.noncurrent_version_expiration.noncurrent_days != 0
                || rule.noncurrent_version_expiration.newer_noncurrent_versions > 0
        } else if args.transition {
            !rule.transition.is_null() || !rule.noncurrent_version_transition.is_null()
        } else {
            true
        }
    });
    if json {
        crate::output::print_json(&ConfigMessage {
            status: "success",
            target: &args.target,
            config: &info.config,
            updated_at: &info.updated_at,
        })?;
        return Ok(());
    }
    for table in rule_tables(&info.config) {
        print!("{table}");
    }
    Ok(())
}

fn prefix_of(rule: &LifecycleRule) -> &str {
    [
        rule.prefix.as_str(),
        rule.filter.prefix.as_str(),
        rule.filter.and.prefix.as_str(),
    ]
    .into_iter()
    .find(|prefix| !prefix.is_empty())
    .unwrap_or("-")
}

fn tags_of(rule: &LifecycleRule) -> String {
    let tags = if !rule.filter.tag.is_empty() {
        vec![&rule.filter.tag]
    } else {
        rule.filter.and.tags.iter().collect()
    };
    if tags.is_empty() {
        return "-".to_string();
    }
    tags.iter()
        .map(|tag| format!("{}={}", tag.key, tag.value))
        .collect::<Vec<_>>()
        .join("&")
}

/// mc `ilm.ToTables`: up to four tables (expiration current/noncurrent, transition
/// current/noncurrent).
pub fn rule_tables(config: &LifecycleConfig) -> Vec<String> {
    let mut exp_current = Vec::new();
    let mut exp_noncurrent = Vec::new();
    let mut tier_current = Vec::new();
    let mut tier_noncurrent = Vec::new();
    for rule in &config.rules {
        let common = || {
            vec![
                rule.id.clone(),
                rule.status.clone(),
                prefix_of(rule).to_string(),
                tags_of(rule),
            ]
        };
        if !rule.expiration.is_null() {
            let mut row = common();
            row.push(rule.expiration.days.to_string());
            row.push(rule.expiration.expired_object_delete_marker.to_string());
            exp_current.push(row);
        }
        let noncurrent = &rule.noncurrent_version_expiration;
        if noncurrent.noncurrent_days != 0 || noncurrent.newer_noncurrent_versions > 0 {
            let mut row = common();
            row.push(noncurrent.noncurrent_days.to_string());
            row.push(noncurrent.newer_noncurrent_versions.to_string());
            exp_noncurrent.push(row);
        }
        if !rule.transition.is_null() {
            let mut row = common();
            row.push(rule.transition.days.to_string());
            row.push(rule.transition.storage_class.clone());
            tier_current.push(row);
        }
        if !rule.noncurrent_version_transition.is_null() {
            let mut row = common();
            row.push(
                rule.noncurrent_version_transition
                    .noncurrent_days
                    .to_string(),
            );
            row.push(rule.noncurrent_version_transition.storage_class.clone());
            tier_noncurrent.push(row);
        }
    }
    let base = ["ID", "Status", "Prefix", "Tags"];
    let with = |extra: [&'static str; 2]| {
        base.iter()
            .copied()
            .chain(extra)
            .collect::<Vec<&'static str>>()
    };
    [
        (
            "Expiration for latest version (Expiration)",
            with(["Days to Expire", "Expire DeleteMarker"]),
            exp_current,
        ),
        (
            "Expiration for older versions (NoncurrentVersionExpiration)",
            with(["Days to Expire", "Keep Versions"]),
            exp_noncurrent,
        ),
        (
            "Transition for latest version (Transition)",
            with(["Days to Tier", "Tier"]),
            tier_current,
        ),
        (
            "Transition for older versions (NoncurrentVersionTransition)",
            with(["Days to Tier", "Tier"]),
            tier_noncurrent,
        ),
    ]
    .into_iter()
    .filter(|(_, _, rows)| !rows.is_empty())
    .map(|(title, headers, rows)| {
        // Integer cells (days, keep versions) are Go ints: go-pretty right-aligns them.
        let numeric = if headers[5] == "Keep Versions" {
            &[4, 5][..]
        } else {
            &[4][..]
        };
        render_table(Some(title), &headers, &rows, numeric)
    })
    .collect()
}

/// Box table in go-pretty `StyleLight` (upper-case headers, left-aligned cells; columns in
/// `numeric` hold numbers, which go-pretty right-aligns).
pub fn render_table(
    title: Option<&str>,
    headers: &[&str],
    rows: &[Vec<String>],
    numeric: &[usize],
) -> String {
    let headers = headers
        .iter()
        .map(|header| header.to_ascii_uppercase())
        .collect::<Vec<_>>();
    let mut widths = headers
        .iter()
        .map(|header| header.chars().count())
        .collect::<Vec<_>>();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let mut inner: usize = widths.iter().map(|width| width + 2).sum::<usize>() + widths.len() - 1;
    if let Some(title) = title {
        let needed = title.chars().count() + 2;
        if needed > inner {
            if let Some(last) = widths.last_mut() {
                *last += needed - inner;
            }
            inner = needed;
        }
    }
    let line = |left: &str, mid: &str, right: &str| {
        let parts = widths
            .iter()
            .map(|width| "─".repeat(width + 2))
            .collect::<Vec<_>>();
        format!("{left}{}{right}\n", parts.join(mid))
    };
    let row_text = |cells: &[String], header: bool| {
        let parts = cells
            .iter()
            .zip(&widths)
            .enumerate()
            .map(|(index, (cell, width))| {
                if !header && numeric.contains(&index) {
                    format!(" {cell:>width$} ")
                } else {
                    format!(" {cell:<width$} ")
                }
            })
            .collect::<Vec<_>>();
        format!("│{}│\n", parts.join("│"))
    };
    let mut out = String::new();
    if let Some(title) = title {
        out.push_str(&format!("┌{}┐\n", "─".repeat(inner)));
        out.push_str(&format!("│ {title:<pad$} │\n", pad = inner - 2));
        out.push_str(&line("├", "┬", "┤"));
    } else {
        out.push_str(&line("┌", "┬", "┐"));
    }
    out.push_str(&row_text(&headers, true));
    out.push_str(&line("├", "┼", "┤"));
    for row in rows {
        out.push_str(&row_text(row, false));
    }
    out.push_str(&line("└", "┴", "┘"));
    out
}

#[derive(Serialize)]
struct RemoveMessage<'a> {
    status: &'a str,
    id: &'a str,
    target: &'a str,
    all: bool,
}

fn remove(args: IlmRuleRemoveArgs, json: bool) -> Result<()> {
    if args.all != args.force {
        bail!("It is mandatory to specify --all and --force flag together for mx ilm rule rm.");
    }
    if !args.all && args.id.as_deref().unwrap_or_default().is_empty() {
        bail!("ilm ID cannot be empty");
    }
    let id = args.id.clone().unwrap_or_default();
    let target = bucket_target(&args.target)?;
    let rt = runtime()?;
    let mut config = rt
        .block_on(crate::s3::get_lifecycle(&target.alias, &target.bucket))
        .context("Unable to fetch lifecycle rules")?
        .map(|info| info.config)
        .filter(|config| !config.rules.is_empty())
        .ok_or_else(|| anyhow!("Unable to remove rule by id: lifecycle configuration not set"))?;
    if args.all {
        config.rules.clear();
    } else {
        let before = config.rules.len();
        config.rules.retain(|rule| rule.id != id);
        if config.rules.len() == before {
            bail!("Unable to remove rule by id: lifecycle rule for id '{id}' not found");
        }
    }
    rt.block_on(crate::s3::put_lifecycle(
        &target.alias,
        &target.bucket,
        &config,
    ))
    .context("Unable to set lifecycle rules")?;
    if json {
        crate::output::print_json(&RemoveMessage {
            status: "success",
            id: &id,
            target: &args.target,
            all: args.all,
        })?;
    } else if args.all {
        output::print_plain(&format!("Rules for `{}` removed.", args.target));
    } else {
        output::print_plain(&format!(
            "Rule ID `{id}` from target {} removed.",
            args.target
        ));
    }
    Ok(())
}

fn export(target_arg: &str, json: bool) -> Result<()> {
    let info = fetch_existing(
        target_arg,
        "export",
        "Unable to get lifecycle configuration",
    )?;
    if json {
        crate::output::print_json(&ConfigMessage {
            status: "success",
            target: target_arg,
            config: &info.config,
            updated_at: &info.updated_at,
        })?;
    } else {
        println!("{}", crate::output::json_indent(&info.config)?);
    }
    Ok(())
}

/// Parses and validates an `ilm rule export` document.
pub fn parse_import(text: &str) -> Result<LifecycleConfig> {
    let config: LifecycleConfig =
        serde_json::from_str(text).context("Unable to read ILM configuration")?;
    if config.rules.is_empty() {
        bail!("The provided ILM configuration does not contain any rule, aborting.");
    }
    for rule in &config.rules {
        let has_transition = rule.transition.days != 0 || !rule.transition.date.is_empty();
        if has_transition && rule.transition.storage_class.is_empty()
            || rule.noncurrent_version_transition.noncurrent_days != 0
                && rule.noncurrent_version_transition.storage_class.is_empty()
        {
            bail!("Unable to read ILM configuration: storage-class cannot be empty");
        }
    }
    Ok(config)
}

#[derive(Serialize)]
struct ImportMessage<'a> {
    status: &'a str,
    target: &'a str,
}

fn import(target_arg: &str, json: bool) -> Result<()> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    let config = parse_import(&text)?;
    let target = bucket_target(target_arg)?;
    runtime()?
        .block_on(crate::s3::put_lifecycle(
            &target.alias,
            &target.bucket,
            &config,
        ))
        .context("Unable to set new lifecycle rules")?;
    if json {
        crate::output::print_json(&ImportMessage {
            status: "success",
            target: target_arg,
        })?;
    } else {
        output::print_plain(&format!(
            "Lifecycle configuration imported successfully to `{target_arg}`."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags() -> RuleFlags {
        RuleFlags::default()
    }

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("100").unwrap(), 100);
        assert_eq!(parse_size("10MiB").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_size("5MB").unwrap(), 5_000_000);
        assert_eq!(parse_size("1.5 KiB").unwrap(), 1536);
        assert!(parse_size("ten").is_err());
        assert!(parse_size("5XB").is_err());
    }

    #[test]
    fn builds_expiry_rule_with_prefix_from_target() {
        let flags = RuleFlags {
            expire_days: Some("30".into()),
            ..Default::default()
        };
        let options = RuleOptions::parse(&flags, "alias/bucket/logs/", Some("r1"), None).unwrap();
        let rule = options.to_rule().unwrap();
        assert_eq!(rule.id, "r1");
        assert_eq!(rule.status, "Enabled");
        assert_eq!(rule.filter.prefix, "logs/");
        assert_eq!(rule.expiration.days, 30);
    }

    #[test]
    fn multiple_predicates_use_and() {
        let flags = RuleFlags {
            expire_days: Some("1".into()),
            prefix: Some("p/".into()),
            tags: Some("a=1&b=2".into()),
            size_gt: Some("1KiB".into()),
            ..Default::default()
        };
        let rule = RuleOptions::parse(&flags, "a/b", None, None)
            .unwrap()
            .to_rule()
            .unwrap();
        assert_eq!(rule.filter.and.prefix, "p/");
        assert_eq!(rule.filter.and.tags.len(), 2);
        assert_eq!(rule.filter.and.object_size_greater_than, 1024);
        assert!(rule.filter.prefix.is_empty());
        assert_eq!(rule.id.len(), 20);
    }

    #[test]
    fn transition_flags_build_rule_and_require_pairs() {
        let flags = RuleFlags {
            transition_days: Some("10".into()),
            transition_tier: Some("warm".into()),
            noncurrent_transition_days: Some(5),
            noncurrent_transition_tier: Some("cold".into()),
            ..Default::default()
        };
        let rule = RuleOptions::parse(&flags, "a/b", None, None)
            .unwrap()
            .to_rule()
            .unwrap();
        assert_eq!(rule.transition.storage_class, "WARM");
        assert_eq!(rule.transition.days, 10);
        assert_eq!(rule.noncurrent_version_transition.storage_class, "COLD");
        assert!(rule.to_xml_contains(
            "<Transition><StorageClass>WARM</StorageClass><Days>10</Days></Transition>"
        ));

        let flags = RuleFlags {
            transition_tier: Some("warm".into()),
            ..Default::default()
        };
        assert!(RuleOptions::parse(&flags, "a/b", None, None).is_err());

        let flags = RuleFlags {
            transition_days: Some("3".into()),
            ..Default::default()
        };
        let error = RuleOptions::parse(&flags, "a/b", None, None)
            .unwrap()
            .to_rule()
            .unwrap_err();
        assert!(error.to_string().contains("--transition-tier"));

        let flags = RuleFlags {
            noncurrent_transition_days: Some(3),
            ..Default::default()
        };
        let error = RuleOptions::parse(&flags, "a/b", None, None)
            .unwrap()
            .to_rule()
            .unwrap_err();
        assert!(error.to_string().contains("StorageClass"));
    }

    #[test]
    fn rejects_empty_and_conflicting_rules() {
        let options = RuleOptions::parse(&flags(), "a/b", None, None).unwrap();
        assert!(
            options
                .to_rule()
                .unwrap_err()
                .to_string()
                .contains("at least one")
        );
        let flags = RuleFlags {
            expire_days: Some("3".into()),
            expire_delete_marker: true,
            ..Default::default()
        };
        let error = RuleOptions::parse(&flags, "a/b", None, None)
            .unwrap()
            .to_rule()
            .unwrap_err();
        assert!(error.to_string().contains("only one parameter"));
        let flags = RuleFlags {
            expire_days: Some("0".into()),
            ..Default::default()
        };
        assert!(
            RuleOptions::parse(&flags, "a/b", None, None)
                .unwrap()
                .to_rule()
                .is_err()
        );
    }

    #[test]
    fn edit_applies_fields() {
        let flags = RuleFlags {
            expire_days: Some("5".into()),
            prefix: Some("old/".into()),
            ..Default::default()
        };
        let mut rule = RuleOptions::parse(&flags, "a/b", Some("x"), None)
            .unwrap()
            .to_rule()
            .unwrap();
        let edit = RuleFlags {
            expire_days: Some("9".into()),
            tags: Some("k=v".into()),
            noncurrent_expire_days: Some("4".into()),
            noncurrent_expire_newer: Some(2),
            expire_all_object_versions: true,
            ..Default::default()
        };
        RuleOptions::parse(&edit, "a/b", Some("x"), Some(false))
            .unwrap()
            .apply(&mut rule)
            .unwrap();
        assert_eq!(rule.expiration.days, 9);
        assert!(rule.expiration.expired_object_all_versions);
        assert_eq!(rule.status, "Disabled");
        assert_eq!(rule.filter.and.prefix, "old/");
        assert_eq!(rule.filter.and.tags[0].key, "k");
        assert!(rule.filter.prefix.is_empty());
        assert_eq!(rule.noncurrent_version_expiration.noncurrent_days, 4);
        assert_eq!(
            rule.noncurrent_version_expiration.newer_noncurrent_versions,
            2
        );
    }

    #[test]
    fn tables_render_like_mc() {
        let flags = RuleFlags {
            expire_days: Some("30".into()),
            ..Default::default()
        };
        let rule = RuleOptions::parse(&flags, "a/b", Some("rule1"), None)
            .unwrap()
            .to_rule()
            .unwrap();
        let tables = rule_tables(&LifecycleConfig { rules: vec![rule] });
        assert_eq!(tables.len(), 1);
        let table = &tables[0];
        assert!(table.contains("│ Expiration for latest version (Expiration)"));
        assert!(table.contains("DAYS TO EXPIRE"));
        assert!(table.contains("│ rule1 │"));
        let widths = table
            .lines()
            .map(|line| line.chars().count())
            .collect::<Vec<_>>();
        assert!(widths.iter().all(|width| *width == widths[0]), "{table}");
    }

    #[test]
    fn import_requires_rules_and_storage_class() {
        assert!(parse_import(r#"{"Rules":[]}"#).is_err());
        assert!(
            parse_import(r#"{"Rules":[{"ID":"a","Status":"Enabled","Transition":{"Days":3}}]}"#)
                .is_err()
        );
        let config =
            parse_import(r#"{"Rules":[{"ID":"a","Status":"Enabled","Expiration":{"Days":3}}]}"#)
                .unwrap();
        assert_eq!(config.rules[0].expiration.days, 3);
    }

    trait XmlContains {
        fn to_xml_contains(&self, needle: &str) -> bool;
    }

    impl XmlContains for LifecycleRule {
        fn to_xml_contains(&self, needle: &str) -> bool {
            LifecycleConfig {
                rules: vec![self.clone()],
            }
            .to_xml()
            .contains(needle)
        }
    }
}
