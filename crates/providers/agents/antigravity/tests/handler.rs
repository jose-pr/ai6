//! The handler against a fake CLI that replays recorded agy 1.4.12 lines and behaves as it was
//! measured to: `init` at start before any message, a recorded turn per message, exit code 3 after
//! an `ERROR` result, a new conversation for an ID it does not hold.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ai6::directorates::m::identity::{MissionId, SessionId};
use ai6::directorates::q::terminal::{
    AgentTerminal, ExitInfo, READ_TIMED_OUT, Spec, TerminalSession,
};
use ai6::directorates::section_00::agent::{
    AgentHandler, AgentResume, AgentStart, AssistantMode, AssistantNetwork, McpServerRef,
};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Status, ToValue};
use ai6::{AgentEvent, AgentMessage, AgentSession};
use ai6_agent_antigravity::{AntigravityConfig, AntigravityHandler};
use serde_json::{Value, json};

const CONV: &str = "00000000-0000-4000-8000-00000000c0de";

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/agy-1.4.12/", $name))
    };
}

/// What the fake does for each `{"event":"user"}` line.
struct Turn {
    lines: String,
    stderr: String,
    exit: Option<i64>,
}

fn turn(lines: &str) -> Turn {
    Turn {
        lines: lines.to_owned(),
        stderr: String::new(),
        exit: None,
    }
}

/// What the fake found when it was started.
#[derive(Debug, Clone)]
struct Launch {
    spec: Spec,
    session_dir: PathBuf,
    agent: Option<String>,
    mcp: Option<Value>,
    hooks: Option<Value>,
    mode_private: bool,
}

impl Launch {
    fn settings(&self) -> Value {
        let at = self
            .spec
            .argv
            .iter()
            .position(|argument| argument == "--settings")
            .expect("--settings");
        serde_json::from_str(&self.spec.argv[at + 1]).unwrap()
    }

    fn rules(&self, list: &str) -> Vec<String> {
        self.settings()["permissions"][list]
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| rule.as_str().unwrap().to_owned())
            .collect()
    }

    fn has(&self, argument: &str) -> bool {
        self.spec.argv.iter().any(|found| found == argument)
    }

    /// The `tools:` line of the agent file, as a list.
    fn tools(&self) -> Vec<String> {
        let agent = self.agent.as_deref().expect("an agent file");
        let line = agent
            .lines()
            .find_map(|line| line.strip_prefix("tools: "))
            .expect("a tools line");
        line.trim_matches(['[', ']'])
            .split(", ")
            .filter(|tool| !tool.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

#[derive(Default)]
struct Shared {
    launches: Vec<Launch>,
    known: Vec<String>,
    turns: VecDeque<Turn>,
    received: Vec<String>,
    kills: usize,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<Shared>>);

impl Fake {
    fn with(turns: Vec<Turn>) -> Self {
        let fake = Self::default();
        {
            let mut shared = fake.0.lock().unwrap();
            shared.turns = turns.into();
            shared.known = vec![CONV.to_owned()];
        }
        fake
    }

    fn launch(&self) -> Launch {
        self.0.lock().unwrap().launches.last().unwrap().clone()
    }

    fn received(&self) -> Vec<String> {
        self.0.lock().unwrap().received.clone()
    }

    fn kills(&self) -> usize {
        self.0.lock().unwrap().kills
    }
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| serde_json::from_str(&text).unwrap())
}

impl AgentTerminal for Fake {
    fn spawn_private(
        &self,
        value: &ai6::guatiao::Value,
    ) -> Result<Object<dyn TerminalSession>, ProviderError> {
        self.spawn(value)
    }

    fn spawn(
        &self,
        value: &ai6::guatiao::Value,
    ) -> Result<Object<dyn TerminalSession>, ProviderError> {
        let spec = Spec::from_value(value).unwrap();
        let at = spec.argv.iter().position(|a| a == "--add-dir").unwrap();
        let session_dir = PathBuf::from(&spec.argv[at + 1]);
        let dot = session_dir.join(".agents");
        let agent = spec
            .argv
            .iter()
            .find_map(|a| a.strip_prefix("--agent="))
            .map(|name| {
                std::fs::read_to_string(dot.join("agents").join(name).join("agent.md")).unwrap()
            });
        #[cfg(unix)]
        let mode_private = {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::fs::metadata(&session_dir)
                .unwrap()
                .permissions()
                .mode();
            let config = std::fs::metadata(dot.join("mcp_config.json"))
                .map_or(0o600, |meta| meta.permissions().mode());
            dir & 0o077 == 0 && config & 0o077 == 0
        };
        #[cfg(not(unix))]
        let mode_private = true;
        let wanted = spec
            .argv
            .iter()
            .find_map(|a| a.strip_prefix("--conversation="))
            .map(str::to_owned);
        let mut shared = self.0.lock().unwrap();
        shared.launches.push(Launch {
            spec,
            session_dir,
            agent,
            mcp: read_json(&dot.join("mcp_config.json")),
            hooks: read_json(&dot.join("hooks.json")),
            mode_private,
        });
        let mut output: Vec<u8> = Vec::new();
        match wanted {
            Some(id) if !shared.known.contains(&id) => {
                output.extend(fixture!("resume_unknown_init.jsonl").bytes());
                output.extend(fixture!("resume_unknown.stderr").bytes());
            }
            _ => output.extend(fixture!("init.jsonl").bytes()),
        }
        Ok(FakeSession {
            shared: self.0.clone(),
            state: Mutex::new(Process {
                output: output.into(),
                exit: None,
                killed: false,
            }),
        }
        .into_object())
    }
}

struct Process {
    output: VecDeque<u8>,
    exit: Option<i64>,
    killed: bool,
}

struct FakeSession {
    shared: Arc<Mutex<Shared>>,
    state: Mutex<Process>,
}

impl TerminalSession for FakeSession {
    fn pid(&self) -> i64 {
        1
    }

    fn read(&self, dst: &mut [u8], timeout_ms: i64) -> Result<i64, ProviderError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(0) as u64);
        loop {
            {
                let mut state = self.state.lock().unwrap();
                if !state.output.is_empty() {
                    let count = dst.len().min(state.output.len());
                    for byte in &mut dst[..count] {
                        *byte = state.output.pop_front().unwrap();
                    }
                    return Ok(count as i64);
                }
                if state.killed || state.exit.is_some() {
                    return Ok(0);
                }
            }
            if Instant::now() >= deadline {
                return Ok(READ_TIMED_OUT);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn write(&self, data: &[u8]) -> Result<(), ProviderError> {
        let text = String::from_utf8(data.to_vec()).unwrap();
        let mut shared = self.shared.lock().unwrap();
        for line in text.lines() {
            let value: Value = serde_json::from_str(line).unwrap();
            assert_eq!(value["event"], "user", "agy's input shape: {line}");
            shared
                .received
                .push(value["message"]["content"].as_str().unwrap().to_owned());
            let next = shared.turns.pop_front().expect("a turn for the message");
            let mut state = self.state.lock().unwrap();
            state.output.extend(next.lines.bytes());
            state.output.extend(next.stderr.bytes());
            state.exit = next.exit;
        }
        Ok(())
    }

    fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
        Ok(())
    }

    fn kill(&self) -> Result<(), ProviderError> {
        self.shared.lock().unwrap().kills += 1;
        let mut state = self.state.lock().unwrap();
        state.killed = true;
        state.output.clear();
        Ok(())
    }

    fn exit_status(&self, _timeout_ms: i64) -> Result<ai6::guatiao::Value, ProviderError> {
        let state = self.state.lock().unwrap();
        let info = if state.killed {
            ExitInfo {
                running: false,
                code: None,
                signal: Some(9),
            }
        } else if let Some(code) = state.exit.filter(|_| state.output.is_empty()) {
            ExitInfo {
                running: false,
                code: Some(code),
                signal: None,
            }
        } else {
            ExitInfo {
                running: true,
                ..ExitInfo::default()
            }
        };
        info.to_value()
            .map_err(|error| ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string()))
    }
}

