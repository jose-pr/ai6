//! One open session: its turn, its permission requests, and the pump that turns its events into updates.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;

use serde_json::{Map, Value, json};

use super::connection::{INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, Shared};
use super::shape::{self, Shapes};
use coqui_acp_api::NoticeKind;

use super::{Conversation, HarnessError, Settings};
use crate::event::{
    Content, Event, Outcome, PermissionRequest, Severity, Stop, ToolCall, TurnEnd, Usage,
};

/// What goes through a session's channel: the harness's events, an answer that has to follow them, and the word that
/// the session's own answer has gone, before which nothing of it is told.
#[derive(Debug)]
pub(crate) enum Item {
    Event(Box<Event>),
    Reply { id: Value, result: Value },
    Release,
}

#[derive(Debug)]
struct Turn {
    number: u64,
    /// The `session/prompt` that version 1 answers at the turn's end.
    request: Option<Value>,
    cancelling: bool,
    /// Whether the prompt has reached the harness: an interrupt before that would reach it first.
    sent: bool,
}

#[derive(Debug)]
struct State {
    shapes: Shapes,
    turn: Option<Turn>,
    turns: u64,
    /// Endings still to come of turns already answered as cancelled: each is dropped when it comes.
    stale_ends: usize,
    /// A turn answered as cancelled before its prompt reached the harness.
    answered_unsent: Option<u64>,
    /// Why the harness is gone, once it is.
    gone: Option<String>,
    closed: bool,
    /// Permission requests put to the client and not yet answered, by the id of our request.
    asking: Vec<u64>,
    /// Questions with a thread of their own.
    questions: usize,
}

pub(crate) struct Session {
    id: OnceLock<String>,
    conversation: Mutex<Option<Arc<dyn Conversation>>>,
    state: Mutex<State>,
}

impl Session {
    pub(crate) fn new(id: Option<String>, version: u8, settings: &Settings) -> Self {
        let known = OnceLock::new();
        if let Some(id) = id {
            let _ = known.set(id);
        }
        Self {
            id: known,
            conversation: Mutex::new(None),
            state: Mutex::new(State {
                shapes: Shapes::bounded(
                    version,
                    settings.max_tools,
                    settings.max_tool_content_bytes,
                    settings.max_tool_content_items,
                ),
                turn: None,
                turns: 0,
                stale_ends: 0,
                answered_unsent: None,
                gone: None,
                closed: false,
                asking: Vec::new(),
                questions: 0,
            }),
        }
    }

