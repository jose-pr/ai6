//! A standard history written as the file `opencode import` reads.
//!
//! The file is the JSON `opencode export` prints: `info` (the session) and `messages`, each `{info, parts}`. Measured on
//! 1.19.6 with a hand-written file: `import` takes ids that start `ses`, `msg` and `prt` (nothing after them is checked),
//! needs `info.{id, slug, title, version, time.{created, updated}}`, on a user message `{role, id, sessionID, time.created,
//! agent, model {providerID, modelID}}`, on an assistant message `{role, id, sessionID, parentID, time.created, mode, agent,
//! path, cost, tokens (with `reasoning` and `cache`), modelID, providerID}`, and on a part `{type, id, sessionID,
//! messageID}`. It takes the session's directory from its own working directory, not from the file; it keeps a session
//! that exists and adds the messages whose ids it has not got (a message id is unique across every session), and it lists
//! messages by their time. Tool parts in `completed`, `error` and `running` states, `file` parts and an assistant message
//! with `summary` were accepted and listed as written. The `compaction` part is written as OpenCode's published types
//! describe it. Not measured: whether the shape of each part is what OpenCode itself writes for a model it did not run.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use coqui_acp_history::conversation::{
    Boundary, Call, Conversation, Item, Stamp, call_note, clean_id, shell_text, valid_time,
};
use coqui_acp_history::{
    ATTACHMENT_BY_DIGEST, ATTACHMENT_META_KEY, Document, LossReport, SUMMARY_MISSING, SeedPoint,
    Stop, StopReason, THINKING, TOOL_AS_NOTE, Tool,
};
use serde_json::{Map, Value, json};

use super::opencode_history::{HARNESS, native_id_of};
use crate::event::context_block;

/// A loss the tool table has no name for: content the target cannot take at all.
pub const CONTENT_DROPPED: &str = "content_dropped";

/// What the tool part of a call that never finished says, as OpenCode words an aborted call.
const ABORTED_CALL: &str = "Tool execution aborted";

/// The longest title a session gets from its first prompt, in characters.
const TITLE_CHARS: usize = 80;

/// A session ready for `opencode import`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Imported {
    /// The file's text.
    pub text: String,
    /// The id the session has in OpenCode's store, which `opencode import` prints.
    pub native_id: String,
    /// What could not be carried over.
    pub report: LossReport,
    /// Where in the history the seed starts.
    pub seeded_from: Option<SeedPoint>,
}

/// The file `opencode import` reads for `doc`, as session `session` (the host's UUID) working in `cwd`.
///
/// The conversation is the seed ([`Conversation::of`]): a compaction summary is written as OpenCode's own compaction
/// (a user message holding a `compaction` part and the assistant's summary), tool calls as OpenCode's own tools when the
/// history's origin is OpenCode or the tool table names an equivalent (and as a plain note otherwise), and reasoning is
/// dropped, since OpenCode takes none from a model it did not run.
///
/// # Errors
///
/// `session` is not a UUID.
pub fn import(doc: &Document, session: &str, cwd: &str) -> Result<Imported, String> {
    let native_id = native_id_of(session).ok_or("the session id is not a UUID")?;
    let conversation = Conversation::of(doc, true);
    let mut writer = Writer::new(doc, &native_id, cwd);
    writer.write(&conversation);
    let mut report = writer.report;
    report.add(
        SUMMARY_MISSING,
        u64::from(conversation.summary_missing),
        Some("the latest compaction has no summary, so the seed starts earlier"),
    );
    let info = writer.info;
    let file = json!({"info": info, "messages": writer.messages});
    Ok(Imported {
        text: file.to_string(),
        native_id,
        report,
        seeded_from: conversation.seeded_from.clone(),
    })
}

struct Writer<'a> {
    doc: &'a Document,
    session: String,
    /// The session's id without `ses_`, which the ids of its messages and parts are made from.
    stem: String,
    cwd: String,
    info: Value,
    messages: Vec<Value>,
    report: LossReport,
    count: u64,
    /// The time of the latest message, in milliseconds since 1970: no message is earlier than it.
    time: i64,
    provider: String,
    model: String,
    /// The user message the turn answers: the parent of its assistant messages.
    user: Option<String>,
    /// The assistant message that still takes parts.
    draft: Option<usize>,
    /// A tool result has come since the draft began, so the next text is a new step.
    stepped: bool,
    /// The latest assistant message of the turn.
    last_assistant: Option<usize>,
    cancelled_turn: bool,
    ids: BTreeMap<String, String>,
}

