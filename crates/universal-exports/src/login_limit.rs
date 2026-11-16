//! How often a password or key may be tried at the login route.
//!
//! A secret is only as strong as the number of guesses allowed against it, whoever checks it.
//! Before an authenticator is asked anything, a try is counted twice: against the address it
//! came from and against the account name it is for. A try past either count is refused
//! without being judged, the right secret along with the wrong ones, and the caller is told
//! how long to wait.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use axum::http::{HeaderMap, HeaderName};
use governor::clock::{Clock, DefaultClock};
use governor::middleware::NoOpMiddleware;
use governor::state::keyed::DashMapStateStore;
use governor::{Quota, RateLimiter};

/// How many password or key tries the login route allows, and where it finds the address a
/// try came from. Every field has a default that is safe for a gateway on the public
/// internet; raise a count for an office behind one address.
#[derive(Debug, Clone)]
pub struct LoginLimits {
    /// Tries a minute from one address (an IPv6 address counts as its /64 network). Default 5.
    pub per_address: NonZeroU32,
    /// Tries in ten minutes for one account name, from anywhere. Default 10.
    pub per_name: NonZeroU32,
    /// A header the trusted reverse proxy sets to the client's address, such as
    /// `x-forwarded-for` (the last entry is used) or `x-real-ip`. The gateway listens on
    /// loopback behind that proxy, so without it every try comes from one address. Default none.
    pub address_header: Option<HeaderName>,
    /// The longest an account name is told apart by; longer names count as one. Default 64.
    pub name_bytes: usize,
    /// Every this many checks, the counts of addresses and names gone quiet are dropped, at
    /// least 1. Default 256.
    pub forget_every: u32,
    /// How long a try from one address is counted. Default one minute.
    pub address_window: Duration,
    /// How long a try for one account name is counted. Default ten minutes.
    pub name_window: Duration,
    /// How long a sign-in that has passed its first step waits for the next. Default five
    /// minutes.
    pub pending_seconds: Duration,
    /// Tries at the next step one waiting sign-in allows before it ends. Default 5.
    pub pending_tries: u32,
    /// How old a sign-in may be and still replace its person's second factor without a code from
    /// it. Default five minutes.
    pub enrol_fresh_seconds: Duration,
}

impl Default for LoginLimits {
    fn default() -> Self {
        let table = ai6::LoginTable::default();
        Self {
            per_address: NonZeroU32::new(5).unwrap_or(NonZeroU32::MIN),
            per_name: NonZeroU32::new(10).unwrap_or(NonZeroU32::MIN),
            address_header: None,
            name_bytes: table.name_bytes.or_max(),
            forget_every: u32::try_from(table.forget_every).unwrap_or(u32::MAX),
            address_window: table.address_window.0,
            name_window: table.name_window.0,
            pending_seconds: table.pending_seconds.0,
            pending_tries: u32::try_from(table.pending_tries).unwrap_or(u32::MAX),
            enrol_fresh_seconds: table.enrol_fresh_seconds.0,
        }
    }
}

type Counted<K> = RateLimiter<K, DashMapStateStore<K>, DefaultClock, NoOpMiddleware>;

/// The counts of tries kept by a gateway.
pub(crate) struct LoginGuard {
    clock: DefaultClock,
    addresses: Counted<IpAddr>,
    names: Counted<String>,
    address_header: Option<HeaderName>,
    checks: AtomicU32,
    name_bytes: usize,
    forget_every: u32,
}

impl LoginGuard {
    pub(crate) fn new(limits: &LoginLimits) -> Self {
        let clock = DefaultClock::default();
        Self {
            addresses: RateLimiter::dashmap_with_clock(
                quota(limits.address_window, limits.per_address),
                clock.clone(),
            ),
            names: RateLimiter::dashmap_with_clock(
                quota(limits.name_window, limits.per_name),
                clock.clone(),
            ),
            clock,
            address_header: limits.address_header.clone(),
            checks: AtomicU32::new(0),
            name_bytes: limits.name_bytes,
            forget_every: limits.forget_every.max(1),
        }
    }

