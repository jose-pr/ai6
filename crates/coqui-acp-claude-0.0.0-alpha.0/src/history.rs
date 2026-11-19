//! Claude Code's session transcript as a standard history, and back.
//!
//! [`export`] reads a transcript (`projects/<directory>/<id>.jsonl`) into a [`Document`]; [`import`]
//! writes a transcript Claude Code resumes from a [`Document`]. What Claude Code keeps beyond the
//! conversation (its system prompt, file-history snapshots, titles, queue operations, subagent
//! transcripts) is not part of a history and is not carried.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;

use coqui_acp_harness::event::{ToolContent, context_block};
use coqui_acp_history::conversation::{
    Boundary, Call, Conversation, Item, Recorder, Stamp, call_note, clean_id, derived_uuid,
    is_uuid, shell_text, valid_time,
};
use coqui_acp_history::{
    ATTACHMENT_BY_DIGEST, ATTACHMENT_META_KEY, Document, Header, LossReport, Origin,
    SUMMARY_MISSING, SeedPoint, Stop, StopReason, THINKING, TOOL_AS_NOTE, Tool, Trigger,
};
use serde_json::{Map, Value, json};

use crate::tools::{describe, finished_diff, result_text};
use crate::{Error, TESTED_VERSIONS, project_directory, valid_session_id};

/// The harness name in a history's header and in the `_meta` this codec keeps.
pub const HARNESS: &str = "claude";

/// A loss the tool table has no name for: content the target cannot take at all.
pub const CONTENT_DROPPED: &str = "content_dropped";

/// The text Claude Code writes where the person interrupted.
const INTERRUPTED: &str = "[Request interrupted by user]";

/// The text a cancelled tool call's result carries.
const CANCELLED_RESULT: &str = "The tool call did not finish before the history ended.";

/// What the codecs read and write, in bytes: a transcript larger than the bound is refused.
pub const DEFAULT_MAX_BYTES: u64 = 256 << 20;

fn refused(why: impl Into<String>) -> Error {
    Error::Refused(why.into())
}

// ---------------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------------

/// The transcript in `text` as a history named `id` (the host's UUID).
///
/// The conversation is the chain of messages from the transcript's last message back through
/// `parentUuid` (a compaction boundary continues through `logicalParentUuid`); when a link of the
/// chain is missing from the file, every message of the file in its order. Subagent (`isSidechain`)
/// messages are left out. A line that is not JSON is skipped.
///
/// # Errors
///
/// [`Error::Refused`] for an `id` that is not a UUID or a history the format refuses.
pub fn export(text: &str, id: &str) -> Result<Document, Error> {
    let values: Vec<Value> = text
        .lines()
        .filter(|line| line.trim_start().starts_with('{'))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    let chain = conversation_of(&values);
    let mut exporter = Exporter::default();
    for value in &chain {
        exporter.line(value);
    }
    let lines = exporter.finish();
    let mut header = Header::new(id, Origin::new(HARNESS));
    header.origin.version = chain
        .iter()
        .rev()
        .find_map(|value| string(value, "version"))
        .filter(|text| text.len() <= 128);
    header.origin.session_id = chain.iter().find_map(|value| string(value, "sessionId"));
    header.created_at = chain
        .iter()
        .find_map(|value| string(value, "timestamp"))
        .filter(|at| valid_time(at));
    header.cwd = chain.iter().find_map(|value| string(value, "cwd"));
    header.model = exporter.model.clone();
    header.effort = exporter.effort.clone();
    header.mode = exporter.mode.clone();
    header = tidied(header);
    let mut document = Document::new(header);
    document.lines = lines;
    document
        .seal()
        .map_err(|error| refused(format!("the transcript is not a history: {error}")))?;
    Ok(document)
}

/// `header` with every optional field the format refuses dropped: they come from a file this code
/// does not control.
fn tidied(mut header: Header) -> Header {
    let ok = |header: &Header| header.to_line().is_ok();
    for clear in [
        (|h: &mut Header| h.origin.session_id = None) as fn(&mut Header),
        |h| h.origin.version = None,
        |h| h.cwd = None,
        |h| h.model = None,
        |h| h.mode = None,
        |h| h.effort = None,
        |h| h.created_at = None,
    ] {
        if ok(&header) {
            break;
        }
        clear(&mut header);
    }
    header
}

fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool) == Some(true)
}

fn is_message(value: &Value) -> bool {
    match value.get("type").and_then(Value::as_str) {
        Some("user" | "assistant") => value.get("message").is_some_and(Value::is_object),
        Some("system") => value.get("subtype").and_then(Value::as_str) == Some("compact_boundary"),
        _ => false,
    }
}

