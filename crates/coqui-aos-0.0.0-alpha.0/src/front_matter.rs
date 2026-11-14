//! `OVERLAY.md` and every document with front matter (`spec/v0/manifest.md#overlaymd`): the
//! split of a document into metadata and body, one step on bytes before any parser, and the
//! metadata read as restricted YAML unless a format line names another format.

use crate::error::{Class, Error};
use crate::kinds::Format;
use crate::limits::Settings;
use crate::read::{Document, choose_format, parse_payload};

const BYTE_ORDER_MARK: &[u8] = b"\xef\xbb\xbf";
const WHITE_SPACE: &[u8] = b" \t\r\n";

/// How a document divides into metadata and body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SplitCase {
    /// The first line is no opening fence: the whole document is the body.
    None,
    /// An opening fence and no closing fence.
    Unterminated,
    /// The head bound ended before the split did.
    TooLarge,
    /// A block whose metadata is only white space.
    Empty,
    /// A block with metadata.
    Present,
}

/// The result of [`split_front_matter`]. Offsets count bytes from the first byte of the file, a
/// byte-order mark included; lines count from 1 at the file's first line.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Split<'a> {
    /// How the document divides.
    pub case: SplitCase,
    /// The bytes between the fences, unchanged; `Some` for `Empty` and `Present`.
    pub metadata: Option<&'a [u8]>,
    /// Where the metadata starts, when there is a block.
    pub metadata_offset: Option<usize>,
    /// The line the metadata starts on, when there is a block.
    pub metadata_line: Option<usize>,
    /// Where the body starts; `None` for `TooLarge`.
    pub body_offset: Option<usize>,
    /// The line the body starts on; `None` for `TooLarge`.
    pub body_line: Option<usize>,
}

impl<'a> Split<'a> {
    /// The body, unchanged and not checked as UTF-8; empty for `TooLarge`.
    #[must_use]
    pub fn body(&self, data: &'a [u8]) -> &'a [u8] {
        self.body_offset
            .map_or(&[][..], |offset| &data[offset.min(data.len())..])
    }
}

fn fence(line: &[u8], ended: bool) -> Option<&'static [u8]> {
    let line = if ended {
        line.strip_suffix(b"\r").unwrap_or(line)
    } else {
        line
    };
    let end = line
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(0, |at| at + 1);
    match &line[..end] {
        b"---" => Some(b"---"),
        b"..." => Some(b"..."),
        _ => None,
    }
}

fn find(data: &[u8], from: usize, to: usize) -> Option<usize> {
    let to = to.min(data.len());
    (from..to).find(|at| data[*at] == b'\n')
}

fn may_open(head: &[u8]) -> bool {
    let tail = head.get(3..).unwrap_or_default();
    let tail = tail.strip_suffix(b"\r").unwrap_or(tail);
    b"---".starts_with(&head[..head.len().min(3)])
        && tail.iter().all(|byte| matches!(byte, b' ' | b'\t'))
}

fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !WHITE_SPACE.contains(byte))
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !WHITE_SPACE.contains(byte))
        .map_or(start, |at| at + 1);
    &bytes[start..end]
}

fn blank(case: SplitCase, start: usize) -> Split<'static> {
    Split {
        case,
        metadata: None,
        metadata_offset: None,
        metadata_line: None,
        body_offset: Some(start),
        body_line: Some(1),
    }
}

/// Splits `data` into metadata and body without parsing or changing it. The head, from after a
/// byte-order mark through the closing fence's line end, is bounded by the manifest size of
/// `settings`, and nothing past it is read.
#[must_use]
pub fn split_front_matter<'a>(data: &'a [u8], settings: &Settings) -> Split<'a> {
    let start = if data.starts_with(BYTE_ORDER_MARK) {
        3
    } else {
        0
    };
    let limit = settings
        .manifest_bytes
        .get()
        .map_or(usize::MAX, |bound| {
            usize::try_from(bound).unwrap_or(usize::MAX)
        })
        .saturating_add(start);
    let too_large = Split {
        case: SplitCase::TooLarge,
        metadata: None,
        metadata_offset: None,
        metadata_line: None,
        body_offset: None,
        body_line: None,
    };
    let end = find(data, start, limit);
    if end.is_none() && data.len() > limit {
        return if may_open(&data[start..limit]) {
            too_large
        } else {
            blank(SplitCase::None, start)
        };
    }
    let first = end.map_or(&data[start..], |end| &data[start..end]);
    if fence(first, end.is_some()) != Some(b"---") {
        return blank(SplitCase::None, start);
    }
    let Some(end) = end else {
        return blank(SplitCase::Unterminated, start);
    };
    let metadata_offset = end + 1;
    let mut at = metadata_offset;
    loop {
        let body_offset = match find(data, at, limit) {
            None => {
                if data.len() > limit {
                    return too_large;
                }
                if fence(&data[at..], false).is_none() {
                    return blank(SplitCase::Unterminated, start);
                }
                data.len()
            }
            Some(lf) if fence(&data[at..lf], true).is_none() => {
                at = lf + 1;
                continue;
            }
            Some(lf) => lf + 1,
        };
        let metadata = &data[metadata_offset..at];
        let lines = data[start..body_offset]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        return Split {
            case: if trim(metadata).is_empty() {
                SplitCase::Empty
            } else {
                SplitCase::Present
            },
            metadata: Some(metadata),
            metadata_offset: Some(metadata_offset),
            metadata_line: Some(2),
            body_offset: Some(body_offset),
            body_line: Some(1 + lines),
        };
    }
}

