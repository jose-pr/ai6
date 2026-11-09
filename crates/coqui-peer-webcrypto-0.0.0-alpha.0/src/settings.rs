//! The settings of the methods as plain values a file, a form or a peer fills.

use crate::message::DEFAULT_MAX_MESSAGE;
use crate::session::DEFAULT_SESSION_LIMIT;

/// What a [`WebCryptoBuilder`](crate::WebCryptoBuilder) takes whole through `settings`.
///
/// Identities, trust and the random source are not settings: they hold keys and code, so the
/// builder takes them as arguments. Every key is optional (an absent one is its default) and an
/// unknown key is refused. The type is a guatiao value (`from_value`, `to_value`, `schema`), and
/// `guatiao_serde::text` reads and writes its text. It is `#[non_exhaustive]`: start from
/// [`Default`] and assign the fields.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, guatiao::ToValue, guatiao::FromValue, guatiao::Schema,
)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct WebCryptoSettings {
    /// The longest handshake message accepted, before it is parsed; [`DEFAULT_MAX_MESSAGE`]. A
    /// value below what the messages need makes every handshake fail.
    #[map(default = WebCryptoSettings::default().max_message)]
    pub max_message: usize,
    /// What each direction of a session may carry, each record counting its length and 32;
    /// [`DEFAULT_SESSION_LIMIT`]. There is no rekeying: at the limit `seal` and `open` give
    /// [`Error::LimitReached`](crate::Error::LimitReached), a close record still passes, and a new
    /// handshake carries on. Above the default, AES-GCM's confidentiality margin shrinks.
    #[map(default = WebCryptoSettings::default().session_limit)]
    pub session_limit: u64,
}

impl Default for WebCryptoSettings {
    fn default() -> Self {
        Self {
            max_message: DEFAULT_MAX_MESSAGE,
            session_limit: DEFAULT_SESSION_LIMIT,
        }
    }
}

/// What a [`PairingMethodBuilder`](crate::PairingMethodBuilder) takes whole through `settings`.
///
/// Read as [`WebCryptoSettings`] is. A pairing session carries no application records, so it has no
/// session limit.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, guatiao::ToValue, guatiao::FromValue, guatiao::Schema,
)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct PairingSettings {
    /// As [`WebCryptoSettings::max_message`].
    #[map(default = PairingSettings::default().max_message)]
    pub max_message: usize,
}

impl Default for PairingSettings {
    fn default() -> Self {
        Self {
            max_message: DEFAULT_MAX_MESSAGE,
        }
    }
}
