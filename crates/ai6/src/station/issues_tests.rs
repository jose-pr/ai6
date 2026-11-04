//! The Headquarters Station's side of creating issues, against a Tracker that records its calls.

use std::fs;
use std::sync::{Arc, Mutex};

use guatiao::Status;
use guatiao::library::ProviderError;

use super::*;
use crate::directorates::registry::mission_issues::{IssueProposal, ProposalState, ProposedIssue};
use crate::directorates::registry::store::FileRegistry;
use crate::{PostedComment, ReplyBatch, ReplyPoll, TrackerCapabilities, TrackerIssue, TrackerPost};

#[derive(Default)]
struct Calls {
    created: Vec<IssueSpec>,
    posted: Vec<TrackerPost>,
    updated: Vec<TrackerUpdate>,
    linked: Vec<TrackerLink>,
}

/// A tracker that records what it is asked. `children` is what it says about attaching.
#[derive(Clone)]
struct Recording {
    prefix: &'static str,
    children: &'static str,
    remote_links: bool,
    refuse_create: bool,
    calls: Arc<Mutex<Calls>>,
}

impl Tracker for Recording {
    fn capabilities(&self, _: Option<String>) -> Result<TrackerCapabilities, ProviderError> {
        Ok(TrackerCapabilities {
            create: true,
            children: self.children.to_owned(),
            links: if self.remote_links {
                vec!["remote_links".to_owned()]
            } else {
                Vec::new()
            },
            ..TrackerCapabilities::default()
        })
    }

    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError> {
        if item.contains("foreign") {
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_BAD_VALUE,
                "foreign item: this instance serves o/r",
            ));
        }
        Ok(TrackerIssue {
            title: format!("The title of {item}"),
            state: "open".to_owned(),
            url: Some(format!("https://tracker.test/{item}")),
            id: item,
            ..TrackerIssue::default()
        })
    }

    fn replies(&self, _: ReplyPoll) -> Result<ReplyBatch, ProviderError> {
        Ok(ReplyBatch::default())
    }

    fn post(&self, post: TrackerPost) -> Result<PostedComment, ProviderError> {
        self.calls.lock().unwrap().posted.push(post);
        Ok(PostedComment {
            id: "1".to_owned(),
            url: None,
        })
    }

    fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError> {
        self.calls.lock().unwrap().updated.push(update);
        Ok(())
    }

    fn create(&self, spec: IssueSpec) -> Result<TrackerIssue, ProviderError> {
        if self.refuse_create {
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "this tracker cannot create issues",
            ));
        }
        let mut calls = self.calls.lock().unwrap();
        calls.created.push(spec.clone());
        let number = 100 + calls.created.len();
        let id = format!("{}{number}", self.prefix);
        // As the providers give them: GitHub's number with its tag, Jira's key alone.
        let numbered = self.prefix.ends_with('#');
        Ok(TrackerIssue {
            url: Some(format!("https://tracker.test/{id}")),
            code: Some(if numbered {
                number.to_string()
            } else {
                id.clone()
            }),
            code_tag: numbered.then(|| "gh".to_owned()),
            id,
            title: spec.title,
            labels: spec
                .labels
                .into_iter()
                .filter(|label| label != "not-on-the-target")
                .collect(),
            parent: spec.parent,
            ..TrackerIssue::default()
        })
    }

    fn link(&self, link: TrackerLink) -> Result<(), ProviderError> {
        self.calls.lock().unwrap().linked.push(link);
        Ok(())
    }
}

struct World {
    _roster_dir: tempfile::TempDir,
    _state_dir: tempfile::TempDir,
    roster: Roster,
    registry: FileRegistry,
    github: Recording,
    jira: Recording,
    memory: DrainState,
}

