use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::future::Future;
use core::net::{IpAddr, SocketAddr};
use core::pin::Pin as Pinned;
use core::task::{Context, Poll, Waker};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use crate::clock::{Clock, later};
use crate::error::Error;
use crate::id::PeerId;
use crate::pair::code::{CodeAlphabet, PairingCode, Pin, UserCode, below, same};
use crate::pair::{PairRefusal, PairingError};
use crate::random::Random;
use crate::store::{DEFAULT_IPV6_PREFIX, source_key};

/// Draws of a user code that is not already pending before the desk gives up.
const ISSUE_DRAWS: usize = 64;

/// What a responder offers and how its codes are made. Every field has a default; any value
/// with a meaning is accepted, and [`non_defaults`](PairingOptions::non_defaults) lists the ones that
/// differ from their default. Every key is optional and an unknown key is refused; a duration
/// reads as `600`, `90s` or `250ms`, and `pin_ttl` and `device_ttl` also read `never`.
/// `#[non_exhaustive]`: start from [`Default`] and assign the fields.
#[derive(Debug, Clone, PartialEq, Eq, coqui_builder::Describe)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct PairingOptions {
    /// Whether joiners may pair with an invitation. Default `true`.
    #[map(default = PairingOptions::default().invitation)]
    pub invitation: bool,
    /// Whether joiners may pair with a PIN (needs the `pin` feature). Default `true`.
    #[map(default = PairingOptions::default().pin)]
    pub pin: bool,
    /// Whether a joiner may show a PIN for an operator to type into this side, which holds its
    /// run until the operator answers or `pin_ttl` passes. Turn it off where nobody is there
    /// to answer. Default `true`.
    #[map(default = PairingOptions::default().pin_shown)]
    pub pin_shown: bool,
    /// Whether joiners may pair by the device flow. Default `true`.
    #[map(default = PairingOptions::default().device)]
    pub device: bool,
    /// The alphabet and length of a PIN the desk shows. Default `0123456789` × 6.
    #[map(default = PairingOptions::default().pin_alphabet)]
    pub pin_alphabet: CodeAlphabet,
    /// How long a shown PIN, or a slot waiting for a joiner's PIN, lasts; `None` (the text
    /// `never`) lasts for good. Default 10 minutes; it is also the window of
    /// [`DeskLimits::pin_runs_per_source`].
    #[map(default = PairingOptions::default().pin_ttl, none = "never")]
    pub pin_ttl: Option<Duration>,
    /// How many runs may try a shown PIN before it is burned; a run counts once it is past
    /// the first PAKE message, whatever its outcome. Default 1.
    #[map(default = PairingOptions::default().pin_attempts)]
    pub pin_attempts: u32,
    /// The alphabet and length of a device-flow user code. Default RFC 8628's
    /// `BCDFGHJKLMNPQRSTVWXZ` × 8.
    #[map(default = PairingOptions::default().user_code_alphabet)]
    pub user_code_alphabet: CodeAlphabet,
    /// How long a user code waits for approval; `None` (the text `never`) waits for good.
    /// Default 10 minutes; it is also the window of [`DeskLimits::wrong_codes_total`].
    #[map(default = PairingOptions::default().device_ttl, none = "never")]
    pub device_ttl: Option<Duration>,
}

impl Default for PairingOptions {
    fn default() -> Self {
        Self {
            invitation: true,
            pin: true,
            pin_shown: true,
            device: true,
            pin_alphabet: CodeAlphabet::pin(),
            pin_ttl: Some(Duration::from_secs(600)),
            pin_attempts: 1,
            user_code_alphabet: CodeAlphabet::user_code(),
            device_ttl: Some(Duration::from_secs(600)),
        }
    }
}

