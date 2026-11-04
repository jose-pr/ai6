//! She reads an Operation's tracker items live: the gateway holds no tracker token, so the test
//! plays the Headquarters Station, which takes the read and answers it as a tracker that
//! records what it is asked.

use ai6::{Reply, SourceRead, SourceReadAnswer, SourceReadComment, SourceReadItem};

use super::*;

/// The read the gateway holds for the Headquarters Station, waiting up to `wait_ms`.
async fn next_read(served: &Served, wait_ms: u64) -> Option<SourceRead> {
    let response = reqwest::Client::new()
        .get(format!("{}/v1/station/source-reads/next", served.base))
        .query(&[("station", "hq"), ("wait_ms", &wait_ms.to_string())])
        .bearer_auth(STATION_TOKEN)
        .send()
        .await
        .unwrap();
    match response.status() {
        StatusCode::OK => Some(response.json().await.unwrap()),
        StatusCode::NO_CONTENT => None,
        other => panic!("next read: {other}"),
    }
}

/// What a tracker holding `item` with five comments answers, the latest `most` of them.
fn recorded(item: &str, most: u32) -> SourceReadAnswer {
    let comments: Vec<SourceReadComment> = (1..=5)
        .map(|n| {
            SourceReadComment::of(Reply {
                id: n.to_string(),
                item: item.to_owned(),
                author_id: format!("u{n}"),
                author: format!("User {n}"),
                permitted: n == 4,
                own: n == 5,
                body: format!("comment {n}: waiting on the vendor's key"),
                created: format!("2026-11-0{n}T00:00:00Z"),
            })
        })
        .collect();
    let skip = comments.len().saturating_sub(most as usize);
    SourceReadAnswer {
        item: Some(SourceReadItem {
            tracker: "gh".to_owned(),
            id: item.to_owned(),
            title: "Ship the export".to_owned(),
            state: Some("open".to_owned()),
            labels: vec!["blocked".to_owned()],
            assignees: vec!["ana".to_owned()],
            ..SourceReadItem::default()
        }),
        comments: comments.into_iter().skip(skip).collect(),
        ..SourceReadAnswer::default()
    }
}

#[tokio::test]
async fn she_reads_a_tracker_item_with_its_comments_through_headquarters() {
    let served = served(AssistantConfig::default()).await;
    let (_app, job) = accepted(&served, &served.operator, "What blocks o/r#7?").await;
    let asked = call_tool(
        &served,
        &job.mcp_token,
        "source_item",
        json!({"operation_id": "G", "tracker": "gh", "item": "o/r#7"}),
    );
    let headquarters = async {
        let read = next_read(&served, 10_000).await.expect("a read is waiting");
        assert_eq!(read.operation, "G");
        assert_eq!(read.tracker.as_deref(), Some("gh"));
        assert_eq!(read.item.as_deref(), Some("o/r#7"));
        assert_eq!(read.comments, 10, "[assistant.gateway] source_comments");
        let answered = reqwest::Client::new()
            .post(format!(
                "{}/v1/station/source-reads/{}/result",
                served.base, read.id
            ))
            .query(&[("station", "hq")])
            .bearer_auth(STATION_TOKEN)
            .json(&recorded("o/r#7", read.comments))
            .send()
            .await
            .unwrap();
        assert_eq!(answered.status(), StatusCode::NO_CONTENT);
    };
    let (result, ()) = tokio::join!(asked, headquarters);
    assert_eq!(result["isError"], false, "{result}");
    let text = said(&result);
    assert!(
        text.starts_with("<ai6-data tool=\"source_item\">"),
        "{text}"
    );
    for wanted in [
        "comment 1: waiting on the vendor",
        "comment 5: waiting on the vendor",
        "\"role\": \"answers\"",
        "\"role\": \"ai6\"",
        "\"assignees\"",
        "Ship the export",
    ] {
        assert!(text.contains(wanted), "{wanted}: {text}");
    }
}

#[tokio::test]
async fn a_person_without_mission_read_is_refused_and_nothing_is_asked() {
    let served = served(AssistantConfig::default()).await;
    let (_app, job) = accepted(&served, &served.member, "What blocks o/r#7?").await;
    for (tool, input) in [
        (
            "source_item",
            json!({"operation_id": "G", "tracker": "gh", "item": "o/r#7"}),
        ),
        ("source_items", json!({"operation_id": "G"})),
    ] {
        let result = call_tool(&served, &job.mcp_token, tool, input).await;
        assert_eq!(result["isError"], true, "{tool}: {result}");
        assert!(said(&result).contains("Refused"), "{tool}: {result}");
    }
    assert!(next_read(&served, 0).await.is_none());
}

#[tokio::test]
async fn a_list_reads_the_sources_and_a_silent_headquarters_is_said() {
    let served = served(tuned(|limits| limits.source_wait = ai6::Seconds::new(1))).await;
    let (_app, job) = accepted(&served, &served.operator, "What is open?").await;
    let asked = call_tool(
        &served,
        &job.mcp_token,
        "source_items",
        json!({"operation_id": "G", "query": "export"}),
    );
    let headquarters = async {
        let read = next_read(&served, 10_000).await.unwrap();
        assert_eq!(read.item, None);
        assert_eq!(read.query.as_deref(), Some("export"));
        let answer = SourceReadAnswer {
            items: vec![SourceReadItem {
                tracker: "gh".to_owned(),
                id: "o/r#7".to_owned(),
                title: "Ship the export".to_owned(),
                ..SourceReadItem::default()
            }],
            ..SourceReadAnswer::default()
        };
        reqwest::Client::new()
            .post(format!(
                "{}/v1/station/source-reads/{}/result",
                served.base, read.id
            ))
            .bearer_auth(STATION_TOKEN)
            .json(&answer)
            .send()
            .await
            .unwrap()
            .status()
    };
    let (result, status) = tokio::join!(asked, headquarters);
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(said(&result).contains("Ship the export"), "{result}");
    // Nobody takes the next one: she is told, and a late answer finds nobody waiting.
    let result = call_tool(
        &served,
        &job.mcp_token,
        "source_items",
        json!({"operation_id": "G"}),
    )
    .await;
    assert_eq!(result["isError"], true, "{result}");
    assert!(said(&result).contains("did not answer"), "{result}");
}
