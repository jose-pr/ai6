use alloc::sync::Arc;
use core::fmt;
use core::net::SocketAddr;
use std::io;

use coqui_peer::pair::{
    Channel, Paired, Pairing, PairingDesk, PairingJoiner, PairingResponder, Shown,
};
use coqui_peer::{
    Established, Framing, Handshake, KeyMaterial, MessageTransport, Method, NonDefault, PeerId,
    Random, ReadTimeout, TrustStore,
};

use crate::error::Error;
use crate::handshake::{Config, Exchange, Finished, METHOD};
use crate::identity::Identity;
use crate::session::Session;
use crate::settings::PairingSettings;
use crate::trust::{Admit, Trust};

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

/// Builds a [`PairingMethod`]: [`PairingMethod::builder`], the settings, then
/// [`build`](Self::build).
#[derive(Debug, Clone)]
#[must_use]
pub struct PairingMethodBuilder(Config);

impl PairingMethodBuilder {
    /// Whom a joiner accepts: a pin or a store. The session is then verified, which the device flow
    /// asks for; the key an invitation names must be the proved one in any case.
    pub fn responders(mut self, trust: Trust) -> Self {
        self.0.responders = Some(Admit::Trust(trust));
        self
    }

    /// The longest handshake message accepted; see [`PairingSettings::max_message`].
    pub fn max_message(mut self, bytes: usize) -> Self {
        self.0.settings.max_message = bytes;
        self
    }

    /// As [`WebCryptoBuilder::random`](crate::WebCryptoBuilder::random).
    pub fn random(mut self, random: Random) -> Self {
        self.0.random = random;
        self.0.random_replaced = true;
        self
    }

    /// The message limit from `settings`. Replaces what [`max_message`](Self::max_message) set
    /// before, and is replaced by it after.
    pub fn settings(mut self, settings: PairingSettings) -> Self {
        self.0.settings.max_message = settings.max_message;
        self
    }

    /// The method.
    pub fn build(self) -> PairingMethod {
        PairingMethod(Arc::new(self.0))
    }
}

impl PairingMethod {
    /// A builder for a method for `identity`, as a responder admitting any proved key, and as a
    /// joiner accepting any proved key until [`responders`](PairingMethodBuilder::responders).
    pub fn builder(identity: Identity) -> PairingMethodBuilder {
        let mut config = Config::new(identity);
        config.responders = Some(Admit::Any);
        config.initiators = Some(Admit::Any);
        PairingMethodBuilder(config)
    }

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
        self.0.non_defaults()
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

    /// Runs the responder until the end, each message sealed: redeeming an invitation, taking the
    /// shown PIN, or blocking in [`PairingDesk::wait`] for the operator. On success the joiner's key
    /// is accepted in `store`. `source` is what the desk throttles by; behind a reverse proxy, pass
    /// the client's address from a header you trust, or every joiner counts as the proxy.
    ///
    /// Each read waits at most until the exchange's deadline, through `ReadTimeout`, and a read
    /// that times out ends the exchange then; before the first message the transport's own timeout
    /// applies.
    ///
    /// # Errors
    ///
    /// As [`join`](PairingSession::join).
    pub fn respond<T: MessageTransport + ReadTimeout>(
        mut self,
        store: &TrustStore,
        desk: &PairingDesk,
        source: Option<SocketAddr>,
        transport: &mut T,
    ) -> Result<Paired, Error> {
        let mut responder = PairingResponder::new(self.pair_channel()?, store, desk, source);
        loop {
            while let Some(message) = responder.transmit() {
                transport.send(&self.session.seal(&message)?)?;
            }
            if responder.is_complete() {
                break;
            }
            let step = match responder.awaiting() {
                Some(ticket) => responder.resume(desk.wait(ticket)),
                None => {
                    transport.set_read_timeout(responder.time_left())?;
                    match transport.recv() {
                        Ok(record) => {
                            let message = self.session.open(&record)?.ok_or(Error::Truncated)?;
                            responder.receive(&message)
                        }
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
                while let Some(message) = responder.transmit() {
                    let record = self.session.seal(&message)?;
                    let _ = transport.send(&record);
                }
                return Err(error.into());
            }
        }
        let close = self.session.close()?;
        let _ = transport.send(&close);
        Ok(responder.finish()?)
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
