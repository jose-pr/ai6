//! A Cable reaches an Agent that is between turns or has no Session: a Session waiting at a
//! prompt takes a turn on it, and an Agent with none has it in the brief of its next. With
//! stand-in harnesses, which answer over the Cable tools as a model would.
#![cfg_attr(
    not(feature = "cables-mcp"),
    allow(
        dead_code,
        unused_imports,
        reason = "only the handler is used without Cable tools"
    )
)]

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use guatiao::Provider;
use guatiao::library::{Object, ProviderError};

use super::tests::{test_state, wait_for_state};
use super::*;
use crate::directorates::q::terminal::AgentTerminal;
use crate::directorates::registry::cables::{
    CableAction, CableGrant, CablePolicy, CableScope, CableState, CableTarget, CrossOperation,
};
use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart};

/// What the Station handed the stand-in at each start or resume: the Mission and its brief's
/// context.
pub(super) static STARTS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());
/// Each message a stand-in Session was sent, with its provider Session ID.
static MESSAGES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// A harness that waits at a prompt, as one does between turns. Sent a message, it answers the
/// Cables in it with the body `MOCK_ANSWER` names, through the Cable tools, and waits again.
#[derive(Default, Provider)]
#[provider(AgentHandler, id = "mock-wake", name = "Mock Wake", crate = crate::guatiao)]
pub(super) struct MockWakeHandler;

impl AgentHandler for MockWakeHandler {
    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        STARTS.lock().unwrap().push((
            request.mission_id.to_string(),
            request.initial_context.clone(),
        ));
        let session_id =
            SessionId::try_new(format!("wake-{}", request.mission_id)).map_err(|error| {
                ProviderError::new(guatiao::Status::GUATIAO_ERR_INTERNAL, &error.to_string())
            })?;
        Ok(WakeSession::waiting(session_id, request.environment, terminal).into_object())
    }

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        STARTS.lock().unwrap().push((
            request.mission_id.to_string(),
            request.initial_context.clone(),
        ));
        Ok(WakeSession::waiting(request.session_id, request.environment, terminal).into_object())
    }
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

struct WakeSession {
    session_id: SessionId,
    environment: BTreeMap<String, String>,
    events: Arc<Mutex<VecDeque<AgentEvent>>>,
    _terminal: Object<dyn AgentTerminal>,
}

impl WakeSession {
    fn waiting(
        session_id: SessionId,
        environment: BTreeMap<String, String>,
        terminal: Object<dyn AgentTerminal>,
    ) -> Self {
        Self {
            session_id,
            environment,
            events: Arc::new(Mutex::new(VecDeque::from([event("blocked", None)]))),
            _terminal: terminal,
        }
    }
}

impl AgentSession for WakeSession {
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.session_id.to_string()))
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        MESSAGES
            .lock()
            .unwrap()
            .push((self.session_id.to_string(), message.text.clone()));
        let (Some(body), Some(url), Some(token)) = (
            self.environment.get("MOCK_ANSWER").cloned(),
            self.environment
                .get(crate::directorates::section_00::gate::CABLE_URL_ENV)
                .cloned(),
            self.environment
                .get(crate::directorates::section_00::gate::CABLE_TOKEN_ENV)
                .cloned(),
        ) else {
            return Ok(());
        };
        let (events, session) = (self.events.clone(), self.session_id.to_string());
        std::thread::spawn(move || {
            let said = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map(|runtime| {
                    runtime.block_on(answer(&url, &token, &message.text, &body, &session))
                })
                .unwrap_or_else(|error| format!("answer failed: {error}"));
            let mut events = events.lock().unwrap();
            events.push_back(event("text", Some(said)));
            events.push_back(event("blocked", None));
        });
        Ok(())
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(0).unsigned_abs());
        loop {
            if let Some(event) = self.events.lock().unwrap().pop_front() {
                return Ok(Some(event));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        Ok(())
    }

    fn close(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

/// Replies, over the Cable tools, to each Cable `message` carries.
async fn answer(url: &str, token: &str, message: &str, body: &str, session: &str) -> String {
    use rmcp::ServiceExt as _;
    use rmcp::model::CallToolRequestParams;
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let cables: Vec<serde_json::Value> = message
        .split_once("\n\n")
        .and_then(|(_, listed)| serde_json::from_str(listed).ok())
        .unwrap_or_default();
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token.to_owned()),
    );
    let client = match ().serve(transport).await {
        Ok(client) => client,
        Err(error) => return format!("no endpoint: {error}"),
    };
    let mut said = Vec::new();
    for cable in cables {
        let id = cable["cable_id"].as_str().unwrap_or_default().to_owned();
        let arguments = serde_json::json!({
            "cable_id": id,
            "body": body,
            "idempotency_key": format!("{session}-{id}"),
        });
        let reply = client
            .call_tool(
                CallToolRequestParams::new("reply_to_cable")
                    .with_arguments(arguments.as_object().cloned().unwrap_or_default()),
            )
            .await;
        said.push(format!("{id}: {}", reply.is_ok()));
    }
    let _ = client.cancel().await;
    said.join(" ")
}

