// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Expect`]: what a profile asks of one setting.

use alloc::string::String;
use alloc::vec::Vec;

/// What a [`Profile`](super::Profile) expects of one place.
///
/// An expectation holds texts, never typed values: each is parsed by the field's own parser
/// when the profile is bound to a type, and values are compared as typed values. It sits at a
/// leaf, at a list ([`ItemsEqual`](Expect::ItemsEqual), [`SubsetOf`](Expect::SubsetOf)), at
/// an optional group ([`Required`](Expect::Required), [`Absent`](Expect::Absent)), or at a
/// variant ([`Arms`](Expect::Arms)).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Expect {
    /// The value is this one.
    Equal(String),
    /// The value lies between the bounds, both inclusive and both optional. The leaf needs an
    /// order.
    Within {
        /// The smallest value allowed.
        min: Option<String>,
        /// The largest value allowed.
        max: Option<String>,
    },
    /// The value is one of these.
    OneOf(Vec<String>),
    /// The value is at least as strict as this one, along the setting's `stricter` order.
    AsStrictAs(String),
    /// The value is no stricter than this one, along the setting's `stricter` order.
    NoStricterThan(String),
    /// Every item of the list is among these: fewer is stricter.
    SubsetOf(Vec<String>),
    /// The list holds exactly these items, in order.
    ItemsEqual(Vec<String>),
    /// The place is set (an optional group is present).
    Required,
    /// The place is not set (an optional group is absent).
    Absent,
    /// The variant is one of these arms.
    Arms(Vec<String>),
    /// No expectation: merging it over another clears that one.
    Any,
}

impl Expect {
    /// This expectation with every text it holds replaced by `***`: what a report shows of an
    /// expectation at a secret.
    #[must_use]
    pub fn masked(&self) -> Expect {
        let star = || String::from("***");
        let stars = |items: &[String]| items.iter().map(|_| star()).collect::<Vec<_>>();
        match self {
            Expect::Equal(_) => Expect::Equal(star()),
            Expect::Within { min, max } => Expect::Within {
                min: min.as_ref().map(|_| star()),
                max: max.as_ref().map(|_| star()),
            },
            Expect::OneOf(items) => Expect::OneOf(stars(items)),
            Expect::AsStrictAs(_) => Expect::AsStrictAs(star()),
            Expect::NoStricterThan(_) => Expect::NoStricterThan(star()),
            Expect::SubsetOf(items) => Expect::SubsetOf(stars(items)),
            Expect::ItemsEqual(items) => Expect::ItemsEqual(stars(items)),
            other => other.clone(),
        }
    }
}
