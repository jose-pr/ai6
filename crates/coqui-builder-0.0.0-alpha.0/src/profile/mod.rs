// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Profile`]: what a settings value is expected to look like.
//!
//! A profile holds one [`Expect`] per key, at a leaf, at an optional group (present or
//! absent) or at a variant (which arms). Checking a value against it ([`Check`]) says, per
//! expectation, whether it is met and why not, with every secret hidden. A profile is
//! made by hand, from a type's defaults ([`Profile::defaults`]), or from a value
//! ([`Profile::against`]), and profiles compose with [`Profile::merge`].
//!
//! Keys are unique. An expectation on an optional group or a variant gates the keys below it:
//! they are checked when the group is present or the arm is the subject's, and otherwise the
//! one finding is at the group or the variant.

pub(crate) mod compare;
mod describable;
mod expand;
mod expect;
mod merge;

use alloc::vec::Vec;

pub use compare::BindError;
pub use describable::{ExpectFill, ProfileFill};
pub use expect::Expect;

use crate::check::Check;
use crate::describe::{Describe, Description};
use crate::inspect::Inspect;
use crate::path::{KeyPath, PathError};
use crate::report::Report;

/// What a settings value is expected to look like.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    entries: Vec<(KeyPath, Expect)>,
}

impl Profile {
    /// A profile that expects nothing.
    pub fn new() -> Profile {
        Profile::default()
    }

    /// This profile with `expect` at `key`, written as a path. An existing expectation at the
    /// key is replaced.
    ///
    /// # Errors
    ///
    /// [`PathError`] when `key` is not a path.
    pub fn expecting(mut self, key: &str, expect: Expect) -> Result<Profile, PathError> {
        self.insert(key.parse()?, expect);
        Ok(self)
    }

    /// Puts `expect` at `key`, returning the one it replaced.
    pub fn insert(&mut self, key: KeyPath, expect: Expect) -> Option<Expect> {
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => Some(core::mem::replace(slot, expect)),
            None => {
                self.entries.push((key, expect));
                None
            }
        }
    }

    /// Takes the expectation at `key` out.
    pub fn remove(&mut self, key: &KeyPath) -> Option<Expect> {
        let at = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(at).1)
    }

    /// The expectation at `key`.
    pub fn get(&self, key: &KeyPath) -> Option<&Expect> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, e)| e)
    }

    /// Every expectation, in the order they were added.
    pub fn entries(&self) -> &[(KeyPath, Expect)] {
        &self.entries
    }

    /// How many expectations there are.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Checks that every expectation can be held by `T`: each text is read by the field's own
    /// parser, and an order is asked only of a leaf that has one.
    ///
    /// # Errors
    ///
    /// The first expectation `T` cannot hold.
    pub fn bind<T: Describe>(&self) -> Result<(), BindError> {
        compare::bind(self, &T::description())
    }

    /// The profile of `T`'s defaults: one expectation per leaf, [`Expect::Equal`] to the
    /// default or [`Expect::Absent`] with none; one [`Expect::ItemsEqual`] per list. A variant
    /// gives its default arm and that arm's leaves.
    pub fn defaults<T: Describe>() -> Profile {
        let description = T::description();
        expand::expand(&description, &expand::Defaults(&description))
    }

    /// The profile of `base`: one expectation per leaf, [`Expect::Equal`] to its value or
    /// [`Expect::Absent`] where it holds none.
    pub fn against<T: Describe + Inspect>(base: &T) -> Profile {
        expand::expand(&T::description(), base)
    }

    /// This profile with `later` laid over it: `later` wins per key, the rest is kept, and an
    /// [`Expect::Any`] in `later` clears that key.
    #[must_use]
    pub fn merge(&self, later: &Profile) -> Profile {
        merge::merge(self, later)
    }
}

impl Profile {
    /// Judges `subject` against this profile, reading its places through `description`, and
    /// pushes a finding per expectation to `report`.
    ///
    /// This is [`Check::check`] for a subject that is not `Describe` itself: a builder holds
    /// the settings it will build, and `description` is theirs.
    pub fn judge(&self, description: &Description, subject: &dyn Inspect, report: &mut Report) {
        compare::run(self, description, subject, report);
    }
}

impl<T: Describe + Inspect> Check<T> for Profile {
    fn check(&self, subject: &T, report: &mut Report) {
        self.judge(&T::description(), subject, report);
    }
}