/// The lines of the conversation, in order.
fn conversation_of(values: &[Value]) -> Vec<&Value> {
    let candidates: Vec<usize> = (0..values.len())
        .filter(|&at| is_message(&values[at]) && !flag(&values[at], "isSidechain"))
        .collect();
    let Some(&leaf) = candidates.last() else {
        return Vec::new();
    };
    let index: BTreeMap<&str, usize> = values
        .iter()
        .enumerate()
        .filter_map(|(at, value)| Some((value.get("uuid")?.as_str()?, at)))
        .collect();
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut at = leaf;
    let complete = loop {
        if !seen.insert(at) {
            break false;
        }
        chain.push(at);
        let value = &values[at];
        let parent = value
            .get("parentUuid")
            .and_then(Value::as_str)
            .or_else(|| value.get("logicalParentUuid").and_then(Value::as_str));
        match parent {
            // The root is the first message; a message that is a root further on is not chained to what came
            // before it, so the file's order is all there is to follow.
            None => break at == candidates[0] || values[at]["type"] == "system",
            Some(parent) => match index.get(parent) {
                Some(&next) => at = next,
                None => break false,
            },
        }
    };
    if complete {
        chain.reverse();
        chain
            .into_iter()
            .filter(|&at| is_message(&values[at]))
            .map(|at| &values[at])
            .collect()
    } else {
        candidates.into_iter().map(|at| &values[at]).collect()
    }
}

#[derive(Default)]
struct Exporter {
    rec: Recorder,
    /// Each announced call's name and input, for its result.
    calls: BTreeMap<String, (String, Value)>,
    open_calls: BTreeSet<String>,
    /// The turn's last assistant `stop_reason`.
    stop_reason: Option<String>,
    /// The turn's API error, in its words.
    error: Option<String>,
    interrupted: bool,
    /// The last message was the person's or a tool's, so the agent owes an answer.
    owes: bool,
    boundary: Option<(Option<Trigger>, Option<u64>, Stamp)>,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
}

impl Exporter {
    fn stamp(value: &Value) -> Stamp {
        Stamp::at(string(value, "timestamp").filter(|at| valid_time(at)))
    }

    fn line(&mut self, value: &Value) {
        match value.get("type").and_then(Value::as_str) {
            Some("system") => {
                self.flush_boundary();
                let meta = value.get("compactMetadata");
                let trigger = match meta
                    .and_then(|meta| meta.get("trigger"))
                    .and_then(Value::as_str)
                {
                    Some("auto") => Some(Trigger::Auto),
                    Some("manual") => Some(Trigger::Manual),
                    _ => None,
                };
                let pre = meta
                    .and_then(|meta| meta.get("preTokens"))
                    .and_then(Value::as_u64);
                self.boundary = Some((trigger, pre, Self::stamp(value)));
            }
            Some("user") => self.user(value),
            Some("assistant") => self.assistant(value),
            _ => {}
        }
    }

    fn flush_boundary(&mut self) {
        if let Some((trigger, pre, stamp)) = self.boundary.take() {
            self.rec.compaction(None, trigger, pre, stamp);
        }
    }

