// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Partial`]: a settings type with some of its keys set.
//!
//! Fill it from any number of places in turn (a file, the environment, flags, code): the last
//! write of a key wins. [`Partial::build`] then makes the value or lists every key that is
//! still missing.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::datum::Datum;
use crate::describe::{Describe, Description, Kind};
use crate::fill::{Fill, FillError, FillErrorKind};
use crate::inspect::{Entries, Inspect};
use crate::path::{KeyPath, Segment};

/// Every key that is required and was not set, and every input a builder takes that was not
/// given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    keys: Vec<KeyPath>,
    inputs: Vec<&'static str>,
}

impl Missing {
    /// A report of `keys` and `inputs`.
    pub fn new(keys: Vec<KeyPath>, inputs: Vec<&'static str>) -> Missing {
        Missing { keys, inputs }
    }

    /// The keys, in the order the type declares them.
    pub fn keys(&self) -> &[KeyPath] {
        &self.keys
    }

    /// The names of the inputs, in the order the parts struct declares them.
    pub fn inputs(&self) -> &[&'static str] {
        &self.inputs
    }

    /// Whether nothing is missing.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.inputs.is_empty()
    }
}

/// `missing settings: a, b.c; missing inputs: identity`: a part with nothing omitted is left
/// out. Only names are written, never a value.
impl fmt::Display for Missing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.keys.is_empty() || self.inputs.is_empty() {
            f.write_str("missing settings: ")?;
            for (at, key) in self.keys.iter().enumerate() {
                if at > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{key}")?;
            }
        }
        if !self.inputs.is_empty() {
            if !self.keys.is_empty() {
                f.write_str("; ")?;
            }
            f.write_str("missing inputs: ")?;
            for (at, input) in self.inputs.iter().enumerate() {
                if at > 0 {
                    f.write_str(", ")?;
                }
                f.write_str(input)?;
            }
        }
        Ok(())
    }
}

impl core::error::Error for Missing {}

/// `T` with some of its keys set.
pub struct Partial<T: Describe> {
    pub(crate) fill: T::Fill,
    pub(crate) description: Description,
}

impl<T: Describe + 'static> Partial<T> {
    /// Sets the place at `key` from a typed value, at any depth: `put("limits.total", 5u32)`,
    /// `put("peers[1]", peer)`, `put("limits", Limits { .. })`. An `Option` place takes the
    /// value it holds or an `Option` of it. The value is not rendered as text, so the bounds
    /// the description states for text (`min_length`, `multiple_of`, ..) are not checked.
    ///
    /// # Errors
    ///
    /// [`FillError`] when the key is not a path, names no setting, or the value is not of the
    /// type of that place; nothing changes then.
    pub fn put<V: 'static>(&mut self, key: &str, value: V) -> Result<(), FillError> {
        let at = parse(key)?;
        self.put_at(at.segments(), value)
    }

    /// [`put`](Partial::put) for a key already split into segments.
    ///
    /// # Errors
    ///
    /// As [`put`](Partial::put).
    pub fn put_at<V: 'static>(&mut self, at: &[Segment], value: V) -> Result<(), FillError> {
        let mut slot = Some(value);
        self.fill.put_any(at, &mut slot)
    }
}

impl<T: Describe> Default for Partial<T> {
    fn default() -> Self {
        Partial::new()
    }
}

impl<T: Describe> fmt::Debug for Partial<T>
where
    T::Fill: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Partial").field("fill", &self.fill).finish()
    }
}

fn bound(e: crate::describe::Bound) -> FillError {
    FillError::new(FillErrorKind::Bound(e))
}

impl<T: Describe> Partial<T> {
    /// Nothing set: every key reads as its default, and the required ones are missing.
    pub fn new() -> Self {
        Partial {
            fill: T::Fill::default(),
            description: T::description(),
        }
    }

    /// What `T` is.
    pub fn description(&self) -> &Description {
        &self.description
    }

    /// The fill, for generated setters.
    #[doc(hidden)]
    pub fn __fill_mut(&mut self) -> &mut T::Fill {
        &mut self.fill
    }

    /// Sets the leaf at `key`, written as a path (`waiting.per_source`, `peers[1].host`), from
    /// its text, which the leaf's own parser reads.
    ///
    /// # Errors
    ///
    /// [`FillError`] when the key is not a path, names no setting, or the leaf refuses the
    /// text or a bound the description states.
    pub fn set(&mut self, key: &str, text: &str) -> Result<(), FillError> {
        let at = parse(key)?;
        self.set_at(at.segments(), text)
    }

