//! A [`Table`] kept in one private file, in whatever serde format the caller supplies.

use core::cell::Cell;
use core::fmt;
use core::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use coqui_access::guatiao::{FromWalk, Walk};
use guatiao_serde::{BridgeError, Presentation, Walked, from_deserializer};
use serde::Serialize;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};

use super::{Stamp, read_bounded, stamp, write_private};
use crate::table::{Keyed, Revision, Snapshot, SourceError, Table};

/// The most a table file may hold unless [`FileTable::max_bytes`] says otherwise: 1 MiB.
pub const DEFAULT_MAX_BYTES: usize = 1024 * 1024;

/// A format refused a document. It carries nothing of the document, which may hold secrets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CodecError;

impl CodecError {
    /// The error a [`Codec`] answers for any refusal.
    pub const fn new() -> Self {
        Self
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the format refused the document")
    }
}

impl core::error::Error for CodecError {}

/// A serde text format a [`FileTable`] keeps its records in. The kit names no format: the
/// caller implements this over the format crate it picks.
///
/// ```
/// use coqui_access_kit::file::{Codec, CodecError};
/// use serde::Serialize;
/// use serde::de::DeserializeSeed;
///
/// struct Json;
///
/// impl Codec for Json {
///     fn name(&self) -> &'static str {
///         "JSON"
///     }
///
///     fn write<D: Serialize + ?Sized>(&self, document: &D) -> Result<String, CodecError> {
///         serde_json::to_string_pretty(document).map_err(|_| CodecError::new())
///     }
///
///     fn read<'de, S: DeserializeSeed<'de>>(
///         &self,
///         text: &'de str,
///         seed: S,
///     ) -> Result<S::Value, CodecError> {
///         let mut deserializer = serde_json::Deserializer::from_str(text);
///         let read = seed.deserialize(&mut deserializer).map_err(|_| CodecError::new())?;
///         deserializer.end().map_err(|_| CodecError::new())?;
///         Ok(read)
///     }
/// }
/// ```
pub trait Codec: Send + Sync {
    /// The format's name, as an error message shows it: `TOML`, `JSON`.
    fn name(&self) -> &'static str;

    /// How bytes and numbers are spelled in this format. Default [`Presentation::new`].
    fn presentation(&self) -> Presentation {
        Presentation::new()
    }

    /// Writes `document` as text.
    ///
    /// # Errors
    ///
    /// [`CodecError`] when the format cannot write it.
    fn write<D: Serialize + ?Sized>(&self, document: &D) -> Result<String, CodecError>;

    /// Reads `text` through `seed`, refusing anything after the document.
    ///
    /// # Errors
    ///
    /// [`CodecError`] when `text` is not a document of this format or `seed` refuses it.
    fn read<'de, S: DeserializeSeed<'de>>(
        &self,
        text: &'de str,
        seed: S,
    ) -> Result<S::Value, CodecError>;
}

/// A table of `T` kept in one private file: a document with one key, `section`, holding the
/// list of records, written by codec `C`.
///
/// Records cross through their guatiao derives (`ToValue`/`FromValue`, which also stream)
/// and guatiao-serde's bridge, with no tree in between. A self-describing format reads a
/// byte string back only as the codec's [`Presentation`] spells it.
///
/// A missing file is an empty table. Every read compares the file's [`Stamp`], so a write by
/// another process is a new revision; a file that is unreadable, oversized or invalid is an
/// error, never a stale answer. Errors never contain file contents.
pub struct FileTable<T, C> {
    path: PathBuf,
    section: String,
    codec: C,
    max_bytes: usize,
    state: Mutex<State>,
    _records: PhantomData<fn() -> T>,
}

struct State {
    stamp: Option<Stamp>,
    revision: u64,
}

impl<T, C: Codec> fmt::Debug for FileTable<T, C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileTable")
            .field("path", &self.path)
            .field("section", &self.section)
            .field("format", &self.codec.name())
            .field("max_bytes", &self.max_bytes)
            .finish_non_exhaustive()
    }
}

impl<T, C> FileTable<T, C> {
    /// The table under `section` in the file at `path`, which need not exist yet, written
    /// with `codec`. Nothing is read until the table is used.
    pub fn new(path: impl Into<PathBuf>, section: impl Into<String>, codec: C) -> Self {
        Self {
            path: path.into(),
            section: section.into(),
            codec,
            max_bytes: DEFAULT_MAX_BYTES,
            state: Mutex::new(State {
                stamp: None,
                revision: 0,
            }),
            _records: PhantomData,
        }
    }