    fn user(&mut self, value: &Value) {
        let stamp = Self::stamp(value);
        let content = value.pointer("/message/content");
        let blocks: Vec<Value> = match content {
            Some(Value::String(text)) => vec![json!({"type": "text", "text": text})],
            Some(Value::Array(blocks)) => blocks.clone(),
            _ => Vec::new(),
        };
        if flag(value, "isCompactSummary") {
            let summary = result_text(content);
            let (trigger, pre, boundary) =
                self.boundary
                    .take()
                    .unwrap_or((None, None, Stamp::default()));
            let stamp = Stamp::at(boundary.at.or(stamp.at));
            self.rec.compaction(Some(summary), trigger, pre, stamp);
            return;
        }
        self.flush_boundary();
        if let Some(mode) = string(value, "permissionMode") {
            self.mode = Some(mode);
        }
        let results: Vec<&Value> = blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            .collect();
        if !results.is_empty() {
            let outcome = value.get("toolUseResult");
            for block in results {
                self.result(block, outcome, &stamp);
            }
            for block in &blocks {
                if block.get("type").and_then(Value::as_str) == Some("text")
                    && block
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.starts_with("[Request interrupted"))
                {
                    self.interrupted = true;
                    self.rec.cancel(stamp.clone());
                }
            }
            self.owes = true;
            return;
        }
        let text: String = blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        if text.starts_with("[Request interrupted") {
            self.interrupted = true;
            self.rec.cancel(stamp);
            return;
        }
        let command = text.starts_with("<local-command-") || text.starts_with("<command-");
        let acp: Vec<Value> = blocks.iter().filter_map(prompt_block).collect();
        if acp.is_empty() {
            return;
        }
        if flag(value, "isMeta") || command {
            let mut fields = Map::new();
            fields.insert("isMeta".into(), json!(flag(value, "isMeta")));
            for block in acp {
                self.rec
                    .note(block, stamp.clone().native(HARNESS, fields.clone()));
            }
            return;
        }
        self.close_turn(false);
        self.rec.prompt(acp, stamp);
        self.owes = true;
    }

    fn result(&mut self, block: &Value, outcome: Option<&Value>, stamp: &Stamp) {
        let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
            return;
        };
        self.open_calls.remove(id);
        let failed = flag(block, "is_error");
        let mut call = Call::new(id);
        call.status = Some(if failed { "failed" } else { "completed" }.to_owned());
        call.raw_output = block.get("content").cloned();
        let known = self.calls.get(id);
        let shown = finished_diff(
            known.map_or("", |(name, _)| name.as_str()),
            outcome.unwrap_or(&Value::Null),
        )
        .filter(|_| !failed);
        call.content = match shown {
            Some((diff, _)) => vec![json!({"type": "diff", "path": diff.path,
                "oldText": diff.old_text, "newText": diff.new_text})],
            None => {
                let text = result_text(block.get("content"));
                if text.is_empty() || known.is_some_and(|(name, _)| name == "Read") {
                    Vec::new()
                } else {
                    vec![json!({"type": "content", "content": {"type": "text", "text": text}})]
                }
            }
        };
        self.rec.update(call.finish(), stamp.clone());
    }

    fn assistant(&mut self, value: &Value) {
        self.flush_boundary();
        let stamp = Self::stamp(value);
        let message = value.get("message").unwrap_or(&Value::Null);
        let model = string(message, "model").filter(|model| model != "<synthetic>");
        if model.is_some() {
            self.model.clone_from(&model);
        }
        if let Some(effort) = string(value, "effort") {
            self.effort = Some(effort);
        }
        let reason = string(message, "stop_reason");
        if reason.is_some() {
            self.stop_reason.clone_from(&reason);
        }
        self.owes = false;
        let blocks = message
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if flag(value, "isApiErrorMessage") || value.get("error").is_some_and(|e| !e.is_null()) {
            let text = blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            self.error = Some(if text.is_empty() {
                "the API refused the request".to_owned()
            } else {
                text
            });
            return;
        }
        let mut base = Map::new();
        if let Some(id) = string(message, "id") {
            base.insert("messageId".into(), json!(id));
        }
        if let Some(model) = &model {
            base.insert("model".into(), json!(model));
        }
        for block in &blocks {
            let mut fields = base.clone();
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        self.rec
                            .message(text, stamp.clone().native(HARNESS, fields));
                    }
                }
                Some("thinking") => {
                    if let Some(signature) = block.get("signature").and_then(Value::as_str) {
                        fields.insert("signature".into(), json!(signature));
                    }
                    let text = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.rec
                        .thought(text, stamp.clone().native(HARNESS, fields));
                }
                Some("redacted_thinking") => {
                    if let Some(data) = block.get("data").and_then(Value::as_str) {
                        fields.insert("redacted".into(), json!(data));
                    }
                    self.rec.thought("", stamp.clone().native(HARNESS, fields));
                }
                Some("tool_use") => self.tool_use(block, &stamp, fields),
                _ => {}
            }
        }
        if blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        {
            self.owes = true;
        }
    }

    fn tool_use(&mut self, block: &Value, stamp: &Stamp, fields: Map<String, Value>) {
        let (Some(id), Some(name)) = (
            block.get("id").and_then(Value::as_str),
            block.get("name").and_then(Value::as_str),
        ) else {
            return;
        };
        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
        let told = describe(id, name, &input, 256);
        let mut call = Call::new(id);
        call.name = Some(name.to_owned());
        call.title = told.title.clone();
        call.kind = told.kind.map(|kind| kind.as_str().to_owned());
        call.status = Some("pending".to_owned());
        call.raw_input = Some(input.clone());
        call.locations = told
            .locations
            .iter()
            .flatten()
            .map(|place| match place.line {
                Some(line) => json!({"path": place.path, "line": line}),
                None => json!({"path": place.path}),
            })
            .collect();
        call.content = told
            .content
            .iter()
            .flatten()
            .filter_map(|content| match content {
                ToolContent::Diff(diff) => Some(json!({"type": "diff", "path": diff.path,
                    "oldText": diff.old_text, "newText": diff.new_text})),
                _ => None,
            })
            .collect();
        self.calls
            .insert(id.to_owned(), (name.to_owned(), input.clone()));
        self.open_calls.insert(id.to_owned());
        self.rec.update(
            call.announce(),
            stamp.clone().native(HARNESS, fields.clone()),
        );
        if name == "TodoWrite"
            && let Some(plan) = crate::tools::todo_plan(&input)
        {
            let entries: Vec<Value> = plan
                .iter()
                .map(|entry| json!({"content": entry.content, "status": entry.status.as_str(), "priority": "medium"}))
                .collect();
            self.rec.update(
                json!({"sessionUpdate": "plan", "entries": entries}),
                stamp.clone(),
            );
        }
    }

    /// Ends the turn that is open: how it ended is told by the messages it held.
    fn close_turn(&mut self, at_end: bool) {
        if !self.rec.is_open() {
            return;
        }
        let owed = self.owes || !self.open_calls.is_empty();
        if at_end && owed && !self.interrupted && self.error.is_none() {
            // A recording cut short: the turn stays open.
            return;
        }
        let (stop, error) = if let Some(error) = self.error.take() {
            (Stop::Error, Some(error))
        } else if self.interrupted || owed {
            (Stop::Acp(StopReason::Cancelled), None)
        } else {
            let reason = match self.stop_reason.as_deref() {
                Some("max_tokens") => StopReason::MaxTokens,
                Some("refusal") => StopReason::Refusal,
                _ => StopReason::EndTurn,
            };
            (Stop::Acp(reason), None)
        };
        self.rec.end_turn(stop, error, Stamp::default());
        self.interrupted = false;
        self.stop_reason = None;
        self.open_calls.clear();
        self.owes = false;
    }

    fn finish(&mut self) -> Vec<coqui_acp_history::Line> {
        self.flush_boundary();
        self.close_turn(true);
        core::mem::take(&mut self.rec).finish()
    }
}

