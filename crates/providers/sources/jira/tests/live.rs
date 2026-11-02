//! Runs the provider against a real Jira Cloud site, in a project set aside for it. Every test
//! is ignored unless asked for, and fails rather than skips when asked for without its settings:
//!
//! ```text
//! AI6_TEST_LIVE_SOURCES=1 AI6_TEST_LIVE_JIRA_SITE=https://example.atlassian.net \
//! AI6_TEST_LIVE_JIRA_PROJECT=KEY AI6_TEST_LIVE_JIRA_EMAIL=... AI6_TEST_LIVE_JIRA_TOKEN=... \
//!   cargo test -p ai6-source-jira --test live -- --ignored --test-threads=1
//! ```
//!
//! The project needs the issue types Task, Bug and Story and a workflow with an in-progress and
//! a done status. Each test creates issues titled as test data and moves them to a done status
//! when it ends, passed or not. Its calls go only to that project.
//! `AI6_TEST_LIVE_JIRA_API_URL` points the calls at a recording proxy.

use std::sync::Once;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ai6::guatiao::Status;
use ai6::{
    IssueSpec, ReplyPoll, Source, SourceItem, SourcePoll, Tracker, TrackerLink, TrackerPost,
    TrackerUpdate,
};
use ai6_source_jira::Jira;
use data_encoding::BASE64;
use serde_json::{Value, json};

const EMAIL_VAR: &str = "AI6_TEST_LIVE_JIRA_EMAIL";
const TOKEN_VAR: &str = "AI6_TEST_LIVE_JIRA_TOKEN";
const FALSE_EMAIL_VAR: &str = "AI6_TEST_LIVE_JIRA_FALSE_EMAIL";
const FALSE_TOKEN_VAR: &str = "AI6_TEST_LIVE_JIRA_FALSE_TOKEN";
/// How many seconds a new issue may take to reach the search index.
const SEARCH_ATTEMPTS: u32 = 60;
const NON_ASCII: &str = "Añadir pruebas: ñandú, café.";
const NON_ASCII_2: &str = "日本語, Ελληνικά, العربية 🧪";

static INIT: Once = Once::new();

/// A workflow transition found on an issue.
struct Target {
    id: String,
    name: String,
    status: String,
}

struct Live {
    site: String,
    project: String,
    authorization: String,
    agent: ureq::Agent,
    stamp: u128,
    opened: Vec<String>,
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("this test needs {name}: see tests/live.rs"))
}

fn text_document(lines: &[&str]) -> Value {
    let content: Vec<Value> = lines
        .iter()
        .map(|line| json!({ "type": "paragraph", "content": [{ "type": "text", "text": line }] }))
        .collect();
    json!({ "type": "doc", "version": 1, "content": content })
}

impl Live {
    fn from_env() -> Self {
        required("AI6_TEST_LIVE_SOURCES");
        INIT.call_once(|| {
            // SAFETY: runs once, and every test calls this before it reads the environment.
            unsafe {
                std::env::set_var(FALSE_EMAIL_VAR, "nobody@example.invalid");
                std::env::set_var(FALSE_TOKEN_VAR, "not-a-token");
            }
        });
        let pair = format!("{}:{}", required(EMAIL_VAR), required(TOKEN_VAR));
        Self {
            site: std::env::var("AI6_TEST_LIVE_JIRA_API_URL")
                .unwrap_or_else(|_| required("AI6_TEST_LIVE_JIRA_SITE")),
            project: required("AI6_TEST_LIVE_JIRA_PROJECT"),
            authorization: format!("Basic {}", BASE64.encode(pair.as_bytes())),
            agent: ureq::Agent::new_with_defaults(),
            stamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            opened: Vec::new(),
        }
    }

    fn provider(&self, extra: &str) -> Jira {
        Jira::from_settings(&format!(
            "site = \"{}\"\nproject = \"{}\"\nemail_env = \"{EMAIL_VAR}\"\n\
             token_env = \"{TOKEN_VAR}\"\n{extra}",
            self.site, self.project
        ))
        .unwrap()
    }