fn handler(settings: &str) -> AntigravityHandler {
    AntigravityHandler::try_from(AntigravityConfig {
        settings_toml: settings.to_owned(),
        ..AntigravityConfig::default()
    })
    .unwrap()
}

/// A working directory beside which the Session's own directory can be made.
fn checkout() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    let wd = root.path().join("checkout");
    std::fs::create_dir(&wd).unwrap();
    let text = wd.to_string_lossy().into_owned();
    (root, text)
}

fn start_request(wd: &str, clearance: &str) -> AgentStart {
    AgentStart {
        mission_id: MissionId::try_new("mission-agy").unwrap(),
        agent_id: "007".to_owned(),
        prompt: "the task".to_owned(),
        initial_context: vec!["standing context".to_owned()],
        working_directory: wd.to_owned(),
        clearance: clearance.to_owned(),
        environment: BTreeMap::new(),
        model: Some("gemini-3.8-flash-low".to_owned()),
        effort: None,
        max_cost_micro_usd: None,
        project_settings: false,
        project_mcp: false,
        answer_only: false,
        assistant: None,
        overlays: Default::default(),
    }
}

fn resume_request(wd: &str, id: &str, clearance: &str) -> AgentResume {
    AgentResume {
        mission_id: MissionId::try_new("mission-agy").unwrap(),
        session_id: SessionId::try_new(id).unwrap(),
        message: Some("go on".to_owned()),
        working_directory: wd.to_owned(),
        clearance: clearance.to_owned(),
        environment: BTreeMap::new(),
        model: Some("gemini-3.8-flash-low".to_owned()),
        effort: None,
        max_cost_micro_usd: None,
        project_settings: false,
        project_mcp: false,
        answer_only: false,
        assistant: None,
        initial_context: Vec::new(),
        overlays: Default::default(),
    }
}

fn moneypenny() -> AssistantMode {
    AssistantMode {
        mcp_servers: vec![McpServerRef {
            name: "moneypenny".to_owned(),
            env_prefix: "AI6_MONEYPENNY_MCP".to_owned(),
        }],
        interpreters: Vec::new(),
        unsandboxed: false,
        skills: Vec::new(),
        skills_dir: None,
        skill_commands: false,
        network: AssistantNetwork::default(),
        files: Vec::new(),
        read_only: Vec::new(),
        context_file: None,
        station_paths: Vec::new(),
    }
}

fn endpoint(prefix: &str) -> [(String, String); 2] {
    [
        (
            format!("{prefix}_URL"),
            format!("http://127.0.0.1:1/{prefix}"),
        ),
        (format!("{prefix}_TOKEN"), format!("token-of-{prefix}")),
    ]
}

/// Every event up to and including the first of `until`, however slow the machine.
fn events_until(session: &Object<dyn AgentSession>, until: &[&str]) -> Vec<AgentEvent> {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut found = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "no {until:?} in {found:?}");
        if let Some(event) = session.next_event(50).unwrap() {
            let done = until.contains(&event.kind.as_str());
            found.push(event);
            if done {
                return found;
            }
        }
    }
}

