//! Claude Code's stream-json output as events.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use std::io;

use coqui_acp_api::usage::LimitHit;
use coqui_acp_harness::event::{
    Child, ChildState, ChildUsage, Command, Content, Event, OptionKind, Outcome, PermissionOption,
    PermissionRequest, PlanEntry, PlanStatus, Responder, Severity, Stop, Terminal, ToolCall,
    ToolContent, ToolStatus, TurnEnd,
};
use serde_json::{Map, Value, json};

use crate::{limits, tools};

/// Writes one JSON line to Claude Code's standard input.
pub type Writer = Arc<dyn Fn(&Value) -> io::Result<()> + Send + Sync>;

/// The id of the option that allows one call.
pub const ALLOW_ONCE: &str = "allow";
/// The id of the option that allows the call and applies the rule Claude Code suggested.
pub const ALLOW_ALWAYS: &str = "allow_always";
/// The id of the option that refuses the call.
pub const REJECT: &str = "reject";

/// What a [`Parser`] keeps and cuts. Every one is a setting with a default, and any value is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParserLimits {
    /// The longest title made from a tool's input, in bytes; a longer input is cut, on a character boundary. Default
    /// 256.
    pub max_title_bytes: usize,
    /// The messages remembered as streamed, so their whole copy is not told again. Default 64.
    pub max_streamed: usize,
    /// The entries of the plan the Task tools keep, the oldest dropped past it. Default 1024.
    pub max_tasks: usize,
    /// The longest task or result text a subagent's report carries, in bytes; a longer one is cut, on a character
    /// boundary. Default 16384.
    pub max_child_text_bytes: usize,
}

impl ParserLimits {
    /// The defaults.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_title_bytes: 256,
            max_streamed: 64,
            max_tasks: 1024,
            max_child_text_bytes: 16 * 1024,
        }
    }
}

impl Default for ParserLimits {
    fn default() -> Self {
        Self::new()
    }
}

/// The answer to one of our control requests: its `response`, or Claude Code's error.
pub type ControlAnswer = (String, Result<Value, String>);

#[derive(Debug)]
struct Tool {
    name: String,
    input: Value,
}

/// Reads Claude Code's stream-json lines into [`Event`]s. A permission request becomes an
/// [`Event::Permission`] whose responder writes the answer through the [`Writer`].
pub struct Parser {
    writer: Writer,
    max_tools: usize,
    /// Tool calls by id, oldest first, for their results.
    tools: BTreeMap<String, Tool>,
    order: VecDeque<String>,
    /// The message being streamed, and its text and thinking blocks by index.
    streaming: Option<String>,
    blocks: BTreeMap<u64, &'static str>,
    /// Messages whose text arrived as stream events, so their whole copy is not told again.
    streamed: VecDeque<String>,
    /// The tokens of the model's context the last request used.
    context_used: Option<u64>,
    /// Set by an interrupt of a running turn, so its error result reads as cancelled.
    interrupted: bool,
    /// Whether a prompt has been written whose result has not come.
    running: bool,
    /// The last rejected `rate_limit_event`: the only place that says when the limit resets.
    rejected: Option<LimitHit>,
    /// A turn an `error` line has ended: the `result` that may follow it is not a second end.
    ended_by_error: bool,
    limits: ParserLimits,
    /// The Task tools' list, by task id, for the plan they make.
    tasks: Vec<(String, PlanEntry)>,
    /// The subagent (the tool call that started it) each background task id belongs to, for the reports that name only the task.
    child_tasks: BTreeMap<String, String>,
    /// The subagents whose final report came, so a later hand-back does not replace their result.
    child_done: BTreeSet<String>,
    answers: Vec<ControlAnswer>,
    version: Option<String>,
    session: Option<String>,
    /// A compaction whose summary is still to come.
    compaction: Option<String>,
    /// Told the permission mode Claude Code reports, or that an answer of the person's switched it.
    mode_hook: Option<ModeHook>,
    /// Which `setMode` suggestions an "allow always" may apply; all of them when unset.
    mode_permit: Option<ModePermit>,
}

/// Told a Claude Code `permissionMode`.
pub type ModeHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Whether a `setMode` an "allow always" would apply names a mode that may be switched to.
pub type ModePermit = Arc<dyn Fn(&str) -> bool + Send + Sync>;

impl core::fmt::Debug for Parser {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Parser")
            .field("tools", &self.tools.len())
            .field("interrupted", &self.interrupted)
            .finish_non_exhaustive()
    }
}