/// Bounds on a desk's pending work and on guessing. Every field has a default; any value is
/// accepted, and [`non_defaults`](DeskLimits::non_defaults) lists the ones that differ from
/// their default. Read like [`PairingOptions`]; `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, coqui_builder::Describe)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct DeskLimits {
    /// Pairing exchanges in progress at once. Default 32.
    #[map(default = DeskLimits::default().pending)]
    pub pending: usize,
    /// Pairing exchanges in progress at once from one source (every mode). A source is an IPv4
    /// address or an IPv6 prefix of [`ipv6_prefix`](DeskLimits::ipv6_prefix) bits. Default 4.
    #[map(default = DeskLimits::default().pending_per_source)]
    pub pending_per_source: usize,
    /// PIN runs one source may start per [`PairingOptions::pin_ttl`]. A source is an IPv4
    /// address or an IPv6 prefix of [`ipv6_prefix`](DeskLimits::ipv6_prefix) bits. Default 8.
    #[map(default = DeskLimits::default().pin_runs_per_source)]
    pub pin_runs_per_source: usize,
    /// Wrong user codes one approver may enter per
    /// [`approver_window`](DeskLimits::approver_window). Default 5.
    #[map(default = DeskLimits::default().wrong_codes_per_approver)]
    pub wrong_codes_per_approver: usize,
    /// The window of [`wrong_codes_per_approver`](DeskLimits::wrong_codes_per_approver).
    /// Default 1 minute.
    #[map(default = DeskLimits::default().approver_window)]
    pub approver_window: Duration,
    /// Wrong user codes all approvers together may enter per
    /// [`PairingOptions::device_ttl`]. Default 60.
    #[map(default = DeskLimits::default().wrong_codes_total)]
    pub wrong_codes_total: usize,
    /// The IPv6 prefix length sources are grouped by. Default 64.
    #[map(default = DeskLimits::default().ipv6_prefix)]
    pub ipv6_prefix: u8,
}

impl Default for DeskLimits {
    fn default() -> Self {
        Self {
            pending: 32,
            pending_per_source: 4,
            pin_runs_per_source: 8,
            wrong_codes_per_approver: 5,
            approver_window: Duration::from_secs(60),
            wrong_codes_total: 60,
            ipv6_prefix: DEFAULT_IPV6_PREFIX,
        }
    }
}

impl PairingOptions {
    /// Whether every option has a meaning.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOptions`] for a meaningless alphabet, a zero TTL or zero attempts.
    pub fn check(&self) -> Result<(), Error> {
        self.pin_alphabet.check()?;
        self.user_code_alphabet.check()?;
        if self.pin_ttl == Some(Duration::ZERO)
            || self.device_ttl == Some(Duration::ZERO)
            || self.pin_attempts == 0
        {
            return Err(Error::InvalidOptions);
        }
        Ok(())
    }

    /// Every option that differs from its default, named by key path (`pin_attempts`,
    /// `pin_alphabet.len`).
    pub fn non_defaults(&self) -> Vec<coqui_builder::NonDefault> {
        coqui_builder::non_defaults(self)
    }
}

impl DeskLimits {
    /// Every limit that differs from its default, named by key path.
    pub fn non_defaults(&self) -> Vec<coqui_builder::NonDefault> {
        coqui_builder::non_defaults(self)
    }
}

/// What a [`PairingDesk`] is built from, besides its random source and clock, in the form a
/// file, a form or a peer fills.
///
/// Every key is optional (an absent one is its default) and an unknown key is refused. The type
/// is `Describe`, so a `Partial<PairingDeskSettings>` is filled from text and built, and it is
/// `#[non_exhaustive]`: start from [`Default`] and assign the fields.
#[derive(Debug, Clone, PartialEq, Eq, Default, coqui_builder::Describe)]
#[builder(setters_on = PairingDeskBuilder)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct PairingDeskSettings {
    /// What the desk offers and how its codes are made.
    #[map(default = PairingDeskSettings::default().options)]
    pub options: PairingOptions,
    /// The bounds on pending work and on guessing.
    #[map(default = PairingDeskSettings::default().limits)]
    pub limits: DeskLimits,
}

impl PairingDeskSettings {
    /// Every setting that differs from its default, named by key path (`limits.pending`).
    pub fn non_defaults(&self) -> Vec<coqui_builder::NonDefault> {
        coqui_builder::non_defaults(self)
    }
}

/// What a [`PairingDeskBuilder`] collects: the settings, the random source and the clock.
///
/// The random source is required, the clock is the system clock unless given. Build the desk with
/// [`PairingDesk::builder`].
#[derive(coqui_builder::Builder, Debug)]
#[builder(build = PairingDesk, fallible)]
pub struct PairingDeskParts {
    settings: PairingDeskSettings,
    #[builder(input)]
    random: Random,
    #[builder(input)]
    clock: Option<Clock>,
}

impl TryFrom<PairingDeskParts> for PairingDesk {
    type Error = Error;

