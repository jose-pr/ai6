//! The settings of the configuration builders as one value type, and the application protocol
//! name they hold.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::str::FromStr;

use coqui_builder::{Describe, Leaf};

use crate::error::PeerError;
use crate::policy::Policy;

/// The longest application protocol name, in bytes.
const ALPN_MAX: usize = 255;

/// An application protocol name (ALPN) of 1 to 255 bytes, held as text (`h2`, `coqui/1`).
///
/// A name that is not UTF-8 cannot be held. Its text form is the name itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Leaf)]
pub struct Alpn(String);

impl Alpn {
    /// The name as the bytes a configuration carries.
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// The name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Alpn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for Alpn {
    type Err = PeerError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() || text.len() > ALPN_MAX {
            return Err(PeerError::Policy(
                "a protocol name of 1 to 255 bytes".to_owned(),
            ));
        }
        Ok(Self(text.to_owned()))
    }
}

impl TryFrom<&str> for Alpn {
    type Error = PeerError;

    /// The name `text`.
    ///
    /// # Errors
    ///
    /// [`PeerError::Policy`] for an empty name and one over 255 bytes.
    fn try_from(text: &str) -> Result<Self, PeerError> {
        text.parse()
    }
}

impl TryFrom<&[u8]> for Alpn {
    type Error = PeerError;

    /// The name `bytes`, which must be UTF-8.
    ///
    /// # Errors
    ///
    /// [`PeerError::Policy`] for an empty name, one over 255 bytes, and bytes that are not UTF-8.
    fn try_from(bytes: &[u8]) -> Result<Self, PeerError> {
        core::str::from_utf8(bytes)
            .map_err(|_| PeerError::Policy("a protocol name is text, not UTF-8 bytes".to_owned()))?
            .parse()
    }
}

/// What a configuration builder takes whole through `fill`: the application protocols and the
/// [`Policy`], in the form a file, a form or a peer fills.
///
/// Identities, trust and verifiers are not settings: they hold keys and code, so a builder
/// takes them as inputs. Every key is optional; an absent one is the default (no ALPN,
/// [`Policy::default`]), and an unknown key is refused.
///
/// The type is [`Describe`]: a `coqui_builder::Partial<TlsSettings>` is filled from text (the
/// `serde` feature reads TOML, JSON and the other serde formats) and built. The setters of its
/// keys are methods of [`ServerBuilder`](crate::ServerBuilder) and
/// [`ClientBuilder`](crate::ClientBuilder), and a builder of another crate that holds this type
/// takes `TlsSettingsSetters` from [`prelude`](crate::prelude). It is `#[non_exhaustive]`: start
/// from [`Default`] and assign the fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Describe)]
#[builder(setters_on = <'a> crate::config::ServerBuilder<'a>)]
#[builder(setters_on = <'a> crate::config::ClientBuilder<'a>)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct TlsSettings {
    /// The application protocols offered, most preferred first, as text (`h2`, `coqui/1`), each
    /// 1 to 255 bytes.
    #[map(default)]
    pub alpn: Vec<Alpn>,
    /// The policy configurations are built under.
    #[map(default)]
    pub policy: Policy,
}

impl TlsSettings {
    /// The protocols as the bytes a configuration carries.
    pub(crate) fn alpn_bytes(&self) -> Vec<Vec<u8>> {
        self.alpn
            .iter()
            .map(|name| name.as_bytes().to_vec())
            .collect()
    }
}
