//! TLS trust (`--insecure`, `<config dir>/certs/CAs/`) and the custom HTTP stack.
//!
//! The SDK's HTTP client cannot disable certificate verification, pin self-signed server
//! certificates or put deadlines on socket IO, so `--insecure`, a non-empty `certs/CAs` and
//! `--conn-read/write-deadline` use a small hyper-util + hyper-rustls client instead
//! ([`custom_http_client`], [`custom_connector`]). It honors `--resolve` like the default
//! client.
//!
//! Certificates in `certs/CAs` are trust anchors; one that is exactly the server's
//! certificate is also accepted when webpki rejects it as an end-entity certificate (a
//! self-signed `CA:TRUE` certificate, as saved by the `alias set` trust prompt), like Go.

use super::deadline::DeadlineConnector;
use crate::resolve::PinnedDnsResolver;
use anyhow::{Context, Result, anyhow};
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
use rustls::crypto::CryptoProvider;
use rustls::{CertificateError, DigitallySignedStruct, SignatureScheme};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

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

/// PEM files of the active `certs/CAs` dir.
fn configured_ca_certs() -> Result<Vec<Vec<u8>>> {
    match cas_dir() {
        Some(dir) => load_ca_certs(&dir),
        None => Ok(Vec::new()),
    }
}

/// HTTP client for the S3 SDK when the SDK default cannot be used (see the module docs);
/// None otherwise.
pub fn custom_http_client(resolver: PinnedDnsResolver) -> Result<Option<SharedHttpClient>> {
    Ok(custom_tls_config()?.map(|config| {
        SharedHttpClient::new(CustomClient {
            config,
            resolver,
            connector: OnceLock::new(),
        })
    }))
}

/// Like [`custom_http_client`], for callers without SDK runtime components (admin client).
pub fn custom_connector(resolver: PinnedDnsResolver) -> Result<Option<SharedHttpConnector>> {
    Ok(custom_tls_config()?.map(|config| build_connector(config, resolver, None)))
}

/// TLS config for the custom HTTP stack; None when the SDK default client suffices.
fn custom_tls_config() -> Result<Option<rustls::ClientConfig>> {
    if crate::globals::insecure() {
        return insecure_tls_config().map(Some);
    }
    let ca_certs = configured_ca_certs()?;
    if ca_certs.is_empty() && !super::deadline::get().is_set() {
        return Ok(None);
    }
    verified_tls_config(&ca_certs).map(Some)
}

fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// System roots plus the `certs/CAs` PEM files, with the pinned-certificate fallback.
fn verified_tls_config(ca_pems: &[Vec<u8>]) -> Result<rustls::ClientConfig> {
    let provider = crypto_provider();
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    let mut pinned = Vec::new();
    for pem in ca_pems {
        for cert in CertificateDer::pem_slice_iter(pem) {
            let cert = cert.context("Unable to parse CA certificate.")?;
            roots.add(cert.clone()).context("Invalid CA certificate.")?;
            pinned.push(cert);
        }
    }
    let inner = if roots.is_empty() {
        None
    } else {
        Some(
            rustls::client::WebPkiServerVerifier::builder_with_provider(
                Arc::new(roots),
                provider.clone(),
            )
            .build()
            .map_err(|err| anyhow!("Unable to configure TLS: {err}"))?,
        )
    };
    let verifier = PinnedVerifier {
        inner,
        pinned,
        provider: provider.clone(),
    };
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("Unable to configure TLS.")?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

fn insecure_tls_config() -> Result<rustls::ClientConfig> {
    let provider = crypto_provider();
    Ok(
        rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .context("Unable to configure TLS.")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth(),
    )
}

fn build_connector(
    config: rustls::ClientConfig,
    resolver: PinnedDnsResolver,
    connect_timeout: Option<Duration>,
) -> SharedHttpConnector {
    let mut http = HyperHttpConnector::new_with_resolver(Resolver(resolver));
    http.enforce_http(false);
    http.set_nodelay(true);
    http.set_connect_timeout(connect_timeout);
    let tcp = DeadlineConnector::new(http, super::deadline::get());
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(config)
        .https_or_http()
        .enable_http1()
        .wrap_connector(tcp);
    let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(https);
    SharedHttpConnector::new(CustomConnector { client })
}

fn verify_tls12(
    provider: &CryptoProvider,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls12_signature(
        message,
        cert,
        dss,
        &provider.signature_verification_algorithms,
    )
}

fn verify_tls13(
    provider: &CryptoProvider,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls13_signature(
        message,
        cert,
        dss,
        &provider.signature_verification_algorithms,
    )
}

/// Accepts any server certificate (`--insecure`); handshake signatures are still checked.
#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

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
        verify_tls12(&self.0, message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13(&self.0, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// webpki verification plus exact matches of pinned `certs/CAs` certificates. An unknown
/// self-signed `CA:TRUE` certificate is reported as an unknown issuer (Go's "certificate
/// signed by unknown authority") rather than webpki's "CA used as end entity".
#[derive(Debug)]
struct PinnedVerifier {
    inner: Option<Arc<rustls::client::WebPkiServerVerifier>>,
    pinned: Vec<CertificateDer<'static>>,
    provider: Arc<CryptoProvider>,
}

fn unknown_issuer() -> rustls::Error {
    rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer)
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let result = match &self.inner {
            Some(inner) => {
                inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            }
            None => Err(unknown_issuer()),
        };
        let Err(err) = result else {
            return result;
        };
        if self
            .pinned
            .iter()
            .any(|cert| cert.as_ref() == end_entity.as_ref())
        {
            let parsed = rustls::server::ParsedCertificate::try_from(end_entity)?;
            rustls::client::verify_server_name(&parsed, server_name)?;
            return Ok(ServerCertVerified::assertion());
        }
        match err {
            rustls::Error::InvalidCertificate(CertificateError::Other(other))
                if format!("{other:?}").contains("CaUsedAsEndEntity") =>
            {
                Err(unknown_issuer())
            }
            err => Err(err),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12(&self.provider, message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13(&self.provider, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Result of [`probe_peer`].
#[derive(Debug)]
pub enum PeerTrust {
    /// The server certificate verifies with the system roots and `certs/CAs` (or the URL is
    /// not `https`).
    Trusted,
    /// Verification failed with an unknown issuer: Go's error text and the server's leaf
    /// certificate.
    UnknownAuthority {
        error: String,
        cert: CertificateDer<'static>,
    },
}

/// mc `promptTrustSelfSignedCert` probe: a TLS handshake with the configured trust and, on an
/// unknown-issuer failure, an unverified handshake to fetch the leaf certificate. Other
/// failures are errors in Go's `Get "URL": ...` form.
pub fn probe_peer(url: &url::Url) -> Result<PeerTrust> {
    if url.scheme() != "https" {
        return Ok(PeerTrust::Trusted);
    }
    let target = url.as_str().trim_end_matches('/').to_string();
    let fail = |err: HandshakeError| {
        let text = match err {
            HandshakeError::Tls(err) => format!("tls: {err}"),
            HandshakeError::Io(err) => go_dial_text(url, &err),
        };
        anyhow::Error::new(crate::error::McError::new(format!(
            "Get \"{target}\": {text}"
        )))
    };
    match handshake(url, verified_tls_config(&configured_ca_certs()?)?) {
        Ok(_) => return Ok(PeerTrust::Trusted),
        Err(HandshakeError::Tls(rustls::Error::InvalidCertificate(
            CertificateError::UnknownIssuer,
        ))) => {}
        Err(err) => return Err(fail(err)),
    }
    let error = format!(
        "Get \"{target}\": tls: failed to verify certificate: x509: certificate signed by unknown authority"
    );
    match handshake(url, insecure_tls_config()?) {
        Ok(Some(cert)) => Ok(PeerTrust::UnknownAuthority { error, cert }),
        Ok(None) => Err(anyhow::Error::new(crate::error::McError::new(
            "Unable to read remote TLS certificate",
        ))),
        Err(err) => Err(fail(err)),
    }
}

#[derive(Debug)]
enum HandshakeError {
    Tls(rustls::Error),
    Io(std::io::Error),
}

/// Go `net` wording for dial errors (`dial tcp HOST:PORT: connect: connection refused`).
fn go_dial_text(url: &url::Url, err: &std::io::Error) -> String {
    let addr = format!(
        "{}:{}",
        url.host_str().unwrap_or_default(),
        url.port_or_known_default().unwrap_or(443)
    );
    match err.kind() {
        std::io::ErrorKind::ConnectionRefused => {
            format!("dial tcp {addr}: connect: connection refused")
        }
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            format!("dial tcp {addr}: i/o timeout")
        }
        _ => format!("dial tcp {addr}: {err}"),
    }
}

/// Blocking TLS handshake with `url`'s host (honoring `--resolve`); returns the server's
/// leaf certificate.
fn handshake(
    url: &url::Url,
    config: rustls::ClientConfig,
) -> Result<Option<CertificateDer<'static>>, HandshakeError> {
    use std::net::ToSocketAddrs;
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let pinned = crate::resolve::configured()
        .into_iter()
        .find(|m| m.host.eq_ignore_ascii_case(&host) && m.port == port);
    let addrs: Vec<SocketAddr> = match pinned {
        Some(mapping) => vec![SocketAddr::new(mapping.ip, port)],
        None => (host.as_str(), port)
            .to_socket_addrs()
            .map_err(HandshakeError::Io)?
            .collect(),
    };
    let timeout = Duration::from_secs(10);
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no such host");
    let mut socket = None;
    for addr in addrs {
        match std::net::TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                socket = Some(stream);
                break;
            }
            Err(err) => last = err,
        }
    }
    let mut socket = socket.ok_or(HandshakeError::Io(last))?;
    let _ = socket.set_read_timeout(Some(timeout));
    let _ = socket.set_write_timeout(Some(timeout));
    let server_name = ServerName::try_from(host)
        .map_err(|err| HandshakeError::Tls(rustls::Error::General(err.to_string())))?;
    let mut conn = rustls::ClientConnection::new(Arc::new(config), server_name)
        .map_err(HandshakeError::Tls)?;
    while conn.is_handshaking() {
        if let Err(err) = conn.complete_io(&mut socket) {
            let tls = err
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<rustls::Error>())
                .cloned();
            return Err(match tls {
                Some(tls) => HandshakeError::Tls(tls),
                None => HandshakeError::Io(err),
            });
        }
    }
    Ok(conn
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|cert| cert.clone().into_owned()))
}

