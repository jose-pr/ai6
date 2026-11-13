//! The one error of the crate.

use core::fmt;
use std::io;

/// Why a history could not be read, written or redacted.
///
/// No message, field or `Debug` output carries text from the document: a line of a history may
/// hold a secret. A variant names the line (1-based, the header being line 1) and, where the
/// document had the wrong shape, the JSON pointer of the place.
#[derive(Debug)]
#[non_exhaustive]
pub enum HistoryError {
    /// The source or sink failed.
    Io(io::Error),
    /// The file holds no header line.
    Empty,
    /// A line is not UTF-8.
    NotUtf8 {
        /// The line.
        line: u64,
    },
    /// A line is empty.
    BlankLine {
        /// The line.
        line: u64,
    },
    /// A line holds a carriage return: lines end in LF alone.
    LineEnding {
        /// The line.
        line: u64,
    },
    /// The file does not end in a line feed, so its last line may be cut short.
    Truncated {
        /// The last line.
        line: u64,
    },
    /// A line is not JSON.
    Syntax {
        /// The line.
        line: u64,
        /// The 1-based column where the parser stopped.
        column: u64,
    },
    /// An object repeats a key, which two readers would resolve differently.
    DuplicateKey {
        /// The line.
        line: u64,
    },
    /// A line is JSON of the wrong shape: a field is missing, of the wrong type or out of range.
    Shape {
        /// The line.
        line: u64,
        /// The JSON pointer of the place, empty for the line itself.
        pointer: String,
        /// What was expected there.
        expected: &'static str,
    },
    /// An object has a field the format does not define at that place.
    UnknownField {
        /// The line.
        line: u64,
        /// The JSON pointer of the object that holds it.
        pointer: String,
    },
    /// The first line is not a history header (`format` is not [`FORMAT`](crate::FORMAT)).
    UnknownFormat,
    /// The header names a version this reader does not know.
    UnknownVersion {
        /// The version the file declares.
        version: u64,
    },
    /// A body line is of a kind the format does not define.
    UnknownLine {
        /// The line.
        line: u64,
    },
    /// A line is in a place the order rules forbid.
    Order {
        /// The line.
        line: u64,
        /// The rule it breaks.
        rule: OrderRule,
    },
    /// A line is longer than the bound.
    TooLong {
        /// The line.
        line: u64,
        /// The bound, in bytes.
        limit: u64,
    },
    /// The file has more lines than the bound.
    TooManyLines {
        /// The bound.
        limit: u64,
    },
    /// The file is larger than the bound.
    TooLarge {
        /// The bound, in bytes.
        limit: u64,
    },
    /// A line nests deeper than the bound.
    TooDeep {
        /// The line.
        line: u64,
        /// The nesting that was allowed: the `max_depth` setting, or the parser's own limit
        /// ([`PARSER_DEPTH`](crate::PARSER_DEPTH)) when the setting is higher.
        limit: u64,
        /// Whether `limit` is the parser's and not the setting's: `max_depth` is higher than the
        /// parser reads, so raising it further changes nothing.
        by_parser: bool,
    },
    /// A line holds more values than the bound.
    TooManyValues {
        /// The line.
        line: u64,
        /// The bound.
        limit: u64,
    },
    /// The header's digest is not the body's.
    DigestMismatch,
    /// A redaction pattern is not a regular expression this crate accepts.
    Pattern {
        /// Its position in the settings' `patterns`.
        index: usize,
    },
}

/// The ordering rule a misplaced line breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OrderRule {
    /// A header after the first line.
    HeaderRepeated,
    /// A prompt while the previous turn has not ended.
    PromptInsideTurn,
    /// A cancel, permission request, permission answer or turn end with no turn open.
    OutsideTurn,
    /// A permission answer for a call no request is waiting on.
    AnswerWithoutRequest,
    /// A permission request for a call already waiting.
    RequestRepeated,
}

impl OrderRule {
    /// The rule's name in vectors and messages.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::HeaderRepeated => "header_repeated",
            Self::PromptInsideTurn => "prompt_inside_turn",
            Self::OutsideTurn => "outside_turn",
            Self::AnswerWithoutRequest => "answer_without_request",
            Self::RequestRepeated => "request_repeated",
        }
    }
}

impl HistoryError {
    /// A line nested past what `max_depth` allows, which the parser caps at
    /// [`PARSER_DEPTH`](crate::PARSER_DEPTH).
    pub(crate) fn too_deep(line: u64, max_depth: u64) -> Self {
        Self::TooDeep {
            line,
            limit: max_depth.min(crate::PARSER_DEPTH),
            by_parser: max_depth > crate::PARSER_DEPTH,
        }
    }

