// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Binding a profile to a type, and checking a value against it.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use super::{Expect, Profile};
use crate::datum::{Datum, Shown};
use crate::describe::{Description, Kind, LeafInfo, Resolved, Stricter};
use crate::inspect::{Entries, Inspect};
use crate::path::{KeyPath, Segment};
use crate::report::{Finding, Reason, Report};

/// The leaf an expectation at `resolved` reads its texts with, and whether the place is a list.
fn leaf_of<'a>(resolved: &Resolved<'a>) -> Option<(&'a LeafInfo, bool)> {
    match resolved.description().kind() {
        Kind::Leaf(info) => Some((info, false)),
        Kind::Union(union) => Some((union.text(), false)),
        Kind::List(item) => match item.kind() {
            Kind::Leaf(info) => Some((info, true)),
            _ => None,
        },
        _ => None,
    }
}

/// The leaf of the items of a list at `resolved`, also where the list is the structured arm
/// of an untagged enum.
fn list_leaf<'a>(resolved: &Resolved<'a>) -> Option<&'a LeafInfo> {
    let description = match resolved.description().kind() {
        Kind::Union(union) => union.structured(),
        _ => resolved.description(),
    };
    match description.kind() {
        Kind::List(item) => match item.kind() {
            Kind::Leaf(info) => Some(info),
            _ => None,
        },
        _ => None,
    }
}

/// Whether two texts of a leaf are the same value; the text that writes `None` equals only itself.
pub(crate) fn same_text(info: &LeafInfo, none: Option<&str>, a: &str, b: &str) -> bool {
    if none == Some(a) || none == Some(b) {
        return a == b;
    }
    info.eq(a, b).unwrap_or(false)
}

/// Why `expect` cannot be held at `resolved`: a text its parser refuses, or an order the leaf
/// lacks.
pub(super) fn validate(expect: &Expect, resolved: &Resolved<'_>) -> Result<(), Reason> {
    let custom = |why: String| Reason::Custom(why);
    let none = resolved.field().and_then(|f| f.none.as_deref());
    let texts = |info: &LeafInfo, texts: &[&str], writes_none: bool| -> Result<(), Reason> {
        for text in texts {
            if writes_none && none == Some(*text) {
                continue;
            }
            info.check(text)
                .map_err(|_| custom(format!("a text that is no {}", info.ty())))?;
        }
        Ok(())
    };
    let ordered = |info: &LeafInfo| -> Result<(), Reason> {
        if info.is_ordered() {
            Ok(())
        } else {
            Err(Reason::NoOrdering)
        }
    };
    let stricter = resolved.description().notes().stricter;
    match expect {
        Expect::Equal(x) => match leaf_of(resolved) {
            Some((info, false)) => texts(info, &[x], true),
            _ => Err(custom("equal applies to a leaf".into())),
        },
        Expect::Within { min, max } => match leaf_of(resolved) {
            Some((info, false)) => {
                ordered(info)?;
                let given: Vec<&str> = min.iter().chain(max).map(String::as_str).collect();
                texts(info, &given, false)
            }
            _ => Err(custom("within applies to a leaf".into())),
        },
        Expect::OneOf(items) => match leaf_of(resolved) {
            Some((info, false)) => texts(
                info,
                &items.iter().map(String::as_str).collect::<Vec<_>>(),
                true,
            ),
            _ => Err(custom("one of applies to a leaf".into())),
        },
        Expect::AsStrictAs(x) | Expect::NoStricterThan(x) => match leaf_of(resolved) {
            Some((info, false)) => {
                ordered(info)?;
                if stricter.is_none() {
                    return Err(Reason::NoOrdering);
                }
                texts(info, &[x], false)
            }
            _ => Err(custom("strictness applies to a leaf".into())),
        },
        Expect::SubsetOf(items) | Expect::ItemsEqual(items) => match list_leaf(resolved) {
            Some(info) => texts(
                info,
                &items.iter().map(String::as_str).collect::<Vec<_>>(),
                false,
            ),
            _ => Err(custom(
                "a list expectation applies to a list of leaves".into(),
            )),
        },
        Expect::Arms(names) => match resolved.description().kind() {
            Kind::Variant(variant) => match names.iter().find(|n| variant.arm(n).is_none()) {
                Some(name) => Err(custom(format!("`{name}` is no arm of the variant"))),
                None => Ok(()),
            },
            _ => Err(custom("arms apply to a variant".into())),
        },
        _ => Ok(()),
    }
}

