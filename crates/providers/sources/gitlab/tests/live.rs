//! Runs the provider against a real GitLab, on a project set aside for it. Every test is
//! ignored unless asked for, and fails rather than skips when asked for without its settings:
//!
//! ```text
//! AI6_TEST_LIVE_SOURCES=1 AI6_TEST_LIVE_GITLAB_PROJECT=group/name AI6_TEST_LIVE_GITLAB_TOKEN=... \
//!   cargo test -p ai6-source-gitlab --test live -- --ignored --test-threads=1
//! ```
//!
//! The token needs the `api` scope and at least the Reporter role on that project, whose
//! labels must include `ai6:ready`, `ai6:working`, `ai6:blocked` and `ai6:done`. Each test
//! opens issues titled as test data and closes them when it ends, passed or not.
//! `AI6_TEST_LIVE_GITLAB_API_URL` points the calls at another instance or a recording proxy.
//! gitlab.com's free plan allows a user about 100 requests a minute: run one test at a time.

use std::sync::Once;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ai6::guatiao::Status;
use ai6::{
    IssueSpec, ReplyPoll, Source, SourceBatch, SourceItem, SourcePoll, Tracker, TrackerLink,
    TrackerPost, TrackerUpdate,
};
use ai6_source_gitlab::Gitlab;
use serde_json::{Value, json};

const TOKEN_VAR: &str = "AI6_TEST_LIVE_GITLAB_TOKEN";
const FALSE_TOKEN_VAR: &str = "AI6_TEST_LIVE_GITLAB_FALSE_TOKEN";
/// How many times a poll is repeated, two seconds apart, for a change to show in the listing.
const LISTING_ATTEMPTS: u32 = 30;
const NON_ASCII: &str = "Añadir pruebas: ñandú, café.\n\n日本語, Ελληνικά, العربية 🧪";

static INIT: Once = Once::new();

/// One project under test, and the issues to close when the test ends.
struct Live {
    project: String,
    api: String,
    token: String,
    agent: ureq::Agent,
    stamp: u128,
    opened: Vec<u64>,
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("this test needs {name}: see tests/live.rs"))
}

/// A project path as one URL path segment.
fn encoded(project: &str) -> String {
    project.replace('/', "%2F")
}

impl Live {
    fn from_env() -> Self {
        required("AI6_TEST_LIVE_SOURCES");
        INIT.call_once(|| {
            // SAFETY: runs once, and every test calls this before it reads the environment.
            unsafe { std::env::set_var(FALSE_TOKEN_VAR, "not-a-token") };
        });
        Self {
            project: required("AI6_TEST_LIVE_GITLAB_PROJECT"),
            api: std::env::var("AI6_TEST_LIVE_GITLAB_API_URL")
                .unwrap_or_else(|_| "https://gitlab.com/api/v4".to_owned()),
            token: required(TOKEN_VAR),
            agent: ureq::Agent::new_with_defaults(),
            stamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            opened: Vec::new(),
        }
    }

    fn provider(&self, extra: &str) -> Gitlab {
        Gitlab::from_settings(&format!(
            "project = \"{}\"\ntoken_env = \"{TOKEN_VAR}\"\napi_url = \"{}\"\n{extra}",
            self.project, self.api
        ))
        .unwrap()
    }

