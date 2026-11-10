//! What a harness reports, in no protocol's terms.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use std::io;

use coqui_acp_api::usage::{Bucket, LimitHit, Spend};
use serde_json::Value;

/// A block of a prompt, or of a message replayed from history.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Content {
    /// Text.
    Text(String),
    /// An image, as base64 data with its media type.
    Image {
        /// Its media type, such as `image/png`.
        mime_type: String,
        /// Its bytes, base64-encoded.
        data: String,
    },
    /// A reference to a resource the agent may read itself.
    ResourceLink {
        /// Its URI.
        uri: String,
        /// A name for people.
        name: String,
    },
    /// A resource's content, given with the prompt.
    Resource {
        /// Its URI.
        uri: String,
        /// Its media type, when known.
        mime_type: Option<String>,
        /// Its text, for a text resource.
        text: Option<String>,
        /// Its bytes, base64-encoded, for any other.
        blob: Option<String>,
    },
}

/// A text resource as prompt text: `<context ref="uri">` around it, the address escaped as an attribute and every
/// `</context` (any case) of the text written `&lt;/context`, so a resource cannot close the block that holds it.
#[must_use]
pub fn context_block(uri: &str, text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut kept = String::with_capacity(text.len());
    let mut from = 0;
    while let Some(at) = lower[from..].find("</context") {
        kept.push_str(&text[from..from + at]);
        kept.push_str("&lt;");
        from += at + 1;
    }
    kept.push_str(&text[from..]);
    let address = uri
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    alloc::format!("<context ref=\"{address}\">\n{kept}\n</context>")
}

/// What a tool does, as ACP names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolKind {
    /// Reads files or data.
    Read,
    /// Changes files.
    Edit,
    /// Deletes files.
    Delete,
    /// Moves or renames files.
    Move,
    /// Searches.
    Search,
    /// Runs a command.
    Execute,
    /// Thinks or plans, or runs a subagent.
    Think,
    /// Fetches from the network.
    Fetch,
    /// Changes the session's mode.
    SwitchMode,
    /// Anything else.
    Other,
}

impl ToolKind {
    /// The kind's name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Edit => "edit",
            Self::Delete => "delete",
            Self::Move => "move",
            Self::Search => "search",
            Self::Execute => "execute",
            Self::Think => "think",
            Self::Fetch => "fetch",
            Self::SwitchMode => "switch_mode",
            Self::Other => "other",
        }
    }
}

/// Where a tool call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolStatus {
    /// Announced, not started: waiting for its input or a permission.
    Pending,
    /// Running.
    InProgress,
    /// Done.
    Completed,
    /// Failed or was refused.
    Failed,
    /// Stopped before it finished.
    Cancelled,
}

impl ToolStatus {
    /// The status's name on the wire. ACP version 1 has no `cancelled`; it reads `failed` there.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A place a tool call touches.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Location {
    /// The file.
    pub path: String,
    /// The line, from 1.
    pub line: Option<u64>,
}

impl Location {
    /// A location in `path`, at `line` when given.
    #[must_use]
    pub fn new(path: impl Into<String>, line: Option<u64>) -> Self {
        Self {
            path: path.into(),
            line,
        }
    }
}

/// How a file changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileChange {
    /// It is created.
    Add,
    /// It is changed.
    Modify,
    /// It is deleted.
    Delete,
}

/// A change to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diff {
    /// The file.
    pub path: String,
    /// How it changes.
    pub change: FileChange,
    /// The text replaced, when known: a snippet or the whole file. `None` for a new file.
    pub old_text: Option<String>,
    /// The text that replaces it, when known.
    pub new_text: Option<String>,
    /// The change as a git patch, when the harness gave its hunks.
    pub patch: Option<String>,
}

impl Diff {
    /// A change to `path`, with no texts and no patch.
    #[must_use]
    pub fn new(path: impl Into<String>, change: FileChange) -> Self {
        Self {
            path: path.into(),
            change,
            old_text: None,
            new_text: None,
            patch: None,
        }
    }
}

/// What a tool call shows.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolContent {
    /// Text, such as its output.
    Text(String),
    /// A change to a file.
    Diff(Diff),
    /// A terminal of the agent's own, by its id, reported by [`Event::Terminal`].
    Terminal(String),
}

/// A tool call, announced or changed. A field left `None` is unchanged; `content` and `locations` replace the whole
/// list.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ToolCall {
    /// Its id, unique in the session.
    pub id: String,
    /// The harness's name for the tool.
    pub name: Option<String>,
    /// A title for people.
    pub title: Option<String>,
    /// What it does.
    pub kind: Option<ToolKind>,
    /// Where it is.
    pub status: Option<ToolStatus>,
    /// Its input, as the harness gave it: untrusted.
    pub input: Option<Value>,
    /// Its output, as the harness gave it.
    pub output: Option<Value>,
    /// What it touches.
    pub locations: Option<Vec<Location>>,
    /// What it shows.
    pub content: Option<Vec<ToolContent>>,
}