/// An expectation of a profile that its type cannot hold.
#[derive(Debug, Clone, PartialEq)]
pub struct BindError {
    key: KeyPath,
    reason: Reason,
}

impl BindError {
    /// The key of the expectation.
    pub fn key(&self) -> &KeyPath {
        &self.key
    }

    /// Why it cannot be held.
    pub fn reason(&self) -> &Reason {
        &self.reason
    }
}

impl core::fmt::Display for BindError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{}: the expectation cannot be held ({:?})",
            self.key, self.reason
        )
    }
}

impl core::error::Error for BindError {}

pub(super) fn bind(profile: &Profile, description: &Description) -> Result<(), BindError> {
    for (key, expect) in profile.entries() {
        if let Some(resolved) = description.resolve(key.segments()) {
            validate(expect, &resolved).map_err(|reason| BindError {
                key: key.clone(),
                reason,
            })?;
        }
    }
    Ok(())
}

pub(super) fn run(
    profile: &Profile,
    description: &Description,
    subject: &dyn Inspect,
    report: &mut Report,
) {
    let mut switched: Vec<KeyPath> = Vec::new();
    for (key, expect) in profile.entries() {
        if matches!(expect, Expect::Any) {
            continue;
        }
        let Some(resolved) = description.resolve(key.segments()) else {
            report.push(unmet(key, Shown::Unset, expect, Reason::UnknownKey));
            continue;
        };
        if let Some((union, own_finding)) = switched_union(profile, &resolved, subject) {
            if own_finding && !matches!(expect, Expect::Absent) && !switched.contains(&union) {
                let datum = subject.at(union.segments());
                let reason = if datum.is_some() {
                    Reason::Differs
                } else {
                    Reason::Missing
                };
                report.push(unmet(
                    &union,
                    datum.map_or(Shown::Unset, Shown::One),
                    &Expect::Required,
                    reason,
                ));
                switched.push(union);
            }
            continue;
        }
        if gated_out(profile, description, &resolved, subject) {
            continue;
        }
        if let Err(reason) = validate(expect, &resolved) {
            report.push(unmet(key, Shown::Unset, expect, reason));
            continue;
        }
        evaluate(key, expect, &resolved, subject, report);
    }
}

/// The untagged enum above `resolved` that the subject holds on its text side while the
/// profile expects keys of its structured arm: it is one finding at the enum, not one per key.
/// A text expectation on the enum speaks for those keys instead, and answers nothing here.
fn switched_union(
    profile: &Profile,
    resolved: &Resolved<'_>,
    subject: &dyn Inspect,
) -> Option<(KeyPath, bool)> {
    let gate = resolved.gates().iter().find(|g| g.is_union())?;
    if subject.entries(gate.at().segments()).is_some() {
        return None;
    }
    let text = matches!(
        profile.get(gate.at()),
        Some(Expect::Equal(_) | Expect::OneOf(_))
    );
    Some((gate.at().clone(), !text))
}

/// The subject's value at `key`: its text, or for an untagged enum on its structured arm the
/// keys it holds there, else `unset`.
fn shown_at(resolved: &Resolved<'_>, subject: &dyn Inspect, key: &KeyPath) -> Shown {
    if let Some(datum) = subject.at(key.segments()) {
        return Shown::One(datum);
    }
    match resolved.description().kind() {
        Kind::Union(union) if subject.entries(key.segments()).is_some() => {
            crate::non_default::structured_shown(union, resolved.field(), subject, key.segments())
        }
        _ => Shown::Unset,
    }
}

fn present(subject: &dyn Inspect, at: &[Segment]) -> bool {
    subject.at(at).is_some() || subject.entries(at).is_some()
}

