//! The three formats an AOS document may be in, each decoding to a guatiao [`Value`] for
//! [`anycfg`] to dispatch to. They are AOS's own rather than the configuration reader's:
//! AOS refuses a TOML date, a YAML tag, anchor, merge key or directive, and a lone surrogate
//! in JSON, types a YAML plain scalar by the YAML 1.2 core schema (`yes` is a string), and
//! tells a JSON `30.0` from `30`.
//!
//! A refusal's message starts with its AOS error class and a colon, and quotes nothing from
//! the document.

use guatiao::value::types::{List, Map, Number, Text};
use guatiao::{Alloc, Value};

use anycfg::CodecError;

/// A refusal of class `class`, with what was wrong and no text from the document.
pub(super) fn refusal(class: &str, detail: &str) -> CodecError {
    CodecError::new().message(format!("{class}: {detail}"))
}

fn number(text: &str) -> Result<Value, CodecError> {
    Number::new(text)
        .map(Value::from)
        .map_err(|_| refusal("parse-error", "a number"))
}

/// A float's text that stays a float when read back: `30.0`, never `30`.
fn float(value: f64) -> Result<Value, CodecError> {
    if !value.is_finite() {
        return Err(refusal("non-finite-float", "a float that is not finite"));
    }
    number(&format!("{value:?}"))
}

/// Builds a value from a stream of scalars, keys and containers, with no recursion.
struct Builder {
    stack: Vec<Frame>,
    root: Option<Value>,
}

enum Frame {
    List(List),
    Map(Map, Option<String>),
}

impl Builder {
    fn new() -> Self {
        Self {
            stack: Vec::new(),
            root: None,
        }
    }

    /// Whether the next value is a mapping's key.
    fn wants_key(&self) -> bool {
        matches!(self.stack.last(), Some(Frame::Map(_, None)))
    }

    fn key(&mut self, key: String) -> Result<(), CodecError> {
        if let Some(Frame::Map(map, slot)) = self.stack.last_mut() {
            if map.contains_key(&key) {
                return Err(refusal(
                    "duplicate-key",
                    "a key appears twice in one mapping",
                ));
            }
            *slot = Some(key);
        }
        Ok(())
    }

    fn value(&mut self, value: Value) -> Result<(), CodecError> {
        match self.stack.last_mut() {
            None => {
                if self.root.is_some() {
                    return Err(refusal("parse-error", "more than one document"));
                }
                self.root = Some(value);
            }
            Some(Frame::List(list)) => list.push(value),
            Some(Frame::Map(map, slot)) => {
                let key = slot.take().ok_or_else(|| {
                    refusal("non-string-key", "a mapping key that is not a string")
                })?;
                map.set(&key, value);
            }
        }
        Ok(())
    }

    fn open_list(&mut self) {
        self.stack.push(Frame::List(List::new_in(Alloc::rust())));
    }

    fn open_map(&mut self) {
        self.stack
            .push(Frame::Map(Map::new_in(Alloc::rust()), None));
    }

    fn close(&mut self) -> Result<(), CodecError> {
        let value = match self.stack.pop() {
            Some(Frame::List(list)) => Value::from(list),
            Some(Frame::Map(map, _)) => Value::from(map),
            None => return Err(refusal("parse-error", "a container closed twice")),
        };
        self.value(value)
    }
}

/// TOML 1.0, a date or time refused.
pub(super) fn toml(bytes: &[u8]) -> Result<Value, CodecError> {
    let text = std::str::from_utf8(bytes).map_err(|_| refusal("invalid-utf8", "not UTF-8"))?;
    let table = toml::from_str::<toml::Table>(text).map_err(|error| {
        let at = error.span().map_or_else(CodecError::new, |span| {
            CodecError::at_offset(bytes, span.start)
        });
        at.message(format!("parse-error: {}", error.message()))
    })?;
    toml_table(table)
}

fn toml_table(table: toml::Table) -> Result<Value, CodecError> {
    let mut map = Map::new_in(Alloc::rust());
    for (key, item) in table {
        map.set(&key, toml_value(item)?);
    }
    Ok(Value::from(map))
}

fn toml_value(item: toml::Value) -> Result<Value, CodecError> {
    Ok(match item {
        toml::Value::String(text) => Value::from(Text::new(&text)),
        toml::Value::Integer(integer) => Value::from(integer),
        toml::Value::Float(value) => float(value)?,
        toml::Value::Boolean(flag) => Value::from(flag),
        toml::Value::Datetime(_) => {
            return Err(refusal("datetime-value", "a date or time has no AOS type"));
        }
        toml::Value::Array(items) => {
            let mut list = List::new_in(Alloc::rust());
            for item in items {
                list.push(toml_value(item)?);
            }
            Value::from(list)
        }
        toml::Value::Table(table) => toml_table(table)?,
    })
}

