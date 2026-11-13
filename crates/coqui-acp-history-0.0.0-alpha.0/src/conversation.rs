//! A history as a conversation: what a codec writing a harness's own files needs, without the
//! envelope, and what a codec reading them builds.
//!
//! [`Conversation::of`] reads a [`Document`] into ordered [`Item`]s with each tool call's updates
//! merged into one [`Call`], seeded from the latest compaction summary the caller wants, and closes
//! what the history left open (importing hostile histories, rule (a)): a tool call with no result
//! gets a cancelled one and a turn left open ends cancelled. Permission requests and answers are
//! not carried over (rule (b)), and nothing here opens a path or a link the history names (rule
//! (d)).
//!
//! [`Recorder`] is the other direction: a codec that reads a harness's files pushes prompts,
//! updates and turn ends into it and gets lines that satisfy the order rules.

use alloc::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    Body, Document, HistoryError, Line, NATIVE_META_KEY, SeedPoint, Stop, StopReason, Trigger,
};

/// When a line happened and what else it carried.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Stamp {
    /// The line's `at`.
    pub at: Option<String>,
    /// The line's `_meta`.
    pub meta: Option<Value>,
}

impl Stamp {
    /// A stamp with a time and no `_meta`.
    #[must_use]
    pub fn at(at: Option<String>) -> Self {
        Self { at, meta: None }
    }

    /// A stamp with a time and a `_meta`.
    #[must_use]
    pub fn new(at: Option<String>, meta: Option<Value>) -> Self {
        Self { at, meta }
    }

    /// The same stamp carrying `fields` as [`NATIVE_META_KEY`] for `harness`, when there are any.
    #[must_use]
    pub fn native(mut self, harness: &str, fields: Map<String, Value>) -> Self {
        self.meta = native_meta(harness, fields);
        self
    }

    /// What `harness` kept of its own in this line: the object under [`NATIVE_META_KEY`] when it
    /// names `harness`.
    #[must_use]
    pub fn native_of(&self, harness: &str) -> Option<&Map<String, Value>> {
        native_of(self.meta.as_ref(), harness)
    }
}

/// A `_meta` holding `fields` under [`NATIVE_META_KEY`], tagged with `harness`; `None` for no fields.
#[must_use]
pub fn native_meta(harness: &str, mut fields: Map<String, Value>) -> Option<Value> {
    if fields.is_empty() {
        return None;
    }
    fields.insert("harness".to_owned(), Value::String(harness.to_owned()));
    let mut meta = Map::new();
    meta.insert(NATIVE_META_KEY.to_owned(), Value::Object(fields));
    Some(Value::Object(meta))
}

/// The object under [`NATIVE_META_KEY`] of `meta`, when it is tagged with `harness`.
#[must_use]
pub fn native_of<'a>(meta: Option<&'a Value>, harness: &str) -> Option<&'a Map<String, Value>> {
    let fields = meta?.get(NATIVE_META_KEY)?.as_object()?;
    (fields.get("harness")?.as_str()? == harness).then_some(fields)
}

/// One tool call: what the harness announced and every update that followed, merged.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Call {
    /// ACP's `toolCallId`.
    pub id: String,
    /// The harness's name for the tool (ACP version 2's `name`), the key into the tool table.
    pub name: Option<String>,
    /// A title for people.
    pub title: Option<String>,
    /// ACP's `kind`.
    pub kind: Option<String>,
    /// ACP's `status`, the latest one told: `pending`, `in_progress`, `completed`, `failed` or
    /// `cancelled`.
    pub status: Option<String>,
    /// `rawInput`.
    pub raw_input: Option<Value>,
    /// `rawOutput`.
    pub raw_output: Option<Value>,
    /// ACP's `content` items, the latest told.
    pub content: Vec<Value>,
    /// ACP's `locations`, the latest told.
    pub locations: Vec<Value>,
    /// The `_meta` of the line that announced the call.
    pub call_meta: Option<Value>,
    /// The `_meta` of the line that told its end.
    pub result_meta: Option<Value>,
    /// The time of the line that announced the call.
    pub call_at: Option<String>,
    /// The time of the line that told its end.
    pub result_at: Option<String>,
    /// Whether a `tool_call` announced it; false for an update of a call the seed does not hold.
    pub announced: bool,
}

