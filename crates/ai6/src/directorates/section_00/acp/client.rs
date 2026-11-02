//! The ACP client: [`AcpSession`] drives one agent connection as an [`AgentSession`].

use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeResponse, NewSessionResponse, PermissionOptionKind,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOptions, SessionNotification, SessionUpdate, ToolCallStatus, ToolKind,
};
use agent_client_protocol::{
    Client, ConnectTo, ConnectionTo, Error, Responder, UntypedMessage, on_receive_notification,
    on_receive_request,
};
use futures::StreamExt;
use futures::channel::{mpsc as async_mpsc, oneshot};
use futures::future::{self, Either};
use guatiao::Status;
use guatiao::library::{Object, ProviderError};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{EVENT_NOTIFICATION, EVENTS_CAPABILITY, RESTRICTED_KINDS, UNBOUNDED_MS};
use crate::directorates::m::identity::SessionId;
use crate::directorates::section_00::agent::{
    AgentEvent, AgentMessage, AgentSession, LIMIT_EVENT, TURN_CANCELLED, TokenUsage, UsageLimit,
};

/// One way a permission request can be answered, as the agent offered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    /// The agent's identifier for the option.
    pub option_id: String,
    /// `allow_once`, `allow_always`, `reject_once` or `reject_always`.
    pub kind: String,
}

/// An agent asking leave to run a tool: the request a [`PermissionDecider`] judges.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PermissionRequest {
    /// The ACP session the request belongs to.
    pub session_id: String,
    /// The agent's identifier of the tool call.
    pub tool_call_id: String,
    /// The tool's title, when the agent gave one.
    pub title: Option<String>,
    /// The tool kind (`read`, `edit`, `execute`, ...), when the agent gave one.
    pub kind: Option<String>,
    /// The tool's input as the agent sent it (a command line, a path).
    pub raw_input: Option<Value>,
    /// What the agent lets the client answer.
    pub options: Vec<PermissionOption>,
}

/// What a [`PermissionDecider`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Let the tool run, once.
    Allow,
    /// Refuse it, for the reason given.
    Deny(String),
}

/// Judges the permission requests of an external agent. The Station answers with its
/// green-ink gate; a caller that sets none refuses everything.
pub trait PermissionDecider: Send + Sync {
    /// Decides one request. It may block (the gate waits for the operator).
    fn decide(&self, request: &PermissionRequest) -> PermissionDecision;
}

/// The decider that refuses every request.
#[derive(Debug, Default, Clone, Copy)]
pub struct RefuseAll;

impl PermissionDecider for RefuseAll {
    fn decide(&self, _request: &PermissionRequest) -> PermissionDecision {
        PermissionDecision::Deny("no permission decider is configured".to_owned())
    }
}

/// An `authenticate` call to make before the first session request.
#[derive(Debug, Clone)]
pub struct Authenticate {
    /// The method id the agent advertised.
    pub method: String,
    /// The request's `_meta` (a bearer token, for a field runner).
    pub meta: Map<String, Value>,
}

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
    /// Whether a connection that ends without being closed is a `failed` event. A runner's
    /// Session is not failed by its link dropping: the Station attaches again.
    pub fail_on_disconnect: bool,
    /// What marks a failed prompt as a provider's usage or rate limit, found in its error without
    /// regard to case. Empty counts no failure as a limit.
    pub limit_phrases: Vec<String>,
    /// Says what became of the agent, for that `failed` event: an exit status, its last words.
    pub disconnect_detail: Option<DisconnectDetail>,
}

/// Asked once a connection has ended without being closed.
pub type DisconnectDetail = Box<dyn Fn() -> Option<String> + Send + Sync>;

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
        }
    }
}

enum Command {
    Prompt(String),
    Ack(u64),
    Cancel,
    Close(mpsc::Sender<()>),
    Abort,
}

enum Lifecycle {
    Started,
    PromptFailed(String),
}

/// What the notification handlers share with the connection's main task.
/// An event with the sequence number its agent spooled it under, when it has one.
type Queued = (Option<u64>, AgentEvent);

struct Shared {
    events: mpsc::Sender<Queued>,
    lost: AtomicBool,
    commands: async_mpsc::UnboundedSender<Command>,
    lifecycle: async_mpsc::UnboundedSender<Lifecycle>,
    decider: Arc<dyn PermissionDecider>,
    restricted: bool,
    /// The names of the Session's MCP servers, whose tools a restricted Session may use.
    servers: Vec<String>,
    close_on_drop: bool,
    fail_on_disconnect: bool,
    limit_phrases: Vec<String>,
    closing: AtomicBool,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The identity the Station records: the provider Session ID a façade reports, else the
    /// ACP session ID.
    provider_session: Option<String>,
    acp_session: Option<String>,
    events_capability: bool,
    /// Replayed history of `session/load` is dropped.
    suppress: bool,
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
    text: String,
    kinds: HashMap<String, String>,
}

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

fn is_terminal(kind: &str) -> bool {
    matches!(kind, "completed" | "failed" | LIMIT_EVENT | "blown")
}

impl Shared {
    /// Whether a failed turn's error words a provider's limit, by [`AcpOptions::limit_phrases`].
    fn is_limit(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.limit_phrases
            .iter()
            .any(|phrase| text.contains(&phrase.to_lowercase()))
    }

    fn emit(&self, event: AgentEvent) {
        if let Ok(mut inner) = self.inner.lock() {
            if is_terminal(&event.kind) {
                inner.finished = true;
            }
            self.flush_locked(&mut inner);
        }
        let _ = self.events.send((None, event));
    }

