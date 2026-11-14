//! The attention summary through the gateway: what is current, what is past, and what could
//! not be checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::charges::{ChargeLogEntry, ChargeUpdate};
use ai6::directorates::registry::reports::{Report, ReportFiler, ReportScope};
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
    actions = ["mission.read", "mission.create", "approval.decide", "audit.read", "station.read", "pool.read"]

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
    subject = "person:operator-007"
    scope = "config:reports"
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

    /// An open Report of `scope`, filed by a person.
    fn open_report(&self, id: &str, scope: ReportScope, title: &str) {
        self.served
            .registry
            .file_report(&Report::new(
                id,
                scope,
                "finding",
                ai6::directorates::registry::logbook::Severity::Info,
                title,
                "What was seen.",
                ReportFiler::Person {
                    person: "operator-007".to_owned(),
                },
                now() - 60.0,
            ))
            .unwrap();
    }

    /// `scope`'s charge, held by `worker` for `lease`, whose last turn recorded `turn`.
    fn charge(&self, scope: &str, lease: Duration, turn: &str) {
        let record = self
            .served
            .registry
            .claim_charge(scope, "kg", "worker", lease)
            .unwrap();
        let mut update = ChargeUpdate::default();
        update.last_wake = Some(1_000.0);
        update.turn_at = Some(1_000.0);
        update.log = vec![ChargeLogEntry::new("turn", turn, 1_000.0)];
        self.served
            .registry
            .update_charge(scope, "worker", record.fence, None, update)
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

    async fn lifecycle(&self, operation: &str, mission: &str, run: &str, reason: &str) {
        let response = reqwest::Client::new()
            .post(format!(
                "{}/v1/station/runs/{run}/lifecycle/1",
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
    // Where a change is reviewed: the record editor on this Station's records.
    assert_eq!(item["revision"], "78");
    assert_eq!(
        item["editor_link"],
        "app:/roster?kind=station&station_id=worker"
    );

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
    assert_eq!(coverage(&summary, "runs")["status"], "checked");
    // The refusal history is not theirs to see, so it is not listed.
    assert!(items(&summary, "config_refused").is_empty());
    let headline = summary["headline"].as_str().unwrap();
    assert!(headline.contains("Not visible to you"), "{headline}");
    assert!(headline.contains("Green ink requests"), "{headline}");
}

#[tokio::test]
async fn a_request_for_green_ink_is_one_item_with_its_blocked_run() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture
        .lifecycle("G", "mission-g", "run-g", "green-ink")
        .await;

    // Blocked for green ink with no request visible: the Run is the item.
    let summary = fixture.summary(&fixture.served.viewer).await;
    let blocked = items(&summary, "blocked_run");
    assert_eq!(blocked.len(), 1, "{summary}");
    assert_eq!(blocked[0]["entity"]["id"], "run-g");
    assert!(
        blocked[0]["title"]
            .as_str()
            .unwrap()
            .contains("waits for green ink")
    );
    assert_eq!(blocked[0]["link"], "app:/green_ink?operation_id=G");

    // Where the person may decide, the request is the item and the Run is not a second.
    let request = ApprovalRequest::try_new(
        "G",
        ai6::MissionId::try_new("mission-g").unwrap(),
        ai6::RunId::try_new("run-g").unwrap(),
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
    assert!(items(&summary, "blocked_run").is_empty(), "{summary}");
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
    fixture.lifecycle("G", "mission-g", "run-g", "budget").await;
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
        items(&summary, "blocked_run").is_empty(),
        "G is not in the scope"
    );

    let (_, all) = fixture
        .get(
            &fixture.served.operator,
            "/v1/user/attention?operation_id=G",
        )
        .await;
    assert_eq!(items(&all, "blocked_run").len(), 1, "{all}");

    let (status, _) = fixture
        .get(&fixture.served.operator, "/v1/user/attention?operation_id=")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = fixture.get("not-a-token", "/v1/user/attention").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_blocked_run_read_through_a_compartment_is_listed_and_the_crossing_is_audited() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture.lifecycle("L", "mission-l", "run-l", "budget").await;
    // The viewer holds no rule on L.
    let before = fixture.summary(&fixture.served.viewer).await;
    assert!(items(&before, "blocked_run").is_empty(), "{before}");
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
    let blocked = items(&after, "blocked_run");
    assert_eq!(blocked.len(), 1, "{after}");
    assert_eq!(blocked[0]["entity"]["operation_id"], "L");
    assert!(
        audits(&fixture) > 0,
        "the crossing is written to the audit log"
    );
}

#[tokio::test]
async fn an_unassessed_report_is_an_item_that_opens_it_and_coverage_says_what_was_read() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture.open_report(
        "report-g",
        ReportScope::Operation("G".to_owned()),
        "Key in log",
    );
    fixture.open_report("report-m", ReportScope::M, "Upstream advisory");
    let summary = fixture.summary(&fixture.served.operator).await;

    let reports = items(&summary, "report");
    assert_eq!(reports.len(), 2, "{summary}");
    let in_g = reports
        .iter()
        .find(|r| r["id"] == "report:report-g")
        .unwrap();
    assert_eq!(in_g["condition"], "current");
    assert_eq!(in_g["scope"], "operation:G");
    assert_eq!(in_g["entity"]["kind"], "report");
    assert_eq!(in_g["entity"]["operation_id"], "G");
    assert_eq!(in_g["link"], "app:/reports?report_id=report-g");
    assert!(in_g["title"].as_str().unwrap().contains("Key in log"));
    let in_m = reports
        .iter()
        .find(|r| r["id"] == "report:report-m")
        .unwrap();
    assert_eq!(in_m["scope"], "m");
    assert_eq!(in_m["link"], "app:/reports?report_id=report-m");

    let read = coverage(&summary, "reports");
    assert_eq!(read["status"], "checked");
    let checked: Vec<&str> = read["checked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|scope| scope.as_str().unwrap())
        .collect();
    assert!(checked.contains(&"operation:G"), "{read}");
    assert!(checked.contains(&"station:worker"), "{read}");
    assert!(checked.contains(&"m"), "{read}");
    assert_eq!(summary["clear"], false);
}

#[tokio::test]
async fn reports_a_person_may_not_read_are_named_not_listed_and_m_is_not_mentioned() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture.open_report(
        "report-g",
        ReportScope::Operation("G".to_owned()),
        "Visible",
    );
    fixture.open_report(
        "report-w",
        ReportScope::Station("worker".to_owned()),
        "Not for the viewer",
    );
    fixture.open_report("report-m", ReportScope::M, "Not for the viewer either");
    let summary = fixture.summary(&fixture.served.viewer).await;

    let reports = items(&summary, "report");
    assert_eq!(reports.len(), 1, "{summary}");
    assert_eq!(reports[0]["id"], "report:report-g");
    let read = coverage(&summary, "reports");
    assert_eq!(read["status"], "partial");
    assert_eq!(read["not_permitted"], json!(["station:worker"]));
    assert!(!summary.to_string().contains("Not for the viewer"));
    assert!(
        summary["headline"]
            .as_str()
            .unwrap()
            .contains("Not visible to you"),
        "{summary}"
    );
}

#[tokio::test]
async fn a_report_read_through_a_compartment_is_listed_and_the_crossing_is_audited() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture.lifecycle("L", "mission-l", "run-l", "budget").await;
    fixture.open_report("report-l", ReportScope::Operation("L".to_owned()), "In L");
    let before = fixture.summary(&fixture.served.viewer).await;
    assert!(items(&before, "report").is_empty(), "{before}");
    let crossings = |fixture: &Fixture| {
        fixture
            .served
            .registry
            .list_audit(100)
            .unwrap()
            .into_iter()
            .filter(|record| record.action == "compartment.read")
            .count()
    };
    assert_eq!(crossings(&fixture), 0);

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
    let reports = items(&after, "report");
    assert_eq!(reports.len(), 1, "{after}");
    assert_eq!(reports[0]["entity"]["operation_id"], "L");
    assert!(crossings(&fixture) > 0, "the crossing is audited");
}

