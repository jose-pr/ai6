//! The runner's ACP server: a Station connects over the loopback port, authenticates with the
//! runner's token, and drives the Sessions the runner supervises as it would any ACP agent,
//! with the spooled events replayed to whoever attaches.

use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::{
    Agent, Client, ConnectionTo, Error, Responder, UntypedMessage, on_receive_notification,
    on_receive_request,
};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use super::{Hosted, Launch, Runner, lock_of};
use crate::directorates::section_00::acp::{
    EVENT_NOTIFICATION, EVENTS_CAPABILITY, LAUNCH_META_KEY, LaunchMeta, RUNNER_TOKEN_METHOD,
    ResumeMeta, lines_over_io,
};
use crate::directorates::section_00::agent::{AgentMessage, LIMIT_EVENT};

/// The `_meta` key of `session/load`: replay what follows this sequence number.
const AFTER_KEY: &str = "dev.coqui.ai6/after";

/// What one connection holds of one Session: its feeder, and the prompts it waits on.
pub(super) struct Binding {
    key: String,
    /// The `session/new` launch, until the first prompt starts the harness.
    launch: Mutex<Option<(LaunchMeta, String)>>,
    hosted: Mutex<Option<Arc<Hosted>>>,
    pending: Mutex<Vec<Responder<Value>>>,
    stop: AtomicBool,
    stream: TcpStream,
    cx: ConnectionTo<Client>,
}

struct Connection {
    runner: Runner,
    token: String,
    handle: tokio::runtime::Handle,
    authenticated: AtomicBool,
    stream: TcpStream,
    binding: Mutex<Option<Arc<Binding>>>,
}

