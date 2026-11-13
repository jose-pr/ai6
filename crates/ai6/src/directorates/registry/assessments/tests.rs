//! Assessing Reports, with a stand-in charge: it calls the same functions the charge's tool and
//! the gateway call, under its own name.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::directorates::registry::approvals::Ruling;
use crate::directorates::registry::logbook::Severity;
use crate::directorates::registry::reports::ReportFiler;
use crate::directorates::registry::sources::SourceItem;
use crate::directorates::registry::store::DEFAULT_LEASE;
use crate::directorates::registry::suite::{Store, TestRegistry, backend_tests};

backend_tests!(
    a_report_assessed_mission_closes_when_its_mission_settles,
    awaiting_comes_back_at_its_deadline_and_a_question_stays_within_the_handling_mark,
    a_decision_closes_when_its_green_ink_is_signed,
    a_duplicate_links_both_reports_and_a_crossing_is_checked,
    an_assessment_sets_credibility_priority_and_judgement,
    a_sources_reliability_follows_the_rule_in_the_settings,
);

const CHARGE: &str = "charge:operation:G";

fn agent() -> ReportFiler {
    ReportFiler::Agent {
        agent: "007".to_owned(),
        run: "run-1".to_owned(),
        mission: "G-1".to_owned(),
    }
}

fn operation(id: &str) -> ReportScope {
    ReportScope::Operation(id.to_owned())
}

fn filed(store: &dyn RegistryBackend, id: &str) -> Report {
    filed_in(store, id, "G")
}

fn filed_in(store: &dyn RegistryBackend, id: &str, scope: &str) -> Report {
    let report = Report::new(
        id,
        operation(scope),
        "finding",
        Severity::Info,
        format!("Title of {id}"),
        "What was seen.",
        agent(),
        100.0,
    );
    store.file_report(&report).unwrap();
    report
}

fn reads_g(scope: &ReportScope) -> bool {
    *scope == operation("G")
}

fn assessed(
    store: &dyn RegistryBackend,
    settings: &ReportSettings,
    id: &str,
    verdict: Verdict,
    at: f64,
) -> io::Result<Report> {
    assess(
        store,
        &Assessment::new(id, verdict, "from reading it", CHARGE, at),
        &Context::within(settings, &reads_g),
    )
}

fn audits(store: &dyn RegistryBackend, action: &str) -> usize {
    store
        .audit_records()
        .unwrap()
        .iter()
        .filter(|record| record.action == action)
        .count()
}

/// A Mission of `G` that exists in the Registry, claimed and so not yet settled; its UID.
fn mission(store: &dyn RegistryBackend, n: u32) -> String {
    let item = SourceItem {
        id: format!("o/r#{n}"),
        title: "Fix the build".to_owned(),
        body: "It fails.".to_owned(),
        url: None,
        labels: Vec::new(),
        author: None,
        updated: "2026-01-01T00:00:00Z".to_owned(),
        code: None,
        code_tag: None,
    };
    store
        .ingest_source_item("G", "github", &item, "station-a")
        .unwrap();
    let lease = store
        .claim_with_lease(None, "station-a", DEFAULT_LEASE, BTreeMap::new())
        .unwrap()
        .unwrap();
    lease.key().to_owned()
}

/// Settles the claimed Mission `uid`.
fn settle(store: &dyn RegistryBackend, uid: &str, ok: bool) {
    let brief = store.read_claimed(uid).unwrap().unwrap();
    let outcome = serde_json::json!({ "state": if ok { "done" } else { "failed" } });
    assert!(
        store
            .finish(&brief, &outcome, ok, None, Some("station-a"))
            .unwrap()
    );
}

