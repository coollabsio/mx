//! TLS trust: extra CAs from `<config dir>/certs/CAs/` and the `--insecure` HTTP client.
//!
//! The SDK's HTTP client cannot disable certificate verification, so `--insecure` uses a small
//! hyper-util + hyper-rustls client with a verifier that accepts any certificate (handshake
//! signatures are still checked). It honors `--resolve` like the default client.

use crate::resolve::PinnedDnsResolver;
use anyhow::{Context, Result};
use aws_smithy_runtime_api::client::dns::ResolveDns;
use aws_smithy_runtime_api::client::http::{
    HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings, SharedHttpClient,
    SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::body::SdkBody;
use hyper_util::client::legacy::connect::HttpConnector as HyperHttpConnector;
use hyper_util::client::legacy::connect::dns::Name;
use hyper_util::rt::TokioExecutor;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::{DigitallySignedStruct, SignatureScheme};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context as TaskContext, Poll};

/// `certs/CAs` under the active config dir: `--config-dir`, else `~/.mx` or `~/.mc` (the one
/// holding the config file, preferring `~/.mx`), like config file selection.
pub fn cas_dir() -> Option<PathBuf> {
    let config_dir = match crate::globals::config_dir() {
        Some(dir) => dir.to_path_buf(),
        None => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .or_else(dirs::home_dir)?;
            let mc = home.join(".mc");
            if !home.join(".mx/config.json").exists() && mc.join("config.json").exists() {
                mc
            } else {
                home.join(".mx")
            }
        }
    };
    Some(config_dir.join("certs").join("CAs"))
}

/// Loads every certificate from the PEM/CRT files in `dir` (missing dir -> empty). Files
/// without any PEM certificate are skipped; malformed PEM or certificates are errors.
pub fn load_ca_certs(dir: &Path) -> Result<Vec<Vec<u8>>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("Unable to read `{}`.", dir.display()));
        }
    };
    let mut paths: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    paths.sort();

    let mut pems = Vec::new();
    for path in paths {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("Unable to read CA file `{}`.", path.display()))?;
        let certs = CertificateDer::pem_slice_iter(&bytes)
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| format!("Unable to parse CA file `{}`.", path.display()))?;
        if certs.is_empty() {
            continue;
        }
        // Validate now: the SDK panics on certificates it cannot parse.
        let mut store = rustls::RootCertStore::empty();
        for cert in certs {
            store
                .add(cert)
                .with_context(|| format!("Invalid CA certificate in `{}`.", path.display()))?;
        }
        pems.push(bytes);
    }
    Ok(pems)
}

/// HTTP client that skips server certificate verification (`--insecure`).
pub fn insecure_http_client(resolver: PinnedDnsResolver) -> Result<SharedHttpClient> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .context("Unable to configure TLS.")?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
        .with_no_client_auth();
    Ok(SharedHttpClient::new(InsecureClient {
        config,
        resolver,
        connector: OnceLock::new(),
    }))
}

#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl NoVerify {
    fn algorithms(&self) -> &WebPkiSupportedAlgorithms {
        &self.0.signature_verification_algorithms
    }
}

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, self.algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, self.algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms().supported_schemes()
    }
}

#[derive(Debug)]
struct InsecureClient {
    config: rustls::ClientConfig,
    resolver: PinnedDnsResolver,
    /// mx uses one set of connector settings per process, so one connector is enough.
    connector: OnceLock<SharedHttpConnector>,
}

impl HttpClient for InsecureClient {
    fn http_connector(
        &self,
        settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        self.connector
            .get_or_init(|| {
                let mut http =
                    HyperHttpConnector::new_with_resolver(Resolver(self.resolver.clone()));
                http.enforce_http(false);
                http.set_nodelay(true);
                http.set_connect_timeout(settings.connect_timeout());
                let https = hyper_rustls::HttpsConnectorBuilder::new()
                    .with_tls_config(self.config.clone())
                    .https_or_http()
                    .enable_http1()
                    .wrap_connector(http);
                let client =
                    hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(https);
                SharedHttpConnector::new(InsecureConnector { client })
            })
            .clone()
    }
}

type HyperClient = hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<HyperHttpConnector<Resolver>>,
    SdkBody,
>;

#[derive(Debug)]
struct InsecureConnector {
    client: HyperClient,
}

impl HttpConnector for InsecureConnector {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let request = match request.try_into_http1x() {
            Ok(request) => request,
            Err(err) => return HttpConnectorFuture::ready(Err(ConnectorError::user(err.into()))),
        };
        let future = self.client.request(request);
        HttpConnectorFuture::new(async move {
            let response = future
                .await
                .map_err(|err| {
                    if err.is_connect() {
                        ConnectorError::io(err.into())
                    } else {
                        ConnectorError::other(err.into(), None)
                    }
                })?
                .map(SdkBody::from_body_1_x);
            HttpResponse::try_from(response).map_err(|err| ConnectorError::other(err.into(), None))
        })
    }
}

/// Adapts the `--resolve` resolver to hyper-util's DNS service interface.
#[derive(Debug, Clone)]
struct Resolver(PinnedDnsResolver);

type ResolveFuture =
    Pin<Box<dyn Future<Output = Result<std::vec::IntoIter<SocketAddr>, BoxError>> + Send>>;
type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl tower_service::Service<Name> for Resolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = BoxError;
    type Future = ResolveFuture;

    fn poll_ready(&mut self, _cx: &mut TaskContext<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let resolver = self.0.clone();
        Box::pin(async move {
            let ips = resolver.resolve_dns(name.as_str()).await?;
            let addrs: Vec<_> = ips.into_iter().map(|ip| SocketAddr::new(ip, 0)).collect();
            Ok(addrs.into_iter())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::load_ca_certs;

    const CERT: &str = include_str!("../../tests/fixtures/test-ca.pem");

    #[test]
    fn loads_pem_files_and_skips_other_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            load_ca_certs(&dir.path().join("missing"))
                .unwrap()
                .is_empty()
        );
        std::fs::write(dir.path().join("a.crt"), CERT).unwrap();
        std::fs::write(dir.path().join("README"), "not a certificate").unwrap();
        assert_eq!(load_ca_certs(dir.path()).unwrap().len(), 1);
    }

    #[test]
    fn rejects_malformed_certificates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("bad.pem"),
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        assert!(load_ca_certs(dir.path()).is_err());
    }
}
