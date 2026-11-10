// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a generated builder shares: the link to its settings that the settings' typed setters
//! are written against.

use core::fmt;

use alloc::vec::Vec;

use crate::describe::Describe;
use crate::inspect::Inspect;
use crate::non_default::NonDefault;
use crate::partial::{Missing, Partial};

/// A builder that holds a [`Partial`] of the settings `S`.
///
/// `#[derive(Describe)]` on a settings type writes a trait of typed setters (`<S>Setters`)
/// that every `Holds<S>` has once the trait is in scope. Every builder `#[derive(Builder)]`
/// writes holds its settings' partial, and so does a hand-written one that implements this.
pub trait Holds<S: Describe> {
    /// The settings being filled.
    fn partial_mut(&mut self) -> &mut Partial<S>;
}

/// Why a fallible build gave no value: something was missing, or the crate's own conversion
/// refused the parts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildError<E> {
    /// Every setting and input that was not given, all of them at once.
    Missing(Missing),
    /// The error of the conversion from the parts.
    Failed(E),
}

impl<E: fmt::Display> fmt::Display for BuildError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::Missing(missing) => fmt::Display::fmt(missing, f),
            BuildError::Failed(error) => fmt::Display::fmt(error, f),
        }
    }
}

/// The conversion's own error is the source of a refused build, so a chain does not print
/// it twice.
impl<E: core::error::Error + 'static> core::error::Error for BuildError<E> {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            BuildError::Missing(_) => None,
            BuildError::Failed(error) => error.source(),
        }
    }
}

/// Something that holds the settings it is built from or was built from: a builder, and the
/// value it built.
///
/// `#[derive(Builder)]` implements it for the builder (its view is the [`Partial`], so a key
/// not set reads as its default), and `#[derive(Configured)]` for a built type with one
/// `#[configured]` field. Each derive also writes `impl Check<Self> for Profile`, so a
/// [`Profile`](crate::Profile) judges either, and an inherent `non_defaults`, so no import is
/// needed to call it.
pub trait Configured {
    /// The type of the settings.
    type Settings: Describe;

    /// The settings, read through [`Inspect`].
    fn configured(&self) -> &dyn Inspect;

    /// Every setting that differs from its default.
    fn non_defaults(&self) -> Vec<NonDefault> {
        crate::non_default::listed(&Self::Settings::description(), self.configured())
    }
}
