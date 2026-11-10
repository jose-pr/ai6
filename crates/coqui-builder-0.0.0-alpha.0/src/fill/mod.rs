// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Fill`]: a settings type being filled one key at a time.
//!
//! A [`Partial`](crate::Partial) wraps the [`Fill`] of its type. The slots in this module
//! ([`LeafSlot`], [`ListSlot`], [`MapSlot`], [`OptionSlot`], [`Wrapped`]) are the building
//! blocks a derive composes and a hand-written impl can use in a few lines.

mod slots;

use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt;

pub use slots::{LeafSlot, ListSlot, MapSlot, OptionSlot, TextMap, Wrap, Wrapped};

use crate::describe::{Bound, LeafError};
use crate::inspect::Inspect;
use crate::path::{KeyPath, PathError, Segment};
use crate::source::{SourceError, Unknown};

/// A type's partly filled state.
///
/// `set` takes the path of a leaf and its text (an empty path means the slot itself, for a
/// leaf); the leaf's own parser reads it. A list is replaced whole with `set_items`. Every
/// method reports an unknown key as [`FillErrorKind::UnknownKey`] with the path relative to
/// the slot, and the caller prepends its own segment with [`FillError::under`].
///
/// Inspecting a fill ([`Inspect`]) answers what was set, never a default.
pub trait Fill<T>: Default + Inspect {
    /// Sets the leaf at `at` from `text`.
    ///
    /// # Errors
    ///
    /// [`FillError`] when the key is unknown or the leaf refuses the text.
    fn set(&mut self, at: &[Segment], text: &str) -> Result<(), FillError>;

    /// Replaces the list at `at` with items of these texts.
    ///
    /// # Errors
    ///
    /// [`FillError`] when the key is unknown, is no list of leaves, or a leaf refuses a text.
    fn set_items(&mut self, at: &[Segment], items: &[String]) -> Result<(), FillError>;

    /// Forgets what was set at `at`: the leaf reads as its default again.
    ///
    /// # Errors
    ///
    /// [`FillError`] when the key is unknown.
    fn clear(&mut self, at: &[Segment]) -> Result<(), FillError>;

    /// Makes `value` what every key reads as when it is not set.
    fn set_default(&mut self, value: T);

    /// The built value. A key that is required and not set is pushed to `missing` under `at`,
    /// every one of them, and the result is `None`.
    fn build(self, at: &KeyPath, missing: &mut Vec<KeyPath>) -> Option<T>;

    /// A fill with every key set from `value`.
    fn from_value(value: T) -> Self;

    /// Sets every key from `value`, keeping the defaults this fill was given.
    ///
    /// The provided body replaces the fill by [`from_value`](Fill::from_value), which keeps
    /// no default; every fill this crate writes overrides it.
    fn put(&mut self, value: T) {
        *self = Self::from_value(value);
    }

    /// Sets the place at `at` from the typed value that `value` holds (an `Option<V>`, taken
    /// out when it is used), keeping the defaults the fill was given.
    ///
    /// The provided body answers a place that is this fill itself: `at` is empty and the
    /// value is a `T`. A container routes a longer `at` to its part, and an optional also
    /// takes the value of the type it holds.
    ///
    /// # Errors
    ///
    /// [`FillError`] when the key is unknown or the value is not of the type of the place;
    /// nothing changes then.
    #[doc(hidden)]
    fn put_any(&mut self, at: &[Segment], value: &mut dyn Any) -> Result<(), FillError>
    where
        T: 'static,
    {
        if let Some(first) = at.first() {
            return Err(FillError::unknown_key(first.clone()));
        }
        put_whole::<T, Self>(self, value)
    }

    /// Whether the place at `at` holds an explicit `None`.
    #[doc(hidden)]
    fn holds_none(&self, at: &[Segment]) -> bool {
        let _ = at;
        false
    }
}

