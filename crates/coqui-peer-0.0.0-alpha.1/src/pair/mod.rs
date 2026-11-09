//! Pairing: how a joiner and a responder come to accept each other's keys, over any channel.
//!
//! Three modes, each of which a responder may turn off ([`PairingOptions`]):
//!
//! - **Invitation**: the responder made an [`Invitation`](crate::Invitation) and the joiner
//!   holds its secret and the inviter's id. The joiner sends the secret only over a channel
//!   that proved the inviter's key; the responder redeems it.
//! - **PIN** (feature `pin`): one side shows a short [`Pin`], a person types it on the other,
//!   and both run SPAKE2 with key confirmation over both sides' keys and the channel binding.
//!   An attacker gets one guess per run.
//! - **Device flow**: the responder issues a [`UserCode`] the joiner shows; an operator enters
//!   it at the responder and approves. Meant for a joiner that verified the responder.
//!
//! The exchange is sans-IO: [`PairingJoiner`] and [`PairingResponder`] take whole messages
//! with `receive` and give them with `transmit`; the caller moves them over a reliable,
//! ordered channel. What a person must see or decide is returned as data
//! ([`PairingJoiner::shown`], the [`PairingDesk`]); the crate shows nothing.

// The PIN mode's slots, throttles and messages are compiled without the `pin` feature too, so
// the wire format and the desk are the same in every build.
#![cfg_attr(not(feature = "pin"), allow(dead_code))]

mod code;
mod desk;
#[cfg(feature = "pin")]
mod pin;

use alloc::collections::VecDeque;
use core::fmt;
use core::net::SocketAddr;
use core::time::Duration;
use std::time::SystemTime;

use zeroize::Zeroizing;

pub use code::{CodeAlphabet, PairingCode, Pin, UserCode};
pub use desk::{
    Decided, Decision, DeskLimits, PairingDesk, PairingDeskBuilder, PairingDeskSettings,
    PairingOptions, PendingDevice, PendingSlot, ShownPin, Ticket,
};

use crate::clock::{from_unix_seconds, later, unix_seconds};
use crate::error::Error;
use crate::id::PeerId;
use crate::key::KeyMaterial;
#[cfg(feature = "pin")]
use crate::random::Random;
use crate::store::TrustStore;
use desk::Admission;

/// The first byte of every message.
const VERSION: u8 = 1;

const HELLO: u8 = 1;
const READY: u8 = 2;
const PAKE: u8 = 3;
const REPLY: u8 = 4;
const CONFIRM: u8 = 5;
const ISSUED: u8 = 6;
const DONE: u8 = 7;
const REFUSED: u8 = 8;
const ANNOUNCE: u8 = 9;

const MODE_INVITATION: u8 = 0;
const MODE_PIN_TYPED: u8 = 1;
const MODE_PIN_SHOWN: u8 = 2;
const MODE_DEVICE: u8 = 3;

/// No slot, on the wire.
const NO_SLOT: u8 = 0xff;

/// Why a pairing was refused, by this side or the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PairRefusal {
    /// The code or secret is wrong, or the key confirmation failed (which is also what a
    /// relaying attacker or a different channel binding looks like).
    WrongCode,
    /// The invitation or code expired.
    Expired,
    /// The PIN was used up, or none is shown.
    Exhausted,
    /// Too many runs, pending exchanges or wrong entries.
    Throttled,
    /// The operator denied the device-flow joiner.
    Denied,
    /// The responder does not offer this mode.
    Disabled,
}

impl PairRefusal {
    const fn wire(self) -> u8 {
        match self {
            Self::WrongCode => 1,
            Self::Expired => 2,
            Self::Exhausted => 3,
            Self::Throttled => 4,
            Self::Denied => 5,
            Self::Disabled => 6,
        }
    }

    const fn from_wire(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => Self::WrongCode,
            2 => Self::Expired,
            3 => Self::Exhausted,
            4 => Self::Throttled,
            5 => Self::Denied,
            6 => Self::Disabled,
            _ => return None,
        })
    }
}

impl fmt::Display for PairRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WrongCode => "the code is wrong or the key confirmation failed",
            Self::Expired => "the invitation or code expired",
            Self::Exhausted => "the PIN is used up or none is shown",
            Self::Throttled => "too many pairing attempts",
            Self::Denied => "the pairing was denied",
            Self::Disabled => "this pairing mode is not offered",
        })
    }
}

/// Why a pairing exchange failed. No message holds a secret, a code or key material.
#[derive(Debug)]
#[non_exhaustive]
pub enum PairingError {
    /// This side or the other refused.
    Refused(PairRefusal),
    /// A message was malformed or unexpected, or a key in it is not the one the channel proved.
    Protocol,
    /// The mode needs the channel to have proven (and, for an invitation, matched) the other
    /// side's key, and it did not.
    Unverified,
    /// A local failure: the store, the random source, or a code or option the caller passed.
    Local(Error),
}

