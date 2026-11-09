//! One ACP session and the provider session behind it.
//!
//! A provider may block, fail or panic in any call, and nothing here can stop a thread stuck in one. So every
//! call runs on a thread that holds only this session up, is timed by the connection's supervisor, and an
//! overdue one ends the session while the thread stays until the provider lets go of it.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::panic::AssertUnwindSafe;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use core::time::Duration;
use std::panic::catch_unwind;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Instant;

use agent_client_protocol::{Client, ConnectionTo, Error as RpcError, Responder, UntypedMessage};
use guatiao::library::{Object, ProviderError};
use serde_json::{Map, Value, json};

use super::EVENT_NOTIFICATION;
use super::connection::Connection;
use super::options::{self, Choices, OptionTarget};
use super::updates::{self, Ending};
use crate::kind::{
    AgentEvent, AgentMessage, AgentSession, FAILED_EVENT, ResumeRequest, StartRequest,
};

/// How often a closing session looks again for a `start` still in flight.
const START_POLL: Duration = Duration::from_millis(10);

/// What a session starts from, kept for the first prompt.
pub(super) struct Launch {
    pub(super) working_directory: String,
    pub(super) context: Vec<String>,
    pub(super) environment: BTreeMap<String, String>,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) meta: Option<guatiao::Value>,
    /// The declared options at their values when the session opened.
    pub(super) choices: Choices,
}

/// What the provider is given: the launch, with each declared option's value where its host said.
struct Given {
    environment: BTreeMap<String, String>,
    model: Option<String>,
    effort: Option<String>,
}

impl Launch {
    fn given(&self, declared: &[options::StartOption], choices: &Choices) -> Given {
        let mut given = Given {
            environment: self.environment.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
        };
        for option in declared {
            let value = choices.get(&option.id).unwrap_or(&option.default).clone();
            match &option.target {
                OptionTarget::Model => given.model = Some(value),
                OptionTarget::Effort => given.effort = Some(value),
                OptionTarget::Environment(name) => {
                    given.environment.insert(name.clone(), value);
                }
            }
        }
        given
    }

    fn start(&self, prompt: String, given: Given) -> StartRequest {
        StartRequest {
            working_directory: self.working_directory.clone(),
            prompt,
            context: self.context.clone(),
            environment: given.environment,
            model: given.model,
            effort: given.effort,
            meta: self.meta.clone(),
        }
    }

    fn resume(&self, session_id: &str, given: Given) -> ResumeRequest {
        ResumeRequest {
            session_id: session_id.to_string(),
            working_directory: self.working_directory.clone(),
            prompt: None,
            context: self.context.clone(),
            environment: given.environment,
            model: given.model,
            effort: given.effort,
            meta: self.meta.clone(),
        }
    }
}

enum Turn {
    Idle,
    /// A prompt waits for its answer. `cancelled` is when `session/cancel` came; `number` counts the
    /// session's turns.
    Active {
        responder: Box<Responder<Value>>,
        cancelled: Option<Instant>,
        number: u64,
    },
    /// The prompt was answered `cancelled` and the provider has not ended the turn. What it sends is the
    /// old turn's, so a new prompt has to wait, or the old ending would answer it.
    Draining(u64),
}

impl Turn {
    /// The turn the provider is working on, if any.
    const fn number(&self) -> Option<u64> {
        match self {
            Self::Idle => None,
            Self::Active { number, .. } | Self::Draining(number) => Some(*number),
        }
    }
}

struct State {
    turn: Turn,
    /// What the provider sent since the turn began, against [`Settings::max_turn_bytes`].
    ///
    /// [`Settings::max_turn_bytes`]: super::Settings::max_turn_bytes
    bytes: usize,
    /// Why the session cannot go on.
    dead: Option<String>,
    open_tools: Vec<String>,
    /// The provider's identity once known, whether or not it could be shown to the client.
    provider_id: Option<String>,
    turns: u64,
    /// The `session/resume` waiting for the provider's.
    resuming: Option<Responder<Value>>,
    /// `session/close` calls waiting for the provider's.
    closing: Vec<Responder<Value>>,
    closed: bool,
    /// The declared options' values, which the provider takes when it starts.
    choices: Choices,
    /// The provider has been given the options, by a first prompt or a resume, so they cannot change.
    fixed: bool,
}

