use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const CONFIG_VERSION: &str = "10";
pub const DEFAULT_ACCESS_KEY: &str = "YOUR-ACCESS-KEY-HERE";
pub const DEFAULT_SECRET_KEY: &str = "YOUR-SECRET-KEY-HERE";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigV10 {
    pub version: String,
    #[serde(default)]
    pub aliases: BTreeMap<String, AliasConfig>,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

impl ConfigV10 {
    pub fn new_with_defaults() -> Self {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "local".to_string(),
            AliasConfig {
                url: "http://localhost:9000".to_string(),
                access_key: String::new(),
                secret_key: String::new(),
                api: "S3v4".to_string(),
                path: "auto".to_string(),
                ..Default::default()
            },
        );
        aliases.insert(
            "s3".to_string(),
            AliasConfig {
                url: "https://s3.amazonaws.com".to_string(),
                access_key: DEFAULT_ACCESS_KEY.to_string(),
                secret_key: DEFAULT_SECRET_KEY.to_string(),
                api: "S3v4".to_string(),
                path: "dns".to_string(),
                ..Default::default()
            },
        );
        aliases.insert(
            "gcs".to_string(),
            AliasConfig {
                url: "https://storage.googleapis.com".to_string(),
                access_key: DEFAULT_ACCESS_KEY.to_string(),
                secret_key: DEFAULT_SECRET_KEY.to_string(),
                api: "S3v2".to_string(),
                path: "dns".to_string(),
                ..Default::default()
            },
        );
        aliases.insert(
            "play".to_string(),
            AliasConfig {
                url: "https://play.min.io".to_string(),
                access_key: "Q3AM3UQ867SPQQA43P2F".to_string(),
                secret_key: "zuf+tfteSlswRu7BJ86wekitnifILbZam1KYY3TG".to_string(),
                api: "S3v4".to_string(),
                path: "auto".to_string(),
                ..Default::default()
            },
        );

        Self {
            version: CONFIG_VERSION.to_string(),
            aliases,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AliasConfig {
    pub url: String,
    #[serde(rename = "accessKey")]
    pub access_key: String,
    #[serde(rename = "secretKey")]
    pub secret_key: String,
    #[serde(rename = "sessionToken", skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    pub api: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(rename = "apiKey", skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}
