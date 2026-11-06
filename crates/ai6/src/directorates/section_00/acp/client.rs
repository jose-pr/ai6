//! The ACP client: [`AcpSession`] drives one agent connection as an [`AgentSession`], over
//! `coqui_acp`. This module adds what is AI6's: the launch extension and its event stream, the
//! sequence numbers a reattaching Station acknowledges, what a restricted Session may use, and
//! the words of `limit`, `blocked` and `failed` events.

use coqui_acp::sdk::{Client as Role, ConnectTo};
use coqui_acp::{Builder, Client, Event, Session};
use guatiao::Status;
use guatiao::library::{Object, ProviderError};
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{
    EVENT_NOTIFICATION, EVENTS_CAPABILITY, PermissionDecider, PermissionDecision,
    PermissionRequest, RESTRICTED_KINDS, RefuseAll, UNBOUNDED_MS,
};
use coqui_acp_provider::{
    AgentEvent, AgentMessage, AgentSession, CANCELLED_EVENT, LIMIT_EVENT, TURN_CANCELLED,
    TokenUsage, UsageLimit,
};

/// The `_meta` key of `session/load` that asks a runner to replay what follows a sequence number.
const AFTER_KEY: &str = "dev.coqui.ai6/after";

/// The method of the notification that acknowledges a runner's event.
const ACK_NOTIFICATION: &str = "_ai6/ack";

/// How often a wait for the operation to start looks at the connection.
const START_POLL: Duration = Duration::from_millis(50);

/// An `authenticate` call to make before the first session request.
pub use coqui_acp::Authenticate;

/// How [`AcpSession::connect`] opens and drives a Session. Every number is a setting, none has
/// a floor.
pub struct AcpOptions {
    /// The agent's working directory (`session/new.cwd`).
    pub cwd: String,
    /// MCP servers for the session, as ACP's `mcpServers` entries.
    pub mcp_servers: Vec<Value>,
    /// The `_meta` of the session request (the launch extension, an adapter's options).
    pub meta: Map<String, Value>,
    /// The requested model, set through the agent's model config option.
    pub model: Option<String>,
    /// The requested reasoning effort, set through the agent's thought-level config option.
    pub effort: Option<String>,
    /// The config option id for the model; default: the option whose category is `model`,
    /// else `model`.
    pub model_option: Option<String>,
    /// The config option id for the effort; default: the option whose category is
    /// `thought_level`, else `effort`.
    pub effort_option: Option<String>,
    /// The first turn: sent as `session/prompt` once the session exists.
    pub prompt: Option<String>,
    /// A session to resume (`session/resume`, else `session/load`) instead of a new one.
    pub resume: Option<String>,
    /// Reattach to a live session by `session/load`, whatever else the agent offers, asking
    /// it to replay what follows this event sequence number (`0` for all it kept).
    pub reattach: Option<u64>,
    /// Whether the Session only reads and answers: every permission request for a tool that
    /// edits, deletes, moves or executes is refused, and a tool of those kinds that runs
    /// without leave ends the Session.
    pub restricted: bool,
    /// Sent before the session request, when the agent needs it.
    pub authenticate: Option<Authenticate>,
    /// How long `initialize` may take.
    pub initialize_timeout_ms: u64,
    /// How long a request other than `session/prompt` may take.
    pub request_timeout_ms: u64,
    /// How long the operation of a façade may take to start, after the first prompt; a
    /// start that fails is reported as soon as it does.
    pub start_timeout_ms: u64,
    /// How long `close` waits for the agent before ending its process.
    pub close_grace_ms: u64,
    /// Answers permission requests.
    pub decider: Arc<dyn PermissionDecider>,
    /// Whether dropping the session without closing it closes the agent's session. A Station
    /// reattaching to a runner's Session sets it off, so a lost connection ends nothing.
    pub close_on_drop: bool,
    /// Whether a connection that ends mid-turn without being closed is a `failed` event. A
    /// runner's Session is not failed by its link dropping: the Station attaches again.
    pub fail_on_disconnect: bool,
    /// What marks a failed prompt as a provider's usage or rate limit, found in its error without
    /// regard to case. Empty counts no failure as a limit. A lost connection is never one.
    pub limit_phrases: Vec<String>,
    /// Says what became of the agent, for that `failed` event: an exit status, its last words.
    pub disconnect_detail: Option<DisconnectDetail>,
    /// UTF-8 bytes of agent text gathered before they are delivered mid-message.
    pub text_flush_bytes: usize,
    /// The tool calls whose kind is kept, by which a later message about one that does not say
    /// its kind is judged; past it the oldest is forgotten, and such a message is of unknown kind.
    pub max_tool_kinds: usize,
    /// Answers the agent's extension notifications other than [`EVENT_NOTIFICATION`]; without
    /// one they are dropped.
    pub extensions: Option<Arc<dyn ExtensionHandler>>,
}

