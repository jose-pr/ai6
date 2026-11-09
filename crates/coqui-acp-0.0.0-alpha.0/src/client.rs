//! The client: opens a session on any ACP agent and drives it from blocking code.

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::future::Future;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::time::Duration;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};

use agent_client_protocol::schema::v1::{
    AuthMethod, InitializeResponse, NewSessionResponse, PermissionOptionKind,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOptions, SessionInfo,
};
use agent_client_protocol::{
    Agent, Client as Role, ConnectTo, ConnectionTo, Error as RpcError, Responder, UntypedMessage,
    on_receive_notification, on_receive_request,
};
use futures::StreamExt;
use futures::channel::{mpsc as async_mpsc, oneshot};
use futures::future::{self, Either};
use serde_json::{Map, Value, json};

use crate::decider::{Decider, Decision, PermissionOption, PermissionRequest, RefuseAll};
use crate::describe::{
    self, Answer, DescribeLimits, Description, SESSION_OPTIONS_CAPABILITY, SESSION_OPTIONS_METHOD,
    SessionList,
};
use crate::elicit::{self, ElicitationAnswer, Elicitor};
use crate::error::Error;
use crate::event::Event;
use crate::state::{Step, TEXT_FLUSH, TOOL_KINDS, Turn, kind_name};

const PROTOCOL_VERSION: u64 = 1;
/// What `coqui-acp-bridge serve --linger` sends when it dropped output while nobody was connected.
const OUTPUT_LOST: &str = "_dev.boriken.coqui.acp/output_lost";

/// An `authenticate` call to make before the first session request.
#[derive(Clone)]
pub struct Authenticate {
    /// The method id the agent advertised.
    pub method: String,
    /// The request's `_meta`, for instance a token. Never logged here.
    pub meta: Map<String, Value>,
}

impl fmt::Debug for Authenticate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authenticate")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

enum Opening {
    New,
    Resume(String),
    Load(String),
}

type EndsTurn = Box<dyn Fn(&str, &Value) -> bool + Send + Sync>;
type DisconnectDetail = Box<dyn FnOnce() -> Option<String> + Send>;

/// How to open a session; see [`Client::builder`].
#[must_use]
pub struct Builder {
    cwd: String,
    opening: Opening,
    client_name: String,
    client_version: String,
    mcp_servers: Vec<Value>,
    meta: Map<String, Value>,
    model: Option<String>,
    effort: Option<String>,
    model_option: Option<String>,
    effort_option: Option<String>,
    prompt: Option<String>,
    refuse_kinds: Vec<String>,
    authenticate: Option<Authenticate>,
    turn_end_by: Option<String>,
    ends_turn: Option<EndsTurn>,
    disconnect_detail: Option<DisconnectDetail>,
    initialize_timeout: Option<Duration>,
    request_timeout: Option<Duration>,
    close_grace: Option<Duration>,
    max_pending_events: usize,
    text_flush: usize,
    max_tool_kinds: usize,
    decider: Arc<dyn Decider>,
    elicitor: Option<Arc<dyn Elicitor>>,
    elicitation_limits: elicit::Limits,
    close_on_drop: bool,
    fail_on_disconnect: bool,
    on_close: Option<Box<dyn FnOnce() + Send>>,
}

impl fmt::Debug for Builder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Builder")
            .field("cwd", &self.cwd)
            .field("client_name", &self.client_name)
            .field("refuse_kinds", &self.refuse_kinds)
            .finish_non_exhaustive()
    }
}

/// Opens sessions on ACP agents.
#[derive(Clone, Copy, Debug)]
pub struct Client;

impl Client {
    /// Starts a session description. `cwd` is the agent's absolute working directory.
    pub fn builder(cwd: impl Into<String>) -> Builder {
        Builder {
            cwd: cwd.into(),
            opening: Opening::New,
            client_name: "coqui-acp".to_owned(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            mcp_servers: Vec::new(),
            meta: Map::new(),
            model: None,
            effort: None,
            model_option: None,
            effort_option: None,
            prompt: None,
            refuse_kinds: Vec::new(),
            authenticate: None,
            turn_end_by: None,
            ends_turn: None,
            disconnect_detail: None,
            initialize_timeout: Some(Duration::from_secs(20)),
            request_timeout: Some(Duration::from_secs(30)),
            close_grace: Some(Duration::from_secs(2)),
            max_pending_events: 100_000,
            text_flush: TEXT_FLUSH,
            max_tool_kinds: TOOL_KINDS,
            decider: Arc::new(RefuseAll),
            elicitor: None,
            elicitation_limits: elicit::Limits {
                fields: 64,
                options: 256,
            },
            close_on_drop: true,
            fail_on_disconnect: true,
            on_close: None,
        }
    }
}

impl Builder {
    /// What this client calls itself in `initialize`.
    pub fn client_info(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.client_name = name.into();
        self.client_version = version.into();
        self
    }

    /// Resumes a session: `session/resume`, else `session/load` with its replayed history dropped.
    pub fn resume(mut self, session: impl Into<String>) -> Self {
        self.opening = Opening::Resume(session.into());
        self
    }

    /// Attaches by `session/load` whatever else the agent offers, and delivers what it replays.
    pub fn load(mut self, session: impl Into<String>) -> Self {
        self.opening = Opening::Load(session.into());
        self
    }

    /// Adds an MCP server: one of ACP's `mcpServers` entries.
    pub fn mcp_server(mut self, server: Value) -> Self {
        self.mcp_servers.push(server);
        self
    }

    /// Sets one key of the session request's `_meta`. ACP's own place for what it has no field for.
    pub fn meta(mut self, key: impl Into<String>, value: Value) -> Self {
        self.meta.insert(key.into(), value);
        self
    }

    /// Requests a model, through the agent's model config option.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Requests a reasoning effort, through the agent's thought-level config option.
    pub fn effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = Some(effort.into());
        self
    }

    /// Names the config option for the model. Default: the option of category `model`, else the id `model`.
    pub fn model_option(mut self, id: impl Into<String>) -> Self {
        self.model_option = Some(id.into());
        self
    }

    /// Names the config option for the effort. Default: the option of category `thought_level`, else `effort`.
    pub fn effort_option(mut self, id: impl Into<String>) -> Self {
        self.effort_option = Some(id.into());
        self
    }