fn a_report_assessed_mission_closes_when_its_mission_settles(store: &dyn Store) {
    let registry: Arc<dyn TestRegistry> = store.handle();
    let store = registry.as_ref();
    let settings = ReportSettings::default();
    let done = mission(store, 1);
    filed(store, "r-done");
    // The Mission has to be the Operation's own, and named.
    assert!(
        assessed(
            store,
            &settings,
            "r-done",
            Verdict::Mission { mission: None },
            200.0
        )
        .is_err()
    );
    let other = Verdict::Mission {
        mission: Some("no-such-mission".to_owned()),
    };
    assert!(assessed(store, &settings, "r-done", other, 200.0).is_err());

    let report = assessed(
        store,
        &settings,
        "r-done",
        Verdict::Mission {
            mission: Some(done.clone()),
        },
        200.0,
    )
    .unwrap();
    assert_eq!(report.state, ReportState::Assessed);
    assert!(report.about_mission(&done), "the Report names its Mission");
    assert_eq!(report.assessment.as_ref().unwrap().by, CHARGE);
    assert_eq!(audits(store, "report.assess"), 1);

    // While the Mission has not settled, nothing closes.
    assert!(sweep(store, 300.0, &settings).unwrap().closed.is_empty());
    assert_eq!(
        store.report("r-done").unwrap().unwrap().state,
        ReportState::Assessed
    );
    // An assessed Report is not assessed again.
    let again = assessed(store, &settings, "r-done", Verdict::NoAction, 210.0).unwrap_err();
    assert!(again.to_string().contains("only an open report"));

    settle(store, &done, true);
    let closed = sweep(store, 400.0, &settings).unwrap().closed;
    assert_eq!(closed.len(), 1);
    let report = store.report("r-done").unwrap().unwrap();
    assert_eq!(report.state, ReportState::Closed);
    let resolution = report.resolution.as_ref().unwrap();
    assert_eq!(resolution.outcome.as_deref(), Some("mission_done"));
    assert!(resolution.note.as_deref().unwrap().contains(&done));
    assert_eq!(resolution.at, 400.0);
    assert_eq!(audits(store, "report.close"), 1);
    // The resolution is kept, and closing twice changes nothing.
    assert!(sweep(store, 500.0, &settings).unwrap().closed.is_empty());
    assert_eq!(audits(store, "report.close"), 1);

    // The source's tally counts a Mission that settled done as the Report being right.
    let source = store.report_source("agent:007").unwrap();
    assert_eq!((source.counted, source.confirmed), (1, 1));
    assert_eq!(
        source.reliability,
        Reliability::F,
        "too few counted to rate yet"
    );

    // A Mission that settles failed closes its Report too, the moment the Station says so.
    let lost = mission(store, 2);
    filed(store, "r-failed");
    assessed(
        store,
        &settings,
        "r-failed",
        Verdict::Mission {
            mission: Some(lost.clone()),
        },
        600.0,
    )
    .unwrap();
    settle(store, &lost, false);
    let closed = mission_settled(store, &lost, 700.0, &settings).unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(
        closed[0].resolution.as_ref().unwrap().outcome.as_deref(),
        Some("mission_failed")
    );
    let source = store.report_source("agent:007").unwrap();
    assert_eq!((source.counted, source.confirmed), (2, 2));

    // Another Operation's Mission is refused.
    let item = SourceItem {
        id: "o/r#9".to_owned(),
        title: "Elsewhere".to_owned(),
        body: String::new(),
        url: None,
        labels: Vec::new(),
        author: None,
        updated: "2026-01-01T00:00:00Z".to_owned(),
        code: None,
        code_tag: None,
    };
    store
        .ingest_source_item("H", "github", &item, "station-a")
        .unwrap();
    let foreign = store.list_new().unwrap()[0].key();
    filed(store, "r-foreign");
    let refused = assessed(
        store,
        &settings,
        "r-foreign",
        Verdict::Mission {
            mission: Some(foreign),
        },
        800.0,
    )
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("not in the report's Operation")
    );
}