/// Whether an expectation on an optional group or a variant above `resolved` already speaks
/// for it: the group is absent, or the subject is another arm.
fn gated_out(
    profile: &Profile,
    description: &Description,
    resolved: &Resolved<'_>,
    subject: &dyn Inspect,
) -> bool {
    for gate in resolved.gates() {
        if gate.is_union() {
            continue;
        }
        match gate.arms() {
            None => match profile.get(gate.at()) {
                Some(Expect::Required) if subject.entries(gate.at().segments()).is_none() => {
                    return true;
                }
                Some(Expect::Absent) => return true,
                _ => {}
            },
            Some(arms) => {
                let Some(found) = description.resolve(gate.at().segments()) else {
                    continue;
                };
                let Kind::Variant(variant) = found.description().kind() else {
                    continue;
                };
                let tag = gate.at().child(Segment::Key(variant.tag().into()));
                if profile.get(gate.at()).is_none() && profile.get(&tag).is_none() {
                    continue;
                }
                match subject.at(tag.segments()) {
                    Some(arm) if arms.iter().any(|a| a == arm.text()) => {}
                    _ => return true,
                }
            }
        }
    }
    false
}

fn met(key: &KeyPath, value: Shown, expect: &Expect) -> Finding {
    Finding::new(key.clone(), true, value, expect.clone(), None)
}

fn unmet(key: &KeyPath, value: Shown, expect: &Expect, reason: Reason) -> Finding {
    Finding::new(key.clone(), false, value, expect.clone(), Some(reason))
}

fn judged(key: &KeyPath, value: Shown, expect: &Expect, outcome: Result<(), Reason>) -> Finding {
    match outcome {
        Ok(()) => met(key, value, expect),
        Err(reason) => unmet(key, value, expect, reason),
    }
}

fn items_of(subject: &dyn Inspect, key: &KeyPath) -> Option<Vec<Datum>> {
    let Some(Entries::Len(count)) = subject.entries(key.segments()) else {
        return None;
    };
    let mut out = Vec::new();
    for index in 0..count {
        out.extend(subject.at(key.child(Segment::Index(index)).segments()));
    }
    Some(out)
}

