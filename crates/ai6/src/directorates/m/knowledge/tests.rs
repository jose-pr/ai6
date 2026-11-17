//! The memory cases of who may read and write a memory, written against Reports of kind `memory`.

use std::cell::RefCell;
use std::collections::BTreeSet;

use super::*;
use crate::directorates::m::reports::ReportSettings;
use crate::directorates::registry::backend::{KnowledgeStore, ReportStore};
use crate::directorates::registry::limits::ReportLimits;
use crate::directorates::registry::reports::{ReportClearance, ReportFilter};
use crate::directorates::registry::store::FileRegistry;

/// A caller holding exactly what a test gives it, recording what it audits and is shown.
#[derive(Default)]
struct Holds {
    reads: BTreeSet<String>,
    writes: BTreeSet<String>,
    proposes: BTreeSet<String>,
    assess: BTreeSet<String>,
    read_in: BTreeSet<String>,
    agent: bool,
    exposed: Vec<String>,
    seen: RefCell<Vec<String>>,
    audited: RefCell<Vec<(String, String)>>,
    audit_fails: bool,
}

impl Holds {
    fn of(scope: &ReportScope) -> Self {
        Self {
            reads: [scope.to_string()].into(),
            writes: [scope.to_string()].into(),
            assess: [scope.to_string()].into(),
            ..Self::default()
        }
    }
}

impl ReportReader for Holds {
    fn reads_scope(&self, scope: &ReportScope) -> bool {
        self.reads.contains(&scope.to_string())
    }

    fn read_in(&self, compartment: &str) -> bool {
        self.read_in.contains(compartment)
    }

    fn subjects(&self) -> Vec<String> {
        Vec::new()
    }

    fn assesses(&self, scope: &ReportScope) -> bool {
        self.assess.contains(&scope.to_string())
    }

    fn filed(&self, filer: &ReportFiler) -> bool {
        *filer == self.filer()
    }
}

impl KnowledgeCaller for Holds {
    fn by(&self) -> String {
        "person:ana".to_owned()
    }

    fn filer(&self) -> ReportFiler {
        if self.agent {
            ReportFiler::Agent {
                agent: "007".to_owned(),
                run: "G-1".to_owned(),
                mission: "G-1".to_owned(),
            }
        } else {
            ReportFiler::Person {
                person: "ana".to_owned(),
            }
        }
    }

    fn run(&self) -> Option<String> {
        None
    }

    fn writes_scope(&self, scope: &ReportScope) -> KnowledgeRight {
        if self.writes.contains(&scope.to_string()) {
            KnowledgeRight::Write
        } else if self.proposes.contains(&scope.to_string()) {
            KnowledgeRight::Propose
        } else {
            KnowledgeRight::Refused
        }
    }

    fn acts_in_person(&self) -> bool {
        !self.agent
    }

    fn exposures(&self) -> Vec<String> {
        let mut all = self.exposed.clone();
        all.extend(self.seen.borrow().iter().cloned());
        all
    }

    fn saw_marked(&self, source: &str) {
        self.seen.borrow_mut().push(source.to_owned());
    }

    fn mark_bytes(&self) -> Limit {
        Limit::at(128)
    }

    fn now(&self) -> f64 {
        100.0
    }

    fn audit(&self, action: &str, scope: &ReportScope, target: &str) -> Result<(), String> {
        if self.audit_fails {
            return Err("the log is gone".to_owned());
        }
        self.audited
            .borrow_mut()
            .push((action.to_owned(), format!("{scope} {target}")));
        Ok(())
    }
}

fn ana() -> ReportScope {
    ReportScope::Person("ana".to_owned())
}

fn bob() -> ReportScope {
    ReportScope::Person("bob".to_owned())
}

fn global() -> ReportScope {
    ReportScope::Global
}

fn store() -> (tempfile::TempDir, FileRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(dir.path());
    (dir, registry)
}

fn rules() -> (ReportSettings, Vec<ReportClearance>) {
    (ReportSettings::default(), Vec::new())
}