/// Asked at most once, when a connection has ended mid-turn without being closed.
pub type DisconnectDetail = Box<dyn FnOnce() -> Option<String> + Send>;

/// What a Session does with the agent's extension notifications (methods starting with `_`)
/// other than [`EVENT_NOTIFICATION`]. They are handed over as they are read, by whoever reads
/// the Session's events, so a handler returns at once and does its work elsewhere.
pub trait ExtensionHandler: Send + Sync {
    /// `method` and `params` as the agent sent them; `reply` notifies the agent back on the same
    /// connection for as long as the Session holds it.
    fn handle(&self, method: &str, params: &Value, reply: Notifier);
}

/// Sends notifications to an agent on the connection a Session holds; once the Session is
/// gone, nothing is sent.
#[derive(Clone)]
pub struct Notifier(Arc<SendNotification>);

/// Sends one notification and says whether the connection took it.
type SendNotification = dyn Fn(&str, Value) -> bool + Send + Sync;

impl Notifier {
    /// A notifier that hands each notification to `send`, which says whether it was taken.
    pub fn new(send: impl Fn(&str, Value) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(send))
    }

    fn of(session: &Arc<Session>) -> Self {
        let session = Arc::downgrade(session);
        Self::new(move |method, params| {
            session
                .upgrade()
                .is_some_and(|session| session.notify(method, params).is_ok())
        })
    }

    /// Sends `method` with `params`; whether the connection took it.
    pub fn notify(&self, method: &str, params: Value) -> bool {
        (self.0)(method, params)
    }
}

/// The default of [`AcpOptions::text_flush_bytes`]: 64 KiB.
pub const DEFAULT_TEXT_FLUSH_BYTES: usize = 64 * 1024;

/// The default of [`AcpOptions::max_tool_kinds`].
pub const DEFAULT_MAX_TOOL_KINDS: usize = 4096;

impl Default for AcpOptions {
    fn default() -> Self {
        Self {
            cwd: String::new(),
            mcp_servers: Vec::new(),
            meta: Map::new(),
            model: None,
            effort: None,
            model_option: None,
            effort_option: None,
            prompt: None,
            resume: None,
            reattach: None,
            restricted: false,
            authenticate: None,
            initialize_timeout_ms: 20_000,
            request_timeout_ms: 30_000,
            start_timeout_ms: 60_000,
            close_grace_ms: 2_000,
            decider: Arc::new(RefuseAll),
            close_on_drop: true,
            fail_on_disconnect: true,
            disconnect_detail: None,
            limit_phrases: Vec::new(),
            text_flush_bytes: DEFAULT_TEXT_FLUSH_BYTES,
            max_tool_kinds: DEFAULT_MAX_TOOL_KINDS,
            extensions: None,
        }
    }
}

/// A limit in milliseconds as `coqui_acp` takes it: [`UNBOUNDED_MS`] is none.
pub(crate) fn duration_of(ms: u64) -> Option<Duration> {
    (ms != UNBOUNDED_MS).then(|| Duration::from_millis(ms))
}

/// An event with the sequence number its agent spooled it under, when it has one.
type Queued = (Option<u64>, AgentEvent);

fn event(kind: &str, text: Option<String>) -> AgentEvent {
    AgentEvent {
        kind: kind.to_owned(),
        text,
        data: None,
        limit: None,
        usage: None,
        tools: Vec::new(),
        quota: Vec::new(),
    }
}

/// A turn stopped by a provider's limit, which names neither a reset time nor a window.
fn limit_event(text: String) -> AgentEvent {
    AgentEvent {
        limit: Some(UsageLimit {
            resets_at: None,
            window: None,
        }),
        ..event(LIMIT_EVENT, Some(text))
    }
}

