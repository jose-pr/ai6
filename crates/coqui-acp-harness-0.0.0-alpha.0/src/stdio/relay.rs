//! One ACP connection to an agent that speaks ACP on its own standard input and output: the host's messages go to the
//! program a line each, the program's lines come back as messages, and the provider answers for itself only what it
//! must (a message that is not a request, the working roots).
//!
//! The connection ends the way the kind says: the host's `close` cancels the turns still out, waits for their answers,
//! ends the program, takes a last look at the account's login file, lets go of the spawner and the keeper, and only then
//! calls the sink's `closed`.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::{
    Channel, ChannelSink, PathRefusal, ProcessSpawner, READ_LINK_DOWN, READ_TIMED_OUT, SpawnSpec,
    SpawnedProcess, WorkingRoots,
};
use serde_json::{Value, json};

use super::replay::{Mapping, Replays};
use crate::account::{Hold, Watcher};
use crate::provider::track;

/// How many messages wait for the sink before a sender is held back.
const QUEUE: usize = 4;
/// How many the program's answers may pile up before it is no longer read, so a sink that takes nothing cannot make the
/// provider hold the program's output without limit.
const BACKLOG: usize = 256;
/// How long the host's close waits for the turns it cancelled to answer.
const CANCEL_WAIT: Duration = Duration::from_secs(3);
/// How long the end waits for one of the connection's threads to return after the program is killed.
const THREAD_WAIT: Duration = Duration::from_secs(2);
/// What a connection is started with.
pub(crate) struct Params {
    pub spawner: Arc<Object<dyn ProcessSpawner>>,
    pub spec: SpawnSpec,
    pub sink: Object<dyn ChannelSink>,
    pub max_message_bytes: usize,
    pub max_requests: usize,
    pub roots: WorkingRoots,
    /// The account's hold, and how often its files are looked at.
    pub hold: Option<(Hold, Duration)>,
    /// The sessions imported by replay, which this connection may open.
    pub replays: Arc<Replays>,
}

#[derive(Default)]
struct State {
    /// Messages for the sink, in order.
    outbound: VecDeque<Vec<u8>>,
    /// No more messages are taken: the host closed, or the connection failed.
    ended: bool,
    /// The sink refused a message.
    dead: bool,
    /// How the connection ended, once it did.
    status: Option<(u32, String)>,
    /// The program's output is over.
    child_done: bool,
    /// Prompts the program has not answered, by request id, with their session.
    pending: BTreeMap<String, (Value, String)>,
    /// Requests the host sent whose answers the sink has not taken.
    in_flight: usize,
    reader_done: bool,
    errors_done: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// The program's input: one writer at a time.
    input: Mutex<()>,
    process: Arc<Object<dyn SpawnedProcess>>,
    roots: WorkingRoots,
    max_message_bytes: usize,
    max_requests: usize,
    replays: Arc<Replays>,
    /// The sessions of this connection opened from an import, and their ids.
    mapping: Mutex<Mapping>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn end(&self, status: u32, reason: &str) {
        let mut state = self.lock();
        state.ended = true;
        state
            .status
            .get_or_insert_with(|| (status, reason.to_string()));
        drop(state);
        self.changed.notify_all();
    }

    fn write_line(&self, value: &Value) -> Result<(), ProviderError> {
        let mut line = serde_json::to_vec(value).unwrap_or_default();
        line.push(b'\n');
        let _one = self.input.lock().unwrap_or_else(PoisonError::into_inner);
        self.process.write(&line)
    }
}

fn error_reply(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The id a reply to `object` carries: its own when it is a number or text, else null.
fn reply_id(object: &serde_json::Map<String, Value>) -> Value {
    match object.get("id") {
        Some(id @ (Value::Number(_) | Value::String(_))) => id.clone(),
        _ => Value::Null,
    }
}

/// What to do with a message from the host.
enum Verdict {
    /// Pass it to the program. `request` is whether it asks for an answer; `prompt` the turn it starts, by the request's
    /// id as text, with the id and the session.
    Forward {
        value: Value,
        request: bool,
        prompt: Option<(String, Value, String)>,
    },
    /// Answer it here.
    Reply(Value),
    /// Say nothing of it.
    Drop,
}

fn directories_of(params: &Value) -> Result<Vec<String>, &'static str> {
    let Some(object) = params.as_object() else {
        return Err("params are not an object");
    };
    let Some(cwd) = object.get("cwd").and_then(Value::as_str) else {
        return Err("cwd is not text");
    };
    let mut directories = vec![cwd.to_string()];
    match object.get("additionalDirectories") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for item in list {
                let Some(text) = item.as_str() else {
                    return Err("an additional directory is not text");
                };
                directories.push(text.to_string());
            }
        }
        Some(_) => return Err("additionalDirectories is not a list"),
    }
    Ok(directories)
}