    /// The first turn, sent once the session exists.
    pub fn prompt(mut self, text: impl Into<String>) -> Self {
        self.prompt = Some(text.into());
        self
    }

    /// Tool kinds this session refuses without asking the decider. A tool of such a kind that runs, whether it asked or
    /// not, ends the session; a kind once announced as refused stays so, whatever a later message claims.
    pub fn refuse_kinds(mut self, kinds: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.refuse_kinds = kinds.into_iter().map(Into::into).collect();
        self
    }

    /// Calls `authenticate` before the session request.
    pub fn authenticate(mut self, authenticate: Authenticate) -> Self {
        self.authenticate = Some(authenticate);
        self
    }

    /// Hands turn endings to an extension, when the agent advertises `key` in its
    /// `agentCapabilities._meta`. Its notifications then arrive as [`Event::Extension`], standard
    /// updates and config options are left alone, and a failed prompt is an [`Event::PromptFailed`].
    pub fn turn_end_by(mut self, key: impl Into<String>) -> Self {
        self.turn_end_by = Some(key.into());
        self
    }

    /// Says which of the extension's notifications end a turn, given their method and parameters, while [`Builder::turn_end_by`]
    /// is active: a connection that ends after one is no [`Event::Failed`]. It is asked on the connection's thread as each one
    /// arrives, ahead of the connection's end, so it cannot race what a caller reads. It must not block.
    pub fn ends_turn(
        mut self,
        ends: impl Fn(&str, &Value) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.ends_turn = Some(Box::new(ends));
        self
    }

    /// Says what became of the agent, for the [`Event::Failed`] a lost connection reports: `<why>: <detail>`. It is asked at most
    /// once, on the connection's thread, only when that event is due, and may wait a while for the agent's exit.
    pub fn disconnect_detail(
        mut self,
        detail: impl FnOnce() -> Option<String> + Send + 'static,
    ) -> Self {
        self.disconnect_detail = Some(Box::new(detail));
        self
    }

    /// How long `initialize` may take; `None` waits without limit. Default 20 s.
    pub const fn initialize_timeout(mut self, limit: Option<Duration>) -> Self {
        self.initialize_timeout = limit;
        self
    }

    /// How long a request other than `session/prompt` may take; `None` waits without limit. Default 30 s.
    pub const fn request_timeout(mut self, limit: Option<Duration>) -> Self {
        self.request_timeout = limit;
        self
    }

    /// How long [`Session::close`] waits for the agent; `None` waits without limit. Default 2 s.
    pub const fn close_grace(mut self, limit: Option<Duration>) -> Self {
        self.close_grace = limit;
        self
    }

    /// Events held for a slow reader, past which the session ends with [`Event::Failed`]. Default 100000.
    pub const fn max_pending_events(mut self, events: usize) -> Self {
        self.max_pending_events = events;
        self
    }

    /// The UTF-8 bytes of agent text gathered before they are delivered as an [`Event::Text`], even mid-message, so a
    /// long message cannot grow without limit. Default 64 KiB.
    pub const fn text_flush(mut self, bytes: usize) -> Self {
        self.text_flush = bytes;
        self
    }

    /// The tool calls whose kind is kept, for what [`Builder::refuse_kinds`] decides of later messages about them. One of a
    /// refused kind is never forgotten: one that cannot be kept ends the session. Forgetting another is said once, as an
    /// [`Event::Notice`]. Default 4096.
    pub const fn max_tool_kinds(mut self, calls: usize) -> Self {
        self.max_tool_kinds = calls;
        self
    }

    /// Answers permission requests. Default: [`RefuseAll`].
    pub fn decider(mut self, decider: Arc<dyn Decider>) -> Self {
        self.decider = decider;
        self
    }

    /// Puts the agent's questions (`elicitation/create`, in form mode) to the person, and advertises that this client takes them.
    /// Without one, nothing is advertised and such a request is not answered as one this client takes.
    pub fn elicitor(mut self, elicitor: Arc<dyn Elicitor>) -> Self {
        self.elicitor = Some(elicitor);
        self
    }

    /// The most questions one form may hold. Default 64; a form with more is answered cancelled without asking.
    pub const fn max_elicitation_fields(mut self, fields: usize) -> Self {
        self.elicitation_limits.fields = fields;
        self
    }

    /// The most choices one question may offer. Default 256; a form with more is answered cancelled without asking.
    pub const fn max_elicitation_options(mut self, options: usize) -> Self {
        self.elicitation_limits.options = options;
        self
    }

    /// Whether dropping the session without closing it closes the agent's session. Default on. Off for an
    /// agent that outlives the connection, which a later connection reattaches to.
    pub const fn close_on_drop(mut self, close: bool) -> Self {
        self.close_on_drop = close;
        self
    }

    /// Whether a connection that ends mid-session is an [`Event::Failed`]. Default on.
    pub const fn fail_on_disconnect(mut self, fail: bool) -> Self {
        self.fail_on_disconnect = fail;
        self
    }

    /// Runs when the session ends or fails to open, for instance to end the agent's process.
    pub fn on_close(mut self, on_close: impl FnOnce() + Send + 'static) -> Self {
        self.on_close = Some(Box::new(on_close));
        self
    }