fn kinds(events: &[AgentEvent]) -> Vec<&str> {
    events.iter().map(|event| event.kind.as_str()).collect()
}

fn started(fake: &Fake, request: AgentStart, settings: &str) -> Object<dyn AgentSession> {
    handler(settings)
        .start(request, fake.clone().into_object())
        .unwrap()
}

#[test]
fn a_sessions_id_is_read_from_its_stream_and_not_before() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let session = started(&fake, start_request(&wd, "field"), "");
    assert_eq!(session.session_id().unwrap(), None, "nothing read yet");
    events_until(&session, &["init"]);
    assert_eq!(
        session.session_id().unwrap().unwrap().as_str(),
        CONV,
        "known from init, before any turn ends"
    );
    let events = events_until(&session, &["completed"]);
    assert_eq!(events.last().unwrap().text.as_deref(), Some("OK.\n"));
    let said: Vec<_> = events.iter().filter(|event| event.kind == "text").collect();
    assert_eq!(said.len(), 1, "a response is one event, its deltas joined");
    assert_eq!(said[0].text.as_deref(), Some("OK.\n"));
    assert_eq!(fake.received(), ["the task"]);
    assert!(
        !fake
            .launch()
            .spec
            .argv
            .iter()
            .any(|a| a.starts_with("--conversation"))
    );
}

#[test]
fn a_resume_holds_its_message_until_the_stream_names_the_conversation_asked_for() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let session = handler("")
        .resume(
            resume_request(&wd, CONV, "field"),
            fake.clone().into_object(),
        )
        .unwrap();
    assert!(fake.received().is_empty(), "held until init");
    let launch = fake.launch();
    assert!(launch.has(&format!("--conversation={CONV}")));
    assert!(launch.agent.is_none(), "a conversation keeps its agent");
    events_until(&session, &["completed"]);
    assert_eq!(fake.received(), ["go on"]);
}

#[test]
fn a_resume_that_got_another_conversation_sends_nothing_and_says_so() {
    let (_root, wd) = checkout();
    let fake = Fake::with(Vec::new());
    let session = handler("")
        .resume(
            resume_request(&wd, "00000000-0000-4000-8000-000000000000", "field"),
            fake.clone().into_object(),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let error = loop {
        assert!(Instant::now() < deadline);
        match session.next_event(50) {
            Err(error) => break error.to_string(),
            Ok(_) => continue,
        }
    };
    assert!(error.contains("different Session"), "{error}");
    assert!(
        fake.received().is_empty(),
        "no message reaches a new conversation"
    );
    assert_eq!(fake.kills(), 1);
}

#[test]
fn a_quota_refusal_is_a_limit_and_the_exit_after_it_adds_nothing() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![Turn {
        lines: fixture!("turn_quota_429.jsonl").to_owned(),
        stderr: fixture!("turn_quota_429.stderr").to_owned(),
        exit: Some(3),
    }]);
    let session = started(&fake, start_request(&wd, "field"), "");
    let events = events_until(&session, &["limit", "failed", "completed"]);
    let last = events.last().unwrap();
    assert_eq!(last.kind, "limit");
    assert!(last.limit.is_some());
    assert!(last.text.as_deref().unwrap().contains("RESOURCE_EXHAUSTED"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(event) = session.next_event(20).unwrap() {
            assert_eq!(
                event.kind, "output",
                "only agy's stderr may follow: {event:?}"
            );
        }
    }
}

#[test]
fn a_429_with_no_details_is_a_limit_too() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![Turn {
        exit: Some(3),
        ..turn(fixture!("turn_plain_429.jsonl"))
    }]);
    let session = started(&fake, start_request(&wd, "field"), "");
    assert_eq!(
        events_until(&session, &["limit", "failed"])
            .last()
            .unwrap()
            .kind,
        "limit"
    );
}

#[test]
fn the_words_for_a_limit_are_a_setting() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![Turn {
        exit: Some(3),
        ..turn(fixture!("turn_plain_429.jsonl"))
    }]);
    let session = started(
        &fake,
        start_request(&wd, "field"),
        "limit_phrases = [\"no such words\"]\n",
    );
    assert_eq!(
        events_until(&session, &["limit", "failed"])
            .last()
            .unwrap()
            .kind,
        "failed"
    );
}

#[test]
fn the_quota_words_of_the_binary_are_a_limit_though_unseen_in_a_run() {
    let (_root, wd) = checkout();
    let line = json!({"event": "result", "result": {"conversation_id": CONV, "status": "ERROR",
        "response": "", "error": "You have exhausted your quota on this model."}});
    let fake = Fake::with(vec![Turn {
        exit: Some(3),
        ..turn(&format!("{line}\n"))
    }]);
    let session = started(&fake, start_request(&wd, "field"), "");
    assert_eq!(
        events_until(&session, &["limit", "failed"])
            .last()
            .unwrap()
            .kind,
        "limit"
    );
}

#[test]
fn a_turn_that_cannot_run_is_a_failure_with_agys_words() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![Turn {
        exit: Some(3),
        ..turn(fixture!("turn_error.jsonl"))
    }]);
    let session = started(&fake, start_request(&wd, "field"), "");
    let events = events_until(&session, &["limit", "failed", "completed"]);
    let last = events.last().unwrap();
    assert_eq!(last.kind, "failed");
    assert!(
        last.text
            .as_deref()
            .unwrap()
            .contains("failed to construct executor")
    );
}

