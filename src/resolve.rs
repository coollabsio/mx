use anyhow::{Result, bail};
use aws_smithy_runtime_api::client::dns::{DnsFuture, ResolveDns, ResolveDnsError};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveMapping {
    pub host: String,
    pub port: u16,
    pub ip: IpAddr,
}

impl FromStr for ResolveMapping {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let (address, ip) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("resolve value must use HOST:PORT=IP"))?;
        let (host, port) = address
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("resolve value must include a port"))?;
        let host = host.trim_matches(['[', ']']);
        if host.is_empty() {
            bail!("resolve host cannot be empty");
        }
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port: port.parse()?,
            ip: ip.parse()?,
        })
    }
}

static MAPPINGS: OnceLock<RwLock<Vec<ResolveMapping>>> = OnceLock::new();

pub fn configure(mappings: Vec<ResolveMapping>) {
    *MAPPINGS
        .get_or_init(|| RwLock::new(Vec::new()))
        .write()
        .expect("resolve configuration lock poisoned") = mappings;
}

pub fn configured() -> Vec<ResolveMapping> {
    MAPPINGS
        .get_or_init(|| RwLock::new(Vec::new()))
        .read()
        .expect("resolve configuration lock poisoned")
        .clone()
}

#[derive(Debug, Clone)]
pub struct PinnedDnsResolver {
    mappings: BTreeMap<String, IpAddr>,
}

impl PinnedDnsResolver {
    pub fn new(mappings: &[ResolveMapping]) -> Result<Self> {
        let mut by_host = BTreeMap::new();
        for mapping in mappings {
            if let Some(previous) = by_host.insert(mapping.host.clone(), mapping.ip)
                && previous != mapping.ip
            {
                bail!("conflicting resolve values for host `{}`", mapping.host);
            }
        }
        Ok(Self { mappings: by_host })
    }
}

impl ResolveDns for PinnedDnsResolver {
    fn resolve_dns<'a>(&'a self, name: &'a str) -> DnsFuture<'a> {
        if let Some(ip) = self.mappings.get(&name.to_ascii_lowercase()) {
            return DnsFuture::ready(Ok(vec![*ip]));
        }
        DnsFuture::new(async move {
            let addresses = tokio::net::lookup_host((name, 0))
                .await
                .map_err(ResolveDnsError::new)?;
            Ok(addresses.map(|address| address.ip()).collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{PinnedDnsResolver, ResolveMapping};
    use aws_smithy_runtime_api::client::dns::ResolveDns;

    #[test]
    fn parses_coolify_resolve_value() {
        let value: ResolveMapping = "s3.internal:9000=127.0.0.1".parse().unwrap();
        assert_eq!(value.host, "s3.internal");
        assert_eq!(value.port, 9000);
        assert_eq!(value.ip.to_string(), "127.0.0.1");
    }

    #[test]
    fn rejects_resolve_without_port() {
        assert!("s3.internal=127.0.0.1".parse::<ResolveMapping>().is_err());
    }

    #[test]
    fn pinned_resolver_returns_configured_ip() {
        let mapping = "s3.internal:9000=127.0.0.2".parse().unwrap();
        let resolver = PinnedDnsResolver::new(&[mapping]).unwrap();
        let addresses = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(resolver.resolve_dns("s3.internal"))
            .unwrap();
        assert_eq!(
            addresses,
            vec!["127.0.0.2".parse::<std::net::IpAddr>().unwrap()]
        );
    }

    #[test]
    fn rejects_conflicting_mappings_for_one_host() {
        let first = "s3.internal:9000=127.0.0.1".parse().unwrap();
        let second = "s3.internal:9000=127.0.0.2".parse().unwrap();
        assert!(PinnedDnsResolver::new(&[first, second]).is_err());
    }
}
