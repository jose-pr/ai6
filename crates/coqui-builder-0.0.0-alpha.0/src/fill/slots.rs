// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The slots a [`Fill`] is built from, and the [`Describe`] impls of the containers that use
//! them.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt;
use core::marker::PhantomData;

use super::{Fill, FillError, FillErrorKind, put_whole};
use crate::datum::Datum;
use crate::describe::{Codec, Describe, Description, Native};
use crate::inspect::{Entries, Inspect};
use crate::path::{KeyPath, Segment};

/// Keeps a type parameter that only names a function alive in a slot.
struct Marker<P>(PhantomData<fn() -> P>);

impl<P> Default for Marker<P> {
    fn default() -> Self {
        Marker(PhantomData)
    }
}

/// A position more than this far past the end of a list is refused: a gap is filled with
/// unset items that `build` reports missing, and an absurd position is not allocated.
const MAX_GAP: usize = 1024;

fn unsupported(why: &'static str) -> FillError {
    FillError::new(FillErrorKind::Unsupported(why))
}

/// The slot of one value: what was set, and what it reads as when not set.
pub struct LeafSlot<T, C = Native> {
    value: Option<T>,
    default: Option<T>,
    _codec: Marker<C>,
}

impl<T, C> Default for LeafSlot<T, C> {
    fn default() -> Self {
        LeafSlot {
            value: None,
            default: None,
            _codec: Marker::default(),
        }
    }
}

/// Shows whether a value was set, never the value: it may be a secret.
impl<T, C> fmt::Debug for LeafSlot<T, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeafSlot")
            .field("set", &self.value.is_some())
            .finish()
    }
}

impl<T, C: Codec<T>> Inspect for LeafSlot<T, C> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        if !at.is_empty() {
            return None;
        }
        self.value.as_ref().map(|v| {
            let datum = Datum::new(C::TYPE, C::to_text(v));
            if C::SENSITIVE {
                datum.sensitive()
            } else {
                datum
            }
        })
    }

    fn entries(&self, _at: &[Segment]) -> Option<Entries> {
        None
    }
}

impl<T, C: Codec<T>> Fill<T> for LeafSlot<T, C> {
    fn set(&mut self, at: &[Segment], text: &str) -> Result<(), FillError> {
        if let Some(first) = at.first() {
            return Err(FillError::unknown_key(first.clone()));
        }
        self.value = Some(C::from_text(text).map_err(FillError::leaf)?);
        Ok(())
    }

    fn set_items(&mut self, at: &[Segment], _items: &[String]) -> Result<(), FillError> {
        match at.first() {
            Some(first) => Err(FillError::unknown_key(first.clone())),
            None => Err(unsupported("a leaf is not a list")),
        }
    }

    fn clear(&mut self, at: &[Segment]) -> Result<(), FillError> {
        if let Some(first) = at.first() {
            return Err(FillError::unknown_key(first.clone()));
        }
        self.value = None;
        Ok(())
    }

    fn set_default(&mut self, value: T) {
        self.default = Some(value);
    }

    fn build(self, at: &KeyPath, missing: &mut Vec<KeyPath>) -> Option<T> {
        let built = self.value.or(self.default);
        if built.is_none() {
            missing.push(at.clone());
        }
        built
    }

    fn from_value(value: T) -> Self {
        LeafSlot {
            value: Some(value),
            ..LeafSlot::default()
        }
    }

    fn put(&mut self, value: T) {
        self.value = Some(value);
    }
}

/// The slot of an `Option<T>`: nothing set is `None`, and setting any key under it makes it
/// `Some`.
///
/// A group is present once any key below it is set. With [`none`](OptionSlot::none), the text
/// that writes `None` is accepted too, and reads back as that text.
pub struct OptionSlot<T, F> {
    inner: F,
    touched: bool,
    explicit_none: bool,
    default_some: bool,
    none: Option<(&'static str, &'static str)>,
    _value: Marker<T>,
}

