//! Sharing a page: who may be named, who then opens it, and with whose permissions its feeds
//! run.

use ai6::{ConfigProvider, ConfigRecord, ConfigScope, SqlProvider};

use super::*;

/// A roster in SQLite holding Operations G and H, so a share can name what the sharer may and
/// may not read, and `pages` as `[assistant.pages]`.
fn sql_roster(directory: &tempfile::TempDir, pages: Value) -> Arc<SqlProvider> {
    let location = format!("sqlite:{}", directory.path().join("roster.db").display());
    let source = Arc::new(SqlProvider::open(&location).unwrap());
    for operation in ["G", "H"] {
        source
            .put(
                ConfigRecord::Operation,
                &ConfigScope::Global,
                operation,
                &json!({}),
            )
            .unwrap();
    }
    put_pages(&source, pages);
    source
}

fn put_pages(source: &SqlProvider, pages: Value) {
    source
        .put(
            ConfigRecord::Assistant,
            &ConfigScope::Global,
            "settings",
            &json!({"pages": pages}),
        )
        .unwrap();
}

/// Compartment `id` covering Operation G, reading in `subject`.
fn compartment(source: &SqlProvider, id: &str, subject: &str) {
    source
        .put(
            ConfigRecord::Compartment,
            &ConfigScope::Global,
            id,
            &json!({
                "actions": ["mission.read"],
                "covers": [{"operation": "G"}],
                "read_in": [{"subject": subject}],
            }),
        )
        .unwrap();
}

async fn set_shares(
    served: &Served,
    bearer: &str,
    id: &str,
    subjects: &[&str],
) -> reqwest::Response {
    reqwest::Client::new()
        .put(format!("{}/v1/user/pages/{id}/shares", served.base))
        .bearer_auth(bearer)
        .json(&json!({"subjects": subjects}))
        .send()
        .await
        .unwrap()
}

async fn status_of(url: String) -> StatusCode {
    plain().get(url).send().await.unwrap().status()
}

/// A dashboard of the operator's whose feed `missions` lists the Missions its viewer may read.
async fn missions_dashboard(served: &Served) -> String {
    let (_app, job) = accepted(served, &served.operator, "Share this.").await;
    published_id(
        &publish(
            served,
            &job.mcp_token,
            json!({"kind": "dashboard", "title": "Board", "content": "<p>x</p>", "format": "html",
                   "feeds": [{"name": "missions", "tool": "missions", "input": {}}]}),
        )
        .await,
    )
}

