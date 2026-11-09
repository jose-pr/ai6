//! Ordered inputs and user-defined value merging.

use alloc::borrow::Cow;
use core::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::Deserializer;
use serde::de::DeserializeOwned;

use crate::codec::CodecError;
use crate::convert;
use crate::error::Error;
use crate::formats::{DEFAULT_MAX_DEPTH, DEFAULT_MAX_SOURCE_BYTES, Formats, Options, in_file};

/// Default maximum size of all documents combined (64 MiB).
pub const DEFAULT_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// How later values affect accumulated configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Merge {
    /// Replace the previous root completely.
    Replace,
    /// Merge top-level mapping entries only.
    Shallow,
    /// Merge nested mappings; the adapter decides what happens to other values.
    #[default]
    Deep,
}

/// Supplies merging and structural validation for a consumer-owned value type.
///
/// The library does not assume what constitutes a mapping, array, number or
/// opaque value. Implementations define those semantics, including replacement,
/// deletion, and the meaning of a deep merge.
pub trait MergeValues<V> {
    /// Check an individual document against the caller's depth budget.
    ///
    /// The default is a no-op for flat or non-recursive value models.
    ///
    /// # Errors
    ///
    /// [`Error::TooDeep`] for a document nested beyond `_max_depth`.
    fn validate(&self, _value: &V, _max_depth: usize) -> Result<(), Error> {
        Ok(())
    }

    /// Merge `next` into `previous` according to the selected strategy.
    ///
    /// Called only for the second and subsequent present sources. An adapter
    /// must handle `Merge::Replace`, or use [`ReplaceValues`] instead.
    ///
    /// # Errors
    ///
    /// [`Error::MergeShape`] for values this strategy cannot combine.
    fn merge(
        &self,
        previous: &mut V,
        next: V,
        strategy: Merge,
        max_depth: usize,
    ) -> Result<(), Error>;
}

/// An adapter that replaces earlier values; no structural interpretation.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReplaceValues;

impl<V> MergeValues<V> for ReplaceValues {
    fn merge(
        &self,
        previous: &mut V,
        next: V,
        strategy: Merge,
        _max_depth: usize,
    ) -> Result<(), Error> {
        if strategy != Merge::Replace {
            return Err(Error::MergeShape);
        }
        *previous = next;
        Ok(())
    }
}

/// Use a callback to implement merge behavior for any value type.
impl<V, F> MergeValues<V> for F
where
    F: Fn(&mut V, V, Merge, usize) -> Result<(), Error>,
{
    fn merge(
        &self,
        previous: &mut V,
        next: V,
        strategy: Merge,
        max_depth: usize,
    ) -> Result<(), Error> {
        self(previous, next, strategy, max_depth)
    }
}

enum Input<'a> {
    Bytes(&'a [u8]),
    Reader(Box<dyn Read + 'a>),
    Path(&'a Path),
}

/// An input with optional format hints; paths are read only during loading.
pub struct Source<'a> {
    input: Input<'a>,
    name: Option<&'a str>,
    format: Option<&'a str>,
}

impl fmt::Debug for Source<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Source")
            .field("name", &self.name)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl<'a> Source<'a> {
    /// Borrow a byte slice without copying its content.
    #[must_use]
    pub fn bytes(bytes: &'a [u8]) -> Self {
        Self {
            input: Input::Bytes(bytes),
            name: None,
            format: None,
        }
    }

    /// Borrow a UTF-8 string without copying its content.
    #[must_use]
    pub fn text(text: &'a str) -> Self {
        Self::bytes(text.as_bytes())
    }

    /// Consume a synchronous reader while loading.
    #[must_use]
    pub fn reader(reader: impl Read + 'a) -> Self {
        Self {
            input: Input::Reader(Box::new(reader)),
            name: None,
            format: None,
        }
    }

    /// Read a path only when loading, selecting a format from its extension.
    #[must_use]
    pub fn path(path: &'a Path) -> Self {
        Self {
            input: Input::Path(path),
            name: None,
            format: None,
        }
    }

    /// Set a logical name used only for its extension.
    #[must_use]
    pub fn name(mut self, name: &'a str) -> Self {
        self.name = Some(name);
        self
    }

    /// Force a registered format by name, without removing a format line.
    #[must_use]
    pub fn format(mut self, format: &'a str) -> Self {
        self.format = Some(format);
        self
    }
}

/// Load and combine registered-format documents as a consumer-owned value type.
///
/// Input is limited before a decoder is called. The loader has no dependency on a concrete
/// document format or a fixed intermediate value representation.
#[derive(Debug)]
pub struct Loader<'f, V> {
    formats: &'f Formats<V>,
    strategy: Merge,
    default_format: String,
    max_source_bytes: usize,
    max_total_bytes: usize,
    max_depth: usize,
}

