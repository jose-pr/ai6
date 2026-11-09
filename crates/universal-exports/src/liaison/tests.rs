//! Liaison over real proven links and the gateway's HTTP binding, with stand-in Agents that read
//! and answer their Cables through the relay as a Station does.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::cables::{
    CableConfig, CableDelivery, CableEndpoint, CablePrincipal, CableRelay, CableRelayError,
    CableScope, CableSendRequest, CableState, CableTarget,
};
use ai6::directorates::registry::store::FileRegistry;
use ai6::guatiao::library::ProviderError;
use ai6::{
    Attempt, Authenticator, Directory, Lookup, MemoryProvider, NewSession, Operation, Person,
    Roster, SessionStatus, SessionUpdate, Verdict, access_from_toml,
};
use coqui_a2a::error::{A2aError, ErrorKind};
use coqui_a2a::model::{
    AgentCard, CancelTaskRequest, GetTaskRequest, ListTasksRequest, Message, Role,
    SendMessageConfiguration, SendMessageRequest, SendMessageResponse, StreamResponse, TaskState,
};
use coqui_a2a::{Agent, Caller, Client, ClientError, ClientTimeouts, PROTOCOL, Server};
use coqui_agentic_peer::auth::{self, Identity, Method, Proof, Sealed, Trust, prove_out};
use coqui_agentic_peer::link::{self, Host, Options, Services};
use coqui_agentic_peer::mux::{Limits, Mux, Side};
use coqui_agentic_peer::peers::{MemoryPeers, NoEvents, PeerRecord, PeerStore};
use coqui_web_channel::websocket::{Channel as WsChannel, Dialer, Frames, Listener};
use coqui_web_channel::{Channel as _, Dialer as _, Kind};
use serde_json::{Value, json};

use super::{Addressee, Liaison, LiaisonConfig, LiaisonPeer, LinkListener};
use crate::{AccessProviders, Gateway, GatewayConfig, GatewayLimits};

const WAIT: Duration = Duration::from_secs(10);
const TOKEN: &str = "m-token-that-is-long-enough-for-testing";
const STATION: &str = "worker";

/// Acme's principal may send and read Cables in G; Rival's and Someone's hold nothing.
const ROLES: &str = r#"
    [role.liaison]
    actions = ["cable.send", "cable.read"]

    [[rule]]
    subject = "person:acme-bot"
    scope = "operation:G"
    role = "liaison"
"#;

/// The Cable grants a peer's message and its reply need, both ways, for Agent 007 only; 009 is
/// an addressee Acme is configured for but its Agent holds no grant.
const CABLES: &str = r#"
    [[grants]]
    agent = "a2a:acme"
    action = "send"
    scope = "agent"
    name = "007"

    [[grants]]
    agent = "007"
    action = "receive"
    scope = "agent"
    name = "a2a:acme"

    [[grants]]
    agent = "007"
    action = "send"
    scope = "agent"
    name = "a2a:acme"

    [[grants]]
    agent = "a2a:acme"
    action = "receive"
    scope = "agent"
    name = "007"

    [[grants]]
    agent = "a2a:acme"
    action = "send"
    scope = "agent"
    name = "009"

    [[grants]]
    agent = "007"
    action = "send"
    scope = "agent"
    name = "a2a:echo"

    [[grants]]
    agent = "a2a:echo"
    action = "receive"
    scope = "agent"
    name = "007"

    [[grants]]
    agent = "a2a:echo"
    action = "send"
    scope = "agent"
    name = "007"

    [[grants]]
    agent = "007"
    action = "receive"
    scope = "agent"
    name = "a2a:echo"
"#;

struct People;

impl Authenticator for People {
    fn authenticate(&self, _attempt: Attempt) -> Result<Verdict, ProviderError> {
        Ok(Verdict::Invalid)
    }
}

impl Directory for People {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        Ok(["acme-bot", "someone"]
            .contains(&lookup.value.as_str())
            .then(|| Person {
                id: lookup.value.clone(),
                name: lookup.value,
                ..Person::default()
            }))
    }
}

/// A gateway with Liaison, its link listener, and the peers' keys.
struct World {
    directory: tempfile::TempDir,
    registry: FileRegistry,
    gateway: Gateway,
    listener: LinkListener,
    acme: Identity,
    mute: Identity,
    rival: Identity,
    stranger: Identity,
}

fn identity(dir: &Path, name: &str) -> Identity {
    Identity::open_or_create(&dir.join(name), Method::DEFAULT).unwrap()
}

fn cable_config() -> CableConfig {
    CableConfig::parse_toml(CABLES).unwrap()
}

impl World {
    fn new() -> Self {
        Self::with_peers(Vec::new())
    }