impl fmt::Display for PairingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => write!(formatter, "pairing refused: {refusal}"),
            Self::Protocol => formatter.write_str("pairing message is malformed or unexpected"),
            Self::Unverified => formatter.write_str("the channel did not verify the other side"),
            Self::Local(error) => write!(formatter, "pairing failed locally: {error}"),
        }
    }
}

impl core::error::Error for PairingError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Local(error) => Some(error),
            _ => None,
        }
    }
}

impl From<Error> for PairingError {
    fn from(error: Error) -> Self {
        Self::Local(error)
    }
}

/// What the channel the exchange runs over gives it. `Debug` does not show the binding.
#[derive(Clone, PartialEq, Eq)]
pub struct Channel {
    /// This side's key, which the other side will accept.
    pub local: KeyMaterial,
    /// The other side's key when the channel proved the other side holds it (a TLS handshake
    /// does); `None` over a channel that proves nothing, where the key comes from the exchange.
    pub remote: Option<KeyMaterial>,
    /// Whether the channel also verified that `remote` is the peer this side meant to reach: a
    /// pin of it, or roots for the name dialled. Proving a key is not that: any certificate
    /// whose key is proven passes `Trust::AnyWithKeyProof`. The device flow's joiner needs it.
    pub verified: bool,
    /// Bytes both ends of this channel and nobody else share (a TLS exporter), bound into the
    /// PIN confirmation so a relay between two channels fails it; `None` without one.
    pub binding: Option<Vec<u8>>,
}

impl fmt::Debug for Channel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Channel")
            .field("local", &self.local.id())
            .field("remote", &self.remote.as_ref().map(KeyMaterial::id))
            .field("verified", &self.verified)
            .field("binding", &self.binding.as_ref().map(|_| ".."))
            .finish()
    }
}

/// The pairing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Mode {
    /// An invitation secret.
    Invitation,
    /// A PIN and a PAKE.
    Pin,
    /// The device flow.
    Device,
}

/// Which side of a PIN pairing shows the PIN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PinRole {
    /// The responder shows it ([`PairingDesk::show_pin`]); a person typed it at the joiner.
    Typed,
    /// The joiner shows it, with the slot the responder assigns; a person types both at the
    /// responder ([`PairingDesk::submit_code`]).
    Shown,
}

/// What the joiner must show a person, as data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Shown {
    /// This joiner's PIN and its slot, to be typed at the responder.
    Pin(PairingCode),
    /// The device-flow user code, to be entered at the responder before `expires`.
    UserCode {
        /// The code.
        code: UserCode,
        /// When it expires.
        expires: SystemTime,
    },
}

/// A successful pairing: the other side's key, now to be trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Paired {
    /// The other side's key. A responder has accepted it in its store; a joiner's caller
    /// stores it.
    pub peer: KeyMaterial,
    /// How it was paired.
    pub mode: Mode,
}

#[derive(Clone)]
enum Kind {
    Invitation {
        inviter: PeerId,
        secret: Zeroizing<[u8; 32]>,
    },
    #[cfg(feature = "pin")]
    Pin {
        pin: Pin,
        role: PinRole,
    },
    Device {
        label: String,
    },
}

/// How a joiner pairs: build one, then [`join`](Pairing::join) a channel.
#[derive(Clone)]
pub struct Pairing {
    kind: Kind,
    allow_unverified: bool,
    #[cfg(feature = "pin")]
    random: Option<Random>,
}

impl fmt::Debug for Pairing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pairing")
            .field("mode", &self.mode())
            .field("allow_unverified", &self.allow_unverified)
            .finish_non_exhaustive()
    }
}

impl Pairing {
    /// Pairs with an invitation: `secret` from the inviter, who is `inviter`. The joiner sends
    /// the secret only when the channel proved `inviter`'s key, so it must be confidential and
    /// authenticate the inviter (a TLS connection that pins it).
    pub fn invitation(inviter: PeerId, secret: [u8; 32]) -> Self {
        Self::of(Kind::Invitation {
            inviter,
            secret: Zeroizing::new(secret),
        })
    }

    /// Pairs with a PIN: the one the responder shows and a person typed here
    /// ([`PinRole::Typed`]), or this joiner's own, which it shows with the slot the responder
    /// assigns ([`PinRole::Shown`]). The PAKE draws from the operating system's generator
    /// unless [`random`](Pairing::random) gives another.
    #[cfg(feature = "pin")]
    pub fn pin(pin: Pin, role: PinRole) -> Self {
        Self::of(Kind::Pin { pin, role })
    }

