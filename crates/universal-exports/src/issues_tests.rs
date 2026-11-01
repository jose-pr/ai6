//! The gateway's linked issue routes: each refused without its action, a foreign reference a
//! `404` before anything is audited, and a person's writes turned into Registry records.

use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::mission_issues::{
    IssueLink, IssueProposal, IssueQuestion, LinkRelation, LinkState, OutboxKind, OutboxState,
    ProposalState, ProposedIssue, QuestionState,
};
use reqwest::Method;
use serde_json::{Value, json};

use crate::controls::tests::{Served, served_with};

/// The operator holds every issue action; the viewer only reads.
const ROLES: &str = r#"
    [role.viewer]
    actions = ["mission.read"]

    [role.worker]
    actions = ["mission.read", "issue.link", "issue.comment", "issue.update", "issue.create"]

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "viewer"

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "worker"
"#;

/// As [`ROLES`], with `mission.create` too.
const ROLES_READY: &str = r#"
    [role.viewer]
    actions = ["mission.read"]

    [role.worker]
    actions = ["mission.read", "mission.create", "issue.create", "issue.update"]

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "viewer"

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "worker"
"#;

const MISSION: &str = "mission-g";

fn link(served: &Served, item: &str, state: LinkState) {
    served
        .registry
        .link_issue(&IssueLink {
            operation: "G".to_owned(),
            mission_id: MISSION.to_owned(),
            tracker: "gh".to_owned(),
            item: item.to_owned(),
            url: None,
            relation: LinkRelation::Source,
            state,
            linked_by: "m".to_owned(),
            linked_at: 1.0,
            status_comment: None,
            phase: None,
            snapshot: None,
        })
        .unwrap();
}

fn escape(reference: &str) -> String {
    reference
        .replace('#', "%23")
        .replace('/', "%2F")
        .replace(':', "%3A")
}

async fn call(
    served: &Served,
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .request(method, format!("{}/v1/user/operations/{path}", served.base))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    request.send().await.unwrap()
}

fn issues_path(rest: &str) -> String {
    format!("G/missions/{MISSION}/issues{rest}")
}

#[tokio::test]
async fn each_route_is_refused_without_its_action() {
    let served = served_with(ROLES).await;
    link(&served, "o/r#1", LinkState::Active);
    let reference = escape("gh:o/r#1");
    let cases: Vec<(Method, String, Option<Value>)> = vec![
        (
            Method::POST,
            issues_path(""),
            Some(json!({"issue": "gh:o/r#2"})),
        ),
        (Method::DELETE, issues_path(&format!("/{reference}")), None),
        (
            Method::POST,
            issues_path(&format!("/{reference}/accept")),
            None,
        ),
        (
            Method::POST,
            issues_path(&format!("/{reference}/comments")),
            Some(json!({"body": "x"})),
        ),
        (
            Method::POST,
            issues_path(&format!("/{reference}/update")),
            Some(json!({"close": true})),
        ),
        (
            Method::POST,
            "G/issues".to_owned(),
            Some(json!({"tracker": "gh", "title": "x"})),
        ),
        (
            Method::POST,
            "G/issue-proposals/nope/approve".to_owned(),
            None,
        ),
        (
            Method::POST,
            "G/issue-proposals/nope/reject".to_owned(),
            None,
        ),
    ];
    for (method, path, body) in cases {
        let refused = call(&served, &served.viewer, method, &path, body).await;
        assert_eq!(refused.status(), 403, "{path}");
    }
    assert!(
        served
            .registry
            .outbox_entries(Some(MISSION))
            .unwrap()
            .is_empty(),
        "nothing was queued"
    );
}

#[tokio::test]
async fn a_foreign_reference_is_not_found_before_any_audit() {
    let served = served_with(ROLES).await;
    link(&served, "o/r#1", LinkState::Active);
    let before = served.registry.audit_records().unwrap().len();
    let stranger = escape("gh:o/r#99");
    for (method, path, body) in [
        (Method::DELETE, issues_path(&format!("/{stranger}")), None),
        (
            Method::POST,
            issues_path(&format!("/{stranger}/comments")),
            Some(json!({"body": "x"})),
        ),
        (
            Method::POST,
            issues_path(&format!("/{stranger}/update")),
            Some(json!({"close": true})),
        ),
        (
            Method::POST,
            issues_path(&format!("/{stranger}/accept")),
            None,
        ),
        (
            Method::GET,
            "G/missions/no-such-mission/issues".to_owned(),
            None,
        ),
    ] {
        let answer = call(&served, &served.operator, method, &path, body).await;
        assert_eq!(answer.status(), 404, "{path}");
    }
    assert_eq!(served.registry.audit_records().unwrap().len(), before);
}

