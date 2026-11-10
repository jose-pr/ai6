// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Leaf`]: a type that is one text, and the leaf types of the standard library.

use alloc::string::{String, ToString};
use core::cmp::Ordering;
use core::fmt;
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use core::time::Duration;

use super::{Describe, Description};
use crate::fill::LeafSlot;

/// Why a leaf refused a text. It never quotes the input: a setting may be a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LeafError {
    /// Text that is not a spelling of the type.
    Invalid {
        /// The type.
        type_name: &'static str,
    },
    /// A number outside the type's range.
    OutOfRange {
        /// The type.
        type_name: &'static str,
    },
    /// A number that is NaN or infinite.
    NotFinite,
    /// A duration below zero.
    Negative,
    /// A duration finer than a nanosecond.
    TooFine,
    /// A name the choice does not have.
    UnknownName,
    /// A leaf's own refusal.
    Refused(String),
}

impl fmt::Display for LeafError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeafError::Invalid { type_name } => write!(f, "text that is not a {type_name}"),
            LeafError::OutOfRange { type_name } => {
                write!(f, "a number outside the range of {type_name}")
            }
            LeafError::NotFinite => f.write_str("a number that is not finite"),
            LeafError::Negative => f.write_str("a duration below zero"),
            LeafError::TooFine => f.write_str("a duration finer than a nanosecond"),
            LeafError::UnknownName => f.write_str("a name the setting does not have"),
            LeafError::Refused(why) => f.write_str(why),
        }
    }
}

impl core::error::Error for LeafError {}

/// A type that is one text: a number, a name, a duration, an address.
///
/// **Contract**: `from_text(&x.to_text()) == Ok(x)`; [`assert_round_trip`] checks it. Every
/// `Leaf` is [`Describe`](crate::Describe) and [`Inspect`](crate::Inspect). `#[derive(Leaf)]`
/// writes it from a type's `Display` and `FromStr`.
pub trait Leaf: Sized {
    /// The name a [`Datum`](crate::Datum) shows: `bool`, `u16`, `Duration`, or the type's own.
    const TYPE: &'static str;

    /// The text of the value.
    fn to_text(&self) -> String;

    /// The value a text stands for.
    ///
    /// # Errors
    ///
    /// [`LeafError`] when the text is not a spelling of the type.
    fn from_text(text: &str) -> Result<Self, LeafError>;

    /// The type declares an order ([`order`](Leaf::order) answers `Some`).
    const ORDERED: bool = false;

    /// Whether two values are the same setting. Settings are compared after both texts are
    /// parsed into this type, never as the raw input: by default the canonical texts of the two
    /// values are equal, which the round-trip contract makes the type's own equality. A type
    /// whose equal values can write different texts (`-0.0` and `0.0`) overrides it.
    fn same(a: &Self, b: &Self) -> bool {
        a.to_text() == b.to_text()
    }

    /// The order of two values, where the type has one.
    fn order(_a: &Self, _b: &Self) -> Option<Ordering> {
        None
    }

    /// The texts a unit-only enum accepts.
    fn choices() -> Option<&'static [&'static str]> {
        None
    }

    /// A label for each fixed text the type names, as `(text, label)`: what a form shows
    /// beside the text. Only the texts that have a label are listed.
    fn labels() -> &'static [(&'static str, &'static str)] {
        &[]
    }
}

impl<T: Leaf> Describe for T {
    type Fill = LeafSlot<T>;
    const LEAF: bool = true;

    fn description() -> Description {
        Description::leaf::<T>()
    }
}

/// Checks the round-trip contract of a [`Leaf`] over `values`.
///
/// # Panics
///
/// When a value's text does not read back as an equal value.
pub fn assert_round_trip<T: Leaf + fmt::Debug>(values: &[T]) {
    for value in values {
        let text = value.to_text();
        match T::from_text(&text) {
            Ok(back) if T::same(&back, value) => {}
            Ok(back) => panic!("{value:?} wrote {text:?}, which reads back as {back:?}"),
            Err(error) => panic!("{value:?} wrote {text:?}, which does not read back: {error}"),
        }
    }
}

