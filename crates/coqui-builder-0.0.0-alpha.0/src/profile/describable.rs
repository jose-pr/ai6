// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Expect`] and [`Profile`] as settings of their own, written by hand with the slots of
//! [`fill`](crate::fill): a profile read from a file or a form is filled like any other type.
//!
//! An expectation is a variant tagged `expect`, its arms named in `snake_case` (`equal`,
//! `within`, `one_of`, `as_strict_as`, `no_stricter_than`, `subset_of`, `items_equal`,
//! `required`, `absent`, `arms`, `any`); a profile is an open map from a key's text to an
//! expectation. The tests hold both to the description a derive would write for the same
//! shapes.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use super::{Expect, Profile};
use crate::datum::Datum;
use crate::describe::{Arm, Describe, Description, Field, LeafError, Variant};
use crate::fill::{Fill, FillError, FillErrorKind, LeafSlot, ListSlot, MapSlot, OptionSlot};
use crate::inspect::{Entries, Inspect};
use crate::path::{KeyPath, Segment};

const TAG: &str = "expect";
const ARMS: [&str; 11] = [
    "equal",
    "within",
    "one_of",
    "as_strict_as",
    "no_stricter_than",
    "subset_of",
    "items_equal",
    "required",
    "absent",
    "arms",
    "any",
];

fn field<T: Describe>(key: &str) -> Field {
    let mut field = Field::new(key, T::description());
    field.optional = T::OPTIONAL;
    field.required = !T::OPTIONAL;
    field
}

fn arm_of(expect: &Expect) -> usize {
    match expect {
        Expect::Equal(_) => 0,
        Expect::Within { .. } => 1,
        Expect::OneOf(_) => 2,
        Expect::AsStrictAs(_) => 3,
        Expect::NoStricterThan(_) => 4,
        Expect::SubsetOf(_) => 5,
        Expect::ItemsEqual(_) => 6,
        Expect::Required => 7,
        Expect::Absent => 8,
        Expect::Arms(_) => 9,
        Expect::Any => 10,
    }
}

impl Describe for Expect {
    type Fill = ExpectFill;

    fn description() -> Description {
        let value = || alloc::vec![field::<String>("value")];
        let values = || alloc::vec![field::<Vec<String>>("values")];
        let arms = alloc::vec![
            Arm::new("equal", value()),
            Arm::new(
                "within",
                alloc::vec![
                    field::<Option<String>>("min"),
                    field::<Option<String>>("max")
                ],
            ),
            Arm::new("one_of", values()),
            Arm::new("as_strict_as", value()),
            Arm::new("no_stricter_than", value()),
            Arm::new("subset_of", values()),
            Arm::new("items_equal", values()),
            Arm::new("required", Vec::new()),
            Arm::new("absent", Vec::new()),
            Arm::new("arms", alloc::vec![field::<Vec<String>>("names")]),
            Arm::new("any", Vec::new()),
        ];
        Description::variant(Variant::new(TAG, arms))
    }
}

type Text = LeafSlot<String>;
type Texts = ListSlot<String, LeafSlot<String>>;

/// An [`Expect`] being filled: the arm the tag names, and the slots its fields share.
#[derive(Default)]
pub struct ExpectFill {
    arm: Option<usize>,
    default: Option<usize>,
    value: Text,
    min: OptionSlot<String, Text>,
    max: OptionSlot<String, Text>,
    values: Texts,
    names: Texts,
}

impl fmt::Debug for ExpectFill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExpectFill")
            .field("arm", &self.arm.map(|a| ARMS[a]))
            .finish_non_exhaustive()
    }
}

impl ExpectFill {
    fn select(&mut self, name: &str) -> Result<(), FillError> {
        match ARMS.iter().position(|arm| *arm == name) {
            Some(arm) => {
                self.arm = Some(arm);
                Ok(())
            }
            None => Err(FillError::leaf(LeafError::UnknownName)),
        }
    }
}

impl Inspect for ExpectFill {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        let (first, rest) = at.split_first()?;
        match &*first.name() {
            TAG => rest
                .is_empty()
                .then_some(self.arm)
                .flatten()
                .map(|a| Datum::new("String", ARMS[a])),
            "value" => self.value.at(rest),
            "min" => self.min.at(rest),
            "max" => self.max.at(rest),
            "values" => self.values.at(rest),
            "names" => self.names.at(rest),
            _ => None,
        }
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        match at.split_first() {
            None => {
                self.arm?;
                Some(Entries::Keys(alloc::vec![String::from(TAG)]))
            }
            Some((first, rest)) => match &*first.name() {
                "values" => self.values.entries(rest),
                "names" => self.names.entries(rest),
                _ => None,
            },
        }
    }
}