    /// An event a runner spooled under `seq`: delivered with it, so taking it can be acknowledged.
    fn emit_seq(&self, seq: u64, event: AgentEvent) {
        if let Ok(mut inner) = self.inner.lock() {
            if is_terminal(&event.kind) {
                inner.finished = true;
            }
            self.flush_locked(&mut inner);
        }
        let _ = self.events.send((Some(seq), event));
    }

    fn flush_locked(&self, inner: &mut Inner) {
        if !inner.text.is_empty() {
            let text = std::mem::take(&mut inner.text);
            let _ = self.events.send((None, event("text", Some(text))));
        }
    }

    fn flush(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            self.flush_locked(&mut inner);
        }
    }

    fn begin_turn(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.finished = false;
            inner.cancelling = false;
        }
    }

    fn asked_to_cancel(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.cancelling = true;
        }
    }

    fn cancelling(&self) -> bool {
        self.inner.lock().is_ok_and(|inner| inner.cancelling)
    }

    fn finished(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.finished)
            .unwrap_or(true)
    }

    fn events_capability(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.events_capability)
            .unwrap_or(false)
    }

    /// The agent's ending of a turn, as `session/prompt` answered it, for an agent that does
    /// not send [`EVENT_NOTIFICATION`].
    fn turn_ended(&self, result: Result<Value, Error>) {
        self.flush();
        // A turn cut off by the connection ending is the connection's to report (lost, or
        // failed when the caller asked for that), not the turn's.
        if let Err(error) = &result
            && (agent_client_protocol::is_incoming_transport_closed(error)
                || agent_client_protocol::is_connection_terminated(error))
        {
            return;
        }
        if self.events_capability() {
            if let Err(error) = result {
                let _ = self
                    .lifecycle
                    .unbounded_send(Lifecycle::PromptFailed(error_text(&error)));
                if !self.finished() {
                    self.emit(event("failed", Some(error_text(&error))));
                }
            }
            return;
        }
        let ended = match result {
            Ok(value) => match value.get("stopReason").and_then(Value::as_str) {
                Some("end_turn" | "max_tokens" | "max_turn_requests") => event("completed", None),
                Some("refusal") => {
                    event("failed", Some("the agent refused to continue".to_owned()))
                }
                // An interrupted turn ends as a native handler's does; one the agent gave up
                // by itself is another matter.
                Some("cancelled") if self.cancelling() => event("completed", None),
                Some("cancelled") => event("blocked", Some(TURN_CANCELLED.to_owned())),
                other => event(
                    "failed",
                    Some(format!("the agent ended the turn with {other:?}")),
                ),
            },
            Err(error) => {
                let text = error_text(&error);
                let _ = self
                    .lifecycle
                    .unbounded_send(Lifecycle::PromptFailed(text.clone()));
                if self.is_limit(&text) {
                    limit_event(text)
                } else {
                    event("failed", Some(text))
                }
            }
        };
        self.emit(ended);
    }

    fn update(&self, update: SessionUpdate) {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let ContentBlock::Text(text) = chunk.content
                    && let Ok(mut inner) = self.inner.lock()
                {
                    inner.text.push_str(&text.text);
                }
            }
            SessionUpdate::ToolCall(call) => {
                let id = call.tool_call_id.0.to_string();
                let kind = self.settled_kind(&id, Some(kind_name(&call.kind)));
                let mut tool = event("tool", Some(call.title.clone()));
                tool.tools.push(call.name.clone().unwrap_or(call.title));
                self.emit(tool);
                if let Some(kind) = kind {
                    self.check_without_leave(&kind, call.status);
                }
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.0.to_string();
                let kind = self.settled_kind(&id, update.fields.kind.as_ref().map(kind_name));
                if let (Some(kind), Some(status)) = (kind, update.fields.status) {
                    self.check_without_leave(&kind, status);
                }
            }
            SessionUpdate::UsageUpdate(usage) => {
                if let Some(cost) = usage.cost {
                    if cost.currency.eq_ignore_ascii_case("USD") {
                        let mut report = event("usage", None);
                        report.usage = Some(TokenUsage {
                            cost_micro_usd: (cost.amount * 1_000_000.0).round().max(0.0) as u64,
                            cumulative: true,
                            ..TokenUsage::default()
                        });
                        self.emit(report);
                    } else {
                        self.emit(event(
                            "notice",
                            Some(format!(
                                "the agent reported its cost in {}, which is not converted",
                                cost.currency
                            )),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    /// Records what the agent claims a tool does and answers what the tool counts as: a kind
    /// once restricted stays so, or an agent could relabel an edit as a read before asking.
    fn settled_kind(&self, id: &str, claimed: Option<&str>) -> Option<String> {
        let mut inner = self.inner.lock().ok()?;
        let recorded = inner.kinds.get(id).cloned();
        let settled = match (recorded, claimed) {
            (Some(recorded), _) if RESTRICTED_KINDS.contains(&recorded.as_str()) => Some(recorded),
            (_, Some(claimed)) => Some(claimed.to_owned()),
            (recorded, None) => recorded,
        };
        if let Some(kind) = &settled {
            inner.kinds.insert(id.to_owned(), kind.clone());
        }
        settled
    }

    /// A tool of a restricted kind that runs ends a restricted Session: asking changes nothing,
    /// since `permission` refuses every such kind there. One reporting failure ran nothing.
    fn check_without_leave(&self, kind: &str, status: ToolCallStatus) {
        if !self.restricted
            || !RESTRICTED_KINDS.contains(&kind)
            || !matches!(
                status,
                ToolCallStatus::InProgress | ToolCallStatus::Completed
            )
            || self.closing.swap(true, Ordering::SeqCst)
        {
            return;
        }
        self.emit(event(
            "failed",
            Some(format!(
                "the agent ran a {kind} tool in a restricted Session without leave; the Session was ended"
            )),
        ));
        let _ = self.commands.unbounded_send(Command::Abort);
    }

    fn event_notification(&self, params: &Value) {
        let Some(started) = params.get("providerSessionId").and_then(Value::as_str) else {
            return self.event_only(params);
        };
        if let Ok(mut inner) = self.inner.lock() {
            inner.provider_session = Some(started.to_owned());
        }
        self.event_only(params);
    }

    fn event_only(&self, params: &Value) {
        match params.get("event") {
            Some(Value::Null) | None => {
                if let Ok(mut inner) = self.inner.lock() {
                    inner.start_info = Some(params.clone());
                }
                let _ = self.lifecycle.unbounded_send(Lifecycle::Started);
            }
            Some(raw) => match serde_json::from_value::<AgentEvent>(raw.clone()) {
                Ok(event) => match params.get("seq").and_then(Value::as_u64) {
                    Some(seq) => self.emit_seq(seq, event),
                    None => self.emit(event),
                },
                Err(error) => self.emit(event_text("notice", format!("unreadable event: {error}"))),
            },
        }
    }
}

fn event_text(kind: &str, text: String) -> AgentEvent {
    event(kind, Some(text))
}

fn kind_name(kind: &ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "switch_mode",
        _ => "other",
    }
}

fn option_kind(kind: &PermissionOptionKind) -> &'static str {
    match kind {
        PermissionOptionKind::AllowOnce => "allow_once",
        PermissionOptionKind::AllowAlways => "allow_always",
        PermissionOptionKind::RejectOnce => "reject_once",
        PermissionOptionKind::RejectAlways => "reject_always",
        _ => "other",
    }
}

fn error_text(error: &Error) -> String {
    match &error.data {
        Some(Value::String(detail)) if !detail.is_empty() => {
            format!("{}: {detail}", error.message)
        }
        _ => error.message.clone(),
    }
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

/// Starts `work` on a thread of its own.
type StartDecider = fn(Box<dyn FnOnce() + Send>) -> std::io::Result<()>;

fn decider_thread(work: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("ai6-acp-permission".to_owned())
        .spawn(work)
        .map(|_| ())
}

/// The answer to `session/request_permission`, always an outcome. An error answer is no refusal:
/// the Claude ACP adapter 0.99.1 has been seen to go ahead with the tool on one, so whatever keeps
/// this from reaching a decision refuses, and the operator is told.
async fn answer_permission(shared: &Arc<Shared>, params: Value, start: StartDecider) -> Value {
    match permission(shared, params, start).await {
        Ok(answer) => answer,
        Err(reason) => {
            shared.emit(event_text(
                "blocked",
                format!("a permission request was refused: {reason}"),
            ));
            serde_json::to_value(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ))
            .unwrap_or_else(|_| serde_json::json!({"outcome": {"outcome": "cancelled"}}))
        }
    }
}

/// Decides `session/request_permission` through the decider, on a thread of its own: the decision
/// may wait for the operator. `Err` says why no decision could be reached or sent.
async fn permission(
    shared: &Arc<Shared>,
    params: Value,
    start: StartDecider,
) -> Result<Value, String> {
    let request: RequestPermissionRequest =
        serde_json::from_value(params).map_err(|error| format!("it could not be read: {error}"))?;
    let id = request.tool_call.tool_call_id.0.to_string();
    let kind = shared.settled_kind(&id, request.tool_call.fields.kind.as_ref().map(kind_name));
    let asked = PermissionRequest {
        session_id: request.session_id.0.to_string(),
        tool_call_id: id,
        title: request.tool_call.fields.title.clone(),
        kind: kind.clone(),
        raw_input: request.tool_call.fields.raw_input.clone(),
        options: request
            .options
            .iter()
            .map(|option| PermissionOption {
                option_id: option.option_id.0.to_string(),
                kind: option_kind(&option.kind).to_owned(),
            })
            .collect(),
    };
    let decision = if shared.restricted && !restricted_allows(&asked, &shared.servers) {
        PermissionDecision::Deny(format!(
            "a restricted Session may not use a tool of the kind {}",
            kind.as_deref().unwrap_or("unknown")
        ))
    } else {
        let (answer, decided) = oneshot::channel();
        let decider = shared.decider.clone();
        let for_decider = asked.clone();
        start(Box::new(move || {
            let _ = answer.send(decider.decide(&for_decider));
        }))
        .map_err(|error| format!("its decision could not start: {error}"))?;
        decided
            .await
            .unwrap_or_else(|_| PermissionDecision::Deny("the decision was lost".to_owned()))
    };
    let wanted = match &decision {
        PermissionDecision::Allow => "allow_once",
        PermissionDecision::Deny(_) => "reject_once",
    };
    let outcome = request
        .options
        .iter()
        .find(|option| option_kind(&option.kind) == wanted)
        .map(|option| {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                option.option_id.clone(),
            ))
        });
    let tool = asked.title.as_deref().unwrap_or("a tool");
    shared.emit(event_text(
        "permission",
        match &decision {
            PermissionDecision::Allow => format!("permission granted for {tool}"),
            PermissionDecision::Deny(reason) => format!("permission refused for {tool}: {reason}"),
        },
    ));
    let outcome = match outcome {
        Some(outcome) => outcome,
        None => {
            shared.emit(event_text(
                "blocked",
                format!(
                    "the agent asked leave for {} and offered no {wanted} option",
                    asked.title.as_deref().unwrap_or("a tool")
                ),
            ));
            RequestPermissionOutcome::Cancelled
        }
    };
    serde_json::to_value(RequestPermissionResponse::new(outcome))
        .map_err(|error| format!("its answer could not be written: {error}"))
}

