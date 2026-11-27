//! Maps peer certificates, verified by the transport, to directory identities.
//!
//! The transport (see `coqui-peer-tls`) proves a peer holds the key of its certificate and
//! hands the certificate's fingerprint in [`Attempt::certificate`]. This provider keeps the
//! fingerprints that were accepted in a [`Table`], each under a name and groups. A
//! certificate nobody accepted is not authenticated and is kept for a while as waiting, in a
//! second table, so whoever runs the service can accept it. Build the provider in code with
//! [`MtlsAccounts::builder`].
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

mod pin;
mod trust;

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use coqui_access::Limit;
use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, Schema, ToValue};
use coqui_access::{
    AccountInfo, AccountPut, Accounts, Attempt, Authenticator, Description, Directory, Lookup,
    Person, PutOutcome, Verdict, Waiting, error, takes,
};
use coqui_access_kit::pick::Pick;
use coqui_access_kit::{
    Keyed, MemoryTable, RecordLimits, Reloading, SourceError, Table, check_keys, secret, unix_now,
};
use coqui_peer::{WaitingLimits, WaitingQueue};
use coqui_peer_tls::{CaPolicy, Digest, PeerId, Policy, Trust};

use pin::{Kind, Normal};

pub use pin::{Pin, PinError};
pub use trust::{TrustMode, default_account};

/// The most peers a table may hold unless the builder says otherwise.
pub const DEFAULT_MAX_RECORDS: usize = 10_000;
/// The most pins (fingerprints and keys together) one account may hold unless the builder
/// says otherwise.
pub const DEFAULT_MAX_PINS: usize = 4;
/// The most certificates a client may send, leaf included, unless the builder says
/// otherwise.
pub const DEFAULT_MAX_CHAIN: Limit = Limit::at(8);
/// The bounds on the waiting list unless the builder says otherwise: one place per source
/// (an IPv4 address, or an IPv6 /64), 16 in all, and a fingerprint from one source written
/// down again at most once a minute.
pub fn default_waiting_limits() -> WaitingLimits {
    let mut limits = WaitingLimits::default();
    limits.per_source = DEFAULT_WAITING_PER_SOURCE;
    limits.total = DEFAULT_WAITING_TOTAL;
    limits.rerecord = DEFAULT_WAITING_RERECORD;
    limits
}

/// Places one source holds on the waiting list unless the builder says otherwise.
pub const DEFAULT_WAITING_PER_SOURCE: usize = 1;
/// Places on the waiting list in all unless the builder says otherwise.
pub const DEFAULT_WAITING_TOTAL: usize = 16;
/// How soon one fingerprint from one source is written down again unless the builder says
/// otherwise.
pub const DEFAULT_WAITING_RERECORD: Duration = Duration::from_secs(60);

/// One accepted peer as a [`Table`] keeps it, keyed by [`name`](PeerRecord::name).
///
/// It derives guatiao's `ToValue`, `FromValue` and `Schema`, so a [`Table`] may keep it in
/// any format.
#[derive(Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct PeerRecord {
    /// Stable identity; what a [`Verdict::Valid`] lookup names.
    pub id: String,
    /// The peer's name; unique in the table.
    pub name: String,
    /// The SHA-256 fingerprint of a certificate it pins: written in its canonical form, read
    /// in any spelling `coqui-peer-tls` reads. Absent for an account pinned by other means.
    #[map(as_text)]
    pub certificate: Option<PeerId>,
    /// Further pins, each naming its `kind` (`fingerprint` or `key`): with `certificate`,
    /// everything the account is known by. A certificate renewed on the same key keeps a
    /// `key` pin. An account holds at least one pin and at most the provider's `max_pins`.
    #[map(default, skip_serializing_if = Vec::is_empty)]
    pub pins: Vec<Pin>,
    /// Email address.
    pub email: Option<String>,
    /// Directory groups.
    #[map(default, skip_serializing_if = Vec::is_empty)]
    pub groups: Vec<String>,
    /// A disabled peer is refused.
    #[map(default, skip_serializing_if = core::ops::Not::not)]
    pub disabled: bool,
    /// When it was accepted, in seconds since the Unix epoch.
    pub made_unix: Option<i64>,
}