    /// Connects, initializes, authenticates, opens the session and sends the first prompt.
    ///
    /// # Errors
    ///
    /// The agent's refusal or silence: an `initialize`, authentication or session error, a protocol version
    /// other than 1, or a capability this session needs and the agent lacks.
    pub fn connect(mut self, transport: impl ConnectTo<Role> + 'static) -> Result<Session, Error> {
        let (events_tx, events_rx) = mpsc::channel();
        let (commands_tx, commands_rx) = async_mpsc::unbounded();
        let (ready_tx, ready_rx) = mpsc::channel();
        let on_close = self.on_close.take();
        let shared = Arc::new(Shared {
            events: events_tx,
            pending: AtomicUsize::new(0),
            max_pending: self.max_pending_events,
            commands: commands_tx.clone(),
            decider: self.decider.clone(),
            elicitor: self.elicitor.clone(),
            elicitation_limits: self.elicitation_limits,
            ends_turn: self.ends_turn.take(),
            disconnect_detail: Mutex::new(self.disconnect_detail.take()),
            fail_on_disconnect: self.fail_on_disconnect,
            close_on_drop: self.close_on_drop,
            lost: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            extension: AtomicBool::new(false),
            suppress: AtomicBool::new(false),
            session: Mutex::new(None),
            turn: Mutex::new(
                Turn::new(self.refuse_kinds.clone())
                    .text_flush(self.text_flush)
                    .max_kinds(self.max_tool_kinds),
            ),
        });
        let close_grace = self.close_grace;
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("coqui-acp-client".to_owned())
            .spawn(move || {
                let for_requests = worker.clone();
                let for_notifications = worker.clone();
                let failed = ready_tx.clone();
                let after = worker.clone();
                let name = self.client_name.clone();
                let result = futures::executor::block_on(
                    Role.builder()
                        .name(name)
                        .on_receive_request(
                            async move |message: UntypedMessage,
                                        responder: Responder<Value>,
                                        _cx| {
                                if message.method() == "session/request_permission" {
                                    let params = message.params().clone();
                                    match permission(&for_requests, params).await {
                                        Ok(value) => responder.respond(value),
                                        Err(error) => responder.respond_with_error(error),
                                    }
                                } else if message.method() == "elicitation/create"
                                    && for_requests.elicitor.is_some()
                                {
                                    let params = message.params().clone();
                                    responder.respond(elicitation(&for_requests, params).await)
                                } else {
                                    responder.respond_with_error(RpcError::method_not_found())
                                }
                            },
                            on_receive_request!(),
                        )
                        .on_receive_notification(
                            async move |message: UntypedMessage, _cx| {
                                for_notifications.notification(&message);
                                Ok(())
                            },
                            on_receive_notification!(),
                        )
                        .connect_with(transport, async |cx| {
                            main_task(cx, self, worker, commands_rx, ready_tx).await
                        }),
                );
                if let Err(error) = result {
                    let _ = failed.send(Err(Error::Refused(format!(
                        "the ACP connection failed: {}",
                        error_text(&error)
                    ))));
                    after.disconnected(&format!(
                        "the agent's connection failed: {}",
                        error_text(&error)
                    ));
                }
            })
            .map_err(|_| Error::Thread)?;
        let opened = ready_rx.recv().unwrap_or(Err(Error::Closed));
        let session = Session {
            shared,
            events: Mutex::new(events_rx),
            commands: commands_tx,
            on_close: Mutex::new(on_close),
            close_grace,
            thread: Mutex::new(Some(thread)),
        };
        match opened {
            Ok(()) => Ok(session),
            Err(error) => {
                session.release();
                Err(error)
            }
        }
    }
}

enum Command {
    Prompt(String),
    Notify(String, Value),
    Cancel,
    Close(mpsc::Sender<()>),
    Abort,
}

struct Shared {
    events: mpsc::Sender<Event>,
    pending: AtomicUsize,
    max_pending: usize,
    commands: async_mpsc::UnboundedSender<Command>,
    decider: Arc<dyn Decider>,
    elicitor: Option<Arc<dyn Elicitor>>,
    elicitation_limits: elicit::Limits,
    ends_turn: Option<EndsTurn>,
    disconnect_detail: Mutex<Option<DisconnectDetail>>,
    fail_on_disconnect: bool,
    close_on_drop: bool,
    lost: AtomicBool,
    closing: AtomicBool,
    extension: AtomicBool,
    suppress: AtomicBool,
    session: Mutex<Option<String>>,
    turn: Mutex<Turn>,
}

impl Shared {
    fn turn(&self) -> std::sync::MutexGuard<'_, Turn> {
        self.turn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, event: Event) {
        if self.pending.fetch_add(1, Ordering::SeqCst) >= self.max_pending {
            if !self.closing.swap(true, Ordering::SeqCst) {
                let _ = self.events.send(Event::Failed(
                    "the reader fell behind the event limit; the session was ended".to_owned(),
                ));
                let _ = self.commands.unbounded_send(Command::Abort);
            }
            return;
        }
        let _ = self.events.send(event);
    }

    fn emit(&self, event: Event) {
        let flushed = {
            let mut turn = self.turn();
            turn.note(&event);
            turn.flush()
        };
        if let Some(text) = flushed {
            self.push(text);
        }
        self.push(event);
    }

    fn apply(&self, step: Step) {
        for event in step.events {
            self.emit(event);
        }
        if step.abort {
            self.closing.store(true, Ordering::SeqCst);
            let _ = self.commands.unbounded_send(Command::Abort);
        }
    }

    fn flush(&self) {
        let flushed = self.turn().flush();
        if let Some(text) = flushed {
            self.push(text);
        }
    }

    /// Whether a message's `sessionId` names this session. Before the session is known nothing does, so what an agent says of another,
    /// such as one an earlier connection left running, never reaches this one.
    fn is_mine(&self, params: &Value) -> bool {
        let named = params.get("sessionId").and_then(Value::as_str);
        let session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        named.is_some() && named == session.as_deref()
    }

    fn notification(&self, message: &UntypedMessage) {
        let method = message.method();
        if method == "session/update" {
            if self.suppress.load(Ordering::SeqCst)
                || self.extension.load(Ordering::SeqCst)
                || !self.is_mine(message.params())
            {
                return;
            }
            let step = self.turn().update(message.params());
            self.apply(step);
        } else if method == OUTPUT_LOST {
            let count = |key: &str| {
                message
                    .params()
                    .get(key)
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            };
            self.emit(Event::OutputLost {
                lines: count("lines"),
                bytes: count("bytes"),
            });
        } else if method.starts_with('_') {
            if self.extension.load(Ordering::SeqCst)
                && self
                    .ends_turn
                    .as_ref()
                    .is_some_and(|ends| ends(method, message.params()))
            {
                self.turn().finish();
            }
            self.push(Event::Extension {
                method: method.to_owned(),
                params: message.params().clone(),
            });
        }
    }

    fn turn_ended(&self, result: Result<Value, RpcError>) {
        self.flush();
        // A turn cut off by the connection ending is the connection's to report.
        if let Err(error) = &result
            && (agent_client_protocol::is_incoming_transport_closed(error)
                || agent_client_protocol::is_connection_terminated(error))
        {
            return;
        }
        if self.extension.load(Ordering::SeqCst) {
            if let Err(error) = result {
                self.push(Event::PromptFailed(error_text(&error)));
            }
            return;
        }
        let event = match result {
            Ok(value) => {
                let stop = value.get("stopReason").and_then(Value::as_str);
                self.turn().stopped(stop)
            }
            Err(error) => Event::Failed(error_text(&error)),
        };
        self.emit(event);
    }

    /// The connection ended without this side closing it.
    fn disconnected(&self, why: &str) {
        if self.closing.load(Ordering::SeqCst) {
            return;
        }
        self.lost.store(true, Ordering::SeqCst);
        self.flush();
        let finished = self.turn().finished();
        if !self.fail_on_disconnect || finished {
            return;
        }
        let detail = self
            .disconnect_detail
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .and_then(|detail| detail());
        self.emit(Event::Failed(match detail {
            Some(detail) => format!("{why}: {detail}"),
            None => why.to_owned(),
        }));
    }
}