fn awaiting_comes_back_at_its_deadline_and_a_question_stays_within_the_handling_mark(
    store: &dyn Store,
) {
    let registry: Arc<dyn TestRegistry> = store.handle();
    let store = registry.as_ref();
    let settings = ReportSettings::default();
    filed(store, "r-ask");
    let question = |via: Option<&str>| Verdict::Awaiting {
        question: "Which branch is the target?".to_owned(),
        of: Some("cable:heads-of-station".to_owned()),
        via: via.map(str::to_owned),
        until: 1_000.0,
    };
    // A Report with no mark is the narrowest: its question may not leave the Operation.
    let refused = assessed(store, &settings, "r-ask", question(Some("peers")), 200.0).unwrap_err();
    assert!(
        refused.to_string().contains("may not go as far as peers"),
        "{refused}"
    );
    assert_eq!(
        store.report("r-ask").unwrap().unwrap().state,
        ReportState::Open
    );
    assert_eq!(
        audits(store, "report.assess"),
        0,
        "a refusal is not audited as given"
    );
    let unknown = assessed(store, &settings, "r-ask", question(Some("moon")), 200.0).unwrap_err();
    assert!(
        unknown
            .to_string()
            .contains("not one of the handling marks")
    );
    // A deadline that is not ahead is refused.
    let past = Verdict::Awaiting {
        question: "q".to_owned(),
        of: None,
        via: None,
        until: 150.0,
    };
    assert!(assessed(store, &settings, "r-ask", past, 200.0).is_err());

    store
        .change_report("r-ask", &mut |report| {
            report.handling = Some("peers".to_owned());
            Ok(())
        })
        .unwrap();
    let report = assessed(store, &settings, "r-ask", question(Some("peers")), 200.0).unwrap();
    assert_eq!(report.state, ReportState::Assessed);
    let assessment = report.assessment.as_ref().unwrap();
    assert_eq!(assessment.verdict, "awaiting");
    assert_eq!(assessment.until, Some(1_000.0));
    assert!(
        assessment
            .waits_for
            .as_deref()
            .unwrap()
            .contains("Which branch")
    );
    assert!(
        assessment
            .waits_for
            .as_deref()
            .unwrap()
            .contains("heads-of-station")
    );
    assert_eq!(audits(store, "report.assess"), 1);
    assert_eq!(audits(store, "report.cross"), 1, "the crossing is audited");

    // Not before its deadline; then it is open again, audited, with its assessment kept.
    assert!(sweep(store, 999.0, &settings).unwrap().reopened.is_empty());
    let swept = sweep(store, 1_000.0, &settings).unwrap();
    assert_eq!(swept.reopened.len(), 1);
    let report = store.report("r-ask").unwrap().unwrap();
    assert_eq!(report.state, ReportState::Open);
    assert_eq!(report.deadline, None);
    assert_eq!(report.assessment.as_ref().unwrap().verdict, "awaiting");
    assert_eq!(audits(store, "report.reopen"), 1);
    assert!(
        sweep(store, 2_000.0, &settings)
            .unwrap()
            .reopened
            .is_empty()
    );
    // It can be assessed again once open.
    assessed(store, &settings, "r-ask", Verdict::NoAction, 1_100.0).unwrap();

    // Later: a date, a condition, or both; a condition alone comes back only when reopened.
    filed(store, "r-later");
    let none = Verdict::Later {
        until: None,
        condition: None,
    };
    assert!(assessed(store, &settings, "r-later", none, 200.0).is_err());
    let report = assessed(
        store,
        &settings,
        "r-later",
        Verdict::Later {
            until: None,
            condition: Some("after the release".to_owned()),
        },
        200.0,
    )
    .unwrap();
    assert_eq!(report.state, ReportState::Assessed);
    assert!(
        sweep(store, 10_000.0, &settings)
            .unwrap()
            .reopened
            .is_empty()
    );
    assert!(reopen(store, "r-later", "person-1", " ", 300.0).is_err());
    let opened = reopen(store, "r-later", "person-1", "the release shipped", 300.0).unwrap();
    assert_eq!(opened.state, ReportState::Open);
    assert_eq!(audits(store, "report.reopen"), 2);
    assert!(
        reopen(store, "r-later", "person-1", "again", 310.0)
            .unwrap_err()
            .to_string()
            .contains("open already")
    );

    // A closed Report reopens too, and loses its resolution.
    let closed = store.report("r-ask").unwrap().unwrap();
    assert_eq!(closed.state, ReportState::Closed);
    let opened = reopen(store, "r-ask", "person-1", "it was not nothing", 400.0).unwrap();
    assert_eq!((opened.state, opened.resolution), (ReportState::Open, None));
}

