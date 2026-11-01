//! Runs the provider against the real GitHub, on a repository set aside for it. Every test is
//! ignored unless asked for, and fails rather than skips when asked for without its settings:
//!
//! ```text
//! AI6_TEST_LIVE_SOURCES=1 AI6_TEST_LIVE_GITHUB_REPO=owner/name AI6_TEST_LIVE_GITHUB_TOKEN=... \
//!   cargo test -p ai6-source-github --test live -- --ignored --test-threads=1
//! ```
//!
//! The token needs Issues read and write on that repository. Each test opens issues titled as
//! test data and closes them when it ends, passed or not.
//! `AI6_TEST_LIVE_GITHUB_API_URL` points the calls at a recording proxy.

use std::sync::Once;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ai6::{CxReport, Source, SourceBatch, SourceItem, SourcePoll};
use ai6_source_github::Github;
use serde_json::{Value, json};

const TOKEN_VAR: &str = "AI6_TEST_LIVE_GITHUB_TOKEN";
const FALSE_TOKEN_VAR: &str = "AI6_TEST_LIVE_GITHUB_FALSE_TOKEN";
/// How many seconds a change may take to show in the listing.
const LISTING_ATTEMPTS: u32 = 60;
const NON_ASCII: &str = "Añadir pruebas: ñandú, café.\n\n日本語, Ελληνικά, العربية 🧪";

static INIT: Once = Once::new();

/// One repository under test, and the issues to close when the test ends.
struct Live {
    repo: String,
    api: String,
    token: String,
    agent: ureq::Agent,
    stamp: u128,
    opened: Vec<u64>,
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("this test needs {name}: see tests/live.rs"))
}

impl Live {
    fn from_env() -> Self {
        required("AI6_TEST_LIVE_SOURCES");
        INIT.call_once(|| {
            // SAFETY: runs once, and every test calls this before it reads the environment.
            unsafe { std::env::set_var(FALSE_TOKEN_VAR, "not-a-token") };
        });
        Self {
            repo: required("AI6_TEST_LIVE_GITHUB_REPO"),
            api: std::env::var("AI6_TEST_LIVE_GITHUB_API_URL")
                .unwrap_or_else(|_| "https://api.github.com".to_owned()),
            token: required(TOKEN_VAR),
            agent: ureq::Agent::new_with_defaults(),
            stamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            opened: Vec::new(),
        }
    }

    fn provider(&self, extra: &str) -> Github {
        Github::from_settings(&format!(
            "repo = \"{}\"\ntoken_env = \"{TOKEN_VAR}\"\napi_url = \"{}\"\n{extra}",
            self.repo, self.api
        ))
        .unwrap()
    }

    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let url = format!("{}/repos/{}{path}", self.api, self.repo);
        let bearer = format!("Bearer {}", self.token);
        let headers = |request: ureq::RequestBuilder<ureq::typestate::WithBody>| {
            request
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "ai6-source-github-live-test")
                .header("Authorization", &bearer)
        };
        let mut response = match method {
            "GET" => self
                .agent
                .get(&url)
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "ai6-source-github-live-test")
                .header("Authorization", &bearer)
                .call(),
            "POST" => headers(self.agent.post(&url)).send_json(body.unwrap()),
            "PATCH" => headers(self.agent.patch(&url)).send_json(body.unwrap()),
            other => panic!("unexpected method {other}"),
        }
        .unwrap_or_else(|error| panic!("{method} {path}: {error}"));
        response.body_mut().read_json().unwrap()
    }

    fn open(&mut self, title: &str, body: Option<&str>, labels: &[&str]) -> u64 {
        let mut issue = json!({
            "title": format!("AI6 test data: {title} ({})", self.stamp),
            "labels": labels,
        });
        if let Some(body) = body {
            issue["body"] = json!(body);
        }
        let number = self.request("POST", "/issues", Some(issue))["number"]
            .as_u64()
            .unwrap();
        self.opened.push(number);
        number
    }

    fn id(&self, number: u64) -> String {
        format!("{}#{number}", self.repo)
    }

    fn close(&mut self, number: u64) {
        self.request(
            "PATCH",
            &format!("/issues/{number}"),
            Some(json!({ "state": "closed" })),
        );
        self.opened.retain(|opened| *opened != number);
    }

    fn report(&self, number: u64, outcome: &str, body: &str) -> CxReport {
        CxReport {
            item_id: self.id(number),
            mission_id: format!("m-{}", self.stamp),
            outcome: outcome.to_owned(),
            body: body.to_owned(),
            branch: Some("agent/live-test".to_owned()),
        }
    }
}

/// Polls until `wanted` holds, since a change reaches the issue listing some time later.
fn poll_until(
    provider: &Github,
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
            println!("listing caught up after {attempt} s");
            return batch;
        }
        sleep(Duration::from_secs(1));
    }
    panic!("the listing did not show the change in {LISTING_ATTEMPTS} s");
}