fn written(outcome: KnowledgeOutcome) -> KnowledgeWritten {
    match outcome {
        KnowledgeOutcome::Kept(written) => written,
        KnowledgeOutcome::Proposed(proposal) => panic!("proposed: {proposal:?}"),
    }
}

fn kept(outcome: KnowledgeOutcome) -> Report {
    written(outcome).report
}

/// One reading the global scope and writing it, read in to `compartments`.
fn global_reader(compartments: &[&str]) -> Holds {
    Holds {
        read_in: compartments.iter().map(|name| (*name).to_owned()).collect(),
        ..Holds::of(&global())
    }
}

fn new_in(scope: &ReportScope, title: &str, text: &str) -> KnowledgeChange {
    KnowledgeChange::new(
        KnowledgeTarget::New {
            scope: scope.clone(),
            kind: "memory".to_owned(),
        },
        title,
        text,
    )
    .summarised("a summary")
}

fn existing(id: &str, revision: u64, text: &str) -> KnowledgeChange {
    KnowledgeChange::new(
        KnowledgeTarget::Existing {
            id: id.to_owned(),
            revision,
        },
        "t",
        text,
    )
}

const NOT_AN_ID: &str = "6f1e4a6a-9c55-4a0c-9d5c-2f8a0b6c1e11";

#[test]
fn one_person_finds_nothing_of_anothers_by_any_read() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    let theirs = kept(
        write_report(
            &registry,
            &Holds::of(&bob()),
            &rules,
            new_in(&bob(), "bob's", "secret"),
        )
        .unwrap(),
    )
    .id;
    let caller = Holds::of(&ana());
    assert!(matches!(
        read_report(&registry, &caller, &rules, &theirs),
        Err(KnowledgeRefusal::Missing)
    ));
    assert!(matches!(
        read_report(&registry, &caller, &rules, NOT_AN_ID),
        Err(KnowledgeRefusal::Missing)
    ));
    assert!(matches!(
        prior_revisions(&registry, &caller, &rules, &theirs),
        Err(KnowledgeRefusal::Missing)
    ));
    assert!(
        list_reports(&registry, &caller, &rules, &bob())
            .unwrap()
            .is_empty()
    );
    let limits = ReportLimits::default();
    for query in [
        ReportQuery::words("secret"),
        ReportQuery::words("secret").in_scope(bob()),
        ReportQuery::default(),
    ] {
        let found = search_reports(&registry, &caller, &rules, &query, &limits).unwrap();
        assert_eq!((found.hits.len(), found.total), (0, 0));
    }
    // Nor can it be changed or forgotten by naming its ID, and each answers as a missing one.
    assert!(matches!(
        write_report(&registry, &caller, &rules, existing(&theirs, 1, "mine")),
        Err(KnowledgeRefusal::Missing)
    ));
    assert!(matches!(
        forget_report(&registry, &caller, &rules, &theirs, None),
        Err(KnowledgeRefusal::Missing)
    ));
    assert!(matches!(
        write_report(
            &registry,
            &caller,
            &rules,
            new_in(&bob(), "into theirs", "x")
        ),
        Err(KnowledgeRefusal::Forbidden(_))
    ));
    assert_eq!(registry.report(&theirs).unwrap().unwrap().text, "secret");
    assert!(
        caller.audited.borrow().is_empty(),
        "nothing refused is recorded as done"
    );
    // Moneypenny's write for a person is the person's own scope, and is still untrusted text.
    let assistant = Holds {
        agent: false,
        ..Holds::of(&ana())
    };
    let hers =
        kept(write_report(&registry, &assistant, &rules, new_in(&ana(), "hers", "x")).unwrap());
    assert_eq!(hers.scope, ana());
}