impl Parser {
    /// A parser that answers permission requests through `writer`, keeping the last `max_tools` tool calls for their
    /// results (any value is taken; 0 keeps none, and a result then shows no diff or terminal).
    #[must_use]
    pub fn new(writer: Writer, max_tools: usize) -> Self {
        Self {
            writer,
            max_tools,
            tools: BTreeMap::new(),
            order: VecDeque::new(),
            streaming: None,
            blocks: BTreeMap::new(),
            streamed: VecDeque::new(),
            context_used: None,
            interrupted: false,
            running: false,
            rejected: None,
            ended_by_error: false,
            limits: ParserLimits::new(),
            tasks: Vec::new(),
            child_tasks: BTreeMap::new(),
            child_done: BTreeSet::new(),
            answers: Vec::new(),
            version: None,
            session: None,
            compaction: None,
            mode_hook: None,
            mode_permit: None,
        }
    }

    /// Sets which modes an "allow always" may switch Claude Code to. A `setMode` suggestion naming another is left out of
    /// what the answer applies, so a person's "always" cannot loosen what a host capped.
    pub fn permit_modes(&mut self, permit: ModePermit) {
        self.mode_permit = Some(permit);
    }

    /// Sets what is told the permission mode Claude Code is in: the one its `system` lines report, and the one an
    /// "allow always" answer switches it to by a `setMode` suggestion it is given.
    pub fn on_permission_mode(&mut self, hook: ModeHook) {
        self.mode_hook = Some(hook);
    }

    /// The same with these bounds.
    #[must_use]
    pub fn with_limits(writer: Writer, max_tools: usize, limits: ParserLimits) -> Self {
        Self {
            limits,
            ..Self::new(writer, max_tools)
        }
    }

    /// A prompt is being written: what was left of the turn before it says nothing of this one.
    pub fn prompting(&mut self) {
        // A prompt written while one runs waits its turn: what the running one was told is still its own.
        if !self.running {
            self.rejected = None;
        }
        self.running = true;
        self.interrupted = false;
        self.ended_by_error = false;
    }

    /// Marks the running turn as interrupted: its error result reads as cancelled. With no turn running this does
    /// nothing, so an interrupt that raced a turn's end cannot touch the next.
    pub fn interrupting(&mut self) {
        self.interrupted = self.running;
    }

    /// The limit a rejected `rate_limit_event` named, for a turn still running when its output ended: no `result` will
    /// say it.
    pub fn end_of_output(&mut self) -> Option<TurnEnd> {
        let limit = self.rejected.take()?;
        core::mem::take(&mut self.running).then_some(TurnEnd::Limited(limit))
    }

    /// The answers to our own control requests read since the last call, by request id.
    pub fn answers(&mut self) -> Vec<ControlAnswer> {
        core::mem::take(&mut self.answers)
    }

    /// Claude Code's version, once it has said it.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Claude Code's session id, once it has said it.
    #[must_use]
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    fn keep(&mut self, id: &str, name: &str, input: &Value) {
        if self.max_tools == 0 {
            return;
        }
        if !self.tools.contains_key(id) {
            if self.order.len() >= self.max_tools
                && let Some(oldest) = self.order.pop_front()
            {
                self.tools.remove(&oldest);
            }
            self.order.push_back(id.to_string());
        }
        self.tools.insert(
            id.to_string(),
            Tool {
                name: name.to_string(),
                input: input.clone(),
            },
        );
    }

    /// The events of one line.
    pub fn line(&mut self, value: &Value) -> Vec<Event> {
        let mut events = Vec::new();
        if let Some(session) = value.get("session_id").and_then(Value::as_str)
            && self.session.as_deref() != Some(session)
        {
            self.session = Some(session.to_string());
            events.push(Event::SessionId(session.to_string()));
        }
        match value.get("type").and_then(Value::as_str).unwrap_or("") {
            "control_response" => self.control_response(value),
            "control_request" => events.extend(self.control_request(value)),
            "system" => events.extend(self.system(value)),
            // A subagent's partial message here would be taken for the parent's; its whole message comes later.
            "stream_event" if parent_of(value).is_some() => {}
            "stream_event" => events.extend(self.stream_event(value)),
            "assistant" => {
                let said = self.assistant(value, false);
                events.extend(in_child(parent_of(value), said));
            }
            "user" => {
                let said = self.user(value, false);
                events.extend(in_child(parent_of(value), said));
            }
            "result" => events.extend(self.result(value)),
            "error" => events.extend(self.error_line(value)),
            "rate_limit_event" => {
                self.rejected = limits::limit(value);
                let buckets = limits::buckets(value);
                if !buckets.is_empty() {
                    events.push(Event::Buckets(buckets));
                }
            }
            _ => {}
        }
        events
    }

