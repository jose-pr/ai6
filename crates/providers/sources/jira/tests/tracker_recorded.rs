//! The Tracker against Jira Cloud's own answers, recorded from a real site and scrubbed.
//! `jira.rs` covers what no recording shows: other people's comments and Server.

use std::sync::Once;

use ai6::{IssueSpec, ReplyPoll, Tracker, TrackerLink, TrackerPost, TrackerUpdate};
use ai6_source_jira::Jira;

mod support;
use support::{canned, serve};

const META: &str = include_str!("fixtures/tracker_createmeta_issuetypes.json");
const PRIORITIES: &str = include_str!("fixtures/tracker_priorities.json");
const TRANSITIONS: &str = include_str!("fixtures/transitions.json");
const ISSUE: &str = include_str!("fixtures/tracker_issue.json");
const COMMENT_CREATED: &str = include_str!("fixtures/comment_created.json");
const COMMENT_EDITED: &str = include_str!("fixtures/tracker_comment_edited.json");
const COMMENTS: &str = include_str!("fixtures/tracker_comments.json");
const CREATED: &str = include_str!("fixtures/tracker_created.json");
const PARENT_SUBTASK: &str = include_str!("fixtures/tracker_parent_subtask.json");
const PARENT_STANDARD: &str = include_str!("fixtures/tracker_parent_standard.json");
const REMOTE_LINK: &str = include_str!("fixtures/tracker_remote_link.json");
const ISSUE_MISSING: &str = include_str!("fixtures/tracker_issue_missing.json");
const COMMENT_EMPTY: &str = include_str!("fixtures/tracker_comment_empty.json");
const TRANSITION_UNKNOWN: &str = include_str!("fixtures/tracker_transition_unknown.json");

const EMAIL_VAR: &str = "AI6_TEST_JIRA_RECORDED_EMAIL";
const TOKEN_VAR: &str = "AI6_TEST_JIRA_RECORDED_TOKEN";
static INIT: Once = Once::new();

fn provider(base: &str) -> Jira {
    INIT.call_once(|| {
        // SAFETY: runs once, and every test calls this before it reads the environment.
        unsafe {
            std::env::set_var(EMAIL_VAR, "reporter@example.invalid");
            std::env::set_var(TOKEN_VAR, "api-token");
        }
    });
    Jira::from_settings(&format!(
        "site = \"{base}\"\nproject = \"AI6\"\nemail_env = \"{EMAIL_VAR}\"\ntoken_env = \"{TOKEN_VAR}\""
    ))
    .unwrap()
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn capabilities_read_the_recorded_metadata_and_transitions() {
    let fake = serve(|_| {
        vec![
            canned(200, META),
            canned(200, PRIORITIES),
            canned(200, TRANSITIONS),
        ]
    });
    let capabilities = provider(&fake.base)
        .capabilities(Some("AI6-7".to_owned()))
        .unwrap();
    assert!(capabilities.create && capabilities.free_labels);
    assert_eq!(capabilities.children, "parent");
    assert!(capabilities.issue_types.iter().any(|name| name == "Task"));
    assert!(
        !capabilities
            .issue_types
            .iter()
            .any(|name| name == "Sub-task")
    );
    assert!(capabilities.priorities.iter().any(|name| name == "Medium"));
    assert_eq!(
        capabilities.transitions,
        ["Backlog", "Selected for Development", "In Progress", "Done"]
    );
}

#[test]
fn a_refused_credential_is_an_error_and_not_a_project_that_cannot_create() {
    let fake = serve(|_| {
        vec![
            canned(404, ISSUE_MISSING),
            canned(404, include_str!("fixtures/project_unknown.json")),
        ]
    });
    let error = provider(&fake.base)
        .capabilities(None)
        .unwrap_err()
        .to_string();
    assert!(error.contains(TOKEN_VAR), "{error}");
}

#[test]
fn a_recorded_issue_reads_its_state_priority_and_author() {
    let fake = serve(|_| vec![canned(200, ISSUE)]);
    let issue = provider(&fake.base).issue("AI6-7".to_owned()).unwrap();
    assert_eq!(issue.id, "AI6-7");
    assert_eq!(issue.state, "open");
    assert_eq!(issue.priority.as_deref(), Some("Medium"));
    assert_eq!(
        issue.author_id.as_deref(),
        Some("000000:00000000-0000-0000-0000-000000000000")
    );
}

#[test]
fn a_recorded_post_and_edit_give_the_comment_and_its_address() {
    let fake = serve(|_| vec![canned(201, COMMENT_CREATED), canned(200, COMMENT_EDITED)]);
    let jira = provider(&fake.base);
    let posted = jira
        .post(TrackerPost {
            item: "AI6-7".to_owned(),
            body: "x".to_owned(),
            ..TrackerPost::default()
        })
        .unwrap();
    assert_eq!(posted.id, json(COMMENT_CREATED)["id"].as_str().unwrap());
    assert!(posted.url.unwrap().contains("focusedCommentId"));
    let edited = jira
        .post(TrackerPost {
            item: "AI6-7".to_owned(),
            body: "y".to_owned(),
            edit: Some(posted.id.clone()),
            marker: Some("status".to_owned()),
        })
        .unwrap();
    assert_eq!(edited.id, json(COMMENT_EDITED)["id"].as_str().unwrap());
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests[1].method, "PUT");
    assert!(
        requests[1]
            .target
            .starts_with("/rest/api/3/issue/AI6-7/comment/")
    );
    assert!(
        !requests[1].body.contains("ai6:"),
        "Jira comments carry no marker"
    );
}