impl PeerRecord {
    /// An enabled peer with no groups, accepted by this fingerprint.
    pub fn new(id: impl Into<String>, name: impl Into<String>, certificate: &PeerId) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            certificate: Some(*certificate),
            pins: Vec::new(),
            email: None,
            groups: Vec::new(),
            disabled: false,
            made_unix: None,
        }
    }

    /// An enabled peer with no groups, accepted by `pin`: a public key, say, so a renewed
    /// certificate on the same key signs in.
    pub fn keyed(id: impl Into<String>, name: impl Into<String>, pin: Pin) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            certificate: None,
            pins: vec![pin],
            email: None,
            groups: Vec::new(),
            disabled: false,
            made_unix: None,
        }
    }

    /// Sets the directory groups.
    #[must_use]
    pub fn groups(mut self, groups: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.groups = groups.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the email address.
    #[must_use]
    pub fn email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// Sets whether the peer is disabled.
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Every pin the peer holds, in one spelling; `None` when one is not valid.
    fn held(&self) -> Option<Vec<Normal>> {
        self.certificate
            .iter()
            .map(|id| Some(Normal::of_fingerprint(id)))
            .chain(self.pins.iter().map(Pin::normal))
            .collect()
    }

    fn holds(&self, pin: &Normal) -> bool {
        self.held().is_some_and(|held| held.contains(pin))
    }

    fn pin_count(&self) -> usize {
        usize::from(self.certificate.is_some()) + self.pins.len()
    }

    /// Adds a pin the peer does not hold yet, unless it holds `most` already.
    fn add_pin(&mut self, pin: &Pin, most: usize) -> Result<(), SourceError> {
        let normal = pin
            .normal()
            .ok_or_else(|| SourceError::invalid("that is not a certificate pin"))?;
        if self.holds(&normal) {
            return Ok(());
        }
        if self.pin_count() >= most {
            return Err(SourceError::invalid(
                "that peer holds as many pins as the provider allows",
            ));
        }
        match (normal.kind, self.certificate) {
            (Kind::Fingerprint, None) => {
                self.certificate = pin.value.parse::<PeerId>().ok();
            }
            _ => self.pins.push(Pin {
                kind: pin.kind.clone(),
                value: normal.value,
            }),
        }
        Ok(())
    }

    /// Takes one pin away, never the last one: a peer nothing could sign in as is removed.
    fn remove_pin(&mut self, pin: &Pin) -> Result<(), SourceError> {
        let normal = pin
            .normal()
            .ok_or_else(|| SourceError::invalid("that is not a certificate pin"))?;
        if !self.holds(&normal) {
            return Err(SourceError::invalid("that peer does not hold that pin"));
        }
        if self.pin_count() == 1 {
            return Err(SourceError::invalid(
                "that is the peer's last pin; remove the peer instead",
            ));
        }
        let own = self.certificate.as_ref().map(Normal::of_fingerprint);
        if own.as_ref() == Some(&normal) {
            self.certificate = None;
        } else {
            self.pins
                .retain(|held| held.normal().as_ref() != Some(&normal));
        }
        Ok(())
    }
}

impl Keyed for PeerRecord {
    fn key(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.name)
    }
}

impl fmt::Debug for PeerRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerRecord")
            .field("id", &"[REDACTED]")
            .field("name", &"[REDACTED]")
            .field(
                "certificate",
                &self.certificate.as_ref().map(|_| "[REDACTED]"),
            )
            .field("pins", &self.pins.len())
            .field("email", &self.email.as_ref().map(|_| "[REDACTED]"))
            .field("groups", &self.groups.len())
            .field("disabled", &self.disabled)
            .finish()
    }
}

/// A fingerprint shown to the service that nobody has accepted, keyed by
/// [`fingerprint`](Unaccepted::fingerprint).
///
/// It derives guatiao's `ToValue`, `FromValue` and `Schema`, so a [`Table`] may keep it in
/// any format.
#[derive(Debug, Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct Unaccepted {
    /// The fingerprint: written in its canonical form, read in any spelling `coqui-peer-tls`
    /// reads.
    #[map(as_text)]
    pub fingerprint: PeerId,
    /// The address it was shown from, when known.
    pub peer: Option<String>,
    /// When it was last written down, in seconds since the Unix epoch.
    pub seen_unix: i64,
}

impl Unaccepted {
    /// A fingerprint seen at `seen_unix`, from `peer`.
    pub fn new(fingerprint: &PeerId, peer: Option<String>, seen_unix: i64) -> Self {
        Self {
            fingerprint: *fingerprint,
            peer,
            seen_unix,
        }
    }
}

impl Keyed for Unaccepted {
    fn key(&self) -> Cow<'_, str> {
        Cow::Owned(self.fingerprint.canonical())
    }
}

/// Options for [`MtlsAccounts`]; every one has a default.
pub struct MtlsAccountsBuilder {
    table: Option<Arc<dyn Table<PeerRecord>>>,
    waiting: Option<Arc<dyn Table<Unaccepted>>>,
    max_records: usize,
    max_pins: usize,
    limits: RecordLimits,
    waiting_limits: WaitingLimits,
    trust: TrustMode,
    ca: Option<CaPolicy>,
    account: Vec<Pick>,
    max_chain: Limit,
    policy: Policy,
}

impl fmt::Debug for MtlsAccountsBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MtlsAccountsBuilder")
            .field("max_records", &self.max_records)
            .field("max_pins", &self.max_pins)
            .field("waiting_limits", &self.waiting_limits)
            .field("trust", &self.trust)
            .field("max_chain", &self.max_chain)
            .finish_non_exhaustive()
    }
}

impl MtlsAccountsBuilder {
    /// How client certificates are trusted. Default [`TrustMode::Pinned`].
    #[must_use]
    pub fn trust(mut self, trust: TrustMode) -> Self {
        self.trust = trust;
        self
    }

    /// The CA and name globs that [`TrustMode::Ca`] and [`TrustMode::PinnedOrCa`] check a
    /// chain against; `ip_in_dns` (on by default) lets an `ip:` glob match an address
    /// written as a DNS name.
    #[must_use]
    pub fn ca(mut self, ca: CaPolicy) -> Self {
        self.ca = Some(ca);
        self
    }