impl Call {
    /// A call named `id` with nothing else known.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }

    /// Whether the call finished, one way or the other.
    #[must_use]
    pub fn is_done(&self) -> bool {
        matches!(
            self.status.as_deref(),
            Some("completed" | "failed" | "cancelled")
        )
    }

    /// Whether the call failed or was cancelled.
    #[must_use]
    pub fn is_failed(&self) -> bool {
        matches!(self.status.as_deref(), Some("failed" | "cancelled"))
    }

    /// The text of the call's output: `rawOutput` when it is text or holds text blocks, else the
    /// text of its `content` items.
    #[must_use]
    pub fn output_text(&self) -> String {
        if let Some(text) = self.raw_output.as_ref().and_then(text_of) {
            return text;
        }
        let parts: Vec<String> = self
            .content
            .iter()
            .filter_map(|item| match item.get("type").and_then(Value::as_str) {
                Some("content") => item
                    .pointer("/content/text")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                _ => None,
            })
            .collect();
        parts.join("\n")
    }

    /// The `tool_call` update announcing the call.
    #[must_use]
    pub fn announce(&self) -> Value {
        let mut update = Map::new();
        update.insert("sessionUpdate".into(), json!("tool_call"));
        update.insert("toolCallId".into(), json!(self.id));
        if let Some(name) = &self.name {
            update.insert("name".into(), json!(name));
        }
        update.insert(
            "title".into(),
            json!(self.title.as_deref().unwrap_or("Tool")),
        );
        if let Some(kind) = &self.kind {
            update.insert("kind".into(), json!(kind));
        }
        update.insert(
            "status".into(),
            json!(self.status.as_deref().unwrap_or("pending")),
        );
        if let Some(input) = &self.raw_input {
            update.insert("rawInput".into(), input.clone());
        }
        if !self.locations.is_empty() {
            update.insert("locations".into(), Value::Array(self.locations.clone()));
        }
        if !self.content.is_empty() {
            update.insert("content".into(), Value::Array(self.content.clone()));
        }
        Value::Object(update)
    }

    /// The `tool_call_update` telling how the call ended.
    #[must_use]
    pub fn finish(&self) -> Value {
        let mut update = Map::new();
        update.insert("sessionUpdate".into(), json!("tool_call_update"));
        update.insert("toolCallId".into(), json!(self.id));
        update.insert(
            "status".into(),
            json!(self.status.as_deref().unwrap_or("completed")),
        );
        if let Some(output) = &self.raw_output {
            update.insert("rawOutput".into(), output.clone());
        }
        if !self.content.is_empty() {
            update.insert("content".into(), Value::Array(self.content.clone()));
        }
        Value::Object(update)
    }
}

/// The text of a value that is a string or an array of blocks holding text.
fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let parts: Vec<&str> = blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        _ => None,
    }
}

/// One thing that happened, in order.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Item {
    /// The client's prompt, as ACP content blocks.
    Prompt {
        /// The blocks.
        blocks: Vec<Value>,
        /// Its time and `_meta`.
        stamp: Stamp,
    },
    /// A message of the person's outside a prompt (`user_message_chunk`): a block.
    Note {
        /// The content block.
        block: Value,
        /// Its time and `_meta`.
        stamp: Stamp,
    },
    /// The agent's text, its chunks joined.
    Message {
        /// The text.
        text: String,
        /// Its time and `_meta`.
        stamp: Stamp,
    },
    /// The agent's reasoning, its chunks joined.
    Thought {
        /// The text.
        text: String,
        /// Its time and `_meta`.
        stamp: Stamp,
    },
    /// A tool call was announced; see [`Conversation::call`].
    Call {
        /// The call's id.
        id: String,
    },
    /// A tool call ended, or was ended here because the history left it open.
    Result {
        /// The call's id.
        id: String,
    },
    /// The agent's plan, whole: ACP's entries.
    Plan {
        /// The entries.
        entries: Vec<Value>,
        /// Its time and `_meta`.
        stamp: Stamp,
    },
    /// The client cancelled the turn.
    Cancel {
        /// Its time and `_meta`.
        stamp: Stamp,
    },
    /// A turn ended.
    TurnEnd {
        /// How.
        stop: Stop,
        /// What failed, for a turn that failed.
        error: Option<String>,
        /// Whether this was made here, for a turn the history left open.
        closed_here: bool,
        /// Its time and `_meta`.
        stamp: Stamp,
    },
}

