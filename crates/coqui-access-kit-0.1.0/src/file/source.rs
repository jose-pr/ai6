//! Where a table is kept, as a provider's configuration names it.

use std::sync::Arc;

use coqui_access::guatiao::{FromValue, FromWalk, Schema, ToValue, Walk};

use super::table::{Codec, FileTable};
use crate::table::{Keyed, SourceError, Table};

/// Where a table is kept: an object holding exactly one source kind, open to more kinds (a
/// SQL database, an HTTP API) without changing what sits beside it.
///
/// `F` is the caller's format choice, a [`Codec`] that is also configuration (an enum of the
/// formats a plugin offers). In a configuration:
///
/// ```json
/// { "file": { "path": "accounts.toml", "format": "toml" } }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao)]
pub struct Source<F> {
    /// Records in one private file.
    pub file: Option<FileSource<F>>,
}

/// A table kept in one private file.
#[derive(Debug, Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao)]
pub struct FileSource<F> {
    /// The file. Created, readable by its owner alone, when the first record is put; a
    /// missing file is an empty table.
    pub path: String,
    /// How the file is written.
    pub format: F,
}

impl<F> Source<F> {
    /// The file at `path`, written in `format`.
    pub fn file(path: impl Into<String>, format: F) -> Self {
        Self {
            file: Some(FileSource {
                path: path.into(),
                format,
            }),
        }
    }

    /// The same kind of source beside this one, for a second table: a file source becomes
    /// the file named `<path>.<suffix>` in the same format.
    #[must_use]
    pub fn sibling(&self, suffix: &str) -> Self
    where
        F: Clone,
    {
        Self {
            file: self.file.as_ref().map(|file| FileSource {
                path: format!("{}.{suffix}", file.path),
                format: file.format.clone(),
            }),
        }
    }

    /// The table of `T` under `section` this source names. Nothing is read yet.
    ///
    /// # Errors
    ///
    /// [`SourceError::Invalid`] when no source kind is set or the file's path is blank.
    pub fn open<T>(&self, section: &str) -> Result<Arc<dyn Table<T>>, SourceError>
    where
        F: Codec + Clone + 'static,
        T: Keyed + Clone + Send + Walk + FromWalk + 'static,
    {
        match &self.file {
            Some(file) if !file.path.trim().is_empty() => Ok(Arc::new(FileTable::new(
                &file.path,
                section,
                file.format.clone(),
            ))),
            Some(_) => Err(SourceError::invalid("source.file.path must name the file")),
            None => Err(SourceError::invalid("source must name a source kind: file")),
        }
    }
}