#[tokio::test]
async fn reading_needs_only_mission_read_and_shows_links_questions_and_posts() {
    let served = served_with(ROLES).await;
    link(&served, "o/r#1", LinkState::Active);
    served
        .registry
        .ask_issue_question(&IssueQuestion {
            id: String::new(),
            mission_id: MISSION.to_owned(),
            tracker: "gh".to_owned(),
            item: "o/r#1".to_owned(),
            entry: "e".to_owned(),
            asked_at: 5.0,
            expires_at: 500.0,
            state: QuestionState::Open,
        })
        .unwrap();
    let answer = call(&served, &served.viewer, Method::GET, &issues_path(""), None).await;
    assert_eq!(answer.status(), 200);
    let answer: Value = answer.json().await.unwrap();
    assert_eq!(answer["issues"][0]["issue"], "gh:o/r#1");
    assert_eq!(answer["issues"][0]["relation"], "source");
    assert_eq!(answer["issues"][0]["open_question"]["expires_at"], 500.0);
    assert_eq!(answer["replies"]["delivered"], 0);
}

#[tokio::test]
async fn a_person_links_comments_updates_and_unlinks_through_the_outbox() {
    let served = served_with(ROLES).await;
    let linked = call(
        &served,
        &served.operator,
        Method::POST,
        "G/missions/mission-g/issues",
        Some(json!({"issue": "gh:o/r#1", "relation": "related"})),
    )
    .await;
    assert_eq!(linked.status(), 200, "{:?}", linked.text().await);
    let links = served.registry.issue_links(MISSION).unwrap();
    assert_eq!(links[0].linked_by, "person:operator-007");
    assert_eq!(links[0].state, LinkState::Active);
    let entries = served.registry.outbox_entries(Some(MISSION)).unwrap();
    assert_eq!(entries[0].kind, OutboxKind::Link);
    assert_eq!(entries[0].payload["resolve"], true);

    let source = call(
        &served,
        &served.operator,
        Method::POST,
        &issues_path(""),
        Some(json!({"issue": "gh:o/r#1", "relation": "source"})),
    )
    .await;
    assert_eq!(source.status(), 400, "only M links a source");

    let reference = escape("gh:o/r#1");
    let path = issues_path(&format!("/{reference}/comments"));
    let posted = call(
        &served,
        &served.operator,
        Method::POST,
        &path,
        Some(json!({"body": "cc @maint, see https://anywhere.example/x"})),
    )
    .await;
    assert_eq!(posted.status(), 200);
    let answer: Value = posted.json().await.unwrap();
    assert!(answer.get("answered").is_none(), "{answer}");
    let entries = served.registry.outbox_entries(Some(MISSION)).unwrap();
    let comment = entries
        .iter()
        .find(|entry| entry.kind == OutboxKind::Comment)
        .unwrap();
    assert!(comment.body.contains("cc @maint"), "{}", comment.body);
    assert!(comment.body.contains("anywhere.example"), "no host rule");
    assert!(
        comment
            .body
            .ends_with("Posted through AI6 by operator-007."),
        "{}",
        comment.body
    );
    let token = ["gh", "p_", "0123456789abcdefghij0123456789abcdef"].concat();
    for bad in [token.as_str(), "invisible\u{202e}", "  "] {
        let refused = call(
            &served,
            &served.operator,
            Method::POST,
            &path,
            Some(json!({"body": bad})),
        )
        .await;
        assert_eq!(refused.status(), 400, "{bad}");
    }

    let updated = call(
        &served,
        &served.operator,
        Method::POST,
        &issues_path(&format!("/{reference}/update")),
        Some(json!({"close": true, "add_labels": ["bug"]})),
    )
    .await;
    assert_eq!(updated.status(), 200);
    let entries = served.registry.outbox_entries(Some(MISSION)).unwrap();
    let update = entries
        .iter()
        .find(|entry| entry.kind == OutboxKind::Update)
        .unwrap();
    assert_eq!(
        update.state,
        OutboxState::Pending,
        "no green ink for a person"
    );
    assert_eq!(update.payload["state"], "closed");
    let ready = call(
        &served,
        &served.operator,
        Method::POST,
        &issues_path(&format!("/{reference}/update")),
        Some(json!({"add_labels": ["ai6:ready"]})),
    )
    .await;
    assert_eq!(
        ready.status(),
        403,
        "marking work ready needs mission.create"
    );

    let unlinked = call(
        &served,
        &served.operator,
        Method::DELETE,
        &issues_path(&format!("/{reference}")),
        None,
    )
    .await;
    assert_eq!(unlinked.status(), 200);
    assert_eq!(
        served.registry.issue_links(MISSION).unwrap()[0].state,
        LinkState::Unlinked
    );
    let actions: Vec<String> = served
        .registry
        .audit_records()
        .unwrap()
        .into_iter()
        .map(|record| record.action)
        .collect();
    for action in [
        "issue.link",
        "issue.comment.queued",
        "issue.update.queued",
        "issue.unlink",
    ] {
        assert!(actions.iter().any(|seen| seen == action), "{action}");
    }
    assert!(
        !served
            .registry
            .audit_records()
            .unwrap()
            .iter()
            .any(|record| record.target.contains("anywhere.example")),
        "audit never holds the text"
    );
}

