//! The Station against a harness that behaves like Claude Code on stream-json: it writes nothing
//! until it has read a message, its Session ID first appears in its stream, and an interrupted
//! turn still ends with a result. A mock that reports its ID at once hides every bug of that
//! timing.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use guatiao::library::{Object, ProviderError};
use guatiao::{Provider, Status};

use super::tests::{mission_state, start_mission_by_id, test_state, wait_for_state};
use super::*;
use crate::directorates::q::terminal::AgentTerminal;
use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart};

/// What a key's sessions were told, so a test reads only its own.
#[derive(Debug, Default, Clone)]
pub(super) struct StreamLog {
    pub(super) resumed: Vec<Option<String>>,
    pub(super) messages: Vec<String>,
    pub(super) interrupts: usize,
    pub(super) closes: usize,
}

static LOGS: Mutex<BTreeMap<String, StreamLog>> = Mutex::new(BTreeMap::new());

pub(super) fn log_of(key: &str) -> StreamLog {
    LOGS.lock().unwrap().get(key).cloned().unwrap_or_default()
}

/// A handler whose Sessions behave as [`StreamSession`] does.
///
/// Its Agent's environment says how: `MOCK_STREAM_KEY` names the log, `MOCK_STREAM_TURN` is what
/// a first turn does after its initialization (`hold` works on, `complete` ends it),
/// `MOCK_STREAM_INTERRUPT` is how an interrupted turn ends (`failed`, `completed`, `silent`), and
/// `MOCK_STREAM_RESUMES_AS` a Session ID a resume reports other than the one asked for.
/// `MOCK_STREAM_RESUME_FAILS` refuses a resume with that text.
#[derive(Default, Provider)]
#[provider(AgentHandler, id = "mock-stream", name = "Mock Stream", crate = crate::guatiao)]
pub(super) struct MockStreamHandler;

impl AgentHandler for MockStreamHandler {
    fn start(
        &self,
        request: AgentStart,
        _terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let key = setting(&request.environment, "MOCK_STREAM_KEY", "default");
        let session = StreamSession::new(
            &request.environment,
            format!("stream-{key}"),
            setting(&request.environment, "MOCK_STREAM_TURN", "hold"),
        );
        session.read_message();
        Ok(session.into_object())
    }

    fn resume(
        &self,
        request: AgentResume,
        _terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        if let Some(reason) = request.environment.get("MOCK_STREAM_RESUME_FAILS") {
            return Err(ProviderError::new(Status::GUATIAO_ERR_INTERNAL, reason));
        }
        let reports = request
            .environment
            .get("MOCK_STREAM_RESUMES_AS")
            .cloned()
            .unwrap_or_else(|| request.session_id.to_string());
        let session = StreamSession::new(&request.environment, reports, "complete".to_owned());
        session.log(|log| log.resumed.push(request.message.clone()));
        if request.message.is_some() {
            session.read_message();
        }
        Ok(session.into_object())
    }
}

fn setting(environment: &BTreeMap<String, String>, name: &str, default: &str) -> String {
    environment
        .get(name)
        .cloned()
        .unwrap_or_else(|| default.to_owned())
}

fn stream_event(kind: &str, text: Option<&str>) -> AgentEvent {
    AgentEvent {
        kind: kind.to_owned(),
        text: text.map(str::to_owned),
        data: None,
        limit: None,
        usage: None,
        tools: Vec::new(),
        quota: Vec::new(),
    }
}

/// How long after reading a message the harness writes its first line, which the Station's
/// answer to a start or resume does not wait for; real Claude Code takes about 0.4 s.
const FIRST_LINE_AFTER: Duration = Duration::from_millis(200);

#[derive(Default)]
struct Stream {
    read_at: Option<Instant>,
    initialized: bool,
    session_id: Option<SessionId>,
    queue: VecDeque<AgentEvent>,
}

struct StreamSession {
    key: String,
    reports: String,
    turn: String,
    on_interrupt: String,
    stream: Mutex<Stream>,
}

impl StreamSession {
    fn new(environment: &BTreeMap<String, String>, reports: String, turn: String) -> Self {
        Self {
            key: setting(environment, "MOCK_STREAM_KEY", "default"),
            reports,
            turn,
            on_interrupt: setting(environment, "MOCK_STREAM_INTERRUPT", "failed"),
            stream: Mutex::new(Stream::default()),
        }
    }

    fn log(&self, change: impl FnOnce(&mut StreamLog)) {
        change(LOGS.lock().unwrap().entry(self.key.clone()).or_default());
    }

