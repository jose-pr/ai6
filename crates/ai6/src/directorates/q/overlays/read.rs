//! Reading an AOS document (`overlay.cfg`, an `aos` env file) into the data model, a `dotenv`
//! file into its lines, and an env script's output (`spec/v0/manifest.md`, `environment.md`).
//!
//! Reading runs nothing, reads no other file and consults no environment.

use std::fmt;
use std::sync::OnceLock;

use anycfg::{Callbacks, CodecError, Formats, Options};
use guatiao::value::types::{Number, Tag};
use guatiao::{TryAsRef, Value};
use serde_json::Value as Data;

use super::codecs;
use super::names::{is_reserved_variable, is_variable_name};
use crate::bounds::Limit;

/// A manifest or env file as read: a mapping with string keys.
pub type Document = serde_json::Map<String, Data>;

/// How much of a document a reader reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    /// Bytes of the document, its byte-order mark and format line included.
    pub bytes: Limit,
    /// Nesting of arrays and mappings, the top mapping being 1.
    pub depth: Limit,
}

impl Default for ReadLimits {
    /// 1 MiB and 64 levels, as AOS recommends.
    fn default() -> Self {
        Self {
            bytes: Limit::at(1024 * 1024),
            depth: Limit::at(64),
        }
    }
}

/// A document AOS refuses. Its text names the class and where, never the document's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    class: String,
    detail: String,
    line: Option<usize>,
    column: Option<usize>,
}

impl ReadError {
    pub(crate) fn new(class: &str, detail: impl Into<String>) -> Self {
        Self {
            class: class.to_owned(),
            detail: detail.into(),
            line: None,
            column: None,
        }
    }

    /// The AOS error class, such as `parse-error`, `duplicate-key` or `yaml-tag`.
    pub fn class(&self) -> &str {
        &self.class
    }