#[tokio::test]
async fn a_persons_comment_on_a_waiting_issue_is_its_answer_as_an_operators_text() {
    let served = served_with(ROLES).await;
    link(&served, "o/r#1", LinkState::Active);
    served
        .registry
        .ask_issue_question(&IssueQuestion {
            id: String::new(),
            mission_id: MISSION.to_owned(),
            tracker: "gh".to_owned(),
            item: "o/r#1".to_owned(),
            entry: "e".to_owned(),
            asked_at: 5.0,
            expires_at: 500.0,
            state: QuestionState::Open,
        })
        .unwrap();
    let posted = call(
        &served,
        &served.operator,
        Method::POST,
        &issues_path(&format!("/{}/comments", escape("gh:o/r#1"))),
        Some(json!({"body": "Use the release branch."})),
    )
    .await;
    let answer: Value = posted.json().await.unwrap();
    assert_eq!(answer["answered"], true, "{answer}");
    let pending = served.registry.pending_issue_replies().unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].operator, "no exposure when it is delivered");
    assert_eq!(pending[0].body, "Use the release branch.");
}

#[tokio::test]
async fn an_agents_proposed_link_is_accepted_and_only_once() {
    let served = served_with(ROLES).await;
    link(&served, "o/r#1", LinkState::Proposed);
    let path = issues_path(&format!("/{}/accept", escape("gh:o/r#1")));
    let accepted = call(&served, &served.operator, Method::POST, &path, None).await;
    assert_eq!(accepted.status(), 200);
    assert_eq!(
        served.registry.issue_links(MISSION).unwrap()[0].state,
        LinkState::Active
    );
    let again = call(&served, &served.operator, Method::POST, &path, None).await;
    assert_eq!(again.status(), 409);
}