impl World {
    fn new(children: &'static str) -> Self {
        let roster_dir = tempfile::tempdir().unwrap();
        fs::write(
            roster_dir.path().join("operations.toml"),
            "[operation.G]\nclearance = \"field\"\ncover = \"overt\"\n\
             [operation.G.sources.gh]\nprovider = \"github\"\nrepo = \"o/r\"\n\
             token_env = \"AI6_TEST_NONE\"\nlabels = [\"ship-it\"]\n\
             [operation.G.sources.jira]\nsite = \"https://x.example\"\nproject = \"AI6\"\n",
        )
        .unwrap();
        let roster = Roster::load(roster_dir.path()).unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state_dir.path());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        let calls = || Arc::new(Mutex::new(Calls::default()));
        Self {
            _roster_dir: roster_dir,
            _state_dir: state_dir,
            roster,
            registry,
            github: Recording {
                prefix: "o/r#",
                children,
                remote_links: false,
                refuse_create: false,
                calls: calls(),
            },
            jira: Recording {
                prefix: "AI6-",
                children: "parent",
                remote_links: true,
                refuse_create: false,
                calls: calls(),
            },
            memory: DrainState::new(),
        }
    }

    fn drain(&self) {
        let (github, jira) = (self.github.clone(), self.jira.clone());
        let build = move |provider: &str, _: &toml::Table| -> Result<Box<dyn Tracker>, String> {
            Ok(match provider {
                "github" => Box::new(github.clone()),
                _ => Box::new(jira.clone()),
            })
        };
        // One pass queues, one sends and one settles; a few more find nothing left to do.
        for _ in 0..5 {
            drain_outbox(
                &self.registry,
                &self.roster,
                &build,
                "station-a",
                &SourceLimits::default(),
                &self.memory,
                unix_now(),
            );
        }
    }

    fn link(&self, mission: &str, tracker: &str, item: &str) {
        self.registry
            .link_issue(&crate::station::sources::source_link(
                "G", mission, tracker, item, None, 1.0,
            ))
            .unwrap();
    }

    fn approved(&self, proposal: IssueProposal, mark_ready: bool) -> String {
        let stored = self.registry.propose_issues(&proposal).unwrap();
        self.registry
            .decide_issue_proposal(&stored.id, true, "sam", mark_ready)
            .unwrap()
            .unwrap();
        stored.id
    }
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn proposal(mission: &str, specs: Vec<ProposedIssue>, dropped: &[&str]) -> IssueProposal {
    IssueProposal {
        id: String::new(),
        operation: "G".to_owned(),
        mission_id: mission.to_owned(),
        proposer: "agent:007".to_owned(),
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
        at: 1.0,
    }
}

fn child(tracker: &str, title: &str, parent: &str) -> ProposedIssue {
    ProposedIssue {
        tracker: tracker.to_owned(),
        title: title.to_owned(),
        body: format!("Do {title}."),
        parent: Some(parent.to_owned()),
        depth: 1,
        ..ProposedIssue::default()
    }
}

#[test]
fn an_approved_split_attaches_each_child_and_links_it_to_the_mission() {
    let world = World::new("sub_issues");
    world.link("m1", "gh", "o/r#1");
    let id = world.approved(
        proposal(
            "m1",
            vec![
                child("gh", "one", "gh:o/r#1"),
                child("gh", "two", "gh:o/r#1"),
            ],
            &[],
        ),
        false,
    );
    world.drain();

    let calls = world.github.calls.lock().unwrap();
    assert_eq!(calls.created.len(), 2, "each once, whatever the passes");
    assert_eq!(calls.created[0].parent.as_deref(), Some("o/r#1"));
    assert!(
        calls.created[0].body.starts_with("Do "),
        "no reference in the text"
    );
    assert!(calls.posted.is_empty(), "the tracker attached them");
    drop(calls);
    let settled = world.registry.issue_proposal(&id).unwrap().unwrap();
    assert_eq!(settled.state, ProposalState::Created);
    let mut created = settled.created.clone();
    created.sort();
    assert_eq!(created, ["gh:o/r#101", "gh:o/r#102"]);
    let links = world.registry.issue_links("m1").unwrap();
    let made: Vec<_> = links
        .iter()
        .filter(|link| link.relation == LinkRelation::Child)
        .collect();
    assert_eq!(made.len(), 2);
    assert_eq!(made[0].linked_by, "agent:007");
    assert!(
        made[0]
            .url
            .as_deref()
            .unwrap()
            .starts_with("https://tracker.test/o/r#10")
    );
    assert!(matches!(
        made[0].snapshot.as_ref().unwrap().title.as_str(),
        "one" | "two"
    ));
    let audit = world.registry.audit_records().unwrap();
    assert!(
        audit
            .iter()
            .any(|record| record.action == "issue.create.posted"),
        "{audit:?}"
    );
}

