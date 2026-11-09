use core::fmt;
use std::io;

use coqui_peer::pair::{PairRefusal, PairingError};

/// Sanitized failures while loading, storing or using a peer identity. No message holds key
/// material; only [`CaFile`](PeerError::CaFile) names a path, the file it could not use.
#[derive(Debug)]
#[non_exhaustive]
pub enum PeerError {
    /// A peer id is malformed.
    InvalidId,
    /// A certificate or its private key is missing or invalid.
    Certificate,
    /// A TLS configuration or handshake was refused.
    Tls,
    /// A fingerprint is an MD5 digest, which no policy allows.
    WeakDigest,
    /// A policy setting is invalid or unsupported; the message names it and lists what is
    /// supported. It never holds key material.
    Policy(String),
    /// A trust glob is malformed.
    TrustPattern,
    /// The peer is not waiting for acceptance.
    NotWaiting,
    /// The peer was already accepted.
    AlreadyAccepted,
    /// The path does not name a file inside a directory.
    Path,
    /// The trust policy cannot be used in this role.
    Unsupported,
    /// A pairing exchange, or a key rotation, failed: refused by either side, a malformed
    /// message, an unverified responder, or the store (a rotation announced by a peer that is
    /// not accepted is [`PairingError::Local`] with `coqui_peer::Error::NotAccepted`).
    Pairing(PairingError),
    /// A filesystem or stream operation failed.
    Io(io::Error),
    /// A file of certificate authorities ([`CaPolicy::ca_file`](crate::CaPolicy::ca_file),
    /// [`Roots::bundle_file`](crate::Roots::bundle_file)) or a bundle given as text cannot be
    /// used: unreadable, holding no certificate, or holding one that is malformed or cannot be
    /// a trust anchor.
    CaFile {
        /// The file; `None` for a bundle given as text.
        path: Option<std::path::PathBuf>,
        /// The certificate that failed, by its position in the file from 0; `None` when the
        /// file cannot be read or holds no certificate.
        certificate: Option<usize>,
        /// Why the file could not be read, when that is the failure.
        read: Option<io::Error>,
    },
}

impl fmt::Display for PeerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Pairing(error) = self {
            return write!(formatter, "{error}");
        }
        if let Self::CaFile {
            path,
            certificate,
            read,
        } = self
        {
            match path {
                Some(path) => write!(formatter, "CA file {}: ", path.display())?,
                None => formatter.write_str("CA bundle: ")?,
            }
            return match (certificate, read) {
                (_, Some(_)) => formatter.write_str("cannot be read"),
                (Some(index), None) => write!(
                    formatter,
                    "certificate {index} is malformed or cannot be a trust anchor"
                ),
                (None, None) => formatter.write_str("holds no certificate"),
            };
        }
        formatter.write_str(match self {
            Self::Pairing(_) => "pairing failed",
            Self::CaFile { .. } => "CA file cannot be used",
            Self::InvalidId => "peer id is invalid",
            Self::Certificate => "certificate or private key is invalid",
            Self::Tls => "TLS configuration or handshake failed",
            Self::WeakDigest => "fingerprint is an MD5 digest, which is not supported",
            Self::Policy(message) => message,
            Self::TrustPattern => "certificate trust pattern is invalid",
            Self::NotWaiting => "certificate is not waiting for acceptance",
            Self::AlreadyAccepted => "certificate is already accepted",
            Self::Path => "path must have a parent directory",
            Self::Unsupported => "trust policy is not supported in this role",
            Self::Io(_) => "storage or stream operation failed",
        })
    }
}

impl From<PairingError> for PeerError {
    fn from(error: PairingError) -> Self {
        Self::Pairing(error)
    }
}

impl From<coqui_peer::Error> for PeerError {
    fn from(error: coqui_peer::Error) -> Self {
        match error {
            coqui_peer::Error::InvalidId => Self::InvalidId,
            coqui_peer::Error::WeakDigest => Self::WeakDigest,
            coqui_peer::Error::UnknownDigest(_) => Self::Policy(error.to_string()),
            coqui_peer::Error::NotWaiting => Self::NotWaiting,
            coqui_peer::Error::AlreadyAccepted => Self::AlreadyAccepted,
            coqui_peer::Error::Io(error) => Self::Io(error),
            coqui_peer::Error::Material => Self::Certificate,
            other => Self::Pairing(PairingError::Local(other)),
        }
    }
}