/// How a field turns into text and back: its type's [`Leaf`] impl ([`Native`]), or the text
/// converters a derive writes for `#[map(with = ..)]` and `#[map(as_text)]`.
pub trait Codec<T> {
    /// The name a datum of the field shows.
    const TYPE: &'static str;
    /// The text of the value.
    fn to_text(value: &T) -> String;
    /// The value a text stands for.
    ///
    /// # Errors
    ///
    /// [`LeafError`] when the text is not one.
    fn from_text(text: &str) -> Result<T, LeafError>;
    /// The codec declares an order.
    const ORDERED: bool = false;
    /// The values are secrets: the place is sensitive in its description and every datum read
    /// from it is marked sensitive.
    const SENSITIVE: bool = false;
    /// Whether two values are the same setting; by default their texts are equal.
    fn same(a: &T, b: &T) -> bool {
        Self::to_text(a) == Self::to_text(b)
    }
    /// The order of two values, where there is one.
    fn order(_a: &T, _b: &T) -> Option<Ordering> {
        None
    }
    /// The texts a closed set accepts.
    fn choices() -> Option<&'static [&'static str]> {
        None
    }
    /// A label for each fixed text, as `(text, label)`.
    fn labels() -> &'static [(&'static str, &'static str)] {
        &[]
    }
}

/// The codec of a type's own [`Leaf`] impl.
#[derive(Debug, Clone, Copy, Default)]
pub struct Native;

impl<T: Leaf> Codec<T> for Native {
    const TYPE: &'static str = T::TYPE;
    const ORDERED: bool = T::ORDERED;
    fn to_text(value: &T) -> String {
        value.to_text()
    }
    fn from_text(text: &str) -> Result<T, LeafError> {
        T::from_text(text)
    }
    fn order(a: &T, b: &T) -> Option<Ordering> {
        T::order(a, b)
    }
    fn same(a: &T, b: &T) -> bool {
        T::same(a, b)
    }
    fn choices() -> Option<&'static [&'static str]> {
        T::choices()
    }
    fn labels() -> &'static [(&'static str, &'static str)] {
        T::labels()
    }
}

macro_rules! leaf_int {
    ($($t:ty),*) => {$(
        impl Leaf for $t {
            const TYPE: &'static str = stringify!($t);
            const ORDERED: bool = true;
            fn to_text(&self) -> String {
                self.to_string()
            }
            fn from_text(text: &str) -> Result<$t, LeafError> {
                text.parse().map_err(|e: core::num::ParseIntError| {
                    use core::num::IntErrorKind::{NegOverflow, PosOverflow};
                    match e.kind() {
                        PosOverflow | NegOverflow => LeafError::OutOfRange { type_name: stringify!($t) },
                        _ => LeafError::Invalid { type_name: stringify!($t) },
                    }
                })
            }
            fn order(a: &$t, b: &$t) -> Option<Ordering> {
                Some(a.cmp(b))
            }
        }
    )*};
}

leaf_int!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize
);

macro_rules! leaf_float {
    ($($t:ty),*) => {$(
        impl Leaf for $t {
            const TYPE: &'static str = stringify!($t);
            const ORDERED: bool = true;
            /// Rust's shortest text that reads back.
            fn to_text(&self) -> String {
                self.to_string()
            }
            fn from_text(text: &str) -> Result<$t, LeafError> {
                match text.parse::<$t>() {
                    Ok(x) if x.is_finite() => Ok(x),
                    Ok(_) => Err(LeafError::NotFinite),
                    Err(_) => Err(LeafError::Invalid { type_name: stringify!($t) }),
                }
            }
            fn order(a: &$t, b: &$t) -> Option<Ordering> {
                a.partial_cmp(b)
            }
            fn same(a: &$t, b: &$t) -> bool {
                a == b
            }
        }
    )*};
}

leaf_float!(f32, f64);

impl Leaf for bool {
    const TYPE: &'static str = "bool";
    fn to_text(&self) -> String {
        String::from(if *self { "true" } else { "false" })
    }
    /// `true`, `false`, and `1 yes on`, `0 no off`, in any case.
    fn from_text(text: &str) -> Result<bool, LeafError> {
        match text.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(LeafError::Invalid { type_name: "bool" }),
        }
    }
}

impl Leaf for String {
    const TYPE: &'static str = "String";
    fn to_text(&self) -> String {
        self.clone()
    }
    fn from_text(text: &str) -> Result<String, LeafError> {
        Ok(text.into())
    }
}