#[tokio::test]
async fn a_person_creates_for_no_mission_and_a_parent_needs_one() {
    let served = served_with(ROLES).await;
    let created = call(
        &served,
        &served.operator,
        Method::POST,
        "G/issues",
        Some(json!({"tracker": "gh", "title": "New work", "body": "Details."})),
    )
    .await;
    assert_eq!(created.status(), 202);
    let answer: Value = created.json().await.unwrap();
    let held = served
        .registry
        .issue_proposal(answer["proposal"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        held.state,
        ProposalState::Approved,
        "the action is the approval"
    );
    assert_eq!(held.proposer, "person:operator-007");
    assert!(held.specs[0].body.starts_with("Details."));
    assert!(
        held.specs[0]
            .body
            .ends_with("Created through AI6 by operator-007.")
    );

    let orphan = call(
        &served,
        &served.operator,
        Method::POST,
        "G/issues",
        Some(json!({"tracker": "gh", "title": "x", "parent": "gh:o/r#1"})),
    )
    .await;
    assert_eq!(orphan.status(), 400);
    let ready = call(
        &served,
        &served.operator,
        Method::POST,
        "G/issues",
        Some(json!({"tracker": "gh", "title": "x", "labels": ["ai6:ready"]})),
    )
    .await;
    assert_eq!(ready.status(), 403, "a label that starts work");
}

fn pending_proposal(served: &Served, operation: &str) -> String {
    served
        .registry
        .propose_issues(&IssueProposal {
            id: String::new(),
            operation: operation.to_owned(),
            mission_id: MISSION.to_owned(),
            proposer: "agent:007".to_owned(),
            specs: vec![ProposedIssue {
                tracker: "gh".to_owned(),
                title: "Split".to_owned(),
                ..ProposedIssue::default()
            }],
            exposure: Vec::new(),
            findings: Vec::new(),
            dropped: vec!["assignees".to_owned()],
            state: ProposalState::Pending,
            decided_by: None,
            mark_ready: false,
            created: Vec::new(),
            failed: Vec::new(),
            retry_key: None,
            told: false,
            at: 1.0,
        })
        .unwrap()
        .id
}

#[tokio::test]
async fn a_proposal_is_listed_decided_once_and_mark_ready_needs_mission_create() {
    let served = served_with(ROLES).await;
    let first = pending_proposal(&served, "G");
    let foreign = pending_proposal(&served, "L");
    let listed: Value = call(
        &served,
        &served.viewer,
        Method::GET,
        "G/issue-proposals",
        None,
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1, "only this Operation's");
    assert_eq!(listed[0]["id"], first.as_str());
    assert_eq!(listed[0]["dropped"][0], "assignees");

    let elsewhere = call(
        &served,
        &served.operator,
        Method::POST,
        &format!("G/issue-proposals/{foreign}/approve"),
        None,
    )
    .await;
    assert_eq!(elsewhere.status(), 404);

    let ready = call(
        &served,
        &served.operator,
        Method::POST,
        &format!("G/issue-proposals/{first}/approve"),
        Some(json!({"mark_ready": true})),
    )
    .await;
    assert_eq!(ready.status(), 403, "mark_ready needs mission.create too");

    let approved = call(
        &served,
        &served.operator,
        Method::POST,
        &format!("G/issue-proposals/{first}/approve"),
        None,
    )
    .await;
    assert_eq!(approved.status(), 200);
    let approved: Value = approved.json().await.unwrap();
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["decided_by"], "operator-007");
    let again = call(
        &served,
        &served.operator,
        Method::POST,
        &format!("G/issue-proposals/{first}/reject"),
        None,
    )
    .await;
    assert_eq!(again.status(), 409);
    assert!(
        served
            .registry
            .audit_records()
            .unwrap()
            .iter()
            .any(|record| record.action == "issue.create.approved" && record.target == first)
    );
}

#[tokio::test]
async fn a_rejections_reason_goes_on_the_timeline_and_not_in_audit() {
    let served = served_with(ROLES).await;
    let id = pending_proposal(&served, "G");
    let path = format!("G/issue-proposals/{id}/reject");
    let control = call(
        &served,
        &served.operator,
        Method::POST,
        &path,
        Some(json!({"reason": "a\u{0}b"})),
    )
    .await;
    assert_eq!(control.status(), 400);
    let rejected = call(
        &served,
        &served.operator,
        Method::POST,
        &path,
        Some(json!({"reason": "Not this quarter."})),
    )
    .await;
    assert_eq!(rejected.status(), 200);
    let events = served.registry.timeline_events("G").unwrap();
    let event = events
        .iter()
        .find(|event| event.action == "proposal_rejected")
        .expect("the reason is recorded");
    assert!(format!("{event:?}").contains("Not this quarter."));
    assert!(
        !served
            .registry
            .audit_records()
            .unwrap()
            .iter()
            .any(|record| record.target.contains("Not this quarter."))
    );
}

#[tokio::test]
async fn approving_with_mark_ready_records_it_for_a_person_who_may() {
    let served = served_with(ROLES_READY).await;
    let id = pending_proposal(&served, "G");
    let approved = call(
        &served,
        &served.operator,
        Method::POST,
        &format!("G/issue-proposals/{id}/approve"),
        Some(json!({"mark_ready": true})),
    )
    .await;
    assert_eq!(approved.status(), 200);
    assert!(
        served
            .registry
            .issue_proposal(&id)
            .unwrap()
            .unwrap()
            .mark_ready
    );
}

#[tokio::test]
async fn a_stations_issue_weakenings_show_only_for_operations_the_person_reads() {
    let served = served_with(ROLES).await;
    served
        .registry
        .check_in(
            "worker",
            std::collections::BTreeMap::from([
                ("operations".to_owned(), json!(["G", "L"])),
                (
                    "issues_weakened".to_owned(),
                    json!([
                        "Operation \"G\" issues is weakened: issues.create direct",
                        "Operation \"L\" issues is weakened: issues.links any",
                    ]),
                ),
            ]),
        )
        .unwrap();
    let board: Value = reqwest::Client::new()
        .get(format!("{}/v1/stations", served.base))
        .bearer_auth(&served.viewer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let worker = board["stations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|station| station["station_id"] == "worker")
        .unwrap();
    assert_eq!(
        worker["issues_weakened"],
        json!(["Operation \"G\" issues is weakened: issues.create direct"])
    );
}
