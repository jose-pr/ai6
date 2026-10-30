//! The `acp` handler against a scripted ACP agent started through the real `pipe` terminal.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ai6::MissionId;
use ai6::directorates::m::identity::SessionId;
use ai6::directorates::q::terminal::{AgentTerminal, TerminalBridge};
use ai6::directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentResume, AgentSession, AgentStart,
};
use ai6::guatiao::library::Object;
use ai6::station::plugins::Plugins;
use ai6_agent_acp::{AcpConfig, AcpHandler};

fn handler(extra: &str) -> AcpHandler {
    let fake = env!("CARGO_BIN_EXE_ai6-acp-fake-agent");
    AcpHandler::try_from(AcpConfig {
        settings_toml: format!("command = '{fake}'\n{extra}"),
    })
    .expect("the settings are valid")
}

/// The `pipe` terminal, loaded from the library built beside this test: linking its crate
/// here would clash with this provider's own Guatiao entry point.
fn terminal() -> Object<dyn AgentTerminal> {
    static PLUGINS: std::sync::OnceLock<Plugins> = std::sync::OnceLock::new();
    let plugins = PLUGINS.get_or_init(|| {
        let deps = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_owned();
        Plugins::scan(&[deps])
    });
    TerminalBridge::agent_terminal(
        plugins
            .terminal("pipe", None)
            .expect("the pipe terminal loads"),
    )
}

/// The host's own environment (the fake agent is a program like any other) plus `vars`.
fn environment(vars: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut environment: BTreeMap<String, String> = std::env::vars().collect();
    environment.extend(vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
    environment
}

fn start(environment: BTreeMap<String, String>) -> AgentStart {
    AgentStart {
        mission_id: MissionId::try_new("mission-acp").unwrap(),
        agent_id: "007".to_owned(),
        prompt: "Find the leak".to_owned(),
        initial_context: vec!["You are 007".to_owned()],
        working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
        clearance: "field".to_owned(),
        environment,
        model: None,
        effort: None,
        max_cost_micro_usd: None,
        project_settings: false,
        project_mcp: false,
        answer_only: false,
        assistant: None,
    }
}

fn until(session: &Object<dyn AgentSession>, kinds: &[&str]) -> Vec<AgentEvent> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Some(event) = session.next_event(100).unwrap() {
            let stop = kinds.contains(&event.kind.as_str());
            seen.push(event);
            if stop {
                return seen;
            }
        }
    }
    panic!("no {kinds:?} event; saw {seen:?}");
}

fn text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter(|event| event.kind == "text")
        .filter_map(|event| event.text.clone())
        .collect::<Vec<_>>()
        .join("")
}

fn run(vars: &[(&str, &str)], extra: &str) -> (Object<dyn AgentSession>, Vec<AgentEvent>) {
    let session = handler(extra)
        .start(start(environment(vars)), terminal())
        .expect("the agent starts");
    let events = until(&session, &["completed", "failed"]);
    (session, events)
}

#[test]
fn the_sessions_environment_and_the_settings_reach_the_agent() {
    let (session, events) = run(
        &[("FAKE_ECHO_ENV", "WHO"), ("WHO", "from the session")],
        "env = { WHO = \"from the settings\", UNUSED = \"x\" }\n",
    );
    let said = text(&events);
    assert!(
        said.contains("env:from the session"),
        "the Session wins: {said}"
    );
    assert!(
        said.contains("You are 007") && said.contains("Find the leak"),
        "{said}"
    );
    assert_eq!(events.last().unwrap().kind, "completed");
    assert_eq!(session.session_id().unwrap().unwrap().as_str(), "fake-1");
    session.close().unwrap();
}

#[test]
fn settings_alone_supply_what_the_session_leaves_out() {
    let (_, events) = run(
        &[("FAKE_ECHO_ENV", "ONLY_SETTINGS")],
        "env = { ONLY_SETTINGS = \"configured\" }\n",
    );
    assert!(text(&events).contains("env:configured"));
}

#[test]
fn a_reported_cost_becomes_usage() {
    let (_, events) = run(&[("FAKE_COST", "0.5")], "");
    let cost = events
        .iter()
        .find_map(|event| event.usage)
        .expect("a cost event");
    assert_eq!(cost.cost_micro_usd, 500_000);
}

#[test]
fn a_refusal_and_an_error_fail_the_session() {
    let (_, events) = run(&[("FAKE_STOP", "refusal")], "");
    assert_eq!(events.last().unwrap().kind, "failed");
    let (_, events) = run(&[("FAKE_ERROR", "1")], "");
    let last = events.last().unwrap();
    assert_eq!(last.kind, "failed");
    assert!(
        last.text
            .as_deref()
            .unwrap()
            .contains("the fake model is down")
    );
}