    fn try_get(&self, path: &str) -> Result<Value, ureq::Error> {
        let url = format!("{}/rest/api/3{path}", self.site);
        self.agent
            .get(&url)
            .header("Accept", "application/json")
            .header("Authorization", &self.authorization)
            .call()?
            .body_mut()
            .read_json()
    }

    fn try_post(&self, path: &str, body: Value) -> Result<String, ureq::Error> {
        let url = format!("{}/rest/api/3{path}", self.site);
        self.agent
            .post(&url)
            .header("Accept", "application/json")
            .header("Authorization", &self.authorization)
            .send_json(body)?
            .body_mut()
            .read_to_string()
    }

    fn get(&self, path: &str) -> Value {
        self.try_get(path)
            .unwrap_or_else(|error| panic!("GET {path}: {error}"))
    }

    fn post(&self, path: &str, body: Value) -> String {
        self.try_post(path, body)
            .unwrap_or_else(|error| panic!("POST {path}: {error}"))
    }

    fn create(
        &mut self,
        issue_type: &str,
        title: &str,
        doc: Option<Value>,
        labels: &[&str],
    ) -> String {
        let mut fields = json!({
            "project": { "key": self.project },
            "issuetype": { "name": issue_type },
            "summary": format!("AI6 test data: {title} ({})", self.stamp),
            "labels": labels,
        });
        if let Some(doc) = doc {
            fields["description"] = doc;
        }
        let created: Value =
            serde_json::from_str(&self.post("/issue", json!({ "fields": fields }))).unwrap();
        let key = created["key"].as_str().unwrap().to_owned();
        self.opened.push(key.clone());
        key
    }

    /// The transition out of `key` into a status of this category.
    fn transition_into(&self, key: &str, category: &str) -> Result<Target, ureq::Error> {
        let listing = self.try_get(&format!("/issue/{key}/transitions"))?;
        let transition = listing["transitions"]
            .as_array()
            .and_then(|all| {
                all.iter()
                    .find(|one| one["to"]["statusCategory"]["key"] == category)
            })
            .unwrap_or_else(|| panic!("{key} has no transition into {category}"));
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        Ok(Target {
            id: text(&transition["id"]),
            name: text(&transition["name"]),
            status: text(&transition["to"]["name"]),
        })
    }

