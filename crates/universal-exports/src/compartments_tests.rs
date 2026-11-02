//! Compartments for people through the gateway: union with the access policy, and who may change
//! what. The refusals come first in each case.

use std::sync::Arc;

use ai6::directorates::registry::backend::*;
use ai6::{ConfigProvider, ConfigRecord, ConfigScope, SqlProvider};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::controls::tests::{Served, served_with_roster};

/// `operator-007` manages compartments everywhere and may consent for G; `viewer-007` reads G and
/// has no rule on L.
const ROLES: &str = r#"
    [role.viewer]
    actions = ["mission.read", "audit.read"]

    [role.manager]
    actions = ["compartment.manage", "compartment.read"]

    [role.owner]
    actions = ["mission.read", "compartment.cover"]

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "viewer"

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "owner"

    [[rule]]
    subject = "person:operator-007"
    scope = "compartment:KESTREL"
    role = "manager"
"#;

/// As [`ROLES`], with the operator also able to read people into `KESTREL`.
const ROLES_PEOPLE: &str = r#"
    [role.viewer]
    actions = ["mission.read", "audit.read"]

    [role.manager]
    actions = ["compartment.manage", "compartment.read", "access.manage"]

    [role.owner]
    actions = ["mission.read", "compartment.cover"]

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "viewer"

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "owner"

    [[rule]]
    subject = "person:operator-007"
    scope = "compartment:KESTREL"
    role = "manager"
"#;

struct Fixture {
    served: Served,
    source: Arc<SqlProvider>,
    _config: tempfile::TempDir,
}

async fn fixture(roles: &str) -> Fixture {
    let config = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", config.path().join("config.db").display());
    let source = Arc::new(SqlProvider::open(&location).unwrap());
    for operation in ["G", "L", "M"] {
        source
            .put(
                ConfigRecord::Operation,
                &ConfigScope::Global,
                operation,
                &json!({}),
            )
            .unwrap();
    }
    let served = served_with_roster(roles, Some(source.clone())).await;
    Fixture {
        served,
        source,
        _config: config,
    }
}

impl Fixture {
    /// Stores a compartment directly, as an administrator with the provider does.
    fn store(&self, id: &str, definition: Value) {
        self.source
            .put(
                ConfigRecord::Compartment,
                &ConfigScope::Global,
                id,
                &definition,
            )
            .unwrap();
    }

    fn stored(&self, id: &str) -> Option<Value> {
        self.source
            .definitions(ConfigRecord::Compartment, &ConfigScope::Global)
            .unwrap()
            .records
            .into_iter()
            .find(|(key, _)| key == id)
            .map(|(_, definition)| definition)
    }

    fn audits(&self, action: &str) -> Vec<ai6::AuditRecord> {
        self.served
            .registry
            .list_audit(100)
            .unwrap()
            .into_iter()
            .filter(|record| record.action == action)
            .collect()
    }

    async fn request(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}{path}", self.served.base))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await.unwrap()
    }

    async fn missions(&self, token: &str, query: &str) -> Vec<String> {
        let body: Value = self
            .request(token, Method::GET, &format!("/v1/missions{query}"), None)
            .await
            .json()
            .await
            .unwrap();
        let mut found: Vec<String> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|mission| mission["mission_id"].as_str().unwrap().to_owned())
            .collect();
        found.sort();
        found
    }
}

fn readers(subject: &str, cover: Value) -> Value {
    json!({
        "actions": ["mission.read", "product.read", "cable.read"],
        "covers": [cover],
        "read_in": [{"subject": subject}]
    })
}

#[tokio::test]
async fn a_person_read_in_reads_a_covered_operation_without_a_rule_and_it_is_logged() {
    let fixture = fixture(ROLES).await;
    let viewer = fixture.served.viewer.clone();
    assert_eq!(fixture.missions(&viewer, "").await, ["mission-g"]);
    assert!(fixture.audits("compartment.read").is_empty());

    fixture.store(
        "LAPWING",
        readers("person:viewer-007", json!({"operation": "L"})),
    );
    assert_eq!(
        fixture.missions(&viewer, "").await,
        ["mission-g", "mission-l"]
    );
    let records = fixture.audits("compartment.read");
    assert!(!records.is_empty());
    assert_eq!(records[0].actor, "viewer-007");
    assert_eq!(records[0].operation, "L");
    assert_eq!(records[0].resource.as_deref(), Some("operation:L"));
    assert!(
        records[0].target.starts_with("LAPWING "),
        "{}",
        records[0].target
    );

    // A read out holds from the next request.
    fixture
        .source
        .remove(ConfigRecord::Compartment, &ConfigScope::Global, "LAPWING")
        .unwrap();
    assert_eq!(fixture.missions(&viewer, "").await, ["mission-g"]);
}

