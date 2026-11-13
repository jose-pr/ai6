//! Settings as guatiao values: one description for parsing, validation, defaults and docs.

use serde::Serialize;

use guatiao::schema::{self, SchemaRef};
use guatiao::{FromValue, Schema, ToValue};
use guatiao_serde::Presentation;
use guatiao_serde::text::json;

/// The longest line a reader takes by default, in bytes.
pub const DEFAULT_MAX_LINE_BYTES: u64 = 16 << 20;
/// The most lines a reader takes by default, the header included.
pub const DEFAULT_MAX_LINES: u64 = 5_000_000;
/// The most bytes a reader takes by default.
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 4 << 30;
/// The deepest nesting a reader takes by default.
pub const DEFAULT_MAX_DEPTH: u64 = 64;
/// The most JSON values a reader takes in one line by default.
pub const DEFAULT_MAX_VALUES: u64 = 500_000;
/// The most bytes a history read into memory as a whole takes by default.
pub const DEFAULT_DOCUMENT_MAX_TOTAL_BYTES: u64 = 16 << 20;
/// The deepest nesting the JSON parser reads, whatever `max_depth` says.
pub const PARSER_DEPTH: u64 = 127;

/// The sizes a reader and a writer hold a history to. Every one is a setting with no floor and no
/// ceiling the crate adds; a looser value is reported by [`non_defaults`].
///
/// Nesting is also bounded by the JSON parser, which stops at [`PARSER_DEPTH`] levels whatever
/// `max_depth` is: a `max_depth` above it changes nothing, and a line nested past it is refused as
/// too deep naming that limit.
///
/// A parsed line holds a multiple of its bytes in memory: 3 or 4 times for ordinary text, and up
/// to about 60 times for a line of tiny objects (`{"a":0}` costs about 430 bytes). `max_values`
/// bounds one line's tree to roughly 150 bytes a value. A history read as a whole
/// ([`Document`](crate::Document)) holds every line at once, so [`ReaderSettings::for_document`]
/// starts from a smaller `max_total_bytes`.
#[derive(ToValue, FromValue, Schema, Serialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct Bounds {
    /// The longest line, in bytes, without its line feed.
    #[map(default = DEFAULT_MAX_LINE_BYTES)]
    pub max_line_bytes: u64,
    /// The most lines, the header included.
    #[map(default = DEFAULT_MAX_LINES)]
    pub max_lines: u64,
    /// The most bytes in all.
    #[map(default = DEFAULT_MAX_TOTAL_BYTES)]
    pub max_total_bytes: u64,
    /// The deepest nesting of arrays and objects in a line.
    #[map(default = DEFAULT_MAX_DEPTH)]
    pub max_depth: u64,
    /// The most values in a line: every scalar, array and object, and every key of an object.
    #[map(default = DEFAULT_MAX_VALUES)]
    pub max_values: u64,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            max_lines: DEFAULT_MAX_LINES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_depth: DEFAULT_MAX_DEPTH,
            max_values: DEFAULT_MAX_VALUES,
        }
    }
}

impl Bounds {
    /// The bounds a history must keep to be read as a whole in memory: the defaults with
    /// `max_total_bytes` at [`DEFAULT_DOCUMENT_MAX_TOTAL_BYTES`].
    #[must_use]
    pub fn for_document() -> Self {
        Self {
            max_total_bytes: DEFAULT_DOCUMENT_MAX_TOTAL_BYTES,
            ..Self::default()
        }
    }

    /// No bound but the parser's own nesting limit, for a check of text this crate wrote itself.
    pub(crate) fn unbounded() -> Self {
        Self {
            max_line_bytes: u64::MAX,
            max_lines: u64::MAX,
            max_total_bytes: u64::MAX,
            max_depth: PARSER_DEPTH,
            max_values: u64::MAX,
        }
    }

    /// The nesting a line may reach: `max_depth`, or the parser's limit when that is lower.
    pub(crate) fn depth_limit(&self) -> u64 {
        self.max_depth.min(PARSER_DEPTH)
    }
}