/// An error's message with the agent's own reason, which agents put in `data` as text or as an object's `message`.
fn error_text(error: &RpcError) -> String {
    let detail = match &error.data {
        Some(Value::String(detail)) => Some(detail),
        Some(Value::Object(data)) => match data.get("message") {
            Some(Value::String(detail)) => Some(detail),
            _ => None,
        },
        _ => None,
    };
    match detail {
        Some(detail) if !detail.is_empty() => format!("{}: {detail}", error.message),
        _ => error.message.clone(),
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

/// Answers `session/request_permission` through the decider, on a thread of its own: it may wait for a person.
/// An agent has been seen to go ahead with the tool on an error answer, so anything that keeps this from being decided is a
/// cancelled outcome and an event, and never an error.
fn not_decided(shared: &Shared, why: &str) -> Result<Value, RpcError> {
    shared.emit(Event::Blocked(why.to_owned()));
    serde_json::to_value(RequestPermissionResponse::new(
        RequestPermissionOutcome::Cancelled,
    ))
    .map_err(|error| RpcError::internal_error().data(error.to_string()))
}

async fn permission(shared: &Arc<Shared>, params: Value) -> Result<Value, RpcError> {
    // Another session's, such as one an earlier connection left waiting: the decider answers for this session only.
    if params.get("sessionId").is_some_and(Value::is_string) && !shared.is_mine(&params) {
        return serde_json::to_value(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ))
        .map_err(|error| RpcError::internal_error().data(error.to_string()));
    }
    let Ok(request) = serde_json::from_value::<RequestPermissionRequest>(params) else {
        return not_decided(
            shared,
            "the agent asked leave in a form this client cannot read",
        );
    };
    let id = request.tool_call.tool_call_id.0.to_string();
    let mut step = Step::default();
    let (kind, refused) = {
        let mut turn = shared.turn();
        let claimed = request.tool_call.fields.kind.as_ref().map(kind_name);
        if let Some(kind) = claimed {
            turn.announce(&id, kind, &mut step);
        }
        let kind = turn.kind_of(&id).or_else(|| claimed.map(ToOwned::to_owned));
        let refused = kind.as_deref().is_some_and(|kind| turn.is_refused(kind));
        (kind, refused)
    };
    shared.apply(step);
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
    let decision = if refused {
        Decision::Deny(format!(
            "this session may not use a tool that does {}",
            kind.as_deref().unwrap_or("this")
        ))
    } else {
        let (answer, decided) = oneshot::channel();
        let decider = shared.decider.clone();
        let for_decider = asked.clone();
        let spawned = std::thread::Builder::new()
            .name("coqui-acp-permission".to_owned())
            .spawn(move || {
                let _ = answer.send(decider.decide(&for_decider));
            });
        if spawned.is_err() {
            return not_decided(
                shared,
                "nothing could be started to decide what the agent asked",
            );
        }
        decided
            .await
            .unwrap_or_else(|_| Decision::Deny("the decision was lost".to_owned()))
    };
    let (wanted, allowed, reason) = match decision {
        Decision::Allow => ("allow_once", true, None),
        Decision::Deny(reason) => ("reject_once", false, Some(reason)),
    };
    let tool = asked.title.clone().unwrap_or_else(|| "a tool".to_owned());
    shared.emit(Event::Permission {
        tool: tool.clone(),
        allowed,
        reason,
    });
    let outcome = match request
        .options
        .iter()
        .find(|option| option_kind(&option.kind) == wanted)
    {
        Some(option) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
            option.option_id.clone(),
        )),
        None => {
            shared.emit(Event::Blocked(format!(
                "the agent asked leave for {tool} and offered no {wanted} option"
            )));
            RequestPermissionOutcome::Cancelled
        }
    };
    serde_json::to_value(RequestPermissionResponse::new(outcome))
        .map_err(|error| RpcError::internal_error().data(error.to_string()))
}

/// Answers `elicitation/create` through the elicitor, on a thread of its own: it may wait for a person. Another session's
/// question is cancelled without asking, and one that cannot be put to a person, or an answer that does not fit its form, is
/// cancelled and said in an [`Event::Blocked`]. Never an error, as with a permission request.
async fn elicitation(shared: &Arc<Shared>, params: Value) -> Value {
    let cancel = || json!({"action": "cancel"});
    let Some(elicitor) = shared.elicitor.clone() else {
        return cancel();
    };
    if params.get("sessionId").is_some_and(Value::is_string) && !shared.is_mine(&params) {
        return cancel();
    }
    let request = match elicit::read(&params, shared.elicitation_limits) {
        Ok(request) => request,
        Err(why) => {
            shared.emit(Event::Blocked(why));
            return cancel();
        }
    };
    let (answer, answered) = oneshot::channel();
    let asked = request.clone();
    let spawned = std::thread::Builder::new()
        .name("coqui-acp-elicitation".to_owned())
        .spawn(move || {
            let _ = answer.send(elicitor.elicit(&asked));
        });
    if spawned.is_err() {
        shared.emit(Event::Blocked(
            "nothing could be started to put the agent's question".to_owned(),
        ));
        return cancel();
    }
    match answered.await.unwrap_or(ElicitationAnswer::Cancel) {
        ElicitationAnswer::Accept(content) => match elicit::fits(&request, &content) {
            Ok(()) => json!({"action": "accept", "content": content}),
            Err(why) => {
                shared.emit(Event::Blocked(why));
                cancel()
            }
        },
        ElicitationAnswer::Decline => json!({"action": "decline"}),
        ElicitationAnswer::Cancel => cancel(),
    }
}