impl<T, F: Default> Default for OptionSlot<T, F> {
    fn default() -> Self {
        OptionSlot {
            inner: F::default(),
            touched: false,
            explicit_none: false,
            default_some: false,
            none: None,
            _value: Marker::default(),
        }
    }
}

impl<T, F: fmt::Debug> fmt::Debug for OptionSlot<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OptionSlot")
            .field("inner", &self.inner)
            .field("touched", &self.touched)
            .field("explicit_none", &self.explicit_none)
            .finish()
    }
}

impl<T, F> OptionSlot<T, F> {
    /// The text that writes `None`, shown as a datum of type `ty`.
    #[must_use]
    pub fn none(mut self, ty: &'static str, text: &'static str) -> Self {
        self.none = Some((ty, text));
        self
    }

    fn is_none_text(&self, text: &str) -> bool {
        self.none.is_some_and(|(_, none)| none == text)
    }
}

impl<T, F: Inspect> Inspect for OptionSlot<T, F> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        if self.explicit_none {
            let (ty, text) = self.none?;
            return at.is_empty().then(|| Datum::new(ty, text));
        }
        if !self.touched {
            return None;
        }
        self.inner.at(at)
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        if self.explicit_none || !self.touched {
            return None;
        }
        self.inner.entries(at)
    }
}

impl<T, F: Fill<T>> Fill<Option<T>> for OptionSlot<T, F> {
    fn set(&mut self, at: &[Segment], text: &str) -> Result<(), FillError> {
        if at.is_empty() && self.is_none_text(text) {
            self.explicit_none = true;
            self.touched = false;
            return Ok(());
        }
        self.inner.set(at, text)?;
        self.touched = true;
        self.explicit_none = false;
        Ok(())
    }

    fn set_items(&mut self, at: &[Segment], items: &[String]) -> Result<(), FillError> {
        self.inner.set_items(at, items)?;
        self.touched = true;
        self.explicit_none = false;
        Ok(())
    }

    /// Clearing the whole slot forgets that it was given, what was set below it and the
    /// default of the value it would hold.
    fn clear(&mut self, at: &[Segment]) -> Result<(), FillError> {
        if at.is_empty() {
            self.touched = false;
            self.explicit_none = false;
            self.default_some = false;
            self.inner = F::default();
            return Ok(());
        }
        self.inner.clear(at)
    }

    fn set_default(&mut self, value: Option<T>) {
        match value {
            Some(inner) => {
                self.inner.set_default(inner);
                self.default_some = true;
            }
            None => self.default_some = false,
        }
    }

    fn build(self, at: &KeyPath, missing: &mut Vec<KeyPath>) -> Option<Option<T>> {
        if self.explicit_none {
            return Some(None);
        }
        if self.touched || self.default_some {
            return self.inner.build(at, missing).map(Some);
        }
        Some(None)
    }

    fn put(&mut self, value: Option<T>) {
        match value {
            Some(inner) => {
                self.inner.put(inner);
                self.touched = true;
                self.explicit_none = false;
            }
            None => {
                crate::__private::forget::<T, F>(&mut self.inner);
                self.touched = false;
                self.explicit_none = true;
            }
        }
    }

    fn put_any(&mut self, at: &[Segment], value: &mut dyn Any) -> Result<(), FillError>
    where
        Option<T>: 'static,
    {
        if at.is_empty() && value.is::<Option<Option<T>>>() {
            return put_whole::<Option<T>, Self>(self, value);
        }
        self.inner.put_any(at, value)?;
        self.touched = true;
        self.explicit_none = false;
        Ok(())
    }

    fn holds_none(&self, at: &[Segment]) -> bool {
        if self.explicit_none {
            return at.is_empty();
        }
        self.touched && self.inner.holds_none(at)
    }