    /// What the proxy configured by `address_header` said: its last line, which is the proxy's
    /// own (a client's lines come before it), and that line's rightmost entry.
    fn forwarded(&self, headers: &HeaderMap) -> Option<IpAddr> {
        let name = self.address_header.as_ref()?;
        headers
            .get_all(name)
            .iter()
            .next_back()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit(',').next())
            .and_then(|entry| parse_address(entry.trim()))
    }

    /// The address a request came from, for a sign-in policy: with a header configured only what
    /// the proxy said in it, so a request that bypassed the proxy has none (never the peer's);
    /// else the connection's. An IPv4-mapped IPv6 address is the IPv4 address.
    pub(crate) fn address(
        &self,
        headers: &HeaderMap,
        connection: Option<SocketAddr>,
    ) -> Option<IpAddr> {
        let found = if self.address_header.is_some() {
            self.forwarded(headers)
        } else {
            connection.map(|address| address.ip())
        };
        found.map(unmapped)
    }

    /// What a request is counted against: the proxy's address, else the connection's, else a
    /// single shared bucket for the unknown.
    pub(crate) fn source(&self, headers: &HeaderMap, connection: Option<SocketAddr>) -> IpAddr {
        self.forwarded(headers)
            .or_else(|| connection.map(|address| address.ip()))
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
    }

    /// Counts a try from `source` for `name`. `Some(wait)` when it is one too many, and it is
    /// then not to be judged.
    pub(crate) fn tried(&self, source: IpAddr, name: &str) -> Option<Duration> {
        if self.checks.fetch_add(1, Ordering::Relaxed) % self.forget_every == self.forget_every - 1
        {
            self.forget();
        }
        let now = self.clock.now();
        if let Err(refused) = self.addresses.check_key(&network(source)) {
            return Some(refused.wait_time_from(now));
        }
        // Counted only for a try the address was allowed, so an address held off cannot use
        // up a name's tries.
        let refused = self.names.check_key(&called(name, self.name_bytes)).err()?;
        Some(refused.wait_time_from(now))
    }

    /// Counts a try for the account `name` alone, whatever address it came from: a try already
    /// counted against its address. `Some(wait)` when it is one too many.
    pub(crate) fn tried_name(&self, name: &str) -> Option<Duration> {
        if self.checks.fetch_add(1, Ordering::Relaxed) % self.forget_every == self.forget_every - 1
        {
            self.forget();
        }
        let now = self.clock.now();
        let refused = self.names.check_key(&called(name, self.name_bytes)).err()?;
        Some(refused.wait_time_from(now))
    }

    /// Counts a try from `source` that names no account, so no account pays for it. `Some(wait)`
    /// when it is one too many.
    pub(crate) fn tried_address(&self, source: IpAddr) -> Option<Duration> {
        if self.checks.fetch_add(1, Ordering::Relaxed) % self.forget_every == self.forget_every - 1
        {
            self.forget();
        }
        let now = self.clock.now();
        let refused = self.addresses.check_key(&network(source)).err()?;
        Some(refused.wait_time_from(now))
    }

    fn forget(&self) {
        self.addresses.retain_recent();
        self.addresses.shrink_to_fit();
        self.names.retain_recent();
        self.names.shrink_to_fit();
    }
}

/// `tries` allowed in `window`, all of them at once or spread evenly.
fn quota(window: Duration, tries: NonZeroU32) -> Quota {
    Quota::with_period(window / tries.get())
        .unwrap_or_else(|| Quota::per_minute(tries))
        .allow_burst(tries)
}

/// An address as a header or a socket writes it, with or without a port.
fn parse_address(text: &str) -> Option<IpAddr> {
    text.parse::<IpAddr>()
        .or_else(|_| text.parse::<SocketAddr>().map(|address| address.ip()))
        .ok()
}

/// An IPv4-mapped IPv6 address as the IPv4 address it stands for.
fn unmapped(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        v4 => v4,
    }
}

/// What a try is counted against for an address: itself, or for IPv6 the /64 network it is in,
/// since whoever has one address there has all of them.
fn network(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !u128::from(u64::MAX))),
        },
        v4 => v4,
    }
}