/// Runs `future`, or gives up. The timer thread ends as soon as the future does.
async fn within<T>(limit: Option<Duration>, future: impl Future<Output = T>) -> Result<T, ()> {
    let Some(limit) = limit else {
        return Ok(future.await);
    };
    let (stop, stopped) = mpsc::channel::<()>();
    let (fire, fired) = oneshot::channel::<()>();
    std::thread::Builder::new()
        .name("coqui-acp-timer".to_owned())
        .spawn(move || {
            if stopped.recv_timeout(limit) == Err(RecvTimeoutError::Timeout) {
                let _ = fire.send(());
            }
        })
        .map_err(|_| ())?;
    let future = core::pin::pin!(future);
    let result = match future::select(future, fired).await {
        Either::Left((value, _)) => Ok(value),
        Either::Right(_) => Err(()),
    };
    drop(stop);
    result
}

async fn call(
    cx: &ConnectionTo<Agent>,
    method: &str,
    params: Value,
    limit: Option<Duration>,
) -> Result<Value, Error> {
    let message =
        UntypedMessage::new(method, params).map_err(|error| Error::Refused(error.to_string()))?;
    match within(limit, cx.send_request(message).block_task()).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(Error::Refused(format!("{method}: {}", error_text(&error)))),
        Err(()) => Err(Error::Timeout {
            what: method.to_owned(),
            limit: limit.unwrap_or_default(),
        }),
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

struct OptionRequest<'a> {
    what: &'a str,
    wanted: &'a str,
    explicit: Option<&'a str>,
    category: SessionConfigOptionCategory,
    fallback: &'a str,
}

/// Sets `wanted` through the named option, or the one of the category, or the one with the fallback id.
/// What could not be set is a notice: a session can still run.
async fn set_option(
    cx: &ConnectionTo<Agent>,
    shared: &Shared,
    session: &str,
    options: &[SessionConfigOption],
    request: OptionRequest<'_>,
    limit: Option<Duration>,
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
        shared.emit(Event::Notice(format!(
            "the agent offers no {what} option; the requested {what} {wanted} was not applied"
        )));
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
            let offered = list
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            shared.emit(Event::Notice(format!(
                "the agent's {what} option has no value {wanted}; it offers {offered}"
            )));
            return;
        }
    };
    let params = json!({"sessionId": session, "configId": &*option.id.0, "value": value});
    if let Err(error) = call(cx, "session/set_config_option", params, limit).await {
        shared.emit(Event::Notice(format!(
            "the {what} was not applied: {error}"
        )));
    }
}

fn session_params(options: &Builder, session: Option<&str>) -> Value {
    let mut params = json!({"cwd": options.cwd, "mcpServers": options.mcp_servers});
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

/// Whether a capability is set: `true` or an object. Absent, `null` and `false` are not.
fn offered(value: Option<&Value>) -> bool {
    value.is_some_and(|value| !matches!(value, Value::Null | Value::Bool(false)))
}

/// What a describing call's failure says, as an [`Answer::Unknown`].
fn not_told<T>(error: &Error, limits: &DescribeLimits) -> Answer<T> {
    Answer::Unknown(describe::cut(
        &format!("the agent could not say: {error}"),
        limits.text,
    ))
}

/// `session/list`, page by page from the cursor each page gives, until one gives none, the sessions limit is reached
/// or the page limit is. The first page decides the answer; a later one that fails ends the list there. A list cut
/// short by a failure or a limit says why in [`SessionList::incomplete`].
async fn listing(
    cx: &ConnectionTo<Agent>,
    limits: &DescribeLimits,
    limit: Option<Duration>,
) -> Answer<SessionList> {
    if limits.pages == 0 {
        return Answer::Unknown("the sessions were not asked: the page limit is 0".to_owned());
    }
    let mut kept = Vec::new();
    let mut cursor: Option<String> = None;
    let mut incomplete = None;
    for page in 0..limits.pages {
        let params = cursor
            .take()
            .map_or_else(|| json!({}), |cursor| json!({"cursor": cursor}));
        let read = match call(cx, "session/list", params, limit).await {
            Ok(answer) => describe::sessions_page(answer, limits, &mut kept),
            Err(error) if page == 0 => return not_told(&error, limits),
            Err(error) => {
                incomplete = Some(error.to_string());
                break;
            }
        };
        let Some(read) = read else {
            if page == 0 {
                return Answer::Unknown("the agent's sessions could not be read".to_owned());
            }
            incomplete = Some("the agent's sessions could not be read".to_owned());
            break;
        };
        // A limit stops the list short only when it leaves out a session or a page the agent offers.
        if read.cut || (read.next.is_some() && kept.len() >= limits.sessions) {
            return listed(kept, Some("the sessions limit was reached".to_owned()));
        }
        cursor = read.next;
        if cursor.is_none() {
            break;
        }
    }
    let incomplete = match incomplete {
        Some(why) => Some(describe::cut(
            &format!("a later page failed: {why}"),
            limits.text,
        )),
        None => cursor.map(|_| "the page limit was reached".to_owned()),
    };
    listed(kept, incomplete)
}

fn listed(sessions: Vec<SessionInfo>, incomplete: Option<String>) -> Answer<SessionList> {
    Answer::Known(SessionList {
        sessions,
        incomplete,
    })
}

/// Asks what `init` says the agent offers, and nothing it does not: no session is opened.
async fn describing(
    cx: &ConnectionTo<Agent>,
    init: &Value,
    limits: &DescribeLimits,
    limit: Option<Duration>,
) -> Description {
    let capabilities = &init["agentCapabilities"];
    let extension = capabilities
        .get("_meta")
        .and_then(|meta| meta.get(SESSION_OPTIONS_CAPABILITY));
    let (options, modes) = if offered(extension) {
        match call(cx, SESSION_OPTIONS_METHOD, json!({}), limit).await {
            Ok(answer) => (
                describe::options(&answer, limits),
                describe::modes(&answer, limits),
            ),
            Err(error) => (not_told(&error, limits), not_told(&error, limits)),
        }
    } else {
        (Answer::NotSupported, Answer::NotSupported)
    };
    let listed = capabilities
        .get("sessionCapabilities")
        .and_then(|session| session.get("list"))
        .is_some_and(Value::is_object);
    let sessions = if listed {
        listing(cx, limits, limit).await
    } else {
        Answer::NotSupported
    };
    Description {
        options,
        modes,
        sessions,
    }
}

fn initialize_params(name: &str, version: &str, elicits: bool) -> Value {
    let mut params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
        "clientInfo": {"name": name, "version": version},
    });
    if elicits {
        params["clientCapabilities"]["elicitation"] = json!({"form": {}});
    }
    params
}

