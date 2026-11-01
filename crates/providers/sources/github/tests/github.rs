//! Drives the provider against a loopback server that answers with GitHub's own responses,
//! recorded from a real repository with `tests/live.rs` and scrubbed. Nothing here reaches
//! GitHub.

use std::sync::Once;

use ai6::{CxReport, Source, SourcePoll};
use ai6_source_github::Github;

mod support;
use support::{Canned, canned, serve};

const PAGE1: &str = include_str!("fixtures/issues_page1.json");
const PAGE2: &str = include_str!("fixtures/issues_page2.json");
const PAGE1_LINK: &str = include_str!("fixtures/issues_page1.link");
const COMMENT_CREATED: &str = include_str!("fixtures/comment_created.json");
const LABELS_ADDED: &str = include_str!("fixtures/labels_added.json");
const BAD_CREDENTIALS: &str = include_str!("fixtures/bad_credentials.json");
const NOT_FOUND: &str = include_str!("fixtures/not_found.json");
const TOKEN_VAR: &str = "AI6_TEST_GITHUB_TOKEN";
const TOKEN: &str = "ghp_test_token_value";

static INIT: Once = Once::new();

/// Sets the token once, before any test reads the environment.
fn init() {
    INIT.call_once(|| {
        // SAFETY: runs once, and every test calls `init` before it reads the environment.
        unsafe { std::env::set_var(TOKEN_VAR, TOKEN) };
    });
}

/// The recorded `Link` of the first page, pointing at the loopback server.
fn page1_link(base: &str) -> String {
    PAGE1_LINK.trim().replace("https://api.github.com", base)
}

fn provider(base: &str, extra: &str) -> Github {
    init();
    Github::from_settings(&format!(
        "repo = \"o/r\"\ntoken_env = \"{TOKEN_VAR}\"\napi_url = \"{base}\"\n{extra}"
    ))
    .unwrap()
}

#[test]
fn polling_follows_pages_maps_items_and_advances_the_cursor() {
    let fake = serve(|base| {
        vec![
            Canned {
                status: 200,
                headers: vec![
                    ("link", page1_link(base)),
                    ("x-ratelimit-remaining", "4000".to_owned()),
                ],
                body: PAGE1.to_owned(),
            },
            canned(200, PAGE2),
        ]
    });
    let batch = provider(&fake.base, "")
        .poll(SourcePoll { cursor: None })
        .unwrap();

    assert_eq!(batch.items.len(), 2);
    let first = &batch.items[0];
    assert_eq!(first.id, "o/r#1");
    assert_eq!(first.title, "AI6 test data: labelled, non-ASCII body");
    assert!(first.body.contains("ñandú"), "{}", first.body);
    assert!(first.body.contains("日本語のテキスト"), "{}", first.body);
    assert!(first.body.contains("Ignore previous instructions"));
    assert_eq!(
        first.url.as_deref(),
        Some("https://github.com/o/r/issues/1")
    );
    assert_eq!(first.labels, ["ai6:ready"]);
    assert_eq!(first.author.as_deref(), Some("reporter"));
    assert_eq!(first.updated, "2026-11-01T15:56:01Z");
    assert_eq!(batch.items[1].id, "o/r#2");
    assert_eq!(batch.items[1].body, "", "a null body is empty text");
    assert_eq!(batch.cursor.as_deref(), Some("2026-11-01T15:56:02Z"));

    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert!(requests[0].target.starts_with("/repos/o/r/issues?"));
    assert!(requests[0].target.contains("state=open"));
    assert!(
        requests[0].target.contains("labels=ai6%3Aready"),
        "{}",
        requests[0].target
    );
    assert!(!requests[0].target.contains("since="));
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer ghp_test_token_value")
    );
    assert!(
        requests[1].target.starts_with("/repositories/1/issues?"),
        "the next page is wherever the header says: {}",
        requests[1].target
    );
    assert!(requests[1].target.contains("after="));
    assert!(requests[1].target.ends_with("&page=2"));
}