    /// Pairs by the device flow: the responder issues a user code that this joiner shows, and
    /// pairs when its operator approves. `label` (at most 255 bytes, one line) is shown to the
    /// operator if the responder's app chooses; it proves nothing.
    ///
    /// The joiner must have verified the responder (a pin, or roots for the name dialled):
    /// [`join`](Pairing::join) refuses a channel that is not [`Channel::verified`] unless
    /// [`allow_unverified`](Pairing::allow_unverified) is set.
    pub fn device(label: &str) -> Self {
        Self::of(Kind::Device {
            label: label.to_owned(),
        })
    }

    fn of(kind: Kind) -> Self {
        Self {
            kind,
            allow_unverified: false,
            #[cfg(feature = "pin")]
            random: None,
        }
    }

    /// Lets the device flow run with a responder this joiner did not verify. A relaying
    /// attacker can then pair in place of both sides: the operator approves the attacker's
    /// code, and this joiner trusts the attacker. Off by default.
    #[must_use]
    pub fn allow_unverified(mut self, allow: bool) -> Self {
        self.allow_unverified = allow;
        self
    }

    /// Whether [`allow_unverified`](Pairing::allow_unverified) is set.
    pub fn allows_unverified(&self) -> bool {
        self.allow_unverified
    }

    /// The PAKE's random source, instead of the operating system's.
    #[cfg(feature = "pin")]
    #[must_use]
    pub fn random(mut self, random: Random) -> Self {
        self.random = Some(random);
        self
    }

    /// The mode.
    pub fn mode(&self) -> Mode {
        match self.kind {
            Kind::Invitation { .. } => Mode::Invitation,
            #[cfg(feature = "pin")]
            Kind::Pin { .. } => Mode::Pin,
            Kind::Device { .. } => Mode::Device,
        }
    }

    /// The inviter's id, for an invitation.
    pub fn inviter(&self) -> Option<PeerId> {
        match self.kind {
            Kind::Invitation { inviter, .. } => Some(inviter),
            _ => None,
        }
    }

    /// Starts the exchange over `channel`; the first message is ready to
    /// [`transmit`](PairingJoiner::transmit).
    ///
    /// # Errors
    ///
    /// [`PairingError::Unverified`] for an invitation whose channel did not prove the
    /// inviter's key, and for the device flow over a channel that did not verify the responder
    /// unless allowed;
    /// [`PairingError::Local`] with [`Error::InvalidOptions`] for a device label longer than
    /// 255 bytes or with a control or bidirectional formatting character.
    pub fn join(self, channel: Channel) -> Result<PairingJoiner, PairingError> {
        let mode_byte = match &self.kind {
            Kind::Invitation { inviter, .. } => {
                if channel.remote.as_ref().map(KeyMaterial::id) != Some(*inviter) {
                    return Err(PairingError::Unverified);
                }
                MODE_INVITATION
            }
            #[cfg(feature = "pin")]
            Kind::Pin { role, .. } => match role {
                PinRole::Typed => MODE_PIN_TYPED,
                PinRole::Shown => MODE_PIN_SHOWN,
            },
            Kind::Device { label } => {
                let verified = channel.verified && channel.remote.is_some();
                if !verified && !self.allow_unverified {
                    return Err(PairingError::Unverified);
                }
                if label.len() > 255 || !plain(label) {
                    return Err(PairingError::Local(Error::InvalidOptions));
                }
                MODE_DEVICE
            }
        };
        let mut hello = Writer::new(HELLO);
        hello.byte(mode_byte);
        hello.key(&channel.local);
        match &self.kind {
            Kind::Invitation { secret, .. } => hello.raw(&secret[..]),
            #[cfg(feature = "pin")]
            Kind::Pin { .. } => {}
            Kind::Device { label } => hello.short(label.as_bytes()),
        }
        let mut joiner = PairingJoiner {
            mode: self.mode(),
            remote: channel.remote.clone(),
            channel,
            pairing: self,
            out: VecDeque::new(),
            state: Joining::AwaitReply,
            shown: None,
        };
        joiner.out.push_back(hello.finish());
        Ok(joiner)
    }
}

enum Joining {
    AwaitReply,
    #[cfg(feature = "pin")]
    AwaitPake {
        pake: pin::Pake,
        slot: Option<u8>,
        sent: Vec<u8>,
    },
    AwaitDone,
    Complete,
    Failed,
}

/// The joiner's side of a pairing exchange.
pub struct PairingJoiner {
    pairing: Pairing,
    mode: Mode,
    channel: Channel,
    remote: Option<KeyMaterial>,
    out: VecDeque<Vec<u8>>,
    state: Joining,
    shown: Option<Shown>,
}

impl fmt::Debug for PairingJoiner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingJoiner")
            .field("mode", &self.mode)
            .field("complete", &self.is_complete())
            .finish_non_exhaustive()
    }
}

impl PairingJoiner {
    /// The next message to send, or `None` until more arrives. Call until `None`.
    pub fn transmit(&mut self) -> Option<Vec<u8>> {
        self.out.pop_front()
    }

