//! The document routes: what the frontmatter holds, which references resolve for whom, and the
//! Markdown alone. The refusals come first in each case.

use std::sync::Arc;
use std::time::Duration;

use ai6::directorates::m::review::{PanelSeat, ReviewGate, ReviewPolicy};
use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::mission_issues::{
    IssueLink, IssueSnapshot, LinkRelation, LinkState,
};
use ai6::directorates::registry::reviews::{MissionReview, ReviewMaterial, ReviewStatus};
use ai6::{ActionScope, Brief, ConfigProvider, ConfigRecord, ConfigScope, SqlProvider};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::controls::tests::{STATION_TOKEN, Served, served_with_unex};

/// `operator-007` reads and decides in G; `viewer-007` only reads G. Neither has a rule on L.
const ROLES: &str = r#"
    [role.viewer]
    actions = ["mission.read"]

    [role.signer]
    actions = ["mission.read", "mission.create", "approval.decide", "charge.converse"]

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "viewer"

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "signer"
"#;

struct Fixture {
    served: Served,
    source: Arc<SqlProvider>,
    _config: tempfile::TempDir,
}

/// Operation G has a GitHub Enterprise and a Jira Source; L has none. Each has a Mission.
async fn fixture() -> Fixture {
    let config = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", config.path().join("config.db").display());
    let source = Arc::new(SqlProvider::open(&location).unwrap());
    source
        .put(
            ConfigRecord::Operation,
            &ConfigScope::Global,
            "G",
            &json!({"sources": {
                "github": {
                    "provider": "github",
                    "repo": "o/n",
                    "api_url": "https://ghe.example/api/v3"
                },
                "jira": {
                    "provider": "jira",
                    "site": "https://tracker.example/",
                    "project": "OPS"
                }
            }}),
        )
        .unwrap();
    source
        .put(
            ConfigRecord::Operation,
            &ConfigScope::Global,
            "L",
            &json!({}),
        )
        .unwrap();
    let mut unex = ai6::UnexLimits::default();
    unex.text.document_board_wait = ai6::Seconds(Duration::from_secs(2));
    let served = served_with_unex(
        ROLES,
        Some(source.clone() as Arc<dyn ConfigProvider>),
        Some(unex),
    )
    .await;
    for (mission, operation) in [("mission-g", "G"), ("mission-l", "L")] {
        let brief: Brief = serde_json::from_value(json!({
            "title": "work",
            "body": "do the work",
            "id": mission,
            "mission_id": mission,
            "operation": operation,
        }))
        .unwrap();
        served.registry.submit_brief(&brief).unwrap();
    }
    Fixture {
        served,
        source,
        _config: config,
    }
}