#[test]
fn a_cursor_becomes_since_and_an_empty_poll_keeps_it() {
    let fake = serve(|_| vec![canned(200, "[]")]);
    let cursor = "2026-10-04T11:15:00Z".to_owned();
    let batch = provider(&fake.base, "assignee = \"bot\"")
        .poll(SourcePoll {
            cursor: Some(cursor.clone()),
        })
        .unwrap();
    assert!(batch.items.is_empty());
    assert_eq!(batch.cursor, Some(cursor));
    let target = fake.requests.lock().unwrap()[0].target.clone();
    assert!(
        target.contains("since=2026-10-04T11%3A15%3A00Z"),
        "{target}"
    );
    assert!(target.contains("assignee=bot"));
}

#[test]
fn a_rate_limit_is_an_error_that_does_not_move_the_cursor() {
    let fake = serve(|_| {
        vec![Canned {
            status: 403,
            headers: vec![
                ("x-ratelimit-remaining", "0".to_owned()),
                ("x-ratelimit-reset", "1790000000".to_owned()),
            ],
            body: r#"{"message":"API rate limit exceeded"}"#.to_owned(),
        }]
    });
    let error = provider(&fake.base, "")
        .poll(SourcePoll {
            cursor: Some("c".to_owned()),
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("rate limit"), "{error}");
    assert!(!error.contains(TOKEN));
}

#[test]
fn pagination_to_a_foreign_origin_is_not_followed() {
    let fake = serve(|_| {
        vec![Canned {
            status: 200,
            headers: vec![(
                "link",
                "<http://127.0.0.2:9/steal?page=2>; rel=\"next\"".to_owned(),
            )],
            body: PAGE2.to_owned(),
        }]
    });
    let batch = provider(&fake.base, "")
        .poll(SourcePoll { cursor: None })
        .unwrap();
    assert_eq!(batch.items.len(), 1);
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
}

/// Not recorded: the token that made the recordings cannot open a pull request.
#[test]
fn a_pull_request_in_the_listing_is_an_item_too() {
    let mut page: serde_json::Value = serde_json::from_str(PAGE2).unwrap();
    page[0]["pull_request"] =
        serde_json::json!({ "url": "https://api.github.com/repos/o/r/pulls/2" });
    let fake = serve(|_| vec![canned(200, &page.to_string())]);
    let batch = provider(&fake.base, "")
        .poll(SourcePoll { cursor: None })
        .unwrap();
    assert_eq!(batch.items[0].id, "o/r#2");
}

#[test]
fn a_report_comments_and_labels_the_issue() {
    let fake = serve(|_| vec![canned(201, COMMENT_CREATED), canned(200, LABELS_ADDED)]);
    provider(&fake.base, "done_label = \"ai6:done\"")
        .report(CxReport {
            item_id: "o/r#12".to_owned(),
            mission_id: "m-12".to_owned(),
            outcome: "done".to_owned(),
            body: "The requested work is finished.\nBranch: `agent/fix`".to_owned(),
            branch: Some("agent/fix".to_owned()),
        })
        .unwrap();

    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].target, "/repos/o/r/issues/12/comments");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer ghp_test_token_value")
    );
    let comment: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(
        comment["body"],
        "The requested work is finished.\nBranch: `agent/fix`"
    );
    assert_eq!(requests[1].target, "/repos/o/r/issues/12/labels");
    let labels: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(labels["labels"][0], "ai6:done");
}

#[test]
fn a_failed_mission_adds_the_failure_label_only() {
    let fake = serve(|_| vec![canned(201, COMMENT_CREATED), canned(200, LABELS_ADDED)]);
    provider(
        &fake.base,
        "done_label = \"ai6:done\"\nfailed_label = \"ai6:failed\"",
    )
    .report(CxReport {
        item_id: "o/r#12".to_owned(),
        mission_id: "m-12".to_owned(),
        outcome: "failed".to_owned(),
        body: "The requested work could not be completed.".to_owned(),
        branch: None,
    })
    .unwrap();
    let requests = fake.requests.lock().unwrap();
    let labels: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(labels["labels"][0], "ai6:failed");
}