#[test]
fn a_tracker_with_no_sub_issues_gets_a_reference_in_each_child_and_a_list_on_the_parent() {
    let world = World::new("none");
    world.link("m1", "gh", "o/r#1");
    world.approved(
        proposal(
            "m1",
            vec![
                child("gh", "one", "gh:o/r#1"),
                child("gh", "two", "gh:o/r#1"),
            ],
            &[],
        ),
        false,
    );
    world.drain();
    let calls = world.github.calls.lock().unwrap();
    assert_eq!(calls.created[0].parent, None);
    assert!(
        calls.created[0].body.starts_with("Part of o/r#1\n\n"),
        "{}",
        calls.created[0].body
    );
    assert_eq!(calls.posted.len(), 1, "one comment naming both");
    assert_eq!(calls.posted[0].item, "o/r#1");
    assert!(
        calls.posted[0]
            .body
            .contains("https://tracker.test/o/r#101")
    );
    assert!(
        calls.posted[0]
            .body
            .contains("https://tracker.test/o/r#102")
    );
}

#[test]
fn a_mirror_lists_what_it_dropped_links_both_sides_and_leads_the_original_to_the_copy() {
    let world = World::new("sub_issues");
    world.link("m1", "jira", "AI6-1");
    let spec = ProposedIssue {
        tracker: "gh".to_owned(),
        title: "From Jira".to_owned(),
        body: "Mirrored from https://x.example/browse/AI6-1\n\n> the text\n".to_owned(),
        labels: vec!["kept".to_owned(), "not-on-the-target".to_owned()],
        mirror_of: Some("jira:AI6-1".to_owned()),
        ..ProposedIssue::default()
    };
    let id = world.approved(proposal("m1", vec![spec], &["assignees", "sprints"]), false);
    world.drain();

    let entries = world.registry.outbox_entries(Some("m1")).unwrap();
    let create = entries
        .iter()
        .find(|entry| entry.kind == OutboxKind::Create)
        .unwrap();
    let result = create.result.as_ref().unwrap();
    assert_eq!(
        result.dropped,
        ["assignees", "sprints", "label not-on-the-target"]
    );

    let settled = world.registry.issue_proposal(&id).unwrap().unwrap();
    assert_eq!(settled.created, ["gh:o/r#101"]);
    let links = world.registry.issue_links("m1").unwrap();
    let copy = links.iter().find(|link| link.tracker == "gh").unwrap();
    assert_eq!(copy.relation, LinkRelation::Mirror);
    let original = links.iter().find(|link| link.tracker == "jira").unwrap();
    assert_eq!(
        original.relation,
        LinkRelation::Source,
        "it keeps what it was"
    );

    let jira = world.jira.calls.lock().unwrap();
    assert_eq!(jira.posted.len(), 1);
    assert_eq!(jira.posted[0].item, "AI6-1");
    assert_eq!(
        jira.posted[0].body,
        "Tracked in https://tracker.test/o/r#101"
    );
    assert_eq!(
        jira.linked.len(),
        1,
        "a remote link where the tracker has them"
    );
    assert_eq!(
        jira.linked[0].url.as_deref(),
        Some("https://tracker.test/o/r#101")
    );
    assert_eq!(jira.linked[0].relation, "mirror");
}

#[test]
fn a_mirror_onto_a_tracker_with_no_remote_links_gets_only_the_comment() {
    let world = World::new("sub_issues");
    world.link("m1", "gh", "o/r#1");
    let spec = ProposedIssue {
        tracker: "jira".to_owned(),
        title: "From GitHub".to_owned(),
        mirror_of: Some("gh:o/r#1".to_owned()),
        ..ProposedIssue::default()
    };
    world.approved(proposal("m1", vec![spec], &[]), false);
    world.drain();
    let github = world.github.calls.lock().unwrap();
    assert_eq!(github.posted.len(), 1);
    assert!(github.linked.is_empty());
}

#[test]
fn ready_labels_come_only_from_an_approval_that_asked_and_name_the_person() {
    let world = World::new("sub_issues");
    world.link("m1", "gh", "o/r#1");
    world.approved(
        proposal("m1", vec![child("gh", "plain", "gh:o/r#1")], &[]),
        false,
    );
    world.drain();
    assert!(world.github.calls.lock().unwrap().updated.is_empty());

    world.approved(
        proposal("m1", vec![child("gh", "ready", "gh:o/r#1")], &[]),
        true,
    );
    world.drain();
    let calls = world.github.calls.lock().unwrap();
    assert_eq!(calls.updated.len(), 1);
    assert_eq!(calls.updated[0].item, "o/r#102");
    assert_eq!(
        calls.updated[0].add_labels,
        ["ship-it"],
        "the instance's own"
    );
    drop(calls);
    let audit = world.registry.audit_records().unwrap();
    let ready = audit
        .iter()
        .find(|record| record.action == "issue.ready")
        .expect("issue.ready is audited");
    assert_eq!(ready.actor, "person:sam");
    assert!(ready.target.contains("gh:o/r#102"), "{}", ready.target);
}

