// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where a value departs from its defaults, reported as data.
//!
//! [`differences`] walks a description with a value (and, optionally, a second one to compare
//! against) and lists the places that differ. [`non_defaults`] is that against the defaults.
//!
//! # What is one difference
//!
//! - A group's fields, an open map's entries and a variant's fields (when both sides are the
//!   same arm) are walked.
//! - A leaf is compared as typed values (`30s` equals `30000ms`), never as text.
//! - An untagged enum is compared as its text arm when both sides are text, as its structured
//!   arm when both are structured, and is one place, at the enum, when the sides differ: the
//!   text shows as `Type(text)` and the structured arm as `{key: Type(text), ..}`.
//! - A list of leaves is one place, compared item by item and shown whole.
//! - A switched arm is one place, at its tag; a group present on one side only is one place,
//!   at the group, with no value.
//! - A key left out reads as its default. With no default it stays absent.
//! - A secret is compared as it is and shown as `Type(***)`.
//!
//! Findings come in the order of their path segments, compared as text.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::Describe;
use crate::datum::{Datum, Shown};
use crate::describe::{Description, Field, Kind, LeafInfo, Union};
use crate::inspect::{Entries, Inspect};
use crate::path::{KeyPath, Segment};
use crate::profile::Expect;
use crate::report::{Finding, Reason, Report};

/// A setting whose value differs from its default, as plain data for the caller to show or log.
///
/// A non-default setting is never refused for it; it is listed, and the application that knows
/// the use case decides what it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonDefault {
    /// The setting's key, as a path (`max_message`, `waiting.per_source`).
    pub setting: String,
    /// Its value, shown as a [`Shown`] (`u16(16)`, `unset`, `String(***)`).
    pub value: Shown,
    /// The default, shown the same way.
    pub default: Shown,
}

impl NonDefault {
    /// A non-default `setting`, now `value`, against `default`.
    pub fn new(setting: impl Into<String>, value: Shown, default: Shown) -> Self {
        NonDefault {
            setting: setting.into(),
            value,
            default,
        }
    }
}

impl From<&Finding> for NonDefault {
    /// The finding as data. A default that was expected is shown with the type name of the
    /// value found (`String` where there is none): [`non_defaults`] knows the exact type.
    fn from(finding: &Finding) -> NonDefault {
        let ty = match finding.value() {
            Shown::One(d) => d.ty(),
            Shown::Items(items) => items.first().map_or("String", Datum::ty),
            Shown::Fields(_) | Shown::Unset => "String",
        };
        let default = match finding.expected() {
            Expect::Equal(text) => Shown::One(Datum::new(ty, text.as_str())),
            Expect::ItemsEqual(items) => {
                Shown::Items(items.iter().map(|t| Datum::new(ty, t.as_str())).collect())
            }
            _ => Shown::Unset,
        };
        NonDefault {
            setting: alloc::string::ToString::to_string(finding.key()),
            value: finding.value().clone(),
            default,
        }
    }
}

impl fmt::Display for NonDefault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} = {} (default {})",
            self.setting, self.value, self.default
        )
    }
}

/// Every setting of `value` that differs from its default.
pub fn non_defaults<T: Describe + Inspect>(value: &T) -> Vec<NonDefault> {
    listed(&T::description(), value)
}

/// Every setting of `value` that differs from the defaults `description` states.
pub(crate) fn listed(description: &Description, value: &dyn Inspect) -> Vec<NonDefault> {
    let (report, defaults) = walk(description, value, None);
    report
        .findings()
        .iter()
        .zip(defaults)
        .map(|(finding, default)| NonDefault {
            setting: alloc::string::ToString::to_string(finding.key()),
            value: finding.value().clone(),
            default: default.masked_like(finding.value()),
        })
        .collect()
}

/// Where `value` differs from `baseline`, or from the defaults of `description` when
/// `baseline` is `None`.
///
/// Each finding is unmet with reason [`Reason::Differs`], and expects the baseline's value
/// ([`Expect::Equal`], [`Expect::ItemsEqual`]) or, when the baseline has none,
/// [`Expect::Absent`].
pub fn differences(
    description: &Description,
    value: &dyn Inspect,
    baseline: Option<&dyn Inspect>,
) -> Report {
    walk(description, value, baseline).0
}

trait MaskedLike {
    fn masked_like(self, value: &Shown) -> Shown;
}

impl MaskedLike for Shown {
    /// A secret's default is hidden as its value is.
    fn masked_like(self, value: &Shown) -> Shown {
        let hidden = match value {
            Shown::One(d) => d.is_sensitive(),
            Shown::Items(items) => items.iter().any(Datum::is_sensitive),
            Shown::Fields(fields) => fields.iter().any(|(_, d)| d.is_sensitive()),
            Shown::Unset => false,
        };
        if hidden { self.sensitive() } else { self }
    }
}