const AGENTS: &str = "[agent.007]\nhandler = \"mock-wake\"\n";

/// A Station with Operation `G`, Agents 007 and 008 on `mock-wake` (`more` is added to 008's
/// settings), Missions `TAG-1` (007) and `TAG-3` (008), and a relay that lets the two exchange
/// Cables, with each Agent's Cables waiting for it while it has no Session. Each test has its own
/// `tag`: the stand-in's records are shared by the tests of one process.
fn wake_station(root: &Path, tag: &str, more: &str) -> DaemonState {
    let mut state = test_state(
        root,
        "",
        &format!("{AGENTS}[agent.008]\nhandler = \"mock-wake\"\n{more}"),
    );
    for (suffix, agent) in [("1", "007"), ("3", "008")] {
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(format!("{tag}-{suffix}")).unwrap(),
                Some("G".to_owned()),
                format!("work for {agent}"),
                Some(agent.to_owned()),
                None,
            )
            .unwrap();
    }
    open_cables(&mut state);
    state
}

/// The policy that lets 007 and 008 send to and receive from each other, or `without` of them
/// only send.
fn policy(without: &[&str], station: &str) -> CablePolicy {
    let station = CableTarget::try_new(CableScope::Station, Some(station.to_owned())).unwrap();
    let mut grants = Vec::new();
    for agent in ["007", "008"] {
        for action in [CableAction::Send, CableAction::Receive] {
            if action == CableAction::Receive && without.contains(&agent) {
                continue;
            }
            grants.push(CableGrant::try_new(agent.to_owned(), action, station.clone()).unwrap());
        }
    }
    CablePolicy::try_new(grants, BTreeMap::new()).unwrap()
}

/// Gives the Station the relay of this module's Stations.
pub(super) fn open_cables(state: &mut DaemonState) {
    let (roster, missions, sessions, station_id) = (
        state.roster.clone(),
        state.missions.clone(),
        state.sessions.clone(),
        state.station_id.clone(),
    );
    let (resident_roster, resident_site) = (state.roster.clone(), state.station_id.clone());
    state.cables = Arc::new(
        CableRelay::new(
            state.intake.clone(),
            policy(&[], &state.station_id),
            move || cable_endpoints(&roster, &missions, &sessions, &station_id),
            BTreeMap::new(),
        )
        .with_residents(move |target| cable_residents(&resident_roster, &resident_site, target))
        .with_cross_operation(CrossOperation::StationGrants)
        .with_durable_audit(|_| Ok(())),
    );
}

async fn start(state: &DaemonState, mission_id: &str, answer: Option<&str>) {
    let environment = answer
        .map(|body| BTreeMap::from([("MOCK_ANSWER".to_owned(), body.to_owned())]))
        .unwrap_or_default();
    let _ = start_mission(
        State(state.clone()),
        axum::extract::Path(mission_id.to_owned()),
        Json(StartMission {
            working_directory: None,
            initial_context: Vec::new(),
            environment,
        }),
    )
    .await
    .unwrap_or_else(|error| panic!("{mission_id} did not start: {:?}", error.1.0.error));
}

/// Waits for the Session of `mission_id` to be waiting at a prompt, as an endpoint Cables reach.
async fn idle(state: &DaemonState, mission_id: &str) {
    wait_for_state(state, mission_id, MissionState::Blocked).await;
    until("the Session is a Cable endpoint", || {
        state
            .sessions
            .lock()
            .unwrap()
            .get(mission_id)
            .is_some_and(|operation| operation.session_id.is_some())
    })
    .await;
}

