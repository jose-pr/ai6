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

use super::tests::{mission_state, start_mission_by_id, test_state, wait_for_done, wait_for_state};
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

/// What a test does at a moment of a key's mock, to see a race go one way. It is told the Session
/// ID the mock reports.
type Hook = Arc<dyn Fn(&str) + Send + Sync>;

static ON_RESUME: Mutex<BTreeMap<String, Hook>> = Mutex::new(BTreeMap::new());
static ON_CLOSE: Mutex<BTreeMap<String, Hook>> = Mutex::new(BTreeMap::new());

fn run_hook(hooks: &Mutex<BTreeMap<String, Hook>>, key: &str, session: &str) {
    let hook = hooks.lock().unwrap().get(key).cloned();
    if let Some(hook) = hook {
        hook(session);
    }
}

/// A handler whose Sessions behave as [`StreamSession`] does.
///
/// Its Agent's environment says how: `MOCK_STREAM_KEY` names the log, `MOCK_STREAM_TURN` is what
/// a first turn does after its initialization (`hold` works on, `complete` ends it),
/// `MOCK_STREAM_INTERRUPT` is how an interrupted turn ends (`failed`, `completed`, `silent`, or
/// `late`: silent the first time, then failed), and `MOCK_STREAM_RESUMES_AS` a Session ID a resume
/// reports other than the one asked for.
/// `MOCK_STREAM_RESUME_FAILS` refuses a resume with that text, and `MOCK_STREAM_SESSIONLESS` keeps
/// the harness from ever reporting a Session.
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
        run_hook(
            &ON_RESUME,
            &setting(&request.environment, "MOCK_STREAM_KEY", "default"),
            request.session_id.as_str(),
        );
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
    sessionless: bool,
    stream: Mutex<Stream>,
}