/// What the latest compaction boundary a seed starts from said.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Boundary {
    /// The harness's own summary.
    pub summary: String,
    /// What set it off.
    pub trigger: Option<Trigger>,
    /// The context size before it, when told.
    pub pre_tokens: Option<u64>,
    /// Its time and `_meta`.
    pub stamp: Stamp,
}

/// A history read for a writer.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Conversation {
    /// The summary the seed starts from, when the caller took it and the history has one.
    pub summary: Option<Boundary>,
    /// Where the seed starts, for the import's report; `None` for a history with no body.
    pub seeded_from: Option<SeedPoint>,
    /// Whether the history's latest compaction boundary has no summary, so the seed starts earlier.
    pub summary_missing: bool,
    /// What happened after the seed's start, in order.
    pub items: Vec<Item>,
    /// Every tool call of the seed by id.
    pub calls: BTreeMap<String, Call>,
    /// How many tool calls the history left open and this ended as cancelled.
    pub closed_calls: u64,
    /// Whether a turn was left open and this ended it as cancelled.
    pub closed_turn: bool,
}

impl Conversation {
    /// The conversation of `doc`, seeded from its latest compaction summary when `use_summary`, and
    /// otherwise from its first line, with no boundary's text anywhere.
    #[must_use]
    pub fn of(doc: &Document, use_summary: bool) -> Self {
        let seed = doc.seed();
        let latest = doc
            .lines
            .iter()
            .rposition(|line| matches!(line.body, Body::Compaction { .. }));
        let (lines, summary, seeded_from) = if use_summary && seed.summary.is_some() {
            let at = usize::try_from(seed.first_line.saturating_sub(3)).unwrap_or(0);
            let boundary = doc.lines.get(at).and_then(|line| match &line.body {
                Body::Compaction {
                    summary: Some(summary),
                    trigger,
                    pre_tokens,
                } => Some(Boundary {
                    summary: summary.clone(),
                    trigger: *trigger,
                    pre_tokens: *pre_tokens,
                    stamp: Stamp {
                        at: line.at.clone(),
                        meta: line.meta.clone(),
                    },
                }),
                _ => None,
            });
            (seed.lines, boundary, seed.point.clone())
        } else {
            let point = doc.first_point();
            (doc.lines.as_slice(), None, point)
        };
        let summary_missing = latest.is_some_and(|latest| {
            matches!(
                &doc.lines[latest].body,
                Body::Compaction { summary: None, .. }
            )
        });
        let mut builder = Builder {
            v2: doc.header.acp_version.is_some_and(|version| version >= 2),
            ..Builder::default()
        };
        for line in lines {
            builder.line(line);
        }
        let mut conversation = builder.finish();
        conversation.summary = summary;
        conversation.seeded_from = seeded_from;
        conversation.summary_missing = summary_missing;
        conversation
    }

    /// The merged call `id`.
    #[must_use]
    pub fn call(&self, id: &str) -> Option<&Call> {
        self.calls.get(id)
    }
}

#[derive(Default)]
struct Builder {
    /// Whether the updates are ACP version 2's: a `tool_call_update` for a call not yet seen then
    /// announces it, as version 2 has no `tool_call`.
    v2: bool,
    /// The item each message id names, so a version 2 upsert replaces it.
    ids: BTreeMap<String, usize>,
    /// The id of the message or thought the last chunk belonged to.
    last_id: Option<String>,
    items: Vec<Item>,
    calls: BTreeMap<String, Call>,
    pending: Vec<String>,
    resulted: BTreeSet<String>,
    open: bool,
    closed_calls: u64,
}