    /// The desk.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOptions`] as [`PairingOptions::check`].
    fn try_from(parts: PairingDeskParts) -> Result<Self, Error> {
        parts.settings.options.check()?;
        Ok(PairingDesk::from_parts(
            parts.random,
            parts.clock.unwrap_or_default(),
            parts.settings,
        ))
    }
}

/// A responder's handle on a decision it waits for: a joiner's PIN typed by the operator, or
/// the approval of a device-flow joiner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ticket(u64);

/// How a waiting exchange is settled.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Decision {
    /// The operator typed this PIN for a joiner that shows its own.
    Code(Pin),
    /// The operator approved a device-flow joiner.
    Approved,
    /// The operator denied a device-flow joiner.
    Denied,
    /// Nobody decided before the code expired.
    Expired,
}

/// A PIN the desk shows, for the caller to display.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShownPin {
    /// The PIN.
    pub pin: Pin,
    /// When it stops being accepted.
    pub expires: SystemTime,
}

/// A joiner that shows its own PIN and waits for the operator to type it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingSlot {
    /// The slot the desk assigned, which the joiner shows with its PIN.
    pub slot: u8,
    /// The joiner's id.
    pub peer: PeerId,
    /// Where it connected from, when known.
    pub source: Option<SocketAddr>,
    /// When the slot expires.
    pub expires: SystemTime,
}

/// A device-flow joiner found by its user code: what an app may show before it approves or
/// denies.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingDevice {
    /// The joiner's id.
    pub peer: PeerId,
    /// Where it connected from, when known.
    pub source: Option<SocketAddr>,
    /// The label the joiner sent; the joiner chose it, so it proves nothing.
    pub label: String,
    /// When its user code expires.
    pub expires: SystemTime,
    ticket: Ticket,
}

#[derive(Debug)]
enum Kind {
    Slot(u8),
    Device(UserCode),
}

#[derive(Debug)]
struct Waiting {
    kind: Kind,
    peer: PeerId,
    source: Option<SocketAddr>,
    label: String,
    expires: SystemTime,
    /// The decision, until the responder takes it.
    decision: Option<Decision>,
    /// Whether a decision was made: a settled entry is listed and decided no more, even after
    /// its decision was taken.
    settled: bool,
    waker: Option<Waker>,
}

#[derive(Debug)]
struct State {
    options: PairingOptions,
    limits: DeskLimits,
    clock: Clock,
    random: Random,
    shown: Option<(ShownPin, u32)>,
    gone: PairRefusal,
    waiting: HashMap<u64, Waiting>,
    next: u64,
    pending: usize,
    pending_by: HashMap<Option<IpAddr>, usize>,
    runs: HashMap<Option<IpAddr>, VecDeque<SystemTime>>,
    wrong_by: HashMap<String, VecDeque<SystemTime>>,
    wrong_all: VecDeque<SystemTime>,
}

#[derive(Debug)]
struct Inner {
    state: Mutex<State>,
    changed: Condvar,
}

/// The responder's side of pairing that involves the operator: the PIN it shows, joiners that
/// wait for a typed PIN or an approval, and the throttles. Clone it to share it between the
/// connections and the app's own surface.
///
/// The desk shows nothing and asks nobody: the caller displays [`show_pin`]'s PIN, lists
/// [`slots`], passes what the operator typed to [`submit_code`] or [`find_device`], and calls
/// [`approve`] or [`deny`].
///
/// [`show_pin`]: PairingDesk::show_pin
/// [`slots`]: PairingDesk::slots
/// [`submit_code`]: PairingDesk::submit_code
/// [`find_device`]: PairingDesk::find_device
/// [`approve`]: PairingDesk::approve
/// [`deny`]: PairingDesk::deny
#[derive(Debug, Clone)]
pub struct PairingDesk {
    inner: Arc<Inner>,
}

/// A pairing exchange in progress, counted against [`DeskLimits::pending`] until dropped; its
/// waiting entry goes with it.
#[derive(Debug)]
pub(crate) struct Admission {
    inner: Arc<Inner>,
    id: u64,
    bucket: Option<IpAddr>,
}

impl Admission {
    pub(crate) fn ticket(&self) -> Ticket {
        Ticket(self.id)
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.pending = state.pending.saturating_sub(1);
        if let Some(count) = state.pending_by.get_mut(&self.bucket) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.pending_by.remove(&self.bucket);
            }
        }
        state.waiting.remove(&self.id);
        drop(state);
        self.inner.changed.notify_all();
    }
}

