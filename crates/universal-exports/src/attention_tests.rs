//! The attention summary through the gateway: what is current, what is past, and what could
//! not be checked.

use std::collections::BTreeMap;
use std::sync::Arc;

use ai6::directorates::registry::backend::*;
use ai6::{
    ActionScope, ApprovalLimits, ApprovalRequest, ConfigProvider, ConfigRecord, ConfigScope,
};
use ai6::{AuditRecord, SqlProvider};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::controls::tests::{STATION_TOKEN, Served, served_with_roster};

/// `watcher-007` reads and decides in G, reads the audit of G and of Station `worker`;
/// `viewer-007` only reads G.
const ROLES: &str = r#"
    [role.watcher]
    actions = ["mission.read", "mission.create", "approval.decide", "audit.read", "station.read"]

    [role.viewer]
    actions = ["mission.read"]

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "watcher"

    [[rule]]
    subject = "person:operator-007"
    scope = "station:worker"
    role = "watcher"

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "viewer"
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
    Fixture {
        served,
        source,
        _config: config,
    }
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

impl Fixture {
    /// Station `worker`'s check-in as it reports `config`.
    fn report(&self, config: Value) {
        self.served
            .registry
            .check_in(
                "worker",
                BTreeMap::from([
                    ("operations".to_owned(), json!(["G", "L"])),
                    ("config".to_owned(), config),
                ]),
            )
            .unwrap();
    }

    /// The audit record a Station writes when it refuses a configuration.
    fn refusal(&self, station: &str, revision: u64, ago: f64) {
        self.served
            .registry
            .record_audit(&AuditRecord {
                id: format!("config-refused-{revision}-{station}"),
                actor: format!("station:{station}"),
                action: "config.refused".to_owned(),
                operation: String::new(),
                resource: Some(format!("station:{station}")),
                target: format!("configuration revision {revision}: the roster does not parse"),
                at: now() - ago,
            })
            .unwrap();
    }

