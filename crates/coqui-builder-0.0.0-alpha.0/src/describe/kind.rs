// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The shapes a [`Description`] is made of, its leaves and its annotations.

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;

use super::{Codec, Description, Leaf, LeafError, Native};
use crate::fill::Fill;
use crate::inspect::Inspect;
use crate::path::Segment;

/// Which way a setting gets stricter: a larger value, or a smaller one.
///
/// A [`Profile`](crate::profile::Profile) compares two values along this order. An unset
/// optional setting ranks loosest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stricter {
    /// A higher value is stricter (a minimum length, an attempt count).
    Higher,
    /// A lower value is stricter (a timeout, a limit).
    Lower,
}

/// What a description says about a setting beyond its shape.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Notes {
    /// A short label.
    pub title: Option<String>,
    /// A longer explanation, from the field's doc comment unless given.
    pub description: Option<String>,
    /// The value is a secret: no report, form or log shows it.
    pub sensitive: bool,
    /// The value cannot be changed by the person filling the settings.
    pub read_only: bool,
    /// The setting is going away.
    pub deprecated: bool,
    /// Example values, as text.
    pub examples: Vec<String>,
    /// The group of settings a form shows this one under.
    pub section: Option<String>,
    /// Where a form shows this one among its siblings.
    pub order: Option<i64>,
    /// A form hides this one until asked.
    pub advanced: bool,
    /// Which way the setting gets stricter.
    pub stricter: Option<Stricter>,
    /// The fewest characters a text may have.
    pub min_length: Option<u64>,
    /// The most characters a text may have.
    pub max_length: Option<u64>,
    /// A regular expression the text should match. Carried for forms and schemas, never
    /// enforced here.
    pub pattern: Option<String>,
    /// A named text format. Carried, never enforced here.
    pub format: Option<String>,
    /// A step a number must be a multiple of.
    pub multiple_of: Option<f64>,
    /// The fewest items a list may have.
    pub min_items: Option<u64>,
    /// The most items a list may have.
    pub max_items: Option<u64>,
}

/// What a leaf is: its type name, its choices and the operations of its parser.
///
/// The operations parse their texts, so two settings are compared as typed values and never
/// as text.
#[derive(Clone, Copy)]
pub struct LeafInfo {
    ty: &'static str,
    choices: Option<&'static [&'static str]>,
    labels: &'static [(&'static str, &'static str)],
    ordered: bool,
    check: fn(&str) -> Result<(), LeafError>,
    eq: fn(&str, &str) -> Result<bool, LeafError>,
    order: fn(&str, &str) -> Result<Option<Ordering>, LeafError>,
}

fn check_with<T, C: Codec<T>>(text: &str) -> Result<(), LeafError> {
    C::from_text(text).map(|_| ())
}

fn eq_with<T, C: Codec<T>>(a: &str, b: &str) -> Result<bool, LeafError> {
    Ok(C::same(&C::from_text(a)?, &C::from_text(b)?))
}

fn order_with<T, C: Codec<T>>(a: &str, b: &str) -> Result<Option<Ordering>, LeafError> {
    Ok(C::order(&C::from_text(a)?, &C::from_text(b)?))
}

impl LeafInfo {
    /// The info of a [`Leaf`] type.
    pub fn of<T: Leaf>() -> LeafInfo {
        LeafInfo::of_codec::<T, Native>()
    }

    /// The info of a type converted by `C`.
    pub fn of_codec<T, C: Codec<T>>() -> LeafInfo {
        LeafInfo {
            ty: C::TYPE,
            choices: C::choices(),
            labels: C::labels(),
            ordered: C::ORDERED,
            check: check_with::<T, C>,
            eq: eq_with::<T, C>,
            order: order_with::<T, C>,
        }
    }

    /// The type name.
    pub fn ty(&self) -> &'static str {
        self.ty
    }

    /// The texts a closed set accepts.
    pub fn choices(&self) -> Option<&'static [&'static str]> {
        self.choices
    }

    /// The labels of the texts the type names, as `(text, label)`; a `with` or `as_text` leaf
    /// has none.
    pub fn labels(&self) -> &'static [(&'static str, &'static str)] {
        self.labels
    }

    /// The label of the fixed text `text`, if it has one.
    pub fn label(&self, text: &str) -> Option<&'static str> {
        self.labels
            .iter()
            .find(|(t, _)| *t == text)
            .map(|(_, label)| *label)
    }

    /// Whether the type declares an order.
    pub fn is_ordered(&self) -> bool {
        self.ordered
    }

    /// Whether `text` is a spelling of the type.
    ///
    /// # Errors
    ///
    /// The parser's refusal.
    pub fn check(&self, text: &str) -> Result<(), LeafError> {
        (self.check)(text)
    }

    /// Whether two texts are the same value of the type.
    ///
    /// # Errors
    ///
    /// The parser's refusal of either text.
    pub fn eq(&self, a: &str, b: &str) -> Result<bool, LeafError> {
        (self.eq)(a, b)
    }

    /// The order of two texts, `None` where the type has none.
    ///
    /// # Errors
    ///
    /// The parser's refusal of either text.
    pub fn order(&self, a: &str, b: &str) -> Result<Option<Ordering>, LeafError> {
        (self.order)(a, b)
    }
}