impl Builder {
    fn line(&mut self, line: &Line) {
        let stamp = Stamp {
            at: line.at.clone(),
            meta: line.meta.clone(),
        };
        match &line.body {
            Body::Prompt { prompt } => {
                self.open = true;
                self.items.push(Item::Prompt {
                    blocks: prompt.clone(),
                    stamp,
                });
            }
            Body::Update { update } => self.update(update, stamp),
            Body::Cancel => self.items.push(Item::Cancel { stamp }),
            Body::TurnEnd { stop, error } => {
                self.close_calls();
                self.open = false;
                self.items.push(Item::TurnEnd {
                    stop: *stop,
                    error: error.clone(),
                    closed_here: false,
                    stamp,
                });
            }
            // A request and its answer are history, not a grant; a boundary is read by `of`.
            Body::PermissionRequest { .. }
            | Body::PermissionAnswer { .. }
            | Body::Compaction { .. } => {}
        }
    }

    fn update(&mut self, update: &Value, stamp: Stamp) {
        let kind = update.get("sessionUpdate").and_then(Value::as_str);
        let message_id = update
            .get("messageId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match kind {
            Some(chunk @ ("agent_message_chunk" | "agent_thought_chunk")) => {
                let Some(text) = update.pointer("/content/text").and_then(Value::as_str) else {
                    return;
                };
                let same_message =
                    message_id.is_none() || self.last_id.is_none() || message_id == self.last_id;
                match (chunk, self.items.last_mut()) {
                    ("agent_message_chunk", Some(Item::Message { text: kept, .. }))
                    | ("agent_thought_chunk", Some(Item::Thought { text: kept, .. }))
                        if same_message =>
                    {
                        kept.push_str(text);
                    }
                    _ => {
                        let item = if chunk == "agent_message_chunk" {
                            Item::Message {
                                text: text.to_owned(),
                                stamp,
                            }
                        } else {
                            Item::Thought {
                                text: text.to_owned(),
                                stamp,
                            }
                        };
                        self.items.push(item);
                        if let Some(id) = &message_id {
                            self.ids.insert(id.clone(), self.items.len() - 1);
                        }
                    }
                }
                self.last_id = message_id;
            }
            Some(whole @ ("agent_message" | "agent_thought" | "user_message")) => {
                // Version 2: the content replaces what the message held; absent leaves it.
                let Some(blocks) = update.get("content").and_then(Value::as_array) else {
                    return;
                };
                let text = blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(
                        "
",
                    );
                let item = match whole {
                    "agent_message" => Item::Message { text, stamp },
                    "agent_thought" => Item::Thought { text, stamp },
                    _ => Item::Note {
                        block: json!({"type": "text", "text": text}),
                        stamp,
                    },
                };
                match message_id.as_ref().and_then(|id| self.ids.get(id)) {
                    Some(&at) => self.items[at] = item,
                    None => {
                        self.items.push(item);
                        if let Some(id) = &message_id {
                            self.ids.insert(id.clone(), self.items.len() - 1);
                        }
                    }
                }
                self.last_id = message_id;
            }
            Some("user_message_chunk") => {
                if let Some(block) = update.get("content").filter(|block| block.is_object()) {
                    self.items.push(Item::Note {
                        block: block.clone(),
                        stamp,
                    });
                }
            }
            Some("plan") => {
                if let Some(entries) = update.get("entries").and_then(Value::as_array) {
                    self.items.push(Item::Plan {
                        entries: entries.clone(),
                        stamp,
                    });
                }
            }
            Some("plan_update") => {
                if let Some(entries) = update.pointer("/plan/entries").and_then(Value::as_array) {
                    self.items.push(Item::Plan {
                        entries: entries.clone(),
                        stamp,
                    });
                }
            }
            Some(kind @ ("tool_call" | "tool_call_update")) => {
                let announces = kind == "tool_call" || self.v2;
                self.tool(announces, update, stamp);
            }
            _ => {}
        }
    }