    async fn get(&self, token: &str, path: &str) -> (StatusCode, Value) {
        let response = reqwest::Client::new()
            .request(Method::GET, format!("{}{path}", self.served.base))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn summary(&self, token: &str) -> Value {
        let (status, body) = self.get(token, "/v1/user/attention").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn lifecycle(&self, operation: &str, mission: &str, session: &str, reason: &str) {
        let response = reqwest::Client::new()
            .post(format!(
                "{}/v1/station/sessions/{session}/lifecycle/1",
                self.served.base
            ))
            .bearer_auth(STATION_TOKEN)
            .json(&json!({"operation_id": operation, "mission_id": mission,
                          "state": "blocked", "reason": reason}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success(), "{}", response.status());
    }

    /// Rewrites Station `worker`'s last check-in to `seconds` ago.
    fn go_dark(&self, seconds: f64) {
        let path = self
            .served
            .directory
            .path()
            .join("stations")
            .join("worker.json");
        let mut record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        record["seen"] = json!(now() - seconds);
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    }
}

fn items<'a>(summary: &'a Value, kind: &str) -> Vec<&'a Value> {
    summary["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == kind)
        .collect()
}

fn coverage<'a>(summary: &'a Value, source: &str) -> &'a Value {
    summary["coverage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|read| read["source"] == source)
        .unwrap_or_else(|| panic!("no coverage for {source}: {summary}"))
}

#[tokio::test]
async fn an_online_station_refusing_a_configuration_now_is_a_current_item() {
    let fixture = fixture().await;
    fixture.report(json!({
        "revision": "77",
        "refused": ["configuration revision 78: the roster does not parse"],
    }));
    // The refusal is also in the audit log; it is one item, not two.
    fixture.refusal("worker", 78, 30.0);
    let summary = fixture.summary(&fixture.served.operator).await;

    let refused = items(&summary, "config_refused");
    assert_eq!(refused.len(), 1, "{summary}");
    let item = refused[0];
    assert_eq!(item["condition"], "current");
    assert_eq!(item["scope"], "station:worker");
    assert_eq!(item["entity"]["kind"], "station");
    assert_eq!(item["entity"]["id"], "worker");
    assert_eq!(item["link"], "app:/stations?station_id=worker&tab=settings");
    assert!(item["title"].as_str().unwrap().contains("revision 78"));
    assert!(
        item["title"]
            .as_str()
            .unwrap()
            .contains("keeps running revision 77")
    );
    assert!(item["checked_at"].as_f64().unwrap() > 0.0);
    assert!(item["observed_at"].as_f64().unwrap() > 0.0);

    assert_eq!(summary["clear"], false);
    assert_eq!(summary["current"], 1);
    assert_eq!(summary["scope"]["label"], "All accessible Operations");
    assert!(
        summary["headline"]
            .as_str()
            .unwrap()
            .contains("1 item needs attention now")
    );
}

#[tokio::test]
async fn an_older_refusal_followed_by_an_accepted_revision_is_historical_and_not_current() {
    let fixture = fixture().await;
    // The Station refused revision 78, then accepted 79 and reports no refusal.
    fixture.report(json!({"revision": "79"}));
    fixture.refusal("worker", 78, 3600.0);
    let summary = fixture.summary(&fixture.served.operator).await;

    let refused = items(&summary, "config_refused");
    assert_eq!(refused.len(), 1, "{summary}");
    assert_eq!(refused[0]["condition"], "historical");
    let detail = refused[0]["detail"].as_str().unwrap();
    assert!(detail.contains("now runs revision 79"), "{detail}");
    assert!(refused[0]["since"].as_f64().unwrap() < now() - 3000.0);

    assert_eq!(summary["current"], 0);
    assert_eq!(summary["historical"], 1);
    assert_eq!(summary["clear"], true, "{summary}");
    let headline = summary["headline"].as_str().unwrap();
    assert!(
        headline.starts_with("Nothing needs attention now"),
        "{headline}"
    );
    assert!(headline.contains("historical"), "{headline}");
}

#[tokio::test]
async fn a_refusal_older_than_the_window_is_left_out() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture.refusal("worker", 78, 30.0 * 86_400.0);
    let summary = fixture.summary(&fixture.served.operator).await;
    assert!(items(&summary, "config_refused").is_empty(), "{summary}");
    assert_eq!(summary["clear"], true);
}

#[tokio::test]
async fn a_dark_station_leaves_its_old_refusal_unverified() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture.refusal("worker", 78, 600.0);
    fixture.go_dark(3600.0);
    let summary = fixture.summary(&fixture.served.operator).await;

    let dark = items(&summary, "station_dark");
    assert_eq!(dark.len(), 1, "{summary}");
    assert_eq!(dark[0]["condition"], "current");
    let refused = items(&summary, "config_refused");
    assert_eq!(refused[0]["condition"], "unverified", "{summary}");
    assert!(
        refused[0]["detail"]
            .as_str()
            .unwrap()
            .contains("could not be checked")
    );
    assert_eq!(summary["clear"], false);
    assert!(
        summary["headline"]
            .as_str()
            .unwrap()
            .contains("could not be confirmed")
    );
}

#[tokio::test]
async fn what_the_person_may_not_read_is_named_and_the_rest_is_still_answered() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture.refusal("worker", 78, 3600.0);
    // The viewer reads G but neither decides nor audits.
    let summary = fixture.summary(&fixture.served.viewer).await;

    for source in ["green_ink", "held_briefs", "mission_proposals", "audit"] {
        let read = coverage(&summary, source);
        assert_eq!(read["status"], "not_permitted", "{source}: {summary}");
        assert!(read["checked"].as_array().unwrap().is_empty());
    }
    assert_eq!(coverage(&summary, "stations")["status"], "checked");
    assert_eq!(coverage(&summary, "sessions")["status"], "checked");
    // The refusal history is not theirs to see, so it is not listed.
    assert!(items(&summary, "config_refused").is_empty());
    let headline = summary["headline"].as_str().unwrap();
    assert!(headline.contains("Not visible to you"), "{headline}");
    assert!(headline.contains("Green ink requests"), "{headline}");
}