async fn request(
    shared: &Arc<Shared>,
    message: UntypedMessage,
    responder: Responder<Value>,
) -> Result<(), Error> {
    match message.method() {
        "session/request_permission" => {
            let params = message.params().clone();
            responder.respond(answer_permission(shared, params, decider_thread).await)
        }
        _ => responder.respond_with_error(Error::method_not_found()),
    }
}

fn notification(shared: &Arc<Shared>, message: &UntypedMessage) {
    match message.method() {
        "session/update" => {
            let suppressed = shared
                .inner
                .lock()
                .map(|inner| inner.suppress)
                .unwrap_or(false);
            if suppressed || shared.events_capability() {
                return;
            }
            if let Ok(notification) =
                serde_json::from_value::<SessionNotification>(message.params().clone())
            {
                shared.update(notification.update);
            }
        }
        EVENT_NOTIFICATION => shared.event_notification(message.params()),
        _ => {}
    }
}

/// Runs `future`, or gives up after `ms` milliseconds. The timer thread ends as soon as the
/// future does.
async fn within<T>(ms: u64, future: impl Future<Output = T>) -> Result<T, ()> {
    if ms == UNBOUNDED_MS {
        return Ok(future.await);
    }
    let (stop, stopped) = mpsc::channel::<()>();
    let (fire, fired) = oneshot::channel::<()>();
    std::thread::Builder::new()
        .name("ai6-acp-timer".to_owned())
        .spawn(move || {
            if stopped.recv_timeout(Duration::from_millis(ms)) == Err(RecvTimeoutError::Timeout) {
                let _ = fire.send(());
            }
        })
        .map_err(|_| ())?;
    let future = std::pin::pin!(future);
    let result = match future::select(future, fired).await {
        Either::Left((value, _)) => Ok(value),
        Either::Right(_) => Err(()),
    };
    drop(stop);
    result
}