impl Fixture {
    async fn get(&self, token: &str, path: &str, accept: Option<&str>) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .get(format!("{}{path}", self.served.base))
            .bearer_auth(token);
        if let Some(accept) = accept {
            request = request.header("accept", accept);
        }
        request.send().await.unwrap()
    }

    async fn document(&self, token: &str, path: &str) -> Value {
        let response = self.get(token, path, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        response.json().await.unwrap()
    }

    /// A Run of `mission-g` whose Station published `prompt` as the brief, with an environment
    /// variable named and a value the manifest has no place for.
    async fn publish_brief(&self, prompt: &str) {
        let registered = reqwest::Client::new()
            .post(format!("{}/v1/station/runs", self.served.base))
            .bearer_auth(STATION_TOKEN)
            .json(&json!({
                "operation_id": "G",
                "mission_id": "mission-g",
                "run_id": "run-brief",
                "mission": {
                    "name": "Stale release notes",
                    "prompt": prompt,
                    "agent_id": "analyst",
                    "role_id": "case-officer",
                    "handler_provider": "claude",
                    "terminal_provider": "pty",
                    "working_directory": "/work/tree",
                    "model": "sonnet",
                    "effort": "high",
                    "environment_keys": ["API_TOKEN"],
                    "environment": {"API_TOKEN": "s3cret-environment-value"},
                    "station_id": "worker",
                    "theater_id": "local",
                    "mode": "field"
                }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(registered.status(), StatusCode::CREATED);
    }

    /// The Headquarters Station, answering each board read as its providers would. It is
    /// stopped when the returned handle is dropped.
    fn headquarters(&self) -> tokio::task::JoinHandle<()> {
        let base = self.served.base.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            loop {
                let Ok(response) = client
                    .get(format!("{base}/v1/station/source-reads/next"))
                    .query(&[("station", "hq"), ("wait_ms", "200")])
                    .bearer_auth(STATION_TOKEN)
                    .send()
                    .await
                else {
                    continue;
                };
                if response.status() != StatusCode::OK {
                    continue;
                }
                let read: Value = response.json().await.unwrap();
                let answer = if read["boards"] == true {
                    json!({"boards": [
                        {"tracker": "github", "provider": "github",
                         "url": "https://ghe.example/o/n/issues"},
                        {"tracker": "jira", "provider": "jira",
                         "url": "https://tracker.example/jira/software/projects/OPS/boards"}
                    ]})
                } else {
                    json!({"error": "not asked for"})
                };
                let _ = client
                    .post(format!(
                        "{base}/v1/station/source-reads/{}/result",
                        read["id"].as_str().unwrap()
                    ))
                    .query(&[("station", "hq")])
                    .bearer_auth(STATION_TOKEN)
                    .json(&answer)
                    .send()
                    .await;
            }
        })
    }

    fn link(&self, item: &str, title: &str) {
        self.served
            .registry
            .link_issue(&IssueLink {
                operation: "G".to_owned(),
                mission_id: "mission-g".to_owned(),
                tracker: "github".to_owned(),
                item: item.to_owned(),
                url: Some(format!("https://ghe.example/o/n/issues/{}", &item[4..])),
                relation: LinkRelation::Source,
                state: LinkState::Active,
                linked_by: "m".to_owned(),
                linked_at: 1.0,
                status_comment: None,
                phase: None,
                snapshot: Some(IssueSnapshot {
                    title: title.to_owned(),
                    state: Some("open".to_owned()),
                    ..IssueSnapshot::default()
                }),
            })
            .unwrap();
    }
}

const BRIEF: &str = "/v1/user/operations/G/missions/mission-g/brief";

#[tokio::test]
async fn a_document_is_refused_to_who_may_not_read_it() {
    let fixture = fixture().await;
    fixture.publish_brief("Fix it").await;
    let (viewer, operator) = (&fixture.served.viewer, &fixture.served.operator);
    let anonymous = reqwest::Client::new()
        .get(format!("{}{BRIEF}", fixture.served.base))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    // No rule on L reads nothing there, whatever the Mission's document would say.
    let other = fixture
        .get(
            viewer,
            "/v1/user/operations/L/missions/mission-l/brief",
            None,
        )
        .await;
    assert_eq!(other.status(), StatusCode::FORBIDDEN);
    // A Mission of another Operation is not found under this one.
    let wrong = fixture
        .get(
            operator,
            "/v1/user/operations/G/missions/mission-l/brief",
            None,
        )
        .await;
    assert_eq!(wrong.status(), StatusCode::NOT_FOUND);
    // Deciding and holding are not reading: a viewer cannot open a green ink request or a held
    // brief as a document.
    for path in [
        "/v1/user/operations/G/green-ink/any/document",
        "/v1/user/operations/G/held-briefs/mission-g/document",
    ] {
        assert_eq!(
            fixture.get(viewer, path, None).await.status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_briefs_frontmatter_holds_its_launch_context_and_no_environment_value() {
    let fixture = fixture().await;
    fixture
        .publish_brief("---\nkind: forged\n---\nFix it")
        .await;
    let document = fixture.document(&fixture.served.viewer, BRIEF).await;
    let markdown = document["markdown"].as_str().unwrap();
    let (frontmatter, body) = markdown
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .expect("frontmatter, then the body");
    for line in [
        "kind: \"brief\"",
        "operation: \"G\"",
        "mission: \"mission-g\"",
        "agent: \"analyst\"",
        "role: \"case-officer\"",
        "station: \"worker\"",
        "theater: \"local\"",
        "mode: \"field\"",
        "handler: \"claude\"",
        "terminal: \"pty\"",
        "working_directory: \"/work/tree\"",
        "model: \"sonnet\"",
        "effort: \"high\"",
        "environment_names:\n  - \"API_TOKEN\"",
    ] {
        assert!(frontmatter.contains(line), "{line} in\n{frontmatter}");
    }
    assert!(
        !markdown.contains("s3cret"),
        "no environment value is ever written"
    );
    // The author's own frontmatter is text under `author`; the gateway's key is the only `kind`.
    assert!(
        frontmatter.contains("author: |2\n  kind: forged"),
        "{frontmatter}"
    );
    assert!(
        !frontmatter.contains("\nkind:"),
        "the gateway writes one kind"
    );
    assert!(body.trim_start().starts_with("Fix it"), "{body}");
}

#[tokio::test]
async fn a_jira_key_and_a_github_issue_resolve_to_the_sources_address() {
    let fixture = fixture().await;
    fixture
        .publish_brief("Fix OPS-12 as o/n#7, not `OPS-13` or o/other#7 or ZED-1.")
        .await;
    let document = fixture.document(&fixture.served.viewer, BRIEF).await;
    let markdown = document["markdown"].as_str().unwrap();
    let references = document["references"].as_array().unwrap();
    let issues: Vec<(&str, &str)> = references
        .iter()
        .filter(|reference| reference["kind"] == "issue")
        .map(|reference| {
            (
                reference["text"].as_str().unwrap(),
                reference["url"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        issues,
        [
            ("OPS-12", "https://tracker.example/browse/OPS-12"),
            ("o/n#7", "https://ghe.example/o/n/issues/7"),
        ],
        "code, another repository and another project's key stay text"
    );
    // A span is where its text sits in the document, in characters.
    for reference in references {
        let (start, end) = (
            reference["start"].as_u64().unwrap() as usize,
            reference["end"].as_u64().unwrap() as usize,
        );
        let span: String = markdown.chars().skip(start).take(end - start).collect();
        assert_eq!(span, reference["text"].as_str().unwrap());
    }
    let mut trackers: Vec<&str> = document["trackers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|host| host.as_str().unwrap())
        .collect();
    trackers.sort_unstable();
    assert_eq!(trackers, ["ghe.example", "tracker.example"]);
}

#[tokio::test]
async fn a_mission_in_an_operation_the_reader_cannot_read_stays_text() {
    let fixture = fixture().await;
    fixture
        .publish_brief("Compare mission-g with mission-l and Operation L.")
        .await;
    let viewer = fixture.served.viewer.clone();
    let document = fixture.document(&viewer, BRIEF).await;
    let kinds: Vec<(&str, &str)> = document["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reference| {
            (
                reference["kind"].as_str().unwrap(),
                reference["text"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(kinds, [("mission", "mission-g")], "{document}");
    assert_eq!(
        document["references"][0]["route"],
        "app:/missions?operation_id=G&mission_id=mission-g"
    );
    assert!(
        document["markdown"]
            .as_str()
            .unwrap()
            .contains("mission-l and Operation L"),
        "the words are still there"
    );

    // A compartment can grant the crossing, and the crossing is audited before it is allowed.
    fixture
        .source
        .put(
            ConfigRecord::Compartment,
            &ConfigScope::Global,
            "LAPWING",
            &json!({
                "actions": ["mission.read"],
                "covers": [{"operation": "L"}],
                "read_in": [{"subject": "person:viewer-007"}]
            }),
        )
        .unwrap();
    let document = fixture.document(&viewer, BRIEF).await;
    let kinds: Vec<(&str, &str)> = document["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reference| {
            (
                reference["kind"].as_str().unwrap(),
                reference["text"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(kinds.contains(&("mission", "mission-l")), "{kinds:?}");
    assert!(kinds.contains(&("operation", "L")), "{kinds:?}");
    let audited = fixture
        .served
        .registry
        .list_audit(100)
        .unwrap()
        .into_iter()
        .any(|record| record.action == "compartment.read" && record.operation == "L");
    assert!(audited, "the crossing into L is in the audit log");
}

#[tokio::test]
async fn text_markdown_returns_the_same_text() {
    let fixture = fixture().await;
    fixture.publish_brief("Fix OPS-12.").await;
    let viewer = fixture.served.viewer.clone();
    let json = fixture.document(&viewer, BRIEF).await;
    let plain = fixture.get(&viewer, BRIEF, Some("text/markdown")).await;
    assert_eq!(plain.status(), StatusCode::OK);
    assert!(
        plain.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/markdown")
    );
    assert_eq!(plain.headers()["vary"], "accept");
    assert_eq!(
        plain.text().await.unwrap(),
        json["markdown"].as_str().unwrap()
    );
    // JSON is still what a client that asks for it, or for anything, gets.
    for accept in ["application/json", "*/*"] {
        let response = fixture.get(&viewer, BRIEF, Some(accept)).await;
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
    }
}

#[tokio::test]
async fn a_mission_reference_lists_its_linked_issues_and_its_operations_boards() {
    let fixture = fixture().await;
    fixture.link("o/n#7", "Stale release notes");
    fixture.link("o/n#9", "Wrong version");
    fixture
        .served
        .registry
        .link_issue(&IssueLink {
            operation: "G".to_owned(),
            mission_id: "mission-g".to_owned(),
            tracker: "github".to_owned(),
            item: "o/n#11".to_owned(),
            url: None,
            relation: LinkRelation::Related,
            state: LinkState::Unlinked,
            linked_by: "m".to_owned(),
            linked_at: 1.0,
            status_comment: None,
            phase: None,
            snapshot: None,
        })
        .unwrap();
    fixture
        .publish_brief("See mission-g, and Operation G.")
        .await;
    let viewer = fixture.served.viewer.clone();

    // With no Headquarters Station answering, the references stand without boards.
    let quiet = fixture.document(&viewer, BRIEF).await;
    assert_eq!(quiet["references"][0]["boards"], Value::Null);

    let headquarters = fixture.headquarters();
    let document = fixture.document(&viewer, BRIEF).await;
    headquarters.abort();
    let references = document["references"].as_array().unwrap();
    let mission = &references[0];
    assert_eq!(mission["kind"], "mission");
    let issues: Vec<(&str, &str, &str)> = mission["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| {
            (
                issue["issue"].as_str().unwrap(),
                issue["title"].as_str().unwrap(),
                issue["url"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        issues,
        [
            (
                "github:o/n#7",
                "Stale release notes",
                "https://ghe.example/o/n/issues/7"
            ),
            (
                "github:o/n#9",
                "Wrong version",
                "https://ghe.example/o/n/issues/9"
            ),
        ],
        "an unlinked issue is not offered"
    );
    let boards = |reference: &Value| -> Vec<(String, String)> {
        reference["boards"]
            .as_array()
            .unwrap()
            .iter()
            .map(|board| {
                (
                    board["provider"].as_str().unwrap().to_owned(),
                    board["url"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let expected = [
        (
            "github".to_owned(),
            "https://ghe.example/o/n/issues".to_owned(),
        ),
        (
            "jira".to_owned(),
            "https://tracker.example/jira/software/projects/OPS/boards".to_owned(),
        ),
    ];
    assert_eq!(boards(mission), expected);
    let operation = &references[1];
    assert_eq!(
        (operation["kind"].as_str(), operation["text"].as_str()),
        (Some("operation"), Some("G"))
    );
    assert_eq!(boards(operation), expected);
    assert_eq!(operation["route"], "app:/operations?operation_id=G");
}

#[tokio::test]
async fn a_text_past_the_documents_limits_is_cut_and_its_references_stay_text() {
    let fixture = {
        let config = tempfile::tempdir().unwrap();
        let location = format!("sqlite:{}", config.path().join("config.db").display());
        let source = Arc::new(SqlProvider::open(&location).unwrap());
        source
            .put(
                ConfigRecord::Operation,
                &ConfigScope::Global,
                "G",
                &json!({"sources": {"jira": {
                    "provider": "jira", "site": "https://tracker.example", "project": "OPS"
                }}}),
            )
            .unwrap();
        let mut unex = ai6::UnexLimits::default();
        unex.text.document_references = ai6::Limit::at(2);
        unex.text.document_bytes = ai6::Limit::at(1_000);
        let served = served_with_unex(
            ROLES,
            Some(source.clone() as Arc<dyn ConfigProvider>),
            Some(unex),
        )
        .await;
        Fixture {
            served,
            source,
            _config: config,
        }
    };
    let long = format!("OPS-1 OPS-2 OPS-3 {}", "x".repeat(3_000));
    fixture.publish_brief(&long).await;
    let document = fixture.document(&fixture.served.viewer, BRIEF).await;
    let markdown = document["markdown"].as_str().unwrap();
    assert!(markdown.len() <= 1_000, "{}", markdown.len());
    assert!(markdown.contains("truncated: true"));
    assert_eq!(document["references"].as_array().unwrap().len(), 2);
    assert_eq!(document["omitted_references"], 1);
}

#[tokio::test]
async fn green_ink_held_briefs_reviews_charges_and_reports_are_documents() {
    let fixture = fixture().await;
    let (operator, viewer) = (
        fixture.served.operator.clone(),
        fixture.served.viewer.clone(),
    );
    let registry = &fixture.served.registry;

    // Green ink: the reason is the body, the step the frontmatter.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let request = ai6::ApprovalRequest::try_new(
        "G",
        ai6::MissionId::try_new("mission-g").unwrap(),
        ai6::RunId::try_new("run-g").unwrap(),
        "007",
        ActionScope::try_new("push", "origin:refs/heads/main").unwrap(),
        "# Release\n\nThe fix for OPS-12 is green; see mission-g.",
        "retry-1",
        &Default::default(),
    )
    .unwrap()
    .expiring(now, Duration::from_secs(600))
    .with_links(vec![ai6::ApprovalLink::new(
        "Pull request",
        "https://ghe.example/o/n/pull/3",
    )]);
    registry.request_approval(&request).unwrap();
    let path = format!("/v1/user/operations/G/green-ink/{}/document", request.id);
    let document = fixture.document(&operator, &path).await;
    let markdown = document["markdown"].as_str().unwrap();
    for line in [
        "kind: \"green_ink\"",
        "state: \"pending\"",
        "step: \"push\"",
        "target: \"origin:refs/heads/main\"",
        "agent: \"007\"",
        "label: \"Pull request\"",
    ] {
        assert!(markdown.contains(line), "{line} in\n{markdown}");
    }
    assert!(markdown.contains("# Release\n\nThe fix for OPS-12"));
    let kinds: Vec<&str> = document["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reference| reference["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["issue", "mission"]);
    assert_eq!(
        fixture
            .get(
                &operator,
                "/v1/user/operations/G/green-ink/nope/document",
                None
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // A held brief.
    let held: Brief = serde_json::from_value(json!({
        "title": "Crash on save",
        "body": "untrusted words about OPS-12",
        "id": "held-1",
        "source": "source:github",
        "operation": "G",
    }))
    .unwrap();
    registry.submit_brief(&held).unwrap();
    let vetting = ai6::Vetting::from_findings(vec![
        ai6::Finding::new(ai6::FindingKind::Duplicate, "the same title as other work")
            .related("mission-g"),
    ]);
    registry
        .hold_brief("G", &held.key(), &vetting, 1.0)
        .unwrap()
        .unwrap();
    let path = format!("/v1/user/operations/G/held-briefs/{}/document", held.key());
    let document = fixture.document(&operator, &path).await;
    let markdown = document["markdown"].as_str().unwrap();
    for line in [
        "kind: \"held_brief\"",
        "title: \"Crash on save\"",
        "verdict: \"hold\"",
        "detail: \"the same title as other work\"",
    ] {
        assert!(markdown.contains(line), "{line} in\n{markdown}");
    }
    assert!(markdown.ends_with("untrusted words about OPS-12"));
    assert_eq!(document["references"][0]["kind"], "issue");

    // A Mission's reviews: the reasons are quoted, so they are never structure.
    let mut review = MissionReview::open(
        ("mission-g", "G", ReviewGate::Output),
        ("one", &ReviewPolicy::new(vec![PanelSeat::agent("r1")])),
        "007",
        ReviewMaterial::default(),
        1.0,
    );
    review.status = ReviewStatus::Person;
    review.why = Some("the panel did not agree".to_owned());
    registry.put_mission_review(&review).unwrap().unwrap();
    let path = "/v1/user/operations/G/missions/mission-g/reviews/document";
    let document = fixture.document(&viewer, path).await;
    let markdown = document["markdown"].as_str().unwrap();
    assert!(markdown.contains("kind: \"review\""), "{markdown}");
    assert!(markdown.contains("gate: \"output\""), "{markdown}");
    assert!(markdown.contains("> the panel did not agree"), "{markdown}");

    // A Station's report is its charge's; here the Operation's.
    let record = registry
        .claim_charge("operation:G", "kg", "worker", Duration::from_secs(600))
        .unwrap();
    let mut update = ai6::directorates::registry::charges::ChargeUpdate::default();
    update.last_wake = Some(1_000.0);
    update.turn_at = Some(1_000.0);
    update.log = vec![ai6::directorates::registry::charges::ChargeLogEntry::new(
        "turn",
        "woke on a timer; the turn ended completed. It said: all quiet, see OPS-12",
        1_000.0,
    )];
    registry
        .update_charge("operation:G", "worker", record.fence, None, update)
        .unwrap();
    let document = fixture
        .document(&operator, "/v1/user/charges/operation:G/document")
        .await;
    let markdown = document["markdown"].as_str().unwrap();
    for line in [
        "kind: \"charge\"",
        "scope: \"operation:G\"",
        "agent: \"kg\"",
        "station: \"worker\"",
    ] {
        assert!(markdown.contains(line), "{line} in\n{markdown}");
    }
    assert!(markdown.contains("# Report of operation:G"));
    assert!(markdown.contains("1970-01-01T00:16:40Z"));
    let kinds: Vec<&str> = document["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reference| reference["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["operation", "issue", "issue"],
        "the Operation the heading names, and the issue the last turn cited, again in the logbook"
    );
    assert_eq!(
        fixture
            .get(&viewer, "/v1/user/charges/operation:G/document", None)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "the logbook needs charge.converse"
    );

    // A Report: the text is the body, the title and the rest the frontmatter.
    let filed = reqwest::Client::new()
        .post(format!("{}/v1/reports", fixture.served.base))
        .bearer_auth(&operator)
        .json(&json!({
            "scope": "operation:G",
            "title": "Release notes are stale",
            "text": "They name 0.3, see OPS-12 and o/n#7."
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(filed.status(), StatusCode::CREATED);
    let id = filed.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let document = fixture
        .document(&viewer, &format!("/v1/reports/{id}/document"))
        .await;
    let markdown = document["markdown"].as_str().unwrap();
    for line in [
        "kind: \"finding\"",
        "scope: \"operation:G\"",
        "title: \"Release notes are stale\"",
        "sight: \"content\"",
    ] {
        assert!(markdown.contains(line), "{line} in\n{markdown}");
    }
    assert!(markdown.ends_with("They name 0.3, see OPS-12 and o/n#7."));
    assert_eq!(document["references"].as_array().unwrap().len(), 2);
    let plain = fixture
        .get(
            &viewer,
            &format!("/v1/reports/{id}/document"),
            Some("text/markdown"),
        )
        .await;
    assert_eq!(plain.text().await.unwrap(), markdown);
    assert_eq!(
        fixture
            .get(&viewer, "/v1/reports/not-a-report/document", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
