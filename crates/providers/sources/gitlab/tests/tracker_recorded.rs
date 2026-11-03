//! The Tracker against GitLab's own answers, recorded from a real project and scrubbed: each
//! fixture is one exchange, the request made and the response, headers included. `gitlab.rs`
//! covers what no recording shows: other people's comments and refused calls.

use std::sync::Once;

use ai6::{IssueSpec, ReplyPoll, Source, SourcePoll, Tracker, TrackerPost, TrackerUpdate};
use ai6_source_gitlab::Gitlab;

mod support;
use support::{canned, recorded, replay, serve};

const PROJECT: &str = include_str!("fixtures/project.json");
const LABELS: &str = include_str!("fixtures/project_labels.json");
const ISSUE: &str = include_str!("fixtures/issue.json");
const PARENT: &str = include_str!("fixtures/parent.json");
const TASK: &str = include_str!("fixtures/task.json");
const TASK_CREATED: &str = include_str!("fixtures/task_created.json");
const GRAPHQL_ATTACH: &str = include_str!("fixtures/graphql_attach.json");
const GRAPHQL_ATTACH_REFUSED: &str = include_str!("fixtures/graphql_attach_refused.json");
const GRAPHQL_PARENT: &str = include_str!("fixtures/graphql_parent.json");
const NOTE_CREATED: &str = include_str!("fixtures/note_created.json");
const NOTE_EDITED: &str = include_str!("fixtures/note_edited.json");
const NOTE_BLANK: &str = include_str!("fixtures/note_blank.json");
const NOTES_DESC: &str = include_str!("fixtures/notes_desc.json");
const MEMBER: &str = include_str!("fixtures/member.json");
const MEMBERS_QUERY: &str = include_str!("fixtures/members_query.json");
const LABELS_CHANGED: &str = include_str!("fixtures/labels_changed.json");
const ASSIGNEE_SET: &str = include_str!("fixtures/assignee_set.json");
const ISSUE_CLOSED: &str = include_str!("fixtures/issue_closed.json");
const NOT_FOUND_UPDATE: &str = include_str!("fixtures/not_found_update.json");
const STATE_EVENT_BAD: &str = include_str!("fixtures/state_event_bad.json");
const PROJECT_NOT_FOUND: &str = include_str!("fixtures/project_not_found.json");

const TOKEN_VAR: &str = "AI6_TEST_GITLAB_RECORDED_TOKEN";
static INIT: Once = Once::new();

fn provider(base: &str) -> Gitlab {
    INIT.call_once(|| {
        // SAFETY: runs once, and every test calls this before it reads the environment.
        unsafe { std::env::set_var(TOKEN_VAR, "test-token-value") };
    });
    Gitlab::from_settings(&format!(
        "project = \"o/r\"\ntoken_env = \"{TOKEN_VAR}\"\napi_url = \"{base}/api/v4\""
    ))
    .unwrap()
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

/// A field of the recorded response body.
fn answered(exchange: &str, pointer: &str) -> serde_json::Value {
    json(exchange)["response"]["body"]
        .pointer(pointer)
        .cloned()
        .unwrap()
}

#[test]
fn capabilities_read_the_recorded_project() {
    let fake = serve(|_| vec![replay(PROJECT, &[]), replay(LABELS, &["link"])]);
    let capabilities = provider(&fake.base).capabilities(None).unwrap();
    assert_eq!(capabilities.visibility, "private");
    assert!(capabilities.create);
    assert!(capabilities.labels.iter().any(|label| label == "ai6:ready"));
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests[0].target, "/api/v4/projects/o%2Fr");
    assert_eq!(requests.len(), 2, "the recorded Link names no next page");
}

#[test]
fn a_recorded_post_and_edit_give_the_note_and_its_address() {
    let fake = serve(|_| vec![replay(NOTE_CREATED, &[]), replay(NOTE_EDITED, &[])]);
    let gitlab = provider(&fake.base);
    let posted = gitlab
        .post(TrackerPost {
            item: "o/r#1".to_owned(),
            body: "x".to_owned(),
            edit: None,
            marker: Some("status".to_owned()),
        })
        .unwrap();
    let id = answered(NOTE_CREATED, "/id").to_string();
    assert_eq!(posted.id, id);
    assert!(
        posted
            .url
            .as_deref()
            .is_some_and(|url| url.ends_with(&format!("/o/r/-/issues/1#note_{id}"))),
        "{:?}",
        posted.url
    );
    let edited = gitlab
        .post(TrackerPost {
            item: "o/r#1".to_owned(),
            body: "y".to_owned(),
            edit: Some(id.clone()),
            marker: Some("status".to_owned()),
        })
        .unwrap();
    assert_eq!(edited.id, answered(NOTE_EDITED, "/id").to_string());
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests[1].method, "PUT");
    assert_eq!(
        requests[1].target,
        format!("/api/v4/projects/o%2Fr/issues/1/notes/{id}")
    );
}