macro_rules! route {
    ($self:ident, $at:ident, $method:ident, $($arg:expr),*) => {{
        let Some((first, rest)) = $at.split_first() else {
            return Err(FillError::new(FillErrorKind::Unsupported(
                "a group is set one key at a time",
            )));
        };
        let under = |e: FillError| e.under(first.clone());
        match &*first.name() {
            "value" => Fill::<String>::$method(&mut $self.value, rest, $($arg),*).map_err(under),
            "min" => Fill::<Option<String>>::$method(&mut $self.min, rest, $($arg),*).map_err(under),
            "max" => Fill::<Option<String>>::$method(&mut $self.max, rest, $($arg),*).map_err(under),
            "values" => Fill::<Vec<String>>::$method(&mut $self.values, rest, $($arg),*).map_err(under),
            "names" => Fill::<Vec<String>>::$method(&mut $self.names, rest, $($arg),*).map_err(under),
            _ => Err(FillError::unknown_key(first.clone())),
        }
    }};
}

impl Fill<Expect> for ExpectFill {
    fn set(&mut self, at: &[Segment], text: &str) -> Result<(), FillError> {
        if let Some((first, rest)) = at.split_first()
            && &*first.name() == TAG
        {
            if let Some(next) = rest.first() {
                return Err(FillError::unknown_key(next.clone()).under(first.clone()));
            }
            return self.select(text).map_err(|e| e.under(first.clone()));
        }
        route!(self, at, set, text)
    }

    fn set_items(&mut self, at: &[Segment], items: &[String]) -> Result<(), FillError> {
        route!(self, at, set_items, items)
    }

    fn clear(&mut self, at: &[Segment]) -> Result<(), FillError> {
        if let Some((first, _)) = at.split_first()
            && &*first.name() == TAG
        {
            self.arm = None;
            return Ok(());
        }
        route!(self, at, clear,)
    }

    fn set_default(&mut self, value: Expect) {
        self.default = Some(arm_of(&value));
        match value {
            Expect::Equal(v) | Expect::AsStrictAs(v) | Expect::NoStricterThan(v) => {
                Fill::<String>::set_default(&mut self.value, v);
            }
            Expect::Within { min, max } => {
                Fill::<Option<String>>::set_default(&mut self.min, min);
                Fill::<Option<String>>::set_default(&mut self.max, max);
            }
            Expect::OneOf(v) | Expect::SubsetOf(v) | Expect::ItemsEqual(v) => {
                Fill::<Vec<String>>::set_default(&mut self.values, v);
            }
            Expect::Arms(v) => Fill::<Vec<String>>::set_default(&mut self.names, v),
            Expect::Required | Expect::Absent | Expect::Any => {}
        }
    }

    fn build(self, at: &KeyPath, missing: &mut Vec<KeyPath>) -> Option<Expect> {
        let Some(arm) = self.arm.or(self.default) else {
            missing.push(at.child(Segment::Key(TAG.into())));
            return None;
        };
        let key = |name: &str| at.child(Segment::Key(name.into()));
        match arm {
            0 | 3 | 4 => {
                let value = Fill::<String>::build(self.value, &key("value"), missing)?;
                Some(match arm {
                    0 => Expect::Equal(value),
                    3 => Expect::AsStrictAs(value),
                    _ => Expect::NoStricterThan(value),
                })
            }
            1 => {
                let min = Fill::<Option<String>>::build(self.min, &key("min"), missing);
                let max = Fill::<Option<String>>::build(self.max, &key("max"), missing);
                Some(Expect::Within {
                    min: min?,
                    max: max?,
                })
            }
            2 | 5 | 6 => {
                let values = Fill::<Vec<String>>::build(self.values, &key("values"), missing)?;
                Some(match arm {
                    2 => Expect::OneOf(values),
                    5 => Expect::SubsetOf(values),
                    _ => Expect::ItemsEqual(values),
                })
            }
            7 => Some(Expect::Required),
            8 => Some(Expect::Absent),
            9 => Fill::<Vec<String>>::build(self.names, &key("names"), missing).map(Expect::Arms),
            10 => Some(Expect::Any),
            _ => None,
        }
    }