async fn call(
    cx: &ConnectionTo<agent_client_protocol::Agent>,
    method: &str,
    params: Value,
    timeout_ms: u64,
) -> Result<Value, String> {
    let message = UntypedMessage::new(method, params).map_err(|error| error.to_string())?;
    match within(timeout_ms, cx.send_request(message).block_task()).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(format!("{method}: {}", error_text(&error))),
        Err(()) => Err(format!("{method}: no answer within {timeout_ms} ms")),
    }
}

/// The select options of a config option, as `(value id, name)`.
fn choices(option: &SessionConfigOption) -> Vec<(String, String)> {
    let SessionConfigKind::Select(select) = &option.kind else {
        return Vec::new();
    };
    match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .map(|choice| (choice.value.0.to_string(), choice.name.clone()))
            .collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter())
            .map(|choice| (choice.value.0.to_string(), choice.name.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// One value to set through a config option.
struct OptionRequest<'a> {
    what: &'a str,
    wanted: &'a str,
    explicit: Option<&'a str>,
    category: SessionConfigOptionCategory,
    fallback: &'a str,
}

/// Sets `wanted` through the config option named `explicit`, or the one of `category`, or
/// the one with id `fallback`; what could not be set is reported as a notice.
async fn set_option(
    cx: &ConnectionTo<agent_client_protocol::Agent>,
    shared: &Shared,
    session: &str,
    options: &[SessionConfigOption],
    request: OptionRequest<'_>,
    timeout_ms: u64,
) {
    let OptionRequest {
        what,
        wanted,
        explicit,
        category,
        fallback,
    } = request;
    let found = options.iter().find(|option| match explicit {
        Some(id) => &*option.id.0 == id,
        None => option.category.as_ref() == Some(&category) || &*option.id.0 == fallback,
    });
    let Some(option) = found else {
        shared.emit(event_text(
            "notice",
            format!(
                "the agent offers no {what} option; the requested {what} {wanted} was not applied"
            ),
        ));
        return;
    };
    let list = choices(option);
    let chosen = list.iter().find(|(id, _)| id == wanted).or_else(|| {
        list.iter()
            .find(|(_, name)| name.eq_ignore_ascii_case(wanted))
    });
    let value = match chosen {
        Some((id, _)) => id.clone(),
        None if list.is_empty() => wanted.to_owned(),
        None => {
            shared.emit(event_text(
                "notice",
                format!(
                    "the agent's {what} option has no value {wanted}; it offers {}",
                    list.iter()
                        .map(|(id, _)| id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
            return;
        }
    };
    let params = json!({"sessionId": session, "configId": &*option.id.0, "value": value});
    if let Err(error) = call(cx, "session/set_config_option", params, timeout_ms).await {
        shared.emit(event_text(
            "notice",
            format!("the {what} was not applied: {error}"),
        ));
    }
}

fn session_params(options: &AcpOptions, session: Option<&str>) -> Value {
    let mut params = json!({
        "cwd": options.cwd,
        "mcpServers": options.mcp_servers,
    });
    if let Some(session) = session {
        params["sessionId"] = json!(session);
    }
    if !options.meta.is_empty() {
        params["_meta"] = Value::Object(options.meta.clone());
    }
    params
}

fn needs_http_mcp(servers: &[Value]) -> bool {
    servers.iter().any(|server| {
        matches!(
            server.get("type").and_then(Value::as_str),
            Some("http" | "sse")
        )
    })
}

/// Opens the session: initialize, authenticate, new or resume, config options. The ACP
/// session id.
async fn open(
    cx: &ConnectionTo<agent_client_protocol::Agent>,
    options: &AcpOptions,
    shared: &Arc<Shared>,
) -> Result<String, String> {
    let init = call(
        cx,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": false
            },
            "clientInfo": {"name": "ai6", "version": env!("CARGO_PKG_VERSION")},
        }),
        options.initialize_timeout_ms,
    )
    .await?;
    let init: InitializeResponse = serde_json::from_value(init)
        .map_err(|error| format!("initialize: unreadable answer: {error}"))?;
    let capabilities = &init.agent_capabilities;
    let events = capabilities
        .meta
        .as_ref()
        .and_then(|meta| meta.get(EVENTS_CAPABILITY))
        .is_some_and(|value| !matches!(value, Value::Null | Value::Bool(false)));
    if let Ok(mut inner) = shared.inner.lock() {
        inner.events_capability = events;
    }
    if !events && needs_http_mcp(&options.mcp_servers) && !capabilities.mcp_capabilities.http {
        return Err(
            "the agent does not accept MCP servers over HTTP, which this Session needs".to_owned(),
        );
    }
    if let Some(auth) = &options.authenticate {
        let mut params = json!({"methodId": auth.method});
        params["_meta"] = Value::Object(auth.meta.clone());
        call(cx, "authenticate", params, options.request_timeout_ms).await?;
    }
    let methods = || {
        let ids = init
            .auth_methods
            .iter()
            .map(|method| match method {
                agent_client_protocol::schema::v1::AuthMethod::Agent(agent) => {
                    agent.id.0.to_string()
                }
                _ => "terminal".to_owned(),
            })
            .collect::<Vec<_>>();
        if ids.is_empty() {
            "none".to_owned()
        } else {
            ids.join(", ")
        }
    };
    let described = |error: String| {
        if error.contains("auth") || error.contains("-32000") {
            format!(
                "{error} (the agent's authentication methods: {})",
                methods()
            )
        } else {
            error
        }
    };
    let (session, config) = match &options.resume {
        Some(id) if options.reattach.is_some() => {
            if !capabilities.load_session {
                return Err("the agent cannot load a session".to_owned());
            }
            let mut params = session_params(options, Some(id));
            let after = options.reattach.unwrap_or_default();
            // `u64::MAX` asks the agent to continue from where it last heard from this side.
            if after != u64::MAX {
                params["_meta"]["dev.coqui.ai6/after"] = json!(after);
            }
            let answer = call(cx, "session/load", params, options.request_timeout_ms)
                .await
                .map_err(described)?;
            let _ = answer;
            (id.clone(), Vec::new())
        }
        None => {
            let answer = call(
                cx,
                "session/new",
                session_params(options, None),
                options.request_timeout_ms,
            )
            .await
            .map_err(described)?;
            let new: NewSessionResponse = serde_json::from_value(answer)
                .map_err(|error| format!("session/new: unreadable answer: {error}"))?;
            (
                new.session_id.0.to_string(),
                new.config_options.unwrap_or_default(),
            )
        }
        Some(id) => {
            let (method, replays) = if capabilities.session_capabilities.resume.is_some() {
                ("session/resume", false)
            } else if capabilities.load_session {
                ("session/load", true)
            } else {
                return Err("the agent can neither resume nor load a session".to_owned());
            };
            if replays && let Ok(mut inner) = shared.inner.lock() {
                inner.suppress = true;
            }
            let answer = call(
                cx,
                method,
                session_params(options, Some(id)),
                options.request_timeout_ms,
            )
            .await;
            if let Ok(mut inner) = shared.inner.lock() {
                inner.suppress = false;
            }
            let answer = answer.map_err(described)?;
            // A field runner names the Session by its key, which is not the provider's ID.
            let named = answer
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let config = answer
                .get("configOptions")
                .and_then(|value| {
                    serde_json::from_value::<Vec<SessionConfigOption>>(value.clone()).ok()
                })
                .unwrap_or_default();
            (named.unwrap_or_else(|| id.clone()), config)
        }
    };
    if let Ok(mut inner) = shared.inner.lock() {
        inner.acp_session = Some(session.clone());
        if !events {
            inner.provider_session = Some(session.clone());
        }
    }
    if !events {
        if let Some(model) = &options.model {
            set_option(
                cx,
                shared,
                &session,
                &config,
                OptionRequest {
                    what: "model",
                    wanted: model,
                    explicit: options.model_option.as_deref(),
                    category: SessionConfigOptionCategory::Model,
                    fallback: "model",
                },
                options.request_timeout_ms,
            )
            .await;
        }
        if let Some(effort) = &options.effort {
            set_option(
                cx,
                shared,
                &session,
                &config,
                OptionRequest {
                    what: "effort",
                    wanted: effort,
                    explicit: options.effort_option.as_deref(),
                    category: SessionConfigOptionCategory::ThoughtLevel,
                    fallback: "effort",
                },
                options.request_timeout_ms,
            )
            .await;
        }
    }
    Ok(session)
}

fn send_prompt(
    cx: &ConnectionTo<agent_client_protocol::Agent>,
    shared: &Arc<Shared>,
    session: &str,
    text: String,
) -> Result<(), Error> {
    shared.begin_turn();
    let message = UntypedMessage::new(
        "session/prompt",
        json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]}),
    )?;
    let shared = shared.clone();
    cx.send_request(message)
        .on_receiving_result(async move |result| {
            shared.turn_ended(result);
            Ok(())
        })
}

