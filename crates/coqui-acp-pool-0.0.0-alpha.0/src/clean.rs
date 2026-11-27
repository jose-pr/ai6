//! The cleaning of a carried session. Before a session that ran out of usage is imported on the next member, its last turn is
//! stripped of what would tell the new agent to wait and of the limit's own text, so no tokens go to either. A pure function over an
//! exported history; it reads and writes no file and names no harness.

use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use coqui_acp_api::{LIMIT_META_KEY, LimitHit, Source, infer_limit};
use coqui_acp_history::{Body, Document, Line, Stop, StopReason};
use serde_json::Value;

/// The one line sent to a session that moved when the role has no `resume_prompt` of its own.
pub const DEFAULT_CONTINUATION: &str =
    "The previous agent's usage ran out; continue this session where it stopped.";

/// The most chunks of agent text a limit's message is looked for across, unless told otherwise. A message a streaming agent splits
/// into more pieces than this is not recognised.
pub const DEFAULT_MAX_WINDOW: usize = 64;

/// What to clean and how to recognise it. The phrases and the mark are the host's: the library names none.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Cleaning<'a> {
    /// The words that name a limit, matched without regard to ASCII case (`coqui_acp_api::DEFAULT_LIMIT_PHRASES`, or the person's).
    pub phrases: &'a [&'a str],
    /// The `_meta` key a host's own notices carry; every line or update that has it is dropped from the whole history.
    pub notice_key: Option<&'a str>,
    /// Whether a limit ended the last turn. When it did not (the turn ended well and a bucket is now used up), only notices go.
    pub ended_by_limit: bool,
    /// How many chunks a limit's message may span.
    pub max_window: usize,
    /// What the harness reported of the limit. When it names the messages that are the limit's words, they are the ones dropped
    /// and no phrase is matched; a report from a harness that states them and names none says there is none. The phrases are the
    /// fallback for a harness that reports nothing, or whose report only read the words of an error.
    pub report: Option<&'a LimitHit>,
}

impl<'a> Cleaning<'a> {
    /// A cleaning for a turn a limit ended, recognising `phrases`.
    #[must_use]
    pub const fn new(phrases: &'a [&'a str]) -> Self {
        Self {
            phrases,
            notice_key: None,
            ended_by_limit: true,
            max_window: DEFAULT_MAX_WINDOW,
            report: None,
        }
    }

    /// Gives what the harness reported of the limit.
    #[must_use]
    pub const fn report(mut self, report: &'a LimitHit) -> Self {
        self.report = Some(report);
        self
    }

    /// Sets the key a host's notices carry in `_meta`.
    #[must_use]
    pub const fn notice_key(mut self, key: &'a str) -> Self {
        self.notice_key = Some(key);
        self
    }

    /// Sets whether a limit ended the last turn.
    #[must_use]
    pub const fn ended_by_limit(mut self, ended: bool) -> Self {
        self.ended_by_limit = ended;
        self
    }

    /// Sets how many chunks a limit's message may span.
    #[must_use]
    pub const fn max_window(mut self, chunks: usize) -> Self {
        self.max_window = chunks;
        self
    }
}

/// What was taken out, by kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Removed {
    /// Lines the host's notice mark names.
    pub notices: usize,
    /// Whether the closing `error` or `refusal` of the last turn went.
    pub end: bool,
    /// Chunks of agent text that named the limit or followed it.
    pub limit_text: usize,
    /// Lines of tool calls that never produced a result, with their permission requests and answers.
    pub open_calls: usize,
}

/// A history cleaned.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Cleaned {
    /// The history, with the last turn left open when its end was taken out (an import closes it as cancelled).
    pub document: Document,
    /// What went.
    pub removed: Removed,
    /// Whether the last turn holds any work of the agent's after cleaning: a message, thought, plan or tool call.
    pub had_work: bool,
    /// The last turn's prompt blocks, to send again when it had no work.
    pub prompt: Option<Vec<Value>>,
    /// Whether a limit ended the turn and none of its text was recognised, so it is still in the history: say "limit text not
    /// removed" to the person.
    pub limit_text_left: bool,
}

/// What is sent to the new member after a move.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Resume {
    /// A turn ended well: nothing to continue; the person's next prompt starts on the new member.
    Nothing,
    /// The role's text, or [`DEFAULT_CONTINUATION`].
    Continue(String),
    /// The interrupted turn had no work of the agent's: the person's own prompt, again as it was.
    Repeat(Vec<Value>),
}

