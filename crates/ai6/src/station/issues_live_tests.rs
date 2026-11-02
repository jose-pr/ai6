//! The Headquarters Station's drain against the real GitHub and Jira, through the built provider
//! libraries, with no Mission and no model. Ignored unless asked for:
//!
//! ```text
//! cargo build -p ai6-source-github -p ai6-source-jira
//! AI6_TEST_LIVE_SOURCES=1 AI6_TEST_LIVE_PLUGIN_DIR=target/debug \
//! AI6_TEST_LIVE_GITHUB_REPO=owner/name AI6_TEST_LIVE_GITHUB_TOKEN=... \
//! AI6_TEST_LIVE_JIRA_SITE=https://example.atlassian.net AI6_TEST_LIVE_JIRA_PROJECT=KEY \
//! AI6_TEST_LIVE_JIRA_EMAIL=... AI6_TEST_LIVE_JIRA_TOKEN=... \
//!   cargo test -p ai6 --all-features --lib live_drain -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Issues it makes are titled as test data and left closed.

use std::fs;
use std::path::PathBuf;

use guatiao::ToValue;

use super::*;
use crate::directorates::registry::mission_issues::{IssueProposal, ProposalState, ProposedIssue};
use crate::directorates::registry::sources::SourceConfig;
use crate::directorates::registry::store::FileRegistry;
use crate::station::plugins::Plugins;
use crate::station::sources::source_link;

const MISSION: &str = "live-drain";

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("this test needs {name}"))
}

struct Live {
    _roster_dir: tempfile::TempDir,
    _state_dir: tempfile::TempDir,
    plugins: Plugins,
    roster: Roster,
    registry: FileRegistry,
    memory: DrainState,
    project: String,
    stamp: u128,
}

impl Live {
    fn new() -> Self {
        required("AI6_TEST_LIVE_SOURCES");
        let plugins = Plugins::scan(&[PathBuf::from(required("AI6_TEST_LIVE_PLUGIN_DIR"))]);
        let repo = required("AI6_TEST_LIVE_GITHUB_REPO");
        let project = required("AI6_TEST_LIVE_JIRA_PROJECT");
        let site = required("AI6_TEST_LIVE_JIRA_SITE");
        let roster_dir = tempfile::tempdir().unwrap();
        fs::write(
            roster_dir.path().join("operations.toml"),
            format!(
                "[operation.G]\nclearance = \"field\"\ncover = \"overt\"\n\
                 [operation.G.issues]\nstatus_interval_secs = 0\n\
                 [operation.G.sources.gh]\nprovider = \"github\"\nrepo = \"{repo}\"\n\
                 token_env = \"AI6_TEST_LIVE_GITHUB_TOKEN\"\nintake = false\n\
                 labels = [\"ai6:ready\"]\n\
                 status_labels = {{ working = \"ai6:working\", waiting = \"ai6:waiting\" }}\n\
                 [operation.G.sources.jira]\nsite = \"{site}\"\nproject = \"{project}\"\n\
                 email_env = \"AI6_TEST_LIVE_JIRA_EMAIL\"\n\
                 token_env = \"AI6_TEST_LIVE_JIRA_TOKEN\"\nintake = false\n\
                 labels = [\"ai6-ready\"]\n\
                 status_labels = {{ working = \"ai6-working\", waiting = \"ai6-waiting\" }}\n"
            ),
        )
        .unwrap();
        let roster = Roster::load(roster_dir.path()).unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state_dir.path());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        Self {
            _roster_dir: roster_dir,
            _state_dir: state_dir,
            plugins,
            roster,
            registry,
            memory: DrainState::new(),
            project,
            stamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        }
    }

    fn tracker(&self, provider: &str, table: toml::Table) -> Box<dyn Tracker> {
        let settings_toml = toml::to_string(&table).unwrap();
        let config = SourceConfig { settings_toml }.to_value().unwrap();
        self.plugins.tracker(provider, Some(&config)).unwrap()
    }

    fn instance(&self, name: &str) -> Box<dyn Tracker> {
        let setup = setup_of(&self.roster, &SourceLimits::default(), "G", name).unwrap();
        self.tracker(&setup.provider, setup.settings)
    }

    fn drain(&self) {
        let build = |provider: &str, settings: &toml::Table| -> Result<Box<dyn Tracker>, String> {
            let settings_toml = toml::to_string(settings).map_err(|e| e.to_string())?;
            let config = SourceConfig { settings_toml }
                .to_value()
                .map_err(|e| e.to_string())?;
            self.plugins
                .tracker(provider, Some(&config))
                .map_err(|e| e.to_string())
        };
        for _ in 0..4 {
            drain_outbox(
                &self.registry,
                &self.roster,
                &build,
                "station-a",
                &SourceLimits::default(),
                &self.memory,
                unix(),
            );
        }
    }

    fn proposal(&self, specs: Vec<ProposedIssue>, dropped: &[&str]) -> String {
        let stored = self
            .registry
            .propose_issues(&IssueProposal {
                id: String::new(),
                operation: "G".to_owned(),
                mission_id: MISSION.to_owned(),
                proposer: "agent:live".to_owned(),
                specs,
                exposure: Vec::new(),
                findings: Vec::new(),
                dropped: dropped.iter().map(|text| (*text).to_owned()).collect(),
                state: ProposalState::Pending,
                decided_by: None,
                mark_ready: false,
                created: Vec::new(),
                failed: Vec::new(),
                retry_key: None,
                told: false,
                at: unix(),
            })
            .unwrap();
        self.registry
            .decide_issue_proposal(&stored.id, true, "live-test", false)
            .unwrap();
        stored.id
    }

    fn close(&self, tracker: &dyn Tracker, item: &str) {
        let closed = tracker.update(TrackerUpdate {
            item: item.to_owned(),
            state: Some("closed".to_owned()),
            ..TrackerUpdate::default()
        });
        println!("closed {item}: {}", closed.is_ok());
    }
}