    /// The most the file may hold, in bytes, read or written. Default [`DEFAULT_MAX_BYTES`].
    #[must_use]
    pub fn max_bytes(mut self, max: usize) -> Self {
        self.max_bytes = max;
        self
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl<T: Walk + FromWalk, C: Codec> FileTable<T, C> {
    fn decode(&self, text: &str) -> Result<Vec<T>, SourceError> {
        let refusal = Cell::new(None);
        let seed = DocumentSeed {
            section: &self.section,
            presentation: self.codec.presentation(),
            refusal: &refusal,
            _records: PhantomData,
        };
        self.codec.read(text, seed).map_err(|_| {
            SourceError::invalid(match refusal.get() {
                Some(reason) => reason.to_owned(),
                None => format!("file is not a valid {} table", self.codec.name()),
            })
        })
    }

    fn encode(&self, records: &[T]) -> Result<String, SourceError> {
        let document = Document {
            section: &self.section,
            records,
            presentation: self.codec.presentation(),
        };
        let text = self
            .codec
            .write(&document)
            .map_err(|_| SourceError::invalid("records cannot be encoded"))?;
        if text.len() > self.max_bytes {
            return Err(SourceError::invalid("table would exceed its size limit"));
        }
        Ok(text)
    }

    /// Reads the file and notes a new revision when it is not the one last seen.
    fn read(&self, state: &mut State) -> Result<Vec<T>, SourceError> {
        let before = stamp(&self.path).map_err(SourceError::unavailable)?;
        let records =
            match read_bounded(&self.path, self.max_bytes).map_err(SourceError::unavailable)? {
                None => Vec::new(),
                Some(text) => self.decode(&text)?,
            };
        let after = stamp(&self.path).map_err(SourceError::unavailable)?;
        if before != after {
            return Err(SourceError::unavailable("file changed while it was read"));
        }
        if after != state.stamp {
            state.stamp = after;
            state.revision += 1;
        }
        Ok(records)
    }

    fn write(&self, state: &mut State, records: &[T]) -> Result<(), SourceError> {
        let text = self.encode(records)?;
        write_private(&self.path, text.as_bytes()).map_err(SourceError::unavailable)?;
        state.stamp = stamp(&self.path).map_err(SourceError::unavailable)?;
        state.revision += 1;
        Ok(())
    }
}

impl<T, C> Table<T> for FileTable<T, C>
where
    T: Keyed + Clone + Send + Walk + FromWalk,
    C: Codec,
{
    fn load(&self) -> Result<Snapshot<T>, SourceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let records = self.read(&mut state)?;
        Ok(Snapshot::new(Revision::new(state.revision), records))
    }

    fn changes_since(&self, since: Revision) -> Result<Option<Snapshot<T>>, SourceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = stamp(&self.path).map_err(SourceError::unavailable)?;
        if now == state.stamp && since.get() == state.revision {
            return Ok(None);
        }
        let records = self.read(&mut state)?;
        Ok(Some(Snapshot::new(Revision::new(state.revision), records)))
    }

    fn put(&self, record: T) -> Result<(), SourceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut records = self.read(&mut state)?;
        match records.iter().position(|had| had.key() == record.key()) {
            Some(at) => records[at] = record,
            None => records.push(record),
        }
        self.write(&mut state, &records)
    }

    fn remove(&self, key: &str) -> Result<bool, SourceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut records = self.read(&mut state)?;
        let before = records.len();
        records.retain(|record| record.key() != key);
        if records.len() == before {
            return Ok(false);
        }
        self.write(&mut state, &records)?;
        Ok(true)
    }
}

/// The document written: `{ <section>: [record, ..] }`.
struct Document<'a, T> {
    section: &'a str,
    records: &'a [T],
    presentation: Presentation,
}

impl<T: Walk> Serialize for Document<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(self.section, &Records(self.records, self.presentation))?;
        map.end()
    }
}

struct Records<'a, T>(&'a [T], Presentation);

impl<T: Walk> Serialize for Records<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut list = serializer.serialize_seq(Some(self.0.len()))?;
        for record in self.0 {
            list.serialize_element(&Walked::new(record, self.1))?;
        }
        list.end()
    }
}

/// Reads the document back. Each refusal of its own is noted in `refusal`, since a format's
/// error may quote the document and is never shown.
struct DocumentSeed<'a, T> {
    section: &'a str,
    presentation: Presentation,
    refusal: &'a Cell<Option<&'static str>>,
    _records: PhantomData<fn() -> T>,
}

impl<T> DocumentSeed<'_, T> {
    fn refuse<E: de::Error>(&self, reason: &'static str) -> E {
        self.refusal.set(Some(reason));
        E::custom(reason)
    }
}

impl<'de, T: FromWalk> DeserializeSeed<'de> for DocumentSeed<'_, T> {
    type Value = Vec<T>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Vec<T>, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de, T: FromWalk> Visitor<'de> for DocumentSeed<'_, T> {
    type Value = Vec<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a document of records")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec<T>, A::Error> {
        let mut records = None;
        while let Some(key) = map.next_key::<String>()? {
            if key != self.section {
                return Err(self.refuse("file has an unknown top-level key"));
            }
            if records.is_some() {
                return Err(self.refuse("file holds its records twice"));
            }
            records = Some(map.next_value_seed(RecordsSeed {
                presentation: self.presentation,
                refusal: self.refusal,
                _records: PhantomData::<fn() -> T>,
            })?);
        }
        Ok(records.unwrap_or_default())
    }
}

struct RecordsSeed<'a, T> {
    presentation: Presentation,
    refusal: &'a Cell<Option<&'static str>>,
    _records: PhantomData<fn() -> T>,
}

impl<'de, T: FromWalk> DeserializeSeed<'de> for RecordsSeed<'_, T> {
    type Value = Vec<T>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Vec<T>, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de, T: FromWalk> Visitor<'de> for RecordsSeed<'_, T> {
    type Value = Vec<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a list of records")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut list: A) -> Result<Vec<T>, A::Error> {
        let mut records = Vec::new();
        while let Some(record) = list.next_element_seed(RecordSeed {
            presentation: self.presentation,
            refusal: self.refusal,
            _record: PhantomData::<fn() -> T>,
        })? {
            records.push(record);
        }
        Ok(records)
    }
}

struct RecordSeed<'a, T> {
    presentation: Presentation,
    refusal: &'a Cell<Option<&'static str>>,
    _record: PhantomData<fn() -> T>,
}

impl<'de, T: FromWalk> DeserializeSeed<'de> for RecordSeed<'_, T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<T, D::Error> {
        from_deserializer::<T, D>(deserializer, self.presentation).map_err(|error| match error {
            BridgeError::Format(error) => error,
            BridgeError::Refused(_) => {
                const REASON: &str = "a record in the file is invalid";
                self.refusal.set(Some(REASON));
                de::Error::custom(REASON)
            }
        })
    }
}

#[cfg(test)]
mod tests;