/// The continuation rule. A limit that ended the turn with work done sends `role_text` (a blank one counts as unset), else
/// [`DEFAULT_CONTINUATION`]; with no work done it sends the person's `prompt` again; a turn that ended well sends nothing. Nothing
/// else goes into the turn: no summary, account, reset time or instruction.
#[must_use]
pub fn resume(
    role_text: Option<&str>,
    (had_work, prompt): (bool, Option<&[Value]>),
    ended_by_limit: bool,
) -> Resume {
    if !ended_by_limit {
        return Resume::Nothing;
    }
    if !had_work && let Some(prompt) = prompt {
        return Resume::Repeat(prompt.to_vec());
    }
    let text = role_text
        .filter(|text| !text.trim().is_empty())
        .unwrap_or(DEFAULT_CONTINUATION);
    Resume::Continue(text.to_string())
}

impl Cleaned {
    /// What to send the new member: [`resume`] over this cleaning.
    #[must_use]
    pub fn resume(&self, role_text: Option<&str>, ended_by_limit: bool) -> Resume {
        resume(
            role_text,
            (self.had_work, self.prompt.as_deref()),
            ended_by_limit,
        )
    }
}

fn kind_of(update: &Value) -> Option<&str> {
    update.get("sessionUpdate")?.as_str()
}

fn is_text(kind: &str) -> bool {
    matches!(
        kind,
        "agent_message_chunk" | "agent_message" | "agent_thought_chunk" | "agent_thought"
    )
}

fn is_tool(kind: &str) -> bool {
    matches!(kind, "tool_call" | "tool_call_update")
}

fn is_plan(kind: &str) -> bool {
    matches!(kind, "plan" | "plan_update")
}

/// The text an update carries: `content.text`, or the `text` of each block of a `content` list.
fn text_of(update: &Value) -> String {
    let mut out = String::new();
    match update.get("content") {
        Some(Value::Object(block)) => {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                out.push_str(text);
            }
        }
        Some(Value::Array(blocks)) => {
            for text in blocks.iter().filter_map(|b| b.get("text")?.as_str()) {
                out.push_str(text);
            }
        }
        _ => {}
    }
    out
}

fn has_mark(value: &Value, key: &str) -> bool {
    value
        .get("_meta")
        .is_some_and(|meta| meta.get(key).is_some())
}

fn is_notice(line: &Line, key: &str) -> bool {
    if line
        .meta
        .as_ref()
        .is_some_and(|meta| meta.get(key).is_some())
    {
        return true;
    }
    match &line.body {
        Body::Update { update } => has_mark(update, key),
        _ => false,
    }
}

fn update_of(line: &Line) -> Option<&Value> {
    match &line.body {
        Body::Update { update } => Some(update),
        _ => None,
    }
}

fn call_id(line: &Line) -> Option<&str> {
    match &line.body {
        Body::Update { update } => update
            .get("toolCallId")?
            .as_str()
            .filter(|_| kind_of(update).is_some_and(is_tool)),
        Body::PermissionRequest { tool_call, .. } => tool_call.get("toolCallId")?.as_str(),
        Body::PermissionAnswer { tool_call_id, .. } => Some(tool_call_id),
        _ => None,
    }
}

fn matches_limit(text: &str, phrases: &[&str]) -> bool {
    !text.is_empty() && infer_limit(text, phrases).is_some()
}