/// An ACP content block of a Claude Code content block, when it has one.
fn prompt_block(block: &Value) -> Option<Value> {
    match block.get("type").and_then(Value::as_str)? {
        "text" => Some(json!({"type": "text", "text": block.get("text")?.as_str()?})),
        "image" => Some(json!({"type": "image",
            "mimeType": block.pointer("/source/media_type")?.as_str()?,
            "data": block.pointer("/source/data")?.as_str()?})),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------------------------

/// How an import is done.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImportOptions {
    /// Seed from the latest compaction summary the history has (the default). Off, the seed is every
    /// line of the history and no summary is written.
    pub use_summary: bool,
    /// The Claude Code version the transcript names; the newest tested one when none.
    pub version: Option<String>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            use_summary: true,
            version: None,
        }
    }
}

impl ImportOptions {
    /// The same options seeding from the latest summary or, with `false`, from the first line.
    #[must_use]
    pub const fn use_summary(mut self, yes: bool) -> Self {
        self.use_summary = yes;
        self
    }

    /// The same options naming `version` as the Claude Code that wrote the transcript.
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }
}

/// A transcript written from a history.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Imported {
    /// The transcript: JSON Lines, every line ended by a line feed.
    pub text: String,
    /// What could not be carried over.
    pub report: LossReport,
    /// Where in the history the seed starts.
    pub seeded_from: Option<SeedPoint>,
    /// How many tool calls the history left open and the transcript ends as cancelled.
    pub closed_calls: u64,
    /// Whether the history left a turn open and the transcript ends it as interrupted.
    pub closed_turn: bool,
}

/// The transcript of `doc` for session `id`, working in `cwd`.
///
/// The conversation is the seed ([`Conversation::of`]): a compaction summary goes only to the slot
/// Claude Code keeps for one, tool calls are written as Claude Code's own when the history's origin
/// is Claude Code or when the tool table names an equivalent (and as a plain note otherwise), and
/// signed thinking is written only for a history from Claude Code. Nothing is run, no permission
/// answer is written as a grant, and no file snapshot is made.
///
/// # Errors
///
/// [`Error::Refused`] for an `id` that is not a UUID or a `cwd` that is not text a path can be.
pub fn import(
    doc: &Document,
    cwd: &str,
    id: &str,
    options: &ImportOptions,
) -> Result<Imported, Error> {
    if !is_uuid(id) {
        return Err(refused("the session id is not a UUID"));
    }
    let conversation = Conversation::of(doc, options.use_summary);
    let mut writer = Transcript::new(doc, cwd, id, options);
    writer.write(&conversation);
    let mut report = writer.report;
    report.add(
        SUMMARY_MISSING,
        u64::from(conversation.summary_missing),
        Some("the latest compaction has no summary, so the seed starts earlier"),
    );
    Ok(Imported {
        text: writer.out,
        report,
        seeded_from: conversation.seeded_from.clone(),
        closed_calls: conversation.closed_calls,
        closed_turn: conversation.closed_turn,
    })
}