fn unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[test]
#[ignore = "calls the real GitHub and Jira: set AI6_TEST_LIVE_SOURCES and see the top of this file"]
fn live_drain_status_split_and_mirror_reach_the_real_trackers() {
    let live = Live::new();
    let (github, jira) = (live.instance("gh"), live.instance("jira"));

    // A source issue on each: one status comment, edited in place through the phases.
    let source = github
        .create(IssueSpec {
            title: format!("AI6 test data: live drain ({})", live.stamp),
            body: "The source issue of a live drain.".to_owned(),
            ..IssueSpec::default()
        })
        .unwrap();
    println!("github source {}", source.id);
    live.registry
        .link_issue(&source_link(
            "G",
            MISSION,
            "gh",
            &source.id,
            source.url.clone(),
            1.0,
        ))
        .unwrap();
    let mut comments = Vec::new();
    for phase in ["working", "waiting", "working"] {
        assert_eq!(
            queue_status(&live.registry, &live.roster, MISSION, phase, None, unix()),
            1,
            "{phase}"
        );
        live.drain();
        let batch = github
            .replies(crate::ReplyPoll {
                items: vec![source.id.clone()],
                since: None,
            })
            .unwrap();
        let own: Vec<_> = batch.replies.iter().filter(|reply| reply.own).collect();
        assert_eq!(own.len(), 1, "{phase}: one comment, edited in place");
        comments.push(own[0].id.clone());
        assert!(own[0].body.contains("ai6:status"), "{}", own[0].body);
        let labels = github.issue(source.id.clone()).unwrap().labels;
        assert_eq!(labels, [format!("ai6:{phase}")], "{phase}: labels swapped");
    }
    assert!(comments.windows(2).all(|pair| pair[0] == pair[1]));
    println!("status comment {} kept through 3 phases", comments[0]);

    // A split: children on GitHub, attached as sub-issues and linked to the Mission.
    let split = live.proposal(
        vec![ProposedIssue {
            tracker: "gh".to_owned(),
            title: format!("AI6 test data: split child ({})", live.stamp),
            body: "A child of the source issue.".to_owned(),
            parent: Some(format!("gh:{}", source.id)),
            depth: 1,
            ..ProposedIssue::default()
        }],
        &[],
    );
    live.drain();
    let settled = live.registry.issue_proposal(&split).unwrap().unwrap();
    assert_eq!(
        settled.state,
        ProposalState::Created,
        "{:?}",
        settled.failed
    );
    let child_item = settled.created[0].trim_start_matches("gh:").to_owned();
    println!("split child {child_item}");
    assert_eq!(
        github.issue(child_item.clone()).unwrap().parent.as_deref(),
        Some(source.id.as_str()),
        "the real sub-issue"
    );
    let links = live.registry.issue_links(MISSION).unwrap();
    let child = links.iter().find(|link| link.item == child_item).unwrap();
    assert_eq!(child.relation, LinkRelation::Child);

    // A Jira child, under a standard issue and so a sub-task.
    let jira_source = jira
        .create(IssueSpec {
            title: format!("AI6 test data: live drain, Jira source ({})", live.stamp),
            body: "The Jira issue to mirror.\n\n- with a list".to_owned(),
            priority: Some("Medium".to_owned()),
            ..IssueSpec::default()
        })
        .unwrap();
    println!("jira source {}", jira_source.id);
    assert!(jira_source.id.starts_with(&live.project));
    live.registry
        .link_issue(&source_link(
            "G",
            MISSION,
            "jira",
            &jira_source.id,
            jira_source.url.clone(),
            1.0,
        ))
        .unwrap();
    let jira_split = live.proposal(
        vec![ProposedIssue {
            tracker: "jira".to_owned(),
            title: format!("AI6 test data: Jira child ({})", live.stamp),
            body: "A child in Jira.".to_owned(),
            parent: Some(format!("jira:{}", jira_source.id)),
            depth: 1,
            ..ProposedIssue::default()
        }],
        &[],
    );
    live.drain();
    let settled = live.registry.issue_proposal(&jira_split).unwrap().unwrap();
    assert_eq!(
        settled.state,
        ProposalState::Created,
        "{:?}",
        settled.failed
    );
    let jira_child = settled.created[0].trim_start_matches("jira:").to_owned();
    assert_eq!(
        jira.issue(jira_child.clone()).unwrap().parent.as_deref(),
        Some(jira_source.id.as_str())
    );
    println!("jira child {jira_child}");

    // A Jira-to-GitHub mirror: its back-link comment and remote link on the original.
    let mirror = live.proposal(
        vec![ProposedIssue {
            tracker: "gh".to_owned(),
            title: format!("AI6 test data: mirror ({})", live.stamp),
            body: format!(
                "Mirrored from {}\n\n> The Jira issue to mirror.\n",
                jira_source.url.as_deref().unwrap_or_default()
            ),
            labels: vec!["ai6:no-such-label".to_owned()],
            mirror_of: Some(format!("jira:{}", jira_source.id)),
            ..ProposedIssue::default()
        }],
        &["assignees", "components", "sprints"],
    );
    live.drain();
    let settled = live.registry.issue_proposal(&mirror).unwrap().unwrap();
    assert_eq!(
        settled.state,
        ProposalState::Created,
        "{:?}",
        settled.failed
    );
    let copy = settled.created[0].trim_start_matches("gh:").to_owned();
    println!("mirror {copy}");
    let copied = github.issue(copy.clone()).unwrap();
    assert!(copied.body.starts_with("Mirrored from "), "{}", copied.body);
    let entries = live.registry.outbox_entries(Some(MISSION)).unwrap();
    let create = entries
        .iter()
        .find(|entry| entry.result.as_ref().and_then(|r| r.created_item.as_ref()) == Some(&copy))
        .unwrap();
    let dropped = &create.result.as_ref().unwrap().dropped;
    assert!(
        dropped
            .iter()
            .any(|field| field == "label ai6:no-such-label")
            && dropped.iter().any(|field| field == "sprints"),
        "{dropped:?}"
    );
    live.drain();
    let replies = jira
        .replies(crate::ReplyPoll {
            items: vec![jira_source.id.clone()],
            since: None,
        })
        .unwrap();
    assert!(
        replies
            .replies
            .iter()
            .any(|reply| reply.own && reply.body.starts_with("Tracked in ")),
        "the back-link comment: {replies:?}"
    );
    assert!(
        entries_settled(&live, OutboxKind::Link, &jira_source.id),
        "the remote link"
    );
    let links = live.registry.issue_links(MISSION).unwrap();
    assert_eq!(
        links
            .iter()
            .find(|link| link.item == copy)
            .unwrap()
            .relation,
        LinkRelation::Mirror
    );
    assert_eq!(
        links
            .iter()
            .find(|link| link.item == jira_source.id)
            .unwrap()
            .relation,
        LinkRelation::Source,
        "the original keeps its relation"
    );

    for item in [&copy, &child_item, &source.id] {
        live.close(github.as_ref(), item);
    }
    for item in [&jira_child, &jira_source.id] {
        live.close(jira.as_ref(), item);
    }
    let failures: Vec<_> = live
        .registry
        .outbox_entries(Some(MISSION))
        .unwrap()
        .into_iter()
        .filter(|entry| matches!(entry.state, OutboxState::Failed | OutboxState::Refused))
        .map(|entry| (entry.kind, entry.error))
        .collect();
    assert!(failures.is_empty(), "{failures:?}");
}

fn entries_settled(live: &Live, kind: OutboxKind, item: &str) -> bool {
    live.registry
        .outbox_entries(Some(MISSION))
        .unwrap()
        .iter()
        .any(|entry| entry.kind == kind && entry.item == item && entry.state == OutboxState::Posted)
}
