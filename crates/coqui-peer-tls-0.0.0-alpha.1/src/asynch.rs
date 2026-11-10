//! Async dial and accept over `tokio-rustls`, for any `AsyncRead + AsyncWrite` stream.

use alloc::sync::Arc;
use core::net::SocketAddr;

use core::pin::Pin;
use core::task::{Context, Poll};
use rustls::pki_types::ServerName;
use rustls::server::ClientHello;
use rustls::{ClientConfig, ServerConfig};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::{TlsConnector, client, server};

use core::time::Duration;
use std::time::Instant;

use coqui_peer::asynch::{read_frame, write_frame};
use coqui_peer::pair::{
    Paired, Pairing, PairingDesk, PairingJoiner, PairingResponder, Rotation, Shown,
};
use coqui_peer::{PeerId, TrustStore};

use crate::error::PeerError;
use crate::error::Refusal;
use crate::identity::{Identity, PeerIdentity};
use crate::pairing::{PairConfig, ResponderTrust, ends, left, sooner};
use crate::source::scoped;
use crate::stream::{Addressed, PeerAddr, default_server_name, handshake_error, identity_of};

/// Dials a peer over `io`, completing the handshake before returning. The server name sent
/// is [`SERVER_NAME`](crate::SERVER_NAME); [`dial_named`] sends another.
///
/// # Errors
///
/// [`PeerError::Io`] when the handshake fails, including a refused peer certificate;
/// [`PeerError::refusal`] says why this side refused one.
pub async fn dial<S>(
    config: Arc<ClientConfig>,
    io: S,
) -> Result<(client::TlsStream<S>, PeerIdentity), PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    dial_named(config, default_server_name(), io).await
}

/// [`dial`] sending `name` as the server name, as [`crate::dial_named`] does.
///
/// # Errors
///
/// [`PeerError::Io`] when the handshake fails, including a refused server certificate;
/// [`PeerError::refusal`] says why this side refused one.
pub async fn dial_named<S>(
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
    io: S,
) -> Result<(client::TlsStream<S>, PeerIdentity), PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let stream = TlsConnector::from(config)
        .connect(name, io)
        .await
        .map_err(PeerError::Io)?;
    let peer = identity_of(stream.get_ref().1.peer_certificates())?;
    Ok((stream, peer))
}

impl PeerAddr for tokio::net::TcpStream {
    fn peer_addr(&self) -> Option<SocketAddr> {
        tokio::net::TcpStream::peer_addr(self).ok()
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        tokio::net::TcpStream::local_addr(self).ok()
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Addressed<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(Addressed::get_mut(Pin::get_mut(self))).poll_read(context, buffer)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Addressed<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(Addressed::get_mut(Pin::get_mut(self))).poll_write(context, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(Addressed::get_mut(Pin::get_mut(self))).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(Addressed::get_mut(Pin::get_mut(self))).poll_shutdown(context)
    }
}

/// Polls `future` with the connection's addresses in scope, since the verifiers' and the
/// address check's callbacks run inside the poll; returns its output and the refusal the
/// address check recorded.
async fn scoped_poll<F: Future>(
    source: Option<SocketAddr>,
    local: Option<SocketAddr>,
    future: F,
) -> (F::Output, Option<Refusal>) {
    let mut future = std::pin::pin!(future);
    let mut refused = None;
    let output = core::future::poll_fn(|context| {
        let (poll, refusal) = scoped(source, local, || future.as_mut().poll(context));
        refused = refused.or(refusal);
        poll
    })
    .await;
    (output, refused)
}

/// Accepts a peer over `io`, completing the handshake before returning.
///
/// A peer that proved its key and is refused by a store-backed pin set is recorded as waiting
/// under the address of `io`.
///
/// # Errors
///
/// [`PeerError::Io`] when the handshake fails; [`PeerError::refusal`] says why this side
/// refused the peer certificate, when it did.
pub async fn accept<S>(
    config: Arc<ServerConfig>,
    io: S,
) -> Result<(server::TlsStream<S>, PeerIdentity), PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin + PeerAddr,
{
    let (source, local) = (io.peer_addr(), io.local_addr());
    accept_as(config, io, source, local).await
}

async fn accept_as<S>(
    config: Arc<ServerConfig>,
    io: S,
    source: Option<SocketAddr>,
    local: Option<SocketAddr>,
) -> Result<(server::TlsStream<S>, PeerIdentity), PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (stream, refused) = scoped_poll(
        source,
        local,
        tokio_rustls::TlsAcceptor::from(config).accept(io),
    )
    .await;
    let stream = stream.map_err(|error| handshake_error(error, refused))?;
    let peer = identity_of(stream.get_ref().1.peer_certificates())?;
    Ok((stream, peer))
}

