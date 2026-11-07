use std::collections::BTreeMap;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{
    AgentRole, Attempt, Authenticator, AuthorizationDecision, AuthorizationRequest, Authorizer,
    Directory, FileRegistry, Lookup, Person, Verdict, guatiao::library::ProviderError,
};
use ai6_diptel::{
    ControlAction, ControlId, FrameReceiver, IncomingFrame, LifecycleEvent, MissionControl,
    MissionId, OperationId, SessionId, SessionState, control_frames,
};
use tokio::sync::oneshot;
use universal_exports::{AccessProviders, Gateway, GatewayConfig, GatewayLimits};

struct TestAuthenticator;

impl Authenticator for TestAuthenticator {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        match (attempt.name.as_deref(), attempt.secret.as_deref()) {
            (Some("viewer"), Some("secret")) => Ok(Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: "viewer".to_owned(),
            })),
            _ => Ok(Verdict::Invalid),
        }
    }
}

struct TestDirectory;

impl Directory for TestDirectory {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        Ok(
            matches!(lookup.value.as_str(), "viewer" | "viewer-007").then(|| Person {
                id: "viewer-007".to_owned(),
                name: "viewer".to_owned(),
                display_name: None,
                email: None,
                groups: Vec::new(),
                attributes: BTreeMap::new(),
            }),
        )
    }
}

struct TestAuthorizer;

impl Authorizer for TestAuthorizer {
    fn authorize(
        &self,
        _request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        Ok(AuthorizationDecision {
            allowed: true,
            limits: BTreeMap::new(),
        })
    }
}

async fn start_gateway() -> (String, oneshot::Sender<()>, String, tempfile::TempDir) {
    let token = "exports-test-credential-with-more-than-32-bytes";
    let config = GatewayConfig::new(
        "https://exports.example",
        token,
        GatewayLimits::default(),
        Duration::from_secs(60),
        Duration::from_secs(30),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(directory.path());
    registry
        .check_in(
            "station-01",
            BTreeMap::from([
                ("operations".to_owned(), serde_json::json!(["operation-01"])),
                ("roles".to_owned(), serde_json::json!(["execution"])),
                ("running".to_owned(), serde_json::json!(1)),
                ("capacity".to_owned(), serde_json::json!(4)),
            ]),
        )
        .unwrap();
    registry
        .elect("station-01", Duration::from_secs(60))
        .unwrap();
    registry
        .put_agent_role(
            "operation-01",
            &serde_json::from_value::<AgentRole>(serde_json::json!({
                "id": "case-officer",
                "purpose": "Review case material",
                "policy": "Follow the Operation policy"
            }))
            .unwrap(),
        )
        .unwrap();
    let gateway = Gateway::with_access(
        config,
        Arc::new(registry),
        AccessProviders::new(
            Arc::new(TestAuthenticator),
            Arc::new(TestDirectory),
            Arc::new(TestAuthorizer),
        ),
    );
    let station = OperationId::new("station-01").unwrap();
    let mission = MissionId::new("mission-007").unwrap();
    let session = SessionId::new("provider-session-007").unwrap();
    gateway
        .register_session(token, station.clone(), mission.clone(), session.clone())
        .await
        .unwrap();
    gateway
        .publish_lifecycle(
            token,
            LifecycleEvent {
                operation_id: station,
                mission_id: mission,
                session_id: session,
                state: SessionState::Working,
                reason: None,
                sequence: 1,
            },
        )
        .await
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, stopped) = oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let server = format!("http://{address}");
    let login = reqwest::Client::new()
        .post(format!("{server}/v1/auth/login"))
        .header("origin", "https://exports.example")
        .header("x-forwarded-proto", "https")
        .json(&serde_json::json!({"name": "viewer", "secret": "secret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), reqwest::StatusCode::CREATED);
    let access_token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    (server, shutdown, access_token, directory)
}

async fn ai6ctl(server: String, token: Option<String>, args: &[&str]) -> std::process::Output {
    let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ai6ctl"));
        command
            // A loaded machine can take longer than the five seconds a person would wait.
            .arg("--timeout")
            .arg("120")
            .arg("--server")
            .arg(server)
            .args(args)
            .env_remove("UNEX_ACCESS_TOKEN");
        if let Some(token) = token {
            command.env("UNEX_ACCESS_TOKEN", token);
        }
        command.output().unwrap()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn health_stations_missions_and_session_commands_use_the_public_gateway() {
    let (server, shutdown, access_token, _registry_dir) = start_gateway().await;

    let response = reqwest::get(format!("{server}/health")).await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

    let health = ai6ctl(server.clone(), None, &["health"]).await;
    assert!(health.status.success());
    assert_eq!(
        String::from_utf8(health.stdout).unwrap().trim(),
        "Universal Exports is healthy"
    );

    let stations = ai6ctl(server.clone(), Some(access_token.clone()), &["stations"]).await;
    assert!(
        stations.status.success(),
        "{}",
        String::from_utf8_lossy(&stations.stderr)
    );
    let stations = String::from_utf8(stations.stdout).unwrap();
    assert!(stations.contains("STATION\tHEALTH\tHQS\tRUNNING/CAPACITY"));
    assert!(stations.contains("station-01\thealthy\tyes\t1/4\texecution\toperation-01"));

    let station = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &["station", "station-01"],
    )
    .await;
    assert!(
        station.status.success(),
        "{}",
        String::from_utf8_lossy(&station.stderr)
    );
    assert!(
        String::from_utf8(station.stdout)
            .unwrap()
            .contains("station-01\thealthy\tyes\t1/4")
    );

    let roles = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &["agent-roles", "--operation", "operation-01"],
    )
    .await;
    assert!(
        roles.status.success(),
        "{}",
        String::from_utf8_lossy(&roles.stderr)
    );
    let roles = String::from_utf8(roles.stdout).unwrap();
    assert!(roles.contains("ROLE\tOPERATION_OVERRIDE\tPURPOSE\tPOLICY"));
    assert!(
        roles.contains("case-officer\ttrue\tReview case material\tFollow the Operation policy")
    );

    let missing_station = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &["station", "not-a-station"],
    )
    .await;
    assert_eq!(missing_station.status.code(), Some(1));
    assert!(
        String::from_utf8(missing_station.stderr)
            .unwrap()
            .contains("Station was not found")
    );

    let missions = ai6ctl(server.clone(), Some(access_token.clone()), &["missions"]).await;
    assert!(
        missions.status.success(),
        "{}",
        String::from_utf8_lossy(&missions.stderr)
    );
    let missions = String::from_utf8(missions.stdout).unwrap();
    assert!(missions.contains("STATION\tMISSION\tSESSIONS"));
    assert!(missions.contains("station-01\tmission-007\t1"));

    let mission = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &["mission", "mission-007"],
    )
    .await;
    assert!(mission.status.success());
    let mission = String::from_utf8(mission.stdout).unwrap();
    assert!(mission.contains("SESSION\tSTATE\tREASON\tSEQUENCE\tEXPOSURE"));
    assert!(mission.contains("provider-session-007\tworking\t-\t1\t-"));

    let sessions = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &["sessions", "--state", "working"],
    )
    .await;
    assert!(
        sessions.status.success(),
        "{}",
        String::from_utf8_lossy(&sessions.stderr)
    );
    let sessions = String::from_utf8(sessions.stdout).unwrap();
    assert!(sessions.contains("STATION\tMISSION\tSESSION\tSTATE\tSEQUENCE\tEXPOSURE"));
    assert!(sessions.contains("station-01\tmission-007\tprovider-session-007\tworking\t1\t-"));

    let session = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &["session", "provider-session-007"],
    )
    .await;
    assert!(session.status.success());
    assert!(
        String::from_utf8(session.stdout)
            .unwrap()
            .contains("provider-session-007")
    );

    let missing = ai6ctl(
        server.clone(),
        Some(access_token),
        &["session", "not-a-session"],
    )
    .await;
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        String::from_utf8(missing.stderr)
            .unwrap()
            .contains("Session was not found")
    );

    let no_token = ai6ctl(server.clone(), None, &["sessions"]).await;
    assert_eq!(no_token.status.code(), Some(1));
    assert!(
        String::from_utf8(no_token.stderr)
            .unwrap()
            .contains("UNEX_ACCESS_TOKEN")
    );

    let no_mission_token = ai6ctl(server.clone(), None, &["missions"]).await;
    assert_eq!(no_mission_token.status.code(), Some(1));
    assert!(
        String::from_utf8(no_mission_token.stderr)
            .unwrap()
            .contains("UNEX_ACCESS_TOKEN")
    );

    let _ = shutdown.send(());
}