async fn feed(url: &str) -> Value {
    let response = plain()
        .get(format!("{url}data/missions"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json::<Value>().await.unwrap()
}

#[tokio::test]
async fn pages_a_member_of_a_shared_group_opens_it_with_their_own_data_until_it_is_unshared() {
    let served = served(AssistantConfig::default()).await;
    let id = missions_dashboard(&served).await;
    // Nobody but its person opens an unshared page: a stranger, a group member, an
    // administrator.
    for bearer in [&served.viewer, &served.member, &served.admin] {
        assert_eq!(
            open_page(&served, bearer, &id, None).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    let shared = set_shares(&served, &served.operator, &id, &["group:ops", "group:ops"]).await;
    assert_eq!(shared.status(), StatusCode::OK);
    assert_eq!(
        shared.json::<Value>().await.unwrap()["shares"],
        json!(["group:ops"])
    );
    let audited = audits(&served, "moneypenny.page.shared");
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].actor, "operator-007");
    assert_eq!(audited[0].target, format!("{id} group:ops"));

    // The member opens it, and its feed reads with the member's permissions, not the author's.
    let own = open_frame(&frame_url(&served, &served.operator, &id, None).await).await;
    let theirs = open_frame(&frame_url(&served, &served.member, &id, None).await).await;
    let text = |value: &Value| value.to_string();
    assert!(text(&feed(&own.view).await).contains("mission-g"));
    assert!(!text(&feed(&theirs.view).await).contains("mission-g"));
    let bar = theirs.reload().await.text().await.unwrap();
    assert!(bar.contains("Shared by operator-007."), "{bar}");
    // It is listed to the member as shared, naming whose it is and not whom else it reaches.
    let listed: Value = reqwest::Client::new()
        .get(format!("{}/v1/user/pages", served.base))
        .bearer_auth(&served.member)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed[0]["id"], id.as_str());
    assert_eq!(listed[0]["shared_by"], "operator-007");
    assert!(listed[0].get("shares").is_none(), "{listed}");
    // Still nobody else, administrator included.
    for bearer in [&served.viewer, &served.admin] {
        assert_eq!(
            open_page(&served, bearer, &id, None).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    // Only its person shares it, keeps it or deletes it.
    assert_eq!(
        set_shares(&served, &served.member, &id, &["group:ops"])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // Unsharing ends the member's link at once; the person's own stays.
    assert_eq!(
        set_shares(&served, &served.operator, &id, &[])
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        status_of(format!("{}f/index.html", theirs.view)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(theirs.reload().await.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        status_of(format!("{}f/index.html", own.view)).await,
        StatusCode::OK
    );
    assert_eq!(own.reload().await.status(), StatusCode::OK);
    assert_eq!(
        open_page(&served, &served.member, &id, None).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        audits(&served, "moneypenny.page.shared")[0].target,
        format!("{id} nobody")
    );
}

#[tokio::test]
async fn pages_a_share_names_only_an_operation_or_compartment_the_sharer_reads() {
    let directory = tempfile::tempdir().unwrap();
    let source = sql_roster(&directory, json!({}));
    compartment(&source, "KESTREL", "person:viewer-007");
    let served = served_with(AssistantConfig::default(), Some(source.clone())).await;
    let id = missions_dashboard(&served).await;
    for (subject, status) in [
        ("operation:H", StatusCode::FORBIDDEN),
        ("compartment:KESTREL", StatusCode::FORBIDDEN),
        ("compartment:NOSUCH", StatusCode::FORBIDDEN),
        ("nobody", StatusCode::BAD_REQUEST),
        ("agent:007", StatusCode::BAD_REQUEST),
        ("person:", StatusCode::BAD_REQUEST),
    ] {
        let refused = set_shares(&served, &served.operator, &id, &[subject]).await;
        assert_eq!(refused.status(), status, "{subject}");
        assert!(refused.json::<Value>().await.unwrap()["error"].is_string());
    }
    assert!(audits(&served, "moneypenny.page.shared").is_empty());

    // An Operation the sharer reads lets in whoever holds mission.read there.
    assert_eq!(
        set_shares(&served, &served.operator, &id, &["operation:G"])
            .await
            .status(),
        StatusCode::OK
    );
    let viewer = open_frame(&frame_url(&served, &served.viewer, &id, None).await).await;
    assert_eq!(viewer.reload().await.status(), StatusCode::OK);
    assert_eq!(
        open_page(&served, &served.member, &id, None).await.status(),
        StatusCode::NOT_FOUND
    );
    // A compartment the sharer is read into lets in the people read into it.
    compartment(&source, "OSPREY", "group:ops");
    assert_eq!(
        set_shares(&served, &served.operator, &id, &["compartment:OSPREY"])
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        viewer.reload().await.status(),
        StatusCode::NOT_FOUND,
        "no longer named"
    );
    let member = open_frame(&frame_url(&served, &served.member, &id, None).await).await;
    assert_eq!(member.reload().await.status(), StatusCode::OK);
    // Reading the member out ends their view on the next request.
    source
        .remove(ConfigRecord::Compartment, &ConfigScope::Global, "OSPREY")
        .unwrap();
    assert_eq!(member.reload().await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pages_with_sharing_off_a_share_is_refused_and_grants_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let source = sql_roster(&directory, json!({"max_shares": 1}));
    let served = served_with(AssistantConfig::default(), Some(source.clone())).await;
    let id = missions_dashboard(&served).await;
    let many = set_shares(
        &served,
        &served.operator,
        &id,
        &["group:ops", "person:viewer-007"],
    )
    .await;
    assert_eq!(many.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        set_shares(&served, &served.operator, &id, &["group:ops"])
            .await
            .status(),
        StatusCode::OK
    );
    let member = open_frame(&frame_url(&served, &served.member, &id, None).await).await;
    assert_eq!(member.reload().await.status(), StatusCode::OK);

    put_pages(&source, json!({"sharing": false}));
    assert_eq!(member.reload().await.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        open_page(&served, &served.member, &id, None).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        set_shares(&served, &served.operator, &id, &["group:ops"])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // Its person still opens it.
    let own = open_frame(&frame_url(&served, &served.operator, &id, None).await).await;
    assert_eq!(own.reload().await.status(), StatusCode::OK);
}
