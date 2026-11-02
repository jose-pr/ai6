//! Reading a configuration document, through [`anycfg`] and [`guatiao`]'s value.
//!
//! `.toml`, `.json`, `.yaml` and `.yml` choose their format by extension. A `.cfg` document names
//! its format on its first line (`#!json`, `#!yaml`, `#!toml`, the line's last word), a document
//! starting `%YAML` is YAML, and anything else is TOML; nothing is guessed from the content. Any
//! other extension is an error that lists the accepted ones. One UTF-8 byte order mark is
//! dropped before parsing.
//!
//! Every format becomes one [`Value`], and a type comes from that value by [`FromValue`]: a
//! refusal names the key or the position, and never quotes the document, which may hold a
//! secret. So a YAML plain scalar has its type before the type reading it is asked: `n`, `yes`
//! and `123` in a text field are refused, and must be quoted.
//!
//! A repeated key is refused in every format.

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use anycfg::{Callbacks, CodecError, Formats, Options};
use guatiao::value::types::Tag;
use guatiao::{Alloc, FromValue, Map, Value};
use guatiao_serde::{Presentation, text};
use serde::Serialize;
use serde::de::{self, DeserializeOwned};

/// The extensions a configuration file may have, in the order a directory is searched.
pub const EXTENSIONS: [&str; 5] = ["cfg", "toml", "json", "yaml", "yml"];

/// A configuration document's format.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// JSON.
    Json,
    /// TOML: also a `.cfg` document whose first line names no other.
    Toml,
    /// YAML.
    Yaml,
}

impl Format {
    /// `json`, `toml` or `yaml`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Json => text::json::NAME,
            Self::Toml => text::toml::NAME,
            Self::Yaml => text::yaml::NAME,
        }
    }

    /// The format a file named `path` is written in unless its first line says otherwise.
    ///
    /// # Errors
    ///
    /// A [`FormatError`] naming the accepted extensions when the extension is missing or
    /// not one of [`EXTENSIONS`].
    pub fn for_path(path: &Path) -> Result<Self, FormatError> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("");
        Self::for_extension(extension).ok_or_else(|| {
            FormatError(format!(
                "{}: the extension must be one of {}",
                path.display(),
                EXTENSIONS.join(", ")
            ))
        })
    }

    /// The format of an extension written without its dot, ASCII case ignored; `None` for one
    /// that is not in [`EXTENSIONS`].
    pub fn for_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "cfg" | "toml" => Some(Self::Toml),
            "json" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            _ => None,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Why a document could not be read or written. It names the key or the position, never the
/// value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(String);

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for FormatError {}

impl FormatError {
    fn of(error: &impl fmt::Display) -> Self {
        Self(error.to_string())
    }
}

/// What a codec may pass on: guatiao's refusals carry a position and the parser's words, and no
/// excerpt of the document.
fn failure(error: text::Error) -> CodecError {
    let at = error.position().map_or_else(CodecError::new, |position| {
        CodecError::new()
            .line(position.line)
            .column(position.column)
    });
    at.message(error.to_string())
}

fn utf8(bytes: &[u8]) -> Result<&str, CodecError> {
    std::str::from_utf8(bytes).map_err(|error| {
        CodecError::at_offset(bytes, error.valid_up_to()).message("not valid UTF-8")
    })
}

/// A map keeps the last of a repeated key, so JSON's are looked for before one is built. TOML's
/// and YAML's parsers refuse them.
fn refuse_repeated_keys(text: &str) -> Result<(), CodecError> {
    serde_json::from_str::<UniqueKeys>(text)
        .map(|_| ())
        .map_err(|error| {
            let (line, column) = (error.line(), error.column());
            let message = error.to_string();
            let position = format!(" at line {line} column {column}");
            // With every value accepted, the text is a fixed syntax message or the one below.
            CodecError::new()
                .line(line)
                .column(column)
                .message(message.strip_suffix(&position).unwrap_or(&message))
        })
}