impl ToolCall {
    /// A change to the tool call `id`, with nothing set.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }
}

/// A terminal of the agent's own: a command it runs, and what it writes. Fields left `None` are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Terminal {
    /// Its id, unique in the session.
    pub id: String,
    /// The command.
    pub command: Option<String>,
    /// Output to add.
    pub output: Option<String>,
    /// Set once the command has ended, with its exit code when known.
    pub exit: Option<Option<i32>>,
}

impl Terminal {
    /// A change to the terminal `id`, with nothing set.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }
}

/// Where a plan's step is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlanStatus {
    /// Not started.
    Pending,
    /// Being done.
    InProgress,
    /// Done.
    Completed,
}

impl PlanStatus {
    /// The status's name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

/// One step of the agent's plan.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlanEntry {
    /// What the step is.
    pub content: String,
    /// Where it is.
    pub status: PlanStatus,
}

impl PlanEntry {
    /// A step.
    #[must_use]
    pub fn new(content: impl Into<String>, status: PlanStatus) -> Self {
        Self {
            content: content.into(),
            status,
        }
    }
}

/// Tokens, cost and context, as far as the harness reports them.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Usage {
    /// Input tokens not read from a cache.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Input tokens read from a cache.
    pub cache_read: u64,
    /// Input tokens written to a cache.
    pub cache_write: u64,
    /// Whether the counts are running totals rather than this report's own.
    pub cumulative: bool,
    /// Whether it is another thread's, such as a sub-agent's: spent, and no part of this conversation's context.
    pub elsewhere: bool,
    /// What the session has cost so far, in US dollars.
    pub cost_usd: Option<f64>,
    /// Tokens of the model's context in use.
    pub context_used: Option<u64>,
    /// The size of the model's context, in tokens.
    pub context_size: Option<u64>,
    /// What the session has spent so far, as running totals; the session's id is filled in by the
    /// connection, so a harness leaves it empty.
    pub spend: Option<Spend>,
}

/// A command the person can give the agent by name.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Command {
    /// Its name, without the leading slash.
    pub name: String,
    /// What it does.
    pub description: String,
    /// What it takes, when it takes anything.
    pub hint: Option<String>,
}

impl Command {
    /// A command.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        hint: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            hint,
        }
    }
}

/// How serious a notice is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Severity {
    /// For information.
    Info,
    /// Something to act on.
    Warning,
    /// Something failed.
    Error,
}

impl Severity {
    /// The severity's name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Why a turn stopped, when it did its work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stop {
    /// The model finished.
    EndTurn,
    /// The model ran out of output tokens.
    MaxTokens,
    /// The turn made as many requests as it may.
    MaxTurnRequests,
    /// The model refused to go on.
    Refusal,
    /// The turn was interrupted.
    Cancelled,
}

impl Stop {
    /// The stop reason's name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EndTurn => "end_turn",
            Self::MaxTokens => "max_tokens",
            Self::MaxTurnRequests => "max_turn_requests",
            Self::Refusal => "refusal",
            Self::Cancelled => "cancelled",
        }
    }
}

/// How a turn ended.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum TurnEnd {
    /// It stopped, for this reason.
    Stopped {
        /// Why.
        stop: Stop,
        /// The text the harness gives as the turn's result, when it gives one.
        result: Option<String>,
    },
    /// It failed, and why, in the harness's words.
    Failed(String),
    /// A quota refused it.
    Limited(LimitHit),
}

impl TurnEnd {
    /// A turn that stopped for this reason, with no result text.
    #[must_use]
    pub const fn stopped(stop: Stop) -> Self {
        Self::Stopped { stop, result: None }
    }

    /// Why it stopped, when it did.
    #[must_use]
    pub const fn stop(&self) -> Option<Stop> {
        match self {
            Self::Stopped { stop, .. } => Some(*stop),
            _ => None,
        }
    }
}

/// What the person may answer a permission request with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OptionKind {
    /// Allow this call.
    AllowOnce,
    /// Allow this call and others like it from now on.
    AllowAlways,
    /// Refuse this call.
    RejectOnce,
    /// Refuse this call and others like it from now on.
    RejectAlways,
}

impl OptionKind {
    /// The kind's name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AllowOnce => "allow_once",
            Self::AllowAlways => "allow_always",
            Self::RejectOnce => "reject_once",
            Self::RejectAlways => "reject_always",
        }
    }
}

/// One answer a permission request offers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PermissionOption {
    /// Its id, which the answer names.
    pub id: String,
    /// Its label for people.
    pub name: String,
    /// What it means.
    pub kind: OptionKind,
}

impl PermissionOption {
    /// An option.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: OptionKind) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
        }
    }
}

/// The answer to a permission request.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// The option of this id was chosen.
    Selected(String),
    /// Nobody chose: the turn is being stopped, or the question could not be put.
    Cancelled,
    /// The client answered, but chose no option that was offered: an error, an outcome that is none of the protocol's,
    /// or an option that was never put. A refusal of this one request, not an end of the turn.
    Refused,
}