fn walk(
    description: &Description,
    value: &dyn Inspect,
    baseline: Option<&dyn Inspect>,
) -> (Report, Vec<Shown>) {
    let mut walk = Walk {
        value,
        baseline,
        found: Vec::new(),
    };
    walk.place(description, None, false, &mut Vec::new());
    let mut found = walk.found;
    found.sort_by(|a, b| {
        let names = |f: &Finding| -> Vec<String> {
            f.key()
                .segments()
                .iter()
                .map(|s| s.name().into_owned())
                .collect()
        };
        names(&a.0).cmp(&names(&b.0))
    });
    let mut report = Report::new(description);
    let mut defaults = Vec::new();
    for (finding, default) in found {
        report.push(finding);
        defaults.push(default);
    }
    (report, defaults)
}

struct Walk<'a> {
    value: &'a dyn Inspect,
    baseline: Option<&'a dyn Inspect>,
    found: Vec<(Finding, Shown)>,
}

fn text_of(datum: Option<Datum>) -> Option<String> {
    datum.map(|d| String::from(d.text()))
}

impl Walk<'_> {
    fn fields(&mut self, fields: &[Field], path: &mut Vec<Segment>) {
        for field in fields {
            path.push(Segment::Key(field.key.clone()));
            self.place(&field.description, Some(field), field.optional, path);
            path.pop();
        }
    }

    fn texts(&self, side: &dyn Inspect, path: &[Segment], count: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = path.to_vec();
        for index in 0..count {
            at.push(Segment::Index(index));
            out.extend(text_of(side.at(&at)));
            at.pop();
        }
        out
    }

    fn present(&self, side: Option<&dyn Inspect>, path: &[Segment]) -> bool {
        side.is_some_and(|s| s.entries(path).is_some())
    }

    fn place(
        &mut self,
        description: &Description,
        field: Option<&Field>,
        optional: bool,
        path: &mut Vec<Segment>,
    ) {
        match description.kind() {
            Kind::Leaf(info) => {
                let default = field.and_then(|f| f.default.clone());
                let v = text_of(self.value.at(path)).or_else(|| default.clone());
                let b = match self.baseline {
                    Some(baseline) => text_of(baseline.at(path)),
                    None => None,
                }
                .or(default);
                let none = field.and_then(|f| f.none.as_deref());
                self.leaf(info, none, path, v, b);
            }
            Kind::List(item) => {
                let defaults = field.and_then(|f| f.default_items.clone());
                let counted = |side: &dyn Inspect| match side.entries(path) {
                    Some(Entries::Len(n)) => Some(n),
                    _ => None,
                };
                if let Kind::Leaf(info) = item.kind() {
                    let v = counted(self.value)
                        .map(|n| self.texts(self.value, path, n))
                        .or_else(|| defaults.clone());
                    let b = match self.baseline {
                        Some(baseline) => counted(baseline).map(|n| self.texts(baseline, path, n)),
                        None => None,
                    }
                    .or(defaults);
                    self.items(info, path, v, b);
                } else {
                    let n = counted(self.value)
                        .into_iter()
                        .chain(self.baseline.and_then(counted))
                        .max()
                        .unwrap_or(0);
                    for index in 0..n {
                        path.push(Segment::Index(index));
                        self.place(item, None, true, path);
                        path.pop();
                    }
                }
            }
            Kind::Map(entry) => {
                let keys = |side: &dyn Inspect| match side.entries(path) {
                    Some(Entries::Keys(keys)) => keys,
                    _ => Vec::new(),
                };
                let mut all = keys(self.value);
                if let Some(baseline) = self.baseline {
                    for key in keys(baseline) {
                        if !all.contains(&key) {
                            all.push(key);
                        }
                    }
                }
                for key in all {
                    path.push(Segment::Key(key));
                    self.place(entry, None, true, path);
                    path.pop();
                }
            }
            Kind::Group(group) => {
                if optional {
                    let (pv, pb) = (
                        self.present(Some(self.value), path),
                        self.present(self.baseline, path),
                    );
                    if pv != pb {
                        self.found.push(self.group_finding(path, pv));
                        return;
                    }
                    if !pv {
                        return;
                    }
                }
                self.fields(&group.fields, path);
            }
            Kind::Variant(variant) => {
                if optional {
                    let (pv, pb) = (
                        self.present(Some(self.value), path),
                        self.present(self.baseline, path),
                    );
                    if pv != pb {
                        self.found.push(self.group_finding(path, pv));
                        return;
                    }
                    if !pv {
                        return;
                    }
                }
                let mut tag = path.clone();
                tag.push(Segment::Key(variant.tag().into()));
                let arm_default = variant.default_arm().map(String::from);
                let v = text_of(self.value.at(&tag)).or_else(|| arm_default.clone());
                let b = match self.baseline {
                    Some(baseline) => text_of(baseline.at(&tag)),
                    None => None,
                }
                .or(arm_default);
                match (&v, &b) {
                    (Some(a), Some(c)) if a == c => {
                        if let Some(arm) = variant.arm(a) {
                            self.fields(&arm.fields, path);
                        }
                    }
                    (None, None) => {}
                    _ => {
                        if let Kind::Leaf(info) = variant.tag_description().kind() {
                            self.leaf(info, None, &tag, v, b);
                        }
                    }
                }
            }
            Kind::Union(union) => self.union(union, field, path),
        }
    }

    /// Which side of an untagged enum `side` holds at `path`.
    fn side(side: &dyn Inspect, path: &[Segment]) -> Side {
        if let Some(text) = text_of(side.at(path)) {
            Side::Text(text)
        } else if side.entries(path).is_some() {
            Side::Structured
        } else {
            Side::Absent
        }
    }

    fn union(&mut self, union: &Union, field: Option<&Field>, path: &mut Vec<Segment>) {
        let default = match field.and_then(|f| f.default.clone()) {
            Some(text) => Side::Text(text),
            None if union.default_is_structured() => Side::Structured,
            None => Side::Absent,
        };
        let v = match Self::side(self.value, path) {
            Side::Absent => default.clone(),
            side => side,
        };
        let baseline = self
            .baseline
            .map(|baseline| (baseline, Self::side(baseline, path)))
            .filter(|(_, side)| !matches!(side, Side::Absent));
        let b = match &baseline {
            Some((_, side)) => side.clone(),
            None => default,
        };
        match (&v, &b) {
            (Side::Absent, Side::Absent) => {}
            (Side::Text(a), Side::Text(c)) => {
                self.leaf(union.text(), None, path, Some(a.clone()), Some(c.clone()));
            }
            (Side::Structured, Side::Structured) => {
                self.place(union.structured(), field, false, path);
            }
            _ => {
                let text = |t: &String| Shown::One(Datum::new(union.text().ty(), t.as_str()));
                let shown = |side: &Side, from: Option<&dyn Inspect>| match side {
                    Side::Absent => Shown::Unset,
                    Side::Text(t) => text(t),
                    Side::Structured => {
                        let mut out = Vec::new();
                        let mut at = path.clone();
                        render(
                            union.structured(),
                            field,
                            from,
                            &mut at,
                            &mut Vec::new(),
                            false,
                            &mut out,
                        );
                        Shown::Fields(out)
                    }
                };
                let expected = match &b {
                    Side::Absent => Expect::Absent,
                    Side::Text(t) => Expect::Equal(t.clone()),
                    Side::Structured => Expect::Required,
                };
                let value = shown(&v, Some(self.value));
                let default = shown(&b, baseline.as_ref().map(|(side, _)| *side));
                self.found.push((
                    Finding::new(
                        KeyPath::new(path.to_vec()),
                        false,
                        value,
                        expected,
                        Some(Reason::Differs),
                    ),
                    default,
                ));
            }
        }
    }

    fn group_finding(&self, path: &[Segment], value_has_it: bool) -> (Finding, Shown) {
        let expected = if value_has_it {
            Expect::Absent
        } else {
            Expect::Required
        };
        (
            Finding::new(
                KeyPath::new(path.to_vec()),
                false,
                Shown::Unset,
                expected,
                Some(Reason::Differs),
            ),
            Shown::Unset,
        )
    }

    fn leaf(
        &mut self,
        info: &LeafInfo,
        none: Option<&str>,
        path: &[Segment],
        v: Option<String>,
        b: Option<String>,
    ) {
        let same = match (&v, &b) {
            (None, None) => true,
            (Some(a), Some(c)) => crate::profile::compare::same_text(info, none, a, c),
            _ => false,
        };
        if same {
            return;
        }
        let shown = |t: &Option<String>| match t {
            Some(t) => Shown::One(Datum::new(info.ty(), t.as_str())),
            None => Shown::Unset,
        };
        let expected = match &b {
            Some(t) => Expect::Equal(t.clone()),
            None => Expect::Absent,
        };
        self.found.push((
            Finding::new(
                KeyPath::new(path.to_vec()),
                false,
                shown(&v),
                expected,
                Some(Reason::Differs),
            ),
            shown(&b),
        ));
    }

    fn items(
        &mut self,
        info: &LeafInfo,
        path: &[Segment],
        v: Option<Vec<String>>,
        b: Option<Vec<String>>,
    ) {
        let same = match (&v, &b) {
            (None, None) => true,
            (Some(a), Some(c)) => {
                a.len() == c.len() && a.iter().zip(c).all(|(x, y)| info.eq(x, y).unwrap_or(false))
            }
            _ => false,
        };
        if same {
            return;
        }
        let shown = |t: &Option<Vec<String>>| match t {
            Some(items) => Shown::Items(
                items
                    .iter()
                    .map(|i| Datum::new(info.ty(), i.as_str()))
                    .collect(),
            ),
            None => Shown::Unset,
        };
        let expected = match &b {
            Some(items) => Expect::ItemsEqual(items.clone()),
            None => Expect::Absent,
        };
        self.found.push((
            Finding::new(
                KeyPath::new(path.to_vec()),
                false,
                shown(&v),
                expected,
                Some(Reason::Differs),
            ),
            shown(&b),
        ));
    }
}