fn a_decision_closes_when_its_green_ink_is_signed(store: &dyn Store) {
    let registry: Arc<dyn TestRegistry> = store.handle();
    let store = registry.as_ref();
    let settings = ReportSettings::default();
    let decide = |id: &str, at: f64, ttl: u64| {
        filed(store, id);
        let mut context = Context::within(&settings, &reads_g);
        context.request_ttl = Duration::from_secs(ttl);
        context.agent = "007";
        assess(
            store,
            &Assessment::new(
                id,
                Verdict::Decision {
                    question: "May we drop the old API?".to_owned(),
                },
                "it needs a person",
                CHARGE,
                at,
            ),
            &context,
        )
        .unwrap()
    };

    let report = decide("r-sign", 1_000.0, 3_600);
    assert_eq!(report.state, ReportState::Assessed);
    let link = report
        .links
        .iter()
        .find(|link| link.kind == ReportLinkKind::Decision)
        .expect("a decision links its request")
        .clone();
    let ApprovalStatus::Pending(request) = store.approval("G", &link.id, 1_001.0).unwrap().unwrap()
    else {
        panic!("the request waits for a signature");
    };
    assert_eq!(request.scope.kind, DECISION_KIND);
    assert!(request.scope.target.contains("r-sign"));
    assert_eq!(request.reason, "May we drop the old API?");
    assert!(sweep(store, 1_100.0, &settings).unwrap().closed.is_empty());

    store
        .decide_approval(
            "G",
            &link.id,
            &Ruling::approve("person-1", 1_200.0, 9_000.0),
        )
        .unwrap();
    let swept = sweep(store, 1_300.0, &settings).unwrap();
    assert_eq!(swept.closed.len(), 1);
    let report = store.report("r-sign").unwrap().unwrap();
    assert_eq!(report.state, ReportState::Closed);
    let resolution = report.resolution.as_ref().unwrap();
    assert_eq!(resolution.outcome.as_deref(), Some("decision_approved"));
    assert_eq!(resolution.note.as_deref(), Some("approved by person-1"));

    // A refusal closes it as rejected.
    let report = decide("r-refused", 2_000.0, 3_600);
    let id = report
        .links
        .iter()
        .find(|link| link.kind == ReportLinkKind::Decision)
        .unwrap()
        .id
        .clone();
    store
        .decide_approval("G", &id, &Ruling::reject("person-1", 2_100.0, None))
        .unwrap();
    sweep(store, 2_200.0, &settings).unwrap();
    let report = store.report("r-refused").unwrap().unwrap();
    assert_eq!(
        report.resolution.unwrap().outcome.as_deref(),
        Some("decision_rejected")
    );

    // A request nobody signs expires, and its Report comes back rather than stay closed on
    // an answer nobody gave.
    decide("r-silent", 3_000.0, 60);
    assert!(
        sweep(store, 3_030.0, &settings)
            .unwrap()
            .reopened
            .is_empty()
    );
    let swept = sweep(store, 3_100.0, &settings).unwrap();
    assert_eq!(swept.reopened.len(), 1);
    assert_eq!(
        store.report("r-silent").unwrap().unwrap().state,
        ReportState::Open
    );

    // The two signed ones counted toward the source: one confirmed, one not.
    let source = store.report_source("agent:007").unwrap();
    assert_eq!((source.counted, source.confirmed), (2, 1));
}