#[test]
fn a_permission_request_is_judged_by_the_gate() {
    // Not a shell command: allowed.
    let (_, events) = run(&[("FAKE_ASK", "read")], "");
    assert!(text(&events).contains("asked:yes"), "{events:?}");
    // A command in a directory that is no repository: green ink cannot be checked, so held.
    let (_, events) = run(&[("FAKE_ASK", "execute")], "");
    assert!(text(&events).contains("asked:no"), "{events:?}");
    assert!(
        events.iter().any(|event| event.kind == "permission"
            && event.text.as_deref().unwrap().contains("refused"))
    );
}

#[test]
fn an_answer_only_session_refuses_an_edit_and_ends_when_one_runs_unasked() {
    let mut request = start(environment(&[("FAKE_ASK", "edit")]));
    request.answer_only = true;
    let session = handler("").start(request, terminal()).unwrap();
    let events = until(&session, &["completed", "failed"]);
    assert!(text(&events).contains("asked:no"), "{events:?}");
    let mut request = start(environment(&[("FAKE_UNASKED", "edit")]));
    request.answer_only = true;
    let session = handler("").start(request, terminal()).unwrap();
    let events = until(&session, &["completed", "failed"]);
    let last = events.last().unwrap();
    assert_eq!(last.kind, "failed");
    assert!(last.text.as_deref().unwrap().contains("without asking"));
}

#[test]
fn an_agent_without_http_mcp_refuses_a_session_with_cable_tools() {
    let error = handler("")
        .start(
            start(environment(&[
                ("FAKE_NO_HTTP", "1"),
                ("AI6_CABLE_MCP_URL", "http://127.0.0.1:1/mcp"),
                ("AI6_CABLE_MCP_TOKEN", "t"),
            ])),
            terminal(),
        )
        .map(|_| ())
        .expect_err("refused");
    assert!(error.to_string().contains("MCP"), "{error}");
}

#[test]
fn a_slow_initialize_is_a_setting_not_a_hang() {
    let terminal = terminal();
    let started = Instant::now();
    let error = handler("initialize_timeout_ms = 200\n")
        .start(
            start(environment(&[("FAKE_INIT_DELAY_MS", "5000")])),
            terminal,
        )
        .map(|_| ())
        .expect_err("times out");
    assert!(
        error.to_string().contains("no answer within 200 ms"),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
fn a_session_resumes_when_the_agent_can() {
    let request = AgentResume {
        mission_id: MissionId::try_new("mission-acp").unwrap(),
        session_id: SessionId::try_new("fake-1").unwrap(),
        message: Some("carry on".to_owned()),
        working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
        environment: environment(&[("FAKE_RESUME", "1")]),
        model: None,
        effort: None,
        max_cost_micro_usd: None,
        project_settings: false,
        project_mcp: false,
        answer_only: false,
        assistant: None,
        initial_context: Vec::new(),
    };
    let session = handler("").resume(request, terminal()).unwrap();
    let events = until(&session, &["completed", "failed"]);
    assert!(text(&events).contains("prompt:carry on"), "{events:?}");
    // An agent that cannot resume or load says so.
    let request = AgentResume {
        mission_id: MissionId::try_new("mission-acp").unwrap(),
        session_id: SessionId::try_new("fake-1").unwrap(),
        message: None,
        working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
        environment: environment(&[]),
        model: None,
        effort: None,
        max_cost_micro_usd: None,
        project_settings: false,
        project_mcp: false,
        answer_only: false,
        assistant: None,
        initial_context: Vec::new(),
    };
    let error = handler("")
        .resume(request, terminal())
        .map(|_| ())
        .expect_err("refused");
    assert!(
        error.to_string().contains("neither resume nor load"),
        "{error}"
    );
}

#[test]
fn settings_are_validated_when_the_handler_loads() {
    let build = |settings: &str| {
        AcpHandler::try_from(AcpConfig {
            settings_toml: settings.to_owned(),
        })
    };
    assert!(
        build("").is_ok(),
        "defaults build; start explains what is missing"
    );
    assert!(build("command = \"agent\"\nprotocol = \"v2\"\n").is_err());
    assert!(build("command = \"agent\"\nunknown = 1\n").is_err());
    assert!(build("command = \" \"\n").is_err());
    assert!(build("args = [\"x\"]\n").is_err(), "a command is required");
    let error = build("")
        .unwrap()
        .start(start(environment(&[])), terminal())
        .map(|_| ())
        .expect_err("no command");
    assert!(error.to_string().contains("handler_settings"), "{error}");
}