const STATION_TOKEN: &str = "exports-test-credential-with-more-than-32-bytes";

#[tokio::test]
async fn mission_prints_the_reason_a_station_gave_for_a_sessions_state() {
    let (server, shutdown, access_token, _registry_dir) = start_gateway().await;
    let published = reqwest::Client::new()
        .post(format!(
            "{server}/v1/station/sessions/provider-session-007/lifecycle/2"
        ))
        .bearer_auth(STATION_TOKEN)
        .json(&serde_json::json!({
            "operation_id": "station-01",
            "mission_id": "mission-007",
            "state": "done",
            "reason": "permission-denied",
        }))
        .send()
        .await
        .unwrap();
    assert!(published.status().is_success(), "{}", published.status());

    let mission = ai6ctl(server, Some(access_token), &["mission", "mission-007"]).await;
    assert!(
        mission.status.success(),
        "{}",
        String::from_utf8_lossy(&mission.stderr)
    );
    let mission = String::from_utf8(mission.stdout).unwrap();
    assert!(
        mission.contains("provider-session-007\tdone\tpermission-denied\t2\t-"),
        "{mission}"
    );
    let _ = shutdown.send(());
}

fn decode_control(schema: &[u8], frame: &[u8]) -> MissionControl {
    let mut receiver = FrameReceiver::new();
    receiver.accept(schema).unwrap();
    match receiver.accept(frame).unwrap() {
        Some(IncomingFrame::Control(control)) => control,
        other => panic!("the gateway must send a control frame, not {other:?}"),
    }
}

/// Plays the Station: takes each control queued for `station-01` and reports the outcome a
/// real Station would give the Mission the test drives through its states.
async fn fake_station(server: String, stop: tokio::sync::watch::Receiver<bool>) {
    let client = reqwest::Client::new();
    let template = MissionControl::new(
        ControlId::new("schema").unwrap(),
        OperationId::new("schema").unwrap(),
        MissionId::new("schema").unwrap(),
        ControlAction::Cancel,
    );
    let schema = control_frames(&template).unwrap().0;
    while !*stop.borrow() {
        let Ok(response) = client
            .get(format!(
                "{server}/v1/station/controls/next?station=station-01&wait_ms=100"
            ))
            .bearer_auth(STATION_TOKEN)
            .send()
            .await
        else {
            continue;
        };
        if response.status() != reqwest::StatusCode::OK {
            continue;
        }
        let frame = response.bytes().await.unwrap();
        let control = decode_control(&schema, &frame);
        let mut report = match control.action() {
            ControlAction::Cancel => {
                serde_json::json!({"outcome": "applied", "state": "abandoned", "reason": null})
            }
            ControlAction::Retry => {
                assert_eq!(control.agent(), Some("008"));
                serde_json::json!({"outcome": "applied", "state": "queued", "reason": null})
            }
            ControlAction::Resume => {
                assert_eq!(control.message(), Some("carry on"));
                serde_json::json!({"outcome": "applied", "state": "working", "reason": null})
            }
            _ => serde_json::json!({
                "outcome": "rejected",
                "state": "blocked",
                "reason": "only a working Session can be interrupted"
            }),
        };
        report["station"] = serde_json::json!("station-01");
        let reported = client
            .post(format!(
                "{server}/v1/station/controls/{}/result",
                control.control_id()
            ))
            .bearer_auth(STATION_TOKEN)
            .json(&report)
            .send()
            .await
            .unwrap();
        assert_eq!(reported.status(), reqwest::StatusCode::NO_CONTENT);
    }
}

