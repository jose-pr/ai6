// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`KeyPath`]: naming one place inside a settings type, as `agent[1].name`.
//!
//! A path is text wherever a source or a person writes one (a variable, a flag, a form's
//! hint), and a list of [`Segment`]s wherever code walks a description. The grammar:
//!
//! ```text
//! path    := segment ( '.' name | '[' index ']' )*
//! segment := name | '[' index ']'
//! name    := one or more characters, none of them '.', '[' or ']'
//! index   := digits | quoted | one or more characters, none of them '[' or ']'
//! quoted  := '"' ( any character, with \" and \\ escaped ) '"'
//! ```
//!
//! A path is rootless (no leading `.`). After a segment comes a `.` and a name, a `[`, or the
//! end: `a[0]b` and `a.[0]` are refused. A bare bracket holds no `[`; `["a[b]"]` spells that
//! key. Digits are a position only without a leading zero: `[01]` is the key `01`, `[0]` the
//! position 0. [`Display`](fmt::Display) writes the spelling that parses back to the same
//! segments.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::str::FromStr;

/// One step of a path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Segment {
    /// A field of a group, an arm's field, or an entry of an open map.
    Key(String),
    /// A position in a list. An open map reads it as the key of its decimal text.
    Index(usize),
}

impl Segment {
    /// A key segment.
    pub fn key(name: impl Into<String>) -> Segment {
        Segment::Key(name.into())
    }

    /// The name a field or map entry is looked up under: a key as it is, a position as its
    /// decimal text.
    pub fn name(&self) -> alloc::borrow::Cow<'_, str> {
        match self {
            Segment::Key(key) => alloc::borrow::Cow::Borrowed(key),
            Segment::Index(at) => alloc::borrow::Cow::Owned(alloc::format!("{at}")),
        }
    }

    /// The key, when this is one.
    pub fn as_key(&self) -> Option<&str> {
        match self {
            Segment::Key(key) => Some(key),
            Segment::Index(_) => None,
        }
    }

    /// The position, when this is one.
    pub fn as_index(&self) -> Option<usize> {
        match self {
            Segment::Index(at) => Some(*at),
            Segment::Key(_) => None,
        }
    }
}

impl From<&str> for Segment {
    fn from(key: &str) -> Segment {
        Segment::Key(key.into())
    }
}

impl From<usize> for Segment {
    fn from(at: usize) -> Segment {
        Segment::Index(at)
    }
}

/// A position as bare digits: no leading zero, small enough for a `usize`.
fn index_of(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    text.parse().ok()
}

/// A key that cannot be written bare after a `.`.
fn needs_quotes(key: &str) -> bool {
    key.is_empty() || key.contains(['.', '[', ']'])
}

fn write_quoted(key: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("[\"")?;
    for c in key.chars() {
        if c == '"' || c == '\\' {
            f.write_str("\\")?;
        }
        fmt::Write::write_char(f, c)?;
    }
    f.write_str("\"]")
}

fn write_segment(segment: &Segment, first: bool, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match segment {
        Segment::Index(at) => write!(f, "[{at}]"),
        Segment::Key(key) if index_of(key).is_some() => write_quoted(key, f),
        Segment::Key(key) if !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) => {
            write!(f, "[{key}]")
        }
        Segment::Key(key) if needs_quotes(key) => write_quoted(key, f),
        Segment::Key(key) => {
            if !first {
                f.write_str(".")?;
            }
            f.write_str(key)
        }
    }
}

impl fmt::Display for Segment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_segment(self, true, f)
    }
}

/// A place inside a settings type: a list of [`Segment`]s from the root.
///
/// The empty path is the root. [`FromStr`] parses the grammar in the module documentation and
/// [`Display`](fmt::Display) writes it, so `waiting.per_source` and `env["a.b"]` round-trip.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyPath {
    segments: Vec<Segment>,
}

impl KeyPath {
    /// The root: no segments.
    pub fn root() -> KeyPath {
        KeyPath::default()
    }

    /// A path of `segments`.
    pub fn new(segments: Vec<Segment>) -> KeyPath {
        KeyPath { segments }
    }

    /// The segments, from the root.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// How many segments there are.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Whether this is the root.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Adds `segment` at the end.
    pub fn push(&mut self, segment: impl Into<Segment>) {
        self.segments.push(segment.into());
    }

    /// Takes the last segment off.
    pub fn pop(&mut self) -> Option<Segment> {
        self.segments.pop()
    }

