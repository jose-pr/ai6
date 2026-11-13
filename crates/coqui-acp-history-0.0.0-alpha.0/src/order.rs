//! The order rules, and what a pass over a history counts.

use std::collections::HashSet;

use serde_json::Value;

use crate::{Body, HistoryError, OrderRule, Stop};

/// Where the latest compaction boundary of a history is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Boundary {
    /// Its line, the header being line 1.
    pub line: u64,
    /// Whether it carries the harness's summary.
    pub has_summary: bool,
}

/// What a pass over a history counted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Summary {
    /// Lines, the header included.
    pub lines: u64,
    /// Bytes, line feeds included.
    pub bytes: u64,
    /// Prompts, which is turns begun.
    pub turns: u64,
    /// `session/update` lines.
    pub updates: u64,
    /// Compaction boundaries.
    pub compactions: u64,
    /// The latest boundary.
    pub last_boundary: Option<Boundary>,
    /// The latest boundary that carries a summary: where a new session seeds from.
    pub last_summary: Option<Boundary>,
    /// How the latest finished turn ended.
    pub last_stop: Option<Stop>,
    /// Whether the history ends inside a turn, as a recording cut short does.
    pub open_turn: bool,
}

/// The state the order rules are checked against.
#[derive(Debug, Default)]
pub(crate) struct Sequencer {
    summary: Summary,
    /// The calls with a permission request waiting; a set, since a turn may hold millions.
    pending: HashSet<String>,
}

impl Sequencer {
    /// Counts the header, which is line 1.
    pub(crate) fn header(&mut self, bytes: u64) {
        self.summary.lines = 1;
        self.summary.bytes = bytes;
    }

    /// Checks `body`, the line numbered `line`, of `bytes` bytes with its line feed, and counts it.
    pub(crate) fn accept(
        &mut self,
        line: u64,
        bytes: u64,
        body: &Body,
    ) -> Result<(), HistoryError> {
        let order = |rule| Err(HistoryError::Order { line, rule });
        let in_turn = self.summary.open_turn;
        match body {
            Body::Prompt { .. } => {
                if in_turn {
                    return order(OrderRule::PromptInsideTurn);
                }
                self.summary.open_turn = true;
                self.summary.turns += 1;
            }
            Body::Update { .. } => self.summary.updates += 1,
            Body::Cancel => {
                if !in_turn {
                    return order(OrderRule::OutsideTurn);
                }
            }
            Body::PermissionRequest { tool_call, .. } => {
                if !in_turn {
                    return order(OrderRule::OutsideTurn);
                }
                let id = tool_call
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !self.pending.insert(id.to_owned()) {
                    return order(OrderRule::RequestRepeated);
                }
            }
            Body::PermissionAnswer { tool_call_id, .. } => {
                if !in_turn {
                    return order(OrderRule::OutsideTurn);
                }
                if !self.pending.remove(tool_call_id) {
                    return order(OrderRule::AnswerWithoutRequest);
                }
            }
            Body::TurnEnd { stop, .. } => {
                if !in_turn {
                    return order(OrderRule::OutsideTurn);
                }
                self.summary.open_turn = false;
                self.summary.last_stop = Some(*stop);
                self.pending.clear();
            }
            Body::Compaction { summary, .. } => {
                self.summary.compactions += 1;
                let boundary = Boundary {
                    line,
                    has_summary: summary.is_some(),
                };
                self.summary.last_boundary = Some(boundary);
                if summary.is_some() {
                    self.summary.last_summary = Some(boundary);
                }
            }
        }
        self.summary.lines = line;
        self.summary.bytes += bytes;
        Ok(())
    }

    pub(crate) fn summary(&self) -> &Summary {
        &self.summary
    }
}