impl StreamSession {
    fn new(environment: &BTreeMap<String, String>, reports: String, turn: String) -> Self {
        Self {
            key: setting(environment, "MOCK_STREAM_KEY", "default"),
            reports,
            turn,
            on_interrupt: setting(environment, "MOCK_STREAM_INTERRUPT", "failed"),
            sessionless: environment.contains_key("MOCK_STREAM_SESSIONLESS"),
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
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        Ok(self
            .stream
            .lock()
            .unwrap()
            .session_id
            .clone()
            .map(String::from))
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
            let first_line_due = !self.sessionless
                && stream
                    .read_at
                    .is_some_and(|read| read.elapsed() >= FIRST_LINE_AFTER);
            if first_line_due && !stream.initialized {
                stream.initialized = true;
                stream.session_id =
                    Some(SessionId::try_new(self.reports.clone()).map_err(|error| {
                        ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string())
                    })?);
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
        let mut interrupts = 0;
        self.log(|log| {
            log.interrupts += 1;
            interrupts = log.interrupts;
        });
        match self.on_interrupt.as_str() {
            "late" if interrupts == 1 => {}
            "failed" | "late" => self.stream.lock().unwrap().queue.push_back(stream_event(
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
        run_hook(&ON_CLOSE, &self.key, &self.reports);
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
    wait_until(
        "the Mission records the Session its stream reported",
        || {
            mission_state(state, mission_id)
                .session_ids
                .last()
                .map(SessionId::as_str)
                == Some(wanted)
        },
    )
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
    wait_for_done(&state, "mission-1").await;
    assert_eq!(log_of("resume").resumed, vec![Some("go on".to_owned())]);
}

/// Claude Code and Codex do nothing until they read a message, so a resume with none of its own
/// (a restarted Station's, an operator's) tells the harness to go on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_with_no_message_tells_the_silent_harness_to_go_on() {
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
    wait_for_done(&state, "mission-1").await;
    let resumed = log_of("silent").resumed;
    assert!(resumed[0].is_some(), "{resumed:?}");
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

/// A harness answers being stopped with the end of a turn: it must no longer be the Mission's
/// Session by then, or that end reads as the end of the Mission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_harness_is_stopped_only_once_it_is_replaced() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(
        root.path(),
        "",
        &agent(
            "replaced",
            "MOCK_STREAM_RESUMES_AS = \"someone-else\"\n",
            "",
        ),
    );
    let _stop = StopOnDrop(state.clone());
    let still_the_missions = Arc::new(Mutex::new(Vec::new()));
    let (watched, seen) = (state.clone(), still_the_missions.clone());
    ON_CLOSE.lock().unwrap().insert(
        "replaced".to_owned(),
        Arc::new(move |session: &str| {
            if session == "stream-replaced" {
                let first_session_live = watched
                    .sessions
                    .lock()
                    .unwrap()
                    .get("mission-1")
                    .is_some_and(|operation| !operation.resumed);
                seen.lock().unwrap().push(first_session_live);
            }
        }),
    );
    start_mission_by_id(&state, "mission-1").await;
    wait_for_session(&state, "mission-1", "stream-replaced").await;
    state
        .missions
        .lock()
        .unwrap()
        .transition("mission-1", MissionState::Blocked)
        .unwrap();
    let _ = resume(&state, "mission-1", Some("go on"))
        .await
        .unwrap_or_else(|error| panic!("the resume was refused: {:?}", error.1.0.error));
    assert_eq!(*still_the_missions.lock().unwrap(), [false]);
}

/// An operator who resumes before the interrupted turn has ended gets the resume: the old harness
/// ending that turn afterwards does not end the Mission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_before_an_interrupted_turn_ended_goes_on() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(
        root.path(),
        "",
        &agent("slow-end", "MOCK_STREAM_INTERRUPT = \"late\"\n", ""),
    );
    let _stop = StopOnDrop(state.clone());
    start_mission_by_id(&state, "mission-1").await;
    wait_for_session(&state, "mission-1", "stream-slow-end").await;
    assert_eq!(
        interrupt(&state, "mission-1").await.state,
        MissionState::Blocked
    );
    let _ = resume(&state, "mission-1", Some("go on"))
        .await
        .unwrap_or_else(|error| panic!("the resume was refused: {:?}", error.1.0.error));
    wait_for_done(&state, "mission-1").await;
}

/// A pump whose Session is no longer the Mission's has no say in it, however late it gets there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_end_of_a_session_no_longer_the_missions_is_not_the_missions() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(root.path(), "", &agent("stale", "", ""));
    let _stop = StopOnDrop(state.clone());
    start_mission_by_id(&state, "mission-1").await;
    wait_for_session(&state, "mission-1", "stream-stale").await;
    publish_finished_event(
        &state,
        "mission-1",
        stream_event("failed", Some("the old harness ended")),
        None,
    );
    assert_eq!(
        mission_state(&state, "mission-1").state,
        MissionState::Working
    );
    while let Some(event) = pop_event(&state, "mission-1") {
        assert_ne!(event.kind, "failed");
    }
}

/// A harness whose turn ends while its resume is under way settles the Mission: the resume is
/// refused and the harness it started stopped, or it would run on with no pump to read it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_ends_during_its_resume_takes_the_resume_with_it() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(root.path(), "", &agent("ending", "", ""));
    let _stop = StopOnDrop(state.clone());
    let during = state.clone();
    ON_RESUME.lock().unwrap().insert(
        "ending".to_owned(),
        Arc::new(move |_: &str| {
            let session = during
                .sessions
                .lock()
                .unwrap()
                .get("mission-1")
                .map(|operation| operation.session.clone())
                .unwrap();
            session.lock().unwrap().interrupt().unwrap();
            let deadline = Instant::now() + Duration::from_secs(60);
            while has_live_session(&during, "mission-1") {
                assert!(Instant::now() < deadline, "the turn never ended");
                std::thread::sleep(Duration::from_millis(5));
            }
        }),
    );
    start_mission_by_id(&state, "mission-1").await;
    wait_for_session(&state, "mission-1", "stream-ending").await;
    state
        .missions
        .lock()
        .unwrap()
        .transition("mission-1", MissionState::Blocked)
        .unwrap();
    assert!(resume(&state, "mission-1", Some("go on")).await.is_err());
    assert!(!has_live_session(&state, "mission-1"));
    wait_for_state(&state, "mission-1", MissionState::Failed).await;
    wait_until("both harnesses are closed", || log_of("ending").closes >= 2).await;
}