#[test]
fn an_exit_without_a_result_is_a_failure_that_quotes_the_tail() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![Turn {
        lines: String::new(),
        stderr: "error: authentication required\n".to_owned(),
        exit: Some(1),
    }]);
    let session = started(&fake, start_request(&wd, "field"), "");
    let events = events_until(&session, &["failed"]);
    let text = events.last().unwrap().text.clone().unwrap();
    assert!(text.contains("exited with code 1"), "{text}");
    assert!(text.contains("authentication required"), "{text}");
}

#[test]
fn a_soft_denied_tool_is_reported_before_the_turn_that_still_says_success() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![Turn {
        stderr: fixture!("turn_soft_denied.stderr").to_owned(),
        ..turn(fixture!("turn_soft_denied.jsonl"))
    }]);
    let session = started(&fake, start_request(&wd, "field"), "");
    let events = events_until(&session, &["completed"]);
    let denied = events.iter().find(|event| event.kind == "denied").unwrap();
    assert_eq!(denied.text.as_deref(), Some("run_command"));
    assert!(denied.tools.is_empty(), "it did not run");
    let at = |kind: &str| events.iter().position(|event| event.kind == kind).unwrap();
    assert!(at("denied") < at("completed"), "{:?}", kinds(&events));
}

#[test]
fn a_refusal_by_a_deny_rule_or_a_hook_is_a_denial_though_agy_does_not_list_it() {
    for (lines, tool) in [
        (
            fixture!("turn_deny_rule.jsonl"),
            "run_command, read_file, write_file, edit_file",
        ),
        (fixture!("turn_hook_denied.jsonl"), "run_command"),
        (fixture!("turn_hook_failed.jsonl"), "run_command"),
    ] {
        let (_root, wd) = checkout();
        let fake = Fake::with(vec![turn(lines)]);
        let session = started(&fake, start_request(&wd, "field"), "");
        let events = events_until(&session, &["completed"]);
        let denied = events.iter().find(|event| event.kind == "denied");
        assert_eq!(denied.and_then(|event| event.text.as_deref()), Some(tool));
    }
}

#[test]
fn field_clearance_gets_commands_and_edits_inside_the_checkout_only() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = started(&fake, start_request(&wd, "field"), "");
    let launch = fake.launch();
    assert!(launch.tools().iter().any(|tool| tool == "run_command"));
    assert!(launch.tools().iter().any(|tool| tool == "write_file"));
    assert!(
        !launch.tools().iter().any(|tool| tool == "search_web"),
        "no rule gates it"
    );
    let allow = launch.rules("allow");
    assert!(allow.contains(&"command(*)".to_owned()));
    assert!(allow.contains(&format!("write_file({wd})")));
    let deny = launch.rules("deny");
    for protected in [".git", ".agents", ".agent"] {
        let path = Path::new(&wd).join(protected);
        assert!(
            deny.contains(&format!("write_file({})", path.display())),
            "{protected}: {deny:?}"
        );
    }
}

#[test]
fn desk_clearance_has_no_command_tool_and_a_rule_against_commands() {
    for clearance in ["desk", "", "someone-elses-word"] {
        let (_root, wd) = checkout();
        let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
        let _session = started(&fake, start_request(&wd, clearance), "");
        let launch = fake.launch();
        assert!(
            !launch.tools().iter().any(|tool| tool == "run_command"),
            "{clearance}"
        );
        assert!(launch.tools().iter().any(|tool| tool == "write_file"));
        assert!(launch.rules("deny").contains(&"command(*)".to_owned()));
        assert!(!launch.rules("allow").contains(&"command(*)".to_owned()));
    }
}

#[test]
fn a_resume_at_desk_of_a_field_conversation_denies_commands_by_rule() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = handler("")
        .resume(
            resume_request(&wd, CONV, "desk"),
            fake.clone().into_object(),
        )
        .unwrap();
    assert!(
        fake.launch()
            .rules("deny")
            .contains(&"command(*)".to_owned())
    );
}

#[test]
fn allowed_tools_can_lift_desk_as_a_reported_weakening() {
    let built = handler("allowed_tools = [\"command(git status)\", \"search_web\"]\n");
    assert_eq!(
        built.weakenings().unwrap(),
        [
            "allowed_tools adds command(git status) to what its desk Sessions may call",
            "allowed_tools adds search_web to what its field Sessions may call",
        ]
    );
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = built
        .start(start_request(&wd, "desk"), fake.clone().into_object())
        .unwrap();
    let launch = fake.launch();
    assert!(launch.tools().iter().any(|tool| tool == "run_command"));
    assert!(launch.tools().iter().any(|tool| tool == "search_web"));
    assert!(
        launch
            .rules("allow")
            .contains(&"command(git status)".to_owned())
    );
    assert!(!launch.rules("deny").contains(&"command(*)".to_owned()));
    assert!(handler("").weakenings().unwrap().is_empty());
    assert!(
        handler("allowed_tools = [\"search_web\"]\ndenied_tools = [\"search_web\"]\n")
            .weakenings()
            .unwrap()
            .is_empty(),
        "denied too, it allows nothing"
    );
}

#[test]
fn settings_that_are_unknown_malformed_or_decide_permissions_are_refused() {
    for settings in [
        "no_such_key = 1\n",
        "allowed_tools = [\"Not A Rule\"]\n",
        "denied_tools = [\"command(git\"]\n",
        "[settings]\ntoolPermission = \"always-proceed\"\n",
        "[settings]\npermissions = {}\n",
    ] {
        let built = AntigravityHandler::try_from(AntigravityConfig {
            settings_toml: settings.to_owned(),
            ..AntigravityConfig::default()
        });
        assert!(built.is_err(), "{settings}");
    }
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let (_root, wd) = checkout();
    let _session = started(
        &fake,
        start_request(&wd, "field"),
        "[settings]\nmodelProvider = \"gemini\"\n",
    );
    assert_eq!(fake.launch().settings()["modelProvider"], "gemini");
}