fn evaluate(
    key: &KeyPath,
    expect: &Expect,
    resolved: &Resolved<'_>,
    subject: &dyn Inspect,
    report: &mut Report,
) {
    let description = resolved.description();
    if let Expect::Arms(names) = expect {
        let Kind::Variant(variant) = description.kind() else {
            report.push(unmet(
                key,
                Shown::Unset,
                expect,
                Reason::Custom(String::from("arms apply to a variant")),
            ));
            return;
        };
        let tag = key.child(Segment::Key(variant.tag().into()));
        let arm = subject.at(tag.segments());
        let outcome = match &arm {
            Some(d) if names.iter().any(|n| n == d.text()) => Ok(()),
            Some(_) => Err(Reason::ArmNotAllowed),
            None => Err(Reason::Missing),
        };
        let shown = arm.map_or(Shown::Unset, Shown::One);
        report.push(judged(key, shown, expect, outcome));
        return;
    }
    if matches!(expect, Expect::Required | Expect::Absent) {
        let here = present(subject, key.segments());
        let shown = shown_at(resolved, subject, key);
        let outcome = match (expect, here) {
            (Expect::Required, true) | (Expect::Absent, false) => Ok(()),
            (Expect::Required, false) => Err(Reason::Missing),
            _ => Err(Reason::Present),
        };
        report.push(judged(key, shown, expect, outcome));
        return;
    }
    let list_expect = matches!(expect, Expect::ItemsEqual(_) | Expect::SubsetOf(_));
    let found = if list_expect {
        list_leaf(resolved).map(|info| (info, true))
    } else {
        leaf_of(resolved)
    };
    let Some((info, is_list)) = found else {
        return;
    };
    if is_list {
        list(key, expect, info, subject, report);
        return;
    }
    let datum = subject.at(key.segments());
    // A subject on the structured arm of an untagged enum holds a value, just not a text.
    let structured = datum.is_none()
        && matches!(description.kind(), Kind::Union(_))
        && subject.entries(key.segments()).is_some();
    let value = datum.as_ref().map(|d| d.text().to_owned());
    let shown = shown_at(resolved, subject, key);
    let none = resolved.field().and_then(|f| f.none.as_deref());
    // `None` for a setting that is not set, and for an optional holding its `none` text.
    let loosest = value.as_deref().is_none_or(|v| none == Some(v));
    let stricter = description.notes().stricter;
    let eq = |a: &str, b: &str| same_text(info, none, a, b);
    let order = |a: &str, b: &str| info.order(a, b).ok().flatten();

    let outcome: Result<(), Reason> = match expect {
        Expect::Equal(x) => match &value {
            Some(v) if eq(v, x) => Ok(()),
            Some(_) => Err(Reason::Differs),
            None if structured => Err(Reason::Differs),
            None => Err(Reason::Missing),
        },
        Expect::OneOf(allowed) => match &value {
            Some(v) if allowed.iter().any(|a| eq(v, a)) => Ok(()),
            Some(_) => Err(Reason::NotOneOf),
            None if structured => Err(Reason::NotOneOf),
            None => Err(Reason::Missing),
        },
        Expect::Within { min, max } => match &value {
            None => Err(Reason::Missing),
            Some(v) => {
                let below = min.as_ref().map(|m| order(v, m));
                let above = max.as_ref().map(|m| order(v, m));
                match (below, above) {
                    (Some(None), _) | (_, Some(None)) => Err(Reason::NoOrdering),
                    (Some(Some(Ordering::Less)), _) => Err(Reason::Below),
                    (_, Some(Some(Ordering::Greater))) => Err(Reason::Above),
                    _ => Ok(()),
                }
            }
        },
        Expect::AsStrictAs(bound) => match (stricter, &value) {
            (None, _) => Err(Reason::NoOrdering),
            (Some(_), _) if loosest => Err(Reason::Looser),
            (Some(way), Some(v)) => match order(v, bound) {
                None => Err(Reason::NoOrdering),
                Some(o) => {
                    let holds = match way {
                        Stricter::Higher => o != Ordering::Less,
                        Stricter::Lower => o != Ordering::Greater,
                    };
                    if holds { Ok(()) } else { Err(Reason::Looser) }
                }
            },
            (Some(_), None) => Err(Reason::Looser),
        },
        Expect::NoStricterThan(bound) => match (stricter, &value) {
            (None, _) => Err(Reason::NoOrdering),
            (Some(_), _) if loosest => Ok(()),
            (Some(way), Some(v)) => match order(v, bound) {
                None => Err(Reason::NoOrdering),
                Some(o) => match (way, o) {
                    (Stricter::Higher, Ordering::Greater) => Err(Reason::Above),
                    (Stricter::Lower, Ordering::Less) => Err(Reason::Below),
                    _ => Ok(()),
                },
            },
            (Some(_), None) => Ok(()),
        },
        _ => return,
    };
    report.push(judged(key, shown, expect, outcome));
}

/// A list of leaves, compared item by item through the item's parser.
fn list(
    key: &KeyPath,
    expect: &Expect,
    info: &LeafInfo,
    subject: &dyn Inspect,
    report: &mut Report,
) {
    let items = items_of(subject, key);
    let shown = items.clone().map_or(Shown::Unset, Shown::Items);
    let texts: Vec<&str> = items.iter().flatten().map(Datum::text).collect();
    let eq = |a: &str, b: &str| info.eq(a, b).unwrap_or(false);
    let outcome = match expect {
        Expect::ItemsEqual(want) => match &items {
            None => Err(Reason::Missing),
            Some(_) => {
                if texts.len() == want.len() && texts.iter().zip(want).all(|(a, b)| eq(a, b)) {
                    Ok(())
                } else {
                    Err(Reason::Differs)
                }
            }
        },
        Expect::SubsetOf(allowed) => {
            if texts.iter().all(|t| allowed.iter().any(|a| eq(t, a))) {
                Ok(())
            } else {
                Err(Reason::NotSubset)
            }
        }
        _ => return,
    };
    report.push(judged(key, shown, expect, outcome));
}