/// Whether an event ends its turn: `blocked` does only as the agent's own cancel.
pub(crate) fn ends_turn(event: &AgentEvent) -> bool {
    turn_ending(&event.kind, event.text.as_deref())
}

fn turn_ending(kind: &str, text: Option<&str>) -> bool {
    matches!(
        kind,
        "completed" | "failed" | LIMIT_EVENT | CANCELLED_EVENT | "blown"
    ) || (kind == "blocked" && text == Some(TURN_CANCELLED))
}

/// Whether a notification of a facade's event stream carries an event that ends its turn.
fn notification_ends_turn(method: &str, params: &Value) -> bool {
    method == EVENT_NOTIFICATION
        && params.get("event").is_some_and(|event| {
            let field = |key: &str| event.get(key).and_then(Value::as_str);
            field("kind").is_some_and(|kind| turn_ending(kind, field("text")))
        })
}

/// Whether a restricted Session lets the agent have what it asked for: reading, and the tools of
/// the Session's own MCP servers. Nothing else, whatever its kind: the Grok CLI reports `monitor`,
/// which runs any command, as `other`.
fn restricted_allows(asked: &PermissionRequest, servers: &[String]) -> bool {
    if matches!(asked.kind.as_deref(), Some("read" | "search" | "think")) {
        return true;
    }
    let of_a_server = |name: &str, prefix: &str| {
        servers
            .iter()
            .any(|server| name.starts_with(&format!("{prefix}{server}__")))
    };
    // The Grok CLI names the tool in the input of its `UseTool`, and lets the model write the title
    // of a subagent; another agent has no such input and names it in the title.
    match asked
        .raw_input
        .as_ref()
        .and_then(|input| input.get("variant"))
    {
        Some(Value::String(variant)) => {
            variant == "UseTool"
                && asked
                    .raw_input
                    .as_ref()
                    .and_then(|input| input.get("tool_name"))
                    .and_then(Value::as_str)
                    .is_some_and(|name| of_a_server(name, ""))
        }
        _ => asked
            .title
            .as_deref()
            .is_some_and(|title| of_a_server(title, "mcp__")),
    }
}

/// The decider of a restricted Session. `coqui_acp` already refuses the kinds that change
/// something; this refuses the rest that is not reading or the Session's own tools.
struct Restricting {
    inner: Arc<dyn PermissionDecider>,
    servers: Vec<String>,
}

impl PermissionDecider for Restricting {
    fn decide(&self, request: &PermissionRequest) -> PermissionDecision {
        if restricted_allows(request, &self.servers) {
            self.inner.decide(request)
        } else {
            PermissionDecision::Deny(format!(
                "a restricted Session may not use a tool of the kind {}",
                request.kind.as_deref().unwrap_or("unknown")
            ))
        }
    }
}

/// What `next_event` and the events it reads share.
#[derive(Default)]
struct State {
    /// Read from the agent and not yet handed out.
    queue: VecDeque<Queued>,
    /// The identity the Station records: the provider Session ID a façade reports, else the
    /// ACP session ID.
    provider_session: Option<String>,
    /// The parameters of the notification that said the operation started.
    start_info: Option<Value>,
    /// The newest runner sequence number handed out by `next_event`.
    delivered: u64,
    /// The one handed out and not yet acknowledged: the caller acknowledges it by asking for
    /// the next, once it has dealt with it.
    unacked: Option<u64>,
    /// A completed, failed or limit event ended the current turn.
    finished: bool,
    /// This side asked the agent to cancel the current turn.
    cancelling: bool,
    /// A façade's `session/prompt` failed.
    prompt_failed: Option<String>,
}

impl State {
    fn push(&mut self, seq: Option<u64>, event: AgentEvent) {
        if ends_turn(&event) {
            self.finished = true;
        }
        self.queue.push_back((seq, event));
    }

    /// What a façade's `_ai6/event` notification says.
    fn extension(&mut self, params: &Value) {
        if let Some(started) = params.get("providerSessionId").and_then(Value::as_str) {
            self.provider_session = Some(started.to_owned());
        }
        match params.get("event") {
            Some(Value::Null) | None => self.start_info = Some(params.clone()),
            Some(raw) => match serde_json::from_value::<AgentEvent>(raw.clone()) {
                Ok(agent_event) => {
                    self.push(params.get("seq").and_then(Value::as_u64), agent_event);
                }
                Err(error) => self.push(
                    None,
                    event("notice", Some(format!("unreadable event: {error}"))),
                ),
            },
        }
    }
}

