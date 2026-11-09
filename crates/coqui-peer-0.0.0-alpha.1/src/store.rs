use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::fmt;
use core::net::{IpAddr, Ipv6Addr, SocketAddr};
use core::str::FromStr;
use core::time::Duration;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Instant, SystemTime};

use data_encoding::{HEXLOWER, HEXLOWER_PERMISSIVE};
use sha2::{Digest as _, Sha256};

use crate::clock::{Clock, from_unix_seconds, unix_seconds};
use crate::error::Error;
use crate::file::{self, Access};
use crate::id::PeerId;
use crate::key::KeyMaterial;
use crate::non_default::NonDefault;
use crate::pem;
use crate::random::Random;
use crate::waiting::{WaitingQueue, WaitingSettings, WhenFull};

/// The version of the on-disk format [`TrustStore`] writes and reads.
pub const FORMAT_VERSION: u32 = 1;

/// The longest invitation label, in bytes.
pub const MAX_LABEL: usize = 128;

/// Rotation events kept for [`TrustStore::take_rotation_events`]; older ones are dropped.
const MAX_EVENTS: usize = 1024;

/// The IPv6 prefix length a waiting source is grouped by, unless the store is given another:
/// one /64 is the usual end-site subnet, and its holder can use any address in it.
pub(crate) const DEFAULT_IPV6_PREFIX: u8 = 64;

/// Bounds on the waiting queue.
///
/// A guatiao value: every key is optional (an absent one is its default) and an unknown key is
/// refused. `rerecord` is a duration: a number of seconds, `[secs, nanos]` or `{secs, nanos}`. The
/// struct is `#[non_exhaustive]`: start from [`Default`] and assign the fields.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, guatiao::ToValue, guatiao::FromValue, guatiao::Schema,
)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct WaitingLimits {
    /// Entries one source may hold; past it, a newcomer is refused unless the store says otherwise
    /// ([`WhenFull`]). A source is an IPv4 address, or an IPv6 prefix
    /// (/64 by default).
    #[map(default = WaitingLimits::default().per_source)]
    pub per_source: usize,
    /// Entries the whole directory may hold; past it nothing is recorded.
    #[map(default = WaitingLimits::default().total)]
    pub total: usize,
    /// How long the same peer from the same source is not recorded again.
    #[map(default = WaitingLimits::default().rerecord)]
    pub rerecord: Duration,
}

impl Default for WaitingLimits {
    fn default() -> Self {
        Self {
            per_source: 16,
            total: 1_000,
            rerecord: Duration::from_secs(60),
        }
    }
}

/// How an accepted peer's key is replaced by its successor.
///
/// A guatiao value like [`WaitingLimits`]; `retire_after` is a duration. `#[non_exhaustive]`:
/// start from [`Default`] and assign the fields.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, guatiao::ToValue, guatiao::FromValue, guatiao::Schema,
)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct RotationOptions {
    /// How long the old key stays accepted once its successor is promoted. Default 24 hours.
    #[map(default = RotationOptions::default().retire_after)]
    pub retire_after: Duration,
    /// Whether a successor waits for [`TrustStore::promote_successor`] instead of being
    /// promoted by the method the first time the peer proves it. Default `false`.
    #[map(default = RotationOptions::default().require_approval)]
    pub require_approval: bool,
    /// Whether [`TrustStore::promote_successor`] promotes a successor nobody proved
    /// ([`TrustStore::prove_successor`]): the old key's holder then vouches alone for a key
    /// that may not be its own. Default `false`.
    #[map(default = RotationOptions::default().allow_unproven)]
    pub allow_unproven: bool,
}

impl Default for RotationOptions {
    fn default() -> Self {
        Self {
            retire_after: Duration::from_secs(24 * 3600),
            require_approval: false,
            allow_unproven: false,
        }
    }
}

/// How [`TrustStore::invite`] makes an invitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct InvitationOptions {
    /// How long the invitation can be redeemed, in whole seconds. Default 24 hours; zero is an
    /// error.
    pub valid_for: Duration,
    /// How many times it can be redeemed. Default 1; zero is an error.
    pub uses: u32,
}

impl Default for InvitationOptions {
    fn default() -> Self {
        Self {
            valid_for: Duration::from_secs(24 * 3600),
            uses: 1,
        }
    }
}

/// A peer waiting for acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct WaitingEntry {
    /// The peer's id.
    pub id: PeerId,
    /// The address the handshake came from; `None` when the caller did not know it.
    pub source: Option<SocketAddr>,
}

/// The id of an invitation: the SHA-256 of its secret, and the name of its file. Knowing it
/// does not let anyone redeem the invitation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InvitationId([u8; 32]);

impl InvitationId {
    /// The id of the invitation whose secret is `secret`.
    pub fn of(secret: &[u8; 32]) -> Self {
        Self(Sha256::digest(secret).into())
    }

    /// 64 lowercase hexadecimal characters.
    pub fn hex(&self) -> String {
        HEXLOWER.encode(&self.0)
    }
}

impl fmt::Display for InvitationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.hex())
    }
}

impl fmt::Debug for InvitationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "InvitationId({})", self.hex())
    }
}

impl FromStr for InvitationId {
    type Err = Error;

