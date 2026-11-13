//! Codex's app-server messages as events.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use std::io;

use coqui_acp_api::usage::{Bucket, LimitHit};
use coqui_acp_harness::event::{
    Content, Diff, Event, FileChange, Location, OptionKind, Outcome, PermissionOption,
    PermissionRequest, PlanEntry, PlanStatus, Responder, Severity, Stop, Terminal, ToolCall,
    ToolContent, ToolKind, ToolStatus, TurnEnd,
};
use coqui_acp_harness::lines::one_line;
use serde_json::{Value, json};

use crate::limits;

/// Writes one JSON-RPC message, as one line, to the app server.
pub type Writer = Arc<dyn Fn(&Value) -> io::Result<()> + Send + Sync>;

/// The answer to one of our requests, by its id as text: its `result`, or the error's message.
pub type Answer = (String, Result<Value, String>);

/// The id of the option that allows one call.
pub const ALLOW_ONCE: &str = "accept";
/// The id of the option that allows calls like it for the rest of the session.
pub const ALLOW_SESSION: &str = "accept_for_session";
/// The id of the option that allows the command and the rule Codex proposed for commands like it.
pub const ALLOW_RULE: &str = "accept_with_rule";
/// The id of the option that always allows an MCP tool.
pub const ALLOW_ALWAYS: &str = "accept_always";
/// The id of the option that refuses the call.
pub const REJECT: &str = "decline";
/// The id of the option that keeps a rule letting a host through.
pub const ALLOW_NETWORK_RULE: &str = "allow_network_rule";
/// The id of the option that keeps a rule refusing a host.
pub const REJECT_NETWORK_RULE: &str = "reject_network_rule";

/// What a [`Parser`] keeps and cuts. Every one is a setting with a default, and any value is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParserLimits {
    /// The longest title made from a command or a query, in bytes. Default 256.
    pub max_title_bytes: usize,
    /// The most of a command a question's description tells, in bytes, marked when cut. Default 16 KiB.
    pub max_told_bytes: usize,
    /// The messages remembered as streamed, so their whole copy is not told again. Default 64.
    pub max_streamed: usize,
}

impl ParserLimits {
    /// The defaults.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_title_bytes: 256,
            max_told_bytes: 16 * 1024,
            max_streamed: 64,
        }
    }
}

impl Default for ParserLimits {
    fn default() -> Self {
        Self::new()
    }
}