#[tokio::test]
async fn mission_and_session_controls_reach_the_station_and_report_its_outcome() {
    let (server, shutdown, access_token, _registry_dir) = start_gateway().await;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let station = tokio::spawn(fake_station(server.clone(), stopped));
    let token = Some(access_token);

    let cancel = ai6ctl(
        server.clone(),
        token.clone(),
        &["mission", "cancel", "mission-007"],
    )
    .await;
    assert!(
        cancel.status.success(),
        "{}",
        String::from_utf8_lossy(&cancel.stderr)
    );
    let cancel = String::from_utf8(cancel.stdout).unwrap();
    assert!(cancel.contains("CONTROL\tACTION\tMISSION\tSTATION\tSTATUS\tSTATE\tREASON"));
    assert!(cancel.contains("\tcancel\tmission-007\tstation-01\tapplied\tabandoned\t-"));

    let retry_args = [
        "mission",
        "retry",
        "mission-007",
        "--agent",
        "008",
        "--operation",
        "station-01",
        "--idempotency-key",
        "retry-1",
    ];
    let retry = ai6ctl(server.clone(), token.clone(), &retry_args).await;
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    let retry = String::from_utf8(retry.stdout).unwrap();
    assert!(retry.contains("\tretry\tmission-007\tstation-01\tapplied\tqueued\t-"));
    // The same key returns the same control rather than asking the Station again.
    let repeated = ai6ctl(server.clone(), token.clone(), &retry_args).await;
    assert_eq!(
        String::from_utf8(repeated.stdout).unwrap().lines().nth(1),
        retry.lines().nth(1)
    );

    let resume = ai6ctl(
        server.clone(),
        token.clone(),
        &["mission", "resume", "mission-007", "--message", "carry on"],
    )
    .await;
    assert!(
        resume.status.success(),
        "{}",
        String::from_utf8_lossy(&resume.stderr)
    );
    assert!(
        String::from_utf8(resume.stdout)
            .unwrap()
            .contains("\tresume\tmission-007\tstation-01\tapplied\tworking\t-")
    );

    let interrupt = ai6ctl(
        server.clone(),
        token.clone(),
        &["session", "interrupt", "provider-session-007"],
    )
    .await;
    assert_eq!(interrupt.status.code(), Some(1));
    assert!(String::from_utf8(interrupt.stdout).unwrap().contains(
        "\tinterrupt\tmission-007\tstation-01\trejected\tblocked\tonly a working Session can be interrupted"
    ));
    assert!(
        String::from_utf8(interrupt.stderr)
            .unwrap()
            .contains("the Station rejected the control")
    );

    let unknown = ai6ctl(
        server.clone(),
        token.clone(),
        &["mission", "cancel", "mission-unseen"],
    )
    .await;
    assert_eq!(unknown.status.code(), Some(1));
    assert!(
        String::from_utf8(unknown.stderr)
            .unwrap()
            .contains("pass --operation")
    );

    let _ = stop.send(true);
    station.await.unwrap();
    let _ = shutdown.send(());
}

/// A green ink request from Agent 007 in operation-01, waiting for an hour.
fn asked(registry: &FileRegistry, target: &str, key: &str) -> ai6::ApprovalRequest {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let request = ai6::ApprovalRequest::try_new(
        "operation-01",
        ai6::MissionId::try_new("mission-007").unwrap(),
        ai6::SessionId::try_new("provider-session-007").unwrap(),
        "007",
        ai6::ActionScope::try_new("push", target).unwrap(),
        "the release branch is green",
        key,
        &Default::default(),
    )
    .unwrap()
    .expiring(now, Duration::from_secs(3600));
    registry.request_approval(&request).unwrap();
    request
}

#[tokio::test]
async fn pools_lists_an_operations_accounts_and_their_quota() {
    use ai6::directorates::registry::backend::*;

    let (server, shutdown, token, directory) = start_gateway().await;
    let registry = FileRegistry::open(directory.path());
    let pool = ai6::AccountPool::new("team", "claude")
        .account(ai6::Account::new("alpha", ai6::AccountKind::ClaudeOauthToken).max_concurrent(2))
        .account(ai6::Account::new("beta", ai6::AccountKind::AnthropicApiKey));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        .floor();
    for name in ["alpha", "beta"] {
        let account = ai6::AccountRef::new("team", name);
        let profile =
            ai6::AccountProfile::of(&pool, name, vec!["operation-01".to_owned()]).unwrap();
        registry
            .declare_account(&account, "station-01", &profile)
            .unwrap();
    }
    let alpha = ai6::AccountRef::new("team", "alpha");
    registry
        .report_quota(
            &alpha,
            &[ai6::QuotaWindow::new("five_hour", now).used_permille(425)],
        )
        .unwrap();
    registry.limit_account(&alpha, now + 600.0).unwrap();
    registry
        .record_account_usage(&alpha, 900, now as i64)
        .unwrap();

    let output = ai6ctl(
        server.clone(),
        Some(token.clone()),
        &["pools", "--operation", "operation-01"],
    )
    .await;
    let printed = lines(&output);
    assert_eq!(
        printed[0],
        "POOL\tACCOUNT\tKIND\tSESSIONS\tWINDOWS\tLIMITED_UNTIL_UNIX\tTOKENS_TODAY"
    );
    assert_eq!(
        printed[1],
        format!(
            "team\talpha\tclaude-oauth-token\t0/2\tfive_hour=42.5%\t{:.0}\t900",
            now + 600.0
        )
    );
    assert_eq!(printed[2], "team\tbeta\tanthropic-api-key\t0\t-\t-\t0");
    let refused = ai6ctl(
        server.clone(),
        None,
        &["pools", "--operation", "operation-01"],
    )
    .await;
    assert!(!refused.status.success());
    let cleared = lines(
        &ai6ctl(
            server.clone(),
            Some(token.clone()),
            &[
                "pools",
                "clear-limit",
                "team",
                "alpha",
                "--operation",
                "operation-01",
            ],
        )
        .await,
    );
    assert_eq!(cleared, ["team/alpha: limit cleared"]);
    assert_eq!(
        registry
            .accounts()
            .unwrap()
            .into_iter()
            .find(|state| state.account == alpha)
            .unwrap()
            .limited_until,
        None
    );
    let _ = shutdown.send(());
}

#[tokio::test]
async fn agents_lists_each_agents_record_in_the_operation() {
    let (server, shutdown, token, directory) = start_gateway().await;
    let registry = FileRegistry::open(directory.path());
    registry
        .check_in(
            "station-01",
            BTreeMap::from([
                ("operations".to_owned(), serde_json::json!(["operation-01"])),
                (
                    "agent_clearances".to_owned(),
                    serde_json::json!({"operation-01": {"007": {
                        "clearance": "field", "configured": "field", "aliases": []
                    }}}),
                ),
            ]),
        )
        .unwrap();
    for (key, outcome) in [
        ("m-1#3", ai6::AgentOutcome::Done),
        ("m-2#3", ai6::AgentOutcome::Done),
        ("m-3#2", ai6::AgentOutcome::Stalled),
        ("m-4#1#lost", ai6::AgentOutcome::Lost),
    ] {
        registry
            .record_agent_outcome(
                &ai6::AgentOutcomeReport::new(
                    "007",
                    Some("operation-01".to_owned()),
                    outcome,
                    key,
                    1_000.0,
                )
                .with_model(Some("opus".to_owned())),
            )
            .unwrap();
    }
    let printed = lines(
        &ai6ctl(
            server.clone(),
            Some(token.clone()),
            &["agents", "--operation", "operation-01"],
        )
        .await,
    );
    assert_eq!(
        printed,
        [
            "AGENT\tPOSTED\tDONE\tFAILED\tBLOWN\tABANDONED\tSTALLED\tLOST\tQUARANTINED\tBUDGET_STOPS\tGREEN_INK_REJECTED\tLAST_MISSION_UNIX\tMODEL\tRETIREMENT\tCLEARANCE\tPROMOTION\tHOLDS",
            "007\tfalse\t2\t0\t0\t0\t1\t1\t0\t0\t0\t1000\topus\t-\tfield\t-\t-",
        ]
    );
    let lowered = lines(
        &ai6ctl(
            server.clone(),
            Some(token.clone()),
            &[
                "agents",
                "lower",
                "007",
                "desk",
                "--reason",
                "under review",
                "--operation",
                "operation-01",
            ],
        )
        .await,
    );
    assert_eq!(lowered, ["007: held at desk in operation-01"]);
    let grant = registry
        .clearance_grant("operation-01", "007")
        .unwrap()
        .unwrap();
    assert_eq!(
        (grant.signer.as_str(), grant.reason.as_deref()),
        ("viewer-007", Some("under review"))
    );
    let restored = lines(
        &ai6ctl(
            server.clone(),
            Some(token.clone()),
            &["agents", "restore", "007", "--operation", "operation-01"],
        )
        .await,
    );
    assert_eq!(
        restored,
        ["007: grant to desk removed; configured clearance applies in operation-01"]
    );
    let missing = ai6ctl(
        server.clone(),
        Some(token.clone()),
        &["agents", "restore", "007", "--operation", "operation-01"],
    )
    .await;
    assert!(!missing.status.success());
    let _ = shutdown.send(());
}