    fn from_value(value: Option<T>) -> Self {
        match value {
            Some(inner) => OptionSlot {
                inner: F::from_value(inner),
                touched: true,
                ..OptionSlot::default()
            },
            None => OptionSlot {
                explicit_none: true,
                ..OptionSlot::default()
            },
        }
    }
}

impl<T: Describe> Describe for Option<T> {
    type Fill = OptionSlot<T, T::Fill>;
    const OPTIONAL: bool = true;
    const LEAF: bool = T::LEAF;
    const MIXED: bool = T::MIXED;

    fn description() -> Description {
        T::description()
    }
}

/// The slot of a `Vec<T>`: replaced whole with `set_items`, or one position at a time.
pub struct ListSlot<T, F> {
    items: Vec<F>,
    touched: bool,
    default: Option<Vec<T>>,
}

impl<T, F> Default for ListSlot<T, F> {
    fn default() -> Self {
        ListSlot {
            items: Vec::new(),
            touched: false,
            default: None,
        }
    }
}

impl<T, F: fmt::Debug> fmt::Debug for ListSlot<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListSlot")
            .field("items", &self.items)
            .field("touched", &self.touched)
            .finish()
    }
}

fn position(segment: &Segment) -> Result<usize, FillError> {
    match segment {
        Segment::Index(at) => Ok(*at),
        other => Err(FillError::unknown_key(other.clone())),
    }
}

impl<T, F: Default> ListSlot<T, F> {
    /// Runs `set` on the item at `index`; a new item is added only if `set` succeeds.
    fn item_for(
        &mut self,
        index: usize,
        set: impl FnOnce(&mut F) -> Result<(), FillError>,
    ) -> Result<(), FillError> {
        let under = |e: FillError| e.under(Segment::Index(index));
        match self.items.get_mut(index) {
            Some(item) => set(item).map_err(under),
            None => {
                if index - self.items.len() > MAX_GAP {
                    return Err(under(unsupported(
                        "a position too far past the end of the list",
                    )));
                }
                let mut item = F::default();
                set(&mut item).map_err(under)?;
                self.items.resize_with(index, F::default);
                self.items.push(item);
                Ok(())
            }
        }
    }
}

impl<T, F: Inspect> Inspect for ListSlot<T, F> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        if !self.touched {
            return None;
        }
        let (first, rest) = at.split_first()?;
        self.items.get(first.as_index()?)?.at(rest)
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        if !self.touched {
            return None;
        }
        match at.split_first() {
            None => Some(Entries::Len(self.items.len())),
            Some((first, rest)) => self.items.get(first.as_index()?)?.entries(rest),
        }
    }
}

impl<T, F: Fill<T>> Fill<Vec<T>> for ListSlot<T, F> {
    fn set(&mut self, at: &[Segment], text: &str) -> Result<(), FillError> {
        let Some((first, rest)) = at.split_first() else {
            return Err(unsupported("a list is set with its items"));
        };
        let index = position(first)?;
        self.item_for(index, |slot| slot.set(rest, text))?;
        self.touched = true;
        Ok(())
    }

    fn set_items(&mut self, at: &[Segment], items: &[String]) -> Result<(), FillError> {
        match at.split_first() {
            None => {
                let mut filled = Vec::with_capacity(items.len());
                for (index, text) in items.iter().enumerate() {
                    let mut slot = F::default();
                    slot.set(&[], text)
                        .map_err(|e| e.under(Segment::Index(index)))?;
                    filled.push(slot);
                }
                self.items = filled;
                self.touched = true;
                Ok(())
            }
            Some((first, rest)) => {
                let index = position(first)?;
                self.item_for(index, |slot| slot.set_items(rest, items))?;
                self.touched = true;
                Ok(())
            }
        }
    }

