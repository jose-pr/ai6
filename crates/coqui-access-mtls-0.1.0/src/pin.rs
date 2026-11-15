//! What an account pins a client certificate by: its fingerprint, or its public key.

use std::fmt;

use coqui_access::guatiao::{FromValue, Schema, ToValue};
use coqui_peer_tls::{Digest, PeerId};

/// One thing an account pins, as a record names it: a `kind` and a canonical `value`.
///
/// - `fingerprint`: the SHA-256 of the whole certificate, as `sha256:<hex>`. A renewed
///   certificate is a new fingerprint.
/// - `key`: the SHA-256 of the certificate's SubjectPublicKeyInfo, as `sha256:<hex>`. A
///   certificate renewed on the same key keeps it.
///
/// As text, for [`AccountPut::secret`](coqui_access::AccountPut::secret), a fingerprint is
/// `sha256:<hex>` (any spelling `coqui-peer-tls` reads) and a key is `key:sha256:<hex>`.
#[derive(Clone, PartialEq, Eq, Hash, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct Pin {
    /// [`Pin::FINGERPRINT`] or [`Pin::KEY`].
    pub kind: String,
    /// `sha256:<64 hex digits>` of what the kind names.
    pub value: String,
}

/// The two kinds, as the program tells them apart.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Kind {
    Fingerprint,
    Key,
}

/// A pin in one spelling, so two spellings of the same thing compare equal.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Normal {
    pub(crate) kind: Kind,
    pub(crate) value: String,
}

impl Pin {
    /// The kind of a pin on the whole certificate.
    pub const FINGERPRINT: &'static str = "fingerprint";
    /// The kind of a pin on the certificate's public key.
    pub const KEY: &'static str = "key";

    /// The pin on a certificate's SHA-256 fingerprint.
    pub fn fingerprint(fingerprint: &PeerId) -> Self {
        Self {
            kind: Self::FINGERPRINT.to_owned(),
            value: fingerprint.canonical(),
        }
    }

    /// The pin on the public key of a DER certificate; `None` when it does not parse.
    pub fn key_of_certificate(der: &[u8]) -> Option<Self> {
        coqui_access_claims::public_key_hash(der).map(|value| Self {
            kind: Self::KEY.to_owned(),
            value,
        })
    }

    /// The pin on a DER `SubjectPublicKeyInfo`.
    pub fn key_of_public_key_info(info: &[u8]) -> Self {
        Self {
            kind: Self::KEY.to_owned(),
            value: coqui_access_claims::public_key_info_hash(info),
        }
    }

    /// Reads a pin from the text [`text`](Pin::text) writes.
    ///
    /// # Errors
    ///
    /// When the text is neither a SHA-256 fingerprint nor `key:sha256:<hex>`.
    pub fn parse(text: &str) -> Result<Self, PinError> {
        let pin = match text.strip_prefix("key:") {
            Some(value) => Self {
                kind: Self::KEY.to_owned(),
                value: value.to_owned(),
            },
            None => Self {
                kind: Self::FINGERPRINT.to_owned(),
                value: text.to_owned(),
            },
        };
        pin.normal()
            .map(|normal| Self {
                kind: pin.kind.clone(),
                value: normal.value,
            })
            .ok_or(PinError)
    }

    /// The text [`parse`](Pin::parse) reads: `sha256:<hex>` for a fingerprint,
    /// `key:sha256:<hex>` for a key.
    pub fn text(&self) -> String {
        match self.kind.as_str() {
            Self::KEY => format!("key:{}", self.value),
            _ => self.value.clone(),
        }
    }

    /// The pin in one spelling, or `None` when its kind or value is not valid.
    pub(crate) fn normal(&self) -> Option<Normal> {
        match self.kind.as_str() {
            Self::FINGERPRINT => {
                let id = self.value.parse::<PeerId>().ok()?;
                (id.digest() == Digest::Sha256).then(|| Normal::of_fingerprint(&id))
            }
            Self::KEY => {
                let hex = self.value.strip_prefix("sha256:")?;
                let valid = hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
                valid.then(|| Normal {
                    kind: Kind::Key,
                    value: format!("sha256:{}", hex.to_ascii_lowercase()),
                })
            }
            _ => None,
        }
    }
}

impl Normal {
    pub(crate) fn of_fingerprint(fingerprint: &PeerId) -> Self {
        Self {
            kind: Kind::Fingerprint,
            value: fingerprint.canonical(),
        }
    }
}

/// The kind of a pin shows; what it pins does not.
impl fmt::Debug for Pin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pin")
            .field("kind", &self.kind)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

/// Text that is not a pin. It never quotes the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PinError;

impl fmt::Display for PinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("not a SHA-256 certificate fingerprint or key:sha256:<hex> key pin")
    }
}

impl core::error::Error for PinError {}