#[tokio::test]
async fn models_prints_what_each_station_found_and_why_a_handler_could_not_say() {
    let (server, shutdown, token, directory) = start_gateway().await;
    let registry = FileRegistry::open(directory.path());
    registry
        .check_in(
            "station-02",
            BTreeMap::from([(
                "models".to_owned(),
                serde_json::json!([
                    {"source": "team/alpha", "pool": "team", "account": "alpha", "handler": "claude", "at": 20.0,
                     "listing": {"known": true, "models": [{"id": "opus"}, {"id": "haiku", "name": "Haiku"}]}},
                    {"source": "007/acp/-", "agent": "007", "handler": "acp", "at": 10.0,
                     "listing": {"known": false, "unknown_because": "this handler has no way to list models", "models": []}},
                ]),
            )]),
        )
        .unwrap();
    let printed = lines(&ai6ctl(server.clone(), Some(token.clone()), &["models"]).await);
    assert_eq!(
        printed,
        [
            "STATION\tSOURCE\tHANDLER\tKNOWN\tMODELS\tUNKNOWN_BECAUSE",
            "station-02\tteam/alpha\tclaude\ttrue\topus,haiku\t-",
            "station-02\t007/acp/-\tacp\tfalse\t-\tthis handler has no way to list models",
        ]
    );
    let _ = shutdown.send(());
}

#[tokio::test]
async fn agents_lift_ends_a_hold_on_the_agent_or_on_one_route() {
    let (server, shutdown, token, directory) = start_gateway().await;
    let registry = FileRegistry::open(directory.path());
    let now = 1_800_000_000.0;
    let route = ai6::RouteRef::new("claude", Some("opus".to_owned()), None);
    registry
        .put_hold(&ai6::RosterHold::new(
            "operation-01",
            "007",
            None,
            "failing",
            "m",
            now,
        ))
        .unwrap();
    registry
        .put_hold(&ai6::RosterHold::new(
            "operation-01",
            "007",
            Some(route),
            "slow",
            "alice",
            now,
        ))
        .unwrap();
    registry
        .record_agent_outcome(&ai6::AgentOutcomeReport::new(
            "007",
            Some("operation-01".to_owned()),
            ai6::AgentOutcome::Done,
            "m-1#1#done",
            1_000.0,
        ))
        .unwrap();
    let listed = lines(
        &ai6ctl(
            server.clone(),
            Some(token.clone()),
            &["agents", "--operation", "operation-01"],
        )
        .await,
    );
    assert!(
        listed[1].ends_with("\tagent:m,claude/opus/-:alice"),
        "{}",
        listed[1]
    );
    let whole = lines(
        &ai6ctl(
            server.clone(),
            Some(token.clone()),
            &["agents", "lift", "007", "--operation", "operation-01"],
        )
        .await,
    );
    assert_eq!(whole, ["007: hold by m lifted in operation-01: failing"]);
    let one = lines(
        &ai6ctl(
            server.clone(),
            Some(token.clone()),
            &[
                "agents",
                "lift",
                "007",
                "--handler",
                "claude",
                "--model",
                "opus",
                "--operation",
                "operation-01",
            ],
        )
        .await,
    );
    assert_eq!(one, ["007: hold by alice lifted in operation-01: slow"]);
    assert!(registry.holds("operation-01").unwrap().is_empty());
    let again = ai6ctl(
        server.clone(),
        Some(token.clone()),
        &["agents", "lift", "007", "--operation", "operation-01"],
    )
    .await;
    assert!(!again.status.success(), "no hold left");
    let _ = shutdown.send(());
}

#[tokio::test]
async fn agents_scorecard_prints_each_row_and_the_attempts_behind_a_judgment() {
    let (server, shutdown, token, directory) = start_gateway().await;
    let registry = FileRegistry::open(directory.path());
    for (mission, state) in [("m-1", "done"), ("m-2", "failed"), ("m-3", "abandoned")] {
        let record = registry
            .open_session(
                ai6::NewSession::new(format!("{mission}.1"), mission, "station-01")
                    .with_operation("operation-01")
                    .with_launch(
                        ai6::SessionLaunch::new("claude", "pipe", "/w")
                            .with_model(Some("opus".to_owned()), None)
                            .with_agent(Some("007".to_owned())),
                    ),
                Duration::from_secs(60),
            )
            .unwrap();
        registry
            .update_session(
                &record.session_id,
                "station-01",
                record.fence,
                ai6::SessionUpdate::default()
                    .result(ai6::SessionResult::new(state))
                    .status(ai6::SessionStatus::Ended),
            )
            .unwrap();
    }
    let printed = lines(
        &ai6ctl(
            server.clone(),
            Some(token.clone()),
            &[
                "agents",
                "scorecard",
                "--operation",
                "operation-01",
                "--show",
                "failed",
            ],
        )
        .await,
    );
    assert!(printed[0].starts_with("# the last 30 days;"), "{printed:?}");
    assert!(printed[1].starts_with("AGENT\tROUTE\tJUDGED\tFAILURE_RATE\tSAMPLE\tDONE\tFAILED\t"));
    assert!(
        printed[2]
            .starts_with("007\tall\t2\t50%\tSMALL\t1\t1\t0\t0\t0\t0\t0\t0\t0\t0\t1\t0\t0\t0\t"),
        "{}",
        printed[2]
    );
    assert!(printed[3].starts_with("007\tclaude opus -\t2\t50%\tSMALL\t"));
    assert!(printed[4].starts_with("(model)\topus\t2\t"));
    assert_eq!(
        &printed[5..],
        [
            "AGENT\tROUTE\tJUDGMENT\tMISSION\tSESSION",
            "007\tall\tfailed\tm-2\tm-2.1",
            "007\tclaude opus -\tfailed\tm-2\tm-2.1",
            "(model)\topus\tfailed\tm-2\tm-2.1",
        ]
    );
    let _ = shutdown.send(());
}

