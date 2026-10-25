//! Reverse-proxy identity header authenticator.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

ai6::guatiao::providers!(ProxyWord);

use std::net::{IpAddr, SocketAddr};

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use ai6::{Attempt, Authenticator, Lookup, Verdict};

/// Reverse-proxy trust and identity-header configuration.
#[derive(Debug, Clone, FromValue, Schema, ToValue)]
#[map(crate = ::ai6::guatiao)]
#[non_exhaustive]
pub struct ProxyWordConfig {
    /// Trusted addresses and CIDR networks; an empty list trusts nobody.
    pub trusted: Vec<String>,
    /// Header written by the proxy with an account name.
    pub user_header: Option<String>,
    /// Optional header containing a client-certificate subject.
    pub subject_header: Option<String>,
    /// The subject field used as the account name. Defaults to `CN` when empty.
    pub subject_field: String,
}

#[derive(Debug, Clone, Copy)]
enum Network {
    V4(u32, u8),
    V6(u128, u8),
}

impl Network {
    fn parse(value: &str) -> Result<Self, String> {
        let (address, prefix) = match value.split_once('/') {
            Some((address, prefix)) => {
                let prefix = prefix.parse::<u8>().map_err(|_| value.to_owned())?;
                (address, Some(prefix))
            }
            None => (value, None),
        };
        let address = address.parse::<IpAddr>().map_err(|_| value.to_owned())?;
        match address {
            IpAddr::V4(address) => {
                let prefix = prefix.unwrap_or(32);
                if prefix > 32 {
                    return Err(value.to_owned());
                }
                Ok(Self::V4(u32::from(address), prefix))
            }
            IpAddr::V6(address) => {
                let prefix = prefix.unwrap_or(128);
                if prefix > 128 {
                    return Err(value.to_owned());
                }
                Ok(Self::V6(u128::from(address), prefix))
            }
        }
    }

    fn contains(self, address: IpAddr) -> bool {
        let address = match address {
            IpAddr::V6(address) => address
                .to_ipv4_mapped()
                .map(IpAddr::V4)
                .unwrap_or(IpAddr::V6(address)),
            address => address,
        };
        match (self, address) {
            (Self::V4(network, prefix), IpAddr::V4(address)) => {
                let mask = u32::MAX.checked_shl(u32::from(32 - prefix)).unwrap_or(0);
                (network & mask) == (u32::from(address) & mask)
            }
            (Self::V6(network, prefix), IpAddr::V6(address)) => {
                let mask = u128::MAX.checked_shl(u32::from(128 - prefix)).unwrap_or(0);
                (network & mask) == (u128::from(address) & mask)
            }
            _ => false,
        }
    }
}

/// A reverse proxy's identity headers as an AI6 Authenticator.
#[derive(Debug, Provider)]
#[provider(
    Authenticator,
    id = "ai6_auth_proxy",
    name = "Trusted reverse proxy",
    config = ProxyWordConfig,
    crate = ::ai6::guatiao
)]
pub struct ProxyWord {
    trusted: Vec<Network>,
    user_header: Option<String>,
    subject_header: Option<String>,
    subject_field: String,
}

impl ProxyWord {
    /// Parses a reverse-proxy policy. Invalid network entries are returned verbatim.
    pub fn new(config: ProxyWordConfig) -> Result<Self, String> {
        let trusted = config
            .trusted
            .iter()
            .map(|network| Network::parse(network))
            .collect::<Result<Vec<_>, _>>()?;
        let header = |value: Option<String>| {
            value
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty())
        };
        let subject_header = header(config.subject_header);
        let user_header = header(config.user_header)
            .or_else(|| subject_header.is_none().then(|| "x-remote-user".to_owned()));
        Ok(Self {
            trusted,
            user_header,
            subject_header,
            subject_field: if config.subject_field.trim().is_empty() {
                "CN".to_owned()
            } else {
                config.subject_field.trim().to_owned()
            },
        })
    }
}

impl TryFrom<ProxyWordConfig> for ProxyWord {
    type Error = ProviderError;

    fn try_from(config: ProxyWordConfig) -> Result<Self, Self::Error> {
        Self::new(config)
            .map_err(|_| provider_error("trusted entries must be IP addresses or CIDR networks"))
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
            Ok(user) => user.flatten().map(str::to_owned),
            Err(()) => return Ok(Verdict::Invalid),
        };
        let subject_name = match (user.is_none(), self.subject_header.as_deref()) {
            (true, Some(subject_header)) => match header(subject_header) {
                Ok(Some(subject)) => field_of(subject, &self.subject_field),
                Ok(None) => None,
                Err(()) => return Ok(Verdict::Invalid),
            },
            _ => None,
        };
        let name = user.or(subject_name);
        Ok(name.map_or(Verdict::Unknown, |name| {
            Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: name,
            })
        }))
    }
}

/// Finds the first non-empty, case-insensitive subject field.
pub fn field_of(subject: &str, field: &str) -> Option<String> {
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
    parts.into_iter().find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name.trim().eq_ignore_ascii_case(field) && !value.trim().is_empty())
            .then(|| value.trim().to_owned())
    })
}

fn parse_peer(peer: &str) -> Option<IpAddr> {
    peer.parse::<IpAddr>()
        .ok()
        .or_else(|| peer.parse::<SocketAddr>().ok().map(|socket| socket.ip()))
}

fn provider_error(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn attempt(peer: Option<&str>, headers: &[(&str, &str)]) -> Attempt {
        Attempt {
            name: None,
            secret: None,
            headers: headers
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>(),
            peer: peer.map(str::to_owned),
            secure: Some(true),
            certificate: None,
        }
    }

    fn proxy() -> ProxyWord {
        ProxyWord::new(ProxyWordConfig {
            trusted: vec!["10.0.0.0/24".to_owned(), "::1".to_owned()],
            user_header: None,
            subject_header: None,
            subject_field: "CN".to_owned(),
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
            subject_field: "CN".to_owned(),
        })
        .unwrap();
        assert_eq!(
            proxy
                .authenticate(attempt(
                    Some("127.0.0.1"),
                    &[("x-client-subject", r"O=Ops,CN=Bond\, James")]
                ))
                .unwrap(),
            Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: "Bond, James".to_owned()
            })
        );
        assert_eq!(
            proxy
                .authenticate(attempt(
                    Some("127.0.0.2"),
                    &[("x-client-subject", "CN=Bond")]
                ))
                .unwrap(),
            Verdict::Unknown
        );
    }

    #[test]
    fn rejects_invalid_networks_and_reads_slash_subjects() {
        assert!(Network::parse("10.0.0.0/33").is_err());
        assert_eq!(field_of("/O=Ops/CN=Bond", "cn"), Some("Bond".to_owned()));
        assert_eq!(field_of("CN=,O=Ops", "CN"), None);
    }
}