#[test]
fn recorded_comments_are_told_by_their_property_and_the_cursor_is_the_newest_change() {
    let fake = serve(|_| vec![canned(200, COMMENTS), canned(200, ISSUE)]);
    let batch = provider(&fake.base)
        .replies(ReplyPoll {
            items: vec!["AI6-7".to_owned()],
            since: None,
        })
        .unwrap();
    assert_eq!(batch.replies.len(), 1);
    let reply = &batch.replies[0];
    assert_eq!(reply.id, "10000");
    assert!(!reply.own, "no property, whoever's account it is on");
    assert!(reply.permitted, "the credential's own account may answer");
    assert_eq!(reply.body, "AI6 test data: the edited comment.");
    assert_eq!(
        batch.cursor.as_deref(),
        Some("2026-11-01T22:57:12.977+0700")
    );
}

#[test]
fn a_cursor_on_another_clock_still_finds_the_comment_after_it() {
    let earlier_in_utc = "2026-11-01T15:57:00.000+0000";
    let fake = serve(|_| vec![canned(200, COMMENTS), canned(200, ISSUE)]);
    let batch = provider(&fake.base)
        .replies(ReplyPoll {
            items: vec!["AI6-7".to_owned()],
            since: Some(earlier_in_utc.to_owned()),
        })
        .unwrap();
    assert_eq!(batch.replies.len(), 1, "22:57 +0700 is after 15:57 +0000");
}

#[test]
fn replies_fail_when_the_credential_is_refused() {
    let fake = serve(|_| vec![canned(401, "Client must be authenticated")]);
    let error = provider(&fake.base)
        .replies(ReplyPoll {
            items: vec!["AI6-7".to_owned()],
            since: None,
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains(TOKEN_VAR), "{error}");
}

fn created_issue() -> String {
    ISSUE.replace("AI6-7", "AI6-6")
}

#[test]
fn a_child_of_a_standard_issue_is_a_subtask_under_it() {
    let reread = created_issue();
    let fake = serve(|_| {
        vec![
            canned(200, META),
            canned(200, PRIORITIES),
            canned(200, PARENT_STANDARD),
            canned(201, CREATED),
            canned(200, &reread),
        ]
    });
    provider(&fake.base)
        .create(IssueSpec {
            title: "AI6 test data: child".to_owned(),
            parent: Some("AI6-1".to_owned()),
            ..IssueSpec::default()
        })
        .unwrap();
    let requests = fake.requests.lock().unwrap();
    let sent = json(&requests[3].body);
    assert_eq!(sent["fields"]["issuetype"]["name"], "Sub-task");
    assert_eq!(sent["fields"]["parent"]["key"], "AI6-1");
    assert!(
        sent["fields"].get("description").is_none(),
        "a blank body is no document"
    );
}

#[test]
fn a_child_of_a_subtask_is_created_beside_it_and_linked() {
    let reread = created_issue();
    let fake = serve(|_| {
        vec![
            canned(200, META),
            canned(200, PRIORITIES),
            canned(200, PARENT_SUBTASK),
            canned(201, CREATED),
            canned(201, ""),
            canned(200, &reread),
        ]
    });
    provider(&fake.base)
        .create(IssueSpec {
            title: "AI6 test data: under a sub-task".to_owned(),
            parent: Some("AI6-5".to_owned()),
            ..IssueSpec::default()
        })
        .unwrap();
    let requests = fake.requests.lock().unwrap();
    let sent = json(&requests[3].body);
    assert_eq!(sent["fields"]["issuetype"]["name"], "Task");
    assert!(sent["fields"].get("parent").is_none());
    assert_eq!(requests[4].target, "/rest/api/3/issueLink");
    let link = json(&requests[4].body);
    assert_eq!(link["type"]["name"], "Relates");
    assert_eq!(link["inwardIssue"]["key"], "AI6-6");
    assert_eq!(link["outwardIssue"]["key"], "AI6-5");
}

#[test]
fn a_recorded_link_answers_with_no_body_and_a_remote_link_with_an_id() {
    let fake = serve(|_| vec![canned(201, ""), canned(201, REMOTE_LINK)]);
    let jira = provider(&fake.base);
    jira.link(TrackerLink {
        from: "AI6-7".to_owned(),
        to: Some("AI6-3".to_owned()),
        relation: "related".to_owned(),
        url: None,
    })
    .unwrap();
    jira.link(TrackerLink {
        from: "AI6-7".to_owned(),
        to: None,
        relation: "mirror".to_owned(),
        url: Some("https://example.com/pull/1".to_owned()),
    })
    .unwrap();
}

#[test]
fn recorded_refusals_name_what_was_rejected_and_not_what_jira_said() {
    let fake = serve(|_| {
        vec![
            canned(400, COMMENT_EMPTY),
            canned(200, TRANSITIONS),
            canned(400, TRANSITION_UNKNOWN),
            canned(404, ISSUE_MISSING),
        ]
    });
    let jira = provider(&fake.base);
    let error = jira
        .post(TrackerPost {
            item: "AI6-7".to_owned(),
            body: "text Jira refused anyway".to_owned(),
            ..TrackerPost::default()
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("rejected the comment on AI6-7"), "{error}");
    assert!(!error.contains("can not be empty"), "{error}");
    let error = jira
        .update(TrackerUpdate {
            item: "AI6-7".to_owned(),
            transition: Some("Done".to_owned()),
            ..TrackerUpdate::default()
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("rejected the transition"), "{error}");
    let error = jira.issue("AI6-99999".to_owned()).unwrap_err().to_string();
    assert!(error.contains(TOKEN_VAR), "{error}");
}
