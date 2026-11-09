//! The crate's one error type.

use core::error::Error as StdError;
use core::fmt;
use std::io;
use std::path::PathBuf;

/// Errors returned by format selection, file lookup and source loading.
///
/// A message, field or `Debug` here quotes no text from a document. The one exception is the
/// name of a key, because a refused key must be named: an unknown field, and the path of a
/// refused value. A file's path and a name the caller registered are not document text.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// An input had no format line, marker or extension, and no default format is set.
    NoDefaultFormat,
    /// A format line, or a format the caller asked for, names no registered format.
    ///
    /// The name is not repeated: a format line is text from the document.
    UnsupportedFormat {
        /// Every registered format, sorted.
        registered: Vec<String>,
    },
    /// A file's extension has no format registered for it.
    UnsupportedExtension {
        /// The extension in lower case, without its dot.
        extension: String,
        /// Every registered extension, sorted.
        registered: Vec<String>,
    },
    /// A format name or extension is invalid or already in use.
    InvalidRegistration,
    /// Output of a codec is not valid UTF-8 where UTF-8 is required.
    InvalidUtf8 {
        /// Byte offset of the first invalid UTF-8 sequence.
        offset: usize,
    },
    /// A document exceeded the configured byte limit.
    TooLarge {
        /// Maximum permitted document or input size, in bytes.
        limit: usize,
    },
    /// A document nests deeper than the configured limit.
    TooDeep {
        /// Maximum permitted nesting depth.
        limit: usize,
    },
    /// No present sources were supplied.
    NoSources,
    /// A selected merge policy could not merge the values.
    MergeShape,
    /// The document parsed but does not have the requested shape, or a conversion refused it.
    Conversion {
        /// Where in the document, such as `server.port` or `items[1]`; `None` at its root or
        /// when unknown.
        path: Option<String>,
        /// What was wrong: names a field, or what was expected and the kind of value found,
        /// never the value.
        message: Option<String>,
    },
    /// A registered decoder rejected the document.
    Parse {
        /// Registered codec name that rejected the document.
        format: String,
        /// Optional one-based source line, counted from the document's first line.
        line: Option<usize>,
        /// Optional one-based source column.
        column: Option<usize>,
        /// What the codec said was wrong ([`CodecError::message`](crate::CodecError::message)).
        message: Option<String>,
    },
    /// A registered encoder rejected the value.
    Serialize {
        /// Registered codec name that could not encode the value.
        format: String,
        /// What the codec said was wrong ([`CodecError::message`](crate::CodecError::message)).
        message: Option<String>,
    },
    /// A source failed to read, decode, validate or merge.
    Source {
        /// Zero-based index of the failing source in the input iterator.
        index: usize,
        /// Error raised while reading, decoding, validating, or merging this source.
        cause: Box<Error>,
    },
    /// Reading or decoding a file failed.
    File {
        /// The file.
        path: PathBuf,
        /// What went wrong with it.
        cause: Box<Error>,
    },
    /// A file the caller or a variable named for a search is not there.
    NotFound {
        /// The path that was named.
        path: PathBuf,
        /// The environment variable that named it, when one did.
        variable: Option<String>,
    },
    /// The user's home directory could not be determined.
    HomeUnavailable,
    /// A default configuration filename was missing or invalid.
    InvalidConfigName,
    /// Reading from or writing to a source failed.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDefaultFormat => f.write_str("no default configuration format set"),
            Self::UnsupportedFormat { registered } => write!(
                f,
                "no configuration format answers to that name (registered: {})",
                registered.join(", ")
            ),
            Self::UnsupportedExtension {
                extension,
                registered,
            } => write!(
                f,
                "configuration extension `.{extension}` is not registered (registered: {})",
                registered.join(", ")
            ),
            Self::InvalidRegistration => f.write_str("invalid or duplicate format registration"),
            Self::InvalidUtf8 { offset } => {
                write!(f, "configuration has invalid UTF-8 at byte {offset}")
            }
            Self::TooLarge { limit } => write!(f, "configuration exceeds {limit} bytes"),
            Self::TooDeep { limit } => write!(f, "configuration exceeds {limit} value levels"),
            Self::NoSources => f.write_str("no configuration sources supplied"),
            Self::MergeShape => f.write_str("configuration values cannot be merged"),
            Self::Conversion { path, message } => {
                f.write_str("invalid configuration")?;
                if let Some(path) = path {
                    write!(f, " at `{path}`")?;
                }
                if let Some(message) = message {
                    write!(f, ": {message}")?;
                }
                Ok(())
            }
            Self::Parse {
                format,
                line,
                column,
                message,
            } => {
                write!(f, "could not parse {format} configuration")?;
                match (line, column) {
                    (Some(line), Some(column)) => write!(f, " at line {line}, column {column}")?,
                    (Some(line), None) => write!(f, " at line {line}")?,
                    _ => {}
                }
                if let Some(message) = message {
                    write!(f, ": {message}")?;
                }
                Ok(())
            }
            Self::Serialize { format, message } => {
                write!(f, "could not serialize {format} configuration")?;
                if let Some(message) = message {
                    write!(f, ": {message}")?;
                }
                Ok(())
            }
            Self::Source { index, cause } => write!(f, "configuration source {index}: {cause}"),
            Self::File { path, cause } => write!(f, "{}: {cause}", path.display()),
            Self::NotFound { path, variable } => {
                write!(f, "no configuration file at {}", path.display())?;
                if let Some(variable) = variable {
                    write!(f, ", which {variable} names")?;
                }
                Ok(())
            }
            Self::HomeUnavailable => f.write_str("could not locate the user home directory"),
            Self::InvalidConfigName => {
                f.write_str("configuration filename must be nonempty and relative")
            }
            Self::Io(err) => write!(f, "configuration I/O error: {err}"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Source { cause, .. } | Self::File { cause, .. } => Some(cause),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