/// Long enough for a message a wake sent to reach a stand-in, which takes it from a queue.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(500)).await;
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let reached = tokio::time::timeout(Duration::from_secs(60), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(reached.is_ok(), "never happened: {what}");
}

fn principal(agent: &str, session: Option<&str>) -> CablePrincipal {
    CablePrincipal::try_new(
        agent.to_owned(),
        "G".to_owned(),
        None,
        "test".to_owned(),
        session.map(str::to_owned),
        None,
    )
    .unwrap()
}

fn send(state: &DaemonState, from: &CablePrincipal, to: &str, body: &str) -> String {
    state
        .cables
        .send(
            from,
            CableSendRequest::new(
                CableTarget::try_new(CableScope::Agent, Some(to.to_owned())).unwrap(),
                body.to_owned(),
                format!("key-{body}"),
            ),
        )
        .unwrap()
        .id()
        .to_owned()
}

fn delivery(state: &DaemonState, cable: &str) -> CableState {
    state.intake.get_cable(cable).unwrap().unwrap().recipients()[0].state()
}

fn messages_to(session: &str) -> Vec<String> {
    MESSAGES
        .lock()
        .unwrap()
        .iter()
        .filter(|(to, _)| to == session)
        .map(|(_, text)| text.clone())
        .collect()
}

/// Stops the Station's pumps when a test ends, however it ends.
struct Stopping(Arc<AtomicBool>);