    /// The stable name of the kind of failure, as the shared vectors spell it.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::Empty => "empty",
            Self::NotUtf8 { .. } => "not_utf8",
            Self::BlankLine { .. } => "blank_line",
            Self::LineEnding { .. } => "line_ending",
            Self::Truncated { .. } => "truncated",
            Self::Syntax { .. } => "syntax",
            Self::DuplicateKey { .. } => "duplicate_key",
            Self::Shape { .. } => "shape",
            Self::UnknownField { .. } => "unknown_field",
            Self::UnknownFormat => "unknown_format",
            Self::UnknownVersion { .. } => "unknown_version",
            Self::UnknownLine { .. } => "unknown_line",
            Self::Order { .. } => "order",
            Self::TooLong { .. } => "too_long",
            Self::TooManyLines { .. } => "too_many_lines",
            Self::TooLarge { .. } => "too_large",
            Self::TooDeep { .. } => "too_deep",
            Self::TooManyValues { .. } => "too_many_values",
            Self::DigestMismatch => "digest_mismatch",
            Self::Pattern { .. } => "pattern",
        }
    }

    /// The JSON pointer of the place a wrongly shaped line failed at, empty for the line itself.
    #[must_use]
    pub fn pointer(&self) -> Option<&str> {
        match self {
            Self::Shape { pointer, .. } | Self::UnknownField { pointer, .. } => Some(pointer),
            _ => None,
        }
    }

    /// The line the failure is at, when it is at one.
    #[must_use]
    pub fn line(&self) -> Option<u64> {
        match self {
            Self::NotUtf8 { line }
            | Self::BlankLine { line }
            | Self::LineEnding { line }
            | Self::Truncated { line }
            | Self::Syntax { line, .. }
            | Self::DuplicateKey { line }
            | Self::Shape { line, .. }
            | Self::UnknownField { line, .. }
            | Self::UnknownLine { line }
            | Self::Order { line, .. }
            | Self::TooLong { line, .. }
            | Self::TooDeep { line, .. }
            | Self::TooManyValues { line, .. } => Some(*line),
            Self::UnknownFormat | Self::UnknownVersion { .. } => Some(1),
            _ => None,
        }
    }
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "could not read or write the history: {error}"),
            Self::Empty => f.write_str("the history has no header line"),
            Self::NotUtf8 { line } => write!(f, "line {line} is not UTF-8"),
            Self::BlankLine { line } => write!(f, "line {line} is empty"),
            Self::LineEnding { line } => write!(f, "line {line} holds a carriage return"),
            Self::Truncated { line } => {
                write!(
                    f,
                    "line {line} does not end in a line feed, so the file may be cut short"
                )
            }
            Self::Syntax { line, column } => {
                write!(f, "line {line} is not JSON, at column {column}")
            }
            Self::DuplicateKey { line } => write!(f, "line {line} repeats an object key"),
            Self::Shape {
                line,
                pointer,
                expected,
            } => {
                write!(f, "line {line} should hold {expected} at \"{pointer}\"")
            }
            Self::UnknownField { line, pointer } => {
                write!(
                    f,
                    "line {line} has a field the format does not define in \"{pointer}\""
                )
            }
            Self::UnknownFormat => f.write_str("the first line is not a coqui-acp-history header"),
            Self::UnknownVersion { version } => {
                write!(
                    f,
                    "the history is format version {version}, which this reader does not know"
                )
            }
            Self::UnknownLine { line } => {
                write!(f, "line {line} is of a kind the format does not define")
            }
            Self::Order { line, rule } => {
                write!(f, "line {line} is out of order ({})", rule.name())
            }
            Self::TooLong { line, limit } => {
                write!(f, "line {line} is longer than {limit} bytes")
            }
            Self::TooManyLines { limit } => write!(f, "the history has more than {limit} lines"),
            Self::TooLarge { limit } => write!(f, "the history is larger than {limit} bytes"),
            Self::TooDeep {
                line,
                limit,
                by_parser: false,
            } => write!(f, "line {line} nests deeper than {limit} levels"),
            Self::TooDeep {
                line,
                limit,
                by_parser: true,
            } => write!(
                f,
                "line {line} nests deeper than {limit} levels, the parser's own limit, which \
                 max_depth cannot raise"
            ),
            Self::TooManyValues { line, limit } => {
                write!(f, "line {line} holds more than {limit} values")
            }
            Self::DigestMismatch => f.write_str("the header's digest is not the body's"),
            Self::Pattern { index } => {
                write!(
                    f,
                    "redaction pattern {index} is not a usable regular expression"
                )
            }
        }
    }
}

impl core::error::Error for HistoryError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for HistoryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
