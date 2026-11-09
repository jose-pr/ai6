//! A client certificate as a reverse proxy forwards it in a header.

use std::collections::BTreeMap;

use coqui_access::guatiao::{FromValue, Schema, ToValue};
use coqui_peer_tls::CertificateFields;
use data_encoding::BASE64;
use percent_encoding::percent_decode_str;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;

/// How the certificate header is written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, rename_all = "kebab-case")]
pub enum CertificateEncoding {
    /// URL-encoded PEM, the leaf first and any chain after it: nginx
    /// `$ssl_client_escaped_cert`, AWS ALB `X-Amzn-Mtls-Clientcert`.
    #[default]
    PemUrl,
    /// Base64 DER, certificates separated by commas: Traefik `X-Forwarded-Tls-Client-Cert`.
    DerBase64,
    /// Envoy `x-forwarded-client-cert`: the `Cert` of the last element, which the proxy
    /// next to this service wrote, as URL-encoded PEM.
    Xfcc,
}

/// The certificates in a header value, leaf first; `None` when it cannot be read.
pub(crate) fn decode(value: &str, encoding: CertificateEncoding) -> Option<Vec<Vec<u8>>> {
    let certificates = match encoding {
        CertificateEncoding::PemUrl => pem(&percent_decode_str(value).decode_utf8().ok()?)?,
        CertificateEncoding::DerBase64 => value
            .split(',')
            .map(|one| {
                let compact: String = one.chars().filter(|c| !c.is_whitespace()).collect();
                BASE64.decode(compact.as_bytes()).ok()
            })
            .collect::<Option<Vec<_>>>()?,
        CertificateEncoding::Xfcc => {
            let element = split_outside_quotes(value, ',').pop()?;
            let cert = split_outside_quotes(&element, ';')
                .into_iter()
                .find_map(|pair| {
                    let (key, value) = pair.split_once('=')?;
                    key.trim()
                        .eq_ignore_ascii_case("cert")
                        .then(|| unquote(value.trim()))
                })?;
            pem(&percent_decode_str(&cert).decode_utf8().ok()?)?
        }
    };
    (!certificates.is_empty()).then_some(certificates)
}

fn pem(text: &str) -> Option<Vec<Vec<u8>>> {
    CertificateDer::pem_slice_iter(text.as_bytes())
        .map(|certificate| certificate.ok().map(|der| der.to_vec()))
        .collect()
}

/// Splits on `separator` outside double quotes; a backslash escapes the next character
/// inside them.
fn split_outside_quotes(value: &str, separator: char) -> Vec<String> {
    let (mut parts, mut part, mut quoted, mut escaped) = (Vec::new(), String::new(), false, false);
    for character in value.chars() {
        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == separator && !quoted {
            parts.push(std::mem::take(&mut part));
            continue;
        }
        part.push(character);
    }
    parts.push(part);
    parts
}

fn unquote(value: &str) -> String {
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return value.to_owned();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(character) = chars.next() {
        if character == '\\' {
            out.extend(chars.next());
        } else {
            out.push(character);
        }
    }
    out
}

/// The leaf's named fields, as coqui-peer-tls's `CertificateFields::to_map` writes them.
pub(crate) fn fields(chain: &[Vec<u8>]) -> Option<BTreeMap<String, Vec<String>>> {
    CertificateFields::parse(chain.first()?)
        .ok()
        .map(|fields| fields.to_map())
}

/// A subject header's fields: `subject` (the whole text) and `subject.<attribute>` in lower
/// case (`emailAddress` as `subject.email`), for the same picks a certificate takes.
pub(crate) fn subject_fields(subject: &str) -> BTreeMap<String, Vec<String>> {
    let mut map = BTreeMap::new();
    map.insert("subject".to_owned(), vec![subject.to_owned()]);
    for part in crate::subject_parts(subject) {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        if value.is_empty() {
            continue;
        }
        let name = if name == "emailaddress" {
            "email".to_owned()
        } else {
            name
        };
        map.entry(format!("subject.{name}"))
            .or_insert_with(Vec::new)
            .push(value.to_owned());
    }
    map
}
