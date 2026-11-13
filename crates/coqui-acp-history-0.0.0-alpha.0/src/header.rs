//! The header, the first line of every history.

use serde::Serialize;
use serde_json::Value;

use crate::HistoryError;
use crate::settings::{Bounds, PARSER_DEPTH};
use crate::shape::{Fields, is_absolute_path, is_harness_name, is_timestamp, is_uuid};

/// The `format` of a header.
pub const FORMAT: &str = "coqui-acp-history";

/// The format version this crate reads and writes.
pub const VERSION: u64 = 1;

/// The order the header's keys are written in.
const ORDER: [&str; 13] = [
    "format",
    "version",
    "id",
    "origin",
    "createdAt",
    "model",
    "mode",
    "effort",
    "cwd",
    "acpVersion",
    "digest",
    "redacted",
    "_meta",
];

/// The harness a history was recorded from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Origin {
    /// Its name: lower-case letters, digits and `.`, `_`, `-`, `:` (`claude`, `codex`, `agy`).
    pub harness: String,
    /// Its version, as it reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Its own id for the session, when it has one: not the host's UUID, which is the header's
    /// [`id`](Header::id).
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl Origin {
    /// An origin named `harness`, with nothing else known.
    #[must_use]
    pub fn new(harness: impl Into<String>) -> Self {
        Self {
            harness: harness.into(),
            version: None,
            session_id: None,
        }
    }
}

/// What a redaction left in the header: the history is not as the harness recorded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Redacted {
    /// How many rules (values and patterns) were applied.
    pub rules: u64,
    /// How many removals were made.
    pub replaced: u64,
}

impl Redacted {
    /// A mark for `replaced` removals by `rules` rules.
    #[must_use]
    pub fn new(rules: u64, replaced: u64) -> Self {
        Self { rules, replaced }
    }
}

/// The first line of a history: what it is, where it came from, and the session's name.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(into = "Wire")]
pub struct Header {
    /// The host's UUID for the session: canonical lower-case, 36 characters.
    pub id: String,
    /// The harness it was recorded from.
    pub origin: Origin,
    /// When the session began, as an RFC 3339 timestamp.
    pub created_at: Option<String>,
    /// The model, as the harness names it.
    pub model: Option<String>,
    /// The permission mode.
    pub mode: Option<String>,
    /// The reasoning effort.
    pub effort: Option<String>,
    /// The absolute working directory of the session, in the path style of the machine it was
    /// recorded on. It is a description: an importer never builds a path from it.
    pub cwd: Option<String>,
    /// The ACP version the updates are in (1 or 2).
    pub acp_version: Option<u64>,
    /// `sha256:` and the lower-case hexadecimal SHA-256 of the body: every byte after the header
    /// line, line feeds included. A reader checks it when it reaches the end.
    pub digest: Option<String>,
    /// Present when a redaction changed the body.
    pub redacted: Option<Redacted>,
    /// Anything else, as an object; the format reads none of it.
    pub meta: Option<Value>,
}

#[derive(Serialize)]
struct Wire {
    format: &'static str,
    version: u64,
    id: String,
    origin: Origin,
    #[serde(rename = "createdAt", skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(rename = "acpVersion", skip_serializing_if = "Option::is_none")]
    acp_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    redacted: Option<Redacted>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    meta: Option<Value>,
}

impl From<Header> for Wire {
    fn from(h: Header) -> Self {
        Self {
            format: FORMAT,
            version: VERSION,
            id: h.id,
            origin: h.origin,
            created_at: h.created_at,
            model: h.model,
            mode: h.mode,
            effort: h.effort,
            cwd: h.cwd,
            acp_version: h.acp_version,
            digest: h.digest,
            redacted: h.redacted,
            meta: h.meta,
        }
    }
}

impl Header {
    /// A header for session `id` recorded from `origin`, with nothing else known.
    #[must_use]
    pub fn new(id: impl Into<String>, origin: Origin) -> Self {
        Self {
            id: id.into(),
            origin,
            created_at: None,
            model: None,
            mode: None,
            effort: None,
            cwd: None,
            acp_version: None,
            digest: None,
            redacted: None,
            meta: None,
        }
    }

    /// The header as its one line, with no line feed, under the default bounds.
    ///
    /// # Errors
    ///
    /// [`HistoryError::Shape`] when a field is not what the format allows, [`HistoryError::TooDeep`]
    /// or [`HistoryError::TooManyValues`] past the default bounds: the refusals reading makes.
    pub fn to_line(&self) -> Result<String, HistoryError> {
        self.to_line_within(&Bounds::default())
    }

