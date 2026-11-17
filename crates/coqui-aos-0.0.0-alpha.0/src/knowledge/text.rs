//! Written-out text and lines.

use alloc::string::String;
use alloc::vec::Vec;

/// Whether `code` hides, reorders or invisibly alters text.
fn hides(code: u32) -> bool {
    matches!(code,
        0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180E
        | 0x200B..=0x200F | 0x2028 | 0x2029 | 0x202A..=0x202E | 0x2060..=0x2064
        | 0x2066..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF9..=0xFFFB
        | 0x1D173..=0x1D17A | 0xE0000..=0xE007F | 0xE0100..=0xE01EF)
}

/// Whether `text` holds a character [`inert`] writes out.
#[must_use]
pub(crate) fn needs_writing_out(text: &str) -> bool {
    text.chars().any(written_out)
}

fn written_out(c: char) -> bool {
    let code = u32::from(c);
    !matches!(c, '\n' | '\t') && (code < 0x20 || (0x7F..=0x9F).contains(&code) || hides(code))
}

/// `text` as an agent may receive it: each control character but tab and line feed, and each
/// character that hides or reorders text, is written `<U+XXXX>`.
#[must_use]
pub fn inert(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if written_out(c) {
            out.push_str(&alloc::format!("<U+{:04X}>", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

/// A text's lines: split at line feed, a carriage return right before the line feed dropped, and no
/// line after a final line feed; an empty text has none.
#[must_use]
pub fn knowledge_lines(text: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = text.split('\n').collect();
    let last = parts.pop().unwrap_or("");
    let mut lines: Vec<&str> = parts
        .into_iter()
        .map(|part| part.strip_suffix('\r').unwrap_or(part))
        .collect();
    if !last.is_empty() {
        lines.push(last);
    }
    lines
}