#[test]
fn a_refused_creation_settles_the_proposal_and_says_why() {
    let mut world = World::new("sub_issues");
    world.github.refuse_create = true;
    world.link("m1", "gh", "o/r#1");
    let id = world.approved(
        proposal("m1", vec![child("gh", "one", "gh:o/r#1")], &[]),
        false,
    );
    world.drain();
    let settled = world.registry.issue_proposal(&id).unwrap().unwrap();
    assert_eq!(settled.state, ProposalState::Created);
    assert!(settled.created.is_empty());
    assert!(
        settled.failed[0].contains("one") && settled.failed[0].contains("cannot create"),
        "{:?}",
        settled.failed
    );
}

/// A person's Mission that waits, in its creation, for the issue it is created with.
fn mission_with_issue(tracker: &str) -> ProposedIssue {
    let mut brief: crate::Brief = serde_json::from_value(serde_json::json!({
        "title": "Fix the login page", "body": "It times out.", "id": crate::new_uid(),
        "source": "universal-exports", "operation": "G", "agent": "007",
    }))
    .unwrap();
    brief.provenance = Some(
        crate::Provenance::new(crate::Origin::Operator, &brief, "universal-exports", 1)
            .with_principal("sam"),
    );
    ProposedIssue {
        tracker: tracker.to_owned(),
        title: "Fix the login page".to_owned(),
        body: "It times out.".to_owned(),
        mission: Some(brief),
        ..ProposedIssue::default()
    }
}

#[test]
fn a_mission_created_with_its_issue_enters_numbered_by_it_and_linked_to_it() {
    let world = World::new("sub_issues");
    let mut asked = proposal(NO_MISSION, vec![mission_with_issue("gh")], &[]);
    asked.proposer = "person:sam".to_owned();
    let id = world.approved(asked.clone(), false);
    world.drain();
    let settled = world.registry.issue_proposal(&id).unwrap().unwrap();
    assert_eq!(settled.created, ["gh:o/r#101"]);
    let queued = world.registry.list_new().unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].shown_handle(), "G-gh-101");
    assert_eq!(queued[0].title, "Fix the login page");
    assert_eq!(queued[0].agent.as_deref(), Some("007"));
    assert_eq!(
        queued[0]
            .provenance
            .as_ref()
            .map(|provenance| provenance.origin),
        Some(crate::Origin::Operator),
        "the person's brief, not the issue's text"
    );
    let links = world.registry.issue_links(&queued[0].key()).unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(
        (
            links[0].tracker.as_str(),
            links[0].item.as_str(),
            links[0].relation
        ),
        ("gh", "o/r#101", LinkRelation::Source)
    );
    assert!(
        world
            .registry
            .audit_records()
            .unwrap()
            .iter()
            .any(|record| record.action == "mission.create.entered"
                && record.target.starts_with(&queued[0].key()))
    );
    // Polled as an item of its Source, the issue leaves the person's brief as it is.
    let item = crate::SourceItem {
        id: "o/r#101".to_owned(),
        title: "Fix the login page".to_owned(),
        body: "Something else.".to_owned(),
        url: None,
        labels: Vec::new(),
        author: None,
        updated: "2026-11-05T00:00:00Z".to_owned(),
        code: Some("101".to_owned()),
        code_tag: Some("gh".to_owned()),
    };
    assert_eq!(
        world
            .registry
            .ingest_source_item("G", "gh", &item, "station-a")
            .unwrap(),
        crate::Ingest::Unchanged
    );
    assert_eq!(world.registry.list_new().unwrap()[0].body, "It times out.");

    // A tracker that cannot create says so on the proposal, and no Mission enters.
    let mut refusing = World::new("sub_issues");
    refusing.github.refuse_create = true;
    let id = refusing.approved(asked, false);
    refusing.drain();
    let settled = refusing.registry.issue_proposal(&id).unwrap().unwrap();
    assert!(
        settled.failed[0].contains("cannot create"),
        "{:?}",
        settled.failed
    );
    assert!(refusing.registry.list_new().unwrap().is_empty());
}