    /// Clearing the list empties it; clearing a position removes that item.
    fn clear(&mut self, at: &[Segment]) -> Result<(), FillError> {
        match at.split_first() {
            None => {
                self.items.clear();
                self.touched = false;
                Ok(())
            }
            Some((first, rest)) => {
                let index = position(first)?;
                if rest.is_empty() {
                    if index < self.items.len() {
                        self.items.remove(index);
                    }
                    return Ok(());
                }
                match self.items.get_mut(index) {
                    Some(item) => item.clear(rest).map_err(|e| e.under(Segment::Index(index))),
                    None => Ok(()),
                }
            }
        }
    }

    fn set_default(&mut self, value: Vec<T>) {
        self.default = Some(value);
    }

    fn build(self, at: &KeyPath, missing: &mut Vec<KeyPath>) -> Option<Vec<T>> {
        if !self.touched {
            return match self.default {
                Some(default) => Some(default),
                None => {
                    missing.push(at.clone());
                    None
                }
            };
        }
        let mut built = Vec::with_capacity(self.items.len());
        let mut complete = true;
        for (index, item) in self.items.into_iter().enumerate() {
            match item.build(&at.child(Segment::Index(index)), missing) {
                Some(value) => built.push(value),
                None => complete = false,
            }
        }
        complete.then_some(built)
    }

    fn put(&mut self, value: Vec<T>) {
        self.items = value.into_iter().map(F::from_value).collect();
        self.touched = true;
    }

    fn put_any(&mut self, at: &[Segment], value: &mut dyn Any) -> Result<(), FillError>
    where
        Vec<T>: 'static,
    {
        let Some((first, rest)) = at.split_first() else {
            return put_whole::<Vec<T>, Self>(self, value);
        };
        let index = position(first)?;
        self.item_for(index, |slot| slot.put_any(rest, value))?;
        self.touched = true;
        Ok(())
    }

    fn holds_none(&self, at: &[Segment]) -> bool {
        let Some((first, rest)) = at.split_first() else {
            return false;
        };
        first
            .as_index()
            .and_then(|index| self.items.get(index))
            .is_some_and(|item| item.holds_none(rest))
    }

    fn from_value(value: Vec<T>) -> Self {
        ListSlot {
            items: value.into_iter().map(F::from_value).collect(),
            touched: true,
            default: None,
        }
    }
}

impl<T: Describe> Describe for Vec<T> {
    type Fill = ListSlot<T, T::Fill>;

    fn description() -> Description {
        Description::list(T::description())
    }
}

/// A map type with text keys that an open map of settings can build.
pub trait TextMap<V>: Sized {
    /// The map holding `entries`.
    fn collect_entries(entries: Vec<(String, V)>) -> Self;
    /// The entries, in the map's order.
    fn into_entries(self) -> Vec<(String, V)>;
}

impl<V> TextMap<V> for BTreeMap<String, V> {
    fn collect_entries(entries: Vec<(String, V)>) -> Self {
        entries.into_iter().collect()
    }
    fn into_entries(self) -> Vec<(String, V)> {
        self.into_iter().collect()
    }
}

#[cfg(feature = "std")]
impl<V, S: core::hash::BuildHasher + Default> TextMap<V>
    for std::collections::HashMap<String, V, S>
{
    fn collect_entries(entries: Vec<(String, V)>) -> Self {
        entries.into_iter().collect()
    }
    fn into_entries(self) -> Vec<(String, V)> {
        self.into_iter().collect()
    }
}

/// The slot of an open map from text keys to `V`: one key at a time.
pub struct MapSlot<M, V, F> {
    entries: BTreeMap<String, F>,
    touched: bool,
    default: Option<M>,
    _value: Marker<V>,
}

impl<M, V, F> Default for MapSlot<M, V, F> {
    fn default() -> Self {
        MapSlot {
            entries: BTreeMap::new(),
            touched: false,
            default: None,
            _value: Marker::default(),
        }
    }
}

impl<M, V, F: fmt::Debug> fmt::Debug for MapSlot<M, V, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MapSlot")
            .field("entries", &self.entries.len())
            .field("touched", &self.touched)
            .finish()
    }
}

