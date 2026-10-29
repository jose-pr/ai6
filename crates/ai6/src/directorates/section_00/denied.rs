//! What an assistant's network may not reach: the deployment's optional deny list.
//!
//! An `open` network reaches everything the Station can reach, the Station itself included;
//! `limited` reaches its listed hosts. Neither refuses anything else by default. A deployment
//! that wants some of it kept out names it in [`AssistantNetwork::denied`]:
//!
//! - a category: `loopback` (`127.0.0.0/8`, `::1`, `0.0.0.0`, `::`, `localhost` and every name
//!   under it, which is the Station's own machine), `private` (RFC 1918 `10.0.0.0/8`,
//!   `172.16.0.0/12`, `192.168.0.0/16`, CGNAT `100.64.0.0/10`, unique-local `fc00::/7`),
//!   `link-local` (`169.254.0.0/16`, `fe80::/10`) or `metadata` (the cloud instance metadata
//!   service: `169.254.169.254`, `fd00:ec2::254`, `metadata.google.internal`);
//! - a network in CIDR notation (`10.20.0.0/16`, `2001:db8::/32`) or one address;
//! - a host name, or `*.example.com` for every name under it.
//!
//! A name is checked as given and by every address it resolves to; an IPv4-mapped or
//! IPv4-compatible IPv6 address counts as its IPv4 address.
//!
//! [`AssistantNetwork::denied`]: super::agent::AssistantNetwork::denied

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::assistant::host_pattern;

/// The categories a deny list may name.
pub const DENY_CATEGORIES: [&str; 4] = ["loopback", "private", "link-local", "metadata"];

/// One entry of a deny list, as [`DenyEntry::parse`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyEntry {
    /// A host name, or `*.name` for every name under it.
    Host(String),
    /// A network: an address and how many leading bits of it count.
    Net(IpAddr, u8),
    /// One of [`DENY_CATEGORIES`].
    Category(&'static str),
}

impl DenyEntry {
    /// Reads `entry`: a category, a CIDR network, an address (IPv6 with or without brackets) or
    /// a host name as [`host_pattern`] takes it.
    ///
    /// # Errors
    ///
    /// A message naming the entry when it is none of them.
    pub fn parse(entry: &str) -> Result<Self, String> {
        if let Some(category) = DENY_CATEGORIES.iter().find(|name| **name == entry) {
            return Ok(Self::Category(category));
        }
        if let Some((address, bits)) = entry.split_once('/') {
            let address = parse_address(address)
                .ok_or_else(|| format!("{entry:?} is not a network: no address before '/'"))?;
            let max = if address.is_ipv4() { 32 } else { 128 };
            let bits = bits
                .parse::<u8>()
                .ok()
                .filter(|bits| *bits <= max)
                .ok_or_else(|| format!("{entry:?} is not a network: 0 to {max} bits after '/'"))?;
            return Ok(Self::Net(canonical(address), bits));
        }
        if let Some(address) = parse_address(entry) {
            let address = canonical(address);
            let bits = if address.is_ipv4() { 32 } else { 128 };
            return Ok(Self::Net(address, bits));
        }
        if host_pattern(entry) || entry == "localhost" {
            return Ok(Self::Host(entry.to_owned()));
        }
        Err(format!(
            "{entry:?} is not a category ({}), a network, an address or a host name",
            DENY_CATEGORIES.join(", ")
        ))
    }

    /// Whether the entry needs an address range checked, which only resolving a name and
    /// matching its addresses can do: a category but `metadata`, or a network wider than one
    /// address.
    pub fn is_range(&self) -> bool {
        match self {
            Self::Host(_) => false,
            Self::Net(address, bits) => *bits < if address.is_ipv4() { 32 } else { 128 },
            Self::Category(category) => *category != "metadata",
        }
    }
}

/// A deny list, read and checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DenyList {
    hosts: Vec<String>,
    nets: Vec<(IpAddr, u8)>,
    entries: Vec<DenyEntry>,
}

