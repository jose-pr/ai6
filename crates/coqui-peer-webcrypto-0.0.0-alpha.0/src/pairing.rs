use alloc::sync::Arc;
use core::fmt;
use core::net::SocketAddr;
use core::time::Duration;
use std::io;
use std::time::SystemTime;

use coqui_builder::NonDefault;
use coqui_peer::pair::{
    Channel, Decision, Paired, Pairing, PairingDesk, PairingJoiner, PairingResponder, PairingStore,
    Shown, Ticket,
};
use coqui_peer::{
    Established, Framing, Handshake, KeyMaterial, MessageTransport, Method, PeerId, Random,
    ReadTimeout,
};

use crate::error::Error;
use crate::handshake::{Config, Exchange, Finished, METHOD};
use crate::identity::Identity;
use crate::session::Session;
use crate::settings::{PairingSettings, WebCryptoSettings};
use crate::trust::{Admission, Trust};

/// The export label [`PairingSession::pair_channel`] binds a pairing to.
pub const PAIR_EXPORT_LABEL: &str = "EXPORTER-coqui-pair";

/// The method for a connection that only pairs: the responder admits whoever proves a key, since a
/// joiner is a stranger, and what it ends in is a [`PairingSession`], which carries the pairing
/// exchange and nothing else. No application message can pass over it.
///
/// A joiner that knows who it is reaching says so with
/// [`responders`](PairingMethodBuilder::responders); an invitation and the device flow need it.
#[derive(Clone)]
pub struct PairingMethod(Arc<Config>);

/// What a [`PairingMethodBuilder`] collects: the settings, the identity and the trust a joiner
/// applies to its responder.
///
/// Build the method with [`PairingMethod::builder`]; the settings' setter (`max_message`) is a
/// method of the builder, and `fill` layers a `Partial<PairingSettings>` or a source over it.
/// A responder admits any proved key, so there is no `initiators`.
#[derive(coqui_builder::Builder, Debug)]
#[builder(build = PairingMethod)]
pub struct PairingMethodParts {
    settings: PairingSettings,
    /// Whose key the method proves; required.
    #[builder(input)]
    identity: Identity,
    /// Whom a joiner accepts: a pin or a store. The session is then verified, which the device flow
    /// asks for; the key an invitation names must be the proved one in any case. Any proved key
    /// until it is given.
    #[builder(input)]
    responders: Option<Trust>,
    /// As [`WebCryptoParts`]'s `random`.
    #[builder(input)]
    random: Option<Random>,
}

impl From<PairingMethodParts> for PairingMethod {
    fn from(parts: PairingMethodParts) -> Self {
        Self(Arc::new(Config {
            identity: parts.identity,
            responders: Some(parts.responders.map_or(Admission::Any, Admission::Trust)),
            initiators: Some(Admission::Any),
            settings: WebCryptoSettings {
                max_message: parts.settings.max_message,
                ..WebCryptoSettings::default()
            },
            random_replaced: parts.random.is_some(),
            random: parts.random.unwrap_or_else(crate::random),
        }))
    }
}

impl PairingMethod {
    /// The limit it was built with.
    #[must_use]
    pub fn settings(&self) -> PairingSettings {
        PairingSettings {
            max_message: self.0.settings.max_message,
        }
    }

    /// Every setting that differs from its default, named by key path, and `random` when the
    /// nonces and ephemeral keys do not come from the operating system. A pairing method admits
    /// any proved key by definition, so that is not listed.
    #[must_use]
    pub fn non_defaults(&self) -> Vec<NonDefault> {
        let mut found = coqui_builder::non_defaults(&self.settings());
        found.extend(self.0.random_entry());
        found
    }
}

impl fmt::Debug for PairingMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingMethod")
            .field("identity", &self.0.identity)
            .finish_non_exhaustive()
    }
}

impl Method for PairingMethod {
    type Handshake = PairingHandshake;

    fn name(&self) -> &'static str {
        METHOD
    }

    /// Starts a handshake as the joiner; the hello is ready at once.
    ///
    /// # Errors
    ///
    /// [`Error::Random`] when the generator fails.
    fn initiate(&self) -> Result<PairingHandshake, Error> {
        Exchange::initiate(&self.0).map(PairingHandshake)
    }

    /// Starts a handshake as the responder.
    ///
    /// # Errors
    ///
    /// None in practice: this role is always configured.
    fn respond(&self, source: Option<SocketAddr>) -> Result<PairingHandshake, Error> {
        Exchange::respond(&self.0, source).map(PairingHandshake)
    }
}