/// The lines a successful ai6ctl run printed.
fn lines(output: &std::process::Output) -> Vec<String> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn green_ink_is_listed_signed_refused_and_revoked_and_class_authorisations_revoked() {
    let (server, shutdown, token, directory) = start_gateway().await;
    let registry = FileRegistry::open(directory.path());
    let main = asked(&registry, "origin:refs/heads/main", "push-main");
    let tag = asked(&registry, "origin:refs/tags/v1.0.0", "push-tag");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let class = ai6::ClassAuthorization::new(
        "operation-01",
        vec!["push".to_owned()],
        "origin:refs/heads/ai6/*",
        "operator-m",
        now,
        now + 3600.0,
    )
    .for_agents(vec!["007".to_owned()])
    .with_max_uses(3);
    registry.put_class_authorization(&class).unwrap();
    let run = |args: Vec<String>| {
        let (server, token) = (server.clone(), token.clone());
        async move {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            ai6ctl(server, Some(token), &args).await
        }
    };
    let words = |text: &str| text.split(' ').map(str::to_owned).collect::<Vec<_>>();
    let decide = |action: &str, id: &str, extra: &[&str]| {
        let mut args = words(&format!("green-ink {action} {id} --operation operation-01"));
        args.extend(extra.iter().map(|arg| (*arg).to_owned()));
        args
    };

    let waiting = lines(&run(words("green-ink list --operation operation-01")).await);
    assert_eq!(
        waiting[0],
        "REQUEST\tMISSION\tAGENT\tSTEP\tTARGET\tEXPIRES_UNIX\tREASON"
    );
    assert_eq!(waiting.len(), 3, "{waiting:?}");
    let main_row = waiting
        .iter()
        .find(|row| row.starts_with(&main.id))
        .unwrap();
    assert!(
        main_row.starts_with(&format!(
            "{}\tmission-007\t007\tpush\torigin:refs/heads/main\t",
            main.id
        )) && main_row.ends_with("\tthe release branch is green"),
        "{main_row}"
    );

    let approved = lines(
        &run(decide(
            "approve",
            &main.id,
            &["--expires-in", "600", "--note", "main only"],
        ))
        .await,
    );
    assert_eq!(
        approved[0],
        "REQUEST\tSTEP\tTARGET\tDECISION\tSIGNER\tEXPIRES_UNIX\tREVOKED_BY"
    );
    let fields: Vec<&str> = approved[1].split('\t').collect();
    assert_eq!(
        (
            fields[0], fields[1], fields[2], fields[3], fields[4], fields[6]
        ),
        (
            main.id.as_str(),
            "push",
            "origin:refs/heads/main",
            "approved",
            "viewer-007",
            "-"
        )
    );
    let lasts = fields[5].parse::<f64>().unwrap() - now;
    assert!((590.0..=700.0).contains(&lasts), "{lasts}");

    let rejected = lines(&run(decide("reject", &tag.id, &["--reason", "not yet"])).await);
    assert!(
        rejected[1].ends_with("\trejected\tviewer-007\t-\t-"),
        "{rejected:?}"
    );

    let again = run(decide("reject", &main.id, &[])).await;
    assert_eq!(again.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("409"),
        "a decided request stays decided"
    );

    let empty = lines(&run(words("green-ink list --operation operation-01")).await);
    assert_eq!(empty.len(), 1, "nothing waits: {empty:?}");

    let revoked = lines(&run(decide("revoke", &main.id, &[])).await);
    assert!(revoked[1].ends_with("\tviewer-007"), "{revoked:?}");
    assert!(
        registry
            .live_approval("operation-01", "mission-007", &main.scope, now + 1.0)
            .unwrap()
            .is_none()
    );

    let classes = lines(&run(words("class-authorizations list --operation operation-01")).await);
    assert_eq!(
        classes[0],
        "CLASS\tKINDS\tTARGET\tAGENTS\tROLES\tCLEARANCE\tUSES\tEXPIRES_UNIX\tSTATE"
    );
    assert!(
        classes[1].starts_with(&format!(
            "{}\tpush\torigin:refs/heads/ai6/*\t007\t-\tfield\t0/3\t",
            class.id
        )) && classes[1].ends_with("\tlive"),
        "{classes:?}"
    );
    let revoked_class = lines(
        &run(words(&format!(
            "class-authorizations revoke {} --operation operation-01",
            class.id
        )))
        .await,
    );
    assert!(revoked_class[1].ends_with("\trevoked"), "{revoked_class:?}");

    let audit = registry.audit_records().unwrap();
    for (action, target) in [
        (
            "green_ink.approved",
            format!("{} push origin:refs/heads/main", main.id),
        ),
        (
            "green_ink.rejected",
            format!("{} push origin:refs/tags/v1.0.0", tag.id),
        ),
        (
            "green_ink.revoked",
            format!("{} push origin:refs/heads/main", main.id),
        ),
        (
            "class_authorization.revoked",
            format!("{} push origin:refs/heads/ai6/*", class.id),
        ),
    ] {
        assert!(
            audit.iter().any(|record| record.action == action
                && record.actor == "viewer-007"
                && record.target == target),
            "{action}: {audit:?}"
        );
    }
    assert!(
        audit
            .iter()
            .all(|record| !record.target.contains("release branch is green")),
        "the Agent's reason is never audited"
    );
    let _ = shutdown.send(());
}

/// A loopback stand-in that answers `GET /health` with `{}`, and the URL it is reached at.
async fn health_stand_in() -> (String, oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().route(
        "/health",
        axum::routing::get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }),
    );
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = stopped.await;
            })
            .await;
    });
    (url, stop)
}

#[tokio::test]
async fn the_origin_comes_from_the_flag_then_unex_url_then_ai6_cfg() {
    let (url, stop) = health_stand_in().await;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("ai6.cfg");
    std::fs::write(
        &config,
        format!(
            "[unex]\nurl = \"{url}\"\n\
             station_token = {{ provider = \"inline\", value = \"never-read\" }}\n"
        ),
    )
    .unwrap();
    let health = |extra: Vec<String>, unex_url: Option<String>| {
        let config = config.clone();
        tokio::task::spawn_blocking(move || {
            let mut command = Command::new(env!("CARGO_BIN_EXE_ai6ctl"));
            command
                .arg("health")
                .args(extra)
                .env("AI6_CONFIG", &config)
                .env_remove("UNEX_URL")
                .env_remove("UNEX_ACCESS_TOKEN");
            if let Some(unex_url) = unex_url {
                command.env("UNEX_URL", unex_url);
            }
            command.output().unwrap()
        })
    };
    let reached = health(Vec::new(), None).await.unwrap();
    assert!(
        reached.status.success(),
        "ai6.cfg's [unex] url: {reached:?}"
    );

    // Nothing listens on port 9 of loopback: a flag or UNEX_URL that names it wins and fails.
    let closed = "http://127.0.0.1:9".to_owned();
    let flag = health(vec!["--server".to_owned(), closed.clone()], None)
        .await
        .unwrap();
    assert!(!flag.status.success(), "--server beats ai6.cfg");
    let variable = health(Vec::new(), Some(closed)).await.unwrap();
    assert!(!variable.status.success(), "UNEX_URL beats ai6.cfg");
    let _ = stop.send(());
}

