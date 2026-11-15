//! Where the pins of an account are kept: the contract a matcher asks, and the two sources
//! that ship (a per-user file in `authorized`, and any [`Table`] of pins).
//!
//! A pin is what one account has said it is known by: the SHA-256 of a whole certificate (a
//! *fingerprint*) or of its public key (a *key*). The library never decides where pins live;
//! a consumer gives the [`Matcher`](crate::matcher::Matcher) a [`PinSource`] by name.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, Schema, ToValue};
use coqui_access::{Claims, Person};

use crate::table::{Keyed, Reloading, SourceError, Table};

pub mod authorized;
pub use authorized::{
    AuthorizedFile, DirectReader, FileRead, PinFileReader, ReadFailure, TemplateError,
};

/// The pins of the certificate a person presented: what a [`PinSource`] looks for. Each is
/// `sha256:<hex>`, lowercase.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PinSet {
    /// The SHA-256 of the whole certificate.
    pub fingerprint: Option<String>,
    /// The SHA-256 of its `SubjectPublicKeyInfo`.
    pub key: Option<String>,
}

impl PinSet {
    /// The pins the claims `fingerprint` and `spki` carry.
    pub fn from_claims(claims: &Claims) -> Self {
        let pick = |name: &str| {
            claims
                .get(name)
                .first()
                .map(|value| value.to_ascii_lowercase())
        };
        Self {
            fingerprint: pick("fingerprint"),
            key: pick("spki"),
        }
    }

    /// Whether there is nothing to look for.
    pub fn is_empty(&self) -> bool {
        self.fingerprint.is_none() && self.key.is_none()
    }
}

/// What a source found for one account.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinCheck {
    /// The account holds one of the pins.
    Holds,
    /// The source was read and the account holds none of them.
    Absent,
    /// The source cannot be trusted for this account (a file with the wrong permissions, a
    /// home that is not known): counted as no pins, and the reason is reported. The reason
    /// never holds a pin.
    Refused(String),
}

/// A place pins are kept, asked about one account at a time.
///
/// `check` is called only for an account a rule already named, never to scan: a source
/// answers for `person` alone. `Err` is "could not find out" and ends the match with an
/// error; a source that can read what it needs but must not use it answers
/// [`PinCheck::Refused`].
pub trait PinSource: Send + Sync {
    /// Whether `person` holds one of `pins`.
    fn check(&self, person: &Person, pins: &PinSet) -> Result<PinCheck, ProviderError>;
}

/// Whether `text` is a pin, and if so whether it is one of `wanted`.
///
/// A pin is `sha256:<64 hex>` (a fingerprint) or `spki-sha256:<64 hex>` (a public key), in
/// any case. `None` is text that is not a pin.
pub fn pin_matches(text: &str, wanted: &PinSet) -> Option<bool> {
    let valid = |hex: &str| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
    if let Some(hex) = text.strip_prefix("sha256:") {
        return valid(hex).then(|| {
            wanted.fingerprint.as_deref() == Some(&format!("sha256:{}", hex.to_ascii_lowercase()))
        });
    }
    let hex = text.strip_prefix("spki-sha256:")?;
    valid(hex)
        .then(|| wanted.key.as_deref() == Some(&format!("sha256:{}", hex.to_ascii_lowercase())))
}

/// One pin an account holds, as a [`Table`] keeps it: the global source of pins.
#[derive(Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct AccountPin {
    /// The account's name, as its directory gives it ([`Person::name`]).
    pub name: String,
    /// The pin, as a line of a pin file has it: `sha256:<hex>` or `spki-sha256:<hex>`.
    pub pin: String,
}

impl AccountPin {
    /// `name` holds `pin`.
    pub fn new(name: impl Into<String>, pin: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            pin: pin.into(),
        }
    }
}

impl Keyed for AccountPin {
    fn key(&self) -> Cow<'_, str> {
        Cow::Owned(format!(
            "{}
{}",
            self.name,
            self.pin.to_ascii_lowercase()
        ))
    }
}

impl fmt::Debug for AccountPin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountPin")
            .field("name", &"[REDACTED]")
            .field("pin", &"[REDACTED]")
            .finish()
    }
}

/// Pins kept in any [`Table`] of [`AccountPin`]s: the global source, for a consumer whose
/// pins live in one place.
pub struct TablePins {
    pins: Reloading<AccountPin>,
}

impl fmt::Debug for TablePins {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("TablePins").finish_non_exhaustive()
    }
}

impl TablePins {
    /// A source over `table`.
    ///
    /// # Errors
    ///
    /// When the table cannot be read, or holds a line that is not a pin.
    pub fn new(table: Arc<dyn Table<AccountPin>>) -> Result<Self, SourceError> {
        let wanted = PinSet::default();
        Ok(Self {
            pins: Reloading::new(table, move |records| {
                if records
                    .iter()
                    .any(|record| pin_matches(&record.pin, &wanted).is_none())
                {
                    return Err(SourceError::invalid("a pin in the table is not a pin"));
                }
                Ok(())
            })?,
        })
    }
}

impl PinSource for TablePins {
    fn check(&self, person: &Person, pins: &PinSet) -> Result<PinCheck, ProviderError> {
        let held = self
            .pins
            .read(|records| {
                records.iter().any(|record| {
                    record.name == person.name && pin_matches(&record.pin, pins) == Some(true)
                })
            })
            .map_err(|reason| reason.reading("pin table"))?;
        Ok(if held {
            PinCheck::Holds
        } else {
            PinCheck::Absent
        })
    }
}
