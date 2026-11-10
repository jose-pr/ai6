// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Describe`]: a settings type as data, at run time.
//!
//! [`Describe::description`] returns an owned [`Description`] tree, built on each call:
//! generic parameters and default expressions cannot live in a `static`. A description says
//! what each key is (its [`Kind`], its [`Notes`], whether it is required and what it defaults
//! to); it holds no value. A form, a schema, a [`Profile`](crate::profile::Profile) and a
//! report are all read from it.

mod kind;
mod leaf;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

pub use kind::{Arm, Field, Group, Kind, LeafInfo, Notes, Stricter, Union, Variant};
pub use leaf::{Codec, Leaf, LeafError, Native, assert_round_trip};

use crate::fill::Fill;
use crate::path::{KeyPath, Segment};

/// A type that can say what its settings are, and be filled one key at a time.
///
/// Derive it with `#[derive(Describe)]` (feature `derive`) for a struct of named fields or an
/// enum; implement [`Leaf`] for a type that is one value. Every [`Leaf`] is `Describe`.
pub trait Describe: Sized {
    /// The partly filled state of this type: what [`Partial`](crate::Partial) holds.
    type Fill: Fill<Self>;

    /// The type is `Option<_>`: leaving a field of it out is a value of its own, so it is
    /// never required.
    const OPTIONAL: bool = false;

    /// The type is one text: a [`Leaf`], or an untagged enum whose arms all are. An untagged
    /// enum counts the arms that are not, and holds at most one.
    const LEAF: bool = false;

    /// The type is an untagged enum with a structured arm of its own, which another untagged
    /// enum cannot hold.
    #[doc(hidden)]
    const MIXED: bool = false;

    /// The names of the typed setters the derive wrote for this type.
    #[doc(hidden)]
    const __SETTERS: &'static [&'static str] = &[];

    /// What this type is.
    fn description() -> Description;
}

/// What a setting is: its [`Kind`] and its [`Notes`].
#[derive(Debug, Clone, PartialEq)]
pub struct Description {
    pub(crate) kind: Kind,
    pub(crate) notes: Notes,
}

impl Description {
    /// A description of `kind` with no notes.
    pub fn new(kind: Kind) -> Description {
        Description {
            kind,
            notes: Notes::default(),
        }
    }

    /// A leaf of type `T`.
    pub fn leaf<T: Leaf>() -> Description {
        Description::new(Kind::Leaf(LeafInfo::of::<T>()))
    }

    /// A leaf of type `T` converted by `C`.
    pub fn leaf_of<T, C: Codec<T>>() -> Description {
        let mut description = Description::new(Kind::Leaf(LeafInfo::of_codec::<T, C>()));
        description.notes.sensitive = C::SENSITIVE;
        description
    }

    /// A list of `item`.
    pub fn list(item: Description) -> Description {
        Description::new(Kind::List(Box::new(item)))
    }

    /// An open map from text to `value`.
    pub fn map(value: Description) -> Description {
        Description::new(Kind::Map(Box::new(value)))
    }

    /// A group of `fields`.
    pub fn group(fields: Vec<Field>) -> Description {
        Description::new(Kind::Group(Group::new(fields)))
    }

    /// A tagged variant.
    pub fn variant(variant: Variant) -> Description {
        Description::new(Kind::Variant(variant))
    }

    /// An untagged enum of the text arms `text` and the structured arm `structured`.
    pub fn union(text: LeafInfo, structured: Description) -> Description {
        Description::new(Kind::Union(Union::new(text, structured)))
    }

    /// The kind.
    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    /// The kind, to change.
    pub fn kind_mut(&mut self) -> &mut Kind {
        &mut self.kind
    }

    /// The notes.
    pub fn notes(&self) -> &Notes {
        &self.notes
    }

    /// The notes, to change.
    pub fn notes_mut(&mut self) -> &mut Notes {
        &mut self.notes
    }

    /// The fields of a group, or of a group a variant's arm adds to.
    pub fn group_fields(&self) -> Option<&[Field]> {
        match &self.kind {
            Kind::Group(group) => Some(&group.fields),
            _ => None,
        }
    }

    /// The fields of a group, taken out; none for any other kind.
    pub fn into_group_fields(self) -> Vec<Field> {
        match self.kind {
            Kind::Group(group) => group.fields,
            _ => Vec::new(),
        }
    }