#[test]
fn a_write_is_on_the_record_before_it_happens_and_not_without_it() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    let caller = Holds::of(&ana());
    let first =
        written(write_report(&registry, &caller, &rules, new_in(&ana(), "s", "one")).unwrap());
    let id = first.report.id.clone();
    assert_eq!(first.before, None);
    assert_eq!(first.report.updated_by.as_deref(), Some("person:ana"));
    assert_eq!(
        caller.audited.borrow().as_slice(),
        [(AUDIT_WRITE.to_owned(), format!("person:ana {id}"))]
    );
    let change = existing(&id, 1, "two");
    let silent = Holds {
        audit_fails: true,
        ..Holds::of(&ana())
    };
    assert!(matches!(
        write_report(&registry, &silent, &rules, change.clone()),
        Err(KnowledgeRefusal::Failed(_))
    ));
    assert!(matches!(
        forget_report(&registry, &silent, &rules, &id, None),
        Err(KnowledgeRefusal::Failed(_))
    ));
    assert_eq!(registry.report(&id).unwrap().unwrap().text, "one");
    let again = written(write_report(&registry, &caller, &rules, change).unwrap());
    assert_eq!(again.before.as_deref(), Some("one"));
    assert_eq!(again.report.revision, 2);
    assert!(matches!(
        forget_report(&registry, &caller, &rules, &id, Some(2)).unwrap(),
        KnowledgeForgotten::Forgotten(_)
    ));
    assert_eq!(
        caller.audited.borrow().last().unwrap(),
        &(AUDIT_FORGET.to_owned(), format!("person:ana {id}"))
    );
    assert!(registry.report(&id).unwrap().is_none());
}

#[test]
fn a_write_out_of_bounds_is_refused_before_it_is_recorded() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    let caller = Holds::of(&ana());
    assert!(matches!(
        write_report(
            &registry,
            &caller,
            &rules,
            new_in(&ana(), "two\nlines", "x")
        ),
        Err(KnowledgeRefusal::Invalid(_))
    ));
    let mut observed = new_in(&ana(), "t", "x");
    observed.target = KnowledgeTarget::New {
        scope: ana(),
        kind: "finding".to_owned(),
    };
    assert!(
        matches!(
            write_report(&registry, &caller, &rules, observed),
            Err(KnowledgeRefusal::Invalid(_))
        ),
        "an observation is not knowledge"
    );
    assert!(caller.audited.borrow().is_empty());
}

#[test]
fn a_search_past_its_bound_is_refused_and_its_answer_is_cut_after_the_count() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    let caller = Holds::of(&ana());
    for title in ["needle one", "needle two", "needle three"] {
        write_report(&registry, &caller, &rules, new_in(&ana(), title, "x")).unwrap();
    }
    let limits = ReportLimits {
        search_results: Limit::at(2),
        query_bytes: Limit::at(8),
        ..ReportLimits::default()
    };
    let found = search_reports(
        &registry,
        &caller,
        &rules,
        &ReportQuery::words("needle"),
        &limits,
    )
    .unwrap();
    assert_eq!((found.hits.len(), found.total), (2, 3));
    assert!(matches!(
        search_reports(
            &registry,
            &caller,
            &rules,
            &ReportQuery::words("too long a query"),
            &limits
        ),
        Err(KnowledgeRefusal::Invalid(_))
    ));
}

#[test]
fn a_model_is_given_summaries_within_their_bound_and_told_how_many_more() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    let caller = Holds::of(&ana());
    for index in 0..5 {
        let change =
            new_in(&ana(), "n", "the content itself").summarised(format!("summary {index}"));
        write_report(&registry, &caller, &rules, change).unwrap();
    }
    let listed = list_reports(&registry, &caller, &rules, &ana()).unwrap();
    let fence = |value: &Value| value.to_string();
    let all = summaries_for_a_model(&listed, Limit::UNBOUNDED, fence).unwrap();
    assert!(all.contains("summary 0") && all.contains("summary 4"));
    assert!(!all.contains("the content itself"), "{all}");
    assert!(all.contains("\"not_shown\":0"), "{all}");
    let some = summaries_for_a_model(&listed, Limit::at(all.len() as u64 / 2), fence).unwrap();
    assert!(some.len() <= all.len() / 2);
    assert!(!some.contains("\"not_shown\":0"), "{some}");
    assert_eq!(summaries_for_a_model(&listed, Limit::at(10), fence), None);
    assert_eq!(summaries_for_a_model(&[], Limit::UNBOUNDED, fence), None);
}

