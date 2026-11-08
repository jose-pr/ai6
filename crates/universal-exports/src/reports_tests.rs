//! Reports through the gateway: who files, who reads what, and what a Mission's link shows.

use std::sync::Arc;

use ai6::directorates::registry::backend::*;
use ai6::{ConfigProvider, ConfigRecord, ConfigScope, SqlProvider};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::controls::tests::{Served, served_with_roster};

/// `operator-007` reads and files in G and manages clearances; `viewer-007` only reads G.
/// Neither is a chief.
const ROLES: &str = r#"
    [role.viewer]
    actions = ["mission.read"]

    [role.filer]
    actions = ["mission.read", "mission.create"]

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "viewer"

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "filer"

    [role.security]
    actions = ["access.manage", "access.read"]

    [[rule]]
    subject = "person:operator-007"
    scope = "config:reports"
    role = "security"
"#;

struct Fixture {
    served: Served,
    source: Arc<SqlProvider>,
    _config: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let config = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", config.path().join("config.db").display());
    let source = Arc::new(SqlProvider::open(&location).unwrap());
    for operation in ["G", "L"] {
        source
            .put(
                ConfigRecord::Operation,
                &ConfigScope::Global,
                operation,
                &json!({}),
            )
            .unwrap();
    }
    let served = served_with_roster(ROLES, Some(source.clone())).await;
    let brief: ai6::Brief = serde_json::from_value(json!({
        "title": "work",
        "id": "mission-g",
        "mission_id": "mission-g",
        "operation": "G",
    }))
    .unwrap();
    served.registry.submit_brief(&brief).unwrap();
    Fixture {
        served,
        source,
        _config: config,
    }
}

impl Fixture {
    fn read_in(&self, id: &str, subjects: &[&str]) {
        let read_in: Vec<Value> = subjects
            .iter()
            .map(|subject| json!({"subject": subject}))
            .collect();
        self.source
            .put(
                ConfigRecord::Compartment,
                &ConfigScope::Global,
                id,
                &json!({"actions": ["mission.status"], "covers": [], "read_in": read_in}),
            )
            .unwrap();
    }

