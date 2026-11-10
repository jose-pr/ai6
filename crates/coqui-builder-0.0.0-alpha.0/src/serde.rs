// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `Deserialize` for [`Partial`]: any serde format reads straight into a partly filled
//! settings type.
//!
//! The input is collected into a private tree of texts, lists and tables, and a
//! [`Source`] over that tree runs [`Partial::fill_from`], so lists, untagged enums, a tag
//! written after the keys of its arm, `none` texts and unknown keys behave as they do for
//! `Env` and any other source. Keys are matched exactly.
//!
//! - **Texts**: a string as it is, a boolean as `true` or `false`, an integer as its decimal
//!   digits, a float as the shortest text that reads back. A format that hands a wider
//!   integer than 64 bits as a float loses digits before this code sees it: write such a
//!   number as a string. A unit or `None` is a key that is not given.
//! - **Unknown keys** are skipped, or refused where the group has `deny_unknown_fields`.
//! - **Shapes** are checked first: a group, a variant or an open map takes a table, a list
//!   takes a list (or a table of positions), an untagged enum takes a value or a table.
//!   Anything else is `expected a table at waiting`.
//! - **Errors** this code writes are the [`FillError`](crate::FillError) text, or a sentence
//!   naming the key path and what was expected. They never quote the input, so a rejected
//!   secret is not echoed. A format's own syntax errors are its own and may show the line.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use ::serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

use crate::describe::{Describe, Description, Kind, LeafInfo};
use crate::inspect::Entries;
use crate::partial::Partial;
use crate::path::{KeyPath, Segment};
use crate::source::{Source, SourceError};

/// A text read from the input; with `zeroize` it is wiped wherever it drops, so a failed
/// read leaves nothing behind either.
#[derive(Default)]
struct Text(String);

