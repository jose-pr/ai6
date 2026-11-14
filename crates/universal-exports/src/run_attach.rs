//! People attached to a charge's or a Mission's live Run, through the gateway.
//!
//! A person with `charge.converse` on a charge's scope (`operation:<codename>`, `station:<id>`,
//! or `config:charge` for M's), or `mission.control` on a Mission's Operation, attaches to the Run
//! the Station keeps: its history is replayed, its live stream follows, and the person may type
//! into it, answer what it asks and interrupt it, with others attached at once; detaching leaves
//! it running. The gateway authenticates and authorizes every request and audits it before it is
//! passed on; the Run itself, its history and its permission requests are the Station's
//! ([`ai6::directorates::section_00::hub`]), reached by the Station polling `GET
//! /v1/station/hub/next` and delivering frames to `POST /v1/station/hub/frames`.
//!
//! `GET /v1/user/attach` is a WebSocket of text frames. The first, within `[attach]
//! hello_timeout`, is `{"type":"hello","token":ACCESS_TOKEN,"charge":SCOPE | "mission":
//! {"operation","mission"},"after"?:SEQ}`. Then the person sends `{"type":"prompt","text"}`,
//! `{"type":"answer","request","option"}`, `{"type":"interrupt"}` or `{"type":"detach"}`, and
//! receives `{"type":"attached","client","target"}`, `{"type":"frame","frame":HubFrame}` for
//! everything the Run shows, and `{"type":"error","code","message"}`.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ai6::directorates::m::charges::ChargeScope;
use ai6::directorates::section_00::hub::{HubCommand, HubDelivery, HubFrame, HubOp, RunTarget};
use ai6::{AuditRecord, Limit, Person, UnexAttachLimits};
use axum::Json;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};
use uuid::Uuid;

use crate::{ApiError, Gateway, actor_of, bearer, resolve_user, resource_action_allowed};

/// Where M's charge is authorized.
const M_RESOURCE: &str = "config:charge";
/// The action that attaches to a charge's Run, types into it and answers it.
const CONVERSE: &str = "charge.converse";
const CONTROL: &str = "mission.control";
const STATION_CONTROL: &str = "station.configure";

/// What a client asks of the gateway, as a text frame.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FromClient {
    Hello {
        token: String,
        #[serde(default)]
        charge: Option<String>,
        #[serde(default)]
        mission: Option<MissionRef>,
        #[serde(default)]
        after: Option<u64>,
    },
    Prompt {
        text: String,
    },
    Answer {
        request: u64,
        option: String,
    },
    Interrupt {},
    Detach {},
}

#[derive(Deserialize)]
struct MissionRef {
    operation: String,
    mission: String,
}

/// What the gateway keeps of attached people: who is attached to which Run, and what waits for
/// each Station.
#[derive(Default)]
pub(crate) struct Relay {
    inner: Mutex<Book>,
    ready: Notify,
}

#[derive(Default)]
struct Book {
    next: u64,
    commands: HashMap<String, VecDeque<HubCommand>>,
    clients: HashMap<String, Slot>,
}

struct Slot {
    target: RunTarget,
    station: String,
    sender: mpsc::UnboundedSender<HubFrame>,
    /// Frames sent and not yet taken by the person's socket.
    queued: std::sync::Arc<AtomicUsize>,
}

/// What a person's socket reads: the frames the gateway queued for them.
struct Frames {
    frames: mpsc::UnboundedReceiver<HubFrame>,
    queued: std::sync::Arc<AtomicUsize>,
}

impl Frames {
    async fn next(&mut self) -> Option<HubFrame> {
        let frame = self.frames.recv().await?;
        self.queued.fetch_sub(1, Ordering::AcqRel);
        Some(frame)
    }
}

/// Why a request could not be taken.
enum Refused {
    /// The Run has all the people attached `[attach] clients` allows.
    Full,
    /// The person is not attached any more: their attachment ended.
    Gone,
    /// As many requests wait for the Station as `[attach] queue` allows.
    Busy,
}

