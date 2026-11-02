//! `ai6ctl compartments` against a real gateway over a provider that takes changes.

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{
    Attempt, Authenticator, AuthorizationDecision, AuthorizationRequest, Authorizer,
    ConfigDefinitions, ConfigProvider, ConfigRecord, ConfigRevision, ConfigScope, ConfigSnapshot,
    Directory, FileRegistry, Lookup, Person, Roster, RosterError, Settings, Verdict,
    guatiao::library::ProviderError,
};
use ai6_diptel::{LifecycleEvent, MissionId, OperationId, SessionId, SessionState};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use universal_exports::{AccessProviders, Gateway, GatewayConfig, GatewayLimits};

struct Everyone;

impl Authenticator for Everyone {
    fn authenticate(&self, _attempt: Attempt) -> Result<Verdict, ProviderError> {
        Ok(Verdict::Valid(Lookup {
            by: "name".to_owned(),
            value: "m".to_owned(),
        }))
    }
}

impl Directory for Everyone {
    fn lookup(&self, _lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        Ok(Some(Person {
            id: "person-m".to_owned(),
            name: "m".to_owned(),
            ..Person::default()
        }))
    }
}

impl Authorizer for Everyone {
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

/// Records in memory, checked only as a compartment's own record checks them.
#[derive(Default)]
struct Held {
    records: Mutex<BTreeMap<(String, String), Value>>,
}

impl ConfigProvider for Held {
    fn load(&self, _station: &str) -> Result<ConfigSnapshot, RosterError> {
        Ok(ConfigSnapshot::new(
            ConfigRevision::new(1),
            Roster::default(),
            Settings::default(),
        ))
    }

    fn definitions(
        &self,
        record: ConfigRecord,
        _scope: &ConfigScope,
    ) -> Result<ConfigDefinitions, RosterError> {
        let records = self
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|((family, _), _)| family == record.name())
            .map(|((_, key), definition)| (key.clone(), definition.clone()))
            .collect();
        Ok(ConfigDefinitions::new(
            ConfigRevision::new(1),
            records,
            true,
        ))
    }

    fn put(
        &self,
        record: ConfigRecord,
        _scope: &ConfigScope,
        key: &str,
        definition: &Value,
    ) -> Result<ConfigRevision, RosterError> {
        self.records.lock().unwrap().insert(
            (record.name().to_owned(), key.to_owned()),
            definition.clone(),
        );
        Ok(ConfigRevision::new(2))
    }

    fn remove(
        &self,
        record: ConfigRecord,
        _scope: &ConfigScope,
        key: &str,
    ) -> Result<Option<ConfigRevision>, RosterError> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .remove(&(record.name().to_owned(), key.to_owned()))
            .map(|_| ConfigRevision::new(3)))
    }
}

async fn start() -> (String, String, oneshot::Sender<()>, tempfile::TempDir) {
    let token = "exports-test-credential-with-more-than-32-bytes";
    let directory = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(directory.path());
    registry
        .check_in(
            "station-01",
            BTreeMap::from([("operations".to_owned(), json!(["G"]))]),
        )
        .unwrap();
    let provider = Arc::new(Held::default());
    provider
        .put(
            ConfigRecord::Operation,
            &ConfigScope::Global,
            "G",
            &json!({}),
        )
        .unwrap();
    let brief: ai6::Brief = serde_json::from_value(json!({
        "title": "work",
        "id": "G-1",
        "mission_id": "G-1",
        "operation": "G",
    }))
    .unwrap();
    registry.submit_brief(&brief).unwrap();
    registry
        .finish(
            &brief,
            &json!({"state": "done", "summary": "shipped", "branch": "ai6/work"}),
            true,
            None,
            None,
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
        Arc::new(registry.clone()),
        AccessProviders::new(Arc::new(Everyone), Arc::new(Everyone), Arc::new(Everyone)),
    )
    .with_roster(provider);
    let (operation, mission, session) = (
        OperationId::new("G").unwrap(),
        MissionId::new("G-1").unwrap(),
        SessionId::new("session-1").unwrap(),
    );
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, stopped) = oneshot::channel();
    tokio::spawn(async move {
        let _ = axum::serve(listener, gateway.router())
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await;
    });
    let server = format!("http://{address}");
    let login: Value = reqwest::Client::new()
        .post(format!("{server}/v1/auth/login"))
        .header("origin", "https://exports.example")
        .header("x-forwarded-proto", "https")
        .json(&json!({"name": "m", "secret": "secret"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let access = login["token"].as_str().unwrap().to_owned();
    (server, access, shutdown, directory)
}

async fn ai6ctl(server: &str, token: &str, args: &[&str]) -> std::process::Output {
    let (server, token) = (server.to_owned(), token.to_owned());
    let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_ai6ctl"))
            .arg("--timeout")
            .arg("120")
            .arg("--server")
            .arg(server)
            .args(args)
            .env("UNEX_ACCESS_TOKEN", token)
            .output()
            .unwrap()
    })
    .await
    .unwrap()
}