    /// This path and one more `segment`.
    #[must_use]
    pub fn child(&self, segment: impl Into<Segment>) -> KeyPath {
        let mut out = self.clone();
        out.push(segment);
        out
    }

    /// Whether `prefix` is this path or one of its ancestors.
    pub fn starts_with(&self, prefix: &[Segment]) -> bool {
        self.segments.starts_with(prefix)
    }
}

impl From<Vec<Segment>> for KeyPath {
    fn from(segments: Vec<Segment>) -> KeyPath {
        KeyPath { segments }
    }
}

impl FromIterator<Segment> for KeyPath {
    fn from_iter<I: IntoIterator<Item = Segment>>(iter: I) -> KeyPath {
        KeyPath {
            segments: iter.into_iter().collect(),
        }
    }
}

impl AsRef<[Segment]> for KeyPath {
    fn as_ref(&self) -> &[Segment] {
        &self.segments
    }
}

impl<'a> IntoIterator for &'a KeyPath {
    type Item = &'a Segment;
    type IntoIter = core::slice::Iter<'a, Segment>;

    fn into_iter(self) -> Self::IntoIter {
        self.segments.iter()
    }
}

impl fmt::Display for KeyPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (at, segment) in self.segments.iter().enumerate() {
            write_segment(segment, at == 0, f)?;
        }
        Ok(())
    }
}

impl FromStr for KeyPath {
    type Err = PathError;

    fn from_str(text: &str) -> Result<KeyPath, PathError> {
        if text.is_empty() {
            return Err(PathError::Empty);
        }
        // A path is rootless: `.host` would be a key with an empty first segment.
        if text.starts_with('.') {
            return Err(PathError::EmptyField { at: 0 });
        }
        let mut segments = Vec::new();
        let mut rest = text;
        let mut at = 0;
        while !rest.is_empty() {
            let (segment, read) = match step(rest, at == 0) {
                Ok(Some(found)) => found,
                Ok(None) => return Err(PathError::EmptyField { at }),
                Err(e) => return Err(e.shifted(at)),
            };
            segments.push(segment);
            rest = &rest[read..];
            at += read;
        }
        Ok(KeyPath { segments })
    }
}

/// Why a text is not a path. Every variant says where, as a byte offset into the text.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathError {
    /// The text is empty. A path names at least one step.
    Empty,
    /// A field name with no characters: a leading, doubled or trailing `.`.
    EmptyField {
        /// Where the name would have started.
        at: usize,
    },
    /// `[]`, which names neither a position nor a key.
    EmptyIndex {
        /// The `[`.
        at: usize,
    },
    /// A `[` with no `]` after it.
    UnclosedIndex {
        /// The `[`.
        at: usize,
    },
    /// A `"` with no closing `"`.
    UnclosedQuote {
        /// The opening `"`.
        at: usize,
    },
    /// A `\` at the end of a quoted key, or before a character that is neither `"` nor `\`.
    BadEscape {
        /// The `\`.
        at: usize,
    },
    /// A `]` where no `[` is open.
    UnexpectedClose {
        /// The `]`.
        at: usize,
    },
    /// Something after a closing `"` other than `]`.
    TrailingQuote {
        /// The first character after the closing `"`.
        at: usize,
    },
    /// A segment after a bracket that starts with neither `.` nor `[`: `a[0]b`.
    MissingSeparator {
        /// The first character of the segment.
        at: usize,
    },
}

impl PathError {
    /// Where in the text, as a byte offset. 0 for an empty path.
    pub fn offset(&self) -> usize {
        match self {
            PathError::Empty => 0,
            PathError::EmptyField { at }
            | PathError::EmptyIndex { at }
            | PathError::UnclosedIndex { at }
            | PathError::UnclosedQuote { at }
            | PathError::BadEscape { at }
            | PathError::UnexpectedClose { at }
            | PathError::TrailingQuote { at }
            | PathError::MissingSeparator { at } => *at,
        }
    }

