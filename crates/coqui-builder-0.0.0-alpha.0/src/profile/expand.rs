// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A profile made from a description and a value.
//!
//! A sensitive place is expected to be set, never to equal a text the profile would hold.

use alloc::string::String;
use alloc::vec::Vec;

use super::{Expect, Profile};
use crate::datum::Datum;
use crate::describe::{Description, Field, Kind};
use crate::inspect::{Entries, Inspect};
use crate::path::{KeyPath, Segment};

/// A description's defaults, answered as a value.
pub(super) struct Defaults<'a>(pub(super) &'a Description);

impl Inspect for Defaults<'_> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        if let Some((Segment::Index(index), parent)) = at.split_last() {
            let list = self.0.resolve(parent)?;
            let Kind::List(item) = structured(list.description()).kind() else {
                return None;
            };
            let Kind::Leaf(info) = item.kind() else {
                return None;
            };
            let text = list.default_items()?.get(*index)?;
            return Some(Datum::new(info.ty(), text.as_str()));
        }
        let resolved = self.0.resolve(at)?;
        let info = match resolved.description().kind() {
            Kind::Leaf(info) => info,
            Kind::Union(union) => union.text(),
            _ => return None,
        };
        resolved.default().map(|text| Datum::new(info.ty(), text))
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        if at.is_empty() {
            return Some(Entries::Keys(Vec::new()));
        }
        let resolved = self.0.resolve(at)?;
        match resolved.description().kind() {
            Kind::List(_) => resolved.default_items().map(|i| Entries::Len(i.len())),
            Kind::Map(_) => Some(Entries::Keys(Vec::new())),
            Kind::Group(_) | Kind::Variant(_) => {
                let optional = resolved.field().is_some_and(|f| f.optional);
                (!optional).then(|| Entries::Keys(Vec::new()))
            }
            Kind::Union(union) if union.default_is_structured() && resolved.default().is_none() => {
                match resolved.default_items() {
                    Some(items) => Some(Entries::Len(items.len())),
                    None => Some(Entries::Keys(Vec::new())),
                }
            }
            Kind::Union(_) => None,
            _ => None,
        }
    }
}

/// The description of an untagged enum's structured arm; any other is itself.
fn structured(description: &Description) -> &Description {
    match description.kind() {
        Kind::Union(union) => union.structured(),
        _ => description,
    }
}

pub(super) fn expand(description: &Description, source: &dyn Inspect) -> Profile {
    let mut out = Profile::new();
    place(description, false, false, source, &mut Vec::new(), &mut out);
    out
}

fn fields(
    fields: &[Field],
    secret: bool,
    source: &dyn Inspect,
    path: &mut Vec<Segment>,
    out: &mut Profile,
) {
    for field in fields {
        path.push(Segment::Key(field.key.clone()));
        place(
            &field.description,
            field.optional,
            secret,
            source,
            path,
            out,
        );
        path.pop();
    }
}

fn put(out: &mut Profile, path: &[Segment], expect: Expect) {
    out.insert(KeyPath::new(path.to_vec()), expect);
}

/// `inherited` is whether a group, map or list above is sensitive: everything below it is too.
fn place(
    description: &Description,
    optional: bool,
    inherited: bool,
    source: &dyn Inspect,
    path: &mut Vec<Segment>,
    out: &mut Profile,
) {
    let secret = inherited || description.notes().sensitive;
    match description.kind() {
        Kind::Group(group) => {
            if optional {
                if source.entries(path).is_none() {
                    put(out, path, Expect::Absent);
                    return;
                }
                put(out, path, Expect::Required);
            }
            fields(&group.fields, secret, source, path, out);
        }
        Kind::Variant(variant) => {
            if optional {
                if source.entries(path).is_none() {
                    put(out, path, Expect::Absent);
                    return;
                }
                put(out, path, Expect::Required);
            }
            let mut tag = path.clone();
            tag.push(Segment::Key(variant.tag().into()));
            match source.at(&tag) {
                Some(arm) => {
                    put(out, &tag, Expect::Equal(String::from(arm.text())));
                    if let Some(found) = variant.arm(arm.text()) {
                        fields(&found.fields, secret, source, path, out);
                    }
                }
                None => put(out, &tag, Expect::Absent),
            }
        }
        Kind::Map(value) => {
            if let Some(Entries::Keys(keys)) = source.entries(path) {
                for key in keys {
                    path.push(Segment::Key(key));
                    place(value, true, secret, source, path, out);
                    path.pop();
                }
            }
        }
        Kind::List(item) => {
            let count = match source.entries(path) {
                Some(Entries::Len(count)) => Some(count),
                _ => None,
            };
            if matches!(item.kind(), Kind::Leaf(_)) {
                let holds_secret = secret || !item.sensitive_patterns().is_empty();
                let expect = match count {
                    None => Expect::Absent,
                    Some(_) if holds_secret => Expect::Required,
                    Some(count) => {
                        let mut texts = Vec::new();
                        for index in 0..count {
                            path.push(Segment::Index(index));
                            texts.extend(source.at(path).map(|d| String::from(d.text())));
                            path.pop();
                        }
                        Expect::ItemsEqual(texts)
                    }
                };
                put(out, path, expect);
            } else {
                for index in 0..count.unwrap_or(0) {
                    path.push(Segment::Index(index));
                    place(item, false, secret, source, path, out);
                    path.pop();
                }
            }
        }
        Kind::Leaf(_) => {
            let expect = match source.at(path) {
                Some(_) if secret => Expect::Required,
                Some(datum) => Expect::Equal(String::from(datum.text())),
                None => Expect::Absent,
            };
            put(out, path, expect);
        }
        Kind::Union(union) => match source.at(path) {
            Some(_) if secret => put(out, path, Expect::Required),
            Some(datum) => put(out, path, Expect::Equal(String::from(datum.text()))),
            None if source.entries(path).is_some() => {
                place(union.structured(), false, secret, source, path, out);
            }
            None => put(out, path, Expect::Absent),
        },
    }
}
