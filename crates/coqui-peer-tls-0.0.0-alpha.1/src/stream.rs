use alloc::sync::Arc;
use core::net::SocketAddr;
use core::time::Duration;
use std::io::{self, Read, Write};
use std::net::TcpStream;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, ServerConfig, ServerConnection, StreamOwned};

use crate::error::{PeerError, Refusal};
use crate::identity::PeerIdentity;
use crate::source::scoped;
use coqui_peer::{PeerId, ReadTimeout, WriteTimeout};

/// The server name [`dial`] and [`TlsHandshake::client`](crate::TlsHandshake::client) send,
/// and the subjectAltName of generated identities. Pinned trust is by fingerprint, so the name
/// carries no meaning there; [`Roots`](crate::Roots) check the name, so dial a server they
/// verify with [`dial_named`].
pub const SERVER_NAME: &str = "coqui-peer.invalid";

/// [`SERVER_NAME`] as the name rustls takes, for a host that dials with a connector of its own
/// (rustls, tokio-rustls, quinn) and trusts by pins.
pub fn default_server_name() -> ServerName<'static> {
    ServerName::try_from(SERVER_NAME).expect("SERVER_NAME is a valid DNS name")
}

pub(crate) fn identity_of(
    certificates: Option<&[CertificateDer<'static>]>,
) -> Result<PeerIdentity, PeerError> {
    let certificate = certificates
        .and_then(<[_]>::first)
        .ok_or(PeerError::Certificate)?;
    Ok(PeerIdentity {
        fingerprint: PeerId::of(certificate.as_ref()),
        certificate: certificate.as_ref().to_vec(),
    })
}

/// Dials a peer over `io`, completing the handshake before returning.
///
/// Returns the encrypted stream and the verified peer. `io` is any blocking byte stream. The
/// server name sent is [`SERVER_NAME`]; [`dial_named`] sends another.
///
/// # Errors
///
/// [`PeerError::Io`] when the handshake fails, including a refused peer certificate;
/// [`PeerError::refusal`] says why this side refused one.
pub fn dial<S: Read + Write>(
    config: Arc<ClientConfig>,
    io: S,
) -> Result<(StreamOwned<ClientConnection, S>, PeerIdentity), PeerError> {
    dial_named(config, default_server_name(), io)
}

/// [`dial`] sending `name` as the server name: the host the client dialled, for a server
/// verified by [`Roots`](crate::Roots), which check that its certificate is for that name
/// (a DNS name, or an IP address the certificate names). Pinned trust ignores the name.
///
/// The returned peer's fingerprint is the server's leaf certificate's, however it was
/// verified.
///
/// # Errors
///
/// [`PeerError::Io`] when the handshake fails, including a refused server certificate;
/// [`PeerError::refusal`] says why this side refused one.
pub fn dial_named<S: Read + Write>(
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
    mut io: S,
) -> Result<(StreamOwned<ClientConnection, S>, PeerIdentity), PeerError> {
    let mut connection = ClientConnection::new(config, name).map_err(|_| PeerError::Tls)?;
    while connection.is_handshaking() {
        connection.complete_io(&mut io).map_err(PeerError::Io)?;
    }
    let peer = identity_of(connection.peer_certificates())?;
    Ok((StreamOwned::new(connection, io), peer))
}

/// A stream that can name the remote address it is connected to.
///
/// [`accept`] uses it to record where a refused peer came from. A stream without an address,
/// or one that is not TCP, is accepted wrapped in an [`Addressed`].
pub trait PeerAddr {
    /// The remote address, when known.
    fn peer_addr(&self) -> Option<SocketAddr>;

    /// The local address the connection arrived at, when known: a policy's access control
    /// list reads it for rules that name a destination or a port. `None` unless implemented.
    fn local_addr(&self) -> Option<SocketAddr> {
        None
    }
}

impl PeerAddr for TcpStream {
    fn peer_addr(&self) -> Option<SocketAddr> {
        TcpStream::peer_addr(self).ok()
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        TcpStream::local_addr(self).ok()
    }
}

/// A stream with the addresses of its connection given, for a stream that cannot name them
/// itself (a pipe, a tunnel, a test double): [`accept`] and the pairing functions read them
/// through [`PeerAddr`], so they accept it as they accept a TCP stream.
///
/// It reads, writes and times out as the stream does ([`Read`], [`Write`],
/// `ReadTimeout`, `WriteTimeout`, and with the `tokio` feature `AsyncRead` and `AsyncWrite`). The
/// stream a function returns holds the wrapper: [`into_inner`](Self::into_inner) takes the
/// stream back.
#[derive(Debug)]
pub struct Addressed<S> {
    io: S,
    source: SocketAddr,
    local: Option<SocketAddr>,
}

impl<S> Addressed<S> {
    /// `io` as a connection from `source`, with no local address.
    pub fn new(io: S, source: SocketAddr) -> Self {
        Self {
            io,
            source,
            local: None,
        }
    }

    /// The local address the connection arrived at, which a policy's access control list reads
    /// for rules that name a destination or a port.
    pub fn local(mut self, local: SocketAddr) -> Self {
        self.local = Some(local);
        self
    }

    /// The wrapped stream.
    pub fn get_ref(&self) -> &S {
        &self.io
    }

    /// The wrapped stream, mutably.
    pub fn get_mut(&mut self) -> &mut S {
        &mut self.io
    }

    /// The wrapped stream.
    pub fn into_inner(self) -> S {
        self.io
    }
}

impl<S> PeerAddr for Addressed<S> {
    fn peer_addr(&self) -> Option<SocketAddr> {
        Some(self.source)
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local
    }
}

impl<S: Read> Read for Addressed<S> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.io.read(buffer)
    }
}

