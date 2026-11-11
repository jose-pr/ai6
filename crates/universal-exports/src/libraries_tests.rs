use std::sync::Arc;

use ai6::directorates::registry::backend::*;
use ai6::{ConfigProvider, ConfigRecord, ConfigScope, SqlProvider};
use axum::http::StatusCode;
use serde_json::json;

use crate::controls::tests::{Served, served_with_roster};

/// operator-007 holds `plugin.manage` and `config.manage` on the family; viewer-007 holds
/// `config.manage` on it and not `plugin.manage`, as a controller does.
const ROLES: &str = r#"
    [role.approvers]
    actions = ["plugin.manage", "config.read", "config.manage", "station.read", "audit.read"]

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

fn source() -> (tempfile::TempDir, Arc<SqlProvider>) {
    let directory = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", directory.path().join("roster.db").display());
    let source = Arc::new(SqlProvider::open(&location).unwrap());
    (directory, source)
}

fn station() -> ConfigScope {
    ConfigScope::Station("worker".to_owned())
}

#[tokio::test]
async fn writing_the_plugin_family_by_any_route_takes_plugin_manage_as_well() {
    let (_directory, source) = source();
    let served = served_with_roster(ROLES, Some(source.clone())).await;
    let pin = json!({"pin": format!("sha256:{}", "ab".repeat(32))});
    let apply = json!({"records": [{"kind": "plugins", "key": "c.dll", "definition": pin}]});

    // Holding config.manage on the family is not enough, global or for one Station.
    for (method, path, body) in [
        (
            reqwest::Method::PUT,
            "roster/plugins/a.dll",
            Some(pin.clone()),
        ),
        (
            reqwest::Method::PUT,
            "roster/plugins/a.dll?station=worker",
            Some(pin.clone()),
        ),
        (reqwest::Method::DELETE, "roster/plugins/a.dll", None),
        (
            reqwest::Method::POST,
            "resources/apply",
            Some(apply.clone()),
        ),
    ] {
        let refused = call(&served, &served.viewer, method.clone(), path, body.clone()).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }
    assert_eq!(audits(&served, "plugin.refused").len(), 4);
    assert!(
        source
            .definitions(ConfigRecord::Plugin, &ConfigScope::Global)
            .unwrap()
            .records
            .is_empty()
    );

    // The app is told the family is read-only to such a person.
    let resources: serde_json::Value = call(
        &served,
        &served.viewer,
        reqwest::Method::GET,
        "resources",
        None,
    )
    .await
    .json()
    .await
    .unwrap();
    let plugin = resources
        .as_array()
        .unwrap()
        .iter()
        .find(|kind| kind["kind"] == "plugin")
        .unwrap();
    assert_eq!(plugin["allowed"], json!(["config.read"]));

    // With both, each route writes.
    for (method, path, body) in [
        (
            reqwest::Method::PUT,
            "roster/plugins/a.dll",
            Some(pin.clone()),
        ),
        (
            reqwest::Method::PUT,
            "roster/plugins/b.dll?station=worker",
            Some(pin.clone()),
        ),
        (reqwest::Method::POST, "resources/apply", Some(apply)),
        (reqwest::Method::DELETE, "roster/plugins/a.dll", None),
    ] {
        let done = call(&served, &served.operator, method.clone(), path, body).await;
        assert_eq!(done.status(), StatusCode::OK, "{method} {path}");
    }
}

