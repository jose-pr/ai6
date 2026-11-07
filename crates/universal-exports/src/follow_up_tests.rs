//! Missions numbered by their ticket: a follow-up takes its item's handle and the next follow-up
//! number, and a Mission created with its issue waits for the issue to be numbered by it.

use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::mission_issues::{LinkRelation, ProposalState};
use ai6::{Origin, SourceItem};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::controls::tests::{Served, served_with};

/// The operator may create Missions and issues; the viewer may create Missions only.
const ROLES: &str = r#"
    [role.creator]
    actions = ["mission.read", "mission.create"]

    [role.worker]
    actions = ["mission.read", "mission.create", "issue.create"]

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "creator"

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "worker"
"#;

async fn create(served: &Served, token: &str, body: Value) -> (StatusCode, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}/v1/user/operations/G/missions", served.base))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// Ticket `o/r#31` of the Operation's `gh` instance, polled: Mission `G-gh-31`.
fn polled_ticket(served: &Served) {
    let item = SourceItem {
        id: "o/r#31".to_owned(),
        title: "The build fails".to_owned(),
        body: "It broke.".to_owned(),
        url: None,
        labels: Vec::new(),
        author: None,
        updated: "2026-11-05T00:00:00Z".to_owned(),
        code: Some("31".to_owned()),
        code_tag: Some("gh".to_owned()),
    };
    served
        .registry
        .ingest_source_item("G", "gh", &item, "hq")
        .unwrap();
    assert_eq!(
        served.registry.list_new().unwrap()[0].shown_handle(),
        "G-gh-31"
    );
}

#[tokio::test]
async fn follow_ups_are_numbered_by_their_ticket_and_linked_to_it() {
    let served = served_with(ROLES).await;
    polled_ticket(&served);
    let follow_up = json!({
        "title": "Add the regression test", "prompt": "Cover the fix.",
        "issue": { "tracker": "gh", "item": "o/r#31" },
    });
    let mut handles = Vec::new();
    for _ in 0..2 {
        let (status, created) = create(&served, &served.operator, follow_up.clone()).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{created}");
        handles.push(created["handle"].as_str().unwrap().to_owned());
        let mission = created["mission_id"].as_str().unwrap();
        let links = served.registry.issue_links(mission).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(
            (
                links[0].tracker.as_str(),
                links[0].item.as_str(),
                links[0].relation
            ),
            ("gh", "o/r#31", LinkRelation::Related)
        );
        assert_eq!(links[0].linked_by, "person:operator-007");
        let queued = served
            .registry
            .list_new()
            .unwrap()
            .into_iter()
            .find(|brief| brief.key() == mission)
            .expect("the follow-up is queued");
        assert_eq!(queued.data["follow_up"]["item"], "o/r#31");
        assert_eq!(
            queued.provenance.unwrap().origin,
            Origin::Operator,
            "a person's brief"
        );
    }
    assert_eq!(handles, ["G-gh-31-2", "G-gh-31-3"]);

    let (status, refused) = create(
        &served,
        &served.operator,
        json!({
            "title": "Elsewhere", "prompt": "Work.",
            "issue": { "tracker": "gh", "item": "o/r#99" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        refused
            .to_string()
            .contains("has no Mission in Operation G"),
        "{refused}"
    );
    let (status, _) = create(
        &served,
        &served.operator,
        json!({
            "title": "Both", "prompt": "Work.",
            "issue": { "tracker": "gh", "item": "o/r#31" },
            "create_issue": { "tracker": "gh" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_mission_created_with_its_issue_waits_for_it_and_needs_issue_create() {
    let served = served_with(ROLES).await;
    let asked = json!({
        "title": "Fix the login page", "prompt": "It times out.", "agent_id": "007",
        "create_issue": { "tracker": "gh" },
    });
    let (status, _) = create(&served, &served.viewer, asked.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "issue.create is needed");
    assert!(served.registry.issue_proposals(None).unwrap().is_empty());

    let (status, created) = create(&served, &served.operator, asked).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["state"], "awaiting_issue");
    assert!(created.get("mission_id").is_none() && created.get("handle").is_none());
    let proposal = served
        .registry
        .issue_proposal(created["proposal"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(proposal.state, ProposalState::Approved);
    assert_eq!(proposal.proposer, "person:operator-007");
    let spec = &proposal.specs[0];
    assert_eq!(
        (spec.tracker.as_str(), spec.title.as_str()),
        ("gh", "Fix the login page")
    );
    let brief = spec
        .mission
        .as_ref()
        .expect("the issue carries its Mission");
    assert_eq!(brief.title, "Fix the login page");
    assert_eq!(brief.agent.as_deref(), Some("007"));
    assert!(
        brief.mission_id.is_none(),
        "numbered only once the issue exists"
    );
    let provenance = brief.provenance.as_ref().unwrap();
    assert_eq!(
        (provenance.origin, provenance.principal.as_deref()),
        (Origin::Operator, Some("operator-007"))
    );
    assert!(served.registry.list_new().unwrap().is_empty());
}

#[tokio::test]
async fn a_mission_is_created_with_the_deliverables_it_owes_or_refused_for_a_malformed_one() {
    let served = served_with(ROLES).await;
    let (status, created) = create(
        &served,
        &served.operator,
        json!({
            "title": "Ship it", "prompt": "Push the branch.",
            "deliverables": ["branch", "file:docs/report.md"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let queued = served
        .registry
        .list_new()
        .unwrap()
        .into_iter()
        .find(|brief| brief.key() == created["mission_id"].as_str().unwrap())
        .expect("queued");
    let owed = ai6::directorates::m::deliverables::declared(&[], &queued.data).unwrap();
    assert_eq!(
        owed.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["branch", "file:docs/report.md"]
    );

    for bad in [json!(["tag:v1"]), json!(["file"]), json!([""])] {
        let (status, refused) = create(
            &served,
            &served.operator,
            json!({ "title": "Ship it", "prompt": "Push.", "deliverables": bad }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    }
}