#[test]
fn a_protected_report_answers_as_a_missing_id_to_who_is_not_read_in() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    let holder = global_reader(&["KESTREL"]);
    let protected = kept(
        write_report(
            &registry,
            &holder,
            &rules,
            new_in(&global(), "hushed needle", "the hush")
                .with_compartments(Some(vec!["KESTREL".to_owned()])),
        )
        .unwrap(),
    );
    assert_eq!(protected.compartments, ["KESTREL"]);
    kept(
        write_report(
            &registry,
            &holder,
            &rules,
            new_in(&global(), "open needle", "x"),
        )
        .unwrap(),
    );
    let limits = ReportLimits::default();

    for outsider in [global_reader(&[]), global_reader(&["OTHER"])] {
        for asked in [protected.id.as_str(), NOT_AN_ID] {
            assert!(matches!(
                read_report(&registry, &outsider, &rules, asked),
                Err(KnowledgeRefusal::Missing)
            ));
            assert!(matches!(
                prior_revisions(&registry, &outsider, &rules, asked),
                Err(KnowledgeRefusal::Missing)
            ));
            assert!(matches!(
                forget_report(&registry, &outsider, &rules, asked, None),
                Err(KnowledgeRefusal::Missing)
            ));
            assert!(matches!(
                write_report(&registry, &outsider, &rules, existing(asked, 1, "taken")),
                Err(KnowledgeRefusal::Missing)
            ));
        }
        let listed = list_reports(&registry, &outsider, &rules, &global()).unwrap();
        assert_eq!(listed.len(), 1);
        let search = |words: &str| {
            search_reports(
                &registry,
                &outsider,
                &rules,
                &ReportQuery::words(words),
                &limits,
            )
            .unwrap()
        };
        let found = search("needle");
        assert_eq!((found.hits.len(), found.total), (1, 1), "nothing counts it");
        assert_eq!(search("hushed").total, 0);
        assert!(outsider.audited.borrow().is_empty());
    }
    // Being read in to one of two is not being read in: every compartment counts, for every Report.
    let two = kept(
        write_report(
            &registry,
            &global_reader(&["KESTREL", "MERLIN"]),
            &rules,
            new_in(&global(), "two needle", "x")
                .with_compartments(Some(vec!["KESTREL".to_owned(), "MERLIN".to_owned()])),
        )
        .unwrap(),
    );
    assert!(read_report(&registry, &holder, &rules, &protected.id).is_ok());
    assert!(matches!(
        read_report(&registry, &holder, &rules, &two.id),
        Err(KnowledgeRefusal::Missing)
    ));
    assert_eq!(
        search_reports(
            &registry,
            &holder,
            &rules,
            &ReportQuery::words("needle"),
            &limits
        )
        .unwrap()
        .total,
        2
    );
}

