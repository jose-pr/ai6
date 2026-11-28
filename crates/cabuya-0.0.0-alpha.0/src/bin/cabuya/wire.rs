//! The link's transport: a framed TCP connection, and dialing a paired host over it.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use cabuya::auth::{Identity, Proof, Sealed, Trust, prove_out};
use coqui_peer::PeerId;
use coqui_transport::frame::LengthPrefix;
use coqui_transport::tcp::Connector;
use coqui_transport::{StreamChannel, StreamChannelBuilder};

/// The longest message a proof may send, which the proof's own check bounds again.
pub const PROOF_MESSAGE: usize = 1 << 16;
/// How long a proof or a pairing may take.
pub const PROOF_WITHIN: Duration = Duration::from_secs(15);
/// How long a connection may take to open.
pub const CONNECT_WITHIN: Duration = Duration::from_secs(10);

/// A channel of length-prefixed messages over `stream`, which both ends build the same way.
pub fn channel(stream: TcpStream) -> Result<StreamChannel, String> {
    StreamChannelBuilder::new()
        .codec(LengthPrefix::u32_le())
        .max_message(1 << 20)
        .stream(stream)
        .build()
        .map_err(|error| format!("the connection could not be used: {error:?}"))
}

/// Opens a connection to `address`, a `HOST:PORT` that may name a host.
pub fn open(address: &str) -> Result<TcpStream, String> {
    let addresses: Vec<SocketAddr> = address
        .to_socket_addrs()
        .map_err(|error| format!("cannot resolve {address}: {error}"))?
        .collect();
    Connector::new(addresses)
        .connect(CONNECT_WITHIN)
        .map_err(|error| format!("cannot connect to {address}: {error}"))
}

/// A connection that has proved `peer` to this side, as `identity`.
pub fn prove_peer(
    address: &str,
    identity: &Identity,
    peer: PeerId,
) -> Result<(StreamChannel, cabuya::auth::Session), String> {
    let channel = channel(open(address)?)?;
    let proof = Proof::builder(identity.clone())
        .responders(Trust::pinned([peer]))
        .build()
        .map_err(|error| error.to_string())?;
    match prove_out(&proof, &channel, PROOF_WITHIN, PROOF_MESSAGE) {
        Ok(done) => Ok((channel, done.session)),
        Err(error) => {
            coqui_transport::Channel::close(&channel);
            Err(format!("{address} did not prove the paired key: {error}"))
        }
    }
}

/// `channel` under `session`, the form a link runs over.
pub fn sealed<'a>(
    channel: &'a StreamChannel,
    session: cabuya::auth::Session,
) -> Sealed<'a, StreamChannel> {
    Sealed::new(channel, session)
}
