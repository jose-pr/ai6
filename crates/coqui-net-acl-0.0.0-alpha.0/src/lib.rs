#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

extern crate alloc;

use core::fmt;
use core::net::{IpAddr, SocketAddr};
use core::str::FromStr;

/// What a rule does to a connection it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
#[cfg_attr(
    feature = "describe",
    derive(coqui_builder::Leaf),
    map(rename_all = "lowercase")
)]
pub enum Action {
    /// Let the connection through.
    Permit,
    /// Close it.
    #[default]
    Deny,
}

impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Permit => "permit",
            Self::Deny => "deny",
        })
    }
}

/// A malformed network or port range. The message names what was expected and never quotes
/// the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(&'static str);

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl core::error::Error for ParseError {}

/// An IPv4 or IPv6 network, a single address, or `any`.
///
/// An IPv4-mapped IPv6 address (`::ffff:10.0.0.9`) is the IPv4 address it maps, on both
/// sides of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IpNet {
    /// Every address.
    #[default]
    Any,
    /// An IPv4 network: address bits and prefix length. A prefix above 32 matches as 32.
    V4(u32, u8),
    /// An IPv6 network: address bits and prefix length. A prefix above 128 matches as 128.
    V6(u128, u8),
}

fn normalize(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        address => address,
    }
}

impl IpNet {
    /// Whether `address` is inside this network.
    pub fn contains(&self, address: IpAddr) -> bool {
        match (*self, normalize(address)) {
            (Self::Any, _) => true,
            (Self::V4(network, prefix), IpAddr::V4(address)) => {
                let mask = u32::MAX
                    .checked_shl(u32::from(32u8.saturating_sub(prefix)))
                    .unwrap_or(0);
                (network & mask) == (u32::from(address) & mask)
            }
            (Self::V6(network, prefix), IpAddr::V6(address)) => {
                let mask = u128::MAX
                    .checked_shl(u32::from(128u8.saturating_sub(prefix)))
                    .unwrap_or(0);
                (network & mask) == (u128::from(address) & mask)
            }
            _ => false,
        }
    }
}

impl FromStr for IpNet {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("any") {
            return Ok(Self::Any);
        }
        let bad = || ParseError("not an address, a CIDR network or `any`");
        let (address, prefix) = match value.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix.parse::<u8>().map_err(|_| bad())?)),
            None => (value, None),
        };
        match normalize(address.parse::<IpAddr>().map_err(|_| bad())?) {
            IpAddr::V4(address) => {
                // A mapped IPv6 network keeps its prefix relative to the IPv6 form.
                let prefix = match (prefix, address.is_unspecified()) {
                    (Some(prefix), _) if value.contains(':') && prefix >= 96 => prefix - 96,
                    (Some(prefix), _) => prefix,
                    (None, _) => 32,
                };
                (prefix <= 32)
                    .then(|| Self::V4(u32::from(address), prefix))
                    .ok_or_else(bad)
            }
            IpAddr::V6(address) => {
                let prefix = prefix.unwrap_or(128);
                (prefix <= 128)
                    .then(|| Self::V6(u128::from(address), prefix))
                    .ok_or_else(bad)
            }
        }
    }
}

impl fmt::Display for IpNet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Any => formatter.write_str("any"),
            Self::V4(address, 32) => write!(formatter, "{}", core::net::Ipv4Addr::from(address)),
            Self::V4(address, prefix) => {
                write!(formatter, "{}/{prefix}", core::net::Ipv4Addr::from(address))
            }
            Self::V6(address, 128) => write!(formatter, "{}", core::net::Ipv6Addr::from(address)),
            Self::V6(address, prefix) => {
                write!(formatter, "{}/{prefix}", core::net::Ipv6Addr::from(address))
            }
        }
    }
}

/// A TCP or UDP port, or an inclusive range of them (`443`, `8000-8100`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortRange {
    /// First port.
    pub start: u16,
    /// Last port, inclusive.
    pub end: u16,
}

impl FromStr for PortRange {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bad = || ParseError("not a port or a port range");
        let (start, end) = match value.trim().split_once('-') {
            Some((start, end)) => (start.trim(), end.trim()),
            None => (value.trim(), value.trim()),
        };
        let (start, end): (u16, u16) = (
            start.parse().map_err(|_| bad())?,
            end.parse().map_err(|_| bad())?,
        );
        (start <= end)
            .then_some(Self { start, end })
            .ok_or_else(bad)
    }
}