#[test]
fn moneypennys_session_reads_and_calls_her_own_mcp_tools_only() {
    let (_root, wd) = checkout();
    let mut request = start_request(&wd, "desk");
    request.assistant = Some(moneypenny());
    request.environment.extend(endpoint("AI6_MONEYPENNY_MCP"));
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let session = started(&fake, request, "allowed_tools = [\"search_web\"]\n");
    let launch = fake.launch();
    assert_eq!(launch.tools(), ["read_file"], "settings do not widen her");
    assert_eq!(launch.rules("allow"), ["mcp(moneypenny/*)"]);
    let deny = launch.rules("deny");
    assert!(deny.contains(&"command(*)".to_owned()));
    assert!(deny.contains(&"write_file(*)".to_owned()));
    assert!(deny.contains(&format!("read_file({})", launch.session_dir.display())));
    assert_eq!(launch.hooks, None, "no green ink hook: she has no command");
    let events = events_until(&session, &["completed"]);
    assert!(!kinds(&events).contains(&"failed"));
}

fn tool_step(tool: &str, parameters: Value) -> String {
    let line = json!({"event": "step_update", "step_update": {"conversation_id": CONV,
        "step_index": 2, "state": "ACTIVE", "step_type": "tool", "tool_name": tool,
        "tool_info": {"name": tool, "parameters": parameters}}});
    format!("{line}\n")
}

#[test]
fn a_restricted_session_that_calls_anything_else_is_ended() {
    for (tool, parameters) in [
        ("run_command", json!({"CommandLine": "true"})),
        ("search_web", json!({"query": "x"})),
        (
            "call_mcp_tool",
            json!({"ServerName": "another", "ToolName": "t", "Arguments": {}}),
        ),
        ("call_mcp_tool", json!({"ToolName": "t", "Arguments": {}})),
    ] {
        let (_root, wd) = checkout();
        let mut request = start_request(&wd, "desk");
        request.assistant = Some(moneypenny());
        request.environment.extend(endpoint("AI6_MONEYPENNY_MCP"));
        let fake = Fake::with(vec![turn(&tool_step(tool, parameters))]);
        let session = started(&fake, request, "");
        let events = events_until(&session, &["failed", "completed"]);
        let last = events.last().unwrap();
        assert_eq!(last.kind, "failed", "{tool}");
        assert!(
            last.text.as_deref().unwrap().contains("restricted"),
            "{tool}"
        );
        assert_eq!(fake.kills(), 1, "{tool}");
        assert_eq!(
            session.next_event(10).unwrap(),
            None,
            "nothing read after that"
        );
    }
}

#[test]
fn a_restricted_session_keeps_going_on_its_own_tools() {
    for (tool, parameters) in [
        ("read_file", json!({"Path": "/x"})),
        (
            "call_mcp_tool",
            json!({"ServerName": "moneypenny", "ToolName": "t", "Arguments": {}}),
        ),
    ] {
        let (_root, wd) = checkout();
        let mut request = start_request(&wd, "desk");
        request.assistant = Some(moneypenny());
        request.environment.extend(endpoint("AI6_MONEYPENNY_MCP"));
        let fake = Fake::with(vec![turn(&format!(
            "{}{}",
            tool_step(tool, parameters),
            fixture!("turn_answer.jsonl")
        ))]);
        let session = started(&fake, request, "");
        let events = events_until(&session, &["failed", "completed"]);
        assert_eq!(events.last().unwrap().kind, "completed", "{tool}");
        assert!(events.iter().any(|event| !event.tools.is_empty()));
    }
}

#[test]
fn what_an_assistant_cannot_be_held_to_is_refused_plainly() {
    let refused = |change: fn(&mut AssistantMode)| {
        let (_root, wd) = checkout();
        let mut mode = moneypenny();
        change(&mut mode);
        let mut request = start_request(&wd, "desk");
        request.assistant = Some(mode);
        request.environment.extend(endpoint("AI6_MONEYPENNY_MCP"));
        let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
        handler("")
            .start(request, fake.into_object())
            .err()
            .map(|error| error.to_string())
    };
    assert!(refused(|mode| mode.interpreters = vec!["python3".to_owned()]).is_some());
    assert!(
        refused(|mode| {
            mode.interpreters = vec!["python3".to_owned()];
            mode.unsandboxed = true;
        })
        .is_some()
    );
    assert!(refused(|mode| mode.files = vec!["personal".to_owned()]).is_some());
    assert!(
        refused(|mode| {
            mode.skills = vec!["s".to_owned()];
            mode.skills_dir = Some("/s".to_owned());
        })
        .is_some()
    );
    assert!(refused(|mode| mode.network.policy = "open".to_owned()).is_some());
    assert!(refused(|mode| mode.network.allowed_hosts = vec!["a.example".to_owned()]).is_some());
    assert_eq!(refused(|_| {}), None);
}

