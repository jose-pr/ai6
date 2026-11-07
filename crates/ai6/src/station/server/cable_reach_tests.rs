//! Cables reach a Session wherever its harness runs: over its runner's connection, on this host
//! or another, through a forward for the detached wrapper, and not at all on a Theater that
//! offers neither, which placement refuses unless the Theater accepts Sessions without Cables.
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

use super::tests::{start_test_runner, test_state, wait_for_done};
use super::*;
use crate::directorates::q::terminal::AgentTerminal;
use crate::directorates::registry::cables::{
    CableAction, CableGrant, CablePolicy, CableScope, CableTarget, CrossOperation,
};
use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart};

/// A harness that uses the Cable endpoint its environment names, as a model would: it lists
/// the tools, then makes the calls `MOCK_CABLES` asks for (a JSON array of `{"tool", "args"}`,
/// one with `"until": TEXT` repeated until its answer holds TEXT, one with `"reply_to": TEXT`
/// given the `cable_id` of a Cable read earlier whose body holds TEXT), and says each answer as
/// a `text` event, `TOOL=>ANSWER`, before it completes. With no endpoint it says `cables:none`.
#[derive(Default, Provider)]
#[provider(AgentHandler, id = "mock-cables", name = "Mock Cables", crate = crate::guatiao)]
pub(super) struct MockCablesHandler;