async fn main_task(
    cx: ConnectionTo<agent_client_protocol::Agent>,
    options: AcpOptions,
    shared: Arc<Shared>,
    mut commands: async_mpsc::UnboundedReceiver<Command>,
    mut lifecycle: async_mpsc::UnboundedReceiver<Lifecycle>,
    ready: mpsc::Sender<Result<(), String>>,
) -> Result<(), Error> {
    let session = match open(&cx, &options, &shared).await {
        Ok(session) => session,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    if let Some(prompt) = options.prompt.clone() {
        if let Err(error) = send_prompt(&cx, &shared, &session, prompt) {
            let _ = ready.send(Err(error.message));
            return Ok(());
        }
        if shared.events_capability() {
            // A façade starts the operation on the first prompt; its failure to start is the
            // start's failure.
            let started = within(options.start_timeout_ms, lifecycle.next()).await;
            match started {
                Ok(Some(Lifecycle::PromptFailed(error))) => {
                    let _ = ready.send(Err(error));
                    return Ok(());
                }
                Ok(Some(Lifecycle::Started) | None) => {}
                Err(()) => {
                    let _ = ready.send(Err(format!(
                        "the operation did not start within {} ms",
                        options.start_timeout_ms
                    )));
                    return Ok(());
                }
            }
        }
    }
    if options.prompt.is_none()
        && options.resume.is_some()
        && options.reattach.is_none()
        && shared.events_capability()
    {
        // A resumed operation says it is running before the session is handed out.
        let _ = within(options.start_timeout_ms, lifecycle.next()).await;
    }
    let _ = ready.send(Ok(()));
    let closed = cx.incoming_closed();
    let mut closed = std::pin::pin!(closed);
    loop {
        let next = commands.next();
        let command = match future::select(std::pin::pin!(next), closed.as_mut()).await {
            Either::Left((command, _)) => command,
            Either::Right(_) => {
                if !shared.closing.load(Ordering::SeqCst) {
                    shared.lost.store(true, Ordering::SeqCst);
                    shared.flush();
                    if shared.fail_on_disconnect && !shared.finished() {
                        let detail = options
                            .disconnect_detail
                            .as_ref()
                            .and_then(|detail| detail());
                        shared.emit(event_text(
                            "failed",
                            detail.map_or_else(
                                || "the agent closed its connection".to_owned(),
                                |detail| format!("the agent closed its connection: {detail}"),
                            ),
                        ));
                    }
                }
                break;
            }
        };
        match command {
            Some(Command::Prompt(text)) => {
                if let Err(error) = send_prompt(&cx, &shared, &session, text) {
                    shared.emit(event_text("failed", error_text(&error)));
                }
            }
            Some(Command::Ack(seq)) => {
                if let Ok(message) =
                    UntypedMessage::new("_ai6/ack", json!({"sessionId": session, "seq": seq}))
                {
                    let _ = cx.send_notification(message);
                }
            }
            Some(Command::Cancel) => {
                shared.asked_to_cancel();
                if let Ok(message) =
                    UntypedMessage::new("session/cancel", json!({"sessionId": session}))
                {
                    let _ = cx.send_notification(message);
                }
            }
            Some(Command::Close(ack)) => {
                shared.closing.store(true, Ordering::SeqCst);
                let _ = call(
                    &cx,
                    "session/close",
                    json!({"sessionId": session}),
                    options.close_grace_ms,
                )
                .await;
                let _ = ack.send(());
                break;
            }
            Some(Command::Abort) => {
                if let Ok(message) =
                    UntypedMessage::new("session/cancel", json!({"sessionId": session}))
                {
                    let _ = cx.send_notification(message);
                }
                break;
            }
            None => break,
        }
    }
    Ok(())
}

/// A JSON-RPC error an agent answered a call with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcFailure {
    /// The JSON-RPC error code (`-32002` is a resource that was not found).
    pub code: i32,
    /// The agent's message, with its detail.
    pub message: String,
}