impl<'a> Writer<'a> {
    fn new(doc: &'a Document, native_id: &str, cwd: &str) -> Self {
        let created = doc
            .header
            .created_at
            .as_deref()
            .and_then(millis)
            .or_else(|| {
                doc.lines
                    .iter()
                    .find_map(|line| line.at.as_deref().and_then(millis))
            })
            .unwrap_or(0);
        let origin = doc.header.origin.harness.clone();
        let (provider, model) = match doc.header.model.as_deref() {
            Some(model) if origin == HARNESS => model
                .split_once('/')
                .map_or((origin.clone(), model.to_owned()), |(provider, model)| {
                    (provider.to_owned(), model.to_owned())
                }),
            Some(model) => (origin.clone(), model.to_owned()),
            None => (origin.clone(), "unknown".to_owned()),
        };
        let stem = native_id.strip_prefix("ses_").unwrap_or(native_id);
        Self {
            doc,
            session: native_id.to_owned(),
            stem: stem.to_owned(),
            cwd: cwd.to_owned(),
            info: Value::Null,
            messages: Vec::new(),
            report: LossReport::new(),
            count: 0,
            time: created,
            provider,
            model,
            user: None,
            draft: None,
            stepped: false,
            last_assistant: None,
            cancelled_turn: false,
            ids: BTreeMap::new(),
        }
    }

    fn is_opencode(&self) -> bool {
        self.doc.header.origin.harness == HARNESS
    }

    fn write(&mut self, conversation: &Conversation) {
        let created = self.time;
        if let Some(boundary) = &conversation.summary {
            self.boundary(boundary);
        }
        for item in &conversation.items {
            self.item(conversation, item);
        }
        self.settle(None);
        let title = self.title();
        self.info = json!({
            "id": self.session,
            "slug": format!("imported-{}", &self.stem[..self.stem.len().min(8)]),
            "projectID": "global",
            "directory": self.cwd,
            "title": title,
            "agent": "build",
            "model": {"id": self.model, "providerID": self.provider},
            "version": self.doc.header.origin.version.clone().unwrap_or_else(|| "imported".to_owned()),
            "time": {"created": created, "updated": self.time},
        });
    }

    /// The first line of the first prompt, cut: what the person would call the session.
    fn title(&self) -> String {
        let first = self.messages.iter().find_map(|message| {
            (message["info"]["role"] == "user")
                .then(|| {
                    message["parts"]
                        .as_array()?
                        .iter()
                        .find_map(|part| part["text"].as_str())
                })
                .flatten()
        });
        let line = first
            .and_then(|text| text.lines().find(|line| !line.trim().is_empty()))
            .map(str::trim)
            .unwrap_or_default();
        if line.is_empty() {
            "Imported session".to_owned()
        } else {
            line.chars().take(TITLE_CHARS).collect()
        }
    }

    /// The time of a message made at `stamp`: never earlier than the one before, and one millisecond later than it
    /// when the stamp says no more, so that listing by time keeps the order.
    fn tick(&mut self, stamp: &Stamp) -> i64 {
        let at = stamp
            .at
            .as_deref()
            .filter(|at| valid_time(at))
            .and_then(millis);
        self.time = at.map_or(self.time + 1, |at| at.max(self.time + 1));
        self.time
    }

    fn message_id(&mut self) -> String {
        self.count += 1;
        format!("msg_{}{:08x}", self.stem, self.count)
    }

    fn part_id(&mut self) -> String {
        self.count += 1;
        format!("prt_{}{:08x}", self.stem, self.count)
    }

    fn part(&mut self, message: &str, kind: &str, fields: Value) -> Value {
        let mut part = Map::new();
        part.insert("type".into(), json!(kind));
        if let Value::Object(fields) = fields {
            part.extend(fields);
        }
        part.insert("id".into(), json!(self.part_id()));
        part.insert("sessionID".into(), json!(self.session));
        part.insert("messageID".into(), json!(message));
        Value::Object(part)
    }

