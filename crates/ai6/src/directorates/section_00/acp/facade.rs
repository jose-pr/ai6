//! The ACP agent side over an [`AgentHandler`]: [`AcpFacade`].

use agent_client_protocol::{
    Agent, Client, ConnectTo, ConnectionTo, Error, Responder, UntypedMessage,
    on_receive_notification, on_receive_request,
};
use guatiao::library::{Object, ProviderError};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::{EVENT_NOTIFICATION, EVENTS_CAPABILITY, LAUNCH_META_KEY, LaunchMeta, ResumeMeta};
use crate::directorates::q::terminal::AgentTerminal;
use crate::directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentMessage, AgentSession, LIMIT_EVENT,
};

/// Makes the terminal an operation's handler spawns through: one per start or resume.
pub type TerminalFactory =
    Arc<dyn Fn() -> Result<Object<dyn AgentTerminal>, ProviderError> + Send + Sync>;

/// Judges the `_meta` of an `authenticate` request.
pub type Credential = dyn Fn(&Map<String, Value>) -> bool + Send + Sync;

/// A method every client must authenticate with before it opens or resumes a session.
#[derive(Clone)]
pub struct FacadeAuth {
    /// The advertised method id.
    pub method: String,
    /// A name for people.
    pub name: String,
    /// Whether the `authenticate` request's `_meta` proves the client.
    pub check: Arc<Credential>,
}

struct Operation {
    id: String,
    cwd: String,
    launch: Mutex<Option<LaunchMeta>>,
    session: Mutex<Option<Arc<Object<dyn AgentSession>>>>,
    /// `session/prompt` calls the current turn has yet to answer.
    pending: Mutex<Vec<Responder<Value>>>,
    sink: Mutex<Option<ConnectionTo<Client>>>,
    stop: AtomicBool,
}

struct Inner {
    handler: Arc<dyn AgentHandler>,
    terminal: TerminalFactory,
    auth: Option<FacadeAuth>,
    operations: Mutex<HashMap<String, Arc<Operation>>>,
}

/// Serves any [`AgentHandler`] as an ACP agent: one operation per ACP session, each started
/// on its first `session/prompt` from the [`LaunchMeta`] its `session/new` carried.
#[derive(Clone)]
pub struct AcpFacade {
    inner: Arc<Inner>,
}

