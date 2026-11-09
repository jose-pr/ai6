//! Choosing an account from the named fields of a credential, such as a certificate's.
//!
//! A provider that reads a certificate gets a map of field names to values (`subject.cn`,
//! `san.email`, `san.upn`, `fingerprint`, as `coqui-peer-tls`'s `CertificateFields::to_map`
//! writes it) and a list of [`Pick`]s from its configuration. [`pick`] tries them in order and
//! says which [`Lookup`](coqui_access::Lookup) the credential names.

use std::collections::BTreeMap;
use std::net::IpAddr;

use coqui_access::guatiao::{FromValue, Schema, ToValue};

/// The field DNS names that are IP literals are listed under, beside `san.dns`.
pub const DNS_IP_FIELD: &str = "san.dns.ip";
/// The field IP address names are listed under.
pub const IP_FIELD: &str = "san.ip";

/// One way to name the account: a field of the credential, and what that field is in the
/// directory.
///
/// ```json
/// { "field": "san.upn", "by": "name", "domain": "example.org" }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao)]
#[non_exhaustive]
pub struct Pick {
    /// The credential's field, such as `san.email` or `subject.cn`.
    pub field: String,
    /// What the value is in the directory: the [`Lookup::by`](coqui_access::Lookup::by) it
    /// is answered with, such as `email` or `name`.
    pub by: String,
    /// Keep only values ending `@<domain>` (any case) and take the part before it, as for a
    /// UPN or an email address naming an account of one domain.
    pub domain: Option<String>,
    /// For `san.ip`: an IP address written as a DNS name counts as an IP name too. Some
    /// issuers put an address in a `dNSName` for clients that read no IP SANs. Default
    /// `true`.
    #[map(default = true)]
    pub ip_in_dns: bool,
}

impl Pick {
    /// The value of `field`, looked up `by`, any domain, IPs in DNS names counted.
    pub fn new(field: impl Into<String>, by: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            by: by.into(),
            domain: None,
            ip_in_dns: true,
        }
    }

    /// This pick keeping only values of `domain`, without it.
    #[must_use]
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// This pick with IP addresses written as DNS names counted (`true`) or not.
    #[must_use]
    pub fn ip_in_dns(mut self, counted: bool) -> Self {
        self.ip_in_dns = counted;
        self
    }

    /// What this pick finds in `fields`.
    fn find(&self, fields: &BTreeMap<String, Vec<String>>) -> Picked {
        let mut values: Vec<String> = fields.get(&self.field).cloned().unwrap_or_default();
        if self.field == IP_FIELD && self.ip_in_dns {
            values.extend(fields.get(DNS_IP_FIELD).cloned().unwrap_or_default());
        }
        if self.field == IP_FIELD || self.field == DNS_IP_FIELD {
            for value in &mut values {
                if let Ok(address) = value.parse::<IpAddr>() {
                    *value = canonical(address).to_string();
                }
            }
        }
        if let Some(domain) = &self.domain {
            let suffix = format!("@{}", domain.to_ascii_lowercase());
            values = values
                .into_iter()
                .filter_map(|value| {
                    let cut = value.len().checked_sub(suffix.len())?;
                    (value.is_char_boundary(cut)
                        && value[cut..].eq_ignore_ascii_case(&suffix)
                        && cut > 0)
                        .then(|| value[..cut].to_owned())
                })
                .collect();
        }
        values.sort();
        values.dedup();
        match values.len() {
            0 => Picked::Absent,
            1 => Picked::Found {
                by: self.by.clone(),
                value: values.remove(0),
            },
            _ => Picked::Ambiguous,
        }
    }
}

/// An address as one text: IPv6 compressed, an IPv4-mapped IPv6 address as its IPv4 one.
fn canonical(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        address => address,
    }
}

/// What [`pick`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Picked {
    /// No pick found a value: the credential names nobody.
    Absent,
    /// A pick's field holds two different values: the credential names nobody for certain.
    Ambiguous,
    /// The first pick with a value, and the value.
    Found {
        /// What the value is in the directory.
        by: String,
        /// The value.
        value: String,
    },
}