/// What a try is counted against for a name: the name as accounts are told apart, and no
/// longer than is worth keeping.
fn called(name: &str, name_bytes: usize) -> String {
    let lowered = name.trim().to_lowercase();
    let mut end = lowered.len().min(name_bytes);
    while !lowered.is_char_boundary(end) {
        end -= 1;
    }
    lowered[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(per_address: u32, per_name: u32) -> LoginLimits {
        LoginLimits {
            per_address: NonZeroU32::new(per_address).unwrap(),
            per_name: NonZeroU32::new(per_name).unwrap(),
            address_header: None,
            ..LoginLimits::default()
        }
    }

    fn address(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn the_defaults_allow_five_a_minute_per_address_and_ten_per_name() {
        let defaults = LoginLimits::default();
        assert_eq!(defaults.per_address.get(), 5);
        assert_eq!(defaults.per_name.get(), 10);
        assert!(defaults.address_header.is_none());
    }

    #[test]
    fn an_address_is_refused_after_its_count_and_told_how_long_to_wait() {
        let guard = LoginGuard::new(&limits(3, 100));
        let source = address("192.0.2.1");
        for attempt in 0..3 {
            assert!(guard.tried(source, "alice").is_none(), "try {attempt}");
        }
        let wait = guard.tried(source, "alice").expect("the fourth is refused");
        assert!(wait > Duration::ZERO && wait <= Duration::from_secs(60));
        assert!(guard.tried(address("192.0.2.2"), "alice").is_none());
    }

    #[test]
    fn a_name_is_refused_after_its_count_from_any_address() {
        let guard = LoginGuard::new(&limits(100, 2));
        assert!(guard.tried(address("192.0.2.1"), "Alice").is_none());
        assert!(guard.tried(address("192.0.2.2"), " alice ").is_none());
        assert!(guard.tried(address("192.0.2.3"), "ALICE").is_some());
        assert!(guard.tried(address("192.0.2.3"), "bob").is_none());
    }

    #[test]
    fn an_address_held_off_does_not_use_up_a_names_tries() {
        let guard = LoginGuard::new(&limits(1, 2));
        assert!(guard.tried(address("192.0.2.1"), "alice").is_none());
        for _ in 0..10 {
            assert!(guard.tried(address("192.0.2.1"), "alice").is_some());
        }
        assert!(guard.tried(address("192.0.2.9"), "alice").is_none());
    }

    #[test]
    fn an_ipv6_address_counts_as_its_network() {
        let guard = LoginGuard::new(&limits(1, 100));
        assert!(guard.tried(address("2001:db8:1:2::1"), "a").is_none());
        assert!(guard.tried(address("2001:db8:1:2:ffff::9"), "b").is_some());
        assert!(guard.tried(address("2001:db8:1:3::1"), "c").is_none());
        assert_eq!(network(address("::ffff:192.0.2.1")), address("192.0.2.1"));
    }

    #[test]
    fn a_long_name_is_counted_by_its_first_bytes_without_splitting_a_character() {
        let long = "é".repeat(100);
        assert!(called(&long, 64).len() <= 64);
        assert!(called(&long, 64).chars().all(|character| character == 'é'));
        assert_eq!(
            called(&long, 5).len(),
            4,
            "the name_bytes setting bounds it"
        );
    }

    #[test]
    fn the_source_is_the_proxy_header_then_the_connection_then_one_shared_bucket() {
        let mut configured = limits(5, 10);
        configured.address_header = Some(HeaderName::from_static("x-forwarded-for"));
        let guard = LoginGuard::new(&configured);
        let connection: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let mut headers = HeaderMap::new();
        assert_eq!(
            guard.source(&headers, Some(connection)),
            address("127.0.0.1")
        );
        assert_eq!(guard.source(&headers, None), address("0.0.0.0"));
        // The trusted proxy appends the address it saw; what came before is the client's claim.
        headers.insert(
            "x-forwarded-for",
            "203.0.113.50, 198.51.100.7".parse().unwrap(),
        );
        assert_eq!(
            guard.source(&headers, Some(connection)),
            address("198.51.100.7")
        );
        headers.insert("x-forwarded-for", "not an address".parse().unwrap());
        assert_eq!(
            guard.source(&headers, Some(connection)),
            address("127.0.0.1")
        );
        // Without a configured header the claim is ignored.
        let plain = LoginGuard::new(&limits(5, 10));
        headers.insert("x-forwarded-for", "198.51.100.7".parse().unwrap());
        assert_eq!(
            plain.source(&headers, Some(connection)),
            address("127.0.0.1")
        );
    }

    #[test]
    fn the_policy_reads_the_last_line_and_rightmost_entry_of_the_address_header() {
        let guard = LoginGuard::new(&LoginLimits {
            address_header: Some(HeaderName::from_static("x-forwarded-for")),
            ..limits(5, 10)
        });
        let mut headers = HeaderMap::new();
        // The client's own line comes first; the proxy's, last, is the one that counts.
        headers.append("x-forwarded-for", "6.6.6.6".parse().unwrap());
        headers.append("x-forwarded-for", "203.0.113.7, 10.1.2.3".parse().unwrap());
        assert_eq!(guard.address(&headers, None), Some(address("10.1.2.3")));
    }

    #[test]
    fn a_missing_or_unreadable_header_is_no_address_to_the_policy_whatever_the_peer() {
        let guard = LoginGuard::new(&LoginLimits {
            address_header: Some(HeaderName::from_static("x-forwarded-for")),
            ..limits(5, 10)
        });
        let peer: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let mut headers = HeaderMap::new();
        assert_eq!(guard.address(&headers, Some(peer)), None);
        headers.insert("x-forwarded-for", "not an address".parse().unwrap());
        assert_eq!(guard.address(&headers, Some(peer)), None);
        // Counting keeps its shared bucket for the peer.
        assert_eq!(guard.source(&headers, Some(peer)), address("127.0.0.1"));
        // With no header configured the peer is the address.
        let plain = LoginGuard::new(&limits(5, 10));
        assert_eq!(
            plain.address(&headers, Some(peer)),
            Some(address("127.0.0.1"))
        );
    }

    #[test]
    fn an_ipv4_mapped_address_is_the_ipv4_address_to_the_policy() {
        let guard = LoginGuard::new(&LoginLimits {
            address_header: Some(HeaderName::from_static("x-forwarded-for")),
            ..limits(5, 10)
        });
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "::ffff:10.1.2.3".parse().unwrap());
        assert_eq!(guard.address(&headers, None), Some(address("10.1.2.3")));
        let plain = LoginGuard::new(&limits(5, 10));
        let peer: SocketAddr = "[::ffff:10.9.9.9]:5000".parse().unwrap();
        assert_eq!(
            plain.address(&HeaderMap::new(), Some(peer)),
            Some(address("10.9.9.9"))
        );
    }
}