    fn user_message(&mut self, stamp: &Stamp, parts: Vec<(&str, Value)>) -> String {
        self.settle(None);
        let at = self.tick(stamp);
        let id = self.message_id();
        let mut made = Vec::new();
        for (kind, fields) in parts {
            made.push(self.part(&id, kind, fields));
        }
        self.messages.push(json!({
            "info": {
                "role": "user", "id": id, "sessionID": self.session, "time": {"created": at},
                "agent": "build", "model": {"providerID": self.provider, "modelID": self.model},
            },
            "parts": made,
        }));
        self.user = Some(id.clone());
        self.cancelled_turn = false;
        id
    }

    /// A summary the seed starts from, as OpenCode's own compaction: the user message that asks for one and the
    /// assistant message that answers it.
    fn boundary(&mut self, boundary: &Boundary) {
        let parent = self.user_message(
            &boundary.stamp,
            vec![("compaction", json!({"auto": false}))],
        );
        self.draft = None;
        let at = self.tick(&boundary.stamp);
        let id = self.message_id();
        let text = self.part(
            &id,
            "text",
            json!({"text": boundary.summary, "time": {"start": at, "end": at}}),
        );
        self.messages.push(json!({
            "info": self.assistant_info(&id, &parent, at, "stop", true),
            "parts": [text],
        }));
        self.last_assistant = None;
    }