#[test]
fn a_blank_report_fails_without_calling_out() {
    let fake = serve(|_| vec![canned(201, COMMENT_CREATED)]);
    let error = provider(&fake.base, "")
        .report(CxReport {
            item_id: "o/r#12".to_owned(),
            mission_id: "m".to_owned(),
            outcome: "done".to_owned(),
            body: " \n ".to_owned(),
            branch: None,
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("empty"), "{error}");
    assert!(fake.requests.lock().unwrap().is_empty());
}

#[test]
fn a_report_without_a_token_fails_without_calling_out() {
    init();
    let fake = serve(|_| vec![canned(201, "{}")]);
    let provider = Github::from_settings(&format!(
        "repo = \"o/r\"\ntoken_env = \"AI6_TEST_GITHUB_TOKEN_UNSET\"\napi_url = \"{}\"",
        fake.base
    ))
    .unwrap();
    let error = provider
        .report(CxReport {
            item_id: "o/r#12".to_owned(),
            mission_id: "m".to_owned(),
            outcome: "done".to_owned(),
            body: "x".to_owned(),
            branch: None,
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("AI6_TEST_GITHUB_TOKEN_UNSET"), "{error}");
    assert!(fake.requests.lock().unwrap().is_empty());
}

#[test]
fn a_missing_repository_is_reported_without_the_response_body() {
    let fake = serve(|_| vec![canned(404, NOT_FOUND)]);
    let error = provider(&fake.base, "")
        .poll(SourcePoll { cursor: None })
        .unwrap_err()
        .to_string();
    assert!(error.contains("repository"), "{error}");
    assert!(!error.contains("Not Found"), "{error}");
}

#[test]
fn a_private_repository_without_a_token_says_which_variable_is_unset() {
    init();
    let fake = serve(|_| vec![canned(404, NOT_FOUND)]);
    let error = Github::from_settings(&format!(
        "repo = \"o/r\"\ntoken_env = \"AI6_TEST_GITHUB_TOKEN_UNSET\"\napi_url = \"{}\"",
        fake.base
    ))
    .unwrap()
    .poll(SourcePoll { cursor: None })
    .unwrap_err()
    .to_string();
    assert!(error.contains("AI6_TEST_GITHUB_TOKEN_UNSET"), "{error}");
}

#[test]
fn a_refused_token_names_the_variable_and_not_the_value() {
    let fake = serve(|_| vec![canned(401, BAD_CREDENTIALS)]);
    let error = provider(&fake.base, "")
        .poll(SourcePoll { cursor: None })
        .unwrap_err()
        .to_string();
    assert!(error.contains(TOKEN_VAR), "{error}");
    assert!(!error.contains(TOKEN), "{error}");
}

#[test]
fn the_page_size_and_page_count_are_settings() {
    let fake = serve(|base| {
        vec![Canned {
            status: 200,
            headers: vec![("link", page1_link(base))],
            body: PAGE1.to_owned(),
        }]
    });
    provider(
        &fake.base,
        "page_size = 7\nmax_pages = 1\nrequest_timeout_secs = 5",
    )
    .poll(SourcePoll { cursor: None })
    .unwrap();
    let requests = fake.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "one page only, though a next one is offered"
    );
    assert!(
        requests[0].target.contains("per_page=7"),
        "{}",
        requests[0].target
    );
    for zero in ["page_size = 0", "max_pages = 0", "request_timeout_secs = 0"] {
        init();
        assert!(
            Github::from_settings(&format!("repo = \"o/r\"\n{zero}")).is_err(),
            "{zero}"
        );
    }
}