impl AgentHandler for MockCablesHandler {
    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let session_id =
            SessionId::try_new(format!("provider-session-cables-{}", request.mission_id)).map_err(
                |error| {
                    ProviderError::new(guatiao::Status::GUATIAO_ERR_INTERNAL, &error.to_string())
                },
            )?;
        let events: Arc<Mutex<VecDeque<AgentEvent>>> = Arc::default();
        let (environment, queue) = (request.environment, events.clone());
        std::thread::spawn(move || {
            let said = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map(|runtime| runtime.block_on(use_cables(&environment)))
                .unwrap_or_else(|error| vec![format!("cables:{error}")]);
            let mut queue = queue.lock().unwrap();
            queue.extend(said.into_iter().map(|line| event("text", Some(line))));
            queue.push_back(event("completed", None));
        });
        Ok(CablesSession {
            session_id,
            events,
            _terminal: Some(terminal),
        }
        .into_object())
    }

    fn resume(
        &self,
        request: AgentResume,
        _terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let events = Arc::new(Mutex::new(VecDeque::from([event("completed", None)])));
        Ok(CablesSession {
            session_id: request.session_id,
            events,
            _terminal: None,
        }
        .into_object())
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

struct CablesSession {
    session_id: SessionId,
    events: Arc<Mutex<VecDeque<AgentEvent>>>,
    /// Held as a harness's process holds it: the detached wrapper's forward lives with it.
    _terminal: Option<Object<dyn AgentTerminal>>,
}

impl AgentSession for CablesSession {
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.session_id.to_string()))
    }

    fn send_message(&self, _message: AgentMessage) -> Result<(), ProviderError> {
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

/// What the harness says it did with the Cable tools.
async fn use_cables(environment: &BTreeMap<String, String>) -> Vec<String> {
    use rmcp::ServiceExt as _;
    use rmcp::model::{CallToolRequestParams, ContentBlock};
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let (Some(url), Some(token)) = (
        environment.get(crate::directorates::section_00::gate::CABLE_URL_ENV),
        environment.get(crate::directorates::section_00::gate::CABLE_TOKEN_ENV),
    ) else {
        return vec!["cables:none".to_owned()];
    };
    let steps: Vec<serde_json::Value> = environment
        .get("MOCK_CABLES")
        .and_then(|script| serde_json::from_str(script).ok())
        .unwrap_or_default();
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.clone()).auth_header(token.clone()),
    );
    let client = match ().serve(transport).await {
        Ok(client) => client,
        Err(error) => return vec![format!("cables:{error}")],
    };
    let mut said = Vec::new();
    match client.list_all_tools().await {
        Ok(tools) => said.push(format!(
            "tools:{}",
            tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )),
        Err(error) => said.push(format!("tools:{error}")),
    }
    for step in steps {
        let tool = step["tool"].as_str().unwrap_or_default().to_owned();
        let mut arguments = step["args"].as_object().cloned().unwrap_or_default();
        if let Some(cable) = step["reply_to"]
            .as_str()
            .and_then(|text| cable_read(&said, text))
        {
            arguments.insert("cable_id".to_owned(), cable.into());
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let answer = loop {
            let answer = match client
                .call_tool(
                    CallToolRequestParams::new(tool.clone()).with_arguments(arguments.clone()),
                )
                .await
            {
                Ok(result) => result
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        ContentBlock::Text(text) => Some(text.text.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                Err(error) => format!("ERROR {error}"),
            };
            match step["until"].as_str() {
                Some(until) if !answer.contains(until) && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                _ => break answer,
            }
        };
        said.push(format!("{tool}=>{answer}"));
    }
    let _ = client.cancel().await;
    said
}

/// The `cable_id` of a Cable among what `said` read whose body holds `text`.
fn cable_read(said: &[String], text: &str) -> Option<String> {
    said.iter()
        .filter_map(|line| line.strip_prefix("read_cables=>"))
        .filter_map(|answer| serde_json::from_str::<Vec<serde_json::Value>>(answer).ok())
        .flatten()
        .find(|delivery| {
            delivery["body"]
                .as_str()
                .is_some_and(|body| body.contains(text))
        })
        .and_then(|delivery| delivery["cable_id"].as_str().map(str::to_owned))
}

/// 007 tells 008 something and waits for the answer; 008 waits for it and replies, which is
/// all a Session that read a Cable from another Operation may send.
fn scripts() -> (String, String) {
    let send = |to: &str, body: &str, key: &str| {
        serde_json::json!({
            "tool": "send_cable",
            "args": {"scope": "agent", "target": to, "body": body, "idempotency_key": key},
            "until": "recipient_count",
        })
    };
    let read = |until: &str| serde_json::json!({"tool": "read_cables", "until": until});
    (
        serde_json::json!([send("008", "hello from 007", "k-007"), read("hello back")]).to_string(),
        serde_json::json!([
            read("hello from 007"),
            {
                "tool": "reply_to_cable",
                "args": {"body": "hello back", "idempotency_key": "k-008"},
                "reply_to": "hello from 007",
            },
        ])
        .to_string(),
    )
}

/// A Station with Operation `G` on Theater `theater_of_g` (declared by the caller) and `L` on
/// its own host, Agents 007 and 008 on `mock-cables`, Mission `mission-1` (G, 007) and
/// `mission-3` (L, 008), and a relay that lets the two exchange Cables across the Operations.
fn cable_station(root: &Path, theater_of_g: &str) -> DaemonState {
    let mut state = test_state(
        root,
        "",
        "[agent.007]\nhandler = \"mock-cables\"\n[agent.008]\nhandler = \"mock-cables\"\n",
    );
    let checkout = root.join("checkout").to_string_lossy().into_owned();
    fs::write(
        root.join("operations.toml"),
        format!(
            "[operation.G]\npath = {checkout:?}\ntheater = {theater_of_g:?}\n\
             [operation.L]\npath = {checkout:?}\n"
        ),
    )
    .unwrap();
    state.roster = Arc::new(Roster::load(root).unwrap());
    state
        .missions
        .lock()
        .unwrap()
        .create(
            MissionId::try_new("mission-3").unwrap(),
            Some("L".to_owned()),
            "answer 007".to_owned(),
            Some("008".to_owned()),
            None,
        )
        .unwrap();
    let station =
        CableTarget::try_new(CableScope::Station, Some(state.station_id.clone())).unwrap();
    let mut grants = Vec::new();
    for agent in ["007", "008"] {
        for action in [CableAction::Send, CableAction::Receive] {
            grants.push(CableGrant::try_new(agent.to_owned(), action, station.clone()).unwrap());
        }
    }
    let (roster, missions, sessions, station_id) = (
        state.roster.clone(),
        state.missions.clone(),
        state.sessions.clone(),
        state.station_id.clone(),
    );
    state.cables = Arc::new(
        CableRelay::new(
            Arc::new(crate::directorates::registry::store::FileRegistry::open(
                root.join("cables"),
            )),
            CablePolicy::try_new(grants, BTreeMap::new()).unwrap(),
            move || cable_endpoints(&roster, &missions, &sessions, &station_id),
            BTreeMap::new(),
        )
        .with_cross_operation(CrossOperation::StationGrants)
        .with_durable_audit(|_| Ok(())),
    );
    state
}

async fn start(state: &DaemonState, mission_id: &str, script: &str) {
    let _ = start_mission(
        State(state.clone()),
        axum::extract::Path(mission_id.to_owned()),
        Json(StartMission {
            working_directory: None,
            initial_context: Vec::new(),
            environment: BTreeMap::from([("MOCK_CABLES".to_owned(), script.to_owned())]),
        }),
    )
    .await
    .unwrap_or_else(|error| panic!("{mission_id} did not start: {:?}", error.1.0.error));
}

/// The text events of each Mission until both complete.
async fn said(state: &DaemonState, missions: &[&str]) -> BTreeMap<String, Vec<String>> {
    let mut said: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut done = BTreeSet::new();
    let finished = tokio::time::timeout(Duration::from_secs(60), async {
        while done.len() < missions.len() {
            let mut idle = true;
            for mission in missions {
                if let Some(event) = pop_event(state, mission) {
                    idle = false;
                    if event.kind == "completed" {
                        done.insert(mission.to_string());
                    }
                    said.entry(mission.to_string())
                        .or_default()
                        .extend(event.text);
                }
            }
            if idle {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    })
    .await;
    assert!(finished.is_ok(), "the Sessions did not complete: {said:?}");
    said
}

fn answer<'a>(said: &'a [String], tool: &str) -> &'a str {
    said.iter()
        .rev()
        .find_map(|line| line.strip_prefix(&format!("{tool}=>")))
        .unwrap_or_else(|| panic!("no {tool} answer in {said:?}"))
}

/// What each side of a 007/008 exchange must have seen.
fn exchanged(said: &BTreeMap<String, Vec<String>>, remote: &str) {
    let (far, near) = (&said[remote], &said["mission-3"]);
    for side in [far, near] {
        let tools = side
            .iter()
            .find_map(|line| line.strip_prefix("tools:"))
            .unwrap_or_else(|| panic!("no tool listing in {side:?}"));
        let tools: Vec<_> = tools.split(',').collect();
        for tool in [
            "send_cable",
            "read_cables",
            "acknowledge_cable",
            "reply_to_cable",
            "discover_agents",
        ] {
            assert!(tools.contains(&tool), "{tool} is not listed: {tools:?}");
        }
    }
    assert!(
        answer(far, "send_cable").contains("recipient_count"),
        "{far:?}"
    );
    assert!(
        answer(near, "read_cables").contains("hello from 007"),
        "{near:?}"
    );
    assert!(
        answer(near, "reply_to_cable").contains("cable_id"),
        "{near:?}"
    );
    assert!(answer(far, "read_cables").contains("hello back"), "{far:?}");
}

/// On this host too, a runner's Session reaches its Cables over the runner's connection: the
/// harness is given the runner's relay, never this Station's endpoint or bearer.
#[cfg(feature = "cables-mcp")]
#[tokio::test(flavor = "multi_thread")]
async fn a_session_on_a_runner_sends_and_reads_cables_over_the_runners_connection() {
    let root = tempfile::tempdir().unwrap();
    let runner_dir = root.path().join("runner");
    let _runner = start_test_runner(&runner_dir).await;
    let mut state = cable_station(root.path(), "near");
    let mut theaters = Theaters::default();
    theaters
        .insert(
            Theater::new("near", TheaterAccess::Local).with_runner(
                crate::RunnerSettings::default()
                    .with_state_dir(runner_dir.to_string_lossy().into_owned())
                    .with_command(root.path().join("no-runner-command").to_string_lossy()),
            ),
        )
        .unwrap();
    state.transports = Arc::new(Transports::build(&theaters, &plugins::Plugins::scan(&[])));

    let (far, near) = scripts();
    start(&state, "mission-1", &far).await;
    start(&state, "mission-3", &near).await;
    let manifest = state
        .missions
        .lock()
        .unwrap()
        .operation("mission-1")
        .unwrap();
    assert!(manifest.runner.is_some(), "mission-1 runs on the runner");
    let said = said(&state, &["mission-1", "mission-3"]).await;
    exchanged(&said, "mission-1");
    wait_for_done(&state, "mission-1").await;
    wait_for_done(&state, "mission-3").await;
}

#[cfg(unix)]
fn remote_station(root: &Path, theater: Theater, transport: ShellTransportSetup) -> DaemonState {
    let mut state = cable_station(root, "box");
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let mut shell =
        crate::station::transports::testing::ShellTransport::new(&home, transport.runner_dir);
    if !transport.forward {
        shell = shell.without_forward();
    }
    *transport.forwards.lock().unwrap() = Some(shell.forwards());
    let mut theaters = Theaters::default();
    theaters.insert(theater.clone()).unwrap();
    state.transports = Arc::new(
        Transports::build(&theaters, &plugins::Plugins::scan(&[]))
            .with_transport(theater, Box::new(shell)),
    );
    state
}

#[cfg(unix)]
struct ShellTransportSetup<'a> {
    runner_dir: Option<&'a Path>,
    forward: bool,
    forwards: &'a Mutex<Option<Arc<std::sync::atomic::AtomicUsize>>>,
}

