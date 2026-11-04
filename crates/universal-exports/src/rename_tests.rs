//! Renaming a Mission: its UID, and everything kept by it, stays; the new handle, the old one
//! and the UID all find it; the rename is audited and on the timeline.

use std::time::Duration;

use ai6::directorates::m::review::{PanelSeat, ReviewGate, ReviewPolicy};
use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::mission_issues::{IssueLink, LinkRelation, LinkState};
use ai6::directorates::registry::reviews::{MissionReview, ReviewMaterial};
use ai6::{
    Brief, Cable, CableDraft, CableRecipient, CableScope, CableState, CableTarget, MissionEntry,
    NewSession, new_uid,
};
use serde_json::{Value, json};

use crate::controls::tests::{STATION_TOKEN, Served, served};

fn brief(title: &str) -> Brief {
    serde_json::from_value(json!({"title": title, "id": title, "operation": "G"})).unwrap()
}

async fn call(
    served: &Served,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .request(method, format!("{}{path}", served.base))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    request.send().await.unwrap()
}

async fn rename(served: &Served, token: &str, id: &str, handle: &str) -> reqwest::Response {
    call(
        served,
        token,
        reqwest::Method::POST,
        &format!("/v1/user/operations/G/missions/{id}/rename"),
        Some(json!({ "handle": handle })),
    )
    .await
}

/// A Mission with a Session on the board and in the Registry, a Cable, a review and a linked
/// issue, every one kept by its UID.
async fn working_mission(served: &Served) -> String {
    let entered = served
        .registry
        .enter_brief(&brief("fix"), &MissionEntry::person(Some("G")))
        .unwrap();
    assert_eq!(entered.handle.as_deref(), Some("G-1"));
    let uid = entered.key();
    let registered = call(
        served,
        STATION_TOKEN,
        reqwest::Method::POST,
        "/v1/station/sessions",
        Some(json!({"operation_id": "G", "mission_id": uid, "session_id": "session-r"})),
    )
    .await;
    assert!(registered.status().is_success(), "{}", registered.status());
    served
        .registry
        .open_session(
            NewSession::new(new_uid(), &uid, "worker")
                .with_operation("G")
                .with_handle("G-1")
                .with_provider_session("session-r"),
            Duration::from_secs(60),
        )
        .unwrap();
    served
        .registry
        .link_issue(&IssueLink {
            operation: "G".to_owned(),
            mission_id: uid.clone(),
            tracker: "gh".to_owned(),
            item: "o/r#7".to_owned(),
            url: None,
            relation: LinkRelation::Related,
            state: LinkState::Active,
            linked_by: "person:operator-007".to_owned(),
            linked_at: 1.0,
            status_comment: None,
            phase: None,
            snapshot: None,
        })
        .unwrap();
    served
        .registry
        .put_mission_review(&MissionReview::open(
            (uid.as_str(), "G", ReviewGate::Output),
            ("one", &ReviewPolicy::new(vec![PanelSeat::agent("r1")])),
            "007",
            ReviewMaterial::default(),
            1.0,
        ))
        .unwrap()
        .unwrap();
    let cable = Cable::try_new(
        CableDraft::new(
            "007".into(),
            "G".into(),
            "worker".into(),
            CableTarget::try_new(CableScope::Agent, Some("008".into())).unwrap(),
            "status".into(),
            vec![CableRecipient::try_new("008".into(), None, CableState::Queued).unwrap()],
            "key-1".into(),
            "cable-1".into(),
            1_700_000_000.0,
            1_700_000_900.0,
        )
        .with_mission(uid.clone()),
        &Default::default(),
    )
    .unwrap();
    served.registry.submit_cable(&cable).unwrap();
    uid
}

