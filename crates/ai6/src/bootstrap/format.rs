//! Reading a configuration document in the format its file name says.
//!
//! `.toml`, `.json`, `.yaml` and `.yml` choose their format by extension. A `.cfg` document is
//! TOML. Any other extension is an error that lists the accepted ones, never a guess. One
//! UTF-8 byte order mark is dropped before parsing.

use std::fmt;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The extensions a configuration file may have, in the order a directory is searched.
pub const EXTENSIONS: [&str; 5] = ["cfg", "toml", "json", "yaml", "yml"];

/// A configuration document's format.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// JSON.
    Json,
    /// TOML: also every `.cfg` document.
    Toml,
    /// YAML.
    Yaml,
}

impl Format {
    /// `json`, `toml` or `yaml`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
        }
    }

    /// The format of a file named `path`, from its extension.
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

/// Why a document could not be read or written: the parser's message, which names the key or
/// position, never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(String);

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for FormatError {}

/// Reads `text` as `format` into `T`.
///
/// # Errors
///
/// A [`FormatError`] with the parser's message when the text is not `format`, or does not
/// have `T`'s shape (an unknown key in a type that refuses them, say).
pub fn from_str<T: DeserializeOwned>(format: Format, text: &str) -> Result<T, FormatError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let failed = |message: String| FormatError(format!("not valid {format}: {message}"));
    match format {
        Format::Toml => toml::from_str(text).map_err(|error| failed(error.to_string())),
        Format::Json => serde_json::from_str(text).map_err(|error| failed(error.to_string())),
        Format::Yaml => serde_saphyr::from_str(text).map_err(|error| failed(error.to_string())),
    }
}

/// Writes `value` as `format`, for a person to read and edit.
///
/// # Errors
///
/// A [`FormatError`] when `value` has no form in `format` (a TOML document must be a table).
pub fn to_string<T: Serialize>(format: Format, value: &T) -> Result<String, FormatError> {
    let failed = |message: String| FormatError(format!("cannot be written as {format}: {message}"));
    match format {
        Format::Toml => toml::to_string_pretty(value).map_err(|error| failed(error.to_string())),
        Format::Json => serde_json::to_string_pretty(value)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|error| failed(error.to_string())),
        Format::Yaml => serde_saphyr::to_string(value).map_err(|error| failed(error.to_string())),
    }
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
    }

    #[test]
    fn the_three_formats_read_to_the_same_value_and_unknown_keys_are_named() {
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
        let refused = from_str::<Sample>(Format::Toml, "name = \"G\"\ncolour = 1\n").unwrap_err();
        assert!(refused.to_string().contains("colour"), "{refused}");
        for format in [Format::Toml, Format::Json, Format::Yaml] {
            let text = to_string(format, &expected).unwrap();
            assert_eq!(from_str::<Sample>(format, &text).unwrap(), expected);
        }
    }
}
