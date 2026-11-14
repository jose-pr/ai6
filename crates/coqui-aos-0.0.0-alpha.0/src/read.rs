//! Reading an AOS document (`overlay.cfg`, an `aos` env file) into the data model, a `dotenv`
//! file into its lines, and an env script's output (`spec/v0/manifest.md`, `environment.md`).
//!
//! Reading runs nothing, reads no other file and consults no environment.

use alloc::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::codecs;
use crate::error::{Class, Error};
use crate::limits::Settings;
use crate::names::{is_reserved_variable, is_variable_name};

/// A manifest or env file as read: a mapping with string keys.
pub type Document = serde_json::Map<String, Value>;

/// The format a document names, and the bytes after its format line; `default` when it names
/// none. A byte-order mark is not dropped here: where the parser's text starts it is an error.
pub(crate) fn choose_format<'a>(
    data: &'a [u8],
    default: &'static str,
) -> Result<(&'static str, &'a [u8], bool), Error> {
    let Some(rest) = data.strip_prefix(b"#!") else {
        return Ok((default, data, false));
    };
    let (line, payload) = match rest.iter().position(|byte| *byte == b'\n') {
        Some(end) => (&rest[..end], &rest[end + 1..]),
        None => (rest, &rest[rest.len()..]),
    };
    let line = core::str::from_utf8(line)
        .map_err(|_| Error::new(Class::InvalidUtf8, "the format line is not UTF-8"))?;
    let word = line
        .split([' ', '\t', '\u{c}', '\r'])
        .rfind(|word| !word.is_empty())
        .unwrap_or_default();
    let word = word.rsplit(['/', '\\']).next().unwrap_or_default();
    let name = word.trim_start_matches('.').to_ascii_lowercase();
    if name.is_empty() {
        return Err(Error::new(
            Class::NoFormatNamed,
            "the format line names no format",
        ));
    }
    let format = match name.as_str() {
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        _ => {
            let safe = name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
            return Err(Error::new(
                Class::UnsupportedFormat,
                if safe {
                    format!("{name} is not toml, json or yaml")
                } else {
                    "the format line names no supported format".to_owned()
                },
            ));
        }
    };
    Ok((format, payload, true))
}

/// The text after any format line, parsed as `format` and checked against the data model. An
/// error's line counts from the document's first line, `lined` saying a format line precedes
/// `payload`.
pub(crate) fn parse_payload(
    payload: &[u8],
    format: &str,
    settings: &Settings,
    lined: bool,
) -> Result<Document, Error> {
    let text =
        core::str::from_utf8(payload).map_err(|_| Error::new(Class::InvalidUtf8, "not UTF-8"))?;
    if text.starts_with('\u{feff}') {
        return Err(Error::new(
            Class::ParseError,
            "a byte-order mark where the document starts",
        )
        .at(1 + usize::from(lined), 1));
    }
    if text.trim_matches([' ', '\t', '\r', '\n']).is_empty() {
        return Ok(Document::new());
    }
    let depth = settings.manifest_depth;
    let value = match format {
        "toml" => codecs::toml(payload, depth),
        "json" => codecs::json(payload, depth, false),
        _ => codecs::yaml(payload, depth),
    }
    .map_err(|error| shift(error, usize::from(lined)))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(Error::new(
            Class::NotAMapping,
            "the document is not a mapping",
        )),
    }
}

/// Reads `data` as an AOS document: an optional byte-order mark, an optional format line
/// (`#!json`, `#!/usr/bin/env yaml`), else TOML, then the data model's rules, within the manifest
/// limits of `settings`.
///
/// # Errors
///
/// An [`Error`] whose class is one of AOS's reading classes, or `too-large` and `too-deep` past
/// the limits.
pub fn read_document(data: &[u8], settings: &Settings) -> Result<Document, Error> {
    if !settings.manifest_bytes.allows(data.len()) {
        return Err(Error::new(
            Class::TooLarge,
            format!("over {} bytes", settings.manifest_bytes),
        )
        .over("manifest_bytes"));
    }
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let (format, payload, lined) = choose_format(data, "toml")?;
    parse_payload(payload, format, settings, lined)
}

/// An error's position counted from the document's first line, a removed format line included.
fn shift(mut error: Error, lines_removed: usize) -> Error {
    error.line = error.line.map(|line| line + lines_removed);
    error
}

/// One line of a `dotenv` file: the variable, its text, and whether it was in single quotes,
/// which keep it as written.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
pub fn parse_dotenv(data: &[u8]) -> Result<Vec<DotenvLine>, Error> {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let text =
        core::str::from_utf8(data).map_err(|_| Error::new(Class::InvalidUtf8, "not UTF-8"))?;
    let mut lines: Vec<DotenvLine> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
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
            let mut error = Error::new(Class::DotenvSyntax, "a line that is no NAME=VALUE");
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
        if !seen.insert(name.to_ascii_uppercase()) {
            let mut error = Error::new(Class::DuplicateKey, format!("{name} is set twice"));
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
pub fn read_env_script_output(data: &[u8], settings: &Settings) -> Result<ScriptOutput, Error> {
    let text =
        core::str::from_utf8(data).map_err(|_| Error::new(Class::InvalidUtf8, "not UTF-8"))?;
    let whole = text.trim_matches([' ', '\t', '\r', '\n']);
    if whole.is_empty() {
        return Ok(Vec::new());
    }
    let json = |text: &str| -> Result<Value, Error> {
        codecs::json(text.as_bytes(), settings.manifest_depth, true)
    };
    let objects = match json(whole) {
        Ok(value) => vec![value],
        Err(error) if error.class == Class::ParseError => {
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
    // Where each name sits in `out`: a name set again keeps the place it first had.
    let mut at: BTreeMap<String, usize> = BTreeMap::new();
    for object in objects {
        let Value::Object(map) = object else {
            return Err(Error::new(Class::EnvScriptOutput, "not an object"));
        };
        for (name, value) in map {
            if !is_variable_name(&name) || is_reserved_variable(&name) {
                return Err(Error::new(
                    Class::EnvScriptOutput,
                    "a name that may not be set",
                ));
            }
            let value = match value {
                Value::Null => None,
                Value::String(text) if !text.contains('\0') => Some(text),
                _ => {
                    return Err(Error::new(
                        Class::EnvScriptOutput,
                        "a value that is not text",
                    ));
                }
            };
            match at.get(&name) {
                Some(place) => out[*place].1 = value,
                None => {
                    at.insert(name.clone(), out.len());
                    out.push((name, value));
                }
            }
        }
    }
    Ok(out)
}