#[cfg(feature = "zeroize")]
impl Drop for Text {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

/// What the input held.
enum Node {
    /// A unit or `None`: the key is not given.
    Absent,
    Text(Text),
    List(Vec<Node>),
    Table(Vec<(String, Node)>),
}

struct NodeVisitor;

impl NodeVisitor {
    fn text(text: impl ToString) -> Node {
        Node::Text(Text(text.to_string()))
    }
}

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a value, a list or a table")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Node, E> {
        Ok(NodeVisitor::text(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Node, E> {
        Ok(NodeVisitor::text(v))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Node, E> {
        Ok(NodeVisitor::text(v))
    }
    fn visit_i128<E: de::Error>(self, v: i128) -> Result<Node, E> {
        Ok(NodeVisitor::text(v))
    }
    fn visit_u128<E: de::Error>(self, v: u128) -> Result<Node, E> {
        Ok(NodeVisitor::text(v))
    }
    fn visit_f32<E: de::Error>(self, v: f32) -> Result<Node, E> {
        Ok(NodeVisitor::text(v))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Node, E> {
        Ok(NodeVisitor::text(v))
    }
    fn visit_char<E: de::Error>(self, v: char) -> Result<Node, E> {
        Ok(NodeVisitor::text(v))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Node, E> {
        Ok(Node::Text(Text(v.into())))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Node, E> {
        Ok(Node::Text(Text(v)))
    }
    fn visit_bytes<E: de::Error>(self, _v: &[u8]) -> Result<Node, E> {
        Err(E::custom("bytes are not a setting"))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
        Ok(Node::Absent)
    }
    fn visit_none<E: de::Error>(self) -> Result<Node, E> {
        Ok(Node::Absent)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Node, D::Error> {
        d.deserialize_any(self)
    }
    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Node, D::Error> {
        d.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<Node>()? {
            items.push(item);
        }
        Ok(Node::List(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut entries: Vec<(String, Node)> = Vec::new();
        while let Some(key) = map.next_key::<Node>()? {
            let Node::Text(mut key) = key else {
                return Err(de::Error::custom("a key must be text"));
            };
            let key = core::mem::take(&mut key.0);
            let value = map.next_value::<Node>()?;
            // A repeated key keeps its first place and the last value.
            match entries.iter_mut().find(|(k, _)| *k == key) {
                Some(entry) => entry.1 = value,
                None => entries.push((key, value)),
            }
        }
        Ok(Node::Table(entries))
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Node, D::Error> {
        d.deserialize_any(NodeVisitor)
    }
}

/// The input as a [`Source`].
struct Tree {
    root: Node,
}

impl Tree {
    fn find(&self, at: &[Segment]) -> Option<&Node> {
        let mut node = &self.root;
        for segment in at {
            node = match node {
                Node::Table(entries) => {
                    let name = segment.name();
                    &entries.iter().find(|(key, _)| *key == *name)?.1
                }
                Node::List(items) => items.get(segment.as_index()?)?,
                _ => return None,
            };
        }
        Some(node)
    }
}

/// The paths below `node` that `known` does not recognise, the first unknown one of each
/// branch. A table key of digits is tried as a position as well: it can stand for a list.
fn walk(node: &Node, path: &mut KeyPath, known: &dyn Fn(&KeyPath) -> bool, out: &mut Vec<KeyPath>) {
    let children: Vec<(Vec<Segment>, &Node)> = match node {
        Node::Table(entries) => entries
            .iter()
            .map(|(key, child)| {
                let mut tries = alloc::vec![Segment::Key(key.clone())];
                if let Ok(at) = key.parse::<usize>()
                    && at.to_string() == *key
                {
                    tries.push(Segment::Index(at));
                }
                (tries, child)
            })
            .collect(),
        Node::List(items) => items
            .iter()
            .enumerate()
            .map(|(at, child)| (alloc::vec![Segment::Index(at)], child))
            .collect(),
        _ => return,
    };
    for (tries, child) in children {
        let mut matched = false;
        for segment in &tries {
            path.push(segment.clone());
            if known(path) {
                matched = true;
                walk(child, path, known, out);
            }
            path.pop();
            if matched {
                break;
            }
        }
        if !matched && !matches!(child, Node::Absent) {
            out.push(path.child(tries[0].clone()));
        }
    }
}

/// Refuses a value of the wrong shape for its place: a table where the description has a
/// group, a variant or an open map, a list (or a table of positions) where it has a list, a
/// value or a table where it has an untagged enum. A leaf holding structure is refused when
/// it is read.
fn shape(node: &Node, description: &Description, path: &mut KeyPath) -> Result<(), SourceError> {
    let wrong = |what: &str, path: &KeyPath| {
        let place = if path.is_empty() {
            String::from("the root")
        } else {
            path.to_string()
        };
        SourceError::new(format!("expected {what} at {place}"))
    };
    let table = |path: &KeyPath| -> Result<&[(String, Node)], SourceError> {
        match node {
            Node::Table(entries) => Ok(entries),
            Node::Absent => Ok(&[]),
            _ => Err(wrong("a table", path)),
        }
    };
    match description.kind() {
        Kind::Group(group) => {
            let entries = table(path)?;
            for field in &group.fields {
                if let Some((_, child)) = entries.iter().find(|(key, _)| **key == *field.key) {
                    path.push(Segment::Key(field.key.clone()));
                    shape(child, &field.description, path)?;
                    path.pop();
                }
            }
        }
        Kind::Variant(_) => {
            table(path)?;
        }
        Kind::Map(value) => {
            for (key, child) in table(path)? {
                path.push(Segment::Key(key.clone()));
                shape(child, value, path)?;
                path.pop();
            }
        }
        Kind::List(item) => match node {
            Node::List(items) => {
                for (at, child) in items.iter().enumerate() {
                    path.push(Segment::Index(at));
                    shape(child, item, path)?;
                    path.pop();
                }
            }
            Node::Table(_) | Node::Absent => {}
            Node::Text(_) => return Err(wrong("a list", path)),
        },
        Kind::Union(union) => match node {
            Node::List(_) => return Err(wrong("a value or a table", path)),
            Node::Table(_) => shape(node, union.structured(), path)?,
            Node::Text(_) | Node::Absent => {}
        },
        Kind::Leaf(_) => {}
    }
    Ok(())
}

impl Source for Tree {
    fn value(&mut self, at: &KeyPath, _leaf: &LeafInfo) -> Result<Option<String>, SourceError> {
        match self.find(at.segments()) {
            None | Some(Node::Absent) => Ok(None),
            Some(Node::Text(text)) => Ok(Some(text.0.clone())),
            Some(_) if at.is_empty() => {
                Err(SourceError::new("expected a single value at the root"))
            }
            Some(_) => Err(SourceError::new(format!("expected a single value at {at}"))),
        }
    }

    fn entries(&mut self, at: &KeyPath) -> Result<Option<Entries>, SourceError> {
        Ok(match self.find(at.segments()) {
            Some(Node::List(items)) => Some(Entries::Len(items.len())),
            Some(Node::Table(entries)) => Some(Entries::Keys(
                entries.iter().map(|(key, _)| key.clone()).collect(),
            )),
            _ => None,
        })
    }

    fn unknown(&mut self, known: &dyn Fn(&KeyPath) -> bool) -> Vec<KeyPath> {
        let mut out = Vec::new();
        walk(&self.root, &mut KeyPath::root(), known, &mut out);
        out
    }
}

/// Any serde format reads into a partly filled `T`; hand it to a builder with `From` or
/// `fill`.
impl<'de, T: Describe> Deserialize<'de> for Partial<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Partial<T>, D::Error> {
        let mut tree = Tree {
            root: Node::deserialize(d)?,
        };
        let mut partial = Partial::<T>::new();
        shape(&tree.root, partial.description(), &mut KeyPath::root())
            .map_err(de::Error::custom)?;
        partial.fill_from(&mut tree).map_err(de::Error::custom)?;
        Ok(partial)
    }
}