#[tokio::test]
async fn a_read_the_policy_allows_is_not_logged_as_a_compartment_read() {
    let fixture = fixture(ROLES).await;
    fixture.store(
        "LAPWING",
        readers("person:viewer-007", json!({"operation": "G"})),
    );
    assert_eq!(
        fixture.missions(&fixture.served.viewer, "").await,
        ["mission-g"]
    );
    assert!(fixture.audits("compartment.read").is_empty());
}

#[tokio::test]
async fn one_covered_mission_is_read_and_its_operation_is_not() {
    let fixture = fixture(ROLES).await;
    fixture.store(
        "LAPWING",
        readers("person:viewer-007", json!({"mission": "mission-l"})),
    );
    assert_eq!(
        fixture.missions(&fixture.served.viewer, "").await,
        ["mission-g", "mission-l"]
    );
    // An Operation-wide action stays out of reach.
    let refused = fixture
        .request(
            &fixture.served.viewer,
            Method::GET,
            "/v1/user/audit?operation_id=L",
            None,
        )
        .await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_compartment_answers_only_for_what_it_can_grant() {
    let fixture = fixture(ROLES).await;
    fixture.store(
        "LAPWING",
        json!({
            "actions": ["mission.status", "mission.read", "product.read", "cable.send", "cable.read"],
            "covers": [{"operation": "L"}],
            "read_in": [{"subject": "person:viewer-007"}]
        }),
    );
    let refused = fixture
        .request(
            &fixture.served.viewer,
            Method::POST,
            "/v1/user/operations/L/missions",
            Some(json!({"title": "work", "prompt": "do it"})),
        )
        .await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
}

/// What a [`Broken`] provider says its `compartment` records are.
type Answer = fn() -> Result<Vec<(String, Value)>, ai6::RosterError>;

/// A provider whose `compartment` records are whatever `answer` says, as a database edited around
/// its checks, or down, would give.
struct Broken {
    source: Arc<SqlProvider>,
    answer: Answer,
}

impl ConfigProvider for Broken {
    fn load(&self, station: &str) -> Result<ai6::ConfigSnapshot, ai6::RosterError> {
        self.source.load(station)
    }

    fn definitions(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
    ) -> Result<ai6::ConfigDefinitions, ai6::RosterError> {
        let mut held = self.source.definitions(record, scope)?;
        if record == ConfigRecord::Compartment {
            held.records = (self.answer)()?;
        }
        Ok(held)
    }
}

async fn viewer_reads_with(answer: Answer) -> Vec<String> {
    let base = fixture(ROLES).await;
    let broken = Arc::new(Broken {
        source: base.source.clone(),
        answer,
    });
    let served = served_with_roster(ROLES, Some(broken)).await;
    let body: Value = reqwest::Client::new()
        .get(format!("{}/v1/missions", served.base))
        .bearer_auth(&served.viewer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut found: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|mission| mission["mission_id"].as_str().unwrap().to_owned())
        .collect();
    found.sort();
    found
}

#[tokio::test]
async fn what_cannot_be_read_grants_nothing() {
    assert_eq!(
        viewer_reads_with(|| Ok(vec![(
            "BROKEN".to_owned(),
            readers("person:viewer-007", json!({"operation": "L"})),
        )]))
        .await,
        ["mission-g", "mission-l"],
        "a control: this record reads"
    );
    assert_eq!(
        viewer_reads_with(|| Ok(vec![(
            "BROKEN".to_owned(),
            json!({
                "actions": ["roster.manage", "mission.read"],
                "covers": [{"operation": "L"}],
                "read_in": [{"subject": "person:viewer-007"}]
            }),
        )]))
        .await,
        ["mission-g"],
        "a record that cannot be read"
    );
    assert_eq!(
        viewer_reads_with(|| Err(ai6::RosterError::unavailable("down"))).await,
        ["mission-g"],
        "a provider that cannot be read"
    );
}

#[tokio::test]
async fn a_failing_audit_log_refuses_a_read_only_a_compartment_allows() {
    let fixture = fixture(ROLES).await;
    fixture.store(
        "LAPWING",
        readers("person:viewer-007", json!({"operation": "L"})),
    );
    let audit = fixture.served.directory.path().join("audit");
    let _ = std::fs::remove_dir_all(&audit);
    std::fs::write(&audit, "in the way").unwrap();
    assert_eq!(
        fixture.missions(&fixture.served.viewer, "").await,
        ["mission-g"],
        "what the policy allows still reads"
    );
}

#[tokio::test]
async fn covering_needs_both_the_compartment_and_the_operation_s_consent() {
    let fixture = fixture(ROLES).await;
    let operator = fixture.served.operator.clone();
    // Nothing on this compartment.
    let refused = fixture
        .request(
            &operator,
            Method::PUT,
            "/v1/user/compartments/OTHER",
            Some(json!({"actions": ["mission.status"]})),
        )
        .await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);

    let made = fixture
        .request(
            &operator,
            Method::PUT,
            "/v1/user/compartments/KESTREL",
            Some(json!({"description": "Release work", "actions": ["mission.status"]})),
        )
        .await;
    assert_eq!(made.status(), StatusCode::OK);

    // manage, but no compartment.cover on L.
    let no_consent = fixture
        .request(
            &operator,
            Method::PUT,
            "/v1/user/compartments/KESTREL/covers/operation:L",
            None,
        )
        .await;
    assert_eq!(no_consent.status(), StatusCode::FORBIDDEN);
    assert_eq!(fixture.stored("KESTREL").unwrap()["covers"], json!([]));

    // Consent on G, and the viewer holds neither.
    let covered = fixture
        .request(
            &operator,
            Method::PUT,
            "/v1/user/compartments/KESTREL/covers/operation:G",
            None,
        )
        .await;
    assert_eq!(
        covered.status(),
        StatusCode::OK,
        "{:?}",
        covered.text().await
    );
    assert_eq!(
        fixture.stored("KESTREL").unwrap()["covers"],
        json!([{"operation": "G"}])
    );
    let viewer = fixture
        .request(
            &fixture.served.viewer,
            Method::PUT,
            "/v1/user/compartments/KESTREL/covers/operation:G",
            None,
        )
        .await;
    assert_eq!(viewer.status(), StatusCode::FORBIDDEN);
    assert!(!fixture.audits("compartment.cover").is_empty());
}

