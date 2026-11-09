//! Pairing over TLS: the `coqui_peer::pair` exchange on a connection of its own, under a pairing
//! ALPN, bound to the connection by its exporter; and key rotation over an authenticated one.

use alloc::sync::Arc;
use core::net::SocketAddr;
use core::time::Duration;
use std::io::{self, Read, Write};
use std::time::{Instant, SystemTime};

use coqui_peer::pair::{
    Channel, Mode, Paired, Pairing, PairingDesk, PairingError, PairingJoiner, PairingResponder,
    Rotation, Shown,
};
use coqui_peer::{
    KeyMaterial, PeerId, Random, ReadTimeout, TrustStore, WriteTimeout, read_frame, write_frame,
};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, ServerConfig, ServerConnection, StreamOwned};

use crate::certificate::certificate_key;
use crate::config::{ClientAuth, ClientBuilder, ServerBuilder, crypto_provider};
use crate::error::PeerError;
use crate::identity::{Identity, PeerIdentity};
use crate::policy::Policy;
use crate::stream::{PeerAddr, SERVER_NAME, accept_from_as, dial_named};
use crate::trust::{Pins, Trust};

/// The default ALPN of a pairing connection.
pub const PAIR_ALPN: &[u8] = b"coqui-pair/1";

/// The label of the TLS exporter (RFC 8446 section 7.5, RFC 5705) that binds a pairing
/// exchange to its connection; 32 bytes with an empty context.
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-coqui-pair";

/// The TLS side of pairing: the device's own [`Policy`] and the ALPN that marks a pairing
/// connection. Nothing here sets a TLS version or suite; the policy does.
///
/// Plain data: start from [`Default`] and assign the fields. It is a guatiao value, the type a
/// file or a form fills (`alpn` as text, `timeout` as a number of seconds, `[secs, nanos]` or
/// `{secs, nanos}`), and `#[non_exhaustive]`.
#[derive(Debug, Clone, PartialEq, Eq, guatiao::ToValue, guatiao::FromValue, guatiao::Schema)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct PairConfig {
    /// The policy both pairing configurations are built with. Default [`Policy::default`].
    #[map(default)]
    pub policy: Policy,
    /// The pairing ALPN; only a connection that negotiated it carries a pairing exchange, and
    /// only the configurations built here admit a key-proven certificate nobody trusts yet.
    /// Default [`PAIR_ALPN`].
    #[map(default = PAIR_ALPN.to_vec(), with = crate::settings::alpn_text)]
    pub alpn: Vec<u8>,
    /// How long one side's whole exchange may take, the TLS handshake included: the joiner's
    /// dial and finish together, the responder's accept. `Duration::MAX` waits forever. Default
    /// 15 minutes, longer than the default `device_ttl` an operator has to approve.
    #[map(default = DEFAULT_TIMEOUT)]
    pub timeout: Duration,
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15 * 60);