    fn tool(&mut self, announces: bool, update: &Value, stamp: Stamp) {
        let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
            return;
        };
        let known = self.calls.contains_key(id);
        let call = self
            .calls
            .entry(id.to_owned())
            .or_insert_with(|| Call::new(id));
        let text = |key: &str| update.get(key).and_then(Value::as_str).map(str::to_owned);
        if announces && !call.announced {
            call.announced = true;
            call.call_meta = stamp.meta.clone();
            call.call_at = stamp.at.clone();
        }
        if let Some(name) = text("name") {
            call.name = Some(name);
        }
        if let Some(title) = text("title") {
            call.title = Some(title);
        }
        if let Some(kind) = text("kind") {
            call.kind = Some(kind);
        }
        let status = text("status");
        if let Some(input) = update.get("rawInput") {
            call.raw_input = Some(input.clone());
        }
        if let Some(output) = update.get("rawOutput") {
            call.raw_output = Some(output.clone());
        }
        if let Some(content) = update.get("content").and_then(Value::as_array) {
            call.content.clone_from(content);
        }
        if let Some(locations) = update.get("locations").and_then(Value::as_array) {
            call.locations.clone_from(locations);
        }
        let ended = matches!(
            status.as_deref(),
            Some("completed" | "failed" | "cancelled")
        );
        if let Some(status) = status {
            call.status = Some(status);
        }
        if ended {
            call.result_meta = stamp.meta.clone();
            call.result_at = stamp.at.clone();
        }
        if !known && announces {
            self.pending.push(id.to_owned());
            self.items.push(Item::Call { id: id.to_owned() });
        }
        if ended && self.resulted.insert(id.to_owned()) {
            self.pending.retain(|pending| pending != id);
            self.items.push(Item::Result { id: id.to_owned() });
        }
    }

    /// Ends the calls still open as cancelled, where the turn ends or the history does.
    fn close_calls(&mut self) {
        for id in core::mem::take(&mut self.pending) {
            self.resulted.insert(id.clone());
            if let Some(call) = self.calls.get_mut(&id) {
                call.status = Some("cancelled".to_owned());
                self.closed_calls += 1;
            }
            self.items.push(Item::Result { id });
        }
    }

    fn finish(mut self) -> Conversation {
        let closed_turn = self.open;
        self.close_calls();
        if self.open {
            self.items.push(Item::TurnEnd {
                stop: Stop::Acp(StopReason::Cancelled),
                error: None,
                closed_here: true,
                stamp: Stamp::default(),
            });
        }
        Conversation {
            summary: None,
            seeded_from: None,
            summary_missing: false,
            items: self.items,
            calls: self.calls,
            closed_calls: self.closed_calls,
            closed_turn,
        }
    }
}

/// Builds the lines of a history from what a codec reads in a harness's files, keeping the order
/// rules: a prompt opens a turn, a turn end and a cancel need one open.
#[derive(Debug, Default)]
pub struct Recorder {
    lines: Vec<Line>,
    open: bool,
}

impl Recorder {
    /// A recorder with nothing in it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many lines it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether it holds no line.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Whether a turn is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// A prompt. A turn still open is ended first as an `end_turn`: a codec that knows better ends
    /// it itself before.
    pub fn prompt(&mut self, blocks: Vec<Value>, stamp: Stamp) {
        if self.open {
            self.end_turn(Stop::Acp(StopReason::EndTurn), None, Stamp::default());
        }
        self.open = true;
        self.push(Line::prompt(blocks), stamp);
    }

    /// An update of the agent.
    pub fn update(&mut self, update: Value, stamp: Stamp) {
        self.push(Line::update(update), stamp);
    }

    /// The agent's text.
    pub fn message(&mut self, text: &str, stamp: Stamp) {
        self.update(
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}),
            stamp,
        );
    }

    /// The agent's reasoning.
    pub fn thought(&mut self, text: &str, stamp: Stamp) {
        self.update(
            json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": text}}),
            stamp,
        );
    }

    /// A message of the person's that is no prompt.
    pub fn note(&mut self, block: Value, stamp: Stamp) {
        self.update(
            json!({"sessionUpdate": "user_message_chunk", "content": block}),
            stamp,
        );
    }

    /// The agent asked for leave to run a tool call; nothing when no turn is open.
    pub fn permission_request(&mut self, tool_call: Value, options: Vec<Value>, stamp: Stamp) {
        if self.open {
            self.push(
                Line::new(Body::PermissionRequest { tool_call, options }),
                stamp,
            );
        }
    }

    /// The client's answer to the request for `tool_call_id`; nothing when no turn is open.
    pub fn permission_answer(&mut self, tool_call_id: String, outcome: Value, stamp: Stamp) {
        if self.open {
            self.push(
                Line::new(Body::PermissionAnswer {
                    tool_call_id,
                    outcome,
                }),
                stamp,
            );
        }
    }

    /// The client cancelled the turn; nothing when none is open.
    pub fn cancel(&mut self, stamp: Stamp) {
        if self.open {
            self.push(Line::new(Body::Cancel), stamp);
        }
    }

    /// Ends the turn; nothing when none is open.
    pub fn end_turn(&mut self, stop: Stop, error: Option<String>, stamp: Stamp) {
        if self.open {
            self.open = false;
            self.push(Line::new(Body::TurnEnd { stop, error }), stamp);
        }
    }

    /// A compaction boundary.
    pub fn compaction(
        &mut self,
        summary: Option<String>,
        trigger: Option<Trigger>,
        pre_tokens: Option<u64>,
        stamp: Stamp,
    ) {
        self.push(
            Line::new(Body::Compaction {
                summary: summary.filter(|summary| !summary.is_empty()),
                trigger,
                pre_tokens,
            }),
            stamp,
        );
    }

    /// The lines, the turn left open if it is open: a recording cut short.
    #[must_use]
    pub fn finish(self) -> Vec<Line> {
        self.lines
    }

    fn push(&mut self, mut line: Line, stamp: Stamp) {
        line.at = stamp.at;
        line.meta = stamp.meta;
        self.lines.push(line);
    }
}

