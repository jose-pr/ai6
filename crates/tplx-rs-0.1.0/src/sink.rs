use crate::error::{Error, Limit};
use alloc::string::String;

/// The output of a render, bounded in bytes.
#[derive(Debug)]
pub(crate) struct Sink {
    buf: String,
    max: usize,
}

impl Sink {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            buf: String::new(),
            max,
        }
    }

    pub(crate) fn push(&mut self, piece: &str) -> Result<(), Error> {
        if piece.len() > self.max.saturating_sub(self.buf.len()) {
            return Err(Error::Limit(Limit::Output));
        }
        self.buf.push_str(piece);
        Ok(())
    }

    pub(crate) fn finish(self) -> String {
        self.buf
    }
}

/// 1-based line and column (in characters) of a byte offset.
pub(crate) fn position(text: &str, offset: usize) -> (usize, usize) {
    let head = &text[..offset.min(text.len())];
    let line = head.matches('\n').count() + 1;
    let start = head.rfind('\n').map_or(0, |i| i + 1);
    (line, head[start..].chars().count() + 1)
}