/// RFC 8259 JSON: a duplicate key, a lone surrogate, `NaN`, `Infinity` and comments refused,
/// and a number kept as the text it was written as.
pub(super) fn json(bytes: &[u8]) -> Result<Value, CodecError> {
    let text = std::str::from_utf8(bytes).map_err(|_| refusal("invalid-utf8", "not UTF-8"))?;
    JsonReader {
        bytes: text.as_bytes(),
        at: 0,
    }
    .document()
    .map_err(|(error, at)| {
        let mut located = CodecError::at_offset(bytes, at);
        located.message = error.message;
        located
    })
}

struct JsonReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

type JsonResult<T> = Result<T, (CodecError, usize)>;

impl JsonReader<'_> {
    fn fail<T>(&self, class: &str, detail: &str) -> JsonResult<T> {
        Err((refusal(class, detail), self.at))
    }

    fn space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.at += 1;
        }
    }

    fn document(mut self) -> JsonResult<Value> {
        let mut builder = Builder::new();
        // What may come next inside each open container: a value, or a separator.
        let mut expect_value = true;
        loop {
            self.space();
            let Some(&byte) = self.bytes.get(self.at) else {
                return self.fail("parse-error", "the document ends early");
            };
            if expect_value {
                let is_key = builder.wants_key();
                match byte {
                    b'{' if !is_key => {
                        self.at += 1;
                        builder.open_map();
                        self.space();
                        if self.bytes.get(self.at) == Some(&b'}') {
                            self.at += 1;
                            builder.close().map_err(|error| (error, self.at))?;
                            expect_value = false;
                        }
                        continue;
                    }
                    b'[' if !is_key => {
                        self.at += 1;
                        builder.open_list();
                        self.space();
                        if self.bytes.get(self.at) == Some(&b']') {
                            self.at += 1;
                            builder.close().map_err(|error| (error, self.at))?;
                            expect_value = false;
                        }
                        continue;
                    }
                    b'"' => {
                        let text = self.string()?;
                        if is_key {
                            builder.key(text).map_err(|error| (error, self.at))?;
                            self.space();
                            if self.bytes.get(self.at) != Some(&b':') {
                                return self.fail("parse-error", "a key without a colon");
                            }
                            self.at += 1;
                            continue;
                        }
                        builder
                            .value(Value::from(Text::new(&text)))
                            .map_err(|error| (error, self.at))?;
                    }
                    _ if is_key => return self.fail("parse-error", "a key that is not a string"),
                    b't' => self.word(b"true", &mut builder, Value::from(true))?,
                    b'f' => self.word(b"false", &mut builder, Value::from(false))?,
                    b'n' => self.word(b"null", &mut builder, Value::null())?,
                    b'-' | b'0'..=b'9' => {
                        let value = self.number()?;
                        builder.value(value).map_err(|error| (error, self.at))?;
                    }
                    _ => return self.fail("parse-error", "not a JSON value"),
                }
                expect_value = false;
            } else {
                match (byte, builder.stack.last()) {
                    (b',', Some(_)) => {
                        self.at += 1;
                        expect_value = true;
                    }
                    (b'}', Some(Frame::Map(..))) | (b']', Some(Frame::List(_))) => {
                        self.at += 1;
                        builder.close().map_err(|error| (error, self.at))?;
                    }
                    _ => return self.fail("parse-error", "an unexpected character"),
                }
            }
            if builder.stack.is_empty() && builder.root.is_some() {
                self.space();
                if self.at != self.bytes.len() {
                    return self.fail("parse-error", "text after the document");
                }
                return Ok(builder.root.take().unwrap_or_else(Value::null));
            }
        }
    }

    fn word(&mut self, word: &[u8], builder: &mut Builder, value: Value) -> JsonResult<()> {
        if !self.bytes[self.at..].starts_with(word) {
            return self.fail("parse-error", "not a JSON value");
        }
        self.at += word.len();
        builder.value(value).map_err(|error| (error, self.at))
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> JsonResult<Value> {
        let start = self.at;
        if self.bytes[self.at] == b'-' {
            self.at += 1;
        }
        let leading_zero = self.bytes.get(self.at) == Some(&b'0');
        let whole = self.digits();
        if whole == 0 || (leading_zero && whole > 1) {
            return self.fail("parse-error", "a malformed number");
        }
        let mut fraction = false;
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            fraction = true;
            if self.digits() == 0 {
                return self.fail("parse-error", "a malformed number");
            }
        }
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            fraction = true;
            if matches!(self.bytes.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return self.fail("parse-error", "a malformed number");
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).unwrap_or_default();
        if fraction {
            let value: f64 = text.parse().unwrap_or(f64::NAN);
            if !value.is_finite() {
                return self.fail("non-finite-float", "a number too large for a float");
            }
        } else if text.parse::<i64>().is_err() {
            return self.fail(
                "integer-range",
                "an integer outside the signed 64-bit range",
            );
        }
        number(text).map_err(|error| (error, start))
    }

    fn hex4(&mut self) -> JsonResult<u32> {
        let digits = self.bytes.get(self.at..self.at + 4);
        let value = digits
            .and_then(|digits| std::str::from_utf8(digits).ok())
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .and_then(|digits| u32::from_str_radix(digits, 16).ok());
        match value {
            Some(value) => {
                self.at += 4;
                Ok(value)
            }
            None => self.fail("parse-error", "a malformed \\u escape"),
        }
    }

    fn string(&mut self) -> JsonResult<String> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let Some(&byte) = self.bytes.get(self.at) else {
                return self.fail("parse-error", "a string that does not end");
            };
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.at += 1;
                    let Some(&escape) = self.bytes.get(self.at) else {
                        return self.fail("parse-error", "a string that does not end");
                    };
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&first) {
                                if !self.bytes[self.at..].starts_with(b"\\u") {
                                    return self.fail("invalid-unicode", "a lone surrogate");
                                }
                                self.at += 2;
                                let second = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&second) {
                                    return self.fail("invalid-unicode", "a lone surrogate");
                                }
                                0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                            } else if (0xdc00..0xe000).contains(&first) {
                                return self.fail("invalid-unicode", "a lone surrogate");
                            } else {
                                first
                            };
                            match char::from_u32(code) {
                                Some(c) => out.push(c),
                                None => return self.fail("invalid-unicode", "not a character"),
                            }
                        }
                        _ => return self.fail("parse-error", "an unknown escape"),
                    }
                }
                0..=0x1f => return self.fail("parse-error", "a control character in a string"),
                _ => {
                    // The text is UTF-8, so the run up to the next quote, backslash or control
                    // character is whole characters.
                    let start = self.at;
                    while self
                        .bytes
                        .get(self.at)
                        .is_some_and(|byte| !matches!(byte, b'"' | b'\\' | 0..=0x1f))
                    {
                        self.at += 1;
                    }
                    out.push_str(
                        std::str::from_utf8(&self.bytes[start..self.at]).unwrap_or_default(),
                    );
                }
            }
        }
    }
}