#[cfg(unix)]
fn box_theater(root: &Path) -> Theater {
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    Theater::new(
        "box",
        TheaterAccess::Transport(crate::TransportAccess::new("shell")),
    )
    .with_workdir(work.to_string_lossy().into_owned())
}

/// A runner on another host carries its Session's Cables over the connection the Station
/// reaches it by, with no forward: the same relay works over any transport that runs it.
#[cfg(all(unix, feature = "cables-mcp"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_runner_on_another_host_carries_cables_over_its_connection_with_no_forward() {
    let root = tempfile::tempdir().unwrap();
    let runner_dir = root.path().join("runner");
    let _runner = start_test_runner(&runner_dir).await;
    let forwards = Mutex::new(None);
    let theater = box_theater(root.path()).with_runner(crate::RunnerSettings::default());
    let state = remote_station(
        root.path(),
        theater,
        ShellTransportSetup {
            runner_dir: Some(&runner_dir),
            forward: false,
            forwards: &forwards,
        },
    );
    let (far, near) = scripts();
    start(&state, "mission-1", &far).await;
    start(&state, "mission-3", &near).await;
    let said = said(&state, &["mission-1", "mission-3"]).await;
    exchanged(&said, "mission-1");
    assert_eq!(
        forwards
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .load(Ordering::Acquire),
        0,
        "the runner's Session needs no forward"
    );
}