/// Connects over `transport`, initializes, authenticates when asked, and makes each of `calls`
/// in turn: what a Station does to ask a field runner a question or give it an order that is no
/// session. The outer error is why the exchange could not take place; each call has its own.
///
/// # Errors
///
/// The connection, `initialize` or `authenticate` failed, in a sentence.
pub fn exchange(
    transport: impl ConnectTo<Client> + 'static,
    authenticate: Option<Authenticate>,
    initialize_timeout_ms: u64,
    request_timeout_ms: u64,
    calls: Vec<(String, Value)>,
) -> Result<Vec<Result<Value, RpcFailure>>, String> {
    let worker = std::thread::Builder::new()
        .name("ai6-acp-exchange".to_owned())
        .spawn(move || {
            futures::executor::block_on(Client.builder().name("ai6").connect_with(
                transport,
                async move |cx| {
                    let started = call(
                        &cx,
                        "initialize",
                        json!({
                            "protocolVersion": 1,
                            "clientCapabilities": {
                                "fs": {"readTextFile": false, "writeTextFile": false},
                                "terminal": false
                            },
                            "clientInfo": {"name": "ai6", "version": env!("CARGO_PKG_VERSION")},
                        }),
                        initialize_timeout_ms,
                    )
                    .await;
                    if let Err(error) = started {
                        return Ok(Err(error));
                    }
                    if let Some(auth) = authenticate {
                        let mut params = json!({"methodId": auth.method});
                        params["_meta"] = Value::Object(auth.meta);
                        if let Err(error) =
                            call(&cx, "authenticate", params, request_timeout_ms).await
                        {
                            return Ok(Err(error));
                        }
                    }
                    let mut results = Vec::new();
                    for (method, params) in calls {
                        let message = UntypedMessage::new(&method, params)?;
                        results.push(
                            match within(request_timeout_ms, cx.send_request(message).block_task())
                                .await
                            {
                                Ok(Ok(value)) => Ok(value),
                                Ok(Err(error)) => Err(RpcFailure {
                                    code: i32::from(error.code),
                                    message: error_text(&error),
                                }),
                                Err(()) => Err(RpcFailure {
                                    code: -1,
                                    message: format!("no answer within {request_timeout_ms} ms"),
                                }),
                            },
                        );
                    }
                    Ok(Ok(results))
                },
            ))
        })
        .map_err(|error| error.to_string())?;
    worker
        .join()
        .map_err(|_| "the ACP exchange panicked".to_owned())?
        .map_err(|error| format!("the ACP connection failed: {}", error_text(&error)))?
}

