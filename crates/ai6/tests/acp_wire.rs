//! The ACP wire: the façade over a fake AgentHandler, the client against a fake ACP agent.

use agent_client_protocol::{
    Agent, Channel, Client, ConnectionTo, Responder, UntypedMessage, on_receive_notification,
    on_receive_request,
};
use ai6::directorates::m::identity::{MissionId, SessionId};
use ai6::directorates::q::terminal::AgentTerminal;
use ai6::directorates::section_00::acp::{
    AcpFacade, AcpOptions, AcpSession, LAUNCH_META_KEY, LaunchMeta, PermissionDecider,
    PermissionDecision, PermissionRequest, ResumeMeta, duplex,
};
use ai6::directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentMessage, AgentResume, AgentSession, AgentStart, QuotaReport,
    TokenUsage, UsageLimit,
};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{Status, Value as GuatiaoValue};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn event(kind: &str, text: Option<&str>) -> AgentEvent {
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

/// What the fake handler was asked.
#[derive(Default)]
struct Log {
    starts: Mutex<Vec<AgentStart>>,
    resumes: Mutex<Vec<AgentResume>>,
    messages: Mutex<Vec<String>>,
    interrupts: AtomicUsize,
    closes: AtomicUsize,
}

struct FakeHandler {
    log: Arc<Log>,
    script: Vec<AgentEvent>,
    fail_start: bool,
}

struct FakeSession {
    log: Arc<Log>,
    id: String,
    events: Mutex<VecDeque<AgentEvent>>,
}

impl AgentHandler for FakeHandler {
    fn start(
        &self,
        request: AgentStart,
        _terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        if self.fail_start {
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_INTERNAL,
                "no harness here",
            ));
        }
        self.log.starts.lock().unwrap().push(request);
        Ok(FakeSession {
            log: self.log.clone(),
            id: "provider-1".to_owned(),
            events: Mutex::new(self.script.iter().cloned().collect()),
        }
        .into_object())
    }

    fn resume(
        &self,
        request: AgentResume,
        _terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let id = request.session_id.to_string();
        self.log.resumes.lock().unwrap().push(request);
        Ok(FakeSession {
            log: self.log.clone(),
            id,
            events: Mutex::new(VecDeque::from([event("text", Some("resumed"))])),
        }
        .into_object())
    }
}

