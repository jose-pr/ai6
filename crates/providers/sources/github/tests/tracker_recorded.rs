//! The Tracker against GitHub's own answers, recorded from a real repository and scrubbed.
//! `github.rs` covers what no recording shows: other people's comments and refused calls.

use std::sync::Once;

use ai6::{IssueSpec, ReplyPoll, Tracker, TrackerPost, TrackerUpdate};
use ai6_source_github::Github;

mod support;
use support::{canned, serve};

const REPO: &str = include_str!("fixtures/tracker_repo.json");
const REPO_LABELS: &str = include_str!("fixtures/tracker_repo_labels.json");
const ISSUE: &str = include_str!("fixtures/tracker_issue.json");
const CLOSED_CHILD: &str = include_str!("fixtures/tracker_issue_closed.json");
const CHILD: &str = include_str!("fixtures/tracker_issue_child.json");
const SUB_ISSUE_ADDED: &str = include_str!("fixtures/tracker_sub_issue_added.json");
const COMMENT_CREATED: &str = include_str!("fixtures/comment_created.json");
const COMMENT_EDITED: &str = include_str!("fixtures/tracker_comment_edited.json");
const COMMENTS_SINCE: &str = include_str!("fixtures/tracker_comments_since.json");
const LABELS_ADDED: &str = include_str!("fixtures/labels_added.json");
const LABEL_REMOVED: &str = include_str!("fixtures/tracker_label_removed.json");
const LABEL_ABSENT: &str = include_str!("fixtures/tracker_label_absent.json");
const ASSIGNEE_ADDED: &str = include_str!("fixtures/tracker_assignee_added.json");
const NOT_FOUND: &str = include_str!("fixtures/not_found.json");
const COMMENT_EMPTY: &str = include_str!("fixtures/tracker_comment_empty.json");

const TOKEN_VAR: &str = "AI6_TEST_GITHUB_RECORDED_TOKEN";
static INIT: Once = Once::new();