/// Serves one accepted connection until it closes, on the calling thread.
pub(super) fn serve_connection(
    runner: Runner,
    token: String,
    handle: tokio::runtime::Handle,
    stream: TcpStream,
) {
    let (Ok(reader), Ok(writer), Ok(kept)) =
        (stream.try_clone(), stream.try_clone(), stream.try_clone())
    else {
        return;
    };
    let (lines, _) = lines_over_io(reader, writer, 0);
    let connection = Arc::new(Connection {
        runner,
        token,
        handle,
        authenticated: AtomicBool::new(false),
        stream: kept,
        binding: Mutex::new(None),
    });
    let for_requests = connection.clone();
    let for_notifications = connection.clone();
    let _ = futures::executor::block_on(
        Agent
            .builder()
            .name("ai6-runner")
            .on_receive_request(
                async move |message: UntypedMessage,
                            responder: Responder<Value>,
                            cx: ConnectionTo<Client>| {
                    handle_request(&for_requests, message, responder, cx);
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
            .connect_to(lines),
    );
    if let Some(binding) = lock_of(&connection.binding).take() {
        binding.stop.store(true, Ordering::SeqCst);
    }
}

fn launch_meta<T: serde::de::DeserializeOwned>(params: &Value) -> Result<T, Error> {
    let raw = params
        .get("_meta")
        .and_then(|meta| meta.get(LAUNCH_META_KEY))
        .ok_or_else(|| {
            Error::invalid_params().data(format!("the request needs _meta[{LAUNCH_META_KEY}]"))
        })?;
    serde_json::from_value(raw.clone())
        .map_err(|error| Error::invalid_params().data(format!("{LAUNCH_META_KEY}: {error}")))
}

fn text_of(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}

fn key_of(params: &Value) -> Result<&str, Error> {
    params
        .get("sessionId")
        .or_else(|| params.get("key"))
        .and_then(Value::as_str)
        .ok_or_else(Error::invalid_params)
}

fn not_found(key: &str) -> Error {
    Error::resource_not_found(Some(key.to_owned()))
}

fn handle_request(
    connection: &Arc<Connection>,
    message: UntypedMessage,
    responder: Responder<Value>,
    cx: ConnectionTo<Client>,
) {
    let method = message.method().to_owned();
    let params = message.params().clone();
    let connection = connection.clone();
    // Nothing here may block the connection's own dispatch loop: starting a harness takes
    // as long as it takes.
    std::thread::spawn(move || {
        let mut slot = Some(responder);
        let outcome = dispatch(&connection, &method, &params, &mut slot, &cx);
        if let (Some(outcome), Some(responder)) = (outcome, slot) {
            let _ = match outcome {
                Ok(value) => responder.respond(value),
                Err(error) => responder.respond_with_error(error),
            };
        }
    });
}

/// Answers one request; `None` when a prompt keeps its responder until its turn ends.
fn dispatch(
    connection: &Arc<Connection>,
    method: &str,
    params: &Value,
    slot: &mut Option<Responder<Value>>,
    cx: &ConnectionTo<Client>,
) -> Option<Result<Value, Error>> {
    match method {
        "initialize" => return Some(Ok(initialize())),
        "authenticate" => return Some(authenticate(connection, params)),
        _ if !connection.authenticated.load(Ordering::SeqCst) => {
            return Some(Err(Error::auth_required()));
        }
        _ => {}
    }
    let runner = &connection.runner;
    Some(match method {
        "_ai6/session_status" => (|| {
            let key = key_of(params)?;
            let hosted = runner.hosted(key).ok_or_else(|| not_found(key))?;
            serde_json::to_value(hosted.report())
                .map_err(|error| Error::internal_error().data(error.to_string()))
        })(),
        "_ai6/cable" => (|| {
            let key = key_of(params)?;
            let hosted = runner.hosted(key).ok_or_else(|| not_found(key))?;
            let upstream: super::Upstream =
                serde_json::from_value(params.get("upstream").cloned().unwrap_or(Value::Null))
                    .map_err(|_| Error::invalid_params())?;
            match lock_of(&hosted.cable).as_ref() {
                Some(proxy) => {
                    proxy.set_upstream(upstream);
                    Ok(json!({}))
                }
                None => Err(Error::invalid_params().data("the Session has no Cable endpoint")),
            }
        })(),
        "session/new" => (|| {
            let launch: LaunchMeta = launch_meta(params)?;
            let key = launch
                .key
                .clone()
                .ok_or_else(|| Error::invalid_params().data("the launch names no Session key"))?;
            let cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .ok_or_else(Error::invalid_params)?
                .to_owned();
            bind(
                connection,
                Arc::new(Binding {
                    key: key.clone(),
                    launch: Mutex::new(Some((launch, cwd))),
                    hosted: Mutex::new(None),
                    pending: Mutex::new(Vec::new()),
                    stop: AtomicBool::new(false),
                    stream: connection
                        .stream
                        .try_clone()
                        .map_err(|error| Error::internal_error().data(error.to_string()))?,
                    cx: cx.clone(),
                }),
            );
            Ok(json!({"sessionId": key}))
        })(),
        "session/prompt" => {
            prompt(connection, params, slot.take()?);
            return None;
        }
        "session/resume" => (|| {
            let meta: ResumeMeta = launch_meta(params)?;
            let key = meta
                .key
                .clone()
                .ok_or_else(|| Error::invalid_params().data("the launch names no Session key"))?;
            let cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .ok_or_else(Error::invalid_params)?
                .to_owned();
            let spec = meta
                .handler
                .clone()
                .ok_or_else(|| Error::invalid_params().data("the launch names no handler"))?;
            let pinned = meta.terminal.clone();
            let hosted = connection
                .handle
                .block_on(runner.launch(
                    &key,
                    &spec,
                    pinned.as_deref(),
                    Launch::Resume(meta.into_resume(cwd)),
                ))
                .map_err(|reason| Error::internal_error().data(reason))?;
            let binding = Arc::new(Binding {
                key: key.clone(),
                launch: Mutex::new(None),
                hosted: Mutex::new(Some(hosted.clone())),
                pending: Mutex::new(Vec::new()),
                stop: AtomicBool::new(false),
                stream: connection
                    .stream
                    .try_clone()
                    .map_err(|error| Error::internal_error().data(error.to_string()))?,
                cx: cx.clone(),
            });
            bind(connection, binding.clone());
            feed(connection, &binding, &hosted, 0);
            Ok(json!({"sessionId": key}))
        })(),
        "session/load" => (|| {
            let key = key_of(params)?;
            let hosted = runner.hosted(key).ok_or_else(|| not_found(key))?;
            let after = params
                .get("_meta")
                .and_then(|meta| meta.get(AFTER_KEY))
                .and_then(Value::as_u64)
                .unwrap_or_else(|| lock_of(&hosted.spool).cursor());
            let binding = Arc::new(Binding {
                key: key.to_owned(),
                launch: Mutex::new(None),
                hosted: Mutex::new(Some(hosted.clone())),
                pending: Mutex::new(Vec::new()),
                stop: AtomicBool::new(false),
                stream: connection
                    .stream
                    .try_clone()
                    .map_err(|error| Error::internal_error().data(error.to_string()))?,
                cx: cx.clone(),
            });
            bind(connection, binding.clone());
            feed(connection, &binding, &hosted, after);
            Ok(json!({}))
        })(),
        "session/close" => (|| {
            let key = key_of(params)?;
            let hosted = runner.hosted(key).ok_or_else(|| not_found(key))?;
            hosted.close();
            Ok(json!({}))
        })(),
        _ => Err(Error::method_not_found()),
    })
}

fn initialize() -> Value {
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {},
            "mcpCapabilities": {"http": false, "sse": false},
            "sessionCapabilities": {"resume": {}, "close": {}},
            "_meta": {EVENTS_CAPABILITY: true},
        },
        "authMethods": [{"id": RUNNER_TOKEN_METHOD, "name": "Runner token"}],
        "agentInfo": {"name": "ai6-runner", "version": env!("CARGO_PKG_VERSION")},
    })
}