fn provider_error(message: impl AsRef<str>) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, message.as_ref())
}

/// One ACP connection to an agent, driven as an [`AgentSession`].
pub struct AcpSession {
    session: Arc<Session>,
    /// Whoever reads the agent's events holds this, as the single reader.
    reader: Mutex<()>,
    state: Mutex<State>,
    events_capability: bool,
    limit_phrases: Vec<String>,
    extensions: Option<Arc<dyn ExtensionHandler>>,
}

impl AcpSession {
    /// Connects over `transport`, opens (or resumes) the session and sends the first prompt.
    /// `on_close` runs when the Session ends or fails to open: it ends the agent's process.
    ///
    /// # Errors
    ///
    /// The agent's refusal or silence, in a sentence: an `initialize`, authentication or
    /// session error, an agent without the capability the Session needs, a start timeout.
    pub fn connect(
        transport: impl ConnectTo<Role> + 'static,
        mut options: AcpOptions,
        on_close: Option<Box<dyn FnOnce() + Send>>,
    ) -> Result<Self, ProviderError> {
        let waits =
            options.prompt.is_some() || (options.resume.is_some() && options.reattach.is_none());
        let start_timeout = duration_of(options.start_timeout_ms);
        let has_prompt = options.prompt.is_some();
        let mut session = Self::builder(&options);
        if let Some(detail) = options.disconnect_detail.take() {
            session = session.disconnect_detail(detail);
        }
        if let Some(on_close) = on_close {
            session = session.on_close(on_close);
        }
        let session = session
            .connect(transport)
            .map_err(|error| provider_error(error.to_string()))?;
        let events_capability = session.extension_active();
        let opened = Self {
            reader: Mutex::new(()),
            state: Mutex::new(State {
                provider_session: if events_capability {
                    None
                } else {
                    session.id()
                },
                ..State::default()
            }),
            session: Arc::new(session),
            events_capability,
            limit_phrases: options.limit_phrases,
            extensions: options.extensions,
        };
        if events_capability && waits {
            // A façade starts its operation on the first prompt, and a resumed one says it is
            // running before the Session is handed out; what the start says is the start's.
            opened
                .await_start(has_prompt, start_timeout)
                .map_err(provider_error)?;
        }
        Ok(opened)
    }