    /// 64 hexadecimal characters.
    fn from_str(text: &str) -> Result<Self, Error> {
        let text = text.trim();
        let mut bytes = [0u8; 32];
        if text.len() != 64 {
            return Err(Error::InvalidId);
        }
        HEXLOWER_PERMISSIVE
            .decode_mut(text.as_bytes(), &mut bytes)
            .map_err(|_| Error::InvalidId)?;
        Ok(Self(bytes))
    }
}

/// A one-time pairing secret: whoever presents it may pair, until it expires or its uses run
/// out.
///
/// The store keeps only the [`id`](Invitation::id), never the secret: the caller hands the
/// secret to the joiner (a link or QR code, with the inviter's id). `Debug` does not show it.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Invitation {
    /// The secret, 32 random bytes.
    pub secret: [u8; 32],
    /// When it stops being redeemable.
    pub expires: SystemTime,
    /// Redemptions left: as made by [`TrustStore::invite`], or after this one by
    /// [`TrustStore::redeem`].
    pub uses: u32,
    /// The inviter's note on whom it is for, one line of at most [`MAX_LABEL`] bytes.
    pub label: String,
}

impl Invitation {
    /// The invitation's id.
    pub fn id(&self) -> InvitationId {
        InvitationId::of(&self.secret)
    }
}

impl Drop for Invitation {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.secret);
    }
}

impl fmt::Debug for Invitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Invitation")
            .field("id", &self.id())
            .field("expires", &self.expires)
            .field("uses", &self.uses)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// An open invitation, as listed: everything but the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InvitationEntry {
    /// Its id.
    pub id: InvitationId,
    /// When it stops being redeemable.
    pub expires: SystemTime,
    /// Redemptions left.
    pub uses: u32,
    /// The inviter's note.
    pub label: String,
}

/// A pending successor: `new` was announced by the peer accepted as `old`, and is accepted once
/// the peer proves it (or the caller promotes it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Successor {
    /// The accepted key that announced it.
    pub old: PeerId,
    /// The announced key.
    pub new: PeerId,
    /// When its holder proved it in a handshake ([`TrustStore::prove_successor`]), if yet.
    pub proven: Option<SystemTime>,
}

/// An accepted key with a retirement time: it is refused from `retires` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Retiring {
    /// The retiring key.
    pub id: PeerId,
    /// The key that replaced it, when it was replaced by rotation.
    pub successor: Option<PeerId>,
    /// When it stops being accepted.
    pub retires: SystemTime,
}

/// A change of a peer's key, for the caller to report as it chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RotationEvent {
    /// The peer accepted as `old` announced `new` as its successor.
    Announced {
        /// The announcing key.
        old: PeerId,
        /// The announced key.
        new: PeerId,
    },
    /// `new` was promoted: it is accepted, and `old` retires at `retires`.
    Promoted {
        /// The replaced key.
        old: PeerId,
        /// The key now accepted.
        new: PeerId,
        /// When `old` stops being accepted.
        retires: SystemTime,
    },
}

/// Persistent trust: peers an operator accepted, peers that were turned away and wait to be
/// accepted (pairing), open invitations, and announced successor keys.
///
/// # Pairing
///
/// 1. A server turns away a peer it does not trust, **after the peer proved it holds the
///    key it presented**, and records its key with [`record_waiting`](TrustStore::record_waiting).
/// 2. The operator lists [`waiting`](TrustStore::waiting) and either
///    [`accept`](TrustStore::accept)s an entry by id or [`remove`](TrustStore::remove)s it.
/// 3. The next handshake finds the peer [`is_accepted`](TrustStore::is_accepted).
///
/// **Every method must meet this contract:** nothing is recorded as waiting until the peer
/// has shown, inside the handshake, that it holds the private key of the public key it
/// presents (a signature over the handshake transcript, a key exchange that only the key's
/// holder can complete, ...). Presenting a key proves nothing: anyone can copy a public
/// certificate, and a store that records unproven keys lets a stranger fill the queue with
/// somebody else's identity, or put a real peer's id in front of the operator under a source
/// address that is not theirs. A handshake that fails to prove the key records nothing.
///
/// An [`invitation`](TrustStore::invite) lets a joiner pair without waiting: it presents the
/// secret over a channel that proves its key, and the inviter [`redeem`](TrustStore::redeem)s
/// it and accepts the key.
///
/// # Rotation
///
/// A peer accepted as key A announces its successor B over a session A authenticated
/// ([`announce_successor`](TrustStore::announce_successor)). The first time the peer proves B
/// in a handshake, the method [`promote_successor`](TrustStore::promote_successor)s it, unless
/// [`RotationOptions::require_approval`] leaves that to the caller; A is then accepted until
/// [`RotationOptions::retire_after`] has passed, and refused from then on. Removing A removes
/// its pending successor. Each step is queued as a [`RotationEvent`].
///
/// # Key material and methods
///
/// The store keeps the bytes a method hands it as a [`KeyMaterial`] and never interprets
/// them: a PEM certificate for `tls`, another method's own key for it. Each file records the
/// method name, so a store can hold the keys of several methods side by side, and a method
/// reads only the entries that name it. The id of an entry is the SHA-256 of its bytes; the
/// store checks that when it reads a file.
///
/// # On-disk format (version 1)
///
/// Under `<state_dir>/peers/`:
///
/// ```text
/// accepted/<64 lowercase hex>.pem     one PEM block; the name is the peer's id
/// waiting/<64 lowercase hex>.pem      the same, for a peer that proved its key and was refused
/// successors/<64 lowercase hex>.pem   an announced successor key, named by its id
/// invitations/<64 lowercase hex>.inv  an open invitation, named by the SHA-256 of its secret
/// ```
///
/// Before the PEM block a file may hold header lines (PEM allows explanatory text there):
/// `Method: <name>`; in a waiting file `Source: <ip>:<port>` (IPv6 as `[addr]:port`), the
/// address the handshake came from; in an accepted file `Successor: <64 hex>` (the announced
/// or promoting successor) and `Retires: <unix seconds>`; in a successor file
/// `Predecessor: <64 hex>`. An invitation file is header lines only: `Expires: <unix
/// seconds>`, `Uses: <count>` and, when not empty, `Label: <text>`; it never holds the
/// secret. A key file without a `Method:` line, or named otherwise, is not read. Accepting moves a file from `waiting/` to
/// `accepted/`, rewritten without the `Source:` line.
#[derive(Debug, Clone)]
pub struct TrustStore {
    root: PathBuf,
    waiting: Arc<Mutex<WaitingQueue>>,
    events: Arc<Mutex<VecDeque<RotationEvent>>>,
    clock: Clock,
    rotation: RotationOptions,
}