/// The detached wrapper's harness reaches the Station's endpoint through a reverse forward.
#[cfg(all(unix, feature = "cables-mcp"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_wrapped_harness_on_another_host_sends_and_reads_cables_through_a_forward() {
    let root = tempfile::tempdir().unwrap();
    let forwards = Mutex::new(None);
    let theater = box_theater(root.path()).with_harness("mock-cables");
    let state = remote_station(
        root.path(),
        theater,
        ShellTransportSetup {
            runner_dir: None,
            forward: true,
            forwards: &forwards,
        },
    );
    let (far, near) = scripts();
    start(&state, "mission-1", &far).await;
    let manifest = state
        .missions
        .lock()
        .unwrap()
        .operation("mission-1")
        .unwrap();
    assert_eq!(manifest.terminal_provider, "detached");
    start(&state, "mission-3", &near).await;
    let said = said(&state, &["mission-1", "mission-3"]).await;
    exchanged(&said, "mission-1");
    assert!(
        forwards
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .load(Ordering::Acquire)
            >= 1,
        "the Cable endpoint was forwarded to the Theater"
    );
}

/// A Theater with neither a runner nor a forward cannot take the wrapped harness to its Cable
/// endpoint: field mode there is refused with the reason, and only `cables = "optional"` starts
/// a Session without Cables, which the Theater reports as weakened.
#[cfg(all(unix, feature = "cables-mcp"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_theater_with_no_way_to_the_cable_endpoint_refuses_field_mode_unless_it_accepts() {
    let root = tempfile::tempdir().unwrap();
    let forwards = Mutex::new(None);
    let sealed = box_theater(root.path())
        .with_harness("mock-cables")
        .with_agents(crate::AgentMode::Local);
    let state = remote_station(
        root.path(),
        sealed.clone(),
        ShellTransportSetup {
            runner_dir: None,
            forward: false,
            forwards: &forwards,
        },
    );
    let refused = start_mission(
        State(state.clone()),
        axum::extract::Path("mission-1".to_owned()),
        Json(StartMission {
            working_directory: None,
            initial_context: Vec::new(),
            environment: BTreeMap::new(),
        }),
    )
    .await
    .err()
    .map(|error| error.1.0.error)
    .expect("field mode with no way to the Cable endpoint is refused");
    assert!(
        refused.contains("the `forward` capability that takes the harness to its Cable endpoint"),
        "{refused}"
    );

    let root = tempfile::tempdir().unwrap();
    let accepting = box_theater(root.path())
        .with_harness("mock-cables")
        .with_agents(crate::AgentMode::Local)
        .with_cables(crate::CableReach::Optional);
    let state = remote_station(
        root.path(),
        accepting,
        ShellTransportSetup {
            runner_dir: None,
            forward: false,
            forwards: &forwards,
        },
    );
    let weakened = state.transports.summary("box", false).unwrap().weakened;
    assert!(
        weakened
            .iter()
            .any(|line| line.contains("cables = \"optional\"")),
        "{weakened:?}"
    );
    start(&state, "mission-1", "[]").await;
    let said = said(&state, &["mission-1"]).await;
    assert_eq!(
        said["mission-1"],
        ["cables:none"],
        "the Session started without Cables"
    );
}