#[tokio::test]
async fn an_operation_s_owner_withdraws_a_cover_with_consent_alone() {
    let fixture = fixture(ROLES).await;
    fixture.store(
        "SOMEONE-ELSES",
        json!({"actions": ["mission.status"], "covers": [{"operation": "G"}]}),
    );
    let outsider = fixture
        .request(
            &fixture.served.viewer,
            Method::DELETE,
            "/v1/user/compartments/SOMEONE-ELSES/covers/operation:G",
            None,
        )
        .await;
    assert_eq!(outsider.status(), StatusCode::FORBIDDEN);
    assert_ne!(
        fixture.stored("SOMEONE-ELSES").unwrap()["covers"],
        json!([])
    );
    // operator-007 has compartment.cover on G and nothing on this compartment.
    let withdrawn = fixture
        .request(
            &fixture.served.operator,
            Method::DELETE,
            "/v1/user/compartments/SOMEONE-ELSES/covers/operation:G",
            None,
        )
        .await;
    assert_eq!(withdrawn.status(), StatusCode::OK);
    assert_eq!(
        fixture.stored("SOMEONE-ELSES").unwrap()["covers"],
        json!([])
    );
    assert_eq!(fixture.audits("compartment.uncover").len(), 1);
}

#[tokio::test]
async fn a_hidden_cover_is_neither_listed_nor_dropped_by_an_edit() {
    let fixture = fixture(ROLES).await;
    fixture.store(
        "KESTREL",
        json!({
            "actions": ["mission.status"],
            "covers": [{"operation": "G"}, {"operation": "M"}],
            "read_in": [{"subject": "operation:L"}, {"subject": "group:release"}]
        }),
    );
    let view: Value = fixture
        .request(
            &fixture.served.operator,
            Method::GET,
            "/v1/user/compartments/KESTREL",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        view["covers"],
        json!([{"entry": "operation:G", "propagate": true}])
    );
    assert_eq!(
        view["read_in"],
        json!([{"entry": "group:release", "propagate": true}])
    );
    let text = view.to_string();
    assert!(
        !text.contains("operation:M") && !text.contains("operation:L"),
        "{text}"
    );

    let edited = fixture
        .request(
            &fixture.served.operator,
            Method::PUT,
            "/v1/user/compartments/KESTREL",
            Some(json!({"description": "edited", "actions": ["mission.read"]})),
        )
        .await;
    assert_eq!(edited.status(), StatusCode::OK);
    let stored = fixture.stored("KESTREL").unwrap();
    assert_eq!(stored["description"], "edited");
    assert_eq!(
        stored["covers"],
        json!([{"operation": "G"}, {"operation": "M"}])
    );
    assert_eq!(
        stored["read_in"],
        json!([{"subject": "operation:L"}, {"subject": "group:release"}])
    );
}