#[test]
fn only_one_read_in_sets_a_compartment_and_nobody_hides_a_report_from_themselves() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    let holder = global_reader(&["MINE"]);
    for names in [&["THEIRS"][..], &["NO-SUCH"][..], &["MINE", "THEIRS"][..]] {
        let change = new_in(&global(), "s", "c")
            .with_compartments(Some(names.iter().map(|name| (*name).to_owned()).collect()));
        assert!(
            matches!(
                write_report(&registry, &holder, &rules, change),
                Err(KnowledgeRefusal::Forbidden(_))
            ),
            "{names:?}"
        );
    }
    assert!(holder.audited.borrow().is_empty());
    let report = kept(
        write_report(
            &registry,
            &holder,
            &rules,
            new_in(&global(), "s", "c").with_compartments(Some(vec!["MINE".to_owned()])),
        )
        .unwrap(),
    );
    assert_eq!(
        holder
            .audited
            .borrow()
            .iter()
            .filter(|(action, _)| action == AUDIT_COMPARTMENTS)
            .count(),
        1
    );
    // One read in to both adds THEIRS; the one read in to MINE alone can no longer read it, so
    // it cannot be changed from there either.
    let both = global_reader(&["MINE", "THEIRS"]);
    let with_both = |names: &[&str], revision: u64| {
        existing(&report.id, revision, "edited")
            .with_compartments(Some(names.iter().map(|name| (*name).to_owned()).collect()))
    };
    assert!(
        matches!(
            write_report(
                &registry,
                &holder,
                &rules,
                with_both(&["MINE", "THEIRS"], 1)
            ),
            Err(KnowledgeRefusal::Forbidden(_))
        ),
        "THEIRS is not the holder's to add"
    );
    kept(write_report(&registry, &both, &rules, with_both(&["MINE", "THEIRS"], 1)).unwrap());
    assert!(matches!(
        write_report(&registry, &holder, &rules, existing(&report.id, 2, "late")),
        Err(KnowledgeRefusal::Missing)
    ));
    // Nobody hides a report from themselves: a compartment they are not read in to is refused.
    assert!(matches!(
        write_report(&registry, &both, &rules, with_both(&["MINE", "NO-SUCH"], 2)),
        Err(KnowledgeRefusal::Forbidden(_))
    ));
    // One read in to a compartment takes it off, and the other holder reads it again.
    let narrowed = kept(write_report(&registry, &both, &rules, with_both(&["MINE"], 2)).unwrap());
    assert_eq!(narrowed.compartments, ["MINE"]);
    assert!(read_report(&registry, &holder, &rules, &report.id).is_ok());
}

#[test]
fn a_mark_stays_through_an_agents_edit_and_goes_with_a_persons_audited() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    let operation = ReportScope::Operation("G".to_owned());
    let agent = Holds {
        agent: true,
        exposed: vec!["tool:WebFetch".to_owned()],
        ..Holds::of(&operation)
    };
    let poisoned =
        kept(write_report(&registry, &agent, &rules, new_in(&operation, "s", "bad")).unwrap());
    assert_eq!(poisoned.exposures, ["tool:WebFetch"]);
    assert!(poisoned.untrusted(), "an Agent's text is not a person's");
    assert_eq!(poisoned.updated_by.as_deref(), Some("person:ana"));

    // A clean Agent reading it is told, and its own edit keeps the mark and adds what it saw.
    let reader = Holds {
        agent: true,
        ..Holds::of(&operation)
    };
    read_report(&registry, &reader, &rules, &poisoned.id).unwrap();
    let mark = format!("report:{}", poisoned.id);
    assert_eq!(reader.seen.borrow().as_slice(), std::slice::from_ref(&mark));
    let edited = kept(
        write_report(
            &registry,
            &reader,
            &rules,
            existing(&poisoned.id, 1, "still bad"),
        )
        .unwrap(),
    );
    assert_eq!(edited.exposures, [mark.clone(), "tool:WebFetch".to_owned()]);
    // A listing and a search tell a reader too, of what they show.
    let lister = Holds {
        agent: true,
        ..Holds::of(&operation)
    };
    list_reports(&registry, &lister, &rules, &operation).unwrap();
    assert_eq!(lister.seen.borrow().as_slice(), std::slice::from_ref(&mark));
    let searcher = Holds {
        agent: true,
        ..Holds::of(&operation)
    };
    search_reports(
        &registry,
        &searcher,
        &rules,
        &ReportQuery::words("zebra"),
        &ReportLimits::default(),
    )
    .unwrap();
    assert!(
        searcher.seen.borrow().is_empty(),
        "nothing shown, nothing told"
    );

    // A person's edit clears it, on the record.
    let person = Holds::of(&operation);
    let cleared = kept(
        write_report(
            &registry,
            &person,
            &rules,
            existing(&poisoned.id, 2, "checked"),
        )
        .unwrap(),
    );
    assert!(cleared.exposures.is_empty());
    assert!(
        person
            .audited
            .borrow()
            .iter()
            .any(|(action, target)| action == AUDIT_MARK_CLEARED && target.contains(&poisoned.id))
    );
}