/// Tries each of `order` against `fields`, first to last: the first that finds one value
/// decides, one that finds two different values is [`Picked::Ambiguous`], and none finding
/// any is [`Picked::Absent`].
///
/// ```
/// use std::collections::BTreeMap;
/// use coqui_access_kit::pick::{Pick, Picked, pick};
///
/// let fields = BTreeMap::from([("san.upn".to_owned(), vec!["alice@EXAMPLE.org".to_owned()])]);
/// let found = pick(&fields, &[Pick::new("san.email", "email"), Pick::new("san.upn", "name").domain("example.org")]);
/// assert_eq!(found, Picked::Found { by: "name".into(), value: "alice".into() });
/// ```
pub fn pick(fields: &BTreeMap<String, Vec<String>>, order: &[Pick]) -> Picked {
    for each in order {
        match each.find(fields) {
            Picked::Absent => {}
            decided => return decided,
        }
    }
    Picked::Absent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(key, values)| {
                (
                    (*key).to_owned(),
                    values.iter().map(|value| (*value).to_owned()).collect(),
                )
            })
            .collect()
    }

    fn found(by: &str, value: &str) -> Picked {
        Picked::Found {
            by: by.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn picks_are_tried_in_order_and_two_values_are_ambiguous() {
        let map = fields(&[
            ("san.email", &["a@example.org", "b@example.org"]),
            ("subject.cn", &["Alice"]),
        ]);
        let cn = Pick::new("subject.cn", "name");
        assert_eq!(
            pick(&map, std::slice::from_ref(&cn)),
            found("name", "Alice")
        );
        assert_eq!(
            pick(&map, &[Pick::new("san.email", "email"), cn.clone()]),
            Picked::Ambiguous
        );
        assert_eq!(
            pick(&map, &[Pick::new("san.uri", "uri"), cn]),
            found("name", "Alice")
        );
        assert_eq!(pick(&map, &[Pick::new("san.uri", "uri")]), Picked::Absent);
        assert_eq!(pick(&map, &[]), Picked::Absent);
    }

    #[test]
    fn a_domain_keeps_its_own_values_and_takes_the_part_before() {
        let map = fields(&[("san.upn", &["alice@CORP.example", "bob@other.example"])]);
        let corp = Pick::new("san.upn", "name").domain("corp.example");
        assert_eq!(pick(&map, &[corp]), found("name", "alice"));
        let none = fields(&[("san.upn", &["@corp.example", "alice@corp.example.evil"])]);
        assert_eq!(
            pick(
                &none,
                &[Pick::new("san.upn", "name").domain("corp.example")]
            ),
            Picked::Absent
        );
    }

    #[test]
    fn an_ip_in_a_dns_name_counts_as_an_ip_name_unless_turned_off() {
        let ip = Pick::new("san.ip", "address");
        let dns_only = fields(&[("san.dns", &["10.0.0.9"]), ("san.dns.ip", &["10.0.0.9"])]);
        let ip_only = fields(&[("san.ip", &["10.0.0.9"])]);
        let both = fields(&[("san.ip", &["10.0.0.9"]), ("san.dns.ip", &["10.0.0.9"])]);
        let differ = fields(&[("san.ip", &["10.0.0.9"]), ("san.dns.ip", &["10.0.0.8"])]);
        for map in [&dns_only, &ip_only, &both] {
            assert_eq!(
                pick(map, std::slice::from_ref(&ip)),
                found("address", "10.0.0.9")
            );
        }
        assert_eq!(pick(&differ, std::slice::from_ref(&ip)), Picked::Ambiguous);
        let strict = ip.ip_in_dns(false);
        assert_eq!(
            pick(&dns_only, std::slice::from_ref(&strict)),
            Picked::Absent
        );
        assert_eq!(pick(&differ, &[strict]), found("address", "10.0.0.9"));
    }

    #[test]
    fn addresses_are_compared_in_one_spelling() {
        let mapped = fields(&[
            ("san.ip", &["::ffff:10.0.0.9"]),
            ("san.dns.ip", &["10.0.0.9"]),
        ]);
        assert_eq!(
            pick(&mapped, &[Pick::new("san.ip", "address")]),
            found("address", "10.0.0.9")
        );
        let six = fields(&[
            ("san.ip", &["2001:db8:0:0:0:0:0:1"]),
            ("san.dns.ip", &["2001:DB8::1"]),
        ]);
        assert_eq!(
            pick(&six, &[Pick::new("san.ip", "address")]),
            found("address", "2001:db8::1")
        );
    }

    #[test]
    fn a_pick_reads_from_configuration_with_ip_in_dns_on() {
        use coqui_access::guatiao::Map;
        let mut map = Map::new();
        map.set("field", "san.ip");
        map.set("by", "address");
        let read = Pick::from_value(&map.into()).unwrap();
        assert_eq!(read, Pick::new("san.ip", "address"));
        assert!(read.ip_in_dns);
    }
}
