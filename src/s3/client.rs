//! S3 client construction (area A owns this file): endpoint/path-style, `--resolve` pinning,
//! TLS (`--insecure`, `certs/CAs`), and request interceptors (`-H`, `--debug`, `--limit-*`).

use crate::config::model::AliasConfig;
use crate::net::throttle::{Limiter, throttle_body};
use crate::net::trace;
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
use aws_smithy_runtime_api::client::http::SharedHttpClient;
use aws_smithy_runtime_api::client::interceptors::context::{
    BeforeDeserializationInterceptorContextMut, BeforeTransmitInterceptorContextMut,
};
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::config_bag::{Storable, StoreReplace};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

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

    let loader = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .credentials_provider(SharedCredentialsProvider::new(credentials));
    let loader = match http_client(alias)? {
        Some(http_client) => loader.http_client(http_client),
        None => loader,
    };
    let shared_config = loader.load().await;

    let force_path_style = force_path_style(alias)?;
    let config = aws_sdk_s3::config::Builder::from(&shared_config)
        .endpoint_url(alias.url.clone())
        .force_path_style(force_path_style)
        .request_checksum_calculation(aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired)
        .response_checksum_validation(aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired)
        .interceptor(StripFlexibleChecksums)
        .interceptor(GlobalFlags::from_globals())
        .build();

    Ok(Client::from_conf(config))
}

/// Picks the HTTP client for the global flags: the SDK default unless `--resolve` pins this
/// endpoint, extra CAs exist in `certs/CAs`, or `--insecure` is set.
fn http_client(alias: &AliasConfig) -> Result<Option<SharedHttpClient>> {
    let endpoint = url::Url::parse(&alias.url)?;
    let endpoint_host = endpoint.host_str().unwrap_or_default();
    let endpoint_port = endpoint.port_or_known_default();
    let mappings: Vec<_> = crate::resolve::configured()
        .into_iter()
        .filter(|mapping| {
            mapping.host.eq_ignore_ascii_case(endpoint_host) && Some(mapping.port) == endpoint_port
        })
        .collect();
    let resolver = crate::resolve::PinnedDnsResolver::new(&mappings)?;

    if crate::globals::insecure() {
        return Ok(Some(crate::net::tls::insecure_http_client(resolver)?));
    }

    let tls_context = crate::net::tls::custom_ca_context()?;
    if mappings.is_empty() && tls_context.is_none() {
        return Ok(None);
    }
    let tls_context = tls_context.unwrap_or_default();
    let builder = HttpClientBuilder::new()
        .tls_provider(tls::Provider::Rustls(CryptoMode::AwsLc))
        .tls_context(tls_context);
    Ok(Some(if mappings.is_empty() {
        builder.build_https()
    } else {
        builder.build_with_resolver(resolver)
    }))
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

/// Strips unsigned SDK metadata headers that MinIO rejects. The SDK adds them in its own
/// `modify_before_transmit` hooks (after signing), so the strip must run there too; client
/// interceptors run after the SDK's. Despite the name, this does NOT remove `x-amz-checksum-*`
/// headers: explicitly requested checksums (see `objects::checksum_override`) are still sent.
#[derive(Debug)]
struct StripFlexibleChecksums;

impl Intercept for StripFlexibleChecksums {
    fn name(&self) -> &'static str {
        "strip-flexible-checksums"
    }

    fn modify_before_transmit(
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

/// Applies `-H/--custom-header`, `--debug` and `--limit-upload/--limit-download` to every
/// request of the client.
#[derive(Debug)]
struct GlobalFlags {
    custom_headers: Vec<(String, String)>,
    debug: bool,
    upload: Option<Arc<Limiter>>,
    download: Option<Arc<Limiter>>,
}

/// Process-wide limiters so concurrent transfers share one budget (like mc).
static UPLOAD_LIMITER: OnceLock<Option<Arc<Limiter>>> = OnceLock::new();
static DOWNLOAD_LIMITER: OnceLock<Option<Arc<Limiter>>> = OnceLock::new();

impl GlobalFlags {
    fn from_globals() -> Self {
        Self {
            custom_headers: crate::globals::custom_headers().to_vec(),
            debug: crate::globals::debug(),
            upload: UPLOAD_LIMITER
                .get_or_init(|| crate::globals::limit_upload().map(Limiter::new))
                .clone(),
            download: DOWNLOAD_LIMITER
                .get_or_init(|| crate::globals::limit_download().map(Limiter::new))
                .clone(),
        }
    }
}

/// Request start time, for the `--debug` response time line.
#[derive(Debug, Clone)]
struct TraceStart(Instant);

impl Storable for TraceStart {
    type Storer = StoreReplace<Self>;
}

impl Intercept for GlobalFlags {
    fn name(&self) -> &'static str {
        "mx-global-flags"
    }

    fn modify_before_signing(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let headers = context.request_mut().headers_mut();
        for (name, value) in &self.custom_headers {
            headers.append(name.clone(), value.clone());
        }
        Ok(())
    }

    fn modify_before_transmit(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let request = context.request_mut();
        if let Some(limiter) = &self.upload {
            let body = std::mem::replace(request.body_mut(), SdkBody::taken());
            *request.body_mut() = throttle_body(body, limiter.clone());
        }
        if self.debug {
            trace::print(&trace::format_request(
                request.method(),
                request.uri(),
                request.headers(),
            ));
            cfg.interceptor_state()
                .store_put(TraceStart(Instant::now()));
        }
        Ok(())
    }

    fn modify_before_deserialization(
        &self,
        context: &mut BeforeDeserializationInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let response = context.response_mut();
        if self.debug {
            trace::print(&trace::format_response(
                response.status().as_u16(),
                response.headers(),
            ));
            if let Some(TraceStart(start)) = cfg.load::<TraceStart>() {
                trace::print(&format!("Response Time: {:?}\n\n", start.elapsed()));
            }
        }
        if let Some(limiter) = &self.download {
            let body = std::mem::replace(response.body_mut(), SdkBody::taken());
            *response.body_mut() = throttle_body(body, limiter.clone());
        }
        Ok(())
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
