//! The three formats an AOS document may be in, each decoded to the data model. They are AOS's own
//! rather than a general reader's: AOS refuses a TOML date, a YAML tag, anchor, merge key or
//! directive, and a lone surrogate in JSON, types a YAML plain scalar by the YAML 1.2 core schema
//! (`yes` is a string), and tells a JSON `30.0` from `30`.
//!
//! A refusal names its class and where, and quotes nothing from the document.

use serde_json::{Map, Number, Value};

use crate::error::{Class, Error};
use crate::limits::Limit;

fn refusal(class: Class, detail: &str) -> Error {
    Error::new(class, detail)
}

/// The data model's number for a number's text: an integer in the signed 64-bit range, or a
/// finite float, which keeps the `.0` of `30.0`.
fn number(text: &str) -> Result<Value, Error> {
    if text.contains(['.', 'e', 'E']) {
        let parsed: f64 = text.parse().unwrap_or(f64::NAN);
        float(parsed)
    } else {
        text.parse::<i64>().map(Value::from).map_err(|_| {
            refusal(
                Class::IntegerRange,
                "an integer outside the signed 64-bit range",
            )
        })
    }
}

fn float(value: f64) -> Result<Value, Error> {
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| refusal(Class::NonFiniteFloat, "a float that is not finite"))
}

/// Builds a value from a stream of scalars, keys and containers, with no recursion, refusing a
/// nesting past `depth` as it opens.
struct Builder {
    stack: Vec<Frame>,
    root: Option<Value>,
    depth: Limit,
    nulls: bool,
}

enum Frame {
    List(Vec<Value>),
    Map(Map<String, Value>, Option<String>),
}

impl Builder {
    fn new(depth: Limit, nulls: bool) -> Self {
        Self {
            stack: Vec::new(),
            root: None,
            depth,
            nulls,
        }
    }

    /// Whether the next value is a mapping's key.
    fn wants_key(&self) -> bool {
        matches!(self.stack.last(), Some(Frame::Map(_, None)))
    }

    fn key(&mut self, key: String) -> Result<(), Error> {
        if let Some(Frame::Map(map, slot)) = self.stack.last_mut() {
            if map.contains_key(&key) {
                return Err(refusal(
                    Class::DuplicateKey,
                    "a key appears twice in one mapping",
                ));
            }
            *slot = Some(key);
        }
        Ok(())
    }

    fn value(&mut self, value: Value) -> Result<(), Error> {
        if value.is_null() && !self.nulls {
            return Err(refusal(Class::NullValue, "a null"));
        }
        match self.stack.last_mut() {
            None => {
                if self.root.is_some() {
                    return Err(refusal(Class::ParseError, "more than one document"));
                }
                self.root = Some(value);
            }
            Some(Frame::List(list)) => list.push(value),
            Some(Frame::Map(map, slot)) => {
                let key = slot.take().ok_or_else(|| {
                    refusal(Class::NonStringKey, "a mapping key that is not a string")
                })?;
                map.insert(key, value);
            }
        }
        Ok(())
    }

    fn open(&mut self, frame: Frame) -> Result<(), Error> {
        if !self.depth.allows(self.stack.len() + 1) {
            return Err(refusal(
                Class::TooDeep,
                &format!("nested deeper than {}", self.depth),
            ));
        }
        self.stack.push(frame);
        Ok(())
    }

    fn open_list(&mut self) -> Result<(), Error> {
        self.open(Frame::List(Vec::new()))
    }

    fn open_map(&mut self) -> Result<(), Error> {
        self.open(Frame::Map(Map::new(), None))
    }

    fn close(&mut self) -> Result<(), Error> {
        let value = match self.stack.pop() {
            Some(Frame::List(list)) => Value::Array(list),
            Some(Frame::Map(map, _)) => Value::Object(map),
            None => return Err(refusal(Class::ParseError, "a container closed twice")),
        };
        self.value(value)
    }
}

/// The one-based line and column of byte `offset` in `bytes`.
pub(crate) fn locate(bytes: &[u8], offset: usize) -> (usize, usize) {
    let before = &bytes[..offset.min(bytes.len())];
    let line = before.iter().filter(|byte| **byte == b'\n').count() + 1;
    let start = before
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1);
    (
        line,
        String::from_utf8_lossy(&before[start..]).chars().count() + 1,
    )
}

