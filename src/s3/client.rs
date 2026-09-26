//! S3 client construction (area A owns this file): endpoint/path-style, `--resolve` pinning,
//! TLS, and request interceptors.

use crate::config::model::AliasConfig;
use anyhow::{Result, bail};
use aws_config::BehaviorVersion;
use aws_credential_types::{Credentials, provider::SharedCredentialsProvider};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{ConfigBag, Intercept, RuntimeComponents};
use aws_smithy_http_client::{
    Builder as HttpClientBuilder,
    tls::{self, rustls_provider::CryptoMode},
};
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::context::BeforeTransmitInterceptorContextMut;

/// Builds an S3 client for an alias. Cheap enough per command, but reuse the returned client
/// for bulk operations (`Client` is `Clone` and shares its connection pool).
pub async fn build_client(alias: &AliasConfig) -> Result<Client> {
    if alias.api.eq_ignore_ascii_case("S3v2") {
        bail!("API `{}` is not supported yet.", alias.api);
    }

    let credentials = Credentials::new(
        alias.access_key.clone(),
        alias.secret_key.clone(),
        alias.session_token.clone(),
        None,
        "mx",
    );

    let mut loader = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .credentials_provider(SharedCredentialsProvider::new(credentials));
    let endpoint = url::Url::parse(&alias.url)?;
    let endpoint_host = endpoint.host_str().unwrap_or_default();
    let endpoint_port = endpoint.port_or_known_default();
    let mappings: Vec<_> = crate::resolve::configured()
        .into_iter()
        .filter(|mapping| {
            mapping.host.eq_ignore_ascii_case(endpoint_host) && Some(mapping.port) == endpoint_port
        })
        .collect();
    if !mappings.is_empty() {
        let resolver = crate::resolve::PinnedDnsResolver::new(&mappings)?;
        let http_client = HttpClientBuilder::new()
            .tls_provider(tls::Provider::Rustls(CryptoMode::AwsLc))
            .build_with_resolver(resolver);
        loader = loader.http_client(http_client);
    }
    let shared_config = loader.load().await;

    let force_path_style = force_path_style(alias)?;
    let config = aws_sdk_s3::config::Builder::from(&shared_config)
        .endpoint_url(alias.url.clone())
        .force_path_style(force_path_style)
        .request_checksum_calculation(aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired)
        .response_checksum_validation(aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired)
        .interceptor(StripFlexibleChecksums)
        .build();

    Ok(Client::from_conf(config))
}

pub fn force_path_style(alias: &AliasConfig) -> Result<bool> {
    match alias.path.to_ascii_lowercase().as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        "dns" => Ok(false),
        _ => {
            let parsed = url::Url::parse(&alias.url)?;
            let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
            Ok(!(host.ends_with("amazonaws.com") || host == "storage.googleapis.com"))
        }
    }
}

/// Strips unsigned SDK metadata headers that MinIO rejects. Despite the name, this does NOT
/// remove `x-amz-checksum-*` headers: explicitly requested checksums (see
/// `objects::checksum_override`) are still sent.
#[derive(Debug)]
struct StripFlexibleChecksums;

impl Intercept for StripFlexibleChecksums {
    fn name(&self) -> &'static str {
        "strip-flexible-checksums"
    }

    fn modify_before_signing(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        strip_unsigned_sdk_headers(context.request_mut().headers_mut());
        Ok(())
    }
}

fn strip_unsigned_sdk_headers(headers: &mut aws_smithy_runtime_api::http::Headers) {
    for name in ["amz-sdk-invocation-id", "amz-sdk-request"] {
        let _ = headers.remove(name);
    }
}

/// True when both aliases point at the same endpoint with the same credentials, so a
/// server-side CopyObject is possible.
pub fn same_endpoint_and_credentials(left: &AliasConfig, right: &AliasConfig) -> bool {
    left.url == right.url
        && left.access_key == right.access_key
        && left.secret_key == right.secret_key
        && left.session_token == right.session_token
}

#[cfg(test)]
mod tests {
    use super::force_path_style;
    use crate::config::model::AliasConfig;

    #[test]
    fn forces_path_style_for_minio_auto() {
        let alias = AliasConfig {
            url: "http://localhost:9000".into(),
            path: "auto".into(),
            ..Default::default()
        };

        assert!(force_path_style(&alias).unwrap());
    }

    #[test]
    fn disables_path_style_for_aws_dns() {
        let alias = AliasConfig {
            url: "https://s3.amazonaws.com".into(),
            path: "dns".into(),
            ..Default::default()
        };

        assert!(!force_path_style(&alias).unwrap());
    }
}