impl<'f, V> Loader<'f, V> {
    /// Create a loader with a required default format, deep merging, and bounded reads.
    ///
    /// The default must name a registered codec. Explicit formats, fixed
    /// extensions, and matching content directives take precedence.
    #[must_use]
    pub fn new(formats: &'f Formats<V>, default_format: impl Into<String>) -> Self {
        Self {
            formats,
            strategy: Merge::Deep,
            default_format: default_format.into(),
            max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }

    /// Select how the consumer's value adapter combines layers.
    #[must_use]
    pub fn merge(mut self, strategy: Merge) -> Self {
        self.strategy = strategy;
        self
    }

    /// Set the fallback for unnamed streams and unpinned auto-detect extensions.
    /// Per-source format overrides and explicit extension defaults take priority.
    #[must_use]
    pub fn default_format(mut self, format: impl Into<String>) -> Self {
        self.default_format = format.into();
        self
    }

    /// Limit the total bytes of any single source.
    #[must_use]
    pub fn max_source_bytes(mut self, max_bytes: usize) -> Self {
        self.max_source_bytes = max_bytes;
        self
    }

    /// Limit the combined bytes of all present sources.
    #[must_use]
    pub fn max_total_bytes(mut self, max_bytes: usize) -> Self {
        self.max_total_bytes = max_bytes;
        self
    }

    /// Limit nesting: passed to the caller's value adapter, and enforced by a typed load.
    /// A root value is at depth 0.
    #[must_use]
    pub fn max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Decode and merge sources in order; later sources win per the adapter.
    ///
    /// `None` entries are skipped, for direct use with [`crate::default_configs`].
    /// The adapter handles both merge shape and nesting constraints. Without any
    /// present sources, this returns [`Error::NoSources`].
    ///
    /// # Errors
    ///
    /// Fails for malformed input, exceeded budgets or a rejected merge.
    pub fn load<'a, I, S, M>(&self, sources: I, adapter: &M) -> Result<V, Error>
    where
        I: IntoIterator<Item = S>,
        S: Into<Option<Source<'a>>>,
        M: MergeValues<V>,
    {
        let mut document: Option<V> = None;
        let mut bytes_total = 0usize;
        for (index, source) in sources.into_iter().enumerate() {
            let Some(source) = source.into() else {
                continue;
            };
            let next = self
                .read_source(source, &mut bytes_total)
                .and_then(|value| {
                    adapter.validate(&value, self.max_depth)?;
                    Ok(value)
                })
                .map_err(|cause| Error::Source {
                    index,
                    cause: Box::new(cause),
                })?;
            if let Some(previous) = document.as_mut() {
                adapter
                    .merge(previous, next, self.strategy, self.max_depth)
                    .map_err(|cause| Error::Source {
                        index,
                        cause: Box::new(cause),
                    })?;
            } else {
                document = Some(next);
            }
        }
        document.ok_or(Error::NoSources)
    }

    /// Load and then convert the merged value using a callback.
    ///
    /// A converter can hydrate a typed structure, retain an opaque value, or
    /// translate into another representation. It reports a failure as a
    /// [`CodecError`], whose message must quote nothing from the document.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Conversion`] with that message if the callback fails.
    pub fn load_as<'a, I, S, M, T, F>(
        &self,
        sources: I,
        adapter: &M,
        convert: F,
    ) -> Result<T, Error>
    where
        I: IntoIterator<Item = S>,
        S: Into<Option<Source<'a>>>,
        M: MergeValues<V>,
        F: FnOnce(V) -> Result<T, CodecError>,
    {
        let value = self.load(sources, adapter)?;
        convert(value).map_err(|error| Error::Conversion {
            path: None,
            message: error.message,
        })
    }