/// Drops the times older than `window` before `now`.
fn prune(times: &mut VecDeque<SystemTime>, now: SystemTime, window: Duration) {
    while times
        .front()
        .is_some_and(|at| now.duration_since(*at).unwrap_or_default() >= window)
    {
        times.pop_front();
    }
}

impl State {
    /// Expires the shown PIN and waiting entries by the clock; whether anything changed.
    fn sweep(&mut self) -> bool {
        let now = self.clock.now();
        let mut changed = false;
        if self
            .shown
            .as_ref()
            .is_some_and(|(shown, _)| now >= shown.expires)
        {
            self.shown = None;
            self.gone = PairRefusal::Expired;
            changed = true;
        }
        for waiting in self.waiting.values_mut() {
            if !waiting.settled && now >= waiting.expires {
                waiting.decision = Some(Decision::Expired);
                waiting.settled = true;
                if let Some(waker) = waiting.waker.take() {
                    waker.wake();
                }
                changed = true;
            }
        }
        // Buckets of past windows go, so the maps stay as large as the recent traffic.
        let (ttl, window) = (
            self.options.pin_ttl.unwrap_or(Duration::MAX),
            self.limits.approver_window,
        );
        self.runs.retain(|_, times| {
            prune(times, now, ttl);
            !times.is_empty()
        });
        self.wrong_by.retain(|_, times| {
            prune(times, now, window);
            !times.is_empty()
        });
        changed
    }

    fn decide(&mut self, ticket: Ticket, decision: Decision) -> Result<(), PairingError> {
        let waiting = self
            .waiting
            .get_mut(&ticket.0)
            .filter(|waiting| !waiting.settled)
            .ok_or(PairingError::Refused(PairRefusal::Expired))?;
        waiting.decision = Some(decision);
        waiting.settled = true;
        if let Some(waker) = waiting.waker.take() {
            waker.wake();
        }
        Ok(())
    }
}

impl PairingDesk {
    /// A desk with the default options and limits, the system clock, and `random` for PINs,
    /// slots and user codes: `PairingDesk::builder().random(random).build()`.
    pub fn new(random: Random) -> Self {
        Self::from_parts(random, Clock::system(), PairingDeskSettings::default())
    }