#[derive(Debug)]
struct CustomClient {
    config: rustls::ClientConfig,
    resolver: PinnedDnsResolver,
    /// mx uses one set of connector settings per process, so one connector is enough.
    connector: OnceLock<SharedHttpConnector>,
}

impl HttpClient for CustomClient {
    fn http_connector(
        &self,
        settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        self.connector
            .get_or_init(|| {
                build_connector(
                    self.config.clone(),
                    self.resolver.clone(),
                    settings.connect_timeout(),
                )
            })
            .clone()
    }
}

type HyperClient = hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<DeadlineConnector<HyperHttpConnector<Resolver>>>,
    SdkBody,
>;

#[derive(Debug)]
struct CustomConnector {
    client: HyperClient,
}

impl HttpConnector for CustomConnector {
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
                    } else if is_timeout(&err) {
                        ConnectorError::timeout(err.into())
                    } else {
                        ConnectorError::other(err.into(), None)
                    }
                })?
                .map(SdkBody::from_body_1_x);
            HttpResponse::try_from(response).map_err(|err| ConnectorError::other(err.into(), None))
        })
    }
}

/// True when a deadline (`TimedOut` IO error) caused `err`.
fn is_timeout(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(err) = current {
        if err
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::TimedOut)
        {
            return true;
        }
        current = err.source();
    }
    false
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
    use super::*;

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

    #[test]
    fn pinned_certificates_still_check_the_server_name() {
        let cert = CertificateDer::from_pem_slice(CERT.as_bytes()).unwrap();
        let name = ServerName::try_from("mx-test-ca").unwrap();
        let verifier = PinnedVerifier {
            inner: None,
            pinned: vec![cert.clone()],
            provider: crypto_provider(),
        };
        // The fixture has no SAN for this name: pinning still checks the server name.
        let err = verifier
            .verify_server_cert(&cert, &[], &name, &[], UnixTime::now())
            .unwrap_err();
        assert!(matches!(err, rustls::Error::InvalidCertificate(_)));
        let unknown = PinnedVerifier {
            inner: None,
            pinned: Vec::new(),
            provider: crypto_provider(),
        };
        let err = unknown
            .verify_server_cert(&cert, &[], &name, &[], UnixTime::now())
            .unwrap_err();
        assert_eq!(err, unknown_issuer());
    }
}