/// What a store file holds.
struct Entry {
    key: KeyMaterial,
    source: Option<SocketAddr>,
    successor: Option<PeerId>,
    retires: Option<u64>,
    predecessor: Option<PeerId>,
    proven: Option<u64>,
}

/// What a [`TrustStore`] is built from, besides the directory and the clock, in the form a
/// file, a form or a peer fills.
///
/// Every key is optional (an absent one is its default) and an unknown key is refused. The type
/// is a guatiao value (`from_value`, `to_value`, `schema`), and `guatiao_serde::text` reads and
/// writes its text. It is `#[non_exhaustive]`: start from [`Default`] and assign the fields.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    guatiao::ToValue,
    guatiao::FromValue,
    guatiao::Schema,
)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct TrustStoreSettings {
    /// The waiting queue: its limits, how sources are grouped and what a full source does.
    #[map(default = TrustStoreSettings::default().waiting)]
    pub waiting: WaitingSettings,
    /// How an accepted peer's key is replaced by its successor.
    #[map(default = TrustStoreSettings::default().rotation)]
    pub rotation: RotationOptions,
}

impl TrustStoreSettings {
    /// Every setting that differs from its default, named by key path (`waiting.limits.total`).
    pub fn non_defaults(&self) -> Vec<NonDefault> {
        NonDefault::of_settings(self)
    }
}

/// Builds a [`TrustStore`]: [`TrustStore::builder`], the settings, then [`build`](Self::build).
#[derive(Debug, Clone)]
#[must_use]
pub struct TrustStoreBuilder {
    state_dir: PathBuf,
    settings: TrustStoreSettings,
    clock: Clock,
}

impl TrustStoreBuilder {
    /// The bounds on the waiting queue. Default 16 per source, 1000 in all, 60 seconds.
    pub fn limits(mut self, limits: WaitingLimits) -> Self {
        self.settings.waiting.limits = limits;
        self
    }

    /// Group IPv6 sources by their first `bits` bits for the per-source limits. Default 64. 128
    /// counts each address alone, which lets the holder of a subnet fill the queue; values above
    /// 128 are 128. IPv4 sources, IPv4-mapped IPv6 ones included, always count per address.
    pub fn ipv6_prefix(mut self, bits: u8) -> Self {
        self.settings.waiting.ipv6_prefix = bits;
        self
    }

    /// What a newcomer to a full source gets. Default [`WhenFull::Refuse`], so that strangers
    /// sharing a source cannot flush those already waiting.
    pub fn when_full(mut self, when_full: WhenFull) -> Self {
        self.settings.waiting.when_full = when_full;
        self
    }

    /// Record a new peer from one source at most once per `interval`, each record being a synced
    /// write a stranger can cause. Default one second; zero is no limit.
    pub fn record_interval(mut self, interval: Duration) -> Self {
        self.settings.waiting.record_interval = interval;
        self
    }

    /// How keys are rotated. Default 24 hours, promoted automatically.
    pub fn rotation(mut self, options: RotationOptions) -> Self {
        self.settings.rotation = options;
        self
    }

    /// The clock invitation expiry and key retirement read. Default the system clock.
    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// Every setting from `settings`. Replaces the limits, the grouping, the full-source
    /// behaviour, the record interval and the rotation set before, and is replaced by what is set
    /// after it. The clock is code, not a setting, and stays.
    pub fn settings(mut self, settings: TrustStoreSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Opens the store (creating the directories when absent).
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the directories cannot be created.
    pub fn build(self) -> Result<TrustStore, Error> {
        let root = self.state_dir.join("peers");
        fs::create_dir_all(root.join("accepted")).map_err(Error::Io)?;
        fs::create_dir_all(root.join("waiting")).map_err(Error::Io)?;
        Ok(TrustStore {
            root,
            waiting: Arc::new(Mutex::new(
                WaitingQueue::builder(self.settings.waiting.limits)
                    .settings(self.settings.waiting)
                    .build(),
            )),
            events: Arc::default(),
            clock: self.clock,
            rotation: self.settings.rotation,
        })
    }
}

impl TrustStore {
    /// A builder for the store under `<state_dir>/peers`, with every setting at its default and
    /// the system clock.
    pub fn builder(state_dir: &Path) -> TrustStoreBuilder {
        TrustStoreBuilder {
            state_dir: state_dir.to_path_buf(),
            settings: TrustStoreSettings::default(),
            clock: Clock::system(),
        }
    }