/// A Mission that ends while its resume is under way leaves no harness behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mission_that_ends_during_its_resume_leaves_no_harness() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(root.path(), "", &agent("abandoned", "", ""));
    let _stop = StopOnDrop(state.clone());
    let during = state.clone();
    ON_RESUME.lock().unwrap().insert(
        "abandoned".to_owned(),
        Arc::new(move |_: &str| {
            during
                .missions
                .lock()
                .unwrap()
                .transition("mission-1", MissionState::Abandoned)
                .unwrap();
        }),
    );
    start_mission_by_id(&state, "mission-1").await;
    wait_for_session(&state, "mission-1", "stream-abandoned").await;
    interrupt(&state, "mission-1").await;
    wait_until("the interrupted turn's end closes the harness", || {
        !has_live_session(&state, "mission-1")
    })
    .await;
    assert!(resume(&state, "mission-1", Some("go on")).await.is_err());
    assert!(!has_live_session(&state, "mission-1"));
    wait_until("the resumed harness is closed", || {
        log_of("abandoned").closes >= 2
    })
    .await;
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

/// Claude Code answers an interrupt with an error result and a clean turn may end with an
/// ordinary one; neither ends the Mission, which stays blocked until it is resumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupted_turn_leaves_the_mission_blocked_however_it_ends() {
    for ending in ["failed", "completed"] {
        let root = tempfile::tempdir().unwrap();
        let key = format!("interrupt-{ending}");
        let state = test_state(
            root.path(),
            "",
            &agent(&key, &format!("MOCK_STREAM_INTERRUPT = \"{ending}\"\n"), ""),
        );
        let _stop = StopOnDrop(state.clone());
        start_mission_by_id(&state, "mission-1").await;
        wait_for_session(&state, "mission-1", &format!("stream-{key}")).await;
        assert_eq!(
            interrupt(&state, "mission-1").await.state,
            MissionState::Blocked
        );
        wait_until("the interrupted turn's end closes the harness", || {
            !has_live_session(&state, "mission-1")
                || mission_state(&state, "mission-1").state != MissionState::Blocked
        })
        .await;
        let mission = mission_state(&state, "mission-1");
        assert_eq!(mission.state, MissionState::Blocked, "ended by {ending}");
        assert!(!has_live_session(&state, "mission-1"), "ended by {ending}");

        let (status, _) = resume(&state, "mission-1", Some("go on"))
            .await
            .unwrap_or_else(|error| panic!("the resume was refused: {:?}", error.1.0.error));
        assert_eq!(status, StatusCode::ACCEPTED);
        wait_for_done(&state, "mission-1").await;
    }
}

/// With no Session ID there is nothing to resume: an interrupted turn that ends before the
/// harness reported one must not leave the Mission waiting for a resume that can never run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupted_turn_with_no_session_to_resume_is_not_left_waiting() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(
        root.path(),
        "",
        &agent("nosession", "MOCK_STREAM_SESSIONLESS = \"1\"\n", ""),
    );
    let _stop = StopOnDrop(state.clone());
    start_mission_by_id(&state, "mission-1").await;
    assert!(mission_state(&state, "mission-1").session_ids.is_empty());
    interrupt(&state, "mission-1").await;
    wait_for_state(&state, "mission-1", MissionState::Failed).await;
}

/// A harness with no running turn says nothing to an interrupt: the Mission waits blocked, not
/// judged a stall, and a message takes it on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupt_answered_by_silence_leaves_the_mission_waiting() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(
        root.path(),
        "",
        &agent(
            "quiet",
            "MOCK_STREAM_INTERRUPT = \"silent\"\n",
            "[agent.007.limits]\nstall_timeout = 1\n",
        ),
    );
    let _stop = StopOnDrop(state.clone());
    start_mission_by_id(&state, "mission-1").await;
    wait_for_session(&state, "mission-1", "stream-quiet").await;
    interrupt(&state, "mission-1").await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(
        mission_state(&state, "mission-1").state,
        MissionState::Blocked,
        "waiting for an operator is not a stall"
    );
    send_message(
        State(state.clone()),
        axum::extract::Path("mission-1".to_owned()),
        Json(SendMessage {
            text: "go on".to_owned(),
        }),
    )
    .await
    .unwrap_or_else(|error| panic!("the message was refused: {:?}", error.1.0.error));
    wait_for_done(&state, "mission-1").await;
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