    fn builder(options: &AcpOptions) -> Builder {
        let mut builder = Client::builder(options.cwd.clone())
            .client_info("ai6", env!("CARGO_PKG_VERSION"))
            .turn_end_by(EVENTS_CAPABILITY)
            .ends_turn(notification_ends_turn)
            .initialize_timeout(duration_of(options.initialize_timeout_ms))
            .request_timeout(duration_of(options.request_timeout_ms))
            .close_grace(duration_of(options.close_grace_ms))
            // The reader is the Station, which keeps up; no bound on what waits for it.
            .max_pending_events(usize::MAX)
            .text_flush(options.text_flush_bytes)
            .max_tool_kinds(options.max_tool_kinds)
            .close_on_drop(options.close_on_drop)
            .fail_on_disconnect(options.fail_on_disconnect);
        for server in &options.mcp_servers {
            builder = builder.mcp_server(server.clone());
        }
        for (key, value) in &options.meta {
            builder = builder.meta(key.clone(), value.clone());
        }
        if let Some(model) = &options.model {
            builder = builder.model(model.clone());
        }
        if let Some(effort) = &options.effort {
            builder = builder.effort(effort.clone());
        }
        if let Some(id) = &options.model_option {
            builder = builder.model_option(id.clone());
        }
        if let Some(id) = &options.effort_option {
            builder = builder.effort_option(id.clone());
        }
        if let Some(prompt) = &options.prompt {
            builder = builder.prompt(prompt.clone());
        }
        if let Some(authenticate) = &options.authenticate {
            builder = builder.authenticate(authenticate.clone());
        }
        builder = match (&options.resume, options.reattach) {
            (Some(id), Some(after)) => {
                // `u64::MAX` asks the agent to continue from where it last heard from this side.
                if after != u64::MAX {
                    builder = builder.meta(AFTER_KEY, json!(after));
                }
                builder.load(id.clone())
            }
            (Some(id), None) => builder.resume(id.clone()),
            (None, _) => builder,
        };
        if options.restricted {
            builder = builder
                .refuse_kinds(RESTRICTED_KINDS)
                .decider(Arc::new(Restricting {
                    inner: options.decider.clone(),
                    servers: options
                        .mcp_servers
                        .iter()
                        .filter_map(|server| server.get("name").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect(),
                }));
        } else {
            builder = builder.decider(options.decider.clone());
        }
        builder
    }

    /// Reads events until the operation says it started. With a prompt, its failure is the
    /// answer; a resumed operation that never says is let through.
    fn await_start(&self, prompted: bool, timeout: Option<Duration>) -> Result<(), String> {
        let deadline = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
        let mut drained = false;
        loop {
            {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| "ACP state lock was poisoned")?;
                if let Some(failed) = &state.prompt_failed
                    && prompted
                {
                    return Err(failed.clone());
                }
                if state.start_info.is_some() {
                    return Ok(());
                }
            }
            if self.session.connection_lost() {
                // What the agent said before it left is read before its leaving: it may have
                // started, or said why it could not.
                if !drained {
                    while self.pump(Duration::ZERO) {}
                    drained = true;
                    continue;
                }
                return if prompted {
                    Err("the agent closed its connection before the operation started".to_owned())
                } else {
                    Ok(())
                };
            }
            let wait = match deadline {
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return if prompted {
                            Err(format!(
                                "the operation did not start within {} ms",
                                timeout.map_or(0, |timeout| timeout.as_millis())
                            ))
                        } else {
                            Ok(())
                        };
                    }
                    left.min(START_POLL)
                }
                None => START_POLL,
            };
            self.pump(wait);
        }
    }

    /// Reads one event from the agent, if one comes within `wait`, and turns it into what the
    /// Station hears.
    fn pump(&self, wait: Duration) -> bool {
        let Some(event) = self.session.next_event(wait) else {
            return false;
        };
        self.absorb(event);
        true
    }

    /// Drains what has arrived, when no one else is reading.
    fn catch_up(&self) {
        if let Ok(_reader) = self.reader.try_lock() {
            while self.pump(Duration::ZERO) {}
        }
    }

    fn is_limit(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.limit_phrases
            .iter()
            .any(|phrase| text.contains(&phrase.to_lowercase()))
    }

    fn absorb(&self, incoming: Event) {
        if let Event::Extension { method, params } = &incoming
            && method != EVENT_NOTIFICATION
        {
            if let Some(handler) = &self.extensions {
                handler.handle(method, params, Notifier::of(&self.session));
            }
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        match incoming {
            Event::Text(text) => state.push(None, event("text", Some(text))),
            Event::Tool { title, name, .. } => {
                let mut tool = event("tool", Some(title.clone()));
                tool.tools.push(name.unwrap_or(title));
                state.push(None, tool);
            }
            Event::Cost { amount, currency } => {
                if currency.eq_ignore_ascii_case("USD") {
                    let mut report = event("usage", None);
                    report.usage = Some(TokenUsage {
                        cost_micro_usd: (amount * 1_000_000.0).round().max(0.0) as u64,
                        cumulative: true,
                        ..TokenUsage::default()
                    });
                    state.push(None, report);
                } else {
                    state.push(
                        None,
                        event(
                            "notice",
                            Some(format!(
                                "the agent reported its cost in {currency}, which is not converted"
                            )),
                        ),
                    );
                }
            }
            Event::Notice(text) => state.push(None, event("notice", Some(text))),
            Event::Permission {
                tool,
                allowed,
                reason,
            } => {
                let text = if allowed {
                    format!("permission granted for {tool}")
                } else {
                    format!(
                        "permission refused for {tool}: {}",
                        reason.unwrap_or_default()
                    )
                };
                state.push(None, event("permission", Some(text)));
            }
            Event::Completed { .. } => state.push(None, event("completed", None)),
            Event::Failed(text) => {
                // A lost connection's failure may quote the agent's last words, which are no
                // provider's answer: only a turn's own failure counts as a limit.
                let failed = if !self.events_capability
                    && !self.session.connection_lost()
                    && self.is_limit(&text)
                {
                    limit_event(text)
                } else {
                    event("failed", Some(text))
                };
                state.push(None, failed);
            }
            Event::Cancelled => {
                // An interrupted turn ends as a native handler's does; one the agent gave up by
                // itself is another matter.
                if state.cancelling {
                    state.push(None, event("completed", None));
                } else {
                    state.push(
                        None,
                        event(CANCELLED_EVENT, Some(TURN_CANCELLED.to_owned())),
                    );
                }
            }
            Event::OutputLost { lines, bytes } => state.push(
                None,
                event(
                    "failed",
                    Some(format!(
                        "the agent's output was lost while no one was connected ({lines} lines, \
                         {bytes} bytes), so what this Session shows has a gap"
                    )),
                ),
            ),
            Event::Blocked(text) => state.push(
                None,
                event(
                    "blocked",
                    Some(format!("a permission request was refused: {text}")),
                ),
            ),
            Event::PromptFailed(text) => {
                state.prompt_failed = Some(text.clone());
                if !state.finished {
                    state.push(None, event("failed", Some(text)));
                }
            }
            Event::Extension { method, params } if method == EVENT_NOTIFICATION => {
                state.extension(&params);
            }
            _ => {}
        }
    }

    fn take(&self) -> Result<Option<AgentEvent>, ProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| provider_error("ACP state lock was poisoned"))?;
        Ok(state.queue.pop_front().map(|(seq, event)| {
            if let Some(seq) = seq {
                state.delivered = state.delivered.max(seq);
                state.unacked = Some(seq);
            }
            event
        }))
    }

    /// Acknowledges the event last handed out, if it has not been: asking for another says
    /// the caller is done with it.
    fn acknowledge_taken(&self) {
        let taken = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.unacked.take());
        if let (Some(seq), Some(id)) = (taken, self.session.id()) {
            let _ = self
                .session
                .notify(ACK_NOTIFICATION, json!({"sessionId": id, "seq": seq}));
        }
    }
}