    /// What this joiner must show a person now, once the responder sent it.
    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    /// Whether the exchange succeeded and [`finish`](PairingJoiner::finish) returns the peer.
    pub fn is_complete(&self) -> bool {
        matches!(self.state, Joining::Complete)
    }

    /// The paired responder.
    ///
    /// # Errors
    ///
    /// [`PairingError::Protocol`] before completion.
    pub fn finish(self) -> Result<Paired, PairingError> {
        match (self.state, self.remote) {
            (Joining::Complete, Some(peer)) => Ok(Paired {
                peer,
                mode: self.mode,
            }),
            _ => Err(PairingError::Protocol),
        }
    }

    /// Takes one message from the responder.
    ///
    /// # Errors
    ///
    /// [`PairingError::Refused`] when the responder refused or the key confirmation failed
    /// (a refusal is then queued for the responder); [`PairingError::Protocol`] for a
    /// malformed or unexpected message. The exchange is over after an error.
    pub fn receive(&mut self, message: &[u8]) -> Result<(), PairingError> {
        let result = self.step(message);
        if result.is_err() {
            self.state = Joining::Failed;
        }
        result
    }

    /// Records the responder's key, which must be the one the channel proved, or the one an
    /// earlier message carried.
    fn responder_key(&mut self, key: KeyMaterial) -> Result<(), PairingError> {
        match &self.remote {
            Some(known) if *known != key => Err(PairingError::Protocol),
            Some(_) => Ok(()),
            None => {
                self.remote = Some(key);
                Ok(())
            }
        }
    }

    fn refuse(&mut self, refusal: PairRefusal) -> PairingError {
        self.out.push_back(refused(refusal));
        PairingError::Refused(refusal)
    }

    fn step(&mut self, message: &[u8]) -> Result<(), PairingError> {
        let (tag, mut reader) = Reader::open(message)?;
        if tag == REFUSED {
            let refusal = PairRefusal::from_wire(reader.byte()?).ok_or(PairingError::Protocol)?;
            reader.end()?;
            return Err(PairingError::Refused(refusal));
        }
        match (core::mem::replace(&mut self.state, Joining::Failed), tag) {
            (Joining::AwaitReply, DONE) if self.mode == Mode::Invitation => {
                let key = reader.key()?;
                reader.end()?;
                self.responder_key(key)?;
                self.state = Joining::Complete;
                Ok(())
            }
            (Joining::AwaitReply, ISSUED) if self.mode == Mode::Device => {
                let key = reader.key()?;
                let code = String::from_utf8(reader.short()?.to_vec())
                    .map_err(|_| PairingError::Protocol)?;
                if code.is_empty() || !plain(&code) {
                    return Err(PairingError::Protocol);
                }
                let expires = from_unix_seconds(reader.long()?);
                reader.end()?;
                self.responder_key(key)?;
                self.shown = Some(Shown::UserCode {
                    code: UserCode::issued(code),
                    expires,
                });
                self.state = Joining::AwaitDone;
                Ok(())
            }
            #[cfg(feature = "pin")]
            (Joining::AwaitReply, READY) if self.mode == Mode::Pin => {
                let key = reader.key()?;
                let slot = reader.byte()?;
                reader.end()?;
                self.responder_key(key)?;
                let Kind::Pin { pin, role } = self.pairing.kind.clone() else {
                    return Err(PairingError::Protocol);
                };
                let slot = (slot != NO_SLOT).then_some(slot);
                if (role == PinRole::Shown) != slot.is_some() {
                    return Err(PairingError::Protocol);
                }
                if role == PinRole::Shown {
                    self.shown = Some(Shown::Pin(PairingCode {
                        slot,
                        pin: pin.clone(),
                    }));
                }
                let remote = self.remote.as_ref().ok_or(PairingError::Protocol)?;
                let random = self.pairing.random.clone().unwrap_or_else(Random::os);
                let (pake, sent) =
                    pin::start(true, &pin, &self.channel.local.id(), &remote.id(), &random)?;
                let mut out = Writer::new(PAKE);
                out.short(&sent);
                self.out.push_back(out.finish());
                self.state = Joining::AwaitPake { pake, slot, sent };
                Ok(())
            }
            #[cfg(feature = "pin")]
            (Joining::AwaitPake { pake, slot, sent }, REPLY) => {
                let received = reader.short()?.to_vec();
                let tag = reader.tag()?;
                reader.end()?;
                let Some(keys) = pin::finish(pake, &received) else {
                    return Err(self.refuse(PairRefusal::WrongCode));
                };
                let remote = self.remote.as_ref().ok_or(PairingError::Protocol)?;
                let record = transcript(
                    &self.channel.local,
                    remote,
                    slot,
                    &sent,
                    &received,
                    &self.channel.binding,
                );
                if !pin::verify(&keys.responder, &record, &tag) {
                    return Err(self.refuse(PairRefusal::WrongCode));
                }
                let mac = pin::mac(&keys.joiner, &record).ok_or(PairingError::Protocol)?;
                let mut out = Writer::new(CONFIRM);
                out.raw(&mac);
                self.out.push_back(out.finish());
                self.state = Joining::AwaitDone;
                Ok(())
            }
            (Joining::AwaitDone, DONE) => {
                let key = reader.key()?;
                reader.end()?;
                self.responder_key(key)?;
                self.state = Joining::Complete;
                Ok(())
            }
            _ => Err(PairingError::Protocol),
        }
    }
}