    fn status(&self, key: &str) -> String {
        self.get(&format!("/issue/{key}?fields=status"))["fields"]["status"]["name"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// What a finished Mission writes: a comment, then the transition of its outcome.
    fn finish(&self, provider: &Jira, key: &str, transition: &str, body: &str) {
        provider
            .post(TrackerPost {
                item: key.to_owned(),
                body: body.to_owned(),
                ..TrackerPost::default()
            })
            .unwrap();
        provider
            .update(TrackerUpdate {
                item: key.to_owned(),
                transition: Some(transition.to_owned()),
                ..TrackerUpdate::default()
            })
            .unwrap();
    }

    /// Polls until `wanted` holds, since a change reaches Jira's search index some time later.
    fn poll_until(
        &self,
        provider: &Jira,
        cursor: Option<String>,
        wanted: impl Fn(&[SourceItem]) -> bool,
    ) -> Vec<SourceItem> {
        for attempt in 0..SEARCH_ATTEMPTS {
            let batch = provider
                .poll(SourcePoll {
                    cursor: cursor.clone(),
                })
                .unwrap();
            if wanted(&batch.items) {
                println!("search caught up after {attempt} s");
                return batch.items;
            }
            sleep(Duration::from_secs(1));
        }
        panic!("the search did not show the change in {SEARCH_ATTEMPTS} s");
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // No panic on a call: it would abort a test already failing, and leave the rest open.
        for key in std::mem::take(&mut self.opened) {
            if let Ok(done) = self.transition_into(&key, "done") {
                let _ = self.try_post(
                    &format!("/issue/{key}/transitions"),
                    json!({ "transition": { "id": done.id } }),
                );
            }
        }
    }
}

fn has(items: &[SourceItem], key: &str) -> bool {
    items.iter().any(|item| item.id == key)
}

#[test]
#[ignore = "calls a real Jira site: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn a_labelled_issue_is_polled_commented_on_and_transitioned() {
    let mut live = Live::from_env();
    let rich = live.create(
        "Task",
        "labelled",
        Some(text_document(&[NON_ASCII, NON_ASCII_2])),
        &["ai6-ready"],
    );
    let plain = live.create("Bug", "labelled, no description", None, &["ai6-ready"]);
    let unlabelled = live.create("Story", "no label", None, &[]);

    let in_progress = live.transition_into(&rich, "indeterminate").unwrap();
    let done = live.transition_into(&plain, "done").unwrap();
    let provider = live.provider("");

    let items = live.poll_until(&provider, None, |items| {
        has(items, &rich) && has(items, &plain)
    });
    let item = items.iter().find(|item| item.id == rich).unwrap();
    assert!(item.title.starts_with("AI6 test data: labelled"));
    assert_eq!(item.body, format!("{NON_ASCII}\n{NON_ASCII_2}"));
    assert_eq!(item.labels, ["ai6-ready"]);
    assert!(item.author.as_deref().is_some_and(|name| !name.is_empty()));
    assert!(
        item.url
            .as_deref()
            .unwrap()
            .ends_with(&format!("/browse/{rich}"))
    );
    let silent = items.iter().find(|item| item.id == plain).unwrap();
    assert_eq!(silent.body, "");
    assert!(!has(&items, &unlabelled));

    let cursor = items.iter().map(|item| item.updated.clone()).max().unwrap();
    let newest = cursor.clone();
    let again = live.poll_until(&provider, Some(cursor.clone()), |items| {
        items.iter().any(|item| item.updated == newest)
    });
    assert!(
        !again.is_empty(),
        "the minute of the cursor is searched again"
    );

    let text = "AI6 test data: the report. ñandú 🧪\n\nBranch: `agent/live-test`";
    live.finish(&provider, &rich, &in_progress.name.to_uppercase(), text);
    assert_eq!(live.status(&rich), in_progress.status);
    let comments = live.get(&format!("/issue/{rich}/comment"));
    assert_eq!(comments["comments"].as_array().unwrap().len(), 1);
    let stored = &comments["comments"][0]["body"]["content"];
    assert_eq!(
        stored[0]["content"][0]["text"],
        "AI6 test data: the report. ñandú 🧪"
    );
    assert_eq!(stored[2]["content"][0]["text"], "Branch: `agent/live-test`");

    live.finish(&provider, &plain, &done.id, "AI6 test data: it failed.");
    assert_eq!(live.status(&plain), done.status, "matched by transition ID");
    let without = live.poll_until(&provider, None, |items| !has(items, &plain));
    assert!(has(&without, &rich), "a done issue leaves the search");
}

#[test]
#[ignore = "calls a real Jira site: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn a_small_page_size_follows_the_real_page_token_to_the_same_items() {
    let mut live = Live::from_env();
    let one = live.create("Task", "page one", None, &["ai6-ready"]);
    let two = live.create("Task", "page two", None, &["ai6-ready"]);
    let whole = live.poll_until(&live.provider(""), None, |items| {
        has(items, &one) && has(items, &two)
    });
    let paged = live
        .provider("page_size = 1\nmax_pages = 100")
        .poll(SourcePoll { cursor: None })
        .unwrap();
    assert_eq!(
        paged.items.iter().map(|i| &i.id).collect::<Vec<_>>(),
        whole.iter().map(|i| &i.id).collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "calls a real Jira site: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn a_search_that_can_see_nothing_is_an_error_not_an_empty_poll() {
    let live = Live::from_env();
    let refused = Jira::from_settings(&format!(
        "site = \"{}\"\nproject = \"{}\"\nemail_env = \"{FALSE_EMAIL_VAR}\"\n\
         token_env = \"{FALSE_TOKEN_VAR}\"",
        live.site, live.project
    ))
    .unwrap()
    .poll(SourcePoll { cursor: None })
    .unwrap_err()
    .to_string();
    assert!(refused.contains(FALSE_TOKEN_VAR), "{refused}");
    assert!(!refused.contains("not-a-token"), "{refused}");

    let refused_jql_only = Jira::from_settings(&format!(
        "site = \"{}\"\njql = \"project = {}\"\nemail_env = \"{FALSE_EMAIL_VAR}\"\n\
         token_env = \"{FALSE_TOKEN_VAR}\"",
        live.site, live.project
    ))
    .unwrap()
    .poll(SourcePoll { cursor: None })
    .unwrap_err()
    .to_string();
    assert!(
        refused_jql_only.contains(FALSE_TOKEN_VAR),
        "{refused_jql_only}"
    );

    let unknown = Jira::from_settings(&format!(
        "site = \"{}\"\nproject = \"NOSUCHPROJECT\"\nemail_env = \"{EMAIL_VAR}\"\n\
         token_env = \"{TOKEN_VAR}\"",
        live.site
    ))
    .unwrap()
    .poll(SourcePoll { cursor: None })
    .unwrap_err()
    .to_string();
    assert!(unknown.contains("NOSUCHPROJECT"), "{unknown}");

    let rejected = live
        .provider("jql = \"AND AND\"")
        .poll(SourcePoll { cursor: None })
        .unwrap_err()
        .to_string();
    assert!(rejected.contains("rejected the search"), "{rejected}");

    let empty = live
        .provider("labels = [\"ai6-no-issue-has-this-label\"]")
        .poll(SourcePoll { cursor: None })
        .unwrap();
    assert!(empty.items.is_empty(), "a search with no match is no error");
}

#[test]
#[ignore = "calls a real Jira site: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn every_tracker_write_reaches_the_issue() {
    let mut live = Live::from_env();
    let key = live.create(
        "Task",
        "tracker writes",
        Some(text_document(&[NON_ASCII])),
        &[],
    );
    let provider = live.provider("");