impl fmt::Debug for LeafInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeafInfo")
            .field("ty", &self.ty)
            .field("choices", &self.choices)
            .field("labels", &self.labels)
            .field("ordered", &self.ordered)
            .finish()
    }
}

impl PartialEq for LeafInfo {
    fn eq(&self, other: &Self) -> bool {
        self.ty == other.ty
            && self.choices == other.choices
            && self.labels == other.labels
            && self.ordered == other.ordered
    }
}

/// The shape of a setting. Structure only: a scalar is a [`Kind::Leaf`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Kind {
    /// One text of one type.
    Leaf(LeafInfo),
    /// A list of one shape of item.
    List(Box<Description>),
    /// An open map from text keys to one shape of value.
    Map(Box<Description>),
    /// Named settings, each with its own description.
    Group(Group),
    /// A group whose fields depend on a tag naming one of its arms.
    Variant(Variant),
    /// An untagged enum: a text side and one structured arm.
    Union(Union),
}

/// An untagged enum that holds text arms and one structured arm.
///
/// The text arms (unit names and single-value arms) are one leaf; the structured arm is a
/// group, a list, a map or a tagged variant. A source that answers a value at the union's
/// place means the text side; a source that answers entries, or any key below, means the
/// structured arm.
#[derive(Debug, Clone, PartialEq)]
pub struct Union {
    text: LeafInfo,
    structured: Box<Description>,
    default_structured: bool,
}

impl Union {
    /// A union of the text arms `text` and the structured arm `structured`.
    pub fn new(text: LeafInfo, structured: Description) -> Union {
        Union {
            text,
            structured: Box::new(structured),
            default_structured: false,
        }
    }

    /// The text arms, as one leaf: its type name is the enum's, its choices the unit names
    /// when the enum has no other text arm, and it reads a text by trying the unit names and
    /// then each single-value arm in declaration order.
    pub fn text(&self) -> &LeafInfo {
        &self.text
    }

    /// The structured arm.
    pub fn structured(&self) -> &Description {
        &self.structured
    }

    /// Whether the default is the structured arm (its fields carry the defaults) rather than
    /// a text.
    pub fn default_is_structured(&self) -> bool {
        self.default_structured
    }
}

/// A named setting of a [`Group`] or an [`Arm`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Field {
    /// The key, as the person writes it.
    pub key: String,
    /// The key must be given: there is no default and the type is not optional.
    pub required: bool,
    /// The type is `Option<_>`: leaving it out is a value of its own.
    pub optional: bool,
    /// The text of the default. For a group, the defaults are on its fields.
    pub default: Option<String>,
    /// The texts of the default of a list.
    pub default_items: Option<Vec<String>>,
    /// The text that writes `None` where the field is an `Option` with a default.
    pub none: Option<String>,
    /// What the setting is.
    pub description: Description,
}

impl Field {
    /// A required field `key` of `description`.
    pub fn new(key: impl Into<String>, description: Description) -> Field {
        Field {
            key: key.into(),
            required: true,
            optional: false,
            default: None,
            default_items: None,
            none: None,
            description,
        }
    }

    /// Makes `value` the default: every leaf below this field takes the text it holds.
    pub fn default_value<T: crate::Describe>(&mut self, value: T) {
        let fill = <T::Fill as Fill<T>>::from_value(value);
        self.default_from(&fill);
        if self.default.is_none()
            && self.default_items.is_none()
            && let Some(none) = &self.none
            && matches!(self.description.kind(), Kind::Leaf(_))
        {
            self.default = Some(none.clone());
        }
        self.required = false;
    }

    /// Makes `value`, converted by `C`, the default of a leaf field.
    #[doc(hidden)]
    pub fn default_leaf<T, C: Codec<T>>(&mut self, value: &T) {
        self.default = Some(C::to_text(value));
        self.required = false;
    }

    /// Makes `value`, converted by `C`, the default of an optional leaf field.
    #[doc(hidden)]
    pub fn default_option<T, C: Codec<T>>(&mut self, value: &Option<T>) {
        self.default = value.as_ref().map(C::to_text).or_else(|| self.none.clone());
        self.required = false;
    }

    /// Makes `items`, converted by `C`, the default of a list field.
    #[doc(hidden)]
    pub fn default_list<T, C: Codec<T>>(&mut self, items: &[T]) {
        self.default_items = Some(items.iter().map(C::to_text).collect());
        self.required = false;
    }