    async fn call(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}{path}", self.served.base))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// Files `body` in G as the operator, linked to `mission-g`, and answers its ID.
    async fn file(&self, mut body: Value) -> String {
        body["scope"] = json!("operation:G");
        body["links"] = json!([{"kind": "mission", "id": "mission-g"}]);
        let (status, filed) = self
            .call(
                &self.served.operator.clone(),
                Method::POST,
                "/v1/reports",
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{filed}");
        filed["id"].as_str().unwrap().to_owned()
    }
}

#[tokio::test]
async fn a_person_files_a_report_that_the_operation_reads_and_no_mission_is_made() {
    let fixture = fixture().await;
    let (operator, viewer) = (
        fixture.served.operator.clone(),
        fixture.served.viewer.clone(),
    );
    let briefs = fixture
        .served
        .registry
        .filed_briefs("G", 100)
        .unwrap()
        .len();
    // Reading is not filing.
    let (status, _) = fixture
        .call(
            &viewer,
            Method::POST,
            "/v1/reports",
            Some(json!({"scope": "operation:G", "title": "t", "text": "x"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = fixture
        .call(
            &operator,
            Method::POST,
            "/v1/reports",
            Some(json!({"scope": "operation:L", "title": "t", "text": "x"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "nor in another Operation");
    let (status, refused) = fixture
        .call(
            &operator,
            Method::POST,
            "/v1/reports",
            Some(json!({"scope": "operation:G", "title": "t", "text": "x", "credibility": 1})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "credibility is the assessor's: {refused}"
    );

    let id = fixture
        .file(json!({"title": "The release notes are stale", "text": "they name 0.3", "priority": "low"}))
        .await;
    let stored = fixture.served.registry.report(&id).unwrap().unwrap();
    assert!(!stored.untrusted(), "a person's text is trusted");
    assert!(stored.about_mission("mission-g"));
    let (status, shown) = fixture
        .call(&viewer, Method::GET, &format!("/v1/reports/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["text"], "they name 0.3");
    assert_eq!(shown["filer"]["person"], "operator-007");
    let (status, listed) = fixture
        .call(&viewer, Method::GET, "/v1/reports?scope=operation:G", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["reports"][0]["id"], id.as_str());
    // L is not theirs to read: its Reports neither list nor show.
    let (status, _) = fixture
        .call(&viewer, Method::GET, "/v1/reports?scope=operation:L", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Filing made no Mission or brief.
    assert_eq!(
        fixture
            .served
            .registry
            .filed_briefs("G", 100)
            .unwrap()
            .len(),
        briefs
    );
}

#[tokio::test]
async fn a_compartment_and_a_classification_hold_back_the_content_and_a_link_shows_markings() {
    let fixture = fixture().await;
    let (operator, viewer) = (
        fixture.served.operator.clone(),
        fixture.served.viewer.clone(),
    );
    fixture.read_in("KESTREL", &["person:operator-007"]);
    let marked = fixture
        .file(json!({
            "title": "A key leaked",
            "text": "the deploy key is in the CI log",
            "compartments": ["KESTREL"],
        }))
        .await;
    let classified = fixture
        .file(json!({
            "title": "Vendor compromise",
            "text": "the vendor signs with a stolen key",
            "classification": "restricted",
            "tearline": {"text": "hold vendor upgrades"},
        }))
        .await;

    // The viewer reads G but is not read in: refused, with its ID and markings alone.
    let (status, refused) = fixture
        .call(&viewer, Method::GET, &format!("/v1/reports/{marked}"), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(refused["report"]["id"], marked.as_str());
    assert_eq!(refused["report"]["compartments"], json!(["KESTREL"]));
    assert!(!refused.to_string().contains("deploy key"), "{refused}");
    // The filer is read in.
    let (status, _) = fixture
        .call(
            &operator,
            Method::GET,
            &format!("/v1/reports/{marked}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Nobody here is cleared above the lowest classification: the tearline, in its place.
    for reader in [&viewer, &operator] {
        let (status, shown) = fixture
            .call(
                reader,
                Method::GET,
                &format!("/v1/reports/{classified}"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(shown["sight"], "tearline");
        assert_eq!(shown["text"], "hold vendor upgrades");
        assert!(!shown.to_string().contains("stolen"), "{shown}");
    }

    // The Mission they both read shows the marked Report by ID and markings only.
    let (status, linked) = fixture
        .call(&viewer, Method::GET, "/v1/missions/mission-g/reports", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    let entry = linked["reports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|report| report["id"] == marked.as_str())
        .unwrap();
    assert_eq!(entry["sight"], "markings");
    assert!(entry.get("title").is_none(), "{entry}");
    assert!(!linked.to_string().contains("deploy key"), "{linked}");

    // Lowering the markings needs someone read in: not the viewer, but the operator.
    let lower = json!({"compartments": []});
    let (status, _) = fixture
        .call(
            &viewer,
            Method::PUT,
            &format!("/v1/reports/{marked}/markings"),
            Some(lower.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = fixture
        .call(
            &operator,
            Method::PUT,
            &format!("/v1/reports/{marked}/markings"),
            Some(json!({"compartments": ["OTHER"]})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "not past what the operator holds"
    );
    let (status, _) = fixture
        .call(
            &operator,
            Method::PUT,
            &format!("/v1/reports/{marked}/markings"),
            Some(lower),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, shown) = fixture
        .call(&viewer, Method::GET, &format!("/v1/reports/{marked}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shown["text"], "the deploy key is in the CI log");
    // Read out, the filer's own compartment Report is held back from them too.
    let again = fixture
        .file(json!({"title": "t", "text": "x", "compartments": ["KESTREL"]}))
        .await;
    fixture.read_in("KESTREL", &["person:someone-else"]);
    let (status, _) = fixture
        .call(
            &operator,
            Method::GET,
            &format!("/v1/reports/{again}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_clearance_granted_through_the_gateway_holds_at_once_and_so_does_its_revoke() {
    let fixture = fixture().await;
    let (operator, viewer) = (
        fixture.served.operator.clone(),
        fixture.served.viewer.clone(),
    );
    let classified = fixture
        .file(json!({
            "title": "A supplier is compromised",
            "text": "the vendor signs with a stolen key",
            "classification": "restricted",
        }))
        .await;
    let path = format!("/v1/reports/{classified}");
    let (status, _) = fixture.call(&viewer, Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "nobody is cleared yet");

    // Granting is a security change: the viewer holds no access.manage on config:reports.
    let grant = "/v1/reports/clearances/person:viewer-007";
    let restricted = json!({"classification": "restricted"});
    let (status, _) = fixture
        .call(&viewer, Method::PUT, grant, Some(restricted.clone()))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = fixture
        .call(
            &operator,
            Method::PUT,
            grant,
            Some(json!({"classification": "cosmic"})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "not one of the classifications"
    );
    let (status, granted) = fixture
        .call(&operator, Method::PUT, grant, Some(restricted))
        .await;
    assert_eq!(status, StatusCode::OK, "{granted}");
    assert_eq!(granted["by"], "person:operator-007");

    // The next request reads it, with nothing restarted.
    let (status, shown) = fixture.call(&viewer, Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["text"], "the vendor signs with a stolen key");
    let (status, listed) = fixture
        .call(&operator, Method::GET, "/v1/reports/clearances", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["clearances"][0]["subject"], "person:viewer-007");
    assert_eq!(listed["clearances"][0]["classification"], "restricted");

    // Revoked, the next request is refused.
    let (status, revoked) = fixture.call(&operator, Method::DELETE, grant, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revoked["revoked"], true);
    let (status, _) = fixture.call(&viewer, Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Each change is in the audit log, by whom, of what.
    let audits: Vec<(String, String, String)> = fixture
        .served
        .registry
        .list_audit(200)
        .unwrap()
        .into_iter()
        .filter(|record| record.action.starts_with("report.clearance."))
        .map(|record| (record.action, record.actor, record.target))
        .collect();
    assert_eq!(audits.len(), 2, "{audits:?}");
    for expected in [
        ("report.clearance.grant", "person:viewer-007 restricted"),
        ("report.clearance.revoke", "person:viewer-007"),
    ] {
        assert!(
            audits
                .iter()
                .any(|(action, actor, target)| action == expected.0
                    && actor == "operator-007"
                    && target == expected.1),
            "{expected:?} in {audits:?}"
        );
    }
}
