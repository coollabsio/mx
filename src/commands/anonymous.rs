//! `anonymous set|set-json|get|get-json|list|links` (mc anonymous). Canned permissions are
//! merged into the bucket policy per bucket/prefix like minio-go `pkg/policy`.

use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::flags::TargetArg;
use crate::output;
use crate::s3::ListOptions;
use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

#[derive(Debug, Args)]
pub struct AnonymousArgs {
    #[command(subcommand)]
    pub command: AnonymousCommand,
}

#[derive(Debug, Subcommand)]
pub enum AnonymousCommand {
    #[command(about = "set anonymous access (private|none, public, download, upload)")]
    Set(AnonymousSetArgs),
    #[command(
        name = "set-json",
        about = "set anonymous access policy from a JSON file"
    )]
    SetJson(AnonymousSetJsonArgs),
    #[command(about = "show anonymous access permission")]
    Get(TargetArg),
    #[command(name = "get-json", about = "show anonymous access policy as JSON")]
    GetJson(TargetArg),
    #[command(about = "list anonymous access rules of a bucket or prefix")]
    List(TargetArg),
    #[command(about = "list publicly downloadable object URLs")]
    Links(AnonymousLinksArgs),
}

#[derive(Debug, Args)]
pub struct AnonymousSetArgs {
    #[arg(help = "private (or none), public, download, or upload")]
    pub permission: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct AnonymousSetJsonArgs {
    pub file: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct AnonymousLinksArgs {
    /// list recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
    pub target: String,
}

pub fn run(command: AnonymousCommand, json: bool) -> Result<()> {
    match command {
        AnonymousCommand::Set(args) => set(args, json),
        AnonymousCommand::SetJson(args) => set_json(args, json),
        AnonymousCommand::Get(args) => get(&args.target, "get", json),
        AnonymousCommand::GetJson(args) => get(&args.target, "get-json", json),
        AnonymousCommand::List(args) => list(&args.target, json),
        AnonymousCommand::Links(args) => links(args, json),
    }
}

/// Maps an mc access permission to a canned bucket policy.
fn canned_policy(permission: &str) -> Result<BucketPolicy> {
    Ok(match permission {
        "none" | "private" => BucketPolicy::None,
        "download" => BucketPolicy::ReadOnly,
        "upload" => BucketPolicy::WriteOnly,
        "public" => BucketPolicy::ReadWrite,
        _ => bail!(
            "Unrecognized permission `{permission}`. Allowed values are [private, public, download, upload]."
        ),
    })
}

/// mc `stringToAccessPerm`.
fn access_perm(policy: &str) -> &'static str {
    match policy {
        "readonly" => "download",
        "writeonly" => "upload",
        "readwrite" => "public",
        "custom" => "custom",
        _ => "private",
    }
}

#[derive(Serialize)]
struct AnonymousMessage<'a> {
    operation: &'a str,
    status: &'a str,
    bucket: &'a str,
    permission: &'a str,
    #[serde(skip_serializing_if = "Map::is_empty")]
    anonymous: Map<String, Value>,
}

fn print_message(
    operation: &str,
    target: &str,
    permission: &str,
    anonymous: Map<String, Value>,
    json: bool,
) -> Result<()> {
    if json {
        crate::output::print_json(&AnonymousMessage {
            operation,
            status: "success",
            bucket: target,
            permission,
            anonymous,
        })?;
        return Ok(());
    }
    match operation {
        "set" => output::print_plain(&format!(
            "Access permission for `{target}` is set to `{permission}`"
        )),
        "set-json" => output::print_plain(&format!(
            "Access permission for `{target}` is set from `{permission}`"
        )),
        "get" => println!("Access permission for `{target}` is `{permission}`"),
        _ => println!("{}", crate::output::json_indent(&anonymous)?),
    }
    Ok(())
}

/// Bucket, object prefix and current policy document (if any) of a target.
struct PolicyTarget {
    alias: crate::config::model::AliasConfig,
    alias_name: String,
    bucket: String,
    prefix: String,
    policy: Option<String>,
}

fn load_target(target_arg: &str) -> Result<PolicyTarget> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, target_arg)?;
    let bucket = target.require_bucket()?.to_string();
    let policy = runtime()?.block_on(crate::s3::get_bucket_policy(&alias, &bucket))?;
    Ok(PolicyTarget {
        alias,
        alias_name: target.alias.clone(),
        prefix: target.key_with_trailing_slash().unwrap_or_default(),
        bucket,
        policy,
    })
}