    /// The header as its one line, held to `bounds` as a reader holding them would.
    pub(crate) fn to_line_within(&self, bounds: &Bounds) -> Result<String, HistoryError> {
        let shape = HistoryError::Shape {
            line: 1,
            pointer: String::new(),
            expected: "a header that serialises",
        };
        // The header is one level of nesting itself.
        if self
            .meta
            .as_ref()
            .is_some_and(|meta| crate::strict::depth(meta) >= PARSER_DEPTH)
        {
            return Err(HistoryError::too_deep(1, PARSER_DEPTH));
        }
        let Ok(value) = serde_json::to_value(self) else {
            return Err(shape);
        };
        let text = crate::canon::to_string(&value, &ORDER)
            .ok_or_else(|| HistoryError::too_deep(1, PARSER_DEPTH))?;
        let value = crate::strict::parse(&text, 1, bounds)?;
        Self::from_value(value)?;
        Ok(text)
    }

    /// Checks a parsed first line (`format`, then `version`, then every field) and takes its
    /// `_meta` out of it rather than copying it.
    pub(crate) fn from_value(mut value: Value) -> Result<Self, HistoryError> {
        let mut header = Self::check(&value)?;
        if let Value::Object(map) = &mut value {
            header.meta = map.remove("_meta");
        }
        Ok(header)
    }

    /// Checks a parsed first line, with `meta` left unset.
    fn check(value: &Value) -> Result<Self, HistoryError> {
        let root = Fields::root(value, 1)?;
        if root.raw("format").and_then(Value::as_str) != Some(FORMAT) {
            return Err(HistoryError::UnknownFormat);
        }
        match root.raw("version") {
            Some(Value::Number(n)) => match n.as_u64() {
                Some(VERSION) => {}
                Some(version) => return Err(HistoryError::UnknownVersion { version }),
                None => return Err(root.shape("version", "an unsigned integer")),
            },
            _ => return Err(root.shape("version", "an unsigned integer")),
        }
        root.only(&[
            "format",
            "version",
            "id",
            "origin",
            "createdAt",
            "model",
            "mode",
            "effort",
            "cwd",
            "acpVersion",
            "digest",
            "redacted",
            "_meta",
        ])?;

        let id = root.require_string("id", 36, "a lower-case UUID")?;
        if !is_uuid(id) {
            return Err(root.shape("id", "a lower-case UUID"));
        }

        let origin = root.require_object("origin")?;
        origin.only(&["harness", "version", "sessionId"])?;
        let harness = origin.require_string("harness", 64, "a harness name")?;
        if !is_harness_name(harness) {
            return Err(HistoryError::Shape {
                line: 1,
                pointer: String::from("/origin/harness"),
                expected: "a harness name",
            });
        }

        let created_at = root.string("createdAt", 40, "an RFC 3339 timestamp")?;
        if created_at.is_some_and(|t| !is_timestamp(t)) {
            return Err(root.shape("createdAt", "an RFC 3339 timestamp"));
        }
        let digest = root.string("digest", 71, "sha256: and 64 hexadecimal digits")?;
        if digest.is_some_and(|d| !is_digest(d)) {
            return Err(root.shape("digest", "sha256: and 64 hexadecimal digits"));
        }
        let acp_version = root.unsigned("acpVersion")?;
        if acp_version == Some(0) {
            return Err(root.shape("acpVersion", "a version of 1 or more"));
        }
        let redacted = match root.object("redacted")? {
            None => None,
            Some(r) => {
                r.only(&["rules", "replaced"])?;
                Some(Redacted {
                    rules: r
                        .unsigned("rules")?
                        .ok_or_else(|| r.shape("rules", "an unsigned integer"))?,
                    replaced: r
                        .unsigned("replaced")?
                        .ok_or_else(|| r.shape("replaced", "an unsigned integer"))?,
                })
            }
        };
        if root.raw("_meta").is_some_and(|meta| !meta.is_object()) {
            return Err(root.shape("_meta", "an object"));
        }

        let cwd = root.plain("cwd", 4096, "an absolute path")?;
        if cwd.is_some_and(|path| !is_absolute_path(path)) {
            return Err(root.shape("cwd", "an absolute path"));
        }
        let owned = |value: Option<&str>| value.map(str::to_owned);
        Ok(Self {
            id: id.to_owned(),
            origin: Origin {
                harness: harness.to_owned(),
                version: owned(origin.plain("version", 128, "text with no control character")?),
                session_id: owned(origin.plain(
                    "sessionId",
                    256,
                    "text with no control character",
                )?),
            },
            created_at: owned(created_at),
            model: owned(root.plain("model", 256, "text with no control character")?),
            mode: owned(root.plain("mode", 256, "text with no control character")?),
            effort: owned(root.plain("effort", 256, "text with no control character")?),
            cwd: owned(cwd),
            acp_version,
            digest: owned(digest),
            redacted,
            meta: None,
        })
    }
}

/// Whether `text` is `sha256:` and 64 lower-case hexadecimal digits.
pub(crate) fn is_digest(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}