fn a_duplicate_links_both_reports_and_a_crossing_is_checked(store: &dyn Store) {
    let registry: Arc<dyn TestRegistry> = store.handle();
    let store = registry.as_ref();
    let settings = ReportSettings::default();
    filed(store, "r-first");
    filed(store, "r-second");
    let itself = Verdict::Duplicate {
        of: "r-first".to_owned(),
    };
    assert!(assessed(store, &settings, "r-first", itself, 200.0).is_err());

    let report = assessed(
        store,
        &settings,
        "r-second",
        Verdict::Duplicate {
            of: "r-first".to_owned(),
        },
        200.0,
    )
    .unwrap();
    assert_eq!(report.state, ReportState::Closed);
    assert_eq!(
        report.resolution.as_ref().unwrap().note.as_deref(),
        Some("duplicate of r-first")
    );
    let link = |kind, id: &str| ReportLink::new(kind, id);
    assert!(
        report
            .links
            .contains(&link(ReportLinkKind::Report, "r-first"))
    );
    let first = store.report("r-first").unwrap().unwrap();
    assert!(
        first
            .links
            .contains(&link(ReportLinkKind::Report, "r-second")),
        "the report it repeats names it back"
    );
    assert_eq!(
        first.state,
        ReportState::Open,
        "what it repeats is not closed by it"
    );
    // A duplicate is not counted toward its source: it says nothing of whether it was right.
    assert_eq!(store.report_source("agent:007").unwrap().counted, 0);

    // Related closes too, and names a Mission or evidence when it is not a Report.
    filed(store, "r-related");
    let related = assessed(
        store,
        &settings,
        "r-related",
        Verdict::Related {
            to: "build-log-17".to_owned(),
        },
        210.0,
    )
    .unwrap();
    assert_eq!(related.state, ReportState::Closed);
    assert!(
        related
            .links
            .contains(&link(ReportLinkKind::Evidence, "build-log-17"))
    );

    // Another Operation's Report: not unless the assessor reads it, and only as far as both
    // Reports' handling marks allow.
    filed_in(store, "r-other", "H");
    filed(store, "r-mine");
    let to_other = || Verdict::Duplicate {
        of: "r-other".to_owned(),
    };
    let refused = assessed(store, &settings, "r-mine", to_other(), 300.0).unwrap_err();
    assert!(
        refused.to_string().contains("scope you may not read"),
        "{refused}"
    );
    let reads_both = |_: &ReportScope| true;
    let crossing = |at: f64| {
        assess(
            store,
            &Assessment::new("r-mine", to_other(), "same fault", CHARGE, at),
            &Context::within(&settings, &reads_both),
        )
    };
    let refused = crossing(300.0).unwrap_err();
    assert!(refused.to_string().contains("narrowest mark"), "{refused}");
    for id in ["r-mine", "r-other"] {
        store
            .change_report(id, &mut |report| {
                report.handling = Some("compartments".to_owned());
                Ok(())
            })
            .unwrap();
    }
    let crossed = crossing(310.0).unwrap();
    assert_eq!(crossed.state, ReportState::Closed);
    assert_eq!(audits(store, "report.cross"), 1, "the crossing is audited");
    assert!(
        store
            .report("r-other")
            .unwrap()
            .unwrap()
            .links
            .contains(&link(ReportLinkKind::Report, "r-mine"))
    );
}