/// One ACP connection to an agent, driven as an [`AgentSession`].
pub struct AcpSession {
    shared: Arc<Shared>,
    events: Mutex<mpsc::Receiver<Queued>>,
    commands: async_mpsc::UnboundedSender<Command>,
    on_close: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    close_grace_ms: u64,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

fn provider_error(message: impl AsRef<str>) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, message.as_ref())
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
        transport: impl ConnectTo<Client> + 'static,
        options: AcpOptions,
        on_close: Option<Box<dyn FnOnce() + Send>>,
    ) -> Result<Self, ProviderError> {
        let (events_tx, events_rx) = mpsc::channel();
        let (commands_tx, commands_rx) = async_mpsc::unbounded();
        let (lifecycle_tx, lifecycle_rx) = async_mpsc::unbounded();
        let (ready_tx, ready_rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            events: events_tx,
            lost: AtomicBool::new(false),
            commands: commands_tx.clone(),
            lifecycle: lifecycle_tx,
            decider: options.decider.clone(),
            restricted: options.restricted,
            servers: options
                .mcp_servers
                .iter()
                .filter_map(|server| server.get("name").and_then(Value::as_str))
                .map(str::to_owned)
                .collect(),
            close_on_drop: options.close_on_drop,
            fail_on_disconnect: options.fail_on_disconnect,
            limit_phrases: options.limit_phrases.clone(),
            closing: AtomicBool::new(false),
            inner: Mutex::new(Inner::default()),
        });
        let close_grace_ms = options.close_grace_ms;
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("ai6-acp-client".to_owned())
            .spawn(move || {
                let for_requests = worker.clone();
                let for_notifications = worker.clone();
                let failed = ready_tx.clone();
                let after = worker.clone();
                let result = futures::executor::block_on(
                    Client
                        .builder()
                        .name("ai6")
                        .on_receive_request(
                            async move |message: UntypedMessage,
                                        responder: Responder<Value>,
                                        _cx| {
                                request(&for_requests, message, responder).await
                            },
                            on_receive_request!(),
                        )
                        .on_receive_notification(
                            async move |message: UntypedMessage, _cx| {
                                notification(&for_notifications, &message);
                                Ok(())
                            },
                            on_receive_notification!(),
                        )
                        .connect_with(transport, async |cx| {
                            main_task(cx, options, worker, commands_rx, lifecycle_rx, ready_tx)
                                .await
                        }),
                );
                if let Err(error) = result {
                    let _ = failed.send(Err(format!(
                        "the ACP connection failed: {}",
                        error_text(&error)
                    )));
                    // A connection that breaks is lost, as one that closes is.
                    if !after.closing.load(Ordering::SeqCst) {
                        after.lost.store(true, Ordering::SeqCst);
                        after.flush();
                        if after.fail_on_disconnect && !after.finished() {
                            after.emit(event_text(
                                "failed",
                                format!("the agent's connection failed: {}", error_text(&error)),
                            ));
                        }
                    }
                }
            })
            .map_err(|error| provider_error(error.to_string()))?;
        let opened = ready_rx
            .recv()
            .unwrap_or_else(|_| Err("the ACP connection ended before it opened".to_owned()));
        let session = Self {
            shared,
            events: Mutex::new(events_rx),
            commands: commands_tx,
            on_close: Mutex::new(on_close),
            close_grace_ms,
            thread: Mutex::new(Some(thread)),
        };
        match opened {
            Ok(()) => Ok(session),
            Err(error) => {
                session.release();
                Err(provider_error(error))
            }
        }
    }

    fn release(&self) {
        if !self.shared.closing.swap(true, Ordering::SeqCst) && self.shared.close_on_drop {
            // Never closed: say so to the agent, which may hold a harness.
            let (ack, _) = mpsc::channel();
            let _ = self.commands.unbounded_send(Command::Close(ack));
        }
        let _ = self.commands.unbounded_send(Command::Abort);
        if let Ok(mut on_close) = self.on_close.lock()
            && let Some(on_close) = on_close.take()
        {
            on_close();
        }
    }
}

