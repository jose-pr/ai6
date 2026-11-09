use core::fmt;
use std::io;

use coqui_peer::pair::PairingError;

/// Why a handshake, a session record, a pairing or an identity failed.
///
/// `Display` is one lowercase clause and never quotes the input, a key or a path.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A message is not one this protocol allows at this point: its encoding, a member, a key or
    /// its place in the exchange.
    Malformed,
    /// A message is longer than [`max_message`](crate::WebCryptoSettings::max_message) allows,
    /// or an export is longer than HKDF can give.
    TooLong,
    /// A signature does not verify: another transcript, another key or another outer binding.
    BadProof,
    /// The peer proved its key, and [`Trust`] does not admit it.
    ///
    /// [`Trust`]: crate::Trust
    Untrusted,
    /// This method has no [`Trust`](crate::Trust) configured for that role.
    Unsupported,
    /// The handshake is over, or a step came outside its place: [`finish`] before the end, a
    /// binding after the first message.
    ///
    /// [`finish`]: coqui_peer::Handshake::finish
    State,
    /// The random source failed.
    Random,
    /// A record does not open: altered, replayed, out of order, under other keys, of an unknown
    /// type, or after the peer's close. The session is over in both directions.
    Record,
    /// The transport ended before the peer's close record: records may have been cut off.
    Truncated,
    /// This side already sent its close record, so it sends no more.
    Closed,
    /// This direction used up [`session_limit`](crate::WebCryptoSettings::session_limit). Only
    /// a close record can still be sent or opened; a new handshake carries on.
    LimitReached,
    /// The text or bytes are not a P-256 private key in PKCS#8.
    Key,
    /// A pairing exchange was refused or failed.
    Pairing(PairingError),
    /// Reading or writing an identity file, or a transport during pairing, failed.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => "the message is not valid here",
            Self::TooLong => "the output or message is over its limit",
            Self::BadProof => "a signature does not verify",
            Self::Untrusted => "the peer proved its key and is not trusted",
            Self::Unsupported => "this method is not configured for that role",
            Self::State => "this step is out of its place in the handshake",
            Self::Random => "the random source failed",
            Self::Record => "a session record does not open, or the session is over",
            Self::Truncated => "the stream ended before the peer closed it",
            Self::Closed => "this side already closed its direction",
            Self::LimitReached => "this direction reached its session limit",
            Self::Key => "not a P-256 PKCS#8 private key",
            Self::Pairing(_) => "the pairing was refused or failed",
            Self::Io(_) => "an identity file or transport could not be read or written",
        })
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Pairing(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<PairingError> for Error {
    fn from(error: PairingError) -> Self {
        Self::Pairing(error)
    }
}