impl Shared {
    fn inspect(&self, bytes: &[u8]) -> Verdict {
        let Ok(mut value) = serde_json::from_slice::<Value>(bytes) else {
            return Verdict::Reply(error_reply(&Value::Null, -32700, "parse error"));
        };
        let opened = if value.is_object() {
            let mut mapping = self.mapping.lock().unwrap_or_else(PoisonError::into_inner);
            mapping.outbound(&self.replays, &mut value)
        } else {
            None
        };
        let verdict = self.judge(value);
        // An open the checks refused never reaches the agent: the import is not used up by it.
        if let Some(request) = opened
            && !matches!(verdict, Verdict::Forward { .. })
        {
            let mut mapping = self.mapping.lock().unwrap_or_else(PoisonError::into_inner);
            mapping.undo(&self.replays, &request);
        }
        verdict
    }

    /// What to do with a message of the host, after an import's open was rewritten.
    fn judge(&self, value: Value) -> Verdict {
        let Some(object) = value.as_object() else {
            return Verdict::Reply(error_reply(&Value::Null, -32600, "invalid request"));
        };
        let id = reply_id(object);
        let is_request = object.contains_key("id");
        let Some(method) = object.get("method") else {
            // A response to something the program asked.
            return Verdict::Forward {
                value,
                request: false,
                prompt: None,
            };
        };
        let Some(method) = method.as_str() else {
            return Verdict::Reply(error_reply(&id, -32600, "method is not text"));
        };
        let refuse = |why: &str| {
            if is_request {
                Verdict::Reply(error_reply(&id, -32602, why))
            } else {
                Verdict::Drop
            }
        };
        let params = object.get("params").unwrap_or(&Value::Null);
        let mut prompt = None;
        match method {
            "session/new" | "session/load" | "session/resume" | "session/fork" => {
                let directories = match directories_of(params) {
                    Ok(directories) => directories,
                    Err(why) => return refuse(why),
                };
                for directory in directories {
                    match self.roots.admits(&directory) {
                        Ok(()) => {}
                        Err(PathRefusal::Outside | PathRefusal::LinkLeadsOutside) => {
                            return refuse("a directory is outside the working roots");
                        }
                        Err(_) => return refuse("a directory is not a plain absolute path"),
                    }
                }
            }
            "session/prompt" => {
                let Some(session) = params.get("sessionId").and_then(Value::as_str) else {
                    return refuse("sessionId is not text");
                };
                if !params.get("prompt").is_some_and(Value::is_array) {
                    return refuse("prompt is not a list");
                }
                if is_request {
                    prompt = Some((id.to_string(), id.clone(), session.to_string()));
                }
            }
            _ => {}
        }
        Verdict::Forward {
            value,
            request: is_request,
            prompt,
        }
    }
}

/// The host's end of a connection.
struct Link {
    shared: Arc<Shared>,
}

impl Channel for Link {
    fn send(&self, bytes: &[u8]) -> Result<(), ProviderError> {
        let shared = &self.shared;
        if bytes.len() > shared.max_message_bytes {
            const WHY: &str = "the message is larger than this provider takes";
            shared.end(Status::GUATIAO_ERR_BAD_VALUE.into(), WHY);
            return Err(ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, WHY));
        }
        let verdict = shared.inspect(bytes);
        let request = matches!(verdict, Verdict::Forward { request: true, .. });
        let mut state = shared.lock();
        loop {
            if state.ended {
                return Err(ProviderError::new(
                    Status::GUATIAO_ERR_GONE,
                    "the connection is closed",
                ));
            }
            // A sink that takes nothing holds the senders back, and so does one that has not taken as many answers as
            // the requests the program may have in hand.
            if state.outbound.len() < QUEUE && !(request && state.in_flight >= shared.max_requests)
            {
                break;
            }
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        match verdict {
            Verdict::Reply(message) => {
                state.in_flight += 1;
                state.outbound.push_back(message.to_string().into_bytes());
                drop(state);
                shared.changed.notify_all();
            }
            Verdict::Drop => {}
            Verdict::Forward {
                value,
                request,
                prompt,
            } => {
                if request {
                    state.in_flight += 1;
                }
                if let Some((key, id, session)) = prompt {
                    state.pending.insert(key, (id, session));
                }
                drop(state);
                if shared.write_line(&value).is_err() {
                    return Err(ProviderError::new(
                        Status::GUATIAO_ERR_GONE,
                        "the agent's program has ended",
                    ));
                }
            }
        }
        Ok(())
    }