impl DenyList {
    /// Reads every entry of `entries`.
    ///
    /// # Errors
    ///
    /// The message of the first entry [`DenyEntry::parse`] refuses.
    pub fn parse<S: AsRef<str>>(entries: &[S]) -> Result<Self, String> {
        let mut list = Self::default();
        for entry in entries {
            let entry = DenyEntry::parse(entry.as_ref())?;
            match &entry {
                DenyEntry::Host(host) => list.hosts.push(host.clone()),
                DenyEntry::Net(address, bits) => list.nets.push((*address, *bits)),
                DenyEntry::Category(category) => {
                    let (hosts, nets) = category_members(category);
                    list.hosts
                        .extend(hosts.iter().map(|host| (*host).to_owned()));
                    list.nets.extend(nets);
                }
            }
            list.entries.push(entry);
        }
        Ok(list)
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries as read.
    pub fn entries(&self) -> &[DenyEntry] {
        &self.entries
    }

    /// Whether `host` (a lowercase name without a trailing dot, or an address) is denied by
    /// name, or, when it is an address, by address.
    pub fn denies_name(&self, host: &str) -> bool {
        if let Some(address) = parse_address(host) {
            return self.denies_address(address);
        }
        self.hosts
            .iter()
            .any(|pattern| match pattern.strip_prefix("*.") {
                Some(parent) => host
                    .strip_suffix(parent)
                    .is_some_and(|sub| sub.ends_with('.') && sub.len() > 1),
                None => host == pattern,
            })
    }

    /// Whether `address` is in a denied network, an IPv4-mapped or IPv4-compatible IPv6 address
    /// counting as its IPv4 address.
    pub fn denies_address(&self, address: IpAddr) -> bool {
        let address = canonical(address);
        self.nets
            .iter()
            .any(|(network, bits)| contains(*network, *bits, address))
    }

    /// The names and single addresses of the list, categories expanded, each as a host
    /// pattern, an IPv4 address or a bracketed IPv6 address: what a list of names can carry.
    pub fn names_and_addresses(&self) -> Vec<String> {
        let mut out: Vec<String> = self.hosts.clone();
        for (address, bits) in &self.nets {
            let single = *bits == if address.is_ipv4() { 32 } else { 128 };
            if single {
                out.push(match address {
                    IpAddr::V4(v4) => v4.to_string(),
                    IpAddr::V6(v6) => format!("[{v6}]"),
                });
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// The entries that need a range checked ([`DenyEntry::is_range`]), as given.
    pub fn ranges(&self) -> Vec<&DenyEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.is_range())
            .collect()
    }
}

/// The names and networks a category stands for.
fn category_members(category: &str) -> (&'static [&'static str], Vec<(IpAddr, u8)>) {
    let v4 = |a, b, c, d, bits| (IpAddr::V4(Ipv4Addr::new(a, b, c, d)), bits);
    let v6 = |segments: [u16; 8], bits| (IpAddr::V6(Ipv6Addr::from(segments)), bits);
    match category {
        "loopback" => (
            &["localhost", "*.localhost"],
            vec![
                v4(127, 0, 0, 0, 8),
                v4(0, 0, 0, 0, 32),
                v6([0, 0, 0, 0, 0, 0, 0, 1], 128),
                v6([0; 8], 128),
            ],
        ),
        "private" => (
            &[],
            vec![
                v4(10, 0, 0, 0, 8),
                v4(172, 16, 0, 0, 12),
                v4(192, 168, 0, 0, 16),
                v4(100, 64, 0, 0, 10),
                v6([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7),
            ],
        ),
        "link-local" => (
            &[],
            vec![
                v4(169, 254, 0, 0, 16),
                v6([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),
            ],
        ),
        "metadata" => (
            &["metadata.google.internal"],
            vec![
                v4(169, 254, 169, 254, 32),
                v6([0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254], 128),
            ],
        ),
        _ => (&[], Vec::new()),
    }
}

/// `text` as an address, IPv6 with or without brackets.
fn parse_address(text: &str) -> Option<IpAddr> {
    let inner = text
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(text);
    inner.parse().ok()
}

/// `address`, an IPv4-mapped (`::ffff:a.b.c.d`) or IPv4-compatible (`::a.b.c.d`, but `::` and
/// `::1`) IPv6 address as its IPv4 address.
fn canonical(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return IpAddr::V4(v4);
            }
            let segments = v6.segments();
            let compatible = segments[..6].iter().all(|segment| *segment == 0)
                && (segments[6] != 0 || segments[7] > 1);
            if compatible {
                let [a, b] = segments[6].to_be_bytes();
                let [c, d] = segments[7].to_be_bytes();
                return IpAddr::V4(Ipv4Addr::new(a, b, c, d));
            }
            IpAddr::V6(v6)
        }
        v4 => v4,
    }
}