fn authenticate(connection: &Connection, params: &Value) -> Result<Value, Error> {
    let supplied = params
        .get("_meta")
        .and_then(|meta| meta.get("token"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let right = params.get("methodId").and_then(Value::as_str) == Some(RUNNER_TOKEN_METHOD)
        && supplied.len() == connection.token.len()
        && bool::from(supplied.as_bytes().ct_eq(connection.token.as_bytes()));
    if right {
        connection.authenticated.store(true, Ordering::SeqCst);
        Ok(json!({}))
    } else {
        Err(Error::auth_required().data("a valid runner token is required"))
    }
}

/// Makes `binding` the connection's Session; a connection that held another Session for the
/// same key is told and closed, so the newer one wins.
fn bind(connection: &Arc<Connection>, binding: Arc<Binding>) {
    let superseded =
        lock_of(&connection.runner.inner.bindings).insert(binding.key.clone(), binding.clone());
    if let Some(old) = superseded
        && !Arc::ptr_eq(&old, &binding)
    {
        old.stop.store(true, Ordering::SeqCst);
        if let Ok(message) = UntypedMessage::new(
            EVENT_NOTIFICATION,
            json!({
                "sessionId": old.key,
                "event": {
                    "kind": super::DETACHED_EVENT,
                    "text": "another connection took this Session over",
                    "data": null, "limit": null, "usage": null, "tools": [],
                },
            }),
        ) {
            let _ = old.cx.send_notification(message);
        }
        // The notice is written before the socket closes under it.
        let stream = old.stream.try_clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            if let Ok(stream) = stream {
                let _ = stream.shutdown(Shutdown::Both);
            }
        });
    }
    *lock_of(&connection.binding) = Some(binding);
}

fn prompt(connection: &Arc<Connection>, params: &Value, responder: Responder<Value>) {
    let fail = |responder: Responder<Value>, error: Error| {
        let _ = responder.respond_with_error(error);
    };
    let key = match key_of(params) {
        Ok(key) => key,
        Err(error) => return fail(responder, error),
    };
    let Some(binding) = lock_of(&connection.binding)
        .clone()
        .filter(|binding| binding.key == key)
    else {
        return fail(responder, not_found(key));
    };
    let text = text_of(params);
    let hosted = lock_of(&binding.hosted).clone();
    match hosted {
        Some(hosted) => {
            let sent = hosted.with_harness(|harness| {
                harness
                    .send_message(AgentMessage { text })
                    .map_err(|error| crate::directorates::q::transport::describe(&error))
            });
            match sent {
                Ok(()) => lock_of(&binding.pending).push(responder),
                Err(reason) => fail(responder, Error::internal_error().data(reason)),
            }
        }
        None => {
            let Some((launch, cwd)) = lock_of(&binding.launch).take() else {
                return fail(
                    responder,
                    Error::internal_error().data("the Session is already starting"),
                );
            };
            let Some(spec) = launch.handler.clone() else {
                return fail(
                    responder,
                    Error::invalid_params().data("the launch names no handler"),
                );
            };
            let pinned = launch.terminal.clone();
            let start = launch.into_start(text, cwd);
            match connection.handle.block_on(connection.runner.launch(
                &binding.key,
                &spec,
                pinned.as_deref(),
                Launch::Start(start),
            )) {
                Ok(hosted) => {
                    lock_of(&binding.pending).push(responder);
                    *lock_of(&binding.hosted) = Some(hosted.clone());
                    feed(connection, &binding, &hosted, 0);
                }
                Err(reason) => fail(responder, Error::internal_error().data(reason)),
            }
        }
    }
}

