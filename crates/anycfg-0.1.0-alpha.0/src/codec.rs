//! The format interface a caller implements, and how a codec reports a failure.

use core::error::Error as StdError;
use core::fmt;
use core::marker::PhantomData;

/// Why a codec refused a document or a value: a position and a message, never its text.
///
/// `message` is shown to whoever reads the error, and that may be a log. **It must hold no text
/// from the document**, which may be a secret: say what was expected, name a key where that is
/// the failure, and never quote a value or a line. A parser's own message is passed on only
/// when it is known not to quote the document; `toml`'s `Display` and `serde-saphyr`'s do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CodecError {
    /// One-based line number when known.
    pub line: Option<usize>,
    /// One-based column number when known.
    pub column: Option<usize>,
    /// What was wrong, with no text from the document.
    pub message: Option<String>,
}

impl CodecError {
    /// A failure with no position and no message.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The failure at the byte `offset` of `input`, counted in lines and characters.
    ///
    /// For a parser that reports an offset rather than a position. An offset past the end
    /// counts as the end.
    #[must_use]
    pub fn at_offset(input: &[u8], offset: usize) -> Self {
        let before = &input[..offset.min(input.len())];
        let line_start = before
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |newline| newline + 1);
        let is_character_start = |byte: &&u8| (**byte & 0b1100_0000) != 0b1000_0000;
        Self {
            line: Some(before.iter().filter(|byte| **byte == b'\n').count() + 1),
            column: Some(
                before[line_start..]
                    .iter()
                    .filter(is_character_start)
                    .count()
                    + 1,
            ),
            message: None,
        }
    }

    /// Set the one-based line.
    #[must_use]
    pub fn line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }

    /// Set the one-based column.
    #[must_use]
    pub fn column(mut self, column: usize) -> Self {
        self.column = Some(column);
        self
    }

    /// Say what was wrong, with no text from the document.
    #[must_use]
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message.as_deref().unwrap_or("codec failed"))?;
        match (self.line, self.column) {
            (Some(line), Some(column)) => write!(f, " at line {line}, column {column}"),
            (Some(line), None) => write!(f, " at line {line}"),
            _ => Ok(()),
        }
    }
}

impl StdError for CodecError {}

/// Converts a registered document representation to and from bytes.
///
/// The codec owns all format-specific semantics, including number and date
/// fidelity. Implementations must never execute code or fetch external data
/// unless their consumer explicitly opts in, and a failure must quote nothing
/// from the document ([`CodecError`]).
pub trait Codec<V> {
    /// Decode exactly one document into the consumer's value model.
    ///
    /// `input` has no byte-order mark and no format line.
    ///
    /// # Errors
    ///
    /// A [`CodecError`] when the bytes are not a document in this format.
    fn decode(&self, input: &[u8]) -> Result<V, CodecError>;

    /// Encode one value into a complete document.
    ///
    /// # Errors
    ///
    /// A [`CodecError`] when the value has no form in this format.
    fn encode(&self, value: &V) -> Result<Vec<u8>, CodecError>;
}

/// Build a [`Codec`] from a decoder and an encoder callback.
pub struct Callbacks<V, D, E> {
    decode: D,
    encode: E,
    marker: PhantomData<fn() -> V>,
}

impl<V, D, E> fmt::Debug for Callbacks<V, D, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Callbacks").finish_non_exhaustive()
    }
}

impl<V, D, E> Callbacks<V, D, E> {
    /// Create a codec from `decode(&[u8])` and `encode(&V)` callbacks.
    #[must_use]
    pub fn new(decode: D, encode: E) -> Self {
        Self {
            decode,
            encode,
            marker: PhantomData,
        }
    }
}

impl<V, D, E> Codec<V> for Callbacks<V, D, E>
where
    D: Fn(&[u8]) -> Result<V, CodecError>,
    E: Fn(&V) -> Result<Vec<u8>, CodecError>,
{
    fn decode(&self, input: &[u8]) -> Result<V, CodecError> {
        (self.decode)(input)
    }

    fn encode(&self, value: &V) -> Result<Vec<u8>, CodecError> {
        (self.encode)(value)
    }
}