#[tokio::test]
async fn a_renamed_mission_reads_under_its_new_handle_its_old_one_and_its_uid() {
    let served = served().await;
    let uid = working_mission(&served).await;

    let renamed = rename(&served, &served.operator, "G-1", "G-build-fix").await;
    assert_eq!(renamed.status(), 200);
    let renamed: Value = renamed.json().await.unwrap();
    assert_eq!(
        renamed,
        json!({"operation_id": "G", "mission_id": uid, "handle": "G-build-fix", "previous": "G-1"})
    );

    for id in ["G-build-fix", "G-1", "g-1", uid.as_str()] {
        let detail = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            &format!("/v1/operations/G/missions/{id}"),
            None,
        )
        .await;
        assert_eq!(detail.status(), 200, "{id}");
        let detail: Value = detail.json().await.unwrap();
        assert_eq!(detail["mission_id"], uid.as_str(), "{id}");
        assert_eq!(detail["handle"], "G-build-fix", "{id}");
        assert_eq!(detail["sessions"][0]["session_id"], "session-r", "{id}");
        assert_eq!(
            detail["attempts"][0]["handle"], "G-1.1",
            "a run keeps the handle it opened with"
        );

        let issues: Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            &format!("/v1/user/operations/G/missions/{id}/issues"),
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(issues["issues"][0]["item"], "o/r#7", "{id}: {issues}");

        let reviews: Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            &format!("/v1/user/operations/G/missions/{id}/reviews"),
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(reviews.as_array().map(Vec::len), Some(1), "{id}: {reviews}");

        let listed: Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            &format!("/v1/missions?mission={id}"),
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(listed.as_array().map(Vec::len), Some(1), "{id}: {listed}");
        assert_eq!(listed[0]["handle"], "G-build-fix", "{id}");
    }
    let cable = served.registry.get_cable("cable-1").unwrap().unwrap();
    assert_eq!(cable.mission_id(), Some(uid.as_str()));

    let audit = served.registry.audit_records().unwrap();
    let record = audit
        .iter()
        .find(|record| record.action == "mission.rename")
        .expect("the rename is audited");
    assert_eq!(
        (record.actor.as_str(), record.operation.as_str()),
        ("operator-007", "G")
    );
    assert_eq!(record.target, uid);
    assert_eq!(
        record.resource.as_deref(),
        Some(format!("mission:{uid}").as_str())
    );

    let timeline: Value = call(
        &served,
        &served.viewer,
        reqwest::Method::GET,
        "/v1/user/operations/G/timeline?mission=G-1",
        None,
    )
    .await
    .json()
    .await
    .unwrap();
    let items = timeline["items"].as_array().unwrap();
    let event = items
        .iter()
        .find(|item| item["action"] == "rename.done")
        .expect("the rename is on the timeline");
    assert_eq!(event["detail"]["previous"], "G-1");
    assert_eq!(event["detail"]["handle"], "G-build-fix");
    assert!(
        items
            .iter()
            .all(|item| item["mission_id"] == uid.as_str() && item["handle"] == "G-build-fix"),
        "{timeline}"
    );
}

#[tokio::test]
async fn a_rename_is_refused_in_the_gateway_s_own_words() {
    let served = served().await;
    let uid = working_mission(&served).await;
    let other = served
        .registry
        .enter_brief(&brief("other"), &MissionEntry::person(Some("G")))
        .unwrap();

    let viewer = rename(&served, &served.viewer, &uid, "G-x").await;
    assert_eq!(viewer.status(), 403, "renaming needs mission.control");

    for (handle, status) in [("G-1", 409), ("g-1", 409), ("a b", 400), ("-x", 400)] {
        let refused = rename(&served, &served.operator, &other.key(), handle).await;
        assert_eq!(refused.status(), status, "{handle}");
        let body: Value = refused.json().await.unwrap();
        assert!(
            body["error"].as_str().is_some_and(|text| !text.is_empty()),
            "{handle}: {body}"
        );
    }

    let missing = rename(&served, &served.operator, "G-404", "G-y").await;
    assert_eq!(missing.status(), 404);
    // A Mission of another Operation is not this Operation's to rename.
    let elsewhere: Brief =
        serde_json::from_value(json!({"title": "l", "id": "l", "operation": "L"})).unwrap();
    let elsewhere = served
        .registry
        .enter_brief(&elsewhere, &MissionEntry::person(Some("L")))
        .unwrap();
    let foreign = rename(&served, &served.operator, &elsewhere.key(), "G-z").await;
    assert_eq!(foreign.status(), 404);
    assert!(
        served
            .registry
            .audit_records()
            .unwrap()
            .iter()
            .all(|record| record.action != "mission.rename"),
        "a refused rename is not audited as one"
    );
}