/// mc `GetAccess`: canned permission of bucket/prefix (`custom` for other policies).
fn current_permission(target: &PolicyTarget) -> Result<&'static str> {
    let Some(policy) = &target.policy else {
        return Ok("private");
    };
    let statements = parse_policy(policy)?.statements;
    let canned = get_policy(&statements, &target.bucket, &target.prefix);
    Ok(if canned == BucketPolicy::None {
        "custom"
    } else {
        access_perm(canned.as_str())
    })
}

fn set(args: AnonymousSetArgs, json: bool) -> Result<()> {
    let permission = args.permission.to_ascii_lowercase();
    let canned = canned_policy(&permission)?;
    let target = load_target(&args.target)?;
    let mut document = match &target.policy {
        Some(policy) => parse_policy(policy)?,
        None => PolicyDocument::default(),
    };
    document.statements = set_policy(document.statements, canned, &target.bucket, &target.prefix);
    let rt = runtime()?;
    if document.statements.is_empty() {
        if target.policy.is_some() {
            rt.block_on(crate::s3::delete_bucket_policy(
                &target.alias,
                &target.bucket,
            ))?;
        }
    } else {
        rt.block_on(crate::s3::put_bucket_policy(
            &target.alias,
            &target.bucket,
            &document.to_json().to_string(),
        ))?;
    }
    let target = load_target(&args.target)?;
    print_message(
        "set",
        &args.target,
        current_permission(&target)?,
        Map::new(),
        json,
    )
}

fn set_json(args: AnonymousSetJsonArgs, json: bool) -> Result<()> {
    const MAX_JSON_SIZE: u64 = 120 * 1024;
    let mut policy = String::new();
    std::fs::File::open(&args.file)
        .with_context(|| format!("Unable to set anonymous for `{}`.", args.target))?
        .take(MAX_JSON_SIZE + 1)
        .read_to_string(&mut policy)?;
    if policy.len() as u64 > MAX_JSON_SIZE {
        bail!("policy file `{}` is larger than 120KiB", args.file);
    }
    serde_json::from_str::<Value>(&policy)
        .with_context(|| format!("`{}` is not valid JSON", args.file))?;
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    runtime()?.block_on(crate::s3::put_bucket_policy(&alias, &bucket, &policy))?;
    print_message("set-json", &args.target, &args.file, Map::new(), json)
}

fn get(target_arg: &str, operation: &str, json: bool) -> Result<()> {
    let target = load_target(target_arg)?;
    let permission = current_permission(&target)?;
    let anonymous = match &target.policy {
        Some(policy) => match serde_json::from_str::<Value>(policy)? {
            Value::Object(map) => map,
            _ => bail!("Unable to unmarshal custom anonymous file."),
        },
        None => Map::new(),
    };
    print_message(operation, target_arg, permission, anonymous, json)
}

/// mc `GetAccessRules`: `bucket/prefix*` resources of the policy with their canned policy.
fn access_rules(target: &PolicyTarget) -> Result<BTreeMap<String, String>> {
    let Some(policy) = &target.policy else {
        return Ok(BTreeMap::new());
    };
    let statements = parse_policy(policy)?.statements;
    Ok(get_policies(&statements, &target.bucket, &target.prefix)
        .into_iter()
        .map(|(resource, policy)| (resource, policy.as_str().to_string()))
        .collect())
}

#[derive(Serialize)]
struct RuleMessage<'a> {
    resource: &'a str,
    allow: &'a str,
}