/// A gateway over BODYGUARD > FORTITUDE > SOUTH and OTHER, a Mission in SOUTH and in OTHER,
/// and a configuration directory holding those Operations.
async fn start_tree_gateway() -> (String, oneshot::Sender<()>, String, tempfile::TempDir) {
    let token = "exports-test-credential-with-more-than-32-bytes";
    let directory = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(directory.path().join("registry"));
    registry
        .check_in(
            "station-01",
            BTreeMap::from([
                (
                    "operations".to_owned(),
                    serde_json::json!(["SOUTH", "OTHER"]),
                ),
                (
                    "operation_lineage".to_owned(),
                    serde_json::json!({
                        "BODYGUARD": [], "FORTITUDE": ["BODYGUARD"],
                        "SOUTH": ["FORTITUDE", "BODYGUARD"], "OTHER": []
                    }),
                ),
            ]),
        )
        .unwrap();
    let config = directory.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("operations.toml"),
        "[operation.BODYGUARD]\n[operation.BODYGUARD.green_ink]\npreset = \"licensed\"\n\
         [operation.FORTITUDE]\nparent = \"BODYGUARD\"\n\
         [operation.SOUTH]\nparent = \"FORTITUDE\"\npath = \"/srv/south\"\n\
         [operation.OTHER]\npath = \"/srv/other\"\n",
    )
    .unwrap();
    let gateway = Gateway::with_access(
        GatewayConfig::new(
            "https://exports.example",
            token,
            GatewayLimits::default(),
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap(),
        Arc::new(registry),
        AccessProviders::new(
            Arc::new(TestAuthenticator),
            Arc::new(TestDirectory),
            Arc::new(TestAuthorizer),
        ),
    )
    .with_roster(Arc::new(ai6::FileProvider::new(&config)));
    for (operation, mission) in [("SOUTH", "m-south"), ("OTHER", "m-other")] {
        let operation = OperationId::new(operation).unwrap();
        let mission = MissionId::new(mission).unwrap();
        let session = SessionId::new(format!("session-{mission}")).unwrap();
        gateway
            .register_session(token, operation.clone(), mission.clone(), session.clone())
            .await
            .unwrap();
        gateway
            .publish_lifecycle(
                token,
                LifecycleEvent {
                    operation_id: operation,
                    mission_id: mission,
                    session_id: session,
                    state: SessionState::Working,
                    reason: None,
                    sequence: 1,
                },
            )
            .await
            .unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, stopped) = oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let server = format!("http://{address}");
    let login = reqwest::Client::new()
        .post(format!("{server}/v1/auth/login"))
        .header("origin", "https://exports.example")
        .header("x-forwarded-proto", "https")
        .json(&serde_json::json!({"name": "viewer", "secret": "secret"}))
        .send()
        .await
        .unwrap();
    let access_token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    (server, shutdown, access_token, directory)
}

#[tokio::test]
async fn operations_prints_the_tree_shows_an_effective_policy_and_missions_take_a_subtree() {
    let (server, shutdown, token, _directory) = start_tree_gateway().await;

    let tree = lines(&ai6ctl(server.clone(), Some(token.clone()), &["operations"]).await);
    let names: Vec<&str> = tree
        .iter()
        .map(|line| line.split('\t').next().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "OPERATION",
            "BODYGUARD",
            "  FORTITUDE",
            "    SOUTH",
            "OTHER"
        ]
    );
    assert!(tree[1].contains("mission.read"), "{tree:?}");

    let shown = lines(
        &ai6ctl(
            server.clone(),
            Some(token.clone()),
            &["operations", "show", "SOUTH", "--effective"],
        )
        .await,
    );
    assert!(shown.contains(&"parent\tFORTITUDE".to_owned()), "{shown:?}");
    assert!(shown.contains(&"lineage\tFORTITUDE > BODYGUARD".to_owned()));
    assert!(
        shown.contains(&"green_ink.preset = \"licensed\"\tfrom BODYGUARD".to_owned()),
        "{shown:?}"
    );
    assert!(shown.contains(&"path = \"/srv/south\"\town".to_owned()));
    let unknown = ai6ctl(
        server.clone(),
        Some(token.clone()),
        &["operations", "show", "MISSING"],
    )
    .await;
    assert!(!unknown.status.success());

    let subtree = lines(
        &ai6ctl(
            server.clone(),
            Some(token.clone()),
            &["missions", "--operation", "BODYGUARD", "--subtree"],
        )
        .await,
    );
    assert_eq!(
        subtree,
        [
            "STATION\tMISSION\tSESSIONS\tNAME\tUID",
            "SOUTH\tm-south\t1\t-\tm-south"
        ]
    );
    let own = lines(
        &ai6ctl(
            server.clone(),
            Some(token),
            &["missions", "--operation", "BODYGUARD"],
        )
        .await,
    );
    assert_eq!(own, ["STATION\tMISSION\tSESSIONS\tNAME\tUID"]);
    let _ = shutdown.send(());
}

/// A TLS listener that answers every request 204, with a certificate for `name` from a new
/// certificate authority: its origin and the authority's PEM.
fn tls_gateway(name: &str) -> (String, String) {
    use std::io::{Read, Write};

    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_pem = ca.self_signed(&ca_key).unwrap().pem();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let leaf = rcgen::CertificateParams::new(vec![name.to_owned()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .unwrap(),
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            let connection = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(connection, stream);
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                match tls.read(&mut byte) {
                    Ok(1) => request.push(byte[0]),
                    _ => break,
                }
            }
            if request.ends_with(b"\r\n\r\n") {
                let _ = tls.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
                tls.conn.send_close_notify();
                let _ = tls.flush();
            }
        }
    });
    (origin, ca_pem)
}

/// `ai6ctl health` against `server`, reading no bootstrap file but the one named in `args`.
fn health(server: &str, args: &[&str]) -> std::process::Output {
    let empty = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ai6ctl"));
    command
        .arg("--server")
        .arg(server)
        .args(args)
        .arg("health")
        .env_remove("UNEX_CA_FILE")
        .env_remove("AI6_CONFIG")
        .env("XDG_CONFIG_HOME", empty.path())
        .env("APPDATA", empty.path())
        .env("HOME", empty.path());
    command.output().unwrap()
}