/// Which arm of an untagged enum a side holds.
#[derive(Clone)]
enum Side {
    Absent,
    Text(String),
    Structured,
}

/// What `side` holds on the structured arm of the untagged enum at `path`, shown as
/// `Shown::Fields`: its keys below the enum, else the defaults the description states.
pub(crate) fn structured_shown(
    union: &Union,
    field: Option<&Field>,
    side: &dyn Inspect,
    path: &[Segment],
) -> Shown {
    let mut out = Vec::new();
    render(
        union.structured(),
        field,
        Some(side),
        &mut path.to_vec(),
        &mut Vec::new(),
        false,
        &mut out,
    );
    Shown::Fields(out)
}

/// The leaves of a structured arm as `(key below the enum, datum)`: what `side` holds there,
/// else the defaults the description states. A secret is marked as one.
fn render(
    description: &Description,
    field: Option<&Field>,
    side: Option<&dyn Inspect>,
    path: &mut Vec<Segment>,
    rel: &mut Vec<Segment>,
    inherited: bool,
    out: &mut Vec<(String, Datum)>,
) {
    let secret = inherited || description.notes().sensitive;
    let name = |rel: &[Segment]| alloc::string::ToString::to_string(&KeyPath::new(rel.to_vec()));
    let children = |fields: &[Field],
                    path: &mut Vec<Segment>,
                    rel: &mut Vec<Segment>,
                    out: &mut Vec<(String, Datum)>| {
        for child in fields {
            path.push(Segment::Key(child.key.clone()));
            rel.push(Segment::Key(child.key.clone()));
            render(
                &child.description,
                Some(child),
                side,
                path,
                rel,
                secret,
                out,
            );
            rel.pop();
            path.pop();
        }
    };
    match description.kind() {
        Kind::Group(group) => children(&group.fields, path, rel, out),
        Kind::Variant(variant) => {
            let mut tag = path.clone();
            tag.push(Segment::Key(variant.tag().into()));
            let chosen = side
                .and_then(|s| text_of(s.at(&tag)))
                .or_else(|| variant.default_arm().map(String::from));
            if let Some(chosen) = chosen {
                let mut at = rel.clone();
                at.push(Segment::Key(variant.tag().into()));
                out.push((name(&at), Datum::new("String", chosen.as_str())));
                if let Some(arm) = variant.arm(&chosen) {
                    children(&arm.fields, path, rel, out);
                }
            }
        }
        Kind::Leaf(info) => {
            let text = side
                .and_then(|s| text_of(s.at(path)))
                .or_else(|| field.and_then(|f| f.default.clone()));
            if let Some(text) = text {
                let datum = Datum::new(info.ty(), text.as_str());
                out.push((name(rel), if secret { datum.sensitive() } else { datum }));
            }
        }
        Kind::List(item) => {
            let count = match side.and_then(|s| s.entries(path)) {
                Some(Entries::Len(count)) => count,
                _ => field
                    .and_then(|f| f.default_items.as_ref())
                    .map_or(0, Vec::len),
            };
            for index in 0..count {
                path.push(Segment::Index(index));
                rel.push(Segment::Index(index));
                let mut entry = Field::new("", (**item).clone());
                entry.default = field
                    .and_then(|f| f.default_items.as_ref())
                    .and_then(|items| items.get(index))
                    .cloned();
                render(item, Some(&entry), side, path, rel, secret, out);
                rel.pop();
                path.pop();
            }
        }
        Kind::Map(value) => {
            if let Some(Entries::Keys(keys)) = side.and_then(|s| s.entries(path)) {
                for key in keys {
                    path.push(Segment::Key(key.clone()));
                    rel.push(Segment::Key(key));
                    render(value, None, side, path, rel, secret, out);
                    rel.pop();
                    path.pop();
                }
            }
        }
        Kind::Union(union) => match side.and_then(|s| text_of(s.at(path))) {
            Some(text) => {
                let datum = Datum::new(union.text().ty(), text.as_str());
                out.push((name(rel), if secret { datum.sensitive() } else { datum }));
            }
            None => render(union.structured(), field, side, path, rel, secret, out),
        },
    }
}
