// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Report`]: what a check found, with every secret hidden.
//!
//! A report is made from a [`Description`], which names the places that hold secrets
//! (`peers[*].key`, `env[*]`, a field of one arm). [`Report::push`] marks every datum at such a
//! place sensitive, so it shows as `Type(***)`, and masks the texts of the expectation at it.
//! Every value a report shows goes through [`Datum`](crate::Datum)'s `Display`: a custom type is shown by its
//! own `Display`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::datum::Shown;
use crate::describe::{Description, PatternSegment};
use crate::path::{KeyPath, Segment};
use crate::profile::Expect;

/// Why an expectation was not met.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reason {
    /// The value is not the expected one, or differs from its default.
    Differs,
    /// A value was expected and the place is not set.
    Missing,
    /// The place is set and was expected not to be.
    Present,
    /// The value is below what is allowed.
    Below,
    /// The value is above what is allowed.
    Above,
    /// The value is none of the allowed ones.
    NotOneOf,
    /// The value is looser than the expectation.
    Looser,
    /// The list holds an item that is not allowed.
    NotSubset,
    /// The variant is an arm that is not allowed.
    ArmNotAllowed,
    /// The profile names a place the type does not have.
    UnknownKey,
    /// The setting has no stricter order, or the values cannot be ordered.
    NoOrdering,
    /// The setting was given and the platform could not apply it.
    NotApplied,
    /// A check's own reason.
    Custom(String),
}

/// One expectation checked, or one place that differs.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    key: KeyPath,
    met: bool,
    value: Shown,
    expected: Expect,
    reason: Option<Reason>,
}

impl Finding {
    /// A finding at `key`. A met finding has no reason.
    pub fn new(
        key: KeyPath,
        met: bool,
        value: Shown,
        expected: Expect,
        reason: Option<Reason>,
    ) -> Finding {
        Finding {
            key,
            met,
            value,
            expected,
            reason,
        }
    }

    /// Where.
    pub fn key(&self) -> &KeyPath {
        &self.key
    }

    /// Whether the expectation holds.
    pub fn met(&self) -> bool {
        self.met
    }

    /// The value found.
    pub fn value(&self) -> &Shown {
        &self.value
    }

    /// What was expected.
    pub fn expected(&self) -> &Expect {
        &self.expected
    }

    /// Why it was not met.
    pub fn reason(&self) -> Option<&Reason> {
        self.reason.as_ref()
    }
}

/// The findings of a check, with secrets hidden.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    findings: Vec<Finding>,
    sensitive: Vec<Vec<PatternSegment>>,
}

impl Report {
    /// An empty report that hides what `description` marks sensitive.
    pub fn new(description: &Description) -> Report {
        Report {
            findings: Vec::new(),
            sensitive: description.sensitive_patterns(),
        }
    }

    /// Adds `finding`, hiding every secret in its value and its expectation.
    pub fn push(&mut self, finding: Finding) {
        let Finding {
            key,
            met,
            value,
            expected,
            reason,
        } = finding;
        let at: Vec<Segment> = key.segments().to_vec();
        let secret = match &value {
            Shown::Items(items) => {
                self.hides(&at)
                    || (0..items.len()).any(|index| {
                        let mut item = at.clone();
                        item.push(Segment::Index(index));
                        self.hides(&item)
                    })
            }
            _ => self.hides(&at),
        };
        let (value, expected) = if secret {
            (value.sensitive(), expected.masked())
        } else {
            (value, expected)
        };
        self.findings.push(Finding {
            key,
            met,
            value,
            expected,
            reason,
        });
    }

    /// Every finding, in the order they were pushed.
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// The findings whose expectation was not met.
    pub fn unmet(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| !f.met)
    }

    /// Whether every expectation holds.
    pub fn is_met(&self) -> bool {
        self.findings.iter().all(|f| f.met)
    }

    /// How many findings there are.
    pub fn len(&self) -> usize {
        self.findings.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    fn hides(&self, at: &[Segment]) -> bool {
        self.sensitive.iter().any(|pattern| {
            pattern.len() <= at.len()
                && pattern.iter().zip(at).all(|(want, got)| match want {
                    PatternSegment::Any => true,
                    PatternSegment::Key(key) => *got.name() == **key,
                })
        })
    }
}