    fn from_parts(random: Random, clock: Clock, settings: PairingDeskSettings) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    options: settings.options,
                    limits: settings.limits,
                    clock,
                    random,
                    shown: None,
                    gone: PairRefusal::Exhausted,
                    waiting: HashMap::new(),
                    next: 0,
                    pending: 0,
                    pending_by: HashMap::new(),
                    runs: HashMap::new(),
                    wrong_by: HashMap::new(),
                    wrong_all: VecDeque::new(),
                }),
                changed: Condvar::new(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The settings in force.
    pub fn settings(&self) -> PairingDeskSettings {
        let state = self.state();
        PairingDeskSettings {
            options: state.options.clone(),
            limits: state.limits,
        }
    }

    /// Every setting that differs from its default, named by key path.
    pub fn non_defaults(&self) -> Vec<coqui_builder::NonDefault> {
        self.settings().non_defaults()
    }

    /// The options in force.
    pub fn options(&self) -> PairingOptions {
        self.state().options.clone()
    }

    /// The limits in force.
    pub fn limits(&self) -> DeskLimits {
        self.state().limits
    }

    /// Makes a new PIN for joiners to type, replacing the one shown before, and returns it for
    /// the caller to display.
    ///
    /// # Errors
    ///
    /// [`Error::Random`] when the random source fails.
    pub fn show_pin(&self) -> Result<ShownPin, Error> {
        let mut state = self.state();
        let pin = Pin::generate(&state.options.pin_alphabet, &state.random)?;
        let shown = ShownPin {
            pin,
            expires: later(state.clock.now(), state.options.pin_ttl),
        };
        state.shown = Some((shown.clone(), state.options.pin_attempts));
        Ok(shown)
    }

    /// The PIN shown now, while it lasts and has attempts left.
    pub fn shown_pin(&self) -> Option<ShownPin> {
        let mut state = self.state();
        state.sweep();
        state.shown.as_ref().map(|(shown, _)| shown.clone())
    }

    /// Withdraws the shown PIN.
    pub fn clear_pin(&self) {
        let mut state = self.state();
        state.shown = None;
        state.gone = PairRefusal::Exhausted;
    }

    /// The joiners that show their own PIN and wait for it to be typed, by slot.
    pub fn slots(&self) -> Vec<PendingSlot> {
        let mut state = self.state();
        state.sweep();
        let mut slots: Vec<PendingSlot> = state
            .waiting
            .values()
            .filter(|waiting| !waiting.settled)
            .filter_map(|waiting| match waiting.kind {
                Kind::Slot(slot) => Some(PendingSlot {
                    slot,
                    peer: waiting.peer,
                    source: waiting.source,
                    expires: waiting.expires,
                }),
                Kind::Device(_) => None,
            })
            .collect();
        slots.sort_by_key(|slot| slot.slot);
        slots
    }

    /// Hands the PIN the operator typed to the joiner waiting in `code.slot`. Whether it is
    /// right is found by the PAKE; the slot is used either way.
    ///
    /// # Errors
    ///
    /// [`PairingError::Refused`] with [`PairRefusal::WrongCode`] when no joiner waits in that
    /// slot (or `slot` is `None`).
    pub fn submit_code(&self, code: &PairingCode) -> Result<(), PairingError> {
        let mut state = self.state();
        state.sweep();
        let ticket = state
            .waiting
            .iter()
            .find(|(_, waiting)| {
                !waiting.settled
                    && matches!(waiting.kind, Kind::Slot(slot) if Some(slot) == code.slot)
            })
            .map(|(id, _)| Ticket(*id))
            .ok_or(PairingError::Refused(PairRefusal::WrongCode))?;
        state.decide(ticket, Decision::Code(code.pin.clone()))?;
        drop(state);
        self.inner.changed.notify_all();
        Ok(())
    }

    /// The device-flow joiner whose user code the operator entered, for the caller to approve
    /// or deny. `approver` names who entered it (an account, a session), for the per-approver
    /// throttle.
    ///
    /// The code is matched exactly over the user-code alphabet, in constant time against every
    /// pending code.
    ///
    /// # Errors
    ///
    /// [`PairingError::Local`] with [`Error::InvalidCode`] when `code` is not of the alphabet
    /// (not counted as a wrong entry); [`PairRefusal::Throttled`] when the approver or all
    /// approvers entered too many wrong codes recently; [`PairRefusal::WrongCode`] when no
    /// pending joiner has it (counted).
    pub fn find_device(&self, approver: &str, code: &str) -> Result<PendingDevice, PairingError> {
        let mut state = self.state();
        state.sweep();
        let entered = UserCode::parse(&state.options.user_code_alphabet, code)
            .map_err(PairingError::Local)?;
        let now = state.clock.now();
        let (limits, ttl) = (
            state.limits,
            state.options.device_ttl.unwrap_or(Duration::MAX),
        );
        let by = state.wrong_by.entry(approver.to_owned()).or_default();
        prune(by, now, limits.approver_window);
        let by_count = by.len();
        prune(&mut state.wrong_all, now, ttl);
        if by_count >= limits.wrong_codes_per_approver
            || state.wrong_all.len() >= limits.wrong_codes_total
        {
            return Err(PairingError::Refused(PairRefusal::Throttled));
        }
        let mut found = None;
        for (id, waiting) in &state.waiting {
            if let Kind::Device(issued) = &waiting.kind
                && !waiting.settled
                && same(issued.as_str(), entered.as_str())
            {
                found = Some(PendingDevice {
                    peer: waiting.peer,
                    source: waiting.source,
                    label: waiting.label.clone(),
                    expires: waiting.expires,
                    ticket: Ticket(*id),
                });
            }
        }
        match found {
            Some(device) => Ok(device),
            None => {
                state
                    .wrong_by
                    .entry(approver.to_owned())
                    .or_default()
                    .push_back(now);
                state.wrong_all.push_back(now);
                Err(PairingError::Refused(PairRefusal::WrongCode))
            }
        }
    }

    /// Approves a device-flow joiner: its responder accepts its key and tells it so.
    ///
    /// # Errors
    ///
    /// [`PairRefusal::Expired`] when it expired, was decided, or disconnected.
    pub fn approve(&self, device: &PendingDevice) -> Result<(), PairingError> {
        self.settle(device.ticket, Decision::Approved)
    }

    /// Denies a device-flow joiner: its responder refuses it and tells it so.
    ///
    /// # Errors
    ///
    /// As [`approve`](PairingDesk::approve).
    pub fn deny(&self, device: &PendingDevice) -> Result<(), PairingError> {
        self.settle(device.ticket, Decision::Denied)
    }

    fn settle(&self, ticket: Ticket, decision: Decision) -> Result<(), PairingError> {
        let mut state = self.state();
        state.sweep();
        state.decide(ticket, decision)?;
        drop(state);
        self.inner.changed.notify_all();
        Ok(())
    }

    /// Expires the shown PIN and the waiting joiners by the clock, waking their responders.
    /// The waits below also do it; call it when the clock is not the system's.
    pub fn sweep(&self) {
        let changed = self.state().sweep();
        if changed {
            self.inner.changed.notify_all();
        }
    }

    /// Blocks until `ticket` is decided or expires. Under the system clock it rechecks expiry
    /// every second; under another clock, [`sweep`](PairingDesk::sweep) wakes it.
    pub fn wait(&self, ticket: Ticket) -> Decision {
        let mut state = self.state();
        loop {
            state.sweep();
            match state.waiting.get_mut(&ticket.0) {
                None => return Decision::Expired,
                Some(waiting) => {
                    if let Some(decision) = waiting.decision.take() {
                        return decision;
                    }
                }
            }
            state = self
                .inner
                .changed
                .wait_timeout(state, Duration::from_secs(1))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// A future of [`wait`](PairingDesk::wait)'s decision. It is woken by a decision or by
    /// [`sweep`](PairingDesk::sweep); an async caller that relies on expiry sweeps now and then.
    pub fn decided(&self, ticket: Ticket) -> Decided {
        Decided {
            desk: self.clone(),
            ticket,
        }
    }

    /// The random source PINs, slots and codes are drawn from.
    pub(crate) fn random(&self) -> Random {
        self.state().random.clone()
    }

    /// The time on the desk's clock.
    pub(crate) fn now(&self) -> SystemTime {
        self.state().clock.now()
    }

    pub(crate) fn admit(&self, source: Option<SocketAddr>) -> Result<Admission, PairRefusal> {
        let mut state = self.state();
        let bucket = source.map(|s| source_key(s.ip(), state.limits.ipv6_prefix));
        let from_source = state.pending_by.get(&bucket).copied().unwrap_or(0);
        if state.pending >= state.limits.pending || from_source >= state.limits.pending_per_source {
            return Err(PairRefusal::Throttled);
        }
        state.pending += 1;
        *state.pending_by.entry(bucket).or_insert(0) += 1;
        state.next += 1;
        Ok(Admission {
            inner: self.inner.clone(),
            id: state.next,
            bucket,
        })
    }

    /// Counts a PIN run from `source`, or refuses it past the per-source limit.
    pub(crate) fn pin_run(&self, source: Option<SocketAddr>) -> Result<(), PairRefusal> {
        let mut state = self.state();
        let now = state.clock.now();
        let (ttl, limits) = (state.options.pin_ttl.unwrap_or(Duration::MAX), state.limits);
        let bucket = source.map(|s| source_key(s.ip(), limits.ipv6_prefix));
        let runs = state.runs.entry(bucket).or_default();
        prune(runs, now, ttl);
        if runs.len() >= limits.pin_runs_per_source {
            return Err(PairRefusal::Throttled);
        }
        runs.push_back(now);
        Ok(())
    }

    /// Whether a shown PIN is there to try; why not, when not.
    pub(crate) fn pin_ready(&self) -> Result<(), PairRefusal> {
        let mut state = self.state();
        state.sweep();
        if state.shown.is_some() {
            Ok(())
        } else {
            Err(state.gone)
        }
    }

    /// Takes one attempt at the shown PIN, burning it after the last.
    pub(crate) fn take_pin(&self) -> Result<Pin, PairRefusal> {
        let mut state = self.state();
        state.sweep();
        let gone = state.gone;
        let (shown, left) = state.shown.as_mut().ok_or(gone)?;
        let pin = shown.pin.clone();
        *left -= 1;
        if *left == 0 {
            state.shown = None;
            state.gone = PairRefusal::Exhausted;
        }
        Ok(pin)
    }

    pub(crate) fn open_slot(
        &self,
        admission: &Admission,
        peer: PeerId,
        source: Option<SocketAddr>,
    ) -> Result<u8, PairingError> {
        let mut state = self.state();
        state.sweep();
        let used: Vec<u8> = state
            .waiting
            .values()
            .filter_map(|waiting| match waiting.kind {
                Kind::Slot(slot) => Some(slot),
                Kind::Device(_) => None,
            })
            .collect();
        let free: Vec<u8> = (0..100).filter(|slot| !used.contains(slot)).collect();
        if free.is_empty() {
            return Err(PairingError::Refused(PairRefusal::Throttled));
        }
        let slot = free[below(&state.random, free.len()).map_err(PairingError::Local)?];
        let expires = later(state.clock.now(), state.options.pin_ttl);
        state.waiting.insert(
            admission.id,
            Waiting {
                kind: Kind::Slot(slot),
                peer,
                source,
                label: String::new(),
                expires,
                decision: None,
                settled: false,
                waker: None,
            },
        );
        Ok(slot)
    }

    pub(crate) fn issue(
        &self,
        admission: &Admission,
        peer: PeerId,
        source: Option<SocketAddr>,
        label: &str,
    ) -> Result<(UserCode, SystemTime), PairingError> {
        let mut state = self.state();
        state.sweep();
        let issued = state
            .waiting
            .values()
            .filter(|waiting| matches!(waiting.kind, Kind::Device(_)))
            .count();
        // A small alphabet can run out of distinct codes, so drawing is bounded.
        if issued as u128 >= state.options.user_code_alphabet.combinations() {
            return Err(PairingError::Refused(PairRefusal::Throttled));
        }
        let mut code = None;
        for _ in 0..ISSUE_DRAWS {
            let drawn = UserCode::generate(&state.options.user_code_alphabet, &state.random)
                .map_err(PairingError::Local)?;
            let taken = state
                .waiting
                .values()
                .any(|waiting| matches!(&waiting.kind, Kind::Device(issued) if issued == &drawn));
            if !taken {
                code = Some(drawn);
                break;
            }
        }
        let code = code.ok_or(PairingError::Refused(PairRefusal::Throttled))?;
        let expires = later(state.clock.now(), state.options.device_ttl);
        state.waiting.insert(
            admission.id,
            Waiting {
                kind: Kind::Device(code.clone()),
                peer,
                source,
                label: label.to_owned(),
                expires,
                decision: None,
                settled: false,
                waker: None,
            },
        );
        Ok((code, expires))
    }
}

/// The future of [`PairingDesk::decided`].
#[derive(Debug)]
pub struct Decided {
    desk: PairingDesk,
    ticket: Ticket,
}

impl Future for Decided {
    type Output = Decision;

    fn poll(self: Pinned<&mut Self>, context: &mut Context<'_>) -> Poll<Decision> {
        let mut state = self.desk.state();
        state.sweep();
        match state.waiting.get_mut(&self.ticket.0) {
            None => Poll::Ready(Decision::Expired),
            Some(waiting) => match waiting.decision.take() {
                Some(decision) => Poll::Ready(decision),
                None => {
                    waiting.waker = Some(context.waker().clone());
                    Poll::Pending
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU64, Ordering};
    use std::time::UNIX_EPOCH;

    #[test]
    fn the_buckets_of_past_windows_are_dropped() {
        let now = Arc::new(AtomicU64::new(1_800_000_000));
        let seen = now.clone();
        let clock =
            Clock::from_fn(move || UNIX_EPOCH + Duration::from_secs(seen.load(Ordering::SeqCst)));
        let limits = DeskLimits {
            wrong_codes_total: 1000,
            ..DeskLimits::default()
        };
        let desk = PairingDesk::builder()
            .random(Random::from_fn(|out| {
                out.fill(7);
                Ok(())
            }))
            .clock(clock)
            .limits(limits)
            .build()
            .unwrap();
        for host in 0..200u8 {
            let source = SocketAddr::from(([192, 0, 2, host], 1000));
            desk.pin_run(Some(source)).unwrap();
            let _ = desk.find_device(&format!("approver {host}"), "BBBBBBBB");
        }
        assert_eq!(desk.state().runs.len(), 200);
        assert_eq!(desk.state().wrong_by.len(), 200);
        now.fetch_add(600, Ordering::SeqCst);
        desk.sweep();
        assert!(desk.state().runs.is_empty());
        assert!(desk.state().wrong_by.is_empty());
    }
}