/// The deepest run of open `[` and `{` outside quotes and comments, which a parser that stops
/// early reports only as its own failure.
fn nesting(text: &str) -> u64 {
    let (mut open, mut deepest) = (0u64, 0u64);
    let mut quote: Option<char> = None;
    let mut comment = false;
    let mut escaped = false;
    for c in text.chars() {
        if comment {
            comment = c != '\n';
            continue;
        }
        if let Some(mark) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' && mark == '"' {
                escaped = true;
            } else if c == mark || c == '\n' {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '#' => comment = true,
            '[' | '{' => {
                open += 1;
                deepest = deepest.max(open);
            }
            ']' | '}' => open = open.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

fn at_offset(error: Error, bytes: &[u8], offset: usize) -> Error {
    let (line, column) = locate(bytes, offset);
    error.at(line, column)
}

/// TOML 1.0, a date or time refused.
pub(crate) fn toml(bytes: &[u8], depth: Limit) -> Result<Value, Error> {
    let text = core::str::from_utf8(bytes).map_err(|_| refusal(Class::InvalidUtf8, "not UTF-8"))?;
    let table = ::toml::from_str::<::toml::Table>(text).map_err(|error| {
        let message = error.message().to_owned();
        let class = if message.contains("overflow") {
            Class::IntegerRange
        } else if message.contains("recurs") || !depth.allows(nesting(text)) {
            Class::TooDeep
        } else {
            Class::ParseError
        };
        let why = match class {
            Class::IntegerRange => "an integer outside the signed 64-bit range",
            Class::TooDeep => "nested past what the reader follows",
            _ => "not valid TOML",
        };
        let refused = refusal(class, why);
        match error.span() {
            Some(span) => at_offset(refused, bytes, span.start),
            None => refused,
        }
    })?;
    toml_value(::toml::Value::Table(table), 1, depth)
}

fn toml_value(item: ::toml::Value, level: usize, depth: Limit) -> Result<Value, Error> {
    let container = matches!(item, ::toml::Value::Array(_) | ::toml::Value::Table(_));
    if container && !depth.allows(level) {
        return Err(refusal(
            Class::TooDeep,
            &format!("nested deeper than {depth}"),
        ));
    }
    Ok(match item {
        ::toml::Value::String(text) => Value::String(text),
        ::toml::Value::Integer(integer) => Value::from(integer),
        ::toml::Value::Float(value) => float(value)?,
        ::toml::Value::Boolean(flag) => Value::Bool(flag),
        ::toml::Value::Datetime(_) => {
            return Err(refusal(
                Class::DatetimeValue,
                "a date or time has no AOS type",
            ));
        }
        ::toml::Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| toml_value(item, level + 1, depth))
                .collect::<Result<_, _>>()?,
        ),
        ::toml::Value::Table(table) => {
            let mut map = Map::new();
            for (key, item) in table {
                map.insert(key, toml_value(item, level + 1, depth)?);
            }
            Value::Object(map)
        }
    })
}

/// RFC 8259 JSON: a duplicate key, a lone surrogate, `NaN`, `Infinity` and comments refused,
/// and a number kept as the text it was written as.
pub(crate) fn json(bytes: &[u8], depth: Limit, nulls: bool) -> Result<Value, Error> {
    let text = core::str::from_utf8(bytes).map_err(|_| refusal(Class::InvalidUtf8, "not UTF-8"))?;
    JsonReader {
        bytes: text.as_bytes(),
        at: 0,
        depth,
        nulls,
    }
    .document()
    .map_err(|(error, at)| at_offset(error, bytes, at))
}

struct JsonReader<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: Limit,
    nulls: bool,
}

type JsonResult<T> = Result<T, (Error, usize)>;

