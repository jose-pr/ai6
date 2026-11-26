//! `ai6ctl reports` against a real gateway.

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
use ai6_diptel::{LifecycleEvent, MissionId, OperationId, RunId, RunState};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use universal_exports::{AccessProviders, Gateway, GatewayConfig, GatewayLimits};

struct Everyone;

impl Authenticator for Everyone {
    fn authenticate(&self, _attempt: Attempt) -> Result<Verdict, ProviderError> {
        Ok(Verdict::Valid(Lookup {
            by: "name".to_owned(),
            value: "m".to_owned(),
            ..Default::default()
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
    let (operation, mission, run) = (
        OperationId::new("G").unwrap(),
        MissionId::new("G-1").unwrap(),
        RunId::new("run-1").unwrap(),
    );
    gateway
        .register_run(token, operation.clone(), mission.clone(), run.clone())
        .await
        .unwrap();
    gateway
        .publish_lifecycle(
            token,
            LifecycleEvent {
                operation_id: operation,
                mission_id: mission,
                run_id: run,
                state: RunState::Working,
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
async fn a_person_files_lists_and_shows_a_report() {
    let (server, token, shutdown, directory) = start().await;
    let run = |args: Vec<&'static str>| {
        let (server, token) = (server.clone(), token.clone());
        async move { ai6ctl(&server, &token, &args).await }
    };
    let filed = printed(
        &run(vec![
            "reports",
            "file",
            "operation:G",
            "--title",
            "The parser leaks memory",
            "--text",
            "RSS grows by 1 MiB a request",
            "--priority",
            "high",
            "--link",
            "mission:G-1",
        ])
        .await,
    );
    let id = filed.split('\t').next().unwrap().to_owned();
    assert!(id.starts_with("report-"), "{filed}");
    assert!(filed.contains("operation:G"), "{filed}");
    let stored = FileRegistry::open(directory.path())
        .report(&id)
        .unwrap()
        .unwrap();
    assert!(stored.about_mission("G-1"));
    assert_eq!(stored.priority.as_deref(), Some("high"));

    let listed = printed(&run(vec!["reports", "list", "--scope", "operation:G"]).await);
    assert!(listed.starts_with("ID\tSCOPE\tPRIORITY"), "{listed}");
    assert!(
        listed.contains(&id) && listed.contains("The parser leaks memory"),
        "{listed}"
    );
    let by_mission = printed(&run(vec!["reports", "list", "--mission", "G-1"]).await);
    assert!(by_mission.contains(&id), "{by_mission}");

    let shown = ai6ctl(&server, &token, &["reports", "show", &id]).await;
    let shown = printed(&shown);
    assert!(shown.contains("RSS grows by 1 MiB a request"), "{shown}");
    assert!(shown.contains("reliability\tF"), "{shown}");

    let refused = run(vec![
        "reports",
        "file",
        "operation:G",
        "--title",
        "t",
        "--text",
        "x",
        "--priority",
        "someday",
    ])
    .await;
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("priority"));
    // Clearances are Registry records, changed live through the gateway.
    let granted = printed(
        &run(vec![
            "reports",
            "clearance",
            "grant",
            "group:security",
            "confidential",
        ])
        .await,
    );
    assert!(granted.starts_with("granted"), "{granted}");
    let held = FileRegistry::open(directory.path())
        .report_clearances()
        .unwrap();
    assert_eq!(held[0].subject, "group:security");
    let listed = printed(&run(vec!["reports", "clearance", "list"]).await);
    assert!(
        listed.contains("group:security") && listed.contains("confidential"),
        "{listed}"
    );
    let revoked = printed(&run(vec!["reports", "clearance", "revoke", "group:security"]).await);
    assert!(revoked.starts_with("revoked"), "{revoked}");
    assert!(
        FileRegistry::open(directory.path())
            .report_clearances()
            .unwrap()
            .is_empty()
    );
    let _ = shutdown.send(());
}

#[tokio::test]
async fn reports_are_exported_as_markdown_files_and_applied_back() {
    let (server, token, shutdown, _directory) = start().await;
    let run = |args: Vec<String>| {
        let (server, token) = (server.clone(), token.clone());
        async move {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            ai6ctl(&server, &token, &args).await
        }
    };
    let filed = printed(
        &run(vec![
            "reports".into(),
            "file".into(),
            "operation:G".into(),
            "--title".into(),
            "The parser leaks".into(),
            "--text".into(),
            "RSS grows".into(),
        ])
        .await,
    );
    let id = filed.split('\t').next().unwrap().to_owned();
    let shown = printed(
        &run(vec![
            "reports".into(),
            "show".into(),
            id.clone(),
            "-o".into(),
            "md".into(),
        ])
        .await,
    );
    assert!(shown.starts_with("---\n"), "{shown}");
    assert!(shown.contains("title: \"The parser leaks\""), "{shown}");
    assert!(shown.trim_end().ends_with("RSS grows"), "{shown}");

    let folder = tempfile::tempdir().unwrap();
    let dir = folder.path().to_string_lossy().into_owned();
    let exported = printed(
        &run(vec![
            "reports".into(),
            "export".into(),
            dir.clone(),
            "--scope".into(),
            "operation:G".into(),
        ])
        .await,
    );
    assert!(exported.starts_with("1 written"), "{exported}");
    let file = folder.path().join("finding").join(format!("{id}.md"));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap().trim_end(),
        shown.trim_end()
    );

    // What was exported applies back as the same Report, and a hand-written document files one.
    let back = printed(&run(vec!["apply".into(), "-f".into(), dir.clone()]).await);
    assert_eq!(
        back.trim_end(),
        format!("{id}\tunchanged\tThe parser leaks")
    );
    let written = folder.path().join("new.md");
    std::fs::write(
        &written,
        "---\nscope: operation:G\ntitle: Seen later\n---\n\nIt slowed down.",
    )
    .unwrap();
    let dry = printed(
        &run(vec![
            "apply".into(),
            "-f".into(),
            written.to_string_lossy().into_owned(),
            "--dry-run".into(),
        ])
        .await,
    );
    assert!(dry.contains("would be created"), "{dry}");
    let made = printed(
        &run(vec![
            "reports".into(),
            "import".into(),
            written.to_string_lossy().into_owned(),
        ])
        .await,
    );
    assert!(made.contains("\tcreated\tSeen later"), "{made}");
    let _ = shutdown.send(());
}