/// What a viewer shows a front-matter document as: the reading's outcome, so the text can be
/// shown whatever went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Case {
    /// No front matter: an empty manifest, and all of the document is the body.
    None,
    /// Not a block, but a person or a laxer reader would take it for one.
    Malformed,
    /// An opening fence and no closing fence.
    Unterminated,
    /// The head bound ended before the split did.
    TooLarge,
    /// A block of only white space, or a format line and white space.
    Empty,
    /// A format line names a format this reader does not read, or none.
    UnknownFormat,
    /// The parser refuses the metadata, or reading does.
    DoesNotParse,
    /// The metadata parses to a sequence or a scalar.
    NotAMapping,
    /// The metadata reads as a mapping.
    Mapping,
}

/// A front-matter document split and read.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct FrontMatter {
    /// How the metadata read.
    pub case: Case,
    /// The manifest: empty when there is no metadata.
    pub data: Option<Document>,
    /// What a gate refuses with, when it refuses; its `line` counts from the file's first line.
    pub error: Option<Error>,
    /// The format the metadata was read as, `toml`, `json` or `yaml`, when it got that far.
    pub format: Option<Format>,
    /// Where the body starts, when the split found one.
    pub body_offset: Option<usize>,
    /// The line the body starts on.
    pub body_line: Option<usize>,
}

/// Whether a document that is no block is one that a person, or a laxer reader, would take for a
/// block: a fence after white space or a second byte-order mark, or a format line before a fence.
fn almost_a_fence(data: &[u8]) -> bool {
    let mut lead = data;
    loop {
        if lead.starts_with(BYTE_ORDER_MARK) {
            lead = &lead[3..];
        } else if matches!(lead.first(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            lead = &lead[1..];
        } else {
            break;
        }
    }
    if lead.starts_with(b"---") || lead.starts_with(b"...") {
        return true;
    }
    let start = if data.starts_with(BYTE_ORDER_MARK) {
        3
    } else {
        0
    };
    if data[start..].starts_with(b"#!")
        && let Some(end) = data[start..].iter().position(|byte| *byte == b'\n')
    {
        let after = start + end + 1;
        let second = data[after..].iter().position(|byte| *byte == b'\n');
        let line = second.map_or(&data[after..], |second| &data[after..after + second]);
        return fence(line, second.is_some()).is_some();
    }
    false
}

fn refused(case: Case, error: Error) -> FrontMatter {
    FrontMatter {
        case,
        data: None,
        error: Some(error),
        format: None,
        body_offset: None,
        body_line: None,
    }
}

/// Splits `data`, then reads its metadata: YAML unless its first line names another format.
///
/// A [`FrontMatter`] always says what to show: `error` is what a gate refuses with, and `data`
/// the manifest where there is one.
#[must_use]
pub fn read_front_matter(data: &[u8], settings: &Settings) -> FrontMatter {
    let split = split_front_matter(data, settings);
    let found = |case: Case, data: Option<Document>, error: Option<Error>| FrontMatter {
        case,
        data,
        error,
        format: None,
        body_offset: split.body_offset,
        body_line: split.body_line,
    };
    match split.case {
        SplitCase::None if almost_a_fence(data) => found(
            Case::Malformed,
            None,
            Some(Error::new(Class::MalformedFence, "").on_line(1)),
        ),
        SplitCase::None => found(Case::None, Some(Document::new()), None),
        SplitCase::Empty => found(Case::Empty, Some(Document::new()), None),
        SplitCase::Unterminated => found(
            Case::Unterminated,
            None,
            Some(Error::new(Class::UnterminatedFrontMatter, "").on_line(1)),
        ),
        SplitCase::TooLarge => refused(
            Case::TooLarge,
            Error::new(Class::TooLarge, "front matter")
                .over("manifest_bytes")
                .on_line(1),
        ),
        _ => {
            let metadata = split.metadata.unwrap_or_default();
            let first_line = split.metadata_line.unwrap_or(2);
            let (format, payload, lined) = match choose_format(metadata, "yaml") {
                Ok(chosen) => chosen,
                Err(error) => {
                    let case = match error.class {
                        Class::UnsupportedFormat | Class::NoFormatNamed => Case::UnknownFormat,
                        _ => Case::DoesNotParse,
                    };
                    return found(case, None, Some(error.on_line(first_line)));
                }
            };
            match parse_payload(payload, format, settings, lined) {
                Ok(document) => {
                    let case = if trim(payload).is_empty() {
                        Case::Empty
                    } else {
                        Case::Mapping
                    };
                    let mut result = found(case, Some(document), None);
                    result.format = Format::named(format);
                    result
                }
                Err(error) => {
                    let case = match error.class {
                        Class::NotAMapping => Case::NotAMapping,
                        _ => Case::DoesNotParse,
                    };
                    let line = error.line.map_or(first_line, |line| first_line + line - 1);
                    let mut result = found(case, None, Some(error.on_line(line)));
                    result.format = Format::named(format);
                    result
                }
            }
        }
    }
}

/// A context file's bytes as context: a closed block is dropped unread, the rest is kept.
///
/// # Errors
///
/// `too-large` when the head bound ended before the split did.
pub fn context_file_text<'a>(data: &'a [u8], settings: &Settings) -> Result<&'a [u8], Error> {
    let split = split_front_matter(data, settings);
    if split.case == SplitCase::TooLarge {
        return Err(Error::new(Class::TooLarge, "front matter")
            .over("manifest_bytes")
            .on_line(1));
    }
    Ok(split.body(data))
}