    /// Opens (creating when absent) the store under `<state_dir>/peers` with every setting at its
    /// default: `TrustStore::builder(state_dir).build()`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the directories cannot be created.
    pub fn open(state_dir: &Path) -> Result<Self, Error> {
        Self::builder(state_dir).build()
    }

    /// This store with a new, empty waiting queue under `settings`, the replacement for the queue it
    /// was built with. Rotation and the clock stay.
    #[must_use]
    pub fn waiting_queue(self, settings: WaitingSettings) -> Self {
        self.reconfigured(|current| current.waiting = settings)
    }

    /// The waiting queue's bounds and what this process recorded.
    fn queue(&self) -> std::sync::MutexGuard<'_, WaitingQueue> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// This store under its settings changed by `change`, with a new empty waiting queue.
    fn reconfigured(mut self, change: impl FnOnce(&mut TrustStoreSettings)) -> Self {
        let mut settings = self.settings();
        change(&mut settings);
        self.waiting = Arc::new(Mutex::new(
            WaitingQueue::builder(settings.waiting.limits)
                .settings(settings.waiting)
                .build(),
        ));
        self.rotation = settings.rotation;
        self
    }

    /// The settings it was built with.
    pub fn settings(&self) -> TrustStoreSettings {
        TrustStoreSettings {
            waiting: self.queue().settings(),
            rotation: self.rotation,
        }
    }

    /// Every setting that differs from its default, named by key path.
    pub fn non_defaults(&self) -> Vec<NonDefault> {
        self.settings().non_defaults()
    }

    /// The rotation options in force.
    pub fn rotation(&self) -> RotationOptions {
        self.rotation
    }

    /// The clock in force.
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// Peers awaiting operator acceptance, sorted by id, each with the address it connected
    /// from. Every entry proved it holds its key.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the directory cannot be read.
    pub fn waiting(&self) -> Result<Vec<WaitingEntry>, Error> {
        Ok(self
            .list("waiting")?
            .into_iter()
            .map(|id| WaitingEntry {
                id,
                source: self.read("waiting", &id).and_then(|entry| entry.source),
            })
            .collect())
    }

    /// Accepted ids, sorted; a key past its retirement time is not listed.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the directory cannot be read.
    pub fn accepted(&self) -> Result<Vec<PeerId>, Error> {
        Ok(self
            .list("accepted")?
            .into_iter()
            .filter(|id| self.current(id).is_some())
            .collect())
    }

    /// The key material of an accepted peer, when it is accepted, not retired, and its file is
    /// sound.
    pub fn accepted_key(&self, id: &PeerId) -> Option<KeyMaterial> {
        self.current(id).map(|entry| entry.key)
    }

    /// The key material of a waiting peer, when it is waiting and its file is sound.
    pub fn waiting_key(&self, id: &PeerId) -> Option<KeyMaterial> {
        self.read("waiting", id).map(|entry| entry.key)
    }

