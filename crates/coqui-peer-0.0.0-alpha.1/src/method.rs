//! The sans-IO handshake every authentication method implements.
//!
//! A [`Handshake`] does no I/O. The caller feeds it what arrived with
//! [`receive`](Handshake::receive), asks what to send with
//! [`transmit`](Handshake::transmit), and when [`is_complete`](Handshake::is_complete)
//! calls [`finish`](Handshake::finish) for the verified peer and whatever the method
//! produced (an encrypted channel, session keys, or nothing). Because the caller owns the
//! transport, one implementation runs over a byte stream, WebSocket messages, QUIC
//! datagrams or shared memory; [`drive_stream`](crate::drive_stream) and
//! [`drive_messages`](crate::drive_messages) (and the `asynch` versions with the `tokio`
//! feature) are the thin loops for the common cases.
//!
//! # Requirements on a method
//!
//! - **Key proof before pairing.** Record a peer in the [`TrustStore`](crate::TrustStore)
//!   as waiting only after it proved, inside this handshake, that it holds the key it
//!   presented. See the store's pairing contract.
//! - **`finish` before completion is an error**, never a half-verified peer.
//! - **No input is dropped.** Bytes received beyond the end of the handshake stay in the
//!   [`Output`](Handshake::Output) (a TLS connection keeps its buffered application data).
//! - **Nothing global.** A method keeps its state in the handshake value.

use core::net::SocketAddr;

use crate::id::PeerId;
use crate::key::KeyMaterial;

/// How a method divides what it sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Framing {
    /// A byte stream: the boundaries between [`receive`](Handshake::receive) calls and
    /// between [`transmit`](Handshake::transmit) results mean nothing, so the bytes may be
    /// split or joined by the transport. Reliable, ordered transports only.
    Stream,
    /// Discrete messages: each [`transmit`](Handshake::transmit) result is one message and
    /// each [`receive`](Handshake::receive) call gets exactly one. A byte-stream transport
    /// must add framing; [`drive_stream`](crate::drive_stream) prefixes each message with its
    /// length as four big-endian bytes.
    Message,
}

/// A verified peer and what the method produced besides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Established<S> {
    /// The peer's id, verified by the handshake.
    pub peer: PeerId,
    /// The peer's public key material, when the method has one, to hand to a
    /// [`TrustStore`](crate::TrustStore).
    pub key: Option<KeyMaterial>,
    /// What the method yields: an encrypted channel (for TLS, the finished connection),
    /// session keys, or `()` for a method that only authenticates. The type says which.
    pub session: S,
}

/// One authentication exchange, driven by the caller.
pub trait Handshake {
    /// What a completed handshake yields besides the peer.
    type Output;
    /// Why the handshake failed. Never holds key material.
    type Error: core::error::Error + Send + Sync + 'static;

    /// How this method divides what it sends.
    fn framing(&self) -> Framing;

    /// Gives the handshake what arrived from the peer: a chunk of the stream, or one message
    /// (see [`Framing`]).
    ///
    /// # Errors
    ///
    /// When the input is invalid or the peer is refused. The handshake is over after an
    /// error.
    fn receive(&mut self, input: &[u8]) -> Result<(), Self::Error>;

    /// The next thing to send to the peer, or `None` when there is nothing to send until more
    /// input arrives. Call until `None`.
    ///
    /// # Errors
    ///
    /// When the handshake cannot continue.
    fn transmit(&mut self) -> Result<Option<Vec<u8>>, Self::Error>;

    /// Whether the exchange is over and [`finish`](Handshake::finish) can be called; it
    /// succeeds when the method verified a peer. Output still waiting in
    /// [`transmit`](Handshake::transmit) must be sent first.
    fn is_complete(&self) -> bool;

    /// The verified peer and the method's output.
    ///
    /// # Errors
    ///
    /// When the handshake is not complete.
    fn finish(self) -> Result<Established<Self::Output>, Self::Error>;
}

/// An authentication method: a configured factory of handshakes.
///
/// Methods are named `coqui-peer-<method>`; [`name`](Method::name) is the `<method>` part and
/// the name under which the method's keys are kept in the trust store.
pub trait Method {
    /// The handshake this method runs.
    type Handshake: Handshake;

    /// The method name: lowercase letters, digits and `-`, as in [`KeyMaterial::method`].
    fn name(&self) -> &'static str;

    /// Starts a handshake as the side that dials.
    ///
    /// # Errors
    ///
    /// When this method is not configured to dial.
    fn initiate(&self) -> Result<Self::Handshake, <Self::Handshake as Handshake>::Error>;

    /// Starts a handshake as the side that accepts. `source` is where the peer connected from;
    /// it is recorded with a waiting entry.
    ///
    /// # Errors
    ///
    /// When this method is not configured to accept.
    fn respond(
        &self,
        source: Option<SocketAddr>,
    ) -> Result<Self::Handshake, <Self::Handshake as Handshake>::Error>;
}