    fn close(&self, status: u32, reason: &str) {
        self.shared.end(status, reason);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.shared.end(Status::GUATIAO_OK.into(), "");
    }
}

fn spawn_thread(name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), ProviderError> {
    let thread = std::thread::Builder::new()
        .name(name.into())
        .spawn(work)
        .map_err(|_| ProviderError::new(Status::GUATIAO_ERR_INTERNAL, "no thread to serve on"))?;
    track(thread);
    Ok(())
}

/// Reads the program's standard output into messages.
fn read_output(shared: &Shared) {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    'reading: loop {
        let n = match shared.process.read(&mut chunk, 200) {
            Ok(READ_TIMED_OUT | READ_LINK_DOWN) => continue,
            Ok(n) if n > 0 => usize::try_from(n).unwrap_or(0),
            Ok(_) | Err(_) => break,
        };
        buffer.extend_from_slice(&chunk[..n]);
        while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=end).collect();
            let line = line.trim_ascii();
            if line.is_empty() {
                continue;
            }
            // Whatever the program prints that is not one JSON value is not a message.
            let Ok(mut value) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let changed = {
                let mut mapping = shared
                    .mapping
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                mapping.inbound(&shared.replays, &mut value)
            };
            let line = if changed {
                value.to_string().into_bytes()
            } else {
                line.to_vec()
            };
            let mut state = shared.lock();
            if let Some(object) = value.as_object()
                && !object.contains_key("method")
                && let Some(id) = object.get("id")
            {
                state.pending.remove(&id.to_string());
            }
            while state.outbound.len() >= BACKLOG && !state.dead && !state.ended {
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if state.dead {
                break 'reading;
            }
            state.outbound.push_back(line);
            drop(state);
            shared.changed.notify_all();
        }
        if buffer.len() > shared.max_message_bytes.saturating_add(1) {
            shared.end(
                Status::GUATIAO_ERR_INTERNAL.into(),
                "the agent sent a message over the limit",
            );
            break;
        }
    }
    let mut state = shared.lock();
    state.child_done = true;
    state.reader_done = true;
    drop(state);
    shared.changed.notify_all();
}

/// Reads the program's standard error away, so that it never fills its pipe.
fn drain_errors(shared: &Shared) {
    let mut chunk = [0u8; 4096];
    loop {
        match shared.process.read_error(&mut chunk, 200) {
            Ok(READ_TIMED_OUT | READ_LINK_DOWN) => {}
            Ok(n) if n > 0 => {}
            Ok(_) | Err(_) => break,
        }
    }
    shared.lock().errors_done = true;
    shared.changed.notify_all();
}

/// What ending the connection takes, which the delivering thread owns.
struct Ending {
    spawner: Arc<Object<dyn ProcessSpawner>>,
    hold: Option<(Hold, Option<Watcher>)>,
}

enum Step {
    Send(Vec<u8>),
    Cancel(Vec<String>),
    Synthesize(Vec<Value>),
    Done,
}