/// Cleans `document` as `cleaning` says; see [`Cleaning`]. Every turn a limit ended loses what the last one does, so no agent
/// that gets the history reads that it must wait. An earlier turn counts as limit-ended when its closing line carries the
/// harness's report (`LIMIT_META_KEY` in its `_meta`) or, with none, when it failed after text that names a limit; a turn that
/// failed for another reason or ended well is not touched. The host's notices go from the whole history.
///
/// With a limit as the cause, from each such turn it takes out:
/// - the closing `error` line or `refusal` stop of the last turn (an earlier turn's is written as a plain `end_turn` instead, with
///   the harness's limit report removed from its `_meta`, so the history stays well formed);
/// - the agent's message and thought chunks after its last tool call that name the limit (found across up to
///   [`Cleaning::max_window`] chunks, since agents stream), and every chunk of text after them ("please wait", "try again in five
///   hours");
/// - tool calls the turn left open with no result, and the permission requests and answers about them.
///
/// It keeps every turn's prompt, every finished tool call and result, and every message of the agent's own work.
#[must_use]
pub fn clean(document: &Document, cleaning: &Cleaning<'_>) -> Cleaned {
    let lines = &document.lines;
    let mut keep = alloc::vec![true; lines.len()];
    let mut removed = Removed::default();
    if let Some(key) = cleaning.notice_key {
        for (at, line) in lines.iter().enumerate() {
            if is_notice(line, key) {
                keep[at] = false;
                removed.notices += 1;
            }
        }
    }
    let start = lines
        .iter()
        .rposition(|line| matches!(line.body, Body::Prompt { .. }));
    let prompt = start.and_then(|at| match &lines[at].body {
        Body::Prompt { prompt } => Some(prompt.clone()),
        _ => None,
    });
    let mut closed: Vec<usize> = Vec::new();
    let prompts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| matches!(line.body, Body::Prompt { .. }))
        .map(|(at, _)| at)
        .collect();
    for pair in prompts.windows(2) {
        let turn = pair[0] + 1..pair[1];
        clean_earlier(
            lines,
            &mut keep,
            &turn,
            cleaning,
            (&mut removed, &mut closed),
        );
    }
    if let (Some(start), true) = (start, cleaning.ended_by_limit) {
        let turn = start + 1..lines.len();
        for at in turn.clone() {
            if keep[at]
                && matches!(
                    lines[at].body,
                    Body::TurnEnd {
                        stop: Stop::Error | Stop::Acp(StopReason::Refusal),
                        ..
                    }
                )
            {
                keep[at] = false;
                removed.end = true;
            }
        }
        drop_open_calls(lines, &mut keep, &turn, &mut removed);
        match cleaning.report {
            Some(report) if !report.text_ids.is_empty() || !report.text_indexes.is_empty() => {
                drop_reported_text(lines, &mut keep, &turn, report, &mut removed);
            }
            Some(report) if report.source == Source::Reported => {}
            _ => drop_limit_text(lines, &mut keep, &turn, cleaning, &mut removed),
        }
    }
    let had_work = start.is_some_and(|start| {
        (start + 1..lines.len()).any(|at| {
            keep[at]
                && update_of(&lines[at]).is_some_and(|update| {
                    kind_of(update).is_some_and(|kind| {
                        is_tool(kind)
                            || is_plan(kind)
                            || (is_text(kind) && !text_of(update).trim().is_empty())
                    })
                })
        })
    });
    let mut out = document.clone();
    out.lines = lines
        .iter()
        .enumerate()
        .filter(|(at, _)| keep[*at])
        .map(|(at, line)| {
            if closed.contains(&at) {
                closed_plainly(line)
            } else {
                line.clone()
            }
        })
        .collect();
    let said_none = cleaning.report.is_some_and(|report| {
        report.source == Source::Reported
            && report.text_ids.is_empty()
            && report.text_indexes.is_empty()
    });
    let limit_text_left =
        cleaning.ended_by_limit && !removed.end && removed.limit_text == 0 && !said_none;
    Cleaned {
        document: out,
        removed,
        had_work,
        prompt,
        limit_text_left,
    }
}

/// The closing line of an earlier turn a limit ended, as a plain `end_turn`: the failure and the harness's limit report go, the
/// time and any other `_meta` stay.
fn closed_plainly(line: &Line) -> Line {
    let mut line = line.clone();
    if let Body::TurnEnd { stop, error } = &mut line.body {
        if matches!(stop, Stop::Error | Stop::Acp(StopReason::Refusal)) {
            *stop = Stop::Acp(StopReason::EndTurn);
        }
        *error = None;
    }
    if let Some(Value::Object(meta)) = &mut line.meta {
        meta.remove(LIMIT_META_KEY);
        if meta.is_empty() {
            line.meta = None;
        }
    }
    line
}

/// An earlier turn goes through the cleaner only when a limit ended it: its closing line carries the harness's report under
/// `LIMIT_META_KEY`, or, with none, it failed after text that names a limit. A turn that failed for another reason, or ended well,
/// is left as it was. Its prompt and work stay, and its ending is listed in `closed` to be written as a plain `end_turn`, so the
/// next turn's prompt does not land inside an open turn.
fn clean_earlier(
    lines: &[Line],
    keep: &mut [bool],
    turn: &core::ops::Range<usize>,
    cleaning: &Cleaning<'_>,
    (removed, closed): (&mut Removed, &mut Vec<usize>),
) {
    let Some(ending) = turn
        .clone()
        .rev()
        .find(|at| keep[*at] && matches!(lines[*at].body, Body::TurnEnd { .. }))
    else {
        return;
    };
    let failed = matches!(
        lines[ending].body,
        Body::TurnEnd {
            stop: Stop::Error | Stop::Acp(StopReason::Refusal),
            ..
        }
    );
    let report: Option<LimitHit> = lines[ending]
        .meta
        .as_ref()
        .and_then(|meta| meta.get(LIMIT_META_KEY))
        .and_then(|hit| serde_json::from_value(hit.clone()).ok());
    let mut taken = Removed::default();
    let mut cut = keep.to_vec();
    match &report {
        Some(report) if !report.text_ids.is_empty() || !report.text_indexes.is_empty() => {
            drop_reported_text(lines, &mut cut, turn, report, &mut taken);
        }
        Some(report) if report.source == Source::Reported => {}
        _ => drop_limit_text(lines, &mut cut, turn, cleaning, &mut taken),
    }
    if report.is_none() && (!failed || taken.limit_text == 0) {
        return;
    }
    taken.end = failed;
    closed.push(ending);
    drop_open_calls(lines, &mut cut, turn, &mut taken);
    keep.copy_from_slice(&cut);
    removed.end |= taken.end;
    removed.limit_text += taken.limit_text;
    removed.open_calls += taken.open_calls;
}