/// Sends the answer to a permission request to the harness, once. One dropped unanswered answers
/// [`Outcome::Cancelled`], so the harness is never left waiting.
pub struct Responder(Option<Box<dyn FnOnce(Outcome) -> io::Result<()> + Send>>);

impl Responder {
    /// A responder that runs `answer` with the outcome.
    #[must_use]
    pub fn new(answer: impl FnOnce(Outcome) -> io::Result<()> + Send + 'static) -> Self {
        Self(Some(Box::new(answer)))
    }

    /// Answers. An id no option has is the harness's to judge; [`crate::acp`] sends only an offered one.
    ///
    /// # Errors
    ///
    /// The answer could not reach the harness.
    pub fn answer(mut self, outcome: Outcome) -> io::Result<()> {
        match self.0.take() {
            Some(answer) => answer(outcome),
            None => Ok(()),
        }
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if let Some(answer) = self.0.take() {
            let _ = answer(Outcome::Cancelled);
        }
    }
}

impl fmt::Debug for Responder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Responder")
    }
}

/// A tool call the harness asks leave for.
#[derive(Debug)]
#[non_exhaustive]
pub struct PermissionRequest {
    /// The call, as far as it is known; its id is that of a call reported before or here.
    pub tool: ToolCall,
    /// The question's title for people.
    pub title: String,
    /// More about what the call does, when the harness says.
    pub description: Option<String>,
    /// The answers offered, in order.
    pub options: Vec<PermissionOption>,
    /// Where the answer goes.
    pub responder: Responder,
    /// The harness's own fields of the request, for a host that decides by them; no client sees them.
    pub details: serde_json::Map<String, Value>,
}

impl PermissionRequest {
    /// The same with the harness's own fields.
    #[must_use]
    pub fn details(mut self, details: serde_json::Map<String, Value>) -> Self {
        self.details = details;
        self
    }

    /// A request.
    #[must_use]
    pub fn new(
        tool: ToolCall,
        title: impl Into<String>,
        description: Option<String>,
        options: Vec<PermissionOption>,
        responder: Responder,
    ) -> Self {
        Self {
            tool,
            title: title.into(),
            description,
            options,
            responder,
            details: serde_json::Map::new(),
        }
    }
}

/// What a harness reports. Message ids tie chunks to their message; a harness that gives none gets one from the
/// driver.
#[derive(Debug)]
#[non_exhaustive]
pub enum Event {
    /// The harness's id for the session, once it has said it.
    SessionId(String),
    /// Part of a message of the person's, when history is replayed.
    UserChunk {
        /// The message's id.
        message: String,
        /// The part.
        content: Content,
    },
    /// The end of a message of the person's.
    UserEnd {
        /// The message's id.
        message: String,
    },
    /// Agent text.
    MessageChunk {
        /// The message's id.
        message: String,
        /// The text.
        text: String,
    },
    /// The end of an agent message.
    MessageEnd {
        /// The message's id.
        message: String,
    },
    /// The agent's reasoning.
    ThoughtChunk {
        /// The message's id.
        message: String,
        /// The text.
        text: String,
    },
    /// The end of a piece of reasoning.
    ThoughtEnd {
        /// The message's id.
        message: String,
    },
    /// A tool call, announced or changed.
    Tool(ToolCall),
    /// Content to add to a tool call.
    ToolChunk {
        /// The tool call's id.
        id: String,
        /// The content.
        content: ToolContent,
    },
    /// A terminal of the agent's own.
    Terminal(Terminal),
    /// The agent's plan, whole.
    Plan(Vec<PlanEntry>),
    /// A tool call that needs leave.
    Permission(PermissionRequest),
    /// Tokens, cost and context.
    Usage(Usage),
    /// The account's allowances, each a bucket named by its id: one replaces the earlier of its id,
    /// and a field it leaves unknown keeps what was known.
    Buckets(Vec<Bucket>),
    /// The commands the person can give.
    Commands(Vec<Command>),
    /// The session's choices as they stand now: a client is told of every change not its own.
    Options(Vec<crate::acp::ConfigOption>),
    /// The session's title.
    Title(String),
    /// Something to tell the person that is not part of the conversation.
    Notice {
        /// How serious.
        severity: Severity,
        /// What to say.
        title: String,
    },
    /// The harness compacted the conversation: its summary, when it gave one.
    Compaction {
        /// The compaction's id.
        id: String,
        /// The summary.
        summary: Option<String>,
    },
    /// Tools the harness refused by its own rules in this turn, by name.
    Denied(Vec<String>),
    /// The end of a turn.
    TurnEnd(TurnEnd),
    /// A line the harness wrote that was not protocol.
    Output(String),
    /// The harness has exited, and how, with what it wrote last.
    Exited(String),
}