struct Transcript<'a> {
    doc: &'a Document,
    session: String,
    cwd: String,
    version: String,
    out: String,
    report: LossReport,
    count: u64,
    previous: Option<String>,
    prompt_id: String,
    time: String,
    /// The last assistant line written, to give it a stop reason.
    last_assistant: Option<usize>,
    lines: Vec<Value>,
    message: Option<(String, String)>,
    cancelled_turn: bool,
    ids: BTreeMap<String, String>,
}

impl<'a> Transcript<'a> {
    fn new(doc: &'a Document, cwd: &str, session: &str, options: &ImportOptions) -> Self {
        Self {
            doc,
            session: session.to_owned(),
            cwd: cwd.to_owned(),
            version: options
                .version
                .clone()
                .or_else(|| TESTED_VERSIONS.last().map(|version| (*version).to_owned()))
                .unwrap_or_default(),
            out: String::new(),
            report: LossReport::new(),
            count: 0,
            previous: None,
            prompt_id: derived_uuid(session, "prompt", 0),
            time: doc
                .header
                .created_at
                .clone()
                .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_owned()),
            last_assistant: None,
            lines: Vec::new(),
            message: None,
            cancelled_turn: false,
            ids: BTreeMap::new(),
        }
    }

    fn is_claude(&self) -> bool {
        self.doc.header.origin.harness == HARNESS
    }

    fn write(&mut self, conversation: &Conversation) {
        if let Some(boundary) = &conversation.summary {
            self.boundary(boundary);
        }
        for item in &conversation.items {
            self.item(conversation, item);
        }
        self.finish();
    }

    fn finish(&mut self) {
        let mut text = String::new();
        for line in &self.lines {
            text.push_str(&line.to_string());
            text.push('\n');
        }
        self.out = text;
    }

    fn note_time(&mut self, stamp: &Stamp) {
        if let Some(at) = stamp.at.as_deref().filter(|at| valid_time(at)) {
            at.clone_into(&mut self.time);
        }
    }

    /// Pushes a line of type `kind` with `message`, chained to the one before.
    fn push(&mut self, kind: &str, message: Value, extra: Map<String, Value>) -> usize {
        let uuid = derived_uuid(&self.session, "line", self.count);
        self.count += 1;
        let mut line = Map::new();
        line.insert(
            "parentUuid".into(),
            self.previous.clone().map_or(Value::Null, Value::String),
        );
        line.insert("isSidechain".into(), json!(false));
        if kind == "user" && !extra.contains_key("isCompactSummary") {
            line.insert("promptId".into(), json!(self.prompt_id));
        }
        line.insert("type".into(), json!(kind));
        line.insert("message".into(), message);
        for (key, value) in extra {
            line.insert(key, value);
        }
        line.insert("uuid".into(), json!(uuid));
        line.insert("timestamp".into(), json!(self.time));
        line.insert("userType".into(), json!("external"));
        line.insert("entrypoint".into(), json!("cli"));
        line.insert("cwd".into(), json!(self.cwd));
        line.insert("sessionId".into(), json!(self.session));
        line.insert("version".into(), json!(self.version));
        line.insert("gitBranch".into(), json!("HEAD"));
        self.previous = Some(uuid);
        self.lines.push(Value::Object(line));
        self.lines.len() - 1
    }

    fn user(&mut self, content: Value, extra: Map<String, Value>) {
        self.message = None;
        self.push("user", json!({"role": "user", "content": content}), extra);
    }

    fn boundary(&mut self, boundary: &Boundary) {
        let mut meta = Map::new();
        meta.insert(
            "trigger".into(),
            json!(match boundary.trigger {
                Some(Trigger::Auto) => "auto",
                _ => "manual",
            }),
        );
        if let Some(pre) = boundary.pre_tokens {
            meta.insert("preTokens".into(), json!(pre));
        }
        self.note_time(&boundary.stamp);
        let logical = self.previous.take();
        let mut extra = Map::new();
        extra.insert("subtype".into(), json!("compact_boundary"));
        extra.insert("content".into(), json!("Conversation compacted"));
        extra.insert("isMeta".into(), json!(false));
        extra.insert("level".into(), json!("info"));
        extra.insert("compactMetadata".into(), Value::Object(meta));
        if let Some(logical) = logical {
            extra.insert("logicalParentUuid".into(), json!(logical));
        }
        let mut line = self.system_line(extra);
        line.remove("message");
        self.lines.push(Value::Object(line));
        let mut extra = Map::new();
        extra.insert("isVisibleInTranscriptOnly".into(), json!(true));
        extra.insert("isCompactSummary".into(), json!(true));
        self.user(json!(boundary.summary), extra);
    }

    fn system_line(&mut self, extra: Map<String, Value>) -> Map<String, Value> {
        let uuid = derived_uuid(&self.session, "line", self.count);
        self.count += 1;
        let mut line = Map::new();
        line.insert("parentUuid".into(), Value::Null);
        line.insert("isSidechain".into(), json!(false));
        line.insert("type".into(), json!("system"));
        line.insert("message".into(), Value::Null);
        for (key, value) in extra {
            line.insert(key, value);
        }
        line.insert("uuid".into(), json!(uuid));
        line.insert("timestamp".into(), json!(self.time));
        line.insert("userType".into(), json!("external"));
        line.insert("entrypoint".into(), json!("cli"));
        line.insert("cwd".into(), json!(self.cwd));
        line.insert("sessionId".into(), json!(self.session));
        line.insert("version".into(), json!(self.version));
        line.insert("gitBranch".into(), json!("HEAD"));
        self.previous = Some(uuid);
        line
    }

    fn item(&mut self, conversation: &Conversation, item: &Item) {
        match item {
            Item::Prompt { blocks, stamp } => {
                self.note_time(stamp);
                self.prompt_id = derived_uuid(&self.session, "prompt", self.count);
                self.cancelled_turn = false;
                let content = self.content(blocks);
                if !content.is_empty() {
                    self.user(Value::Array(content), Map::new());
                }
            }
            Item::Note { block, stamp } => {
                self.note_time(stamp);
                let meta = stamp
                    .native_of(HARNESS)
                    .filter(|_| self.is_claude())
                    .and_then(|fields| fields.get("isMeta"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let mut extra = Map::new();
                if meta {
                    extra.insert("isMeta".into(), json!(true));
                }
                let content = self.content(core::slice::from_ref(block));
                match content.as_slice() {
                    [only] if only.get("type").and_then(Value::as_str) == Some("text") => {
                        self.user(only["text"].clone(), extra);
                    }
                    [] => {}
                    _ => self.user(Value::Array(content), extra),
                }
            }
            Item::Message { text, stamp } => {
                self.note_time(stamp);
                let model = self.model_of(stamp);
                self.assistant(stamp, &model, json!({"type": "text", "text": text}));
            }
            Item::Thought { text, stamp } => {
                self.note_time(stamp);
                self.thought(text, stamp);
            }
            Item::Call { id } => {
                if let Some(call) = conversation.call(id) {
                    self.call(call);
                }
            }
            Item::Result { id } => {
                if let Some(call) = conversation.call(id) {
                    self.result(call);
                }
            }
            Item::Plan { .. } => {}
            Item::Cancel { stamp } => {
                self.note_time(stamp);
                self.cancelled_turn = true;
                self.user(json!(INTERRUPTED), Map::new());
            }
            Item::TurnEnd { stop, stamp, .. } => {
                self.note_time(stamp);
                if matches!(stop, Stop::Acp(StopReason::Cancelled)) && !self.cancelled_turn {
                    self.cancelled_turn = true;
                    self.user(json!(INTERRUPTED), Map::new());
                }
                self.end_message(*stop);
            }
            _ => {}
        }
    }

    /// Gives the last assistant line of a turn the stop reason the turn ended with.
    fn end_message(&mut self, stop: Stop) {
        if let Some(at) = self.last_assistant.take() {
            let reason = match stop {
                Stop::Acp(StopReason::MaxTokens) => "max_tokens",
                Stop::Acp(StopReason::Refusal) => "refusal",
                _ => "end_turn",
            };
            let line = &mut self.lines[at];
            let has_tool = line["message"]["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_use"));
            if !has_tool && line["message"]["stop_reason"].is_null() {
                line["message"]["stop_reason"] = json!(reason);
            }
        }
        self.message = None;
    }

    fn model_of(&self, stamp: &Stamp) -> String {
        stamp
            .native_of(HARNESS)
            .filter(|_| self.is_claude())
            .and_then(|fields| fields.get("model"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| self.doc.header.model.clone())
            .unwrap_or_else(|| "unknown".to_owned())
    }

    /// Writes one block of the assistant's message, in the message the last one belongs to.
    fn assistant(&mut self, stamp: &Stamp, model: &str, block: Value) {
        let id = stamp
            .native_of(HARNESS)
            .filter(|_| self.is_claude())
            .and_then(|fields| fields.get("messageId"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let message_id = match (&self.message, id) {
            (Some((current, _)), None) => current.clone(),
            (Some((current, _)), Some(id)) if *current == id => id,
            (_, Some(id)) => id,
            (None, None) => format!(
                "msg_{}",
                derived_uuid(&self.session, "message", self.count)[..24].replace('-', "")
            ),
        };
        self.message = Some((message_id.clone(), model.to_owned()));
        let at = self.push(
            "assistant",
            json!({"id": message_id, "type": "message", "role": "assistant", "model": model,
                "content": [block], "stop_reason": null, "stop_sequence": null,
                "usage": {"input_tokens": 0, "output_tokens": 0}}),
            Map::new(),
        );
        self.last_assistant = Some(at);
    }

    fn thought(&mut self, text: &str, stamp: &Stamp) {
        let native = stamp.native_of(HARNESS).filter(|_| self.is_claude());
        let model = self.model_of(stamp);
        if let Some(signature) = native
            .and_then(|fields| fields.get("signature"))
            .and_then(Value::as_str)
        {
            self.assistant(
                stamp,
                &model,
                json!({"type": "thinking", "thinking": text, "signature": signature}),
            );
        } else if let Some(data) = native
            .and_then(|fields| fields.get("redacted"))
            .and_then(Value::as_str)
        {
            self.assistant(
                stamp,
                &model,
                json!({"type": "redacted_thinking", "data": data}),
            );
        } else {
            self.report.add(
                THINKING,
                1,
                Some("Claude Code takes thinking only with the signature it was written with"),
            );
        }
    }

    /// How a call is written: Claude Code's own tool when there is one.
    fn native_call(&self, call: &Call) -> Option<(String, Value)> {
        let name = call.name.as_deref()?;
        let input = call.raw_input.as_ref()?;
        if self.is_claude() {
            return input.is_object().then(|| (name.to_owned(), input.clone()));
        }
        let (tool, normal) =
            coqui_acp_history::tools::normalise(&self.doc.header.origin.harness, name, input)?;
        let (tool, normal) = if tool == Tool::Patch {
            patch_as_write_or_edit(&normal)?
        } else {
            (tool, normal)
        };
        let (native, input) = native_input(tool, &normal)?;
        Some((native.to_owned(), input))
    }

    fn call_id(&mut self, id: &str) -> String {
        if let Some(known) = self.ids.get(id) {
            return known.clone();
        }
        let clean = clean_id(id, &self.ids);
        self.ids.insert(id.to_owned(), clean.clone());
        clean
    }

    fn call(&mut self, call: &Call) {
        let stamp = Stamp::new(call.call_at.clone(), call.call_meta.clone());
        self.note_time(&stamp);
        if let Some((name, input)) = self.native_call(call) {
            let id = self.call_id(&call.id);
            let model = self.model_of(&stamp);
            self.assistant(
                &stamp,
                &model,
                json!({"type": "tool_use", "id": id, "name": name, "input": input}),
            );
            let at = self.last_assistant;
            if let Some(at) = at {
                self.lines[at]["message"]["stop_reason"] = json!("tool_use");
            }
        }
    }

    fn result(&mut self, call: &Call) {
        if !call.announced {
            return;
        }
        let stamp = Stamp::new(
            call.result_at.clone().or_else(|| call.call_at.clone()),
            call.result_meta.clone(),
        );
        self.note_time(&stamp);
        if self.native_call(call).is_some() {
            let id = self.call_id(&call.id);
            let failed = call.is_failed();
            let content = self.result_content(call);
            let mut block = Map::new();
            block.insert("tool_use_id".into(), json!(id));
            block.insert("type".into(), json!("tool_result"));
            block.insert("content".into(), content);
            if failed {
                block.insert("is_error".into(), json!(true));
            }
            self.user(json!([Value::Object(block)]), Map::new());
        } else {
            self.report.add(
                TOOL_AS_NOTE,
                1,
                Some("Claude Code has no equivalent tool, so the call is written as text"),
            );
            let model = self.model_of(&stamp);
            let note = call_note(call);
            self.assistant(&stamp, &model, json!({"type": "text", "text": note}));
        }
    }

    fn result_content(&self, call: &Call) -> Value {
        if call.status.as_deref() == Some("cancelled")
            && call.raw_output.is_none()
            && call.content.is_empty()
        {
            return json!(CANCELLED_RESULT);
        }
        if self.is_claude()
            && let Some(output) = call
                .raw_output
                .as_ref()
                .filter(|o| o.is_string() || o.is_array())
        {
            return output.clone();
        }
        let text = call.output_text();
        match (&call.raw_output, text.is_empty()) {
            (Some(Value::String(text)), _) => json!(text),
            (Some(other), true) if !other.is_null() => json!(other.to_string()),
            _ => json!(text),
        }
    }

    /// Claude Code content blocks of ACP's.
    fn content(&mut self, blocks: &[Value]) -> Vec<Value> {
        let mut out = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        out.push(json!({"type": "text", "text": text}));
                    }
                }
                Some("image") => {
                    let data = block
                        .get("data")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let mime = block
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("image/png");
                    if data.is_empty() {
                        self.left_out(block);
                        out.push(json!({"type": "text", "text": format!("[an attachment of {mime} was left out of the history]")}));
                    } else {
                        out.push(json!({"type": "image", "source": {"type": "base64", "media_type": mime, "data": data}}));
                    }
                }
                Some("resource_link") => {
                    let uri = block.get("uri").and_then(Value::as_str).unwrap_or_default();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|n| !n.is_empty())
                        .unwrap_or(uri);
                    out.push(json!({"type": "text", "text": format!("[@{name}]({uri})")}));
                }
                Some("resource") => {
                    let uri = block
                        .pointer("/resource/uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if let Some(text) = block.pointer("/resource/text").and_then(Value::as_str) {
                        out.push(json!({"type": "text", "text": context_block(uri, text)}));
                    } else {
                        self.report.add(
                            CONTENT_DROPPED,
                            1,
                            Some("a resource of bytes, which Claude Code reads only as an image"),
                        );
                    }
                }
                _ => self.report.add(
                    CONTENT_DROPPED,
                    1,
                    Some("a content block of a kind Claude Code does not take"),
                ),
            }
        }
        out
    }

    fn left_out(&mut self, block: &Value) {
        let by_digest = block
            .get("_meta")
            .and_then(|meta| meta.get(ATTACHMENT_META_KEY))
            .is_some();
        self.report.add(
            if by_digest {
                ATTACHMENT_BY_DIGEST
            } else {
                CONTENT_DROPPED
            },
            1,
            Some("the history left the bytes out"),
        );
    }
}