impl Leaf for char {
    const TYPE: &'static str = "char";
    fn to_text(&self) -> String {
        String::from(*self)
    }
    fn from_text(text: &str) -> Result<char, LeafError> {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(c),
            _ => Err(LeafError::Invalid { type_name: "char" }),
        }
    }
}

const UNITS: [(&str, u128); 6] = [
    ("ns", 1),
    ("us", 1_000),
    ("ms", 1_000_000),
    ("s", 1_000_000_000),
    ("m", 60_000_000_000),
    ("h", 3_600_000_000_000),
];

impl Leaf for Duration {
    const TYPE: &'static str = "Duration";
    const ORDERED: bool = true;

    /// Whole seconds as `30s`, else whole milliseconds as `250ms`, else `us`, else `ns`.
    fn to_text(&self) -> String {
        let nanos = u128::from(self.as_secs()) * 1_000_000_000 + u128::from(self.subsec_nanos());
        for (unit, size) in [("s", 1_000_000_000), ("ms", 1_000_000), ("us", 1_000)] {
            if nanos % size == 0 {
                return alloc::format!("{}{unit}", nanos / size);
            }
        }
        alloc::format!("{nanos}ns")
    }

    /// A decimal number and an optional unit (`ns us ms s m h`; none is seconds): `30`,
    /// `0.25`, `30s`, `250ms`, `1.5h`. The number is read exactly.
    fn from_text(text: &str) -> Result<Duration, LeafError> {
        let invalid = || LeafError::Invalid {
            type_name: "Duration",
        };
        if text.starts_with('-') {
            return Err(LeafError::Negative);
        }
        let end = text
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(text.len());
        let (number, unit) = text.split_at(end);
        let size = if unit.is_empty() {
            1_000_000_000
        } else {
            UNITS
                .iter()
                .find(|(u, _)| *u == unit)
                .ok_or_else(invalid)?
                .1
        };
        let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
        if whole.is_empty() || (number.contains('.') && fraction.is_empty()) {
            return Err(invalid());
        }
        let out_of_range = || LeafError::OutOfRange {
            type_name: "Duration",
        };
        let fraction = fraction.trim_end_matches('0');
        let whole: u128 = whole.parse().map_err(|_| out_of_range())?;
        let mut nanos = whole.checked_mul(size).ok_or_else(out_of_range)?;
        if !fraction.is_empty() {
            if fraction.len() > 30 {
                return Err(LeafError::TooFine);
            }
            let digits: u128 = fraction.parse().map_err(|_| invalid())?;
            let scale = 10u128.pow(fraction.len() as u32);
            let part = digits.checked_mul(size).ok_or_else(out_of_range)?;
            if part % scale != 0 {
                return Err(LeafError::TooFine);
            }
            nanos = nanos.checked_add(part / scale).ok_or_else(out_of_range)?;
        }
        let secs = u64::try_from(nanos / 1_000_000_000).map_err(|_| out_of_range())?;
        Ok(Duration::new(secs, (nanos % 1_000_000_000) as u32))
    }

    fn order(a: &Duration, b: &Duration) -> Option<Ordering> {
        Some(a.cmp(b))
    }
}

macro_rules! leaf_text {
    ($($t:ty => $name:literal),* $(,)?) => {$(
        impl Leaf for $t {
            const TYPE: &'static str = $name;
            fn to_text(&self) -> String {
                self.to_string()
            }
            fn from_text(text: &str) -> Result<$t, LeafError> {
                text.parse().map_err(|_| LeafError::Invalid { type_name: $name })
            }
        }
    )*};
}

leaf_text! {
    IpAddr => "IpAddr",
    Ipv4Addr => "Ipv4Addr",
    Ipv6Addr => "Ipv6Addr",
    SocketAddr => "SocketAddr",
    SocketAddrV4 => "SocketAddrV4",
    SocketAddrV6 => "SocketAddrV6",
}

#[cfg(feature = "std")]
impl Leaf for std::path::PathBuf {
    const TYPE: &'static str = "Path";
    /// The path as text; a part that is not UTF-8 is replaced.
    fn to_text(&self) -> String {
        self.to_string_lossy().into_owned()
    }
    fn from_text(text: &str) -> Result<std::path::PathBuf, LeafError> {
        Ok(text.into())
    }
}