    /// Accepts a waiting peer by its id.
    ///
    /// # Errors
    ///
    /// [`Error::NotWaiting`], [`Error::AlreadyAccepted`], or [`Error::Material`] when the
    /// stored key does not hash to the name.
    pub fn accept(&self, id: &PeerId) -> Result<(), Error> {
        if self.is_accepted(id) {
            return Err(Error::AlreadyAccepted);
        }
        let path = self.existing("waiting", id).ok_or(Error::NotWaiting)?;
        let text = fs::read_to_string(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Error::NotWaiting
            } else {
                Error::Io(error)
            }
        })?;
        let entry = parse(&text).ok_or(Error::Material)?;
        if entry.key.id() != *id {
            return Err(Error::Material);
        }
        self.write_current(&entry.key)?;
        let _ = fs::remove_file(path);
        Ok(())
    }

    /// Accepts a key directly, without it having waited.
    ///
    /// # Errors
    ///
    /// [`Error::Material`] when the key is not valid, [`Error::AlreadyAccepted`] when it is
    /// accepted and current, [`Error::Io`] on write failure. A key past its retirement time is
    /// accepted again, its file rewritten without `Successor:` and `Retires:`.
    pub fn accept_key(&self, key: &KeyMaterial) -> Result<PeerId, Error> {
        key.check()?;
        let id = key.id();
        self.write_current(key)?;
        let _ = fs::remove_file(self.path("waiting", &id));
        Ok(id)
    }

    /// Removes an accepted, waiting or pending-successor peer, and the pending successor an
    /// accepted one announced; `true` when something was removed.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when a file cannot be removed.
    pub fn remove(&self, id: &PeerId) -> Result<bool, Error> {
        let mut removed = false;
        if let Some(successor) = self.read("accepted", id).and_then(|entry| entry.successor)
            && self
                .read("successors", &successor)
                .is_some_and(|entry| entry.predecessor == Some(*id))
        {
            fs::remove_file(self.path("successors", &successor)).map_err(Error::Io)?;
        }
        for section in ["accepted", "waiting", "successors"] {
            if let Some(path) = self.existing(section, id) {
                fs::remove_file(path).map_err(Error::Io)?;
                removed = true;
            }
        }
        Ok(removed)
    }

    /// Whether `id` is accepted, not past its retirement time, and its stored key really hashes
    /// to it.
    pub fn is_accepted(&self, id: &PeerId) -> bool {
        self.current(id).is_some()
    }

    /// Records a turned-away peer's key as waiting. Failures are not reported to the peer.
    ///
    /// **Call this only after the peer proved it holds the key** (see the
    /// [pairing contract](TrustStore#pairing)).
    ///
    /// Bounded: invalid keys are ignored; one source holds at most
    /// [`per_source`](WaitingLimits::per_source) entries, a newcomer being refused
    /// ([`WhenFull`]); it records a new peer at most once per
    /// [`record_interval`](TrustStoreBuilder::record_interval); the same
    /// peer from the same source is recorded at most once per
    /// [`rerecord`](WaitingLimits::rerecord); the directory holds at most
    /// [`total`](WaitingLimits::total). A source is an IPv4 address (IPv4-mapped IPv6 included)
    /// or an IPv6 prefix of [`ipv6_prefix`](TrustStoreBuilder::ipv6_prefix) bits, /64 by
    /// default. The full `source` address is stored with the entry. `None` is the source of
    /// callers that do not know it, which share one bucket.
    pub fn record_waiting(&self, key: &KeyMaterial, source: Option<SocketAddr>) {
        if key.check().is_err() {
            return;
        }
        let id = key.id();
        if self.existing("waiting", &id).is_some() {
            return;
        }
        let Ok(stored) = self.list("waiting") else {
            return;
        };
        let now = Instant::now();
        let mut queue = self.queue();
        // Another process (an operator's command) may have accepted or removed entries.
        queue.prune(now, |had| stored.binary_search(had).is_ok());
        let admission = queue.admit(id, source, stored.len(), now);
        if !admission.record {
            return;
        }
        for old in &admission.evict {
            let _ = fs::remove_file(self.path("waiting", old));
        }
        let mut text = source.map_or_else(String::new, |s| format!("Source: {s}\n"));
        text.push_str(&format!("Method: {}\n", key.method));
        text.push_str(&pem::encode(&key.label, &key.bytes));
        let _ = write_new(&self.path("waiting", &id), &text);
    }

    /// Makes an invitation with a secret from `random` and records it (without the secret).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOptions`] when `label` is not one line of at most [`MAX_LABEL`]
    /// bytes without surrounding spaces, `uses` is 0 or `valid_for` is zero;
    /// [`Error::Random`] when `random` fails; [`Error::Io`] when the file cannot be written.
    pub fn invite(
        &self,
        random: &Random,
        options: InvitationOptions,
        label: &str,
    ) -> Result<Invitation, Error> {
        let label_ok = label.len() <= MAX_LABEL
            && label.trim() == label
            && !label.chars().any(char::is_control);
        if !label_ok || options.uses == 0 || options.valid_for.is_zero() {
            return Err(Error::InvalidOptions);
        }
        let mut secret = [0u8; 32];
        random.fill(&mut secret)?;
        let expires = from_unix_seconds(
            unix_seconds(self.clock.now()).saturating_add(options.valid_for.as_secs()),
        );
        let invitation = Invitation {
            secret,
            expires,
            uses: options.uses,
            label: label.to_owned(),
        };
        fs::create_dir_all(self.root.join("invitations")).map_err(Error::Io)?;
        write_new(
            &self.invitation_path(&invitation.id()),
            &invitation_text(expires, options.uses, label),
        )
        .map_err(Error::Io)?;
        Ok(invitation)
    }

    /// Redeems an invitation by its secret, using one of its uses: the invitation with the uses
    /// left after this one. The last use removes it.
    ///
    /// Atomic: of concurrent redemptions of a single-use invitation exactly one succeeds. An
    /// expired invitation is removed.
    ///
    /// # Errors
    ///
    /// [`Error::NotInvited`] when no open invitation has this secret, [`Error::Expired`] when
    /// it expired, [`Error::Io`] when its file cannot be read or moved.
    pub fn redeem(&self, secret: &[u8; 32]) -> Result<Invitation, Error> {
        let path = self.invitation_path(&InvitationId::of(secret));
        let claimed = file::temporary_path(&path);
        // Renaming is the claim: only one redeemer can move the file away.
        if let Err(error) = fs::rename(&path, &claimed) {
            return Err(if error.kind() == io::ErrorKind::NotFound {
                Error::NotInvited
            } else {
                Error::Io(error)
            });
        }
        let text = fs::read_to_string(&claimed).ok();
        // Removing the claimed file decides the claim. A rename is not enough: on Windows a
        // redeemer that opened the file before another moved it can still move it again, but a
        // file is removed only once.
        if file::retried(|| fs::remove_file(&claimed)).is_err() {
            return Err(Error::NotInvited);
        }
        let (expires, uses, label) = text
            .as_deref()
            .and_then(parse_invitation)
            .ok_or(Error::NotInvited)?;
        let revoked = self.revoked_path(&InvitationId::of(secret));
        if revoked.exists() {
            return Err(Error::NotInvited);
        }
        if self.clock.now() >= from_unix_seconds(expires) {
            return Err(Error::Expired);
        }
        let left = uses - 1;
        if left > 0 {
            // Losing the remaining uses on a write failure fails closed.
            let _ = write_new(
                &path,
                &invitation_text(from_unix_seconds(expires), left, &label),
            );
            // A revocation that came while the file was claimed wins: either it sees this
            // file and removes it, or this sees its record and does.
            if revoked.exists() {
                let _ = fs::remove_file(&path);
                return Err(Error::NotInvited);
            }
        }
        Ok(Invitation {
            secret: *secret,
            expires: from_unix_seconds(expires),
            uses: left,
            label,
        })
    }

    /// The open invitations, sorted by id, expired ones included until redeemed or revoked.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the directory cannot be read.
    pub fn invitations(&self) -> Result<Vec<InvitationEntry>, Error> {
        let directory = self.root.join("invitations");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(Error::Io(error)),
        };
        let mut found = Vec::new();
        for entry in entries {
            let path = entry.map_err(Error::Io)?.path();
            let Some(id) = path
                .extension()
                .filter(|extension| *extension == "inv")
                .and_then(|_| path.file_stem())
                .and_then(|stem| stem.to_str())
                .filter(|stem| is_hex_name(stem))
                .and_then(|stem| stem.parse::<InvitationId>().ok())
            else {
                continue;
            };
            if let Some((expires, uses, label)) = fs::read_to_string(&path)
                .ok()
                .as_deref()
                .and_then(parse_invitation)
            {
                found.push(InvitationEntry {
                    id,
                    expires: from_unix_seconds(expires),
                    uses,
                    label,
                });
            }
        }
        found.sort_by_key(|entry| entry.id);
        Ok(found)
    }

    /// Revokes an invitation; `true` when it was open. A redemption in flight when it is
    /// revoked fails, and its remaining uses are not restored: a `<64 hex>.revoked` file
    /// records the revocation before the invitation file goes.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the revocation cannot be recorded or the file removed.
    pub fn revoke_invitation(&self, id: &InvitationId) -> Result<bool, Error> {
        fs::create_dir_all(self.root.join("invitations")).map_err(Error::Io)?;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.revoked_path(id))
            .map_err(Error::Io)?;
        match file::retried(|| fs::remove_file(self.invitation_path(id))) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(Error::Io(error)),
        }
    }

    /// Records `new` as the pending successor of the accepted key `old`, replacing a successor
    /// `old` announced before. Call it only with a key the holder of `old` sent over a session
    /// `old` authenticated.
    ///
    /// # Errors
    ///
    /// [`Error::NotAccepted`] when `old` is not accepted or is already retiring;
    /// [`Error::AlreadyAccepted`] when `new` is accepted; [`Error::SuccessorTaken`] when `new` is
    /// the pending successor of another key that is still accepted; [`Error::Material`] when `new` is not
    /// valid or is `old`; [`Error::Io`] on write failure.
    pub fn announce_successor(&self, old: &PeerId, new: &KeyMaterial) -> Result<(), Error> {
        new.check()?;
        let new_id = new.id();
        if new_id == *old {
            return Err(Error::Material);
        }
        let entry = self
            .current(old)
            .filter(|entry| entry.retires.is_none())
            .ok_or(Error::NotAccepted)?;
        if self.existing("accepted", &new_id).is_some() {
            return Err(Error::AlreadyAccepted);
        }
        // Another accepted key that announced `new` keeps it: overwriting would let a peer that
        // learned someone else's successor claim it, and promoting it would retire the wrong key.
        if self.successor_key(&new_id).is_some()
            && self
                .read("successors", &new_id)
                .and_then(|pending| pending.predecessor)
                .is_some_and(|predecessor| predecessor != *old)
        {
            return Err(Error::SuccessorTaken);
        }
        if let Some(previous) = entry.successor
            && previous != new_id
            && self
                .read("successors", &previous)
                .is_some_and(|pending| pending.predecessor == Some(*old))
        {
            let _ = fs::remove_file(self.path("successors", &previous));
        }
        fs::create_dir_all(self.root.join("successors")).map_err(Error::Io)?;
        let text = format!(
            "Predecessor: {}\nMethod: {}\n{}",
            old.hex(),
            new.method,
            pem::encode(&new.label, &new.bytes)
        );
        replace(&self.path("successors", &new_id), &text).map_err(Error::Io)?;
        let path = self.existing("accepted", old).ok_or(Error::NotAccepted)?;
        replace(&path, &accepted_text(&entry.key, Some(&new_id), None)).map_err(Error::Io)?;
        self.push_event(RotationEvent::Announced {
            old: *old,
            new: new_id,
        });
        Ok(())
    }

    /// The key of `new` when it is the pending successor of a key that is still accepted, not
    /// retiring, and names it.
    pub fn successor_key(&self, new: &PeerId) -> Option<KeyMaterial> {
        let pending = self.read("successors", new)?;
        let old = pending.predecessor?;
        self.current(&old)
            .filter(|entry| entry.retires.is_none() && entry.successor == Some(*new))
            .map(|_| pending.key)
    }

    /// Records that the holder of the pending successor `new` proved it in a handshake (a
    /// `Proven:` line with the time). A method calls it after the handshake signature verifies,
    /// whether or not it then promotes. The first proof's time is kept.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuccessor`] when `new` is not the pending successor of an accepted key;
    /// [`Error::Io`] on write failure.
    pub fn prove_successor(&self, new: &PeerId) -> Result<(), Error> {
        let key = self.successor_key(new).ok_or(Error::NoSuccessor)?;
        let pending = self.read("successors", new).ok_or(Error::NoSuccessor)?;
        if pending.proven.is_some() {
            return Ok(());
        }
        let old = pending.predecessor.ok_or(Error::NoSuccessor)?;
        let text = format!(
            "Predecessor: {}
Method: {}
Proven: {}
{}",
            old.hex(),
            key.method,
            unix_seconds(self.clock.now()),
            pem::encode(&key.label, &key.bytes)
        );
        replace(&self.path("successors", new), &text).map_err(Error::Io)
    }

    /// Accepts the pending successor `new` and gives its predecessor a retirement time
    /// ([`RotationOptions::retire_after`] from now). Returns the predecessor's id. A method calls
    /// it once the peer proved `new`, unless [`RotationOptions::require_approval`] is set; the
    /// caller calls it to approve.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuccessor`] when `new` is not the pending successor of an accepted key;
    /// [`Error::NotProven`] when nobody proved it and [`RotationOptions::allow_unproven`] is not
    /// set; [`Error::Io`] on write failure.
    pub fn promote_successor(&self, new: &PeerId) -> Result<PeerId, Error> {
        let key = self.successor_key(new).ok_or(Error::NoSuccessor)?;
        if !self.rotation.allow_unproven
            && self
                .read("successors", new)
                .is_none_or(|pending| pending.proven.is_none())
        {
            return Err(Error::NotProven);
        }
        let old = self
            .read("successors", new)
            .and_then(|entry| entry.predecessor)
            .ok_or(Error::NoSuccessor)?;
        let old_entry = self.current(&old).ok_or(Error::NoSuccessor)?;
        match self.write_accepted(&key) {
            Ok(()) | Err(Error::AlreadyAccepted) => {}
            Err(error) => return Err(error),
        }
        let retires =
            unix_seconds(self.clock.now()).saturating_add(self.rotation.retire_after.as_secs());
        let path = self.existing("accepted", &old).ok_or(Error::NoSuccessor)?;
        replace(
            &path,
            &accepted_text(&old_entry.key, Some(new), Some(retires)),
        )
        .map_err(Error::Io)?;
        let _ = fs::remove_file(self.path("successors", new));
        self.push_event(RotationEvent::Promoted {
            old,
            new: *new,
            retires: from_unix_seconds(retires),
        });
        Ok(old)
    }

    /// The pending successors, sorted by the new key's id.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the directory cannot be read.
    pub fn successors(&self) -> Result<Vec<Successor>, Error> {
        Ok(self
            .list("successors")?
            .into_iter()
            .filter(|new| self.successor_key(new).is_some())
            .filter_map(|new| {
                let pending = self.read("successors", &new)?;
                Some(Successor {
                    old: pending.predecessor?,
                    new,
                    proven: pending.proven.map(from_unix_seconds),
                })
            })
            .collect())
    }

    /// Accepted keys with a retirement time, sorted by id, those already retired included.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the directory cannot be read.
    pub fn retiring(&self) -> Result<Vec<Retiring>, Error> {
        Ok(self
            .list("accepted")?
            .into_iter()
            .filter_map(|id| {
                let entry = self.read("accepted", &id)?;
                Some(Retiring {
                    id,
                    successor: entry.successor,
                    retires: from_unix_seconds(entry.retires?),
                })
            })
            .collect())
    }

    /// The rotation events since the last call, oldest first. Only events of this process are
    /// kept, at most 1024; the on-disk state is in [`successors`](TrustStore::successors) and
    /// [`retiring`](TrustStore::retiring).
    pub fn take_rotation_events(&self) -> Vec<RotationEvent> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    fn push_event(&self, event: RotationEvent) {
        let mut events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        if events.len() >= MAX_EVENTS {
            events.pop_front();
        }
        events.push_back(event);
    }

    /// Writes `key` as accepted and current: new, or over a retired file.
    fn write_current(&self, key: &KeyMaterial) -> Result<(), Error> {
        if self.is_accepted(&key.id()) {
            return Err(Error::AlreadyAccepted);
        }
        match self.existing("accepted", &key.id()) {
            Some(path) => replace(&path, &accepted_text(key, None, None)).map_err(Error::Io),
            None => self.write_accepted(key),
        }
    }

    fn write_accepted(&self, key: &KeyMaterial) -> Result<(), Error> {
        match write_new(
            &self.path("accepted", &key.id()),
            &accepted_text(key, None, None),
        ) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                Err(Error::AlreadyAccepted)
            }
            Err(error) => Err(Error::Io(error)),
        }
    }

    /// The accepted entry for `id` when it is sound and not past its retirement time.
    fn current(&self, id: &PeerId) -> Option<Entry> {
        let entry = self.read("accepted", id)?;
        match entry.retires {
            Some(retires) if self.clock.now() >= from_unix_seconds(retires) => None,
            _ => Some(entry),
        }
    }

    /// The sound entry for `id` in `section`: its file parses and its key hashes to `id`.
    fn read(&self, section: &str, id: &PeerId) -> Option<Entry> {
        let text = fs::read_to_string(self.existing(section, id)?).ok()?;
        parse(&text).filter(|entry| entry.key.id() == *id)
    }

    fn list(&self, section: &str) -> Result<Vec<PeerId>, Error> {
        let entries = match fs::read_dir(self.root.join(section)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(Error::Io(error)),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let path = entry.map_err(Error::Io)?.path();
            if path.extension().is_some_and(|extension| extension == "pem")
                && let Some(Ok(id)) = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .filter(|stem| is_hex_name(stem))
                    .map(str::parse::<PeerId>)
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    fn path(&self, section: &str, id: &PeerId) -> PathBuf {
        self.root.join(section).join(format!("{}.pem", id.hex()))
    }

    fn revoked_path(&self, id: &InvitationId) -> PathBuf {
        self.root
            .join("invitations")
            .join(format!("{}.revoked", id.hex()))
    }

    fn invitation_path(&self, id: &InvitationId) -> PathBuf {
        self.root
            .join("invitations")
            .join(format!("{}.inv", id.hex()))
    }

    /// The file for `id` in `section`, when it exists.
    fn existing(&self, section: &str, id: &PeerId) -> Option<PathBuf> {
        let path = self.path(section, id);
        path.is_file().then_some(path)
    }
}

/// Whether `stem` is a file name this store writes: 64 lowercase hexadecimal characters.
fn is_hex_name(stem: &str) -> bool {
    stem.len() == 64 && HEXLOWER.decode_mut(stem.as_bytes(), &mut [0u8; 32]).is_ok()
}

/// The source an address counts against: IPv4 as it is, IPv4-mapped IPv6 as the IPv4 address,
/// other IPv6 cut to its first `prefix` bits.
pub(crate) fn source_key(address: IpAddr, prefix: u8) -> IpAddr {
    match address {
        IpAddr::V4(_) => address,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(prefix.min(128)))
                    .unwrap_or(0);
                IpAddr::V6(Ipv6Addr::from(u128::from(v6) & mask))
            }
        },
    }
}