fn title_within(max: usize, text: &str) -> String {
    let first = text.lines().next().unwrap_or_default();
    let cut = one_line(coqui_acp_harness::lines::cut(first, max));
    if cut.len() < text.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// Reads Codex's app-server lines into [`Event`]s. A server request asking leave becomes an [`Event::Permission`]
/// whose responder writes the answer through the [`Writer`]; any other server request is refused at once.
pub struct Parser {
    writer: Writer,
    max_tools: usize,
    /// Items whose text arrived in pieces, so their whole copy is not told again.
    streamed: VecDeque<String>,
    /// Items started, oldest first, by id: their kind, for their deltas and results.
    items: BTreeMap<String, String>,
    order: VecDeque<String>,
    /// Commands whose output arrived in pieces.
    output_streamed: BTreeSet<String>,
    limits: ParserLimits,
    /// This session's thread, once known: what another thread says is not this session's.
    thread: Option<String>,
    /// Calls of MCP tools started and not finished, oldest first.
    mcp_running: VecDeque<McpCall>,
    /// The file changes started and not finished, as last told, for their approvals.
    edits: BTreeMap<String, ToolCall>,
    /// Whether the next turn to start was given up on before it started.
    abandon_next: bool,
    /// The `turn/start` whose answer was given up on: when it comes after all, it names a turn given up on.
    abandoned_request: Option<String>,
    /// The turns given up on: each is interrupted, and its start and end are nobody's.
    abandoned: BTreeSet<String>,
    turn: Option<String>,
    /// The turn that ended last: `turn/start`'s answer can be handled after that turn's end.
    ended: Option<String>,
    interrupted: bool,
    /// The latest window the account reported as used up.
    exhausted: Option<LimitHit>,
    /// What the account has said of its allowances, for merging each sparse update into.
    account: limits::Account,
    /// The `account/rateLimits/read` requests not yet answered: their answers are applied where they are read, so
    /// an update read before one is not overwritten by it.
    reads: BTreeSet<String>,
    /// The error the running turn reported before its end.
    error: Option<Value>,
    answers: Vec<Answer>,
}

impl core::fmt::Debug for Parser {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Parser")
            .field("turn", &self.turn)
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

impl Parser {
    /// A parser that answers server requests through `writer`, keeping the last `max_tools` items it has seen
    /// started (any value is taken; 0 keeps none, and a delta of an item forgotten is told as text of its own).
    #[must_use]
    pub fn new(writer: Writer, max_tools: usize) -> Self {
        Self {
            writer,
            max_tools,
            streamed: VecDeque::new(),
            items: BTreeMap::new(),
            order: VecDeque::new(),
            output_streamed: BTreeSet::new(),
            limits: ParserLimits::new(),
            thread: None,
            mcp_running: VecDeque::new(),
            edits: BTreeMap::new(),
            abandon_next: false,
            abandoned_request: None,
            abandoned: BTreeSet::new(),
            turn: None,
            ended: None,
            interrupted: false,
            exhausted: None,
            account: limits::Account::default(),
            reads: BTreeSet::new(),
            error: None,
            answers: Vec::new(),
        }
    }

    /// The running turn's id, once `turn/started` has said it.
    #[must_use]
    pub fn turn(&self) -> Option<&str> {
        self.turn.as_deref()
    }

    /// Takes `turn` as the running turn, as `turn/start`'s answer names it, unless it has already ended.
    pub fn started(&mut self, turn: &str) {
        if self.ended.as_deref() != Some(turn) {
            self.turn = Some(turn.to_string());
        }
    }

    /// Names this session's thread: a notification of another thread is dropped, and another thread's question says
    /// where it comes from.
    pub fn set_thread(&mut self, thread: &str) {
        self.thread = Some(thread.to_string());
    }

    /// Gives up on the turn just asked for by request `request`, whose `turn/start` went unanswered: it is interrupted, now,
    /// when it starts or when that answer comes after all, and its end is told to nobody, since the prompt has already
    /// failed.
    pub fn abandon(&mut self, request: &str) {
        match self.turn.clone() {
            Some(turn) => self.give_up(turn, None),
            None => {
                self.abandon_next = true;
                self.abandoned_request = Some(request.to_string());
            }
        }
    }

    /// A new prompt is on its way: a turn given up on that has not started is not the one that starts next, and what was
    /// left of the turn before says nothing of this one.
    pub fn prompting(&mut self) {
        self.abandon_next = false;
        self.interrupted = false;
        self.error = None;
    }

    fn give_up(&mut self, turn: String, thread: Option<&str>) {
        let thread = self.thread.as_deref().or(thread);
        let _ = (self.writer)(
            &json!({"id": format!("given-up-{turn}"), "method": "turn/interrupt",
            "params": {"threadId": thread, "turnId": turn}}),
        );
        self.abandoned.insert(turn);
    }

    /// The same with these bounds.
    #[must_use]
    pub fn with_limits(writer: Writer, max_tools: usize, limits: ParserLimits) -> Self {
        Self {
            limits,
            ..Self::new(writer, max_tools)
        }
    }

    fn title(&self, text: &str) -> String {
        title_within(self.limits.max_title_bytes, text)
    }

    /// `text` within the bound of what a question's description tells, saying when it was cut.
    fn told(&self, text: &str) -> String {
        let cut = coqui_acp_harness::lines::cut(text, self.limits.max_told_bytes);
        if cut.len() < text.len() {
            format!("{cut}\n[cut at {} bytes of {}]", cut.len(), text.len())
        } else {
            cut.to_string()
        }
    }

    /// How many commands' output is remembered as having come in pieces.
    #[must_use]
    pub fn kept_output(&self) -> usize {
        self.output_streamed.len()
    }

    /// Marks the running turn as interrupted: its end reads as cancelled.
    pub fn interrupting(&mut self) {
        self.interrupted = true;
    }

    /// The answers to our own requests read since the last call.
    pub fn answers(&mut self) -> Vec<Answer> {
        core::mem::take(&mut self.answers)
    }

    fn keep(&mut self, id: &str, kind: &str) {
        if self.max_tools == 0 {
            return;
        }
        if !self.items.contains_key(id) {
            if self.order.len() >= self.max_tools
                && let Some(oldest) = self.order.pop_front()
            {
                self.items.remove(&oldest);
                self.output_streamed.remove(&oldest);
                self.edits.remove(&oldest);
            }
            self.order.push_back(id.to_string());
        }
        self.items.insert(id.to_string(), kind.to_string());
    }

    fn streamed(&mut self, id: &str) {
        if !self.streamed.iter().any(|known| known == id) {
            self.streamed.push_back(id.to_string());
            while self.streamed.len() > self.limits.max_streamed {
                self.streamed.pop_front();
            }
        }
    }

    /// The events of one message of the app server.
    pub fn line(&mut self, message: &Value) -> Vec<Event> {
        let method = message.get("method").and_then(Value::as_str);
        match (method, message.get("id")) {
            (None, Some(id)) => {
                let answer = match message.get("error") {
                    Some(error) => Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the app server refused")
                        .to_string()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let request = id_text(id);
                if self.abandoned_request.as_deref() == Some(request.as_str()) {
                    self.abandoned_request = None;
                    self.abandon_next = false;
                    if let Some(turn) = text(message, "/result/turn/id") {
                        self.give_up(turn.to_string(), None);
                    }
                }
                if self.reads.remove(&request)
                    && let Ok(result) = &answer
                {
                    self.apply_read(result);
                }
                self.answers.push((request, answer));
                Vec::new()
            }
            (Some(method), Some(id)) => {
                self.request(method, id, message.get("params").unwrap_or(&Value::Null))
            }
            (Some(method), None)
                if self.other_thread(message.get("params").unwrap_or(&Value::Null)) =>
            {
                // Another thread's, such as a sub-agent's: nothing of it is this conversation's, but what it spent is spent.
                if method == "thread/tokenUsage/updated" {
                    limits::usage(message)
                        .map(|mut usage| {
                            usage.elsewhere = true;
                            usage.spend = None;
                            usage.context_used = None;
                            usage.context_size = None;
                            vec![Event::Usage(usage)]
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            }
            (Some(method), None) => self.notification(method, message),
            (None, None) => Vec::new(),
        }
    }

    /// Takes a full `account/rateLimits/read` answer as the account's allowances, in place of what was known, and
    /// answers the buckets of it. Later sparse updates merge into it.
    pub fn rate_limits_read(&mut self, answer: &Value) -> Vec<Bucket> {
        self.apply_read(answer)
    }

    /// Says that the answer to request `id` is an `account/rateLimits/read` answer, to be applied as it is read.
    pub fn expect_rate_limits_read(&mut self, id: &Value) {
        self.reads.insert(id_text(id));
    }

    fn apply_read(&mut self, answer: &Value) -> Vec<Bucket> {
        let buckets = self.account.read(answer);
        if let Some(window) = self.account.exhausted_any() {
            self.exhausted = Some(window);
        }
        buckets
    }

    /// The account's allowances as they now stand: the last full read with every update merged in.
    #[must_use]
    pub fn rate_limits(&self) -> Vec<Bucket> {
        self.account.buckets()
    }

    fn notification(&mut self, method: &str, message: &Value) -> Vec<Event> {
        let params = message.get("params").unwrap_or(&Value::Null);
        let mut buckets = Vec::new();
        if method == "account/rateLimits/updated" {
            buckets = self.account.update(message);
            if let Some(window) = self.account.exhausted(message) {
                self.exhausted = Some(window);
            }
        }
        match method {
            "turn/started" => {
                if text(params, "/turn/id").is_some_and(|turn| self.abandoned.contains(turn)) {
                    return Vec::new();
                }
                self.turn = text(params, "/turn/id").map(str::to_string);
                if core::mem::take(&mut self.abandon_next)
                    && let Some(turn) = self.turn.clone()
                {
                    self.give_up(turn, text(params, "/threadId"));
                }
                Vec::new()
            }
            "item/fileChange/patchUpdated" => {
                let Some(id) = text(params, "/itemId") else {
                    return Vec::new();
                };
                let call = file_change(id, params);
                if self.items.contains_key(id) {
                    self.edits.insert(id.to_string(), call.clone());
                }
                vec![Event::Tool(call)]
            }
            "item/started" => params
                .get("item")
                .map(|item| self.item(item, false, false))
                .unwrap_or_default(),
            "item/completed" => params
                .get("item")
                .map(|item| self.item(item, true, false))
                .unwrap_or_default(),
            "item/agentMessage/delta" => {
                let (Some(id), Some(delta)) = (text(params, "/itemId"), text(params, "/delta"))
                else {
                    return Vec::new();
                };
                self.streamed(id);
                vec![Event::MessageChunk {
                    message: id.to_string(),
                    text: delta.to_string(),
                }]
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                let (Some(id), Some(delta)) = (text(params, "/itemId"), text(params, "/delta"))
                else {
                    return Vec::new();
                };
                self.streamed(id);
                vec![Event::ThoughtChunk {
                    message: id.to_string(),
                    text: delta.to_string(),
                }]
            }
            "item/commandExecution/outputDelta" => {
                let (Some(id), Some(delta)) = (text(params, "/itemId"), text(params, "/delta"))
                else {
                    return Vec::new();
                };
                // Before the item is known, or after it was forgotten, it is not to be kept for good.
                if self.items.contains_key(id) {
                    self.output_streamed.insert(id.to_string());
                }
                let mut terminal = Terminal::new(id);
                terminal.output = Some(delta.to_string());
                vec![Event::Terminal(terminal)]
            }
            "item/fileChange/outputDelta" | "item/mcpToolCall/progress" => {
                let id = text(params, "/itemId");
                let said = text(params, "/delta").or_else(|| text(params, "/message"));
                match (id, said) {
                    (Some(id), Some(said)) => vec![Event::ToolChunk {
                        id: id.to_string(),
                        content: ToolContent::Text(said.to_string()),
                    }],
                    _ => Vec::new(),
                }
            }
            "turn/plan/updated" => {
                let entries = params
                    .get("plan")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|step| {
                        let status = match step.get("status").and_then(Value::as_str)? {
                            "completed" => PlanStatus::Completed,
                            "inProgress" | "in_progress" => PlanStatus::InProgress,
                            _ => PlanStatus::Pending,
                        };
                        Some(PlanEntry::new(
                            step.get("step").and_then(Value::as_str)?,
                            status,
                        ))
                    })
                    .collect();
                vec![Event::Plan(entries)]
            }
            "thread/tokenUsage/updated" => limits::usage(message)
                .map(|usage| vec![Event::Usage(usage)])
                .unwrap_or_default(),
            "account/rateLimits/updated" if !buckets.is_empty() => vec![Event::Buckets(buckets)],
            "error" => {
                let of_another_turn = text(params, "/turnId").is_some_and(|turn| {
                    self.abandoned.contains(turn)
                        || self.turn.as_deref().is_some_and(|own| own != turn)
                });
                if !of_another_turn
                    && params.get("willRetry").and_then(Value::as_bool) != Some(true)
                {
                    self.error = params.get("error").cloned();
                }
                Vec::new()
            }
            "turn/completed" | "turn/failed" => {
                let done = text(params, "/turn/id").map(str::to_string);
                if done
                    .as_ref()
                    .is_some_and(|turn| self.abandoned.remove(turn))
                {
                    if self.turn == done {
                        self.turn = None;
                    }
                    self.ended = done;
                    return Vec::new();
                }
                if self.turn.is_some() && done.is_some() && self.turn != done {
                    // A turn that is not the one running: its end is not this one's.
                    self.ended = done;
                    return Vec::new();
                }
                let end = self.end(method, message);
                self.ended = self.turn.take().or(done);
                vec![Event::TurnEnd(end)]
            }
            "thread/compacted" => vec![Event::Compaction {
                id: text(params, "/turnId").unwrap_or("compaction").to_string(),
                summary: None,
            }],
            "thread/name/updated" => text(params, "/threadName")
                .or_else(|| text(params, "/name"))
                .map(|name| vec![Event::Title(name.to_string())])
                .unwrap_or_default(),
            "warning" | "configWarning" | "deprecationNotice" | "guardianWarning" => {
                let said = text(params, "/message")
                    .or_else(|| text(params, "/summary"))
                    .or_else(|| text(params, "/details"));
                let severity = if method == "deprecationNotice" {
                    Severity::Info
                } else {
                    Severity::Warning
                };
                said.map(|said| {
                    vec![Event::Notice {
                        severity,
                        title: said.to_string(),
                    }]
                })
                .unwrap_or_default()
            }
            "mcpServer/startupStatus/updated" if text(params, "/status") == Some("failed") => {
                let name = text(params, "/name").unwrap_or("an MCP server");
                let why = text(params, "/error")
                    .or_else(|| text(params, "/failureReason"))
                    .unwrap_or("no reason given");
                vec![Event::Notice {
                    severity: Severity::Warning,
                    title: format!("MCP server {name} did not start: {why}"),
                }]
            }
            _ => Vec::new(),
        }
    }

    fn end(&mut self, method: &str, message: &Value) -> TurnEnd {
        let interrupted = core::mem::take(&mut self.interrupted);
        let reported = self.error.take();
        let error = limits::error_of(message).cloned().or(reported);
        if let Some(limit) = error
            .as_ref()
            .and_then(|error| limits::limit(error, self.exhausted.as_ref()))
        {
            return TurnEnd::Limited(limit);
        }
        let status = text(message, "/params/turn/status").unwrap_or(if method == "turn/failed" {
            "failed"
        } else {
            "completed"
        });
        match status {
            "interrupted" => TurnEnd::stopped(Stop::Cancelled),
            "failed" if interrupted => TurnEnd::stopped(Stop::Cancelled),
            "failed" => TurnEnd::Failed(
                error
                    .as_ref()
                    .and_then(|error| text(error, "/message"))
                    .unwrap_or("the turn failed")
                    .to_string(),
            ),
            _ => TurnEnd::stopped(Stop::EndTurn),
        }
    }

    /// The items of an earlier turn, as history: the person's messages are told too.
    pub fn history(&mut self, turn: &Value) -> Vec<Event> {
        turn.get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|item| {
                let mut events = self.item(item, false, true);
                events.extend(self.item(item, true, true));
                events
            })
            .collect()
    }

    fn item(&mut self, item: &Value, done: bool, history: bool) -> Vec<Event> {
        let (Some(id), Some(kind)) = (text(item, "/id"), text(item, "/type")) else {
            return Vec::new();
        };
        let id = id.to_string();
        if !done {
            self.keep(&id, kind);
        }
        match kind {
            "userMessage" if history && done => {
                let mut events: Vec<Event> = item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|block| {
                        user_content(block).map(|content| Event::UserChunk {
                            message: id.clone(),
                            content,
                        })
                    })
                    .collect();
                if !events.is_empty() {
                    events.push(Event::UserEnd { message: id });
                }
                events
            }
            "agentMessage" if done => {
                let mut events = Vec::new();
                if !self.streamed.iter().any(|known| *known == id)
                    && let Some(said) = text(item, "/text").filter(|said| !said.is_empty())
                {
                    events.push(Event::MessageChunk {
                        message: id.clone(),
                        text: said.to_string(),
                    });
                }
                events.push(Event::MessageEnd { message: id });
                events
            }
            "reasoning" if done => {
                let mut events = Vec::new();
                if !self.streamed.iter().any(|known| *known == id) {
                    let said: Vec<&str> = ["summary", "content"]
                        .iter()
                        .flat_map(|key| {
                            item.get(*key)
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                        })
                        .collect();
                    if said.is_empty() {
                        return Vec::new();
                    }
                    events.push(Event::ThoughtChunk {
                        message: id.clone(),
                        text: said.join("\n\n"),
                    });
                }
                events.push(Event::ThoughtEnd { message: id });
                events
            }
            "commandExecution" => self.command(&id, item, done),
            "fileChange" => {
                let mut call = file_change(&id, item);
                if done {
                    self.edits.remove(&id);
                } else if !history && self.items.contains_key(&id) {
                    self.edits.insert(id.clone(), call.clone());
                }
                call.status = Some(status(item, done));
                if done {
                    call.output = Some(json!({"status": item.get("status")}));
                }
                vec![Event::Tool(call)]
            }
            "mcpToolCall" | "dynamicToolCall" => {
                let server = text(item, "/server").or_else(|| text(item, "/namespace"));
                let tool = text(item, "/tool").unwrap_or("tool");
                if kind == "mcpToolCall" && !history {
                    self.mcp_running.retain(|call| call.id != id);
                    if !done && self.items.contains_key(&id) {
                        if self.mcp_running.len() >= self.max_tools {
                            self.mcp_running.pop_front();
                        }
                        self.mcp_running.push_back(McpCall {
                            id: id.clone(),
                            server: server.unwrap_or_default().to_string(),
                            arguments: item.get("arguments").cloned(),
                        });
                    }
                }
                let mut call = ToolCall::new(id.clone());
                call.name = Some(match server {
                    Some(server) => format!("mcp__{server}__{tool}"),
                    None => tool.to_string(),
                });
                call.title = Some(match server {
                    Some(server) => format!("{server}: {tool}"),
                    None => tool.to_string(),
                });
                call.kind = Some(ToolKind::Other);
                call.input = item.get("arguments").cloned();
                call.status = Some(status(item, done));
                let mut events = Vec::new();
                if done {
                    let said = text(item, "/error/message")
                        .map(str::to_string)
                        .or_else(|| {
                            let blocks =
                                item.pointer("/result/content").and_then(Value::as_array)?;
                            let said: Vec<&str> = blocks
                                .iter()
                                .filter_map(|block| block.get("text").and_then(Value::as_str))
                                .collect();
                            (!said.is_empty()).then(|| said.join("\n"))
                        });
                    if let Some(said) = said {
                        events.push(Event::ToolChunk {
                            id: id.clone(),
                            content: ToolContent::Text(said),
                        });
                    }
                    call.output = ["result", "error"]
                        .iter()
                        .filter_map(|key| item.get(*key))
                        .find(|value| !value.is_null())
                        .cloned();
                }
                events.push(Event::Tool(call));
                events
            }
            "webSearch" => {
                let mut call = ToolCall::new(id);
                call.name = Some("web_search".into());
                call.kind = Some(ToolKind::Fetch);
                call.title = Some(
                    match text(item, "/query").filter(|query| !query.is_empty()) {
                        Some(query) => format!("Search the web for {}", self.title(query)),
                        None => "Search the web".to_string(),
                    },
                );
                call.input =
                    Some(json!({"query": item.get("query"), "action": item.get("action")}));
                call.status = Some(if done {
                    ToolStatus::Completed
                } else {
                    ToolStatus::InProgress
                });
                if done {
                    call.output = Some(json!({"status": "completed"}));
                }
                vec![Event::Tool(call)]
            }
            "imageView" => {
                let mut call = ToolCall::new(id);
                call.name = Some("view_image".into());
                call.kind = Some(ToolKind::Read);
                let path = text(item, "/path").unwrap_or_default();
                call.title = Some(format!("View {path}"));
                call.locations = Some(vec![Location::new(path, None)]);
                call.input = Some(json!({"path": path}));
                call.status = Some(if done {
                    ToolStatus::Completed
                } else {
                    ToolStatus::InProgress
                });
                if done {
                    call.output = Some(json!({"status": "completed"}));
                }
                vec![Event::Tool(call)]
            }
            "contextCompaction" if done => vec![Event::Compaction { id, summary: None }],
            _ => Vec::new(),
        }
    }

    fn command(&mut self, id: &str, item: &Value, done: bool) -> Vec<Event> {
        let command = text(item, "/command").unwrap_or_default();
        let mut call = ToolCall::new(id);
        call.name = Some("exec_command".into());
        call.kind = Some(ToolKind::Execute);
        call.title = Some(self.title(command));
        call.input = Some(json!({"command": command, "cwd": item.get("cwd")}));
        call.status = Some(status(item, done));
        if done {
            call.output = Some(json!({
                "aggregatedOutput": item.get("aggregatedOutput"),
                "exitCode": item.get("exitCode"),
                "status": item.get("status"),
            }));
        }
        call.content = Some(vec![ToolContent::Terminal(id.to_string())]);
        let mut terminal = Terminal::new(id);
        if done {
            if !self.output_streamed.remove(id)
                && let Some(output) =
                    text(item, "/aggregatedOutput").filter(|output| !output.is_empty())
            {
                terminal.output = Some(output.to_string());
            }
            let code = item
                .get("exitCode")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok());
            terminal.exit = Some(code);
            if text(item, "/status") == Some("declined") {
                call.content = Some(vec![
                    ToolContent::Terminal(id.to_string()),
                    ToolContent::Text("The command was declined.".into()),
                ]);
            }
        } else {
            terminal.command = Some(command.to_string());
        }
        if done {
            vec![Event::Terminal(terminal), Event::Tool(call)]
        } else {
            vec![Event::Tool(call), Event::Terminal(terminal)]
        }
    }

    fn request(&mut self, method: &str, id: &Value, params: &Value) -> Vec<Event> {
        match method {
            "item/commandExecution/requestApproval" => self.command_approval(method, id, params),
            "execCommandApproval" => self.legacy_command_approval(method, id, params),
            "item/fileChange/requestApproval" | "applyPatchApproval" => {
                self.file_approval(method, id, params)
            }
            "item/permissions/requestApproval" => self.permissions_approval(method, id, params),
            "mcpServer/elicitation/request" => match self.mcp_call_asked_about(params) {
                Some(call) => self.mcp_approval(method, &call, id, params),
                None => self.not_taken(id),
            },
            _ => self.not_taken(id),
        }
    }

    fn not_taken(&self, id: &Value) -> Vec<Event> {
        let _ = (self.writer)(
            &json!({"id": id, "error": {"code": -32601, "message": "this client does not take that request"}}),
        );
        Vec::new()
    }

    /// Whether `params` name a thread other than this session's.
    fn other_thread(&self, params: &Value) -> bool {
        let said = text(params, "/threadId").or_else(|| text(params, "/conversationId"));
        matches!((&self.thread, said), (Some(own), Some(said)) if own != said)
    }

    /// A question whose chosen option's `result` goes back; an option not offered gets `rejected`, and nobody choosing
    /// gets `cancelled`.
    #[expect(
        clippy::too_many_arguments,
        reason = "each is a different part of the question"
    )]
    fn ask(
        &self,
        method: &str,
        id: &Value,
        params: &Value,
        tool: ToolCall,
        question: String,
        description: Option<String>,
        choices: Vec<Choice>,
        rejected: Value,
        cancelled: Value,
    ) -> Vec<Event> {
        let question = if self.other_thread(params) {
            format!("In another thread of this session: {question}")
        } else {
            question
        };
        let mut details = serde_json::Map::new();
        details.insert("method".into(), json!(method));
        for key in [
            "kind",
            "grantRoot",
            "threadId",
            "conversationId",
            "turnId",
            "itemId",
            "approvalId",
            "serverName",
        ] {
            if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
                details.insert(key.into(), value.clone());
            }
        }
        let options = choices.iter().map(|choice| choice.option.clone()).collect();
        let writer = Arc::clone(&self.writer);
        let id = id.clone();
        let responder = Responder::new(move |outcome| {
            let result = match outcome {
                Outcome::Selected(chosen) => choices
                    .into_iter()
                    .find(|choice| choice.option.id == chosen)
                    .map_or(rejected, |choice| choice.result),
                Outcome::Refused => rejected,
                _ => cancelled,
            };
            writer(&json!({"id": id, "result": result}))
        });
        vec![Event::Permission(
            PermissionRequest::new(tool, question, description, options, responder)
                .details(details),
        )]
    }

    fn command_approval(&self, method: &str, id: &Value, params: &Value) -> Vec<Event> {
        let command = text(params, "/command").unwrap_or_default();
        let shown = self.title(command);
        let network = text(params, "/networkApprovalContext/host").map(one_line);
        let question = match &network {
            Some(host) => format!(
                "Let a command reach {host} over {}",
                one_line(text(params, "/networkApprovalContext/protocol").unwrap_or("the network"))
            ),
            None if text(params, "/kind") == Some("writeStdin") => {
                format!("Send input to a running command: {shown}")
            }
            None => format!("Run {shown}"),
        };
        let mut tool = ToolCall::new(item_of(params));
        tool.name = Some("exec_command".into());
        tool.kind = Some(ToolKind::Execute);
        tool.title = Some(if command.is_empty() {
            question.clone()
        } else {
            shown.clone()
        });
        let mut input = json!({"command": command, "cwd": params.get("cwd")});
        if let Some(context) = params
            .get("networkApprovalContext")
            .filter(|it| !it.is_null())
        {
            input["network"] = context.clone();
        }
        tool.input = Some(input);
        let mut told: Vec<String> = text(params, "/reason")
            .map(str::to_string)
            .into_iter()
            .collect();
        if shown != command || (network.is_some() && !command.is_empty()) {
            told.push(format!("The whole command:\n{}", self.told(command)));
        }
        let description = (!told.is_empty()).then(|| told.join("\n\n"));

        let listed = params.get("availableDecisions").and_then(Value::as_array);
        let default;
        let decisions = if let Some(listed) = listed {
            listed
        } else {
            default = default_decisions(params);
            &default
        };
        let decided = |decision: &Value| json!({"decision": decision});
        let mut choices = Vec::new();
        let mut cancel = false;
        for decision in decisions {
            match decision {
                Value::String(word) => match word.as_str() {
                    "accept" => choices.push(Choice::new(
                        ALLOW_ONCE,
                        network.as_ref().map_or_else(
                            || "Allow".to_string(),
                            |host| format!("Allow {host} this once"),
                        ),
                        OptionKind::AllowOnce,
                        decided(decision),
                    )),
                    "acceptForSession" => choices.push(Choice::new(
                        ALLOW_SESSION,
                        network.as_ref().map_or_else(
                            || "Allow for this session".to_string(),
                            |host| format!("Allow {host} for this session"),
                        ),
                        OptionKind::AllowAlways,
                        decided(decision),
                    )),
                    "cancel" => cancel = true,
                    _ => {}
                },
                Value::Object(rule) => {
                    if let Some(words) = rule
                        .get("acceptWithExecpolicyAmendment")
                        .and_then(|amendment| amendment.get("execpolicy_amendment"))
                        .and_then(words_of)
                    {
                        choices.push(Choice::new(
                            ALLOW_RULE,
                            format!(
                                "Allow, and from now on every command starting with `{}`",
                                self.title(&one_line(&words.join(" ")))
                            ),
                            OptionKind::AllowAlways,
                            decided(decision),
                        ));
                    } else if let Some(amendment) = rule
                        .get("applyNetworkPolicyAmendment")
                        .and_then(|amendment| amendment.get("network_policy_amendment"))
                    {
                        let host = one_line(text(amendment, "/host").unwrap_or("this host"));
                        choices.push(if text(amendment, "/action") == Some("allow") {
                            Choice::new(
                                ALLOW_NETWORK_RULE,
                                format!("Allow {host} from now on"),
                                OptionKind::AllowAlways,
                                decided(decision),
                            )
                        } else {
                            Choice::new(
                                REJECT_NETWORK_RULE,
                                format!("Refuse {host} from now on"),
                                OptionKind::RejectAlways,
                                decided(decision),
                            )
                        });
                    }
                }
                _ => {}
            }
        }
        // Refusing one call never needs Codex's leave: 0.164.0 and 0.166.0 take `decline` where they list only `cancel`, and go on.
        let rejected = decided(&json!("decline"));
        choices.push(Choice::new(
            REJECT,
            "Reject",
            OptionKind::RejectOnce,
            rejected.clone(),
        ));
        let cancelled = decided(&json!(if cancel { "cancel" } else { "decline" }));
        self.ask(
            method,
            id,
            params,
            tool,
            question,
            description,
            choices,
            rejected,
            cancelled,
        )
    }

    fn legacy_command_approval(&self, method: &str, id: &Value, params: &Value) -> Vec<Event> {
        let command = match params.get("command") {
            Some(Value::Array(words)) => words
                .iter()
                .filter_map(Value::as_str)
                .map(|word| {
                    if word.is_empty() || word.contains(|c: char| c.is_whitespace() || c == '"') {
                        format!("{word:?}")
                    } else {
                        word.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            Some(Value::String(command)) => command.clone(),
            _ => String::new(),
        };
        let shown = self.title(&command);
        let mut tool = ToolCall::new(item_of(params));
        tool.name = Some("exec_command".into());
        tool.kind = Some(ToolKind::Execute);
        tool.title = Some(shown.clone());
        tool.input = Some(json!({"command": command, "cwd": params.get("cwd")}));
        let mut told: Vec<String> = text(params, "/reason")
            .map(str::to_string)
            .into_iter()
            .collect();
        if shown != command {
            told.push(format!("The whole command:\n{}", self.told(&command)));
        }
        let description = (!told.is_empty()).then(|| told.join("\n\n"));
        self.ask(
            method,
            id,
            params,
            tool,
            format!("Run {shown}"),
            description,
            legacy_choices(),
            legacy_denied(),
            json!({"decision": "abort"}),
        )
    }

    fn file_approval(&self, method: &str, id: &Value, params: &Value) -> Vec<Event> {
        let legacy = method == "applyPatchApproval";
        let item = item_of(params);
        let mut tool = if legacy {
            legacy_file_change(&item, params)
        } else {
            self.edits
                .get(&item)
                .cloned()
                .unwrap_or_else(|| ToolCall::new(item.clone()))
        };
        tool.status = None;
        tool.kind = Some(ToolKind::Edit);
        let question = tool
            .title
            .get_or_insert_with(|| "Edit files".to_string())
            .clone();
        let root = text(params, "/grantRoot").map(one_line);
        let session = root.as_ref().map_or_else(
            || "Allow for this session".to_string(),
            |root| format!("Allow for this session, and writing under {root}"),
        );
        let reason = text(params, "/reason").map(str::to_string);
        if legacy {
            return self.ask(
                method,
                id,
                params,
                tool,
                question,
                reason,
                legacy_choices(),
                legacy_denied(),
                json!({"decision": "abort"}),
            );
        }
        let choices = vec![
            Choice::new(
                ALLOW_ONCE,
                "Allow",
                OptionKind::AllowOnce,
                json!({"decision": "accept"}),
            ),
            Choice::new(
                ALLOW_SESSION,
                session,
                OptionKind::AllowAlways,
                json!({"decision": "acceptForSession"}),
            ),
            Choice::new(
                REJECT,
                "Reject",
                OptionKind::RejectOnce,
                json!({"decision": "decline"}),
            ),
        ];
        self.ask(
            method,
            id,
            params,
            tool,
            question,
            reason,
            choices,
            json!({"decision": "decline"}),
            json!({"decision": "cancel"}),
        )
    }

    fn permissions_approval(&self, method: &str, id: &Value, params: &Value) -> Vec<Event> {
        let requested = params
            .get("permissions")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let what = granted(&requested, self.limits.max_title_bytes);
        let mut tool = ToolCall::new(item_of(params));
        tool.name = Some("permissions".into());
        tool.kind = Some(ToolKind::Other);
        let question = format!("Grant more permissions: {what}");
        tool.title = Some(question.clone());
        tool.input = Some(requested.clone());
        let none = json!({"permissions": {}, "scope": "turn"});
        let choices = vec![
            Choice::new(
                ALLOW_ONCE,
                format!("Allow for this turn: {what}"),
                OptionKind::AllowOnce,
                json!({"permissions": requested, "scope": "turn"}),
            ),
            Choice::new(
                ALLOW_SESSION,
                format!("Allow for this session: {what}"),
                OptionKind::AllowAlways,
                json!({"permissions": requested, "scope": "session"}),
            ),
            Choice::new(REJECT, "Reject", OptionKind::RejectOnce, none.clone()),
        ];
        let reason = text(params, "/reason").map(str::to_string);
        self.ask(
            method,
            id,
            params,
            tool,
            question,
            reason,
            choices,
            none.clone(),
            none,
        )
    }

    /// The running call an MCP tool's approval is about, the one with its arguments or else the server's latest. Any
    /// other elicitation has none: a server's own question, which is not an approval, and has no fields to fill here.
    fn mcp_call_asked_about(&self, params: &Value) -> Option<String> {
        if text(params, "/_meta/codex_approval_kind") != Some("mcp_tool_call")
            || text(params, "/mode") != Some("form")
            || params
                .pointer("/requestedSchema/properties")
                .and_then(Value::as_object)
                .is_some_and(|fields| !fields.is_empty())
        {
            return None;
        }
        let server = text(params, "/serverName")?;
        let arguments = params.pointer("/_meta/tool_params");
        let running = || {
            self.mcp_running
                .iter()
                .rev()
                .filter(|call| call.server == server)
        };
        running()
            .find(|call| arguments == call.arguments.as_ref())
            .or_else(|| running().next())
            .map(|call| call.id.clone())
    }

    fn mcp_approval(&self, method: &str, call: &str, id: &Value, params: &Value) -> Vec<Event> {
        let persist: Vec<&str> = match params.pointer("/_meta/persist") {
            Some(Value::String(one)) => vec![one.as_str()],
            Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let server = one_line(text(params, "/serverName").unwrap_or("an MCP server"));
        let mut tool = ToolCall::new(call);
        tool.kind = Some(ToolKind::Other);
        tool.input = params.pointer("/_meta/tool_params").cloned();
        let accepted = |meta: Value| json!({"action": "accept", "content": null, "_meta": meta});
        let mut choices = vec![Choice::new(
            ALLOW_ONCE,
            "Allow",
            OptionKind::AllowOnce,
            accepted(Value::Null),
        )];
        if persist.contains(&"session") {
            choices.push(Choice::new(
                ALLOW_SESSION,
                "Allow for this session",
                OptionKind::AllowAlways,
                accepted(json!({"persist": "session"})),
            ));
        }
        if persist.contains(&"always") {
            choices.push(Choice::new(
                ALLOW_ALWAYS,
                "Always allow",
                OptionKind::AllowAlways,
                accepted(json!({"persist": "always"})),
            ));
        }
        let declined = json!({"action": "decline", "content": null});
        choices.push(Choice::new(
            REJECT,
            "Reject",
            OptionKind::RejectOnce,
            declined.clone(),
        ));
        // The server's name comes first: the message after it is the server's own words.
        let question = format!(
            "MCP server {server}: {}",
            self.title(text(params, "/message").unwrap_or("run one of its tools"))
        );
        let reason = text(params, "/reason").map(str::to_string);
        self.ask(
            method,
            id,
            params,
            tool,
            question,
            reason,
            choices,
            declined,
            json!({"action": "cancel", "content": null}),
        )
    }
}

/// One option of a question, and the result that goes back when it is chosen.
struct Choice {
    option: PermissionOption,
    result: Value,
}

impl Choice {
    fn new(id: &str, name: impl Into<String>, kind: OptionKind, result: Value) -> Self {
        Self {
            option: PermissionOption::new(id, name, kind),
            result,
        }
    }
}

/// A running MCP tool call.
struct McpCall {
    id: String,
    server: String,
    arguments: Option<Value>,
}

fn item_of(params: &Value) -> String {
    text(params, "/itemId")
        .or_else(|| text(params, "/callId"))
        .unwrap_or("approval")
        .to_string()
}

/// The decisions of a command approval from a Codex that lists none.
fn default_decisions(params: &Value) -> Vec<Value> {
    let mut decisions = vec![json!("accept"), json!("acceptForSession")];
    if let Some(rule) = params
        .get("proposedExecpolicyAmendment")
        .filter(|rule| !rule.is_null())
    {
        decisions.push(json!({"acceptWithExecpolicyAmendment": {"execpolicy_amendment": rule}}));
    }
    decisions.push(json!("decline"));
    decisions.push(json!("cancel"));
    decisions
}

/// A rule's words, when it has some and every one is text: a rule that cannot be shown whole is not offered.
fn words_of(rule: &Value) -> Option<Vec<&str>> {
    let words: Option<Vec<&str>> = rule.as_array()?.iter().map(Value::as_str).collect();
    words.filter(|words| !words.is_empty())
}

fn legacy_choices() -> Vec<Choice> {
    vec![
        Choice::new(
            ALLOW_ONCE,
            "Allow",
            OptionKind::AllowOnce,
            json!({"decision": "approved"}),
        ),
        Choice::new(
            ALLOW_SESSION,
            "Allow for this session",
            OptionKind::AllowAlways,
            json!({"decision": "approved_for_session"}),
        ),
        Choice::new(REJECT, "Reject", OptionKind::RejectOnce, legacy_denied()),
    ]
}

fn legacy_denied() -> Value {
    json!({"decision": {"denied": {"rejection": "The person refused this."}}})
}

/// What a permissions request grants, in words: its paths, each with its access, and the network.
fn granted(requested: &Value, max_title: usize) -> String {
    let mut what = Vec::new();
    for (access, pointer) in [("read", "/fileSystem/read"), ("write", "/fileSystem/write")] {
        for path in requested
            .pointer(pointer)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            what.push(format!("{access} {}", one_line(path)));
        }
    }
    for entry in requested
        .pointer("/fileSystem/entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let access = text(entry, "/access").unwrap_or("access");
        let path = entry
            .get("path")
            .map_or_else(String::new, |path| match path.as_str() {
                Some(path) => one_line(path),
                None => one_line(&path.to_string()),
            });
        what.push(format!("{access} {path}"));
    }
    if requested.pointer("/network/enabled") == Some(&Value::Bool(true)) {
        what.push("network access".to_string());
    }
    if what.is_empty() {
        return "none named".to_string();
    }
    title_within(max_title, &what.join(", "))
}

/// A request or answer id as text, for matching.
#[must_use]
pub fn id_text(id: &Value) -> String {
    match id {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn status(item: &Value, done: bool) -> ToolStatus {
    match text(item, "/status") {
        Some("completed") => ToolStatus::Completed,
        Some("failed" | "declined") => ToolStatus::Failed,
        _ if done => ToolStatus::Completed,
        _ => ToolStatus::InProgress,
    }
}

fn user_content(block: &Value) -> Option<Content> {
    match block.get("type").and_then(Value::as_str)? {
        "text" => Some(Content::Text(block.get("text")?.as_str()?.to_string())),
        "image" => {
            let url = block.get("url")?.as_str()?;
            let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
            let mime_type = head.strip_suffix(";base64")?;
            Some(Content::Image {
                mime_type: mime_type.to_string(),
                data: data.to_string(),
            })
        }
        _ => None,
    }
}

/// A `fileChange` item as an edit, or an update of one: a diff per change, its patch made from Codex's hunks, and where
/// a moved file goes, in the title and the locations.
fn file_change(id: &str, item: &Value) -> ToolCall {
    let changes = item
        .get("changes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut call = ToolCall::new(id);
    call.name = Some("apply_patch".into());
    call.kind = Some(ToolKind::Edit);
    call.input = Some(json!({"changes": changes}));
    let paths: Vec<String> = changes
        .iter()
        .filter_map(|change| text(change, "/path").map(str::to_string))
        .collect();
    let moves: Vec<(&str, &str)> = changes
        .iter()
        .filter_map(|change| Some((text(change, "/path")?, text(change, "/kind/move_path")?)))
        .collect();
    call.title = Some(match (paths.as_slice(), moves.as_slice()) {
        ([_], [(from, to)]) => format!("Move {from} to {to}"),
        ([one], _) => format!("Edit {one}"),
        ([], _) => "Edit files".to_string(),
        (many, []) => format!("Edit {} files", many.len()),
        (many, [(from, to), ..]) => format!(
            "Edit {} files, moving {} of them, {from} to {to}",
            many.len(),
            moves.len()
        ),
    });
    call.locations = Some(
        paths
            .iter()
            .map(String::as_str)
            .chain(moves.iter().map(|(_, to)| *to))
            .map(|path| Location::new(path, None))
            .collect(),
    );
    call.content = Some(
        changes
            .iter()
            .filter_map(|change| {
                let path = text(change, "/path")?;
                let kind = match text(change, "/kind/type") {
                    Some("add") => FileChange::Add,
                    Some("delete") => FileChange::Delete,
                    _ => FileChange::Modify,
                };
                let mut diff = Diff::new(path, kind);
                if let Some(hunks) = text(change, "/diff").filter(|hunks| !hunks.is_empty()) {
                    diff.patch = Some(git_patch(path, kind, hunks));
                }
                Some(ToolContent::Diff(diff))
            })
            .collect(),
    );
    call
}

/// An `applyPatchApproval`'s changes, by path, as an edit.
fn legacy_file_change(id: &str, params: &Value) -> ToolCall {
    let mut call = ToolCall::new(id);
    let paths: Vec<String> = params
        .get("fileChanges")
        .and_then(Value::as_object)
        .map(|changes| changes.keys().cloned().collect())
        .unwrap_or_default();
    call.title = Some(match paths.as_slice() {
        [one] => format!("Edit {one}"),
        _ => "Edit files".to_string(),
    });
    call.locations = Some(
        paths
            .into_iter()
            .map(|path| Location::new(path, None))
            .collect(),
    );
    call
}

/// A git patch of one file from Codex's hunks, which start at `@@`. The path keeps its spelling with forward slashes.
#[must_use]
pub fn git_patch(path: &str, change: FileChange, hunks: &str) -> String {
    let path = path.replace('\\', "/");
    let name = if path
        .bytes()
        .any(|byte| byte == b'"' || byte == b'\\' || byte < 0x20)
    {
        format!("{path:?}")
    } else {
        path
    };
    let mut patch = format!("diff --git {name} {name}\n");
    match change {
        FileChange::Add => patch.push_str(&format!(
            "new file mode 100644\n--- /dev/null\n+++ {name}\n"
        )),
        FileChange::Delete => patch.push_str(&format!(
            "deleted file mode 100644\n--- {name}\n+++ /dev/null\n"
        )),
        _ => patch.push_str(&format!("--- {name}\n+++ {name}\n")),
    }
    patch.push_str(hunks);
    if !hunks.ends_with('\n') {
        patch.push('\n');
    }
    patch
}