fn an_assessment_sets_credibility_priority_and_judgement(store: &dyn Store) {
    let registry: Arc<dyn TestRegistry> = store.handle();
    let store = registry.as_ref();
    let settings = ReportSettings::default();
    filed(store, "r-graded");
    let with = |credibility, priority: Option<&str>| {
        assess(
            store,
            &Assessment::new("r-graded", Verdict::NoAction, "known", CHARGE, 200.0)
                .believed(credibility)
                .prioritised(priority)
                .judged(Judgement::parse(Some("Highly-Likely"), Some("moderate")).unwrap()),
            &Context::within(&settings, &reads_g),
        )
    };
    assert!(with(Some(7), None).is_err(), "credibility is 1 to 6");
    assert!(with(Some(1), None).is_err(), "1 needs a corroborating link");
    assert!(
        with(None, Some("whenever")).is_err(),
        "a priority is from the list"
    );
    assert_eq!(
        audits(store, "report.assess"),
        0,
        "a refused assessment is not given"
    );
    assert_eq!(
        store.report("r-graded").unwrap().unwrap().state,
        ReportState::Open
    );

    let report = with(Some(3), Some("high")).unwrap();
    assert_eq!(
        (report.credibility, report.priority.as_deref()),
        (Some(3), Some("high"))
    );
    let judgement = report.assessment.as_ref().unwrap().judgement.unwrap();
    assert_eq!(judgement.likelihood, Likelihood::HighlyLikely);
    assert_eq!(judgement.confidence, Confidence::Moderate);
    assert_eq!(
        judgement.statement(),
        "highly likely (80-90%), moderate confidence"
    );
    assert_eq!(
        report.assessment.as_ref().unwrap().comment.as_deref(),
        Some("known"),
        "the comment is apart from the Report's words"
    );
    assert_eq!(report.text, "What was seen.");

    // 1 is allowed with a corroborating link, which a `related` tie provides.
    filed(store, "r-confirmed");
    let confirmed = assess(
        store,
        &Assessment::new(
            "r-confirmed",
            Verdict::Related {
                to: "probe-3".to_owned(),
            },
            "a probe read it back",
            CHARGE,
            210.0,
        )
        .believed(Some(1)),
        &Context::within(&settings, &reads_g),
    )
    .unwrap();
    assert_eq!(confirmed.credibility, Some(1));

    // Only both words, or neither.
    assert!(Judgement::parse(Some("likely"), None).is_err());
    assert_eq!(Judgement::parse(None, None).unwrap(), None);
    assert!(Judgement::parse(Some("probably"), Some("high")).is_err());
}

fn a_sources_reliability_follows_the_rule_in_the_settings(store: &dyn Store) {
    let registry: Arc<dyn TestRegistry> = store.handle();
    let store = registry.as_ref();
    let settings = ReportSettings {
        rating_min_reports: 2,
        ..ReportSettings::default()
    };
    // Two Reports the charge saw nothing in: counted, and not confirmed.
    for (n, id) in ["r-a", "r-b"].into_iter().enumerate() {
        filed(store, id);
        assessed(store, &settings, id, Verdict::NoAction, 200.0 + n as f64).unwrap();
    }
    let source = store.report_source("agent:007").unwrap();
    assert_eq!((source.counted, source.confirmed), (2, 0));
    assert_eq!(source.reliability, Reliability::E);
    assert_eq!(source.rated, 201.0);

    // The Reports behind the rating are shown with it.
    let standing = standing(store, "agent:007").unwrap();
    assert_eq!(standing.source.reliability, Reliability::E);
    let mut behind: Vec<&str> = standing
        .behind
        .iter()
        .map(|report| report.id.as_str())
        .collect();
    behind.sort_unstable();
    assert_eq!(behind, ["r-a", "r-b"]);
    assert!(
        standing.behind.iter().all(
            |report| report.resolution.as_ref().unwrap().outcome.as_deref() == Some("no_action")
        )
    );

    // Another source with Missions that settled done earns its way up, by the deployment's bands.
    for (n, id) in ["r-c", "r-d"].into_iter().enumerate() {
        store
            .file_report(&Report::new(
                id,
                operation("G"),
                "finding",
                Severity::Info,
                "t",
                "x",
                ReportFiler::Probe {
                    probe: "deliverables".to_owned(),
                },
                100.0,
            ))
            .unwrap();
        let uid = mission(store, 10 + n as u32);
        assessed(
            store,
            &settings,
            id,
            Verdict::Mission {
                mission: Some(uid.clone()),
            },
            200.0,
        )
        .unwrap();
        settle(store, &uid, true);
    }
    sweep(store, 300.0, &settings).unwrap();
    let probe = store.report_source("probe:deliverables").unwrap();
    assert_eq!((probe.counted, probe.confirmed), (2, 2));
    assert_eq!(probe.reliability, Reliability::A);

    // An outcome the rule does not count leaves the tally alone.
    let stricter = ReportSettings {
        rating_counts: vec!["mission_done".to_owned()],
        ..settings.clone()
    };
    filed(store, "r-e");
    assessed(store, &stricter, "r-e", Verdict::NoAction, 400.0).unwrap();
    assert_eq!(store.report_source("agent:007").unwrap().counted, 2);
}