enum Responding {
    AwaitHello,
    #[cfg(feature = "pin")]
    AwaitPake {
        typed: bool,
        slot: Option<u8>,
    },
    #[cfg(feature = "pin")]
    AwaitCode {
        received: Vec<u8>,
        slot: Option<u8>,
    },
    #[cfg(feature = "pin")]
    AwaitConfirm {
        key: Zeroizing<[u8; 32]>,
        record: Vec<u8>,
    },
    AwaitDecision,
    Complete,
    Failed,
}

/// The responder's side of a pairing exchange: it redeems invitations from its
/// [`TrustStore`], takes PINs and decisions from its [`PairingDesk`], and accepts the joiner's
/// key in the store when the pairing succeeds.
pub struct PairingResponder {
    channel: Channel,
    store: TrustStore,
    desk: PairingDesk,
    source: Option<SocketAddr>,
    joiner: Option<KeyMaterial>,
    mode: Option<Mode>,
    admission: Option<Admission>,
    deadline: Option<SystemTime>,
    out: VecDeque<Vec<u8>>,
    state: Responding,
}

impl fmt::Debug for PairingResponder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingResponder")
            .field("mode", &self.mode)
            .field("complete", &self.is_complete())
            .finish_non_exhaustive()
    }
}

impl PairingResponder {
    /// A responder over `channel`, for a joiner that connected from `source` (used by the
    /// desk's per-source throttle).
    pub fn new(
        channel: Channel,
        store: &TrustStore,
        desk: &PairingDesk,
        source: Option<SocketAddr>,
    ) -> Self {
        Self {
            channel,
            store: store.clone(),
            desk: desk.clone(),
            source,
            joiner: None,
            mode: None,
            admission: None,
            deadline: None,
            out: VecDeque::new(),
            state: Responding::AwaitHello,
        }
    }

    /// The next message to send, or `None`. Call until `None`.
    pub fn transmit(&mut self) -> Option<Vec<u8>> {
        self.out.pop_front()
    }

    /// The decision this responder waits for, when it waits: hand it to
    /// [`PairingDesk::wait`] or [`PairingDesk::decided`], then to
    /// [`resume`](PairingResponder::resume).
    pub fn awaiting(&self) -> Option<Ticket> {
        let waits = match self.state {
            #[cfg(feature = "pin")]
            Responding::AwaitCode { .. } => true,
            Responding::AwaitDecision => true,
            _ => false,
        };
        waits
            .then(|| self.admission.as_ref().map(Admission::ticket))
            .flatten()
    }

    /// The joiner's key, once its first message arrived.
    pub fn joiner(&self) -> Option<&KeyMaterial> {
        self.joiner.as_ref()
    }

    /// Whether the exchange succeeded.
    pub fn is_complete(&self) -> bool {
        matches!(self.state, Responding::Complete)
    }

    /// The paired joiner, already accepted in the store.
    ///
    /// # Errors
    ///
    /// [`PairingError::Protocol`] before completion.
    pub fn finish(self) -> Result<Paired, PairingError> {
        match (self.state, self.joiner, self.mode) {
            (Responding::Complete, Some(peer), Some(mode)) => Ok(Paired { peer, mode }),
            _ => Err(PairingError::Protocol),
        }
    }

    /// Takes one message from the joiner.
    ///
    /// # Errors
    ///
    /// [`PairingError::Refused`] when this side refuses (a refusal is then queued for the
    /// joiner) or the joiner did; [`PairingError::Protocol`]; [`PairingError::Local`] when the
    /// store or the random source fails. The exchange is over after an error.
    pub fn receive(&mut self, message: &[u8]) -> Result<(), PairingError> {
        self.check_deadline()?;
        let result = self.step(message);
        if result.is_err() {
            self.state = Responding::Failed;
            self.admission = None;
        }
        result
    }

    /// Settles the decision [`awaiting`](PairingResponder::awaiting) named.
    ///
    /// # Errors
    ///
    /// As [`receive`](PairingResponder::receive); [`PairRefusal::Denied`] or
    /// [`PairRefusal::Expired`] when that is the decision.
    pub fn resume(&mut self, decision: Decision) -> Result<(), PairingError> {
        self.check_deadline()?;
        let result = self.decide(decision);
        if result.is_err() {
            self.state = Responding::Failed;
            self.admission = None;
        }
        result
    }