/// Any JSON document, read only to look at its keys.
struct UniqueKeys;

impl<'de> de::Deserialize<'de> for UniqueKeys {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueKeysVisitor)
    }
}

struct UniqueKeysVisitor;

impl<'de> de::Visitor<'de> for UniqueKeysVisitor {
    type Value = UniqueKeys;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }
    fn visit_unit<E>(self) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_bool<E>(self, _: bool) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_i64<E>(self, _: i64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_u64<E>(self, _: u64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_f64<E>(self, _: f64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_str<E>(self, _: &str) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_seq<A: de::SeqAccess<'de>>(self, mut items: A) -> Result<UniqueKeys, A::Error> {
        while items.next_element::<UniqueKeys>()?.is_some() {}
        Ok(UniqueKeys)
    }
    fn visit_map<A: de::MapAccess<'de>>(self, mut entries: A) -> Result<UniqueKeys, A::Error> {
        let mut seen = std::collections::HashSet::new();
        while let Some(key) = entries.next_key::<String>()? {
            let repeated = seen.contains(&key);
            if repeated {
                return Err(de::Error::custom(format_args!(
                    "duplicate key `{}`",
                    key.escape_default()
                )));
            }
            entries.next_value::<UniqueKeys>()?;
            seen.insert(key);
        }
        Ok(UniqueKeys)
    }
}