#[test]
fn the_probability_words_and_their_bands_are_the_yardsticks() {
    let words: Vec<(String, &str)> = [
        "remote chance",
        "highly unlikely",
        "unlikely",
        "realistic_possibility",
        "LIKELY",
        "highly-likely",
        "almost certain",
    ]
    .into_iter()
    .map(|word| {
        let parsed: Likelihood = word.parse().unwrap();
        (parsed.words().to_owned(), parsed.band())
    })
    .collect();
    assert_eq!(
        words,
        [
            ("remote chance".to_owned(), "under 5%"),
            ("highly unlikely".to_owned(), "10-20%"),
            ("unlikely".to_owned(), "25-35%"),
            ("realistic possibility".to_owned(), "40-50%"),
            ("likely".to_owned(), "55-75%"),
            ("highly likely".to_owned(), "80-90%"),
            ("almost certain".to_owned(), "over 95%"),
        ]
    );
    assert!("certain".parse::<Likelihood>().is_err());
    assert!("sure".parse::<Confidence>().is_err());
    assert_eq!("High".parse::<Confidence>().unwrap(), Confidence::High);
}

#[test]
fn a_verdict_takes_only_what_it_needs() {
    let parse = |word: &str, parts: VerdictParts<'_>| Verdict::parse(word, &parts, 100.0);
    assert_eq!(
        parse("mission", VerdictParts::default()),
        Ok(Verdict::Mission { mission: None })
    );
    assert!(parse("duplicate", VerdictParts::default()).is_err());
    assert!(parse("related", VerdictParts::default()).is_err());
    assert!(
        parse(
            "awaiting",
            VerdictParts {
                question: Some("q"),
                ..VerdictParts::default()
            }
        )
        .is_err()
    );
    assert!(parse("decision", VerdictParts::default()).is_err());
    assert!(parse("later", VerdictParts::default()).is_err());
    assert!(
        parse(
            "no_action",
            VerdictParts {
                of: Some("x"),
                ..VerdictParts::default()
            }
        )
        .is_err()
    );
    assert!(
        parse(
            "mission",
            VerdictParts {
                wait_seconds: Some(5),
                ..VerdictParts::default()
            }
        )
        .is_err()
    );
    assert!(parse("closed", VerdictParts::default()).is_err());
    assert_eq!(
        parse(
            "later",
            VerdictParts {
                wait_seconds: Some(60),
                condition: Some("after the release"),
                ..VerdictParts::default()
            }
        ),
        Ok(Verdict::Later {
            until: Some(160.0),
            condition: Some("after the release".to_owned())
        })
    );
}

#[test]
fn a_tally_becomes_a_reliability_by_the_bands() {
    let bands = [90, 75, 50, 25];
    let of = |confirmed, counted| reliability_of(confirmed, counted, &bands);
    assert_eq!(of(10, 10), Reliability::A);
    assert_eq!(of(8, 10), Reliability::B);
    assert_eq!(of(5, 10), Reliability::C);
    assert_eq!(of(3, 10), Reliability::D);
    assert_eq!(of(2, 10), Reliability::E);
    assert_eq!(of(0, 0), Reliability::E);
    assert_eq!(
        reliability_of(10, 10, &[]),
        Reliability::E,
        "no bands, no letters above E"
    );
}

#[test]
fn a_handling_mark_lets_a_report_go_as_far_as_it_says() {
    let settings = ReportSettings::default();
    let mut report = Report::new(
        "r",
        operation("G"),
        "finding",
        Severity::Info,
        "t",
        "x",
        agent(),
        100.0,
    );
    assert!(settings.permits(&report, "operation").is_ok());
    assert!(
        settings.permits(&report, "compartments").is_err(),
        "unmarked is the narrowest"
    );
    report.handling = Some("compartments".to_owned());
    assert!(settings.permits(&report, "compartments").is_ok());
    assert!(settings.permits(&report, "peers").is_err());
    report.handling = Some("peers".to_owned());
    assert!(settings.permits(&report, "peers").is_ok());
    assert!(
        settings.permits(&report, "tracker").is_err(),
        "an unknown crossing is refused"
    );
}