/// A stall is not an end: the safe house its resume starts in stays until the Mission ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_mission_keeps_its_safe_house_until_it_ends() {
    let root = tempfile::tempdir().unwrap();
    let state = stalling_state(root.path(), "stall");
    let _stop = StopOnDrop(state.clone());
    start_mission_by_id(&state, "mission-1").await;
    wait_for_state(&state, "mission-1", MissionState::Stalled).await;
    wait_until("the stalled harness is closed", || {
        !has_live_session(&state, "mission-1")
    })
    .await;
    let report = mission_state(&state, "mission-1").safe_house.unwrap();
    assert!(report.kept, "{report:?}");
    assert!(Path::new(&report.path).is_dir(), "{report:?}");

    let _ = resume(&state, "mission-1", Some("go on"))
        .await
        .unwrap_or_else(|error| panic!("the resume was refused: {:?}", error.1.0.error));
    wait_for_done(&state, "mission-1").await;
    let report_after = mission_state(&state, "mission-1").safe_house.unwrap();
    assert!(!report_after.kept, "the end of the Mission finishes it");
    assert!(!Path::new(&report.path).exists());
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

    /// Cancelling a Mission with no live Session still ends what it left behind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_a_stalled_mission_finishes_its_safe_house() {
        let root = tempfile::tempdir().unwrap();
        let state = stalling_state(root.path(), "stall-cancel");
        let _stop = StopOnDrop(state.clone());
        start_mission_by_id(&state, "mission-1").await;
        wait_for_state(&state, "mission-1", MissionState::Stalled).await;
        wait_until("the stalled harness is closed", || {
            !has_live_session(&state, "mission-1")
        })
        .await;
        let report = mission_state(&state, "mission-1").safe_house.unwrap();
        assert!(Path::new(&report.path).is_dir());

        let outcome = super::super::controls::apply(&state, &control(ControlAction::Cancel)).await;
        assert_eq!(outcome, ControlOutcome::applied("abandoned"));
        assert!(!mission_state(&state, "mission-1").safe_house.unwrap().kept);
        assert!(!Path::new(&report.path).exists());
    }

    /// What is wrong reaches the operator; a credential in the provider's words does not.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refused_resume_says_why_and_holds_no_secret() {
        for (key, failure, wanted, hidden) in [
            (
                "cause",
                "cannot start claude: The directory name is invalid. (os error 267)",
                "The directory name is invalid",
                None,
            ),
            (
                "leak",
                "login failed for sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "refused",
                Some("sk-ant-"),
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let state = test_state(
                root.path(),
                "",
                &agent(
                    key,
                    &format!("MOCK_STREAM_RESUME_FAILS = \"{failure}\"\n"),
                    "",
                ),
            );
            let _stop = StopOnDrop(state.clone());
            start_mission_by_id(&state, "mission-1").await;
            wait_for_session(&state, "mission-1", &format!("stream-{key}")).await;
            state
                .missions
                .lock()
                .unwrap()
                .transition("mission-1", MissionState::Blocked)
                .unwrap();
            let outcome =
                super::super::controls::apply(&state, &control(ControlAction::Resume)).await;
            let text = format!("{outcome:?}");
            assert!(outcome != ControlOutcome::applied("working"), "{text}");
            assert!(text.contains(wanted), "{text}");
            assert!(
                key != "cause" || text.contains("directory name"),
                "the cause is hidden: {text}"
            );
            if let Some(hidden) = hidden {
                assert!(!text.contains(hidden), "{text}");
            }
        }
    }
}
