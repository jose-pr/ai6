// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Datum`]: one setting's value as a type name and its text.
//!
//! A datum is always one scalar leaf. Structure (a list, an open map, a group) lives only in
//! the [`Description`](crate::Description), and every leaf parses and prints its own text
//! ([`Leaf`](crate::Leaf)). A datum exists to be shown and passed around: equality and order
//! of two settings are the field type's, never the comparison of their texts.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// A type name and a text, with a mark for a secret.
///
/// [`Display`](fmt::Display) is `Type(text)` (`Duration(30s)`, `SocketMode(0o660)`), and a
/// sensitive datum is `Type(***)`; `Debug` masks it the same way.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Datum {
    ty: &'static str,
    text: String,
    sensitive: bool,
}

impl Datum {
    /// A datum of type `ty` written `text`.
    pub fn new(ty: &'static str, text: impl Into<String>) -> Datum {
        Datum {
            ty,
            text: text.into(),
            sensitive: false,
        }
    }

    /// The type name.
    pub fn ty(&self) -> &'static str {
        self.ty
    }

    /// The text. A sensitive datum still holds it: the caller decides where it goes.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the datum is a secret.
    pub fn is_sensitive(&self) -> bool {
        self.sensitive
    }

    /// This datum marked as a secret: it shows as `Type(***)`.
    #[must_use]
    pub fn sensitive(mut self) -> Datum {
        self.sensitive = true;
        self
    }
}

/// With `zeroize`, the text of a sensitive datum is wiped when it drops.
#[cfg(feature = "zeroize")]
impl Drop for Datum {
    fn drop(&mut self) {
        if self.sensitive {
            zeroize::Zeroize::zeroize(&mut self.text);
        }
    }
}

impl fmt::Display for Datum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.sensitive {
            write!(f, "{}(***)", self.ty)
        } else {
            write!(f, "{}({})", self.ty, self.text)
        }
    }
}

impl fmt::Debug for Datum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// What a finding shows of a place: nothing, one datum, the items of a list, or the leaves
/// of the structured arm of an untagged enum.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Shown {
    /// The place is not set.
    #[default]
    Unset,
    /// A leaf.
    One(Datum),
    /// A list, item by item.
    Items(Vec<Datum>),
    /// The structured arm of an untagged enum, leaf by leaf as `(key below the enum, datum)`.
    Fields(Vec<(String, Datum)>),
}

impl Shown {
    /// The same, with every datum marked as a secret.
    #[must_use]
    pub fn sensitive(self) -> Shown {
        match self {
            Shown::Unset => Shown::Unset,
            Shown::One(d) => Shown::One(d.sensitive()),
            Shown::Items(items) => Shown::Items(items.into_iter().map(Datum::sensitive).collect()),
            Shown::Fields(fields) => Shown::Fields(
                fields
                    .into_iter()
                    .map(|(key, datum)| (key, datum.sensitive()))
                    .collect(),
            ),
        }
    }
}

impl fmt::Display for Shown {
    /// `unset`, `Type(text)`, `[Type(a), Type(b)]`, or `{key: Type(a), other: Type(b)}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Shown::Unset => f.write_str("unset"),
            Shown::One(d) => d.fmt(f),
            Shown::Items(items) => {
                f.write_str("[")?;
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    item.fmt(f)?;
                }
                f.write_str("]")
            }
            Shown::Fields(fields) => {
                f.write_str("{")?;
                for (at, (key, datum)) in fields.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{key}: ")?;
                    datum.fmt(f)?;
                }
                f.write_str("}")
            }
        }
    }
}