    fn put_any(&mut self, at: &[Segment], value: &mut dyn core::any::Any) -> Result<(), FillError> {
        let Some((first, rest)) = at.split_first() else {
            return crate::fill::put_whole::<Expect, Self>(self, value);
        };
        let under = |e: FillError| e.under(first.clone());
        match &*first.name() {
            "value" => Fill::<String>::put_any(&mut self.value, rest, value).map_err(under),
            "min" => Fill::<Option<String>>::put_any(&mut self.min, rest, value).map_err(under),
            "max" => Fill::<Option<String>>::put_any(&mut self.max, rest, value).map_err(under),
            "values" => Fill::<Vec<String>>::put_any(&mut self.values, rest, value).map_err(under),
            "names" => Fill::<Vec<String>>::put_any(&mut self.names, rest, value).map_err(under),
            _ => Err(FillError::unknown_key(first.clone())),
        }
    }

    fn put(&mut self, value: Expect) {
        self.arm = Some(arm_of(&value));
        match value {
            Expect::Equal(v) | Expect::AsStrictAs(v) | Expect::NoStricterThan(v) => {
                Fill::<String>::put(&mut self.value, v);
            }
            Expect::Within { min, max } => {
                Fill::<Option<String>>::put(&mut self.min, min);
                Fill::<Option<String>>::put(&mut self.max, max);
            }
            Expect::OneOf(v) | Expect::SubsetOf(v) | Expect::ItemsEqual(v) => {
                Fill::<Vec<String>>::put(&mut self.values, v);
            }
            Expect::Arms(v) => Fill::<Vec<String>>::put(&mut self.names, v),
            Expect::Required | Expect::Absent | Expect::Any => {}
        }
    }

    fn holds_none(&self, at: &[Segment]) -> bool {
        let Some((first, rest)) = at.split_first() else {
            return false;
        };
        match &*first.name() {
            "min" => Fill::<Option<String>>::holds_none(&self.min, rest),
            "max" => Fill::<Option<String>>::holds_none(&self.max, rest),
            _ => false,
        }
    }

    fn from_value(value: Expect) -> Self {
        let mut fill = ExpectFill {
            arm: Some(arm_of(&value)),
            ..ExpectFill::default()
        };
        match value {
            Expect::Equal(v) | Expect::AsStrictAs(v) | Expect::NoStricterThan(v) => {
                fill.value = LeafSlot::from_value(v);
            }
            Expect::Within { min, max } => {
                fill.min = OptionSlot::from_value(min);
                fill.max = OptionSlot::from_value(max);
            }
            Expect::OneOf(v) | Expect::SubsetOf(v) | Expect::ItemsEqual(v) => {
                fill.values = ListSlot::from_value(v);
            }
            Expect::Arms(v) => fill.names = ListSlot::from_value(v),
            Expect::Required | Expect::Absent | Expect::Any => {}
        }
        fill
    }
}

impl Inspect for Expect {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        let name = ARMS[arm_of(self)];
        let (first, rest) = at.split_first()?;
        let text = |t: &String| Datum::new("String", t.as_str());
        match (&*first.name(), self) {
            (TAG, _) => rest.is_empty().then(|| Datum::new("String", name)),
            ("value", Expect::Equal(v) | Expect::AsStrictAs(v) | Expect::NoStricterThan(v)) => {
                rest.is_empty().then(|| text(v))
            }
            ("min", Expect::Within { min, .. }) => {
                min.as_ref().filter(|_| rest.is_empty()).map(text)
            }
            ("max", Expect::Within { max, .. }) => {
                max.as_ref().filter(|_| rest.is_empty()).map(text)
            }
            ("values", Expect::OneOf(v) | Expect::SubsetOf(v) | Expect::ItemsEqual(v))
            | ("names", Expect::Arms(v)) => {
                let (index, rest) = rest.split_first()?;
                rest.is_empty()
                    .then(|| v.get(index.as_index()?).map(text))
                    .flatten()
            }
            _ => None,
        }
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        match at.split_first() {
            None => {
                let mut keys = alloc::vec![String::from(TAG)];
                match self {
                    Expect::Equal(_) | Expect::AsStrictAs(_) | Expect::NoStricterThan(_) => {
                        keys.push(String::from("value"));
                    }
                    Expect::Within { min, max } => {
                        if min.is_some() {
                            keys.push(String::from("min"));
                        }
                        if max.is_some() {
                            keys.push(String::from("max"));
                        }
                    }
                    Expect::OneOf(_) | Expect::SubsetOf(_) | Expect::ItemsEqual(_) => {
                        keys.push(String::from("values"));
                    }
                    Expect::Arms(_) => keys.push(String::from("names")),
                    Expect::Required | Expect::Absent | Expect::Any => {}
                }
                Some(Entries::Keys(keys))
            }
            Some((first, [])) => match (&*first.name(), self) {
                ("values", Expect::OneOf(v) | Expect::SubsetOf(v) | Expect::ItemsEqual(v))
                | ("names", Expect::Arms(v)) => Some(Entries::Len(v.len())),
                _ => None,
            },
            _ => None,
        }
    }
}