    /// Where `at` leads, or `None` when the description has no such place.
    ///
    /// A list is entered by a position, an open map by any key, a group by a field name, and a
    /// variant by its tag or by the name of a field some arm declares.
    pub fn resolve(&self, at: &[Segment]) -> Option<Resolved<'_>> {
        self.resolve_chosen(at, &|_| None)
    }

    /// [`resolve`](Description::resolve) where `chosen` tells the arm a variant at the given
    /// path holds, which a key several arms share is read from before the default arm.
    pub fn resolve_chosen(
        &self,
        at: &[Segment],
        chosen: &dyn Fn(&[Segment]) -> Option<String>,
    ) -> Option<Resolved<'_>> {
        let mut current = self;
        let mut field: Option<&Field> = None;
        let mut default: Option<String> = None;
        let mut default_items: Option<Vec<String>> = None;
        let mut gates: Vec<Gate> = Vec::new();
        for (depth, segment) in at.iter().enumerate() {
            if let Kind::Union(union) = &current.kind {
                gates.push(Gate {
                    at: KeyPath::new(at[..depth].to_vec()),
                    arms: None,
                    union: true,
                });
                current = union.structured();
            }
            let name = segment.name();
            match &current.kind {
                Kind::Group(group) => {
                    let found = group.field(&name)?;
                    if found.optional && matches!(found.description.kind, Kind::Group(_)) {
                        gates.push(Gate {
                            at: KeyPath::new(at[..=depth].to_vec()),
                            arms: None,
                            union: false,
                        });
                    }
                    field = Some(found);
                    default = found.default.clone();
                    default_items = found.default_items.clone();
                    current = &found.description;
                }
                Kind::Variant(variant) => {
                    if *name == *variant.tag {
                        field = None;
                        default = variant.default_arm.clone();
                        default_items = None;
                        current = &variant.tag_description;
                        continue;
                    }
                    let mut arms = Vec::new();
                    let mut found: Option<&Field> = None;
                    let mut tag = at[..depth].to_vec();
                    tag.push(Segment::Key(variant.tag.clone()));
                    let preferred = chosen(&tag).or_else(|| variant.default_arm.clone());
                    for arm in &variant.arms {
                        if let Some(f) = arm.fields.iter().find(|f| *f.key == *name) {
                            arms.push(arm.name.clone());
                            // A shared key reads as the chosen arm's field, else the default's.
                            if found.is_none() || preferred.as_deref() == Some(&arm.name) {
                                found = Some(f);
                            }
                        }
                    }
                    let found = found?;
                    gates.push(Gate {
                        at: KeyPath::new(at[..depth].to_vec()),
                        arms: Some(arms),
                        union: false,
                    });
                    if found.optional && matches!(found.description.kind, Kind::Group(_)) {
                        gates.push(Gate {
                            at: KeyPath::new(at[..=depth].to_vec()),
                            arms: None,
                            union: false,
                        });
                    }
                    field = Some(found);
                    default = found.default.clone();
                    default_items = found.default_items.clone();
                    current = &found.description;
                }
                Kind::List(item) => {
                    if !matches!(segment, Segment::Index(_)) {
                        return None;
                    }
                    field = None;
                    default = None;
                    default_items = None;
                    current = item;
                }
                Kind::Map(value) => {
                    field = None;
                    default = None;
                    default_items = None;
                    current = value;
                }
                _ => return None,
            }
        }
        gates.retain(|g| g.at.len() < at.len());
        Some(Resolved {
            description: current,
            field,
            default,
            default_items,
            gates,
        })
    }

    /// Whether the description has a place at `at`.
    pub fn knows(&self, at: &[Segment]) -> bool {
        self.resolve(at).is_some()
    }

    /// The text `at` is when left out, if it has a default.
    pub fn default_at(&self, at: &[Segment]) -> Option<String> {
        self.resolve(at)?.default
    }

    /// Checks a leaf's `text` against the bounds the notes carry: length and step.
    ///
    /// `pattern` and `format` are carried for forms and schemas and are not checked.
    ///
    /// # Errors
    ///
    /// The bound the text breaks.
    pub fn check_text(&self, text: &str) -> Result<(), Bound> {
        let notes = &self.notes;
        let length = text.chars().count() as u64;
        if notes.min_length.is_some_and(|min| length < min) {
            return Err(Bound::TooShort);
        }
        if notes.max_length.is_some_and(|max| length > max) {
            return Err(Bound::TooLong);
        }
        if let (Some(step), Ok(x)) = (notes.multiple_of, text.parse::<f64>()) {
            let ratio = x / step;
            let frac = (ratio % 1.0).abs();
            if frac > 1e-9 && 1.0 - frac > 1e-9 {
                return Err(Bound::NotAMultiple);
            }
        }
        Ok(())
    }

    /// Checks the number of items of a list against the bounds the notes carry.
    ///
    /// # Errors
    ///
    /// The bound the count breaks.
    pub fn check_items(&self, count: usize) -> Result<(), Bound> {
        let count = count as u64;
        if self.notes.min_items.is_some_and(|min| count < min) {
            return Err(Bound::TooFewItems);
        }
        if self.notes.max_items.is_some_and(|max| count > max) {
            return Err(Bound::TooManyItems);
        }
        Ok(())
    }

    /// The places whose value is a secret, as patterns a report matches a key against.
    pub(crate) fn sensitive_patterns(&self) -> Vec<Vec<PatternSegment>> {
        let mut out = Vec::new();
        collect_sensitive(self, &mut Vec::new(), &mut out);
        out
    }
}