/// initialize, authenticate, new or resume, config options. The session id.
async fn open(
    cx: &ConnectionTo<Agent>,
    options: &Builder,
    shared: &Arc<Shared>,
) -> Result<String, Error> {
    let init = call(
        cx,
        "initialize",
        initialize_params(
            &options.client_name,
            &options.client_version,
            options.elicitor.is_some(),
        ),
        options.initialize_timeout,
    )
    .await?;
    let init: InitializeResponse = serde_json::from_value(init)
        .map_err(|_| Error::Refused("initialize: unreadable answer".to_owned()))?;
    #[allow(clippy::needless_borrows_for_generic_args)]
    let version = serde_json::to_value(init.protocol_version)
        .ok()
        .and_then(|value| value.as_u64());
    if version != Some(PROTOCOL_VERSION) {
        return Err(Error::Unsupported(
            "the agent does not speak ACP version 1".to_owned(),
        ));
    }
    let capabilities = &init.agent_capabilities;
    let extension = options.turn_end_by.as_deref().is_some_and(|key| {
        capabilities
            .meta
            .as_ref()
            .and_then(|meta| meta.get(key))
            .is_some_and(|value| !matches!(value, Value::Null | Value::Bool(false)))
    });
    shared.extension.store(extension, Ordering::SeqCst);
    if !extension && needs_http_mcp(&options.mcp_servers) && !capabilities.mcp_capabilities.http {
        return Err(Error::Unsupported(
            "the agent does not accept MCP servers over HTTP, which this session needs".to_owned(),
        ));
    }
    if let Some(auth) = &options.authenticate {
        let mut params = json!({"methodId": auth.method});
        params["_meta"] = Value::Object(auth.meta.clone());
        call(cx, "authenticate", params, options.request_timeout).await?;
    }
    let methods = init
        .auth_methods
        .iter()
        .map(|method| match method {
            AuthMethod::Agent(agent) => agent.id.0.to_string(),
            _ => "terminal".to_owned(),
        })
        .collect::<Vec<_>>();
    let described = |error: Error| match error {
        Error::Refused(text) if text.contains("auth") || text.contains("-32000") => {
            let listed = if methods.is_empty() {
                "none".to_owned()
            } else {
                methods.join(", ")
            };
            Error::Refused(format!(
                "{text} (the agent's authentication methods: {listed})"
            ))
        }
        other => other,
    };
    // Known before the agent answers, so what it sends of this session meanwhile, a replay or a turn it kept running, is this session's.
    if let Opening::Load(id) | Opening::Resume(id) = &options.opening {
        *shared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(id.clone());
    }
    let (session, config) = match &options.opening {
        Opening::New => {
            let answer = call(
                cx,
                "session/new",
                session_params(options, None),
                options.request_timeout,
            )
            .await
            .map_err(described)?;
            let new: NewSessionResponse = serde_json::from_value(answer)
                .map_err(|_| Error::Refused("session/new: unreadable answer".to_owned()))?;
            (
                new.session_id.0.to_string(),
                new.config_options.unwrap_or_default(),
            )
        }
        Opening::Load(id) => {
            if !capabilities.load_session {
                return Err(Error::Unsupported(
                    "the agent cannot load a session".to_owned(),
                ));
            }
            call(
                cx,
                "session/load",
                session_params(options, Some(id)),
                options.request_timeout,
            )
            .await
            .map_err(described)?;
            (id.clone(), Vec::new())
        }
        Opening::Resume(id) => {
            let (method, replays) = if capabilities.session_capabilities.resume.is_some() {
                ("session/resume", false)
            } else if capabilities.load_session {
                ("session/load", true)
            } else {
                return Err(Error::Unsupported(
                    "the agent can neither resume nor load a session".to_owned(),
                ));
            };
            shared.suppress.store(replays, Ordering::SeqCst);
            let answer = call(
                cx,
                method,
                session_params(options, Some(id)),
                options.request_timeout,
            )
            .await;
            shared.suppress.store(false, Ordering::SeqCst);
            let answer = answer.map_err(described)?;
            // An agent may name the session by a key of its own.
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
    *shared
        .session
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(session.clone());
    if !extension {
        if let Some(model) = &options.model {
            let request = OptionRequest {
                what: "model",
                wanted: model,
                explicit: options.model_option.as_deref(),
                category: SessionConfigOptionCategory::Model,
                fallback: "model",
            };
            set_option(
                cx,
                shared,
                &session,
                &config,
                request,
                options.request_timeout,
            )
            .await;
        }
        if let Some(effort) = &options.effort {
            let request = OptionRequest {
                what: "effort",
                wanted: effort,
                explicit: options.effort_option.as_deref(),
                category: SessionConfigOptionCategory::ThoughtLevel,
                fallback: "effort",
            };
            set_option(
                cx,
                shared,
                &session,
                &config,
                request,
                options.request_timeout,
            )
            .await;
        }
    }
    Ok(session)
}

fn send_prompt(
    cx: &ConnectionTo<Agent>,
    shared: &Arc<Shared>,
    session: &str,
    text: String,
) -> Result<(), RpcError> {
    shared.turn().begin();
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
    cx: ConnectionTo<Agent>,
    options: Builder,
    shared: Arc<Shared>,
    mut commands: async_mpsc::UnboundedReceiver<Command>,
    ready: mpsc::Sender<Result<(), Error>>,
) -> Result<(), RpcError> {
    let session = match open(&cx, &options, &shared).await {
        Ok(session) => session,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    if let Some(prompt) = options.prompt.clone()
        && let Err(error) = send_prompt(&cx, &shared, &session, prompt)
    {
        let _ = ready.send(Err(Error::Refused(error.message)));
        return Ok(());
    }
    let _ = ready.send(Ok(()));
    let closed = cx.incoming_closed();
    let mut closed = core::pin::pin!(closed);
    loop {
        let next = commands.next();
        let command = match future::select(core::pin::pin!(next), closed.as_mut()).await {
            Either::Left((command, _)) => command,
            Either::Right(_) => {
                shared.disconnected("the agent closed its connection");
                break;
            }
        };
        let cancel = || UntypedMessage::new("session/cancel", json!({"sessionId": session}));
        match command {
            Some(Command::Prompt(text)) => {
                if let Err(error) = send_prompt(&cx, &shared, &session, text) {
                    shared.emit(Event::Failed(error_text(&error)));
                }
            }
            Some(Command::Notify(method, params)) => {
                if let Ok(message) = UntypedMessage::new(method.as_str(), params) {
                    let _ = cx.send_notification(message);
                }
            }
            Some(Command::Cancel) => {
                if let Ok(message) = cancel() {
                    let _ = cx.send_notification(message);
                }
            }
            Some(Command::Close(ack)) => {
                shared.closing.store(true, Ordering::SeqCst);
                let _ = call(
                    &cx,
                    "session/close",
                    json!({"sessionId": session}),
                    options.close_grace,
                )
                .await;
                let _ = ack.send(());
                break;
            }
            Some(Command::Abort) => {
                if let Ok(message) = cancel() {
                    let _ = cx.send_notification(message);
                }
                break;
            }
            None => break,
        }
    }
    Ok(())
}

/// One open session. Every method takes `&self`, so one thread can read events while another sends.
pub struct Session {
    shared: Arc<Shared>,
    events: Mutex<mpsc::Receiver<Event>>,
    commands: async_mpsc::UnboundedSender<Command>,
    on_close: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    close_grace: Option<Duration>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id())
            .field("lost", &self.connection_lost())
            .finish_non_exhaustive()
    }
}

impl Session {
    /// The session id the agent gave.
    #[must_use]
    pub fn id(&self) -> Option<String> {
        self.shared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether the agent took over turn endings: see [`Builder::turn_end_by`].
    #[must_use]
    pub fn extension_active(&self) -> bool {
        self.shared.extension.load(Ordering::SeqCst)
    }

    /// Whether the connection ended without this side closing it.
    #[must_use]
    pub fn connection_lost(&self) -> bool {
        self.shared.lost.load(Ordering::SeqCst)
    }

    /// Sends a turn. It is not retried: after a lost connection, its outcome is unknown.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`].
    pub fn send(&self, text: impl Into<String>) -> Result<(), Error> {
        self.commands
            .unbounded_send(Command::Prompt(text.into()))
            .map_err(|_| Error::Closed)
    }

    /// Sends a notification, typically an extension's.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`].
    pub fn notify(&self, method: impl Into<String>, params: Value) -> Result<(), Error> {
        self.commands
            .unbounded_send(Command::Notify(method.into(), params))
            .map_err(|_| Error::Closed)
    }

    /// The next event, or `None` when none came within `timeout` or the session has ended.
    #[must_use]
    pub fn next_event(&self, timeout: Duration) -> Option<Event> {
        let events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        let event = events.recv_timeout(timeout).ok()?;
        self.shared.pending.fetch_sub(1, Ordering::SeqCst);
        Some(event)
    }

    /// Asks the agent to cancel the turn: `session/cancel`. Its answer arrives as an event.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`].
    pub fn cancel(&self) -> Result<(), Error> {
        self.commands
            .unbounded_send(Command::Cancel)
            .map_err(|_| Error::Closed)
    }

    /// Closes the session, waiting for the agent no longer than the close grace, and runs `on_close`.
    pub fn close(&self) {
        let (ack, acked) = mpsc::channel();
        self.shared.closing.store(true, Ordering::SeqCst);
        if self.commands.unbounded_send(Command::Close(ack)).is_ok() {
            match self.close_grace {
                Some(limit) => drop(acked.recv_timeout(limit)),
                None => drop(acked.recv()),
            }
        }
        self.release();
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            // The connection ends once its task does; do not wait on a peer that is gone.
            std::thread::spawn(move || {
                let _ = thread.join();
            });
        }
    }

    fn release(&self) {
        if !self.shared.closing.swap(true, Ordering::SeqCst) && self.shared.close_on_drop {
            // Never closed: say so to the agent, which may hold a process.
            let (ack, _) = mpsc::channel();
            let _ = self.commands.unbounded_send(Command::Close(ack));
        }
        let _ = self.commands.unbounded_send(Command::Abort);
        if let Some(on_close) = self
            .on_close
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            on_close();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.release();
    }
}

/// A JSON-RPC error an agent answered a call with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcFailure {
    /// The JSON-RPC error code; `-1` when no answer came in time.
    pub code: i32,
    /// The agent's message, with its detail.
    pub message: String,
}