#[test]
fn an_answer_only_session_has_no_file_tool_and_no_mcp() {
    let (_root, wd) = checkout();
    let mut request = start_request(&wd, "desk");
    request.answer_only = true;
    request.environment.extend(endpoint("AI6_CABLE_MCP"));
    let fake = Fake::with(vec![turn(&tool_step("list_resources", json!({})))]);
    let session = started(&fake, request, "");
    let launch = fake.launch();
    assert!(launch.tools().is_empty());
    assert!(
        launch
            .agent
            .as_deref()
            .unwrap()
            .contains("inheritMcp: false")
    );
    assert_eq!(launch.mcp, None);
    assert_eq!(launch.hooks, None);
    let deny = launch.rules("deny");
    for rule in ["mcp(*)", "read_file(*)", "write_file(*)", "command(*)"] {
        assert!(deny.contains(&rule.to_owned()), "{rule}");
    }
    let events = events_until(&session, &["failed"]);
    assert!(
        events
            .last()
            .unwrap()
            .text
            .as_deref()
            .unwrap()
            .contains("list_resources")
    );
}

#[test]
fn the_model_name_carries_its_effort_or_the_flag_does() {
    let argv_with = |model: &str, effort: Option<&str>| {
        let (_root, wd) = checkout();
        let mut request = start_request(&wd, "field");
        request.model = Some(model.to_owned());
        request.effort = effort.map(str::to_owned);
        let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
        let _session = started(&fake, request, "");
        fake.launch().spec.argv
    };
    // agy refuses `--model gemini-3.8-flash-low --effort=high`, and an Agent's effort is often the
    // route's too, so the name wins.
    let argv = argv_with("gemini-3.8-flash-low", Some("high"));
    assert!(argv.contains(&"--model=gemini-3.8-flash-low".to_owned()));
    assert!(!argv.iter().any(|a| a.starts_with("--effort")));
    let argv = argv_with("gemini-3.8-flash", Some("low"));
    assert!(argv.contains(&"--model=gemini-3.8-flash".to_owned()));
    assert!(argv.contains(&"--effort=low".to_owned()));
    let argv = argv_with("gpt-oss-120b-medium", Some("high"));
    assert!(!argv.iter().any(|a| a.starts_with("--effort")));
}

#[test]
fn mcp_servers_reach_it_by_a_private_file_and_never_by_a_flag() {
    let (_root, wd) = checkout();
    let mut request = start_request(&wd, "field");
    request.environment.extend(endpoint("AI6_CABLE_MCP"));
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let session = started(&fake, request, "");
    let launch = fake.launch();
    let server = &launch.mcp.as_ref().unwrap()["mcpServers"]["ai6-cables"];
    assert_eq!(server["serverUrl"], "http://127.0.0.1:1/AI6_CABLE_MCP");
    assert_eq!(
        server["headers"]["Authorization"],
        "Bearer token-of-AI6_CABLE_MCP"
    );
    assert!(server.get("timeoutSeconds").is_none());
    assert!(launch.mode_private);
    assert!(
        !launch.spec.argv.join(" ").contains("token-of"),
        "no token in argv"
    );
    assert!(
        launch
            .rules("allow")
            .contains(&"mcp(ai6-cables/*)".to_owned())
    );
    let dir = launch.session_dir.display().to_string();
    assert!(
        launch.rules("deny").contains(&format!("read_file({dir})")),
        "not by the file tools"
    );
    assert!(
        launch
            .agent
            .as_deref()
            .unwrap()
            .contains("inheritMcp: true")
    );
    session.close().unwrap();
    assert!(
        !launch.session_dir.exists(),
        "the token goes with the Session"
    );
}

#[test]
fn the_sessions_directory_sorts_after_the_checkout_so_commands_run_in_the_checkout() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = started(&fake, start_request(&wd, "field"), "");
    let launch = fake.launch();
    assert_eq!(launch.spec.cwd.as_deref(), Some(wd.as_str()));
    assert!(launch.session_dir.to_string_lossy().as_ref() > wd.as_str());
    assert!(
        launch
            .session_dir
            .to_string_lossy()
            .starts_with(wd.as_str())
    );
}

#[test]
fn the_standing_context_is_the_system_prompt_and_stays_whole() {
    let (_root, wd) = checkout();
    let mut request = start_request(&wd, "field");
    request.initial_context = vec!["# A heading\n---\nfirst".to_owned(), "second".to_owned()];
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = started(&fake, request, "");
    let agent = fake.launch().agent.unwrap();
    assert!(
        agent.contains("# System Prompt\n# A heading\n---\nfirst\n\nsecond"),
        "{agent}"
    );
    assert_eq!(fake.received(), ["the task"], "not in the message");
}

#[test]
fn an_assistants_context_file_joins_the_system_prompt_within_its_bound() {
    let (root, wd) = checkout();
    let file = root.path().join("context.md");
    std::fs::write(&file, "from the file").unwrap();
    let mut mode = moneypenny();
    mode.context_file = Some(file.to_string_lossy().into_owned());
    let mut request = start_request(&wd, "desk");
    request.assistant = Some(mode);
    request.environment.extend(endpoint("AI6_MONEYPENNY_MCP"));
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = started(&fake, request.clone(), "");
    assert!(fake.launch().agent.unwrap().contains("from the file"));
    let small = AntigravityHandler::try_from(AntigravityConfig {
        context_file_bytes: 3,
        ..AntigravityConfig::default()
    })
    .unwrap();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let refused = small.start(request, fake.into_object()).err().unwrap();
    assert!(
        refused.to_string().contains("context_file_bytes"),
        "{refused}"
    );
}