fn printed(output: &std::process::Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).replace('\r', "")
}

#[tokio::test]
async fn each_command_changes_and_shows_a_compartment() {
    let (server, token, shutdown, _directory) = start().await;
    let run = |args: Vec<&'static str>| {
        let (server, token) = (server.clone(), token.clone());
        async move { ai6ctl(&server, &token, &args).await }
    };

    assert_eq!(
        printed(&run(vec!["compartments"]).await),
        "COMPARTMENT\tACTIONS\tCOVERS\tREAD_IN\n"
    );
    let refused = run(vec![
        "compartments",
        "set",
        "KESTREL",
        "--actions",
        "roster.manage",
    ])
    .await;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("roster.manage"),
        "the gateway's reason: {}",
        String::from_utf8_lossy(&refused.stderr)
    );

    assert_eq!(
        printed(
            &run(vec![
                "compartments",
                "set",
                "KESTREL",
                "--description",
                "Release work",
                "--actions",
                "mission.status,product.read",
                "--min-clearance",
                "field",
                "--max-fanout",
                "none",
            ])
            .await
        ),
        "KESTREL: saved\n"
    );
    assert_eq!(
        printed(
            &run(vec![
                "compartments",
                "cover",
                "KESTREL",
                "operation:G",
                "--no-propagate"
            ])
            .await
        ),
        "KESTREL: covers operation:G\n"
    );
    assert_eq!(
        printed(&run(vec!["compartments", "read-in", "KESTREL", "group:release"]).await),
        "KESTREL: group:release read in\n"
    );
    let shown = printed(&run(vec!["compartments", "show", "KESTREL"]).await);
    assert!(shown.contains("description\tRelease work\n"), "{shown}");
    assert!(
        shown.contains("actions\tmission.status,product.read\n"),
        "{shown}"
    );
    assert!(shown.contains("min_clearance\tfield\n"), "{shown}");
    assert!(shown.contains("limit\tmax_fanout\t\"none\"\n"), "{shown}");
    assert!(
        shown.contains("weakening\tcompartment KESTREL: limits.max_fanout"),
        "{shown}"
    );
    assert!(
        shown.contains("cover\toperation:G (this one only)\n"),
        "{shown}"
    );
    assert!(shown.contains("read_in\tgroup:release\n"), "{shown}");

    // Another `set` keeps what it does not name.
    printed(
        &run(vec![
            "compartments",
            "set",
            "KESTREL",
            "--description",
            "Edited",
        ])
        .await,
    );
    let edited = printed(&run(vec!["compartments", "show", "KESTREL"]).await);
    assert!(edited.contains("description\tEdited\n"), "{edited}");
    assert!(
        edited.contains("actions\tmission.status,product.read\n"),
        "{edited}"
    );
    assert!(edited.contains("cover\toperation:G"), "{edited}");

    let listed = printed(&run(vec!["missions", "--compartment", "KESTREL"]).await);
    assert!(listed.contains("G-1"), "{listed}");
    let none = printed(&run(vec!["missions", "--compartment", "NONE"]).await);
    assert!(!none.contains("G-1"), "{none}");
    let product = printed(&run(vec!["missions", "products", "G-1"]).await);
    assert!(product.contains("mission\tG-1\n"), "{product}");
    assert!(
        product.contains("outcome.summary\t\"shipped\"\n"),
        "{product}"
    );
    assert!(product.contains("ref.branch\t\"ai6/work\"\n"), "{product}");

    assert_eq!(
        printed(&run(vec!["compartments", "read-out", "KESTREL", "group:release"]).await),
        "KESTREL: group:release read out\n"
    );
    assert_eq!(
        printed(&run(vec!["compartments", "uncover", "KESTREL", "operation:G"]).await),
        "KESTREL: no longer covers operation:G\n"
    );
    let emptied = printed(&run(vec!["compartments", "show", "KESTREL"]).await);
    assert!(
        !emptied.contains("cover\t") && !emptied.contains("read_in\t"),
        "{emptied}"
    );
    assert_eq!(
        printed(&run(vec!["compartments", "remove", "KESTREL"]).await),
        "KESTREL: removed\n"
    );
    assert!(
        !run(vec!["compartments", "show", "KESTREL"])
            .await
            .status
            .success()
    );
    let _ = shutdown.send(());
}