#[derive(Clone, Copy)]
enum What {
    Start,
    Resume,
    SendMessage,
    NextEvent,
    SessionId,
    Interrupt,
    Close,
}

impl What {
    const fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Resume => "resume",
            Self::SendMessage => "send_message",
            Self::NextEvent => "next_event",
            Self::SessionId => "session_id",
            Self::Interrupt => "interrupt",
            Self::Close => "close",
        }
    }
}

struct Call {
    id: u64,
    what: What,
    started: Instant,
    limit: Duration,
}

pub(super) struct Operation {
    pub(super) id: String,
    launch: Launch,
    connection: Arc<Connection>,
    cx: ConnectionTo<Client>,
    session: Mutex<Option<Arc<Object<dyn AgentSession>>>>,
    /// The provider identity this session holds across the facade, freed with it.
    pub(super) held: Mutex<Option<String>>,
    state: Mutex<State>,
    calls: Mutex<Vec<Call>>,
    next_call: AtomicU64,
    threads: AtomicUsize,
    closing: AtomicBool,
    close_called: AtomicBool,
    /// The turn an `interrupt` is in flight for.
    interrupting: Mutex<Option<u64>>,
    starting: AtomicBool,
}

/// Counts a thread of the session, which leaves the connection's books with its last one.
struct Running(Arc<Operation>);

impl Drop for Running {
    fn drop(&mut self) {
        if self.0.threads.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.finish_if_done();
        }
    }
}

fn failure(text: String) -> RpcError {
    RpcError::internal_error().data(text)
}

