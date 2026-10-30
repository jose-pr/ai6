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
}

impl Default for LoginLimits {
    fn default() -> Self {
        Self {
            per_address: NonZeroU32::new(5).unwrap_or(NonZeroU32::MIN),
            per_name: NonZeroU32::new(10).unwrap_or(NonZeroU32::MIN),
            address_header: None,
            name_bytes: 64,
            forget_every: 256,
            address_window: Duration::from_secs(60),
            name_window: Duration::from_secs(600),
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

    /// The address a request came from: the proxy's header when one is configured and holds
    /// an address, else the connection's, else a single shared bucket for the unknown.
    pub(crate) fn source(&self, headers: &HeaderMap, connection: Option<SocketAddr>) -> IpAddr {
        self.address_header
            .as_ref()
            .and_then(|name| headers.get(name))
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit(',').next())
            .and_then(|entry| parse_address(entry.trim()))
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
}
