//! Minimal DER reader for the certificate fields mc's TLS trust prompt looks at (`tofu.go`):
//! subject/authority key ids, the CA flag and the public key.

use anyhow::{Result, anyhow, bail};

/// Fields of an X.509 certificate used to decide whether it is self-signed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CertInfo {
    /// DER `SubjectPublicKeyInfo` (Go `RawSubjectPublicKeyInfo`).
    pub spki: Vec<u8>,
    /// `subjectPublicKey` bit string contents (Go `marshalPublicKey` for RSA/ECDSA/Ed25519).
    pub public_key: Vec<u8>,
    pub subject_key_id: Vec<u8>,
    pub authority_key_id: Vec<u8>,
    /// Basic constraints `cA`.
    pub is_ca: bool,
}

impl CertInfo {
    /// mc `promptTrustSelfSignedCert`: a CA without an authority key id must have the
    /// RFC 5280 method-1 subject key id (SHA-1 of the public key); otherwise the subject and
    /// authority key ids must match.
    pub fn is_self_signed(&self) -> bool {
        if self.is_ca && self.authority_key_id.is_empty() {
            let digest = aws_lc_rs::digest::digest(
                &aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY,
                &self.public_key,
            );
            return digest.as_ref() == self.subject_key_id.as_slice();
        }
        self.subject_key_id == self.authority_key_id
    }

    /// Hex SHA-256 of the `SubjectPublicKeyInfo` (the fingerprint mc asks to confirm).
    pub fn fingerprint(&self) -> String {
        aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &self.spki)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// One DER element: tag, full encoding and contents.
struct Tlv<'a> {
    tag: u8,
    raw: &'a [u8],
    value: &'a [u8],
}

/// Reads the next element of `input`, returning it and the rest.
fn next(input: &[u8]) -> Result<(Tlv<'_>, &[u8])> {
    let bad = || anyhow!("malformed certificate");
    let tag = *input.first().ok_or_else(bad)?;
    let first = *input.get(1).ok_or_else(bad)?;
    let (len, header) = if first < 0x80 {
        (usize::from(first), 2)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 {
            bail!("malformed certificate");
        }
        let bytes = input.get(2..2 + count).ok_or_else(bad)?;
        let len = bytes
            .iter()
            .fold(0usize, |acc, b| (acc << 8) | usize::from(*b));
        (len, 2 + count)
    };
    let end = header.checked_add(len).ok_or_else(bad)?;
    let raw = input.get(..end).ok_or_else(bad)?;
    Ok((
        Tlv {
            tag,
            raw,
            value: &raw[header..],
        },
        &input[end..],
    ))
}

/// All elements of a constructed value.
fn children(mut input: &[u8]) -> Result<Vec<Tlv<'_>>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        let (tlv, rest) = next(input)?;
        out.push(tlv);
        input = rest;
    }
    Ok(out)
}

const OID_SUBJECT_KEY_ID: &[u8] = &[0x55, 0x1d, 0x0e];
const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
const OID_AUTHORITY_KEY_ID: &[u8] = &[0x55, 0x1d, 0x23];

/// Parses the fields of [`CertInfo`] from a DER certificate.
pub fn parse(der: &[u8]) -> Result<CertInfo> {
    let (cert, _) = next(der)?;
    let (tbs, _) = next(cert.value)?;
    let fields = children(tbs.value)?;
    // Optional explicit version [0], then serial, signature, issuer, validity, subject, SPKI.
    let first = usize::from(fields.first().is_some_and(|f| f.tag == 0xa0));
    let spki = fields
        .get(first + 5)
        .ok_or_else(|| anyhow!("certificate has no public key"))?;
    let mut info = CertInfo {
        spki: spki.raw.to_vec(),
        ..Default::default()
    };
    if let Some(bits) = children(spki.value)?.get(1) {
        info.public_key = bits.value.get(1..).unwrap_or_default().to_vec();
    }
    for field in fields.iter().filter(|f| f.tag == 0xa3) {
        let (extensions, _) = next(field.value)?;
        for extension in children(extensions.value)? {
            let parts = children(extension.value)?;
            let (Some(oid), Some(value)) = (parts.first(), parts.last()) else {
                continue;
            };
            let (inner, _) = next(value.value)?;
            match oid.value {
                OID_SUBJECT_KEY_ID => info.subject_key_id = inner.value.to_vec(),
                OID_AUTHORITY_KEY_ID => {
                    if let Some(key_id) = children(inner.value)?.iter().find(|f| f.tag == 0x80) {
                        info.authority_key_id = key_id.value.to_vec();
                    }
                }
                OID_BASIC_CONSTRAINTS => {
                    info.is_ca = children(inner.value)?
                        .first()
                        .is_some_and(|f| f.tag == 0x01 && f.value.first().is_some_and(|b| *b != 0));
                }
                _ => {}
            }
        }
    }
    Ok(info)
}

/// PEM `CERTIFICATE` block with 64-column lines (Go `pem.EncodeToMemory`).
pub fn to_pem(der: &[u8]) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push('\n');
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls_pki_types::CertificateDer;
    use rustls_pki_types::pem::PemObject;

    fn der(pem: &str) -> Vec<u8> {
        CertificateDer::from_pem_slice(pem.as_bytes())
            .unwrap()
            .to_vec()
    }

    #[test]
    fn detects_self_signed_ca_certificate() {
        let der = der(include_str!("../../tests/fixtures/test-ca.pem"));
        let info = parse(&der).unwrap();
        assert!(info.is_ca);
        assert!(!info.subject_key_id.is_empty());
        assert!(info.is_self_signed());
        // `openssl x509 -pubkey -noout | openssl pkey -pubin -outform der | sha256sum`
        assert_eq!(
            info.fingerprint(),
            "28b5a8fd19596215c2b5c4d6e7735643457fd517cc8a990c86222e1e3c2c90fa"
        );
        // openssl derives the subject key id with RFC 5280 method 1, so the CA check
        // without an authority key id holds too.
        let mut without_aki = info.clone();
        without_aki.authority_key_id.clear();
        assert!(without_aki.is_self_signed());
        let pem = to_pem(&der);
        assert!(pem.lines().all(|line| line.len() <= 64));
        assert_eq!(
            CertificateDer::from_pem_slice(pem.as_bytes())
                .unwrap()
                .to_vec(),
            der
        );
    }

    #[test]
    fn issued_certificates_are_not_self_signed() {
        let info = CertInfo {
            subject_key_id: vec![1],
            authority_key_id: vec![2],
            ..Default::default()
        };
        assert!(!info.is_self_signed());
        let ca_without_aki = CertInfo {
            is_ca: true,
            public_key: vec![1, 2, 3],
            subject_key_id: vec![9],
            ..Default::default()
        };
        assert!(!ca_without_aki.is_self_signed());
        assert!(parse(b"\x30\x05\x01").is_err());
    }
}