impl Operation {
    pub(super) fn new(
        id: String,
        launch: Launch,
        connection: Arc<Connection>,
        cx: ConnectionTo<Client>,
    ) -> Arc<Self> {
        let choices = launch.choices.clone();
        Arc::new(Self {
            id,
            launch,
            connection,
            cx,
            session: Mutex::new(None),
            held: Mutex::new(None),
            state: Mutex::new(State {
                turn: Turn::Idle,
                bytes: 0,
                dead: None,
                open_tools: Vec::new(),
                provider_id: None,
                turns: 0,
                resuming: None,
                closing: Vec::new(),
                closed: false,
                choices,
                fixed: false,
            }),
            calls: Mutex::new(Vec::new()),
            next_call: AtomicU64::new(0),
            threads: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            close_called: AtomicBool::new(false),
            interrupting: Mutex::new(None),
            starting: AtomicBool::new(false),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn calls(&self) -> MutexGuard<'_, Vec<Call>> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn provider_session(&self) -> Option<Arc<Object<dyn AgentSession>>> {
        self.session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Runs `work` on a thread of this session. A session that cannot start one answers `false`.
    fn spawn(self: &Arc<Self>, name: &str, work: impl FnOnce(&Arc<Self>) + Send + 'static) -> bool {
        self.threads.fetch_add(1, Ordering::SeqCst);
        let running = Running(self.clone());
        thread::Builder::new()
            .name(format!("coqui-acp-{name}"))
            .spawn(move || {
                work(&running.0);
                drop(running);
            })
            .is_ok()
    }

    /// One provider call: timed by the supervisor, and a panic stops here. The panic's words stay out of
    /// what the client is told, since they can hold anything the provider held.
    fn call<T>(
        &self,
        what: What,
        limit: Duration,
        work: impl FnOnce() -> Result<T, ProviderError>,
    ) -> Result<T, String> {
        let id = self.next_call.fetch_add(1, Ordering::SeqCst);
        self.calls().push(Call {
            id,
            what,
            started: Instant::now(),
            limit,
        });
        let outcome = catch_unwind(AssertUnwindSafe(work));
        self.calls().retain(|call| call.id != id);
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err(format!("the provider panicked in {}", what.name())),
        }
    }

    /// Sends what the facade itself says. It counts toward the connection's total, but a few short lines as a
    /// session ends are not refused for it.
    fn notify(&self, method: &str, params: Value) {
        self.connection.charge(updates::cost(method, &params));
        self.send(method, params);
    }

    fn send(&self, method: &str, params: Value) {
        if let Ok(message) = UntypedMessage::new(method, params) {
            let _ = self.cx.send_notification(message);
        }
    }

    /// Answers the prompt, ending the turn's tool calls first so none is left running.
    fn answer(&self, responder: Responder<Value>, ending: Ending, open_tools: Vec<String>) {
        let status = if ending == Ending::Stop("end_turn") {
            "completed"
        } else {
            "failed"
        };
        for tool in open_tools {
            self.notify(
                "session/update",
                updates::tool_finished(&self.id, &tool, status),
            );
        }
        let _ = match ending {
            Ending::Stop(stop) => responder.respond(json!({"stopReason": stop})),
            Ending::Failure(text) => responder.respond_with_error(failure(text)),
        };
    }

    /// Ends the current turn, if one is waiting: `next` is what the session is in afterwards. ACP answers a
    /// cancelled turn `cancelled`, however the provider came to end it.
    fn end_turn(&self, ending: Ending, next: Turn) {
        let (responder, cancelled, open_tools) = {
            let mut state = self.state();
            match core::mem::replace(&mut state.turn, next) {
                Turn::Active {
                    responder,
                    cancelled,
                    ..
                } => (responder, cancelled, core::mem::take(&mut state.open_tools)),
                other => {
                    state.turn = other;
                    return;
                }
            }
        };
        let ending = if cancelled.is_some() {
            Ending::Stop("cancelled")
        } else {
            ending
        };
        self.answer(*responder, ending, open_tools);
    }

    /// Ends the session for a reason the client should hear.
    fn abandon(self: &Arc<Self>, reason: String) {
        {
            let mut state = self.state();
            if state.dead.is_some() {
                return;
            }
            state.dead = Some(reason.clone());
        }
        if let Some(responder) = self.state().resuming.take() {
            let _ = responder.respond_with_error(failure(reason.clone()));
        }
        // With no prompt waiting there is no answer to carry it, and the client should still hear.
        if !matches!(self.state().turn, Turn::Active { .. }) {
            let event = AgentEvent::new(FAILED_EVENT, Some(reason.clone()));
            self.notify(
                EVENT_NOTIFICATION,
                json!({"sessionId": self.id, "event": event}),
            );
        }
        self.end_turn(Ending::Failure(reason), Turn::Idle);
        self.begin_close();
    }

    /// Takes a prompt. Everything the provider is asked happens on a thread of its own.
    pub(super) fn prompt(self: &Arc<Self>, text: String, responder: Responder<Value>) {
        let refused = {
            let mut state = self.state();
            let why = if let Some(reason) = &state.dead {
                Some(format!("this session has ended: {reason}"))
            } else if state.resuming.is_some() {
                Some("this session is still being resumed".to_string())
            } else if self.closing.load(Ordering::SeqCst) {
                Some("this session is closing".to_string())
            } else {
                match state.turn {
                    Turn::Idle => {
                        state.fixed = true;
                        None
                    }
                    Turn::Active { .. } => Some("a turn is in progress".to_string()),
                    Turn::Draining(_) => {
                        Some("the provider has not ended the cancelled turn".to_string())
                    }
                }
            };
            match why {
                Some(why) => Some((why, responder)),
                None => {
                    state.bytes = 0;
                    state.turns += 1;
                    state.turn = Turn::Active {
                        responder: Box::new(responder),
                        cancelled: None,
                        number: state.turns,
                    };
                    None
                }
            }
        };
        if let Some((why, responder)) = refused {
            let _ = responder.respond_with_error(failure(why));
            return;
        }
        if !self.spawn("prompt", move |this| this.deliver_prompt(text)) {
            self.end_turn(
                Ending::Failure("no thread could be started for the prompt".to_string()),
                Turn::Idle,
            );
        }
    }

    fn deliver_prompt(self: &Arc<Self>, text: String) {
        let outcome = match self.provider_session() {
            Some(session) => self.call(
                What::SendMessage,
                self.connection.settings.call_timeout,
                || session.send_message(AgentMessage { text }),
            ),
            None => self.start(text),
        };
        if let Err(why) = outcome {
            let label = updates::label(&why, self.connection.settings.max_label_bytes);
            self.end_turn(Ending::Failure(label), Turn::Idle);
            // The provider never took it, so no ending will come for a turn already answered.
            let mut state = self.state();
            if matches!(state.turn, Turn::Draining(_)) {
                state.turn = Turn::Idle;
            }
        }
    }

    /// Its identity and working directory, while it is open.
    pub(super) fn listed(&self) -> Option<(String, String)> {
        if self.closing.load(Ordering::SeqCst) || self.state().dead.is_some() {
            return None;
        }
        Some((self.id.clone(), self.launch.working_directory.clone()))
    }

    /// What the provider is given now: the options as they stand.
    fn given(&self) -> Given {
        let choices = self.state().choices.clone();
        self.launch
            .given(&self.connection.settings.options, &choices)
    }

    /// Sets a declared option, until the provider has been given them. Every option at its value.
    pub(super) fn choose(&self, option: &str, value: &str) -> Result<Choices, RpcError> {
        let mut state = self.state();
        if state.dead.is_some() || self.closing.load(Ordering::SeqCst) {
            return Err(RpcError::invalid_request().data("this session has ended".to_string()));
        }
        if state.fixed {
            return Err(RpcError::invalid_request().data(
                "the provider has started this session, and its options cannot change now"
                    .to_string(),
            ));
        }
        let mut choices = state.choices.clone();
        options::choose(
            &self.connection.settings.options,
            &mut choices,
            option,
            value,
        )
        .map_err(|why| RpcError::invalid_params().data(why))?;
        state.choices.clone_from(&choices);
        Ok(choices)
    }

    fn start(self: &Arc<Self>, prompt: String) -> Result<(), String> {
        let request = self.launch.start(prompt, self.given());
        let provider = self.connection.provider.clone();
        let limit = self.connection.settings.start_timeout;
        self.starting.store(true, Ordering::SeqCst);
        let started = self.call(What::Start, limit, || provider.start(request));
        let adopted = started.map(|session| self.adopt(session));
        self.starting.store(false, Ordering::SeqCst);
        adopted
    }

    /// Resumes on a thread of its own, and answers `session/resume` with how it went.
    pub(super) fn resume(self: &Arc<Self>, responder: Responder<Value>) {
        {
            let mut state = self.state();
            state.resuming = Some(responder);
            state.fixed = true;
        }
        let spawned = self.spawn("resume", |this| {
            let request = this.launch.resume(&this.id, this.given());
            let provider = this.connection.provider.clone();
            let limit = this.connection.settings.start_timeout;
            this.starting.store(true, Ordering::SeqCst);
            let resumed = this.call(What::Resume, limit, || provider.resume(request));
            let adopted = resumed.map(|session| this.adopt(session));
            this.starting.store(false, Ordering::SeqCst);
            let responder = this.state().resuming.take();
            match adopted {
                Ok(()) => {
                    if let Some(responder) = responder {
                        let mut answer = Map::new();
                        let choices = this.state().choices.clone();
                        options::session_fields(
                            &this.connection.settings.options,
                            &choices,
                            &mut answer,
                        );
                        let _ = responder.respond(Value::Object(answer));
                    }
                }
                // The session ends before it leaves the books, so nothing that came meanwhile can start
                // one, and the identity is free again before the client hears.
                Err(why) => {
                    this.closing.store(true, Ordering::SeqCst);
                    {
                        let mut state = this.state();
                        state.dead = Some(why.clone());
                        state.closed = true;
                    }
                    this.connection.remove(this);
                    if let Some(responder) = responder {
                        let label = updates::label(&why, this.connection.settings.max_label_bytes);
                        let _ = responder.respond_with_error(failure(label));
                    }
                }
            }
        });
        if !spawned {
            self.closing.store(true, Ordering::SeqCst);
            self.state().closed = true;
            self.connection.remove(self);
            if let Some(responder) = self.state().resuming.take() {
                let _ =
                    responder.respond_with_error(failure("no thread could be started".to_string()));
            }
        }
    }

    /// Keeps a session the provider handed over and starts reading its events. One that arrives after the
    /// session ended is closed here, so no harness outlives what asked for it.
    fn adopt(self: &Arc<Self>, session: Object<dyn AgentSession>) {
        let session = Arc::new(session);
        let replaced = self
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(session.clone());
        // Two harnesses for one session would leave one running with nothing to close it.
        if let Some(replaced) = replaced {
            let limit = self.connection.settings.close_timeout;
            let _ = self.call(What::Close, limit, || replaced.close());
        }
        if self.closing.load(Ordering::SeqCst) {
            let _ = self.close_provider(&session);
            return;
        }
        if !self.spawn("pump", {
            let session = session.clone();
            move |this| this.pump(&session)
        }) {
            self.abandon("no thread could be started to read the provider".to_string());
            let _ = self.close_provider(&session);
            self.finish_if_done();
            return;
        }
        // A cancel that came while the provider was starting had nothing to reach.
        let cancelled = match self.state().turn {
            Turn::Active {
                cancelled: Some(_),
                number,
                ..
            }
            | Turn::Draining(number) => Some(number),
            _ => None,
        };
        if let Some(number) = cancelled {
            self.interrupt(number);
        }
    }

    /// Reads the provider's events until the session closes or the provider fails.
    fn pump(self: &Arc<Self>, session: &Object<dyn AgentSession>) {
        let settings = &self.connection.settings;
        let poll = settings.poll.max(Duration::from_millis(1));
        let poll_ms = i64::try_from(poll.as_millis()).unwrap_or(i64::MAX);
        let limit = poll.saturating_add(settings.call_timeout);
        self.announce_identity(session);
        while !self.closing.load(Ordering::SeqCst) {
            let began = Instant::now();
            match self.call(What::NextEvent, limit, || session.next_event(poll_ms)) {
                // The client has been told this session is over, so nothing more of it goes out.
                Ok(Some(_)) if self.closing.load(Ordering::SeqCst) => return,
                Ok(Some(event)) => {
                    self.announce_identity(session);
                    self.deliver(&event);
                }
                // A provider that returns at once must not make this loop spin.
                Ok(None) => thread::sleep(poll.saturating_sub(began.elapsed())),
                Err(why) => {
                    self.abandon(updates::label(&why, settings.max_label_bytes));
                    return;
                }
            }
        }
    }

    /// Tells the client the provider's own identity for this session, once it is known: what a later
    /// `session/resume` names. A harness's first event may be what reports it.
    fn announce_identity(self: &Arc<Self>, session: &Object<dyn AgentSession>) {
        if self.state().provider_id.is_some() {
            return;
        }
        let limit = self.connection.settings.call_timeout;
        let Ok(Some(id)) = self.call(What::SessionId, limit, || session.session_id()) else {
            return;
        };
        self.state().provider_id = Some(id.clone());
        {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            if held.is_none() && self.connection.hold(&id) {
                *held = Some(id.clone());
            }
        }
        // An identity that would not show as it is cannot be named back, so it is not told.
        if updates::title(&id, self.connection.settings.max_label_bytes) == id {
            self.notify(
                EVENT_NOTIFICATION,
                json!({"sessionId": self.id, "providerSessionId": id}),
            );
        }
    }

    /// Passes one event on, and ends the turn if it does.
    /// Every notification is costed as the line it becomes before any of it is sent.
    fn deliver(self: &Arc<Self>, event: &AgentEvent) {
        let settings = &self.connection.settings;
        let ending = updates::ending(event, settings.max_label_bytes);
        let (overflowed, outgoing) = {
            let mut state = self.state();
            if matches!(state.turn, Turn::Draining(_)) {
                if ending.is_some() {
                    state.turn = Turn::Idle;
                }
                return;
            }
            let limits = updates::Limits {
                update: settings.max_update_bytes,
                label: settings.max_label_bytes,
            };
            let shown = state
                .provider_id
                .as_deref()
                .filter(|id| updates::title(id, settings.max_label_bytes) == *id);
            let outgoing = updates::outgoing(&self.id, shown, event, limits);
            let cost: usize = outgoing
                .messages
                .iter()
                .map(|(method, params)| updates::cost(method, params))
                .sum();
            state.bytes = state.bytes.saturating_add(cost);
            let overflowed = state.bytes > settings.max_turn_bytes;
            (overflowed || !self.connection.charge(cost), outgoing)
        };
        if overflowed {
            self.abandon("the provider sent more than this agent passes on".to_string());
            return;
        }
        for (method, params) in outgoing.messages {
            self.send(method, params);
        }
        self.state().open_tools.extend(outgoing.opened);
        if let Some(ending) = ending {
            self.end_turn(ending, Turn::Idle);
        }
    }

    /// `session/cancel`: remembers it, whichever call it arrives during, and asks the provider.
    pub(super) fn cancel(self: &Arc<Self>) {
        let number = {
            let mut state = self.state();
            let Turn::Active {
                cancelled, number, ..
            } = &mut state.turn
            else {
                return;
            };
            if cancelled.is_some() {
                return;
            }
            *cancelled = Some(Instant::now());
            *number
        };
        self.interrupt(number);
    }

    /// One `interrupt` at a time for turn `number`, however many `session/cancel` a client sends, and none
    /// once that turn is over: it would stop the next one.
    fn interrupt(self: &Arc<Self>, number: u64) {
        let Some(session) = self.provider_session() else {
            return;
        };
        {
            let mut interrupting = self
                .interrupting
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if *interrupting == Some(number) {
                return;
            }
            *interrupting = Some(number);
        }
        let spawned = self.spawn("interrupt", move |this| {
            if this.state().turn.number() == Some(number) {
                let limit = this.connection.settings.call_timeout;
                let _ = this.call(What::Interrupt, limit, || session.interrupt());
            }
            this.interrupted(number);
        });
        if !spawned {
            self.interrupted(number);
        }
    }

    fn interrupted(&self, number: u64) {
        let mut interrupting = self
            .interrupting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *interrupting == Some(number) {
            *interrupting = None;
        }
    }

    /// `session/close`: answered once the provider has closed, or when it runs out of time.
    pub(super) fn close(self: &Arc<Self>, responder: Responder<Value>) {
        let done = {
            let mut state = self.state();
            if state.closed {
                Some(responder)
            } else {
                state.closing.push(responder);
                None
            }
        };
        if let Some(responder) = done {
            let _ = responder.respond(json!({}));
            return;
        }
        self.begin_close();
    }

    /// Ends the session: a waiting prompt is answered `cancelled`, and the provider's `close` runs on a
    /// thread of its own. A `start` in flight closes what it returns, if it returns.
    pub(super) fn begin_close(self: &Arc<Self>) {
        if self.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(responder) = self.state().resuming.take() {
            let _ = responder.respond_with_error(failure("this session is closing".to_string()));
        }
        self.end_turn(Ending::Stop("cancelled"), Turn::Idle);
        let spawned = self.spawn("close", |this| {
            let limit = this.connection.settings.close_timeout;
            let began = Instant::now();
            while this.starting.load(Ordering::SeqCst) && began.elapsed() < limit {
                thread::sleep(START_POLL);
            }
            let outcome = if this.starting.load(Ordering::SeqCst) {
                Err("the provider is still starting this session".to_string())
            } else {
                this.provider_session()
                    .map_or(Ok(()), |session| this.close_provider(&session))
            };
            this.closed(outcome);
        });
        // With no thread to spare the provider is asked here: nothing it holds may outlive the session.
        if !spawned {
            let outcome = self
                .provider_session()
                .map_or(Ok(()), |session| self.close_provider(&session));
            self.closed(outcome);
            self.finish_if_done();
        }
    }

    /// The provider's `close`, once.
    fn close_provider(&self, session: &Object<dyn AgentSession>) -> Result<(), String> {
        if self.close_called.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let limit = self.connection.settings.close_timeout;
        self.call(What::Close, limit, || session.close())
    }

    fn closed(self: &Arc<Self>, outcome: Result<(), String>) {
        let waiting = {
            let mut state = self.state();
            state.closed = true;
            core::mem::take(&mut state.closing)
        };
        for responder in waiting {
            let _ = match &outcome {
                Ok(()) => responder.respond(json!({})),
                Err(why) => responder.respond_with_error(failure(updates::label(
                    why,
                    self.connection.settings.max_label_bytes,
                ))),
            };
        }
    }

    /// Frees the session's place once nothing of it is left running.
    fn finish_if_done(&self) {
        if self.state().closed && self.threads.load(Ordering::SeqCst) == 0 {
            self.connection.remove(self);
        }
    }

    /// What only time can end: a provider call past its limit, and a cancelled turn past its grace.
    pub(super) fn check(self: &Arc<Self>, now: Instant) {
        let overdue = self
            .calls()
            .iter()
            .find(|call| now.duration_since(call.started) > call.limit)
            .map(|call| (call.what, call.limit));
        match overdue {
            Some((What::Close, limit)) => {
                let why = format!("the provider did not close within {} ms", limit.as_millis());
                self.closed(Err(why));
            }
            Some((what, limit)) => {
                self.abandon(format!(
                    "the provider did not answer {} within {} ms",
                    what.name(),
                    limit.as_millis()
                ));
            }
            None => {}
        }
        let grace = self.connection.settings.cancel_grace;
        let late = match self.state().turn {
            Turn::Active {
                cancelled: Some(at),
                number,
                ..
            } if now.duration_since(at) > grace => Some(number),
            _ => None,
        };
        if let Some(number) = late {
            self.end_turn(Ending::Stop("cancelled"), Turn::Draining(number));
        }
    }
}