impl JsonReader<'_> {
    fn fail<T>(&self, class: Class, detail: &str) -> JsonResult<T> {
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
        let mut builder = Builder::new(self.depth, self.nulls);
        // What may come next inside each open container: a value, or a separator.
        let mut expect_value = true;
        loop {
            self.space();
            let Some(&byte) = self.bytes.get(self.at) else {
                return self.fail(Class::ParseError, "the document ends early");
            };
            if expect_value {
                let is_key = builder.wants_key();
                match byte {
                    b'{' if !is_key => {
                        self.at += 1;
                        builder.open_map().map_err(|error| (error, self.at))?;
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
                        builder.open_list().map_err(|error| (error, self.at))?;
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
                                return self.fail(Class::ParseError, "a key without a colon");
                            }
                            self.at += 1;
                            continue;
                        }
                        builder
                            .value(Value::String(text))
                            .map_err(|error| (error, self.at))?;
                    }
                    _ if is_key => {
                        return self.fail(Class::ParseError, "a key that is not a string");
                    }
                    b't' => self.word(b"true", &mut builder, Value::Bool(true))?,
                    b'f' => self.word(b"false", &mut builder, Value::Bool(false))?,
                    b'n' => self.word(b"null", &mut builder, Value::Null)?,
                    b'-' | b'0'..=b'9' => {
                        let value = self.number()?;
                        builder.value(value).map_err(|error| (error, self.at))?;
                    }
                    _ => return self.fail(Class::ParseError, "not a JSON value"),
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
                    _ => return self.fail(Class::ParseError, "an unexpected character"),
                }
            }
            if builder.stack.is_empty() && builder.root.is_some() {
                self.space();
                if self.at != self.bytes.len() {
                    return self.fail(Class::ParseError, "text after the document");
                }
                return Ok(builder.root.take().unwrap_or(Value::Null));
            }
        }
    }

    fn word(&mut self, word: &[u8], builder: &mut Builder, value: Value) -> JsonResult<()> {
        if !self.bytes[self.at..].starts_with(word) {
            return self.fail(Class::ParseError, "not a JSON value");
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
            return self.fail(Class::ParseError, "a malformed number");
        }
        let mut fraction = false;
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            fraction = true;
            if self.digits() == 0 {
                return self.fail(Class::ParseError, "a malformed number");
            }
        }
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            fraction = true;
            if matches!(self.bytes.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return self.fail(Class::ParseError, "a malformed number");
            }
        }
        let text = core::str::from_utf8(&self.bytes[start..self.at]).unwrap_or_default();
        if fraction {
            let value: f64 = text.parse().unwrap_or(f64::NAN);
            if !value.is_finite() {
                return self.fail(Class::NonFiniteFloat, "a number too large for a float");
            }
        } else if text.parse::<i64>().is_err() {
            return self.fail(
                Class::IntegerRange,
                "an integer outside the signed 64-bit range",
            );
        }
        number(text).map_err(|error| (error, start))
    }

    fn hex4(&mut self) -> JsonResult<u32> {
        let digits = self.bytes.get(self.at..self.at + 4);
        let value = digits
            .and_then(|digits| core::str::from_utf8(digits).ok())
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .and_then(|digits| u32::from_str_radix(digits, 16).ok());
        match value {
            Some(value) => {
                self.at += 4;
                Ok(value)
            }
            None => self.fail(Class::ParseError, "a malformed \\u escape"),
        }
    }

    fn string(&mut self) -> JsonResult<String> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let Some(&byte) = self.bytes.get(self.at) else {
                return self.fail(Class::ParseError, "a string that does not end");
            };
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.at += 1;
                    let Some(&escape) = self.bytes.get(self.at) else {
                        return self.fail(Class::ParseError, "a string that does not end");
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
                                    return self.fail(Class::InvalidUnicode, "a lone surrogate");
                                }
                                self.at += 2;
                                let second = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&second) {
                                    return self.fail(Class::InvalidUnicode, "a lone surrogate");
                                }
                                0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                            } else if (0xdc00..0xe000).contains(&first) {
                                return self.fail(Class::InvalidUnicode, "a lone surrogate");
                            } else {
                                first
                            };
                            match char::from_u32(code) {
                                Some(c) => out.push(c),
                                None => return self.fail(Class::InvalidUnicode, "not a character"),
                            }
                        }
                        _ => return self.fail(Class::ParseError, "an unknown escape"),
                    }
                }
                0..=0x1f => return self.fail(Class::ParseError, "a control character in a string"),
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
                        core::str::from_utf8(&self.bytes[start..self.at]).unwrap_or_default(),
                    );
                }
            }
        }
    }
}