    let capabilities = provider.capabilities(Some(key.clone())).unwrap();
    assert!(capabilities.comment && capabilities.edit_comment && capabilities.create);
    assert_eq!(capabilities.children, "parent");
    assert!(capabilities.issue_types.iter().any(|name| name == "Task"));
    assert!(capabilities.priorities.iter().any(|name| name == "Medium"));
    assert!(!capabilities.transitions.is_empty());
    assert!(capabilities.markers);

    let read = provider.issue(key.clone()).unwrap();
    assert_eq!(read.body, NON_ASCII);
    assert_eq!(read.state, "open");
    assert!(
        read.url
            .as_deref()
            .is_some_and(|url| url.ends_with(&format!("/browse/{key}")))
    );

    let markdown = "# AI6 test data\n\nA [link](https://example.com/x) and text.\n\n- one\n- two\n\n1. first\n\n```rust\nlet x = 1;\n```";
    let posted = provider
        .post(TrackerPost {
            item: key.clone(),
            body: markdown.to_owned(),
            edit: None,
            marker: Some("status".to_owned()),
        })
        .unwrap();
    let stored = live.get(&format!("/issue/{key}/comment/{}", posted.id));
    let kinds: Vec<&str> = stored["body"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|block| block["type"].as_str())
        .collect();
    for kind in ["heading", "bulletList", "orderedList", "codeBlock"] {
        assert!(kinds.contains(&kind), "{kind} in {kinds:?}");
    }
    let edited = provider
        .post(TrackerPost {
            item: key.clone(),
            body: "AI6 test data: waiting.".to_owned(),
            edit: Some(posted.id.clone()),
            marker: Some("status".to_owned()),
        })
        .unwrap();
    assert_eq!(edited.id, posted.id, "the same comment, edited in place");
    let comments = live.get(&format!("/issue/{key}/comment"));
    assert_eq!(comments["comments"].as_array().unwrap().len(), 1);

    let blank = provider
        .post(TrackerPost {
            item: key.clone(),
            body: " \n ".to_owned(),
            ..TrackerPost::default()
        })
        .unwrap_err();
    assert!(blank.to_string().contains("empty"), "{blank}");