    /// How a CA-trusted certificate names its account: picks tried in order over its fields
    /// (`coqui-peer-tls`'s `CertificateFields::to_map`). Default: its `fingerprint`, by
    /// `certificate`.
    #[must_use]
    pub fn account(mut self, picks: Vec<Pick>) -> Self {
        self.account = picks;
        self
    }

    /// The most certificates a client may send, leaf included. Default 8.
    #[must_use]
    pub fn max_chain(mut self, most: Limit) -> Self {
        self.max_chain = most;
        self
    }

    /// The policy a chain is checked under (validity dates, algorithms). Default
    /// [`Policy::default`].
    #[must_use]
    pub fn policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// Keeps the accepted peers in `table` instead of in memory.
    #[must_use]
    pub fn table(mut self, table: Arc<dyn Table<PeerRecord>>) -> Self {
        self.table = Some(table);
        self
    }

    /// Keeps the waiting fingerprints in `table` instead of in memory. A table that cannot be
    /// written means nobody waits.
    #[must_use]
    pub fn waiting(mut self, table: Arc<dyn Table<Unaccepted>>) -> Self {
        self.waiting = Some(table);
        self
    }

    /// The most peers the table may hold. Default [`DEFAULT_MAX_RECORDS`].
    #[must_use]
    pub fn max_records(mut self, max: usize) -> Self {
        self.max_records = max;
        self
    }

    /// The most pins, fingerprints and keys together, one peer may hold. Default
    /// [`DEFAULT_MAX_PINS`].
    #[must_use]
    pub fn max_pins(mut self, max: usize) -> Self {
        self.max_pins = max;
        self
    }

    /// How large a peer's fields may be. Default [`RecordLimits::DEFAULT`].
    #[must_use]
    pub fn record_limits(mut self, limits: RecordLimits) -> Self {
        self.limits = limits;
        self
    }

    /// The bounds on the waiting list, applied by coqui-peer's [`WaitingQueue`]: places per
    /// source, places in all (a newcomer past them is not written down; nobody is pushed out
    /// for it), and how soon one fingerprint from one source is written down again. Default
    /// [`default_waiting_limits`].
    #[must_use]
    pub fn waiting_limits(mut self, limits: WaitingLimits) -> Self {
        self.waiting_limits = limits;
        self
    }

    /// Loads the table and makes the provider.
    ///
    /// # Errors
    ///
    /// Returns an error when the table cannot be loaded or holds an invalid or duplicated
    /// peer, or the trust needs a CA and none is given.
    pub fn build(self) -> Result<MtlsAccounts, SourceError> {
        if self.account.iter().any(|pick| pick.check().is_err()) {
            return Err(SourceError::invalid(
                "an account pick is looked up by an attribute, not by filter or claims",
            ));
        }
        let ca = match (self.trust.uses_ca(), self.ca) {
            (true, Some(ca)) => Some(Trust::Ca(ca)),
            (true, None) => {
                return Err(SourceError::invalid(
                    "trust by a certificate authority needs its ca_file",
                ));
            }
            (false, _) => None,
        };
        let table = self
            .table
            .unwrap_or_else(|| Arc::new(MemoryTable::<PeerRecord>::new()));
        let (max, limits, max_pins) = (self.max_records, self.limits, self.max_pins);
        let waiting_table = self
            .waiting
            .unwrap_or_else(|| Arc::new(MemoryTable::<Unaccepted>::new()));
        let mut queue = WaitingQueue::new(self.waiting_limits);
        // What is waiting already counts against its source, as old as it was written down.
        let (now, now_unix) = (Instant::now(), unix_now());
        for had in waiting_table
            .load()
            .map(|kept| kept.records)
            .unwrap_or_default()
        {
            let age = Duration::from_secs(u64::try_from(now_unix - had.seen_unix).unwrap_or(0));
            let at = now.checked_sub(age).unwrap_or(now);
            queue.remember(had.fingerprint, source_of(had.peer.as_deref()), at);
        }
        Ok(MtlsAccounts {
            peers: Reloading::new(table, move |records| {
                validate(records, max, max_pins, &limits)
            })?,
            limits,
            max_pins,
            waiting_table,
            queue: Mutex::new(queue),
            trust: self.trust,
            ca,
            account: self.account,
            max_chain: self.max_chain,
            policy: self.policy,
        })
    }
}

/// MtlsAccounts as an Authenticator, Directory and Accounts implementation.
///
/// The table is read again whenever it reports a change.
///
/// ```
/// use coqui_access::{Attempt, Authenticator, Verdict};
/// use coqui_access_mtls::MtlsAccounts;
///
/// let peers = MtlsAccounts::builder().build().unwrap();
/// let seen = peers
///     .authenticate(Attempt {
///         certificate: Some(
///             "sha256:0000000000000000000000000000000000000000000000000000000000000000".into(),
///         ),
///         ..Attempt::default()
///     })
///     .unwrap();
/// // Nobody accepted it: not authenticated, and kept as waiting.
/// assert_eq!(seen, Verdict::Unknown);
/// ```
pub struct MtlsAccounts {
    peers: Reloading<PeerRecord>,
    waiting_table: Arc<dyn Table<Unaccepted>>,
    /// Decides who is written down as waiting; held while the waiting table is read and
    /// written.
    queue: Mutex<WaitingQueue>,
    limits: RecordLimits,
    max_pins: usize,
    trust: TrustMode,
    ca: Option<Trust>,
    account: Vec<Pick>,
    max_chain: Limit,
    policy: Policy,
}