impl Relay {
    fn lock(&self) -> std::sync::MutexGuard<'_, Book> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Attaches `person` to `target`, run by `station`: the client's ID and its frames.
    fn open(
        &self,
        limits: &UnexAttachLimits,
        station: &str,
        target: &RunTarget,
        person: &str,
        after: Option<u64>,
    ) -> Result<(String, Frames), Refused> {
        let mut state = self.lock();
        let attached = state
            .clients
            .values()
            .filter(|slot| &slot.target == target)
            .count();
        if !limits.clients.allows(attached + 1) {
            return Err(Refused::Full);
        }
        let waiting = state.commands.get(station).map_or(0, VecDeque::len);
        if !limits.queue.allows(waiting + 1) {
            return Err(Refused::Busy);
        }
        state.next += 1;
        let client = format!("c{}-{}", state.next, Uuid::new_v4().simple());
        let (sender, frames) = mpsc::unbounded_channel();
        let queued = std::sync::Arc::new(AtomicUsize::new(0));
        state.clients.insert(
            client.clone(),
            Slot {
                target: target.clone(),
                station: station.to_owned(),
                sender,
                queued: queued.clone(),
            },
        );
        state
            .commands
            .entry(station.to_owned())
            .or_default()
            .push_back(HubCommand::new(
                target.clone(),
                client.clone(),
                person,
                HubOp::Attach { after },
            ));
        drop(state);
        self.ready.notify_waiters();
        Ok((client, Frames { frames, queued }))
    }

    /// Passes a request of `client`'s person to the Station.
    fn submit(
        &self,
        limits: &UnexAttachLimits,
        client: &str,
        person: &str,
        op: HubOp,
    ) -> Result<(), Refused> {
        let mut state = self.lock();
        let Some((station, target)) = state
            .clients
            .get(client)
            .map(|slot| (slot.station.clone(), slot.target.clone()))
        else {
            return Err(Refused::Gone);
        };
        let queue = state.commands.entry(station).or_default();
        if !limits.queue.allows(queue.len() + 1) {
            return Err(Refused::Busy);
        }
        queue.push_back(HubCommand::new(target, client, person, op));
        drop(state);
        self.ready.notify_waiters();
        Ok(())
    }

    /// Lets `client` go and tells its Station, whatever is waiting for it.
    fn close(&self, client: &str, person: &str) {
        let mut state = self.lock();
        if let Some(slot) = state.clients.remove(client) {
            state
                .commands
                .entry(slot.station)
                .or_default()
                .push_back(HubCommand::new(slot.target, client, person, HubOp::Detach));
            drop(state);
            self.ready.notify_waiters();
        }
    }

    /// Hands what a Station delivered to the people it is for. `false` when it was for a client
    /// that is not here (gone), so the Station stops sending to it.
    fn deliver(&self, backlog: Limit, delivery: HubDelivery) -> bool {
        let mut state = self.lock();
        let mut known = true;
        let mut dropped: Vec<String> = Vec::new();
        for frame in delivery.frames {
            let closing = frame.kind == "closed";
            let recipients: Vec<String> = match &frame.client {
                Some(client) => {
                    if state
                        .clients
                        .get(client)
                        .is_some_and(|slot| slot.station == delivery.station)
                    {
                        vec![client.clone()]
                    } else {
                        known = false;
                        Vec::new()
                    }
                }
                None => state
                    .clients
                    .iter()
                    .filter(|(_, slot)| {
                        slot.target == delivery.target && slot.station == delivery.station
                    })
                    .map(|(client, _)| client.clone())
                    .collect(),
            };
            for client in recipients {
                let Some(slot) = state.clients.get(&client) else {
                    continue;
                };
                // A person further behind than the backlog allows is let go.
                let room = backlog.allows(slot.queued.load(Ordering::Acquire) + 1);
                let sent = room && {
                    slot.queued.fetch_add(1, Ordering::AcqRel);
                    let sent = slot.sender.send(frame.clone()).is_ok();
                    if !sent {
                        slot.queued.fetch_sub(1, Ordering::AcqRel);
                    }
                    sent
                };
                if !sent || closing {
                    dropped.push(client);
                }
            }
        }
        // A person who fell too far behind is let go, as one whose Run closed is.
        for client in dropped {
            if let Some(slot) = state.clients.remove(&client) {
                state
                    .commands
                    .entry(slot.station)
                    .or_default()
                    .push_back(HubCommand::new(slot.target, client, "", HubOp::Detach));
            }
        }
        known
    }

