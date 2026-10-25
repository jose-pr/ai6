use std::collections::BTreeMap;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use ai6::{
    AgentRole, Attempt, Authenticator, AuthorizationDecision, AuthorizationRequest, Authorizer,
    Directory, FileRegistry, Lookup, Person, Verdict, guatiao::library::ProviderError,
};
use ai6_diptel::{LifecycleEvent, MissionId, OperationId, SessionId, SessionState};
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
        registry,
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
            .arg("--server")
            .arg(server)
            .args(args)
            .env_remove("AI6_ACCESS_TOKEN");
        if let Some(token) = token {
            command.env("AI6_ACCESS_TOKEN", token);
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
    assert!(mission.contains("SESSION\tSTATE\tSEQUENCE\tEXPOSURE"));
    assert!(mission.contains("provider-session-007\tworking\t1\t-"));

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
            .contains("AI6_ACCESS_TOKEN")
    );

    let no_mission_token = ai6ctl(server.clone(), None, &["missions"]).await;
    assert_eq!(no_mission_token.status.code(), Some(1));
    assert!(
        String::from_utf8(no_mission_token.stderr)
            .unwrap()
            .contains("AI6_ACCESS_TOKEN")
    );

    let _ = shutdown.send(());
}