/// Accepts a peer over `io`, choosing the configuration from the `ClientHello` (for example
/// by its offered ALPN protocols), so one port can serve peers and other clients.
///
/// # Errors
///
/// [`PeerError::Io`] when the hello cannot be read or the handshake fails;
/// [`PeerError::refusal`] says why this side refused the peer certificate, when it did.
pub async fn accept_with<S>(
    io: S,
    select: impl FnOnce(&ClientHello<'_>) -> Arc<ServerConfig>,
) -> Result<(server::TlsStream<S>, PeerIdentity), PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin + PeerAddr,
{
    let (source, local) = (io.peer_addr(), io.local_addr());
    accept_with_as(io, (source, local), select).await
}

async fn accept_with_as<S>(
    io: S,
    (source, local): (Option<SocketAddr>, Option<SocketAddr>),
    select: impl FnOnce(&ClientHello<'_>) -> Arc<ServerConfig>,
) -> Result<(server::TlsStream<S>, PeerIdentity), PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let start = tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), io)
        .await
        .map_err(PeerError::Io)?;
    let config = select(&start.client_hello());
    let (stream, refused) = scoped_poll(source, local, start.into_stream(config)).await;
    let stream = stream.map_err(|error| handshake_error(error, refused))?;
    let peer = identity_of(stream.get_ref().1.peer_certificates())?;
    Ok((stream, peer))
}

/// `work`, given at most `wait` (forever when `None`); `TimedOut` when it took longer.
async fn within<F: Future>(wait: Option<Duration>, work: F) -> std::io::Result<F::Output> {
    match wait {
        Some(wait) => tokio::time::timeout(wait, work)
            .await
            .map_err(|_| std::io::ErrorKind::TimedOut.into()),
        None => Ok(work.await),
    }
}

/// The asynchronous [`PairDial`](crate::PairDial).
#[derive(Debug)]
pub struct PairDial<S> {
    tls: client::TlsStream<S>,
    joiner: PairingJoiner,
    until: Option<Instant>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> PairDial<S> {
    /// What to show a person now, as [`crate::PairDial::shown`].
    pub fn shown(&self) -> Option<&Shown> {
        self.joiner.shown()
    }

    /// Runs the exchange to its end and accepts the responder in `store`, as
    /// [`crate::PairDial::finish`].
    ///
    /// # Errors
    ///
    /// As [`crate::PairDial::finish`].
    pub async fn finish(mut self, store: &TrustStore) -> Result<Paired, PeerError> {
        join_until(&mut self.tls, &mut self.joiner, self.until, |_| false).await?;
        let paired = self.joiner.finish()?;
        match store.accept_key(&paired.peer) {
            Ok(_) | Err(coqui_peer::Error::AlreadyAccepted) => {}
            Err(error) => return Err(error.into()),
        }
        let _ = tokio::io::AsyncWriteExt::shutdown(&mut self.tls).await;
        Ok(paired)
    }
}

async fn join_until<S: AsyncRead + AsyncWrite + Unpin>(
    tls: &mut client::TlsStream<S>,
    joiner: &mut PairingJoiner,
    until: Option<Instant>,
    stop: impl Fn(&PairingJoiner) -> bool,
) -> Result<(), PeerError> {
    loop {
        while let Some(message) = joiner.transmit() {
            within(left(until), write_frame(tls, &message))
                .await
                .and_then(|written| written)
                .map_err(PeerError::Io)?;
        }
        if joiner.is_complete() || stop(joiner) {
            return Ok(());
        }
        let message = within(left(until), read_frame(tls))
            .await
            .and_then(|read| read)
            .map_err(PeerError::Io)?;
        if let Err(error) = joiner.receive(&message) {
            while let Some(message) = joiner.transmit() {
                let _ = write_frame(tls, &message).await;
            }
            return Err(error.into());
        }
    }
}

/// [`crate::pair_dial`] over an async stream.
///
/// # Errors
///
/// As [`crate::pair_dial`].
pub async fn pair_dial<S>(
    stream: S,
    identity: &Identity,
    pairing: Pairing,
    responder: &ResponderTrust,
    config: &PairConfig,
) -> Result<PairDial<S>, PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let until = ends(config.timeout);
    let (client, name) = crate::pair_client_config(identity, &pairing, responder, config)?;
    let verified = crate::pairing::responder_verified(&pairing, responder);
    let (tls, peer) = within(left(until), dial_named(Arc::new(client), name, stream))
        .await
        .map_err(PeerError::Io)??;
    let connection = tls.get_ref().1;
    let exporter = connection
        .export_keying_material([0u8; 32], crate::EXPORTER_LABEL, Some(&[]))
        .map_err(|_| PeerError::Tls)?;
    let channel = crate::pairing::channel(
        connection,
        exporter,
        identity,
        &peer,
        verified,
        config.alpn.as_bytes(),
    )?;
    let mut dial = PairDial {
        tls,
        joiner: pairing.join(channel)?,
        until,
    };
    join_until(&mut dial.tls, &mut dial.joiner, until, |joiner| {
        joiner.shown().is_some()
    })
    .await?;
    Ok(dial)
}