    fn take(&self, station: &str) -> Vec<HubCommand> {
        self.lock()
            .commands
            .get_mut(station)
            .map(|queue| queue.drain(..).collect())
            .unwrap_or_default()
    }
}

/// What a person asked to attach to, and what it takes.
struct Subject {
    target: RunTarget,
    /// The scope the actions are held on.
    resource: String,
    /// What attaching, typing and answering take.
    converse: &'static str,
    /// What interrupting takes.
    control: &'static str,
    /// The Operation, when the Run belongs to one; the audit records it.
    operation: Option<String>,
    /// `charge` or `mission`: the audit action's family.
    family: &'static str,
}

impl Subject {
    fn of(request: (Option<String>, Option<MissionRef>)) -> Option<Self> {
        match request {
            (Some(scope), None) => {
                let scope: ChargeScope = scope.parse().ok()?;
                let target = RunTarget::Charge {
                    scope: scope.to_string(),
                };
                Some(match &scope {
                    ChargeScope::M => Self {
                        target,
                        resource: M_RESOURCE.to_owned(),
                        converse: CONVERSE,
                        control: STATION_CONTROL,
                        operation: None,
                        family: "charge",
                    },
                    ChargeScope::Operation(operation) => Self {
                        target,
                        resource: crate::tree::operation_resource(operation),
                        converse: CONVERSE,
                        control: CONTROL,
                        operation: Some(operation.clone()),
                        family: "charge",
                    },
                    ChargeScope::Station(station) => Self {
                        target,
                        resource: format!("station:{station}"),
                        converse: CONVERSE,
                        control: STATION_CONTROL,
                        operation: None,
                        family: "charge",
                    },
                    _ => return None,
                })
            }
            (None, Some(mission)) => Some(Self {
                resource: crate::tree::operation_resource(&mission.operation),
                converse: CONTROL,
                control: CONTROL,
                operation: Some(mission.operation.clone()),
                family: "mission",
                target: RunTarget::Mission {
                    operation: mission.operation,
                    mission: mission.mission,
                },
            }),
            _ => None,
        }
    }

    async fn allows(&self, gateway: &Gateway, person: &Person, action: &'static str) -> bool {
        resource_action_allowed(gateway, person.clone(), self.resource.clone(), action)
            .await
            .unwrap_or(false)
    }

    /// Records what `person` did, before it is done.
    async fn audit(&self, gateway: &Gateway, person: &Person, what: &str) -> Result<(), ApiError> {
        let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
        let resource = (self.resource.starts_with("station:")
            || self.resource.starts_with("config:"))
        .then(|| self.resource.clone());
        let record = AuditRecord {
            id: Uuid::new_v4().to_string(),
            actor: actor_of(person),
            action: format!("{}.{what}", self.family),
            operation: self.operation.clone().unwrap_or_default(),
            resource,
            target: self.target.key(),
            at: crate::unix_timestamp()?,
        };
        tokio::task::spawn_blocking(move || registry.record_audit(&record))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|_| ApiError::internal())
    }

    /// The Station that runs the Run now, from the Registry.
    async fn station(&self, gateway: &Gateway) -> Option<(String, RunTarget)> {
        let registry = gateway.registry.clone()?;
        let target = self.target.clone();
        tokio::task::spawn_blocking(move || {
            let now = crate::unix_timestamp().ok()?;
            match &target {
                RunTarget::Charge { scope } => {
                    let record = registry.charge(scope).ok()??;
                    (record.lease_expires > now).then(|| (record.station, target.clone()))
                }
                RunTarget::Mission { operation, mission } => {
                    let uid = registry
                        .resolve_mission(mission)
                        .ok()
                        .flatten()
                        .map_or_else(|| mission.clone(), |handle| handle.mission_id);
                    let run = registry
                        .runs()
                        .ok()?
                        .into_iter()
                        .filter(|run| run.mission_id == uid && !run.status.is_closed())
                        .max_by_key(|run| run.number)?;
                    // The Operation is the Registry's word, never the client's: from the Run, else
                    // from the Mission's filed brief. A hello naming another is refused.
                    let filed = match run.operation.clone() {
                        Some(filed) => filed,
                        None => registry.filed_brief(&uid).ok().flatten()?.brief.operation?,
                    };
                    let station = run.station;
                    (filed == *operation).then_some((
                        station,
                        RunTarget::Mission {
                            operation: filed,
                            mission: uid,
                        },
                    ))
                }
                _ => None,
            }
        })
        .await
        .ok()
        .flatten()
    }
}