#[tokio::test]
async fn a_library_a_station_found_is_approved_by_whoever_holds_plugin_manage_and_audited() {
    let signed_digest = format!("sha256:{}", "ab".repeat(32));
    let plain_digest = format!("sha256:{}", "cd".repeat(32));
    let old_digest = format!("sha256:{}", "ef".repeat(32));
    let key = format!("ed25519:{}", "12".repeat(32));
    let stranger = format!("ed25519:{}", "34".repeat(32));
    let (_directory, source) = source();
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
                    found("ai6_unnamed.dll", &plain_digest, Some(&stranger)),
                    // Entries that are not well formed are not shown or acted on.
                    found("ai6_bad.dll", "sha256:AB", None),
                    found("ai6\u{202e}lld.exe", &plain_digest, None),
                    found("../ai6_up.dll", &plain_digest, None),
                ]),
            )]),
        )
        .unwrap();
    let stored = |file: &str| {
        source
            .definitions(ConfigRecord::Plugin, &station())
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
    let plain = |confirm: bool, revision: Option<&str>| {
        let mut body = json!({
            "file": "ai6_plain.dll", "digest": plain_digest, "trust": "pin", "confirm": confirm
        });
        if let Some(revision) = revision {
            body["record_revision"] = json!(revision);
        }
        body
    };

    // Whoever may read the Station sees what it claims, the well-formed entries only.
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
    let files: Vec<&str> = listed["libraries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|library| library["file"].as_str().unwrap())
        .collect();
    assert_eq!(
        files,
        ["ai6_signed.dll", "ai6_plain.dll", "ai6_unnamed.dll"]
    );

    // config.manage on the family is not plugin.manage: nothing is written, and it is audited.
    let refused = approve(served.viewer.clone(), plain(true, Some("0"))).await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(stored("ai6_plain.dll"), None);
    let refusals = audits(&served, "plugin.refused");
    assert_eq!(refusals.len(), 1);
    assert!(refusals[0].target.contains("ai6_plain.dll"));

    // Not confirmed: what would load, and nothing stored or audited as approved.
    let held: serde_json::Value = approve(served.operator.clone(), plain(false, None))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(held["applied"], false);
    assert!(
        held["changes"][0].as_str().unwrap().contains(&plain_digest),
        "{held}"
    );
    let revision = held["record_revision"].as_str().unwrap().to_owned();
    assert_eq!(stored("ai6_plain.dll"), None);
    assert!(audits(&served, "plugin.approve").is_empty());

    // A confirmation that does not carry the preview's revision is not taken.
    let bare = approve(served.operator.clone(), plain(true, None)).await;
    assert_eq!(bare.status(), StatusCode::BAD_REQUEST);

    // A record changed since the preview is a conflict, and nothing is stored over it.
    source
        .put(
            ConfigRecord::Plugin,
            &station(),
            "ai6_plain.dll",
            &json!({"pin": old_digest}),
        )
        .unwrap();
    let stale = approve(served.operator.clone(), plain(true, Some(&revision))).await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(stored("ai6_plain.dll"), Some(json!({"pin": old_digest})));
    source
        .remove(ConfigRecord::Plugin, &station(), "ai6_plain.dll")
        .unwrap();

    // With a global record for the file and none of the Station's own, only the approved pin is
    // written, and the preview says the global record is set aside on this Station.
    source
        .put(
            ConfigRecord::Plugin,
            &ConfigScope::Global,
            "ai6_plain.dll",
            &json!({"pin": old_digest, "publisher": key}),
        )
        .unwrap();
    let held: serde_json::Value = approve(served.operator.clone(), plain(false, None))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        held["changes"].as_array().unwrap().iter().any(|line| line
            .as_str()
            .unwrap()
            .contains("replaces the global record")),
        "{held}"
    );
    let revision = held["record_revision"].as_str().unwrap();
    let done = approve(served.operator.clone(), plain(true, Some(revision))).await;
    assert_eq!(done.status(), StatusCode::OK);
    assert_eq!(stored("ai6_plain.dll"), Some(json!({"pin": plain_digest})));
    let approved = audits(&served, "plugin.approve");
    assert_eq!(approved[0].actor, "operator-007");
    assert_eq!(approved[0].resource.as_deref(), Some("config:plugin"));
    assert!(
        approved[0].target.contains("ai6_plain.dll") && approved[0].target.contains(&plain_digest),
        "{}",
        approved[0].target
    );

    // A digest or file the Station did not report is refused and audited.
    for body in [
        json!({"file": "ai6_plain.dll", "digest": old_digest, "trust": "pin", "confirm": false}),
        json!({"file": "ai6_other.dll", "digest": plain_digest, "trust": "pin", "confirm": false}),
        json!({"file": "ai6_bad.dll", "digest": "sha256:AB", "trust": "pin", "confirm": false}),
        json!({"file": "ai6_plain.dll", "digest": plain_digest, "trust": "publisher", "confirm": false}),
    ] {
        let refused = approve(served.operator.clone(), body).await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(audits(&served, "plugin.refused").len(), 5);

    // A publisher no record names is not one the gateway can vouch for, whatever is reported.
    let signed = |confirm: bool, revision: Option<&str>, publisher: Option<&str>| {
        let mut body = json!({
            "file": "ai6_signed.dll", "digest": signed_digest, "trust": "publisher",
            "confirm": confirm
        });
        if let Some(revision) = revision {
            body["record_revision"] = json!(revision);
        }
        if let Some(publisher) = publisher {
            body["publisher"] = json!(publisher);
        }
        body
    };
    let unnamed = approve(
        served.operator.clone(),
        json!({"file": "ai6_unnamed.dll", "digest": plain_digest, "trust": "publisher"}),
    )
    .await;
    assert_eq!(unnamed.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(unnamed.text().await.unwrap().contains("cannot vouch"));
    assert_eq!(stored("ai6_unnamed.dll"), None);

    // Named by a record for another file, it can be extended to this one, and the preview says so.
    source
        .put(
            ConfigRecord::Plugin,
            &ConfigScope::Global,
            "ai6_base.dll",
            &json!({"publisher": key}),
        )
        .unwrap();
    let held: serde_json::Value = approve(served.operator.clone(), signed(false, None, None))
        .await
        .json()
        .await
        .unwrap();
    let changes = held["changes"].as_array().unwrap();
    assert!(
        changes.iter().any(|line| line
            .as_str()
            .unwrap()
            .contains("already trusted for ai6_base.dll")),
        "{held}"
    );
    assert!(
        held["weakened"][0].as_str().unwrap().contains(&key),
        "{held}"
    );
    assert_eq!(held["publisher"], key);
    let revision = held["record_revision"].as_str().unwrap().to_owned();
    assert_eq!(stored("ai6_signed.dll"), None);

    // The confirmation names the key previewed; another is refused, as is a missing one.
    let other = approve(
        served.operator.clone(),
        signed(true, Some(&revision), Some(&stranger)),
    )
    .await;
    assert_eq!(other.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let missing = approve(served.operator.clone(), signed(true, Some(&revision), None)).await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    assert_eq!(stored("ai6_signed.dll"), None);
    let done = approve(
        served.operator.clone(),
        signed(true, Some(&revision), Some(&key)),
    )
    .await;
    assert_eq!(done.status(), StatusCode::OK);
    assert_eq!(stored("ai6_signed.dll"), Some(json!({"publisher": key})));
    assert!(
        audits(&served, "plugin.approve")
            .iter()
            .any(|record| record.target.contains(&key))
    );
}