impl fmt::Display for PortRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start == self.end {
            write!(formatter, "{}", self.start)
        } else {
            write!(formatter, "{}-{}", self.start, self.end)
        }
    }
}

/// One line of an ACL.
///
/// `source` is matched against the connecting address; `destination`, `port` and `fingerprint`
/// narrow the rule and are ignored when absent.
///
/// With the `describe` feature a rule is described field by field: `source`, `destination` and `port` are
/// their text forms, and every key but `action` is optional.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "describe",
    derive(coqui_builder::Describe),
    map(deny_unknown_fields)
)]
pub struct Rule {
    /// What to do on a match.
    pub action: Action,
    /// The connecting address.
    #[cfg_attr(feature = "describe", map(default, as_text))]
    pub source: IpNet,
    /// The local address connected to (a host with several, or a routed setup). `None` is
    /// any.
    #[cfg_attr(feature = "describe", map(as_text))]
    pub destination: Option<IpNet>,
    /// The local port.
    #[cfg_attr(feature = "describe", map(as_text))]
    pub port: Option<PortRange>,
    /// The peer's certificate fingerprint in canonical text (for example
    /// `sha256:<hex>`), compared case-insensitively. A rule with one is decided only after
    /// the handshake.
    pub fingerprint: Option<String>,
    /// A hint, returned in the decision, that a host should log a match.
    #[cfg_attr(feature = "describe", map(default))]
    pub log: bool,
}

impl Rule {
    /// A rule for `source` with no further conditions.
    pub fn new(action: Action, source: IpNet) -> Self {
        Self {
            action,
            source,
            destination: None,
            port: None,
            fingerprint: None,
            log: false,
        }
    }

    fn matches_addresses(&self, source: SocketAddr, local: SocketAddr) -> bool {
        self.source.contains(source.ip())
            && self.destination.is_none_or(|net| net.contains(local.ip()))
            && self
                .port
                .is_none_or(|ports| (ports.start..=ports.end).contains(&local.port()))
    }
}

/// The result of evaluating a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Outcome {
    /// Let it through.
    Permit,
    /// Close it.
    Deny,
    /// The first rule to match names a fingerprint: proceed to the handshake and call
    /// [`Acl::decide_peer`].
    Deferred,
}

/// Which rule decided, for audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AclDecision {
    /// What to do.
    pub outcome: Outcome,
    /// The index of the matching rule; `None` when the default applied.
    pub rule_index: Option<usize>,
    /// The matching rule's `log` hint (false for the default).
    pub log: bool,
}

impl AclDecision {
    /// Whether the connection may proceed (permitted, or deferred to the handshake).
    pub fn proceeds(&self) -> bool {
        self.outcome != Outcome::Deny
    }
}

/// Whether any rule could permit traffic to a local address, and which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindDecision {
    /// Whether a rule (or the default) could permit it.
    pub permitted: bool,
    /// The rule that decided; `None` when the default did.
    pub rule_index: Option<usize>,
}

/// An ordered rule list and the action when none matches.
///
/// With the `describe` feature a list is described in the shape of its serde text form: a
/// `default` action and the rules under the key `rule`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(
    feature = "describe",
    derive(coqui_builder::Describe),
    map(deny_unknown_fields)
)]
pub struct Acl {
    /// Rules, top to bottom.
    #[cfg_attr(feature = "describe", map(default, rename = "rule"))]
    pub rules: Vec<Rule>,
    /// The action when no rule matches. [`Action::Deny`] unless set.
    #[cfg_attr(feature = "describe", map(default))]
    pub default: Action,
}