#[test]
fn a_recorded_blank_note_refusal_says_nothing_of_the_response() {
    let fake = serve(|_| vec![replay(NOTE_BLANK, &[])]);
    let error = provider(&fake.base)
        .post(TrackerPost {
            item: "o/r#1".to_owned(),
            body: "text that GitLab refused anyway".to_owned(),
            ..TrackerPost::default()
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("rejected"), "{error}");
    assert!(!error.contains("blank"), "{error}");
}

#[test]
fn recorded_notes_give_a_marked_own_reply_and_a_persons_reply() {
    let fake = serve(|_| vec![replay(NOTES_DESC, &["link"]), replay(PARENT, &[])]);
    let batch = provider(&fake.base)
        .replies(ReplyPoll {
            items: vec!["o/r#12".to_owned()],
            since: None,
        })
        .unwrap();
    let summary: Vec<(bool, bool)> = batch
        .replies
        .iter()
        .map(|reply| (reply.own, reply.permitted))
        .collect();
    assert_eq!(
        summary,
        [(true, true), (false, true)],
        "oldest first: the marked status, then the person's reply on the same account"
    );
    assert_eq!(batch.replies[0].author_id, "reporter");
    assert_eq!(
        batch.cursor,
        answered(NOTES_DESC, "/0/updated_at")
            .as_str()
            .map(str::to_owned)
    );
    assert_eq!(
        fake.requests.lock().unwrap().len(),
        2,
        "the author answers it"
    );
}

#[test]
fn a_recorded_member_may_answer_by_access_level() {
    let notes = format!(
        "[{}]",
        serde_json::json!({
            "id": 1, "body": "Use the main branch.", "system": false,
            "author": { "id": 1, "username": "someone" },
            "created_at": "2026-11-03T15:33:53.265Z", "updated_at": "2026-11-03T15:33:53.265Z",
        })
    );
    let fake = serve(|_| vec![canned(200, &notes), replay(ISSUE, &[]), replay(MEMBER, &[])]);
    let batch = provider(&fake.base)
        .replies(ReplyPoll {
            items: vec!["o/r#1".to_owned()],
            since: None,
        })
        .unwrap();
    assert!(batch.replies[0].permitted, "an Owner, level 50");
    assert_eq!(
        fake.requests.lock().unwrap()[2].target,
        "/api/v4/projects/o%2Fr/members/all/1"
    );
}

#[test]
fn a_recorded_issue_reads_its_state_labels_and_a_tasks_parent() {
    let fake = serve(|_| {
        vec![
            replay(ISSUE, &[]),
            replay(TASK, &[]),
            replay(GRAPHQL_PARENT, &[]),
        ]
    });
    let gitlab = provider(&fake.base);
    let issue = gitlab.issue("o/r#1".to_owned()).unwrap();
    assert_eq!(issue.state, "open");
    assert_eq!(issue.native_id, answered(ISSUE, "/id").to_string());
    assert_eq!(issue.labels, ["ai6:ready"]);
    assert!(issue.body.contains("ñandú"));
    assert_eq!(issue.parent, None);
    let task = gitlab.issue("o/r#14".to_owned()).unwrap();
    assert_eq!(task.parent.as_deref(), Some("o/r#12"));
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests[2].method, "POST");
    assert_eq!(requests[2].target, "/api/graphql");
    assert_eq!(
        json(&requests[2].body)["variables"],
        serde_json::json!({ "path": "o/r", "iid": "14" })
    );
}