/// One YAML 1.2 document under the core schema, built from parser events so that no tag,
/// anchor, alias, merge key or directive is applied: each is refused.
pub(crate) fn yaml(bytes: &[u8], depth: Limit) -> Result<Value, Error> {
    use granit_parser::{Event, Parser, ScalarStyle, Scanner, StrInput, TokenType};

    let text = core::str::from_utf8(bytes).map_err(|_| refusal(Class::InvalidUtf8, "not UTF-8"))?;
    if text.contains(['\u{85}', '\u{2028}', '\u{2029}']) {
        return Err(refusal(
            Class::ParseError,
            "a Unicode line break YAML 1.1 and 1.2 read differently",
        ));
    }
    let located = |error: &granit_parser::ScanError, class: Class| {
        let marker = error.marker();
        let class = if !depth.allows(nesting(text)) {
            Class::TooDeep
        } else {
            class
        };
        refusal(class, "not valid YAML").at(marker.line(), marker.col() + 1)
    };
    if text.char_indices().any(|(at, c)| c == '\u{feff}' && at > 0) {
        return Err(refusal(
            Class::ParseError,
            "a byte-order mark on a later line",
        ));
    }
    for token in Scanner::new(StrInput::new(text)) {
        match token {
            Ok(token) => {
                if matches!(
                    token.token_type(),
                    TokenType::VersionDirective(..) | TokenType::TagDirective(..)
                ) {
                    return Err(refusal(Class::YamlDirective, "a %YAML or %TAG directive"));
                }
            }
            Err(error) => return Err(located(&error, Class::ParseError)),
        }
    }
    let mut builder = Builder::new(depth, false);
    let mut documents = 0usize;
    let mut step = |event: Event| -> Result<(), Error> {
        let sequence = matches!(event, Event::SequenceStart(..));
        match event {
            Event::DocumentStart(_, version) => {
                if version.is_some() {
                    return Err(refusal(Class::YamlDirective, "a %YAML directive"));
                }
                documents += 1;
                if documents > 1 {
                    return Err(refusal(Class::ParseError, "more than one document"));
                }
            }
            Event::Alias(_) => return Err(refusal(Class::YamlAnchor, "an alias")),
            Event::Scalar(value, style, anchor, tag) => {
                if anchor != 0 {
                    return Err(refusal(Class::YamlAnchor, "an anchor"));
                }
                if tag.is_some() {
                    return Err(refusal(Class::YamlTag, "a tag"));
                }
                let plain = matches!(style, ScalarStyle::Plain);
                if builder.wants_key() {
                    if plain {
                        let typed = core_scalar(&value)?;
                        if !matches!(typed, Scalar::Text) {
                            return Err(refusal(
                                Class::NonStringKey,
                                "a mapping key that is not a string",
                            ));
                        }
                    }
                    if value == "<<" {
                        return Err(refusal(Class::YamlMergeKey, "a merge key"));
                    }
                    builder.key(value.into_owned())?;
                } else {
                    let item = if plain {
                        match core_scalar(&value)? {
                            Scalar::Text => Value::String(value.to_string()),
                            Scalar::Bool(flag) => Value::Bool(flag),
                            Scalar::Integer(integer) => Value::from(integer),
                            Scalar::Float(value) => float(value)?,
                        }
                    } else {
                        Value::String(value.to_string())
                    };
                    builder.value(item)?;
                }
            }
            Event::SequenceStart(_, anchor, tag) | Event::MappingStart(_, anchor, tag) => {
                if anchor != 0 {
                    return Err(refusal(Class::YamlAnchor, "an anchor"));
                }
                if tag.is_some() {
                    return Err(refusal(Class::YamlTag, "a tag"));
                }
                if builder.wants_key() {
                    return Err(refusal(
                        Class::NonStringKey,
                        "a mapping key that is not a string",
                    ));
                }
                if sequence {
                    builder.open_list()?;
                } else {
                    builder.open_map()?;
                }
            }
            Event::SequenceEnd | Event::MappingEnd => builder.close()?,
            _ => {}
        }
        Ok(())
    };
    for event in Parser::new_from_str(text) {
        let (event, span) = event.map_err(|error| located(&error, Class::ParseError))?;
        step(event).map_err(|error| error.at(span.start.line(), span.start.col() + 1))?;
    }
    Ok(builder.root.unwrap_or_else(|| Value::Object(Map::new())))
}

enum Scalar {
    Text,
    Bool(bool),
    Integer(i64),
    Float(f64),
}

/// A plain scalar's type under the YAML 1.2 core schema.
fn core_scalar(value: &str) -> Result<Scalar, Error> {
    if matches!(value, "" | "~" | "null" | "Null" | "NULL") {
        return Err(refusal(Class::NullValue, "a null"));
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
                    Class::IntegerRange,
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
        return Err(refusal(Class::NonFiniteFloat, "a float that is not finite"));
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