impl Acl {
    /// Adds a rule at the end, before the default.
    pub fn push(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    /// Adds a rule at the top, so it is considered first.
    pub fn prepend(&mut self, rule: Rule) {
        self.rules.insert(0, rule);
    }

    /// Evaluates a connection from its addresses alone, before any byte is read.
    pub fn decide(&self, source: SocketAddr, local: SocketAddr) -> AclDecision {
        for (index, rule) in self.rules.iter().enumerate() {
            if !rule.matches_addresses(source, local) {
                continue;
            }
            let outcome = if rule.fingerprint.is_some() {
                Outcome::Deferred
            } else {
                match rule.action {
                    Action::Permit => Outcome::Permit,
                    Action::Deny => Outcome::Deny,
                }
            };
            return AclDecision {
                outcome,
                rule_index: Some(index),
                log: rule.log,
            };
        }
        self.by_default()
    }

    /// Finishes a decision after the handshake, with the verified peer fingerprint in
    /// canonical text. Never [`Outcome::Deferred`].
    ///
    /// A rule's fingerprint and `fingerprint` are compared as text, ignoring ASCII case: two
    /// spellings of one fingerprint (hex and base32, with and without a prefix) do not match.
    /// A host whose rules may use other spellings rewrites them with
    /// [`canonicalize_fingerprints`](Acl::canonicalize_fingerprints) when it builds the list.
    pub fn decide_peer(
        &self,
        source: SocketAddr,
        local: SocketAddr,
        fingerprint: &str,
    ) -> AclDecision {
        for (index, rule) in self.rules.iter().enumerate() {
            let named = rule
                .fingerprint
                .as_deref()
                .is_none_or(|own| own.eq_ignore_ascii_case(fingerprint));
            if named && rule.matches_addresses(source, local) {
                return AclDecision {
                    outcome: match rule.action {
                        Action::Permit => Outcome::Permit,
                        Action::Deny => Outcome::Deny,
                    },
                    rule_index: Some(index),
                    log: rule.log,
                };
            }
        }
        self.by_default()
    }

    /// Decides a peer that proves no identity (a plaintext connection, or TLS with no client
    /// certificate) from its addresses alone. A rule that names a fingerprint can never match
    /// such a peer and is passed over; the first other rule that matches decides, then the
    /// default. Never [`Outcome::Deferred`].
    pub fn decide_anonymous(&self, source: SocketAddr, local: SocketAddr) -> AclDecision {
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.fingerprint.is_none() && rule.matches_addresses(source, local) {
                return AclDecision {
                    outcome: match rule.action {
                        Action::Permit => Outcome::Permit,
                        Action::Deny => Outcome::Deny,
                    },
                    rule_index: Some(index),
                    log: rule.log,
                };
            }
        }
        self.by_default()
    }

    /// Rewrites every rule's fingerprint into the canonical text `canonical` returns, so
    /// [`decide_peer`](Acl::decide_peer) matches whatever spelling a rule was written in.
    ///
    /// `canonical` receives the rule's index and its fingerprint as written. This crate knows
    /// no fingerprint format; the host that verifies peers supplies it, for example by parsing
    /// the text into its peer id type and printing that.
    ///
    /// # Errors
    ///
    /// The first error `canonical` returns. The list is left unchanged then: every rule is
    /// rewritten, or none is.
    pub fn canonicalize_fingerprints<E>(
        &mut self,
        mut canonical: impl FnMut(usize, &str) -> Result<String, E>,
    ) -> Result<(), E> {
        let mut rewritten = Vec::new();
        for (index, rule) in self.rules.iter().enumerate() {
            if let Some(text) = &rule.fingerprint {
                rewritten.push((index, canonical(index, text)?));
            }
        }
        for (index, text) in rewritten {
            self.rules[index].fingerprint = Some(text);
        }
        Ok(())
    }

    fn by_default(&self) -> AclDecision {
        AclDecision {
            outcome: match self.default {
                Action::Permit => Outcome::Permit,
                Action::Deny => Outcome::Deny,
            },
            rule_index: None,
            log: false,
        }
    }

    /// Whether any rule could permit traffic to `local`, and which rule says so. Advisory:
    /// nothing enforces it.
    ///
    /// Rules are walked in order. A rule whose `destination` and `port` cover `local` is
    /// considered (a wildcard address is covered by every rule): a Permit permits it whatever its
    /// `source`, so a routed setup may listen on an address that clients on other networks
    /// reach; a Deny with source `any` and no fingerprint refuses it. A rule with a narrower
    /// source is passed over. With no rule the default decides.
    pub fn permits_bind(&self, local: SocketAddr) -> BindDecision {
        let wildcard = local.ip().is_unspecified();
        for (index, rule) in self.rules.iter().enumerate() {
            let here = wildcard
                || (rule.destination.is_none_or(|net| net.contains(local.ip()))
                    && rule
                        .port
                        .is_none_or(|ports| (ports.start..=ports.end).contains(&local.port())));
            if !here {
                continue;
            }
            if rule.action == Action::Permit {
                return BindDecision {
                    permitted: true,
                    rule_index: Some(index),
                };
            }
            if rule.source == IpNet::Any && rule.fingerprint.is_none() {
                return BindDecision {
                    permitted: false,
                    rule_index: Some(index),
                };
            }
        }
        BindDecision {
            permitted: self.default == Action::Permit,
            rule_index: None,
        }
    }
}