impl fmt::Debug for MtlsAccounts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        formatter
            .debug_struct("MtlsAccounts")
            .field("waiting_limits", &queue.limits())
            .field("trust", &self.trust)
            .finish_non_exhaustive()
    }
}

impl MtlsAccounts {
    /// Options for a peer table, kept in memory unless [`table`](MtlsAccountsBuilder::table) says
    /// otherwise.
    pub fn builder() -> MtlsAccountsBuilder {
        MtlsAccountsBuilder {
            table: None,
            waiting: None,
            max_records: DEFAULT_MAX_RECORDS,
            max_pins: DEFAULT_MAX_PINS,
            limits: RecordLimits::DEFAULT,
            trust: TrustMode::Pinned,
            ca: None,
            account: default_account(),
            max_chain: DEFAULT_MAX_CHAIN,
            policy: Policy::default(),
            waiting_limits: default_waiting_limits(),
        }
    }

    /// Writes a fingerprint nobody accepted down as waiting, when coqui-peer's
    /// [`WaitingQueue`] admits it: one already waiting stays as it is.
    ///
    /// The decision is made inside the waiting table's own update, from the entries it holds
    /// then (a file table is locked for it), with a queue built from them. So entries another
    /// provider or process wrote count against their source and the total, and two racing
    /// providers cannot both take the last place.
    fn wait(&self, fingerprint: &PeerId, from: Option<String>) {
        let queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let limits = queue.limits();
        let source = source_of(from.as_deref());
        let mut evict = Vec::new();
        // Whether it could be written is not the peer's to be told.
        let _ = self.waiting_table.update(&mut |waits| {
            if waits.iter().any(|had| had.fingerprint == *fingerprint) {
                return Ok(None);
            }
            let (now, now_unix) = (Instant::now(), unix_now());
            let mut fresh = WaitingQueue::new(limits);
            for had in waits {
                let age = Duration::from_secs(u64::try_from(now_unix - had.seen_unix).unwrap_or(0));
                let at = now.checked_sub(age).unwrap_or(now);
                fresh.remember(had.fingerprint, source_of(had.peer.as_deref()), at);
            }
            let admission = fresh.admit(*fingerprint, source, waits.len(), now);
            if !admission.record {
                return Ok(None);
            }
            evict = admission.evict;
            Ok(Some(Unaccepted::new(fingerprint, from.clone(), now_unix)))
        });
        for old in &evict {
            let _ = self.waiting_table.remove(&old.canonical());
        }
    }

    /// MtlsAccounts that are accepted wait no more.
    fn waited(&self, accepted: &[Normal]) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        for had in self.waits() {
            if accepted.contains(&Normal::of_fingerprint(&had.fingerprint)) {
                queue.forget(&had.fingerprint);
                let _ = self.waiting_table.remove(&had.fingerprint.canonical());
            }
        }
    }

    /// The waiting entries, oldest first. A table that cannot be read is nobody waiting.
    fn waits(&self) -> Vec<Unaccepted> {
        self.waiting_table
            .load()
            .map(|snapshot| snapshot.records)
            .unwrap_or_default()
    }
}

impl Authenticator for MtlsAccounts {
    /// Pinned first (unless the trust is `ca`): a known, enabled record is `Valid` by `id`, a
    /// disabled one `Invalid`. Then the CA (unless the trust is `pinned`), for a `secure`
    /// attempt carrying `certificates`: a trusted chain is answered by the `account` picks,
    /// `Ambiguous` being `Invalid`. Anything else is `Unknown`, and under a pinning trust the
    /// fingerprint waits.
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let chain = attempt.certificates.as_deref().unwrap_or_default();
        if !self.max_chain.allows(chain.len()) {
            return Ok(Verdict::Unknown);
        }
        let fingerprint = attempt
            .certificate
            .as_deref()
            .and_then(|text| text.parse::<PeerId>().ok())
            .or_else(|| chain.first().map(|leaf| PeerId::of(leaf.as_ref())))
            .filter(|fingerprint| fingerprint.digest() == Digest::Sha256);
        if self.trust.pins() {
            let by_fingerprint = fingerprint.as_ref().map(Normal::of_fingerprint);
            let by_key = key_of_leaf(&attempt, chain).and_then(|pin| pin.normal());
            let found = self
                .peers
                .read(|records| {
                    // The certificate itself outranks its key, should two accounts pin them.
                    by_fingerprint
                        .iter()
                        .chain(&by_key)
                        .find_map(|pin| records.iter().find(|peer| peer.holds(pin)))
                        .map(|peer| (peer.id.clone(), peer.disabled))
                })
                .map_err(|reason| reason.reading("peer table"))?;
            match found {
                Some((_, true)) => return Ok(Verdict::Invalid),
                Some((id, false)) => {
                    return Ok(Verdict::Valid(Lookup {
                        by: "id".to_owned(),
                        value: id,
                        claims: chain
                            .first()
                            .and_then(|leaf| coqui_access_claims::of_certificate(leaf.as_ref())),
                    }));
                }
                None => {}
            }
        }
        if let Some(ca) = &self.ca
            && attempt.secure == Some(true)
            && let Some(verdict) = trust::by_ca(ca, &self.policy, &self.account, chain)?
        {
            return Ok(verdict);
        }
        if self.trust.pins()
            && let Some(fingerprint) = fingerprint
        {
            self.wait(&fingerprint, attempt.peer);
        }
        Ok(Verdict::Unknown)
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description {
            name: Some("Client certificates".to_owned()),
            takes: Some(takes::REQUEST.to_owned()),
        })
    }
}

