//! Reverse-proxy identity header authenticator.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

mod certificate;

use std::net::{IpAddr, SocketAddr};

use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, Provider, Schema, ToValue};
use coqui_access::{Attempt, Authenticator, Limit, Lookup, Verdict};
use coqui_access_kit::pick::{Pick, Picked, pick};
use coqui_net_acl::IpNet;

pub use certificate::CertificateEncoding;

/// The most bytes a certificate header may hold unless configured otherwise.
pub const DEFAULT_MAX_CERTIFICATE_BYTES: Limit = Limit::at(16_384);

/// The account a certificate or subject names by default: its subject's common name, by
/// `name`.
pub fn default_account() -> Vec<Pick> {
    vec![Pick::new("subject.cn", "name")]
}

/// Reverse-proxy trust and identity-header configuration.
#[derive(Debug, Clone, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao)]
#[non_exhaustive]
pub struct ProxyWordConfig {
    /// Trusted addresses and CIDR networks; an empty list trusts nobody.
    #[map(default)]
    pub trusted: Vec<String>,
    /// Header written by the proxy with an account name.
    pub user_header: Option<String>,
    /// Header holding the client certificate the proxy verified.
    pub certificate_header: Option<String>,
    /// How `certificate_header` is written: `pem-url` (nginx, AWS ALB), `der-base64`
    /// (Traefik) or `xfcc` (Envoy).
    #[map(default)]
    pub certificate_encoding: CertificateEncoding,
    /// Header holding a client certificate's subject, such as `CN=Doe,O=Ops`.
    pub subject_header: Option<String>,
    /// How a certificate or subject names the account: picks tried in order over its fields
    /// (`subject.cn`, `san.email`, `san.upn`, ...). Default its subject's CN, by `name`.
    #[map(default = default_account())]
    pub account: Vec<Pick>,
    /// The most bytes `certificate_header` may hold; `null` for no bound.
    #[map(default = DEFAULT_MAX_CERTIFICATE_BYTES)]
    pub max_certificate_bytes: Limit,
}

impl Default for ProxyWordConfig {
    /// What an empty configuration reads as: nobody trusted, every other default.
    fn default() -> Self {
        Self::from_value(&coqui_access::guatiao::Map::new().into())
            .expect("an empty configuration reads, every key having a default")
    }
}

/// A reverse proxy's identity headers as an Authenticator.
#[derive(Debug, Provider)]
#[provider(
    Authenticator,
    id = "coqui_access_proxy",
    name = "Trusted reverse proxy",
    config = ProxyWordConfig,
    crate = ::coqui_access::guatiao
)]
pub struct ProxyWord {
    trusted: Vec<IpNet>,
    user_header: Option<String>,
    certificate_header: Option<String>,
    certificate_encoding: CertificateEncoding,
    subject_header: Option<String>,
    account: Vec<Pick>,
    max_certificate_bytes: Limit,
}

impl ProxyWord {
    /// Parses a reverse-proxy policy. Invalid network entries are returned verbatim.
    pub fn new(config: ProxyWordConfig) -> Result<Self, String> {
        let trusted = config
            .trusted
            .iter()
            .map(|network| match network.parse::<IpNet>() {
                // Trusting every address is not a reverse proxy.
                Ok(IpNet::Any) | Err(_) => Err(network.clone()),
                Ok(network) => Ok(network),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let header = |value: Option<String>| {
            value
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty())
        };
        let subject_header = header(config.subject_header);
        let certificate_header = header(config.certificate_header);
        let user_header = header(config.user_header).or_else(|| {
            (subject_header.is_none() && certificate_header.is_none())
                .then(|| "x-remote-user".to_owned())
        });
        Ok(Self {
            trusted,
            user_header,
            certificate_header,
            certificate_encoding: config.certificate_encoding,
            subject_header,
            account: config.account,
            max_certificate_bytes: config.max_certificate_bytes,
        })
    }
}

impl TryFrom<ProxyWordConfig> for ProxyWord {
    type Error = ProviderError;