    /// Replaces the list at `key` with items of these texts.
    ///
    /// # Errors
    ///
    /// [`FillError`] when the key is not a path, is no list of leaves, or an item or the
    /// count breaks what the leaf or the description states.
    pub fn set_items<S: AsRef<str>>(
        &mut self,
        key: &str,
        items: impl IntoIterator<Item = S>,
    ) -> Result<(), FillError> {
        let at = parse(key)?;
        let items: Vec<String> = items.into_iter().map(|s| s.as_ref().into()).collect();
        self.set_items_at(at.segments(), &items)
    }

    /// Forgets what was set at `key`.
    ///
    /// # Errors
    ///
    /// [`FillError`] when the key is not a path or names no setting.
    pub fn clear(&mut self, key: &str) -> Result<(), FillError> {
        let at = parse(key)?;
        self.clear_at(at.segments())
    }

    /// [`set`](Partial::set) for a key already split into segments.
    ///
    /// # Errors
    ///
    /// As [`set`](Partial::set).
    pub fn set_at(&mut self, at: &[Segment], text: &str) -> Result<(), FillError> {
        if let Some(resolved) = self.resolve_place(at) {
            resolved.description().check_text(text).map_err(bound)?;
        }
        self.fill.set(at, text)
    }

    /// [`set_items`](Partial::set_items) for a key already split into segments.
    ///
    /// # Errors
    ///
    /// As [`set_items`](Partial::set_items).
    pub fn set_items_at(&mut self, at: &[Segment], items: &[String]) -> Result<(), FillError> {
        if let Some(resolved) = self.resolve_place(at) {
            let list = resolved.description();
            list.check_items(items.len()).map_err(bound)?;
            if let Kind::List(item) = list.kind() {
                for text in items {
                    item.check_text(text).map_err(bound)?;
                }
            }
        }
        self.fill.set_items(at, items)
    }

    /// [`clear`](Partial::clear) for a key already split into segments.
    ///
    /// # Errors
    ///
    /// [`FillError`] when the key names no setting.
    pub fn clear_at(&mut self, at: &[Segment]) -> Result<(), FillError> {
        self.fill.clear(at)
    }

    /// Takes the keys `later` has set, leaving the rest as they are.
    ///
    /// A list `later` sets replaces the list; an open map is merged key by key.
    pub fn overlay(&mut self, later: Partial<T>) {
        let description = later.description.clone();
        let mut path = Vec::new();
        overlay_walk(self, &later, &description, &mut path);
    }

    /// The value, or every key that is still missing.
    ///
    /// # Errors
    ///
    /// [`Missing`], listing all of them.
    pub fn build(self) -> Result<T, Missing> {
        let mut missing = Vec::new();
        let built = self.fill.build(&KeyPath::root(), &mut missing);
        match built {
            Some(value) if missing.is_empty() => Ok(value),
            _ => Err(Missing {
                keys: missing,
                inputs: Vec::new(),
            }),
        }
    }
}

fn parse(key: &str) -> Result<KeyPath, FillError> {
    key.parse()
        .map_err(|e| FillError::new(FillErrorKind::Path(e)))
}

/// Copies what `later` set at and below `description`'s place `path` onto `target`.
fn overlay_walk<T: Describe>(
    target: &mut Partial<T>,
    later: &Partial<T>,
    description: &Description,
    path: &mut Vec<Segment>,
) {
    // An explicit `None` forgets what the target holds; one with a text is laid over as that
    // text, below. A list position is left alone: clearing it would shift its neighbours.
    if !matches!(path.last(), Some(Segment::Index(_)))
        && later.fill.holds_none(path)
        && later.fill.at(path).is_none()
    {
        let _ = target.fill.clear(path);
        return;
    }
    match description.kind() {
        Kind::Group(group) => {
            for field in &group.fields {
                path.push(Segment::Key(field.key.clone()));
                overlay_walk(target, later, &field.description, path);
                path.pop();
            }
        }
        Kind::Variant(variant) => {
            let mut tag = path.clone();
            tag.push(Segment::Key(variant.tag().into()));
            if let Some(datum) = later.fill.at(&tag) {
                let _ = target.set_at(&tag, datum.text());
            }
            for arm in variant.arms() {
                for field in &arm.fields {
                    path.push(Segment::Key(field.key.clone()));
                    overlay_walk(target, later, &field.description, path);
                    path.pop();
                }
            }
        }
        Kind::List(item) => {
            let Some(Entries::Len(count)) = later.fill.entries(path) else {
                return;
            };
            if matches!(item.kind(), Kind::Leaf(_)) {
                let mut texts = Vec::new();
                for index in 0..count {
                    path.push(Segment::Index(index));
                    texts.extend(later.fill.at(path).map(|d| String::from(d.text())));
                    path.pop();
                }
                let _ = target.set_items_at(path, &texts);
            } else {
                let _ = target.set_items_at(path, &[]);
                for index in 0..count {
                    path.push(Segment::Index(index));
                    overlay_walk(target, later, item, path);
                    path.pop();
                }
            }
        }
        Kind::Map(value) => {
            if let Some(Entries::Keys(keys)) = later.fill.entries(path) {
                let _ = target.set_items_at(path, &[]);
                for key in keys {
                    path.push(Segment::Key(key));
                    overlay_walk(target, later, value, path);
                    path.pop();
                }
            }
        }
        Kind::Leaf(_) => {
            if let Some(datum) = later.fill.at(path) {
                let _ = target.set_at(path, datum.text());
            }
        }
        Kind::Union(union) => {
            if let Some(datum) = later.fill.at(path) {
                let _ = target.set_at(path, datum.text());
            } else if later.fill.entries(path).is_some() {
                // Choosing the structured arm drops a text, and then its keys are laid over.
                let _ = target.set_items_at(path, &[]);
                overlay_walk(target, later, union.structured(), path);
            }
        }
    }
}