/// Puts the `T` that `value` holds, an `Option<T>`, into `fill`.
///
/// # Errors
///
/// [`FillErrorKind::Unsupported`] when the value is of another type; it is left as it was.
#[doc(hidden)]
pub fn put_whole<T: 'static, F: Fill<T>>(
    fill: &mut F,
    value: &mut dyn Any,
) -> Result<(), FillError> {
    match value.downcast_mut::<Option<T>>().and_then(Option::take) {
        Some(taken) => {
            fill.put(taken);
            Ok(())
        }
        None => Err(FillError::new(FillErrorKind::Unsupported(
            "the value is not of the type of this setting",
        ))),
    }
}

/// Why a key could not be filled.
#[derive(Debug, Clone, PartialEq)]
pub struct FillError {
    at: KeyPath,
    kind: FillErrorKind,
}

/// What went wrong filling a key.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum FillErrorKind {
    /// No such key.
    UnknownKey,
    /// The path is not a path.
    Path(PathError),
    /// The leaf refused the text.
    Leaf(LeafError),
    /// The text breaks a bound the description states.
    Bound(Bound),
    /// The slot cannot be set this way; the text says how it can.
    Unsupported(&'static str),
    /// The source failed.
    Source(SourceError),
    /// The source holds keys that a group refusing unknown keys does not declare.
    UnknownKeys(Unknown),
}

impl FillError {
    /// An error of `kind` at the slot itself.
    pub fn new(kind: FillErrorKind) -> FillError {
        FillError {
            at: KeyPath::root(),
            kind,
        }
    }

    /// An unknown key: the first segment of what was asked for.
    pub fn unknown_key(segment: Segment) -> FillError {
        FillError {
            at: KeyPath::new(alloc::vec![segment]),
            kind: FillErrorKind::UnknownKey,
        }
    }

    /// A leaf's refusal.
    pub fn leaf(error: LeafError) -> FillError {
        FillError::new(FillErrorKind::Leaf(error))
    }

    /// The same error one level up, under `segment`.
    #[must_use]
    pub fn under(mut self, segment: Segment) -> FillError {
        let mut segments = alloc::vec![segment];
        segments.extend(self.at.segments().iter().cloned());
        self.at = KeyPath::new(segments);
        self
    }

    /// Where the key is, relative to the slot that reported it.
    pub fn at(&self) -> &KeyPath {
        &self.at
    }

    /// What went wrong.
    pub fn kind(&self) -> &FillErrorKind {
        &self.kind
    }

    /// Whether the first segment asked for is no key of the slot that reported it.
    pub fn is_unknown_here(&self) -> bool {
        self.kind == FillErrorKind::UnknownKey && self.at.len() == 1
    }
}

impl fmt::Display for FillErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FillErrorKind::UnknownKey => f.write_str("no such setting"),
            FillErrorKind::Path(e) => write!(f, "not a path: {e}"),
            FillErrorKind::Leaf(e) => write!(f, "{e}"),
            FillErrorKind::Bound(e) => write!(f, "{e}"),
            FillErrorKind::Unsupported(why) => f.write_str(why),
            FillErrorKind::Source(e) => write!(f, "the source failed: {e}"),
            FillErrorKind::UnknownKeys(unknown) => {
                f.write_str("unknown settings: ")?;
                for (at, key) in unknown.keys().iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{key}")?;
                }
                Ok(())
            }
        }
    }
}

impl fmt::Display for FillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.at.is_empty() {
            write!(f, "{}", self.kind)
        } else {
            write!(f, "{}: {}", self.at, self.kind)
        }
    }
}

impl core::error::Error for FillError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match &self.kind {
            FillErrorKind::Path(e) => Some(e),
            FillErrorKind::Leaf(e) => Some(e),
            FillErrorKind::Source(e) => Some(e),
            _ => None,
        }
    }
}

impl From<SourceError> for FillError {
    fn from(e: SourceError) -> FillError {
        FillError::new(FillErrorKind::Source(e))
    }
}
