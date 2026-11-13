//! The writer: it checks every line as the reader would, so it cannot write a file the reader
//! refuses.

use std::io::Write;

use sha2::{Digest, Sha256};

use crate::header::Header;
use crate::order::{Sequencer, Summary};
use crate::reader::body_digest;
use crate::settings::{Bounds, WriterSettings};
use crate::{HistoryError, Line, strict};

/// Sets a [`Writer`] up. Nothing is written until [`start`](Self::start).
#[derive(Debug)]
#[must_use]
pub struct WriterBuilder<W> {
    sink: W,
    settings: WriterSettings,
}

impl<W: Write> WriterBuilder<W> {
    /// All the settings at once, as a file or a peer filled them.
    pub fn settings(mut self, settings: WriterSettings) -> Self {
        self.settings = settings;
        self
    }

    /// The sizes the history is held to.
    pub fn bounds(mut self, bounds: Bounds) -> Self {
        self.settings.bounds = bounds;
        self
    }

    /// Writes the header line and returns the writer for the body.
    ///
    /// A header that carries a digest is written as it is, and [`Writer::finish`] fails with
    /// [`HistoryError::DigestMismatch`] if the body written is not the one the digest names: the
    /// writer cannot know the body that is still to come, so [`Writer::body_digest`] or
    /// [`Document::seal`](crate::Document::seal) is how a digest is made.
    ///
    /// # Errors
    ///
    /// The header's refusals under the writer's bounds (nesting and values included),
    /// [`HistoryError::TooLong`] when it passes the line bound, or the sink's error.
    pub fn start(mut self, header: &Header) -> Result<Writer<W>, HistoryError> {
        let text = header.to_line_within(&self.settings.bounds)?;
        let limit = self.settings.bounds.max_line_bytes;
        if text.len() as u64 > limit {
            return Err(HistoryError::TooLong { line: 1, limit });
        }
        self.sink.write_all(text.as_bytes())?;
        self.sink.write_all(b"\n")?;
        let mut sequencer = Sequencer::default();
        sequencer.header(text.len() as u64 + 1);
        Ok(Writer {
            sink: self.sink,
            settings: self.settings,
            sequencer,
            hasher: Sha256::new(),
            declared: header.digest.clone(),
            failed: false,
        })
    }
}

/// Writes a history one line at a time.
#[derive(Debug)]
pub struct Writer<W> {
    sink: W,
    settings: WriterSettings,
    sequencer: Sequencer,
    hasher: Sha256,
    declared: Option<String>,
    failed: bool,
}

impl<W: Write> Writer<W> {
    /// A writer to `sink`, with the default settings until the builder says otherwise.
    pub fn builder(sink: W) -> WriterBuilder<W> {
        WriterBuilder {
            sink,
            settings: WriterSettings::default(),
        }
    }

    /// Checks `line` against the format, the order rules and the bounds, then writes it.
    ///
    /// Nothing is written for a line that is refused, and the writer goes on.
    ///
    /// # Errors
    ///
    /// The refusal a reader would give the line, or the sink's error, after which the writer
    /// refuses everything.
    pub fn write(&mut self, line: &Line) -> Result<(), HistoryError> {
        if self.failed {
            return Err(HistoryError::Io(std::io::Error::other(
                "an earlier write failed",
            )));
        }
        let bounds = &self.settings.bounds;
        let number = self.sequencer.summary().lines + 1;
        let text = line.to_json().map_err(|error| match error {
            HistoryError::TooDeep {
                limit, by_parser, ..
            } => HistoryError::TooDeep {
                line: number,
                limit,
                by_parser,
            },
            _ => HistoryError::Shape {
                line: number,
                pointer: String::new(),
                expected: "a line that serialises",
            },
        })?;
        if text.len() as u64 > bounds.max_line_bytes {
            return Err(HistoryError::TooLong {
                line: number,
                limit: bounds.max_line_bytes,
            });
        }
        if number > bounds.max_lines {
            return Err(HistoryError::TooManyLines {
                limit: bounds.max_lines,
            });
        }
        let bytes = text.len() as u64 + 1;
        if self.sequencer.summary().bytes + bytes > bounds.max_total_bytes {
            return Err(HistoryError::TooLarge {
                limit: bounds.max_total_bytes,
            });
        }
        let value = strict::parse(&text, number, bounds)?;
        let checked = Line::from_value(value, number)?;
        self.sequencer.accept(number, bytes, &checked.body)?;
        self.failed = true;
        self.sink.write_all(text.as_bytes())?;
        self.sink.write_all(b"\n")?;
        self.failed = false;
        self.hasher.update(text.as_bytes());
        self.hasher.update(b"\n");
        Ok(())
    }

    /// What has been written so far.
    #[must_use]
    pub fn summary(&self) -> &Summary {
        self.sequencer.summary()
    }

    /// The digest of the body written so far, in the form a header carries it.
    #[must_use]
    pub fn body_digest(&self) -> String {
        body_digest(&self.hasher)
    }

    /// Flushes the sink and returns it with what was written.
    ///
    /// # Errors
    ///
    /// The sink's error, or [`HistoryError::DigestMismatch`] when the header carried a digest and
    /// the body written is not the one it names (the sink then holds the whole file, which a
    /// reader refuses).
    pub fn finish(mut self) -> Result<(W, Summary), HistoryError> {
        self.sink.flush()?;
        if self
            .declared
            .as_deref()
            .is_some_and(|declared| declared != body_digest(&self.hasher))
        {
            return Err(HistoryError::DigestMismatch);
        }
        Ok((self.sink, self.sequencer.summary().clone()))
    }
}
