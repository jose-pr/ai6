// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Source`]: anything a [`Partial`] can be filled from.
//!
//! coqui-builder parses no file format. A source is asked for what the description lists: a
//! text per leaf, the tag before an arm, and how many items a list has or which keys an open
//! map has. It answers what it holds and nothing else, so a file, the environment, a flag set
//! or a value of another library are all sources, and the last one asked wins. A source hands
//! over text only; the leaf's own parser decides whether it is valid.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

pub use crate::inspect::Entries;

use crate::describe::{Describe, Description, Kind, LeafInfo};
use crate::fill::{FillError, FillErrorKind};
use crate::partial::Partial;
use crate::path::{KeyPath, Segment};

/// A source's own failure: a file that cannot be read, a value that is structure where a leaf
/// is expected.
///
/// The message never quotes a setting's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    message: String,
}

impl SourceError {
    /// A failure described by `message`, one lowercase clause.
    pub fn new(message: impl Into<String>) -> SourceError {
        SourceError {
            message: message.into(),
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl core::error::Error for SourceError {}

/// What a [`Partial`] asks while it is being filled.
pub trait Source {
    /// The text at `at`, which is a leaf of type `leaf`; `None` when the source holds none.
    ///
    /// At an untagged enum that has a structured arm this is the text side: `leaf` is the
    /// enum's text arms, and a source holding a table there answers [`entries`](Source::entries)
    /// and may refuse this, which is not a failure.
    ///
    /// # Errors
    ///
    /// [`SourceError`] when the source cannot answer, or holds structure here.
    fn value(&mut self, at: &KeyPath, leaf: &LeafInfo) -> Result<Option<String>, SourceError>;

    /// How many items the list at `at` has, or which keys the open map at `at` has; `None`
    /// when the source holds none. At an untagged enum that has a structured arm, any answer
    /// means the source holds the structured arm there (an empty table included), and giving
    /// a value as well is an error; a source answers `None` where it holds a scalar.
    ///
    /// # Errors
    ///
    /// [`SourceError`] when the source cannot answer.
    fn entries(&mut self, at: &KeyPath) -> Result<Option<Entries>, SourceError>;

    /// The keys the source holds that `known` does not recognise as places of the description.
    fn unknown(&mut self, known: &dyn Fn(&KeyPath) -> bool) -> Vec<KeyPath>;
}

/// The keys a source held that the description does not have.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unknown {
    keys: Vec<KeyPath>,
}

impl Unknown {
    /// The keys.
    pub fn keys(&self) -> &[KeyPath] {
        &self.keys
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl<T: Describe> Partial<T> {
    /// Fills what `source` holds, leaf by leaf; what it does not answer stays as it was. A
    /// list the source answers is replaced whole.
    ///
    /// Returns the keys the source held that the description lacks.
    ///
    /// # Errors
    ///
    /// The first [`FillError`] of the source or of a leaf stops the fill.
    /// [`FillErrorKind::UnknownKeys`] comes last, after everything else was filled, when a
    /// group that refuses unknown keys met one.
    pub fn fill_from(&mut self, source: &mut impl Source) -> Result<Unknown, FillError> {
        let description = self.description.clone();
        let mut path = KeyPath::root();
        fill_place(self, source, &description, &mut path, false)?;
        let unknown = source.unknown(&|key| description.knows(key.segments()));
        let (refused, kept): (Vec<_>, Vec<_>) = unknown
            .into_iter()
            .partition(|key| refuses(&description, key.segments()));
        if refused.is_empty() {
            Ok(Unknown { keys: kept })
        } else {
            Err(FillError::new(FillErrorKind::UnknownKeys(Unknown {
                keys: refused,
            })))
        }
    }
}

/// The largest list position a source may name by a key; a larger one is ignored.
const MAX_POSITION: usize = 65_535;

/// How many items a list has when a source answered `entries`: the length, or one past the
/// largest of the keys, which a source that cannot tell a list from a map writes as digits.
/// `None` when every position named is too large to hold.
///
/// # Errors
///
/// A key that is not a position.
fn positions(entries: Entries) -> Result<Option<usize>, SourceError> {
    match entries {
        Entries::Len(count) => Ok(Some(count)),
        Entries::Keys(keys) => {
            let mut largest: Option<usize> = None;
            for key in &keys {
                if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(SourceError::new("a list has positions, not keys"));
                }
                // Digits too many to hold are a position too large, like any other.
                if let Ok(at) = key.parse::<usize>()
                    && at < MAX_POSITION
                {
                    largest = largest.max(Some(at));
                }
            }
            Ok(if keys.is_empty() {
                Some(0)
            } else {
                largest.map(|at| at + 1)
            })
        }
    }
}

/// Whether the group that would have held `at` refuses keys it does not declare.
fn refuses(description: &Description, at: &[Segment]) -> bool {
    let mut current = description;
    for segment in at {
        if let Kind::Union(union) = current.kind() {
            current = union.structured();
        }
        let name = segment.name();
        match current.kind() {
            Kind::Group(group) => match group.field(&name) {
                Some(field) => current = &field.description,
                None => return group.deny_unknown_fields,
            },
            Kind::Variant(variant) => {
                let found = variant
                    .arms()
                    .iter()
                    .find_map(|arm| arm.fields.iter().find(|f| *f.key == *name));
                match found {
                    Some(field) => current = &field.description,
                    None => return false,
                }
            }
            Kind::List(item) | Kind::Map(item) => current = item,
            _ => return false,
        }
    }
    false
}

/// A text a source handed over, wiped when it drops if it belongs to a sensitive place.
struct Held {
    text: String,
    secret: bool,
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.secret {
            wipe(&mut self.text);
        }
    }
}

/// The texts of a list, held the same way.
struct HeldAll {
    texts: Vec<String>,
    secret: bool,
}

impl Drop for HeldAll {
    fn drop(&mut self) {
        if self.secret {
            self.texts.iter_mut().for_each(wipe);
        }
    }
}

#[cfg(feature = "zeroize")]
fn wipe(text: &mut String) {
    zeroize::Zeroize::zeroize(text);
}

#[cfg(not(feature = "zeroize"))]
fn wipe(_text: &mut String) {}

/// `secret` is whether a place above this one is sensitive: everything below it is too.
fn fill_place<T: Describe>(
    partial: &mut Partial<T>,
    source: &mut impl Source,
    description: &Description,
    path: &mut KeyPath,
    secret: bool,
) -> Result<(), FillError> {
    let secret = secret || description.notes().sensitive;
    match description.kind() {
        Kind::Group(group) => {
            for field in &group.fields {
                path.push(Segment::Key(field.key.clone()));
                fill_place(partial, source, &field.description, path, secret)?;
                path.pop();
            }
        }
        Kind::Variant(variant) => {
            let tag = path.child(Segment::Key(variant.tag().into()));
            if let Kind::Leaf(info) = variant.tag_description().kind()
                && let Some(text) = source.value(&tag, info)?
            {
                let held = Held { text, secret };
                partial.set_at(tag.segments(), &held.text)?;
            }
            let chosen = crate::inspect::Inspect::at(partial, tag.segments());
            if let Some(chosen) = chosen
                && let Some(arm) = variant.arm(chosen.text())
            {
                for field in &arm.fields {
                    path.push(Segment::Key(field.key.clone()));
                    fill_place(partial, source, &field.description, path, secret)?;
                    path.pop();
                }
            }
        }
        Kind::List(item) => {
            let count = source.entries(path)?.map(positions).transpose()?.flatten();
            if let Some(count) = count {
                partial.clear_at(path.segments())?;
                if let Kind::Leaf(info) = item.kind() {
                    let mut held = HeldAll {
                        texts: Vec::with_capacity(count.min(1024)),
                        secret: secret || item.notes().sensitive,
                    };
                    let mut gap = None;
                    for index in 0..count {
                        path.push(Segment::Index(index));
                        let found = source.value(path, info)?;
                        match found {
                            Some(text) if gap.is_none() => held.texts.push(text),
                            Some(mut text) => {
                                if held.secret {
                                    wipe(&mut text);
                                }
                                return Err(SourceError::new(format!(
                                    "the list holds {path} but not {}",
                                    gap.unwrap_or_default()
                                ))
                                .into());
                            }
                            None => {
                                if gap.is_none() {
                                    gap = Some(path.to_string());
                                }
                            }
                        }
                        path.pop();
                    }
                    partial.set_items_at(path.segments(), &held.texts)?;
                } else {
                    partial.set_items_at(path.segments(), &[])?;
                    for index in 0..count {
                        path.push(Segment::Index(index));
                        fill_place(partial, source, item, path, secret)?;
                        path.pop();
                    }
                }
            }
        }
        Kind::Map(value) => match source.entries(path)? {
            Some(Entries::Keys(keys)) => {
                partial.set_items_at(path.segments(), &[])?;
                for key in keys {
                    path.push(Segment::Key(key));
                    fill_place(partial, source, value, path, secret)?;
                    path.pop();
                }
            }
            Some(Entries::Len(_)) => {
                return Err(SourceError::new("a map has keys, not positions").into());
            }
            None => {}
        },
        Kind::Leaf(info) => {
            if let Some(text) = source.value(path, info)? {
                let held = Held { text, secret };
                partial.set_at(path.segments(), &held.text)?;
            }
        }
        Kind::Union(union) => {
            // A source holding a table here answers `entries`; its refusal to read a value
            // from a table is the same answer, not a failure.
            let table = source.entries(path)?.is_some();
            let text = match source.value(path, union.text()) {
                Ok(text) => text,
                Err(_) if table => None,
                Err(error) => return Err(error.into()),
            };
            match (text, table) {
                (Some(text), true) => {
                    drop(Held { text, secret });
                    return Err(SourceError::new(format!(
                        "{path} is given both as a value and as keys below it"
                    ))
                    .into());
                }
                (Some(text), false) => {
                    let held = Held { text, secret };
                    partial.set_at(path.segments(), &held.text)?;
                }
                (None, table) => {
                    if table {
                        // An empty table still chooses the structured arm.
                        partial.set_items_at(path.segments(), &[])?;
                    }
                    fill_place(partial, source, union.structured(), path, secret)?;
                }
            }
        }
    }
    Ok(())
}