fn provider(base: &str) -> Github {
    INIT.call_once(|| {
        // SAFETY: runs once, and every test calls this before it reads the environment.
        unsafe { std::env::set_var(TOKEN_VAR, "ghp_test_token_value") };
    });
    Github::from_settings(&format!(
        "repo = \"o/r\"\ntoken_env = \"{TOKEN_VAR}\"\napi_url = \"{base}\""
    ))
    .unwrap()
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn capabilities_read_the_recorded_repository() {
    let fake = serve(|_| vec![canned(200, REPO), canned(200, REPO_LABELS)]);
    let capabilities = provider(&fake.base).capabilities(None).unwrap();
    assert_eq!(capabilities.visibility, "private");
    assert!(capabilities.create);
    assert!(capabilities.labels.iter().any(|label| label == "ai6:ready"));
}

#[test]
fn a_recorded_post_and_edit_give_the_comment_and_its_address() {
    let fake = serve(|_| vec![canned(201, COMMENT_CREATED), canned(200, COMMENT_EDITED)]);
    let github = provider(&fake.base);
    let posted = github
        .post(TrackerPost {
            item: "o/r#23".to_owned(),
            body: "x".to_owned(),
            edit: None,
            marker: Some("status".to_owned()),
        })
        .unwrap();
    assert_eq!(posted.id, json(COMMENT_CREATED)["id"].to_string());
    assert_eq!(
        posted.url.as_deref(),
        json(COMMENT_CREATED)["html_url"].as_str()
    );
    let edited = github
        .post(TrackerPost {
            item: "o/r#4".to_owned(),
            body: "y".to_owned(),
            edit: Some(json(COMMENT_EDITED)["id"].to_string()),
            marker: None,
        })
        .unwrap();
    assert_eq!(edited.id, json(COMMENT_EDITED)["id"].to_string());
    let requests = fake.requests.lock().unwrap();
    assert!(
        requests[1]
            .target
            .starts_with("/repos/o/r/issues/comments/")
    );
    assert_eq!(requests[1].method, "PATCH");
}

#[test]
fn a_recorded_comment_with_a_marker_is_ai6s_own_and_moves_the_cursor() {
    let fake = serve(|_| vec![canned(200, COMMENTS_SINCE)]);
    let batch = provider(&fake.base)
        .replies(ReplyPoll {
            items: vec!["o/r#4".to_owned()],
            since: Some("2026-11-01T15:54:23Z".to_owned()),
        })
        .unwrap();
    assert_eq!(batch.replies.len(), 1);
    let reply = &batch.replies[0];
    assert_eq!(reply.id, "6560299123");
    assert_eq!(reply.item, "o/r#4");
    assert_eq!(reply.created, "2026-11-01T15:56:39Z");
    assert!(reply.own && reply.permitted);
    assert_eq!(batch.cursor.as_deref(), Some("2026-11-01T15:56:42Z"));
    assert!(
        fake.requests.lock().unwrap()[0]
            .target
            .contains("since=2026-11-01T15%3A54%3A23Z")
    );
}

#[test]
fn a_recorded_issue_reads_its_state_labels_and_parent() {
    let fake = serve(|_| vec![canned(200, ISSUE), canned(200, CLOSED_CHILD)]);
    let github = provider(&fake.base);
    let issue = github.issue("o/r#4".to_owned()).unwrap();
    assert_eq!(issue.state, "open");
    assert_eq!(issue.native_id, json(ISSUE)["id"].to_string());
    assert!(issue.labels.is_empty());
    assert_eq!(issue.parent, None);
    let child = github.issue("o/r#5".to_owned()).unwrap();
    assert_eq!(child.state, "closed");
    assert_eq!(child.parent.as_deref(), Some("o/r#4"));
}

#[test]
fn a_recorded_child_attaches_by_its_numeric_id_and_not_its_number() {
    let fake = serve(|_| {
        vec![
            canned(200, REPO),
            canned(200, REPO_LABELS),
            canned(201, CHILD),
            canned(201, SUB_ISSUE_ADDED),
        ]
    });
    let child = provider(&fake.base)
        .create(IssueSpec {
            title: "AI6 test data: sub-issue child".to_owned(),
            body: "A child issue.".to_owned(),
            parent: Some("o/r#4".to_owned()),
            ..IssueSpec::default()
        })
        .unwrap();
    assert_eq!(child.id, "o/r#5");
    assert_eq!(child.parent.as_deref(), Some("o/r#4"));
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests[3].target, "/repos/o/r/issues/4/sub_issues");
    let sent = json(&requests[3].body);
    assert_eq!(sent["sub_issue_id"], json(CHILD)["id"]);
    assert_ne!(sent["sub_issue_id"], json(CHILD)["number"]);
}

#[test]
fn removing_a_recorded_absent_label_is_fine_and_a_missing_issue_is_not() {
    let fake = serve(|_| {
        vec![
            canned(200, LABELS_ADDED),
            canned(200, LABEL_REMOVED),
            canned(404, LABEL_ABSENT),
            canned(201, ASSIGNEE_ADDED),
            canned(404, NOT_FOUND),
        ]
    });
    let github = provider(&fake.base);
    github
        .update(TrackerUpdate {
            item: "o/r#4".to_owned(),
            add_labels: vec!["ai6:new".to_owned()],
            remove_labels: vec!["ai6:new".to_owned(), "ai6:not-there".to_owned()],
            add_assignees: vec!["reporter".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap();
    let error = github
        .update(TrackerUpdate {
            item: "o/r#99999".to_owned(),
            remove_labels: vec!["ai6:new".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not show"), "{error}");
}

#[test]
fn a_recorded_refusal_of_an_empty_comment_says_nothing_of_the_response() {
    let fake = serve(|_| vec![canned(422, COMMENT_EMPTY)]);
    let error = provider(&fake.base)
        .post(TrackerPost {
            item: "o/r#4".to_owned(),
            body: "text that GitHub refused anyway".to_owned(),
            ..TrackerPost::default()
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("rejected"), "{error}");
    assert!(!error.contains("Body cannot be blank"), "{error}");
}

#[test]
fn every_request_asks_for_the_api_version_that_carries_no_deprecation() {
    let fake = serve(|_| vec![canned(200, ISSUE)]);
    provider(&fake.base).issue("o/r#4".to_owned()).unwrap();
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests[0].api_version.as_deref(), Some("2026-03-10"));
}
