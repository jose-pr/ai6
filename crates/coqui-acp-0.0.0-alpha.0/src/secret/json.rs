//! JSON text that is hashed as sent is also read strictly: an object with the same key twice is refused, because the digest covers
//! both while a reader keeps one, and a relay could use the difference.

use super::error::Error;
use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use core::fmt;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

/// Walks a JSON value and fails on an object that repeats a key.
struct Unique;

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = Unique;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_str<E>(self, _: &str) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_unit<E>(self) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Unique, A::Error> {
        while seq.next_element::<Unique>()?.is_some() {}
        Ok(Unique)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
        let mut seen = BTreeSet::<String>::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key) {
                return Err(de::Error::custom("a key twice"));
            }
            map.next_value::<Unique>()?;
        }
        Ok(Unique)
    }
}

/// Reads JSON text, refusing a repeated key anywhere in it. `make` builds the refusal (`Error::Schema` or `Error::Malformed`), with
/// `what` as the reason when the text is not JSON (a lone surrogate escape included) and `twice` when it repeats a key.
pub(crate) fn strict(
    text: &str,
    make: fn(&'static str) -> Error,
    what: &'static str,
    twice: &'static str,
) -> Result<Value, Error> {
    match serde_json::from_str::<Unique>(text) {
        Ok(Unique) => {}
        Err(error) if error.to_string().starts_with("a key twice") => return Err(make(twice)),
        Err(_) => return Err(make(what)),
    }
    serde_json::from_str(text).map_err(|_| make(what))
}