    /// The one-based line, when known; a removed format line counts.
    pub fn line(&self) -> Option<usize> {
        self.line
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.class)?;
        if !self.detail.is_empty() {
            write!(formatter, ": {}", self.detail)?;
        }
        if let Some(line) = self.line {
            write!(formatter, " at line {line}")?;
            if let Some(column) = self.column {
                write!(formatter, " column {column}")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for ReadError {}

/// The classes a codec's message may start with.
const CLASSES: [&str; 13] = [
    "invalid-utf8",
    "parse-error",
    "null-value",
    "duplicate-key",
    "non-string-key",
    "datetime-value",
    "non-finite-float",
    "integer-range",
    "invalid-unicode",
    "yaml-tag",
    "yaml-anchor",
    "yaml-merge-key",
    "yaml-directive",
];

fn formats() -> &'static Formats<Value> {
    static FORMATS: OnceLock<Formats<Value>> = OnceLock::new();
    FORMATS.get_or_init(|| {
        let mut formats = Formats::new();
        let never =
            |_: &Value| Err(CodecError::new().message("AOS documents are read, not written"));
        let registered = formats
            .register("toml", ["toml"], Callbacks::new(codecs::toml, never))
            .and_then(|()| formats.register("json", ["json"], Callbacks::new(codecs::json, never)))
            .and_then(|()| {
                formats.register("yaml", ["yaml", "yml"], Callbacks::new(codecs::yaml, never))
            });
        debug_assert!(registered.is_ok(), "three distinct format names");
        formats
    })
}

/// The format a document names, and the bytes after its format line.
fn select_format(data: &[u8]) -> Result<(String, &[u8], bool), ReadError> {
    let Some(rest) = data.strip_prefix(b"#!") else {
        return Ok(("toml".to_owned(), data, false));
    };
    let (line, payload) = match rest.iter().position(|byte| *byte == b'\n') {
        Some(end) => (&rest[..end], &rest[end + 1..]),
        None => (rest, &rest[rest.len()..]),
    };
    let line = std::str::from_utf8(line)
        .map_err(|_| ReadError::new("invalid-utf8", "the format line is not UTF-8"))?;
    let word = line
        .split([' ', '\t', '\u{c}', '\r'])
        .rfind(|word| !word.is_empty())
        .unwrap_or_default();
    let word = word.rsplit(['/', '\\']).next().unwrap_or_default();
    let name = word.trim_start_matches('.').to_ascii_lowercase();
    if name.is_empty() {
        return Err(ReadError::new(
            "no-format-named",
            "the format line names no format",
        ));
    }
    Ok((name, payload, true))
}

/// Reads `data` as an AOS document: an optional byte-order mark, an optional format line
/// (`#!json`, `#!/usr/bin/env yaml`), else TOML, then the data model's rules.
///
/// # Errors
///
/// A [`ReadError`] whose [`class`](ReadError::class) is one of AOS's reading classes, or
/// `too-large` and `too-deep` past `limits`.
pub fn read_document(data: &[u8], limits: &ReadLimits) -> Result<Document, ReadError> {
    if !limits.bytes.allows(data.len()) {
        return Err(ReadError::new(
            "too-large",
            format!("over {} bytes", limits.bytes),
        ));
    }
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let (name, payload, lined) = select_format(data)?;
    let format = match name.as_str() {
        "toml" | "json" | "yaml" => name.as_str(),
        "yml" => "yaml",
        _ => {
            let safe = name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
            return Err(ReadError::new(
                "unsupported-format",
                if safe {
                    format!("{name} is not toml, json or yaml")
                } else {
                    "the format line names no supported format".to_owned()
                },
            ));
        }
    };
    let text =
        std::str::from_utf8(payload).map_err(|_| ReadError::new("invalid-utf8", "not UTF-8"))?;
    if text.starts_with('\u{feff}') {
        return Err(ReadError::new(
            "parse-error",
            "a byte-order mark where the document starts",
        ));
    }
    if text.trim_matches([' ', '\t', '\r', '\n']).is_empty() {
        return Ok(Document::new());
    }
    let value = formats()
        .from_slice(payload, Options::new().format(format).max_bytes(usize::MAX))
        .map_err(|error| codec_error(error, usize::from(lined)))?;
    match to_data(&value, 1, limits.depth, false)? {
        Data::Object(map) => Ok(map),
        _ => Err(ReadError::new(
            "not-a-mapping",
            "the document is not a mapping",
        )),
    }
}

fn codec_error(error: anycfg::Error, lines_removed: usize) -> ReadError {
    match error {
        anycfg::Error::Parse {
            line,
            column,
            message,
            ..
        } => {
            let message = message.unwrap_or_default();
            let (class, detail) = match message.split_once(": ") {
                Some((class, detail)) if CLASSES.contains(&class) => (class, detail),
                _ => ("parse-error", message.as_str()),
            };
            ReadError {
                class: class.to_owned(),
                detail: detail.to_owned(),
                line: line.map(|line| line + lines_removed),
                column,
            }
        }
        other => ReadError::new("parse-error", other.to_string()),
    }
}

/// A guatiao value as the data model, refusing what the model has no type for.
fn to_data(value: &Value, depth: u64, limit: Limit, nulls: bool) -> Result<Data, ReadError> {
    let tag = value
        .tag()
        .map_err(|_| ReadError::new("parse-error", "an unknown value"))?;
    let container = matches!(tag, Tag::GUATIAO_LIST | Tag::GUATIAO_MAP);
    if container && !limit.allows(depth) {
        return Err(ReadError::new(
            "too-deep",
            format!("nested deeper than {limit}"),
        ));
    }
    Ok(match tag {
        Tag::GUATIAO_NULL if nulls => Data::Null,
        Tag::GUATIAO_NULL => return Err(ReadError::new("null-value", "a null")),
        Tag::GUATIAO_BOOL => Data::Bool(bool::try_from(value).unwrap_or_default()),
        Tag::GUATIAO_NUMBER => {
            let text: &str = TryAsRef::<Number>::try_as_ref(value).map_or("", |number| number);
            if text.contains(['.', 'e', 'E']) {
                let parsed: f64 = text.parse().unwrap_or(f64::NAN);
                serde_json::Number::from_f64(parsed)
                    .map(Data::Number)
                    .ok_or_else(|| {
                        ReadError::new("non-finite-float", "a float that is not finite")
                    })?
            } else {
                let parsed: i64 = text.parse().map_err(|_| {
                    ReadError::new(
                        "integer-range",
                        "an integer outside the signed 64-bit range",
                    )
                })?;
                Data::from(parsed)
            }
        }
        Tag::GUATIAO_STRING => Data::String(<&str>::try_from(value).unwrap_or_default().to_owned()),
        Tag::GUATIAO_LIST => Data::Array(
            <&[Value]>::try_from(value)
                .unwrap_or_default()
                .iter()
                .map(|item| to_data(item, depth + 1, limit, nulls))
                .collect::<Result<_, _>>()?,
        ),
        Tag::GUATIAO_MAP => {
            let mut map = serde_json::Map::new();
            for entry in <&[guatiao::value::types::Entry]>::try_from(value).unwrap_or_default() {
                map.insert(
                    entry.key().to_owned(),
                    to_data(entry.value(), depth + 1, limit, nulls)?,
                );
            }
            Data::Object(map)
        }
        _ => {
            return Err(ReadError::new(
                "parse-error",
                "a value the data model has no type for",
            ));
        }
    })
}

/// One line of a `dotenv` file: the variable, its text, and whether it was in single quotes,
/// which keep it as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DotenvLine {
    /// The variable.
    pub name: String,
    /// The value, quotes removed.
    pub text: String,
    /// Whether it was in single quotes, so it is never substituted.
    pub literal: bool,
}

fn variable_at(text: &str) -> Option<(&str, &str)> {
    let end = text
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_'))
        .map_or(text.len(), |(at, _)| at);
    let name = &text[..end];
    is_variable_name(name).then(|| (name, &text[end..]))
}

/// `NAME[ \t]*=` at the start of `text`, and what follows the `=`.
fn assignment(text: &str) -> Option<(&str, &str)> {
    let (name, rest) = variable_at(text)?;
    let rest = rest.trim_start_matches([' ', '\t']).strip_prefix('=')?;
    Some((name, rest))
}

/// Reads a `dotenv` file into its lines, in order.
///
/// # Errors
///
/// `invalid-utf8`, `dotenv-syntax` for a line that is no assignment, and `duplicate-key` for
/// two names equal without regard to ASCII case.
pub fn parse_dotenv(data: &[u8]) -> Result<Vec<DotenvLine>, ReadError> {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let text =
        std::str::from_utf8(data).map_err(|_| ReadError::new("invalid-utf8", "not UTF-8"))?;
    let mut lines: Vec<DotenvLine> = Vec::new();
    for (number, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let stripped = line.trim_matches([' ', '\t']);
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        let start = line.trim_start_matches([' ', '\t']);
        let exported = start.strip_prefix("export").and_then(|rest| {
            let after = rest.trim_start_matches([' ', '\t']);
            (after.len() < rest.len()).then_some(after)
        });
        let Some((name, value)) = exported.and_then(assignment).or_else(|| assignment(start))
        else {
            let mut error = ReadError::new("dotenv-syntax", "a line that is no NAME=VALUE");
            error.line = Some(number + 1);
            return Err(error);
        };
        let mut value = value.trim_matches([' ', '\t']);
        let mut literal = false;
        let mut chars = value.chars();
        if let (Some(first), Some(last)) = (chars.next(), chars.next_back())
            && first == last
            && matches!(first, '"' | '\'')
        {
            literal = first == '\'';
            value = &value[1..value.len() - 1];
        }
        if lines
            .iter()
            .any(|earlier| earlier.name.eq_ignore_ascii_case(name))
        {
            let mut error = ReadError::new("duplicate-key", format!("{name} is set twice"));
            error.line = Some(number + 1);
            return Err(error);
        }
        lines.push(DotenvLine {
            name: name.to_owned(),
            text: value.to_owned(),
            literal,
        });
    }
    Ok(lines)
}

/// What an env script's output sets: each variable to a value, or `None` to unset it, the
/// later object winning.
pub type ScriptOutput = Vec<(String, Option<String>)>;

/// Reads an env script's standard output: one JSON object, possibly over several lines, or
/// else one object per line that is not blank.
///
/// # Errors
///
/// `invalid-utf8`, the JSON reading classes, and `env-script-output` for something other than
/// objects of variable names that are not reserved, mapped to a string without a zero byte or
/// to null.
pub fn read_env_script_output(data: &[u8]) -> Result<ScriptOutput, ReadError> {
    let text =
        std::str::from_utf8(data).map_err(|_| ReadError::new("invalid-utf8", "not UTF-8"))?;
    let whole = text.trim_matches([' ', '\t', '\r', '\n']);
    if whole.is_empty() {
        return Ok(Vec::new());
    }
    let json = |text: &str| -> Result<Data, ReadError> {
        let value = formats()
            .from_slice(
                text.as_bytes(),
                Options::new().format("json").max_bytes(usize::MAX),
            )
            .map_err(|error| codec_error(error, 0))?;
        to_data(&value, 1, Limit::UNBOUNDED, true)
    };
    let objects = match json(whole) {
        Ok(value) => vec![value],
        Err(error) if error.class() == "parse-error" => {
            let mut objects = Vec::new();
            for line in text.split('\n') {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if !line.trim_matches([' ', '\t']).is_empty() {
                    objects.push(json(line)?);
                }
            }
            objects
        }
        Err(error) => return Err(error),
    };
    let mut out: ScriptOutput = Vec::new();
    for object in objects {
        let Data::Object(map) = object else {
            return Err(ReadError::new("env-script-output", "not an object"));
        };
        for (name, value) in map {
            if !is_variable_name(&name) || is_reserved_variable(&name) {
                return Err(ReadError::new(
                    "env-script-output",
                    "a name that may not be set",
                ));
            }
            let value = match value {
                Data::Null => None,
                Data::String(text) if !text.contains('\0') => Some(text),
                _ => {
                    return Err(ReadError::new(
                        "env-script-output",
                        "a value that is not text",
                    ));
                }
            };
            out.retain(|(earlier, _)| *earlier != name);
            out.push((name, value));
        }
    }
    Ok(out)
}