impl Directory for MtlsAccounts {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        let by_certificate = if lookup.by == "certificate" {
            lookup
                .value
                .parse::<PeerId>()
                .ok()
                .map(|id| Normal::of_fingerprint(&id))
        } else {
            None
        };
        self.peers
            .read(|records| {
                records
                    .iter()
                    .find(|peer| {
                        !peer.disabled
                            && match lookup.by.as_str() {
                                "id" => peer.id == lookup.value,
                                "name" => peer.name == lookup.value,
                                "certificate" => {
                                    by_certificate.as_ref().is_some_and(|own| peer.holds(own))
                                }
                                "email" => peer.email.as_deref() == Some(&lookup.value),
                                _ => false,
                            }
                    })
                    .map(|peer| Person {
                        id: peer.id.clone(),
                        name: peer.name.clone(),
                        display_name: None,
                        email: peer.email.clone(),
                        groups: peer.groups.clone(),
                        attributes: Default::default(),
                    })
            })
            .map_err(|reason| reason.reading("peer table"))
    }
}

impl Accounts for MtlsAccounts {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError> {
        self.peers
            .read(|records| {
                records
                    .iter()
                    .map(|peer| AccountInfo {
                        id: peer.id.clone(),
                        name: peer.name.clone(),
                        groups: peer.groups.clone(),
                        disabled: Some(peer.disabled),
                        made_unix: peer.made_unix,
                        expires_unix: None,
                        ..AccountInfo::default()
                    })
                    .collect()
            })
            .map_err(|reason| reason.reading("peer table"))
    }

    /// Accepts a pin under a name, or changes what is said of a peer there is.
    ///
    /// The `secret` of a peer is a pin: a certificate fingerprint in any spelling
    /// `coqui-peer-tls` reads, or `key:sha256:<hex>` for a public key. For a new name it is
    /// the peer's first pin; for a peer there is it is one more, up to `max_pins`, and saying
    /// one the peer holds changes nothing. `remove:` before a pin takes that pin away alone,
    /// never the last one.
    fn put(&self, account: AccountPut) -> Result<PutOutcome, ProviderError> {
        if !self.limits.allows_field(&account.name) {
            return Err(error::bad_value("a peer has a name"));
        }
        if account.reset || account.unlock {
            return Err(error::unsupported(
                "this provider keeps no reset mark or failure count",
            ));
        }
        if account.expires_unix.is_some() {
            return Err(error::bad_value("peers do not expire"));
        }
        let change = account
            .secret
            .as_deref()
            .map(|text| match text.strip_prefix(REMOVE_PREFIX) {
                Some(pin) => Pin::parse(pin).map(Change::Remove),
                None => Pin::parse(text).map(Change::Add),
            })
            .transpose()
            .map_err(|_| error::bad_value("that is not a certificate fingerprint or key pin"))?;
        let accepted = self
            .peers
            .put_with(|records| {
                if let Some(Change::Add(pin)) = &change {
                    let normal = pin
                        .normal()
                        .ok_or_else(|| SourceError::invalid("that is not a certificate pin"))?;
                    if records
                        .iter()
                        .any(|peer| peer.holds(&normal) && peer.name != account.name)
                    {
                        return Err(SourceError::invalid(
                            "that certificate belongs to another peer",
                        ));
                    }
                }
                let mut peer = match (
                    records.iter().find(|peer| peer.name == account.name),
                    &change,
                ) {
                    (Some(peer), _) => peer.clone(),
                    (None, Some(Change::Add(_))) => PeerRecord {
                        id: secret::identifier().map_err(SourceError::unavailable)?,
                        name: account.name.clone(),
                        certificate: None,
                        pins: Vec::new(),
                        email: None,
                        groups: Vec::new(),
                        disabled: false,
                        made_unix: Some(unix_now()),
                    },
                    _ => {
                        return Err(SourceError::invalid(
                            "a new peer is accepted by a certificate pin",
                        ));
                    }
                };
                match &change {
                    Some(Change::Add(pin)) => peer.add_pin(pin, self.max_pins)?,
                    Some(Change::Remove(pin)) => peer.remove_pin(pin)?,
                    None => {}
                }
                if let Some(groups) = account.groups {
                    peer.groups = groups;
                }
                if let Some(disabled) = account.disabled {
                    peer.disabled = disabled;
                }
                let accepted = records
                    .iter()
                    .filter(|other| other.name != peer.name)
                    .chain(std::iter::once(&peer))
                    .filter_map(PeerRecord::held)
                    .flatten()
                    .collect::<Vec<_>>();
                Ok((peer, accepted))
            })
            .map_err(|reason| reason.writing("peer table"))?;
        self.waited(&accepted);
        Ok(PutOutcome::default())
    }