#[test]
fn a_checkouts_own_hooks_and_servers_are_refused_unless_the_operation_allows_them() {
    for carried in [
        ".agents/hooks.json",
        ".agent/hooks.json",
        ".agents/plugins",
        ".agents/mcp_config.json",
        ".agent/mcp_config.json",
    ] {
        let (_root, wd) = checkout();
        let path = Path::new(&wd).join(carried);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();
        let refused = handler("")
            .start(start_request(&wd, "field"), Fake::default().into_object())
            .err()
            .map(|error| error.to_string());
        assert!(
            refused.as_deref().is_some_and(|why| why.contains(carried)),
            "{carried}: {refused:?}"
        );
        let resumed = handler("").resume(
            resume_request(&wd, CONV, "field"),
            Fake::default().into_object(),
        );
        assert!(resumed.is_err(), "{carried}");
        let mut allowed = start_request(&wd, "field");
        allowed.project_settings = true;
        allowed.project_mcp = true;
        let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
        assert!(
            handler("").start(allowed, fake.into_object()).is_ok(),
            "{carried}"
        );
    }
    let (_root, wd) = checkout();
    let path = Path::new(&wd).join(".agents/mcp_config.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{}").unwrap();
    let mut settings_only = start_request(&wd, "field");
    settings_only.project_settings = true;
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let Err(refused) = handler("").start(settings_only, fake.into_object()) else {
        panic!("servers need project_mcp as well");
    };
    assert!(refused.to_string().contains("mcp_config.json"), "{refused}");
}

#[test]
fn the_updater_is_off_during_a_session_unless_the_agent_lets_it_run() {
    let env_with = |settings: &str| {
        let (_root, wd) = checkout();
        let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
        let _session = started(&fake, start_request(&wd, "field"), settings);
        fake.launch().spec.env.unwrap()
    };
    assert_eq!(
        env_with("")
            .get("AGY_CLI_DISABLE_AUTO_UPDATE")
            .map(String::as_str),
        Some("true"),
        "agy 1.4.12 updates on with the value 1"
    );
    assert_eq!(
        env_with("auto_update = true\n").get("AGY_CLI_DISABLE_AUTO_UPDATE"),
        None
    );
}

#[test]
fn a_session_with_a_station_has_the_green_ink_hook_on_its_commands() {
    let (_root, wd) = checkout();
    let mut request = start_request(&wd, "field");
    request.environment.extend(endpoint("AI6_CABLE_MCP"));
    request
        .environment
        .insert("AI6_GREEN_INK_GATE".to_owned(), "/opt/it's/ai6".to_owned());
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = started(&fake, request, "");
    let hooks = fake.launch().hooks.unwrap();
    let entry = &hooks["ai6-green-ink"]["PreToolUse"][0];
    assert_eq!(entry["matcher"], "^run_command$");
    assert_eq!(entry["hooks"][0]["timeout"], 60);
    assert_eq!(
        entry["hooks"][0]["command"],
        "'/opt/it'\\''s/ai6' green-ink gate --cable-url 'http://127.0.0.1:1/AI6_CABLE_MCP'"
    );

    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = started(&fake, start_request(&wd, "field"), "");
    assert_eq!(fake.launch().hooks, None, "no Station to ask");

    let (_root, wd) = checkout();
    let mut desk = start_request(&wd, "desk");
    desk.environment.extend(endpoint("AI6_CABLE_MCP"));
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = started(&fake, desk, "");
    assert_eq!(fake.launch().hooks, None, "no command to gate");
}

#[test]
fn remote_hands_turns_its_own_tools_off_and_at_desk_the_theaters_commands_too() {
    let (_root, wd) = checkout();
    let mut request = start_request(&wd, "field");
    request.environment.extend(endpoint("AI6_THEATER_MCP"));
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = started(&fake, request.clone(), "");
    let launch = fake.launch();
    assert!(launch.tools().is_empty());
    assert!(
        launch
            .rules("allow")
            .contains(&"mcp(ai6-theater/*)".to_owned())
    );
    assert!(launch.rules("deny").contains(&"command(*)".to_owned()));
    assert!(
        !launch
            .rules("deny")
            .iter()
            .any(|rule| rule.contains("theater_run"))
    );

    let (_root, wd) = checkout();
    request.working_directory = wd;
    request.clearance = "desk".to_owned();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = started(&fake, request, "");
    let deny = fake.launch().rules("deny");
    assert!(
        deny.iter()
            .any(|rule| rule == "mcp(ai6-theater/theater_run)")
    );
    assert!(
        deny.iter()
            .any(|rule| rule == "mcp(ai6-theater/theater_write)")
    );
}

#[test]
fn an_interrupt_ends_agy_reports_the_turn_and_the_next_message_goes_on_the_conversation() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![
        turn(
            "{\"event\":\"step_update\",\"step_update\":{\"step_index\":0,\"state\":\"DONE\",\"step_type\":\"user_input\"}}\n",
        ),
        turn(fixture!("turn_answer.jsonl")),
    ]);
    let session = started(&fake, start_request(&wd, "field"), "");
    events_until(&session, &["init"]);
    session.interrupt().unwrap();
    assert_eq!(fake.kills(), 1);
    let events = events_until(&session, &["completed", "failed"]);
    assert_eq!(
        events.last().unwrap().kind,
        "completed",
        "{:?}",
        kinds(&events)
    );
    session
        .send_message(AgentMessage {
            text: "and then?".to_owned(),
        })
        .unwrap();
    let launch = fake.launch();
    assert!(launch.has(&format!("--conversation={CONV}")));
    assert!(!launch.spec.argv.iter().any(|a| a.starts_with("--agent=")));
    let events = events_until(&session, &["completed", "failed"]);
    assert_eq!(events.last().unwrap().kind, "completed");
    assert_eq!(fake.received(), ["the task", "and then?"]);
}

