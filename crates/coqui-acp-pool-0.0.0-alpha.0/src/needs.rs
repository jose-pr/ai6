//! Whether a model has the capabilities a role needs.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The answer of [`fits`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fit {
    /// The model reports every capability asked.
    Yes,
    /// It reports capabilities and lacks these.
    No(Vec<String>),
    /// It reports none, so nothing can be said; it counts as able, as a model picker treats it.
    Unreported,
}

impl Fit {
    /// Whether the model may serve: everything but [`Fit::No`].
    #[must_use]
    pub const fn able(&self) -> bool {
        !matches!(self, Self::No(_))
    }
}

/// Whether a model that reports `capabilities` has every name in `needs`. The names are an open set (`chat`, and any name an
/// account provider or extension reports); the library names none. A model that reports none is [`Fit::Unreported`], and so is any
/// model when nothing is needed and nothing is reported.
#[must_use]
pub fn fits<N: AsRef<str>, C: AsRef<str>>(needs: &[N], capabilities: &[C]) -> Fit {
    if needs.is_empty() {
        return Fit::Yes;
    }
    if capabilities.is_empty() {
        return Fit::Unreported;
    }
    let missing: Vec<String> = needs
        .iter()
        .map(AsRef::as_ref)
        .filter(|need| !capabilities.iter().any(|have| have.as_ref() == *need))
        .map(ToString::to_string)
        .collect();
    if missing.is_empty() {
        Fit::Yes
    } else {
        Fit::No(missing)
    }
}
