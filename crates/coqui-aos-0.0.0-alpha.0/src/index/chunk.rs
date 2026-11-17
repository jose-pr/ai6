//! Cutting a text file into passages: by Markdown heading first, then by size, each passage keeping its line range.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// One passage of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Piece {
    /// The headings above it, outermost first, joined with ` > `; empty before the first heading and in a file that is not
    /// Markdown.
    pub heading: String,
    /// Its first line, counting from 1.
    pub from: usize,
    /// Its last line, inclusive.
    pub to: usize,
    /// Its text: the lines joined with `\n`, no line ending after the last.
    pub text: String,
}

/// A run of text of one line, no longer than the budget.
struct Unit<'a> {
    line: usize,
    text: &'a str,
    /// Whether it belongs to a heading's line.
    heading: bool,
}

/// Whether `path` names a Markdown file, whose headings divide it.
pub(super) fn is_markdown(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// The passages of `text`, each at most `budget` bytes, adjacent ones sharing at most `overlap` bytes of whole lines (a line
/// longer than the budget is cut at a character boundary, and its cuts share the same way).
pub(super) fn chunk(text: &str, markdown: bool, budget: usize, overlap: usize) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut section: Vec<Unit<'_>> = Vec::new();
    let mut section_heading = String::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (index, line) in crate::knowledge_lines(text).into_iter().enumerate() {
        let number = index + 1;
        let mut heading = false;
        if markdown {
            if let Some((mark, size)) = fence {
                if closes_fence(line, mark, size) {
                    fence = None;
                }
            } else if let Some(opened) = opens_fence(line) {
                fence = Some(opened);
            } else if let Some((level, title)) = heading_of(line) {
                emit(&mut pieces, &section, &section_heading, budget, overlap);
                section.clear();
                while stack.last().is_some_and(|(top, _)| *top >= level) {
                    stack.pop();
                }
                stack.push((level, title));
                section_heading = stack
                    .iter()
                    .map(|(_, title)| title.as_str())
                    .collect::<Vec<_>>()
                    .join(" > ");
                heading = true;
            }
        }
        for segment in split_line(line, budget) {
            section.push(Unit {
                line: number,
                text: segment,
                heading,
            });
        }
    }
    emit(&mut pieces, &section, &section_heading, budget, overlap);
    pieces
}

fn opens_fence(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let mark = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let size = trimmed.chars().take_while(|c| *c == mark).count();
    (size >= 3).then_some((mark, size))
}

fn closes_fence(line: &str, mark: char, size: usize) -> bool {
    let trimmed = line.trim_start_matches(' ');
    let run = trimmed.chars().take_while(|c| *c == mark).count();
    line.len() - trimmed.len() <= 3 && run >= size && trimmed[run..].trim().is_empty()
}

/// The level and title of an ATX heading line.
fn heading_of(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut title = rest.trim();
    // A closing run of `#` counts only after a space, so `# C#` keeps its title.
    let stripped = title.trim_end_matches('#');
    if stripped.is_empty() || stripped.ends_with([' ', '\t']) {
        title = stripped.trim();
    }
    Some((level, title.to_string()))
}

/// `line` in runs of at most `budget` bytes, cut at a space in the later half of a run where there is one, else at a character
/// boundary. An empty line is one empty run.
fn split_line(line: &str, budget: usize) -> Vec<&str> {
    let mut runs = Vec::new();
    let mut rest = line;
    while rest.len() > budget {
        let mut cut = budget;
        while cut > 0 && !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut == 0 {
            // A budget narrower than one character: take the character whole.
            cut = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        if let Some(space) = rest[..cut].rfind([' ', '\t'])
            && space + 1 > cut / 2
        {
            cut = space + 1;
        }
        runs.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    runs.push(rest);
    runs
}

/// Packs one section's units into passages and appends them.
fn emit(out: &mut Vec<Piece>, units: &[Unit<'_>], heading: &str, budget: usize, overlap: usize) {
    // The passage being built is `units[first..index]`; its first `shared` units repeat the previous passage's tail.
    let mut first = 0;
    let mut size = 0usize;
    let mut shared = 0usize;
    let mut index = 0;
    while index < units.len() {
        let len = units[index].text.len();
        let separator = usize::from(index > first);
        if index > first && size + separator + len > budget {
            if index - first > shared {
                push(out, &units[first..index], shared, heading);
                let (begin, kept) = tail(units, first, index, overlap, len, budget);
                shared = index - begin;
                first = begin;
                size = kept;
            } else {
                // Only the repeated tail is held and the next unit does not fit beside it: drop the tail.
                first = index;
                shared = 0;
                size = 0;
            }
            continue;
        }
        size += separator + len;
        index += 1;
    }
    if units.len() - first > shared {
        push(out, &units[first..], shared, heading);
    }
}

/// Where the next passage starts after `units[first..end]` was emitted: the longest run of units ending at `end` that is at
/// most `overlap` bytes and leaves room for the next unit of `next_len` bytes. Returns its first unit and its size.
fn tail(
    units: &[Unit<'_>],
    first: usize,
    end: usize,
    overlap: usize,
    next_len: usize,
    budget: usize,
) -> (usize, usize) {
    let mut begin = end;
    let mut size = 0usize;
    while begin > first + 1 {
        let grown = size + units[begin - 1].text.len() + usize::from(begin < end);
        if grown > overlap || grown + 1 + next_len > budget {
            break;
        }
        size = grown;
        begin -= 1;
    }
    (begin, size)
}

fn push(out: &mut Vec<Piece>, units: &[Unit<'_>], shared: usize, heading: &str) {
    let blank = |unit: &Unit<'_>| unit.text.trim().is_empty();
    // A passage of nothing but the previous one's tail and blank lines adds nothing.
    if units[shared..].iter().all(blank) {
        return;
    }
    let Some(lead) = units.iter().position(|unit| !blank(unit)) else {
        return;
    };
    let last = units.iter().rposition(|unit| !blank(unit)).unwrap_or(lead);
    let kept = &units[lead..=last];
    if kept.iter().all(|unit| unit.heading || blank(unit)) {
        return;
    }
    out.push(Piece {
        heading: heading.to_string(),
        from: kept[0].line,
        to: kept[kept.len() - 1].line,
        text: join(kept),
    });
}

/// The units' text: runs of one line joined with nothing, lines with `\n`.
fn join(units: &[Unit<'_>]) -> String {
    let mut text = String::new();
    for (position, unit) in units.iter().enumerate() {
        if position > 0 && units[position - 1].line != unit.line {
            text.push('\n');
        }
        text.push_str(unit.text);
    }
    text
}