    /// Takes the harness's session, once it is open.
    pub(crate) fn set(&self, id: String, conversation: Box<dyn Conversation>) {
        let _ = self.id.set(id);
        *self
            .conversation
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::from(conversation));
    }

    pub(crate) fn id(&self) -> &str {
        self.id.get().map_or("", String::as_str)
    }

    pub(crate) fn conversation(&self) -> Option<Arc<dyn Conversation>> {
        self.conversation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn version(&self) -> u8 {
        self.state().shapes.version
    }

    /// Sends this session's update.
    fn update(&self, shared: &Shared, update: Value) {
        shared.notify(
            "session/update",
            json!({"sessionId": self.id(), "update": update}),
        );
    }

    /// Takes the events of `items` until every sender is gone: the harness's, once its session is closed. Until
    /// [`Item::Release`] comes, unless `released`, they are held, so a client hears nothing of a session before its
    /// answer.
    pub(crate) fn pump(
        self: &Arc<Self>,
        shared: &Arc<Shared>,
        items: Receiver<Item>,
        released: bool,
    ) {
        let mut held: Option<Vec<Event>> = if released { None } else { Some(Vec::new()) };
        let mut dropped = 0_usize;
        for item in items {
            match item {
                Item::Reply { id, result } => shared.respond(&id, result),
                Item::Release => {
                    for event in held.take().unwrap_or_default() {
                        self.event(shared, event);
                    }
                    if dropped > 0 {
                        self.event(
                            shared,
                            Event::Notice {
                                severity: Severity::Warning,
                                title: format!(
                                    "{dropped} updates from before this session opened were dropped"
                                ),
                            },
                        );
                    }
                }
                Item::Event(event) => match held.as_mut() {
                    // A question among them is answered cancelled when it is dropped; how the harness ended is kept.
                    Some(held)
                        if held.len() >= shared.settings.max_queued_events
                            && !matches!(*event, Event::Exited(_)) =>
                    {
                        dropped += 1;
                    }
                    Some(held) => held.push(*event),
                    None => self.event(shared, *event),
                },
            }
        }
    }

    fn event(self: &Arc<Self>, shared: &Arc<Shared>, event: Event) {
        if self.state().closed {
            // What a closed session's harness says last is nobody's; a question is answered cancelled on drop.
            return;
        }
        match event {
            Event::Permission(request) => {
                let turn = {
                    let mut state = self.state();
                    let Some(turn) = state.turn.as_ref().map(|turn| turn.number) else {
                        // Its turn has ended: nobody is there to ask, and dropping it answers cancelled.
                        return;
                    };
                    if state.questions >= shared.settings.max_open_questions {
                        drop(state);
                        shared.note(
                            NoticeKind::Fault,
                            "a permission request past the limit was answered cancelled",
                        );
                        return;
                    }
                    state.questions += 1;
                    turn
                };
                let session = Arc::clone(self);
                let asking = Arc::clone(shared);
                let spawned = thread::Builder::new()
                    .name("coqui-acp-harness-permission".into())
                    .spawn(move || {
                        session.ask(&asking, request, turn);
                        session.state().questions -= 1;
                    });
                if let Err(error) = spawned {
                    self.state().questions -= 1;
                    shared.note(
                        NoticeKind::Fault,
                        &format!("a permission request could not be asked: {error}"),
                    );
                }
            }
            Event::TurnEnd(end) => {
                self.finish(shared, end);
                self.read_usage_in_background(shared);
            }
            Event::Exited(reason) => {
                self.state().gone = Some(reason.clone());
                shared.note(NoticeKind::Exited, crate::stream::without_tail(&reason));
                self.finish(shared, TurnEnd::Failed(reason));
            }
            Event::Output(_) | Event::SessionId(_) => {}
            Event::Buckets(buckets) => shared.usage_buckets(self.id(), buckets),
            other => {
                if let Event::Notice { title, .. } = &other {
                    shared.note(NoticeKind::Notice, title);
                }
                if let Event::Usage(usage) = &other
                    && let Some(spend) = &usage.spend
                {
                    shared.usage_spend(self.id(), spend.clone());
                }
                let updates = self.state().shapes.updates(&other);
                for update in updates {
                    self.update(shared, update);
                }
            }
        }
    }

    /// Takes a turn for the prompt `id`, on the thread that reads the client, so a cancel read after it finds it.
    ///
    /// # Errors
    ///
    /// The error code and message when no turn can start.
    pub(crate) fn begin(&self, id: &Value) -> Result<u64, (i64, String)> {
        let mut state = self.state();
        if state.closed {
            return Err((INVALID_PARAMS, "the session is closed".into()));
        }
        if let Some(gone) = &state.gone {
            return Err((INTERNAL_ERROR, format!("the harness is gone: {gone}")));
        }
        if state.turn.is_some() || state.stale_ends > 0 {
            return Err((INVALID_REQUEST, "a turn is running".into()));
        }
        state.turns += 1;
        let number = state.turns;
        let version = state.shapes.version;
        state.turn = Some(Turn {
            number,
            request: (version < 2).then(|| id.clone()),
            cancelling: false,
            sent: false,
        });
        state.shapes.usage = None;
        Ok(number)
    }

    /// Gives up turn `number` before its prompt was sent. One answered as cancelled meanwhile has no ending to come.
    pub(crate) fn abort(&self, number: u64) {
        let mut state = self.state();
        if state
            .turn
            .as_ref()
            .is_some_and(|turn| turn.number == number)
        {
            state.turn = None;
        } else if state.answered_unsent == Some(number) {
            state.answered_unsent = None;
            state.stale_ends = state.stale_ends.saturating_sub(1);
        }
    }

    fn answered_early(&self, number: u64) -> bool {
        self.state().answered_unsent == Some(number)
    }

    /// Sends turn `number`'s prompt, answering `id`: at once with the user message's id in version 2, at the turn's
    /// end in version 1. A cancel that came first interrupts it now that it has reached the harness.
    pub(crate) fn send(
        self: &Arc<Self>,
        shared: &Arc<Shared>,
        id: &Value,
        number: u64,
        blocks: &[Value],
        content: Vec<Content>,
    ) {
        let version = self.version();
        let Some(conversation) = self.conversation() else {
            self.abort(number);
            shared.respond_error(id, INVALID_PARAMS, "the session is closed");
            return;
        };
        let message = uuid::Uuid::new_v4().to_string();
        if version >= 2 {
            self.update(
                shared,
                json!({"sessionUpdate": "user_message", "messageId": message, "content": blocks}),
            );
            self.update(
                shared,
                json!({"sessionUpdate": "state_update", "state": "running"}),
            );
        }
        if let Err(error) = conversation.prompt(content) {
            let answered = self.answered_early(number);
            self.abort(number);
            if answered {
                // The turn has been answered as cancelled; version 2's prompt still has its own answer.
                if version >= 2 {
                    shared.respond(id, json!({"messageId": message}));
                }
                return;
            }
            if version >= 2 {
                self.update(
                    shared,
                    json!({"sessionUpdate": "state_update", "state": "idle", "stopReason": "error",
                    "error": {"code": INTERNAL_ERROR, "message": error.to_string()}}),
                );
            }
            let code = match error {
                HarnessError::Refused(_) => INVALID_PARAMS,
                _ => INTERNAL_ERROR,
            };
            shared.respond_error(id, code, &error.to_string());
            return;
        }
        if version >= 2 {
            shared.respond(id, json!({"messageId": message}));
        }
        let deferred = {
            let mut state = self.state();
            match state.turn.as_mut().filter(|turn| turn.number == number) {
                Some(turn) => {
                    turn.sent = true;
                    turn.cancelling
                }
                None => {
                    let answered = state.answered_unsent == Some(number);
                    if answered {
                        state.answered_unsent = None;
                    }
                    answered
                }
            }
        };
        if deferred {
            Self::interrupt(shared, &conversation);
        }
    }

    /// Interrupts the running turn, and answers it as cancelled if it has not ended within the grace. A turn whose
    /// prompt has not reached the harness yet is interrupted once it has.
    pub(crate) fn cancel(self: &Arc<Self>, shared: &Arc<Shared>) {
        let (number, sent) = {
            let mut state = self.state();
            let Some(turn) = state.turn.as_mut().filter(|turn| !turn.cancelling) else {
                return;
            };
            turn.cancelling = true;
            (turn.number, turn.sent)
        };
        if sent && let Some(conversation) = self.conversation() {
            Self::interrupt(shared, &conversation);
        }
        self.grace(shared, number);
    }

    fn interrupt(shared: &Arc<Shared>, conversation: &Arc<dyn Conversation>) {
        // A harness that has stopped reading must not hold up the thread that reads the client.
        let interrupting = Arc::clone(conversation);
        let noting = Arc::clone(shared);
        let _ = thread::Builder::new()
            .name("coqui-acp-harness-interrupt".into())
            .spawn(move || {
                if let Err(error) = interrupting.interrupt() {
                    noting.note(
                        NoticeKind::Fault,
                        &format!("the turn could not be interrupted: {error}"),
                    );
                }
            });
    }

    fn grace(self: &Arc<Self>, shared: &Arc<Shared>, number: u64) {
        let session = Arc::clone(self);
        let shared = Arc::clone(shared);
        let grace = shared.settings.cancel_grace;
        let _ = thread::Builder::new()
            .name("coqui-acp-harness-cancel".into())
            .spawn(move || {
                thread::sleep(grace);
                let mut state = session.state();
                if state
                    .turn
                    .as_ref()
                    .is_some_and(|turn| turn.number == number)
                {
                    let turn = state.turn.take();
                    state.stale_ends += 1;
                    if turn.as_ref().is_some_and(|turn| !turn.sent) {
                        state.answered_unsent = Some(number);
                    }
                    let version = state.shapes.version;
                    let asking = core::mem::take(&mut state.asking);
                    drop(state);
                    shared.withdraw(&asking);
                    if let Some(turn) = turn {
                        session.answer(
                            &shared,
                            version,
                            turn,
                            &TurnEnd::stopped(Stop::Cancelled),
                            None,
                        );
                    }
                }
            });
    }

    /// Closes the session: a running turn is answered cancelled, open questions are answered cancelled to the
    /// harness, and the harness's session is closed and let go of.
    pub(crate) fn close(&self, shared: &Shared) {
        let (turn, version, asking) = {
            let mut state = self.state();
            state.closed = true;
            let asking = core::mem::take(&mut state.asking);
            (state.turn.take(), state.shapes.version, asking)
        };
        shared.withdraw(&asking);
        if let Some(turn) = turn {
            self.answer(
                shared,
                version,
                turn,
                &TurnEnd::stopped(Stop::Cancelled),
                None,
            );
        }
        let conversation = self
            .conversation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(conversation) = conversation {
            conversation.close();
        }
    }

    /// A harness that can be read with no turn is read once its session has opened and once each turn has
    /// ended, off the thread that delivers its events and off the request that opened it: where the account
    /// stands shows without a host's poll, and a slow read holds nothing up.
    pub(crate) fn read_usage_in_background(&self, shared: &Arc<Shared>) {
        let support = shared.usage_support();
        if !support.reports() || support.needs_turn {
            return;
        }
        let Some(conversation) = self.conversation() else {
            return;
        };
        let (reading, id) = (Arc::clone(shared), self.id().to_string());
        let spawned = thread::Builder::new()
            .name("coqui-acp-harness-usage".into())
            .spawn(move || {
                if let Some(buckets) = conversation.read_usage() {
                    reading.usage_buckets(&id, buckets);
                }
            });
        if let Err(error) = spawned {
            shared.note(
                NoticeKind::Fault,
                &format!("usage could not be read: {error}"),
            );
        }
    }

    fn finish(&self, shared: &Shared, end: TurnEnd) {
        // A hit nobody dated is dated when the connection hears it, in the report and in the answer alike.
        let end = match end {
            TurnEnd::Limited(mut hit) => {
                super::usage::stamp(&mut hit, super::usage::now());
                TurnEnd::Limited(hit)
            }
            other => other,
        };
        if let TurnEnd::Limited(hit) = &end {
            shared.usage_limit(self.id(), hit.clone());
        }
        let mut state = self.state();
        let asking = core::mem::take(&mut state.asking);
        if state.stale_ends > 0 && state.turn.is_none() {
            state.stale_ends -= 1;
            drop(state);
            shared.withdraw(&asking);
            return;
        }
        let version = state.shapes.version;
        let usage = state.shapes.usage.take();
        let Some(turn) = state.turn.take() else {
            drop(state);
            shared.withdraw(&asking);
            // A turn the harness ran by itself, or an exit between turns.
            if version >= 2
                && let TurnEnd::Stopped { stop, .. } = end
            {
                self.update(shared, json!({"sessionUpdate": "state_update", "state": "idle", "stopReason": stop.as_str()}));
            }
            return;
        };
        drop(state);
        shared.withdraw(&asking);
        let end = match end {
            TurnEnd::Limited(limit) => TurnEnd::Limited(limit),
            _ if turn.cancelling => TurnEnd::stopped(Stop::Cancelled),
            other => other,
        };
        self.answer(shared, version, turn, &end, usage.as_ref());
    }

    fn answer(
        &self,
        shared: &Shared,
        version: u8,
        turn: Turn,
        end: &TurnEnd,
        usage: Option<&Usage>,
    ) {
        if version >= 2 {
            let mut idle = Map::new();
            idle.insert("sessionUpdate".into(), json!("state_update"));
            idle.insert("state".into(), json!("idle"));
            match end {
                TurnEnd::Stopped { stop, .. } => {
                    idle.insert("stopReason".into(), json!(stop.as_str()));
                }
                TurnEnd::Limited(limit) => {
                    let title = limit
                        .text
                        .clone()
                        .unwrap_or_else(|| "A usage limit stopped the turn".into());
                    self.update(
                        shared,
                        json!({"sessionUpdate": "notice", "severity": "warning", "title": title}),
                    );
                    idle.insert("stopReason".into(), json!("refusal"));
                    idle.insert("_meta".into(), shape::limit_meta(limit));
                }
                TurnEnd::Failed(why) => {
                    idle.insert("stopReason".into(), json!("error"));
                    idle.insert(
                        "error".into(),
                        json!({"code": INTERNAL_ERROR, "message": why}),
                    );
                }
            }
            if let Some(usage) = usage {
                idle.insert("usage".into(), shape::turn_usage(usage));
            }
            self.update(shared, Value::Object(idle));
            return;
        }
        let Some(id) = turn.request else {
            return;
        };
        match end {
            TurnEnd::Stopped { stop, .. } => {
                let mut result = json!({"stopReason": stop.as_str()});
                if let Some(usage) = usage {
                    result["usage"] = shape::turn_usage(usage);
                }
                shared.respond(&id, result);
            }
            TurnEnd::Limited(limit) => {
                shared.respond(
                    &id,
                    json!({"stopReason": "refusal", "_meta": shape::limit_meta(limit)}),
                );
            }
            TurnEnd::Failed(why) => {
                shared.respond_error_data(
                    &id,
                    INTERNAL_ERROR,
                    "Internal error",
                    json!({"message": why}),
                );
            }
        }
    }

    /// Puts a permission request to the client and sends its answer to the harness. A question still open when its
    /// turn ends, is cancelled or its session closes is answered cancelled, and a later answer from the client is
    /// dropped.
    fn ask(&self, shared: &Shared, request: PermissionRequest, turn: u64) {
        let version = self.version();
        if version >= 2 {
            self.update(
                shared,
                json!({"sessionUpdate": "state_update", "state": "requires_action"}),
            );
        }
        let options: Vec<Value> = request
            .options
            .iter()
            .map(|option| json!({"optionId": option.id, "name": option.name, "kind": option.kind.as_str()}))
            .collect();
        let call = tool_call(&request.tool, version);
        let params = if version >= 2 {
            let mut params = json!({"sessionId": self.id(), "title": request.title,
                "subject": {"type": "tool_call", "toolCall": call}, "options": options});
            if let Some(description) = &request.description {
                params["description"] = json!(description);
            }
            params
        } else {
            json!({"sessionId": self.id(), "toolCall": version_1_question(call, &request), "options": options})
        };
        let (number, answered) = shared.begin_request("session/request_permission", params);
        {
            let mut state = self.state();
            let same_turn = state
                .turn
                .as_ref()
                .is_some_and(|running| running.number == turn);
            if state.closed || !same_turn {
                drop(state);
                shared.withdraw(&[number]);
            } else {
                state.asking.push(number);
            }
        }
        let answer = shared.wait(number, &answered, shared.settings.permission_timeout);
        self.state().asking.retain(|asked| *asked != number);
        let outcome = match answer {
            Ok(result) => {
                let outcome = result.get("outcome");
                let kind = outcome
                    .and_then(|outcome| outcome.get("outcome"))
                    .and_then(Value::as_str);
                let chosen = (kind == Some("selected"))
                    .then(|| {
                        outcome
                            .and_then(|outcome| outcome.get("optionId"))
                            .and_then(Value::as_str)
                    })
                    .flatten();
                match chosen {
                    Some(chosen) if request.options.iter().any(|option| option.id == chosen) => {
                        Outcome::Selected(chosen.to_string())
                    }
                    _ if kind == Some("cancelled") => Outcome::Cancelled,
                    // Chosen and not offered, or an outcome nobody knows: a refusal of this request alone.
                    _ => Outcome::Refused,
                }
            }
            // The client answered with an error: a refusal. A client that is gone, no answer in time and a question
            // withdrawn are the turn's end.
            Err(why) if why.starts_with(super::connection::CLIENT_ERROR) => Outcome::Refused,
            Err(_) => Outcome::Cancelled,
        };
        if let Err(error) = request.responder.answer(outcome) {
            shared.note(
                NoticeKind::Fault,
                &format!("a permission answer could not reach the harness: {error}"),
            );
        }
        if version >= 2 {
            let running = self
                .state()
                .turn
                .as_ref()
                .is_some_and(|turn| !turn.cancelling);
            if running {
                self.update(
                    shared,
                    json!({"sessionUpdate": "state_update", "state": "running"}),
                );
            }
        }
    }
}