    let replies = provider
        .replies(ReplyPoll {
            items: vec![key.clone()],
            since: None,
        })
        .unwrap();
    assert_eq!(replies.replies.len(), 1);
    let reply = &replies.replies[0];
    assert!(reply.own, "a comment AI6 wrote");
    assert!(reply.permitted, "the issue's reporter");
    assert_eq!(reply.body, "AI6 test data: waiting.");
    let cursor = replies.cursor.clone().expect("a cursor");
    let later = provider
        .replies(ReplyPoll {
            items: vec![key.clone()],
            since: Some(cursor),
        })
        .unwrap();
    assert!(later.replies.is_empty(), "nothing after the cursor");
    // The same account, through Jira's API with no property: what a person's comment looks like.
    let person: Value = serde_json::from_str(&live.post(
        &format!("/issue/{key}/comment"),
        json!({"body": {"type": "doc", "version": 1, "content": [{"type": "paragraph",
            "content": [{"type": "text", "text": "AI6 test data: a person's reply."}]}]}}),
    ))
    .unwrap();
    let with_person = provider
        .replies(ReplyPoll {
            items: vec![key.clone()],
            since: None,
        })
        .unwrap();
    let reply = with_person
        .replies
        .iter()
        .find(|reply| Some(reply.id.as_str()) == person["id"].as_str())
        .expect("the person's reply is listed");
    assert!(
        !reply.own,
        "a person's comment on the credential's own account"
    );
    assert!(reply.permitted, "its author is the issue's reporter");