impl<T: Describe> From<T> for Partial<T> {
    /// Every key set from `value`.
    fn from(value: T) -> Self {
        Partial {
            fill: <T::Fill as Fill<T>>::from_value(value),
            description: T::description(),
        }
    }
}

impl<T: Describe> Partial<T> {
    /// The place `at` in the description, a key several arms share read from the arm this
    /// partial holds.
    fn resolve_place(&self, at: &[Segment]) -> Option<crate::describe::Resolved<'_>> {
        self.description
            .resolve_chosen(at, &|tag| self.fill.at(tag).map(|d| String::from(d.text())))
    }

    /// Whether the optional groups and variant arms above `resolved` let the place exist.
    fn reachable(&self, resolved: &crate::describe::Resolved<'_>) -> bool {
        for gate in resolved.gates() {
            if gate.is_union() {
                let at = gate.at().segments();
                if self.fill.entries(at).is_some() {
                    continue;
                }
                let on_default = self.fill.at(at).is_none()
                    && self.resolve_place(at).is_some_and(|found| {
                        matches!(found.description().kind(),
                            Kind::Union(union) if union.default_is_structured())
                    });
                if !on_default {
                    return false;
                }
                continue;
            }
            match gate.arms() {
                None => {
                    if self.fill.entries(gate.at().segments()).is_none() {
                        return false;
                    }
                }
                Some(arms) => {
                    let Some(found) = self.resolve_place(gate.at().segments()) else {
                        return false;
                    };
                    let Kind::Variant(variant) = found.description().kind() else {
                        return false;
                    };
                    let mut tag = gate.at().segments().to_vec();
                    tag.push(Segment::Key(variant.tag().into()));
                    match self.at(&tag) {
                        Some(chosen) if arms.iter().any(|a| a == chosen.text()) => {}
                        _ => return false,
                    }
                }
            }
        }
        true
    }
}

impl<T: Describe> Inspect for Partial<T> {
    /// The keys that were set, else the defaults the description states. A key below an
    /// optional group that was not given, or below an arm that is not chosen, answers `None`.
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        if let Some(set) = self.fill.at(at) {
            return Some(set);
        }
        let resolved = self.resolve_place(at);
        if let Some(resolved) = &resolved {
            let info = match resolved.description().kind() {
                Kind::Leaf(info) => Some(info),
                // A union answers its default text only while no structured key is set.
                Kind::Union(union) if self.fill.entries(at).is_none() => Some(union.text()),
                _ => None,
            };
            if let Some(info) = info {
                if !self.reachable(resolved) {
                    return None;
                }
                if let Some(text) = resolved.default() {
                    return Some(Datum::new(info.ty(), text));
                }
            }
        }
        // An item of a list left to its default.
        let (last, parent) = at.split_last()?;
        let index = last.as_index()?;
        let list = self.resolve_place(parent)?;
        if !self.reachable(&list) || self.fill.entries(parent).is_some() {
            return None;
        }
        let list_kind = match list.description().kind() {
            Kind::Union(union) => union.structured().kind(),
            other => other,
        };
        let Kind::List(item) = list_kind else {
            return None;
        };
        let Kind::Leaf(info) = item.kind() else {
            return None;
        };
        list.default_items()?
            .get(index)
            .map(|text| Datum::new(info.ty(), text.as_str()))
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        if let Some(set) = self.fill.entries(at) {
            return Some(set);
        }
        let resolved = self.resolve_place(at)?;
        if !self.reachable(&resolved) {
            return None;
        }
        if let Kind::Union(union) = resolved.description().kind()
            && union.default_is_structured()
            && self.fill.at(at).is_none()
            && resolved.default_items().is_none()
        {
            return Some(Entries::Keys(Vec::new()));
        }
        resolved
            .default_items()
            .map(|items| Entries::Len(items.len()))
    }
}
