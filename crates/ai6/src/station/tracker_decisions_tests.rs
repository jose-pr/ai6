//! Green ink decided by a comment on a linked item, against a Tracker that says what it is told.

use std::fs;
use std::sync::{Arc, Mutex};

use guatiao::library::ProviderError;

use super::issues::poll_replies;
use crate::directorates::m::approvals::ActionScope;
use crate::directorates::m::identity::{MissionId, RunId};
use crate::directorates::m::roster::Roster;
use crate::directorates::registry::approvals::{ApprovalRequest, ApprovalStatus, Decision};
use crate::directorates::registry::backend::*;
use crate::directorates::registry::limits::ApprovalLimits;
use crate::directorates::registry::mission_issues::ReplyState;
use crate::directorates::registry::store::FileRegistry;
use crate::{
    IssueSpec, PostedComment, Reply, ReplyBatch, ReplyPoll, Tracker, TrackerCapabilities,
    TrackerIssue, TrackerLink, TrackerPost, TrackerUpdate,
};

/// A tracker whose comments are whatever the test put there.
#[derive(Clone, Default)]
struct Said(Arc<Mutex<Vec<Reply>>>);

impl Tracker for Said {
    fn capabilities(&self, _: Option<String>) -> Result<TrackerCapabilities, ProviderError> {
        Ok(TrackerCapabilities::default())
    }
    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError> {
        Ok(TrackerIssue {
            id: item,
            ..TrackerIssue::default()
        })
    }
    fn replies(&self, _: ReplyPoll) -> Result<ReplyBatch, ProviderError> {
        Ok(ReplyBatch {
            replies: self.0.lock().unwrap().clone(),
            cursor: None,
        })
    }
    fn post(&self, _: TrackerPost) -> Result<PostedComment, ProviderError> {
        unreachable!("a decision posts nothing")
    }
    fn update(&self, _: TrackerUpdate) -> Result<(), ProviderError> {
        unreachable!("a decision updates nothing")
    }
    fn create(&self, _: IssueSpec) -> Result<TrackerIssue, ProviderError> {
        unreachable!("a decision creates nothing")
    }
    fn link(&self, _: TrackerLink) -> Result<(), ProviderError> {
        unreachable!("a decision links nothing")
    }
}

struct World {
    _roster_dir: tempfile::TempDir,
    _state_dir: tempfile::TempDir,
    roster: Roster,
    registry: FileRegistry,
    said: Said,
}

impl World {
    /// Operation G with the Source `gh`, whose `decisions` table is `decisions`, and Mission
    /// `m1` linked to `o/r#1`.
    fn new(decisions: &str) -> Self {
        let roster_dir = tempfile::tempdir().unwrap();
        fs::write(
            roster_dir.path().join("operations.toml"),
            format!(
                "[operation.G]\nclearance = \"field\"\ncover = \"overt\"\n\
                 [operation.G.sources.gh]\nprovider = \"github\"\nrepo = \"o/r\"\n\
                 token_env = \"AI6_TEST_NONE\"\n{decisions}\n"
            ),
        )
        .unwrap();
        let roster = Roster::load(roster_dir.path()).unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state_dir.path());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        registry
            .link_issue(&crate::station::sources::source_link(
                "G", "m1", "gh", "o/r#1", None, 1.0,
            ))
            .unwrap();
        Self {
            _roster_dir: roster_dir,
            _state_dir: state_dir,
            roster,
            registry,
            said: Said::default(),
        }
    }

    /// Green ink the Mission's Agent waits for.
    fn ask(&self, kind: &str, target: &str) -> String {
        let now = unix_now();
        let request = ApprovalRequest::try_new(
            "G",
            MissionId::try_new("m1").unwrap(),
            RunId::try_new("run-1").unwrap(),
            "007",
            ActionScope::try_new(kind, target).unwrap(),
            "the release is ready",
            target,
            &ApprovalLimits::default(),
        )
        .unwrap()
        .expiring(now, std::time::Duration::from_secs(3_600));
        self.registry.request_approval(&request).unwrap();
        request.id
    }

    fn comment(&self, id: &str, author: &str, body: &str) {
        self.said.0.lock().unwrap().push(Reply {
            id: id.to_owned(),
            item: "o/r#1".to_owned(),
            author_id: author.to_owned(),
            author: author.to_owned(),
            permitted: true,
            own: false,
            body: body.to_owned(),
            created: String::new(),
        });
    }

    fn poll(&self) {
        let said = self.said.clone();
        let build = move |_: &str, _: &toml::Table| -> Result<Box<dyn Tracker>, String> {
            Ok(Box::new(said.clone()))
        };
        poll_replies(
            &self.registry,
            &self.roster,
            &build,
            "station-a",
            unix_now(),
        );
    }

    fn status(&self, id: &str) -> ApprovalStatus {
        self.registry
            .approval("G", id, unix_now())
            .unwrap()
            .expect("the request exists")
    }

    fn audited(&self, action: &str) -> Vec<crate::AuditRecord> {
        self.registry
            .list_audit(100)
            .unwrap()
            .into_iter()
            .filter(|record| record.action == action)
            .collect()
    }
}