    /// Merge sources and deserialize the result into a Serde type.
    ///
    /// The value model `V` is selected by the consumer and must implement
    /// [`serde::Deserializer`]. The merge adapter retains control of layering
    /// and special value semantics. `T` may be any owned Serde-deserializable
    /// type; no specific JSON, TOML, or YAML value type is required.
    ///
    /// # Errors
    ///
    /// The usual source and merge errors, then as
    /// [`Formats::from_slice_typed`] for the merged value.
    pub fn load_typed<'a, I, S, M, T>(&self, sources: I, adapter: &M) -> Result<T, Error>
    where
        I: IntoIterator<Item = S>,
        S: Into<Option<Source<'a>>>,
        M: MergeValues<V>,
        V: for<'de> Deserializer<'de>,
        T: DeserializeOwned,
    {
        convert::from_value(self.load(sources, adapter)?, self.max_depth)
    }

    /// Load sources or return a caller-supplied default if all were absent.
    ///
    /// # Errors
    ///
    /// Returns errors for present but malformed or unreadable sources.
    pub fn load_or<'a, I, S, M>(&self, sources: I, adapter: &M, fallback: V) -> Result<V, Error>
    where
        I: IntoIterator<Item = S>,
        S: Into<Option<Source<'a>>>,
        M: MergeValues<V>,
    {
        match self.load(sources, adapter) {
            Ok(value) => Ok(value),
            Err(Error::NoSources) => Ok(fallback),
            Err(err) => Err(err),
        }
    }

    fn read_source(&self, source: Source<'_>, bytes_total: &mut usize) -> Result<V, Error> {
        match source.input {
            Input::Path(path) => self
                .read_input(source, bytes_total)
                .map_err(|cause| in_file(path, cause)),
            _ => self.read_input(source, bytes_total),
        }
    }

    fn read_input(&self, source: Source<'_>, bytes_total: &mut usize) -> Result<V, Error> {
        let name = match (&source.input, source.name) {
            (_, Some(name)) => Some(Cow::Borrowed(name)),
            (Input::Path(path), None) => Some(Cow::Owned(path.to_string_lossy().into_owned())),
            _ => None,
        };
        let remaining = self.max_total_bytes.saturating_sub(*bytes_total);
        let limit = self.max_source_bytes.min(remaining);
        let exceeded_limit = if remaining < self.max_source_bytes {
            self.max_total_bytes
        } else {
            self.max_source_bytes
        };
        let bytes = match source.input {
            Input::Bytes(bytes) => Cow::Borrowed(bytes),
            Input::Reader(reader) => Cow::Owned(
                read_bounded(reader, limit).map_err(|e| translate_budget(e, exceeded_limit))?,
            ),
            Input::Path(path) => Cow::Owned(
                read_bounded(File::open(path)?, limit)
                    .map_err(|e| translate_budget(e, exceeded_limit))?,
            ),
        };
        if bytes.len() > self.max_source_bytes {
            return Err(Error::TooLarge {
                limit: self.max_source_bytes,
            });
        }
        if bytes.len() > remaining {
            return Err(Error::TooLarge {
                limit: self.max_total_bytes,
            });
        }
        *bytes_total = bytes_total
            .checked_add(bytes.len())
            .ok_or(Error::TooLarge {
                limit: self.max_total_bytes,
            })?;
        let opts = Options {
            format: source.format,
            name: name.as_deref(),
            default_format: Some(self.default_format.as_str()),
            max_bytes: limit,
            max_depth: self.max_depth,
        };
        self.formats.from_slice(bytes.as_ref(), opts)
    }
}

fn translate_budget(error: Error, bound: usize) -> Error {
    match error {
        Error::TooLarge { .. } => Error::TooLarge { limit: bound },
        other => other,
    }
}

fn read_bounded(mut source: impl Read, max_bytes: usize) -> Result<Vec<u8>, Error> {
    let mut buffer = Vec::new();
    let limit = u64::try_from(max_bytes).unwrap_or(u64::MAX - 1);
    source
        .by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut buffer)?;
    if buffer.len() > max_bytes {
        return Err(Error::TooLarge { limit: max_bytes });
    }
    Ok(buffer)
}