impl AcpFacade {
    /// A façade over `handler`, whose operations spawn through `terminal`.
    pub fn new(handler: Arc<dyn AgentHandler>, terminal: TerminalFactory) -> Self {
        Self {
            inner: Arc::new(Inner {
                handler,
                terminal,
                auth: None,
                operations: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Requires clients to authenticate before opening a session.
    #[must_use]
    pub fn with_auth(self, auth: FacadeAuth) -> Self {
        let inner = &*self.inner;
        Self {
            inner: Arc::new(Inner {
                handler: inner.handler.clone(),
                terminal: inner.terminal.clone(),
                auth: Some(auth),
                operations: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Serves one connection until its transport closes, on a thread of its own.
    pub fn serve(&self, transport: impl ConnectTo<Agent> + 'static) -> std::thread::JoinHandle<()> {
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("ai6-acp-facade".to_owned())
            .spawn(move || {
                let authenticated = Arc::new(AtomicBool::new(inner.auth.is_none()));
                let for_requests = inner.clone();
                let for_notifications = inner.clone();
                let _ = futures::executor::block_on(
                    Agent
                        .builder()
                        .name("ai6-facade")
                        .on_receive_request(
                            async move |message: UntypedMessage,
                                        responder: Responder<Value>,
                                        cx: ConnectionTo<Client>| {
                                handle_request(
                                    &for_requests,
                                    &authenticated,
                                    message,
                                    responder,
                                    cx,
                                );
                                Ok(())
                            },
                            on_receive_request!(),
                        )
                        .on_receive_notification(
                            async move |message: UntypedMessage, _cx: ConnectionTo<Client>| {
                                handle_notification(&for_notifications, &message);
                                Ok(())
                            },
                            on_receive_notification!(),
                        )
                        .connect_to(transport),
                );
            })
            .expect("a thread can be spawned")
    }
}

fn text_of(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}

fn meta_of<T: serde::de::DeserializeOwned>(params: &Value) -> Result<T, Error> {
    let raw = params
        .get("_meta")
        .and_then(|meta| meta.get(LAUNCH_META_KEY))
        .ok_or_else(|| {
            Error::invalid_params().data(format!("the request needs _meta[{LAUNCH_META_KEY}]"))
        })?;
    serde_json::from_value(raw.clone())
        .map_err(|error| Error::invalid_params().data(format!("{LAUNCH_META_KEY}: {error}")))
}

fn failure(error: &ProviderError) -> Error {
    Error::internal_error().data(error.to_string())
}

fn operation(inner: &Inner, params: &Value) -> Result<Arc<Operation>, Error> {
    let id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(Error::invalid_params)?;
    inner
        .operations
        .lock()
        .ok()
        .and_then(|operations| operations.get(id).cloned())
        .ok_or_else(|| Error::resource_not_found(Some(id.to_owned())))
}

fn handle_request(
    inner: &Arc<Inner>,
    authenticated: &Arc<AtomicBool>,
    message: UntypedMessage,
    responder: Responder<Value>,
    cx: ConnectionTo<Client>,
) {
    let method = message.method().to_owned();
    let params = message.params().clone();
    let outcome = match method.as_str() {
        "initialize" => Ok(initialize(inner)),
        "authenticate" => authenticate(inner, authenticated, &params),
        _ if !authenticated.load(Ordering::SeqCst) => Err(Error::auth_required()),
        "session/new" => new_session(inner, &params, &cx),
        "session/prompt" => {
            return prompt(inner, &params, responder, cx);
        }
        "session/resume" | "session/load" => {
            return resume(inner, &params, responder, cx);
        }
        "session/close" => {
            return close(inner, &params, responder);
        }
        _ => Err(Error::method_not_found()),
    };
    let _ = match outcome {
        Ok(value) => responder.respond(value),
        Err(error) => responder.respond_with_error(error),
    };
}

fn handle_notification(inner: &Arc<Inner>, message: &UntypedMessage) {
    if message.method() != "session/cancel" {
        return;
    }
    let Ok(operation) = operation(inner, message.params()) else {
        return;
    };
    std::thread::spawn(move || {
        if let Some(session) = operation
            .session
            .lock()
            .ok()
            .and_then(|session| session.clone())
        {
            let _ = session.interrupt();
        }
        answer_all(&operation, |responder| {
            responder.respond(json!({"stopReason": "cancelled"}))
        });
    });
}

fn initialize(inner: &Inner) -> Value {
    let methods = inner
        .auth
        .iter()
        .map(|auth| json!({"id": auth.method, "name": auth.name}))
        .collect::<Vec<_>>();
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {},
            "mcpCapabilities": {"http": false, "sse": false},
            "sessionCapabilities": {"resume": {}, "close": {}},
            "_meta": {EVENTS_CAPABILITY: true},
        },
        "authMethods": methods,
        "agentInfo": {"name": "ai6", "version": env!("CARGO_PKG_VERSION")},
    })
}

fn authenticate(inner: &Inner, authenticated: &AtomicBool, params: &Value) -> Result<Value, Error> {
    let Some(auth) = &inner.auth else {
        return Ok(json!({}));
    };
    let method = params.get("methodId").and_then(Value::as_str);
    let meta = params.get("_meta").and_then(Value::as_object);
    if method == Some(auth.method.as_str()) && meta.is_some_and(|meta| (auth.check)(meta)) {
        authenticated.store(true, Ordering::SeqCst);
        Ok(json!({}))
    } else {
        Err(Error::auth_required().data("the credential was not accepted"))
    }
}

fn new_session(
    inner: &Arc<Inner>,
    params: &Value,
    cx: &ConnectionTo<Client>,
) -> Result<Value, Error> {
    let launch: LaunchMeta = meta_of(params)?;
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(Error::invalid_params)?
        .to_owned();
    let id = uuid::Uuid::new_v4().to_string();
    let operation = Arc::new(Operation {
        id: id.clone(),
        cwd,
        launch: Mutex::new(Some(launch)),
        session: Mutex::new(None),
        pending: Mutex::new(Vec::new()),
        sink: Mutex::new(Some(cx.clone())),
        stop: AtomicBool::new(false),
    });
    inner
        .operations
        .lock()
        .map_err(|_| Error::internal_error())?
        .insert(id.clone(), operation);
    Ok(json!({"sessionId": id}))
}

fn prompt(
    inner: &Arc<Inner>,
    params: &Value,
    responder: Responder<Value>,
    cx: ConnectionTo<Client>,
) {
    let operation = match operation(inner, params) {
        Ok(operation) => operation,
        Err(error) => {
            let _ = responder.respond_with_error(error);
            return;
        }
    };
    let text = text_of(params);
    if let Ok(mut sink) = operation.sink.lock() {
        *sink = Some(cx);
    }
    if let Ok(mut pending) = operation.pending.lock() {
        pending.push(responder);
    }
    let inner = inner.clone();
    std::thread::spawn(move || {
        let outcome = send_or_start(&inner, &operation, text);
        if let Err(error) = outcome {
            answer_all(&operation, |responder| {
                responder.respond_with_error(error.clone())
            });
        }
    });
}

/// Starts the operation on the first prompt, else submits the prompt to it.
fn send_or_start(
    inner: &Arc<Inner>,
    operation: &Arc<Operation>,
    text: String,
) -> Result<(), Error> {
    let existing = operation
        .session
        .lock()
        .ok()
        .and_then(|session| session.clone());
    if let Some(session) = existing {
        return session
            .send_message(AgentMessage { text })
            .map_err(|error| failure(&error));
    }
    let launch = operation
        .launch
        .lock()
        .map_err(|_| Error::internal_error())?
        .take()
        .ok_or_else(|| Error::internal_error().data("the operation is already starting"))?;
    let terminal = (inner.terminal)().map_err(|error| failure(&error))?;
    let start = launch.into_start(text, operation.cwd.clone());
    let session = inner
        .handler
        .start(start, terminal)
        .map_err(|error| failure(&error))?;
    attach(inner, operation, session);
    Ok(())
}

fn resume(
    inner: &Arc<Inner>,
    params: &Value,
    responder: Responder<Value>,
    cx: ConnectionTo<Client>,
) {
    let inner = inner.clone();
    let params = params.clone();
    std::thread::spawn(move || {
        let outcome = (|| {
            let id = params
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(Error::invalid_params)?
                .to_owned();
            if let Some(live) = inner
                .operations
                .lock()
                .ok()
                .and_then(|operations| operations.get(&id).cloned())
            {
                if let Ok(mut sink) = live.sink.lock() {
                    *sink = Some(cx.clone());
                }
                return Ok(json!({}));
            }
            let meta: ResumeMeta = meta_of(&params)?;
            let cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .ok_or_else(Error::invalid_params)?
                .to_owned();
            let terminal = (inner.terminal)().map_err(|error| failure(&error))?;
            let session = inner
                .handler
                .resume(meta.into_resume(cwd.clone()), terminal)
                .map_err(|error| failure(&error))?;
            let operation = Arc::new(Operation {
                id: id.clone(),
                cwd,
                launch: Mutex::new(None),
                session: Mutex::new(None),
                pending: Mutex::new(Vec::new()),
                sink: Mutex::new(Some(cx.clone())),
                stop: AtomicBool::new(false),
            });
            inner
                .operations
                .lock()
                .map_err(|_| Error::internal_error())?
                .insert(id, operation.clone());
            attach(&inner, &operation, session);
            Ok(json!({}))
        })();
        let _ = match outcome {
            Ok(value) => responder.respond(value),
            Err(error) => responder.respond_with_error(error),
        };
    });
}

fn close(inner: &Arc<Inner>, params: &Value, responder: Responder<Value>) {
    let operation = match operation(inner, params) {
        Ok(operation) => operation,
        Err(error) => {
            let _ = responder.respond_with_error(error);
            return;
        }
    };
    let inner = inner.clone();
    std::thread::spawn(move || {
        operation.stop.store(true, Ordering::SeqCst);
        let session = operation
            .session
            .lock()
            .ok()
            .and_then(|session| session.clone());
        let closed = session.map_or(Ok(()), |session| session.close());
        if let Ok(mut operations) = inner.operations.lock() {
            operations.remove(&operation.id);
        }
        answer_all(&operation, |responder| {
            responder.respond(json!({"stopReason": "cancelled"}))
        });
        let _ = match closed {
            Ok(()) => responder.respond(json!({})),
            Err(error) => responder.respond_with_error(failure(&error)),
        };
    });
}

fn answer_all(operation: &Operation, answer: impl Fn(Responder<Value>) -> Result<(), Error>) {
    let pending = operation
        .pending
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default();
    for responder in pending {
        let _ = answer(responder);
    }
}

/// Keeps the started operation and pumps its events to the client until it is closed.
fn attach(inner: &Arc<Inner>, operation: &Arc<Operation>, session: Object<dyn AgentSession>) {
    let session = Arc::new(session);
    if let Ok(mut slot) = operation.session.lock() {
        *slot = Some(session.clone());
    }
    let provider = session.session_id().ok().flatten().map(|id| id.to_string());
    notify(operation, &provider, None);
    let operation = operation.clone();
    let inner = inner.clone();
    std::thread::Builder::new()
        .name("ai6-acp-pump".to_owned())
        .spawn(move || {
            while !operation.stop.load(Ordering::SeqCst) {
                let provider = session.session_id().ok().flatten().map(|id| id.to_string());
                match session.next_event(100) {
                    Ok(Some(event)) => {
                        notify(&operation, &provider, Some(&event));
                        end_turn(&operation, &provider, &event);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let event = AgentEvent {
                            kind: "failed".to_owned(),
                            text: Some(error.to_string()),
                            data: None,
                            limit: None,
                            usage: None,
                            tools: Vec::new(),
                            quota: Vec::new(),
                        };
                        notify(&operation, &provider, Some(&event));
                        end_turn(&operation, &provider, &event);
                        break;
                    }
                }
            }
            if let Ok(mut operations) = inner.operations.lock() {
                operations.remove(&operation.id);
            }
        })
        .ok();
}

/// A terminal event answers the prompts waiting on the turn.
fn end_turn(operation: &Operation, provider: &Option<String>, event: &AgentEvent) {
    let meta = json!({"providerSessionId": provider});
    match event.kind.as_str() {
        "completed" => answer_all(operation, |responder| {
            responder.respond(json!({"stopReason": "end_turn", "_meta": meta}))
        }),
        LIMIT_EVENT => answer_all(operation, |responder| {
            responder.respond(json!({"stopReason": "refusal", "_meta": meta}))
        }),
        "failed" | "blown" => {
            let text = event.text.clone().unwrap_or_else(|| event.kind.clone());
            answer_all(operation, |responder| {
                responder.respond_with_error(Error::internal_error().data(text.clone()))
            });
        }
        _ => {}
    }
}

/// Sends `event` as [`EVENT_NOTIFICATION`] and as the standard updates ACP has for it; no
/// event marks the start of the operation.
fn notify(operation: &Operation, provider: &Option<String>, event: Option<&AgentEvent>) {
    let Some(cx) = operation.sink.lock().ok().and_then(|sink| sink.clone()) else {
        return;
    };
    let send = |method: &str, params: Value| {
        if let Ok(message) = UntypedMessage::new(method, params) {
            let _ = cx.send_notification(message);
        }
    };
    send(
        EVENT_NOTIFICATION,
        json!({
            "sessionId": operation.id,
            "providerSessionId": provider,
            "event": event,
        }),
    );
    let Some(event) = event else {
        return;
    };
    let update = |update: Value| json!({"sessionId": operation.id, "update": update});
    if event.kind == "text"
        && let Some(text) = &event.text
    {
        send(
            "session/update",
            update(json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": text},
            })),
        );
    }
    for tool in &event.tools {
        send(
            "session/update",
            update(json!({
                "sessionUpdate": "tool_call",
                "toolCallId": uuid::Uuid::new_v4().to_string(),
                "title": tool,
                "status": "in_progress",
            })),
        );
    }
    if let Some(usage) = &event.usage
        && usage.cost_micro_usd > 0
    {
        send(
            "session/update",
            update(json!({
                "sessionUpdate": "usage_update",
                "used": usage.input + usage.output + usage.cache_read + usage.cache_write,
                "size": 0,
                "cost": {"amount": usage.cost_micro_usd as f64 / 1_000_000.0, "currency": "USD"},
            })),
        );
    }
}