impl AgentSession for AcpSession {
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        self.catch_up();
        Ok(self
            .state
            .lock()
            .map_err(|_| provider_error("ACP state lock was poisoned"))?
            .provider_session
            .clone())
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        if let Ok(mut state) = self.state.lock() {
            state.finished = false;
            state.cancelling = false;
        }
        self.session
            .send(message.text)
            .map_err(|_| provider_error("the ACP connection is closed"))
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        let _reader = self
            .reader
            .lock()
            .map_err(|_| provider_error("ACP event lock was poisoned"))?;
        self.acknowledge_taken();
        let wait = Duration::from_millis(timeout_ms.max(0).unsigned_abs());
        let deadline = Instant::now().checked_add(wait);
        loop {
            if let Some(event) = self.take()? {
                return Ok(Some(event));
            }
            let left = deadline.map_or(wait, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            if !self.pump(left) {
                return self.take();
            }
        }
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        if let Ok(mut state) = self.state.lock() {
            state.cancelling = true;
        }
        self.session
            .cancel()
            .map_err(|_| provider_error("the ACP connection is closed"))
    }

    fn close(&self) -> Result<(), ProviderError> {
        // What was taken is acknowledged, so nothing is left to replay.
        self.acknowledge_taken();
        self.session.close();
        Ok(())
    }
}

impl AcpSession {
    /// The parameters of the notification that said the operation started: the provider
    /// Session ID and whatever else the agent told (a runner names the terminal it chose).
    pub fn start_info(&self) -> Option<Value> {
        self.catch_up();
        self.state
            .lock()
            .ok()
            .and_then(|state| state.start_info.clone())
    }

    /// The newest runner sequence number `next_event` has handed out; `0` before any.
    pub fn last_seq(&self) -> u64 {
        self.state
            .lock()
            .map(|state| state.delivered)
            .unwrap_or_default()
    }

    /// Whether the connection ended without this side closing it: the agent, or the link to
    /// it, is gone.
    pub fn connection_lost(&self) -> bool {
        self.session.connection_lost()
    }

    /// This session as the object handle a Station holds.
    pub fn into_handle(self) -> Object<dyn AgentSession> {
        self.into_object()
    }
}
