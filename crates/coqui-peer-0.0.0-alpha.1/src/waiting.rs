//! The bounds on a waiting queue, kept apart from where its entries are stored.

use alloc::collections::VecDeque;
use core::net::{IpAddr, SocketAddr};
use core::time::Duration;
use std::collections::HashMap;
use std::time::Instant;

use crate::id::PeerId;
use crate::store::{DEFAULT_IPV6_PREFIX, WaitingLimits, source_key};

/// The least time between two new records from one source, unless
/// [`record_interval`](WaitingQueueBuilder::record_interval) says otherwise.
pub const DEFAULT_RECORD_INTERVAL: Duration = Duration::from_secs(1);

/// What a source at [`per_source`](WaitingLimits::per_source) entries does with a newcomer.
///
/// In text `refuse` or `replace-oldest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, coqui_builder::Leaf)]
#[map(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum WhenFull {
    /// Refuses the newcomer and leaves those waiting alone. A flood can fill a source, so that
    /// nobody new is recorded from it until an operator clears it, but cannot remove anyone.
    #[default]
    Refuse,
    /// Replaces the source's oldest entry. Peers sharing a source (everyone behind one reverse
    /// proxy, when it is what connects) can then flush each other from the queue.
    ReplaceOldest,
}

/// What [`WaitingQueue::admit`] decided for one peer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Admission {
    /// Whether to record the peer as waiting now.
    pub record: bool,
    /// Entries of the same source to remove first, oldest first; always empty unless the queue
    /// is [`WhenFull::ReplaceOldest`].
    pub evict: Vec<PeerId>,
}

/// Decides which unaccepted peers are recorded as waiting, without storing anything: the
/// policy of [`TrustStore::record_waiting`](crate::TrustStore::record_waiting), for a host
/// that keeps its waiting entries elsewhere.
///
/// A source is an IPv4 address (an IPv4-mapped IPv6 address counts as its IPv4 address), an
/// IPv6 prefix of [`ipv6_prefix`](WaitingQueueBuilder::ipv6_prefix) bits (/64 unless set,
/// since one holder controls every address of a /64), or no address at all, which is one
/// source. The queue remembers what it admitted, per source, oldest first; it forgets nothing
/// on its own, beyond the `per_source` entries it keeps for each source.
#[derive(Debug, Clone, coqui_builder::Configured)]
pub struct WaitingQueue {
    #[configured]
    settings: WaitingSettings,
    sources: HashMap<Option<IpAddr>, VecDeque<(PeerId, Instant)>>,
}

/// What a [`WaitingQueue`] is built from, in the form a file, a form or a peer fills.
///
/// Every key is optional (an absent one is its default) and an unknown key is refused. The type
/// is `Describe`, so a `Partial<WaitingSettings>` is filled from text and built, and it is
/// `#[non_exhaustive]`: start from [`Default`] and assign the fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, coqui_builder::Describe)]
#[builder(setters_on = WaitingQueueBuilder)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct WaitingSettings {
    /// The bounds on entries. Default [`WaitingLimits::default`].
    #[map(default = WaitingSettings::default().limits)]
    pub limits: WaitingLimits,
    /// The IPv6 prefix length sources are grouped by: 64, since one holder controls every address
    /// of a /64. 128 counts each address alone, which lets the holder of a subnet fill the queue;
    /// values above 128 are 128. IPv4 sources, IPv4-mapped IPv6 ones included, count per address.
    #[map(default = WaitingSettings::default().ipv6_prefix)]
    pub ipv6_prefix: u8,
    /// What a source at its limit does with a newcomer. Default [`WhenFull::Refuse`], so that
    /// strangers sharing a source cannot flush those already waiting.
    #[map(default = WaitingSettings::default().when_full)]
    pub when_full: WhenFull,
    /// The least time between two new records from one source, each record being a synced write
    /// a stranger can cause. Default [`DEFAULT_RECORD_INTERVAL`]; zero is no limit.
    #[map(default = WaitingSettings::default().record_interval)]
    pub record_interval: Duration,
}

impl Default for WaitingSettings {
    fn default() -> Self {
        Self {
            limits: WaitingLimits::default(),
            ipv6_prefix: DEFAULT_IPV6_PREFIX,
            when_full: WhenFull::default(),
            record_interval: DEFAULT_RECORD_INTERVAL,
        }
    }
}

impl WaitingSettings {
    /// Every setting that differs from its default, named by key path (`limits.per_source`).
    pub fn non_defaults(&self) -> Vec<coqui_builder::NonDefault> {
        coqui_builder::non_defaults(self)
    }
}

/// What a [`WaitingQueueBuilder`] collects: the settings.
///
/// Build the queue with [`WaitingQueue::builder`].
#[derive(coqui_builder::Builder, Debug)]
#[builder(build = WaitingQueue)]
pub struct WaitingQueueParts {
    settings: WaitingSettings,
}

impl From<WaitingQueueParts> for WaitingQueue {
    fn from(parts: WaitingQueueParts) -> Self {
        Self::from_settings(parts.settings)
    }
}