impl AgentSession for FakeSession {
    fn session_id(&self) -> Result<Option<SessionId>, ProviderError> {
        Ok(Some(SessionId::try_new(self.id.clone()).unwrap()))
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        self.log.messages.lock().unwrap().push(message.text);
        Ok(())
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        let next = self.events.lock().unwrap().pop_front();
        if next.is_none() {
            std::thread::sleep(Duration::from_millis(timeout_ms.clamp(0, 20) as u64));
        }
        Ok(next)
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        self.log.interrupts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn close(&self) -> Result<(), ProviderError> {
        self.log.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct NoTerminal;

impl AgentTerminal for NoTerminal {
    fn spawn(
        &self,
        _spec: &GuatiaoValue,
    ) -> Result<Object<dyn ai6::directorates::q::terminal::TerminalSession>, ProviderError> {
        Err(ProviderError::new(
            Status::GUATIAO_ERR_INTERNAL,
            "no terminal in this test",
        ))
    }
}

fn facade(script: Vec<AgentEvent>) -> (AcpFacade, Arc<Log>) {
    facade_with(script, false)
}

fn facade_with(script: Vec<AgentEvent>, fail_start: bool) -> (AcpFacade, Arc<Log>) {
    let log = Arc::new(Log::default());
    let handler = FakeHandler {
        log: log.clone(),
        script,
        fail_start,
    };
    let facade = AcpFacade::new(Arc::new(handler), Arc::new(|| Ok(NoTerminal.into_object())));
    (facade, log)
}

fn start() -> AgentStart {
    AgentStart {
        mission_id: MissionId::try_new("mission-1").unwrap(),
        agent_id: "007".to_owned(),
        prompt: "Find the leak".to_owned(),
        initial_context: vec!["You are 007".to_owned(), "Be brief".to_owned()],
        working_directory: "/work/mission-1".to_owned(),
        clearance: "restricted".to_owned(),
        environment: BTreeMap::from([
            ("AI6_TOKEN".to_owned(), "hunter2-secret".to_owned()),
            ("PATH".to_owned(), "/bin".to_owned()),
        ]),
        model: Some("haiku".to_owned()),
        effort: Some("low".to_owned()),
        max_cost_micro_usd: Some(5_000_000),
        project_settings: true,
        project_mcp: false,
        answer_only: false,
        assistant: None,
    }
}

fn options_for(start: &AgentStart) -> AcpOptions {
    let mut options = AcpOptions {
        cwd: start.working_directory.clone(),
        prompt: Some(start.prompt.clone()),
        ..AcpOptions::default()
    };
    options.meta.insert(
        LAUNCH_META_KEY.to_owned(),
        LaunchMeta::from_start(start).to_value(),
    );
    options
}

/// Events until one of `kinds` arrives, within a deadline.
fn until(session: &AcpSession, kinds: &[&str]) -> Vec<AgentEvent> {
    let deadline = Instant::now() + Duration::from_secs(20);
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

fn connect_facade(
    facade: &AcpFacade,
    options: AcpOptions,
    closed: Option<Arc<AtomicBool>>,
) -> Result<AcpSession, ProviderError> {
    let (agent_end, client_end) = duplex();
    facade.serve(agent_end);
    AcpSession::connect(
        client_end,
        options,
        closed.map(|flag| {
            Box::new(move || flag.store(true, Ordering::SeqCst)) as Box<dyn FnOnce() + Send>
        }),
    )
}

#[test]
fn a_handler_runs_through_the_facade_with_every_event_verbatim() {
    let mut limit = event("limit", Some("five hour window used"));
    limit.limit = Some(UsageLimit {
        resets_at: Some(1_790_000_000),
        window: Some("five_hour".to_owned()),
    });
    limit.quota = vec![QuotaReport {
        window: "five_hour".to_owned(),
        used_permille: Some(1000),
        resets_at: Some(1_790_000_000),
        status: Some("rejected".to_owned()),
    }];
    let mut tool = event("tool", Some("fetching"));
    tool.tools = vec!["WebFetch".to_owned()];
    let mut spend = event("usage", None);
    spend.usage = Some(TokenUsage {
        input: 12,
        output: 34,
        cache_read: 5,
        cache_write: 6,
        cost_micro_usd: 250_000,
        cumulative: true,
    });
    let mut raw = event("text", Some("with data"));
    raw.data = Some(vec![0, 159, 146, 150, 255]);
    let script = vec![
        event("text", Some("hello")),
        tool,
        spend,
        raw,
        limit,
        event("completed", None),
    ];
    let (facade, log) = facade(script.clone());
    let begun = start();
    let session = connect_facade(&facade, options_for(&begun), None).unwrap();
    let seen = until(&session, &["completed"]);
    assert_eq!(seen, script);
    assert_eq!(
        session.session_id().unwrap().unwrap().as_str(),
        "provider-1"
    );
    // The handler got the AgentStart the Station built, prompt and working directory included.
    assert_eq!(*log.starts.lock().unwrap(), vec![begun]);
}

#[test]
fn a_start_that_fails_is_a_start_failure() {
    let (facade, _log) = facade_with(Vec::new(), true);
    let error = connect_facade(&facade, options_for(&start()), None)
        .err()
        .expect("the start fails");
    assert!(error.to_string().contains("no harness here"), "{error}");
}

#[test]
fn messages_interrupts_and_close_reach_the_handler() {
    let (facade, log) = facade(vec![event("text", Some("hi"))]);
    let closed = Arc::new(AtomicBool::new(false));
    let session = connect_facade(&facade, options_for(&start()), Some(closed.clone())).unwrap();
    until(&session, &["text"]);
    session
        .send_message(AgentMessage {
            text: "and then?".to_owned(),
        })
        .unwrap();
    session.interrupt().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while log.interrupts.load(Ordering::SeqCst) == 0 || log.messages.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the handler was not reached");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(*log.messages.lock().unwrap(), vec!["and then?".to_owned()]);
    session.close().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while log.closes.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "close was not forwarded");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(closed.load(Ordering::SeqCst));
}

/// A client written straight on the SDK, for what the façade says to a client that is not AI6.
fn raw_call(
    cx: &ConnectionTo<Agent>,
    method: &str,
    params: Value,
) -> impl std::future::Future<Output = Result<Value, agent_client_protocol::Error>> {
    cx.send_request(UntypedMessage::new(method, params).unwrap())
        .block_task()
}

#[test]
fn a_cancel_answers_the_waiting_prompt_cancelled() {
    // The handler never ends its turn.
    let (facade, log) = facade(vec![event("text", Some("working"))]);
    let (agent_end, client_end) = duplex();
    facade.serve(agent_end);
    let begun = start();
    let meta = LaunchMeta::from_start(&begun).to_value();
    let answer = futures::executor::block_on(async {
        Client
            .builder()
            .connect_with(client_end, async |cx| {
                let init = raw_call(&cx, "initialize", json!({"protocolVersion": 1})).await?;
                assert_eq!(init["agentCapabilities"]["loadSession"], true);
                assert!(init["agentCapabilities"]["sessionCapabilities"]["resume"].is_object());
                assert_eq!(
                    init["agentCapabilities"]["_meta"]["dev.coqui.ai6/events"],
                    true
                );
                let new = raw_call(
                    &cx,
                    "session/new",
                    json!({"cwd": "/work", "mcpServers": [], "_meta": {LAUNCH_META_KEY: meta}}),
                )
                .await?;
                let id = new["sessionId"].as_str().unwrap().to_owned();
                let prompt = cx
                    .send_request(
                        UntypedMessage::new(
                            "session/prompt",
                            json!({"sessionId": id, "prompt": [{"type": "text", "text": "go"}]}),
                        )
                        .unwrap(),
                    )
                    .block_task();
                let (started_tx, started) = futures::channel::oneshot::channel::<()>();
                let watched = log.clone();
                std::thread::spawn(move || {
                    while watched.starts.lock().unwrap().is_empty() {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    let _ = started_tx.send(());
                });
                let cancel = async {
                    started.await.unwrap();
                    cx.send_notification(
                        UntypedMessage::new("session/cancel", json!({"sessionId": id})).unwrap(),
                    )
                };
                let (answer, sent) = futures::future::join(prompt, cancel).await;
                sent?;
                answer
            })
            .await
    })
    .unwrap();
    assert_eq!(answer["stopReason"], "cancelled");
    assert_eq!(log.interrupts.load(Ordering::SeqCst), 1);
}

#[test]
fn a_session_resumes_through_the_facade() {
    let (facade, log) = facade(Vec::new());
    let begun = start();
    let resume = AgentResume {
        mission_id: begun.mission_id.clone(),
        session_id: SessionId::try_new("provider-9").unwrap(),
        message: Some("carry on".to_owned()),
        working_directory: begun.working_directory.clone(),
        environment: begun.environment.clone(),
        model: begun.model.clone(),
        effort: None,
        max_cost_micro_usd: None,
        project_settings: false,
        project_mcp: false,
        answer_only: false,
        assistant: None,
        initial_context: vec!["standing".to_owned()],
    };
    let mut options = AcpOptions {
        cwd: resume.working_directory.clone(),
        resume: Some("provider-9".to_owned()),
        ..AcpOptions::default()
    };
    options.meta.insert(
        LAUNCH_META_KEY.to_owned(),
        ResumeMeta::from_resume(&resume).to_value(),
    );
    let (agent_end, client_end) = duplex();
    facade.serve(agent_end);
    let session = AcpSession::connect(client_end, options, None).unwrap();
    until(&session, &["text"]);
    assert_eq!(*log.resumes.lock().unwrap(), vec![resume]);
}

// ---- the client against an ACP agent that is not AI6's façade ----

#[derive(Clone, Default)]
struct Script {
    resume: bool,
    load: bool,
    http: bool,
    model_option: bool,
    /// The kind of tool the agent asks leave for during the turn.
    ask: Option<&'static str>,
    /// A tool of this kind runs without being asked.
    run_unasked: Option<&'static str>,
    cost: Option<f64>,
    stop: Option<&'static str>,
    fail_prompt: bool,
}

#[derive(Default)]
struct Seen {
    calls: Mutex<Vec<(String, Value)>>,
    outcome: Mutex<Option<Value>>,
    cancels: AtomicUsize,
}

fn serve_agent(script: Script, seen: Arc<Seen>, transport: Channel) {
    std::thread::spawn(move || {
        let for_requests = seen.clone();
        let for_notifications = seen.clone();
        let _ = futures::executor::block_on(
            Agent
                .builder()
                .on_receive_request(
                    async move |message: UntypedMessage,
                                responder: Responder<Value>,
                                cx: ConnectionTo<Client>| {
                        agent_request(&script, &for_requests, message, responder, cx);
                        Ok(())
                    },
                    on_receive_request!(),
                )
                .on_receive_notification(
                    async move |message: UntypedMessage, _cx: ConnectionTo<Client>| {
                        if message.method() == "session/cancel" {
                            for_notifications.cancels.fetch_add(1, Ordering::SeqCst);
                        }
                        Ok(())
                    },
                    on_receive_notification!(),
                )
                .connect_to(transport),
        );
    });
}

fn update(cx: &ConnectionTo<Client>, update: Value) {
    cx.send_notification(
        UntypedMessage::new(
            "session/update",
            json!({"sessionId": "ext-1", "update": update}),
        )
        .unwrap(),
    )
    .unwrap();
}

fn agent_request(
    script: &Script,
    seen: &Arc<Seen>,
    message: UntypedMessage,
    responder: Responder<Value>,
    cx: ConnectionTo<Client>,
) {
    let method = message.method().to_owned();
    let params = message.params().clone();
    seen.calls
        .lock()
        .unwrap()
        .push((method.clone(), params.clone()));
    macro_rules! respond {
        ($value:expr) => {
            responder.respond($value).unwrap()
        };
    }
    match method.as_str() {
        "initialize" => respond!(json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": script.load,
                "mcpCapabilities": {"http": script.http, "sse": false},
                "sessionCapabilities": {"resume": if script.resume { json!({}) } else { Value::Null }},
            },
            "authMethods": [{"id": "login", "name": "Log in"}],
        })),
        "session/new" => {
            let options = if script.model_option {
                json!([{
                    "id": "model", "name": "Model", "category": "model", "type": "select",
                    "currentValue": "sonnet",
                    "options": [{"value": "sonnet", "name": "Sonnet"}, {"value": "haiku", "name": "Haiku"}],
                }])
            } else {
                json!([])
            };
            respond!(json!({"sessionId": "ext-1", "configOptions": options}));
        }
        "session/resume" => respond!(json!({})),
        "session/load" => {
            update(
                &cx,
                json!({"sessionUpdate": "agent_message_chunk",
                       "content": {"type": "text", "text": "OLD HISTORY"}}),
            );
            respond!(json!({}));
        }
        "session/set_config_option" => respond!(json!({"configOptions": []})),
        "session/close" => respond!(json!({})),
        "session/prompt" => {
            let script = script.clone();
            let seen = seen.clone();
            std::thread::spawn(move || {
                if script.fail_prompt {
                    responder
                        .respond_with_error(
                            agent_client_protocol::Error::internal_error()
                                .data("the model is down"),
                        )
                        .unwrap();
                    return;
                }
                for part in ["he", "llo"] {
                    update(
                        &cx,
                        json!({"sessionUpdate": "agent_message_chunk",
                               "content": {"type": "text", "text": part}}),
                    );
                }
                if let Some(kind) = script.run_unasked {
                    update(
                        &cx,
                        json!({"sessionUpdate": "tool_call", "toolCallId": "t0", "title": "Edit a file",
                               "kind": kind, "status": "completed"}),
                    );
                }
                if let Some(kind) = script.ask {
                    let asked = futures::executor::block_on(
                        cx.send_request(
                            UntypedMessage::new(
                                "session/request_permission",
                                json!({
                                    "sessionId": "ext-1",
                                    "toolCall": {"toolCallId": "t1", "title": "Run it", "kind": kind,
                                                 "status": "pending", "rawInput": {"command": "ls"}},
                                    "options": [
                                        {"optionId": "yes", "name": "Allow", "kind": "allow_once"},
                                        {"optionId": "no", "name": "Reject", "kind": "reject_once"},
                                    ],
                                }),
                            )
                            .unwrap(),
                        )
                        .block_task(),
                    )
                    .unwrap();
                    *seen.outcome.lock().unwrap() = Some(asked);
                }
                if let Some(amount) = script.cost {
                    update(
                        &cx,
                        json!({"sessionUpdate": "usage_update", "used": 10, "size": 1000,
                               "cost": {"amount": amount, "currency": "USD"}}),
                    );
                }
                responder
                    .respond(json!({"stopReason": script.stop.unwrap_or("end_turn")}))
                    .unwrap();
            });
        }
        _ => responder
            .respond_with_error(agent_client_protocol::Error::method_not_found())
            .unwrap(),
    }
}

fn connect_agent(
    script: Script,
    options: AcpOptions,
) -> (Result<AcpSession, ProviderError>, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let (agent_end, client_end) = duplex();
    serve_agent(script, seen.clone(), agent_end);
    (AcpSession::connect(client_end, options, None), seen)
}

fn external(prompt: &str) -> AcpOptions {
    AcpOptions {
        cwd: "/work".to_owned(),
        prompt: Some(prompt.to_owned()),
        ..AcpOptions::default()
    }
}

#[test]
fn an_external_agents_turn_becomes_text_cost_and_completion() {
    let script = Script {
        cost: Some(0.25),
        ..Script::default()
    };
    let (session, seen) = connect_agent(script, external("go"));
    let session = session.unwrap();
    let seen_events = until(&session, &["completed"]);
    assert_eq!(seen_events[0].kind, "text");
    assert_eq!(seen_events[0].text.as_deref(), Some("hello"));
    let usage = seen_events[1].usage.expect("cost is reported");
    assert_eq!(usage.cost_micro_usd, 250_000);
    assert!(usage.cumulative);
    assert_eq!(seen_events.last().unwrap().kind, "completed");
    assert_eq!(session.session_id().unwrap().unwrap().as_str(), "ext-1");
    let calls = seen.calls.lock().unwrap();
    let prompt = calls
        .iter()
        .find(|(method, _)| method == "session/prompt")
        .unwrap();
    assert_eq!(prompt.1["prompt"][0]["text"], "go");
}

#[test]
fn stop_reasons_and_errors_map_to_events() {
    let (refused, _) = connect_agent(
        Script {
            stop: Some("refusal"),
            ..Script::default()
        },
        external("go"),
    );
    let events = until(&refused.unwrap(), &["failed", "completed"]);
    assert_eq!(events.last().unwrap().kind, "failed");
    let (broken, _) = connect_agent(
        Script {
            fail_prompt: true,
            ..Script::default()
        },
        external("go"),
    );
    let events = until(&broken.unwrap(), &["failed", "completed"]);
    let last = events.last().unwrap();
    assert_eq!(last.kind, "failed");
    assert!(last.text.as_deref().unwrap().contains("the model is down"));
}

#[test]
fn model_and_effort_use_the_agents_options() {
    let mut options = external("go");
    options.model = Some("Haiku".to_owned());
    options.effort = Some("high".to_owned());
    let (session, seen) = connect_agent(
        Script {
            model_option: true,
            ..Script::default()
        },
        options,
    );
    let session = session.unwrap();
    let events = until(&session, &["completed"]);
    let calls = seen.calls.lock().unwrap();
    let set = calls
        .iter()
        .find(|(method, _)| method == "session/set_config_option")
        .expect("the model was set");
    assert_eq!(set.1["configId"], "model");
    assert_eq!(set.1["value"], "haiku");
    // The agent offers no effort option: that is said, not guessed.
    assert!(events.iter().any(|event| {
        event.kind == "notice"
            && event
                .text
                .as_deref()
                .is_some_and(|text| text.contains("effort"))
    }));
}

#[test]
fn an_agent_without_http_mcp_refuses_a_session_that_needs_it() {
    let mut options = external("go");
    options.mcp_servers.push(
        json!({"type": "http", "name": "cables", "url": "http://127.0.0.1:1/mcp", "headers": []}),
    );
    let (session, _) = connect_agent(Script::default(), options);
    let error = session.err().expect("refused");
    assert!(error.to_string().contains("MCP"), "{error}");
}

#[test]
fn a_load_replay_is_not_surfaced_and_a_resume_continues() {
    let mut options = external("again");
    options.resume = Some("ext-1".to_owned());
    let (session, seen) = connect_agent(
        Script {
            load: true,
            ..Script::default()
        },
        options.clone_for_test(),
    );
    let session = session.unwrap();
    let events = until(&session, &["completed"]);
    assert!(
        events
            .iter()
            .all(|event| event.text.as_deref() != Some("OLD HISTORY")),
        "{events:?}"
    );
    assert!(
        seen.calls
            .lock()
            .unwrap()
            .iter()
            .any(|(method, _)| method == "session/load")
    );
    let (resumed, seen) = connect_agent(
        Script {
            resume: true,
            load: true,
            ..Script::default()
        },
        options,
    );
    until(&resumed.unwrap(), &["completed"]);
    let calls = seen.calls.lock().unwrap();
    assert!(calls.iter().any(|(method, _)| method == "session/resume"));
    assert!(!calls.iter().any(|(method, _)| method == "session/load"));
}

trait CloneForTest {
    fn clone_for_test(&self) -> Self;
}

impl CloneForTest for AcpOptions {
    fn clone_for_test(&self) -> Self {
        AcpOptions {
            cwd: self.cwd.clone(),
            prompt: self.prompt.clone(),
            resume: self.resume.clone(),
            ..AcpOptions::default()
        }
    }
}

#[derive(Default)]
struct Decider {
    asked: Mutex<Vec<PermissionRequest>>,
    allow: bool,
}

impl PermissionDecider for Decider {
    fn decide(&self, request: &PermissionRequest) -> PermissionDecision {
        self.asked.lock().unwrap().push(request.clone());
        if self.allow {
            PermissionDecision::Allow
        } else {
            PermissionDecision::Deny("not today".to_owned())
        }
    }
}

fn outcome(seen: &Seen) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(outcome) = seen.outcome.lock().unwrap().clone() {
            return outcome;
        }
        assert!(Instant::now() < deadline, "the agent got no answer");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_permission_request_reaches_the_decider_and_both_answers_work() {
    for allow in [true, false] {
        let decider = Arc::new(Decider {
            allow,
            ..Decider::default()
        });
        let mut options = external("go");
        options.decider = decider.clone();
        let (session, seen) = connect_agent(
            Script {
                ask: Some("execute"),
                ..Script::default()
            },
            options,
        );
        let session = session.unwrap();
        until(&session, &["completed"]);
        let asked = decider.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].kind.as_deref(), Some("execute"));
        assert_eq!(asked[0].raw_input, Some(json!({"command": "ls"})));
        let answer = outcome(&seen);
        assert_eq!(answer["outcome"]["outcome"], "selected");
        assert_eq!(
            answer["outcome"]["optionId"],
            if allow { "yes" } else { "no" }
        );
    }
}