fn drop_open_calls(
    lines: &[Line],
    keep: &mut [bool],
    turn: &core::ops::Range<usize>,
    removed: &mut Removed,
) {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut done: BTreeSet<&str> = BTreeSet::new();
    for at in turn.clone().filter(|at| keep[*at]) {
        let Some(update) = update_of(&lines[at]) else {
            continue;
        };
        if let Some(id) = call_id(&lines[at]) {
            seen.insert(id);
            if matches!(
                update.get("status").and_then(Value::as_str),
                Some("completed" | "failed")
            ) {
                done.insert(id);
            }
        }
    }
    for at in turn.clone() {
        if !keep[at] {
            continue;
        }
        let open = match &lines[at].body {
            Body::PermissionRequest { .. }
            | Body::PermissionAnswer { .. }
            | Body::Update { .. } => call_id(&lines[at]),
            _ => None,
        }
        .is_some_and(|id| seen.contains(id) && !done.contains(id));
        // A permission request for a call that never appeared in an update is still about a call that did not run.
        let asked_only = matches!(
            lines[at].body,
            Body::PermissionRequest { .. } | Body::PermissionAnswer { .. }
        ) && call_id(&lines[at])
            .is_some_and(|id| !seen.contains(id) && !done.contains(id));
        if open || asked_only {
            keep[at] = false;
            removed.open_calls += 1;
        }
    }
}

/// The updates the harness named as the limit's words: message and thought chunks by their position among the turn's, or any
/// message update by its `messageId`.
fn drop_reported_text(
    lines: &[Line],
    keep: &mut [bool],
    turn: &core::ops::Range<usize>,
    report: &LimitHit,
    removed: &mut Removed,
) {
    let mut position = 0u32;
    for at in turn.clone() {
        let Some(update) = update_of(&lines[at]) else {
            continue;
        };
        let Some(kind) = kind_of(update) else {
            continue;
        };
        let chunk = matches!(kind, "agent_message_chunk" | "agent_thought_chunk");
        let by_position = chunk && report.text_indexes.contains(&position);
        if chunk {
            position += 1;
        }
        let by_id = is_text(kind)
            && update
                .get("messageId")
                .and_then(Value::as_str)
                .is_some_and(|id| report.text_ids.iter().any(|each| each == id));
        if keep[at] && (by_position || by_id) {
            keep[at] = false;
            removed.limit_text += 1;
        }
    }
}

fn drop_limit_text(
    lines: &[Line],
    keep: &mut [bool],
    turn: &core::ops::Range<usize>,
    cleaning: &Cleaning<'_>,
    removed: &mut Removed,
) {
    // The tail: chunks of text after the last line that is work.
    let work_at = turn.clone().rev().find(|at| {
        keep[*at]
            && match &lines[*at].body {
                Body::PermissionRequest { .. } | Body::PermissionAnswer { .. } => true,
                Body::Update { update } => {
                    kind_of(update).is_some_and(|kind| is_tool(kind) || is_plan(kind))
                }
                _ => false,
            }
    });
    let from = work_at.map_or(turn.start, |at| at + 1);
    let tail: Vec<(usize, String)> = (from..turn.end)
        .filter(|at| keep[*at])
        .filter_map(|at| {
            let update = update_of(&lines[at])?;
            kind_of(update)
                .filter(|kind| is_text(kind))
                .map(|_| (at, text_of(update)))
        })
        .collect();
    let window = cleaning.max_window.max(1);
    for last in 0..tail.len() {
        for size in 1..=window.min(last + 1) {
            let first = last + 1 - size;
            let joined: String = tail[first..=last]
                .iter()
                .map(|(_, text)| text.as_str())
                .collect();
            if matches_limit(&joined, cleaning.phrases) {
                for (at, _) in &tail[first..] {
                    keep[*at] = false;
                    removed.limit_text += 1;
                }
                return;
            }
        }
    }
}
