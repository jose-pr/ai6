//! A harness's output, one JSON value per line, read within bounds.

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::io::{self, BufRead, BufReader, Read};
use std::sync::{Mutex, PoisonError};

use serde_json::Value;

/// One line a harness wrote.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// A JSON value.
    Json(Value),
    /// Anything else: the harness's own diagnostics, without its line end.
    Text(String),
    /// A line longer than the limit, dropped: how many bytes were read of it before giving up.
    TooLong(u64),
}

/// Reads `reader` line by line until it ends, handing each line to `each` until `each` answers `false`. A line over
/// `max_line` bytes is handed over as [`Line::TooLong`] and reading stops, since what follows cannot be told apart
/// from the rest of it.
///
/// # Errors
///
/// Reading failed.
pub fn read_lines(
    reader: impl Read,
    max_line: u64,
    mut each: impl FnMut(Line) -> bool,
) -> io::Result<()> {
    let mut reader = BufReader::new(reader);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        // One more than the limit, so an overlong line is seen rather than split.
        let count = (&mut reader)
            .take(max_line.saturating_add(1))
            .read_until(b'\n', &mut bytes)?;
        if count == 0 {
            return Ok(());
        }
        let ended = bytes.last() == Some(&b'\n');
        if u64::try_from(count).unwrap_or(u64::MAX) > max_line && !ended {
            each(Line::TooLong(u64::try_from(count).unwrap_or(u64::MAX)));
            return Ok(());
        }
        let line = parse(&bytes);
        if let Some(line) = line
            && !each(line)
        {
            return Ok(());
        }
    }
}

/// One line's bytes as a [`Line`]; `None` for a blank line.
#[must_use]
pub fn parse(bytes: &[u8]) -> Option<Line> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_end_matches(['\n', '\r']);
    if text.trim().is_empty() {
        return None;
    }
    if (text.trim_start().starts_with('{') || text.trim_start().starts_with('['))
        && let Ok(value) = serde_json::from_str::<Value>(text)
    {
        return Some(Line::Json(value));
    }
    Some(Line::Text(text.to_string()))
}

/// The last lines a harness wrote that were not protocol, each cut to a size, for the message shown when it ends.
#[derive(Clone, Debug, Default)]
pub struct Tail {
    lines: VecDeque<String>,
    max_lines: usize,
    max_line_bytes: usize,
}

impl Tail {
    /// Keeps the last `max_lines` lines, each cut at `max_line_bytes` on a character boundary.
    #[must_use]
    pub const fn new(max_lines: usize, max_line_bytes: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            max_lines,
            max_line_bytes,
        }
    }

    /// Keeps `line`, forgetting the oldest past the limit.
    pub fn push(&mut self, line: &str) {
        if self.max_lines == 0 {
            return;
        }
        if self.lines.len() >= self.max_lines {
            self.lines.pop_front();
        }
        self.lines
            .push_back(cut(line, self.max_line_bytes).to_string());
    }

    /// The kept lines, oldest first, one per line; empty when none.
    #[must_use]
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether no line is kept.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Keeps what `reader` writes in `tail`, each line as text, until it ends: a harness's standard error. Past a line over
/// `max_line` bytes the rest is read and dropped, since a harness writing to a closed pipe would fail.
pub fn read_diagnostics(mut reader: impl Read, max_line: u64, tail: &Mutex<Tail>) {
    let _ = read_lines(&mut reader, max_line, |line| {
        let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
        match line {
            Line::Text(text) => tail.push(&text),
            Line::Json(value) => tail.push(&value.to_string()),
            Line::TooLong(bytes) => {
                tail.push(&alloc::format!(
                    "a diagnostic line of {bytes} bytes or more, past the limit"
                ));
            }
        }
        true
    });
    let _ = io::copy(&mut reader, &mut io::sink());
}

/// `text` for one line of a log or a label: each control character is a space, so it cannot forge another line or move
/// the cursor.
#[must_use]
pub fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// `text` cut to at most `max` bytes on a character boundary.
#[must_use]
pub fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_text_blank_and_overlong_lines_are_told_apart() {
        let input = b"{\"a\":1}\r\nnot json\n\n[1,2]\n{broken\n".to_vec();
        let mut seen = Vec::new();
        read_lines(&input[..], 100, |line| {
            seen.push(line);
            true
        })
        .unwrap();
        assert_eq!(
            seen,
            [
                Line::Json(serde_json::json!({"a": 1})),
                Line::Text("not json".to_string()),
                Line::Json(serde_json::json!([1, 2])),
                Line::Text("{broken".to_string()),
            ]
        );
        let mut seen = Vec::new();
        read_lines(&b"0123456789ABCDEF\nnext\n"[..], 8, |line| {
            seen.push(line);
            true
        })
        .unwrap();
        assert_eq!(seen, [Line::TooLong(9)]);
    }

    #[test]
    fn diagnostics_are_kept_and_the_rest_past_an_overlong_line_is_still_read() {
        let mut input = b"first\n".to_vec();
        input.extend(core::iter::repeat_n(b'x', 100));
        input.extend(b"\nafter\n");
        let mut reader = &input[..];
        let tail = Mutex::new(Tail::new(8, 64));
        read_diagnostics(&mut reader, 16, &tail);
        assert!(reader.is_empty(), "everything was read");
        let kept = tail.lock().unwrap().text();
        assert!(
            kept.starts_with("first\n") && kept.contains("past the limit"),
            "{kept}"
        );
    }

    #[test]
    fn a_line_for_a_log_cannot_forge_another_or_move_the_cursor() {
        assert_eq!(one_line("a\nb\r\x1b[2Jc"), "a b  [2Jc");
    }

    #[test]
    fn the_configured_tail_bounds_what_a_failed_session_quotes() {
        let mut tail = Tail::new(2, 4);
        for line in ["one one", "two two", "three three"] {
            tail.push(line);
        }
        assert_eq!(tail.text(), "two \nthre");
        let mut none = Tail::new(0, 4);
        none.push("x");
        assert!(none.is_empty());
        assert_eq!(cut("añb", 2), "a");
    }
}