    /// A request outside the provider; `path` is under the project, or absolute from `/`.
    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let url = if path.starts_with("/user") {
            format!("{}{path}", self.api)
        } else {
            format!("{}/projects/{}{path}", self.api, encoded(&self.project))
        };
        let mut response = match method {
            "GET" => self
                .agent
                .get(&url)
                .header("PRIVATE-TOKEN", &self.token)
                .call(),
            "POST" => self
                .agent
                .post(&url)
                .header("PRIVATE-TOKEN", &self.token)
                .send_json(body.unwrap()),
            "PUT" => self
                .agent
                .put(&url)
                .header("PRIVATE-TOKEN", &self.token)
                .send_json(body.unwrap()),
            other => panic!("unexpected method {other}"),
        }
        .unwrap_or_else(|error| panic!("{method} {path}: {error}"));
        response.body_mut().read_json().unwrap()
    }

    fn open(&mut self, title: &str, body: Option<&str>, labels: &[&str]) -> u64 {
        let mut issue = json!({
            "title": format!("AI6 test data: {title} ({})", self.stamp),
            "labels": labels.join(","),
        });
        if let Some(body) = body {
            issue["description"] = json!(body);
        }
        let iid = self.request("POST", "/issues", Some(issue))["iid"]
            .as_u64()
            .unwrap();
        self.opened.push(iid);
        iid
    }

    fn id(&self, iid: u64) -> String {
        format!("{}#{iid}", self.project)
    }

    fn close(&mut self, iid: u64) {
        self.request(
            "PUT",
            &format!("/issues/{iid}"),
            Some(json!({ "state_event": "close" })),
        );
        self.opened.retain(|opened| *opened != iid);
    }

    /// The comments people and AI6 wrote on an issue, without GitLab's own notes.
    fn comments(&self, iid: u64) -> Vec<Value> {
        self.request("GET", &format!("/issues/{iid}/notes?sort=asc"), None)
            .as_array()
            .unwrap()
            .iter()
            .filter(|note| note["system"] != true)
            .cloned()
            .collect()
    }

    fn labels(&self, iid: u64) -> Vec<String> {
        self.request("GET", &format!("/issues/{iid}"), None)["labels"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|label| label.as_str().map(str::to_owned))
            .collect()
    }

    /// What a finished Mission writes: a comment, then the label of its outcome.
    fn finish(&self, provider: &Gitlab, iid: u64, label: &str, body: &str) {
        provider
            .post(TrackerPost {
                item: self.id(iid),
                body: body.to_owned(),
                ..TrackerPost::default()
            })
            .unwrap();
        provider
            .update(TrackerUpdate {
                item: self.id(iid),
                add_labels: vec![label.to_owned()],
                ..TrackerUpdate::default()
            })
            .unwrap();
    }
}

/// Polls until `wanted` holds, since a change can reach the issue listing some time later.
fn poll_until(
    provider: &Gitlab,
    cursor: Option<String>,
    wanted: impl Fn(&[SourceItem]) -> bool,
) -> SourceBatch {
    for attempt in 0..LISTING_ATTEMPTS {
        let batch = provider
            .poll(SourcePoll {
                cursor: cursor.clone(),
            })
            .unwrap();
        if wanted(&batch.items) {
            println!("listing caught up after {} s", attempt * 2);
            return batch;
        }
        sleep(Duration::from_secs(2));
    }
    panic!(
        "the listing did not show the change in {} s",
        LISTING_ATTEMPTS * 2
    );
}

impl Drop for Live {
    fn drop(&mut self) {
        // No panic here: it would abort a test already failing, and leave the rest open.
        for iid in std::mem::take(&mut self.opened) {
            let url = format!(
                "{}/projects/{}/issues/{iid}",
                self.api,
                encoded(&self.project)
            );
            let _ = self
                .agent
                .put(&url)
                .header("PRIVATE-TOKEN", &self.token)
                .send_json(json!({ "state_event": "close" }));
        }
    }
}

fn find<'a>(items: &'a [SourceItem], id: &str) -> Option<&'a SourceItem> {
    items.iter().find(|item| item.id == id)
}