impl<S: Write> Write for Addressed<S> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.io.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.io.flush()
    }
}

impl<S: ReadTimeout> ReadTimeout for Addressed<S> {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.io.set_read_timeout(timeout)
    }
}

impl<S: WriteTimeout> WriteTimeout for Addressed<S> {
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.io.set_write_timeout(timeout)
    }
}

/// The error of a handshake that failed: the refusal the address check recorded, as a
/// verifier's refusal is carried, or the transport's error.
pub(crate) fn handshake_error(error: io::Error, refused: Option<Refusal>) -> PeerError {
    match refused {
        Some(refusal) => PeerError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            refusal.into_rustls(),
        )),
        None => PeerError::Io(error),
    }
}

/// Accepts a peer over `io`, completing the handshake before returning.
///
/// A client whose certificate the configured [`Trust`](crate::Trust) refuses fails the
/// handshake; when the trust is a store-backed pin set and the client proved it holds the
/// certificate's key, it is recorded as waiting under the address of `io`.
///
/// It returns a verified client, so it needs one: with a server built with
/// [`ClientAuth::None`](crate::ClientAuth::None) it fails
/// with [`PeerError::Certificate`] after the handshake. Accept those with
/// [`TlsHandshake::finish_unauthenticated`](crate::TlsHandshake::finish_unauthenticated), or
/// with rustls's or tokio-rustls's own acceptor, which need nothing from this crate.
///
/// # Errors
///
/// [`PeerError::Io`] when the handshake fails; [`PeerError::refusal`] says why this side
/// refused the peer certificate, when it did.
pub fn accept<S: Read + Write + PeerAddr>(
    config: Arc<ServerConfig>,
    io: S,
) -> Result<(StreamOwned<ServerConnection, S>, PeerIdentity), PeerError> {
    let (source, local) = (io.peer_addr(), io.local_addr());
    accept_from_as(config, io, source, local)
}

pub(crate) fn accept_from_as<S: Read + Write>(
    config: Arc<ServerConfig>,
    mut io: S,
    source: Option<SocketAddr>,
    local: Option<SocketAddr>,
) -> Result<(StreamOwned<ServerConnection, S>, PeerIdentity), PeerError> {
    let mut connection = ServerConnection::new(config).map_err(|_| PeerError::Tls)?;
    let (handshake, refused) = scoped(source, local, || {
        while connection.is_handshaking() {
            connection.complete_io(&mut io)?;
        }
        Ok::<(), io::Error>(())
    });
    handshake.map_err(|error| handshake_error(error, refused))?;
    let peer = identity_of(connection.peer_certificates())?;
    Ok((StreamOwned::new(connection, io), peer))
}