    /// When this exchange must be over, on the desk's clock: from the joiner's first message,
    /// [`PairingOptions::pin_ttl`] for a PIN and [`PairingOptions::device_ttl`] for the device
    /// flow. `None` before that message, and for an invitation, which ends with it. A transport
    /// reads with a timeout up to it, then calls [`check_deadline`](PairingResponder::check_deadline).
    pub fn deadline(&self) -> Option<SystemTime> {
        self.deadline
    }

    /// How long until the [`deadline`](PairingResponder::deadline), on the desk's clock: zero
    /// once it passed, `None` while there is none. A transport's read timeout.
    pub fn time_left(&self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.duration_since(self.desk.now()).unwrap_or_default())
    }

    /// Ends the exchange once its [`deadline`](PairingResponder::deadline) passed: the joiner is
    /// told `Expired`, and the slot or user code is released. [`receive`](PairingResponder::receive)
    /// and [`resume`](PairingResponder::resume) check it first.
    ///
    /// # Errors
    ///
    /// [`PairRefusal::Expired`] when the deadline passed.
    pub fn check_deadline(&mut self) -> Result<(), PairingError> {
        if matches!(self.state, Responding::Complete | Responding::Failed) {
            return Ok(());
        }
        if self
            .deadline
            .is_some_and(|deadline| self.desk.now() >= deadline)
        {
            self.state = Responding::Failed;
            self.admission = None;
            return Err(self.refuse(PairRefusal::Expired));
        }
        Ok(())
    }

    fn refuse(&mut self, refusal: PairRefusal) -> PairingError {
        self.out.push_back(refused(refusal));
        PairingError::Refused(refusal)
    }

    /// Accepts the joiner and tells it so.
    fn accept(&mut self) -> Result<(), PairingError> {
        let joiner = self.joiner.as_ref().ok_or(PairingError::Protocol)?;
        match self.store.accept_key(joiner) {
            Ok(_) | Err(Error::AlreadyAccepted) => {}
            Err(error) => return Err(PairingError::Local(error)),
        }
        let mut done = Writer::new(DONE);
        done.key(&self.channel.local);
        self.out.push_back(done.finish());
        self.state = Responding::Complete;
        self.admission = None;
        Ok(())
    }

    fn decide(&mut self, decision: Decision) -> Result<(), PairingError> {
        match (
            core::mem::replace(&mut self.state, Responding::Failed),
            decision,
        ) {
            (Responding::AwaitDecision, Decision::Approved) => self.accept(),
            (Responding::AwaitDecision, Decision::Denied) => Err(self.refuse(PairRefusal::Denied)),
            #[cfg(feature = "pin")]
            (Responding::AwaitCode { received, slot }, Decision::Code(pin)) => {
                self.reply(&pin, &received, slot)
            }
            (_, Decision::Expired) => Err(self.refuse(PairRefusal::Expired)),
            _ => Err(PairingError::Protocol),
        }
    }

    #[cfg(feature = "pin")]
    fn reply(&mut self, pin: &Pin, received: &[u8], slot: Option<u8>) -> Result<(), PairingError> {
        let joiner = self.joiner.clone().ok_or(PairingError::Protocol)?;
        let random = self.desk.random();
        let (pake, sent) = pin::start(false, pin, &joiner.id(), &self.channel.local.id(), &random)?;
        let Some(keys) = pin::finish(pake, received) else {
            return Err(self.refuse(PairRefusal::WrongCode));
        };
        let record = transcript(
            &joiner,
            &self.channel.local,
            slot,
            received,
            &sent,
            &self.channel.binding,
        );
        let mac = pin::mac(&keys.responder, &record).ok_or(PairingError::Protocol)?;
        let mut out = Writer::new(REPLY);
        out.short(&sent);
        out.raw(&mac);
        self.out.push_back(out.finish());
        self.state = Responding::AwaitConfirm {
            key: Zeroizing::new(keys.joiner),
            record,
        };
        Ok(())
    }

    fn step(&mut self, message: &[u8]) -> Result<(), PairingError> {
        let (tag, mut reader) = Reader::open(message)?;
        if tag == REFUSED {
            let refusal = PairRefusal::from_wire(reader.byte()?).ok_or(PairingError::Protocol)?;
            reader.end()?;
            return Err(PairingError::Refused(refusal));
        }
        match (core::mem::replace(&mut self.state, Responding::Failed), tag) {
            (Responding::AwaitHello, HELLO) => self.hello(&mut reader),
            #[cfg(feature = "pin")]
            (Responding::AwaitPake { typed, slot }, PAKE) => {
                let received = reader.short()?.to_vec();
                reader.end()?;
                if typed {
                    match self.desk.take_pin() {
                        Ok(pin) => self.reply(&pin, &received, slot),
                        Err(refusal) => Err(self.refuse(refusal)),
                    }
                } else {
                    self.state = Responding::AwaitCode { received, slot };
                    Ok(())
                }
            }
            #[cfg(feature = "pin")]
            (Responding::AwaitConfirm { key, record }, CONFIRM) => {
                let tag = reader.tag()?;
                reader.end()?;
                if pin::verify(&key, &record, &tag) {
                    self.accept()
                } else {
                    Err(self.refuse(PairRefusal::WrongCode))
                }
            }
            _ => Err(PairingError::Protocol),
        }
    }

    fn hello(&mut self, reader: &mut Reader<'_>) -> Result<(), PairingError> {
        let mode = reader.byte()?;
        let joiner = reader.key()?;
        if self
            .channel
            .remote
            .as_ref()
            .is_some_and(|remote| *remote != joiner)
        {
            return Err(PairingError::Protocol);
        }
        let options = self.desk.options();
        let (kind, enabled) = match mode {
            MODE_INVITATION => (Mode::Invitation, options.invitation),
            MODE_PIN_SHOWN if !options.pin_shown => (Mode::Pin, false),
            MODE_PIN_TYPED | MODE_PIN_SHOWN => (Mode::Pin, options.pin && cfg!(feature = "pin")),
            MODE_DEVICE => (Mode::Device, options.device),
            _ => return Err(PairingError::Protocol),
        };
        self.joiner = Some(joiner.clone());
        self.mode = Some(kind);
        if !enabled {
            return Err(self.refuse(PairRefusal::Disabled));
        }
        let admission = match self.desk.admit(self.source) {
            Ok(admission) => admission,
            Err(refusal) => return Err(self.refuse(refusal)),
        };
        let ttl = match mode {
            MODE_DEVICE => Some(options.device_ttl),
            MODE_PIN_TYPED | MODE_PIN_SHOWN => Some(options.pin_ttl),
            _ => None,
        };
        self.deadline = ttl.map(|ttl| later(self.desk.now(), ttl));
        match mode {
            MODE_INVITATION => {
                let secret: Zeroizing<[u8; 32]> = Zeroizing::new(
                    reader
                        .take(32)?
                        .try_into()
                        .map_err(|_| PairingError::Protocol)?,
                );
                reader.end()?;
                self.admission = Some(admission);
                match self.store.redeem(&secret) {
                    Ok(_) => self.accept(),
                    Err(Error::NotInvited) => Err(self.refuse(PairRefusal::WrongCode)),
                    Err(Error::Expired) => Err(self.refuse(PairRefusal::Expired)),
                    Err(error) => Err(PairingError::Local(error)),
                }
            }
            MODE_DEVICE => {
                let label = String::from_utf8(reader.short()?.to_vec())
                    .map_err(|_| PairingError::Protocol)?;
                if !plain(&label) {
                    return Err(PairingError::Protocol);
                }
                reader.end()?;
                let (code, expires) =
                    self.desk
                        .issue(&admission, joiner.id(), self.source, &label)?;
                let mut issued = Writer::new(ISSUED);
                issued.key(&self.channel.local);
                issued.short(code.as_str().as_bytes());
                issued.long(unix_seconds(expires));
                self.out.push_back(issued.finish());
                self.admission = Some(admission);
                self.state = Responding::AwaitDecision;
                Ok(())
            }
            #[cfg(feature = "pin")]
            _ => {
                reader.end()?;
                if let Err(refusal) = self.desk.pin_run(self.source) {
                    return Err(self.refuse(refusal));
                }
                let typed = mode == MODE_PIN_TYPED;
                let slot = if typed {
                    if let Err(refusal) = self.desk.pin_ready() {
                        return Err(self.refuse(refusal));
                    }
                    None
                } else {
                    Some(self.desk.open_slot(&admission, joiner.id(), self.source)?)
                };
                let mut ready = Writer::new(READY);
                ready.key(&self.channel.local);
                ready.byte(slot.unwrap_or(NO_SLOT));
                self.out.push_back(ready.finish());
                self.admission = Some(admission);
                self.state = Responding::AwaitPake { typed, slot };
                Ok(())
            }
            #[cfg(not(feature = "pin"))]
            _ => Err(self.refuse(PairRefusal::Disabled)),
        }
    }
}