impl Default for PairConfig {
    fn default() -> Self {
        Self {
            policy: Policy::default(),
            alpn: PAIR_ALPN.to_vec(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// How a joiner checks the responder's certificate on the pairing connection.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ResponderTrust {
    /// Any certificate whose key the responder proves. Enough for a PIN, which authenticates
    /// the responder; the device flow refuses it unless [`Pairing::allow_unverified`].
    Unverified,
    /// Verified by `trust` (pins, a store, or roots for `name`) before anything is sent.
    Verified {
        /// The trust.
        trust: Box<Trust>,
        /// The server name to send: the host dialled, which roots check; `None` sends
        /// [`SERVER_NAME`], which pins ignore.
        name: Option<ServerName<'static>>,
    },
}

impl ResponderTrust {
    /// Verified by pinning `id`.
    pub fn pinned(id: PeerId) -> Self {
        Self::Verified {
            trust: Box::new(Trust::Pinned(Pins::fixed([id]))),
            name: None,
        }
    }

    fn verifies(&self) -> bool {
        match self {
            Self::Unverified => false,
            Self::Verified { trust, .. } => !matches!(**trust, Trust::AnyWithKeyProof),
        }
    }
}

/// A [`Random`] drawing from the TLS provider's secure random source, for a
/// [`PairingDesk`] and invitations.
pub fn random() -> Random {
    let provider = crypto_provider();
    Random::from_fn(move |out| {
        provider
            .secure_random
            .fill(out)
            .map_err(|_| coqui_peer::Error::Random)
    })
}

/// The server configuration of a pairing connection: `identity`, any client certificate whose
/// key is proven, `config`'s ALPN and policy. A server on one port serves it to clients that
/// offer the pairing ALPN (`asynch::accept_with`) and its own configuration to the rest.
///
/// # Errors
///
/// As [`ServerBuilder::build`].
pub fn pair_server_config(
    identity: &Identity,
    config: &PairConfig,
) -> Result<ServerConfig, PeerError> {
    Ok(
        ServerBuilder::new(identity, ClientAuth::Required(Trust::AnyWithKeyProof))
            .alpn([&config.alpn])
            .policy(config.policy.clone())
            .build()?
            .config,
    )
}

/// The client configuration of a pairing connection for `pairing`: an invitation pins the
/// inviter; otherwise `responder` decides.
///
/// # Errors
///
/// [`PeerError::Pairing`] with [`PairingError::Unverified`] for the device flow under
/// [`ResponderTrust::Unverified`] unless the pairing allows it; as [`ClientBuilder::build`].
pub fn pair_client_config(
    identity: &Identity,
    pairing: &Pairing,
    responder: &ResponderTrust,
    config: &PairConfig,
) -> Result<(ClientConfig, ServerName<'static>), PeerError> {
    let chosen = match pairing.inviter() {
        Some(inviter) => ResponderTrust::pinned(inviter),
        None => responder.clone(),
    };
    if pairing.mode() == Mode::Device && !chosen.verifies() && !pairing.allows_unverified() {
        return Err(PeerError::Pairing(PairingError::Unverified));
    }
    let (trust, name) = match chosen {
        ResponderTrust::Unverified => (Trust::AnyWithKeyProof, None),
        ResponderTrust::Verified { trust, name } => (*trust, name),
    };
    let name = match name {
        Some(name) => name,
        None => ServerName::try_from(SERVER_NAME).map_err(|_| PeerError::Tls)?,
    };
    let built = ClientBuilder::new(trust)
        .identity(identity)
        .alpn([&config.alpn])
        .policy(config.policy.clone())
        .build()?;
    Ok((built.config, name))
}

/// Whether the joiner's check of the responder verifies who it is, not only that it holds its
/// key: an invitation pins the inviter, and `Verified` trust other than `AnyWithKeyProof`.
pub(crate) fn responder_verified(pairing: &Pairing, responder: &ResponderTrust) -> bool {
    pairing.inviter().is_some() || responder.verifies()
}

/// The channel a finished pairing connection gives the exchange: both certificates (proven by
/// the handshake), whether the remote one was also verified, and the exporter.
pub(crate) fn channel(
    connection: &rustls::CommonState,
    exporter: [u8; 32],
    local: &Identity,
    peer: &PeerIdentity,
    verified: bool,
    alpn: &[u8],
) -> Result<Channel, PeerError> {
    if connection.alpn_protocol() != Some(alpn) {
        return Err(PeerError::Tls);
    }
    Ok(Channel {
        local: certificate_key(local.certificate_der())?,
        remote: Some(certificate_key(&peer.certificate)?),
        verified,
        binding: Some(exporter.to_vec()),
    })
}

/// The instant `timeout` from now; `None` when it is too far to represent, which waits forever.
pub(crate) fn ends(timeout: Duration) -> Option<Instant> {
    Instant::now().checked_add(timeout)
}

/// The time left until `until`, at least a millisecond (a zero read timeout is an error).
pub(crate) fn left(until: Option<Instant>) -> Option<Duration> {
    until.map(|until| {
        until
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
    })
}

/// The sooner of `left` and a responder's own time left on the desk's clock.
pub(crate) fn sooner(left: Option<Duration>, responder: Option<Duration>) -> Option<Duration> {
    let responder = responder.map(|time| time.max(Duration::from_millis(1)));
    match (left, responder) {
        (Some(left), Some(responder)) => Some(left.min(responder)),
        (left, responder) => left.or(responder),
    }
}

pub(crate) fn timed_out(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// A joiner's pairing connection, past the point where it has something to show (or done).
#[derive(Debug)]
pub struct PairDial<S: Read + Write + ReadTimeout + WriteTimeout> {
    tls: StreamOwned<ClientConnection, S>,
    joiner: PairingJoiner,
    until: Option<Instant>,
}

impl<S: Read + Write + ReadTimeout + WriteTimeout> PairDial<S> {
    /// What to show a person now: this joiner's PIN with its slot, or the device-flow user
    /// code. `None` when the mode shows nothing.
    pub fn shown(&self) -> Option<&Shown> {
        self.joiner.shown()
    }

    /// Runs the exchange to its end (for a shown code, until the operator acts at the
    /// responder), accepts the responder's certificate in `store`, and closes the connection.
    /// It waits at most what is left of [`PairConfig::timeout`] from [`pair_dial`].
    ///
    /// # Errors
    ///
    /// [`PeerError::Pairing`] when either side refused; [`PeerError::Io`] when the connection
    /// fails or the timeout passes (`TimedOut`); [`PeerError::Certificate`] or
    /// [`PeerError::Io`] when the store cannot write.
    pub fn finish(mut self, store: &TrustStore) -> Result<Paired, PeerError> {
        join_until(&mut self.tls, &mut self.joiner, self.until, |_| false)?;
        let paired = self.joiner.finish()?;
        match store.accept_key(&paired.peer) {
            Ok(_) | Err(coqui_peer::Error::AlreadyAccepted) => {}
            Err(error) => return Err(error.into()),
        }
        self.tls.conn.send_close_notify();
        let _ = self.tls.flush();
        Ok(paired)
    }
}

/// Moves joiner messages until `stop` holds, the joiner completes, it fails, or `until` passes.
fn join_until<S: Read + Write + ReadTimeout + WriteTimeout>(
    tls: &mut StreamOwned<ClientConnection, S>,
    joiner: &mut PairingJoiner,
    until: Option<Instant>,
    stop: impl Fn(&PairingJoiner) -> bool,
) -> Result<(), PeerError> {
    loop {
        tls.sock
            .set_write_timeout(left(until))
            .map_err(PeerError::Io)?;
        while let Some(message) = joiner.transmit() {
            write_frame(tls, &message).map_err(PeerError::Io)?;
        }
        if joiner.is_complete() || stop(joiner) {
            return Ok(());
        }
        tls.sock
            .set_read_timeout(left(until))
            .map_err(PeerError::Io)?;
        let message = read_frame(tls).map_err(PeerError::Io)?;
        if let Err(error) = joiner.receive(&message) {
            // The joiner may have queued a refusal for the responder.
            while let Some(message) = joiner.transmit() {
                let _ = write_frame(tls, &message);
            }
            return Err(error.into());
        }
    }
}

/// Dials a pairing connection over `stream` as `identity` and runs `pairing` until there is
/// something to [`show`](PairDial::shown), or to its end; [`PairDial::finish`] completes it.
///
/// The connection uses `config`'s policy and ALPN and is bound to the exchange by its exporter.
/// An invitation pins the inviter; a PIN pairing may leave the responder
/// [`Unverified`](ResponderTrust::Unverified) because the PIN authenticates it; the device flow
/// needs a [`Verified`](ResponderTrust::Verified) responder unless the pairing allows otherwise.
/// The handshake, this call and [`PairDial::finish`] together wait at most
/// [`PairConfig::timeout`].
///
/// # Errors
///
/// [`PeerError::Pairing`]; [`PeerError::Io`] when the handshake or connection fails or the
/// timeout passes (a pinned inviter that is someone else is refused here, with
/// [`PeerError::refusal`]); [`PeerError::Tls`] when the server did not negotiate the pairing
/// ALPN.
pub fn pair_dial<S: Read + Write + ReadTimeout + WriteTimeout>(
    stream: S,
    identity: &Identity,
    pairing: Pairing,
    responder: &ResponderTrust,
    config: &PairConfig,
) -> Result<PairDial<S>, PeerError> {
    let until = ends(config.timeout);
    let (client, name) = pair_client_config(identity, &pairing, responder, config)?;
    let verified = responder_verified(&pairing, responder);
    stream
        .set_read_timeout(left(until))
        .and_then(|()| stream.set_write_timeout(left(until)))
        .map_err(PeerError::Io)?;
    let (tls, peer) = dial_named(Arc::new(client), name, stream)?;
    let exporter = tls
        .conn
        .export_keying_material([0u8; 32], EXPORTER_LABEL, Some(&[]))
        .map_err(|_| PeerError::Tls)?;
    let channel = channel(&tls.conn, exporter, identity, &peer, verified, &config.alpn)?;
    let mut dial = PairDial {
        tls,
        joiner: pairing.join(channel)?,
        until,
    };
    join_until(&mut dial.tls, &mut dial.joiner, until, |joiner| {
        joiner.shown().is_some()
    })?;
    Ok(dial)
}

/// Accepts a pairing connection over `stream` as `identity` and runs the responder's side to
/// its end: redeeming an invitation from `store`, taking PINs and decisions from `desk` (this
/// call blocks while the operator decides), and accepting the joiner's certificate in `store`.
/// The address of `stream` is the source the desk throttles by.
///
/// Every read waits at most the sooner of what is left of [`PairConfig::timeout`] and the
/// responder's own deadline (`pin_ttl` or `device_ttl` from the joiner's first message); past
/// that deadline the joiner is told `Expired`.
///
/// # Errors
///
/// [`PeerError::Pairing`] when either side refused or the deadline passed; [`PeerError::Io`]
/// when the handshake or the connection fails or the timeout passes (`TimedOut`);
/// [`PeerError::Tls`] when the client did not negotiate the pairing ALPN.
pub fn pair_accept<S: Read + Write + PeerAddr + ReadTimeout + WriteTimeout>(
    stream: S,
    identity: &Identity,
    store: &TrustStore,
    desk: &PairingDesk,
    config: &PairConfig,
) -> Result<Paired, PeerError> {
    let (source, local) = (stream.peer_addr(), stream.local_addr());
    pair_accept_as(stream, (source, local), identity, store, desk, config)
}

fn pair_accept_as<S: Read + Write + ReadTimeout + WriteTimeout>(
    stream: S,
    (source, local): (Option<SocketAddr>, Option<SocketAddr>),
    identity: &Identity,
    store: &TrustStore,
    desk: &PairingDesk,
    config: &PairConfig,
) -> Result<Paired, PeerError> {
    let until = ends(config.timeout);
    let server = Arc::new(pair_server_config(identity, config)?);
    stream
        .set_read_timeout(left(until))
        .and_then(|()| stream.set_write_timeout(left(until)))
        .map_err(PeerError::Io)?;
    let (mut tls, peer) = accept_from_as(server, stream, source, local)?;
    let exporter = tls
        .conn
        .export_keying_material([0u8; 32], EXPORTER_LABEL, Some(&[]))
        .map_err(|_| PeerError::Tls)?;
    let channel = channel(&tls.conn, exporter, identity, &peer, false, &config.alpn)?;
    let mut responder = PairingResponder::new(channel, store, desk, source);
    let result = respond(&mut tls, &mut responder, desk, until);
    tls.conn.send_close_notify();
    let _ = tls.flush();
    result?;
    Ok(responder.finish()?)
}

fn respond<S: Read + Write + ReadTimeout + WriteTimeout>(
    tls: &mut StreamOwned<ServerConnection, S>,
    responder: &mut PairingResponder,
    desk: &PairingDesk,
    until: Option<Instant>,
) -> Result<(), PeerError> {
    loop {
        tls.sock
            .set_write_timeout(left(until))
            .map_err(PeerError::Io)?;
        while let Some(message) = responder.transmit() {
            write_frame(tls, &message).map_err(PeerError::Io)?;
        }
        if responder.is_complete() {
            return Ok(());
        }
        let step = match responder.awaiting() {
            Some(ticket) => responder.resume(desk.wait(ticket)),
            None => {
                let wait = sooner(left(until), responder.time_left());
                tls.sock.set_read_timeout(wait).map_err(PeerError::Io)?;
                match read_frame(tls) {
                    Ok(message) => responder.receive(&message),
                    // Past the responder's deadline the joiner is told; past the transport's
                    // own timeout the connection just ends.
                    Err(error) if timed_out(&error) => match responder.check_deadline() {
                        Err(expired) => Err(expired),
                        Ok(()) => return Err(PeerError::Io(error)),
                    },
                    Err(error) => return Err(PeerError::Io(error)),
                }
            }
        };
        if let Err(error) = step {
            while let Some(message) = responder.transmit() {
                let _ = write_frame(tls, &message);
            }
            return Err(error.into());
        }
    }
}

/// Announces `successor` as the next identity of this side, over a connection the current
/// identity authenticated: one framed [`Rotation`] message. What follows on the connection is
/// the caller's protocol.
///
/// # Errors
///
/// [`PeerError::Certificate`] for an unusable certificate, [`PeerError::Io`].
pub fn announce(stream: &mut impl Write, successor: &Identity) -> Result<(), PeerError> {
    let key = certificate_key(successor.certificate_der())?;
    write_frame(stream, &Rotation::announce(&key)).map_err(PeerError::Io)
}

/// Reads an [`announce`]ment from `sender` (the verified peer of this connection) and records
/// it in `store` as the pending successor of `sender`'s certificate. Returns the successor's id.
/// The next handshake that proves it promotes it, unless the store's rotation options require
/// approval.
///
/// # Errors
///
/// [`PeerError::Pairing`] with [`PairingError::Protocol`] for anything but an announcement of a
/// certificate; [`PeerError::NotWaiting`]-style store errors as [`PeerError`] (an announcement
/// by a peer that is not accepted, or retiring, is refused); [`PeerError::Io`].
pub fn receive_announcement(
    stream: &mut impl Read,
    store: &TrustStore,
    sender: &PeerIdentity,
) -> Result<PeerId, PeerError> {
    let message = read_frame(stream).map_err(PeerError::Io)?;
    record_announcement(&message, store, sender)
}

pub(crate) fn record_announcement(
    message: &[u8],
    store: &TrustStore,
    sender: &PeerIdentity,
) -> Result<PeerId, PeerError> {
    let key: KeyMaterial = Rotation::parse(message)?;
    if key.method != crate::METHOD {
        return Err(PeerError::Pairing(PairingError::Protocol));
    }
    store.announce_successor(&sender.fingerprint, &key)?;
    Ok(key.id())
}

impl Identity {
    /// Whether this identity's certificate ends within `margin` from now: time to make a
    /// successor and [`announce`] it. An unreadable validity is due.
    pub fn renew_before(&self, margin: Duration) -> bool {
        self.renew_before_at(margin, SystemTime::now())
    }

    /// [`renew_before`](Identity::renew_before) at `now`.
    pub fn renew_before_at(&self, margin: Duration, now: SystemTime) -> bool {
        let Ok((_, not_after)) = crate::x509::validity(self.certificate_der()) else {
            return true;
        };
        let now = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let deadline = i128::from(now) + i128::from(margin.as_secs());
        deadline >= i128::from(not_after)
    }
}
