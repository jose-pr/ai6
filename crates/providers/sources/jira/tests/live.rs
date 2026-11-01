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

use ai6::{CxReport, Source, SourceItem, SourcePoll};
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

    fn report(&self, key: &str, outcome: &str, body: &str) -> CxReport {
        CxReport {
            item_id: key.to_owned(),
            mission_id: format!("m-{}", self.stamp),
            outcome: outcome.to_owned(),
            body: body.to_owned(),
            branch: None,
        }
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
    let provider = live.provider(&format!(
        "done_transition = \"{}\"\nfailed_transition = \"{}\"",
        in_progress.name.to_uppercase(),
        done.id,
    ));

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
    provider.report(live.report(&rich, "done", text)).unwrap();
    assert_eq!(live.status(&rich), in_progress.status);
    let comments = live.get(&format!("/issue/{rich}/comment"));
    assert_eq!(comments["comments"].as_array().unwrap().len(), 1);
    let stored = &comments["comments"][0]["body"]["content"];
    assert_eq!(
        stored[0]["content"][0]["text"],
        "AI6 test data: the report. ñandú 🧪"
    );
    assert_eq!(stored[2]["content"][0]["text"], "Branch: `agent/live-test`");

    provider
        .report(live.report(&plain, "failed", "AI6 test data: it failed."))
        .unwrap();
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