/// The three formats over one value, with `.cfg` choosing by its first line and then TOML.
fn formats() -> Result<&'static Formats<Value>, FormatError> {
    static FORMATS: OnceLock<Result<Formats<Value>, FormatError>> = OnceLock::new();
    let how = Presentation::default;
    FORMATS
        .get_or_init(|| {
            let mut formats = Formats::new();
            let registered = formats
                .register(
                    Format::Toml.name(),
                    ["toml"],
                    Callbacks::new(
                        move |bytes: &[u8]| {
                            text::toml::from_str(utf8(bytes)?, Alloc::rust(), how())
                                .map_err(failure)
                        },
                        move |value: &Value| {
                            // Its pretty writer keeps a table's keys in the order they came.
                            toml::to_string_pretty(&guatiao_serde::Serializable::from(value))
                                .map(String::into_bytes)
                                .map_err(|_| {
                                    CodecError::new().message("the value has no TOML form")
                                })
                        },
                    ),
                )
                .and_then(|()| {
                    formats.register(
                        Format::Json.name(),
                        ["json"],
                        Callbacks::new(
                            move |bytes: &[u8]| {
                                let text = utf8(bytes)?;
                                refuse_repeated_keys(text)?;
                                text::json::from_str(text, Alloc::rust(), how()).map_err(failure)
                            },
                            move |value: &Value| {
                                text::json::to_string_pretty(value, how())
                                    .map(|text| format!("{text}\n").into_bytes())
                                    .map_err(failure)
                            },
                        ),
                    )
                })
                .and_then(|()| {
                    formats.register(
                        Format::Yaml.name(),
                        ["yaml", "yml"],
                        Callbacks::new(
                            move |bytes: &[u8]| {
                                text::yaml::from_str(utf8(bytes)?, Alloc::rust(), how())
                                    .map_err(failure)
                            },
                            move |value: &Value| {
                                text::yaml::to_string(value, how())
                                    .map(String::into_bytes)
                                    .map_err(failure)
                            },
                        ),
                    )
                })
                .and_then(|()| formats.register_auto(["cfg"], Format::Toml.name()))
                .and_then(|()| formats.register_marker(Format::Yaml.name(), b"%YAML"));
            registered
                .map(|()| formats)
                .map_err(|error| FormatError::of(&error))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// A person's own file, and every limit on what it holds is a setting read from it or after it.
fn options<'a>() -> Options<'a> {
    Options::new().max_bytes(usize::MAX)
}

/// An empty YAML document is null, and every document here is a table.
fn table_or_empty(value: Value) -> Value {
    if matches!(value.tag(), Ok(Tag::GUATIAO_NULL)) {
        Map::new_in(Alloc::rust()).into()
    } else {
        value
    }
}

/// The document `text` of a file named `path`, as a value: the extension chooses the format, and
/// a `.cfg` file's first line may name another.
///
/// # Errors
///
/// A [`FormatError`] naming the accepted extensions or formats, or the position of a syntax
/// error.
pub fn parse(path: &Path, text: &str) -> Result<Value, FormatError> {
    Format::for_path(path)?;
    let name = format!(
        "document.{}",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
    );
    formats()?
        .from_str(text, options().name(&name))
        .map(table_or_empty)
        .map_err(|error| FormatError::of(&error))
}

/// A `T` from the document `text` of a file named `path` ([`parse`]).
///
/// # Errors
///
/// As [`parse`], and a [`FormatError`] naming the first key `T` refuses.
pub fn read<T: FromValue>(path: &Path, text: &str) -> Result<T, FormatError> {
    T::from_value(&parse(path, text)?).map_err(|error| FormatError::of(&error))
}

/// Reads `text` as `format`, whatever its first line says, into a serde type.
///
/// # Errors
///
/// A [`FormatError`] when the text is not `format` or does not have `T`'s shape.
pub fn from_str<T: DeserializeOwned>(format: Format, text: &str) -> Result<T, FormatError> {
    let value = formats()?
        .from_str(text, options().format(format.name()))
        .map_err(|error| FormatError::of(&error))?;
    guatiao_serde::adapt::from_value(&value).map_err(|error| FormatError::of(&error))
}

/// Writes `value` as `format`, for a person to read and edit.
///
/// # Errors
///
/// A [`FormatError`] when `value` has no form in `format` (a TOML document must be a table).
pub fn to_string<T: Serialize>(format: Format, value: &T) -> Result<String, FormatError> {
    let value = guatiao_serde::adapt::to_value_in(value, Alloc::rust())
        .map_err(|error| FormatError::of(&error))?;
    formats()?
        .to_string(&value, format.name())
        .map_err(|error| FormatError::of(&error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    #[serde(deny_unknown_fields)]
    struct Sample {
        name: String,
        #[serde(default)]
        count: u32,
    }

    #[derive(Debug, PartialEq, guatiao::FromValue)]
    #[map(rename_all = "lowercase")]
    enum Colour {
        Red,
    }

    #[derive(Debug, PartialEq, guatiao::FromValue)]
    #[map(deny_unknown_fields)]
    struct Inner {
        #[map(default)]
        count: u32,
        colour: Option<Colour>,
    }

    #[derive(Debug, PartialEq, guatiao::FromValue)]
    #[map(deny_unknown_fields)]
    struct Typed {
        name: String,
        listen: Option<std::net::SocketAddr>,
        inner: Option<Inner>,
    }

    /// A value a person could keep in a document, and what no refusal may repeat.
    const PLANTED: &str = "pl4nted-s3cret-value";

    fn refusal_in<'a>(name: &'a str, text: &'a str) -> (&'a str, String) {
        (
            name,
            parse(Path::new(name), text)
                .map_or_else(|error| error.to_string(), |_| panic!("{name} read: {text}")),
        )
    }

    #[test]
    fn each_extension_reads_its_own_format_and_others_are_refused() {
        assert_eq!(Format::for_path(Path::new("a.cfg")), Ok(Format::Toml));
        assert_eq!(Format::for_path(Path::new("a.TOML")), Ok(Format::Toml));
        assert_eq!(Format::for_path(Path::new("a.json")), Ok(Format::Json));
        assert_eq!(Format::for_path(Path::new("a.yml")), Ok(Format::Yaml));
        let refused = Format::for_path(Path::new("a.ini")).unwrap_err();
        assert!(
            refused.to_string().contains("cfg, toml, json, yaml, yml"),
            "{refused}"
        );
        assert!(Format::for_path(Path::new("ai6")).is_err());
        assert!(parse(Path::new("ai6"), "").is_err());
    }

    #[test]
    fn the_three_formats_read_to_the_same_value() {
        let expected = Sample {
            name: "G".to_owned(),
            count: 2,
        };
        assert_eq!(
            from_str::<Sample>(Format::Toml, "\u{feff}name = \"G\"\ncount = 2\n").unwrap(),
            expected
        );
        assert_eq!(
            from_str::<Sample>(Format::Json, r#"{"name": "G", "count": 2}"#).unwrap(),
            expected
        );
        assert_eq!(
            from_str::<Sample>(Format::Yaml, "name: G\ncount: 2\n").unwrap(),
            expected
        );
        for format in [Format::Toml, Format::Json, Format::Yaml] {
            let text = to_string(format, &expected).unwrap();
            assert_eq!(from_str::<Sample>(format, &text).unwrap(), expected);
        }
    }

    #[test]
    fn a_table_keeps_the_order_the_document_wrote_it_in() {
        for (name, text) in [
            ("a.toml", "zebra = 1\napple = 2\nmango = 3\n"),
            ("a.json", r#"{"zebra": 1, "apple": 2, "mango": 3}"#),
            ("a.yaml", "zebra: 1\napple: 2\nmango: 3\n"),
        ] {
            let value = parse(Path::new(name), text).unwrap();
            let map = <&Map>::try_from(&value).unwrap();
            let keys: Vec<&str> = map.into_iter().map(|entry| entry.key()).collect();
            assert_eq!(keys, ["zebra", "apple", "mango"], "{name}");
        }
    }

    #[test]
    fn a_cfg_file_names_its_format_on_its_first_line() {
        let expected = Typed {
            name: "G".to_owned(),
            listen: None,
            inner: None,
        };
        for text in [
            "name = \"G\"\n",
            "#!toml\nname = \"G\"\n",
            "#!.TOML\nname = \"G\"\n",
            "\u{feff}#!toml\nname = \"G\"\n",
            "#!json\n{\"name\": \"G\"}\n",
            "#!yaml\nname: G\n",
            "#!yml\nname: G\n",
            "#!/usr/bin/env yaml\nname: G\n",
            "%YAML 1.2\n---\nname: G\n",
        ] {
            assert_eq!(
                read::<Typed>(Path::new("ai6.cfg"), text).as_ref(),
                Ok(&expected),
                "{text}"
            );
        }
        // The extension settles it: a first line is read only where the format is left open.
        assert!(read::<Typed>(Path::new("ai6.json"), "#!yaml\nname: G\n").is_err());
        // Nothing is guessed from the content.
        assert!(read::<Typed>(Path::new("ai6.cfg"), "{\"name\": \"G\"}\n").is_err());
        let unknown = read::<Typed>(Path::new("ai6.cfg"), "#!ini\nname = 1\n").unwrap_err();
        for format in ["json", "toml", "yaml"] {
            assert!(unknown.to_string().contains(format), "{unknown}");
        }
        // A format line is text from the document, so it is not repeated.
        assert!(!unknown.to_string().contains("ini"), "{unknown}");
        // A line number counts the removed line.
        let refused = parse(Path::new("ai6.cfg"), "#!json\n{\n  \"a\": ?\n}\n").unwrap_err();
        assert!(refused.to_string().contains("line 3"), "{refused}");
    }

    #[test]
    fn a_refused_document_names_the_key_and_never_the_value() {
        // A key nothing declares, a wrong kind, an address and a choice that are not, and the
        // same one level down.
        for (kind, key) in [
            ("other", "other"),
            ("nested", "name"),
            ("number", "name"),
            ("address", "listen"),
            ("count", "inner.count"),
            ("colour", "inner.colour"),
            ("unknown", "inner.unknown"),
        ] {
            for (file, text) in documents(kind) {
                let refusal = read::<Typed>(Path::new(file), &text).unwrap_err();
                let shown = format!("{refusal} {refusal:?}");
                assert!(!shown.contains(PLANTED), "{file} {kind}: {shown}");
                assert!(shown.contains(key), "{file} {kind} names {key}: {shown}");
            }
        }
    }

    /// One document of each format refused for `kind`, with [`PLANTED`] where a value goes.
    fn documents(kind: &str) -> [(&'static str, String); 3] {
        let (toml, json, yaml) = match kind {
            "other" => (
                format!("name = \"G\"\nother = \"{PLANTED}\"\n"),
                format!(r#"{{"name": "G", "other": "{PLANTED}"}}"#),
                format!("name: G\nother: {PLANTED}\n"),
            ),
            "nested" => (
                format!("[name]\nsecret = \"{PLANTED}\"\n"),
                format!(r#"{{"name": {{"secret": "{PLANTED}"}}}}"#),
                format!("name:\n  secret: {PLANTED}\n"),
            ),
            "number" => (
                "name = 7\n".to_owned(),
                r#"{"name": 7}"#.to_owned(),
                "name: 7\n".to_owned(),
            ),
            "address" => (
                format!("name = \"G\"\nlisten = \"{PLANTED}\"\n"),
                format!(r#"{{"name": "G", "listen": "{PLANTED}"}}"#),
                format!("name: G\nlisten: {PLANTED}\n"),
            ),
            "count" => (
                format!("name = \"G\"\n[inner]\ncount = \"{PLANTED}\"\n"),
                format!(r#"{{"name": "G", "inner": {{"count": "{PLANTED}"}}}}"#),
                format!("name: G\ninner:\n  count: {PLANTED}\n"),
            ),
            "colour" => (
                format!("name = \"G\"\n[inner]\ncolour = \"{PLANTED}\"\n"),
                format!(r#"{{"name": "G", "inner": {{"colour": "{PLANTED}"}}}}"#),
                format!("name: G\ninner:\n  colour: {PLANTED}\n"),
            ),
            _ => (
                format!("name = \"G\"\n[inner]\nunknown = \"{PLANTED}\"\n"),
                format!(r#"{{"name": "G", "inner": {{"unknown": "{PLANTED}"}}}}"#),
                format!("name: G\ninner:\n  unknown: {PLANTED}\n"),
            ),
        };
        [("a.toml", toml), ("a.json", json), ("a.yaml", yaml)]
    }

    #[test]
    fn a_syntax_error_says_where_and_quotes_nothing() {
        for (name, text) in [
            ("a.toml", format!("key = \"{PLANTED}\n")),
            ("a.toml", format!("key = {PLANTED}\n")),
            ("a.json", format!("{{\"key\": \"{PLANTED}\n")),
            ("a.json", format!("{{\"key\": {PLANTED}}}\n")),
            ("a.yaml", format!("key: \"{PLANTED}\n")),
            ("a.yaml", format!("key: [{PLANTED}\n")),
            ("a.yaml", format!("key: *{PLANTED}\n")),
            ("a.cfg", format!("#!json\n{{\"key\": {PLANTED}}}\n")),
            ("a.cfg", format!("#!yaml\nkey: \"{PLANTED}\n")),
        ] {
            let (name, refusal) = refusal_in(name, &text);
            assert!(!refusal.contains(PLANTED), "{name}: {refusal}");
            assert!(refusal.contains("line "), "{name}: {refusal}");
        }
    }

    #[test]
    fn a_repeated_key_is_refused_in_every_format_and_its_value_is_not_quoted() {
        for (name, text) in [
            ("a.toml", format!("key = \"{PLANTED}\"\nkey = \"other\"\n")),
            (
                "a.json",
                format!(r#"{{"key": "{PLANTED}", "key": "other"}}"#),
            ),
            ("a.yaml", format!("key: {PLANTED}\nkey: other\n")),
        ] {
            let (name, refusal) = refusal_in(name, &text);
            assert!(!refusal.contains(PLANTED), "{name}: {refusal}");
            assert!(refusal.contains("duplicate"), "{name}: {refusal}");
        }
    }
}