/// Calls that are no session, for a question or an order: connects, initializes, authenticates when asked, and makes each in
/// turn. See [`Exchange::run`].
#[derive(Clone, Debug)]
#[must_use]
pub struct Exchange {
    client_name: String,
    client_version: String,
    authenticate: Option<Authenticate>,
    initialize_timeout: Option<Duration>,
    request_timeout: Option<Duration>,
    describe_limits: DescribeLimits,
}

impl Default for Exchange {
    fn default() -> Self {
        Self::new()
    }
}

impl Exchange {
    /// Calls as `coqui-acp`, with the waits [`Client::builder`] starts with and nothing to authenticate.
    pub fn new() -> Self {
        Self {
            client_name: "coqui-acp".to_owned(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            authenticate: None,
            initialize_timeout: Some(Duration::from_secs(20)),
            request_timeout: Some(Duration::from_secs(30)),
            describe_limits: DescribeLimits::new(),
        }
    }

    /// What this client calls itself in `initialize`.
    pub fn client_info(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.client_name = name.into();
        self.client_version = version.into();
        self
    }

    /// Calls `authenticate` before the first call.
    pub fn authenticate(mut self, authenticate: Authenticate) -> Self {
        self.authenticate = Some(authenticate);
        self
    }

    /// How long `initialize` may take; `None` waits without limit. Default 20 s.
    pub const fn initialize_timeout(mut self, limit: Option<Duration>) -> Self {
        self.initialize_timeout = limit;
        self
    }

    /// How long each call may take; `None` waits without limit. Default 30 s.
    pub const fn request_timeout(mut self, limit: Option<Duration>) -> Self {
        self.request_timeout = limit;
        self
    }

    /// What [`Exchange::describe`] keeps of the agent's answer, and how many pages of sessions it asks for. Default
    /// [`DescribeLimits::new`].
    pub const fn describe_limits(mut self, limits: DescribeLimits) -> Self {
        self.describe_limits = limits;
        self
    }

    /// Connects over `transport` and makes `calls`, as `(method, params)`. The outer error is why the exchange could not take
    /// place; each call has its own.
    ///
    /// # Errors
    ///
    /// The connection, `initialize` or `authenticate` failed.
    ///
    /// # Panics
    ///
    /// Never by design; a panic on its worker thread is reported as [`Error::Closed`].
    pub fn run(
        self,
        transport: impl ConnectTo<Role> + 'static,
        calls: Vec<(String, Value)>,
    ) -> Result<Vec<Result<Value, RpcFailure>>, Error> {
        let request_timeout = self.request_timeout;
        let worker = std::thread::Builder::new()
            .name("coqui-acp-exchange".to_owned())
            .spawn(move || {
                let name = self.client_name.clone();
                futures::executor::block_on(Role.builder().name(name).connect_with(
                    transport,
                    async move |cx| {
                        if let Err(error) = self.begin(&cx).await {
                            return Ok(Err(error));
                        }
                        let mut results = Vec::new();
                        for (method, params) in calls {
                            let message = UntypedMessage::new(method.as_str(), params)?;
                            results.push(
                                match within(request_timeout, cx.send_request(message).block_task())
                                    .await
                                {
                                    Ok(Ok(value)) => Ok(value),
                                    Ok(Err(error)) => Err(RpcFailure {
                                        code: i32::from(error.code),
                                        message: error_text(&error),
                                    }),
                                    Err(()) => Err(RpcFailure {
                                        code: -1,
                                        message: "no answer in time".to_owned(),
                                    }),
                                },
                            );
                        }
                        Ok(Ok(results))
                    },
                ))
            })
            .map_err(|_| Error::Thread)?;
        worker.join().map_err(|_| Error::Closed)?.map_err(|error| {
            Error::Refused(format!("the ACP connection failed: {}", error_text(&error)))
        })?
    }

    /// Initializes and authenticates when asked: `initialize`'s answer.
    async fn begin(&self, cx: &ConnectionTo<Agent>) -> Result<Value, Error> {
        let params = initialize_params(&self.client_name, &self.client_version, false);
        let init = call(cx, "initialize", params, self.initialize_timeout).await?;
        if let Some(auth) = &self.authenticate {
            let mut params = json!({"methodId": auth.method});
            params["_meta"] = Value::Object(auth.meta.clone());
            call(cx, "authenticate", params, self.request_timeout).await?;
        }
        Ok(init)
    }

    /// What the agent offers before any session, asked over `transport` without opening one: the options a
    /// session can start with and its modes, by [`SESSION_OPTIONS_METHOD`] when `initialize` advertises
    /// [`SESSION_OPTIONS_CAPABILITY`] in `agentCapabilities._meta`, and its sessions, by `session/list` when it
    /// advertises `sessionCapabilities.list`. What it does not advertise is [`Answer::NotSupported`], and nothing is
    /// sent for it; a call that fails or times out is [`Answer::Unknown`]. What it keeps, and how many pages of
    /// sessions it asks for, is [`Exchange::describe_limits`].
    ///
    /// # Errors
    ///
    /// The connection, `initialize` or `authenticate` failed.
    pub fn describe(self, transport: impl ConnectTo<Role> + 'static) -> Result<Description, Error> {
        let request_timeout = self.request_timeout;
        let worker = std::thread::Builder::new()
            .name("coqui-acp-describe".to_owned())
            .spawn(move || {
                let name = self.client_name.clone();
                futures::executor::block_on(Role.builder().name(name).connect_with(
                    transport,
                    async move |cx| {
                        Ok(match self.begin(&cx).await {
                            Ok(init) => {
                                Ok(
                                    describing(&cx, &init, &self.describe_limits, request_timeout)
                                        .await,
                                )
                            }
                            Err(error) => Err(error),
                        })
                    },
                ))
            })
            .map_err(|_| Error::Thread)?;
        worker.join().map_err(|_| Error::Closed)?.map_err(|error| {
            Error::Refused(format!("the ACP connection failed: {}", error_text(&error)))
        })?
    }
}

#[cfg(test)]
mod error_text_tests {
    use super::*;

    fn error(data: Option<Value>) -> RpcError {
        let mut error = RpcError::internal_error();
        error.data = data;
        error
    }

    #[test]
    fn a_reason_given_as_text_is_kept() {
        let text = error_text(&error(Some(json!("out of credits"))));
        assert_eq!(text, "Internal error: out of credits");
    }

    #[test]
    fn a_reason_inside_an_object_is_kept() {
        let data = json!({"message": "API error (status 402 Payment Required): out of credits", "http_status": 402});
        let text = error_text(&error(Some(data)));
        assert_eq!(
            text,
            "Internal error: API error (status 402 Payment Required): out of credits"
        );
    }

    #[test]
    fn data_with_no_reason_adds_nothing() {
        assert_eq!(
            error_text(&error(Some(json!({"http_status": 402})))),
            "Internal error"
        );
        assert_eq!(
            error_text(&error(Some(json!({"message": ""})))),
            "Internal error"
        );
        assert_eq!(error_text(&error(Some(json!([1, 2])))), "Internal error");
        assert_eq!(error_text(&error(None)), "Internal error");
    }
}
