//! The Station's hub for people attached to its live Runs.
//!
//! [`LocalHub`] implements [`RunHub`]: each Run the Station drives has a [`Room`] holding the
//! history it shows a client that attaches, the clients attached, the permission requests open
//! and the way to type into the Run and interrupt it. The code that drives a Run (a charge's
//! turns, a Mission's event pump) opens its room, publishes what the harness does, and asks
//! the room for a person's answer when the harness asks leave. Everything in a room is memory:
//! a Station that restarts starts its Runs' rooms empty.
//!
//! [`link`] carries the hub to Universal Exports, which is where people connect.

#[cfg(feature = "publisher")]
pub(super) mod link;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use coqui_acp_provider::AgentEvent;
use serde_json::{Value, json};

use super::unix_now;
use crate::directorates::section_00::acp::{
    PermissionDecider, PermissionDecision, PermissionRequest, RefuseAll,
};
use crate::directorates::section_00::hub::{
    Attachment, FrameSender, HubError, HubFrame, RunHub, RunTarget, frame_queue,
};
use crate::station::limits::AttachLimits;

/// What a Run's driver lets people do to the Run.
pub(crate) trait RunPort: Send + Sync {
    /// Types `text` into the Run as `person`: one turn of its own provider session, begun when the
    /// Run is free, after any turn under way.
    /// `accepted` is called once the prompt is taken and before its turn can begin, so what the
    /// attached people are shown comes in order.
    fn prompt(&self, person: &str, text: &str, accepted: &dyn Fn()) -> Result<(), HubError>;

    /// Interrupts the turn under way: whether there was one to interrupt.
    fn interrupt(&self, person: &str) -> Result<bool, HubError>;
}

struct Client {
    person: String,
    sender: FrameSender,
}

struct Request {
    options: Vec<String>,
    answer: Option<(String, String)>,
}

#[derive(Default)]
struct Inner {
    next_seq: u64,
    history: VecDeque<HubFrame>,
    clients: BTreeMap<String, Client>,
    requests: BTreeMap<u64, Request>,
    next_request: u64,
    busy: bool,
    closed: bool,
}

/// One Run's history, clients and requests.
pub(crate) struct Room {
    limits: AttachLimits,
    inner: Mutex<Inner>,
    answered: Condvar,
    port: Mutex<Option<Arc<dyn RunPort>>>,
}

