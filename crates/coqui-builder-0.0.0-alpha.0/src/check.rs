// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Check`]: anything that can judge a settings value and say what it found.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use crate::describe::Describe;
use crate::report::Report;

/// Judges a `T` and records what it found in a [`Report`].
///
/// A [`Profile`](crate::Profile) is a check, so is a closure `Fn(&T, &mut Report)`, and so is
/// a slice or a `Vec` of checks of one type. [`Checks`] holds checks of different types (a
/// profile composed with custom ones). A custom check reads values through
/// [`Inspect`](crate::Inspect), which a built value, a builder and a
/// [`Partial`](crate::Partial) all implement.
pub trait Check<T: ?Sized> {
    /// Judges `subject`, pushing a [`Finding`](crate::Finding) per expectation.
    fn check(&self, subject: &T, report: &mut Report);

    /// A new report of judging `subject`, hiding what `T`'s description marks sensitive.
    fn verify(&self, subject: &T) -> Report
    where
        T: Describe,
    {
        let mut report = Report::new(&T::description());
        self.check(subject, &mut report);
        report
    }
}

impl<T: ?Sized, F: Fn(&T, &mut Report)> Check<T> for F {
    fn check(&self, subject: &T, report: &mut Report) {
        self(subject, report);
    }
}

impl<T: ?Sized, C: Check<T>> Check<T> for [C] {
    fn check(&self, subject: &T, report: &mut Report) {
        for check in self {
            check.check(subject, report);
        }
    }
}

impl<T: ?Sized, C: Check<T>> Check<T> for Vec<C> {
    fn check(&self, subject: &T, report: &mut Report) {
        self.as_slice().check(subject, report);
    }
}

/// Checks of different types, run one after the other on the same subject.
pub struct Checks<T: ?Sized> {
    checks: Vec<Box<dyn Check<T>>>,
}

impl<T: ?Sized> Checks<T> {
    /// No checks.
    pub fn new() -> Self {
        Checks { checks: Vec::new() }
    }

    /// Adds `check` after the ones already held.
    pub fn push(&mut self, check: impl Check<T> + 'static) {
        self.checks.push(Box::new(check));
    }

    /// How many checks there are.
    pub fn len(&self) -> usize {
        self.checks.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.checks.is_empty()
    }
}

impl<T: ?Sized> Default for Checks<T> {
    fn default() -> Self {
        Checks::new()
    }
}

impl<T: ?Sized> fmt::Debug for Checks<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Checks")
            .field("len", &self.checks.len())
            .finish()
    }
}

impl<T: ?Sized> Check<T> for Checks<T> {
    fn check(&self, subject: &T, report: &mut Report) {
        for check in &self.checks {
            check.check(subject, report);
        }
    }
}