/// One YAML 1.2 document under the core schema, built from parser events so that no tag,
/// anchor, alias, merge key or directive is applied: each is refused.
pub(super) fn yaml(bytes: &[u8]) -> Result<Value, CodecError> {
    use granit_parser::{Event, Parser, ScalarStyle, Scanner, StrInput, TokenType};

    let text = std::str::from_utf8(bytes).map_err(|_| refusal("invalid-utf8", "not UTF-8"))?;
    if text.contains(['\u{85}', '\u{2028}', '\u{2029}']) {
        return Err(refusal(
            "parse-error",
            "a Unicode line break YAML 1.1 and 1.2 read differently",
        ));
    }
    let located = |error: &granit_parser::ScanError, class: &str| {
        let marker = error.marker();
        CodecError::new()
            .line(marker.line())
            .column(marker.col() + 1)
            .message(format!("{class}: not valid YAML"))
    };
    for token in Scanner::new(StrInput::new(text)) {
        match token {
            Ok(token) => {
                if matches!(
                    token.token_type(),
                    TokenType::VersionDirective(..) | TokenType::TagDirective(..)
                ) {
                    return Err(refusal("yaml-directive", "a %YAML or %TAG directive"));
                }
            }
            Err(error) => return Err(located(&error, "parse-error")),
        }
    }
    let mut builder = Builder::new();
    let mut documents = 0usize;
    for event in Parser::new_from_str(text) {
        let (event, _) = event.map_err(|error| located(&error, "parse-error"))?;
        let sequence = matches!(event, Event::SequenceStart(..));
        match event {
            Event::DocumentStart(_, version) => {
                if version.is_some() {
                    return Err(refusal("yaml-directive", "a %YAML directive"));
                }
                documents += 1;
                if documents > 1 {
                    return Err(refusal("parse-error", "more than one document"));
                }
            }
            Event::Alias(_) => return Err(refusal("yaml-anchor", "an alias")),
            Event::Scalar(value, style, anchor, tag) => {
                if anchor != 0 {
                    return Err(refusal("yaml-anchor", "an anchor"));
                }
                if tag.is_some() {
                    return Err(refusal("yaml-tag", "a tag"));
                }
                let plain = matches!(style, ScalarStyle::Plain);
                if builder.wants_key() {
                    if plain {
                        let typed = core_scalar(&value)?;
                        if !matches!(typed, Scalar::Text) {
                            return Err(refusal(
                                "non-string-key",
                                "a mapping key that is not a string",
                            ));
                        }
                    }
                    if value == "<<" {
                        return Err(refusal("yaml-merge-key", "a merge key"));
                    }
                    builder.key(value.into_owned())?;
                } else {
                    let item = if plain {
                        match core_scalar(&value)? {
                            Scalar::Text => Value::from(Text::new(&value)),
                            Scalar::Bool(flag) => Value::from(flag),
                            Scalar::Integer(integer) => Value::from(integer),
                            Scalar::Float(value) => float(value)?,
                        }
                    } else {
                        Value::from(Text::new(&value))
                    };
                    builder.value(item)?;
                }
            }
            Event::SequenceStart(_, anchor, tag) | Event::MappingStart(_, anchor, tag) => {
                if anchor != 0 {
                    return Err(refusal("yaml-anchor", "an anchor"));
                }
                if tag.is_some() {
                    return Err(refusal("yaml-tag", "a tag"));
                }
                if builder.wants_key() {
                    return Err(refusal(
                        "non-string-key",
                        "a mapping key that is not a string",
                    ));
                }
                if sequence {
                    builder.open_list();
                } else {
                    builder.open_map();
                }
            }
            Event::SequenceEnd | Event::MappingEnd => builder.close()?,
            _ => {}
        }
    }
    Ok(builder
        .root
        .unwrap_or_else(|| Value::from(Map::new_in(Alloc::rust()))))
}

