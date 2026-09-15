use crate::cli::{AnonymousCommand, AnonymousSetArgs, TargetArg};
use crate::commands::runtime;
use crate::commands::util::require_s3;
use crate::config::ConfigStore;
use crate::output;
use anyhow::{Result, bail};

pub fn run(command: AnonymousCommand, json: bool) -> Result<()> {
    match command {
        AnonymousCommand::Set(args) => set(args, json),
        AnonymousCommand::Get(args) => get(args, json),
    }
}

fn set(args: AnonymousSetArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let policy_name = args.policy.to_ascii_lowercase();
    match policy_name.as_str() {
        "none" | "private" => {
            runtime()?.block_on(crate::s3::delete_bucket_policy(&alias, &bucket))?;
        }
        "download" | "upload" | "public" => {
            let policy = policy_document(&bucket, &policy_name);
            runtime()?.block_on(crate::s3::put_bucket_policy(&alias, &bucket, &policy))?;
        }
        _ => bail!("policy must be one of download, upload, public, none"),
    }
    if json {
        println!(r#"{{"status":"success","policy":"{}"}}"#, args.policy);
    } else {
        output::print_plain(&format!(
            "Access permission for `{}` is `{}`",
            args.target, args.policy
        ));
    }
    Ok(())
}

fn get(args: TargetArg, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let (alias, target) = require_s3(&store, &args.target)?;
    let bucket = target.require_bucket()?.to_string();
    let policy = runtime()?.block_on(crate::s3::get_bucket_policy(&alias, &bucket))?;
    let name = classify_policy(policy.as_deref());
    if json {
        println!(r#"{{"status":"success","policy":"{name}"}}"#);
    } else {
        println!("Access permission for `{}` is `{name}`", args.target);
    }
    Ok(())
}

fn policy_document(bucket: &str, policy: &str) -> String {
    let actions = match policy {
        "download" => r#"["s3:GetObject"]"#,
        "upload" => r#"["s3:PutObject"]"#,
        _ => r#"["s3:GetObject","s3:PutObject","s3:ListBucket"]"#,
    };
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":["*"]}},"Action":{actions},"Resource":["arn:aws:s3:::{bucket}","arn:aws:s3:::{bucket}/*"]}}]}}"#
    )
}

fn classify_policy(policy: Option<&str>) -> &'static str {
    let Some(policy) = policy else {
        return "none";
    };
    let get = policy.contains("s3:GetObject");
    let put = policy.contains("s3:PutObject");
    match (get, put) {
        (true, true) => "public",
        (true, false) => "download",
        (false, true) => "upload",
        _ => "none",
    }
}