#[test]
fn an_enterprise_gateway_is_refused_without_its_ca_and_accepted_with_it() {
    let (server, ca) = tls_gateway("127.0.0.1");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ca.pem");
    std::fs::write(&file, &ca).unwrap();
    let file = file.to_str().unwrap();

    let refused = health(&server, &[]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("could not contact"),
        "{refused:?}"
    );

    let accepted = health(&server, &["--ca-file", file]);
    assert!(accepted.status.success(), "{accepted:?}");
    assert!(String::from_utf8_lossy(&accepted.stdout).contains("healthy"));

    // The bootstrap file's, relative to it; and the flag beats it.
    let config = dir.path().join("ai6.cfg");
    std::fs::write(
        &config,
        format!("[unex]\nurl = \"{server}\"\nca_file = \"ca.pem\"\n"),
    )
    .unwrap();
    let config = config.to_str().unwrap();
    let from_file = health(&server, &["--config", config]);
    assert!(from_file.status.success(), "{from_file:?}");
    let other = dir.path().join("other.pem");
    let (_, other_ca) = tls_gateway("127.0.0.1");
    std::fs::write(&other, other_ca).unwrap();
    let flag_wins = health(
        &server,
        &["--config", config, "--ca-file", other.to_str().unwrap()],
    );
    assert!(!flag_wins.status.success(), "{flag_wins:?}");
}

#[test]
fn a_ca_does_not_vouch_for_a_gateway_of_another_name() {
    let (server, ca) = tls_gateway("other.example");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ca.pem");
    std::fs::write(&file, &ca).unwrap();
    let refused = health(&server, &["--ca-file", file.to_str().unwrap()]);
    assert!(!refused.status.success(), "{refused:?}");
}

#[test]
fn a_ca_file_that_trusts_nothing_is_refused_naming_it_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "").unwrap();
    for file in [empty, dir.path().join("missing.pem")] {
        let refused = health("http://127.0.0.1:9", &["--ca-file", file.to_str().unwrap()]);
        let said = String::from_utf8_lossy(&refused.stderr).into_owned();
        assert!(!refused.status.success(), "{said}");
        assert!(said.contains(file.to_str().unwrap()), "{said}");
        assert!(!said.contains("could not contact"), "{said}");
    }
}

#[tokio::test]
async fn ai6ctl_starts_from_the_one_file_every_program_shares_and_refuses_a_key_nobody_knows() {
    let (url, stop) = health_stand_in().await;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("ai6.cfg");
    let text = include_str!("../../ai6/tests/one_file.cfg")
        .replace("UNEX_URL", &url)
        .replace("GATEWAY_LISTEN", "127.0.0.1:9")
        .replace("STATION_LISTEN", "127.0.0.1:9");
    let health = |document: String| {
        let config = config.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::write(&config, document).unwrap();
            Command::new(env!("CARGO_BIN_EXE_ai6ctl"))
                .arg("health")
                .arg("--config")
                .arg(&config)
                .env_remove("UNEX_URL")
                .env_remove("UNEX_ACCESS_TOKEN")
                .output()
                .unwrap()
        })
    };
    let reached = health(text.clone()).await.unwrap();
    assert!(reached.status.success(), "{reached:?}");
    for (document, key) in [
        (
            format!("nobody_knows = \"pl4nted\"\n{text}"),
            "nobody_knows",
        ),
        (
            text.replace("[station]", "[station]\nnobody = \"pl4nted\""),
            "station.nobody",
        ),
    ] {
        let refused = health(document).await.unwrap();
        let said = String::from_utf8_lossy(&refused.stderr).into_owned();
        assert_eq!(refused.status.code(), Some(1), "{said}");
        assert!(said.contains(key), "names {key}: {said}");
        assert!(!said.contains("pl4nted"), "{said}");
    }
    let _ = stop.send(());
}

#[tokio::test]
async fn a_mission_is_found_by_any_handle_it_had_and_renamed_through_the_gateway() {
    let (server, shutdown, access_token, registry_dir) = start_gateway().await;
    let registry = FileRegistry::open(registry_dir.path());
    let brief: ai6::Brief = serde_json::from_value(serde_json::json!({
        "title": "Fix it", "id": "fix", "operation": "station-01"
    }))
    .unwrap();
    let entered = registry
        .enter_brief(&brief, &ai6::MissionEntry::person(Some("station-01")))
        .unwrap();
    let uid = entered.key();
    assert_eq!(entered.handle.as_deref(), Some("station-01-1"));
    let registered = reqwest::Client::new()
        .post(format!("{server}/v1/station/sessions"))
        .bearer_auth("exports-test-credential-with-more-than-32-bytes")
        .json(&serde_json::json!({
            "operation_id": "station-01", "mission_id": uid, "session_id": "provider-session-8"
        }))
        .send()
        .await
        .unwrap();
    assert!(registered.status().is_success());

    let renamed = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &["missions", "rename", "station-01-1", "station-01-fix"],
    )
    .await;
    assert!(
        renamed.status.success(),
        "{}",
        String::from_utf8_lossy(&renamed.stderr)
    );
    assert_eq!(
        String::from_utf8(renamed.stdout).unwrap().trim(),
        format!("{uid}: station-01-1 is now station-01-fix")
    );

    for id in ["station-01-1", "STATION-01-FIX", uid.as_str()] {
        let shown = ai6ctl(server.clone(), Some(access_token.clone()), &["mission", id]).await;
        assert!(
            shown.status.success(),
            "{id}: {}",
            String::from_utf8_lossy(&shown.stderr)
        );
        let shown = String::from_utf8(shown.stdout).unwrap();
        assert!(
            shown.contains("STATION\tMISSION\tSESSIONS\tNAME\tUID"),
            "{shown}"
        );
        assert!(
            shown.contains(&format!("station-01\tstation-01-fix\t1\t-\t{uid}")),
            "{id}: {shown}"
        );
    }
    let sessions = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &["sessions", "--mission", "station-01-1"],
    )
    .await;
    let sessions = String::from_utf8(sessions.stdout).unwrap();
    assert!(sessions.contains("provider-session-8"), "{sessions}");
    assert!(!sessions.contains("provider-session-007"), "{sessions}");

    let other = registry
        .enter_brief(&brief, &ai6::MissionEntry::person(Some("station-01")))
        .unwrap();
    let refused = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &[
            "missions",
            "rename",
            &other.key(),
            "station-01-1",
            "--operation",
            "station-01",
        ],
    )
    .await;
    assert_eq!(refused.status.code(), Some(1));
    let said = String::from_utf8(refused.stderr).unwrap();
    assert!(said.contains("409"), "{said}");
    assert!(said.contains("is taken by another Mission"), "{said}");
    let _ = shutdown.send(());
}