/// An Agent that reads `scope` and may only propose to it.
fn proposer(scope: &ReportScope, exposed: &str) -> Holds {
    Holds {
        agent: true,
        exposed: vec![exposed.to_owned()],
        reads: [scope.to_string()].into(),
        proposes: [scope.to_string()].into(),
        ..Holds::default()
    }
}

#[test]
fn a_proposal_keeps_its_filer_and_only_one_who_assesses_the_scope_decides_it() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    let operation = ReportScope::Operation("G".to_owned());
    let agent = proposer(&operation, "issue:github");
    let KnowledgeOutcome::Proposed(proposal) = write_report(
        &registry,
        &agent,
        &rules,
        new_in(&operation, "worth it", "text"),
    )
    .unwrap() else {
        panic!("an Agent proposes");
    };
    assert_eq!(proposal.state, ReportState::Open);
    assert_eq!(proposal.exposures, ["issue:github"]);
    assert_eq!(
        agent.audited.borrow().as_slice(),
        [(
            AUDIT_PROPOSED.to_owned(),
            format!("operation:G {}", proposal.id)
        )]
    );
    let limits = ReportLimits::default();
    let current = |who: &Holds| {
        search_reports(
            &registry,
            who,
            &rules,
            &ReportQuery::words("worth"),
            &limits,
        )
        .unwrap()
        .total
    };
    assert_eq!(current(&agent), 0, "a proposal is not current knowledge");

    // Not a reader who does not assess, nor one who may not read it; the Agent reads its own.
    let reader = Holds {
        reads: [operation.to_string()].into(),
        ..Holds::default()
    };
    assert!(matches!(
        read_report(&registry, &reader, &rules, &proposal.id),
        Err(KnowledgeRefusal::Missing)
    ));
    assert!(read_report(&registry, &agent, &rules, &proposal.id).is_ok());
    for who in [&agent, &reader] {
        assert!(decide_proposal(&registry, who, &rules, &proposal.id, true, "yes").is_err());
    }
    // A proposal behind a compartment the person is not read in to is not theirs to see.
    let hidden = Report::new(
        "hidden-1",
        operation.clone(),
        "memory",
        Severity::Info,
        "behind KESTREL",
        "text",
        agent.filer(),
        2.0,
    )
    .in_compartments(vec!["KESTREL".to_owned()]);
    registry.file_report(&hidden).unwrap();
    let person = Holds::of(&operation);
    assert!(matches!(
        decide_proposal(&registry, &person, &rules, "hidden-1", true, "yes"),
        Err(KnowledgeRefusal::Missing)
    ));

    // Accepted, it stands, with who asked as its filer, still marked, and who decided as the one who made it.
    let accepted = decide_proposal(&registry, &person, &rules, &proposal.id, true, "yes").unwrap();
    assert_eq!(accepted.state, ReportState::Standing);
    assert_eq!(accepted.filer, agent.filer());
    assert_eq!(accepted.exposures, ["issue:github"]);
    assert_eq!(accepted.updated_by.as_deref(), Some("person:ana"));
    assert_eq!(accepted.assessment.as_ref().unwrap().verdict, "accepted");
    assert_eq!(current(&person), 1);
    // Not accepted, it is closed and nothing is kept.
    let KnowledgeOutcome::Proposed(again) =
        write_report(&registry, &agent, &rules, new_in(&operation, "no", "text")).unwrap()
    else {
        panic!("an Agent proposes");
    };
    let closed = decide_proposal(&registry, &person, &rules, &again.id, false, "no").unwrap();
    assert_eq!(closed.state, ReportState::Closed);
    assert_eq!(
        closed.resolution.as_ref().unwrap().outcome.as_deref(),
        Some("no_action")
    );
    assert_eq!(current(&person), 1);

    // A rewrite waits too, and accepted it is the target's next revision, linked from the closed proposal.
    let KnowledgeOutcome::Proposed(rewrite) = write_report(
        &registry,
        &agent,
        &rules,
        existing(&accepted.id, 1, "rewritten"),
    )
    .unwrap() else {
        panic!("an Agent proposes a rewrite");
    };
    assert_eq!(rewrite.proposes.as_ref().unwrap().revision, 1);
    let applied = decide_proposal(&registry, &person, &rules, &rewrite.id, true, "yes").unwrap();
    assert_eq!(applied.state, ReportState::Closed);
    let target = registry.report(&accepted.id).unwrap().unwrap();
    assert_eq!((target.revision, target.text.as_str()), (2, "rewritten"));
    assert_eq!(target.state, ReportState::Standing);
    assert!(
        applied.links.iter().any(|link| link.id == accepted.id),
        "the closed proposal names what it changed"
    );
    assert_eq!(registry.report_versions(&accepted.id).unwrap().len(), 1);
    // A rewrite made from a revision the target has left is refused and left for a no.
    let KnowledgeOutcome::Proposed(stale) = write_report(
        &registry,
        &agent,
        &rules,
        existing(&accepted.id, 2, "stale soon"),
    )
    .unwrap() else {
        panic!("an Agent proposes a rewrite");
    };
    kept(
        write_report(
            &registry,
            &person,
            &rules,
            existing(&accepted.id, 2, "a person's own"),
        )
        .unwrap(),
    );
    assert!(decide_proposal(&registry, &person, &rules, &stale.id, true, "yes").is_err());
    assert_eq!(
        registry.report(&stale.id).unwrap().unwrap().state,
        ReportState::Open
    );

    // A forget proposed by an Agent waits too, and accepted a memory goes.
    let KnowledgeForgotten::Proposed(forgetting) =
        forget_report(&registry, &agent, &rules, &accepted.id, None).unwrap()
    else {
        panic!("an Agent proposes a forget");
    };
    assert!(forgetting.proposes.as_ref().unwrap().forget);
    assert!(registry.report(&accepted.id).unwrap().is_some());
    // Made from revision 3 (the person's), the target has not moved.
    decide_proposal(&registry, &person, &rules, &forgetting.id, true, "yes").unwrap();
    assert!(registry.report(&accepted.id).unwrap().is_none());
    assert!(registry.report_versions(&accepted.id).unwrap().is_empty());
}