    /// A line of `type: error`: the turn has failed, or a quota refused it.
    fn error_line(&mut self, value: &Value) -> Vec<Event> {
        let text = [
            "/result",
            "/error/message",
            "/message/content/0/text",
            "/message",
            "/error",
        ]
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .unwrap_or("Claude Code reported an error")
        .to_string();
        let rejected = self.rejected.take();
        let end = match limits::limit_of_text(&text) {
            Some(limit) => TurnEnd::Limited(limits::with_hint(limit, rejected)),
            None => match rejected {
                Some(mut limit) => {
                    limit.text = Some(text);
                    TurnEnd::Limited(limit)
                }
                None => TurnEnd::Failed(text),
            },
        };
        self.ended_by_error = true;
        self.running = false;
        self.interrupted = false;
        vec![Event::TurnEnd(end)]
    }

    /// The events of one line of a session's transcript, as history: the person's messages are told too.
    pub fn history(&mut self, value: &Value) -> Vec<Event> {
        let sidechain = value.get("isSidechain").and_then(Value::as_bool) == Some(true);
        let told = match value.get("type").and_then(Value::as_str).unwrap_or("") {
            "assistant" => self.assistant(value, true),
            "user" => self.user(value, true),
            _ => Vec::new(),
        };
        match parent_of(value) {
            Some(parent) => in_child(Some(parent), told),
            // A subagent's line that does not say whose it is would read as the parent's.
            None if sidechain => Vec::new(),
            None => told,
        }
    }

    fn control_response(&mut self, value: &Value) {
        let Some(response) = value.get("response") else {
            return;
        };
        let Some(id) = response.get("request_id").and_then(Value::as_str) else {
            return;
        };
        let answer = if response.get("subtype").and_then(Value::as_str) == Some("error") {
            Err(format!(
                "Claude Code refused the request: {}",
                response
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("no reason given")
            ))
        } else {
            if let Some(version) = response
                .pointer("/response/claude_code_version")
                .and_then(Value::as_str)
            {
                self.version = Some(version.to_string());
            }
            Ok(response.get("response").cloned().unwrap_or(Value::Null))
        };
        self.answers.push((id.to_string(), answer));
    }

