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
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{EVENT_NOTIFICATION, EVENTS_CAPABILITY, RESTRICTED_KINDS};
use crate::directorates::m::identity::SessionId;
use crate::directorates::section_00::agent::{
    AgentEvent, AgentMessage, AgentSession, LIMIT_EVENT, TokenUsage,
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
    /// Whether the Session only reads and answers: every permission request for a tool that
    /// edits, deletes, moves or executes is refused, and a tool of those kinds that runs
    /// without asking ends the Session.
    pub restricted: bool,
    /// Sent before the session request, when the agent needs it.
    pub authenticate: Option<Authenticate>,
    /// How long `initialize` may take.
    pub initialize_timeout_ms: u64,
    /// How long a request other than `session/prompt` may take.
    pub request_timeout_ms: u64,
    /// How long `close` waits for the agent before ending its process.
    pub close_grace_ms: u64,
    /// Answers permission requests.
    pub decider: Arc<dyn PermissionDecider>,
}

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
            restricted: false,
            authenticate: None,
            initialize_timeout_ms: 20_000,
            request_timeout_ms: 30_000,
            close_grace_ms: 2_000,
            decider: Arc::new(RefuseAll),
        }
    }
}

enum Command {
    Prompt(String),
    Cancel,
    Close(mpsc::Sender<()>),
    Abort,
}

enum Lifecycle {
    Started,
    PromptFailed(String),
}

/// What the notification handlers share with the connection's main task.
struct Shared {
    events: mpsc::Sender<AgentEvent>,
    commands: async_mpsc::UnboundedSender<Command>,
    lifecycle: async_mpsc::UnboundedSender<Lifecycle>,
    decider: Arc<dyn PermissionDecider>,
    restricted: bool,
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
    /// A completed, failed or limit event ended the current turn.
    finished: bool,
    text: String,
    asked: HashSet<String>,
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

fn is_terminal(kind: &str) -> bool {
    matches!(kind, "completed" | "failed" | LIMIT_EVENT | "blown")
}

impl Shared {
    fn emit(&self, event: AgentEvent) {
        if let Ok(mut inner) = self.inner.lock() {
            if is_terminal(&event.kind) {
                inner.finished = true;
            }
            self.flush_locked(&mut inner);
        }
        let _ = self.events.send(event);
    }