    provider
        .update(TrackerUpdate {
            item: key.clone(),
            add_labels: vec!["ai6-working".to_owned(), "two words".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap();
    provider
        .update(TrackerUpdate {
            item: key.clone(),
            remove_labels: vec!["ai6-working".to_owned()],
            ..TrackerUpdate::default()
        })
        .unwrap();
    assert_eq!(provider.issue(key.clone()).unwrap().labels, ["two-words"]);

    let me = live.get("/myself")["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    provider
        .update(TrackerUpdate {
            item: key.clone(),
            add_assignees: vec![me.clone()],
            ..TrackerUpdate::default()
        })
        .unwrap();
    assert_eq!(
        provider.issue(key.clone()).unwrap().assignees,
        std::slice::from_ref(&me)
    );
    provider
        .update(TrackerUpdate {
            item: key.clone(),
            remove_assignees: vec![me],
            ..TrackerUpdate::default()
        })
        .unwrap();
    assert!(provider.issue(key.clone()).unwrap().assignees.is_empty());

    let in_progress = live.transition_into(&key, "indeterminate").unwrap();
    provider
        .update(TrackerUpdate {
            item: key.clone(),
            transition: Some(in_progress.name.to_lowercase()),
            ..TrackerUpdate::default()
        })
        .unwrap();
    assert_eq!(live.status(&key), in_progress.status);
    let unknown = provider
        .update(TrackerUpdate {
            item: key.clone(),
            transition: Some("Ship it".to_owned()),
            ..TrackerUpdate::default()
        })
        .unwrap_err();
    assert_eq!(unknown.status, Status::GUATIAO_ERR_UNSUPPORTED);
    assert!(unknown.to_string().contains("Ship it"), "{unknown}");
    provider
        .update(TrackerUpdate {
            item: key.clone(),
            state: Some("closed".to_owned()),
            ..TrackerUpdate::default()
        })
        .unwrap();
    assert_eq!(provider.issue(key.clone()).unwrap().state, "closed");
    provider
        .update(TrackerUpdate {
            item: key.clone(),
            state: Some("open".to_owned()),
            ..TrackerUpdate::default()
        })
        .unwrap();
    assert_eq!(provider.issue(key.clone()).unwrap().state, "open");

    let long = "ñ".repeat(300);
    let created = provider
        .create(IssueSpec {
            title: format!(
                "AI6 test data: created ({}) {long}\nsecond line",
                live.stamp
            ),
            body: "A *new* issue.\n\n- one".to_owned(),
            labels: vec!["two words".to_owned()],
            priority: Some("Medium".to_owned()),
            ..IssueSpec::default()
        })
        .unwrap();
    live.opened.push(created.id.clone());
    assert!(created.title.len() <= 255 && !created.title.contains('\n'));
    assert_eq!(created.labels, ["two-words"]);
    assert!(created.body.contains("A *new* issue."));

    let bare = provider
        .create(IssueSpec {
            title: format!("AI6 test data: no body ({})", live.stamp),
            ..IssueSpec::default()
        })
        .unwrap();
    live.opened.push(bare.id.clone());
    assert_eq!(bare.body, "");

    let child = provider
        .create(IssueSpec {
            title: format!("AI6 test data: a sub-task ({})", live.stamp),
            parent: Some(key.clone()),
            ..IssueSpec::default()
        })
        .unwrap();
    live.opened.push(child.id.clone());
    assert_eq!(child.parent.as_deref(), Some(key.as_str()));

    let epic = live.create("Epic", "an epic", None, &[]);
    let under_epic = provider
        .create(IssueSpec {
            title: format!("AI6 test data: under an epic ({})", live.stamp),
            parent: Some(epic.clone()),
            ..IssueSpec::default()
        })
        .unwrap();
    live.opened.push(under_epic.id.clone());
    assert_eq!(under_epic.parent.as_deref(), Some(epic.as_str()));
    let kind = live.get(&format!("/issue/{}?fields=issuetype", under_epic.id));
    assert_eq!(kind["fields"]["issuetype"]["name"], "Task");

    let under_subtask = provider
        .create(IssueSpec {
            title: format!("AI6 test data: under a sub-task ({})", live.stamp),
            parent: Some(child.id.clone()),
            ..IssueSpec::default()
        })
        .unwrap();
    live.opened.push(under_subtask.id.clone());
    assert_eq!(under_subtask.parent, None, "a sub-task has no children");
    let links = live.get(&format!("/issue/{}?fields=issuelinks", under_subtask.id));
    assert_eq!(links["fields"]["issuelinks"].as_array().unwrap().len(), 1);

    provider
        .link(TrackerLink {
            from: created.id.clone(),
            to: Some(key.clone()),
            relation: "related".to_owned(),
            url: None,
        })
        .unwrap();
    let links = live.get(&format!("/issue/{}?fields=issuelinks", created.id));
    assert_eq!(links["fields"]["issuelinks"].as_array().unwrap().len(), 1);
    provider
        .link(TrackerLink {
            from: created.id.clone(),
            to: None,
            relation: "mirror".to_owned(),
            url: Some("https://example.com/ai6-test-data".to_owned()),
        })
        .unwrap();
    let remote = live.get(&format!("/issue/{}/remotelink", created.id));
    assert_eq!(remote.as_array().unwrap().len(), 1);

    let foreign = provider.issue("OTHER-1".to_owned()).unwrap_err();
    assert_eq!(foreign.status, Status::GUATIAO_ERR_BAD_VALUE);
    assert!(foreign.to_string().contains("foreign item"), "{foreign}");
}

#[test]
#[ignore = "calls a real Jira site: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn a_refused_credential_on_each_tracker_call_names_the_variable() {
    let mut live = Live::from_env();
    let key = live.create("Task", "refused credential", None, &[]);
    let refused = Jira::from_settings(&format!(
        "site = \"{}\"\nproject = \"{}\"\nemail_env = \"{FALSE_EMAIL_VAR}\"\n\
         token_env = \"{FALSE_TOKEN_VAR}\"",
        live.site, live.project
    ))
    .unwrap();
    let errors = [
        refused.issue(key.clone()).unwrap_err(),
        refused
            .post(TrackerPost {
                item: key.clone(),
                body: "x".to_owned(),
                ..TrackerPost::default()
            })
            .unwrap_err(),
        refused
            .update(TrackerUpdate {
                item: key.clone(),
                state: Some("closed".to_owned()),
                ..TrackerUpdate::default()
            })
            .unwrap_err(),
        refused.capabilities(None).unwrap_err(),
        refused
            .replies(ReplyPoll {
                items: vec![key.clone()],
                since: None,
            })
            .unwrap_err(),
        refused
            .create(IssueSpec {
                title: "AI6 test data: must not exist".to_owned(),
                ..IssueSpec::default()
            })
            .unwrap_err(),
    ];
    for error in errors {
        let text = error.to_string();
        assert!(text.contains(FALSE_TOKEN_VAR), "{text}");
        assert!(!text.contains("not-a-token"), "{text}");
    }
}