impl<M, V, F: Default> MapSlot<M, V, F> {
    /// Runs `set` on the entry under `key`; a new entry is added only if `set` succeeds.
    fn entry_for(
        &mut self,
        key: String,
        set: impl FnOnce(&mut F) -> Result<(), FillError>,
    ) -> Result<(), FillError> {
        match self.entries.get_mut(&key) {
            Some(entry) => set(entry),
            None => {
                let mut entry = F::default();
                set(&mut entry)?;
                self.entries.insert(key, entry);
                Ok(())
            }
        }
    }
}

impl<M, V, F: Inspect> Inspect for MapSlot<M, V, F> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        if !self.touched {
            return None;
        }
        let (first, rest) = at.split_first()?;
        self.entries.get(first.name().as_ref())?.at(rest)
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        if !self.touched {
            return None;
        }
        match at.split_first() {
            None => Some(Entries::Keys(self.entries.keys().cloned().collect())),
            Some((first, rest)) => self.entries.get(first.name().as_ref())?.entries(rest),
        }
    }
}

impl<M: TextMap<V>, V: 'static, F: Fill<V>> Fill<M> for MapSlot<M, V, F> {
    fn set(&mut self, at: &[Segment], text: &str) -> Result<(), FillError> {
        let Some((first, rest)) = at.split_first() else {
            return Err(unsupported("a map is set one key at a time"));
        };
        self.entry_for(first.name().into_owned(), |entry| entry.set(rest, text))
            .map_err(|e| e.under(first.clone()))?;
        self.touched = true;
        Ok(())
    }

    /// No items at the map itself marks it as set without changing its entries.
    fn set_items(&mut self, at: &[Segment], items: &[String]) -> Result<(), FillError> {
        let Some((first, rest)) = at.split_first() else {
            if items.is_empty() {
                self.touched = true;
                return Ok(());
            }
            return Err(unsupported("a map is not a list"));
        };
        self.entry_for(first.name().into_owned(), |entry| {
            entry.set_items(rest, items)
        })
        .map_err(|e| e.under(first.clone()))?;
        self.touched = true;
        Ok(())
    }

    /// Clearing the map empties it; clearing a key removes that entry.
    fn clear(&mut self, at: &[Segment]) -> Result<(), FillError> {
        let Some((first, rest)) = at.split_first() else {
            self.entries.clear();
            self.touched = false;
            return Ok(());
        };
        let key = first.name().into_owned();
        if rest.is_empty() {
            self.entries.remove(&key);
            return Ok(());
        }
        match self.entries.get_mut(&key) {
            Some(entry) => entry.clear(rest).map_err(|e| e.under(first.clone())),
            None => Ok(()),
        }
    }

    fn set_default(&mut self, value: M) {
        self.default = Some(value);
    }

    fn build(self, at: &KeyPath, missing: &mut Vec<KeyPath>) -> Option<M> {
        if !self.touched {
            return match self.default {
                Some(default) => Some(default),
                None => {
                    missing.push(at.clone());
                    None
                }
            };
        }
        let mut built = Vec::with_capacity(self.entries.len());
        let mut complete = true;
        for (key, entry) in self.entries {
            match entry.build(&at.child(Segment::Key(key.clone())), missing) {
                Some(value) => built.push((key, value)),
                None => complete = false,
            }
        }
        complete.then(|| M::collect_entries(built))
    }

    fn put(&mut self, value: M) {
        self.entries = value
            .into_entries()
            .into_iter()
            .map(|(key, entry)| (key, F::from_value(entry)))
            .collect();
        self.touched = true;
    }

    fn put_any(&mut self, at: &[Segment], value: &mut dyn Any) -> Result<(), FillError>
    where
        M: 'static,
    {
        let Some((first, rest)) = at.split_first() else {
            return put_whole::<M, Self>(self, value);
        };
        self.entry_for(first.name().into_owned(), |entry| {
            entry.put_any(rest, value)
        })
        .map_err(|e| e.under(first.clone()))?;
        self.touched = true;
        Ok(())
    }

    fn holds_none(&self, at: &[Segment]) -> bool {
        let Some((first, rest)) = at.split_first() else {
            return false;
        };
        self.entries
            .get(first.name().as_ref())
            .is_some_and(|entry| entry.holds_none(rest))
    }

    fn from_value(value: M) -> Self {
        MapSlot {
            entries: value
                .into_entries()
                .into_iter()
                .map(|(key, entry)| (key, F::from_value(entry)))
                .collect(),
            touched: true,
            default: None,
            _value: Marker::default(),
        }
    }
}