/// The key-rotation announcement: the message a peer sends, over a session its current key
/// authenticated, to name its successor key.
#[derive(Debug, Clone, Copy)]
pub struct Rotation;

impl Rotation {
    /// The announcement of `successor`.
    pub fn announce(successor: &KeyMaterial) -> Vec<u8> {
        let mut out = Writer::new(ANNOUNCE);
        out.key(successor);
        out.finish()
    }

    /// The successor key an announcement names.
    ///
    /// # Errors
    ///
    /// [`PairingError::Protocol`] when `message` is not an announcement.
    pub fn parse(message: &[u8]) -> Result<KeyMaterial, PairingError> {
        let (tag, mut reader) = Reader::open(message)?;
        if tag != ANNOUNCE {
            return Err(PairingError::Protocol);
        }
        let key = reader.key()?;
        reader.end()?;
        Ok(key)
    }
}

/// Whether `text` is safe to show as it is: no control characters (line breaks included) and
/// no bidirectional formatting characters, which can make a label or code read as another.
fn plain(text: &str) -> bool {
    !text.chars().any(|c| {
        c.is_control()
            || matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}')
            || ('\u{202A}'..='\u{202E}').contains(&c)
            || ('\u{2066}'..='\u{2069}').contains(&c)
    })
}

fn refused(refusal: PairRefusal) -> Vec<u8> {
    let mut out = Writer::new(REFUSED);
    out.byte(refusal.wire());
    out.finish()
}