    fn control_request(&mut self, value: &Value) -> Option<Event> {
        let id = value.get("request_id").and_then(Value::as_str)?.to_string();
        let request = value.get("request")?;
        if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
            let refusal = json!({"type": "control_response", "response": {"subtype": "error", "request_id": id,
                "error": "this client does not take that request"}});
            let _ = (self.writer)(&refusal);
            return None;
        }
        let name = request
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("a tool")
            .to_string();
        let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
        let tool_id = request
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut tool = tools::describe(
            tool_id.as_deref().unwrap_or(&id),
            &name,
            &input,
            self.limits.max_title_bytes,
        );
        tool.status = None;
        let display = request
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(&name);
        let title = request
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                tool.title
                    .clone()
                    .map(|title| format!("{display}: {title}"))
            })
            .unwrap_or_else(|| display.to_string());
        let description = request
            .get("decision_reason")
            .or_else(|| request.get("description"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let permit = self.mode_permit.clone();
        let suggestions = request
            .get("permission_suggestions")
            .and_then(Value::as_array)
            .map(|all| {
                all.iter()
                    .filter(|each| {
                        each.get("type").and_then(Value::as_str) != Some("setMode")
                            || each
                                .get("mode")
                                .and_then(Value::as_str)
                                .is_some_and(|mode| {
                                    permit.as_ref().is_none_or(|permit| permit(mode))
                                })
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .filter(|suggestions| !suggestions.is_empty());
        let mut options = vec![PermissionOption::new(
            ALLOW_ONCE,
            "Allow",
            OptionKind::AllowOnce,
        )];
        if let Some(label) = suggestions.as_deref().and_then(always_label) {
            options.push(PermissionOption::new(
                ALLOW_ALWAYS,
                label,
                OptionKind::AllowAlways,
            ));
        }
        options.push(PermissionOption::new(
            REJECT,
            "Reject",
            OptionKind::RejectOnce,
        ));
        let mut details = Map::new();
        details.insert("toolName".into(), json!(name));
        details.insert("toolUseId".into(), json!(tool_id));
        details.insert("input".into(), input.clone());
        let writer = Arc::clone(&self.writer);
        let hook = self.mode_hook.clone();
        let responder = Responder::new(move |outcome| {
            let mut switched: Option<String> = None;
            let answer = match outcome {
                Outcome::Selected(chosen) if chosen == ALLOW_ONCE => {
                    json!({"behavior": "allow", "updatedInput": input})
                }
                Outcome::Selected(chosen) if chosen == ALLOW_ALWAYS => {
                    let mut answer = json!({"behavior": "allow", "updatedInput": input});
                    if let Some(suggestions) = suggestions {
                        // The last `setMode` is the one Claude Code ends up in.
                        switched = suggestions
                            .iter()
                            .rev()
                            .find(|each| {
                                each.get("type").and_then(Value::as_str) == Some("setMode")
                            })
                            .and_then(|each| each.get("mode").and_then(Value::as_str))
                            .map(str::to_string);
                        answer["updatedPermissions"] = Value::Array(suggestions);
                    }
                    answer
                }
                Outcome::Cancelled => {
                    json!({"behavior": "deny", "message": "The person did not answer; the turn is stopping.", "interrupt": true})
                }
                _ => json!({"behavior": "deny", "message": "The person refused this."}),
            };
            let written = writer(
                &json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": answer}}),
            );
            if written.is_ok()
                && let (Some(mode), Some(hook)) = (&switched, &hook)
            {
                hook(mode);
            }
            written
        });
        Some(Event::Permission(
            PermissionRequest::new(tool, title, description, options, responder).details(details),
        ))
    }

    fn system(&mut self, value: &Value) -> Vec<Event> {
        let uuid = value
            .get("uuid")
            .and_then(Value::as_str)
            .unwrap_or("compaction")
            .to_string();
        if let (Some(mode), Some(hook)) = (
            value.get("permissionMode").and_then(Value::as_str),
            &self.mode_hook,
        ) {
            hook(mode);
        }
        match value.get("subtype").and_then(Value::as_str).unwrap_or("") {
            "init" => {
                if let Some(version) = value.get("claude_code_version").and_then(Value::as_str) {
                    self.version = Some(version.to_string());
                }
                Vec::new()
            }
            "compact_boundary" => {
                // The summary follows as a synthetic message of the person's; the compaction is told with it.
                let earlier = self.compaction.replace(uuid);
                earlier
                    .map(|id| vec![Event::Compaction { id, summary: None }])
                    .unwrap_or_default()
            }
            "task_started" | "task_progress" | "task_updated" | "task_notification" => {
                self.background_task(value)
            }
            "informational" | "notification" => value
                .get("content")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(|text| {
                    vec![Event::Notice {
                        severity: Severity::Info,
                        title: text.to_string(),
                    }]
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn stream_event(&mut self, value: &Value) -> Vec<Event> {
        let Some(event) = value.get("event") else {
            return Vec::new();
        };
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                self.streaming = event
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.blocks.clear();
                if let Some(usage) = event.pointer("/message/usage") {
                    self.context_used = limits::context_used(usage).or(self.context_used);
                }
                Vec::new()
            }
            "content_block_start" => {
                let Some(block) = event.get("content_block") else {
                    return Vec::new();
                };
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        self.blocks.insert(index, "text");
                        Vec::new()
                    }
                    "thinking" => {
                        self.blocks.insert(index, "thinking");
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            "content_block_delta" => {
                let Some(message) = self.block_message(index) else {
                    return Vec::new();
                };
                let delta = event.get("delta");
                let text = |key: &str| {
                    delta
                        .and_then(|delta| delta.get(key))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                };
                match delta
                    .and_then(|delta| delta.get("type"))
                    .and_then(Value::as_str)
                {
                    Some("text_delta") => text("text")
                        .map(|text| vec![Event::MessageChunk { message, text }])
                        .unwrap_or_default(),
                    Some("thinking_delta") => text("thinking")
                        .map(|text| vec![Event::ThoughtChunk { message, text }])
                        .unwrap_or_default(),
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => {
                let kind = self.blocks.get(&index).copied();
                let Some(message) = self.block_message(index) else {
                    return Vec::new();
                };
                match kind {
                    Some("text") => vec![Event::MessageEnd { message }],
                    Some("thinking") => vec![Event::ThoughtEnd { message }],
                    _ => Vec::new(),
                }
            }
            "message_stop" => {
                if let Some(message) = self.streaming.take() {
                    self.streamed.push_back(message);
                    while self.streamed.len() > self.limits.max_streamed {
                        self.streamed.pop_front();
                    }
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The id of the message a streamed text or thinking block belongs to: the message's, with the block's index.
    fn block_message(&self, index: u64) -> Option<String> {
        self.blocks.get(&index)?;
        let message = self.streaming.as_ref()?;
        Some(format!("{message}:{index}"))
    }

    fn assistant(&mut self, value: &Value, history: bool) -> Vec<Event> {
        let mut events = Vec::new();
        let message = value.get("message");
        let message_id = message
            .and_then(|message| message.get("id"))
            .and_then(Value::as_str)
            .or_else(|| value.get("uuid").and_then(Value::as_str))
            .unwrap_or("message")
            .to_string();
        if parent_of(value).is_none()
            && let Some(usage) = message.and_then(|message| message.get("usage"))
        {
            self.context_used = limits::context_used(usage).or(self.context_used);
        }
        let streamed = !history
            && (self.streamed.iter().any(|known| *known == message_id)
                || self.streaming.as_deref() == Some(&message_id));
        // An API error Claude Code writes as a message of its own: the turn's result says it again.
        let failed = value.get("error").is_some_and(|error| !error.is_null());
        let line = value
            .get("uuid")
            .and_then(Value::as_str)
            .unwrap_or(&message_id)
            .to_string();
        let blocks = message
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (index, block) in blocks.iter().enumerate() {
            let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
            let id = format!("{line}:{index}");
            match kind {
                "text" if !streamed && !failed => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        events.push(Event::MessageChunk {
                            message: id.clone(),
                            text: text.to_string(),
                        });
                        events.push(Event::MessageEnd { message: id });
                    }
                }
                "thinking" if !streamed => {
                    if let Some(text) = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                    {
                        events.push(Event::ThoughtChunk {
                            message: id.clone(),
                            text: text.to_string(),
                        });
                        events.push(Event::ThoughtEnd { message: id });
                    }
                }
                "tool_use" => events.extend(self.tool_use(block)),
                kind if kind.ends_with("_tool_use") => {
                    let name = block.get("name").and_then(Value::as_str).unwrap_or(kind);
                    if let Some(tool_id) = block.get("id").and_then(Value::as_str) {
                        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                        let mut call = tools::describe(
                            tool_id,
                            &format!("{kind}:{name}"),
                            &input,
                            self.limits.max_title_bytes,
                        );
                        call.status = Some(ToolStatus::InProgress);
                        events.push(Event::Tool(call));
                    }
                }
                _ => {}
            }
        }
        events
    }

    fn tool_use(&mut self, block: &Value) -> Vec<Event> {
        let (Some(id), Some(name)) = (
            block.get("id").and_then(Value::as_str),
            block.get("name").and_then(Value::as_str),
        ) else {
            return Vec::new();
        };
        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
        self.keep(id, name, &input);
        if name == "TodoWrite" {
            return tools::todo_plan(&input)
                .map(|plan| vec![Event::Plan(plan)])
                .unwrap_or_default();
        }
        if name.starts_with("Task") && name != "Task" {
            return Vec::new();
        }
        let call = tools::describe(id, name, &input, self.limits.max_title_bytes);
        let mut events = vec![Event::Tool(call)];
        if matches!(name, "Task" | "Agent") {
            let mut child = Child::new(id);
            child.parent_tool_call = Some(id.to_string());
            child.title = input
                .get("description")
                .and_then(Value::as_str)
                .map(|text| tools::cut(text, self.limits.max_title_bytes));
            child.task = input
                .get("prompt")
                .and_then(Value::as_str)
                .map(|text| self.child_text(text));
            child.state = Some(ChildState::Starting);
            events.push(Event::Child(child));
        }
        if tools::TERMINAL_TOOLS.contains(&name) {
            let mut terminal = Terminal::new(id);
            terminal.command = input
                .get("command")
                .and_then(Value::as_str)
                .map(str::to_string);
            events.push(Event::Terminal(terminal));
        }
        events
    }

    fn user(&mut self, value: &Value, history: bool) -> Vec<Event> {
        let mut events = Vec::new();
        let content = value.pointer("/message/content");
        let id = value
            .get("uuid")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .to_string();
        let flag = |key: &str| value.get(key).and_then(Value::as_bool) == Some(true);
        if flag("isCompactSummary") || (flag("isSynthetic") && self.compaction.is_some()) {
            let summary = tools::result_text(content);
            let id = self.compaction.take().unwrap_or(id);
            return vec![Event::Compaction {
                id,
                summary: Some(summary),
            }];
        }
        let replayed = value.get("isReplay").and_then(Value::as_bool) == Some(true);
        let meta = value.get("isMeta").and_then(Value::as_bool) == Some(true);
        let mut told = false;
        let blocks: Vec<Value> = match content {
            Some(Value::String(text)) => vec![json!({"type": "text", "text": text})],
            Some(Value::Array(blocks)) => blocks.clone(),
            _ => Vec::new(),
        };
        for block in &blocks {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "tool_result" => {
                    // The stream calls it `tool_use_result`, a transcript `toolUseResult`.
                    let result = value
                        .get("tool_use_result")
                        .or_else(|| value.get("toolUseResult"));
                    events.extend(self.tool_result(block, result));
                }
                "text" if history && !meta && !replayed => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    if !text.starts_with("<local-command-")
                        && !text.starts_with("<command-")
                        && !text.starts_with("[Request interrupted")
                    {
                        events.push(Event::UserChunk {
                            message: id.clone(),
                            content: Content::Text(text.to_string()),
                        });
                        told = true;
                    }
                }
                "image" if history && !meta => {
                    if let (Some(mime_type), Some(data)) = (
                        block.pointer("/source/media_type").and_then(Value::as_str),
                        block.pointer("/source/data").and_then(Value::as_str),
                    ) {
                        events.push(Event::UserChunk {
                            message: id.clone(),
                            content: Content::Image {
                                mime_type: mime_type.to_string(),
                                data: data.to_string(),
                            },
                        });
                        told = true;
                    }
                }
                _ => {}
            }
        }
        if told {
            events.push(Event::UserEnd { message: id });
        }
        events
    }

    fn tool_result(&mut self, block: &Value, result: Option<&Value>) -> Vec<Event> {
        let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
            return Vec::new();
        };
        let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
        let text = tools::result_text(block.get("content"));
        let Some(tool) = self.tools.get(id) else {
            let mut call = ToolCall::new(id);
            call.status = Some(if failed {
                ToolStatus::Failed
            } else {
                ToolStatus::Completed
            });
            call.output = block.get("content").cloned();
            return vec![Event::Tool(call)];
        };
        let name = tool.name.clone();
        let input = tool.input.clone();
        if name.starts_with("Task") && name != "Task" {
            return if failed {
                Vec::new()
            } else {
                self.task_plan(&name, &input, result, &text)
            };
        }
        if name == "TodoWrite" {
            return Vec::new();
        }
        let mut events = Vec::new();
        let mut call = ToolCall::new(id);
        call.status = Some(if failed {
            ToolStatus::Failed
        } else {
            ToolStatus::Completed
        });
        // What the model was given back, as it was given.
        call.output = block.get("content").cloned();
        let handed_back = if matches!(name.as_str(), "Task" | "Agent") {
            self.child_handed_back(id, failed, &text)
        } else {
            None
        };
        if tools::TERMINAL_TOOLS.contains(&name.as_str()) {
            let mut terminal = Terminal::new(id);
            let output = match result.filter(|result| result.is_object()) {
                Some(result) => {
                    let stdout = result.get("stdout").and_then(Value::as_str).unwrap_or("");
                    let stderr = result.get("stderr").and_then(Value::as_str).unwrap_or("");
                    if stderr.is_empty() {
                        stdout.to_string()
                    } else {
                        format!("{stdout}{stderr}")
                    }
                }
                None => text.clone(),
            };
            terminal.output = Some(output);
            terminal.exit = Some(if failed {
                tools::exit_code(&text)
            } else {
                Some(0)
            });
            events.push(Event::Terminal(terminal));
            if failed {
                call.content = Some(vec![
                    ToolContent::Terminal(id.to_string()),
                    ToolContent::Text(text),
                ]);
            }
        } else if let Some((diff, locations)) = result
            .filter(|_| !failed)
            .and_then(|result| tools::finished_diff(&name, result))
        {
            call.content = Some(vec![ToolContent::Diff(diff)]);
            call.locations = Some(locations);
        } else if failed {
            call.content = Some(vec![ToolContent::Text(text)]);
        } else if name != "Read" && !text.is_empty() {
            events.push(Event::ToolChunk {
                id: id.to_string(),
                content: ToolContent::Text(text),
            });
        }
        events.extend(handed_back);
        events.push(Event::Tool(call));
        events
    }

    /// The report of a finished Task or Agent call. One launched in the background returns at once and reports its end
    /// later, so its return is no end; and a result the task's own report already gave is kept.
    fn child_handed_back(&mut self, id: &str, failed: bool, text: &str) -> Option<Event> {
        if !failed && text.trim_start().starts_with("Async agent launched") {
            return None;
        }
        let mut child = Child::new(id);
        child.parent_tool_call = Some(id.to_string());
        child.state = Some(if failed {
            ChildState::Failed
        } else {
            ChildState::Completed
        });
        if self.child_done.insert(id.to_string()) || failed {
            child.result = Some(self.child_text(text));
        }
        Some(Event::Child(child))
    }

    fn child_text(&self, text: &str) -> String {
        coqui_acp_harness::lines::cut(text, self.limits.max_child_text_bytes).to_string()
    }

    /// The `task_*` lines Claude Code writes for a subagent; other background work has no `subagent_type`.
    fn background_task(&mut self, value: &Value) -> Vec<Event> {
        let text = |key: &str| value.get(key).and_then(Value::as_str);
        let Some(task) = text("task_id") else {
            return Vec::new();
        };
        let subagent = text("subagent_type").is_some() || text("task_type") == Some("local_agent");
        let known = self.child_tasks.get(task).cloned();
        if !subagent && known.is_none() {
            return Vec::new();
        }
        let Some(call) = text("tool_use_id").map(str::to_string).or(known) else {
            return Vec::new();
        };
        if self.max_tools > 0 {
            if self.child_tasks.len() >= self.max_tools && !self.child_tasks.contains_key(task) {
                self.child_tasks.pop_first();
            }
            self.child_tasks.insert(task.to_string(), call.clone());
        }
        let mut child = Child::new(call.clone());
        child.parent_tool_call = Some(call.clone());
        let usage = |counts: Option<&Value>| {
            counts.map(|counts| {
                let mut usage = ChildUsage::default();
                usage.total_tokens = counts.get("total_tokens").and_then(Value::as_u64);
                usage.tool_uses = counts.get("tool_uses").and_then(Value::as_u64);
                usage.duration_ms = counts.get("duration_ms").and_then(Value::as_u64);
                usage
            })
        };
        let state = |status: Option<&str>| match status? {
            "running" | "in_progress" | "pending" => Some(ChildState::Working),
            "completed" => Some(ChildState::Completed),
            "failed" | "error" | "errored" => Some(ChildState::Failed),
            "killed" | "stopped" | "cancelled" | "canceled" | "interrupted" => {
                Some(ChildState::Cancelled)
            }
            _ => None,
        };
        match value.get("subtype").and_then(Value::as_str).unwrap_or("") {
            "task_started" => {
                child.title =
                    text("description").map(|text| tools::cut(text, self.limits.max_title_bytes));
                child.task = text("prompt").map(|text| self.child_text(text));
                child.state = Some(ChildState::Working);
            }
            "task_progress" => {
                child.state = Some(ChildState::Working);
                child.usage = usage(value.get("usage"));
            }
            "task_updated" => {
                child.state = state(value.pointer("/patch/status").and_then(Value::as_str));
                if child.state.is_none() {
                    return Vec::new();
                }
            }
            _ => {
                child.state = state(text("status"));
                child.usage = usage(value.get("usage"));
                if let Some(summary) = text("summary") {
                    child.result = Some(self.child_text(summary));
                    self.child_done.insert(call);
                }
            }
        }
        vec![Event::Child(child)]
    }

    /// The plan the Task tools keep, after one of them finished.
    fn task_plan(
        &mut self,
        name: &str,
        input: &Value,
        result: Option<&Value>,
        text: &str,
    ) -> Vec<Event> {
        match name {
            "TaskCreate" => {
                let id = result
                    .and_then(|result| result.pointer("/task/id"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        let digits: String = text
                            .chars()
                            .skip_while(|c| !c.is_ascii_digit())
                            .take_while(char::is_ascii_digit)
                            .collect();
                        (!digits.is_empty()).then_some(digits)
                    })
                    .unwrap_or_else(|| (self.tasks.len() + 1).to_string());
                let subject = input
                    .get("subject")
                    .and_then(Value::as_str)
                    .unwrap_or("Task")
                    .to_string();
                self.tasks
                    .push((id, PlanEntry::new(subject, PlanStatus::Pending)));
                if self.tasks.len() > self.limits.max_tasks {
                    let over = self.tasks.len() - self.limits.max_tasks;
                    self.tasks.drain(..over);
                }
            }
            "TaskUpdate" => {
                let Some(task) = input.get("taskId").and_then(Value::as_str) else {
                    return Vec::new();
                };
                match input.get("status").and_then(Value::as_str) {
                    Some("deleted") => self.tasks.retain(|(id, _)| id != task),
                    status => {
                        if let Some((_, entry)) = self.tasks.iter_mut().find(|(id, _)| id == task) {
                            if let Some(status) = status.and_then(tools::plan_status) {
                                entry.status = status;
                            }
                            if let Some(subject) = input.get("subject").and_then(Value::as_str) {
                                entry.content = subject.to_string();
                            }
                        }
                    }
                }
            }
            _ => return Vec::new(),
        }
        vec![Event::Plan(
            self.tasks.iter().map(|(_, entry)| entry.clone()).collect(),
        )]
    }

    fn result(&mut self, value: &Value) -> Vec<Event> {
        let mut events = Vec::new();
        if let Some(id) = self.compaction.take() {
            events.push(Event::Compaction { id, summary: None });
        }
        if let Some(mut usage) = limits::usage(value) {
            usage.context_used = self.context_used;
            events.push(Event::Usage(usage));
        }
        let denied = limits::denials(value);
        if !denied.is_empty() {
            events.push(Event::Denied(denied));
        }
        let interrupted = core::mem::take(&mut self.interrupted);
        let rejected = self.rejected.take();
        self.running = false;
        if core::mem::take(&mut self.ended_by_error) {
            return events;
        }
        let end = if let Some(limit) = limits::limit(value) {
            TurnEnd::Limited(limits::with_hint(limit, rejected))
        } else if limits::is_error(value) {
            let aborted = value
                .get("terminal_reason")
                .and_then(Value::as_str)
                .is_some_and(|reason| reason.starts_with("aborted"));
            match value.get("subtype").and_then(Value::as_str) {
                _ if interrupted || aborted => TurnEnd::stopped(Stop::Cancelled),
                Some("error_max_turns") => TurnEnd::stopped(Stop::MaxTurnRequests),
                _ => match rejected {
                    Some(mut limit) => {
                        limit.text = Some(limits::error_text(value));
                        TurnEnd::Limited(limit)
                    }
                    None => TurnEnd::Failed(limits::error_text(value)),
                },
            }
        } else if interrupted && value.get("stop_reason").is_none_or(Value::is_null) {
            TurnEnd::stopped(Stop::Cancelled)
        } else {
            TurnEnd::Stopped {
                stop: match value.get("stop_reason").and_then(Value::as_str) {
                    Some("max_tokens") => Stop::MaxTokens,
                    Some("refusal") => Stop::Refusal,
                    _ => Stop::EndTurn,
                },
                result: value
                    .get("result")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }
        };
        events.push(Event::TurnEnd(end));
        events
    }
}

/// What applying `suggestions` does, every one of them and where it is kept, as the always-allow option's name;
/// `None` when none of them grants anything, so there is nothing to allow always.
/// The tool call of the subagent a line belongs to, when it is one of a subagent's own.
fn parent_of(value: &Value) -> Option<&str> {
    value
        .get("parent_tool_use_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

/// What a subagent's own line became, marked as the subagent's. A plan, usage or title is the parent's alone.
fn in_child(parent: Option<&str>, events: Vec<Event>) -> Vec<Event> {
    let Some(parent) = parent else {
        return events;
    };
    events
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                Event::MessageChunk { .. }
                    | Event::MessageEnd { .. }
                    | Event::ThoughtChunk { .. }
                    | Event::ThoughtEnd { .. }
                    | Event::Tool(_)
                    | Event::ToolChunk { .. }
                    | Event::Terminal(_)
            )
        })
        .map(|event| Event::InChild {
            child: parent.to_string(),
            parent_tool_call: Some(parent.to_string()),
            event: Box::new(event),
        })
        .collect()
}

fn always_label(suggestions: &[Value]) -> Option<String> {
    let mut parts = Vec::new();
    let mut grants = false;
    for suggestion in suggestions {
        let kept = match suggestion.get("destination").and_then(Value::as_str) {
            Some("session") => "for this session",
            Some("userSettings") => "in user settings",
            Some("projectSettings") => "in project settings",
            Some("localSettings") => "in local settings",
            Some("cliArg") => "for this run",
            _ => "where Claude Code keeps it",
        };
        let what = match suggestion.get("type").and_then(Value::as_str) {
            Some("setMode") => {
                let mode = suggestion
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("another");
                if mode != "default" {
                    grants = true;
                }
                match mode {
                    "acceptEdits" => "accept edits (acceptEdits)".to_string(),
                    other => format!("switch to {other} mode"),
                }
            }
            Some(kind @ ("addRules" | "replaceRules" | "removeRules")) => {
                let behavior = suggestion
                    .get("behavior")
                    .and_then(Value::as_str)
                    .unwrap_or("ask");
                if behavior == "allow" && kind != "removeRules" {
                    grants = true;
                }
                let rules: Vec<String> = suggestion
                    .get("rules")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|rule| {
                        let tool = rule.get("toolName").and_then(Value::as_str)?;
                        Some(match rule.get("ruleContent").and_then(Value::as_str) {
                            Some(content) => format!("{tool}({content})"),
                            None => tool.to_string(),
                        })
                    })
                    .collect();
                let verb = match kind {
                    "removeRules" => "stop applying",
                    _ => behavior,
                };
                format!("{verb} {}", rules.join(", "))
            }
            Some("addDirectories") => {
                grants = true;
                let directories: Vec<&str> = suggestion
                    .get("directories")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                format!("allow the directories {}", directories.join(", "))
            }
            Some(other) => format!("apply {other}"),
            None => "apply a change".to_string(),
        };
        parts.push(format!("{what} {kept}"));
    }
    grants.then(|| format!("Allow, and from now on: {}", parts.join("; ")))
}

/// The commands an `initialize` answer lists.
pub(crate) fn commands(answer: &Value) -> Vec<Command> {
    answer
        .get("commands")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|command| {
            let name = command.get("name").and_then(Value::as_str)?;
            let description = command
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let hint = command
                .get("argumentHint")
                .and_then(Value::as_str)
                .filter(|hint| !hint.is_empty())
                .map(str::to_string);
            Some(Command::new(name, description, hint))
        })
        .collect()
}