/// Whether `address` lies in `network`/`bits` of the same family.
fn contains(network: IpAddr, bits: u8, address: IpAddr) -> bool {
    match (network, address) {
        (IpAddr::V4(network), IpAddr::V4(address)) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(bits)).unwrap_or(0);
            u32::from(network) & mask == u32::from(address) & mask
        }
        (IpAddr::V6(network), IpAddr::V6(address)) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(bits)).unwrap_or(0);
            u128::from(network) & mask == u128::from(address) & mask
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(entries: &[&str]) -> DenyList {
        DenyList::parse(entries).unwrap()
    }

    #[test]
    fn an_empty_list_denies_nothing() {
        let empty = list(&[]);
        for host in [
            "127.0.0.1",
            "localhost",
            "10.0.0.1",
            "169.254.169.254",
            "example.com",
        ] {
            assert!(!empty.denies_name(host), "{host}");
        }
    }

    #[test]
    fn loopback_takes_every_spelling_of_the_stations_own_machine() {
        let loopback = list(&["loopback"]);
        for host in [
            "127.0.0.1",
            "127.0.0.2",
            "127.255.255.254",
            "0.0.0.0",
            "::1",
            "[::1]",
            "::",
            "::ffff:127.0.0.1",
            "[::ffff:7f00:1]",
            "::127.0.0.2",
            "localhost",
            "a.localhost",
        ] {
            assert!(loopback.denies_name(host), "{host}");
        }
        for host in [
            "128.0.0.1",
            "10.0.0.1",
            "::2:1:0:0",
            "localhost.example",
            "example.com",
        ] {
            assert!(!loopback.denies_name(host), "{host}");
        }
    }

    #[test]
    fn the_other_categories_networks_addresses_and_names() {
        let private = list(&["private"]);
        for host in [
            "10.1.2.3",
            "172.31.0.1",
            "192.168.1.1",
            "100.64.0.1",
            "fd12::1",
        ] {
            assert!(private.denies_name(host), "{host}");
        }
        for host in ["172.32.0.1", "100.128.0.1", "11.0.0.1", "fe80::1"] {
            assert!(!private.denies_name(host), "{host}");
        }
        let link = list(&["link-local"]);
        assert!(link.denies_name("169.254.1.1") && link.denies_name("fe80::1"));
        let metadata = list(&["metadata"]);
        for host in [
            "169.254.169.254",
            "fd00:ec2::254",
            "metadata.google.internal",
        ] {
            assert!(metadata.denies_name(host), "{host}");
        }
        assert!(!metadata.denies_name("169.254.169.253"));
        let mixed = list(&[
            "10.20.0.0/16",
            "2001:db8::/32",
            "192.0.2.7",
            "*.corp.example",
        ]);
        for host in [
            "10.20.9.9",
            "2001:db8:1::1",
            "192.0.2.7",
            "git.corp.example",
        ] {
            assert!(mixed.denies_name(host), "{host}");
        }
        for host in ["10.21.0.1", "192.0.2.8", "corp.example"] {
            assert!(!mixed.denies_name(host), "{host}");
        }
    }

    #[test]
    fn what_a_name_list_carries_and_what_needs_a_range() {
        let deny = list(&[
            "loopback",
            "metadata",
            "10.0.0.0/8",
            "192.0.2.7",
            "intranet.example",
        ]);
        assert_eq!(
            deny.names_and_addresses(),
            [
                "*.localhost",
                "0.0.0.0",
                "169.254.169.254",
                "192.0.2.7",
                "[::1]",
                "[::]",
                "[fd00:ec2::254]",
                "intranet.example",
                "localhost",
                "metadata.google.internal",
            ]
        );
        assert_eq!(
            deny.ranges(),
            [
                &DenyEntry::Category("loopback"),
                &DenyEntry::Net("10.0.0.0".parse().unwrap(), 8)
            ]
        );
    }

    #[test]
    fn a_bad_entry_is_refused() {
        for bad in [
            "*",
            "10.0.0.0/33",
            "::/129",
            "/8",
            "Example.com",
            "nohost",
            "lan",
        ] {
            assert!(DenyEntry::parse(bad).is_err(), "{bad}");
        }
    }
}