/// One run of the [`PairingMethod`], driven by the caller. `Output` is the [`PairingSession`].
pub struct PairingHandshake(Exchange);

impl fmt::Debug for PairingHandshake {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingHandshake")
            .field("complete", &self.0.is_complete())
            .finish_non_exhaustive()
    }
}

impl Handshake for PairingHandshake {
    type Output = PairingSession;
    type Error = Error;

    fn framing(&self) -> Framing {
        Framing::Message
    }

    fn receive(&mut self, input: &[u8]) -> Result<(), Error> {
        self.0.receive(input)
    }

    fn transmit(&mut self) -> Result<Option<Vec<u8>>, Error> {
        self.0.transmit()
    }

    fn is_complete(&self) -> bool {
        self.0.is_complete()
    }

    fn finish(self) -> Result<Established<PairingSession>, Error> {
        let local = self.0.local_key();
        let Finished {
            peer,
            id,
            keys,
            initiator,
            identified,
            limit,
        } = self.0.finish()?;
        Ok(Established {
            peer: peer.id(),
            key: Some(peer.clone()),
            session: PairingSession {
                session: Session::new(id, &keys, initiator, limit),
                local,
                peer,
                verified: identified,
            },
        })
    }
}

/// What a [`PairingMethod`] handshake yields: a proved key that is nobody yet. It runs one
/// [`coqui_peer::pair`] exchange with its messages sealed, and offers nothing else: after the
/// exchange, both sides reconnect under [`WebCrypto`](crate::WebCrypto).
///
/// It carries no application message:
///
/// ```compile_fail
/// fn send(session: &mut coqui_peer_webcrypto::PairingSession) {
///     let _ = session.seal(b"anything");
/// }
/// ```
pub struct PairingSession {
    session: Session,
    local: KeyMaterial,
    peer: KeyMaterial,
    verified: bool,
}

impl fmt::Debug for PairingSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingSession")
            .field("peer", &self.peer.id())
            .field("verified", &self.verified)
            .finish_non_exhaustive()
    }
}

impl PairingSession {
    /// The id both ends agreed on, as [`Session::id`].
    #[must_use]
    pub const fn id(&self) -> &[u8; 32] {
        self.session.id()
    }

    /// The key the peer proved.
    #[must_use]
    pub const fn peer(&self) -> &KeyMaterial {
        &self.peer
    }