    fn read_message(&self) {
        self.stream
            .lock()
            .unwrap()
            .read_at
            .get_or_insert_with(Instant::now);
    }
}

impl AgentSession for StreamSession {
    fn session_id(&self) -> Result<Option<SessionId>, ProviderError> {
        Ok(self.stream.lock().unwrap().session_id.clone())
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        self.log(|log| log.messages.push(message.text.clone()));
        let mut stream = self.stream.lock().unwrap();
        if stream.initialized {
            stream
                .queue
                .push_back(stream_event("completed", Some(&message.text)));
        }
        stream.read_at.get_or_insert_with(Instant::now);
        Ok(())
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        let waited = Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(0).min(10));
        {
            let mut stream = self.stream.lock().unwrap();
            let first_line_due = stream
                .read_at
                .is_some_and(|read| read.elapsed() >= FIRST_LINE_AFTER);
            if first_line_due && !stream.initialized {
                stream.initialized = true;
                stream.session_id = Some(SessionId::try_new(self.reports.clone()).map_err(
                    |error| ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string()),
                )?);
                if self.turn == "complete" {
                    stream
                        .queue
                        .push_back(stream_event("completed", Some("done")));
                }
                return Ok(Some(stream_event("system", None)));
            }
            if let Some(event) = stream.queue.pop_front() {
                return Ok(Some(event));
            }
        }
        std::thread::sleep(waited);
        Ok(None)
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        self.log(|log| log.interrupts += 1);
        match self.on_interrupt.as_str() {
            "failed" => self.stream.lock().unwrap().queue.push_back(stream_event(
                "failed",
                Some("[ede_diagnostic] result_type=user"),
            )),
            "completed" => self
                .stream
                .lock()
                .unwrap()
                .queue
                .push_back(stream_event("completed", None)),
            _ => {}
        }
        Ok(())
    }

    fn close(&self) -> Result<(), ProviderError> {
        self.log(|log| log.closes += 1);
        Ok(())
    }
}

/// One Agent 007 on the mock, with these lines added to its environment.
fn agent(key: &str, environment: &str, limits: &str) -> String {
    format!(
        "[agent.007]\nhandler = \"mock-stream\"\n[agent.007.env]\nMOCK_STREAM_KEY = \"{key}\"\n\
         {environment}{limits}"
    )
}

async fn wait_until(what: &str, mut holds: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !holds() {
        assert!(Instant::now() < deadline, "never happened: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn wait_for_session(state: &DaemonState, mission_id: &str, wanted: &str) {
    wait_until("the Mission records the Session its stream reported", || {
        mission_state(state, mission_id)
            .session_ids
            .last()
            .map(SessionId::as_str)
            == Some(wanted)
    })
    .await;
}

fn has_live_session(state: &DaemonState, mission_id: &str) -> bool {
    state.sessions.lock().unwrap().contains_key(mission_id)
}

async fn resume(
    state: &DaemonState,
    mission_id: &str,
    message: Option<&str>,
) -> Result<(StatusCode, Json<StartedMission>), ApiErrorResponse> {
    resume_mission(
        State(state.clone()),
        axum::extract::Path(mission_id.to_owned()),
        Json(ResumeMission {
            message: message.map(str::to_owned),
            environment: Default::default(),
        }),
    )
    .await
}

/// Ends the event pumps when a test ends, passed or not: a live pump keeps the runtime waiting.
struct StopOnDrop(DaemonState);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stopping.store(true, Ordering::Release);
    }
}

/// The Cable identity tools answer to exists only once the endpoint knows the provider Session.
#[cfg(feature = "cables-mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_session_is_bound_to_its_cable_endpoint_when_its_stream_reports_it() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(root.path(), "", &agent("bind", "", ""));
    let _stop = StopOnDrop(state.clone());
    start_mission_by_id(&state, "mission-1").await;
    wait_for_session(&state, "mission-1", "stream-bind").await;
    wait_until("the Cable endpoint is bound to the Session", || {
        state
            .sessions
            .lock()
            .unwrap()
            .get("mission-1")
            .and_then(|operation| operation.cables_mcp.as_ref())
            .and_then(cables_mcp::Endpoint::bound_session)
            .as_deref()
            == Some("stream-bind")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_is_accepted_before_the_stream_reports_its_session() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(root.path(), "", &agent("resume", "", ""));
    let _stop = StopOnDrop(state.clone());
    start_mission_by_id(&state, "mission-1").await;
    wait_for_session(&state, "mission-1", "stream-resume").await;
    state
        .missions
        .lock()
        .unwrap()
        .transition("mission-1", MissionState::Blocked)
        .unwrap();

    let (status, Json(started)) = resume(&state, "mission-1", Some("go on"))
        .await
        .unwrap_or_else(|error| panic!("the resume was refused: {:?}", error.1.0.error));
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        started.provider_session_id.as_ref().map(SessionId::as_str),
        Some("stream-resume")
    );
    wait_for_state(&state, "mission-1", MissionState::Done).await;
    assert_eq!(log_of("resume").resumed, vec![Some("go on".to_owned())]);
}

