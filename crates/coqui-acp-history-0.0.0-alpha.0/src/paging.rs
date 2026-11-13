//! Cutting a history's text into pages of whole lines, for a provider answering the `acp_harness`
//! kind's `export_session`.
//!
//! A page never splits a line and never exceeds the bound it was asked for. The position that
//! carries on is `<byte offset>:<sha256 of the whole text>`, so a session that changed between two
//! pages refuses the second instead of mixing the two.

use core::fmt;

use data_encoding::HEXLOWER;
use sha2::{Digest, Sha256};

/// One page.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Page {
    /// Whole lines, each ended by a line feed.
    pub text: String,
    /// The position to ask the next page with; `None` after the last page.
    pub next: Option<String>,
}

/// Why a page could not be cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PageError {
    /// The position is not one this crate gave for this text.
    Position,
    /// The next line, line feed included, is this many bytes and does not fit.
    LineTooLarge(u64),
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Position => f.write_str(
                "the position is not one the export gave, or the session has changed since",
            ),
            Self::LineTooLarge(size) => write!(f, "line too large: {size} bytes"),
        }
    }
}

impl core::error::Error for PageError {}

/// The page of `text` (whole lines, each with its line feed) that follows position `after`, at most
/// `max_bytes` long.
///
/// # Errors
///
/// [`PageError::Position`] for an `after` this function did not give for this very text, and
/// [`PageError::LineTooLarge`] when not even the next line fits `max_bytes`.
pub fn page(text: &str, after: Option<&str>, max_bytes: u64) -> Result<Page, PageError> {
    let digest = HEXLOWER.encode(&Sha256::digest(text.as_bytes()));
    let start = match after {
        None => 0,
        Some(position) => {
            let (offset, given) = position.split_once(':').ok_or(PageError::Position)?;
            let offset: usize = offset.parse().map_err(|_| PageError::Position)?;
            let at_line_start =
                offset > 0 && offset < text.len() && text.as_bytes()[offset - 1] == b'\n';
            if given != digest || !at_line_start {
                return Err(PageError::Position);
            }
            offset
        }
    };
    let limit = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut end = start;
    for line in text[start..].split_inclusive('\n') {
        if end - start + line.len() > limit {
            if end == start {
                return Err(PageError::LineTooLarge(line.len() as u64));
            }
            break;
        }
        end += line.len();
    }
    Ok(Page {
        text: text[start..end].to_owned(),
        next: (end < text.len()).then(|| format!("{end}:{digest}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_joined_are_the_text_and_never_split_a_line() {
        let text = "aaa\nbb\ncccc\nd\n";
        let mut got = String::new();
        let mut after = None;
        loop {
            let page = page(text, after.as_deref(), 8).unwrap();
            assert!(page.text.len() <= 8 && page.text.ends_with('\n'));
            got.push_str(&page.text);
            match page.next {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        assert_eq!(got, text);
    }

    #[test]
    fn a_line_that_does_not_fit_is_refused_with_its_size() {
        assert_eq!(page("abcdef\n", None, 4), Err(PageError::LineTooLarge(7)));
    }

    #[test]
    fn a_position_of_another_text_or_in_the_middle_of_a_line_is_refused() {
        let first = page("aa\nbb\ncc\n", None, 3).unwrap();
        let next = first.next.unwrap();
        assert_eq!(
            page("aa\nbb\ncd\n", Some(&next), 3),
            Err(PageError::Position)
        );
        let (_, digest) = next.split_once(':').unwrap();
        assert_eq!(
            page("aa\nbb\ncc\n", Some(&format!("4:{digest}")), 3),
            Err(PageError::Position)
        );
        assert_eq!(page("aa\nbb\ncc\n", Some("x"), 3), Err(PageError::Position));
    }
}