#[test]
fn only_an_approved_proposal_creates_anything() {
    let world = World::new("sub_issues");
    world.link("m1", "gh", "o/r#1");
    world
        .registry
        .propose_issues(&proposal("m1", vec![child("gh", "x", "gh:o/r#1")], &[]))
        .unwrap();
    let rejected = world
        .registry
        .propose_issues(&proposal("m1", vec![child("gh", "y", "gh:o/r#1")], &[]))
        .unwrap();
    world
        .registry
        .decide_issue_proposal(&rejected.id, false, "sam", false)
        .unwrap();
    world.drain();
    assert!(world.github.calls.lock().unwrap().created.is_empty());
}

#[test]
fn a_link_a_person_made_is_read_and_one_that_cannot_be_comes_back_unlinked_with_the_reason() {
    let world = World::new("sub_issues");
    for item in ["o/r#5", "foreign/x#1"] {
        world.link("m1", "gh", item);
        let mut entry =
            OutboxEntry::new("G", "m1", "gh", item, OutboxKind::Link, "person:sam", 1.0);
        entry.payload = serde_json::json!({ "resolve": true });
        world.registry.queue_outbox(&entry).unwrap();
    }
    world.drain();
    let links = world.registry.issue_links("m1").unwrap();
    let read = links.iter().find(|link| link.item == "o/r#5").unwrap();
    assert_eq!(read.state, LinkState::Active);
    assert_eq!(read.snapshot.as_ref().unwrap().title, "The title of o/r#5");
    assert_eq!(read.url.as_deref(), Some("https://tracker.test/o/r#5"));
    let foreign = links
        .iter()
        .find(|link| link.item == "foreign/x#1")
        .unwrap();
    assert_eq!(foreign.state, LinkState::Unlinked);
    let entries = world.registry.outbox_entries(Some("m1")).unwrap();
    let refused = entries
        .iter()
        .find(|entry| entry.item == "foreign/x#1")
        .unwrap();
    assert_eq!(refused.state, OutboxState::Refused);
    assert!(refused.error.as_deref().unwrap().contains("foreign item"));
}

#[test]
fn a_comment_older_than_the_link_is_not_a_reply() {
    struct Comments(Recording);
    impl Tracker for Comments {
        fn capabilities(&self, item: Option<String>) -> Result<TrackerCapabilities, ProviderError> {
            self.0.capabilities(item)
        }
        fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError> {
            self.0.issue(item)
        }
        fn replies(&self, _: ReplyPoll) -> Result<ReplyBatch, ProviderError> {
            let reply = |id: &str, created: &str| crate::Reply {
                id: id.to_owned(),
                item: "o/r#1".to_owned(),
                author_id: "maint".to_owned(),
                author: "Maint".to_owned(),
                permitted: true,
                own: false,
                body: format!("comment {id}"),
                created: created.to_owned(),
            };
            Ok(ReplyBatch {
                replies: vec![
                    reply("before", "2001-09-09T01:46:39Z"),
                    reply("after", "2001-09-09T08:46:41+0700"),
                ],
                cursor: None,
            })
        }
        fn post(&self, post: TrackerPost) -> Result<PostedComment, ProviderError> {
            self.0.post(post)
        }
        fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError> {
            self.0.update(update)
        }
        fn create(&self, spec: IssueSpec) -> Result<TrackerIssue, ProviderError> {
            self.0.create(spec)
        }
        fn link(&self, link: TrackerLink) -> Result<(), ProviderError> {
            self.0.link(link)
        }
    }
    let world = World::new("sub_issues");
    // 1,000,000,000 is 2001-09-09T01:46:40Z.
    world
        .registry
        .link_issue(&crate::station::sources::source_link(
            "G",
            "m1",
            "gh",
            "o/r#1",
            None,
            1_000_000_000.0,
        ))
        .unwrap();
    let github = world.github.clone();
    let build = move |_: &str, _: &toml::Table| -> Result<Box<dyn Tracker>, String> {
        Ok(Box::new(Comments(github.clone())))
    };
    poll_replies(
        &world.registry,
        &world.roster,
        &build,
        "station-a",
        unix_now(),
    );
    let stored = world.registry.issue_replies("m1").unwrap();
    assert_eq!(stored.len(), 1, "{stored:?}");
    assert_eq!(stored[0].id, "after");
}
