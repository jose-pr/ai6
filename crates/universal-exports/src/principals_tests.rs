//! Naming the people who signed in: what is learned, who may ask, and what an audit record says.

use std::time::Duration;

use ai6::{AuditRecord, AuditStore, PrincipalStore};
use serde_json::{Value, json};

use crate::controls::tests::{Served, served, served_with};

async fn get(served: &Served, token: &str, path: &str) -> (u16, Value) {
    let response = reqwest::Client::new()
        .get(format!("{}{path}", served.base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    (
        response.status().as_u16(),
        response.json().await.unwrap_or(Value::Null),
    )
}

async fn post(served: &Served, token: &str, path: &str, body: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}{path}", served.base))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    (
        response.status().as_u16(),
        response.json().await.unwrap_or(Value::Null),
    )
}

/// Signs `token`'s person in as far as the gateway sees it, and waits for the sighting to be kept.
async fn seen(served: &Served, token: &str, id: &str) {
    get(served, token, "/v1/user/profile").await;
    for _ in 0..200 {
        if served.registry.principal(id).unwrap().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{id} was never noted");
}

fn audited(served: &Served, actor: &str, id: &str) {
    served
        .registry
        .record_audit(&AuditRecord {
            id: id.to_owned(),
            actor: actor.to_owned(),
            action: "mission.create".to_owned(),
            operation: "G".to_owned(),
            resource: None,
            target: "mission-g".to_owned(),
            at: 100.0,
        })
        .unwrap();
}

#[tokio::test]
async fn a_person_who_signed_in_is_named_on_the_audit_and_one_never_seen_is_not() {
    let served = served().await;
    seen(&served, &served.operator, "operator-007").await;
    let kept = served.registry.principal("operator-007").unwrap().unwrap();
    assert_eq!(kept.label(), "Ada Lovelace");
    assert_eq!(kept.name, "operator-007");

    audited(&served, "operator-007", "a1");
    audited(&served, "operator-007 via key k1", "a2");
    audited(&served, "S-1-5-21-1-2-3-1000", "a3");
    let (status, audit) = get(&served, &served.viewer, "/v1/user/audit?operation_id=G").await;
    assert_eq!(status, 200);
    let name_of = |id: &str| {
        audit
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["id"] == id)
            .map(|record| record["actor_name"].clone())
            .unwrap()
    };
    assert_eq!(name_of("a1"), "Ada Lovelace");
    assert_eq!(name_of("a2"), "Ada Lovelace", "a key acts as its person");
    assert_eq!(
        name_of("a3"),
        Value::Null,
        "never seen: the actor stays the ID"
    );
}

#[tokio::test]
async fn names_are_given_to_people_who_may_audit_and_to_everyone_for_themselves() {
    // The viewer audits Operation G, so the names of others widen nothing for them.
    let served = served().await;
    seen(&served, &served.operator, "operator-007").await;
    let (status, found) = post(
        &served,
        &served.viewer,
        "/v1/user/principals",
        json!({"ids": ["operator-007", "S-1-5-21-1-2-3-1000"]}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(found["people"].as_array().unwrap().len(), 1);
    assert_eq!(found["people"][0]["label"], "Ada Lovelace");
    assert_eq!(found["people"][0]["id"], "operator-007");

    // Someone who may neither audit nor read access is named to themself only.
    let served = served_with(
        r#"
        [role.reader]
        actions = ["mission.read"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "reader"
        "#,
    )
    .await;
    seen(&served, &served.operator, "operator-007").await;
    seen(&served, &served.viewer, "viewer-007").await;
    let (_, found) = post(
        &served,
        &served.viewer,
        "/v1/user/principals",
        json!({"ids": ["operator-007", "viewer-007"]}),
    )
    .await;
    let ids: Vec<&str> = found["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|person| person["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["viewer-007"]);
    let (status, _) = get(&served, &served.viewer, "/v1/user/principals?q=ada").await;
    assert_eq!(status, 403, "a search needs access.read");
}

#[tokio::test]
async fn the_picker_finds_a_person_and_a_group_by_name() {
    let served = served_with(
        r#"
        [role.admin]
        actions = ["access.read", "mission.read"]

        [[rule]]
        subject = "person:operator-007"
        scope = "*"
        role = "admin"
        "#,
    )
    .await;
    seen(&served, &served.operator, "operator-007").await;
    // A group the directory reported for someone, noted as the gateway would.
    served
        .registry
        .note_principal(
            &ai6::PrincipalRecord::new("person:1000", "grace", 5.0)
                .displayed_as("Grace Hopper")
                .in_groups(vec!["domain admins".to_owned()]),
        )
        .unwrap();
    let (status, found) = get(&served, &served.operator, "/v1/user/principals?q=hop").await;
    assert_eq!(status, 200);
    assert_eq!(found["people"][0]["label"], "Grace Hopper");
    assert_eq!(found["people"].as_array().unwrap().len(), 1);
    assert_eq!(found["groups"], json!([]));

    let (_, found) = get(&served, &served.operator, "/v1/user/principals?q=domain").await;
    assert_eq!(found["groups"], json!(["domain admins"]));
    assert_eq!(found["people"], json!([]));

    // Words are all required, in any order, against the name, the account name and the ID.
    let (_, found) = get(
        &served,
        &served.operator,
        "/v1/user/principals?q=1000%20grace",
    )
    .await;
    assert_eq!(found["people"][0]["id"], "person:1000");
    let (_, found) = get(&served, &served.operator, "/v1/user/principals?q=lovelace").await;
    assert_eq!(found["people"][0]["id"], "operator-007");
}
