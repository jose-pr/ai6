// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Inspect`]: reading the value of a setting by its key, without knowing the type.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::datum::Datum;
use crate::describe::Leaf;
use crate::path::Segment;

/// How many items a list has, which keys an open map has, or which keys of a group are set.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Entries {
    /// A list of this many items, at positions `0..n`.
    Len(usize),
    /// An open map's keys, or the keys of a group that are set.
    Keys(Vec<String>),
}

/// A settings value that answers what a key holds.
///
/// [`at`](Inspect::at) answers a leaf as a [`Datum`]; a place that is not a leaf, or is not
/// set (a `None` `Option` included), answers `None`. [`entries`](Inspect::entries) answers the
/// structure: the length of a list, the keys of an open map, the set keys of a group; `None`
/// where the place is not there or is a leaf. A report walks structure through the
/// description and never receives a list.
///
/// Custom [`Check`](crate::Check)s read values through this trait.
pub trait Inspect {
    /// The leaf at `at`, or `None`.
    fn at(&self, at: &[Segment]) -> Option<Datum>;

    /// The structure at `at`, or `None`.
    fn entries(&self, at: &[Segment]) -> Option<Entries>;
}

impl<T: Leaf> Inspect for T {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        at.is_empty().then(|| Datum::new(T::TYPE, self.to_text()))
    }

    fn entries(&self, _at: &[Segment]) -> Option<Entries> {
        None
    }
}

impl<T: Inspect> Inspect for Option<T> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        self.as_ref()?.at(at)
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        self.as_ref()?.entries(at)
    }
}

impl<T: Inspect> Inspect for Vec<T> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        let (first, rest) = at.split_first()?;
        self.get(first.as_index()?)?.at(rest)
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        match at.split_first() {
            None => Some(Entries::Len(self.len())),
            Some((first, rest)) => self.get(first.as_index()?)?.entries(rest),
        }
    }
}

impl<V: Inspect> Inspect for BTreeMap<String, V> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        let (first, rest) = at.split_first()?;
        self.get(first.name().as_ref())?.at(rest)
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        match at.split_first() {
            None => Some(Entries::Keys(self.keys().cloned().collect())),
            Some((first, rest)) => self.get(first.name().as_ref())?.entries(rest),
        }
    }
}

#[cfg(feature = "std")]
impl<V: Inspect, S: core::hash::BuildHasher> Inspect for std::collections::HashMap<String, V, S> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        let (first, rest) = at.split_first()?;
        self.get(first.name().as_ref())?.at(rest)
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        match at.split_first() {
            None => {
                let mut keys: Vec<String> = self.keys().cloned().collect();
                keys.sort();
                Some(Entries::Keys(keys))
            }
            Some((first, rest)) => self.get(first.name().as_ref())?.entries(rest),
        }
    }
}
