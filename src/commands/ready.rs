//! `ready` (mc ready): waits until MinIO's cluster health endpoint reports healthy.

use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use std::time::Duration;

const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Args)]
pub struct HealthArgs {
    /// check if the cluster has enough read quorum
    #[arg(long = "cluster-read")]
    pub cluster_read: bool,
    /// check if the cluster is taken down for maintenance
    #[arg(long)]
    pub maintenance: bool,
    pub target: String,
}

#[derive(Debug, Default, Serialize)]
struct ReadyMessage<'a> {
    status: &'a str,
    alias: &'a str,
    healthy: bool,
    #[serde(rename = "maintenanceMode")]
    maintenance_mode: bool,
    #[serde(rename = "writeQuorum")]
    write_quorum: i64,
    #[serde(rename = "healingDrives")]
    healing_drives: i64,
    error: Option<String>,
}

impl ReadyMessage<'_> {
    fn text(&self) -> String {
        match (&self.error, self.healthy) {
            (_, true) => format!("The cluster '{}' is ready", self.alias),
            (Some(error), false) => {
                format!("The cluster '{}' is unreachable: {error}", self.alias)
            }
            (None, false) => format!("The cluster '{}' is not ready", self.alias),
        }
    }
}

pub fn run(args: HealthArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let alias = alias_config(&store, args.target.split('/').next().unwrap_or_default())
        .with_context(|| format!("Couldn't construct anonymous client for `{}`.", args.target))?;
    let (path, query) = if args.cluster_read {
        ("/minio/health/cluster/read", None)
    } else {
        (
            "/minio/health/cluster",
            args.maintenance.then_some("maintenance=true"),
        )
    };
    let rt = runtime()?;
    loop {
        let mut message = ReadyMessage {
            status: "success",
            alias: &args.target,
            ..Default::default()
        };
        match rt.block_on(crate::s3::health_request(&alias, path, query)) {
            Ok(response) => {
                message.healthy = response.status == 200;
                message.maintenance_mode = !args.cluster_read && response.status == 412;
                let number = |name: &str| {
                    response
                        .header(name)
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or_default()
                };
                if !args.cluster_read {
                    message.write_quorum = number("x-minio-write-quorum");
                    message.healing_drives = number("x-minio-healing-drives");
                }
            }
            Err(error) => message.error = Some(format!("{error:#}")),
        }
        if json {
            crate::output::print_json(&message)?;
        } else {
            println!("{}", message.text());
        }
        if message.healthy {
            return Ok(());
        }
        std::thread::sleep(HEALTH_CHECK_INTERVAL);
    }
}