    fn try_from(config: ProxyWordConfig) -> Result<Self, Self::Error> {
        Self::new(config).map_err(|_| {
            coqui_access::error::bad_value("trusted entries must be IP addresses or CIDR networks")
        })
    }
}

impl Authenticator for ProxyWord {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let peer = attempt.peer.as_deref().and_then(parse_peer);
        if !peer.is_some_and(|peer| self.trusted.iter().any(|network| network.contains(peer))) {
            return Ok(Verdict::Unknown);
        }
        let header = |name: &str| -> Result<Option<&str>, ()> {
            let values: Vec<&str> = attempt
                .headers
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
                .collect();
            match values.as_slice() {
                [] => Ok(None),
                [one] => Ok((!one.is_empty()).then_some(one)),
                _ => Err(()),
            }
        };
        let user = match self.user_header.as_deref().map(header).transpose() {
            Ok(user) => user.flatten(),
            Err(()) => return Ok(Verdict::Invalid),
        };
        if let Some(name) = user {
            return Ok(Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: name.to_owned(),
            }));
        }
        if let Some(name) = self.certificate_header.as_deref() {
            let fields = match header(name) {
                Err(()) => return Ok(Verdict::Invalid),
                Ok(None) => None,
                Ok(Some(value)) => {
                    if !self.max_certificate_bytes.allows(value.len()) {
                        return Ok(Verdict::Invalid);
                    }
                    let Some(fields) = certificate::decode(value, self.certificate_encoding)
                        .and_then(|chain| certificate::fields(&chain))
                    else {
                        return Ok(Verdict::Invalid);
                    };
                    Some(fields)
                }
            };
            if let Some(verdict) = fields.and_then(|fields| self.picked(&fields)) {
                return Ok(verdict);
            }
        }
        if let Some(name) = self.subject_header.as_deref() {
            match header(name) {
                Err(()) => return Ok(Verdict::Invalid),
                Ok(None) => {}
                Ok(Some(subject)) => {
                    if let Some(verdict) = self.picked(&certificate::subject_fields(subject)) {
                        return Ok(verdict);
                    }
                }
            }
        }
        Ok(Verdict::Unknown)
    }
}

impl ProxyWord {
    /// The verdict the account picks give over `fields`; `None` when they find nothing.
    fn picked(&self, fields: &std::collections::BTreeMap<String, Vec<String>>) -> Option<Verdict> {
        match pick(fields, &self.account) {
            Picked::Found { by, value } => Some(Verdict::Valid(Lookup { by, value })),
            Picked::Ambiguous => Some(Verdict::Invalid),
            _ => None,
        }
    }
}

/// Finds the first non-empty, case-insensitive subject field.
pub fn field_of(subject: &str, field: &str) -> Option<String> {
    subject_parts(subject).into_iter().find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name.trim().eq_ignore_ascii_case(field) && !value.trim().is_empty())
            .then(|| value.trim().to_owned())
    })
}

/// A subject's `name=value` parts: comma-separated with `\` escaping, or slash-separated
/// when it starts with `/`.
fn subject_parts(subject: &str) -> Vec<String> {
    let slash_separated = subject.starts_with('/');
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut chars = subject.chars();
    while let Some(character) = chars.next() {
        if !slash_separated && character == '\\' {
            part.extend(chars.next());
        } else if character == if slash_separated { '/' } else { ',' } {
            parts.push(std::mem::take(&mut part));
        } else {
            part.push(character);
        }
    }
    parts.push(part);
    parts
}