fn handle_notification(connection: &Arc<Connection>, message: &UntypedMessage) {
    if !connection.authenticated.load(Ordering::SeqCst) {
        return;
    }
    let params = message.params();
    let Ok(key) = key_of(params) else {
        return;
    };
    let Some(hosted) = connection.runner.hosted(key) else {
        return;
    };
    match message.method() {
        "_ai6/ack" => {
            if let Some(seq) = params.get("seq").and_then(Value::as_u64)
                && let Err(error) = lock_of(&hosted.spool).acknowledge(seq)
            {
                eprintln!("ai6 runner: Session {key:?} cursor not written: {error}");
            }
        }
        "session/cancel" => {
            let binding = lock_of(&connection.binding).clone();
            std::thread::spawn(move || {
                let _ = hosted.with_harness(|harness| {
                    harness
                        .interrupt()
                        .map_err(|error| crate::directorates::q::transport::describe(&error))
                });
                if let Some(binding) = binding {
                    answer(&binding, |responder| {
                        responder.respond(json!({"stopReason": "cancelled"}))
                    });
                }
            });
        }
        _ => {}
    }
}

fn answer(binding: &Binding, reply: impl Fn(Responder<Value>) -> Result<(), Error>) {
    for responder in std::mem::take(&mut *lock_of(&binding.pending)) {
        let _ = reply(responder);
    }
}

/// Sends `binding`'s connection the Session's start marker and every spooled event after
/// `after`, then follows the spool live until the connection is superseded or closes.
fn feed(connection: &Arc<Connection>, binding: &Arc<Binding>, hosted: &Arc<Hosted>, after: u64) {
    let limits = connection.runner.inner.limits.clone();
    let (binding, hosted) = (binding.clone(), hosted.clone());
    std::thread::Builder::new()
        .name(format!("ai6-runner-feed-{}", binding.key))
        .spawn(move || {
            let send = |params: Value| {
                if let Ok(message) = UntypedMessage::new(EVENT_NOTIFICATION, params) {
                    let _ = binding.cx.send_notification(message);
                }
            };
            let provider = |hosted: &Hosted| {
                lock_of(&hosted.session_id)
                    .as_ref()
                    .map(ToString::to_string)
            };
            send(json!({
                "sessionId": binding.key,
                "providerSessionId": provider(&hosted),
                "event": null,
                "terminal": hosted.terminal,
                "handler": hosted.handler,
                "last": lock_of(&hosted.spool).last(),
            }));
            let replay_until = lock_of(&hosted.spool).last();
            let mut sent = after;
            loop {
                if binding.stop.load(Ordering::SeqCst) {
                    break;
                }
                let spooled = lock_of(&hosted.spool).after(sent, limits.replay_batch);
                if spooled.is_empty() {
                    if hosted.ended.load(Ordering::Acquire) && sent >= lock_of(&hosted.spool).last()
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(limits.feed_poll_ms));
                    continue;
                }
                for entry in spooled {
                    sent = entry.seq;
                    let last = matches!(
                        entry.event.kind.as_str(),
                        "completed" | "failed" | LIMIT_EVENT
                    );
                    let kind = entry.event.kind.clone();
                    let text = entry.event.text.clone();
                    send(json!({
                        "sessionId": binding.key,
                        "providerSessionId": provider(&hosted),
                        "seq": entry.seq,
                        "replay": entry.seq <= replay_until && after < replay_until,
                        "event": entry.event,
                    }));
                    if last && entry.seq > replay_until {
                        match kind.as_str() {
                            "completed" => answer(&binding, |responder| {
                                responder.respond(json!({"stopReason": "end_turn"}))
                            }),
                            _ => answer(&binding, |responder| {
                                responder.respond_with_error(
                                    Error::internal_error().data(text.clone().unwrap_or_default()),
                                )
                            }),
                        }
                    }
                }
            }
        })
        .ok();
}