    fn flush_locked(&self, inner: &mut Inner) {
        if !inner.text.is_empty() {
            let text = std::mem::take(&mut inner.text);
            let _ = self.events.send(event("text", Some(text)));
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
        }
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
                Some("cancelled") => event("blocked", Some("the turn was cancelled".to_owned())),
                other => event(
                    "failed",
                    Some(format!("the agent ended the turn with {other:?}")),
                ),
            },
            Err(error) => {
                let _ = self
                    .lifecycle
                    .unbounded_send(Lifecycle::PromptFailed(error_text(&error)));
                event("failed", Some(error_text(&error)))
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
                let kind = kind_name(&call.kind);
                let id = call.tool_call_id.0.to_string();
                if let Ok(mut inner) = self.inner.lock() {
                    inner.kinds.insert(id.clone(), kind.to_owned());
                }
                let mut tool = event("tool", Some(call.title.clone()));
                tool.tools.push(call.name.clone().unwrap_or(call.title));
                self.emit(tool);
                self.check_unasked(&id, kind, call.status);
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.0.to_string();
                let kind = update
                    .fields
                    .kind
                    .as_ref()
                    .map(|kind| kind_name(kind).to_owned())
                    .or_else(|| {
                        self.inner
                            .lock()
                            .ok()
                            .and_then(|inner| inner.kinds.get(&id).cloned())
                    });
                if let (Some(kind), Some(status)) = (kind, update.fields.status) {
                    self.check_unasked(&id, &kind, status);
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

    /// A tool of a restricted kind that runs without the agent having asked leave ends a
    /// restricted Session.
    fn check_unasked(&self, id: &str, kind: &str, status: ToolCallStatus) {
        if !self.restricted
            || !RESTRICTED_KINDS.contains(&kind)
            || !matches!(
                status,
                ToolCallStatus::InProgress | ToolCallStatus::Completed
            )
        {
            return;
        }
        let asked = self
            .inner
            .lock()
            .map(|inner| inner.asked.contains(id))
            .unwrap_or(true);
        if asked || self.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        self.emit(event(
            "failed",
            Some(format!(
                "the agent ran a {kind} tool in a restricted Session without asking leave; the Session was ended"
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
                let _ = self.lifecycle.unbounded_send(Lifecycle::Started);
            }
            Some(raw) => {
                let suppressed = self
                    .inner
                    .lock()
                    .map(|inner| inner.suppress)
                    .unwrap_or(false);
                if suppressed {
                    return;
                }
                match serde_json::from_value::<AgentEvent>(raw.clone()) {
                    Ok(event) => self.emit(event),
                    Err(error) => {
                        self.emit(event_text("notice", format!("unreadable event: {error}")))
                    }
                }
            }
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

/// Answers `session/request_permission` through the decider, on a thread of its own: the
/// decision may wait for the operator.
async fn permission(shared: &Arc<Shared>, params: Value) -> Result<Value, Error> {
    let request: RequestPermissionRequest =
        serde_json::from_value(params).map_err(|_| Error::invalid_params())?;
    let id = request.tool_call.tool_call_id.0.to_string();
    let kind = request
        .tool_call
        .fields
        .kind
        .as_ref()
        .map(|kind| kind_name(kind).to_owned())
        .or_else(|| {
            shared
                .inner
                .lock()
                .ok()
                .and_then(|inner| inner.kinds.get(&id).cloned())
        });
    if let Ok(mut inner) = shared.inner.lock() {
        inner.asked.insert(id.clone());
    }
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
    let decision = if shared.restricted
        && kind
            .as_deref()
            .is_some_and(|kind| RESTRICTED_KINDS.contains(&kind))
    {
        PermissionDecision::Deny(format!(
            "a restricted Session may not use a tool that does {}",
            kind.as_deref().unwrap_or("this")
        ))
    } else {
        let (answer, decided) = oneshot::channel();
        let decider = shared.decider.clone();
        let for_decider = asked.clone();
        std::thread::Builder::new()
            .name("ai6-acp-permission".to_owned())
            .spawn(move || {
                let _ = answer.send(decider.decide(&for_decider));
            })
            .map_err(|error| Error::internal_error().data(error.to_string()))?;
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
    if let PermissionDecision::Deny(reason) = &decision {
        shared.emit(event_text(
            "notice",
            format!(
                "permission refused for {}: {reason}",
                asked.title.as_deref().unwrap_or("a tool")
            ),
        ));
    }
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
        .map_err(|error| Error::internal_error().data(error.to_string()))
}

async fn request(
    shared: &Arc<Shared>,
    message: UntypedMessage,
    responder: Responder<Value>,
) -> Result<(), Error> {
    match message.method() {
        "session/request_permission" => {
            let params = message.params().clone();
            match permission(shared, params).await {
                Ok(value) => responder.respond(value),
                Err(error) => responder.respond_with_error(error),
            }
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
            let config = answer
                .get("configOptions")
                .and_then(|value| {
                    serde_json::from_value::<Vec<SessionConfigOption>>(value.clone()).ok()
                })
                .unwrap_or_default();
            (id.clone(), config)
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
            let started = within(options.request_timeout_ms, lifecycle.next()).await;
            match started {
                Ok(Some(Lifecycle::PromptFailed(error))) => {
                    let _ = ready.send(Err(error));
                    return Ok(());
                }
                Ok(Some(Lifecycle::Started) | None) => {}
                Err(()) => {
                    let _ = ready.send(Err(format!(
                        "the operation did not start within {} ms",
                        options.request_timeout_ms
                    )));
                    return Ok(());
                }
            }
        }
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
                    shared.flush();
                    if !shared.finished() {
                        shared.emit(event_text(
                            "failed",
                            "the agent closed its connection".to_owned(),
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
            Some(Command::Cancel) => {
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

/// One ACP connection to an agent, driven as an [`AgentSession`].
pub struct AcpSession {
    shared: Arc<Shared>,
    events: Mutex<mpsc::Receiver<AgentEvent>>,
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
            commands: commands_tx.clone(),
            lifecycle: lifecycle_tx,
            decider: options.decider.clone(),
            restricted: options.restricted,
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
        self.shared.closing.store(true, Ordering::SeqCst);
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
        match events.recv_timeout(Duration::from_millis(timeout_ms.max(0).unsigned_abs())) {
            Ok(event) => Ok(Some(event)),
            Err(_) => Ok(None),
        }
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        self.commands
            .unbounded_send(Command::Cancel)
            .map_err(|_| provider_error("the ACP connection is closed"))
    }

    fn close(&self) -> Result<(), ProviderError> {
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
    /// This session as the object handle a Station holds.
    pub fn into_handle(self) -> Object<dyn AgentSession> {
        self.into_object()
    }
}