    fn shifted(self, by: usize) -> PathError {
        match self {
            PathError::Empty => PathError::Empty,
            PathError::EmptyField { at } => PathError::EmptyField { at: at + by },
            PathError::EmptyIndex { at } => PathError::EmptyIndex { at: at + by },
            PathError::UnclosedIndex { at } => PathError::UnclosedIndex { at: at + by },
            PathError::UnclosedQuote { at } => PathError::UnclosedQuote { at: at + by },
            PathError::BadEscape { at } => PathError::BadEscape { at: at + by },
            PathError::UnexpectedClose { at } => PathError::UnexpectedClose { at: at + by },
            PathError::TrailingQuote { at } => PathError::TrailingQuote { at: at + by },
            PathError::MissingSeparator { at } => PathError::MissingSeparator { at: at + by },
        }
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::Empty => f.write_str("an empty path names nothing"),
            PathError::EmptyField { at } => write!(f, "an empty field name at {at}"),
            PathError::EmptyIndex { at } => write!(f, "an empty `[]` at {at}"),
            PathError::UnclosedIndex { at } => write!(f, "a `[` at {at} with no `]`"),
            PathError::UnclosedQuote { at } => write!(f, "a `\"` at {at} with no closing `\"`"),
            PathError::BadEscape { at } => {
                write!(f, "a `\\` at {at} before something other than `\"` or `\\`")
            }
            PathError::UnexpectedClose { at } => write!(f, "a `]` at {at} with no `[`"),
            PathError::TrailingQuote { at } => write!(f, "text after a quoted key at {at}"),
            PathError::MissingSeparator { at } => {
                write!(f, "a segment at {at} that follows a bracket without a `.`")
            }
        }
    }
}

impl core::error::Error for PathError {}

const NAME_ENDS: [char; 3] = ['.', '[', ']'];

/// One segment off the front of `rest`, and how many bytes it took. `rest` starts at a segment
/// boundary: the very beginning (`first`), or just after a segment, where a `.` or a `[` must
/// follow.
fn step(rest: &str, first: bool) -> Result<Option<(Segment, usize)>, PathError> {
    if rest.is_empty() {
        return Ok(None);
    }
    let (body, skipped) = match rest.strip_prefix('.') {
        Some(body) => (body, 1),
        None => (rest, 0),
    };
    if skipped == 0 && !first && !rest.starts_with('[') {
        return Err(PathError::MissingSeparator { at: 0 });
    }
    if body.starts_with('[') {
        if skipped == 1 {
            // A `.` is followed by a name; a bracket is its own segment.
            return Err(PathError::EmptyField { at: 1 });
        }
        let (segment, read) = bracket(body)?;
        return Ok(Some((segment, skipped + read)));
    }
    let end = body.find(NAME_ENDS).unwrap_or(body.len());
    if let Some(']') = body[end..].chars().next() {
        return Err(PathError::UnexpectedClose { at: skipped + end });
    }
    if end == 0 {
        return Err(PathError::EmptyField { at: skipped });
    }
    Ok(Some((Segment::Key(body[..end].into()), skipped + end)))
}

/// A bracket segment, `body` starting at the `[`. Answers how many bytes it took, the closing
/// `]` included.
fn bracket(body: &str) -> Result<(Segment, usize), PathError> {
    let inside = &body[1..];
    if let Some(quoted) = inside.strip_prefix('"') {
        let (key, read) = quoted_key(quoted).map_err(|e| e.shifted(1))?;
        return match inside[1 + read..].strip_prefix(']') {
            Some(_) => Ok((Segment::Key(key), 1 + 1 + read + 1)),
            None => Err(PathError::TrailingQuote { at: 1 + 1 + read }),
        };
    }
    let Some(end) = inside.find(']') else {
        return Err(PathError::UnclosedIndex { at: 0 });
    };
    if end == 0 {
        return Err(PathError::EmptyIndex { at: 0 });
    }
    let text = &inside[..end];
    if text.contains('[') {
        return Err(PathError::UnclosedIndex { at: 0 });
    }
    let segment = match index_of(text) {
        Some(at) => Segment::Index(at),
        None => Segment::Key(text.into()),
    };
    Ok((segment, 1 + end + 1))
}

/// A quoted key, `quoted` starting just after the opening `"`. Answers how many bytes it took,
/// the closing `"` included.
fn quoted_key(quoted: &str) -> Result<(String, usize), PathError> {
    let mut out = String::new();
    let mut chars = quoted.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Ok((out, i + 1)),
            '\\' => match chars.next() {
                Some((_, next @ ('"' | '\\'))) => out.push(next),
                _ => return Err(PathError::BadEscape { at: i + 1 }),
            },
            other => out.push(other),
        }
    }
    Err(PathError::UnclosedQuote { at: 0 })
}