#[tokio::test]
async fn a_charge_whose_last_turn_failed_or_whose_lease_lapsed_is_an_item() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture.charge(
        "operation:G",
        Duration::from_secs(600),
        "woke for a timer; the turn ended failed: the model refused. It said: nothing",
    );
    fixture.charge(
        "station:worker",
        Duration::ZERO,
        "woke for a timer; the turn ended completed. It said: all well",
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    let summary = fixture.summary(&fixture.served.operator).await;

    let failed = items(&summary, "charge_failed");
    assert_eq!(failed.len(), 1, "{summary}");
    assert_eq!(failed[0]["id"], "charge_failed:operation:G");
    assert_eq!(failed[0]["condition"], "current");
    assert_eq!(failed[0]["entity"]["kind"], "charge");
    assert_eq!(failed[0]["entity"]["operation_id"], "G");
    assert_eq!(failed[0]["link"], "app:/charges?operation_id=G");
    assert!(
        failed[0]["detail"]
            .as_str()
            .unwrap()
            .contains("failed: the model refused"),
        "{summary}"
    );
    // A completed turn is no trouble; its lapsed lease is.
    let lapsed = items(&summary, "charge_lapsed");
    assert_eq!(lapsed.len(), 1, "{summary}");
    assert_eq!(lapsed[0]["entity"]["station_id"], "worker");
    assert_eq!(lapsed[0]["link"], "app:/charges?station_id=worker");
    assert_eq!(coverage(&summary, "charges")["status"], "checked");
}

#[tokio::test]
async fn a_healthy_charge_is_not_an_item_and_one_the_person_cannot_see_is_not_read() {
    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    fixture.charge(
        "operation:G",
        Duration::from_secs(600),
        "woke for a timer; the turn ended completed",
    );
    fixture.charge(
        "operation:L",
        Duration::from_secs(600),
        "woke for a timer; the turn ended failed: no quota",
    );
    let summary = fixture.summary(&fixture.served.operator).await;
    assert!(items(&summary, "charge_failed").is_empty(), "{summary}");
    assert!(items(&summary, "charge_lapsed").is_empty(), "{summary}");
}