/// The only caller of the sink: hands over every message, cancels the turns still out when the host closed, ends the
/// program, takes the last look at the login and calls `closed`.
fn deliver(mut sink: Object<dyn ChannelSink>, shared: &Shared, ending: Ending) {
    let mut cancel_by: Option<Instant> = None;
    loop {
        let step = {
            let mut state = shared.lock();
            loop {
                if let Some(message) = state.outbound.pop_front() {
                    break Step::Send(message);
                }
                if state.dead || state.child_done {
                    break Step::Done;
                }
                if state.ended {
                    if state.pending.is_empty() {
                        break Step::Done;
                    }
                    match cancel_by {
                        None => {
                            break Step::Cancel(
                                state.pending.values().map(|(_, s)| s.clone()).collect(),
                            );
                        }
                        Some(by) => {
                            let now = Instant::now();
                            if now >= by {
                                let ids = state.pending.values().map(|(id, _)| id.clone());
                                let synthesized = ids
                                    .map(|id| json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "cancelled"}}))
                                    .collect();
                                state.pending.clear();
                                break Step::Synthesize(synthesized);
                            }
                            state = shared
                                .changed
                                .wait_timeout(state, by - now)
                                .unwrap_or_else(PoisonError::into_inner)
                                .0;
                            continue;
                        }
                    }
                }
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        shared.changed.notify_all();
        match step {
            Step::Send(message) => {
                let answer = serde_json::from_slice::<Value>(&message)
                    .is_ok_and(|v| v.get("id").is_some() && v.get("method").is_none());
                let received = sink.receive(&message);
                if answer {
                    let mut state = shared.lock();
                    state.in_flight = state.in_flight.saturating_sub(1);
                }
                if let Err(refused) = received {
                    let mut state = shared.lock();
                    state.dead = true;
                    state.ended = true;
                    state.outbound.clear();
                    state.status = Some((u32::from(refused.status), refused.message().to_owned()));
                    drop(state);
                    shared.changed.notify_all();
                }
            }
            Step::Cancel(sessions) => {
                cancel_by = Some(Instant::now() + CANCEL_WAIT);
                for session in sessions {
                    let _ = shared.write_line(&json!({
                        "jsonrpc": "2.0",
                        "method": "session/cancel",
                        "params": {"sessionId": session},
                    }));
                }
            }
            Step::Synthesize(messages) => {
                let mut state = shared.lock();
                for message in messages {
                    state.outbound.push_back(message.to_string().into_bytes());
                }
            }
            Step::Done => break,
        }
    }
    let exit = {
        let _ = shared.process.kill();
        shared.process.wait(1000).ok()
    };
    wait_for(shared, |state| state.reader_done && state.errors_done);
    // Imports this connection took and did not use wait again.
    shared
        .mapping
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .restore_all(&shared.replays);
    let Ending { spawner, hold } = ending;
    if let Some((hold, watcher)) = hold {
        if let Some(watcher) = watcher {
            watcher.stop();
        }
        hold.finish();
    }
    let (status, reason) = {
        let mut state = shared.lock();
        let known = state.status.take();
        known.unwrap_or_else(|| match exit {
            Some(exit) if !exit.running => (
                Status::GUATIAO_ERR_INTERNAL.into(),
                exit.code.map_or_else(
                    || "the agent's program was ended".to_string(),
                    |code| alloc::format!("the agent's program ended with code {code}"),
                ),
            ),
            _ => (Status::GUATIAO_OK.into(), String::new()),
        })
    };
    drop(spawner);
    sink.closed(status, &reason);
}

/// Waits up to [`THREAD_WAIT`] for `done` to hold of the state.
fn wait_for(shared: &Shared, done: impl Fn(&State) -> bool) {
    let by = Instant::now() + THREAD_WAIT;
    let mut state = shared.lock();
    while !done(&state) {
        let now = Instant::now();
        if now >= by {
            return;
        }
        state = shared
            .changed
            .wait_timeout(state, by - now)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/// Starts the program and the threads that serve it, and answers the channel the host talks over.
///
/// # Errors
///
/// The spawner refused the program, or no thread could be started. The sink is destroyed, and the hold let go.
pub(crate) fn open(params: Params) -> Result<Object<dyn Channel>, ProviderError> {
    let Params {
        spawner,
        spec,
        sink,
        max_message_bytes,
        max_requests,
        roots,
        hold,
        replays,
    } = params;
    let process = match spawner.spawn(spec) {
        Ok(process) => Arc::new(process),
        Err(error) => {
            if let Some((hold, _)) = hold {
                hold.finish();
            }
            return Err(error);
        }
    };
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        changed: Condvar::new(),
        input: Mutex::new(()),
        process,
        roots,
        max_message_bytes,
        max_requests,
        replays,
        mapping: Mutex::new(Mapping::default()),
    });
    let ending = Ending {
        spawner,
        hold: hold.map(|(hold, poll)| {
            let watcher = (!poll.is_zero()).then(|| hold.watch(poll));
            (hold, watcher)
        }),
    };
    let started = (|| {
        let reading = Arc::clone(&shared);
        spawn_thread("coqui-acp-stdio-read", move || read_output(&reading))?;
        let errors = Arc::clone(&shared);
        spawn_thread("coqui-acp-stdio-errors", move || drain_errors(&errors))?;
        let delivering = Arc::clone(&shared);
        spawn_thread("coqui-acp-stdio-deliver", move || {
            deliver(sink, &delivering, ending);
        })
    })();
    if let Err(error) = started {
        let _ = shared.process.kill();
        shared.end(Status::GUATIAO_ERR_INTERNAL.into(), "no thread to serve on");
        return Err(error);
    }
    Ok(Link { shared }.into_object())
}
