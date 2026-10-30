//! The tree of Operations through the gateway: rules on a parent, hidden ancestors, subtree
//! filters and the effective policy of a subordinate Operation.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{
    Attempt, Authenticator, Directory, FileProvider, FileRegistry, Lookup, Person, Verdict,
    guatiao::library::ProviderError, role_authorizer_from_toml,
};
use ai6_diptel::{LifecycleEvent, MissionId, OperationId, SessionId, SessionState};
use tokio::sync::oneshot;
use universal_exports::{AccessProviders, Gateway, GatewayConfig, GatewayLimits};

const STATION_TOKEN: &str = "exports-test-credential-with-more-than-32-bytes";

struct ByName;

impl Authenticator for ByName {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        Ok(match (attempt.name, attempt.secret.as_deref()) {
            (Some(name), Some("secret")) => Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: name,
            }),
            _ => Verdict::Invalid,
        })
    }
}

impl Directory for ByName {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        // Signing in looks a person up by name; a token, later, by the ID it was issued to.
        let name = lookup.value.trim_end_matches("-id").to_owned();
        Ok(Some(Person {
            id: format!("{name}-id"),
            name,
            ..Person::default()
        }))
    }
}

/// BODYGUARD > FORTITUDE > SOUTH, and OTHER beside them; a Mission in SOUTH and in OTHER.
async fn serve() -> (String, oneshot::Sender<()>, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(directory.path().join("registry"));
    registry
        .check_in(
            "station-a",
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
         station_labels = { site = \"dover\" }\n[operation.OTHER]\npath = \"/srv/other\"\n",
    )
    .unwrap();
    let policy = role_authorizer_from_toml(
        r#"
            [role.viewer]
            actions = ["mission.read"]
            [role.admin]
            actions = ["mission.read", "roster.read"]

            [[rule]]
            subject = "account:viewer"
            scope = "operation:FORTITUDE"
            role = "viewer"

            [[rule]]
            subject = "account:admin"
            scope = "*"
            role = "admin"
        "#,
        ai6::Limit::UNBOUNDED,
    )
    .unwrap();
    let gateway = Gateway::with_access(
        GatewayConfig::new(
            "https://exports.example",
            STATION_TOKEN,
            GatewayLimits::default(),
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap(),
        Arc::new(registry),
        AccessProviders::new(Arc::new(ByName), Arc::new(ByName), Arc::new(policy)),
    )
    .with_roster(Arc::new(FileProvider::new(&config)));
    for (operation, mission) in [("SOUTH", "m-south"), ("OTHER", "m-other")] {
        let (operation, mission) = (
            OperationId::new(operation).unwrap(),
            MissionId::new(mission).unwrap(),
        );
        let session = SessionId::new(format!("session-{mission}")).unwrap();
        gateway
            .register_session(
                STATION_TOKEN,
                operation.clone(),
                mission.clone(),
                session.clone(),
            )
            .await
            .unwrap();
        gateway
            .publish_lifecycle(
                STATION_TOKEN,
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
    (format!("http://{address}"), shutdown, directory)
}

async fn login(base: &str, name: &str) -> String {
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/auth/login"))
        .header("origin", "https://exports.example")
        .header("x-forwarded-proto", "https")
        .json(&serde_json::json!({"name": name, "secret": "secret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    response.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn get(base: &str, token: &str, path: &str) -> (reqwest::StatusCode, serde_json::Value) {
    let response = reqwest::Client::new()
        .get(format!("{base}{path}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or_default())
}

fn missions(listed: &serde_json::Value) -> Vec<String> {
    let mut missions: Vec<String> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|mission| mission["mission_id"].as_str().unwrap().to_owned())
        .collect();
    missions.sort();
    missions
}

#[tokio::test]
async fn a_rule_on_a_parent_reaches_its_subtree_and_hidden_ancestors_stay_unnamed() {
    let (base, shutdown, _directory) = serve().await;
    let viewer = login(&base, "viewer").await;

    let (status, listed) = get(&base, &viewer, "/v1/missions").await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(
        missions(&listed),
        ["m-south"],
        "the FORTITUDE rule reads SOUTH, not OTHER"
    );

    let (_, profile) = get(&base, &viewer, "/v1/user/profile").await;
    let operations = profile["operations"].as_array().unwrap();
    let named: Vec<&str> = operations
        .iter()
        .map(|access| access["operation_id"].as_str().unwrap())
        .collect();
    assert_eq!(named, ["FORTITUDE", "SOUTH"], "BODYGUARD is hidden");
    assert!(operations[0].get("parent").is_none(), "{profile}");
    assert_eq!(operations[1]["parent"], "FORTITUDE");
    assert!(!profile.to_string().contains("BODYGUARD"));

    let (_, board) = get(&base, &viewer, "/v1/stations").await;
    assert_eq!(
        board["operations"],
        serde_json::json!([
            {"operation_id": "FORTITUDE"},
            {"operation_id": "SOUTH", "parent": "FORTITUDE"}
        ])
    );
    let (status, _) = get(&base, &viewer, "/v1/user/roster/operation/SOUTH/effective").await;
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN, "needs roster.read");
    let _ = shutdown.send(());
}

#[tokio::test]
async fn a_subtree_filter_lists_a_grandchilds_missions_and_effective_names_each_writer() {
    let (base, shutdown, _directory) = serve().await;
    let admin = login(&base, "admin").await;

    let (_, subtree) = get(
        &base,
        &admin,
        "/v1/missions?operation=BODYGUARD&subtree=true",
    )
    .await;
    assert_eq!(missions(&subtree), ["m-south"]);
    let (_, only) = get(&base, &admin, "/v1/missions?operation=BODYGUARD").await;
    assert!(
        missions(&only).is_empty(),
        "without subtree, BODYGUARD's own"
    );
    let (_, south) = get(&base, &admin, "/v1/missions?operation=SOUTH").await;
    assert_eq!(missions(&south), ["m-south"]);

    let (status, effective) = get(&base, &admin, "/v1/user/roster/operation/SOUTH/effective").await;
    assert_eq!(status, reqwest::StatusCode::OK, "{effective}");
    assert_eq!(
        effective["lineage"],
        serde_json::json!(["FORTITUDE", "BODYGUARD"])
    );
    assert_eq!(effective["definition"]["green_ink"]["preset"], "licensed");
    assert_eq!(effective["origins"]["green_ink.preset"], "BODYGUARD");
    assert_eq!(effective["origins"]["station_labels.site"], "SOUTH");
    let (status, _) = get(&base, &admin, "/v1/user/roster/operation/MISSING/effective").await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
    let _ = shutdown.send(());
}