#[tokio::test]
async fn titles_name_an_operation_by_its_name_and_by_its_id_when_it_has_none() {
    let fixture = fixture().await;
    fixture
        .served
        .registry
        .check_in(
            "worker",
            BTreeMap::from([
                ("operations".to_owned(), json!(["G", "L"])),
                ("operation_lineage".to_owned(), json!({"G": [], "L": []})),
                ("operation_names".to_owned(), json!({"G": "Guatiao"})),
                ("config".to_owned(), json!({"revision": "79"})),
            ]),
        )
        .unwrap();
    fixture.open_report("report-g", ReportScope::Operation("G".to_owned()), "In G");
    fixture.charge(
        "operation:G",
        Duration::from_secs(600),
        "woke for a timer; the turn ended failed: no quota",
    );
    let summary = fixture.summary(&fixture.served.operator).await;
    let title = |kind: &str| {
        items(&summary, kind)[0]["title"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(
        title("report").ends_with("in Operation Guatiao"),
        "{summary}"
    );
    assert!(
        title("charge_failed").starts_with("Charge of Operation Guatiao"),
        "{summary}"
    );
    let all = summary.to_string();
    assert!(!all.contains("Operation G "), "{all}");
    // The scope's label is the person's words too; the ID stays in the machine fields.
    assert_eq!(summary["scope"]["label"], "All accessible Operations");
    let (_, narrowed) = fixture
        .get(
            &fixture.served.operator,
            "/v1/user/attention?operation_id=G",
        )
        .await;
    assert_eq!(narrowed["scope"]["label"], "Operation Guatiao");
    assert_eq!(narrowed["scope"]["operation_id"], "G");
}

#[tokio::test]
async fn a_harness_login_that_needs_a_person_is_one_item_saying_what_to_run() {
    use ai6::{Account, AccountKind, AccountPool, AccountProfile, AccountRef};

    let fixture = fixture().await;
    fixture.report(json!({"revision": "79"}));
    let pool = AccountPool::new("free", "acp")
        .account(Account::new("shared", AccountKind::HarnessLogin).files([".fake/auth.json"]))
        .account(Account::new("steady", AccountKind::HarnessLogin).files([".fake/auth.json"]));
    for name in ["shared", "steady"] {
        fixture
            .served
            .registry
            .declare_account(
                &AccountRef::new("free", name),
                "worker",
                &AccountProfile::of(&pool, name, vec!["G".to_owned()]).unwrap(),
            )
            .unwrap();
    }
    let at = now() - 60.0;
    let standing = |needs: Option<&str>| {
        let needs = needs.map(str::to_owned);
        move |state: &mut Option<ai6::LoginState>| {
            let mut login = ai6::LoginState::default();
            login.stations = vec!["worker".to_owned()];
            login.last_attempt_at = Some(at);
            login.needs_person = needs.clone();
            *state = Some(login);
        }
    };
    // `shared` has none yet; `steady` is healthy.
    fixture
        .served
        .registry
        .change_login(&AccountRef::new("free", "steady"), &mut standing(None))
        .unwrap();
    let summary = fixture.summary(&fixture.served.operator).await;
    let needing = items(&summary, "login_needs_person");
    assert_eq!(needing.len(), 1, "{summary}");
    assert_eq!(needing[0]["id"], "login_needs_person:free/shared");
    assert_eq!(needing[0]["condition"], "current");
    assert_eq!(needing[0]["entity"]["kind"], "account");
    assert_eq!(needing[0]["entity"]["operation_id"], "G");
    assert_eq!(needing[0]["link"], "app:/pools?operation_id=G&pool=free");
    assert!(
        needing[0]["detail"]
            .as_str()
            .unwrap()
            .contains("ai6 pools add-login free shared --handler acp --replace"),
        "{summary}"
    );
    assert_eq!(coverage(&summary, "pools")["status"], "checked");
    assert!(!summary["clear"].as_bool().unwrap());

    // A renewal that was refused marks the other one as well, and says why.
    fixture
        .served
        .registry
        .change_login(
            &AccountRef::new("free", "steady"),
            &mut standing(Some("3 renewals failed in a row")),
        )
        .unwrap();
    let summary = fixture.summary(&fixture.served.operator).await;
    let needing = items(&summary, "login_needs_person");
    assert_eq!(needing.len(), 2, "{summary}");
    let steady = needing
        .iter()
        .find(|item| item["id"] == "login_needs_person:free/steady")
        .unwrap();
    assert_eq!(steady["since"], at);
    assert!(
        steady["detail"]
            .as_str()
            .unwrap()
            .contains("3 renewals failed in a row")
    );
    assert!(!summary.to_string().contains("auth.json"), "{summary}");

    // Reading logins is `pool.read`, which the viewer lacks: named, not a gap.
    let summary = fixture.summary(&fixture.served.viewer).await;
    assert_eq!(coverage(&summary, "pools")["status"], "not_permitted");
    assert!(items(&summary, "login_needs_person").is_empty());
}