#[test]
fn an_interrupt_with_no_turn_running_leaves_agy_alone() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let session = started(&fake, start_request(&wd, "field"), "");
    events_until(&session, &["completed"]);
    session.interrupt().unwrap();
    assert_eq!(fake.kills(), 0);
}

#[test]
fn a_second_message_is_a_second_turn_in_the_same_process() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![
        turn(fixture!("turn_answer.jsonl")),
        turn(fixture!("turn_answer.jsonl")),
    ]);
    let session = started(&fake, start_request(&wd, "field"), "");
    events_until(&session, &["completed"]);
    session
        .send_message(AgentMessage {
            text: "again".to_owned(),
        })
        .unwrap();
    events_until(&session, &["completed"]);
    assert_eq!(fake.0.lock().unwrap().launches.len(), 1);
    assert_eq!(fake.received(), ["the task", "again"]);
}

#[test]
fn a_model_calls_usage_is_counted_from_its_step_as_agy_reports_it() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_cached_usage.jsonl"))]);
    let session = started(&fake, start_request(&wd, "field"), "");
    let events = events_until(&session, &["completed"]);
    let usage = events
        .iter()
        .find_map(|event| event.usage)
        .expect("the step's usage");
    assert_eq!((usage.input, usage.output, usage.cache_read), (40, 35, 70));
    assert!(!usage.cumulative, "the result's total runs across launches");
    assert_eq!(events.last().unwrap().usage, None, "counted once");
}

#[test]
fn mcp_calls_are_named_by_server_and_tool_each_once() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_mcp_call.jsonl"))]);
    let session = started(&fake, start_request(&wd, "field"), "");
    let events = events_until(&session, &["completed"]);
    let tools: Vec<&str> = events
        .iter()
        .flat_map(|event| event.tools.iter().map(String::as_str))
        .collect();
    assert_eq!(
        tools,
        ["mcp__addprobe__probe_echo", "mcp__addprobe__probe_other"]
    );
    assert_eq!(events.last().unwrap().text.as_deref(), Some("K\n"));
}

#[test]
fn it_speaks_on_pipes() {
    assert_eq!(handler("").stdio().unwrap(), ai6::HarnessStdio::Json);
}

/// A resumed conversation keeps the tools of its first launch, so a Session started holding only
/// what reads could never be given the Mission's: the handler says so, and the Station never
/// starts it under a plan review.
#[test]
fn it_cannot_hold_a_session_to_its_plan() {
    let refused = handler("").read_only_refusal().unwrap();
    assert!(refused.is_some());
}

fn overlay_request(wd: &str, skills: &Path) -> AgentStart {
    let mut request = start_request(wd, "field");
    request.overlays.roots.push("/store/tools".to_owned());
    for (name, native) in [("review", true), ("audit", false)] {
        request.overlays.skills.push(ai6::OverlaySkill {
            name: name.to_owned(),
            overlay: "tools".to_owned(),
            description: "Does a thing.".to_owned(),
            path: skills.join(name).to_string_lossy().into_owned(),
            native,
        });
    }
    request
}

/// Measured with agy 1.4.12: `skills/<name>/SKILL.md` in a workspace's `.agents` is listed under an
/// `--agent` and read with `read_file`, and a read outside the workspaces with no rule ends the turn.
#[test]
fn an_overlays_skill_is_in_a_second_workspace_and_its_root_may_be_read() {
    let (root, wd) = checkout();
    let source = root.path().join("source");
    for name in ["review", "audit"] {
        std::fs::create_dir_all(source.join(name).join("refs")).unwrap();
        std::fs::write(
            source.join(name).join("SKILL.md"),
            format!("the skill {name}"),
        )
        .unwrap();
        std::fs::write(source.join(name).join("refs/more.md"), "more").unwrap();
    }
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let session = started(&fake, overlay_request(&wd, &source), "");
    let launch = fake.launch();
    let dirs: Vec<&String> = launch
        .spec
        .argv
        .iter()
        .enumerate()
        .filter(|(at, _)| *at > 0 && launch.spec.argv[at - 1] == "--add-dir")
        .map(|(_, dir)| dir)
        .collect();
    assert_eq!(dirs.len(), 2, "{:?}", launch.spec.argv);
    let skills = PathBuf::from(dirs[1]);
    assert!(
        dirs[1].starts_with(&wd),
        "its name extends the working directory's"
    );
    let copied = skills.join(".agents").join("skills").join("review");
    assert_eq!(
        std::fs::read_to_string(copied.join("SKILL.md")).unwrap(),
        "the skill review"
    );
    assert!(copied.join("refs/more.md").is_file());
    assert!(
        !skills.join(".agents").join("skills").join("audit").exists(),
        "not a native skill"
    );
    assert!(
        launch
            .rules("allow")
            .contains(&"read_file(/store/tools)".to_owned()),
        "{:?}",
        launch.rules("allow")
    );
    let agent = launch.agent.unwrap();
    assert!(agent.contains("Skill `audit` (overlay `tools`)"), "{agent}");
    assert!(!agent.contains("Skill `review`"), "{agent}");
    session.close().unwrap();
    assert!(!skills.exists(), "removed with the Session");
}

#[test]
fn a_session_with_no_overlay_has_one_workspace_beside_its_own() {
    let (_root, wd) = checkout();
    let fake = Fake::with(vec![turn(fixture!("turn_answer.jsonl"))]);
    let _session = started(&fake, start_request(&wd, "field"), "");
    let launch = fake.launch();
    assert_eq!(
        launch
            .spec
            .argv
            .iter()
            .filter(|word| *word == "--add-dir")
            .count(),
        1
    );
}