/// An accepted file: `Method:`, then `Successor:` and `Retires:` when set, then the PEM block.
fn accepted_text(key: &KeyMaterial, successor: Option<&PeerId>, retires: Option<u64>) -> String {
    let mut text = format!("Method: {}\n", key.method);
    if let Some(successor) = successor {
        text.push_str(&format!("Successor: {}\n", successor.hex()));
    }
    if let Some(retires) = retires {
        text.push_str(&format!("Retires: {retires}\n"));
    }
    text.push_str(&pem::encode(&key.label, &key.bytes));
    text
}

fn invitation_text(expires: SystemTime, uses: u32, label: &str) -> String {
    let mut text = format!("Expires: {}\nUses: {uses}\n", unix_seconds(expires));
    if !label.is_empty() {
        text.push_str(&format!("Label: {label}\n"));
    }
    text
}

/// `(expires, uses, label)` of an invitation file; `None` unless `Expires:` and a positive
/// `Uses:` are present.
fn parse_invitation(text: &str) -> Option<(u64, u32, String)> {
    let (mut expires, mut uses, mut label) = (None, None, String::new());
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("Expires: ") {
            expires = value.trim().parse().ok();
        } else if let Some(value) = line.strip_prefix("Uses: ") {
            uses = value.trim().parse::<u32>().ok().filter(|uses| *uses > 0);
        } else if let Some(value) = line.strip_prefix("Label: ") {
            value.trim().clone_into(&mut label);
        }
    }
    Some((expires?, uses?, label))
}