/// How a [`Reader`](crate::Reader) behaves.
#[derive(ToValue, FromValue, Schema, Serialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct ReaderSettings {
    /// The sizes the history is held to.
    #[map(default)]
    pub bounds: Bounds,
    /// Whether a digest in the header is checked against the body once the reader reaches the
    /// end. Off, a history whose body was changed is read as it is.
    #[map(default = true)]
    pub verify_digest: bool,
}

impl Default for ReaderSettings {
    fn default() -> Self {
        Self {
            bounds: Bounds::default(),
            verify_digest: true,
        }
    }
}

impl ReaderSettings {
    /// The settings [`Document::from_slice`](crate::Document::from_slice) and
    /// [`str::parse`] read with: [`Bounds::for_document`], the digest checked.
    #[must_use]
    pub fn for_document() -> Self {
        Self {
            bounds: Bounds::for_document(),
            verify_digest: true,
        }
    }
}

/// How a [`Writer`](crate::Writer) behaves.
#[derive(ToValue, FromValue, Schema, Serialize, Debug, Clone, PartialEq, Eq, Default)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct WriterSettings {
    /// The sizes the history is held to: a line past them is refused, never cut.
    #[map(default)]
    pub bounds: Bounds,
}

/// What a [`Redactor`](crate::Redactor) removes. Both lists empty removes nothing.
#[derive(ToValue, FromValue, Schema, Serialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct RedactionSettings {
    /// Text removed wherever it appears, as it is spelled. These may be credentials.
    #[map(default)]
    #[schema(sensitive)]
    pub values: Vec<String>,
    /// Regular expressions (the `regex` crate's syntax, which matches in linear time) whose
    /// matches are removed.
    #[map(default)]
    pub patterns: Vec<String>,
    /// What replaces each removal.
    #[map(default = String::from(DEFAULT_REPLACEMENT))]
    pub replacement: String,
}

/// What a removal leaves by default.
pub const DEFAULT_REPLACEMENT: &str = "[redacted]";

impl Default for RedactionSettings {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            patterns: Vec::new(),
            replacement: String::from(DEFAULT_REPLACEMENT),
        }
    }
}

/// A setting that is not at its default.
#[derive(ToValue, FromValue, Schema, Serialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct NonDefault {
    /// The setting's key path (`bounds.max_line_bytes`).
    pub setting: String,
    /// Its value, as JSON text; a field the schema marks sensitive (a redaction's `values`) is
    /// shown as `"[not shown]"`.
    pub value: String,
    /// The default, as JSON text.
    pub default: String,
}

/// The text a sensitive setting is reported as.
const NOT_SHOWN: &str = "[not shown]";

/// Every setting of `settings` that is not at its default, by key path, in key order.
///
/// The list comes from the settings' schema: a list is one setting, and a field the schema marks
/// sensitive is named and not shown, wherever the settings are nested in a caller's own.
#[must_use]
pub fn non_defaults<T: ToValue + Schema + Default>(settings: &T) -> Vec<NonDefault> {
    let (Ok(schema), Ok(now), Ok(base)) =
        (T::schema(), settings.to_value(), T::default().to_value())
    else {
        return Vec::new();
    };
    let Ok(schema) = schema::inline_refs(&schema) else {
        return Vec::new();
    };
    let Some(root) = SchemaRef::new(&schema) else {
        return Vec::new();
    };
    let Ok(found) = schema::differences(root, &now, Some(&base), NOT_SHOWN) else {
        return Vec::new();
    };
    let render = |value: Option<&guatiao::Value>| {
        value
            .and_then(|v| json::to_string_of(v, Presentation::new()).ok())
            .unwrap_or_else(|| String::from("null"))
    };
    found
        .iter()
        .map(|difference| NonDefault {
            setting: difference.key(),
            value: render(difference.value.as_ref()),
            default: render(difference.baseline.as_ref()),
        })
        .collect()
}
