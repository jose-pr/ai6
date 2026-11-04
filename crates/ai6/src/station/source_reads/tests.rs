//! The Headquarters Station's answer to a live read, against providers that record what they
//! are asked.

use std::fs;
use std::sync::{Arc, Mutex};

use guatiao::library::ProviderError;

use super::*;
use crate::directorates::registry::sources::{Source, SourceBatch, SourceItem};
use crate::{
    IssueSpec, PostedComment, Reply, ReplyBatch, TrackerCapabilities, TrackerIssue, TrackerLink,
    TrackerPost, TrackerUpdate,
};

/// A tracker with one issue and five comments, recording the items it was asked about.
#[derive(Clone, Default)]
struct Recording {
    asked: Arc<Mutex<Vec<String>>>,
}

impl Tracker for Recording {
    fn capabilities(&self, _: Option<String>) -> Result<TrackerCapabilities, ProviderError> {
        Ok(TrackerCapabilities::default())
    }

    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError> {
        self.asked.lock().unwrap().push(item.clone());
        Ok(TrackerIssue {
            title: "Blocked on the vendor".to_owned(),
            body: "Waiting for the vendor's key.".to_owned(),
            state: "open".to_owned(),
            labels: vec!["blocked".to_owned()],
            assignees: vec!["ana".to_owned()],
            id: item,
            ..TrackerIssue::default()
        })
    }

    fn replies(&self, poll: ReplyPoll) -> Result<ReplyBatch, ProviderError> {
        assert_eq!(poll.since, None);
        let replies = (1..=5)
            .map(|n| Reply {
                id: n.to_string(),
                item: poll.items[0].clone(),
                author_id: format!("u{n}"),
                author: format!("User {n}"),
                permitted: n == 4,
                own: n == 5,
                body: format!("comment {n}"),
                created: format!("2026-11-0{n}T00:00:00Z"),
            })
            .collect();
        Ok(ReplyBatch {
            replies,
            cursor: None,
        })
    }

    fn post(&self, _: TrackerPost) -> Result<PostedComment, ProviderError> {
        unreachable!("a read posts nothing")
    }

    fn update(&self, _: TrackerUpdate) -> Result<(), ProviderError> {
        unreachable!("a read changes nothing")
    }

    fn create(&self, _: IssueSpec) -> Result<TrackerIssue, ProviderError> {
        unreachable!("a read creates nothing")
    }

    fn link(&self, _: TrackerLink) -> Result<(), ProviderError> {
        unreachable!("a read links nothing")
    }
}

/// A source that finds three items.
struct Three;

impl Source for Three {
    fn poll(&self, poll: SourcePoll) -> Result<SourceBatch, ProviderError> {
        assert_eq!(
            poll.cursor, None,
            "a list reads everything the filters find"
        );
        Ok(SourceBatch {
            items: ["Fix the build", "Vendor key", "Fix the docs"]
                .iter()
                .enumerate()
                .map(|(n, title)| SourceItem {
                    id: format!("o/r#{n}"),
                    title: (*title).to_owned(),
                    body: String::new(),
                    url: None,
                    labels: Vec::new(),
                    author: None,
                    updated: String::new(),
                    code: None,
                    code_tag: None,
                })
                .collect(),
            cursor: Some("advanced".to_owned()),
        })
    }
}

fn roster() -> (tempfile::TempDir, Roster) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("operations.toml"),
        "[operation.G]\nclearance = \"field\"\ncover = \"overt\"\n\
         [operation.G.sources.gh]\nprovider = \"github\"\nrepo = \"o/r\"\n\
         token_env = \"AI6_TEST_NONE\"\n",
    )
    .unwrap();
    let roster = Roster::load(dir.path()).unwrap();
    (dir, roster)
}

fn read(item: Option<&str>, query: Option<&str>) -> SourceRead {
    SourceRead {
        id: "r1".to_owned(),
        operation: "G".to_owned(),
        tracker: Some("gh".to_owned()),
        item: item.map(str::to_owned),
        query: query.map(str::to_owned),
        comments: 3,
        items: 10,
    }
}

#[test]
fn one_item_comes_with_its_latest_comments_and_their_authors_roles() {
    let (_dir, roster) = roster();
    let tracker = Recording::default();
    let build_tracker = |provider: &str, _: &toml::Table| -> Result<Box<dyn Tracker>, String> {
        assert_eq!(provider, "github");
        Ok(Box::new(tracker.clone()))
    };
    let build = |_: &str, _: &toml::Table| -> Result<Box<dyn Source>, String> {
        unreachable!("one item is read from its tracker")
    };
    let answer = answer(
        &read(Some("o/r#7"), None),
        &roster,
        &SourceLimits::default(),
        &build,
        &build_tracker,
    );
    assert_eq!(answer.error, None);
    assert_eq!(*tracker.asked.lock().unwrap(), ["o/r#7"]);
    let item = answer.item.unwrap();
    assert_eq!(
        (
            item.tracker.as_str(),
            item.id.as_str(),
            item.state.as_deref()
        ),
        ("gh", "o/r#7", Some("open"))
    );
    assert_eq!(item.assignees, ["ana"]);
    let comments: Vec<(&str, &str)> = answer
        .comments
        .iter()
        .map(|comment| (comment.body.as_str(), comment.role.as_str()))
        .collect();
    assert_eq!(
        comments,
        [
            ("comment 3", "other"),
            ("comment 4", "answers"),
            ("comment 5", "ai6")
        ]
    );
}

#[test]
fn a_list_filters_by_title_and_stops_at_its_bound() {
    let (_dir, roster) = roster();
    let build =
        |_: &str, _: &toml::Table| -> Result<Box<dyn Source>, String> { Ok(Box::new(Three)) };
    let build_tracker = |_: &str, _: &toml::Table| -> Result<Box<dyn Tracker>, String> {
        unreachable!("a list reads the source")
    };
    let limits = SourceLimits::default();
    let fixes = answer(
        &read(None, Some("fix")),
        &roster,
        &limits,
        &build,
        &build_tracker,
    );
    let titles: Vec<&str> = fixes.items.iter().map(|item| item.title.as_str()).collect();
    assert_eq!(titles, ["Fix the build", "Fix the docs"]);
    let mut one = read(None, None);
    one.items = 1;
    assert_eq!(
        answer(&one, &roster, &limits, &build, &build_tracker)
            .items
            .len(),
        1
    );
}

#[test]
fn an_unknown_operation_or_source_is_an_answer_not_a_panic() {
    let (_dir, roster) = roster();
    let build = |_: &str, _: &toml::Table| -> Result<Box<dyn Source>, String> {
        Err("never built".to_owned())
    };
    let build_tracker = |_: &str, _: &toml::Table| -> Result<Box<dyn Tracker>, String> {
        Err("never built".to_owned())
    };
    let limits = SourceLimits::default();
    let mut unknown = read(Some("o/r#1"), None);
    unknown.operation = "Z".to_owned();
    assert!(
        answer(&unknown, &roster, &limits, &build, &build_tracker)
            .error
            .unwrap()
            .contains("no Operation Z")
    );
    let mut other = read(Some("o/r#1"), None);
    other.tracker = Some("jira".to_owned());
    assert!(
        answer(&other, &roster, &limits, &build, &build_tracker)
            .error
            .unwrap()
            .contains("no source jira")
    );
}