fn parse_peer(peer: &str) -> Option<IpAddr> {
    peer.parse::<IpAddr>()
        .ok()
        .or_else(|| peer.parse::<SocketAddr>().ok().map(|socket| socket.ip()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(peer: Option<&str>, headers: &[(&str, &str)]) -> Attempt {
        Attempt {
            name: None,
            secret: None,
            headers: headers
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            peer: peer.map(str::to_owned),
            secure: Some(true),
            certificate: None,
            certificates: None,
        }
    }

    fn proxy() -> ProxyWord {
        ProxyWord::new(ProxyWordConfig {
            trusted: vec!["10.0.0.0/24".to_owned(), "::1".to_owned()],
            user_header: None,
            subject_header: None,
            ..ProxyWordConfig::default()
        })
        .unwrap()
    }

    #[test]
    fn trusts_only_configured_peers_and_returns_a_directory_lookup() {
        let proxy = proxy();
        assert_eq!(
            proxy
                .authenticate(attempt(
                    Some("10.0.0.9:8080"),
                    &[("X-Remote-User", "agent")]
                ))
                .unwrap(),
            Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: "agent".to_owned()
            })
        );
        assert_eq!(
            proxy
                .authenticate(attempt(Some("192.0.2.8"), &[("x-remote-user", "attacker")]))
                .unwrap(),
            Verdict::Unknown
        );
        assert_eq!(
            proxy
                .authenticate(attempt(None, &[("x-remote-user", "attacker")]))
                .unwrap(),
            Verdict::Unknown
        );
    }

    #[test]
    fn rejects_ambiguous_identity_headers_from_a_trusted_proxy() {
        let proxy = proxy();
        assert_eq!(
            proxy
                .authenticate(attempt(
                    Some("10.0.0.3"),
                    &[("x-remote-user", "first"), ("X-Remote-User", "second")],
                ))
                .unwrap(),
            Verdict::Invalid
        );
    }

    #[test]
    fn reads_subjects_only_from_the_configured_peer() {
        let proxy = ProxyWord::new(ProxyWordConfig {
            trusted: vec!["127.0.0.1".to_owned()],
            user_header: None,
            subject_header: Some("x-client-subject".to_owned()),
            ..ProxyWordConfig::default()
        })
        .unwrap();
        assert_eq!(
            proxy
                .authenticate(attempt(
                    Some("127.0.0.1"),
                    &[("x-client-subject", r"O=Ops,CN=Doe\, Jane")]
                ))
                .unwrap(),
            Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: "Doe, Jane".to_owned()
            })
        );
        assert_eq!(
            proxy
                .authenticate(attempt(
                    Some("127.0.0.2"),
                    &[("x-client-subject", "CN=Doe")]
                ))
                .unwrap(),
            Verdict::Unknown
        );
    }

    #[test]
    fn rejects_invalid_networks_and_reads_slash_subjects() {
        let trusting = |entries: &[&str]| {
            ProxyWord::try_from(ProxyWordConfig {
                trusted: entries.iter().map(|entry| (*entry).to_owned()).collect(),
                user_header: None,
                subject_header: None,
                ..ProxyWordConfig::default()
            })
        };
        assert!(trusting(&["10.0.0.0/33"]).is_err());
        assert!(
            trusting(&["any"]).is_err(),
            "trusting every address is not a proxy"
        );
        assert!(trusting(&["10.0.0.0/8", "ANY"]).is_err());
        assert_eq!(field_of("/O=Ops/CN=Doe", "cn"), Some("Doe".to_owned()));
        assert_eq!(field_of("CN=,O=Ops", "CN"), None);
    }

    #[test]
    fn an_ipv4_mapped_network_trusts_the_addresses_it_maps() {
        let proxy = ProxyWord::new(ProxyWordConfig {
            trusted: vec!["::ffff:10.0.0.0/120".to_owned()],
            user_header: None,
            subject_header: None,
            ..ProxyWordConfig::default()
        })
        .unwrap();
        let from = |peer: &str| {
            proxy
                .authenticate(attempt(Some(peer), &[("x-remote-user", "alice")]))
                .unwrap()
        };
        assert!(matches!(from("10.0.0.9:443"), Verdict::Valid(_)));
        assert!(matches!(from("[::ffff:10.0.0.9]:443"), Verdict::Valid(_)));
        assert_eq!(from("10.0.1.9:443"), Verdict::Unknown);
    }
}