enum Scalar {
    Text,
    Bool(bool),
    Integer(i64),
    Float(f64),
}

/// A plain scalar's type under the YAML 1.2 core schema.
fn core_scalar(value: &str) -> Result<Scalar, CodecError> {
    if matches!(value, "" | "~" | "null" | "Null" | "NULL") {
        return Err(refusal("null-value", "a null"));
    }
    if matches!(value, "true" | "True" | "TRUE") {
        return Ok(Scalar::Bool(true));
    }
    if matches!(value, "false" | "False" | "FALSE") {
        return Ok(Scalar::Bool(false));
    }
    let integer = |digits: &str, radix: u32, negative: bool| {
        i128::from_str_radix(digits, radix)
            .ok()
            .map(|magnitude| if negative { -magnitude } else { magnitude })
            .and_then(|value| i64::try_from(value).ok())
            .map(Scalar::Integer)
            .ok_or_else(|| {
                refusal(
                    "integer-range",
                    "an integer outside the signed 64-bit range",
                )
            })
    };
    let unsigned = value.strip_prefix(['-', '+']).unwrap_or(value);
    if !unsigned.is_empty() && unsigned.bytes().all(|byte| byte.is_ascii_digit()) {
        return integer(unsigned, 10, value.starts_with('-'));
    }
    if let Some(octal) = value.strip_prefix("0o")
        && !octal.is_empty()
        && octal.bytes().all(|byte| (b'0'..=b'7').contains(&byte))
    {
        return integer(octal, 8, false);
    }
    if let Some(hex) = value.strip_prefix("0x")
        && !hex.is_empty()
        && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return integer(hex, 16, false);
    }
    if is_core_float(unsigned) {
        let parsed: f64 = value.parse().unwrap_or(f64::NAN);
        return Ok(Scalar::Float(parsed));
    }
    let infinity = matches!(unsigned, ".inf" | ".Inf" | ".INF");
    if infinity || matches!(value, ".nan" | ".NaN" | ".NAN") {
        return Err(refusal("non-finite-float", "a float that is not finite"));
    }
    Ok(Scalar::Text)
}

/// `(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?`, the sign already taken off.
fn is_core_float(text: &str) -> bool {
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(at) => (&text[..at], Some(&text[at + 1..])),
        None => (text, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        Some(("", fraction)) => digits(fraction),
        Some((whole, fraction)) => {
            digits(whole) && fraction.bytes().all(|byte| byte.is_ascii_digit())
        }
        None => digits(mantissa),
    };
    let exponent_ok = exponent
        .is_none_or(|exponent| digits(exponent.strip_prefix(['+', '-']).unwrap_or(exponent)));
    mantissa_ok && exponent_ok
}
