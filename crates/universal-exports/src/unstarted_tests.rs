//! Missions with no Run yet are listed with where they stand, for a person who may read
//! their Operation and nobody else.

use std::collections::BTreeMap;

use ai6::directorates::registry::backend::*;
use ai6::{Brief, TimelineEvent};
use serde_json::{Value, json};

use crate::controls::tests::served;

async fn get(base: &str, token: &str, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{base}{path}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
}

fn entry<'a>(missions: &'a Value, id: &str) -> &'a Value {
    missions
        .as_array()
        .unwrap()
        .iter()
        .find(|mission| mission["mission_id"] == id)
        .unwrap_or_else(|| panic!("{id} is not listed: {missions}"))
}

#[tokio::test]
async fn queued_waiting_retrying_and_failed_missions_are_listed_with_their_reasons() {
    let served = served().await;
    served
        .registry
        .check_in(
            "worker",
            BTreeMap::from([
                ("operations".to_owned(), json!(["G", "L"])),
                (
                    "queue".to_owned(),
                    json!([
                        {"mission_id": "m-retry", "operation_id": "G", "agent": "009",
                         "state": "queued", "failures": 2, "retry_in_secs": 160.0,
                         "waiting": "credentials could not be leased"},
                        {"mission_id": "m-wait", "operation_id": "G", "state": "queued",
                         "waiting": "its Agent is retired"},
                        {"mission_id": "m-plain", "operation_id": "G", "state": "queued"},
                        {"mission_id": "m-hidden", "operation_id": "L", "state": "queued"},
                    ]),
                ),
                (
                    "failed_starts".to_owned(),
                    json!([
                        {"mission_id": "m-failed", "operation_id": "G", "agent": "007",
                         "reason": "no PATH"},
                        {"mission_id": "m-hidden-failed", "operation_id": "L", "reason": "x"},
                    ]),
                ),
            ]),
        )
        .unwrap();
    let brief: Brief =
        serde_json::from_value(json!({"title": "Not taken", "id": "not-taken", "operation": "G"}))
            .unwrap();
    served.registry.submit_brief(&brief).unwrap();
    let silent: Brief = serde_json::from_value(
        json!({"title": "Never noted", "id": "never-noted", "operation": "G"}),
    )
    .unwrap();
    served.registry.submit_brief(&silent).unwrap();
    served
        .registry
        .record_event(
            &TimelineEvent::new("account", "waiting", "G", 1_000.0)
                .detail("brief", brief.key())
                .detail("reason", "every place that fits is taken"),
        )
        .unwrap();

    let listed = get(&served.base, &served.viewer, "/v1/missions").await;
    assert_eq!(listed.status(), 200);
    let missions: Value = listed.json().await.unwrap();

    let retry = entry(&missions, "m-retry");
    assert_eq!(retry["runs"], json!([]));
    assert_eq!(retry["pending"]["state"], "queued");
    assert_eq!(retry["pending"]["kind"], "retrying");
    assert_eq!(retry["pending"]["failures"], 2);
    assert_eq!(retry["pending"]["agent"], "009");
    assert_eq!(retry["pending"]["station"], "worker");
    assert_eq!(
        retry["pending"]["reason"],
        "credentials could not be leased"
    );
    let seen = retry["pending"]["at"].as_f64().unwrap();
    assert_eq!(retry["pending"]["retry_at"].as_f64().unwrap(), seen + 160.0);

    assert_eq!(entry(&missions, "m-wait")["pending"]["kind"], "waiting");
    assert_eq!(entry(&missions, "m-plain")["pending"]["kind"], "queued");
    let failed = entry(&missions, "m-failed");
    assert_eq!(failed["pending"]["state"], "failed");
    assert_eq!(failed["pending"]["reason"], "no PATH");

    let unplaced = entry(&missions, &brief.key());
    assert_eq!(unplaced["pending"]["kind"], "waiting");
    assert_eq!(unplaced["pending"]["state"], "queued");
    assert_eq!(
        unplaced["pending"]["reason"],
        "every place that fits is taken"
    );
    assert_eq!(unplaced["pending"]["name"], "Not taken");
    assert!(unplaced["pending"]["station"].is_null());
    assert_eq!(
        entry(&missions, &silent.key())["pending"]["kind"],
        "unplaced"
    );

    // A Mission with a Run is listed by it, with nothing pending.
    assert!(entry(&missions, "mission-g").get("pending").is_none());
    // Another Operation's Missions are not theirs to read.
    let ids: Vec<&str> = missions
        .as_array()
        .unwrap()
        .iter()
        .map(|mission| mission["mission_id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&"m-hidden") && !ids.contains(&"m-hidden-failed"));
    assert!(!ids.contains(&"mission-l"));

    // The filters apply to them as to the rest.
    let one: Value = get(
        &served.base,
        &served.viewer,
        "/v1/missions?mission=m-failed",
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(one.as_array().unwrap().len(), 1);
    let other: Value = get(&served.base, &served.viewer, "/v1/missions?operation=L")
        .await
        .json()
        .await
        .unwrap();
    assert!(other.as_array().unwrap().is_empty());

    // Its detail answers, with no attempts.
    let detail = get(
        &served.base,
        &served.viewer,
        "/v1/operations/G/missions/m-retry",
    )
    .await;
    assert_eq!(detail.status(), 200);
    let detail: Value = detail.json().await.unwrap();
    assert_eq!(detail["pending"]["kind"], "retrying");
    assert_eq!(detail["attempts"], json!([]));
    let hidden = get(
        &served.base,
        &served.viewer,
        "/v1/operations/L/missions/m-hidden",
    )
    .await;
    assert_eq!(hidden.status(), 404);
}

#[tokio::test]
async fn a_mission_that_started_is_not_listed_twice() {
    let served = served().await;
    served
        .registry
        .check_in(
            "worker",
            BTreeMap::from([
                ("operations".to_owned(), json!(["G", "L"])),
                (
                    "queue".to_owned(),
                    json!([{"mission_id": "mission-g", "operation_id": "G", "state": "queued"}]),
                ),
            ]),
        )
        .unwrap();
    let missions: Value = get(&served.base, &served.operator, "/v1/missions")
        .await
        .json()
        .await
        .unwrap();
    let listed = missions
        .as_array()
        .unwrap()
        .iter()
        .filter(|mission| mission["mission_id"] == "mission-g")
        .count();
    assert_eq!(listed, 1);
    assert!(entry(&missions, "mission-g").get("pending").is_none());
}