/// Reads a store file: header lines, then one PEM block.
fn parse(text: &str) -> Option<Entry> {
    let pem::Block {
        headers,
        label,
        bytes,
        ..
    } = pem::decode(text)?;
    let mut method = None;
    let (mut source, mut successor, mut retires) = (None, None, None);
    let (mut predecessor, mut proven) = (None, None);
    for line in headers {
        if let Some(value) = line.strip_prefix("Method: ") {
            method = Some(value.trim());
        } else if let Some(value) = line.strip_prefix("Source: ") {
            source = value.trim().parse().ok();
        } else if let Some(value) = line.strip_prefix("Successor: ") {
            successor = Some(value.trim().parse().ok()?);
        } else if let Some(value) = line.strip_prefix("Retires: ") {
            retires = Some(value.trim().parse().ok()?);
        } else if let Some(value) = line.strip_prefix("Predecessor: ") {
            predecessor = Some(value.trim().parse().ok()?);
        } else if let Some(value) = line.strip_prefix("Proven: ") {
            proven = Some(value.trim().parse().ok()?);
        }
    }
    Some(Entry {
        key: KeyMaterial::new(method?, label, bytes).ok()?,
        source,
        successor,
        retires,
        predecessor,
        proven,
    })
}

/// Writes `text` to `path` unless it exists, atomically.
fn write_new(path: &Path, text: &str) -> io::Result<()> {
    file::write_new(path, text.as_bytes(), Access::Default)
}

/// Writes `text` to `path`, replacing what is there, atomically.
fn replace(path: &Path, text: &str) -> io::Result<()> {
    file::replace(path, text.as_bytes(), Access::Default)
}