impl AgentSession for AcpSession {
    fn session_id(&self) -> Result<Option<SessionId>, ProviderError> {
        let id = self
            .shared
            .inner
            .lock()
            .map_err(|_| provider_error("ACP state lock was poisoned"))?
            .provider_session
            .clone();
        id.map(|id| SessionId::try_new(id).map_err(|error| provider_error(error.to_string())))
            .transpose()
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        self.commands
            .unbounded_send(Command::Prompt(message.text))
            .map_err(|_| provider_error("the ACP connection is closed"))
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        let events = self
            .events
            .lock()
            .map_err(|_| provider_error("ACP event lock was poisoned"))?;
        self.acknowledge_taken();
        match events.recv_timeout(Duration::from_millis(timeout_ms.max(0).unsigned_abs())) {
            Ok((seq, event)) => {
                if let Some(seq) = seq
                    && let Ok(mut inner) = self.shared.inner.lock()
                {
                    inner.delivered = inner.delivered.max(seq);
                    inner.unacked = Some(seq);
                }
                Ok(Some(event))
            }
            Err(_) => Ok(None),
        }
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        self.commands
            .unbounded_send(Command::Cancel)
            .map_err(|_| provider_error("the ACP connection is closed"))
    }

    fn close(&self) -> Result<(), ProviderError> {
        // What was taken is acknowledged, so nothing is left to replay.
        self.acknowledge_taken();
        let (ack, acked) = mpsc::channel();
        self.shared.closing.store(true, Ordering::SeqCst);
        if self.commands.unbounded_send(Command::Close(ack)).is_ok() {
            let _ = acked.recv_timeout(Duration::from_millis(self.close_grace_ms));
        }
        self.release();
        if let Ok(mut thread) = self.thread.lock()
            && let Some(thread) = thread.take()
        {
            // The connection ends once its main task does; do not wait on a peer that is gone.
            std::thread::spawn(move || {
                let _ = thread.join();
            });
        }
        Ok(())
    }
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        self.release();
    }
}

impl AcpSession {
    /// The parameters of the notification that said the operation started: the provider
    /// Session ID and whatever else the agent told (a runner names the terminal it chose).
    pub fn start_info(&self) -> Option<Value> {
        self.shared
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.start_info.clone())
    }

    /// Acknowledges the event last handed out, if it has not been: asking for another says
    /// the caller is done with it.
    fn acknowledge_taken(&self) {
        let taken = self
            .shared
            .inner
            .lock()
            .ok()
            .and_then(|mut inner| inner.unacked.take());
        if let Some(seq) = taken {
            let _ = self.commands.unbounded_send(Command::Ack(seq));
        }
    }

    /// The newest runner sequence number `next_event` has handed out; `0` before any.
    pub fn last_seq(&self) -> u64 {
        self.shared
            .inner
            .lock()
            .map(|inner| inner.delivered)
            .unwrap_or_default()
    }

    /// Whether the connection ended without this side closing it: the agent, or the link to
    /// it, is gone.
    pub fn connection_lost(&self) -> bool {
        self.shared.lost.load(Ordering::SeqCst)
    }

    /// This session as the object handle a Station holds.
    pub fn into_handle(self) -> Object<dyn AgentSession> {
        self.into_object()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(decider: Arc<dyn PermissionDecider>) -> (Arc<Shared>, mpsc::Receiver<Queued>) {
        let (events, received) = mpsc::channel();
        let (commands, _) = async_mpsc::unbounded();
        let (lifecycle, _) = async_mpsc::unbounded();
        (
            Arc::new(Shared {
                events,
                lost: AtomicBool::new(false),
                commands,
                lifecycle,
                decider,
                restricted: false,
                servers: Vec::new(),
                close_on_drop: false,
                fail_on_disconnect: false,
                limit_phrases: Vec::new(),
                closing: AtomicBool::new(false),
                inner: Mutex::default(),
            }),
            received,
        )
    }

    /// The decider that would let anything through, were it asked.
    struct Allows;

    impl PermissionDecider for Allows {
        fn decide(&self, _request: &PermissionRequest) -> PermissionDecision {
            PermissionDecision::Allow
        }
    }

    fn request() -> Value {
        serde_json::json!({
            "sessionId": "s",
            "toolCall": {"toolCallId": "t", "title": "Run it", "kind": "execute"},
            "options": [{"optionId": "yes", "name": "Allow", "kind": "allow_once"}],
        })
    }

    fn refuses_with_the_reason(answer: &Value, events: &mpsc::Receiver<Queued>, reason: &str) {
        assert_eq!(
            *answer,
            serde_json::json!({"outcome": {"outcome": "cancelled"}})
        );
        let (_, told) = events.try_recv().expect("the operator is told");
        assert_eq!(told.kind, "blocked");
        let text = told.text.unwrap();
        assert!(text.contains(reason), "{text}");
    }

    #[test]
    fn a_decider_that_cannot_start_refuses_the_tool() {
        let (shared, events) = shared(Arc::new(Allows));
        let answer = futures::executor::block_on(answer_permission(&shared, request(), |_| {
            Err(std::io::Error::other("no thread today"))
        }));
        refuses_with_the_reason(&answer, &events, "no thread today");
    }

    #[test]
    fn a_request_that_does_not_parse_refuses_the_tool() {
        let (shared, events) = shared(Arc::new(Allows));
        let started = |_: Box<dyn FnOnce() + Send>| -> std::io::Result<()> {
            panic!("nothing is decided on a request that cannot be read")
        };
        let answer = futures::executor::block_on(answer_permission(
            &shared,
            serde_json::json!({"sessionId": "s", "toolCall": 7}),
            started,
        ));
        refuses_with_the_reason(&answer, &events, "could not be read");
    }

    #[test]
    fn a_request_that_parses_still_reaches_the_decider() {
        let (shared, _events) = shared(Arc::new(Allows));
        let answer =
            futures::executor::block_on(answer_permission(&shared, request(), decider_thread));
        assert_eq!(answer["outcome"]["optionId"], "yes");
    }
}