/// With no message Claude Code writes nothing at all: the resume cannot wait for its stream, and
/// the first message starts it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_with_no_message_runs_once_a_message_reaches_the_silent_harness() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(root.path(), "", &agent("silent", "", ""));
    let _stop = StopOnDrop(state.clone());
    start_mission_by_id(&state, "mission-1").await;
    wait_for_session(&state, "mission-1", "stream-silent").await;
    state
        .missions
        .lock()
        .unwrap()
        .transition("mission-1", MissionState::Blocked)
        .unwrap();
    let _ = resume(&state, "mission-1", None)
        .await
        .unwrap_or_else(|error| panic!("the resume was refused: {:?}", error.1.0.error));
    assert_eq!(
        mission_state(&state, "mission-1").state,
        MissionState::Working
    );
    send_message(
        State(state.clone()),
        axum::extract::Path("mission-1".to_owned()),
        Json(SendMessage {
            text: "now".to_owned(),
        }),
    )
    .await
    .unwrap_or_else(|error| panic!("the message was refused: {:?}", error.1.0.error));
    wait_for_state(&state, "mission-1", MissionState::Done).await;
}

/// A harness that resumes another Session than the one asked for is refused when it says so: no
/// final state, nothing recorded, and the harness stopped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_that_reports_another_session_is_refused_when_it_appears() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(
        root.path(),
        "",
        &agent("other", "MOCK_STREAM_RESUMES_AS = \"someone-else\"\n", ""),
    );
    let _stop = StopOnDrop(state.clone());
    start_mission_by_id(&state, "mission-1").await;
    wait_for_session(&state, "mission-1", "stream-other").await;
    state
        .missions
        .lock()
        .unwrap()
        .transition("mission-1", MissionState::Blocked)
        .unwrap();
    let _ = resume(&state, "mission-1", Some("go on"))
        .await
        .unwrap_or_else(|error| panic!("the resume was refused early: {:?}", error.1.0.error));
    wait_for_state(&state, "mission-1", MissionState::Blocked).await;
    wait_until("the refused harness is stopped", || {
        !has_live_session(&state, "mission-1")
    })
    .await;
    let mission = mission_state(&state, "mission-1");
    assert_eq!(mission.state, MissionState::Blocked);
    assert_eq!(
        mission
            .session_ids
            .iter()
            .map(SessionId::as_str)
            .collect::<Vec<_>>(),
        ["stream-other"],
        "the other Session is not the Mission's"
    );
    // The Mission can be resumed again.
    assert!(log_of("other").closes >= 1);
}

async fn interrupt(state: &DaemonState, mission_id: &str) -> Mission {
    let Json(mission) = interrupt_mission(
        State(state.clone()),
        axum::extract::Path(mission_id.to_owned()),
    )
    .await
    .unwrap_or_else(|error| panic!("the interrupt was refused: {:?}", error.1.0.error));
    mission
}

fn stalling_state(root: &Path, key: &str) -> DaemonState {
    let mut state = test_state(
        root,
        "workspace = { provider = \"mock-safehouse\" }\n",
        &agent(key, "", "[agent.007.limits]\nstall_timeout = 1\n"),
    );
    state.station = StationConfig::default()
        .with_terminal("mock-terminal")
        .with_state_dir(root.join("state"));
    state
}

#[cfg(feature = "publisher")]
mod controls_on_a_stream {
    use ai6_diptel::{ControlAction, ControlId, MissionControl, MissionId, OperationId};

    use super::*;
    use crate::station::publisher::ControlOutcome;

    fn control(action: ControlAction) -> MissionControl {
        MissionControl::new(
            ControlId::new(format!("control-{}", action.as_str())).unwrap(),
            OperationId::new("G").unwrap(),
            MissionId::new("mission-1").unwrap(),
            action,
        )
    }

}