#[test]
fn a_recorded_child_is_a_task_attached_by_global_ids() {
    let fake = serve(|_| {
        vec![
            replay(PROJECT, &[]),
            replay(LABELS, &[]),
            replay(PARENT, &[]),
            replay(TASK_CREATED, &[]),
            replay(GRAPHQL_ATTACH, &[]),
        ]
    });
    let child = provider(&fake.base)
        .create(IssueSpec {
            title: "AI6 test data: a child".to_owned(),
            body: "A child.".to_owned(),
            labels: vec!["ai6:working".to_owned(), "ai6:no-such-label".to_owned()],
            parent: Some("o/r#12".to_owned()),
            ..IssueSpec::default()
        })
        .unwrap();
    assert_eq!(child.id, "o/r#14");
    assert_eq!(child.parent.as_deref(), Some("o/r#12"));
    assert_eq!(child.labels, ["ai6:working"]);
    let requests = fake.requests.lock().unwrap();
    let created = json(&requests[3].body);
    assert_eq!(created["issue_type"], "task");
    assert_eq!(created["labels"], "ai6:working");
    let attach = json(&requests[4].body);
    assert_eq!(
        attach["variables"]["id"],
        format!("gid://gitlab/WorkItem/{}", answered(TASK_CREATED, "/id"))
    );
    assert_eq!(
        attach["variables"]["parent"],
        format!("gid://gitlab/WorkItem/{}", answered(PARENT, "/id"))
    );
}

#[test]
fn a_recorded_refusal_to_attach_leaves_the_child_created_with_no_parent() {
    let fake = serve(|_| {
        vec![
            replay(PROJECT, &[]),
            replay(LABELS, &[]),
            replay(PARENT, &[]),
            replay(TASK_CREATED, &[]),
            replay(GRAPHQL_ATTACH_REFUSED, &[]),
        ]
    });
    let child = provider(&fake.base)
        .create(IssueSpec {
            title: "AI6 test data: a child".to_owned(),
            parent: Some("o/r#12".to_owned()),
            ..IssueSpec::default()
        })
        .unwrap();
    assert_eq!(child.id, "o/r#14");
    assert_eq!(child.parent, None);
}

#[test]
fn recorded_updates_change_labels_assignees_and_state() {
    let fake = serve(|_| {
        vec![
            replay(ISSUE, &[]),
            replay(MEMBERS_QUERY, &[]),
            replay(ASSIGNEE_SET, &[]),
            replay(LABELS_CHANGED, &[]),
            replay(ISSUE_CLOSED, &[]),
            replay(NOT_FOUND_UPDATE, &[]),
        ]
    });
    let gitlab = provider(&fake.base);
    gitlab
        .update(TrackerUpdate {
            item: "o/r#1".to_owned(),
            add_assignees: vec!["reporter".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap();
    gitlab
        .update(TrackerUpdate {
            item: "o/r#1".to_owned(),
            add_labels: vec!["ai6:working".to_owned()],
            remove_labels: vec!["ai6:ready".to_owned(), "ai6:not-there".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap();
    gitlab
        .update(TrackerUpdate {
            item: "o/r#3".to_owned(),
            state: Some("closed".to_owned()),
            ..TrackerUpdate::default()
        })
        .unwrap();
    let error = gitlab
        .update(TrackerUpdate {
            item: "o/r#99999".to_owned(),
            remove_labels: vec!["ai6:ready".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not show"), "{error}");
    let requests = fake.requests.lock().unwrap();
    assert_eq!(
        json(&requests[2].body)["assignee_ids"],
        serde_json::json!([answered(MEMBERS_QUERY, "/0/id")])
    );
}

#[test]
fn a_recorded_refusal_of_a_state_event_is_an_invalid_request() {
    let fake = serve(|_| vec![replay(STATE_EVENT_BAD, &[])]);
    // The provider never sends an unknown event; a request GitLab refuses reads the same way.
    let error = provider(&fake.base)
        .update(TrackerUpdate {
            item: "o/r#1".to_owned(),
            add_labels: vec!["x".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("rejected"), "{error}");
}

#[test]
fn a_recorded_missing_project_names_no_response_text() {
    let fake = serve(|_| vec![replay(PROJECT_NOT_FOUND, &[])]);
    let error = provider(&fake.base)
        .poll(SourcePoll { cursor: None })
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not show"), "{error}");
    assert!(!error.contains("Project Not Found"), "{error}");
    assert!(recorded(PROJECT_NOT_FOUND).contains("Project Not Found"));
}