fn frame(value: &Value) -> Message {
    Message::Text(value.to_string().into())
}

fn problem(code: &str, message: &str) -> Value {
    json!({"type": "error", "code": code, "message": message})
}

fn close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

/// The most a frame carrying a prompt of `[attach] prompt_bytes` takes: every byte written as a
/// six-byte JSON escape, and the slack.
fn frame_limit(limits: &UnexAttachLimits) -> usize {
    limits
        .prompt_bytes
        .or_max()
        .saturating_mul(6)
        .saturating_add(limits.frame_slack.or_max())
}

/// `GET /v1/user/attach`: a person's attachment to a Run, as described in this module.
pub(crate) async fn socket(State(gateway): State<Gateway>, upgrade: WebSocketUpgrade) -> Response {
    let most = gateway.config.unex().attach.socket_bytes.or_max();
    upgrade
        .max_message_size(most)
        .on_upgrade(move |socket| attached(socket, gateway))
}

type Out = futures_util::stream::SplitSink<WebSocket, Message>;

async fn refuse(sender: &mut Out, code: &str, message: &str) {
    let _ = sender.send(frame(&problem(code, message))).await;
    let _ = sender.send(close(1008, "refused")).await;
}

/// Who `token` is now, and that they still hold the right to attach to `subject`.
async fn still_holds(
    gateway: &Gateway,
    subject: &Subject,
    token: &str,
) -> Result<Person, (&'static str, String)> {
    let person = resolve_user(gateway, token)
        .await
        .map_err(|_| ("unauthorized", "Your sign-in has ended.".to_owned()))?;
    if subject.allows(gateway, &person, subject.converse).await {
        Ok(person)
    } else {
        Err((
            "forbidden",
            "You may no longer attach to that Run.".to_owned(),
        ))
    }
}

/// When `token`'s sign-in ends, as Unix seconds; `None` for one the Registry holds no record of
/// (a key).
async fn sign_in_ends(gateway: &Gateway, token: &str) -> Option<f64> {
    let registry = gateway.registry.clone()?;
    let digest = crate::access_token_digest(token);
    let record = tokio::task::spawn_blocking(move || registry.find_access_token(&digest))
        .await
        .ok()?
        .ok()??;
    Some(record.expires_at())
}

/// The next time to look at whether the person still holds what they attached with: after
/// `[attach] recheck_every`, or when their sign-in ends if that is sooner.
async fn next_check(gateway: &Gateway, token: &str) -> tokio::time::Instant {
    let every = gateway.config.unex().attach.recheck_every.0;
    let mut wait = every;
    if let (Some(ends), Ok(now)) = (sign_in_ends(gateway, token).await, crate::unix_timestamp()) {
        wait = wait.min(Duration::from_secs_f64((ends - now).max(0.0)));
    }
    tokio::time::Instant::now() + wait
}