impl WaitingQueue {
    /// An empty queue under `settings`; an IPv6 prefix above 128 is 128.
    pub(crate) fn from_settings(settings: WaitingSettings) -> Self {
        Self {
            settings: WaitingSettings {
                ipv6_prefix: settings.ipv6_prefix.min(128),
                ..settings
            },
            sources: HashMap::new(),
        }
    }

    /// An empty queue under `limits` with every other setting at its default:
    /// `WaitingQueue::builder().limits(limits).build()`.
    pub fn new(limits: WaitingLimits) -> Self {
        Self::from_settings(WaitingSettings {
            limits,
            ..WaitingSettings::default()
        })
    }

    /// The settings it was built from.
    pub fn settings(&self) -> WaitingSettings {
        self.settings
    }

    /// What it does with a newcomer to a full source.
    pub fn when_full(&self) -> WhenFull {
        self.settings.when_full
    }

    /// The least time between two new records from one source.
    pub fn record_interval(&self) -> Duration {
        self.settings.record_interval
    }

    /// The limits it applies.
    pub fn limits(&self) -> WaitingLimits {
        self.settings.limits
    }

    /// The IPv6 prefix length sources are grouped by.
    pub fn ipv6_prefix(&self) -> u8 {
        self.settings.ipv6_prefix
    }

    /// Decides whether `id`, from `source`, is recorded now.
    ///
    /// `stored` is how many waiting entries the host holds; at
    /// [`total`](WaitingLimits::total) or above, nothing is recorded (the newcomer waits, no
    /// entry is dropped for it). The same peer from the same source is recorded again only
    /// after [`rerecord`](WaitingLimits::rerecord). A source records a new peer at most once per
    /// [`record_interval`](WaitingQueue::record_interval). A source at
    /// [`per_source`](WaitingLimits::per_source) does what [`WhenFull`] says: by default the
    /// newcomer is not recorded; with [`WhenFull::ReplaceOldest`] its oldest entries are listed in
    /// [`Admission::evict`] for the host to remove. The host checks itself whether `id` is
    /// already stored, and does not call this then.
    pub fn admit(
        &mut self,
        id: PeerId,
        source: Option<SocketAddr>,
        stored: usize,
        now: Instant,
    ) -> Admission {
        let refused = Admission {
            record: false,
            evict: Vec::new(),
        };
        if stored >= self.settings.limits.total {
            return refused;
        }
        let bucket = source.map(|source| source_key(source.ip(), self.settings.ipv6_prefix));
        let held = self.sources.entry(bucket).or_default();
        if held.iter().any(|(had, at)| {
            *had == id && now.saturating_duration_since(*at) < self.settings.limits.rerecord
        }) {
            return refused;
        }
        let interval = self.settings.record_interval;
        if held
            .iter()
            .any(|(_, at)| now.saturating_duration_since(*at) < interval)
        {
            return refused;
        }
        held.retain(|(had, _)| *had != id);
        if self.settings.when_full == WhenFull::Refuse
            && held.len() >= self.settings.limits.per_source
        {
            return refused;
        }
        held.push_back((id, now));
        let mut evict = Vec::new();
        while held.len() > self.settings.limits.per_source {
            if let Some((old, _)) = held.pop_front() {
                evict.push(old);
            }
        }
        Admission {
            record: true,
            evict,
        }
    }

    /// Remembers that `id` from `source` was recorded at `at`, for a host that seeds the queue
    /// from entries it stored earlier. Entries past `per_source` for that source are
    /// forgotten, oldest first, and returned.
    pub fn remember(&mut self, id: PeerId, source: Option<SocketAddr>, at: Instant) -> Vec<PeerId> {
        let bucket = source.map(|source| source_key(source.ip(), self.settings.ipv6_prefix));
        let held = self.sources.entry(bucket).or_default();
        held.retain(|(had, _)| *had != id);
        let position = held
            .iter()
            .position(|(_, when)| *when > at)
            .unwrap_or(held.len());
        held.insert(position, (id, at));
        let mut forgotten = Vec::new();
        while held.len() > self.settings.limits.per_source {
            if let Some((old, _)) = held.pop_front() {
                forgotten.push(old);
            }
        }
        forgotten
    }

    /// Forgets every entry the host no longer stores (`stored` is false for it) once its
    /// [`rerecord`](WaitingLimits::rerecord) time has passed; within it the entry still keeps
    /// the same peer from being recorded again from its source.
    ///
    /// For a host whose entries another process may accept or remove: without it, the queue
    /// remembers them, and its memory grows with every source it has seen.
    pub fn prune(&mut self, now: Instant, stored: impl Fn(&PeerId) -> bool) {
        let rerecord = self.settings.limits.rerecord;
        for held in self.sources.values_mut() {
            held.retain(|(id, at)| stored(id) || now.saturating_duration_since(*at) < rerecord);
        }
        self.sources.retain(|_, held| !held.is_empty());
    }

    /// The number of entries remembered, across every source.
    pub fn len(&self) -> usize {
        self.sources.values().map(VecDeque::len).sum()
    }

    /// Whether nothing is remembered.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Forgets `id` from every source, as when the host accepts or removes it.
    pub fn forget(&mut self, id: &PeerId) {
        for held in self.sources.values_mut() {
            held.retain(|(had, _)| had != id);
        }
        self.sources.retain(|_, held| !held.is_empty());
    }
}
