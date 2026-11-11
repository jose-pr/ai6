use std::sync::Arc;

use ai6::directorates::registry::backend::*;
use ai6::{ConfigProvider, ConfigRecord, ConfigScope, SqlProvider};
use axum::http::StatusCode;
use serde_json::json;

use crate::controls::tests::{Served, served_with_roster};

const ROLES: &str = r#"
    [role.approvers]
    actions = ["plugin.manage", "station.read", "audit.read"]

    [[rule]]
    subject = "person:operator-007"
    scope = "config:plugin"
    role = "approvers"

    [[rule]]
    subject = "person:operator-007"
    scope = "station:worker"
    role = "approvers"

    [role.family-managers]
    actions = ["station.read", "config.read", "config.manage"]

    [[rule]]
    subject = "person:viewer-007"
    scope = "station:worker"
    role = "family-managers"

    [[rule]]
    subject = "person:viewer-007"
    scope = "config:plugin"
    role = "family-managers"
"#;

async fn call(
    served: &Served,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> reqwest::Response {
    let request = reqwest::Client::new()
        .request(method, format!("{}/v1/user/{path}", served.base))
        .bearer_auth(token);
    match body {
        Some(body) => request.json(&body),
        None => request,
    }
    .send()
    .await
    .unwrap()
}

fn audits(served: &Served, action: &str) -> Vec<ai6::AuditRecord> {
    served
        .registry
        .list_audit(100)
        .unwrap()
        .into_iter()
        .filter(|record| record.action == action)
        .collect()
}

fn found(file: &str, digest: &str, publisher: Option<&str>) -> serde_json::Value {
    let mut library = json!({
        "file": file, "digest": digest, "kinds": ["ai6_agent_handler"],
        "verdict": "unpinned", "signed": publisher.is_some()
    });
    if let Some(publisher) = publisher {
        library["publisher"] = json!(publisher);
    }
    library
}

#[tokio::test]
async fn a_library_a_station_found_is_approved_by_whoever_holds_plugin_manage_and_audited() {
    let signed_digest = format!("sha256:{}", "ab".repeat(32));
    let plain_digest = format!("sha256:{}", "cd".repeat(32));
    let old_digest = format!("sha256:{}", "ef".repeat(32));
    let key = format!("ed25519:{}", "12".repeat(32));
    let directory = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", directory.path().join("roster.db").display());
    let source = Arc::new(SqlProvider::open(&location).unwrap());
    let served = served_with_roster(ROLES, Some(source.clone())).await;
    served
        .registry
        .check_in(
            "worker",
            std::collections::BTreeMap::from([(
                "found_libraries".to_owned(),
                json!([
                    found("ai6_signed.dll", &signed_digest, Some(&key)),
                    found("ai6_plain.dll", &plain_digest, None),
                ]),
            )]),
        )
        .unwrap();
    let stored = |file: &str| {
        source
            .definitions(
                ConfigRecord::Plugin,
                &ConfigScope::Station("worker".to_owned()),
            )
            .unwrap()
            .records
            .into_iter()
            .find(|(key, _)| key == file)
            .map(|(_, definition)| definition)
    };
    let approve = |token: String, body: serde_json::Value| {
        let served = &served;
        async move {
            call(
                served,
                &token,
                reqwest::Method::POST,
                "stations/worker/libraries/approve",
                Some(body),
            )
            .await
        }
    };
    let plain = |confirm: bool| json!({"file": "ai6_plain.dll", "digest": plain_digest, "trust": "pin", "confirm": confirm});

    // Whoever may read the Station sees what it found.
    let listed: serde_json::Value = call(
        &served,
        &served.viewer,
        reqwest::Method::GET,
        "stations/worker/libraries",
        None,
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(listed["libraries"][0]["publisher"], key);
    assert_eq!(listed["libraries"][1]["digest"], plain_digest);

    // config.manage on the family is not plugin.manage: nothing is written.
    let refused = approve(served.viewer.clone(), plain(true)).await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(stored("ai6_plain.dll"), None);

    // Not confirmed: what would load, and nothing stored or audited as approved.
    let held: serde_json::Value = approve(served.operator.clone(), plain(false))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(held["applied"], false);
    assert!(
        held["changes"][0].as_str().unwrap().contains(&plain_digest),
        "{held}"
    );
    assert_eq!(stored("ai6_plain.dll"), None);
    assert!(audits(&served, "plugin.approve").is_empty());

    // Confirmed: the Station's own record pins the reported bytes, and it is audited.
    let done = approve(served.operator.clone(), plain(true)).await;
    assert_eq!(done.status(), StatusCode::OK);
    assert_eq!(stored("ai6_plain.dll"), Some(json!({"pin": plain_digest})));
    let global = source
        .definitions(ConfigRecord::Plugin, &ConfigScope::Global)
        .unwrap();
    assert!(global.records.is_empty(), "a Station's approval is its own");
    let approved = audits(&served, "plugin.approve");
    assert_eq!(approved[0].actor, "operator-007");
    assert_eq!(approved[0].resource.as_deref(), Some("config:plugin"));
    assert!(
        approved[0].target.contains("ai6_plain.dll") && approved[0].target.contains(&plain_digest),
        "{}",
        approved[0].target
    );

    // A digest or file the Station did not report is refused and audited, and so is a
    // publisher for a library nobody known signed.
    for body in [
        json!({"file": "ai6_plain.dll", "digest": old_digest, "trust": "pin", "confirm": true}),
        json!({"file": "ai6_other.dll", "digest": plain_digest, "trust": "pin", "confirm": true}),
        json!({"file": "ai6_plain.dll", "digest": plain_digest, "trust": "publisher", "confirm": true}),
    ] {
        let refused = approve(served.operator.clone(), body).await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(audits(&served, "plugin.refused").len(), 3);
    assert_eq!(stored("ai6_other.dll"), None);

    // A signed library can be trusted by its publisher, which the held answer calls out.
    let signed = |confirm: bool| json!({"file": "ai6_signed.dll", "digest": signed_digest, "trust": "publisher", "confirm": confirm});
    let held: serde_json::Value = approve(served.operator.clone(), signed(false))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        held["weakened"][0].as_str().unwrap().contains(&key),
        "{held}"
    );
    assert_eq!(stored("ai6_signed.dll"), None);
    let done = approve(served.operator.clone(), signed(true)).await;
    assert_eq!(done.status(), StatusCode::OK);
    assert_eq!(stored("ai6_signed.dll"), Some(json!({"publisher": key})));
    assert!(
        audits(&served, "plugin.approve")
            .iter()
            .any(|record| record.target.contains(&key))
    );

    // A rebuild under a record that pinned other bytes replaces that pin, and says so.
    source
        .put(
            ConfigRecord::Plugin,
            &ConfigScope::Station("worker".to_owned()),
            "ai6_plain.dll",
            &json!({"pin": old_digest}),
        )
        .unwrap();
    let held: serde_json::Value = approve(served.operator.clone(), plain(false))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        held["changes"][1].as_str().unwrap().contains(&old_digest),
        "{held}"
    );
}
