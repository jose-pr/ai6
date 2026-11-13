//! A whole history in memory.

use core::str::FromStr;
use std::io::{Read, Write};

use sha2::{Digest, Sha256};

use crate::header::Header;
use crate::order::Summary;
use crate::reader::{Reader, body_digest};
use crate::seed::Seed;
use crate::settings::ReaderSettings;
use crate::{HistoryError, Line, Writer};

/// A header and the lines after it, all in memory.
///
/// Reading then writing a document gives the same lines in this crate's canonical spelling: keys
/// sorted inside ACP payloads, numbers spelled as the format document says. A file that must stay
/// byte for byte as it is goes through [`Reader`] and [`Entry::raw`](crate::Entry::raw) instead.
///
/// A document is returned only once the digest in its header, if any, has been checked, and it is
/// written with a digest that names the body as written: a header's digest is made again when the
/// spelling changes it.
///
/// It holds every line at once, a small multiple of the bytes read. [`from_slice`](Self::from_slice)
/// and [`str::parse`] hold the history to [`ReaderSettings::for_document`]; a larger history is
/// read with a [`Reader`], or with [`from_reader`](Self::from_reader) under bounds the caller chose.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Document {
    /// The first line.
    pub header: Header,
    /// The lines after it, the first being line 2.
    pub lines: Vec<Line>,
}

impl Document {
    /// A document with a `header` and no lines.
    #[must_use]
    pub fn new(header: Header) -> Self {
        Self {
            header,
            lines: Vec::new(),
        }
    }

    /// Reads every line `reader` has, under the reader's own settings, and returns only once its
    /// digest has been checked.
    ///
    /// # Errors
    ///
    /// The first refusal the reader makes.
    pub fn from_reader<R: Read>(mut reader: Reader<R>) -> Result<Self, HistoryError> {
        let mut lines = Vec::new();
        while let Some(entry) = reader.next_entry()? {
            lines.push(entry.line);
        }
        Ok(Self {
            header: reader.header().clone(),
            lines,
        })
    }

    /// Reads a history held in bytes, under [`ReaderSettings::for_document`].
    ///
    /// # Errors
    ///
    /// The first refusal the reader makes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, HistoryError> {
        Self::from_reader(
            Reader::builder(bytes)
                .settings(ReaderSettings::for_document())
                .open()?,
        )
    }

    /// Checks the lines against the order rules and counts them.
    ///
    /// # Errors
    ///
    /// The first line out of order, as the writer would refuse it.
    pub fn check(&self) -> Result<Summary, HistoryError> {
        let mut header = self.header.clone();
        header.digest = None;
        let mut writer = Writer::builder(std::io::sink()).start(&header)?;
        for line in &self.lines {
            writer.write(line)?;
        }
        Ok(writer.finish()?.1)
    }

    /// Writes the document, checking each line as the writer does. A header that has a digest is
    /// written with the digest of the body as it is written here.
    ///
    /// # Errors
    ///
    /// The first line the format refuses, or the sink's error. The sink may hold the lines before
    /// it; write to memory first where that matters.
    pub fn to_writer<W: Write>(&self, sink: W) -> Result<(), HistoryError> {
        let mut header = self.header.clone();
        if header.digest.is_some() {
            header.digest = Some(self.body_digest()?);
        }
        let mut writer = Writer::builder(sink).start(&header)?;
        for line in &self.lines {
            writer.write(line)?;
        }
        writer.finish().map(|_| ())
    }

    /// The document as bytes.
    ///
    /// # Errors
    ///
    /// The first line the format refuses.
    pub fn to_vec(&self) -> Result<Vec<u8>, HistoryError> {
        let mut bytes = Vec::new();
        self.to_writer(&mut bytes)?;
        Ok(bytes)
    }

    /// The document as text.
    ///
    /// # Errors
    ///
    /// The first line the format refuses.
    pub fn to_string(&self) -> Result<String, HistoryError> {
        String::from_utf8(self.to_vec()?).map_err(|_| HistoryError::Shape {
            line: 0,
            pointer: String::new(),
            expected: "UTF-8",
        })
    }

    /// Sets the header's digest to the body's, so a reader can tell the body was not changed.
    ///
    /// # Errors
    ///
    /// The first line the format refuses.
    pub fn seal(&mut self) -> Result<(), HistoryError> {
        self.header.digest = Some(self.body_digest()?);
        Ok(())
    }

    /// The digest of the body as it is written.
    fn body_digest(&self) -> Result<String, HistoryError> {
        let mut hasher = Sha256::new();
        for line in &self.lines {
            hasher.update(line.to_json()?.as_bytes());
            hasher.update(b"\n");
        }
        Ok(body_digest(&hasher))
    }

    /// What a new session is seeded from: the latest compaction summary and the lines after it.
    #[must_use]
    pub fn seed(&self) -> Seed<'_> {
        Seed::of(&self.lines)
    }

    /// The first body line as a seed point, whatever boundaries follow it; `None` for no body.
    #[must_use]
    pub fn first_point(&self) -> Option<crate::SeedPoint> {
        self.lines
            .first()
            .map(|line| crate::seed::point_of(2, line))
    }
}

impl FromStr for Document {
    type Err = HistoryError;

    /// Reads a history held in text, under [`ReaderSettings::for_document`].
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_slice(text.as_bytes())
    }
}