/// A router-style listing: numbered rules in steps of ten, then the default.
impl fmt::Display for Acl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, rule) in self.rules.iter().enumerate() {
            write!(
                formatter,
                "{} {} {}",
                (index + 1) * 10,
                rule.action,
                rule.source
            )?;
            if let Some(destination) = rule.destination {
                write!(formatter, " destination {destination}")?;
            }
            if let Some(port) = rule.port {
                write!(formatter, " port {port}")?;
            }
            if let Some(fingerprint) = &rule.fingerprint {
                write!(formatter, " fingerprint {fingerprint}")?;
            }
            if rule.log {
                formatter.write_str(" log")?;
            }
            formatter.write_str("\n")?;
        }
        writeln!(formatter, "default {}", self.default)
    }
}

#[cfg(feature = "serde")]
mod text {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{Acl, Action, IpNet, PortRange, Rule};

    macro_rules! as_text {
        ($type:ty) => {
            impl Serialize for $type {
                fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    serializer.collect_str(self)
                }
            }
            impl<'de> Deserialize<'de> for $type {
                fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                    String::deserialize(deserializer)?
                        .parse()
                        .map_err(D::Error::custom)
                }
            }
        };
    }
    as_text!(IpNet);
    as_text!(PortRange);

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RuleText {
        action: Action,
        #[serde(default)]
        source: IpNet,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination: Option<IpNet>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<PortRange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        log: bool,
    }

    impl Serialize for Rule {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            RuleText {
                action: self.action,
                source: self.source,
                destination: self.destination,
                port: self.port,
                fingerprint: self.fingerprint.clone(),
                log: self.log,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Rule {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let text = RuleText::deserialize(deserializer)?;
            Ok(Self {
                action: text.action,
                source: text.source,
                destination: text.destination,
                port: text.port,
                fingerprint: text.fingerprint,
                log: text.log,
            })
        }
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AclText {
        #[serde(default)]
        default: Action,
        #[serde(default, rename = "rule")]
        rules: Vec<Rule>,
    }

    impl Serialize for Acl {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            AclText {
                default: self.default,
                rules: self.rules.clone(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Acl {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let text = AclText::deserialize(deserializer)?;
            Ok(Self {
                rules: text.rules,
                default: text.default,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(source: &str, local: &str) -> (SocketAddr, SocketAddr) {
        (source.parse().unwrap(), local.parse().unwrap())
    }

    fn acl(rules: Vec<Rule>, default: Action) -> Acl {
        Acl { rules, default }
    }

    fn net(text: &str) -> IpNet {
        text.parse().unwrap()
    }

    #[test]
    fn order_matters_and_the_default_applies_last() {
        let (source, local) = at("10.6.1.1:5000", "10.6.0.2:443");
        let deny_first = acl(
            vec![
                Rule::new(Action::Deny, net("10.6.1.0/24")),
                Rule::new(Action::Permit, net("10.6.0.0/16")),
            ],
            Action::Deny,
        );
        let decision = deny_first.decide(source, local);
        assert_eq!(
            (decision.outcome, decision.rule_index),
            (Outcome::Deny, Some(0))
        );
        let permit_first = acl(
            vec![
                Rule::new(Action::Permit, net("10.6.0.0/16")),
                Rule::new(Action::Deny, net("10.6.1.0/24")),
            ],
            Action::Deny,
        );
        assert_eq!(permit_first.decide(source, local).outcome, Outcome::Permit);
        // Nothing matches: the default, with no rule index.
        let (outside, _) = at("192.0.2.1:1", "10.6.0.2:443");
        let decision = permit_first.decide(outside, local);
        assert_eq!(
            (decision.outcome, decision.rule_index),
            (Outcome::Deny, None)
        );
        assert_eq!(Acl::default().default, Action::Deny);
        assert_eq!(
            acl(vec![], Action::Permit).decide(outside, local).outcome,
            Outcome::Permit
        );
    }

    #[test]
    fn v4_v6_and_mapped_addresses_match() {
        let list = acl(
            vec![
                Rule::new(Action::Permit, net("10.0.0.0/24")),
                Rule::new(Action::Permit, net("fd00::/8")),
                Rule::new(Action::Permit, net("::1")),
            ],
            Action::Deny,
        );
        let local = "127.0.0.1:1".parse().unwrap();
        let permitted =
            |source: &str| list.decide(source.parse().unwrap(), local).outcome == Outcome::Permit;
        assert!(permitted("10.0.0.9:1"));
        assert!(!permitted("10.0.1.9:1"));
        assert!(permitted("[fd12::5]:1"));
        assert!(!permitted("[fe80::5]:1"));
        assert!(permitted("[::1]:1"));
        // An IPv4-mapped IPv6 peer matches its IPv4 rule.
        assert!(permitted("[::ffff:10.0.0.9]:1"));
        assert!(!permitted("[::ffff:10.0.1.9]:1"));
        // And a rule written in mapped form is the IPv4 rule.
        assert_eq!(
            net("::ffff:10.0.0.0/120"),
            IpNet::V4(u32::from_be_bytes([10, 0, 0, 0]), 24)
        );
        assert!("10.0.0.0/33".parse::<IpNet>().is_err());
        assert!("nonsense".parse::<IpNet>().is_err());
    }

    #[test]
    fn local_address_and_port_narrow_a_rule() {
        let mut rule = Rule::new(Action::Permit, IpNet::Any);
        rule.destination = Some(net("10.0.0.2"));
        rule.port = Some("8000-8100".parse().unwrap());
        let list = acl(vec![rule], Action::Deny);
        let ok = |local: &str| {
            list.decide("192.0.2.1:1".parse().unwrap(), local.parse().unwrap())
                .outcome
        };
        assert_eq!(ok("10.0.0.2:8050"), Outcome::Permit);
        assert_eq!(ok("10.0.0.2:9000"), Outcome::Deny);
        assert_eq!(ok("10.0.0.3:8050"), Outcome::Deny);
    }

    #[test]
    fn a_fingerprint_rule_is_decided_only_after_the_handshake() {
        let mut named = Rule::new(Action::Permit, net("10.0.0.0/8"));
        named.fingerprint = Some("sha256:AB".to_owned());
        named.log = true;
        let list = acl(
            vec![named, Rule::new(Action::Deny, IpNet::Any)],
            Action::Deny,
        );
        let (source, local) = at("10.1.1.1:1", "10.0.0.2:443");
        let early = list.decide(source, local);
        assert_eq!(early.outcome, Outcome::Deferred);
        assert!(early.proceeds() && early.log);
        let named = list.decide_peer(source, local, "sha256:ab");
        assert_eq!(
            (named.outcome, named.rule_index),
            (Outcome::Permit, Some(0))
        );
        // Another fingerprint falls through to the next rule.
        let other = list.decide_peer(source, local, "sha256:cd");
        assert_eq!((other.outcome, other.rule_index), (Outcome::Deny, Some(1)));
        // An address no rule names never reaches the handshake.
        let (outside, _) = at("192.0.2.1:1", "10.0.0.2:443");
        assert_eq!(list.decide(outside, local).outcome, Outcome::Deny);
        // A deny before the fingerprint rule decides early.
        let denied = acl(
            vec![Rule::new(Action::Deny, net("10.1.0.0/16")), {
                let mut rule = Rule::new(Action::Permit, IpNet::Any);
                rule.fingerprint = Some("sha256:ab".to_owned());
                rule
            }],
            Action::Deny,
        );
        assert_eq!(denied.decide(source, local).outcome, Outcome::Deny);
    }

    fn local(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn to(action: Action, source: &str, destination: Option<&str>) -> Rule {
        let mut rule = Rule::new(action, net(source));
        rule.destination = destination.map(net);
        rule
    }

    #[test]
    fn a_routed_permit_allows_the_bind_though_its_source_is_another_subnet() {
        let list = acl(
            vec![to(Action::Permit, "10.9.0.0/16", Some("10.6.20.5"))],
            Action::Deny,
        );
        let decision = list.permits_bind(local("10.6.20.5:7000"));
        assert!(decision.permitted);
        assert_eq!(decision.rule_index, Some(0));
        // Another local address is not named by any rule.
        assert!(!list.permits_bind(local("10.6.20.6:7000")).permitted);
        // And accept time honours the destination too.
        let from = local("10.9.1.1:5000");
        assert_eq!(
            list.decide(from, local("10.6.20.5:7000")).outcome,
            Outcome::Permit
        );
        assert_eq!(
            list.decide(from, local("10.6.20.6:7000")).outcome,
            Outcome::Deny
        );
    }

    #[test]
    fn a_destination_deny_ahead_of_a_broad_permit_refuses_that_address() {
        let list = acl(
            vec![
                to(Action::Deny, "any", Some("192.0.2.9")),
                to(Action::Permit, "10.6.0.0/16", None),
            ],
            Action::Deny,
        );
        let refused = list.permits_bind(local("192.0.2.9:443"));
        assert!(!refused.permitted);
        assert_eq!(refused.rule_index, Some(0));
        let allowed = list.permits_bind(local("192.0.2.10:443"));
        assert!(allowed.permitted);
        assert_eq!(allowed.rule_index, Some(1));
        // A narrower deny does not close the address to every source.
        let narrow = acl(
            vec![
                to(Action::Deny, "10.9.0.0/16", Some("192.0.2.9")),
                to(Action::Permit, "any", Some("192.0.2.9")),
            ],
            Action::Deny,
        );
        assert!(narrow.permits_bind(local("192.0.2.9:443")).permitted);
    }

    #[test]
    fn rules_with_any_destination_permit_specific_and_wildcard_addresses() {
        let list = acl(vec![to(Action::Permit, "10.6.0.0/16", None)], Action::Deny);
        assert!(list.permits_bind(local("203.0.113.5:443")).permitted);
        assert!(list.permits_bind(local("0.0.0.0:443")).permitted);
        assert!(list.permits_bind(local("[::]:443")).permitted);
        // A wildcard is covered by every rule, so a deny for everyone refuses it.
        let closed = acl(vec![to(Action::Deny, "any", None)], Action::Permit);
        assert!(!closed.permits_bind(local("0.0.0.0:443")).permitted);
        assert!(
            !acl(vec![], Action::Deny)
                .permits_bind(local("10.6.0.2:443"))
                .permitted
        );
        assert!(
            acl(vec![], Action::Permit)
                .permits_bind(local("10.6.0.2:443"))
                .permitted
        );
    }

    #[test]
    fn callers_can_add_rules_and_change_the_default() {
        let mut list = Acl::default();
        list.push(Rule::new(Action::Deny, net("10.1.0.0/16")));
        list.prepend(Rule::new(Action::Permit, net("10.1.1.1")));
        list.default = Action::Permit;
        assert_eq!(list.rules.len(), 2);
        let at = |source: &str| {
            list.decide(source.parse().unwrap(), local("10.0.0.2:443"))
                .outcome
        };
        assert_eq!(at("10.1.1.1:1"), Outcome::Permit);
        assert_eq!(at("10.1.2.2:1"), Outcome::Deny);
        assert_eq!(at("192.0.2.1:1"), Outcome::Permit);
    }

    #[test]
    fn the_listing_reads_like_a_router() {
        let mut named = Rule::new(Action::Permit, net("10.6.0.0/16"));
        named.port = Some("443".parse().unwrap());
        named.log = true;
        let list = acl(
            vec![named, Rule::new(Action::Deny, IpNet::Any)],
            Action::Deny,
        );
        assert_eq!(
            list.to_string(),
            "10 permit 10.6.0.0/16 port 443 log\n20 deny any\ndefault deny\n"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn the_text_form_round_trips() {
        let text = r#"
            default = "deny"

            [[rule]]
            action = "permit"
            source = "10.6.0.0/16"
            port = "443"
            log = true

            [[rule]]
            action = "deny"
            source = "any"
        "#;
        let list: Acl = toml::from_str(text).unwrap();
        assert_eq!(list.rules.len(), 2);
        assert_eq!(
            list.rules[0].port,
            Some(PortRange {
                start: 443,
                end: 443
            })
        );
        let again: Acl = toml::from_str(&toml::to_string(&list).unwrap()).unwrap();
        assert_eq!(again, list);
        assert!(toml::from_str::<Acl>("surprise = 1").is_err());
        assert!(toml::from_str::<Acl>("[[rule]]\naction='permit'\nsource='nope'").is_err());
    }
}