impl<V: Describe + 'static> Describe for BTreeMap<String, V> {
    type Fill = MapSlot<BTreeMap<String, V>, V, V::Fill>;

    fn description() -> Description {
        Description::map(V::description())
    }
}

#[cfg(feature = "std")]
impl<V: Describe + 'static, S: core::hash::BuildHasher + Default + 'static> Describe
    for std::collections::HashMap<String, V, S>
{
    type Fill = MapSlot<std::collections::HashMap<String, V, S>, V, V::Fill>;

    fn description() -> Description {
        Description::map(V::description())
    }
}

/// A struct with one field that is that field's value: a newtype, or a `#[map(transparent)]`
/// struct.
pub trait Wrap: Sized {
    /// The field's type.
    type Inner: 'static;
    /// The struct holding `inner`.
    fn wrap(inner: Self::Inner) -> Self;
    /// The field.
    fn inner(&self) -> &Self::Inner;
    /// The field, taken out.
    fn into_inner(self) -> Self::Inner;
}

/// The slot of a [`Wrap`] type: its field's slot.
pub struct Wrapped<T: Wrap, F> {
    inner: F,
    _wrapper: Marker<T>,
}

impl<T: Wrap, F: Default> Default for Wrapped<T, F> {
    fn default() -> Self {
        Wrapped {
            inner: F::default(),
            _wrapper: Marker::default(),
        }
    }
}

impl<T: Wrap, F: fmt::Debug> fmt::Debug for Wrapped<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Wrapped").field(&self.inner).finish()
    }
}

impl<T: Wrap, F: Inspect> Inspect for Wrapped<T, F> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        self.inner.at(at)
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        self.inner.entries(at)
    }
}

impl<T: Wrap, F: Fill<T::Inner>> Fill<T> for Wrapped<T, F> {
    fn set(&mut self, at: &[Segment], text: &str) -> Result<(), FillError> {
        self.inner.set(at, text)
    }

    fn set_items(&mut self, at: &[Segment], items: &[String]) -> Result<(), FillError> {
        self.inner.set_items(at, items)
    }

    fn clear(&mut self, at: &[Segment]) -> Result<(), FillError> {
        self.inner.clear(at)
    }

    fn set_default(&mut self, value: T) {
        self.inner.set_default(value.into_inner());
    }

    fn build(self, at: &KeyPath, missing: &mut Vec<KeyPath>) -> Option<T> {
        self.inner.build(at, missing).map(T::wrap)
    }

    fn put(&mut self, value: T) {
        self.inner.put(value.into_inner());
    }

    fn put_any(&mut self, at: &[Segment], value: &mut dyn Any) -> Result<(), FillError>
    where
        T: 'static,
    {
        if at.is_empty() && value.is::<Option<T>>() {
            return put_whole::<T, Self>(self, value);
        }
        self.inner.put_any(at, value)
    }

    fn holds_none(&self, at: &[Segment]) -> bool {
        self.inner.holds_none(at)
    }

    fn from_value(value: T) -> Self {
        Wrapped {
            inner: F::from_value(value.into_inner()),
            _wrapper: Marker::default(),
        }
    }
}