#[tokio::test]
async fn a_queued_mission_is_withdrawn_with_a_note_and_a_placed_one_is_told_to_cancel() {
    let (server, shutdown, access_token, registry_dir) = start_gateway().await;
    let registry = FileRegistry::open(registry_dir.path());
    let brief = |id: &str| -> ai6::Brief {
        serde_json::from_value(serde_json::json!({
            "title": "Never placed", "id": id, "operation": "station-01"
        }))
        .unwrap()
    };
    let queued = registry
        .enter_brief(
            &brief("queued"),
            &ai6::MissionEntry::person(Some("station-01")),
        )
        .unwrap();
    let withdrawn = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &[
            "mission",
            "withdraw",
            &queued.key(),
            "--operation",
            "station-01",
            "--note",
            "pinned to a Station that is gone",
        ],
    )
    .await;
    assert!(
        withdrawn.status.success(),
        "{}",
        String::from_utf8_lossy(&withdrawn.stderr)
    );
    assert_eq!(
        String::from_utf8(withdrawn.stdout).unwrap(),
        format!(
            "MISSION\tSTATE\tREASON\n{}\twithdrawn\twithdrawn by viewer-007: pinned to a Station \
             that is gone\n",
            queued.key()
        )
    );
    let filed = registry.filed_brief(&queued.key()).unwrap().unwrap();
    assert_eq!(filed.state, "failed");

    let placed = registry
        .enter_brief(
            &brief("placed"),
            &ai6::MissionEntry::person(Some("station-01")),
        )
        .unwrap();
    registry
        .claim_with_lease(
            Some(&placed.key()),
            "station-01",
            Duration::from_secs(60),
            BTreeMap::new(),
        )
        .unwrap()
        .unwrap();
    let refused = ai6ctl(
        server.clone(),
        Some(access_token.clone()),
        &[
            "mission",
            "withdraw",
            &placed.key(),
            "--operation",
            "station-01",
        ],
    )
    .await;
    assert_eq!(refused.status.code(), Some(1));
    let said = String::from_utf8(refused.stderr).unwrap();
    assert!(
        said.contains("409") && said.contains("`mission cancel`"),
        "{said}"
    );
    let _ = shutdown.send(());
}

#[tokio::test]
async fn a_verifying_mission_is_probed_and_decided_from_the_command_line() {
    let (server, shutdown, access_token, registry_dir) = start_gateway().await;
    let registry = FileRegistry::open(registry_dir.path());
    // The board's Mission `mission-007` is `station-01`'s; a Station must serve that Operation.
    registry
        .check_in(
            "worker-01",
            BTreeMap::from([("operations".to_owned(), serde_json::json!(["station-01"]))]),
        )
        .unwrap();
    let verifying = |key: &str, state: &str| {
        let opened = registry
            .open_session(
                ai6::NewSession::new(format!("{key}-run"), key, "worker-01")
                    .with_operation("station-01"),
                Duration::from_secs(60),
            )
            .unwrap();
        registry
            .update_session(
                &opened.session_id,
                "worker-01",
                opened.fence,
                ai6::SessionUpdate::default()
                    .result(ai6::SessionResult::new(state))
                    .deliverables(vec![
                        ai6::directorates::m::deliverables::Deliverable::parse("branch:fix-1")
                            .unwrap(),
                    ]),
            )
            .unwrap();
    };
    let key = "mission-007";
    verifying(key, "verifying");
    let run = |args: Vec<&'static str>, key: String| {
        let (server, token) = (server.clone(), access_token.clone());
        async move {
            let mut full = vec!["missions", args[0], key.as_str()];
            full.extend_from_slice(&args[1..]);
            ai6ctl(server, Some(token), &full).await
        }
    };

    // Asked and not answered: the Station has not run it.
    let asked = run(
        vec!["probe", "--operation", "station-01", "--wait", "1"],
        key.to_owned(),
    )
    .await;
    assert!(
        asked.status.success(),
        "{}",
        String::from_utf8_lossy(&asked.stderr)
    );
    let said = String::from_utf8(asked.stdout).unwrap();
    assert!(
        said.contains("no findings yet") && said.contains("has not answered"),
        "{said}"
    );
    let record = registry.latest_session(key).unwrap().unwrap();
    assert!(record.probe_requested.is_some());

    // The Station's findings are printed.
    registry
        .update_session(
            &record.session_id,
            "worker-01",
            record.fence,
            ai6::SessionUpdate::default()
                .evidence(vec![ai6::Evidence::new(
                    ai6::directorates::m::deliverables::Deliverable::parse("branch:fix-1").unwrap(),
                    ai6::ProbeResult::Missing,
                    "branch fix-1 is not on the remote",
                    f64::MAX / 2.0,
                )])
                .probed(),
        )
        .unwrap();
    let again = run(
        vec!["probe", "--operation", "station-01", "--wait", "5"],
        key.to_owned(),
    )
    .await;
    let said = String::from_utf8(again.stdout).unwrap();
    assert!(
        said.contains("EVIDENCE\tRESULT\tDETAIL")
            && said.contains("branch:fix-1\tmissing\tbranch fix-1 is not on the remote"),
        "{said}"
    );

    // A decision needs its reason and is the attempt's verdict.
    let bare = run(
        vec!["verify", "redispatch", "--operation", "station-01"],
        key.to_owned(),
    )
    .await;
    assert_eq!(bare.status.code(), Some(2), "--reason is required");
    let decided = run(
        vec![
            "verify",
            "redispatch",
            "--reason",
            "no branch on the remote",
            "--amend",
            "Push the branch.",
            "--operation",
            "station-01",
        ],
        key.to_owned(),
    )
    .await;
    assert!(
        decided.status.success(),
        "{}",
        String::from_utf8_lossy(&decided.stderr)
    );
    assert_eq!(
        String::from_utf8(decided.stdout).unwrap(),
        format!("{key} attempt 1: redispatch by viewer-007\n")
    );
    let verdict = registry
        .latest_session(key)
        .unwrap()
        .unwrap()
        .verdict
        .unwrap();
    assert_eq!(verdict.verdict, ai6::OutcomeVerdict::Redispatch);
    assert_eq!(verdict.note.as_deref(), Some("no branch on the remote"));
    assert_eq!(verdict.amendment.as_deref(), Some("Push the branch."));

    // A Mission that is not verifying is refused.
    verifying("settled", "done");
    let settled = run(
        vec![
            "verify",
            "done",
            "--reason",
            "x",
            "--operation",
            "station-01",
        ],
        "settled".to_owned(),
    )
    .await;
    assert_eq!(settled.status.code(), Some(1));
    assert!(
        String::from_utf8(settled.stderr).unwrap().contains("409"),
        "refused with the gateway's status"
    );
    let _ = shutdown.send(());
}