async fn attached(socket: WebSocket, gateway: Gateway) {
    let limits = gateway.config.unex().attach.clone();
    let (mut sender, mut receiver) = socket.split();
    let hello = tokio::time::timeout(limits.hello_timeout.0, receiver.next()).await;
    let Ok(Some(Ok(Message::Text(hello)))) = hello else {
        let _ = sender.send(close(1008, "say hello first")).await;
        return;
    };
    let Ok(FromClient::Hello {
        token,
        charge,
        mission,
        after,
    }) = serde_json::from_str::<FromClient>(&hello)
    else {
        let _ = sender.send(close(1008, "say hello first")).await;
        return;
    };
    let Ok(person) = resolve_user(&gateway, &token).await else {
        refuse(&mut sender, "unauthorized", "Sign in again to attach.").await;
        return;
    };
    let Some(subject) = Subject::of((charge, mission)) else {
        refuse(
            &mut sender,
            "bad_request",
            "Name a charge (m, operation:CODENAME or station:ID) or a Mission.",
        )
        .await;
        return;
    };
    if !subject.allows(&gateway, &person, subject.converse).await {
        refuse(&mut sender, "forbidden", "You may not attach to that Run.").await;
        return;
    }
    let Some((station, target)) = subject.station(&gateway).await else {
        refuse(&mut sender, "no_run", "No Station runs that Run now.").await;
        return;
    };
    if subject.audit(&gateway, &person, "attach").await.is_err() {
        refuse(
            &mut sender,
            "unavailable",
            "The attachment could not be recorded.",
        )
        .await;
        return;
    }
    let relay = gateway.attach.clone();
    let (client, mut frames) = match relay.open(&limits, &station, &target, &person.id, after) {
        Ok(opened) => opened,
        Err(Refused::Busy | Refused::Gone) => {
            refuse(
                &mut sender,
                "busy",
                "The Station has too many requests waiting.",
            )
            .await;
            return;
        }
        Err(Refused::Full) => {
            refuse(
                &mut sender,
                "full",
                "The Run has all the people attached it allows.",
            )
            .await;
            return;
        }
    };
    if sender
        .send(frame(
            &json!({"type": "attached", "client": client, "target": target}),
        ))
        .await
        .is_err()
    {
        relay.close(&client, &person.id);
        return;
    }
    let widest = frame_limit(&limits);
    let mut check = next_check(&gateway, &token).await;
    let mut person = person;
    loop {
        tokio::select! {
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let answer = if text.len() > widest {
                        Err(("bad_request", "That frame is larger than a prompt may be.".to_owned()))
                    } else {
                        request(&gateway, &subject, &token, &client, &text).await
                    };
                    match answer {
                        Ok(Some(acting)) => person = acting,
                        Ok(None) => break,
                        Err((code, message)) => {
                            let ended = matches!(code, "unauthorized");
                            if sender.send(frame(&problem(code, &message))).await.is_err() {
                                break;
                            }
                            if ended {
                                let _ = sender.send(close(1008, "sign-in ended")).await;
                                break;
                            }
                        }
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    if sender.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Binary(_))) => {
                    let _ = sender.send(close(1003, "text frames only")).await;
                    break;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
            },
            shown = frames.next() => match shown {
                Some(shown) => {
                    if sender.send(frame(&json!({"type": "frame", "frame": shown}))).await.is_err() {
                        break;
                    }
                }
                // The Station closed the Run, or this person fell too far behind.
                None => {
                    let _ = sender.send(close(1013, "the attachment ended; attach again")).await;
                    break;
                }
            },
            () = tokio::time::sleep_until(check) => {
                // A person whose sign-in ended or role was withdrawn stops receiving.
                match still_holds(&gateway, &subject, &token).await {
                    Ok(holder) => {
                        person = holder;
                        check = next_check(&gateway, &token).await;
                    }
                    Err((code, message)) => {
                        let _ = sender.send(frame(&problem(code, &message))).await;
                        let _ = sender.send(close(1008, "no longer allowed")).await;
                        break;
                    }
                }
            },
        }
    }
    relay.close(&client, &person.id);
    let _ = subject.audit(&gateway, &person, "detach").await;
}