    fn remove(&self, name: String) -> Result<(), ProviderError> {
        self.peers
            .remove(&name)
            .map_err(|reason| reason.writing("peer table"))
            .and_then(|removed| {
                removed
                    .then_some(())
                    .ok_or_else(|| error::not_found("there is no such peer"))
            })
    }

    fn waiting(&self) -> Result<Vec<Waiting>, ProviderError> {
        let _queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(self
            .waits()
            .into_iter()
            .rev()
            .map(|waits| Waiting {
                shown: waits.fingerprint.canonical(),
                peer: waits.peer,
                seen_unix: Some(waits.seen_unix),
            })
            .collect())
    }
}

/// The source a waiting entry counts against: `address:port`, or a bare address; anything
/// else shares the one bucket of peers whose address is not known.
fn source_of(peer: Option<&str>) -> Option<SocketAddr> {
    let peer = peer?;
    peer.parse::<SocketAddr>()
        .ok()
        .or_else(|| peer.parse().ok().map(|address| SocketAddr::new(address, 0)))
}

/// The records as a source of pins for a matcher: an enabled record named like the person
/// holds the fingerprint or the public key the certificate presented.
impl coqui_access_kit::pins::PinSource for MtlsAccounts {
    fn check(
        &self,
        person: &Person,
        pins: &coqui_access_kit::pins::PinSet,
    ) -> Result<coqui_access_kit::pins::PinCheck, ProviderError> {
        use coqui_access_kit::pins::PinCheck;
        let wanted: Vec<Normal> = pins
            .fingerprint
            .as_deref()
            .and_then(|text| text.parse::<PeerId>().ok())
            .map(|id| Normal::of_fingerprint(&id))
            .into_iter()
            .chain(pins.key.as_ref().and_then(|value| {
                Pin {
                    kind: Pin::KEY.to_owned(),
                    value: value.clone(),
                }
                .normal()
            }))
            .collect();
        let held = self
            .peers
            .read(|records| {
                records.iter().any(|peer| {
                    peer.name == person.name
                        && !peer.disabled
                        && wanted.iter().any(|pin| peer.holds(pin))
                })
            })
            .map_err(|reason| reason.reading("peer table"))?;
        Ok(if held {
            PinCheck::Holds
        } else {
            PinCheck::Absent
        })
    }
}

/// What a `secret` asks of a peer's pins.
enum Change {
    Add(Pin),
    Remove(Pin),
}

/// Before a pin in a `secret`: take that pin away from the peer.
const REMOVE_PREFIX: &str = "remove:";

/// The public key pin of the certificate the transport proved the client holds the key of:
/// the first of the chain, and only when the attempt is `secure` and its fingerprint, if
/// it carries one, is that certificate's.
fn key_of_leaf(attempt: &Attempt, chain: &[coqui_access::guatiao::Bytes]) -> Option<Pin> {
    let leaf = chain.first()?;
    if attempt.secure != Some(true) {
        return None;
    }
    if let Some(text) = &attempt.certificate
        && text.parse::<PeerId>().ok() != Some(PeerId::of(leaf.as_ref()))
    {
        return None;
    }
    Pin::key_of_certificate(leaf.as_ref())
}