    fn with_peers(extra: Vec<LiaisonPeer>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(directory.path().join("registry"));
        registry
            .check_in(
                STATION,
                BTreeMap::from([("operations".to_owned(), json!(["G"]))]),
            )
            .unwrap();
        let mut operation = Operation::new("G");
        operation.roster = vec!["007".to_owned(), "008".to_owned(), "009".to_owned()];
        let mut coordinator = ai6::Agent::new("007");
        coordinator.roles = vec!["coordinator".to_owned()];
        let roster = Roster::builder()
            .operation(operation)
            .agent(coordinator)
            .agent(ai6::Agent::new("008"))
            .agent(ai6::Agent::new("009"))
            .build()
            .unwrap();
        let mut settings = ai6::Settings::default();
        settings.cables = cable_config();
        let provider: Arc<dyn ai6::ConfigProvider> =
            Arc::new(MemoryProvider::new(roster).with_settings(settings));
        let keys = directory.path().join("keys");
        let (acme, mute, rival, stranger) = (
            identity(&keys, "acme"),
            identity(&keys, "mute"),
            identity(&keys, "rival"),
            identity(&keys, "stranger"),
        );
        let peer = |name: &str, key: &Identity, principal: &str, a2a: bool| LiaisonPeer {
            name: name.to_owned(),
            id: Some(key.peer_id().to_string()),
            principal: principal.to_owned(),
            addressees: ["agent:G/007", "role:G/coordinator", "agent:G/009"]
                .iter()
                .map(|addressee| addressee.parse::<Addressee>().unwrap())
                .collect(),
            a2a,
            url: None,
        };
        let mut config = LiaisonConfig::default()
            .with_peer(peer("acme", &acme, "person:acme-bot", true))
            .with_peer(peer("mute", &mute, "person:nobody-bot", false))
            .with_peer(peer("rival", &rival, "person:rival-bot", true));
        for extra in extra {
            config = config.with_peer(extra);
        }
        config.poll = Duration::from_millis(50);
        config.recheck = Duration::from_millis(100);
        let gateway = Gateway::with_access(
            GatewayConfig::new(
                "https://localhost",
                TOKEN,
                GatewayLimits::default(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
            Arc::new(registry.clone()),
            AccessProviders::new(
                Arc::new(People),
                Arc::new(People),
                Arc::new(access_from_toml(ROLES, ai6::Limit::UNBOUNDED).unwrap()),
            ),
        )
        .with_roster(provider)
        .with_liaison(config);
        let listener = LinkListener::start(
            &gateway.liaison().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            None,
            &directory.path().join("gateway-link-key"),
        )
        .unwrap();
        Self {
            directory,
            registry,
            gateway,
            listener,
            acme,
            mute,
            rival,
            stranger,
        }
    }

    fn registry_dir(&self) -> PathBuf {
        self.directory.path().join("registry")
    }

    fn url(&self) -> url::Url {
        format!("ws://{}/link", self.listener.local_addr())
            .parse()
            .unwrap()
    }

    /// Dials the gateway, proves `client`, and runs `body` over the link.
    fn over_link<R>(
        &self,
        client: &Identity,
        body: impl FnOnce(&Mux<'_, Sealed<'_, WsChannel>>) -> R,
    ) -> Result<R, String> {
        over_link(&self.url(), self.listener.peer_id(), client, body)
    }

    /// Whatever the Registry holds of briefs, Missions or leases: none of it, ever, here.
    fn work_left(&self) -> Vec<String> {
        let mut found = Vec::new();
        for kind in [
            "new",
            "claimed",
            "leases",
            "mission-ids",
            "attempts",
            "entries",
            "pending",
            "approved",
            "proposals",
            "issued",
            "cancels",
        ] {
            if let Ok(entries) = std::fs::read_dir(self.registry_dir().join(kind)) {
                found.extend(
                    entries
                        .flatten()
                        .map(|entry| entry.path().display().to_string()),
                );
            }
        }
        found
    }

    /// Opens a Registry record for provider Session `session` of Agent 007, as its Station would.
    fn open_session(&self, session: &str) -> (String, u64) {
        let record = self
            .registry
            .open_session(
                NewSession::new(
                    format!("record-{session}"),
                    format!("mission-{session}"),
                    STATION,
                )
                .with_operation("G")
                .with_provider_session(session),
                Duration::from_secs(600),
            )
            .unwrap();
        (record.session_id, record.fence)
    }

    fn end_session(&self, (record, fence): &(String, u64)) {
        self.registry
            .update_session(
                record,
                STATION,
                *fence,
                SessionUpdate::default().status(SessionStatus::Ended),
            )
            .unwrap();
    }
}

fn over_link<R>(
    url: &url::Url,
    server: impl Into<String>,
    client: &Identity,
    body: impl FnOnce(&Mux<'_, Sealed<'_, WsChannel>>) -> R,
) -> Result<R, String> {
    let server: String = server.into();
    let proof =
        Proof::new(client.clone()).with_responders(Trust::pinned([server.parse().unwrap()]));
    let channel = Dialer::new()
        .with_frames(Frames::Binary)
        .dial(Kind::WebSocket, url, WAIT)
        .map_err(|error| error.to_string())?;
    let established = prove_out(
        &proof,
        &channel,
        WAIT,
        proof.method().record_length(Limits::default().max_message),
    )
    .map_err(|error| error.to_string())?;
    let sealed = Sealed::new(&channel, established.session);
    let result = Mux::run(&sealed, Side::Dialed, Limits::default(), None, body);
    sealed.close();
    channel.close();
    Ok(result)
}

fn station_relay(registry: FileRegistry, session: &str) -> (CableRelay, CablePrincipal) {
    let endpoint = CableEndpoint::try_new(
        "007".to_owned(),
        "G".to_owned(),
        None,
        STATION.to_owned(),
        Some(session.to_owned()),
        Some(format!("mission-{session}")),
    )
    .unwrap();
    let relay = CableRelay::new(
        Arc::new(registry),
        cable_config().policy().unwrap(),
        move || vec![endpoint.clone()],
        BTreeMap::new(),
    );
    let principal = CablePrincipal::try_new(
        "007".to_owned(),
        "G".to_owned(),
        None,
        STATION.to_owned(),
        Some(session.to_owned()),
        Some(format!("mission-{session}")),
    )
    .unwrap();
    (relay, principal)
}

/// Agent 007 working in one provider Session on Station `worker`: it reads its Cables, and
/// answers each `re: <body>` when told to.
struct StandIn {
    stop: Arc<AtomicBool>,
    read: Arc<Mutex<Vec<CableDelivery>>>,
    thread: Option<JoinHandle<()>>,
}

impl StandIn {
    fn start(registry: FileRegistry, session: &str, replies: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let read: Arc<Mutex<Vec<CableDelivery>>> = Arc::default();
        let (flag, seen) = (stop.clone(), read.clone());
        let session = session.to_owned();
        let thread = thread::spawn(move || {
            let (relay, principal) = station_relay(registry, &session);
            while !flag.load(Ordering::SeqCst) {
                for delivery in relay.inbox(&principal, 50).unwrap() {
                    let mut seen = seen.lock().unwrap();
                    if seen.iter().any(|known| known.cable_id == delivery.cable_id) {
                        continue;
                    }
                    if replies {
                        relay
                            .reply(
                                &principal,
                                &delivery.cable_id,
                                format!("re: {}", delivery.body),
                                format!("reply-{}", delivery.cable_id),
                            )
                            .unwrap();
                    }
                    seen.push(delivery);
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        Self {
            stop,
            read,
            thread: Some(thread),
        }
    }

    fn read(&self) -> Vec<CableDelivery> {
        self.read.lock().unwrap().clone()
    }

    fn bodies(&self) -> Vec<String> {
        self.read()
            .into_iter()
            .map(|delivery| delivery.body)
            .collect()
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn client<'m, 'c, C: coqui_web_channel::Channel>(mux: &'m Mux<'c, C>) -> Client<'m, 'c, C> {
    let mut timeouts = ClientTimeouts::default();
    timeouts.answer = Duration::from_secs(30);
    Client::new(mux).with_timeouts(timeouts)
}

fn to(addressee: Option<&str>, id: &str, text: &str) -> SendMessageRequest {
    let mut message = Message::text(Role::User, id, text);
    if let Some(addressee) = addressee {
        let mut metadata = serde_json::Map::new();
        metadata.insert("addressee".to_owned(), Value::String(addressee.to_owned()));
        message.metadata = Some(metadata);
    }
    SendMessageRequest::new(message)
}

fn in_context(context: &str, id: &str, text: &str) -> SendMessageRequest {
    let mut message = Message::text(Role::User, id, text);
    message.context_id = Some(context.to_owned());
    SendMessageRequest::new(message)
}

fn at_once(mut request: SendMessageRequest) -> SendMessageRequest {
    request.configuration = Some(SendMessageConfiguration {
        return_immediately: Some(true),
        ..SendMessageConfiguration::default()
    });
    request
}

fn refusal(result: Result<impl std::fmt::Debug, ClientError>) -> ErrorKind {
    match result {
        Err(ClientError::Remote(error)) => error.kind().unwrap(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn reply_of(response: SendMessageResponse) -> Message {
    match response {
        SendMessageResponse::Message(reply) => reply,
        other => panic!("a blocking send that got its reply answers with it: {other:?}"),
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_peer_s_message_reaches_the_agent_as_a_cable_and_its_reply_comes_back_with_no_mission() {
    let world = World::new();
    let standin = StandIn::start(world.registry.clone(), "session-7", true);
    let (card, reply, task) = world
        .over_link(&world.acme, |mux| {
            let client = client(mux);
            let card = client.agent_card().unwrap();
            let reply = reply_of(
                client
                    .send_message(&to(Some("agent:G/007"), "m-1", "status of the build?"))
                    .unwrap(),
            );
            let task = client
                .get_task(&GetTaskRequest {
                    id: reply.task_id.clone().unwrap(),
                    ..GetTaskRequest::default()
                })
                .unwrap();
            (card, reply, task)
        })
        .unwrap();
    assert_eq!(card.name, "AI6 Liaison");
    let skills: Vec<_> = card.skills.iter().map(|skill| skill.id.as_str()).collect();
    assert_eq!(skills, ["agent:G/007", "role:G/coordinator", "agent:G/009"]);
    assert_eq!(reply.joined_text(), "re: status of the build?");
    assert_eq!(reply.role, Role::Agent);
    assert_eq!(task.status.state, TaskState::Completed);
    assert_eq!(task.history.as_ref().unwrap().len(), 2);

    // The Task is the peer's Cable: its ID is the Cable's, its context the conversation.
    let cable = world.registry.get_cable(&task.id).unwrap().unwrap();
    assert_eq!(cable.sender_agent(), "a2a:acme");
    assert_eq!(cable.sender_station(), super::LIAISON_STATION);
    assert_eq!(cable.on_behalf_of(), Some("person:acme-bot"));
    assert_eq!(
        cable.peer(),
        Some(world.acme.peer_id().to_string().as_str())
    );
    assert_eq!(cable.conversation_id(), task.context_id.as_deref());
    assert_eq!(cable.mission_id(), None, "a peer's Cable names no Mission");
    assert_ne!(
        cable.session_id(),
        Some(task.id.as_str()),
        "the Task ID is no Session ID"
    );
    let read = standin.read();
    assert_eq!(read.len(), 1);
    assert_eq!(
        read[0].sent_as.as_deref(),
        Some("a2a:acme for person:acme-bot")
    );
    assert_eq!(
        cable.recipients()[0].taken_by(),
        Some("session-7"),
        "the Session that took it holds the thread"
    );

    assert!(world.registry.list_new().unwrap().is_empty(), "no brief");
    assert_eq!(
        world.work_left(),
        Vec::<String>::new(),
        "no brief, Mission or lease"
    );
    let kept: Vec<_> = std::fs::read_dir(world.directory.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        kept.iter()
            .all(|name| ["registry", "keys", "gateway-link-key"].contains(&name.as_str())),
        "Liaison keeps nothing of its own: {kept:?}"
    );

    let audit = world.registry.audit_records().unwrap();
    let actions: Vec<_> = audit.iter().map(|record| record.action.as_str()).collect();
    for wanted in ["cable.send", "liaison.send_message", "liaison.get_task"] {
        assert!(actions.contains(&wanted), "{actions:?}");
    }
    for record in &audit {
        assert!(
            !record.target.contains("build"),
            "audit holds no body: {record:?}"
        );
    }
}

#[test]
fn a_role_reaches_the_agent_the_roster_selects_and_a_stream_follows_it_to_the_reply() {
    let world = World::new();
    let standin = StandIn::start(world.registry.clone(), "session-7", true);
    let items = world
        .over_link(&world.acme, |mux| {
            client(mux)
                .send_streaming_message(&to(Some("role:G/coordinator"), "m-2", "who coordinates?"))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        })
        .unwrap();
    // The first Task says where the cable is when the stream opens: the stand-in may have had it,
    // or answered it, by then.
    assert!(
        matches!(&items[0], StreamResponse::Task(task) if matches!(task.status.state,
            TaskState::Submitted | TaskState::Working | TaskState::Completed)),
        "the stream opens on its Task: {items:?}"
    );
    assert!(items.iter().any(|item| matches!(item,
        StreamResponse::ArtifactUpdate(update) if update.artifact.parts[0].text.as_deref() == Some("re: who coordinates?"))));
    let StreamResponse::StatusUpdate(last) = items.last().unwrap() else {
        panic!("the stream ends on its state: {items:?}");
    };
    assert_eq!(last.status.state, TaskState::Completed);
    assert_eq!(standin.bodies(), ["who coordinates?"]);
}

#[test]
fn an_undelivered_message_is_withdrawn_and_a_delivered_one_cannot_be() {
    let world = World::new();
    let canceled = world
        .over_link(&world.acme, |mux| {
            let client = client(mux);
            let SendMessageResponse::Task(task) = client
                .send_message(&at_once(to(Some("agent:G/007"), "m-3", "never mind")))
                .unwrap()
            else {
                panic!("returnImmediately answers with the task");
            };
            assert_eq!(task.status.state, TaskState::Submitted);
            client
                .cancel_task(&CancelTaskRequest {
                    id: task.id.clone(),
                    ..CancelTaskRequest::default()
                })
                .unwrap()
        })
        .unwrap();
    assert_eq!(canceled.status.state, TaskState::Canceled);
    let cable = world.registry.get_cable(&canceled.id).unwrap().unwrap();
    assert_eq!(cable.recipients()[0].state(), CableState::Withdrawn);
    let late = StandIn::start(world.registry.clone(), "session-7", true);
    thread::sleep(Duration::from_millis(300));
    assert!(late.read().is_empty(), "a withdrawn Cable reaches nobody");

    let refused = world
        .over_link(&world.acme, |mux| {
            let client = client(mux);
            let reply = reply_of(
                client
                    .send_message(&to(Some("agent:G/007"), "m-4", "ping"))
                    .unwrap(),
            );
            refusal(client.cancel_task(&CancelTaskRequest {
                id: reply.task_id.unwrap(),
                ..CancelTaskRequest::default()
            }))
        })
        .unwrap();
    assert_eq!(refused, ErrorKind::TaskNotCancelable);
}

#[test]
fn an_unpaired_key_an_ungranted_peer_and_a_principal_without_cable_send_are_refused() {
    let world = World::new();
    let standin = StandIn::start(world.registry.clone(), "session-7", true);
    // Its own half of the proof may finish before the gateway refuses it; nothing works after.
    let stranger = world.over_link(&world.stranger, |mux| client(mux).agent_card());
    assert!(
        !matches!(stranger, Ok(Ok(_))),
        "a key nobody paired reaches nothing: {stranger:?}"
    );
    let not_granted = world
        .over_link(&world.mute, |mux| {
            client(mux).send_message(&to(Some("agent:G/007"), "m-5", "hello"))
        })
        .unwrap();
    assert!(
        matches!(not_granted, Err(ClientError::NotGranted(_))),
        "{not_granted:?}"
    );

    let (card, refused) = world
        .over_link(&world.rival, |mux| {
            let client = client(mux);
            (
                client.agent_card().unwrap(),
                refusal(client.send_message(&to(Some("agent:G/007"), "m-6", "hello"))),
            )
        })
        .unwrap();
    assert!(
        card.skills.is_empty(),
        "a principal without cable.send sees no addressee"
    );
    assert_eq!(refused, ErrorKind::PermissionDenied);

    let (unlisted, ungranted) = world
        .over_link(&world.acme, |mux| {
            let client = client(mux);
            (
                refusal(client.send_message(&to(Some("agent:G/008"), "m-7", "hello"))),
                refusal(client.send_message(&to(Some("agent:G/009"), "m-8", "hello"))),
            )
        })
        .unwrap();
    assert_eq!(
        unlisted,
        ErrorKind::PermissionDenied,
        "not among its addressees"
    );
    assert_eq!(
        ungranted,
        ErrorKind::PermissionDenied,
        "009 holds no receive grant"
    );
    thread::sleep(Duration::from_millis(200));
    assert!(standin.read().is_empty(), "nothing refused was sent");
    let refusals = world
        .registry
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|record| {
            record.action == "liaison.send_message" && record.target.ends_with("refused")
        })
        .count();
    assert_eq!(refusals, 3);
}

#[test]
fn a_peer_sees_only_its_own_tasks_and_one_message_id_is_one_cable() {
    let world = World::new();
    let task = world
        .over_link(&world.acme, |mux| {
            let client = client(mux);
            let request = at_once(to(Some("agent:G/007"), "m-9", "mine"));
            let (SendMessageResponse::Task(first), SendMessageResponse::Task(second)) = (
                client.send_message(&request).unwrap(),
                client.send_message(&request).unwrap(),
            ) else {
                panic!("tasks");
            };
            assert_eq!(first.id, second.id, "a resent message ID is the same Cable");
            let listed = client.list_tasks(&ListTasksRequest::default()).unwrap();
            assert_eq!(listed.tasks.len(), 1);
            assert_eq!(listed.next_page_token, "");
            first.id
        })
        .unwrap();
    let cables = std::fs::read_dir(world.registry_dir().join("cables"))
        .unwrap()
        .count();
    assert_eq!(cables, 1);
    let (listed, read) = world
        .over_link(&world.rival, |mux| {
            let client = client(mux);
            (
                client.list_tasks(&ListTasksRequest::default()).unwrap(),
                refusal(client.get_task(&GetTaskRequest {
                    id: task.clone(),
                    ..GetTaskRequest::default()
                })),
            )
        })
        .unwrap();
    assert!(listed.tasks.is_empty());
    assert_eq!(listed.total_size, 0);
    assert_eq!(
        read,
        ErrorKind::TaskNotFound,
        "another peer's task is not found"
    );
}

#[test]
fn a_follow_up_in_a_context_reaches_the_session_that_took_it_then_the_agent_s_next_session() {
    let world = World::new();
    let first = world.open_session("session-a");
    let a = StandIn::start(world.registry.clone(), "session-a", true);
    let context = world
        .over_link(&world.acme, |mux| {
            reply_of(
                client(mux)
                    .send_message(&to(Some("agent:G/007"), "f-1", "one"))
                    .unwrap(),
            )
            .context_id
            .unwrap()
        })
        .unwrap();
    world.open_session("session-b");
    let b = StandIn::start(world.registry.clone(), "session-b", true);
    let second = world
        .over_link(&world.acme, |mux| {
            reply_of(
                client(mux)
                    .send_message(&in_context(&context, "f-2", "two"))
                    .unwrap(),
            )
        })
        .unwrap();
    assert_eq!(second.context_id.as_deref(), Some(context.as_str()));
    assert_eq!(
        a.bodies(),
        ["one", "two"],
        "the follow-up goes to the Session of the thread"
    );
    assert!(
        b.bodies().is_empty(),
        "never to another open Session of the Agent"
    );

    // Session a ends; b was open before it did, c opens after: the thread goes to c.
    drop(a);
    world.end_session(&first);
    thread::sleep(Duration::from_millis(20));
    world.open_session("session-c");
    let c = StandIn::start(world.registry.clone(), "session-c", true);
    let third = world
        .over_link(&world.acme, |mux| {
            reply_of(
                client(mux)
                    .send_message(&in_context(&context, "f-3", "three"))
                    .unwrap(),
            )
        })
        .unwrap();
    assert_eq!(third.joined_text(), "re: three");
    assert_eq!(c.bodies(), ["three"]);
    assert!(
        b.bodies().is_empty(),
        "a Session open when the thread's ended is not its next"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_http_binding_carries_the_same_exchange_with_a_bearer_and_refuses_without_one() {
    let world = Arc::new(World::new());
    let _standin = StandIn::start(world.registry.clone(), "session-7", true);
    let token = |person: &str| {
        let registry = Arc::new(world.registry.clone());
        let person = person.to_owned();
        async move {
            crate::issue_access_token_record(
                registry,
                person,
                Duration::from_secs(600),
                ai6::TimeLimit::UNBOUNDED,
            )
            .await
            .ok()
            .unwrap()
            .token
        }
    };
    let (acme, someone) = (token("acme-bot").await, token("someone").await);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1/a2a", listener.local_addr().unwrap());
    let router = world.gateway.clone().router();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    let http = reqwest::Client::new();
    let send = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "SendMessage",
        "params": {"message": {
            "messageId": "http-1",
            "role": "ROLE_USER",
            "parts": [{"text": "over http"}],
            "metadata": {"addressee": "agent:G/007"}
        }}
    });
    let unsigned = http.post(&base).json(&send).send().await.unwrap();
    assert_eq!(unsigned.status(), reqwest::StatusCode::UNAUTHORIZED);
    let stranger = http
        .post(&base)
        .bearer_auth(&someone)
        .json(&send)
        .send()
        .await
        .unwrap();
    assert_eq!(
        stranger.status(),
        reqwest::StatusCode::FORBIDDEN,
        "no peer's principal"
    );

    let card: AgentCard = http
        .get(format!("{base}/.well-known/agent-card.json"))
        .bearer_auth(&acme)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(card.supported_interfaces[0].url, "https://localhost/v1/a2a");
    assert_eq!(card.supported_interfaces[0].protocol_binding, "JSONRPC");
    assert_eq!(card.skills[0].id, "agent:G/007");
    let answer: Value = http
        .post(&base)
        .bearer_auth(&acme)
        .json(&send)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(answer["id"], 7);
    assert_eq!(
        answer["result"]["message"]["parts"][0]["text"], "re: over http",
        "{answer}"
    );
    let task = answer["result"]["message"]["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let cable = world.registry.get_cable(&task).unwrap().unwrap();
    assert_eq!(
        cable.sender_agent(),
        "a2a:acme",
        "the same peer as on its link"
    );
    assert_eq!(cable.peer(), Some("signed-in:person:acme-bot"));

    let streamed = http
        .post(&base)
        .bearer_auth(&acme)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "SendStreamingMessage",
            "params": {"message": {
                "messageId": "http-2",
                "role": "ROLE_USER",
                "parts": [{"text": "streamed"}],
                "metadata": {"addressee": "agent:G/007"}
            }}
        }))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(streamed.contains("re: streamed"), "{streamed}");
    assert!(streamed.contains("TASK_STATE_COMPLETED"), "{streamed}");
    assert_eq!(world.work_left(), Vec::<String>::new());
}

/// Unmodified A2A software reaches Liaison through coqui-a2a's loopback HTTP endpoint on a link.
#[tokio::test(flavor = "multi_thread")]
async fn the_coqui_a2a_loopback_endpoint_carries_a_message_and_its_reply() {
    use coqui_a2a::loopback::{Config, Hub, Loopback};

    let world = Arc::new(World::new());
    let _standin = StandIn::start(world.registry.clone(), "session-7", true);
    let hub = Hub::new();
    let token = "loopback-token-for-the-test";
    let endpoint = Loopback::start(
        Config::new("127.0.0.1:0".parse().unwrap())
            .unwrap()
            .with_token(token),
        hub.clone(),
    )
    .unwrap();
    let (linked, attached) = (world.clone(), hub.clone());
    let link = thread::spawn(move || linked.over_link(&linked.acme, |mux| attached.attach(mux)));
    let until = Instant::now() + WAIT;
    while !hub.is_up() {
        assert!(Instant::now() < until, "the link came up");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let base = format!("http://{}/", endpoint.local_addr());
    let http = reqwest::Client::new();
    let card: Value = http
        .get(format!("{base}.well-known/agent-card.json"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(card["name"], "AI6 Liaison");
    let answer: Value = http
        .post(&base)
        .bearer_auth(token)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "SendMessage",
            "params": {"message": {
                "messageId": "loop-1",
                "role": "ROLE_USER",
                "parts": [{"text": "over the loopback"}],
                "metadata": {"addressee": "agent:G/007"}
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        answer["result"]["message"]["parts"][0]["text"], "re: over the loopback",
        "{answer}"
    );
    hub.close();
    link.join().unwrap().unwrap();
    drop(endpoint);
}

/// An A2A agent outside AI6, on a link of its own: it answers `echo: <text>`, and keeps what it
/// was sent.
#[derive(Clone, Default)]
struct Echo {
    got: Arc<Mutex<Vec<Message>>>,
}

impl Agent for Echo {
    fn agent_card(&self, _caller: &Caller<'_>) -> AgentCard {
        AgentCard {
            name: "Echo".to_owned(),
            ..AgentCard::default()
        }
    }

    fn send_message(
        &self,
        _caller: &Caller<'_>,
        request: SendMessageRequest,
    ) -> Result<SendMessageResponse, A2aError> {
        let text = request.message.joined_text();
        self.got.lock().unwrap().push(request.message);
        Ok(SendMessageResponse::Message(Message::text(
            Role::Agent,
            "echo-reply",
            format!("echo: {text}"),
        )))
    }
}

/// Serves `agent` on a loopback link to the one peer `client` names.
fn serve_peer(
    identity: Identity,
    client: &str,
    agent: Echo,
    stop: Arc<AtomicBool>,
) -> (SocketAddr, JoinHandle<()>) {
    let listener = Listener::bind("127.0.0.1:0".parse().unwrap(), None)
        .unwrap()
        .with_limits(coqui_web_channel::Limits::default().with_max_message(auth::MAX_PROOF_MESSAGE))
        .with_frames(Frames::Binary);
    let address = listener.local_addr().unwrap();
    let store = MemoryPeers::default();
    store
        .put(&PeerRecord::new(client).granting(PROTOCOL))
        .unwrap();
    let proof = Proof::new(identity).with_initiators(Trust::pinned([client.parse().unwrap()]));
    let services = Server::new(agent).serving(Services::default());
    let thread = thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let Ok(pending) = listener.next(Some(Duration::from_millis(50))) else {
                continue;
            };
            let Ok(channel) = pending.handshake() else {
                continue;
            };
            let options = Options::default();
            if let Ok(established) = auth::prove(
                &proof,
                &channel,
                WAIT,
                proof.method().record_length(options.limits.max_message),
            ) {
                let peer = established.peer.to_string();
                let sealed = Sealed::new(&channel, established.session);
                let host = Host {
                    store: &store,
                    services: &services,
                    events: &NoEvents,
                };
                link::serve(&sealed, Side::Answered, options, None, &peer, &host);
                sealed.close();
            }
            channel.close();
        }
    });
    (address, thread)
}

#[test]
fn a_cable_to_a_peer_leaves_as_a2a_and_its_answer_comes_back_as_a_reply_cable() {
    let keys = tempfile::tempdir().unwrap();
    let echo_key = identity(keys.path(), "echo");
    // The gateway's link key is made where World puts it; the peer pins it once it exists.
    let gateway_key_dir = tempfile::tempdir().unwrap();
    let gateway_key = identity(gateway_key_dir.path(), "gateway");
    let stop = Arc::new(AtomicBool::new(false));
    let echo = Echo::default();
    let (address, peer_thread) = serve_peer(
        echo_key.clone(),
        &gateway_key.peer_id().to_string(),
        echo.clone(),
        stop.clone(),
    );
    let world = World::with_peers(vec![LiaisonPeer {
        name: "echo".to_owned(),
        id: Some(echo_key.peer_id().to_string()),
        principal: "person:acme-bot".to_owned(),
        addressees: Vec::new(),
        a2a: true,
        url: Some(format!("ws://{address}/link").parse().unwrap()),
    }]);
    // World made a key of its own; this test's gateway proves with the one the peer pins.
    drop(world.listener);
    let world = World {
        listener: LinkListener::start(
            &world.gateway.liaison().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            None,
            &gateway_key_dir.path().join("gateway"),
        )
        .unwrap(),
        ..world
    };

    let (relay, principal) = station_relay(world.registry.clone(), "session-7");
    let to_peer = |name: &str, body: &str, key: &str| {
        relay.send(
            &principal,
            CableSendRequest::new(
                CableTarget::try_new(CableScope::Agent, Some(name.to_owned())).unwrap(),
                body.to_owned(),
                key.to_owned(),
            ),
        )
    };
    let sent = to_peer("a2a:echo/helper", "hello echo", "out-1").unwrap();
    let mut reply = None;
    wait_until("the peer's reply Cable", || {
        reply = relay
            .inbox(&principal, 50)
            .unwrap()
            .into_iter()
            .find(|delivery| delivery.reply_to.as_deref() == Some(sent.id()));
        reply.is_some()
    });
    let reply = reply.unwrap();
    assert_eq!(reply.body, "echo: hello echo");
    assert_eq!(reply.from_agent, "a2a:echo");
    assert_eq!(
        reply.peer.as_deref(),
        Some(echo_key.peer_id().to_string().as_str())
    );
    let got = echo.got.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].joined_text(), "hello echo");
    let metadata = got[0].metadata.clone().unwrap();
    assert_eq!(metadata["addressee"], "helper");
    assert_eq!(metadata["from"], "agent:G/007");

    assert!(
        matches!(
            to_peer("a2a:stranger", "hello", "out-2"),
            Err(CableRelayError::Unavailable)
        ),
        "a peer no send grant names is refused"
    );
    // The outbound record follows the reply Cable, since it says whether one was made.
    wait_until("the outbound audit record", || {
        world
            .registry
            .audit_records()
            .unwrap()
            .iter()
            .any(|record| {
                record.action == "liaison.outbound"
                    && record.target.contains(sent.id())
                    && record.target.contains("answered")
            })
    });
    let audit = world.registry.audit_records().unwrap();
    for record in &audit {
        assert!(
            !record.target.contains("hello") && !record.target.contains("echo:"),
            "{record:?}"
        );
    }
    stop.store(true, Ordering::SeqCst);
    drop(world);
    peer_thread.join().unwrap();
}

#[test]
fn settings_read_peers_and_refuse_what_is_not_one() {
    let mut settings = ai6::bootstrap::LiaisonSettings::default();
    let mut peer = ai6::bootstrap::LiaisonPeerSettings::default();
    peer.id = Some("sha256:00".to_owned());
    peer.principal = Some("person:acme-bot".to_owned());
    peer.addressees = vec!["agent:G/007".to_owned(), "role:G/coordinator".to_owned()];
    peer.url = Some("wss://peer.example/link".to_owned());
    settings.peer.insert("acme".to_owned(), peer.clone());
    let config = LiaisonConfig::from_settings(&settings).unwrap();
    let acme = &config.peers["acme"];
    assert!(acme.a2a);
    assert_eq!(acme.addressees.len(), 2);
    for (field, value) in [
        ("principal", "acme-bot"),
        ("addressee", "agent:G"),
        ("url", "https://x"),
    ] {
        let mut wrong = peer.clone();
        match field {
            "principal" => wrong.principal = Some(value.to_owned()),
            "addressee" => wrong.addressees = vec![value.to_owned()],
            _ => wrong.url = Some(value.to_owned()),
        }
        let mut settings = ai6::bootstrap::LiaisonSettings::default();
        settings.peer.insert("acme".to_owned(), wrong);
        assert!(
            LiaisonConfig::from_settings(&settings).is_err(),
            "{field} {value}"
        );
    }
    let _ = Liaison::new;
}

/// The official A2A JavaScript SDK through both bindings (`sdk_check.mjs`). Ignored by default: it
/// needs Node in `AI6_TEST_NODE` and `@a2a-js/sdk` installed under `A2A_JS_SDK_DIR`, and fails
/// when asked for without them.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Node and @a2a-js/sdk: AI6_TEST_NODE, A2A_JS_SDK_DIR"]
async fn the_official_a2a_javascript_sdk_completes_exchanges_on_both_bindings() {
    use coqui_a2a::loopback::{Config, Hub, Loopback};

    let node = std::env::var("AI6_TEST_NODE").expect("AI6_TEST_NODE names node");
    let sdk = std::env::var("A2A_JS_SDK_DIR").expect("A2A_JS_SDK_DIR holds @a2a-js/sdk");
    let world = Arc::new(World::new());
    let _standin = StandIn::start(world.registry.clone(), "session-7", true);
    let bearer = crate::issue_access_token_record(
        Arc::new(world.registry.clone()),
        "acme-bot".to_owned(),
        Duration::from_secs(600),
        ai6::TimeLimit::UNBOUNDED,
    )
    .await
    .ok()
    .unwrap()
    .token;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = format!("http://{}/v1/a2a/", listener.local_addr().unwrap());
    let router = world.gateway.clone().router();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    let hub = Hub::new();
    let token = "loopback-token-for-the-sdk";
    let endpoint = Loopback::start(
        Config::new("127.0.0.1:0".parse().unwrap())
            .unwrap()
            .with_token(token),
        hub.clone(),
    )
    .unwrap();
    let (linked, attached) = (world.clone(), hub.clone());
    let link = thread::spawn(move || linked.over_link(&linked.acme, |mux| attached.attach(mux)));
    let until = Instant::now() + WAIT;
    while !hub.is_up() {
        assert!(Instant::now() < until, "the link came up");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/liaison/sdk_check.mjs");
    let loopback = format!("http://{}/", endpoint.local_addr());
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(node)
            .arg(script)
            .env("A2A_JS_SDK_DIR", sdk)
            .env("LIAISON_LOOPBACK", loopback)
            .env("LIAISON_LOOPBACK_TOKEN", token)
            .env("LIAISON_HTTP", http)
            .env("LIAISON_BEARER", bearer)
            .env("LIAISON_ORIGIN", "https://localhost")
            .output()
    })
    .await
    .unwrap()
    .unwrap();
    let printed = String::from_utf8_lossy(&output.stdout);
    println!("{printed}{}", String::from_utf8_lossy(&output.stderr));
    hub.close();
    link.join().unwrap().unwrap();
    assert!(output.status.success(), "the SDK check failed:\n{printed}");
    assert_eq!(world.work_left(), Vec::<String>::new());
}
