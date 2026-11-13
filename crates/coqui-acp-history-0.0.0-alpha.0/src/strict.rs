//! Reading one line of JSON strictly: no repeated keys, and bounds on nesting and on the number of
//! values that are checked while the line is built, so a line cannot cost more than its bounds.

use core::cell::Cell;
use core::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::HistoryError;
use crate::settings::{Bounds, PARSER_DEPTH};

/// The message of the error a repeated key raises inside the parser.
const DUPLICATE: &str = "duplicate key";
/// The message of the error a line nested past its limit raises inside the parser.
const TOO_DEEP: &str = "nesting limit";
/// The message of the error a line with too many values raises inside the parser.
const TOO_MANY: &str = "value limit";

/// What a parse has counted, and the limits it holds the line to.
struct Budget {
    values: Cell<u64>,
    max_values: u64,
    depth: Cell<u64>,
    max_depth: u64,
}

impl Budget {
    /// Counts one value or key.
    fn value<E: de::Error>(&self) -> Result<(), E> {
        let seen = self.values.get() + 1;
        self.values.set(seen);
        if seen > self.max_values {
            return Err(E::custom(TOO_MANY));
        }
        Ok(())
    }

    /// Enters an array or an object.
    fn enter<E: de::Error>(&self) -> Result<(), E> {
        self.value()?;
        let level = self.depth.get() + 1;
        self.depth.set(level);
        if level > self.max_depth {
            return Err(E::custom(TOO_DEEP));
        }
        Ok(())
    }

    fn leave(&self) {
        self.depth.set(self.depth.get() - 1);
    }
}

/// A JSON value that refuses an object with a repeated key and stays within its budget.
struct Strict<'a>(&'a Budget);

impl<'de> DeserializeSeed<'de> for Strict<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Strict<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        self.0.value()?;
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        self.0.value()?;
        Ok(Value::Number(v.into()))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        self.0.value()?;
        Ok(Value::Number(v.into()))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        self.0.value()?;
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("number out of range"))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        self.0.value()?;
        Ok(Value::String(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        self.0.value()?;
        Ok(Value::String(v))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        self.0.value()?;
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        self.0.enter()?;
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(Strict(self.0))? {
            items.push(item);
        }
        items.shrink_to_fit();
        self.0.leave();
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        self.0.enter()?;
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            self.0.value()?;
            let value = map.next_value_seed(Strict(self.0))?;
            if object.insert(key, value).is_some() {
                return Err(de::Error::custom(DUPLICATE));
            }
        }
        self.0.leave();
        Ok(Value::Object(object))
    }
}

/// Parses `text` as one JSON value held to `bounds` (nesting and values), with no object
/// repeating a key. The line is the number the error names.
pub(crate) fn parse(text: &str, line: u64, bounds: &Bounds) -> Result<Value, HistoryError> {
    let budget = Budget {
        values: Cell::new(0),
        max_values: bounds.max_values,
        depth: Cell::new(0),
        max_depth: bounds.depth_limit(),
    };
    let mut parser = serde_json::Deserializer::from_str(text);
    let parsed = Strict(&budget)
        .deserialize(&mut parser)
        .and_then(|value| parser.end().map(|()| value));
    parsed.map_err(|error| {
        let message = error.to_string();
        if message.starts_with(DUPLICATE) {
            HistoryError::DuplicateKey { line }
        } else if message.starts_with(TOO_MANY) {
            HistoryError::TooManyValues {
                line,
                limit: bounds.max_values,
            }
        } else if message.starts_with(TOO_DEEP) || message.starts_with("recursion limit") {
            HistoryError::too_deep(line, bounds.max_depth)
        } else {
            HistoryError::Syntax {
                line,
                column: error.column() as u64,
            }
        }
    })
}

/// The nesting of `value`: a scalar is 0, a container one more than its deepest child. Walks with
/// an explicit stack, since the depth is what is in question.
pub(crate) fn depth(value: &Value) -> u64 {
    let mut deepest = 0;
    let mut stack = vec![(value, 0_u64)];
    while let Some((value, level)) = stack.pop() {
        match value {
            Value::Array(items) => {
                deepest = deepest.max(level + 1);
                stack.extend(items.iter().map(|item| (item, level + 1)));
            }
            Value::Object(map) => {
                deepest = deepest.max(level + 1);
                stack.extend(map.values().map(|item| (item, level + 1)));
            }
            _ => {}
        }
    }
    deepest
}

/// Whether `value` nests deeper than the parser reads back.
pub(crate) fn beyond_parser(value: &Value) -> bool {
    depth(value) > PARSER_DEPTH
}