/// What both PIN confirmations cover: both keys, the slot, both PAKE messages and the channel
/// binding, each length-prefixed.
#[cfg(feature = "pin")]
fn transcript(
    joiner: &KeyMaterial,
    responder: &KeyMaterial,
    slot: Option<u8>,
    joiner_message: &[u8],
    responder_message: &[u8],
    binding: &Option<Vec<u8>>,
) -> Vec<u8> {
    let mut out = Writer(Vec::new());
    out.raw(b"coqui-pair pin");
    out.key(joiner);
    out.key(responder);
    out.byte(slot.unwrap_or(NO_SLOT));
    out.short(joiner_message);
    out.short(responder_message);
    match binding {
        Some(binding) => {
            out.byte(1);
            out.wide(binding);
        }
        None => out.byte(0),
    }
    out.finish()
}

/// Builds a message: the version, a tag, then fields.
struct Writer(Vec<u8>);

impl Writer {
    fn new(tag: u8) -> Self {
        Self(vec![VERSION, tag])
    }

    fn byte(&mut self, value: u8) {
        self.0.push(value);
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    /// At most 255 bytes, after a one-byte length.
    fn short(&mut self, bytes: &[u8]) {
        let bytes = &bytes[..bytes.len().min(255)];
        self.0.push(bytes.len() as u8);
        self.0.extend_from_slice(bytes);
    }

    /// After a four-byte big-endian length.
    fn wide(&mut self, bytes: &[u8]) {
        let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        self.0.extend_from_slice(&length.to_be_bytes());
        self.0.extend_from_slice(&bytes[..length as usize]);
    }

    fn long(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    /// The method, the label, then the bytes.
    fn key(&mut self, key: &KeyMaterial) {
        self.short(key.method.as_bytes());
        self.short(key.label.as_bytes());
        self.wide(&key.bytes);
    }

    fn finish(self) -> Vec<u8> {
        self.0
    }
}

/// Reads a message's fields; every shortfall is [`PairingError::Protocol`].
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    /// A reader of `bytes` past the version, and the message tag.
    fn open(bytes: &'a [u8]) -> Result<(u8, Self), PairingError> {
        let mut reader = Self(bytes);
        if reader.byte()? != VERSION {
            return Err(PairingError::Protocol);
        }
        let tag = reader.byte()?;
        Ok((tag, reader))
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], PairingError> {
        if self.0.len() < count {
            return Err(PairingError::Protocol);
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8, PairingError> {
        Ok(self.take(1)?[0])
    }

    fn short(&mut self) -> Result<&'a [u8], PairingError> {
        let length = usize::from(self.byte()?);
        self.take(length)
    }

    fn wide(&mut self) -> Result<&'a [u8], PairingError> {
        let length: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| PairingError::Protocol)?;
        self.take(usize::try_from(u32::from_be_bytes(length)).map_err(|_| PairingError::Protocol)?)
    }

    fn long(&mut self) -> Result<u64, PairingError> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| PairingError::Protocol)?;
        Ok(u64::from_be_bytes(bytes))
    }

    #[cfg(feature = "pin")]
    fn tag(&mut self) -> Result<[u8; 32], PairingError> {
        self.take(32)?
            .try_into()
            .map_err(|_| PairingError::Protocol)
    }

    fn key(&mut self) -> Result<KeyMaterial, PairingError> {
        let method = core::str::from_utf8(self.short()?).map_err(|_| PairingError::Protocol)?;
        let label = core::str::from_utf8(self.short()?).map_err(|_| PairingError::Protocol)?;
        let bytes = self.wide()?;
        KeyMaterial::new(method, label, bytes).map_err(|_| PairingError::Protocol)
    }

    fn end(&self) -> Result<(), PairingError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(PairingError::Protocol)
        }
    }
}