impl Drop for Stopping {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn missions_held(state: &DaemonState) -> usize {
    state.missions.lock().unwrap().list().len()
}

/// A Cable made no Mission, no brief to place and no placement.
fn nothing_was_made(state: &DaemonState, missions: usize) {
    assert_eq!(missions_held(state), missions, "a Mission was created");
    assert!(
        state.intake.list_new().unwrap().is_empty(),
        "a brief was made"
    );
    assert!(
        state.intake.leases().unwrap().is_empty(),
        "a Mission was placed"
    );
}

#[cfg(feature = "cables-mcp")]
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_session_takes_a_turn_on_a_cable_and_answers_it() {
    let root = tempfile::tempdir().unwrap();
    let state = wake_station(root.path(), "idle", "");
    let missions = missions_held(&state);
    let _stopping = Stopping(state.stopping.clone());
    start(&state, "idle-1", None).await;
    start(&state, "idle-3", Some("pong")).await;
    idle(&state, "idle-1").await;
    idle(&state, "idle-3").await;

    let sender = principal("007", Some("wake-idle-1"));
    let cable = send(&state, &sender, "008", "ping from 007");
    cable_wakes::wake_idle(&state).await;

    until("the Session is told of the Cable", || {
        !messages_to("wake-idle-3").is_empty()
    })
    .await;
    let heard = messages_to("wake-idle-3");
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert!(heard[0].contains("ping from 007"), "{}", heard[0]);
    assert!(
        heard[0].contains("untrusted input"),
        "the Cable arrives marked as untrusted: {}",
        heard[0]
    );
    assert_eq!(delivery(&state, &cable), CableState::Delivered);

    until("the answer reaches the sender", || {
        state
            .cables
            .inbox(&sender, 10)
            .unwrap()
            .iter()
            .any(|reply| reply.body == "pong" && reply.reply_to.as_deref() == Some(&cable))
    })
    .await;
    wait_for_state(&state, "idle-3", MissionState::Blocked).await;
    assert!(
        messages_to("wake-idle-1").is_empty(),
        "the sender's own Session heard nothing"
    );
    nothing_was_made(&state, missions);
}

#[cfg(feature = "cables-mcp")]
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_opts_out_is_not_woken_and_reads_the_cable_itself() {
    let root = tempfile::tempdir().unwrap();
    let state = wake_station(root.path(), "optout", "wake_on_cable = false\n");
    let missions = missions_held(&state);
    let _stopping = Stopping(state.stopping.clone());
    start(&state, "optout-3", None).await;
    idle(&state, "optout-3").await;

    let cable = send(&state, &principal("007", None), "008", "are you there");
    cable_wakes::wake_idle(&state).await;
    settle().await;
    assert!(messages_to("wake-optout-3").is_empty());
    assert_eq!(delivery(&state, &cable), CableState::Queued);

    let read = state
        .cables
        .inbox(&principal("008", Some("wake-optout-3")), 10)
        .unwrap();
    assert_eq!(read.len(), 1, "its own poll takes it");
    assert_eq!(read[0].body, "are you there");
    nothing_was_made(&state, missions);
}

#[cfg(feature = "cables-mcp")]
#[tokio::test(flavor = "multi_thread")]
async fn the_wake_quota_stops_a_storm_and_the_cables_wait_for_the_sessions_own_read() {
    let root = tempfile::tempdir().unwrap();
    let state = wake_station(root.path(), "storm", "wakes_per_hour = 1\n");
    let missions = missions_held(&state);
    let _stopping = Stopping(state.stopping.clone());
    start(&state, "storm-1", None).await;
    start(&state, "storm-3", Some("pong")).await;
    idle(&state, "storm-1").await;
    idle(&state, "storm-3").await;
    let sender = principal("007", Some("wake-storm-1"));

    send(&state, &sender, "008", "first");
    cable_wakes::wake_idle(&state).await;
    until("the first wake is answered", || {
        state
            .cables
            .inbox(&sender, 10)
            .unwrap()
            .iter()
            .any(|reply| reply.body == "pong")
    })
    .await;
    wait_for_state(&state, "storm-3", MissionState::Blocked).await;

    let storm: Vec<_> = (0..3)
        .map(|round| send(&state, &sender, "008", &format!("storm {round}")))
        .collect();
    for _ in 0..3 {
        cable_wakes::wake_idle(&state).await;
    }
    settle().await;
    assert_eq!(
        messages_to("wake-storm-3").len(),
        1,
        "one wake an hour: the storm woke nothing"
    );
    for cable in &storm {
        assert_eq!(delivery(&state, cable), CableState::Queued);
    }
    let read = state
        .cables
        .inbox(&principal("008", Some("wake-storm-3")), 10)
        .unwrap();
    assert_eq!(read.len(), 4, "its own poll takes all of them");
    nothing_was_made(&state, missions);
}

#[cfg(feature = "cables-mcp")]
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_revoked_before_the_wake_stops_the_delivery() {
    let root = tempfile::tempdir().unwrap();
    let state = wake_station(root.path(), "revoked", "");
    let missions = missions_held(&state);
    let _stopping = Stopping(state.stopping.clone());
    start(&state, "revoked-3", None).await;
    idle(&state, "revoked-3").await;

    let cable = send(&state, &principal("007", None), "008", "while you could");
    state
        .cables
        .update_policy(policy(&["008"], &state.station_id))
        .unwrap();
    cable_wakes::wake_idle(&state).await;
    settle().await;
    assert!(messages_to("wake-revoked-3").is_empty());
    assert_eq!(delivery(&state, &cable), CableState::Refused);
    assert!(
        state
            .cables
            .inbox(&principal("008", Some("wake-revoked-3")), 10)
            .unwrap()
            .is_empty()
    );
    nothing_was_made(&state, missions);
}

#[cfg(feature = "cables-mcp")]
#[tokio::test(flavor = "multi_thread")]
async fn a_cable_to_an_agent_with_no_session_is_in_the_brief_of_its_next() {
    let root = tempfile::tempdir().unwrap();
    let state = wake_station(root.path(), "brief", "");
    let missions = missions_held(&state);
    let _stopping = Stopping(state.stopping.clone());

    let sender = principal("007", None);
    let first = send(&state, &sender, "008", "while you were away");
    let second = send(&state, &sender, "008", "and once more");
    assert_eq!(delivery(&state, &first), CableState::Queued);
    nothing_was_made(&state, missions);

    start(&state, "brief-3", None).await;
    let briefs: Vec<_> = STARTS
        .lock()
        .unwrap()
        .iter()
        .filter(|(mission, _)| mission == "brief-3")
        .map(|(_, context)| context.join("\n"))
        .collect();
    assert_eq!(briefs.len(), 1);
    let (one, two) = (
        briefs[0].find("while you were away").expect("the first"),
        briefs[0].find("and once more").expect("the second"),
    );
    assert!(one < two, "in the order they were sent");
    assert!(briefs[0].contains("untrusted input"));
    assert_eq!(delivery(&state, &first), CableState::Delivered);
    assert_eq!(delivery(&state, &second), CableState::Delivered);
    idle(&state, "brief-3").await;
    cable_wakes::wake_idle(&state).await;
    settle().await;
    assert!(
        messages_to("wake-brief-3").is_empty(),
        "what the brief carried does not wake the Session again"
    );
    nothing_was_made(&state, missions);
}
