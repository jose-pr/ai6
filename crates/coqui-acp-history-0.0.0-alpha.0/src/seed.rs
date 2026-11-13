//! Seeding a new session from a history: the latest compaction summary, then what came after.

use serde::Serialize;
use sha2::{Digest, Sha256};

use guatiao::{FromValue, Schema, ToValue};

use crate::{Body, Line};

/// Where a seed starts, so a later reader can tell it is the same history.
#[derive(ToValue, FromValue, Schema, Serialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct SeedPoint {
    /// The line the seed starts at, the header being line 1: the latest compaction boundary that
    /// has a summary, else the first body line.
    pub line: u64,
    /// `sha256:` and the digest of that line's text as [`Line::to_json`] writes it.
    pub digest: String,
}

/// What a new session is seeded from: the summary of the latest compaction boundary that has one,
/// then the lines after it. With no such boundary, every body line.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Seed<'a> {
    /// The harness's summary text.
    pub summary: Option<&'a str>,
    /// Where the seed starts; `None` for a history with no body.
    pub point: Option<SeedPoint>,
    /// The lines after the boundary.
    pub lines: &'a [Line],
    /// The line number of `lines[0]`.
    pub first_line: u64,
    /// Whether the boundary fell inside a turn, so `lines` begin with the rest of one.
    pub mid_turn: bool,
}

/// The part of a [`Seed`] that fits a budget.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Fit<'a> {
    /// The summary, always kept: without it the lines are not this history's context.
    pub summary: Option<&'a str>,
    /// The latest whole turns that fit.
    pub lines: &'a [Line],
    /// The line number of `lines[0]`.
    pub first_line: u64,
    /// How many turns of the seed were left out.
    pub dropped_turns: u64,
    /// The size of `summary` and `lines` together, in bytes.
    pub bytes: u64,
    /// Whether all of the seed fits.
    pub complete: bool,
}

impl<'a> Seed<'a> {
    /// The seed of body `lines`, the first of which is line 2.
    pub(crate) fn of(lines: &'a [Line]) -> Self {
        let boundary = lines.iter().rposition(|line| {
            matches!(
                &line.body,
                Body::Compaction {
                    summary: Some(_),
                    ..
                }
            )
        });
        let (summary, after, first_line, point) = match boundary {
            Some(at) => {
                let summary = match &lines[at].body {
                    Body::Compaction { summary, .. } => summary.as_deref(),
                    _ => None,
                };
                (
                    summary,
                    &lines[at + 1..],
                    at as u64 + 3,
                    Some(point_of(at as u64 + 2, &lines[at])),
                )
            }
            None => (None, lines, 2, lines.first().map(|line| point_of(2, line))),
        };
        let mid_turn = boundary.is_some_and(|at| {
            let mut open = false;
            for line in &lines[..at] {
                match line.body {
                    Body::Prompt { .. } => open = true,
                    Body::TurnEnd { .. } => open = false,
                    _ => {}
                }
            }
            open
        });
        Self {
            summary,
            point,
            lines: after,
            first_line,
            mid_turn,
        }
    }

    /// How many turns the seed holds: its prompts.
    #[must_use]
    pub fn turns(&self) -> u64 {
        self.lines.iter().filter(|line| is_prompt(line)).count() as u64
    }

    /// The most recent whole turns that fit `max_bytes`, counting the summary and each line as
    /// [`Line::to_json`] writes it, a line feed included. A caller with a window in tokens divides
    /// by its own bytes-per-token estimate.
    ///
    /// A turn runs from its prompt to the next one; lines before the first prompt (the rest of a
    /// turn the boundary cut) count as a turn of their own. When not even the latest turn fits,
    /// `lines` is empty and `complete` is false.
    #[must_use]
    pub fn fit(&self, max_bytes: u64) -> Fit<'a> {
        let summary_bytes = self.summary.map_or(0, |summary| summary.len() as u64);
        let budget = max_bytes.saturating_sub(summary_bytes);
        let mut start = self.lines.len();
        let mut used = 0_u64;
        let mut turn = 0_u64;
        for at in (0..self.lines.len()).rev() {
            turn += size(&self.lines[at]);
            if is_prompt(&self.lines[at]) || at == 0 {
                if used + turn > budget {
                    break;
                }
                used += turn;
                turn = 0;
                start = at;
            }
        }
        let dropped_turns = self.lines[..start]
            .iter()
            .filter(|line| is_prompt(line))
            .count() as u64;
        Fit {
            summary: self.summary,
            lines: &self.lines[start..],
            first_line: self.first_line + start as u64,
            dropped_turns,
            bytes: summary_bytes + used,
            complete: start == 0 && summary_bytes <= max_bytes,
        }
    }
}

fn is_prompt(line: &Line) -> bool {
    matches!(line.body, Body::Prompt { .. })
}

fn size(line: &Line) -> u64 {
    line.to_json().map_or(0, |text| text.len() as u64 + 1)
}

pub(crate) fn point_of(number: u64, line: &Line) -> SeedPoint {
    let text = line.to_json().unwrap_or_default();
    SeedPoint {
        line: number,
        digest: format!(
            "sha256:{}",
            data_encoding::HEXLOWER.encode(&Sha256::digest(text.as_bytes()))
        ),
    }
}