/// One segment of a pattern naming secret places: a key, or any key or position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PatternSegment {
    Key(String),
    Any,
}

fn collect_sensitive(
    description: &Description,
    prefix: &mut Vec<PatternSegment>,
    out: &mut Vec<Vec<PatternSegment>>,
) {
    if description.notes.sensitive {
        out.push(prefix.clone());
        return;
    }
    let mut fields = |fields: &[Field], prefix: &mut Vec<PatternSegment>| {
        for field in fields {
            prefix.push(PatternSegment::Key(field.key.clone()));
            collect_sensitive(&field.description, prefix, out);
            prefix.pop();
        }
    };
    match &description.kind {
        Kind::Group(group) => fields(&group.fields, prefix),
        Kind::Variant(variant) => {
            for arm in &variant.arms {
                fields(&arm.fields, prefix);
            }
        }
        Kind::List(item) | Kind::Map(item) => {
            prefix.push(PatternSegment::Any);
            collect_sensitive(item, prefix, out);
            prefix.pop();
        }
        Kind::Union(union) => collect_sensitive(union.structured(), prefix, out),
        _ => {}
    }
}

/// Which bound of a description a datum breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Bound {
    /// Fewer characters than `min_length`.
    TooShort,
    /// More characters than `max_length`.
    TooLong,
    /// Fewer items than `min_items`.
    TooFewItems,
    /// More items than `max_items`.
    TooManyItems,
    /// Not a multiple of `multiple_of`.
    NotAMultiple,
}

impl core::fmt::Display for Bound {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Bound::TooShort => "shorter than the setting allows",
            Bound::TooLong => "longer than the setting allows",
            Bound::TooFewItems => "fewer items than the setting allows",
            Bound::TooManyItems => "more items than the setting allows",
            Bound::NotAMultiple => "not a multiple of the setting's step",
        })
    }
}

/// A place a profile's expectation can depend on: an optional group's presence, the arm of a
/// variant, or the structured arm of an untagged enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    at: KeyPath,
    arms: Option<Vec<String>>,
    union: bool,
}

impl Gate {
    /// The path of the optional group, variant or untagged enum.
    pub fn at(&self) -> &KeyPath {
        &self.at
    }

    /// For a variant, the arms that declare the key reached through it; `None` for an optional
    /// group or an untagged enum.
    pub fn arms(&self) -> Option<&[String]> {
        self.arms.as_deref()
    }

    /// Whether the key is reached through the structured arm of an untagged enum, which holds
    /// only while the enum is on that arm.
    pub fn is_union(&self) -> bool {
        self.union
    }
}

/// A place found in a description.
#[derive(Debug, Clone)]
pub struct Resolved<'a> {
    description: &'a Description,
    field: Option<&'a Field>,
    default: Option<String>,
    default_items: Option<Vec<String>>,
    gates: Vec<Gate>,
}

impl<'a> Resolved<'a> {
    /// What the place is.
    pub fn description(&self) -> &'a Description {
        self.description
    }

    /// The field the last segment named, if it named one.
    pub fn field(&self) -> Option<&'a Field> {
        self.field
    }

    /// The text the place is when left out.
    pub fn default(&self) -> Option<&str> {
        self.default.as_deref()
    }

    /// The texts of the items a list is when left out.
    pub fn default_items(&self) -> Option<&[String]> {
        self.default_items.as_deref()
    }

    /// The optional groups and variants between the root and the place, outermost first.
    pub fn gates(&self) -> &[Gate] {
        &self.gates
    }
}