/// A patch of one change as the normalised Write or Edit it is.
fn patch_as_write_or_edit(normal: &Value) -> Option<(Tool, Value)> {
    use coqui_acp_history::patch::{FileOp, parse};
    Some(match parse(normal.get("patch")?.as_str()?)? {
        FileOp::Add { path, content } => (Tool::Write, json!({"path": path, "content": content})),
        FileOp::Update { path, old, new } => {
            (Tool::Edit, json!({"path": path, "old": old, "new": new}))
        }
        _ => return None,
    })
}

/// The input of Claude Code's tool for the normalised `tool`, from its normalised `input`.
fn native_input(tool: Tool, normal: &Value) -> Option<(&'static str, Value)> {
    let (name, mut input) = coqui_acp_history::tools::denormalise(tool, HARNESS, normal)?;
    if tool == Tool::Shell {
        // Claude Code's Bash takes a command line as text; an argument vector is joined.
        if let Some(argv) = input.get("command").and_then(Value::as_array) {
            let words: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
            input["command"] = json!(shell_text(&words));
        }
        let keep = ["command", "timeout", "description"];
        input
            .as_object_mut()?
            .retain(|key, _| keep.contains(&key.as_str()));
    }
    Some((name, input))
}

// ---------------------------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------------------------

/// The history of session `id` that worked in `cwd`, from the transcript under `config`.
///
/// # Errors
///
/// [`Error::Refused`] when there is no such transcript, it is larger than `max_bytes`, or it is not a
/// transcript.
pub fn export_file(config: &Path, cwd: &Path, id: &str, max_bytes: u64) -> Result<Document, Error> {
    let path = crate::transcript(config, cwd, id)?;
    let file = std::fs::File::open(&path)
        .map_err(|_| refused("the session's transcript could not be read"))?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| refused("the session's transcript could not be read"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(refused(format!(
            "the session's transcript is larger than {max_bytes} bytes"
        )));
    }
    let text = String::from_utf8_lossy(&bytes);
    export(&text, id)
}

/// Writes the transcript of `doc` as session `id` in `cwd` under `config`, where `--resume` finds it. A
/// transcript there already is never replaced.
///
/// # Errors
///
/// [`Error::Refused`] for an `id` that is not a UUID, a transcript that exists or a directory that
/// cannot be made.
pub fn import_file(
    config: &Path,
    cwd: &Path,
    id: &str,
    doc: &Document,
    options: &ImportOptions,
) -> Result<Imported, Error> {
    if !valid_session_id(id) {
        return Err(refused("that is not a Claude Code session id"));
    }
    let imported = import(doc, &cwd.to_string_lossy(), id, options)?;
    let directory = config.join("projects").join(project_directory(cwd));
    std::fs::create_dir_all(&directory)
        .map_err(|_| refused("the session's directory could not be made"))?;
    let path = directory.join(format!("{id}.jsonl"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                refused("the id names a session already")
            } else {
                refused("the session's transcript could not be written")
            }
        })?;
    if file.write_all(imported.text.as_bytes()).is_err() {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(refused("the session's transcript could not be written"));
    }
    Ok(imported)
}