    /// The id of the key the peer proved.
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        self.peer.id()
    }

    /// The [`Channel`] an exchange runs on: `local` this side's key, `remote` the key this handshake
    /// proved, which no caller can name instead, `verified` when a joiner's
    /// [`responders`](PairingMethodBuilder::responders) admitted the peer, and `binding` a 32-byte
    /// export under [`PAIR_EXPORT_LABEL`], tying the exchange to this session and not to any TLS
    /// connection beneath it.
    ///
    /// # Errors
    ///
    /// None in practice: 32 bytes are within what [`Session::export`] gives.
    pub fn pair_channel(&self) -> Result<Channel, Error> {
        let mut binding = vec![0u8; 32];
        self.session.export(PAIR_EXPORT_LABEL, &mut binding)?;
        Ok(Channel {
            local: self.local.clone(),
            remote: Some(self.peer.clone()),
            verified: self.verified,
            binding: Some(binding),
        })
    }

    /// Joins with `pairing`, each message sealed, until there is something to show a person
    /// ([`PairDial::shown`]) or the end. An invitation's secret is only ever sent sealed, and
    /// [`Pairing::join`] refuses unless the proved key is the inviter's.
    ///
    /// # Errors
    ///
    /// [`Error::Pairing`] when refused, [`Error::Io`] from the transport, [`Error::Record`] for a
    /// bad record, [`Error::Truncated`] when the responder closed early.
    pub fn join<T: MessageTransport>(
        self,
        pairing: Pairing,
        transport: &mut T,
    ) -> Result<PairDial<'_, T>, Error> {
        let joiner = pairing.join(self.pair_channel()?)?;
        let mut dial = PairDial {
            session: self.session,
            joiner,
            transport,
        };
        dial.run(true)?;
        Ok(dial)
    }

    /// The responder as a step driver, each record sealed: the caller feeds it records and sends what it gives, and decides when
    /// to wait for the operator, for a read, or for the deadline. `store` is a [`TrustStore`](coqui_peer::TrustStore) or a host's own [`PairingStore`];
    /// `source` is what the desk throttles by.
    ///
    /// # Errors
    ///
    /// None in practice: see [`pair_channel`](PairingSession::pair_channel).
    pub fn responder<S: PairingStore + Clone + 'static>(
        self,
        store: &S,
        desk: &PairingDesk,
        source: Option<SocketAddr>,
    ) -> Result<PairResponder, Error> {
        self.responder_with_store(Arc::new(store.clone()), desk, source)
    }

    /// As [`responder`](PairingSession::responder), over a store that is already shared, or that cannot be cloned.
    ///
    /// # Errors
    ///
    /// As [`responder`](PairingSession::responder).
    pub fn responder_with_store(
        self,
        store: Arc<dyn PairingStore>,
        desk: &PairingDesk,
        source: Option<SocketAddr>,
    ) -> Result<PairResponder, Error> {
        let responder = PairingResponder::with_store(self.pair_channel()?, store, desk, source);
        Ok(PairResponder {
            session: self.session,
            responder,
        })
    }

    /// Runs the responder until the end, each message sealed: redeeming an invitation, taking the
    /// shown PIN, or blocking in [`PairingDesk::wait`] for the operator. On success the joiner's key
    /// is accepted in `store`. `source` is what the desk throttles by; behind a reverse proxy, pass
    /// the client's address from a header you trust, or every joiner counts as the proxy.
    ///
    /// Each read waits at most until the exchange's deadline, through `ReadTimeout`, and a read
    /// that times out ends the exchange then; before the first message the transport's own timeout
    /// applies. A host that drives the exchange itself uses [`responder`](PairingSession::responder).
    ///
    /// # Errors
    ///
    /// As [`join`](PairingSession::join).
    pub fn respond<S: PairingStore + Clone + 'static, T: MessageTransport + ReadTimeout>(
        self,
        store: &S,
        desk: &PairingDesk,
        source: Option<SocketAddr>,
        transport: &mut T,
    ) -> Result<Paired, Error> {
        let mut responder = self.responder(store, desk, source)?;
        loop {
            while let Some(record) = responder.transmit()? {
                transport.send(&record)?;
            }
            if responder.is_complete() {
                break;
            }
            let step = match responder.awaiting() {
                Some(ticket) => responder.resume(desk.wait(ticket)),
                None => {
                    transport.set_read_timeout(responder.time_left())?;
                    match transport.recv() {
                        Ok(record) => responder.receive(&record),
                        Err(error) if timed_out(&error) => {
                            responder.check_deadline()?;
                            return Err(error.into());
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            };
            if let Err(error) = step {
                // The joiner is told why, as far as the transport still takes it.
                while let Ok(Some(record)) = responder.transmit() {
                    let _ = transport.send(&record);
                }
                return Err(error);
            }
        }
        let close = responder.close()?;
        let _ = transport.send(&close);
        responder.finish()
    }
}

/// The responder of a pairing exchange over a [`PairingSession`], driven a step at a time: it does no I/O, waits for nothing and
/// seals every record it gives and opens every record it takes. It wraps [`PairingResponder`], whose steps these are; a host that
/// owns its transport and its timers calls them in this order:
///
/// 1. [`transmit`](PairResponder::transmit) until it gives `None`, and send each record;
/// 2. if [`is_complete`](PairResponder::is_complete), [`close`](PairResponder::close), send the record, and
///    [`finish`](PairResponder::finish);
/// 3. else if [`awaiting`](PairResponder::awaiting) is a ticket, wait for the operator on [`PairingDesk::wait`] or
///    [`PairingDesk::decided`] and give the decision to [`resume`](PairResponder::resume);
/// 4. else read one record, bounded by [`time_left`](PairResponder::time_left), and give it to
///    [`receive`](PairResponder::receive); on a read that timed out, [`check_deadline`](PairResponder::check_deadline).
///
/// After an error from `receive`, `resume` or `check_deadline` the exchange is over, and `transmit` still gives the records that tell
/// the joiner why.
pub struct PairResponder {
    session: Session,
    responder: PairingResponder,
}

impl fmt::Debug for PairResponder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairResponder")
            .field("responder", &self.responder)
            .finish_non_exhaustive()
    }
}