#[tokio::test]
async fn a_manager_cannot_put_policy_authority_in_a_compartment() {
    let fixture = fixture(ROLES).await;
    let refused = fixture
        .request(
            &fixture.served.operator,
            Method::PUT,
            "/v1/user/compartments/KESTREL",
            Some(json!({"actions": ["roster.manage"]})),
        )
        .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let reason: Value = refused.json().await.unwrap();
    assert!(
        reason["error"].as_str().unwrap().contains("roster.manage"),
        "{reason}"
    );
    assert_eq!(fixture.stored("KESTREL"), None);
}

#[tokio::test]
async fn reading_people_in_needs_access_manage_and_sessions_do_not() {
    let fixture_without = fixture(ROLES).await;
    fixture_without.store(
        "KESTREL",
        json!({"actions": ["mission.status"], "covers": [{"operation": "G"}]}),
    );
    let operator = fixture_without.served.operator.clone();
    let person = fixture_without
        .request(
            &operator,
            Method::PUT,
            "/v1/user/compartments/KESTREL/read-in/group:release",
            None,
        )
        .await;
    assert_eq!(person.status(), StatusCode::FORBIDDEN);
    let agent = fixture_without
        .request(
            &operator,
            Method::PUT,
            "/v1/user/compartments/KESTREL/read-in/agent:007",
            None,
        )
        .await;
    assert_eq!(agent.status(), StatusCode::OK);
    let out = fixture_without
        .request(
            &operator,
            Method::DELETE,
            "/v1/user/compartments/KESTREL/read-in/agent:007",
            None,
        )
        .await;
    assert_eq!(out.status(), StatusCode::OK);
    assert_eq!(
        fixture_without.stored("KESTREL").unwrap()["read_in"],
        json!([])
    );
    assert_eq!(fixture_without.audits("compartment.read_in").len(), 1);
    assert_eq!(fixture_without.audits("compartment.read_out").len(), 1);

    let with = fixture(ROLES_PEOPLE).await;
    with.store(
        "KESTREL",
        json!({"actions": ["mission.status"], "covers": [{"operation": "G"}]}),
    );
    let person = with
        .request(
            &with.served.operator,
            Method::PUT,
            "/v1/user/compartments/KESTREL/read-in/group:release",
            None,
        )
        .await;
    assert_eq!(person.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_read_only_provider_answers_conflict_with_its_reason() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("operations.toml"), "[operation.G]\n").unwrap();
    let source = Arc::new(ai6::FileProvider::new(directory.path()));
    let served = served_with_roster(ROLES, Some(source)).await;
    let refused = reqwest::Client::new()
        .put(format!("{}/v1/user/compartments/KESTREL", served.base))
        .bearer_auth(&served.operator)
        .json(&json!({"actions": ["mission.status"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let reason: Value = refused.json().await.unwrap();
    assert!(!reason["error"].as_str().unwrap().is_empty());
}

fn finished(fixture: &Fixture, id: &str, operation: &str) {
    let brief: ai6::Brief = serde_json::from_value(json!({
        "title": "work",
        "body": "the brief",
        "id": id,
        "mission_id": id,
        "operation": operation,
    }))
    .unwrap();
    fixture.served.registry.submit_brief(&brief).unwrap();
    fixture
        .served
        .registry
        .finish(
            &brief,
            &json!({"state": "done", "summary": "é".repeat(10), "session_ids": ["secret-session"]}),
            true,
            None,
            None,
        )
        .unwrap();
}

#[tokio::test]
async fn products_are_read_through_product_read_and_anything_else_is_not_found() {
    let fixture = fixture(ROLES).await;
    finished(&fixture, "L-1", "L");
    let viewer = fixture.served.viewer.clone();
    let path = "/v1/missions/L-1/products";
    let unknown = fixture
        .request(&viewer, Method::GET, "/v1/missions/no-such/products", None)
        .await;
    let before = fixture.request(&viewer, Method::GET, path, None).await;
    assert_eq!(before.status(), StatusCode::NOT_FOUND);
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_eq!(before.text().await.unwrap(), unknown.text().await.unwrap());

    fixture.store(
        "LAPWING",
        json!({
            "actions": ["mission.status"],
            "covers": [{"operation": "L"}],
            "read_in": [{"subject": "person:viewer-007"}]
        }),
    );
    let status_only = fixture.request(&viewer, Method::GET, path, None).await;
    assert_eq!(status_only.status(), StatusCode::NOT_FOUND);

    fixture.store(
        "LAPWING",
        json!({
            "actions": ["product.read"],
            "covers": [{"operation": "L"}],
            "read_in": [{"subject": "person:viewer-007"}],
            "limits": {"max_product_bytes": 4}
        }),
    );
    let product: Value = fixture
        .request(&viewer, Method::GET, path, None)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(product["outcome_untrusted"]["summary"], "éé… [cut]");
    assert!(!product.to_string().contains("secret-session"), "{product}");
    assert!(
        fixture
            .audits("compartment.read")
            .iter()
            .any(|record| record.resource.as_deref() == Some("mission:L-1"))
    );
}

#[tokio::test]
async fn the_mission_list_filters_by_a_compartment_the_person_may_see() {
    let fixture = fixture(ROLES).await;
    let (operator, viewer) = (
        fixture.served.operator.clone(),
        fixture.served.viewer.clone(),
    );
    fixture.store(
        "KESTREL",
        json!({"actions": ["mission.status"], "covers": [{"operation": "G"}]}),
    );
    assert_eq!(
        fixture.missions(&operator, "?compartment=KESTREL").await,
        ["mission-g"]
    );
    assert!(
        fixture
            .missions(&viewer, "?compartment=KESTREL")
            .await
            .is_empty(),
        "the viewer neither reads it nor is read in"
    );
    assert!(
        fixture
            .missions(&operator, "?compartment=NONE")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn the_profile_says_what_grants_what_and_reads_nothing() {
    let fixture = fixture(ROLES).await;
    fixture.store(
        "LAPWING",
        readers("person:viewer-007", json!({"operation": "L"})),
    );
    let profile: Value = fixture
        .request(
            &fixture.served.viewer,
            Method::GET,
            "/v1/user/profile",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
    let operations = profile["operations"].as_array().unwrap();
    let on_l = operations
        .iter()
        .find(|operation| operation["operation_id"] == "L")
        .expect("L appears through the compartment");
    assert_eq!(on_l["compartments"], json!(["LAPWING"]));
    let actions = on_l["actions"].as_array().unwrap();
    assert!(actions.contains(&json!("mission.read")));
    assert!(actions.contains(&json!("product.read")));
    assert!(!actions.contains(&json!("mission.control")));
    assert!(fixture.audits("compartment.read").is_empty());
}

#[tokio::test]
async fn the_list_shows_only_what_the_person_may_read() {
    let fixture = fixture(ROLES).await;
    fixture.store(
        "KESTREL",
        json!({"actions": ["mission.status"], "covers": [{"operation": "G"}], "limits": {"max_fanout": "none"}}),
    );
    fixture.store(
        "OTHER",
        json!({"actions": ["mission.status"], "covers": [{"operation": "G"}]}),
    );
    let listed: Value = fixture
        .request(
            &fixture.served.operator,
            Method::GET,
            "/v1/user/compartments",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["KESTREL"]);
    assert_eq!(
        listed[0]["weakenings"][0],
        "compartment KESTREL: limits.max_fanout none is looser than the default 100"
    );
    let refused = fixture
        .request(
            &fixture.served.viewer,
            Method::GET,
            "/v1/user/compartments/KESTREL",
            None,
        )
        .await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let form: Value = fixture
        .request(
            &fixture.served.viewer,
            Method::GET,
            "/v1/user/compartment-form",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
    assert!(
        form["schema"]["properties"]["actions"].is_object(),
        "{form}"
    );
}