fn validate(
    peers: &[PeerRecord],
    max: usize,
    max_pins: usize,
    limits: &RecordLimits,
) -> Result<(), SourceError> {
    check_keys(peers, max)?;
    let mut ids = BTreeSet::new();
    let mut pinned = BTreeSet::new();
    for peer in peers {
        let Some(held) = peer.held() else {
            return Err(SourceError::invalid(
                "a peer is pinned by a SHA-256 fingerprint or a public key",
            ));
        };
        if held.is_empty()
            || held.len() > max_pins
            || held.iter().any(|pin| !pinned.insert(pin.clone()))
        {
            return Err(SourceError::invalid(
                "a peer holds no pin, too many, or a pin another peer or itself holds",
            ));
        }
        if peer
            .certificate
            .is_some_and(|id| id.digest() != Digest::Sha256)
        {
            return Err(SourceError::invalid(
                "a peer certificate is identified by its SHA-256 fingerprint",
            ));
        }
        if !limits.allows_field(&peer.id)
            || !limits.allows_field(&peer.name)
            || !ids.insert(&peer.id)
            || !limits.allows_groups(&peer.groups)
            || peer
                .email
                .as_ref()
                .is_some_and(|email| !limits.allows_email(email))
        {
            return Err(SourceError::invalid(
                "a peer entry is invalid or duplicated",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use coqui_peer_tls::Identity;

    use super::*;

    fn attempt(certificate: Option<&str>) -> Attempt {
        Attempt {
            name: Some("ignored".to_owned()),
            secret: Some("ignored".to_owned()),
            secure: Some(true),
            certificate: certificate.map(str::to_owned),
            peer: Some("192.0.2.7:4000".to_owned()),
            ..Attempt::default()
        }
    }

    fn fingerprint() -> PeerId {
        Identity::generate().unwrap().fingerprint()
    }

    #[test]
    fn authenticates_only_known_certificates_and_resolves_groups() {
        let known = fingerprint();
        let certificate = known.to_string();
        let peers = MtlsAccounts::builder()
            .table(Arc::new(MemoryTable::from_records([PeerRecord::new(
                "service-1",
                "site-a",
                &known,
            )
            .groups(["site-a"])])))
            .build()
            .unwrap();
        let Verdict::Valid(lookup) = peers.authenticate(attempt(Some(&certificate))).unwrap()
        else {
            panic!("known certificate was rejected");
        };
        let person = peers.lookup(lookup).unwrap().unwrap();
        assert_eq!(person.id, "service-1");
        assert_eq!(person.groups, vec!["site-a"]);
        // Any spelling of the fingerprint is the same certificate.
        let hex = known.canonical();
        assert!(matches!(
            peers.authenticate(attempt(Some(&hex))).unwrap(),
            Verdict::Valid(_)
        ));
        assert_eq!(peers.authenticate(attempt(None)).unwrap(), Verdict::Unknown);
        assert_eq!(
            peers
                .authenticate(attempt(Some("not-a-fingerprint")))
                .unwrap(),
            Verdict::Unknown
        );
        assert!(!format!("{peers:?}").contains(&certificate));
    }

    #[test]
    fn disabled_peers_and_malformed_tables_are_refused() {
        let known = fingerprint();
        let certificate = known.to_string();
        let build = |records: Vec<PeerRecord>| {
            MtlsAccounts::builder()
                .table(Arc::new(MemoryTable::from_records(records)))
                .build()
        };
        let peers = build(vec![
            PeerRecord::new("service-1", "site-a", &known).disabled(true),
        ])
        .unwrap();
        assert_eq!(
            peers.authenticate(attempt(Some(&certificate))).unwrap(),
            Verdict::Invalid
        );
        assert!(
            peers
                .lookup(Lookup {
                    by: "id".to_owned(),
                    value: "service-1".to_owned(),
                    ..Lookup::default()
                })
                .unwrap()
                .is_none()
        );
        let twice = vec![
            PeerRecord::new("1", "a", &known),
            PeerRecord::new("2", "b", &known),
        ];
        assert!(build(twice).is_err(), "one certificate is one peer");
    }

    #[test]
    fn an_unknown_certificate_waits_and_is_accepted_by_putting_it() {
        let peers = MtlsAccounts::builder().build().unwrap();
        let fingerprint = fingerprint();
        let shown = fingerprint.to_string();
        assert_eq!(
            peers.authenticate(attempt(Some(&shown))).unwrap(),
            Verdict::Unknown
        );
        // Asking again from the same address does not write it again.
        peers.authenticate(attempt(Some(&shown))).unwrap();
        let waiting = peers.waiting().unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].shown, fingerprint.canonical());
        assert_eq!(waiting[0].peer.as_deref(), Some("192.0.2.7:4000"));

        peers
            .put(AccountPut {
                name: "site-b".to_owned(),
                secret: Some(waiting[0].shown.clone()),
                groups: Some(vec!["site-b".to_owned()]),
                ..AccountPut::default()
            })
            .unwrap();
        assert!(peers.waiting().unwrap().is_empty());
        assert!(matches!(
            peers.authenticate(attempt(Some(&shown))).unwrap(),
            Verdict::Valid(_)
        ));
        assert_eq!(peers.accounts().unwrap()[0].name, "site-b");
        // The same certificate cannot be two peers; a new peer needs a certificate.
        assert!(
            peers
                .put(AccountPut {
                    name: "site-c".to_owned(),
                    secret: Some(shown),
                    ..AccountPut::default()
                })
                .is_err()
        );
        assert!(
            peers
                .put(AccountPut {
                    name: "site-c".to_owned(),
                    ..AccountPut::default()
                })
                .is_err()
        );
        peers.remove("site-b".to_owned()).unwrap();
        assert!(peers.remove("site-b".to_owned()).is_err());
    }

    #[test]
    fn one_address_holds_one_waiting_place() {
        let peers = MtlsAccounts::builder().build().unwrap();
        for _ in 0..3 {
            let shown = fingerprint().to_string();
            peers.authenticate(attempt(Some(&shown))).unwrap();
        }
        assert_eq!(peers.waiting().unwrap().len(), 1);
    }

    fn from_source(last: u8, certificate: &PeerId) -> Attempt {
        Attempt {
            peer: Some(format!("192.0.2.{last}:4000")),
            certificate: Some(certificate.to_string()),
            ..Attempt::default()
        }
    }

    #[test]
    fn two_providers_over_one_waiting_table_share_a_sources_place() {
        let waiting = Arc::new(MemoryTable::<Unaccepted>::new());
        let build = || {
            MtlsAccounts::builder()
                .waiting(waiting.clone())
                .build()
                .unwrap()
        };
        let (first, second) = (build(), build());
        first.authenticate(from_source(9, &fingerprint())).unwrap();
        // The second provider has not seen the first one's entry in memory, only in the table.
        second.authenticate(from_source(9, &fingerprint())).unwrap();
        assert_eq!(
            waiting.load().unwrap().records.len(),
            1,
            "one place for 192.0.2.9"
        );
    }

    #[test]
    fn providers_racing_over_one_waiting_table_never_pass_its_total() {
        let waiting = Arc::new(MemoryTable::<Unaccepted>::new());
        let limits = {
            let mut limits = default_waiting_limits();
            limits.total = 3;
            limits
        };
        let providers: Vec<Arc<MtlsAccounts>> = (0..2)
            .map(|_| {
                Arc::new(
                    MtlsAccounts::builder()
                        .waiting(waiting.clone())
                        .waiting_limits(limits)
                        .build()
                        .unwrap(),
                )
            })
            .collect();
        let start = Arc::new(std::sync::Barrier::new(16));
        let threads: Vec<_> = (0..16u8)
            .map(|number| {
                let provider = providers[usize::from(number % 2)].clone();
                let start = start.clone();
                let from = from_source(number + 10, &fingerprint());
                std::thread::spawn(move || {
                    start.wait();
                    provider.authenticate(from).unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(waiting.load().unwrap().records.len(), 3);
    }

    #[test]
    fn the_waiting_limits_and_the_waiting_table_are_the_callers() {
        let waiting = Arc::new(MemoryTable::<Unaccepted>::new());
        let peers = MtlsAccounts::builder()
            .waiting(waiting.clone())
            .waiting_limits({
                let mut limits = default_waiting_limits();
                limits.total = 2;
                limits
            })
            .build()
            .unwrap();
        for last in 1..=3 {
            let from = Attempt {
                peer: Some(format!("192.0.2.{last}:4000")),
                certificate: Some(fingerprint().to_string()),
                ..Attempt::default()
            };
            peers.authenticate(from).unwrap();
        }
        // Full: the newcomer is not written down, and nobody is pushed out for it.
        let kept = waiting.load().unwrap().records;
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].peer.as_deref(), Some("192.0.2.1:4000"));
        assert_eq!(peers.waiting().unwrap().len(), 2);
    }

    #[test]
    fn two_addresses_in_one_ipv6_64_share_one_place() {
        let peers = MtlsAccounts::builder().build().unwrap();
        for (address, expected) in [
            ("[2001:db8:1:2::1]:4000", 1),
            ("[2001:db8:1:2:ffff::9]:4000", 1),
            ("[2001:db8:1:3::1]:4000", 2),
            ("[::ffff:192.0.2.1]:4000", 3),
            ("192.0.2.1:5000", 3),
        ] {
            let from = Attempt {
                peer: Some(address.to_owned()),
                certificate: Some(fingerprint().to_string()),
                ..Attempt::default()
            };
            peers.authenticate(from).unwrap();
            assert_eq!(peers.waiting().unwrap().len(), expected, "{address}");
        }
    }

    #[test]
    fn a_provider_built_over_a_waiting_table_counts_what_is_there() {
        let waiting = Arc::new(MemoryTable::<Unaccepted>::new());
        let one = fingerprint();
        let shown = one.to_string();
        let first = MtlsAccounts::builder()
            .waiting(waiting.clone())
            .build()
            .unwrap();
        first.authenticate(attempt(Some(&shown))).unwrap();
        // A provider built over the same table counts the entry against its source: the
        // source's one place stays with the first fingerprint, and a newcomer is not written down.
        let again = MtlsAccounts::builder()
            .waiting(waiting.clone())
            .build()
            .unwrap();
        again.authenticate(attempt(Some(&shown))).unwrap();
        assert_eq!(waiting.load().unwrap().records.len(), 1, "already waiting");
        let other = fingerprint();
        again
            .authenticate(attempt(Some(&other.to_string())))
            .unwrap();
        let kept = waiting.load().unwrap().records;
        assert_eq!(kept.len(), 1, "one place for 192.0.2.7");
        assert_eq!(kept[0].fingerprint, one);
    }

    #[test]
    fn fingerprints_are_kept_in_their_canonical_form_and_read_in_any() {
        let known = fingerprint();
        let record = PeerRecord::new("1", "a", &known);
        let value = record.to_value().unwrap();
        let map = coqui_access::guatiao::Map::try_from(value.clone()).unwrap();
        assert_eq!(
            map.get("certificate")
                .map(|text| String::from_value(text).unwrap()),
            Some(known.canonical())
        );
        assert_eq!(PeerRecord::from_value(&value).unwrap(), record);
        let mut grouped = map.clone();
        grouped.set("certificate", known.to_base32_grouped());
        assert_eq!(PeerRecord::from_value(&grouped.into()).unwrap(), record);
        let mut bad = map;
        bad.set("certificate", "bad");
        assert!(PeerRecord::from_value(&bad.into()).is_err());
    }

    #[test]
    fn a_waiting_table_that_cannot_be_written_means_nobody_waits() {
        struct Fixed;
        impl Table<Unaccepted> for Fixed {
            fn load(&self) -> Result<coqui_access_kit::Snapshot<Unaccepted>, SourceError> {
                Ok(coqui_access_kit::Snapshot::new(
                    coqui_access_kit::Revision::default(),
                    Vec::new(),
                ))
            }
        }
        let peers = MtlsAccounts::builder()
            .waiting(Arc::new(Fixed))
            .build()
            .unwrap();
        assert_eq!(
            peers
                .authenticate(attempt(Some(&fingerprint().to_string())))
                .unwrap(),
            Verdict::Unknown
        );
        assert!(peers.waiting().unwrap().is_empty());
    }
}