const LISTED: &str = "decisions = { approvers = [\"alice-id\"] }";

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[test]
fn an_approve_from_a_listed_approver_decides_and_is_audited_with_the_comment() {
    let world = World::new(LISTED);
    let id = world.ask("merge", "PR 43");
    world.comment("c-1", "alice-id", "/approve");
    world.poll();

    let ApprovalStatus::Decided(approval) = world.status(&id) else {
        panic!("the request is still pending");
    };
    assert_eq!(approval.decision, Decision::Approved);
    assert_eq!(approval.signer, "tracker:gh:alice-id");
    let audited = world.audited("green_ink.approved");
    assert_eq!(audited.len(), 1, "{audited:?}");
    assert_eq!(audited[0].actor, "tracker:gh:alice-id");
    assert_eq!(audited[0].operation, "G");
    assert!(audited[0].target.contains(&id), "{}", audited[0].target);
    assert!(
        audited[0].target.contains("comment c-1"),
        "{}",
        audited[0].target
    );
    // The command is not a message for the Agent, and polling again decides nothing twice.
    let stored = world.registry.issue_replies("m1").unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].state, ReplyState::Ignored);
    assert!(stored[0].body.is_empty());
    world.poll();
    assert_eq!(world.audited("green_ink.approved").len(), 1);
}

#[test]
fn a_reject_with_a_reason_refuses_and_carries_the_reason() {
    let world = World::new(LISTED);
    let id = world.ask("push", "main");
    world.comment("c-1", "alice-id", "/reject the tests are red");
    world.poll();
    let ApprovalStatus::Decided(approval) = world.status(&id) else {
        panic!("the request is still pending");
    };
    assert_eq!(approval.decision, Decision::Rejected);
    assert_eq!(approval.note.as_deref(), Some("the tests are red"));
    assert_eq!(world.audited("green_ink.rejected").len(), 1);
}

#[test]
fn a_command_from_anyone_else_decides_nothing_and_is_audited_as_refused() {
    let world = World::new(LISTED);
    let id = world.ask("merge", "PR 43");
    world.comment("c-1", "mallory-id", "/approve");
    world.comment("c-2", "mallory-id", "/reject nope");
    world.poll();
    assert!(matches!(world.status(&id), ApprovalStatus::Pending(_)));
    assert!(world.audited("green_ink.approved").is_empty());
    assert!(world.audited("green_ink.rejected").is_empty());
    let refused = world.audited("green_ink.refused");
    assert_eq!(refused.len(), 2, "{refused:?}");
    assert!(
        refused
            .iter()
            .all(|record| record.actor == "tracker:gh:mallory-id")
    );
    assert!(refused[0].target.contains("not a listed approver"));
    // Nor is the text handed to the Agent.
    assert!(
        world
            .registry
            .issue_replies("m1")
            .unwrap()
            .iter()
            .all(|reply| reply.state == ReplyState::Ignored && reply.body.is_empty())
    );
}

#[test]
fn nothing_is_decided_when_the_request_is_ambiguous_a_promotion_or_absent() {
    let world = World::new(LISTED);
    // Nothing waiting.
    world.comment("c-0", "alice-id", "/approve");
    world.poll();
    assert_eq!(world.audited("green_ink.refused").len(), 1);

    let first = world.ask("merge", "PR 1");
    let second = world.ask("merge", "PR 2");
    world.comment("c-1", "alice-id", "/approve");
    world.poll();
    for id in [&first, &second] {
        assert!(matches!(world.status(id), ApprovalStatus::Pending(_)));
    }
    assert!(
        world
            .audited("green_ink.refused")
            .iter()
            .any(|record| record.target.contains("more than one"))
    );

    let world = World::new(LISTED);
    let promotion = world.ask("clearance", "007 desk");
    world.comment("c-1", "alice-id", "/approve");
    world.poll();
    assert!(matches!(
        world.status(&promotion),
        ApprovalStatus::Pending(_)
    ));
    assert!(
        world.audited("green_ink.refused")[0]
            .target
            .contains("agent.configure")
    );
}

#[test]
fn with_no_approvers_listed_a_command_is_an_ordinary_comment() {
    let world = World::new("");
    let id = world.ask("merge", "PR 43");
    world.comment("c-1", "alice-id", "/approve");
    world.poll();
    assert!(matches!(world.status(&id), ApprovalStatus::Pending(_)));
    assert!(world.audited("green_ink.refused").is_empty());
    let stored = world.registry.issue_replies("m1").unwrap();
    assert_eq!(stored[0].state, ReplyState::Pending);
    assert_eq!(stored[0].body, "/approve");
}

#[test]
fn listing_approvers_is_reported_as_a_weakening() {
    let world = World::new(LISTED);
    let found = super::issues::weakenings(&world.roster);
    assert!(
        found
            .iter()
            .any(|line| line.contains("source \"gh\"") && line.contains("decide green ink")),
        "{found:?}"
    );
    assert!(super::issues::weakenings(&World::new("").roster).is_empty());
}