#[test]
#[ignore = "calls a real GitLab: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn a_labelled_issue_is_polled_and_its_outcome_reaches_the_issue() {
    let mut live = Live::from_env();
    let labelled = live.open("labelled", Some(NON_ASCII), &["ai6:ready"]);
    let no_body = live.open("labelled, no body", None, &["ai6:ready"]);
    let unlabelled = live.open("no label", Some("Must not be polled."), &[]);
    let provider = live.provider("");

    let (labelled_id, no_body_id) = (live.id(labelled), live.id(no_body));
    let batch = poll_until(&provider, None, |items| {
        find(items, &labelled_id).is_some() && find(items, &no_body_id).is_some()
    });
    let item = find(&batch.items, &labelled_id).expect("the labelled issue is polled");
    assert!(item.title.starts_with("AI6 test data: labelled"));
    assert_eq!(item.body, NON_ASCII, "the text survives byte for byte");
    assert_eq!(item.labels, ["ai6:ready"]);
    assert_eq!(item.code_tag.as_deref(), Some("gl"));
    assert_eq!(item.code.as_deref(), Some(labelled.to_string().as_str()));
    assert!(
        item.author
            .as_deref()
            .is_some_and(|username| !username.is_empty())
    );
    let url = item.url.as_deref().unwrap();
    assert!(url.ends_with(&format!("/{labelled}")), "{url}");
    assert_eq!(item.updated.len(), "2026-10-02T09:30:00.000Z".len());
    let silent = find(&batch.items, &no_body_id).expect("no body is still an item");
    assert_eq!(silent.body, "");
    assert!(find(&batch.items, &live.id(unlabelled)).is_none());

    let newest = batch.cursor.clone().unwrap();
    let again = live
        .provider("settling_secs = 0")
        .poll(SourcePoll {
            cursor: Some(newest.clone()),
        })
        .unwrap();
    assert!(
        again.items.iter().any(|item| item.updated == newest),
        "updated_after drops the fraction and is inclusive, so the newest item repeats"
    );

    let text = "AI6 test data: the report. ñandú 🧪\nBranch: `agent/live-test`";
    live.finish(&provider, labelled, "ai6:done", text);
    let comments = live.comments(labelled);
    assert_eq!(comments.len(), 1);
    assert_eq!(
        comments[0]["body"],
        format!("{text}\n\n<!-- ai6:comment -->")
    );
    let labels = live.labels(labelled);
    assert!(labels.contains(&"ai6:done".to_owned()), "{labels:?}");
    assert!(!labels.contains(&"ai6:blocked".to_owned()), "{labels:?}");

    live.finish(
        &provider,
        no_body,
        "ai6:blocked",
        "AI6 test data: it failed.",
    );
    assert!(live.labels(no_body).contains(&"ai6:blocked".to_owned()));

    let moved = poll_until(&provider, Some(newest), |items| {
        find(items, &labelled_id)
            .is_some_and(|item| item.labels.iter().any(|label| label == "ai6:done"))
    });
    assert!(
        find(&moved.items, &no_body_id).is_some(),
        "within the margin, the next poll sees the issue again"
    );

    live.close(labelled);
    poll_until(&provider, None, |items| find(items, &labelled_id).is_none());
}

#[test]
#[ignore = "calls a real GitLab: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn a_small_page_size_follows_the_real_link_header_to_the_same_items() {
    let mut live = Live::from_env();
    let first = live.open("page one", Some("One."), &["ai6:ready"]);
    let second = live.open("page two", Some("Two."), &["ai6:ready"]);

    let (first_id, second_id) = (live.id(first), live.id(second));
    let whole = poll_until(&live.provider(""), None, |items| {
        find(items, &first_id).is_some() && find(items, &second_id).is_some()
    });
    let paged = live
        .provider("page_size = 1\nmax_pages = 100")
        .poll(SourcePoll { cursor: None })
        .unwrap();
    let ids = |items: &[SourceItem]| items.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
    assert!(paged.items.len() >= 2);
    assert_eq!(ids(&paged.items), ids(&whole.items));
}

#[test]
#[ignore = "calls a real GitLab: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn a_false_or_missing_token_is_an_error_naming_the_variable() {
    let live = Live::from_env();
    let settings = |token_env: &str| {
        Gitlab::from_settings(&format!(
            "project = \"{}\"\ntoken_env = \"{token_env}\"\napi_url = \"{}\"",
            live.project, live.api
        ))
        .unwrap()
    };
    let refused = settings(FALSE_TOKEN_VAR)
        .poll(SourcePoll { cursor: None })
        .unwrap_err()
        .to_string();
    assert!(refused.contains(FALSE_TOKEN_VAR), "{refused}");
    assert!(!refused.contains("not-a-token"), "{refused}");

    let unset = "AI6_TEST_LIVE_GITLAB_TOKEN_UNSET";
    let absent = settings(unset)
        .poll(SourcePoll { cursor: None })
        .unwrap_err()
        .to_string();
    assert!(absent.contains(unset), "{absent}");
}