/// Applies one frame of an attached person, as who the token is now. `Ok(None)` ends the
/// attachment; `Ok(Some(person))` goes on.
async fn request(
    gateway: &Gateway,
    subject: &Subject,
    token: &str,
    client: &str,
    text: &str,
) -> Result<Option<Person>, (&'static str, String)> {
    let limits = &gateway.config.unex().attach;
    let wrong = || ("bad_request", "That frame is not understood.".to_owned());
    let (what, action, op) = match serde_json::from_str::<FromClient>(text).map_err(|_| wrong())? {
        FromClient::Detach {} => return Ok(None),
        FromClient::Hello { .. } => return Err(wrong()),
        FromClient::Prompt { text } => {
            if text.trim().is_empty() || !limits.prompt_bytes.allows(text.len()) {
                return Err((
                    "bad_request",
                    "A prompt is not blank and fits the limit.".to_owned(),
                ));
            }
            ("prompt", subject.converse, HubOp::Prompt { text })
        }
        FromClient::Answer { request, option } => (
            "answer",
            subject.converse,
            HubOp::Answer { request, option },
        ),
        FromClient::Interrupt {} => ("interrupt", subject.control, HubOp::Interrupt),
    };
    // Resolved again for every act: a sign-in ended or revoked, or a role withdrawn since
    // attaching, applies at once.
    let person = still_holds(gateway, subject, token).await?;
    if action != subject.converse && !subject.allows(gateway, &person, action).await {
        return Err(("forbidden", format!("You may not {what} that Run.")));
    }
    subject
        .audit(gateway, &person, what)
        .await
        .map_err(|_| ("unavailable", "That could not be recorded.".to_owned()))?;
    gateway
        .attach
        .submit(limits, client, &person.id, op)
        .map_err(|refused| match refused {
            Refused::Gone => (
                "gone",
                "You are no longer attached to that Run; attach again.".to_owned(),
            ),
            Refused::Busy | Refused::Full => (
                "busy",
                "The Station has too many requests waiting.".to_owned(),
            ),
        })?;
    Ok(Some(person))
}

#[derive(Deserialize)]
pub(crate) struct NextQuery {
    station: String,
    wait_ms: Option<u64>,
}

/// `GET /v1/station/hub/next?station=ID&wait_ms=N`: what attached people asked of the Runs the
/// Station keeps (`200` and a JSON array of [`HubCommand`]), or `204` after `wait_ms` (default
/// `[attach] poll_wait`, at most `[attach] max_wait`).
pub(crate) async fn next(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<NextQuery>,
) -> Result<Response, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    if !ai6::directorates::section_00::assistant::valid_identity(&query.station) {
        return Err(ApiError::bad_request());
    }
    let limits = &gateway.config.unex().attach;
    let wait = query
        .wait_ms
        .map_or(limits.poll_wait.0, Duration::from_millis)
        .min(limits.max_wait.0);
    let deadline = tokio::time::Instant::now() + wait;
    let relay = gateway.attach.clone();
    loop {
        let notified = relay.ready.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let commands = relay.take(&query.station);
        if !commands.is_empty() {
            return Ok(Json(commands).into_response());
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
    }
}

/// `POST /v1/station/hub/frames`: frames a Station's Run shows, for the people attached to it
/// (`204`), or `404` when the client they are for is not attached.
pub(crate) async fn frames(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(delivery): Json<HubDelivery>,
) -> Result<StatusCode, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    if gateway
        .attach
        .deliver(gateway.config.unex().attach.backlog, delivery)
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> RunTarget {
        RunTarget::Charge {
            scope: "operation:G".to_owned(),
        }
    }

    #[test]
    fn a_request_for_a_client_already_let_go_is_gone_and_not_busy() {
        let relay = Relay::default();
        let refused = relay.submit(
            &UnexAttachLimits::default(),
            "nobody",
            "p",
            HubOp::Interrupt,
        );
        assert!(matches!(refused, Err(Refused::Gone)));
    }

    #[test]
    fn a_person_further_behind_than_the_backlog_is_let_go() {
        let relay = Relay::default();
        let limits = UnexAttachLimits::default();
        let (client, mut frames) = relay
            .open(&limits, "worker", &target(), "p", None)
            .ok()
            .unwrap();
        let frame = |text: &str| HubFrame::new("event", 1.0, json!({ "text": text }));
        let delivery = HubDelivery::new("worker", target(), vec![frame("a"), frame("b")]);
        // Room for one frame: the second finds the person behind, and they are let go.
        assert!(relay.deliver(Limit::at(1), delivery));
        assert!(matches!(
            relay.submit(&limits, &client, "p", HubOp::Interrupt),
            Err(Refused::Gone)
        ));
        let first = frames.frames.try_recv().unwrap();
        assert_eq!(first.data["text"], "a");
        assert!(frames.frames.try_recv().is_err());
    }
}