impl Drop for Live {
    fn drop(&mut self) {
        // No panic here: it would abort a test already failing, and leave the rest open.
        for number in std::mem::take(&mut self.opened) {
            let url = format!("{}/repos/{}/issues/{number}", self.api, self.repo);
            let _ = self
                .agent
                .patch(&url)
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "ai6-source-github-live-test")
                .header("Authorization", &format!("Bearer {}", self.token))
                .send_json(json!({ "state": "closed" }));
        }
    }
}

fn find<'a>(items: &'a [SourceItem], id: &str) -> Option<&'a SourceItem> {
    items.iter().find(|item| item.id == id)
}

#[test]
#[ignore = "calls the real GitHub: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn a_labelled_issue_is_polled_and_its_report_reaches_the_issue() {
    let mut live = Live::from_env();
    let labelled = live.open("labelled", Some(NON_ASCII), &["ai6:ready"]);
    let no_body = live.open("labelled, no body", None, &["ai6:ready"]);
    let unlabelled = live.open("no label", Some("Must not be polled."), &[]);
    let provider = live.provider("done_label = \"ai6:done\"\nfailed_label = \"ai6:failed\"");

    let (labelled_id, no_body_id) = (live.id(labelled), live.id(no_body));
    let batch = poll_until(&provider, None, |items| {
        find(items, &labelled_id).is_some() && find(items, &no_body_id).is_some()
    });
    let item = find(&batch.items, &labelled_id).expect("the labelled issue is polled");
    assert!(item.title.starts_with("AI6 test data: labelled"));
    assert_eq!(item.body, NON_ASCII, "the text survives byte for byte");
    assert_eq!(item.labels, ["ai6:ready"]);
    assert!(
        item.author
            .as_deref()
            .is_some_and(|login| !login.is_empty())
    );
    let url = item.url.as_deref().unwrap();
    assert!(url.ends_with(&format!("/issues/{labelled}")), "{url}");
    assert_eq!(item.updated.len(), "2026-10-02T09:30:00Z".len());
    let silent = find(&batch.items, &no_body_id).expect("no body is still an item");
    assert_eq!(silent.body, "");
    assert!(find(&batch.items, &live.id(unlabelled)).is_none());

    let newest = batch.cursor.clone().unwrap();
    let again = provider
        .poll(SourcePoll {
            cursor: Some(newest.clone()),
        })
        .unwrap();
    assert!(
        again.items.iter().any(|item| item.updated == newest),
        "since is inclusive, so the newest item repeats"
    );

    let text = "AI6 test data: the report. ñandú 🧪\nBranch: `agent/live-test`";
    provider
        .report(live.report(labelled, "done", text))
        .unwrap();
    let comments = live.request("GET", &format!("/issues/{labelled}/comments"), None);
    assert_eq!(comments.as_array().unwrap().len(), 1);
    assert_eq!(comments[0]["body"], text);
    let issue = live.request("GET", &format!("/issues/{labelled}"), None);
    let labels: Vec<&str> = issue["labels"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|label| label["name"].as_str())
        .collect();
    assert!(labels.contains(&"ai6:done"), "{labels:?}");
    assert!(!labels.contains(&"ai6:failed"), "{labels:?}");

    provider
        .report(live.report(no_body, "failed", "AI6 test data: it failed."))
        .unwrap();
    let issue = live.request("GET", &format!("/issues/{no_body}"), None);
    assert!(
        issue["labels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|label| label["name"] == "ai6:failed"),
        "{}",
        issue["labels"]
    );

    let moved = poll_until(&provider, Some(newest), |items| {
        find(items, &labelled_id)
            .is_some_and(|item| item.labels.iter().any(|label| label == "ai6:done"))
    });
    assert!(
        find(&moved.items, &no_body_id).is_some(),
        "a report moves updated_at, so the next poll sees the issue again"
    );

    live.close(labelled);
    poll_until(&provider, None, |items| find(items, &labelled_id).is_none());
}

#[test]
#[ignore = "calls the real GitHub: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
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
#[ignore = "calls the real GitHub: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn a_false_or_missing_token_is_an_error_naming_the_variable() {
    let live = Live::from_env();
    let settings = |token_env: &str| {
        Github::from_settings(&format!(
            "repo = \"{}\"\ntoken_env = \"{token_env}\"\napi_url = \"{}\"",
            live.repo, live.api
        ))
        .unwrap()
    };
    let refused = settings(FALSE_TOKEN_VAR)
        .poll(SourcePoll { cursor: None })
        .unwrap_err()
        .to_string();
    assert!(refused.contains(FALSE_TOKEN_VAR), "{refused}");
    assert!(!refused.contains("not-a-token"), "{refused}");

    let unset = "AI6_TEST_LIVE_GITHUB_TOKEN_UNSET";
    let absent = settings(unset)
        .poll(SourcePoll { cursor: None })
        .unwrap_err()
        .to_string();
    assert!(absent.contains(unset), "{absent}");
}
