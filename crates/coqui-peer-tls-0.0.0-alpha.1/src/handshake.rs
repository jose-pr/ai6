use alloc::sync::Arc;
use core::net::SocketAddr;
use std::io::Cursor;

use coqui_peer::{Established, Framing, Handshake, Method};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, Connection, ServerConfig, ServerConnection};

use crate::certificate::certificate_key;
use crate::error::PeerError;
use crate::identity::PeerIdentity;
use crate::source::scoped;
use crate::stream::handshake_error;
use crate::stream::{default_server_name, identity_of};

/// The TLS method: client and server configurations, each optional, that start handshakes.
///
/// ```no_run
/// # fn demo(client: rustls::ClientConfig, server: rustls::ServerConfig) {
/// let method = coqui_peer_tls::Tls::new()
///     .client(std::sync::Arc::new(client))
///     .server(std::sync::Arc::new(server));
/// # let _ = method;
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct Tls {
    client: Option<Arc<ClientConfig>>,
    server: Option<Arc<ServerConfig>>,
}

impl Tls {
    /// A method that can neither dial nor accept until configured.
    pub fn new() -> Self {
        Self::default()
    }

    /// This method able to dial with `config` (see [`ClientBuilder`](crate::ClientBuilder)).
    #[must_use]
    pub fn client(mut self, config: Arc<ClientConfig>) -> Self {
        self.client = Some(config);
        self
    }

    /// This method able to accept with `config` (see [`ServerBuilder`](crate::ServerBuilder)).
    #[must_use]
    pub fn server(mut self, config: Arc<ServerConfig>) -> Self {
        self.server = Some(config);
        self
    }
}

impl Method for Tls {
    type Handshake = TlsHandshake;

    fn name(&self) -> &'static str {
        crate::METHOD
    }

    fn initiate(&self) -> Result<TlsHandshake, PeerError> {
        let config = self.client.clone().ok_or(PeerError::Unsupported)?;
        TlsHandshake::client(config)
    }

    fn respond(&self, source: Option<SocketAddr>) -> Result<TlsHandshake, PeerError> {
        let config = self.server.clone().ok_or(PeerError::Unsupported)?;
        TlsHandshake::server(config, source)
    }
}

/// One TLS handshake, driven by the caller: rustls's own sans-IO connection.
///
/// A server records a refused, key-proven client as waiting under the `source` it was given,
/// exactly as [`accept`](crate::accept) does. After [`finish`](Handshake::finish) the
/// [`session`](Established::session) is the finished [`rustls::Connection`], which keeps any
/// application data that arrived with the last handshake bytes.
#[derive(Debug)]
pub struct TlsHandshake {
    connection: Connection,
    source: Option<SocketAddr>,
    local: Option<SocketAddr>,
}

impl TlsHandshake {
    /// A handshake as the dialing side, sending [`SERVER_NAME`](crate::SERVER_NAME) as the
    /// server name.
    ///
    /// # Errors
    ///
    /// [`PeerError::Tls`] when the connection cannot be created.
    pub fn client(config: Arc<ClientConfig>) -> Result<Self, PeerError> {
        Self::client_named(config, default_server_name())
    }

    /// A handshake as the dialing side, sending `name` as the server name: the host dialled,
    /// for a server verified by [`Roots`](crate::Roots). [`Established::peer`] is the
    /// server's leaf certificate's fingerprint, however it was verified.
    ///
    /// # Errors
    ///
    /// [`PeerError::Tls`] when the connection cannot be created.
    pub fn client_named(
        config: Arc<ClientConfig>,
        name: ServerName<'static>,
    ) -> Result<Self, PeerError> {
        let connection = ClientConnection::new(config, name).map_err(|_| PeerError::Tls)?;
        Ok(Self {
            connection: connection.into(),
            source: None,
            local: None,
        })
    }

    /// A handshake as the accepting side; `source` is where the peer connected from.
    ///
    /// # Errors
    ///
    /// [`PeerError::Tls`] when the connection cannot be created.
    pub fn server(
        config: Arc<ServerConfig>,
        source: Option<SocketAddr>,
    ) -> Result<Self, PeerError> {
        let connection = ServerConnection::new(config).map_err(|_| PeerError::Tls)?;
        Ok(Self {
            connection: connection.into(),
            source,
            local: None,
        })
    }

    /// This handshake knowing the local address the connection arrived at, which a policy's
    /// access control list reads for rules that name a destination or a port. A server
    /// handshake without one refuses a connection such a rule would decide
    /// ([`Refusal::AddressUnknown`](crate::Refusal::AddressUnknown)).
    #[must_use]
    pub fn local(mut self, local: SocketAddr) -> Self {
        self.local = Some(local);
        self
    }

    /// Finishes a completed handshake whose peer may have presented no certificate: the
    /// connection, and the peer's verified certificate when it sent one.
    ///
    /// A server built with [`ClientAuth::None`](crate::ClientAuth::None)
    /// asks for none, so its peer is always `None` and [`Handshake::finish`], which needs a
    /// peer, fails there. **`None` means the client is not authenticated**: the application
    /// must authenticate it inside the connection before trusting it. Drive the handshake with
    /// `coqui_peer::complete_stream` or `complete_messages`, then call this.
    ///
    /// # Errors
    ///
    /// [`PeerError::Tls`] when the handshake is not complete.
    pub fn finish_unauthenticated(self) -> Result<(Connection, Option<PeerIdentity>), PeerError> {
        if self.connection.is_handshaking() {
            return Err(PeerError::Tls);
        }
        let peer = match self.connection.peer_certificates() {
            Some(certificates) if !certificates.is_empty() => {
                Some(identity_of(Some(certificates))?)
            }
            _ => None,
        };
        Ok((self.connection, peer))
    }
}

impl Handshake for TlsHandshake {
    type Output = Connection;
    type Error = PeerError;

    fn framing(&self) -> Framing {
        Framing::Stream
    }

    fn receive(&mut self, input: &[u8]) -> Result<(), PeerError> {
        let mut cursor = Cursor::new(input);
        while usize::try_from(cursor.position()).unwrap_or(usize::MAX) < input.len() {
            let read = self
                .connection
                .read_tls(&mut cursor)
                .map_err(PeerError::Io)?;
            let (processed, refused) = scoped(self.source, self.local, || {
                self.connection.process_new_packets().map(|_| ())
            });
            processed.map_err(|error| {
                handshake_error(
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                    refused,
                )
            })?;
            if read == 0 {
                return Err(PeerError::Tls);
            }
        }
        Ok(())
    }

    fn transmit(&mut self) -> Result<Option<Vec<u8>>, PeerError> {
        if !self.connection.wants_write() {
            return Ok(None);
        }
        let mut out = Vec::new();
        self.connection.write_tls(&mut out).map_err(PeerError::Io)?;
        Ok((!out.is_empty()).then_some(out))
    }

    fn is_complete(&self) -> bool {
        !self.connection.is_handshaking()
    }

    fn finish(self) -> Result<Established<Connection>, PeerError> {
        if self.connection.is_handshaking() {
            return Err(PeerError::Tls);
        }
        let peer = identity_of(self.connection.peer_certificates())?;
        Ok(Established {
            peer: peer.fingerprint,
            key: Some(certificate_key(&peer.certificate)?),
            session: self.connection,
        })
    }
}