impl Room {
    fn new(limits: AttachLimits) -> Self {
        Self {
            limits,
            inner: Mutex::new(Inner::default()),
            answered: Condvar::new(),
            port: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn port(&self) -> Option<Arc<dyn RunPort>> {
        self.port
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Puts `frame` in the history and sends it to the clients it is for.
    fn publish_in(&self, inner: &mut Inner, mut frame: HubFrame) {
        match frame.client.clone() {
            Some(client) => {
                if let Some(entry) = inner.clients.get(&client)
                    && entry.sender.try_send(frame).is_err()
                {
                    inner.clients.remove(&client);
                }
            }
            None => {
                inner.next_seq += 1;
                frame.seq = inner.next_seq;
                inner.history.push_back(frame.clone());
                while !self.limits.history_frames.allows(inner.history.len()) {
                    inner.history.pop_front();
                }
                inner
                    .clients
                    .retain(|_, client| client.sender.try_send(frame.clone()).is_ok());
            }
        }
    }

    fn publish(&self, kind: &str, data: Value) {
        let mut inner = self.lock();
        self.publish_in(&mut inner, HubFrame::new(kind, unix_now(), data));
    }

    /// Shows what the harness did.
    pub(crate) fn publish_event(&self, event: &AgentEvent) {
        self.publish(
            "event",
            json!({"kind": event.kind, "text": event.text, "tools": event.tools}),
        );
    }

    /// Shows a message the Station itself gave the Run (a wake), as it has no person.
    pub(crate) fn publish_prompt(&self, from: &str, text: &str) {
        self.publish("prompt", json!({"from": from, "text": text}));
    }

    /// Shows that something went wrong that the attached people should know.
    pub(crate) fn publish_error(&self, reason: &str) {
        self.publish("error", json!({"reason": reason}));
    }

    /// Marks a turn begun or ended, `by` being what began it: a person, or `station`.
    pub(crate) fn turn(&self, begun: bool, by: &str) {
        let mut inner = self.lock();
        inner.busy = begun;
        let state = if begun { "begun" } else { "ended" };
        self.publish_in(
            &mut inner,
            HubFrame::new("turn", unix_now(), json!({"state": state, "by": by})),
        );
    }

    /// Whether anyone is attached.
    #[cfg(test)]
    pub(crate) fn attached(&self) -> usize {
        self.lock().clients.len()
    }

    /// Asks the attached people to decide a permission request of `tool`, offering `options`
    /// (an ID and a label each). The first answer decides. With nobody attached, or nobody left
    /// by the time `permission_wait` has passed, `None`: the asker decides as it would have.
    pub(crate) fn ask(
        &self,
        tool: &str,
        options: &[(String, String)],
        detail: &Value,
    ) -> Option<(String, String)> {
        let mut inner = self.lock();
        if inner.clients.is_empty() || inner.closed {
            return None;
        }
        inner.next_request += 1;
        let id = inner.next_request;
        inner.requests.insert(
            id,
            Request {
                options: options.iter().map(|(option, _)| option.clone()).collect(),
                answer: None,
            },
        );
        let offered: Vec<Value> = options
            .iter()
            .map(|(option, label)| json!({"id": option, "label": label}))
            .collect();
        self.publish_in(
            &mut inner,
            HubFrame::new(
                "permission",
                unix_now(),
                json!({"request": id, "tool": tool, "options": offered, "detail": detail}),
            ),
        );
        let deadline = self
            .limits
            .permission_wait
            .get()
            .map(|wait| Instant::now() + wait);
        loop {
            if let Some(answer) = inner.requests.get(&id).and_then(|r| r.answer.clone()) {
                inner.requests.remove(&id);
                return Some(answer);
            }
            let over = deadline.is_some_and(|deadline| Instant::now() >= deadline);
            if inner.clients.is_empty() || inner.closed || over {
                inner.requests.remove(&id);
                let reason = if over {
                    "unanswered"
                } else {
                    "nobody attached"
                };
                self.publish_in(
                    &mut inner,
                    HubFrame::new(
                        "answered",
                        unix_now(),
                        json!({"request": id, "by": Value::Null, "option": Value::Null,
                               "reason": reason}),
                    ),
                );
                return None;
            }
            let every = self.limits.poll_slice.0;
            let slice = deadline
                .map_or(every, |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                })
                .min(every);
            inner = self
                .answered
                .wait_timeout(inner, slice)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The Run is gone: its clients are told and let go, and a request waiting is given up.
    pub(crate) fn close(&self, reason: &str) {
        let mut inner = self.lock();
        if inner.closed {
            return;
        }
        inner.closed = true;
        self.publish_in(
            &mut inner,
            HubFrame::new("closed", unix_now(), json!({"reason": reason})),
        );
        inner.clients.clear();
        self.answered.notify_all();
    }

    fn attach(
        &self,
        client: &str,
        person: &str,
        after: Option<u64>,
    ) -> Result<Attachment, HubError> {
        let mut inner = self.lock();
        if inner.closed {
            return Err(HubError::NoRun);
        }
        if !inner.clients.contains_key(client)
            && !self.limits.clients.allows(inner.clients.len() + 1)
        {
            return Err(HubError::TooManyClients);
        }
        let mut replay: Vec<HubFrame> = inner
            .history
            .iter()
            .filter(|frame| after.is_none_or(|after| frame.seq > after))
            .cloned()
            .collect();
        let bound = self
            .limits
            .client_backlog
            .get()
            .map(|bound| usize::try_from(bound).unwrap_or(usize::MAX));
        let (sender, live) = frame_queue(bound);
        inner.clients.insert(
            client.to_owned(),
            Client {
                person: person.to_owned(),
                sender,
            },
        );
        let attached: BTreeMap<String, String> = inner
            .clients
            .iter()
            .map(|(client, entry)| (client.clone(), entry.person.clone()))
            .collect();
        replay.push(
            HubFrame::new(
                "ready",
                unix_now(),
                json!({"busy": inner.busy, "attached": attached,
                       "last": inner.next_seq}),
            )
            .to(client),
        );
        // The newcomer's own `attached` frame is the first of its live ones.
        self.publish_in(
            &mut inner,
            HubFrame::new(
                "attached",
                unix_now(),
                json!({"person": person, "client": client}),
            ),
        );
        Ok(Attachment::new(replay, live, attached))
    }

    fn detach(&self, client: &str) {
        let mut inner = self.lock();
        if let Some(entry) = inner.clients.get(client) {
            let person = entry.person.clone();
            self.publish_in(
                &mut inner,
                HubFrame::new(
                    "detached",
                    unix_now(),
                    json!({"person": person, "client": client}),
                ),
            );
            inner.clients.remove(client);
            self.answered.notify_all();
        }
    }

    fn person_of(&self, client: &str) -> Result<(), HubError> {
        self.lock()
            .clients
            .contains_key(client)
            .then_some(())
            .ok_or(HubError::NotAttached)
    }

    fn prompt(&self, client: &str, person: &str, text: &str) -> Result<(), HubError> {
        self.person_of(client)?;
        let port = self.port().ok_or(HubError::NoRun)?;
        let accepted = || self.publish("prompt", json!({"from": person, "text": text}));
        let sent = port.prompt(person, text, &accepted);
        if let Err(error) = &sent {
            self.publish(
                "error",
                json!({"reason": error.to_string(), "refused": "prompt", "from": person}),
            );
        }
        sent
    }

    fn answer(
        &self,
        client: &str,
        person: &str,
        request: u64,
        option: &str,
    ) -> Result<(), HubError> {
        let mut inner = self.lock();
        if !inner.clients.contains_key(client) {
            return Err(HubError::NotAttached);
        }
        let open = inner
            .requests
            .get_mut(&request)
            .filter(|open| open.answer.is_none())
            .ok_or(HubError::NoRequest)?;
        if !open.options.iter().any(|offered| offered == option) {
            return Err(HubError::NotAnOption);
        }
        open.answer = Some((person.to_owned(), option.to_owned()));
        self.publish_in(
            &mut inner,
            HubFrame::new(
                "answered",
                unix_now(),
                json!({"request": request, "by": person, "option": option}),
            ),
        );
        self.answered.notify_all();
        Ok(())
    }

    fn interrupt(&self, client: &str, person: &str) -> Result<(), HubError> {
        self.person_of(client)?;
        let port = self.port().ok_or(HubError::NoRun)?;
        if port.interrupt(person)? {
            self.publish("interrupted", json!({"by": person}));
        }
        Ok(())
    }
}

/// The hub of this Station: the rooms of the Runs it drives.
#[derive(Default)]
pub(crate) struct LocalHub {
    limits: AttachLimits,
    rooms: Mutex<BTreeMap<String, Arc<Room>>>,
}

impl LocalHub {
    /// A hub whose rooms follow `limits`.
    pub(crate) fn new(limits: AttachLimits) -> Self {
        Self {
            limits,
            rooms: Mutex::default(),
        }
    }

    fn rooms(&self) -> MutexGuard<'_, BTreeMap<String, Arc<Room>>> {
        self.rooms.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The room of `target`, made if there is none, driven through `port` from now on. A room
    /// that is open already keeps its history and its clients: a Run resumed after a restart of
    /// its harness is the same Run to the people attached.
    pub(crate) fn open(&self, target: &RunTarget, port: Arc<dyn RunPort>) -> Arc<Room> {
        let mut rooms = self.rooms();
        let room = rooms
            .entry(target.key())
            .or_insert_with(|| Arc::new(Room::new(self.limits.clone())))
            .clone();
        *room.port.lock().unwrap_or_else(PoisonError::into_inner) = Some(port);
        room
    }

    /// The room of `target`, when its Run is driven here.
    pub(crate) fn room(&self, target: &RunTarget) -> Option<Arc<Room>> {
        self.rooms().get(&target.key()).cloned()
    }

    /// Closes the room of `target` when `port` still drives it: a newer driver of the same Run
    /// keeps it.
    pub(crate) fn close(&self, target: &RunTarget, port: &Arc<dyn RunPort>, reason: &str) {
        let room = {
            let mut rooms = self.rooms();
            let key = target.key();
            let current = rooms.get(&key).and_then(|room| room.port());
            if current.is_some_and(|current| {
                Arc::as_ptr(&current).cast::<()>() == Arc::as_ptr(port).cast::<()>()
            }) {
                rooms.remove(&key)
            } else {
                None
            }
        };
        if let Some(room) = room {
            room.close(reason);
        }
    }

    fn of(&self, target: &RunTarget) -> Result<Arc<Room>, HubError> {
        self.room(target).ok_or(HubError::NoRun)
    }
}

impl RunHub for LocalHub {
    fn attach(
        &self,
        target: &RunTarget,
        client: &str,
        person: &str,
        after: Option<u64>,
    ) -> Result<Attachment, HubError> {
        self.of(target)?.attach(client, person, after)
    }

    fn detach(&self, target: &RunTarget, client: &str) {
        if let Some(room) = self.room(target) {
            room.detach(client);
        }
    }

    fn prompt(
        &self,
        target: &RunTarget,
        client: &str,
        person: &str,
        text: &str,
    ) -> Result<(), HubError> {
        self.of(target)?.prompt(client, person, text)
    }

    fn answer(
        &self,
        target: &RunTarget,
        client: &str,
        person: &str,
        request: u64,
        option: &str,
    ) -> Result<(), HubError> {
        self.of(target)?.answer(client, person, request, option)
    }

    fn interrupt(&self, target: &RunTarget, client: &str, person: &str) -> Result<(), HubError> {
        self.of(target)?.interrupt(client, person)
    }
}

/// How people attached to a Mission's Run type into it and interrupt it: the same paths the
/// Station's own message and interrupt routes take.
struct MissionPort {
    state: super::DaemonState,
    mission_id: String,
    runtime: tokio::runtime::Handle,
}

impl RunPort for MissionPort {
    fn prompt(&self, _person: &str, text: &str, accepted: &dyn Fn()) -> Result<(), HubError> {
        self.runtime
            .block_on(super::send_message(
                axum::extract::State(self.state.clone()),
                axum::extract::Path(self.mission_id.clone()),
                axum::Json(super::SendMessage {
                    text: text.to_owned(),
                }),
            ))
            .map_err(|refused| HubError::Failed(format!("the Run refused it ({})", refused.0)))?;
        accepted();
        Ok(())
    }

    fn interrupt(&self, _person: &str) -> Result<bool, HubError> {
        self.runtime
            .block_on(super::interrupt_mission(
                axum::extract::State(self.state.clone()),
                axum::extract::Path(self.mission_id.clone()),
            ))
            .map(|_| true)
            .map_err(|refused| HubError::Failed(format!("the Run refused it ({})", refused.0)))
    }
}

/// A Mission's room for as long as one event pump drives its Run: dropped, the room closes
/// unless a later pump of the same Mission has taken it over.
pub(super) struct MissionRoom {
    hub: Arc<LocalHub>,
    target: RunTarget,
    port: Arc<dyn RunPort>,
    pub(super) room: Arc<Room>,
}

impl MissionRoom {
    pub(super) fn open(
        state: &super::DaemonState,
        mission_id: &str,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let operation = state
            .missions
            .lock()
            .ok()
            .and_then(|missions| {
                missions
                    .get(mission_id)
                    .and_then(|m| m.operation_id.clone())
            })
            .unwrap_or_default();
        let target = RunTarget::Mission {
            operation,
            mission: mission_id.to_owned(),
        };
        let port: Arc<dyn RunPort> = Arc::new(MissionPort {
            state: state.clone(),
            mission_id: mission_id.to_owned(),
            runtime,
        });
        let room = state.hub.open(&target, port.clone());
        Self {
            hub: state.hub.clone(),
            target,
            port,
            room,
        }
    }
}

impl Drop for MissionRoom {
    fn drop(&mut self) {
        self.hub.close(&self.target, &self.port, "the Run ended");
    }
}

/// Offers an agent's permission requests to the people attached to its Run: with someone
/// attached, the first answer decides, and with nobody the `inner` decider does. A restricted
/// Run keeps its refusals, which wrap this decider.
pub(crate) struct HubDecider {
    hub: Arc<LocalHub>,
    target: RunTarget,
    inner: Arc<dyn PermissionDecider>,
}

impl HubDecider {
    /// A decider for the Run of `target` that refuses what nobody attached answers.
    pub(crate) fn refusing(hub: &Arc<LocalHub>, target: RunTarget) -> Arc<dyn PermissionDecider> {
        Arc::new(Self {
            hub: hub.clone(),
            target,
            inner: Arc::new(RefuseAll),
        })
    }
}

impl PermissionDecider for HubDecider {
    fn decide(&self, request: &PermissionRequest) -> PermissionDecision {
        let Some(room) = self.hub.room(&self.target) else {
            return self.inner.decide(request);
        };
        let tool = request
            .title
            .clone()
            .or_else(|| request.kind.clone())
            .unwrap_or_else(|| "a tool".to_owned());
        let options = [
            (ALLOW.to_owned(), "Allow once".to_owned()),
            (REFUSE.to_owned(), "Refuse".to_owned()),
        ];
        let detail = json!({"kind": request.kind, "input": request.raw_input});
        match room.ask(&tool, &options, &detail) {
            Some((_, option)) if option == ALLOW => PermissionDecision::Allow,
            Some((by, _)) => PermissionDecision::Deny(format!("{by} refused it")),
            None => self.inner.decide(request),
        }
    }
}

/// The option of a permission request that lets the tool run once.
pub(crate) const ALLOW: &str = "allow";
/// The option that refuses it.
pub(crate) const REFUSE: &str = "refuse";

#[cfg(test)]
mod tests {
    use super::super::tests::{RECORDED_MESSAGES, start_mission_by_id, test_state, wait_for_state};
    use super::super::{MissionId, MissionState};
    use super::*;

    /// A Mission's Run is attached to like a charge's: its history, then what people type into the
    /// Run it already has.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_missions_run_is_attached_like_a_charges() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(
            root.path(),
            "",
            "[agent.007]\nhandler = \"mock-recorder\"\n",
        );
        let mission_id = format!("hub-mission-{}", std::process::id());
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(&mission_id).unwrap(),
                Some("G".to_owned()),
                "work".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        start_mission_by_id(&state, &mission_id).await;
        wait_for_state(&state, &mission_id, MissionState::Blocked).await;
        let target = RunTarget::Mission {
            operation: "G".to_owned(),
            mission: mission_id.clone(),
        };

        let hub = state.hub.clone();
        let attachment = hub.attach(&target, "c1", "alice", None).unwrap();
        assert!(
            attachment.replay.iter().any(|frame| frame.kind == "event"),
            "the Run's history is replayed: {:?}",
            attachment.replay
        );
        let (typing, asking) = (hub.clone(), target.clone());
        tokio::task::spawn_blocking(move || typing.prompt(&asking, "c1", "alice", "carry on"))
            .await
            .unwrap()
            .unwrap();
        // The harness takes it from the ACP connection a moment later.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !RECORDED_MESSAGES
            .lock()
            .unwrap()
            .iter()
            .any(|(_, text)| text == "carry on")
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the Run was not sent what alice typed"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let seen: Vec<HubFrame> = attachment.live.drain();
        assert!(
            seen.iter()
                .any(|frame| frame.kind == "prompt" && frame.data["from"] == "alice"),
            "{seen:?}"
        );
        // Detaching leaves the Mission's Run where it was.
        hub.detach(&target, "c1");
        assert!(state.runs.lock().unwrap().contains_key(&mission_id));
        state
            .stopping
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