#[tokio::test]
async fn a_request_for_green_ink_is_one_item_with_its_blocked_session() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture
        .lifecycle("G", "mission-g", "session-g", "green-ink")
        .await;

    // Blocked for green ink with no request visible: the Session is the item.
    let summary = fixture.summary(&fixture.served.viewer).await;
    let blocked = items(&summary, "blocked_session");
    assert_eq!(blocked.len(), 1, "{summary}");
    assert_eq!(blocked[0]["entity"]["id"], "session-g");
    assert!(
        blocked[0]["title"]
            .as_str()
            .unwrap()
            .contains("waits for green ink")
    );
    assert_eq!(blocked[0]["link"], "app:/green_ink?operation_id=G");

    // Where the person may decide, the request is the item and the Session is not a second.
    let request = ApprovalRequest::try_new(
        "G",
        ai6::MissionId::try_new("mission-g").unwrap(),
        ai6::SessionId::try_new("session-g").unwrap(),
        "007",
        ActionScope::try_new("push", "origin main").unwrap(),
        "release",
        "push-1",
        &ApprovalLimits::default(),
    )
    .unwrap()
    .expiring(now(), std::time::Duration::from_secs(3600));
    fixture.served.registry.request_approval(&request).unwrap();
    let summary = fixture.summary(&fixture.served.operator).await;
    assert!(items(&summary, "blocked_session").is_empty(), "{summary}");
    let asked = items(&summary, "green_ink");
    assert_eq!(asked.len(), 1, "{summary}");
    assert_eq!(asked[0]["entity"]["kind"], "approval");
    assert_eq!(asked[0]["scope"], "operation:G");
    assert!(
        asked[0]["link"]
            .as_str()
            .unwrap()
            .starts_with("app:/green_ink?operation_id=G&request_id=green-ink-")
    );
}

#[tokio::test]
async fn an_operation_filter_narrows_the_scope_and_says_so() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture
        .lifecycle("G", "mission-g", "session-g", "budget")
        .await;
    let (status, summary) = fixture
        .get(
            &fixture.served.operator,
            "/v1/user/attention?operation_id=L",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["scope"]["kind"], "operation");
    assert_eq!(summary["scope"]["label"], "Operation L");
    assert!(
        items(&summary, "blocked_session").is_empty(),
        "G is not in the scope"
    );

    let (_, all) = fixture
        .get(
            &fixture.served.operator,
            "/v1/user/attention?operation_id=G",
        )
        .await;
    assert_eq!(items(&all, "blocked_session").len(), 1, "{all}");

    let (status, _) = fixture
        .get(&fixture.served.operator, "/v1/user/attention?operation_id=")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = fixture.get("not-a-token", "/v1/user/attention").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_blocked_session_read_through_a_compartment_is_listed_and_the_crossing_is_audited() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture
        .lifecycle("L", "mission-l", "session-l", "budget")
        .await;
    // The viewer holds no rule on L.
    let before = fixture.summary(&fixture.served.viewer).await;
    assert!(items(&before, "blocked_session").is_empty(), "{before}");
    let audits = |fixture: &Fixture| {
        fixture
            .served
            .registry
            .list_audit(100)
            .unwrap()
            .into_iter()
            .filter(|record| record.action == "compartment.read")
            .count()
    };
    assert_eq!(audits(&fixture), 0);

    fixture
        .source
        .put(
            ConfigRecord::Compartment,
            &ConfigScope::Global,
            "LAPWING",
            &json!({
                "actions": ["mission.read"],
                "covers": [{"operation": "L"}],
                "read_in": [{"subject": "person:viewer-007"}],
            }),
        )
        .unwrap();
    let after = fixture.summary(&fixture.served.viewer).await;
    let blocked = items(&after, "blocked_session");
    assert_eq!(blocked.len(), 1, "{after}");
    assert_eq!(blocked[0]["entity"]["operation_id"], "L");
    assert!(
        audits(&fixture) > 0,
        "the crossing is written to the audit log"
    );
}