/// [`crate::pair_accept`] over an async stream; it waits for the operator without blocking
/// the runtime, and sweeps `desk` every second meanwhile so expiry is seen.
///
/// # Errors
///
/// As [`crate::pair_accept`].
pub async fn pair_accept<S>(
    stream: S,
    identity: &Identity,
    store: &TrustStore,
    desk: &PairingDesk,
    config: &PairConfig,
) -> Result<Paired, PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin + PeerAddr,
{
    let (source, local) = (stream.peer_addr(), stream.local_addr());
    pair_accept_as(stream, (source, local), identity, store, desk, config).await
}

async fn pair_accept_as<S>(
    stream: S,
    (source, local): (Option<SocketAddr>, Option<SocketAddr>),
    identity: &Identity,
    store: &TrustStore,
    desk: &PairingDesk,
    config: &PairConfig,
) -> Result<Paired, PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let until = ends(config.timeout);
    let server = Arc::new(crate::pair_server_config(identity, config)?);
    let (tls, _) = within(left(until), accept_as(server, stream, source, local))
        .await
        .map_err(PeerError::Io)??;
    respond_on(tls, source, identity, store, desk, config, until).await
}

/// Runs the responder's side of pairing on a connection already accepted with
/// [`pair_server_config`](crate::pair_server_config) (for example chosen by
/// [`accept_with`] for a client that offered the pairing ALPN), as `identity`, for a client
/// from `source`. The exchange waits at most [`PairConfig::timeout`] from this call.
///
/// # Errors
///
/// As [`crate::pair_accept`].
pub async fn pair_respond<S>(
    tls: server::TlsStream<S>,
    source: Option<SocketAddr>,
    identity: &Identity,
    store: &TrustStore,
    desk: &PairingDesk,
    config: &PairConfig,
) -> Result<Paired, PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let until = ends(config.timeout);
    respond_on(tls, source, identity, store, desk, config, until).await
}

async fn respond_on<S>(
    mut tls: server::TlsStream<S>,
    source: Option<SocketAddr>,
    identity: &Identity,
    store: &TrustStore,
    desk: &PairingDesk,
    config: &PairConfig,
    until: Option<Instant>,
) -> Result<Paired, PeerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let connection = tls.get_ref().1;
    let peer = identity_of(connection.peer_certificates())?;
    let exporter = connection
        .export_keying_material([0u8; 32], crate::EXPORTER_LABEL, Some(&[]))
        .map_err(|_| PeerError::Tls)?;
    let channel = crate::pairing::channel(
        connection,
        exporter,
        identity,
        &peer,
        false,
        config.alpn.as_bytes(),
    )?;
    let mut responder = PairingResponder::new(channel, store, desk, source);
    let result = respond(&mut tls, &mut responder, desk, until).await;
    let _ = tokio::io::AsyncWriteExt::shutdown(&mut tls).await;
    result?;
    Ok(responder.finish()?)
}

async fn respond<S: AsyncRead + AsyncWrite + Unpin>(
    tls: &mut server::TlsStream<S>,
    responder: &mut PairingResponder,
    desk: &PairingDesk,
    until: Option<Instant>,
) -> Result<(), PeerError> {
    loop {
        while let Some(message) = responder.transmit() {
            within(left(until), write_frame(tls, &message))
                .await
                .and_then(|written| written)
                .map_err(PeerError::Io)?;
        }
        if responder.is_complete() {
            return Ok(());
        }
        let step = match responder.awaiting() {
            Some(ticket) => {
                let decision = loop {
                    let wait = tokio::time::timeout(Duration::from_secs(1), desk.decided(ticket));
                    match wait.await {
                        Ok(decision) => break decision,
                        Err(_) => desk.sweep(),
                    }
                };
                responder.resume(decision)
            }
            None => {
                let wait = sooner(left(until), responder.time_left());
                match within(wait, read_frame(tls)).await {
                    Ok(Ok(message)) => responder.receive(&message),
                    Ok(Err(error)) => return Err(PeerError::Io(error)),
                    // Past the responder's deadline the joiner is told; past the transport's
                    // own timeout the connection just ends.
                    Err(error) => match responder.check_deadline() {
                        Err(expired) => Err(expired),
                        Ok(()) => return Err(PeerError::Io(error)),
                    },
                }
            }
        };
        if let Err(error) = step {
            while let Some(message) = responder.transmit() {
                let _ = write_frame(tls, &message).await;
            }
            return Err(error.into());
        }
    }
}

/// [`crate::announce`] over an async stream.
///
/// # Errors
///
/// As [`crate::announce`].
pub async fn announce<S: AsyncWrite + Unpin>(
    stream: &mut S,
    successor: &Identity,
) -> Result<(), PeerError> {
    let key = crate::certificate_key(successor.certificate_der())?;
    write_frame(stream, &Rotation::announce(&key))
        .await
        .map_err(PeerError::Io)
}

/// [`crate::receive_announcement`] over an async stream.
///
/// # Errors
///
/// As [`crate::receive_announcement`].
pub async fn receive_announcement<S: AsyncRead + Unpin>(
    stream: &mut S,
    store: &TrustStore,
    sender: &PeerIdentity,
) -> Result<PeerId, PeerError> {
    let message = read_frame(stream).await.map_err(PeerError::Io)?;
    crate::pairing::record_announcement(&message, store, sender)
}