#[test]
fn a_restricted_session_refuses_what_changes_things_without_asking_the_decider() {
    let decider = Arc::new(Decider {
        allow: true,
        ..Decider::default()
    });
    let mut options = external("go");
    options.decider = decider.clone();
    options.restricted = true;
    let (session, seen) = connect_agent(
        Script {
            ask: Some("edit"),
            ..Script::default()
        },
        options,
    );
    until(&session.unwrap(), &["completed"]);
    assert!(decider.asked.lock().unwrap().is_empty());
    assert_eq!(outcome(&seen)["outcome"]["optionId"], "no");
}

#[test]
fn a_restricted_session_ends_when_a_tool_runs_without_asking() {
    let mut options = external("go");
    options.restricted = true;
    let (session, seen) = connect_agent(
        Script {
            run_unasked: Some("edit"),
            ..Script::default()
        },
        options,
    );
    let events = until(&session.unwrap(), &["failed", "completed"]);
    let last = events.last().unwrap();
    assert_eq!(last.kind, "failed");
    assert!(last.text.as_deref().unwrap().contains("without asking"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while seen.cancels.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the agent was not told to stop");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn launch_meta_round_trips_and_never_prints_the_environment() {
    let begun = start();
    let meta = LaunchMeta::from_start(&begun);
    let sent: LaunchMeta = serde_json::from_value(meta.to_value()).unwrap();
    assert_eq!(sent, meta);
    assert_eq!(
        sent.into_start(begun.prompt.clone(), begun.working_directory.clone()),
        begun
    );
    let shown = format!("{meta:?}");
    assert!(shown.contains("AI6_TOKEN"), "names are shown: {shown}");
    assert!(!shown.contains("hunter2-secret"), "{shown}");
    let resume = ResumeMeta::from_resume(&AgentResume {
        mission_id: begun.mission_id.clone(),
        session_id: SessionId::try_new("provider-3").unwrap(),
        message: Some("m".to_owned()),
        working_directory: "/w".to_owned(),
        environment: begun.environment.clone(),
        model: None,
        effort: None,
        max_cost_micro_usd: None,
        project_settings: false,
        project_mcp: true,
        answer_only: true,
        assistant: None,
        initial_context: Vec::new(),
    });
    assert!(!format!("{resume:?}").contains("hunter2-secret"));
    let back: ResumeMeta = serde_json::from_value(resume.to_value()).unwrap();
    assert_eq!(back, resume);
}