/// Version 1's request has only a tool call to say what is asked: the question is its title when it has none, and the
/// question and its description come first in its content.
fn version_1_question(mut call: Value, request: &PermissionRequest) -> Value {
    let own = call
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut said = Vec::new();
    if own.as_deref() != Some(request.title.as_str()) && own.is_some() {
        said.push(request.title.clone());
    }
    said.extend(request.description.clone());
    if own.is_none() {
        call["title"] = json!(request.title);
    }
    if !said.is_empty() {
        let mut content = alloc::vec![json!({"type": "content",
            "content": {"type": "text", "text": said.join("\n\n")}})];
        if let Some(Value::Array(rest)) = call.get("content") {
            content.extend(rest.iter().cloned());
        }
        call["content"] = Value::Array(content);
    }
    call
}

/// The tool call a permission request is about, as the version's tool call fields.
fn tool_call(call: &ToolCall, version: u8) -> Value {
    let mut shapes = Shapes::new(version.max(2), 1);
    let mut value = shapes
        .updates(&Event::Tool(call.clone()))
        .pop()
        .unwrap_or_else(|| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.remove("sessionUpdate");
        if version < 2 {
            object.remove("name");
            if let Some(content) = &call.content {
                let mut v1 = Shapes::new(1, 1);
                let mut one = ToolCall::new(call.id.clone());
                one.content = Some(content.clone());
                if let Some(rendered) = v1.updates(&Event::Tool(one)).pop() {
                    object.insert("content".into(), rendered["content"].clone());
                }
            }
            if let Some(status) = object.get("status").and_then(Value::as_str)
                && status == "cancelled"
            {
                object.insert("status".into(), json!("failed"));
            }
        }
    }
    value
}
