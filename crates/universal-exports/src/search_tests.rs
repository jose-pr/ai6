//! Searching Missions through the gateway: found from the Registry alone, and only what the
//! person may read.

use std::sync::Arc;

use ai6::directorates::registry::backend::*;
use ai6::{
    Brief, ConfigProvider, ConfigRecord, ConfigScope, MissionEntry, SqlProvider, UnexLimits,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::controls::tests::{Served, served_with_roster, served_with_unex};

/// Viewer 007 reads Operation G and nothing of L; operator 007 reads both.
const ROLES: &str = r#"
    [role.viewer]
    actions = ["mission.read"]

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "viewer"

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "viewer"

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:L"
    role = "viewer"
"#;

fn mission(registry: &dyn RegistryBackend, operation: &str, title: &str, body: &str) -> Brief {
    let brief: Brief = serde_json::from_value(json!({
        "title": title, "body": body, "id": title, "operation": operation, "agent": "007",
    }))
    .unwrap();
    registry
        .enter_brief(&brief, &MissionEntry::person(Some(operation)))
        .unwrap()
}

async fn search(served: &Served, token: &str, query: &str) -> (StatusCode, Value) {
    let response = reqwest::Client::new()
        .get(format!("{}/v1/missions/search{query}", served.base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}

fn handles(page: &Value) -> Vec<String> {
    let mut found: Vec<String> = page["missions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|mission| mission["handle"].as_str().unwrap().to_owned())
        .collect();
    found.sort();
    found
}

/// Two Missions of G, one finished with an outcome, and one of L; nothing is published.
fn seed(served: &Served) -> (Brief, Brief, Brief) {
    let registry = &served.registry;
    let build = mission(registry, "G", "Repair the build", "The linker fails.");
    let guide = mission(registry, "G", "Write the guide", "Cover quota pools.");
    let secret = mission(
        registry,
        "L",
        "Repair the vault",
        "The linker fails here too.",
    );
    let claimed = registry.claim_new(Some(&build.key())).unwrap().unwrap();
    registry
        .finish(
            &claimed,
            &json!({"summary": "Pinned the toolchain."}),
            true,
            None,
            None,
        )
        .unwrap();
    (build, guide, secret)
}

#[tokio::test]
async fn a_mission_is_found_with_no_publication_and_another_operations_stays_hidden() {
    let served = served_with_roster(ROLES, None).await;
    let (_, _, secret) = seed(&served);

    let (status, page) = search(&served, &served.viewer, "?text=linker").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(handles(&page), ["G-1"], "{page}");
    assert_eq!(page["more"], false);
    assert_eq!(page["missions"][0]["operation_id"], "G");
    assert_eq!(
        page["missions"][0]["attempts"], 0,
        "no Station has claimed it"
    );

    // Naming the other Operation, its handle or its words reveals nothing either.
    for query in [
        "?operation=L".to_owned(),
        format!("?handle={}", secret.shown_handle()),
        "?text=vault".to_owned(),
        "?agent=007&state=new&text=repair".to_owned(),
    ] {
        let (status, page) = search(&served, &served.viewer, &query).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        let seen = handles(&page);
        assert!(
            seen.iter().all(|handle| handle.starts_with("G-")),
            "{query}: {seen:?}"
        );
    }
    let (_, page) = search(&served, &served.operator, "?text=linker").await;
    assert_eq!(handles(&page), ["G-1", "L-1"]);
}

#[tokio::test]
async fn a_search_reads_the_outcome_and_filters_on_state_handle_and_agent() {
    let served = served_with_roster(ROLES, None).await;
    let (build, guide, _) = seed(&served);
    served
        .registry
        .rename_mission(&guide.key(), "G-guide")
        .unwrap();

    let (_, page) = search(&served, &served.viewer, "?text=toolchain").await;
    assert_eq!(
        handles(&page),
        [build.shown_handle()],
        "the outcome is searched"
    );
    assert_eq!(page["missions"][0]["state"], "done");
    let (_, page) = search(&served, &served.viewer, "?state=new").await;
    assert_eq!(handles(&page), ["G-guide"]);
    // The former handle, written whole, still finds the renamed Mission.
    let (_, page) = search(
        &served,
        &served.viewer,
        &format!("?handle={}", guide.shown_handle()),
    )
    .await;
    assert_eq!(handles(&page), ["G-guide"]);
    let (_, page) = search(&served, &served.viewer, "?agent=007&handle=guide").await;
    assert_eq!(handles(&page), ["G-guide"]);
    let (_, page) = search(&served, &served.viewer, "?agent=008").await;
    assert!(handles(&page).is_empty());
}

#[tokio::test]
async fn a_search_refuses_what_is_malformed_or_over_its_settings() {
    let mut unex = UnexLimits::default();
    unex.search.page = 1;
    unex.search.max_page = ai6::Limit::at(2);
    unex.search.filter_bytes = ai6::Limit::at(8);
    let served = served_with_unex(ROLES, None, Some(unex)).await;
    seed(&served);
    mission(&served.registry, "G", "Third", "More.");

    let (status, page) = search(&served, &served.viewer, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        page["missions"].as_array().unwrap().len(),
        1,
        "the page default"
    );
    assert_eq!(page["more"], true);
    let (_, page) = search(&served, &served.viewer, "?limit=2").await;
    assert_eq!(page["missions"].as_array().unwrap().len(), 2);
    for query in [
        "?limit=3",
        "?limit=0",
        "?text=longer-than-eight",
        "?state=lost",
        "?operation=",
        "?nonsense=1",
    ] {
        let (status, _) = search(&served, &served.viewer, query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }
    let response = reqwest::Client::new()
        .get(format!("{}/v1/missions/search", served.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_compartment_covering_one_mission_shows_it_and_the_crossing_is_audited() {
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
    let (_, _, secret) = seed(&served);
    let other = mission(
        &served.registry,
        "L",
        "Repair the dock",
        "The linker fails there.",
    );

    let (_, page) = search(&served, &served.viewer, "?text=linker").await;
    assert_eq!(handles(&page), ["G-1"]);
    source
        .put(
            ConfigRecord::Compartment,
            &ConfigScope::Global,
            "LAPWING",
            &json!({
                "actions": ["mission.read"],
                "covers": [{"mission": secret.key()}],
                "read_in": [{"subject": "person:viewer-007"}],
            }),
        )
        .unwrap();
    let (_, page) = search(&served, &served.viewer, "?text=linker").await;
    assert_eq!(
        handles(&page),
        ["G-1", secret.shown_handle().as_str()],
        "{page}"
    );
    assert!(!handles(&page).contains(&other.shown_handle()));
    let crossed: Vec<_> = served
        .registry
        .list_audit(100)
        .unwrap()
        .into_iter()
        .filter(|record| record.action == "compartment.read")
        .collect();
    assert_eq!(crossed.len(), 1, "{crossed:?}");
    assert_eq!(crossed[0].actor, "viewer-007");
    assert_eq!(
        crossed[0].resource.as_deref(),
        Some(format!("mission:{}", secret.key()).as_str())
    );
}