impl PairResponder {
    /// The next sealed record to send, or `None`. Call until `None`.
    ///
    /// # Errors
    ///
    /// [`Error::LimitReached`] or [`Error::Closed`] from the session.
    pub fn transmit(&mut self) -> Result<Option<Vec<u8>>, Error> {
        self.responder
            .transmit()
            .map(|message| self.session.seal(&message))
            .transpose()
    }

    /// Takes one record from the joiner.
    ///
    /// # Errors
    ///
    /// [`Error::Record`] for a bad record, [`Error::Truncated`] for the joiner's close before the end, [`Error::Pairing`] when
    /// this side refuses or the joiner did.
    pub fn receive(&mut self, record: &[u8]) -> Result<(), Error> {
        let message = self.session.open(record)?.ok_or(Error::Truncated)?;
        Ok(self.responder.receive(&message)?)
    }

    /// The decision this responder waits for, when it waits: hand it to [`PairingDesk::wait`] or [`PairingDesk::decided`], then
    /// to [`resume`](PairResponder::resume).
    #[must_use]
    pub fn awaiting(&self) -> Option<Ticket> {
        self.responder.awaiting()
    }

    /// Continues with the operator's decision.
    ///
    /// # Errors
    ///
    /// [`Error::Pairing`] when the decision is a refusal or the deadline passed.
    pub fn resume(&mut self, decision: Decision) -> Result<(), Error> {
        Ok(self.responder.resume(decision)?)
    }

    /// When the exchange ends if nothing finishes it, as [`PairingResponder::deadline`].
    #[must_use]
    pub fn deadline(&self) -> Option<SystemTime> {
        self.responder.deadline()
    }

    /// How long until the [`deadline`](PairResponder::deadline), as [`PairingResponder::time_left`]: a read's timeout.
    #[must_use]
    pub fn time_left(&self) -> Option<Duration> {
        self.responder.time_left()
    }

    /// Ends the exchange once its deadline passed, telling the joiner.
    ///
    /// # Errors
    ///
    /// [`Error::Pairing`] with `Expired` when it passed.
    pub fn check_deadline(&mut self) -> Result<(), Error> {
        Ok(self.responder.check_deadline()?)
    }

    /// Whether the exchange succeeded, and what is left is the close record and [`finish`](PairResponder::finish).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.responder.is_complete()
    }

    /// The sealed record that ends this side's direction: the last one to send.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] when it was already given.
    pub fn close(&mut self) -> Result<Vec<u8>, Error> {
        self.session.close()
    }

    /// The paired joiner, already accepted in the store.
    ///
    /// # Errors
    ///
    /// [`Error::Pairing`] before completion.
    pub fn finish(self) -> Result<Paired, Error> {
        Ok(self.responder.finish()?)
    }
}

fn timed_out(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

/// A joiner's exchange in progress: what to show a person, then the rest.
pub struct PairDial<'t, T> {
    session: Session,
    joiner: PairingJoiner,
    transport: &'t mut T,
}

impl<T> fmt::Debug for PairDial<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("PairDial").finish_non_exhaustive()
    }
}

impl<T: MessageTransport> PairDial<'_, T> {
    fn run(&mut self, stop_when_shown: bool) -> Result<(), Error> {
        loop {
            while let Some(message) = self.joiner.transmit() {
                self.transport.send(&self.session.seal(&message)?)?;
            }
            if self.joiner.is_complete() || (stop_when_shown && self.joiner.shown().is_some()) {
                return Ok(());
            }
            let record = self.transport.recv()?;
            let message = self.session.open(&record)?.ok_or(Error::Truncated)?;
            if let Err(error) = self.joiner.receive(&message) {
                // The responder is told why, as far as the transport still takes it.
                while let Some(message) = self.joiner.transmit() {
                    let record = self.session.seal(&message)?;
                    let _ = self.transport.send(&record);
                }
                return Err(error.into());
            }
        }
    }

    /// What to show a person now: a PIN with its slot, or a device flow's user code.
    #[must_use]
    pub fn shown(&self) -> Option<&Shown> {
        self.joiner.shown()
    }

    /// Runs the rest of the exchange. The caller stores [`Paired::peer`], and reconnects under
    /// [`WebCrypto`](crate::WebCrypto).
    ///
    /// # Errors
    ///
    /// As [`PairingSession::join`].
    pub fn finish(mut self) -> Result<Paired, Error> {
        self.run(false)?;
        let close = self.session.close()?;
        let _ = self.transport.send(&close);
        Ok(self.joiner.finish()?)
    }
}
