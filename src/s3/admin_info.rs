//! MinIO admin `ServerInfo` (`GET /minio/admin/v3/info`), read-only; used by `ping -a/--node`.

use super::admin::AdminClient;
use anyhow::{Context, Result};
use serde::Deserialize;

/// One server of `madmin.InfoMessage.Servers` (only the fields mx needs).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ServerProperties {
    /// `host:port` as the cluster knows the node.
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub scheme: String,
}

#[derive(Debug, Deserialize)]
struct InfoMessage {
    #[serde(default)]
    servers: Vec<ServerProperties>,
}

/// Servers of the cluster behind `client`.
pub async fn server_info(client: &AdminClient) -> Result<Vec<ServerProperties>> {
    let response = client.admin("GET", "info", &[], Vec::new()).await?;
    let info: InfoMessage =
        serde_json::from_slice(&response.body).context("invalid server info response")?;
    Ok(info.servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_servers() {
        let info: InfoMessage = serde_json::from_str(
            r#"{"mode":"online","servers":[{"state":"online","endpoint":"10.0.0.1:9000","scheme":"https"},{"endpoint":"10.0.0.2:9000"}]}"#,
        )
        .unwrap();
        assert_eq!(info.servers.len(), 2);
        assert_eq!(info.servers[0].scheme, "https");
        assert_eq!(info.servers[1].endpoint, "10.0.0.2:9000");
    }
}
