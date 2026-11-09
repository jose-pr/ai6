//! Typed reads: a value becomes a `T` with no document text able to reach the error.
//!
//! A value's own deserializer reports a refusal in its own error type, whose message quotes the
//! value (`invalid type: string "..."`) and may hold anything a `custom` call was given. So the
//! value is first copied into [`Content`], which then deserializes `T` under [`Refusal`], an
//! error type that keeps the names serde hands it and drops the values.

use core::cell::Cell;
use core::fmt;

use serde::Deserializer;
use serde::de::value::{MapAccessDeserializer, MapDeserializer, SeqDeserializer};
use serde::de::{self, DeserializeOwned, DeserializeSeed, IntoDeserializer, Unexpected, Visitor};
use serde::forward_to_deserialize_any;

use crate::error::Error;

/// Deserialize `value` into `T`, refusing a document nested deeper than `max_depth`.
pub(crate) fn from_value<'de, V, T>(value: V, max_depth: usize) -> Result<T, Error>
where
    V: Deserializer<'de>,
    T: DeserializeOwned,
{
    let content = Content::read(value, max_depth)?;
    serde_path_to_error::deserialize(content).map_err(|error| {
        let path = error.path();
        let path = path
            .iter()
            .next()
            .is_some()
            .then(|| escape(&path.to_string()));
        Error::Conversion {
            path,
            message: Some(error.into_inner().0),
        }
    })
}

/// A document's value, in document order, read once and thrown away with the read.
enum Content {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    I128(i128),
    U128(u128),
    F64(f64),
    Str(String),
    Bytes(Vec<u8>),
    Seq(Vec<Content>),
    Map(Vec<(Content, Content)>),
}

impl Content {
    fn read<'de, V: Deserializer<'de>>(value: V, max_depth: usize) -> Result<Self, Error> {
        let too_deep = Cell::new(false);
        let seed = ContentSeed {
            depth: 0,
            max_depth,
            too_deep: &too_deep,
        };
        // The value's error is not looked at: its text may quote the value.
        seed.deserialize(value).map_err(|_| {
            if too_deep.get() {
                Error::TooDeep { limit: max_depth }
            } else {
                Error::Conversion {
                    path: None,
                    message: Some("the value could not be read".to_owned()),
                }
            }
        })
    }

    fn kind(&self) -> Unexpected<'_> {
        match self {
            Self::Null => Unexpected::Unit,
            Self::Bool(value) => Unexpected::Bool(*value),
            Self::I64(value) => Unexpected::Signed(*value),
            Self::U64(value) => Unexpected::Unsigned(*value),
            Self::I128(_) | Self::U128(_) => Unexpected::Other("an integer"),
            Self::F64(value) => Unexpected::Float(*value),
            Self::Str(value) => Unexpected::Str(value),
            Self::Bytes(value) => Unexpected::Bytes(value),
            Self::Seq(_) => Unexpected::Seq,
            Self::Map(_) => Unexpected::Map,
        }
    }
}

/// Reads one node. The walk is bounded because a document's nesting is a stack its author
/// controls: the same bound then covers `T`'s deserialization and the drop of the copy.
#[derive(Clone, Copy)]
struct ContentSeed<'a> {
    depth: usize,
    max_depth: usize,
    too_deep: &'a Cell<bool>,
}

impl ContentSeed<'_> {
    fn child(self) -> Self {
        Self {
            depth: self.depth + 1,
            ..self
        }
    }
}

impl<'de> DeserializeSeed<'de> for ContentSeed<'_> {
    type Value = Content;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Content, D::Error> {
        if self.depth > self.max_depth {
            self.too_deep.set(true);
            return Err(de::Error::custom("too deep"));
        }
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ContentSeed<'_> {
    type Value = Content;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Content, E> {
        Ok(Content::Bool(value))
    }
    fn visit_i64<E>(self, value: i64) -> Result<Content, E> {
        Ok(Content::I64(value))
    }
    fn visit_u64<E>(self, value: u64) -> Result<Content, E> {
        Ok(Content::U64(value))
    }
    fn visit_i128<E>(self, value: i128) -> Result<Content, E> {
        Ok(Content::I128(value))
    }
    fn visit_u128<E>(self, value: u128) -> Result<Content, E> {
        Ok(Content::U128(value))
    }
    fn visit_f64<E>(self, value: f64) -> Result<Content, E> {
        Ok(Content::F64(value))
    }
    fn visit_str<E>(self, value: &str) -> Result<Content, E> {
        Ok(Content::Str(value.to_owned()))
    }
    fn visit_string<E>(self, value: String) -> Result<Content, E> {
        Ok(Content::Str(value))
    }
    fn visit_bytes<E>(self, value: &[u8]) -> Result<Content, E> {
        Ok(Content::Bytes(value.to_vec()))
    }
    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Content, E> {
        Ok(Content::Bytes(value))
    }
    fn visit_unit<E>(self) -> Result<Content, E> {
        Ok(Content::Null)
    }
    fn visit_none<E>(self) -> Result<Content, E> {
        Ok(Content::Null)
    }
    fn visit_some<D: Deserializer<'de>>(self, value: D) -> Result<Content, D::Error> {
        self.deserialize(value)
    }
    fn visit_newtype_struct<D: Deserializer<'de>>(self, value: D) -> Result<Content, D::Error> {
        self.deserialize(value)
    }
    fn visit_seq<A: de::SeqAccess<'de>>(self, mut items: A) -> Result<Content, A::Error> {
        let mut out = Vec::new();
        while let Some(item) = items.next_element_seed(self.child())? {
            out.push(item);
        }
        Ok(Content::Seq(out))
    }
    fn visit_map<A: de::MapAccess<'de>>(self, mut entries: A) -> Result<Content, A::Error> {
        let mut out = Vec::new();
        while let Some(key) = entries.next_key_seed(self.child())? {
            out.push((key, entries.next_value_seed(self.child())?));
        }
        Ok(Content::Map(out))
    }
}