    fn assistant_info(
        &self,
        id: &str,
        parent: &str,
        at: i64,
        finish: &str,
        summary: bool,
    ) -> Value {
        let mut info = json!({
            "role": "assistant", "id": id, "sessionID": self.session, "parentID": parent,
            "time": {"created": at, "completed": at},
            "mode": if summary { "compaction" } else { "build" },
            "agent": if summary { "compaction" } else { "build" },
            "path": {"cwd": self.cwd, "root": "/"}, "cost": 0,
            "tokens": {"total": 0, "input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
            "modelID": self.model, "providerID": self.provider, "finish": finish,
        });
        if summary {
            info["summary"] = json!(true);
        }
        info
    }

    /// The assistant message that takes the next part: the draft, or a new step.
    fn assistant(&mut self, stamp: &Stamp) -> usize {
        if let Some(at) = self.draft.filter(|_| !self.stepped) {
            return at;
        }
        self.settle(None);
        let parent = match self.user.clone() {
            Some(parent) => parent,
            None => self.user_message(
                stamp,
                vec![("text", json!({"text": "(the conversation so far)"}))],
            ),
        };
        let at = self.tick(stamp);
        let id = self.message_id();
        let start = self.part(&id, "step-start", json!({}));
        let mut info = self.assistant_info(&id, &parent, at, "stop", false);
        info["time"]["completed"] = Value::Null;
        self.messages.push(json!({"info": info, "parts": [start]}));
        let index = self.messages.len() - 1;
        self.draft = Some(index);
        self.last_assistant = Some(index);
        self.stepped = false;
        index
    }

    fn push_part(&mut self, index: usize, kind: &str, fields: Value) {
        let message = self.messages[index]["info"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let part = self.part(&message, kind, fields);
        if let Some(parts) = self.messages[index]["parts"].as_array_mut() {
            parts.push(part);
        }
    }

    /// Ends the draft: its `finish`, its time, and the step's end. A `stop` of the turn it belongs to, when given, is
    /// written to it.
    fn settle(&mut self, stop: Option<(Stop, Option<&str>)>) {
        let Some(index) = self.draft.take() else {
            return;
        };
        let has_call = self.messages[index]["parts"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|part| part["type"] == "tool"));
        let mut reason = if has_call { "tool-calls" } else { "stop" };
        if let Some((Stop::Acp(StopReason::MaxTokens), _)) = stop
            && !has_call
        {
            reason = "length";
        }
        let at = self.time;
        self.messages[index]["info"]["finish"] = json!(reason);
        self.messages[index]["info"]["time"]["completed"] = json!(at);
        self.push_part(
            index,
            "step-finish",
            json!({"reason": reason, "cost": 0,
                   "tokens": {"total": 0, "input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}}}),
        );
    }

    fn item(&mut self, conversation: &Conversation, item: &Item) {
        match item {
            Item::Prompt { blocks, stamp } => {
                self.last_assistant = None;
                let parts = self.content(blocks);
                if !parts.is_empty() {
                    self.user_message(stamp, parts);
                }
            }
            Item::Note { block, stamp } => {
                let parts = self.content(core::slice::from_ref(block));
                if !parts.is_empty() {
                    let kept = self.user.clone();
                    self.user_message(stamp, parts);
                    // A note is not a turn: the answer that follows is still the one the prompt asked for.
                    if kept.is_some() {
                        self.user = kept;
                    }
                }
            }
            Item::Message { text, stamp } => {
                let at = self.assistant(stamp);
                let now = self.time;
                self.push_part(at, "text", json!({"text": text, "time": {"start": now, "end": now}}));
            }
            Item::Thought { .. } => self.report.add(
                THINKING,
                1,
                Some("OpenCode takes reasoning only from the model that wrote it, with its signature"),
            ),
            Item::Call { id } => {
                if let Some(call) = conversation.call(id) {
                    self.call(call);
                }
            }
            Item::Result { .. } => self.stepped = true,
            Item::Cancel { .. } => self.cancelled_turn = true,
            Item::TurnEnd { stop, error, stamp, .. } => self.end_turn(*stop, error.as_deref(), stamp),
            _ => {}
        }
    }

    fn end_turn(&mut self, stop: Stop, error: Option<&str>, stamp: &Stamp) {
        let cancelled = self.cancelled_turn || matches!(stop, Stop::Acp(StopReason::Cancelled));
        let failed = matches!(stop, Stop::Error);
        if !cancelled && !failed {
            self.settle(Some((stop, None)));
            self.last_assistant = None;
            return;
        }
        let index = match self.last_assistant {
            Some(index) => index,
            None => {
                // The turn made no message: an empty one carries how it ended, as OpenCode writes a failed request.
                self.draft = None;
                self.stepped = false;
                let index = self.assistant(stamp);
                self.last_assistant = Some(index);
                index
            }
        };
        let info = &mut self.messages[index]["info"];
        info["error"] = if cancelled {
            json!({"name": "MessageAbortedError", "data": {"message": "The operation was aborted."}})
        } else {
            json!({"name": "UnknownError",
                   "data": {"message": error.unwrap_or("the turn failed")}})
        };
        // A message a note already closed keeps its end; the error is all that is added to it.
        if self.messages[index]["info"]["time"]["completed"].is_null() {
            self.draft = Some(index);
            self.settle(Some((stop, error)));
        }
        self.last_assistant = None;
    }

    fn call_id(&mut self, id: &str) -> String {
        if let Some(known) = self.ids.get(id) {
            return known.clone();
        }
        let clean = clean_id(id, &self.ids);
        self.ids.insert(id.to_owned(), clean.clone());
        clean
    }

    /// How a call is written: OpenCode's own tool when it has one.
    fn native_call(&self, call: &Call) -> Option<(String, Value)> {
        let name = call.name.as_deref()?;
        let input = call.raw_input.as_ref()?;
        if self.is_opencode() {
            return input.is_object().then(|| (name.to_owned(), input.clone()));
        }
        let (tool, normal) =
            coqui_acp_history::tools::normalise(&self.doc.header.origin.harness, name, input)?;
        let (tool, normal) = if tool == Tool::Patch {
            patch_as_write_or_edit(&normal)?
        } else {
            (tool, normal)
        };
        native_input(tool, &normal)
    }

    fn call(&mut self, call: &Call) {
        let stamp = Stamp::new(call.call_at.clone(), call.call_meta.clone());
        let Some((name, input)) = self.native_call(call) else {
            self.note(call, &stamp);
            return;
        };
        let at = self.assistant(&stamp);
        let started = self.time;
        let ended = call
            .result_at
            .as_deref()
            .and_then(millis)
            .map_or(started, |ended| ended.max(started));
        self.time = ended;
        let output = output_of(call);
        let title = call
            .title
            .clone()
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| name.clone());
        let state = match call.status.as_deref() {
            Some("completed") => json!({"status": "completed", "input": input, "output": output,
                "title": title, "metadata": {}, "time": {"start": started, "end": ended}}),
            Some("failed") => json!({"status": "error", "input": input,
                "error": if output.is_empty() { "the tool call failed".to_owned() } else { output },
                "time": {"start": started, "end": ended}}),
            // A call the history left open, or cancelled, is one OpenCode ended as aborted.
            _ => json!({"status": "error", "input": input, "error": ABORTED_CALL,
                "time": {"start": started, "end": ended}}),
        };
        let id = self.call_id(&call.id);
        self.push_part(
            at,
            "tool",
            json!({"tool": name, "callID": id, "state": state}),
        );
    }

    /// A call OpenCode has no tool for, as the text its model reads.
    fn note(&mut self, call: &Call, stamp: &Stamp) {
        self.report.add(
            TOOL_AS_NOTE,
            1,
            Some("OpenCode has no equivalent tool, so the call is written as text"),
        );
        let at = self.assistant(stamp);
        let now = self.time;
        self.push_part(
            at,
            "text",
            json!({"text": call_note(call), "time": {"start": now, "end": now}}),
        );
    }

    /// OpenCode's parts of ACP's content blocks, each with the kind of part it is.
    fn content(&mut self, blocks: &[Value]) -> Vec<(&'static str, Value)> {
        let mut out = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        out.push(("text", json!({"text": text})));
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
                        out.push(("text", json!({"text": format!("[an attachment of {mime} was left out of the history]")})));
                    } else {
                        out.push((
                            "file",
                            json!({"mime": mime, "url": format!("data:{mime};base64,{data}")}),
                        ));
                    }
                }
                Some("resource_link") => {
                    let uri = block.get("uri").and_then(Value::as_str).unwrap_or_default();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(uri);
                    out.push(("text", json!({"text": format!("[@{name}]({uri})")})));
                }
                Some("resource") => {
                    let uri = block
                        .pointer("/resource/uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if let Some(text) = block.pointer("/resource/text").and_then(Value::as_str) {
                        out.push(("text", json!({"text": context_block(uri, text)})));
                    } else {
                        self.report.add(
                            CONTENT_DROPPED,
                            1,
                            Some("a resource of bytes, which OpenCode reads only as an image"),
                        );
                    }
                }
                _ => self.report.add(
                    CONTENT_DROPPED,
                    1,
                    Some("a content block of a kind OpenCode does not take"),
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

/// What a call printed, as text.
fn output_of(call: &Call) -> String {
    let text = call.output_text();
    match (&call.raw_output, text.is_empty()) {
        (Some(Value::String(text)), _) => text.clone(),
        (Some(other), true) if !other.is_null() => other.to_string(),
        _ => text,
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

/// OpenCode's tool and input for a normalised `tool`: the names and keys of its built-in tools (`read`, `write`, `edit`,
/// `bash`, `grep`, `glob`, `webfetch`), with the keys renamed as its tool schemas spell them.
fn native_input(tool: Tool, normal: &Value) -> Option<(String, Value)> {
    let Value::Object(map) = normal else {
        return None;
    };
    let (name, keys): (&str, &[(&str, &str)]) = match tool {
        Tool::Read => ("read", &[("path", "filePath")]),
        Tool::Write => ("write", &[("path", "filePath")]),
        Tool::Edit => (
            "edit",
            &[
                ("path", "filePath"),
                ("old", "oldString"),
                ("new", "newString"),
                ("replace_all", "replaceAll"),
            ],
        ),
        Tool::Shell => ("bash", &[("timeout_ms", "timeout"), ("cwd", "workdir")]),
        Tool::Grep => ("grep", &[]),
        Tool::Glob => ("glob", &[]),
        Tool::Fetch => ("webfetch", &[]),
        _ => return None,
    };
    let mut out = Map::new();
    for (key, value) in map {
        let renamed = keys
            .iter()
            .find(|(normal, _)| key == normal)
            .map_or(key.as_str(), |(_, native)| native);
        if out.insert(renamed.to_owned(), value.clone()).is_some() {
            return None;
        }
    }
    if tool == Tool::Shell
        && let Some(argv) = out.get("command").and_then(Value::as_array)
    {
        let words: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
        let line = shell_text(&words);
        out.insert("command".into(), json!(line));
    }
    Some((name.to_owned(), Value::Object(out)))
}

/// The time `at` (RFC 3339) in milliseconds since 1970, when it is one.
fn millis(at: &str) -> Option<i64> {
    let digits = |from: usize, length: usize| at.get(from..from + length)?.parse::<i64>().ok();
    let (year, month, day) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    let (hour, minute, second) = (digits(11, 2)?, digits(14, 2)?, digits(17, 2)?);
    let rest = at.get(19..)?;
    let (fraction, rest) = match rest.strip_prefix('.') {
        Some(after) => {
            let end = after
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len());
            let mut digits: String = after[..end].chars().take(3).collect();
            while digits.len() < 3 {
                digits.push('0');
            }
            (digits.parse::<i64>().ok()?, &after[end..])
        }
        None => (0, rest),
    };
    let offset = match rest.as_bytes().first()? {
        b'Z' | b'z' => 0,
        sign @ (b'+' | b'-') => {
            let hours = rest.get(1..3)?.parse::<i64>().ok()?;
            let minutes = rest.get(4..6)?.parse::<i64>().ok()?;
            let seconds = hours * 3600 + minutes * 60;
            if *sign == b'+' { seconds } else { -seconds }
        }
        _ => return None,
    };
    // Days from the civil date, by the proleptic Gregorian calendar.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted = (month + 9) % 12;
    let day_of_year = (153 * shifted + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    Some(seconds * 1000 + fraction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeSet;
    use coqui_acp_history::conversation::Recorder;
    use coqui_acp_history::{Header, Origin};

    const UUID: &str = "7d8c1f4e-2b9a-4c63-8f10-5a6b7c8d9e0f";

    fn at(second: u32) -> Stamp {
        Stamp::at(Some(format!("2026-11-18T12:00:{second:02}.000Z")))
    }

    fn call(
        id: &str,
        name: &str,
        input: &Value,
        status: &str,
        output: Option<&str>,
    ) -> (Value, Value) {
        let mut call = Call::new(id);
        call.name = Some(name.to_owned());
        call.title = Some(name.to_owned());
        call.status = Some("pending".to_owned());
        call.raw_input = Some(input.clone());
        let announced = call.announce();
        call.status = Some(status.to_owned());
        call.raw_output = output.map(|text| json!(text));
        (announced, call.finish())
    }

    fn history(harness: &str, build: impl FnOnce(&mut Recorder)) -> Document {
        let mut recorder = Recorder::new();
        build(&mut recorder);
        let mut header = Header::new(UUID, Origin::new(harness));
        header.created_at = Some("2026-11-18T12:00:00.000Z".to_owned());
        header.model = Some("claude-sonnet-4-5".to_owned());
        let mut document = Document::new(header);
        document.lines = recorder.finish();
        document.seal().expect("a history");
        document
    }

    fn prompt(rec: &mut Recorder, text: &str, second: u32) {
        rec.prompt(vec![json!({"type": "text", "text": text})], at(second));
    }

    fn two_turns(rec: &mut Recorder) {
        prompt(rec, "list the files", 1);
        rec.thought("looking", at(2));
        let (announced, done) = call(
            "toolu_1",
            "Bash",
            &json!({"command": "ls", "description": "list"}),
            "completed",
            Some("a.txt b.txt"),
        );
        rec.update(announced, at(3));
        rec.update(done, at(4));
        rec.message("two files", at(5));
        rec.end_turn(Stop::Acp(StopReason::EndTurn), None, at(6));
        prompt(rec, "thanks", 7);
        rec.message("welcome", at(8));
        rec.end_turn(Stop::Acp(StopReason::EndTurn), None, at(9));
    }

    fn written(document: &Document) -> (Imported, Value) {
        let imported = import(document, UUID, "/work/project").expect("written");
        let file = serde_json::from_str(&imported.text).expect("JSON");
        (imported, file)
    }

    fn shape(file: &Value) -> Vec<String> {
        file["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| {
                let parts: Vec<&str> = message["parts"]
                    .as_array()
                    .expect("parts")
                    .iter()
                    .filter_map(|part| part["type"].as_str())
                    .collect();
                format!(
                    "{}:{}",
                    message["info"]["role"].as_str().unwrap_or("?"),
                    parts.join("+")
                )
            })
            .collect()
    }

    #[test]
    fn a_history_is_the_file_opencode_import_reads_with_each_step_a_message() {
        let (imported, file) = written(&history("claude", two_turns));
        assert_eq!(imported.native_id, "ses_7d8c1f4e2b9a4c638f105a6b7c8d9e0f");
        let info = &file["info"];
        assert_eq!(info["id"], imported.native_id);
        assert_eq!(info["title"], "list the files");
        assert_eq!(info["directory"], "/work/project");
        for key in ["slug", "version", "time"] {
            assert!(!info[key].is_null(), "{key}");
        }
        assert_eq!(info["time"]["created"], 1_795_003_200_000_i64);
        assert_eq!(
            shape(&file),
            [
                "user:text",
                "assistant:step-start+tool+step-finish",
                "assistant:step-start+text+step-finish",
                "user:text",
                "assistant:step-start+text+step-finish",
            ]
        );
        let messages = file["messages"].as_array().expect("messages");
        // The call is OpenCode's own tool, with its result in the same part.
        let tool = &messages[1]["parts"][1];
        assert_eq!(tool["tool"], "bash");
        assert_eq!(tool["state"]["status"], "completed");
        assert_eq!(tool["state"]["input"]["command"], "ls");
        assert_eq!(tool["state"]["output"], "a.txt b.txt");
        assert_eq!(messages[1]["info"]["finish"], "tool-calls");
        assert_eq!(messages[2]["info"]["finish"], "stop");
        // Each answer is the child of the prompt it answers.
        assert_eq!(messages[1]["info"]["parentID"], messages[0]["info"]["id"]);
        assert_eq!(messages[2]["info"]["parentID"], messages[0]["info"]["id"]);
        assert_eq!(messages[4]["info"]["parentID"], messages[3]["info"]["id"]);
        // Ids carry the prefixes the program checks and are unique, and listing by time keeps the order.
        let mut ids = BTreeSet::new();
        let mut last = 0;
        for message in messages {
            let info = &message["info"];
            let id = info["id"].as_str().expect("an id");
            assert!(id.starts_with("msg_"));
            assert!(ids.insert(id.to_owned()));
            assert_eq!(info["sessionID"], imported.native_id);
            let created = info["time"]["created"].as_i64().expect("a time");
            assert!(created > last, "{created} after {last}");
            last = created;
            for part in message["parts"].as_array().expect("parts") {
                let id = part["id"].as_str().expect("an id");
                assert!(id.starts_with("prt_"));
                assert!(ids.insert(id.to_owned()));
                assert_eq!(part["messageID"], info["id"]);
            }
        }
        // Reasoning is not carried: OpenCode takes none from a model it did not run.
        assert_eq!(imported.report.count(THINKING), 1);
        assert!(!imported.text.contains("looking"));
    }

    #[test]
    fn calls_that_failed_never_finished_or_have_no_tool_of_opencodes_are_told_as_such() {
        let document = history("claude", |rec| {
            prompt(rec, "go", 1);
            let (announced, done) = call(
                "c1",
                "Read",
                &json!({"file_path": "/x"}),
                "failed",
                Some("no such file"),
            );
            rec.update(announced, at(2));
            rec.update(done, at(3));
            let (announced, _) = call(
                "c2",
                "Bash",
                &json!({"command": "sleep 9"}),
                "pending",
                None,
            );
            rec.update(announced, at(4));
            let (announced, done) = call(
                "c3",
                "mcp__notes__add",
                &json!({"text": "hi"}),
                "completed",
                Some("added"),
            );
            rec.update(announced, at(5));
            rec.update(done, at(6));
            rec.end_turn(Stop::Acp(StopReason::EndTurn), None, at(7));
        });
        let (imported, file) = written(&document);
        let parts: Vec<&Value> = file["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .flat_map(|message| message["parts"].as_array().expect("parts"))
            .collect();
        let read = parts
            .iter()
            .find(|part| part["callID"] == "c1")
            .expect("the read");
        assert_eq!(read["tool"], "read");
        assert_eq!(read["state"]["status"], "error");
        assert_eq!(read["state"]["error"], "no such file");
        assert_eq!(read["state"]["input"]["filePath"], "/x");
        let open = parts
            .iter()
            .find(|part| part["callID"] == "c2")
            .expect("the open call");
        assert_eq!(open["state"]["status"], "error");
        assert_eq!(open["state"]["error"], ABORTED_CALL);
        let note = parts
            .iter()
            .find(|part| {
                part["type"] == "text"
                    && part["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("mcp__notes__add"))
            })
            .expect("a note");
        assert!(note["text"].as_str().expect("text").contains("added"));
        assert_eq!(imported.report.count(TOOL_AS_NOTE), 1);
    }

    #[test]
    fn a_cancelled_or_failed_turn_ends_its_last_message_as_opencode_writes_one() {
        let document = history("claude", |rec| {
            prompt(rec, "one", 1);
            rec.message("part of an", at(2));
            rec.cancel(at(3));
            rec.end_turn(Stop::Acp(StopReason::Cancelled), None, at(4));
            prompt(rec, "two", 5);
            rec.end_turn(Stop::Error, Some("overloaded".to_owned()), at(6));
        });
        let (_, file) = written(&document);
        let messages = file["messages"].as_array().expect("messages");
        assert_eq!(messages[1]["info"]["error"]["name"], "MessageAbortedError");
        assert_eq!(messages[1]["parts"][1]["text"], "part of an");
        assert_eq!(messages[3]["info"]["error"]["name"], "UnknownError");
        assert_eq!(
            messages[3]["info"]["error"]["data"]["message"],
            "overloaded"
        );
        assert_eq!(messages[3]["info"]["parentID"], messages[2]["info"]["id"]);
    }

    #[test]
    fn a_compaction_summary_is_written_as_opencodes_own_and_what_it_replaced_is_not() {
        let document = history("claude", |rec| {
            prompt(rec, "old question", 1);
            rec.message("old answer", at(2));
            rec.end_turn(Stop::Acp(StopReason::EndTurn), None, at(3));
            rec.compaction(
                Some("we listed the files".to_owned()),
                None,
                Some(900),
                at(4),
            );
            prompt(rec, "next question", 5);
            rec.message("next answer", at(6));
            rec.end_turn(Stop::Acp(StopReason::EndTurn), None, at(7));
        });
        let (imported, file) = written(&document);
        assert_eq!(
            shape(&file),
            [
                "user:compaction",
                "assistant:text",
                "user:text",
                "assistant:step-start+text+step-finish"
            ]
        );
        let messages = file["messages"].as_array().expect("messages");
        assert_eq!(messages[1]["info"]["summary"], true);
        assert_eq!(messages[1]["info"]["parentID"], messages[0]["info"]["id"]);
        assert_eq!(messages[1]["parts"][0]["text"], "we listed the files");
        assert!(!imported.text.contains("old answer"));
        assert!(imported.seeded_from.is_some());
    }

    #[test]
    fn a_history_of_opencodes_own_keeps_its_tools_and_a_claude_call_gets_opencodes_keys() {
        let document = history("opencode", |rec| {
            prompt(rec, "edit", 1);
            let (announced, done) = call(
                "call_1",
                "edit",
                &json!({"filePath": "/a", "oldString": "o", "newString": "n"}),
                "completed",
                Some("done"),
            );
            rec.update(announced, at(2));
            rec.update(done, at(3));
            rec.end_turn(Stop::Acp(StopReason::EndTurn), None, at(4));
        });
        let (_, file) = written(&document);
        let tool = &file["messages"][1]["parts"][1];
        assert_eq!(tool["tool"], "edit");
        assert_eq!(tool["state"]["input"]["oldString"], "o");
        let document = history("claude", |rec| {
            prompt(rec, "edit", 1);
            let (announced, done) = call(
                "c",
                "Edit",
                &json!({"file_path": "/a", "old_string": "o", "new_string": "n", "replace_all": true}),
                "completed",
                Some("done"),
            );
            rec.update(announced, at(2));
            rec.update(done, at(3));
            rec.end_turn(Stop::Acp(StopReason::EndTurn), None, at(4));
        });
        let (_, file) = written(&document);
        assert_eq!(
            file["messages"][1]["parts"][1]["state"]["input"],
            json!({"filePath": "/a", "oldString": "o", "newString": "n", "replaceAll": true})
        );
    }

    #[test]
    fn what_is_no_uuid_is_refused_and_an_empty_history_is_a_session_with_no_messages() {
        let document = history("claude", |_| {});
        assert!(import(&document, "ses_abc", "/w").is_err());
        let (_, file) = written(&document);
        assert_eq!(file["messages"], json!([]));
        assert_eq!(file["info"]["title"], "Imported session");
    }

    #[test]
    fn time_in_milliseconds_is_read_from_rfc_3339() {
        assert_eq!(millis("2026-11-18T12:00:00.000Z"), Some(1_795_003_200_000));
        assert_eq!(millis("2026-11-18T12:00:00+01:00"), Some(1_794_999_600_000));
        assert_eq!(millis("later"), None);
    }
}