/// A UUID made from `session`, a `what` and a counter: the same history gives the same files.
#[must_use]
pub fn derived_uuid(session: &str, what: &str, n: u64) -> String {
    let digest = Sha256::digest(format!("{session}:{what}:{n}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = data_encoding::HEXLOWER.encode(&bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Whether `text` is a canonical lower-case UUID.
#[must_use]
pub fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(at, byte)| match at {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

/// A call id a harness will take, from the history's: letters, digits, `_` and `-`, at most 64, and
/// not one of `taken`'s values.
#[must_use]
pub fn clean_id(id: &str, taken: &BTreeMap<String, String>) -> String {
    let mut clean: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if clean.is_empty() {
        clean = "call".to_owned();
    }
    let mut n = taken.len();
    while taken.values().any(|used| *used == clean) {
        clean = format!("{}_{n}", &clean[..clean.len().min(56)]);
        n += 1;
    }
    clean
}

/// A command line from an argument vector: the script of `sh -c` and `powershell -Command`, else the
/// words quoted.
#[must_use]
pub fn shell_text(words: &[&str]) -> String {
    if let [shell, flag, script] = words {
        let shell = shell.to_ascii_lowercase();
        let runs_script = shell.ends_with("sh")
            || shell.contains("powershell")
            || shell.ends_with("pwsh")
            || shell.ends_with("cmd")
            || shell.ends_with("cmd.exe");
        if runs_script && matches!(*flag, "-c" | "-lc" | "-Command" | "-command" | "/c") {
            return (*script).to_owned();
        }
    }
    words
        .iter()
        .map(|word| {
            if !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c))
            {
                (*word).to_owned()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A tool call with no equivalent in the target, as the text the target's model reads: the call
/// and what came of it.
#[must_use]
pub fn call_note(call: &Call) -> String {
    let name = call
        .name
        .as_deref()
        .or(call.title.as_deref())
        .unwrap_or("tool");
    let mut note = format!("[Tool call: {name}");
    if let Some(input) = &call.raw_input {
        note.push_str(&format!(" {input}"));
    }
    note.push(']');
    let output = call.output_text();
    let outcome = match call.status.as_deref() {
        Some("failed") => "failed",
        Some("cancelled") => "did not finish",
        _ => "result",
    };
    match (&call.raw_output, output.is_empty()) {
        (None, true) => note.push_str(&format!(
            "
[Tool {outcome}]"
        )),
        (Some(raw), true) => note.push_str(&format!(
            "
[Tool {outcome}: {raw}]"
        )),
        (_, false) => note.push_str(&format!(
            "
[Tool {outcome}]
{output}"
        )),
    }
    note
}

/// A header's `createdAt` and the other text fields are checked by the format; this is the check a
/// codec runs on a time it read from a harness's file before it uses it.
#[must_use]
pub fn valid_time(at: &str) -> bool {
    crate::shape::is_timestamp(at)
}

/// The error a codec reports for a history it cannot write from.
pub type Result<T> = core::result::Result<T, HistoryError>;
