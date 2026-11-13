//! Checking that a parsed line has the shape the format gives it, naming a place by pointer and
//! never quoting the document.

use serde_json::{Map, Value};

use crate::HistoryError;

/// An object of a line being checked.
pub(crate) struct Fields<'a> {
    map: &'a Map<String, Value>,
    line: u64,
    pointer: String,
}

impl<'a> Fields<'a> {
    /// The line itself, which must be an object.
    pub(crate) fn root(value: &'a Value, line: u64) -> Result<Self, HistoryError> {
        Self::of(value, line, String::new())
    }

    fn of(value: &'a Value, line: u64, pointer: String) -> Result<Self, HistoryError> {
        match value {
            Value::Object(map) => Ok(Self { map, line, pointer }),
            _ => Err(HistoryError::Shape {
                line,
                pointer,
                expected: "an object",
            }),
        }
    }

    fn at(&self, key: &str) -> String {
        format!("{}/{}", self.pointer, key)
    }

    pub(crate) fn shape(&self, key: &str, expected: &'static str) -> HistoryError {
        HistoryError::Shape {
            line: self.line,
            pointer: self.at(key),
            expected,
        }
    }

    /// Refuses a key outside `allowed`.
    pub(crate) fn only(&self, allowed: &[&str]) -> Result<(), HistoryError> {
        if self.map.keys().all(|key| allowed.contains(&key.as_str())) {
            Ok(())
        } else {
            Err(HistoryError::UnknownField {
                line: self.line,
                pointer: self.pointer.clone(),
            })
        }
    }

    pub(crate) fn has(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    pub(crate) fn raw(&self, key: &str) -> Option<&'a Value> {
        self.map.get(key)
    }

    /// A string field of at most `max` bytes and at least one.
    pub(crate) fn string(
        &self,
        key: &str,
        max: usize,
        expected: &'static str,
    ) -> Result<Option<&'a str>, HistoryError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::String(text)) if !text.is_empty() && text.len() <= max => Ok(Some(text)),
            Some(_) => Err(self.shape(key, expected)),
        }
    }

    /// A string field like [`string`](Self::string) that holds no control character: NUL, the
    /// C0 and C1 controls and DEL. For text a person or a program will print or use as a name.
    pub(crate) fn plain(
        &self,
        key: &str,
        max: usize,
        expected: &'static str,
    ) -> Result<Option<&'a str>, HistoryError> {
        match self.string(key, max, expected)? {
            Some(text) if text.chars().any(char::is_control) => Err(self.shape(key, expected)),
            found => Ok(found),
        }
    }

    pub(crate) fn require_string(
        &self,
        key: &str,
        max: usize,
        expected: &'static str,
    ) -> Result<&'a str, HistoryError> {
        self.string(key, max, expected)?
            .ok_or_else(|| self.shape(key, expected))
    }

    pub(crate) fn unsigned(&self, key: &str) -> Result<Option<u64>, HistoryError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::Number(n)) => n
                .as_u64()
                .map(Some)
                .ok_or_else(|| self.shape(key, "an unsigned integer")),
            Some(_) => Err(self.shape(key, "an unsigned integer")),
        }
    }

    /// An object field, as fields of its own.
    pub(crate) fn object(&self, key: &str) -> Result<Option<Fields<'a>>, HistoryError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(value) => Self::of(value, self.line, self.at(key))
                .map(Some)
                .map_err(|_| self.shape(key, "an object")),
        }
    }

    pub(crate) fn require_object(&self, key: &str) -> Result<Fields<'a>, HistoryError> {
        self.object(key)?
            .ok_or_else(|| self.shape(key, "an object"))
    }

    /// An array field whose every item is an object holding a non-empty string at `tag`.
    pub(crate) fn tagged_array(
        &self,
        key: &str,
        tag: &str,
        at_least_one: bool,
    ) -> Result<&'a Vec<Value>, HistoryError> {
        let Some(Value::Array(items)) = self.map.get(key) else {
            return Err(self.shape(key, "an array"));
        };
        if at_least_one && items.is_empty() {
            return Err(self.shape(key, "a non-empty array"));
        }
        for (index, item) in items.iter().enumerate() {
            let at = format!("{}/{index}", self.at(key));
            let fields = Self::of(item, self.line, at.clone())?;
            if fields.string(tag, 256, "a string")?.is_none() {
                return Err(HistoryError::Shape {
                    line: self.line,
                    pointer: format!("{at}/{tag}"),
                    expected: "a non-empty string",
                });
            }
        }
        Ok(items)
    }
}

/// An RFC 3339 timestamp, checked for its shape alone: `2026-11-09T12:30:00Z`, with optional
/// fraction and either `Z` or a numeric offset.
pub(crate) fn is_timestamp(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() < 20 || b.len() > 40 {
        return false;
    }
    let digits = |from: usize, to: usize| b[from..to].iter().all(u8::is_ascii_digit);
    let number = |from: usize, to: usize| -> u32 { text[from..to].parse().unwrap_or(u32::MAX) };
    if !(digits(0, 4)
        && b[4] == b'-'
        && digits(5, 7)
        && b[7] == b'-'
        && digits(8, 10)
        && (b[10] == b'T' || b[10] == b't')
        && digits(11, 13)
        && b[13] == b':'
        && digits(14, 16)
        && b[16] == b':'
        && digits(17, 19))
    {
        return false;
    }
    if !(1..=12).contains(&number(5, 7))
        || !(1..=31).contains(&number(8, 10))
        || number(11, 13) > 23
        || number(14, 16) > 59
        || number(17, 19) > 60
    {
        return false;
    }
    let mut rest = &text[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let n = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return false;
        }
        rest = &fraction[n..];
    }
    match rest.as_bytes() {
        [b'Z' | b'z'] => true,
        [b'+' | b'-', h1, h2, b':', m1, m2] => {
            [h1, h2, m1, m2].iter().all(|d| d.is_ascii_digit())
                && number(text.len() - 5, text.len() - 3) < 24
                && number(text.len() - 2, text.len()) < 60
        }
        _ => false,
    }
}

/// Whether `text` is an absolute path in the Unix style (`/dir`) or the Windows style (`C:\dir`,
/// `C:/dir`, `\\server\share`, `\\?\C:\dir`).
pub(crate) fn is_absolute_path(text: &str) -> bool {
    let b = text.as_bytes();
    match b {
        [b'/', ..] | [b'\\', b'\\', ..] => true,
        [drive, b':', b'\\' | b'/', ..] => drive.is_ascii_alphabetic(),
        _ => false,
    }
}

/// A canonical session UUID: 36 characters, lower-case hexadecimal, hyphens at 8, 13, 18 and 23.
pub(crate) fn is_uuid(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
        })
}

/// A name for a harness: lower-case letters, digits and `.`, `_`, `-`, `:`, 1 to 64 bytes.
pub(crate) fn is_harness_name(text: &str) -> bool {
    let b = text.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b".-_:".contains(c))
}
