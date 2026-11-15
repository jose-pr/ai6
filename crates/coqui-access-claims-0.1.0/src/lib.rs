//! The claims a client certificate carries.
//!
//! Providers that read a certificate (the TLS one, and the one behind a reverse proxy that
//! forwards it) answer a lookup with these claims, and a matcher reads them, so both name
//! the same things the same way.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;

use coqui_access::Claims;
use coqui_access::guatiao::Bytes;
use coqui_peer_tls::CertificateFields;
use data_encoding::HEXLOWER;
use sha2::{Digest, Sha256};

/// The claims of a leaf certificate, or `None` when it does not parse.
///
/// The certificate's own named fields (`subject.cn`, `san.upn`, `san.email`, `san.dns`,
/// `san.uri`, `san.ip`, `fingerprint`, ...) with `subject`, `issuer` and `serial` named
/// `subject.dn`, `issuer.dn` and `serial.hex`, `serial.reversed` (the serial's bytes last
/// first, as a directory that spells them that way wants), `spki` (`sha256:<hex>` of the
/// public key) and the DER itself.
pub fn of_certificate(der: &[u8]) -> Option<Claims> {
    let fields = CertificateFields::parse(der).ok()?;
    let mut values: BTreeMap<String, Vec<String>> = fields.to_map();
    for (from, to) in [
        ("subject", "subject.dn"),
        ("issuer", "issuer.dn"),
        ("serial", "serial.hex"),
    ] {
        if let Some(found) = values.remove(from) {
            values.insert(to.to_owned(), found);
        }
    }
    if let Some(serial) = values.get("serial.hex").and_then(|found| found.first()) {
        values.insert("serial.reversed".to_owned(), vec![reversed(serial)]);
    }
    if let Some(key) = public_key_hash(der) {
        values.insert("spki".to_owned(), vec![key]);
    }
    Some(Claims {
        values,
        certificate: Some(Bytes::from(der.to_vec())),
    })
}

/// `sha256:<hex>` of a DER certificate's SubjectPublicKeyInfo, which a certificate renewed
/// on the same key keeps; `None` when the certificate does not parse.
pub fn public_key_hash(der: &[u8]) -> Option<String> {
    let (_, certificate) = x509_parser::parse_x509_certificate(der).ok()?;
    Some(public_key_info_hash(
        certificate.tbs_certificate.subject_pki.raw,
    ))
}

/// `sha256:<hex>` of a DER `SubjectPublicKeyInfo`.
pub fn public_key_info_hash(info: &[u8]) -> String {
    format!("sha256:{}", HEXLOWER.encode(&Sha256::digest(info)))
}

/// The bytes of a hex string in the opposite order: `0a1b2c` is `2c1b0a`. An odd length is
/// padded with a leading zero first, as the serial's own bytes would be.
fn reversed(hex: &str) -> String {
    let padded = if hex.len() % 2 == 1 {
        format!("0{hex}")
    } else {
        hex.to_owned()
    };
    padded
        .as_bytes()
        .chunks(2)
        .rev()
        .map(|pair| String::from_utf8_lossy(pair).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use rcgen::{CertificateParams, DnType, KeyPair, SanType};

    use super::*;

    fn certificate(key: &KeyPair, name: &str) -> Vec<u8> {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(DnType::CommonName, name);
        params.subject_alt_names = vec![SanType::Rfc822Name("one@example.org".try_into().unwrap())];
        params.self_signed(key).unwrap().der().to_vec()
    }

    #[test]
    fn a_certificate_carries_its_names_key_and_der() {
        let key = KeyPair::generate().unwrap();
        let der = certificate(&key, "Client One");
        let claims = of_certificate(&der).unwrap();
        assert_eq!(claims.get("san.email"), ["one@example.org".to_owned()]);
        assert_eq!(claims.get("subject.cn"), ["Client One".to_owned()]);
        assert_eq!(claims.get("spki"), [public_key_hash(&der).unwrap()]);
        assert!(!claims.get("subject.dn").is_empty() && !claims.get("issuer.dn").is_empty());
        assert!(!claims.get("serial.hex").is_empty() && !claims.get("fingerprint").is_empty());
        assert_eq!(
            claims.certificate.as_ref().map(|bytes| bytes.as_ref()),
            Some(der.as_slice())
        );
    }

    #[test]
    fn the_key_hash_follows_the_key_through_a_renewal() {
        let key = KeyPair::generate().unwrap();
        let (first, renewed) = (certificate(&key, "A"), certificate(&key, "B"));
        assert_ne!(first, renewed);
        assert_eq!(public_key_hash(&first), public_key_hash(&renewed));
        let other = certificate(&KeyPair::generate().unwrap(), "A");
        assert_ne!(public_key_hash(&first), public_key_hash(&other));
        assert_eq!(public_key_hash(b"not a certificate"), None);
        assert!(of_certificate(b"not a certificate").is_none());
    }

    #[test]
    fn a_reversed_serial_has_its_bytes_last_first() {
        assert_eq!(reversed("0a1b2c"), "2c1b0a");
        assert_eq!(reversed("a1b2c"), "2c1b0a");
    }
}