fn list(target_arg: &str, json: bool) -> Result<()> {
    let target = load_target(target_arg)?;
    for (resource, allow) in access_rules(&target)? {
        if json {
            crate::output::print_json(&RuleMessage {
                resource: &resource,
                allow: &allow,
            })?;
        } else {
            println!("{resource} => {allow}");
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct LinkMessage<'a> {
    status: &'a str,
    url: &'a str,
}

fn links(args: AnonymousLinksArgs, json: bool) -> Result<()> {
    let target = load_target(&args.target)?;
    let path = format!("{}/{}", target.bucket, target.prefix);
    let rt = runtime()?;
    let client = rt.block_on(crate::s3::build_client(&target.alias))?;
    let base_url = crate::s3::bucket_url(&target.alias, &target.bucket)?;
    for (resource, allow) in access_rules(&target)? {
        let anonymous_path = resource.strip_suffix('*').unwrap_or(&resource);
        if !anonymous_path.starts_with(&path) {
            continue;
        }
        if !matches!(allow.as_str(), "readonly" | "readwrite") {
            continue;
        }
        let prefix = anonymous_path
            .strip_prefix(&format!("{}/", target.bucket))
            .unwrap_or_default();
        let items = rt
            .block_on(crate::s3::list_objects_with(
                &client,
                &target.bucket,
                Some(prefix).filter(|prefix| !prefix.is_empty()),
                &ListOptions {
                    recursive: args.recursive,
                    ..Default::default()
                },
            ))
            .with_context(|| format!("Unable to list `{}/{anonymous_path}`.", target.alias_name))?;
        for item in items {
            if item.is_prefix && args.recursive {
                continue;
            }
            let url = format!("{base_url}{}", crate::s3::full_key(prefix, &item.key));
            let url = if item.is_prefix && !url.ends_with('/') {
                format!("{url}/")
            } else {
                url
            };
            if json {
                crate::output::print_json(&LinkMessage {
                    status: "success",
                    url: &url,
                })?;
            } else {
                println!("{url}");
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// canned bucket policies (port of minio-go pkg/policy)
// ---------------------------------------------------------------------------

const RESOURCE_PREFIX: &str = "arn:aws:s3:::";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketPolicy {
    None,
    ReadOnly,
    ReadWrite,
    WriteOnly,
}

impl BucketPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            BucketPolicy::None => "none",
            BucketPolicy::ReadOnly => "readonly",
            BucketPolicy::ReadWrite => "readwrite",
            BucketPolicy::WriteOnly => "writeonly",
        }
    }
}

type StringSet = BTreeSet<String>;
/// condition operator -> condition key -> values
type ConditionMap = BTreeMap<String, BTreeMap<String, StringSet>>;

fn set_of(items: &[&str]) -> StringSet {
    items.iter().map(|item| item.to_string()).collect()
}

fn common_bucket_actions() -> StringSet {
    set_of(&["s3:GetBucketLocation"])
}
fn read_only_bucket_actions() -> StringSet {
    set_of(&["s3:ListBucket"])
}
fn write_only_bucket_actions() -> StringSet {
    set_of(&["s3:ListBucketMultipartUploads"])
}
fn read_only_object_actions() -> StringSet {
    set_of(&["s3:GetObject"])
}
fn write_only_object_actions() -> StringSet {
    set_of(&[
        "s3:AbortMultipartUpload",
        "s3:DeleteObject",
        "s3:ListMultipartUploadParts",
        "s3:PutObject",
    ])
}

fn valid_actions() -> StringSet {
    let mut all = common_bucket_actions();
    all.extend(read_only_bucket_actions());
    all.extend(write_only_bucket_actions());
    all.extend(read_only_object_actions());
    all.extend(write_only_object_actions());
    all
}

fn contains_all(set: &StringSet, required: &StringSet) -> bool {
    required.is_subset(set)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Statement {
    pub actions: StringSet,
    pub conditions: ConditionMap,
    pub effect: String,
    pub principal_aws: StringSet,
    pub principal_canonical: StringSet,
    pub resources: StringSet,
    pub sid: String,
}

impl Statement {
    fn allows_anyone(&self) -> bool {
        self.effect == "Allow" && self.principal_aws.contains("*")
    }

    fn has_resource_prefix(&self, prefix: &str) -> bool {
        self.resources
            .iter()
            .any(|resource| resource.starts_with(prefix))
    }

    fn anonymous(actions: StringSet, resource: String) -> Self {
        Statement {
            actions,
            effect: "Allow".to_string(),
            principal_aws: set_of(&["*"]),
            resources: BTreeSet::from([resource]),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDocument {
    pub version: String,
    pub statements: Vec<Statement>,
}

impl Default for PolicyDocument {
    fn default() -> Self {
        Self {
            version: "2012-10-17".to_string(),
            statements: Vec::new(),
        }
    }
}

fn string_set(value: Option<&Value>) -> Result<StringSet> {
    Ok(match value {
        None | Some(Value::Null) => StringSet::new(),
        Some(Value::String(item)) => BTreeSet::from([item.clone()]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| anyhow!("invalid policy value {item}"))
            })
            .collect::<Result<_>>()?,
        Some(other) => bail!("invalid policy value {other}"),
    })
}

pub fn parse_policy(text: &str) -> Result<PolicyDocument> {
    let value: Value = serde_json::from_str(text).context("invalid bucket policy JSON")?;
    let mut document = PolicyDocument {
        version: value
            .get("Version")
            .and_then(Value::as_str)
            .unwrap_or("2012-10-17")
            .to_string(),
        statements: Vec::new(),
    };
    let statements = match value.get("Statement") {
        Some(Value::Array(items)) => items.clone(),
        Some(item @ Value::Object(_)) => vec![item.clone()],
        _ => Vec::new(),
    };
    for item in statements {
        let mut statement = Statement {
            actions: string_set(item.get("Action"))?,
            effect: item
                .get("Effect")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            resources: string_set(item.get("Resource"))?,
            sid: item
                .get("Sid")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            ..Default::default()
        };
        match item.get("Principal") {
            Some(Value::String(principal)) if principal == "*" => {
                statement.principal_aws = set_of(&["*"]);
            }
            Some(Value::Object(principal)) => {
                statement.principal_aws = string_set(principal.get("AWS"))?;
                statement.principal_canonical = string_set(principal.get("CanonicalUser"))?;
            }
            None | Some(Value::Null) => {}
            Some(_) => bail!("unrecognized Principal field"),
        }
        if let Some(Value::Object(conditions)) = item.get("Condition") {
            for (operator, keys) in conditions {
                let Value::Object(keys) = keys else {
                    bail!("invalid policy condition `{operator}`");
                };
                let entry = statement.conditions.entry(operator.clone()).or_default();
                for (key, values) in keys {
                    entry.insert(key.clone(), string_set(Some(values))?);
                }
            }
        }
        document.statements.push(statement);
    }
    Ok(document)
}

impl PolicyDocument {
    pub fn to_json(&self) -> Value {
        let statements = self
            .statements
            .iter()
            .map(|statement| {
                let mut object = Map::new();
                object.insert("Action".into(), serde_json::json!(statement.actions));
                if !statement.conditions.is_empty() {
                    object.insert("Condition".into(), serde_json::json!(statement.conditions));
                }
                object.insert("Effect".into(), Value::from(statement.effect.clone()));
                let mut principal = Map::new();
                if !statement.principal_aws.is_empty() {
                    principal.insert("AWS".into(), serde_json::json!(statement.principal_aws));
                }
                if !statement.principal_canonical.is_empty() {
                    principal.insert(
                        "CanonicalUser".into(),
                        serde_json::json!(statement.principal_canonical),
                    );
                }
                object.insert("Principal".into(), Value::Object(principal));
                object.insert("Resource".into(), serde_json::json!(statement.resources));
                object.insert("Sid".into(), Value::from(statement.sid.clone()));
                Value::Object(object)
            })
            .collect::<Vec<_>>();
        serde_json::json!({"Version": self.version, "Statement": statements})
    }
}

fn is_valid_statement(statement: &Statement, bucket: &str) -> bool {
    if statement.actions.is_disjoint(&valid_actions()) || !statement.allows_anyone() {
        return false;
    }
    let bucket_resource = format!("{RESOURCE_PREFIX}{bucket}");
    statement.resources.contains(&bucket_resource)
        || statement.has_resource_prefix(&format!("{bucket_resource}/"))
}

fn new_statements(policy: BucketPolicy, bucket: &str, prefix: &str) -> Vec<Statement> {
    if policy == BucketPolicy::None || bucket.is_empty() {
        return Vec::new();
    }
    let bucket_resource = format!("{RESOURCE_PREFIX}{bucket}");
    let mut statements = vec![Statement::anonymous(
        common_bucket_actions(),
        bucket_resource.clone(),
    )];
    if matches!(policy, BucketPolicy::ReadOnly | BucketPolicy::ReadWrite) {
        let mut statement =
            Statement::anonymous(read_only_bucket_actions(), bucket_resource.clone());
        if !prefix.is_empty() {
            statement.conditions.insert(
                "StringLike".into(),
                BTreeMap::from([("s3:prefix".to_string(), set_of(&[&format!("{prefix}*")]))]),
            );
        }
        statements.push(statement);
    }
    if matches!(policy, BucketPolicy::WriteOnly | BucketPolicy::ReadWrite) {
        statements.push(Statement::anonymous(
            write_only_bucket_actions(),
            bucket_resource,
        ));
    }
    let object_actions = match policy {
        BucketPolicy::ReadOnly => read_only_object_actions(),
        BucketPolicy::WriteOnly => write_only_object_actions(),
        _ => {
            let mut actions = read_only_object_actions();
            actions.extend(write_only_object_actions());
            actions
        }
    };
    statements.push(Statement::anonymous(
        object_actions,
        format!("{RESOURCE_PREFIX}{bucket}/{prefix}*"),
    ));
    statements
}

fn in_use_policy(statements: &[Statement], bucket: &str, prefix: &str) -> (bool, bool) {
    let resource_prefix = format!("{RESOURCE_PREFIX}{bucket}/");
    let object_resource = format!("{RESOURCE_PREFIX}{bucket}/{prefix}*");
    let (mut read_only, mut write_only) = (false, false);
    for statement in statements {
        if !statement.resources.contains(&object_resource)
            && statement.has_resource_prefix(&resource_prefix)
        {
            read_only |= contains_all(&statement.actions, &read_only_object_actions());
            write_only |= contains_all(&statement.actions, &write_only_object_actions());
        }
    }
    (read_only, write_only)
}

fn remove_object_actions(statement: &mut Statement, object_resource: &str) {
    if statement.conditions.is_empty() {
        if statement.resources.len() > 1 {
            statement.resources.remove(object_resource);
        } else {
            statement.actions = &statement.actions - &read_only_object_actions();
            statement.actions = &statement.actions - &write_only_object_actions();
        }
    }
}

fn remove_bucket_actions(
    statement: &mut Statement,
    prefix: &str,
    bucket_resource: &str,
    read_only_in_use: bool,
    write_only_in_use: bool,
) {
    if statement.resources.len() > 1 {
        statement.resources.remove(bucket_resource);
        return;
    }
    if !read_only_in_use && contains_all(&statement.actions, &read_only_bucket_actions()) {
        if statement.conditions.is_empty() {
            statement.actions = &statement.actions - &read_only_bucket_actions();
        } else if !prefix.is_empty() {
            // minio-go only strips `StringEquals`, but canned prefix policies are written with
            // `StringLike` (`prefix*`); strip both so `none` removes what `download` added.
            let like = format!("{prefix}*");
            for (operator, value) in [("StringEquals", prefix), ("StringLike", like.as_str())] {
                if let Some(keys) = statement.conditions.get_mut(operator) {
                    if let Some(values) = keys.get_mut("s3:prefix") {
                        values.remove(value);
                        if values.is_empty() {
                            keys.remove("s3:prefix");
                        }
                    }
                    if keys.is_empty() {
                        statement.conditions.remove(operator);
                    }
                }
            }
            if statement.conditions.is_empty() {
                statement.actions = &statement.actions - &read_only_bucket_actions();
            }
        }
    }
    if !write_only_in_use && statement.conditions.is_empty() {
        statement.actions = &statement.actions - &write_only_bucket_actions();
    }
}

fn remove_statements(statements: Vec<Statement>, bucket: &str, prefix: &str) -> Vec<Statement> {
    let bucket_resource = format!("{RESOURCE_PREFIX}{bucket}");
    let object_resource = format!("{RESOURCE_PREFIX}{bucket}/{prefix}*");
    let (read_only_in_use, write_only_in_use) = in_use_policy(&statements, bucket, prefix);
    let mut out = Vec::new();
    let mut read_only_bucket_statements = Vec::new();
    let mut s3_prefix_values = StringSet::new();
    for mut statement in statements {
        if !is_valid_statement(&statement, bucket) {
            out.push(statement);
            continue;
        }
        if statement.resources.contains(&bucket_resource) {
            if statement.conditions.is_empty() {
                remove_bucket_actions(
                    &mut statement,
                    prefix,
                    &bucket_resource,
                    read_only_in_use,
                    write_only_in_use,
                );
            } else {
                remove_bucket_actions(&mut statement, prefix, &bucket_resource, false, false);
            }
        } else if statement.resources.contains(&object_resource) {
            remove_object_actions(&mut statement, &object_resource);
        }
        if statement.actions.is_empty() {
            continue;
        }
        if statement.resources.contains(&bucket_resource)
            && contains_all(&statement.actions, &read_only_bucket_actions())
            && statement.allows_anyone()
        {
            if !statement.conditions.is_empty() {
                for operator in ["StringEquals", "StringLike"] {
                    if let Some(values) = statement
                        .conditions
                        .get(operator)
                        .and_then(|keys| keys.get("s3:prefix"))
                    {
                        s3_prefix_values.extend(values.iter().map(|value| {
                            format!("{bucket_resource}/{}*", value.trim_end_matches('*'))
                        }));
                    }
                }
            } else if !s3_prefix_values.is_empty() {
                read_only_bucket_statements.push(statement);
                continue;
            }
        }
        out.push(statement);
    }
    let resource_prefix = format!("{bucket_resource}/");
    let skip_bucket_statement = !out.iter().any(|statement| {
        statement.has_resource_prefix(&resource_prefix)
            && s3_prefix_values.is_disjoint(&statement.resources)
    });
    for statement in read_only_bucket_statements {
        if skip_bucket_statement
            && statement.resources.contains(&bucket_resource)
            && statement.allows_anyone()
            && statement.conditions.is_empty()
        {
            continue;
        }
        out.push(statement);
    }
    if out.len() == 1 {
        let statement = &out[0];
        if statement.resources.contains(&bucket_resource)
            && contains_all(&statement.actions, &common_bucket_actions())
            && statement.allows_anyone()
            && statement.conditions.is_empty()
        {
            out.clear();
        }
    }
    out
}

fn merge_conditions(left: &ConditionMap, right: &ConditionMap) -> ConditionMap {
    let mut merged = left.clone();
    for (operator, keys) in right {
        let entry = merged.entry(operator.clone()).or_default();
        for (key, values) in keys {
            entry
                .entry(key.clone())
                .or_default()
                .extend(values.iter().cloned());
        }
    }
    merged
}

fn append_statement(mut statements: Vec<Statement>, statement: Statement) -> Vec<Statement> {
    for existing in statements.iter_mut() {
        let same_principal = existing.effect == statement.effect
            && existing.principal_aws == statement.principal_aws
            && existing.conditions == statement.conditions;
        if existing.actions == statement.actions && same_principal {
            existing
                .resources
                .extend(statement.resources.iter().cloned());
            return statements;
        }
        if existing.resources == statement.resources && same_principal {
            existing.actions.extend(statement.actions.iter().cloned());
            return statements;
        }
        if statement.resources.is_subset(&existing.resources)
            && statement.actions.is_subset(&existing.actions)
            && existing.effect == statement.effect
            && statement.principal_aws.is_subset(&existing.principal_aws)
        {
            if existing.conditions == statement.conditions {
                return statements;
            }
            if !existing.conditions.is_empty()
                && !statement.conditions.is_empty()
                && existing.resources == statement.resources
            {
                existing.conditions = merge_conditions(&existing.conditions, &statement.conditions);
                return statements;
            }
        }
    }
    if !statement.actions.is_empty() || !statement.resources.is_empty() {
        statements.push(statement);
    }
    statements
}

/// minio-go `SetPolicy`.
pub fn set_policy(
    statements: Vec<Statement>,
    policy: BucketPolicy,
    bucket: &str,
    prefix: &str,
) -> Vec<Statement> {
    let mut out = remove_statements(statements, bucket, prefix);
    for statement in new_statements(policy, bucket, prefix) {
        out = append_statement(out, statement);
    }
    out
}

fn bucket_policy_flags(statement: &Statement, prefix: &str) -> (bool, bool, bool) {
    let (mut common, mut read_only, mut write_only) = (false, false, false);
    if !statement.allows_anyone() {
        return (common, read_only, write_only);
    }
    let no_conditions = statement.conditions.is_empty();
    if contains_all(&statement.actions, &common_bucket_actions()) && no_conditions {
        common = true;
    }
    if contains_all(&statement.actions, &write_only_bucket_actions()) && no_conditions {
        write_only = true;
    }
    if contains_all(&statement.actions, &read_only_bucket_actions()) {
        if !prefix.is_empty() && !no_conditions {
            let values = |operator: &str| {
                statement
                    .conditions
                    .get(operator)
                    .map(|keys| keys.get("s3:prefix"))
            };
            if let Some(values) = values("StringEquals") {
                read_only = values.is_some_and(|values| values.contains(prefix));
            } else if let Some(values) = values("StringNotEquals") {
                read_only = values.is_some_and(|values| !values.contains(prefix));
            } else if let Some(values) = values("StringLike") {
                read_only = values.is_some_and(|values| values.contains(&format!("{prefix}*")));
            } else if let Some(values) = values("StringNotLike") {
                read_only = values.is_some_and(|values| !values.contains(&format!("{prefix}*")));
            }
        } else if no_conditions {
            read_only = true;
        }
    }
    (common, read_only, write_only)
}

fn object_policy_flags(statement: &Statement) -> (bool, bool) {
    if statement.allows_anyone() && statement.conditions.is_empty() {
        (
            contains_all(&statement.actions, &read_only_object_actions()),
            contains_all(&statement.actions, &write_only_object_actions()),
        )
    } else {
        (false, false)
    }
}

/// minio-go `resourceMatch` (`*` wildcards).
fn resource_match(pattern: &str, resource: &str) -> bool {
    if pattern.is_empty() {
        return resource.is_empty();
    }
    if pattern == "*" {
        return true;
    }
    let parts = pattern.split('*').collect::<Vec<_>>();
    if parts.len() == 1 {
        return resource == pattern;
    }
    let trailing_glob = pattern.ends_with('*');
    let end = parts.len() - 1;
    if !resource.starts_with(parts[0]) {
        return false;
    }
    let mut rest = resource;
    for part in &parts[1..end] {
        let Some(index) = rest.find(part) else {
            return false;
        };
        rest = &rest[index + part.len()..];
    }
    trailing_glob || rest.ends_with(parts[end])
}

/// minio-go `GetPolicy`: canned policy in effect for bucket/prefix.
pub fn get_policy(statements: &[Statement], bucket: &str, prefix: &str) -> BucketPolicy {
    let bucket_resource = format!("{RESOURCE_PREFIX}{bucket}");
    let object_resource = format!("{RESOURCE_PREFIX}{bucket}/{prefix}*");
    let (mut bucket_common, mut bucket_read, mut bucket_write) = (false, false, false);
    let (mut object_read, mut object_write) = (false, false);
    let mut matched = String::new();
    for statement in statements {
        let matched_resources: Vec<&String> = if statement.resources.contains(&object_resource) {
            vec![&object_resource]
        } else {
            statement
                .resources
                .iter()
                .filter(|resource| resource_match(resource, &object_resource))
                .collect()
        };
        if !matched_resources.is_empty() {
            let (read, write) = object_policy_flags(statement);
            for resource in matched_resources {
                if matched.len() < resource.len() {
                    object_read = read;
                    object_write = write;
                    matched = resource.clone();
                } else if matched.len() == resource.len() {
                    object_read |= read;
                    object_write |= write;
                    matched = resource.clone();
                }
            }
        }
        if statement.resources.contains(&bucket_resource) {
            let (common, read, write) = bucket_policy_flags(statement, prefix);
            bucket_common |= common;
            bucket_read |= read;
            bucket_write |= write;
        }
    }
    if !bucket_common {
        return BucketPolicy::None;
    }
    if bucket_read && bucket_write && object_read && object_write {
        BucketPolicy::ReadWrite
    } else if bucket_read && object_read {
        BucketPolicy::ReadOnly
    } else if bucket_write && object_write {
        BucketPolicy::WriteOnly
    } else {
        BucketPolicy::None
    }
}

/// minio-go `GetPolicies`: `bucket/prefix[*]` -> canned policy for every object resource.
pub fn get_policies(
    statements: &[Statement],
    bucket: &str,
    prefix: &str,
) -> BTreeMap<String, BucketPolicy> {
    let bucket_resource = format!("{RESOURCE_PREFIX}{bucket}");
    let object_prefix = format!("{bucket_resource}/{prefix}");
    let resources = statements
        .iter()
        .flat_map(|statement| statement.resources.iter())
        .filter(|resource| resource.starts_with(&object_prefix))
        .collect::<BTreeSet<_>>();
    let mut rules = BTreeMap::new();
    for resource in resources {
        let (resource, asterisk) = match resource.strip_suffix('*') {
            Some(stripped) => (stripped, "*"),
            None => (resource.as_str(), ""),
        };
        let object_path = resource
            .get(bucket_resource.len() + 1..)
            .unwrap_or_default();
        rules.insert(
            format!("{bucket}/{object_path}{asterisk}"),
            get_policy(statements, bucket, object_path),
        );
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(existing: Vec<Statement>, policy: BucketPolicy, prefix: &str) -> Vec<Statement> {
        set_policy(existing, policy, "b", prefix)
    }

    #[test]
    fn download_policy_matches_minio_go() {
        let statements = apply(Vec::new(), BucketPolicy::ReadOnly, "");
        let json = PolicyDocument {
            statements: statements.clone(),
            ..Default::default()
        }
        .to_json();
        assert_eq!(
            json,
            serde_json::json!({"Version":"2012-10-17","Statement":[
                {"Action":["s3:GetBucketLocation","s3:ListBucket"],"Effect":"Allow","Principal":{"AWS":["*"]},"Resource":["arn:aws:s3:::b"],"Sid":""},
                {"Action":["s3:GetObject"],"Effect":"Allow","Principal":{"AWS":["*"]},"Resource":["arn:aws:s3:::b/*"],"Sid":""}
            ]})
        );
        assert_eq!(get_policy(&statements, "b", ""), BucketPolicy::ReadOnly);
        assert_eq!(get_policy(&statements, "b", "dir/"), BucketPolicy::ReadOnly);
    }

    #[test]
    fn set_none_removes_everything() {
        for policy in [
            BucketPolicy::ReadOnly,
            BucketPolicy::WriteOnly,
            BucketPolicy::ReadWrite,
        ] {
            let statements = apply(Vec::new(), policy, "");
            assert_eq!(get_policy(&statements, "b", ""), policy);
            assert!(apply(statements, BucketPolicy::None, "").is_empty());
        }
    }

    #[test]
    fn prefix_policies_are_independent() {
        let statements = apply(Vec::new(), BucketPolicy::ReadOnly, "public/");
        let statements = apply(statements, BucketPolicy::WriteOnly, "uploads/");
        assert_eq!(
            get_policy(&statements, "b", "public/"),
            BucketPolicy::ReadOnly
        );
        assert_eq!(
            get_policy(&statements, "b", "uploads/"),
            BucketPolicy::WriteOnly
        );
        assert_eq!(get_policy(&statements, "b", ""), BucketPolicy::None);
        let rules = get_policies(&statements, "b", "");
        assert_eq!(
            rules,
            BTreeMap::from([
                ("b/public/*".to_string(), BucketPolicy::ReadOnly),
                ("b/uploads/*".to_string(), BucketPolicy::WriteOnly),
            ])
        );
        let statements = apply(statements, BucketPolicy::None, "public/");
        assert_eq!(get_policy(&statements, "b", "public/"), BucketPolicy::None);
        assert_eq!(
            get_policy(&statements, "b", "uploads/"),
            BucketPolicy::WriteOnly
        );
    }

    #[test]
    fn prefix_none_removes_conditional_list_statement() {
        let statements = apply(Vec::new(), BucketPolicy::ReadOnly, "dir");
        assert!(apply(statements.clone(), BucketPolicy::None, "dir").is_empty());
        let statements = apply(statements, BucketPolicy::WriteOnly, "up/");
        let statements = apply(statements, BucketPolicy::None, "dir");
        assert_eq!(get_policy(&statements, "b", "up/"), BucketPolicy::WriteOnly);
        assert_eq!(get_policy(&statements, "b", "dir"), BucketPolicy::None);
        assert!(apply(statements, BucketPolicy::None, "up/").is_empty());
    }

    #[test]
    fn parses_string_principal_and_single_values() {
        let document = parse_policy(
            r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::b/x*"}}"#,
        )
        .unwrap();
        let statement = &document.statements[0];
        assert!(statement.principal_aws.contains("*"));
        assert_eq!(statement.actions, set_of(&["s3:GetObject"]));
        assert!(parse_policy("not json").is_err());
    }

    #[test]
    fn keeps_foreign_statements() {
        let foreign = Statement {
            actions: set_of(&["s3:GetObject"]),
            effect: "Allow".into(),
            principal_aws: set_of(&["arn:aws:iam::1:user/x"]),
            resources: set_of(&["arn:aws:s3:::b/*"]),
            ..Default::default()
        };
        let statements = apply(vec![foreign.clone()], BucketPolicy::None, "");
        assert_eq!(statements, vec![foreign]);
    }

    #[test]
    fn resource_match_handles_wildcards() {
        assert!(resource_match("arn:aws:s3:::b/*", "arn:aws:s3:::b/x*"));
        assert!(resource_match("arn:aws:s3:::b/*/y*", "arn:aws:s3:::b/a/y*"));
        assert!(!resource_match("arn:aws:s3:::c/*", "arn:aws:s3:::b/x*"));
        assert!(resource_match("*", "anything"));
    }

    #[test]
    fn canned_names() {
        assert_eq!(canned_policy("private").unwrap(), BucketPolicy::None);
        assert_eq!(canned_policy("none").unwrap(), BucketPolicy::None);
        assert!(canned_policy("bogus").is_err());
        assert_eq!(access_perm("readonly"), "download");
        assert_eq!(access_perm("none"), "private");
    }
}
