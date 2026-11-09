use core::fmt;
use std::io;

/// Failures of ids, key material and the trust store. No message holds key material or a
/// path.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A [`PeerId`](crate::PeerId) is malformed.
    InvalidId,
    /// An id is an MD5 digest, which is not supported.
    WeakDigest,
    /// A digest name is unknown. The variant carries the name; `Display` does not quote it.
    UnknownDigest(String),
    /// Stored key material is malformed, too large, or does not hash to its id.
    Material,
    /// The peer is not waiting for acceptance.
    NotWaiting,
    /// The peer was already accepted.
    AlreadyAccepted,
    /// The peer is not accepted, or its key is retiring.
    NotAccepted,
    /// The key is not the pending successor of an accepted key.
    NoSuccessor,
    /// The key is the pending successor another accepted key announced.
    SuccessorTaken,
    /// Nobody proved the successor's key, and the rotation options require it.
    NotProven,
    /// No open invitation has this secret: it never existed, was used up or was revoked.
    NotInvited,
    /// The invitation or code expired.
    Expired,
    /// An option has no meaning: an invitation label that is not one line, zero uses or
    /// validity, an empty code alphabet, a repeated character, length 0.
    InvalidOptions,
    /// A code holds a character outside its alphabet, or has the wrong length.
    InvalidCode,
    /// The random source failed.
    Random,
    /// A filesystem or stream operation failed.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId => formatter.write_str("peer id is invalid"),
            Self::WeakDigest => {
                formatter.write_str("peer id is an MD5 digest, which is not supported")
            }
            Self::UnknownDigest(_) => {
                formatter.write_str("unknown digest name; supported: sha256, sha384, sha512, sha1")
            }
            Self::Material => formatter.write_str("key material is invalid"),
            Self::NotWaiting => formatter.write_str("peer is not waiting for acceptance"),
            Self::AlreadyAccepted => formatter.write_str("peer is already accepted"),
            Self::NotAccepted => formatter.write_str("peer is not accepted or is retiring"),
            Self::NoSuccessor => formatter.write_str("key is not a pending successor"),
            Self::NotProven => formatter.write_str("nobody proved the successor's key"),
            Self::SuccessorTaken => {
                formatter.write_str("key is the pending successor of another accepted key")
            }
            Self::NotInvited => formatter.write_str("no open invitation has this secret"),
            Self::Expired => formatter.write_str("invitation or code expired"),
            Self::InvalidOptions => formatter.write_str("an option has no meaning"),
            Self::InvalidCode => formatter.write_str("code does not fit its alphabet"),
            Self::Random => formatter.write_str("random source failed"),
            Self::Io(_) => formatter.write_str("storage or stream operation failed"),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}