impl Describe for Profile {
    type Fill = ProfileFill;

    fn description() -> Description {
        Description::map(Expect::description())
    }
}

/// A [`Profile`] being filled: an open map from a key's text to an expectation.
///
/// A key that is not a path is reported missing by `build`.
pub struct ProfileFill {
    entries: MapSlot<BTreeMap<String, Expect>, Expect, ExpectFill>,
}

impl Default for ProfileFill {
    fn default() -> Self {
        let mut entries = MapSlot::default();
        entries.set_default(BTreeMap::new());
        ProfileFill { entries }
    }
}

impl fmt::Debug for ProfileFill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileFill").finish_non_exhaustive()
    }
}

fn entries_of(profile: Profile) -> BTreeMap<String, Expect> {
    profile
        .entries()
        .iter()
        .map(|(key, expect)| (key.to_string(), expect.clone()))
        .collect()
}

impl Inspect for ProfileFill {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        self.entries.at(at)
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        self.entries.entries(at)
    }
}

impl Fill<Profile> for ProfileFill {
    fn set(&mut self, at: &[Segment], text: &str) -> Result<(), FillError> {
        self.entries.set(at, text)
    }

    fn set_items(&mut self, at: &[Segment], items: &[String]) -> Result<(), FillError> {
        self.entries.set_items(at, items)
    }

    fn clear(&mut self, at: &[Segment]) -> Result<(), FillError> {
        self.entries.clear(at)
    }

    fn set_default(&mut self, value: Profile) {
        self.entries.set_default(entries_of(value));
    }

    fn build(self, at: &KeyPath, missing: &mut Vec<KeyPath>) -> Option<Profile> {
        let entries = self.entries.build(at, missing)?;
        let mut profile = Profile::new();
        let mut complete = true;
        for (key, expect) in entries {
            match key.parse::<KeyPath>() {
                Ok(path) => {
                    profile.insert(path, expect);
                }
                Err(_) => {
                    missing.push(at.child(Segment::Key(key)));
                    complete = false;
                }
            }
        }
        complete.then_some(profile)
    }

    fn put(&mut self, value: Profile) {
        self.entries.put(entries_of(value));
    }

    fn put_any(&mut self, at: &[Segment], value: &mut dyn core::any::Any) -> Result<(), FillError> {
        if at.is_empty() {
            return crate::fill::put_whole::<Profile, Self>(self, value);
        }
        self.entries.put_any(at, value)
    }

    fn holds_none(&self, at: &[Segment]) -> bool {
        self.entries.holds_none(at)
    }

    fn from_value(value: Profile) -> Self {
        ProfileFill {
            entries: MapSlot::from_value(entries_of(value)),
        }
    }
}

impl Inspect for Profile {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        let (first, rest) = at.split_first()?;
        let name = first.name();
        self.entries()
            .iter()
            .find(|(key, _)| key.to_string() == *name)?
            .1
            .at(rest)
    }

    fn entries(&self, at: &[Segment]) -> Option<Entries> {
        match at.split_first() {
            None => Some(Entries::Keys(
                self.entries().iter().map(|(k, _)| k.to_string()).collect(),
            )),
            Some((first, rest)) => {
                let name = first.name();
                self.entries()
                    .iter()
                    .find(|(key, _)| key.to_string() == *name)?
                    .1
                    .entries(rest)
            }
        }
    }
}