impl<'de> Deserializer<'de> for Content {
    type Error = Refusal;

    fn deserialize_any<W: Visitor<'de>>(self, visitor: W) -> Result<W::Value, Refusal> {
        match self {
            Self::Null => visitor.visit_unit(),
            Self::Bool(value) => visitor.visit_bool(value),
            Self::I64(value) => visitor.visit_i64(value),
            Self::U64(value) => visitor.visit_u64(value),
            Self::I128(value) => visitor.visit_i128(value),
            Self::U128(value) => visitor.visit_u128(value),
            Self::F64(value) => visitor.visit_f64(value),
            Self::Str(value) => visitor.visit_string(value),
            Self::Bytes(value) => visitor.visit_byte_buf(value),
            Self::Seq(items) => {
                let mut items = SeqDeserializer::new(items.into_iter());
                let value = visitor.visit_seq(&mut items)?;
                items.end()?;
                Ok(value)
            }
            Self::Map(entries) => {
                let mut entries = MapDeserializer::new(entries.into_iter());
                let value = visitor.visit_map(&mut entries)?;
                entries.end()?;
                Ok(value)
            }
        }
    }

    fn deserialize_option<W: Visitor<'de>>(self, visitor: W) -> Result<W::Value, Refusal> {
        match self {
            Self::Null => visitor.visit_none(),
            other => visitor.visit_some(other),
        }
    }

    fn deserialize_newtype_struct<W: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: W,
    ) -> Result<W::Value, Refusal> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<W: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: W,
    ) -> Result<W::Value, Refusal> {
        match self {
            Self::Str(variant) => visitor.visit_enum(variant.into_deserializer()),
            Self::Map(entries) if entries.len() == 1 => {
                let entries = MapDeserializer::new(entries.into_iter());
                visitor.visit_enum(MapAccessDeserializer::new(entries))
            }
            other => Err(de::Error::invalid_type(
                other.kind(),
                &"a variant's name, or a map of one key",
            )),
        }
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf unit
        unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

impl IntoDeserializer<'_, Refusal> for Content {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

/// What serde says is wrong with a typed read: a field's name, what the target expected and the
/// kind of value found. `expected` is the target type's own description, not document text.
#[derive(Debug)]
struct Refusal(String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::error::Error for Refusal {}

impl de::Error for Refusal {
    // A `custom` message may hold anything, the refused value included.
    fn custom<T: fmt::Display>(_message: T) -> Self {
        Self("refused by its type".to_owned())
    }

    fn invalid_type(found: Unexpected<'_>, expected: &dyn de::Expected) -> Self {
        Self(format!("expected {expected}, found {}", kind(&found)))
    }

    fn invalid_value(found: Unexpected<'_>, expected: &dyn de::Expected) -> Self {
        Self(format!(
            "invalid value, expected {expected}, found {}",
            kind(&found)
        ))
    }

    fn invalid_length(length: usize, expected: &dyn de::Expected) -> Self {
        Self(format!("invalid length {length}, expected {expected}"))
    }

    // The variant's name is a value in the document, so it is not repeated.
    fn unknown_variant(_variant: &str, expected: &'static [&'static str]) -> Self {
        Self(format!("unknown variant{}", one_of(expected)))
    }

    fn unknown_field(field: &str, expected: &'static [&'static str]) -> Self {
        Self(format!(
            "unknown field `{}`{}",
            escape(field),
            one_of(expected)
        ))
    }

    fn missing_field(field: &'static str) -> Self {
        Self(format!("missing field `{field}`"))
    }

    fn duplicate_field(field: &'static str) -> Self {
        Self(format!("duplicate field `{field}`"))
    }
}

/// The kind of a value, without the value.
fn kind(found: &Unexpected<'_>) -> &'static str {
    match found {
        Unexpected::Bool(_) => "a boolean",
        Unexpected::Unsigned(_) | Unexpected::Signed(_) => "an integer",
        Unexpected::Float(_) => "a float",
        Unexpected::Char(_) => "a character",
        Unexpected::Str(_) => "a string",
        Unexpected::Bytes(_) => "bytes",
        Unexpected::Unit => "null",
        Unexpected::Option => "an optional value",
        Unexpected::NewtypeStruct => "a newtype struct",
        Unexpected::Seq => "a sequence",
        Unexpected::Map => "a map",
        Unexpected::Enum => "an enum",
        Unexpected::UnitVariant
        | Unexpected::NewtypeVariant
        | Unexpected::TupleVariant
        | Unexpected::StructVariant => "an enum variant",
        Unexpected::Other(_) => "another kind of value",
    }
}

fn one_of(expected: &[&str]) -> String {
    if expected.is_empty() {
        return ", there are none".to_owned();
    }
    let names: Vec<String> = expected.iter().map(|name| format!("`{name}`")).collect();
    format!(", expected one of {}", names.join(", "))
}

/// A key is the one document text an error repeats; a control character in it must not reach a
/// terminal or split a log line.
fn escape(key: &str) -> String {
    let mut escaped = String::with_capacity(key.len());
    for character in key.chars() {
        if character.is_control() {
            escaped.extend(character.escape_default());
        } else {
            escaped.push(character);
        }
    }
    escaped
}