#[test]
#[ignore = "calls a real GitLab: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn every_tracker_write_reaches_the_issue() {
    let mut live = Live::from_env();
    let iid = live.open("tracker writes", Some(NON_ASCII), &[]);
    let other = live.open("tracker link target", None, &[]);
    let item = live.id(iid);
    let provider = live.provider("");
    let me = live.request("GET", "/user", None)["username"]
        .as_str()
        .unwrap()
        .to_owned();

    let capabilities = provider.capabilities(Some(item.clone())).unwrap();
    assert!(capabilities.comment && capabilities.edit_comment && capabilities.markers);
    assert_eq!(capabilities.children, "subtasks");
    assert_eq!(capabilities.links, ["issue_links"]);
    assert_eq!(capabilities.visibility, "private");
    assert!(capabilities.transitions.is_empty());

    let read = provider.issue(item.clone()).unwrap();
    assert_eq!(read.body, NON_ASCII);
    assert_eq!(read.state, "open");
    assert_eq!(read.author_id.as_deref(), Some(me.as_str()));

    let before = provider
        .replies(ReplyPoll {
            items: vec![item.clone()],
            since: None,
        })
        .unwrap();
    assert!(before.replies.is_empty(), "a new issue has no replies");

    let posted = provider
        .post(TrackerPost {
            item: item.clone(),
            body: "AI6 test data: started. ñandú 🧪".to_owned(),
            edit: None,
            marker: Some("status".to_owned()),
        })
        .unwrap();
    assert!(
        posted
            .url
            .as_deref()
            .is_some_and(|url| url.ends_with(&format!("/-/issues/{iid}#note_{}", posted.id))),
        "{:?}",
        posted.url
    );
    let edited = provider
        .post(TrackerPost {
            item: item.clone(),
            body: "AI6 test data: waiting.".to_owned(),
            edit: Some(posted.id.clone()),
            marker: Some("status".to_owned()),
        })
        .unwrap();
    assert_eq!(edited.id, posted.id, "the same comment, edited in place");
    let comments = live.comments(iid);
    assert_eq!(comments.len(), 1, "edited, not posted again");
    assert_eq!(
        comments[0]["body"],
        "AI6 test data: waiting.\n\n<!-- ai6:status -->"
    );

    let blank = provider
        .post(TrackerPost {
            item: item.clone(),
            body: "  \n".to_owned(),
            ..TrackerPost::default()
        })
        .unwrap_err();
    assert!(blank.to_string().contains("empty"), "{blank}");

    let replies = provider
        .replies(ReplyPoll {
            items: vec![item.clone()],
            since: None,
        })
        .unwrap();
    let reply = replies
        .replies
        .iter()
        .find(|reply| reply.id == posted.id)
        .unwrap();
    assert!(reply.own, "the provider's own comment");
    assert!(reply.permitted, "the issue's author");
    let cursor = replies.cursor.clone();
    assert!(cursor.is_some());
    let unmarked = provider
        .post(TrackerPost {
            item: item.clone(),
            body: "AI6 test data: no marker.".to_owned(),
            ..TrackerPost::default()
        })
        .unwrap();
    let person = live.request(
        "POST",
        &format!("/issues/{iid}/notes"),
        Some(json!({ "body": "AI6 test data: a person's reply on the token's account." })),
    )["id"]
        .to_string();
    let replies = provider
        .replies(ReplyPoll {
            items: vec![item.clone()],
            since: cursor,
        })
        .unwrap();
    let ids: Vec<&str> = replies
        .replies
        .iter()
        .map(|reply| reply.id.as_str())
        .collect();
    assert!(
        ids.ends_with(&[unmarked.id.as_str(), person.as_str()]),
        "only what came after the cursor, oldest first: {ids:?}"
    );
    let summary: Vec<(bool, bool)> = replies.replies[ids.len() - 2..]
        .iter()
        .map(|reply| (reply.own, reply.permitted))
        .collect();
    assert_eq!(
        summary,
        [(true, true), (false, true)],
        "a post with no marker named still carries one; a person's comment on the same account does not"
    );

    provider
        .update(TrackerUpdate {
            item: item.clone(),
            add_labels: vec!["ai6:working".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap();
    provider
        .update(TrackerUpdate {
            item: item.clone(),
            add_labels: vec!["ai6:blocked".to_owned()],
            remove_labels: vec!["ai6:working".to_owned(), "ai6:never-on-it".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap();
    assert_eq!(
        provider.issue(item.clone()).unwrap().labels,
        ["ai6:blocked"]
    );
    provider
        .update(TrackerUpdate {
            item: item.clone(),
            add_assignees: vec![me.clone()],
            ..TrackerUpdate::default()
        })
        .unwrap();
    assert_eq!(
        provider.issue(item.clone()).unwrap().assignees,
        std::slice::from_ref(&me)
    );
    provider
        .update(TrackerUpdate {
            item: item.clone(),
            remove_assignees: vec![me],
            ..TrackerUpdate::default()
        })
        .unwrap();
    assert!(provider.issue(item.clone()).unwrap().assignees.is_empty());
    let missing = provider
        .update(TrackerUpdate {
            item: live.id(999_999),
            remove_labels: vec!["ai6:blocked".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap_err();
    assert!(missing.to_string().contains("does not show"), "{missing}");

    provider
        .update(TrackerUpdate {
            item: item.clone(),
            state: Some("closed".to_owned()),
            ..TrackerUpdate::default()
        })
        .unwrap();
    let closed = live.request("GET", &format!("/issues/{iid}"), None);
    assert_eq!(closed["state"], "closed");
    provider
        .update(TrackerUpdate {
            item: item.clone(),
            state: Some("open".to_owned()),
            ..TrackerUpdate::default()
        })
        .unwrap();
    assert_eq!(provider.issue(item.clone()).unwrap().state, "open");

    let child = provider
        .create(IssueSpec {
            title: format!("AI6 test data: a child ({})", live.stamp),
            body: "A child. ñandú".to_owned(),
            labels: vec!["ai6:blocked".to_owned(), "ai6:no-such-label".to_owned()],
            parent: Some(item.clone()),
            ..IssueSpec::default()
        })
        .unwrap();
    let child_iid: u64 = child.id.rsplit('#').next().unwrap().parse().unwrap();
    live.opened.push(child_iid);
    assert_eq!(
        child.parent.as_deref(),
        Some(item.as_str()),
        "a task under it"
    );
    assert_eq!(
        child.labels,
        ["ai6:blocked"],
        "only the labels the project has"
    );
    let reread = provider.issue(child.id.clone()).unwrap();
    assert_eq!(reread.parent.as_deref(), Some(item.as_str()));
    assert_eq!(
        provider
            .capabilities(Some(child.id.clone()))
            .unwrap()
            .children,
        "none",
        "a task takes no children"
    );
    assert!(
        live.provider("")
            .capabilities(None)
            .unwrap()
            .labels
            .iter()
            .all(|label| label != "ai6:no-such-label"),
        "creating does not make a label"
    );

    let error = provider.issue("someone/else#1".to_owned()).unwrap_err();
    assert_eq!(error.status, Status::GUATIAO_ERR_BAD_VALUE);
    assert!(error.to_string().contains("foreign item"), "{error}");
    let link = TrackerLink {
        from: item.clone(),
        to: Some(live.id(other)),
        relation: "related".to_owned(),
        url: None,
    };
    provider.link(link.clone()).unwrap();
    provider.link(link).unwrap();
    let links = live.request("GET", &format!("/issues/{iid}/links"), None);
    let linked: Vec<u64> = links
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|issue| issue["iid"].as_u64())
        .collect();
    assert_eq!(linked, [other], "linked once, as `relates_to`");
    let error = provider
        .link(TrackerLink {
            from: item.clone(),
            to: None,
            relation: "mirror".to_owned(),
            url: Some("https://example.com/x".to_owned()),
        })
        .unwrap_err();
    assert_eq!(error.status, Status::GUATIAO_ERR_UNSUPPORTED);
    let error = provider
        .update(TrackerUpdate {
            item,
            transition: Some("Done".to_owned()),
            ..TrackerUpdate::default()
        })
        .unwrap_err();
    assert_eq!(error.status, Status::GUATIAO_ERR_UNSUPPORTED);
}