impl core::error::Error for PeerError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Pairing(error) => Some(error),
            Self::CaFile {
                read: Some(error), ..
            } => Some(error),
            _ => None,
        }
    }
}

impl PeerError {
    /// Why a pairing was refused, by this side or the other, when that is the failure.
    pub fn pair_refusal(&self) -> Option<PairRefusal> {
        match self {
            Self::Pairing(PairingError::Refused(refusal)) => Some(*refusal),
            _ => None,
        }
    }

    /// The trust refusal behind this error, when a handshake failed because this side's
    /// verifier refused the peer's certificate. The refused side sees only the TLS alert, so
    /// its own error carries none.
    pub fn refusal(&self) -> Option<Refusal> {
        match self {
            Self::Io(error) => error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<rustls::Error>())
                .and_then(Refusal::from_rustls),
            _ => None,
        }
    }
}

/// Why this side refused a peer: its certificate, or its addresses under a policy's access
/// control list.
///
/// A verifier returns it inside
/// `rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(..)))`, so rustls
/// sends the `certificate_unknown` alert; [`Refusal::from_rustls`] and
/// [`PeerError::refusal`] take it back out. [`AddressDenied`](Refusal::AddressDenied) and an
/// [`AddressUnknown`](Refusal::AddressUnknown) found at the `ClientHello` stop the handshake
/// before any certificate: rustls sends `access_denied`, and the accept helpers report the
/// refusal the same way. Expired and not-yet-valid certificates are rustls's own
/// `CertificateError::Expired` and `NotValidYet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Refusal {
    /// The certificate matches no fixed pin.
    NotPinned,
    /// The certificate is not one the trust store accepted. A server with a store-backed pin
    /// set records it as waiting, within the store's limits, once the peer proved its key.
    NotAccepted,
    /// The certificate matches no pin, and the CA policy does not trust its chain or names,
    /// or the [`Roots`](crate::Roots) do not trust its chain (an unknown issuer, a bad
    /// signature, a certificate not for server authentication).
    NotTrusted,
    /// A pinned peer presented a certificate chain, which the policy does not allow
    /// ([`Policy::allow_chain_when_pinned`](crate::Policy::allow_chain_when_pinned)).
    ChainWhenPinned,
    /// The server's chain is trusted by the [`Roots`](crate::Roots), but the certificate is
    /// not for the server name the client dialled.
    NameMismatch,
    /// The policy's access control list denies the connection's addresses: the rule at index
    /// `rule`, or its default when `None`. Decided from the `ClientHello`, before any
    /// certificate.
    AddressDenied {
        /// The index of the deciding rule in the list; `None` for the default.
        rule: Option<usize>,
    },
    /// The policy's access control list denies this client's certificate from these
    /// addresses: the rule at index `rule`, or its default when `None`. Decided before the
    /// trust store is consulted, so nothing is recorded as waiting.
    PeerDenied {
        /// The index of the deciding rule in the list; `None` for the default.
        rule: Option<usize>,
    },
    /// The policy has an access control list, and the connection's source address, or the
    /// local address a rule with a destination or port needs, is unknown: the list is not
    /// decided, and the connection is refused.
    AddressUnknown,
}

impl Refusal {
    /// The refusal inside `error`, when one of this crate's verifiers produced it.
    pub fn from_rustls(error: &rustls::Error) -> Option<Self> {
        match error {
            rustls::Error::InvalidCertificate(rustls::CertificateError::Other(other)) => {
                other.0.downcast_ref::<Self>().copied()
            }
            _ => None,
        }
    }

    /// The rustls error a verifier returns for this refusal.
    pub(crate) fn into_rustls(self) -> rustls::Error {
        rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
            alloc::sync::Arc::new(self),
        )))
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotPinned => "peer certificate matches no pinned fingerprint",
            Self::NotAccepted => "peer certificate is not an accepted peer of the trust store",
            Self::NotTrusted => {
                "peer certificate matches no pin and is not trusted by the CA policy or roots"
            }
            Self::ChainWhenPinned => {
                "pinned peer presented a certificate chain, which the policy does not allow"
            }
            Self::NameMismatch => "server certificate is not valid for the name dialled",
            Self::AddressDenied { .. } => "access control list denies the client's address",
            Self::PeerDenied { .. } => "access control list denies the client's certificate",
            Self::AddressUnknown => {
                "access control list needs the connection's addresses, which are unknown"
            }
        })
    }
}

impl core::error::Error for Refusal {}