    /// Fills the defaults from what `from` answers for this field's place (the root is this
    /// field itself).
    pub fn default_from(&mut self, from: &dyn Inspect) {
        let mut path = Vec::new();
        self.default_at(from, &mut path);
    }

    fn default_at(&mut self, from: &dyn Inspect, path: &mut Vec<Segment>) {
        match &mut self.description.kind {
            Kind::Group(group) => default_fields(&mut group.fields, from, path),
            Kind::Variant(variant) => {
                let mut tag = path.clone();
                tag.push(Segment::Key(variant.tag.clone()));
                if let Some(arm) = from.at(&tag).map(|d| d.text().to_owned()) {
                    if let Some(found) = variant.arms.iter_mut().find(|a| a.name == arm) {
                        default_fields(&mut found.fields, from, path);
                    }
                    variant.default_arm = Some(arm.clone());
                    self.default = Some(arm);
                }
            }
            Kind::List(item) if matches!(item.kind, Kind::Leaf(_)) => {
                if let Some(crate::inspect::Entries::Len(count)) = from.entries(path) {
                    let mut items = Vec::new();
                    for index in 0..count {
                        path.push(Segment::Index(index));
                        items.extend(from.at(path).map(|d| d.text().to_owned()));
                        path.pop();
                    }
                    self.default_items = Some(items);
                }
            }
            Kind::Leaf(_) => self.default = from.at(path).map(|d| d.text().to_owned()),
            Kind::Union(_) => {
                if let Some(datum) = from.at(path) {
                    self.default = Some(datum.text().to_owned());
                } else if from.entries(path).is_some() {
                    self.default_structured_arm(from, path);
                }
            }
            _ => {}
        }
        self.required = false;
    }
}

impl Field {
    /// The default is the structured arm of an untagged enum: the arm's own fields take the
    /// texts `from` holds.
    fn default_structured_arm(&mut self, from: &dyn Inspect, path: &mut Vec<Segment>) {
        let Kind::Union(union) = &mut self.description.kind else {
            return;
        };
        union.default_structured = true;
        let placeholder = Description::new(Kind::Group(Group::new(Vec::new())));
        let mut arm = Field::new("", core::mem::replace(&mut *union.structured, placeholder));
        arm.default_at(from, path);
        *union.structured = arm.description;
        self.default_items = arm.default_items;
    }
}

fn default_fields(fields: &mut [Field], from: &dyn Inspect, path: &mut Vec<Segment>) {
    for field in fields {
        path.push(Segment::Key(field.key.clone()));
        field.default_at(from, path);
        path.pop();
    }
}

/// Named settings.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Group {
    /// The fields, in declaration order.
    pub fields: Vec<Field>,
    /// A key the group does not declare is refused rather than reported.
    pub deny_unknown_fields: bool,
}

impl Group {
    /// A group of `fields`.
    pub fn new(fields: Vec<Field>) -> Group {
        Group {
            fields,
            deny_unknown_fields: false,
        }
    }

    /// The field named `key`.
    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }
}

/// One arm of a [`Variant`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Arm {
    /// The text the tag holds for this arm.
    pub name: String,
    /// A short label.
    pub title: Option<String>,
    /// A longer explanation.
    pub description: Option<String>,
    /// The arm's own fields.
    pub fields: Vec<Field>,
}

impl Arm {
    /// An arm `name` with `fields`.
    pub fn new(name: impl Into<String>, fields: Vec<Field>) -> Arm {
        Arm {
            name: name.into(),
            title: None,
            description: None,
            fields,
        }
    }
}

/// A group whose fields depend on which arm a tag names.
///
/// The tag is itself a setting: it sits at `<variant>.<tag>`, a text leaf.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub(crate) tag: String,
    pub(crate) arms: Vec<Arm>,
    pub(crate) default_arm: Option<String>,
    pub(crate) tag_description: Box<Description>,
}

impl Variant {
    /// A variant told apart by the key `tag`.
    pub fn new(tag: impl Into<String>, arms: Vec<Arm>) -> Variant {
        Variant {
            tag: tag.into(),
            arms,
            default_arm: None,
            tag_description: Box::new(Description::leaf::<String>()),
        }
    }

    /// The key the arm's name is written under.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// The arms, in declaration order.
    pub fn arms(&self) -> &[Arm] {
        &self.arms
    }

    /// The arm named `name`.
    pub fn arm(&self, name: &str) -> Option<&Arm> {
        self.arms.iter().find(|a| a.name == name)
    }

    /// The arm the variant starts as, when it has a default.
    pub fn default_arm(&self) -> Option<&str> {
        self.default_arm.as_deref()
    }

    /// What the tag is: a text leaf.
    pub fn tag_description(&self) -> &Description {
        &self.tag_description
    }
}