#[test]
fn tradecraft_is_retired_not_forgotten_and_a_proposal_to_forget_it_retires_it() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    let person = Holds::of(&global());
    let mut change = new_in(&global(), "Raising a periscope", "Check the mast first.");
    change.target = KnowledgeTarget::New {
        scope: global(),
        kind: "tradecraft".to_owned(),
    };
    let guide = kept(write_report(&registry, &person, &rules, change).unwrap());
    let KnowledgeForgotten::Retired(retired) =
        forget_report(&registry, &person, &rules, &guide.id, None).unwrap()
    else {
        panic!("tradecraft is retired");
    };
    assert_eq!(retired.state, ReportState::Closed);
    assert_eq!(
        retired.resolution.as_ref().unwrap().outcome.as_deref(),
        Some("retired")
    );
    assert!(registry.report(&guide.id).unwrap().is_some(), "kept");
    assert!(
        search_reports(
            &registry,
            &person,
            &rules,
            &ReportQuery::words("periscope"),
            &ReportLimits::default()
        )
        .unwrap()
        .hits
        .is_empty(),
        "retired is not current"
    );
    assert!(
        person
            .audited
            .borrow()
            .iter()
            .any(|(action, _)| action == AUDIT_FORGET)
    );
}

#[test]
fn nobody_proposes_into_a_persons_memory() {
    let (_dir, registry) = store();
    let (settings, clearances) = rules();
    let rules = ReportRules::new(&settings, &clearances);
    // An Agent that reads Ana's scope by some grant still has no right to it.
    let agent = Holds {
        agent: true,
        reads: [ana().to_string()].into(),
        ..Holds::default()
    };
    assert!(matches!(
        write_report(&registry, &agent, &rules, new_in(&ana(), "into hers", "x")),
        Err(KnowledgeRefusal::Forbidden(_))
    ));
    assert!(registry.reports(&ReportFilter::all()).unwrap().is_empty());
}
