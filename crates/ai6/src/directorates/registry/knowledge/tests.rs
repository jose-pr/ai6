//! The memory cases of the memory store, written against Reports of kind `memory`: every one
//! runs on the file Registry, SQLite and PostgreSQL.

use super::*;
use crate::bounds::Limit;
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::logbook::Severity;
use crate::directorates::registry::reports::{
    Report, ReportFiler, ReportFilter, ReportProposes, ReportScope, ReportState, ReportStore,
};
use crate::directorates::registry::suite::{Store, backend_tests};

backend_tests!(
    two_reports_may_share_a_title_and_a_summary,
    scopes_are_kept_apart,
    a_write_from_an_old_revision_is_refused,
    what_a_scope_may_hold_is_limited,
    a_search_ranks_a_match_over_a_non_match,
    a_search_finds_a_word_only_as_a_word,
    a_search_finds_every_word_at_any_length_of_query,
    a_search_filters_by_kind_topics_title_and_scope,
    prior_revisions_are_kept_to_the_limit_and_go_with_a_forget,
    an_id_that_is_not_one_names_nothing,
    proposals_wait_per_scope_within_their_limit,
    only_a_memory_is_forgotten,
    a_known_kind_is_standing_and_an_observation_is_not,
    what_a_report_held_goes_when_it_ages_out_closed,
);

fn person(id: &str) -> ReportScope {
    ReportScope::Person(id.to_owned())
}

fn filer() -> ReportFiler {
    ReportFiler::Person {
        person: "ana".to_owned(),
    }
}

/// A new standing memory with a fresh ID.
fn memory(scope: ReportScope, title: &str, summary: &str, text: &str) -> Report {
    Report::new(
        uuid::Uuid::new_v4().to_string(),
        scope,
        "memory",
        Severity::Info,
        title,
        text,
        filer(),
        10.0,
    )
    .summarised(summary)
    .standing()
}

fn found(registry: &dyn RegistryBackend, scopes: &[ReportScope], query: &str) -> Vec<String> {
    let query = ReportQuery::words(query);
    matching(registry.report_candidates(scopes, &query).unwrap(), &query)
        .into_iter()
        .map(|report| report.id)
        .collect()
}

fn two_reports_may_share_a_title_and_a_summary(store: &dyn Store) {
    let registry = store.handle();
    let first = memory(person("ana"), "prefs", "How she likes it", "tables");
    let second = memory(person("ana"), "prefs", "How she likes it", "charts");
    registry.file_report(&first).unwrap();
    registry.file_report(&second).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(first.revision, 1);
    let listed = registry
        .reports(&ReportFilter::all().in_scope(person("ana")))
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(registry.report(&second.id).unwrap().unwrap().text, "charts");
    // Taking an ID already there changes nothing.
    let mut again = second.clone();
    again.text = "over it".to_owned();
    assert_eq!(
        registry.file_report(&again).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    for bad in [
        memory(person("ana"), "t", "", "x"),
        memory(person("ana"), "t", "two\nlines", "x"),
        memory(person("ana"), "", "s", "x"),
        memory(person("ana"), "t", "s", "nul\0"),
        memory(person("ana"), "t", "s", "  "),
        memory(person(""), "t", "s", "x"),
    ] {
        assert_eq!(
            registry.file_report(&bad).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput,
            "out of bounds: {bad:?}"
        );
    }
    let mut not_an_id = memory(person("ana"), "t", "s", "x");
    not_an_id.id = "../../elsewhere".to_owned();
    assert_eq!(
        registry.file_report(&not_an_id).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
}

fn scopes_are_kept_apart(store: &dyn Store) {
    let registry = store.handle();
    let hers = memory(person("ana"), "ana's own", "s", "x");
    // Two people whose IDs differ only by case stay two.
    let other = memory(person("ANA"), "another's", "s", "y");
    let ops = memory(ReportScope::Operation("ana".to_owned()), "op", "s", "z");
    let everyone = memory(ReportScope::Global, "all", "s", "w");
    for each in [&hers, &other, &ops, &everyone] {
        registry.file_report(each).unwrap();
    }
    for (scope, id) in [
        (person("ana"), &hers.id),
        (person("ANA"), &other.id),
        (ReportScope::Operation("ana".to_owned()), &ops.id),
        (ReportScope::Global, &everyone.id),
    ] {
        let listed = registry
            .reports(&ReportFilter::all().in_scope(scope.clone()))
            .unwrap();
        assert_eq!(
            listed.iter().map(|report| &report.id).collect::<Vec<_>>(),
            [id],
            "{scope}"
        );
    }
    assert!(
        registry
            .reports(&ReportFilter::all().in_scope(person("nobody")))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        found(registry.as_ref(), &[person("ana")], "ana"),
        std::slice::from_ref(&hers.id)
    );
    assert_eq!(
        found(registry.as_ref(), &[person("ANA")], "another"),
        std::slice::from_ref(&other.id)
    );
    assert!(found(registry.as_ref(), &[person("ana")], "another").is_empty());
    let mut scopes = registry.report_scopes().unwrap();
    scopes.sort_by_key(ToString::to_string);
    assert_eq!(scopes.len(), 4);
    // A Report cannot be moved to another scope by naming its ID there.
    let mut moved = hers.clone();
    moved.scope = person("bob");
    assert!(matches!(
        registry.write_report(&moved, 1),
        Err(ReportWriteError::Missing)
    ));
    assert_eq!(registry.report(&hers.id).unwrap().unwrap(), hers);
    assert!(
        registry
            .reports(&ReportFilter::all().in_scope(person("bob")))
            .unwrap()
            .is_empty()
    );
}

fn a_write_from_an_old_revision_is_refused(store: &dyn Store) {
    let registry = store.handle();
    let first = memory(person("ana"), "t", "s", "one");
    registry.file_report(&first).unwrap();
    let mut second = first.clone();
    second.text = "two".to_owned();
    second.created = 5.0;
    let second = registry.write_report(&second, 1).unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(second.created, 10.0, "made when it was made");
    let mut late = first.clone();
    late.text = "late".to_owned();
    assert!(matches!(
        registry.write_report(&late, 1),
        Err(ReportWriteError::Changed { revision: 2 })
    ));
    assert_eq!(registry.report(&first.id).unwrap().unwrap().text, "two");
    assert!(matches!(
        registry.forget_report(&first.id, Some(1)),
        Err(ReportWriteError::Changed { revision: 2 })
    ));
    assert!(registry.report(&first.id).unwrap().is_some());
    let gone = memory(person("ana"), "t", "s", "gone");
    assert!(matches!(
        registry.write_report(&gone, 1),
        Err(ReportWriteError::Missing)
    ));
}

fn what_a_scope_may_hold_is_limited(store: &dyn Store) {
    let registry = store.handle();
    let limits = registry.limits().reports.clone();
    // A Report over the text limit is refused for what it is.
    let huge = "x".repeat(limits.text_bytes.or_max() + 1);
    assert_eq!(
        registry
            .file_report(&memory(person("ana"), "t", "s", &huge))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
    // Filling a scope to its total stops the next, and another scope is not touched.
    let large = "y".repeat(limits.text_bytes.or_max());
    let registry = store.handle_with(crate::directorates::registry::limits::RegistryLimits {
        reports: crate::directorates::registry::limits::ReportLimits {
            scope_bytes: Limit::at(2 * limits.text_bytes.or_max() as u64 + 1_024),
            ..Default::default()
        },
        ..Default::default()
    });
    let mut kept = Vec::new();
    for _ in 0..3 {
        let next = memory(person("ana"), "t", "s", &large);
        match registry.file_report(&next) {
            Ok(()) => kept.push(next),
            Err(error) if error.kind() == std::io::ErrorKind::QuotaExceeded => break,
            Err(error) => panic!("{error}"),
        }
    }
    assert_eq!(kept.len(), 2);
    assert_eq!(
        registry
            .file_report(&memory(person("ana"), "t", "s", &large))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::QuotaExceeded,
        "the total stops it"
    );
    registry
        .file_report(&memory(person("bob"), "t", "s", &large))
        .unwrap();
    // A forget makes room.
    let first = kept.remove(0);
    registry.forget_report(&first.id, None).unwrap();
    registry
        .file_report(&memory(person("ana"), "t", "s", &large))
        .unwrap();
    // The count of Reports in a scope is bounded too.
    let few = store.handle_with(crate::directorates::registry::limits::RegistryLimits {
        reports: crate::directorates::registry::limits::ReportLimits {
            scope_entries: Limit::at(2),
            ..Default::default()
        },
        ..Default::default()
    });
    for _ in 0..2 {
        few.file_report(&memory(ReportScope::Global, "t", "s", "x"))
            .unwrap();
    }
    assert_eq!(
        few.file_report(&memory(ReportScope::Global, "t", "s", "x"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::QuotaExceeded
    );
}

fn a_search_ranks_a_match_over_a_non_match(store: &dyn Store) {
    let registry = store.handle();
    let scopes = [person("ana")];
    let write = |title: &str, summary: &str, text: &str| {
        let report = memory(person("ana"), title, summary, text);
        registry.file_report(&report).unwrap();
        report.id
    };
    let none = write("Nothing", "Nothing here", "unrelated words");
    let body = write("Else", "Something else", "she prefers tables in reports");
    let title = write("Likes", "Prefers tables", "details below");
    let both = write("tables", "Reports", "she prefers tables");
    // All three hold both words; more of them in the title or summary ranks first.
    assert_eq!(
        found(registry.as_ref(), &scopes, "prefers tables"),
        [title.clone(), both.clone(), body.clone()]
    );
    let any = found(registry.as_ref(), &scopes, "Prefers ZEBRA");
    assert_eq!(any.len(), 3, "one word of two is a match: {any:?}");
    assert!(!any.contains(&none));
    assert!(found(registry.as_ref(), &scopes, "zebra").is_empty());
    // No words at all lists everything.
    assert_eq!(found(registry.as_ref(), &scopes, " ... ").len(), 4);
}

fn a_search_finds_a_word_only_as_a_word(store: &dyn Store) {
    let registry = store.handle();
    let scopes = [person("ana")];
    let hyphen = memory(
        person("ana"),
        "Host",
        "where",
        "reach api-gateway.example.com:8443",
    );
    let accent = memory(person("ana"), "Café", "drink", "Mañana");
    registry.file_report(&hyphen).unwrap();
    registry.file_report(&accent).unwrap();
    for query in ["gateway", "EXAMPLE", "api-gateway", "8443", "com"] {
        assert_eq!(
            found(registry.as_ref(), &scopes, query),
            std::slice::from_ref(&hyphen.id),
            "{query}"
        );
    }
    assert!(found(registry.as_ref(), &scopes, "gate").is_empty());
    assert_eq!(
        found(registry.as_ref(), &scopes, "mañana"),
        std::slice::from_ref(&accent.id)
    );
    assert_eq!(found(registry.as_ref(), &scopes, "CAFÉ"), [accent.id]);
    // Syntax of an index's own query language is only words.
    for query in ["\"", "a' | 'b", "NOT", "*", "col:x", "x OR", "(", "\\"] {
        let _ = found(registry.as_ref(), &scopes, query);
    }
}

fn a_search_finds_every_word_at_any_length_of_query(store: &dyn Store) {
    let registry = store.handle();
    let needle = memory(person("ana"), "needle", "s", "x");
    registry.file_report(&needle).unwrap();
    let many: Vec<String> = (0..2_000).map(|n| format!("w{n}")).collect();
    let query = format!("{} needle", many.join(" "));
    assert_eq!(
        found(registry.as_ref(), &[person("ana")], &query),
        [needle.id]
    );
}

fn a_search_filters_by_kind_topics_title_and_scope(store: &dyn Store) {
    let registry = store.handle();
    let brief = {
        let mut report = memory(
            ReportScope::Global,
            "Writing a brief",
            "How a brief is written",
            "Name the goal first.",
        )
        .on_topics(["Briefs", "writing"]);
        report.kind = "tradecraft".to_owned();
        report
    };
    let note = memory(ReportScope::Global, "A note", "s", "about a brief too").on_topics(["notes"]);
    let finding = Report::new(
        "obs-1",
        ReportScope::Operation("G".to_owned()),
        "finding",
        Severity::Info,
        "Brief seen",
        "a brief was seen",
        ReportFiler::Watchdog,
        5.0,
    );
    for each in [&brief, &note, &finding] {
        registry.file_report(each).unwrap();
    }
    let scopes = [ReportScope::Global, ReportScope::Operation("G".to_owned())];
    let search = |query: ReportQuery| -> Vec<String> {
        matching(registry.report_candidates(&scopes, &query).unwrap(), &query)
            .into_iter()
            .map(|report| report.id)
            .collect()
    };
    let words = ReportQuery::words("brief");
    assert_eq!(search(words.clone()).len(), 3, "every kind, one search");
    assert_eq!(
        search(words.clone().of_kind("tradecraft")),
        std::slice::from_ref(&brief.id)
    );
    assert_eq!(
        search(ReportQuery::default().on_topic("briefs")),
        std::slice::from_ref(&brief.id),
        "a topic is found whatever its case"
    );
    assert_eq!(
        search(ReportQuery::default().titled("writing a")),
        std::slice::from_ref(&brief.id)
    );
    assert_eq!(
        search(
            words
                .clone()
                .in_scope(ReportScope::Operation("G".to_owned()))
        ),
        ["obs-1"]
    );
    assert_eq!(
        search(ReportQuery::words("writing")),
        std::slice::from_ref(&brief.id),
        "topics are searched"
    );
    // A proposal waits out of the current knowledge; retired tradecraft is out of it too.
    let mut proposal = memory(ReportScope::Global, "A proposal", "s", "brief brief brief");
    proposal.state = ReportState::Open;
    registry.file_report(&proposal).unwrap();
    assert_eq!(search(words.clone()).len(), 3);
    assert_eq!(
        search(words.clone().in_state(ReportState::Open)),
        ["obs-1".to_owned(), proposal.id.clone()],
        "an observation is open too; the one with the word in its title ranks first"
    );
    assert_eq!(
        search(words.in_state(ReportState::Open).of_kind("memory")),
        [proposal.id.clone()]
    );
}

fn prior_revisions_are_kept_to_the_limit_and_go_with_a_forget(store: &dyn Store) {
    let registry = store.handle();
    let keep = registry.limits().reports.versions.or_max();
    let mut current = memory(person("ana"), "t", "s", "v1");
    registry.file_report(&current).unwrap();
    let other = memory(person("ana"), "t", "s", "other");
    registry.file_report(&other).unwrap();
    let mut second = other.clone();
    second.text = "other two".to_owned();
    registry.write_report(&second, 1).unwrap();
    for revision in 1..=(keep as u64 + 2) {
        let mut next = current.clone();
        next.text = format!("v{}", revision + 1);
        current = registry.write_report(&next, revision).unwrap();
    }
    let versions = registry.report_versions(&current.id).unwrap();
    assert_eq!(versions.len(), keep);
    assert_eq!(versions[0].text, format!("v{}", keep + 2), "newest first");
    assert_eq!(
        versions.last().unwrap().revision,
        current.revision - keep as u64
    );
    let forgotten = registry.forget_report(&current.id, None).unwrap();
    assert_eq!(forgotten.id, current.id);
    assert!(registry.report(&current.id).unwrap().is_none());
    assert!(
        registry.report_versions(&current.id).unwrap().is_empty(),
        "nothing of it is left"
    );
    assert_eq!(
        registry.report_versions(&other.id).unwrap().len(),
        1,
        "another Report keeps its own"
    );
    assert!(matches!(
        registry.forget_report(&current.id, None),
        Err(ReportWriteError::Missing)
    ));
    // Nothing of it can be found by what it said.
    assert!(found(registry.as_ref(), &[person("ana")], "v3").is_empty());
}

fn an_id_that_is_not_one_names_nothing(store: &dyn Store) {
    let registry = store.handle();
    let written = memory(person("ana"), "t", "s", "x");
    registry.file_report(&written).unwrap();
    for id in [
        "",
        "..",
        "../x",
        "not-a-uuid",
        "a/b",
        written.id.to_uppercase().as_str(),
        written.id.replace('-', "").as_str(),
    ] {
        assert!(registry.report(id).ok().flatten().is_none(), "{id}");
        assert!(registry.report_versions(id).unwrap().is_empty(), "{id}");
        assert!(
            matches!(
                registry.forget_report(id, None),
                Err(ReportWriteError::Missing)
            ),
            "{id}"
        );
    }
    assert!(registry.report(&written.id).unwrap().is_some());
}

fn proposal(scope: ReportScope, summary: &str) -> Report {
    let mut report = Report::new(
        uuid::Uuid::new_v4().to_string(),
        scope,
        "memory",
        Severity::Info,
        "a proposal",
        "text",
        ReportFiler::Agent {
            agent: "007".to_owned(),
            run: "G-1".to_owned(),
            mission: "G-1".to_owned(),
        },
        10.0,
    )
    .summarised(summary)
    .exposed_by(["tool:WebFetch".to_owned()]);
    report.state = ReportState::Open;
    report
}

fn proposals_wait_per_scope_within_their_limit(store: &dyn Store) {
    let registry = store.handle_with(crate::directorates::registry::limits::RegistryLimits {
        reports: crate::directorates::registry::limits::ReportLimits {
            per_run: Limit::UNBOUNDED,
            per_operation: Limit::UNBOUNDED,
            ..Default::default()
        },
        ..Default::default()
    });
    let operation = ReportScope::Operation("G".to_owned());
    let waiting = |scope: &ReportScope| {
        registry
            .reports(
                &ReportFilter::all()
                    .in_scope(scope.clone())
                    .in_state(ReportState::Open),
            )
            .unwrap()
    };
    assert!(waiting(&operation).is_empty());
    let first = proposal(operation.clone(), "first");
    registry.file_report(&first).unwrap();
    let mut later = proposal(operation.clone(), "later");
    later.created = 20.0;
    later.updated = 20.0;
    registry.file_report(&later).unwrap();
    registry
        .file_report(&proposal(ReportScope::Global, "elsewhere"))
        .unwrap();
    let all = waiting(&operation);
    assert_eq!(
        all.iter()
            .map(|one| one.summary.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["first", "later"]
    );
    assert_eq!(all[0].exposures, ["tool:WebFetch"], "its mark stays");
    assert_eq!(registry.report(&first.id).unwrap().unwrap(), first);
    let mut bad = proposal(operation.clone(), "two\nlines");
    assert_eq!(
        registry.file_report(&bad).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    bad.summary = Some("s".to_owned());
    bad.proposes = Some(ReportProposes::rewrite("../x", 1));
    assert_eq!(
        registry.file_report(&bad).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    // Past the limit the next is refused, in that scope alone.
    let room = registry.limits().reports.proposals.or_max();
    for index in 2..room {
        registry
            .file_report(&proposal(operation.clone(), &format!("{index}")))
            .unwrap();
    }
    assert_eq!(
        registry
            .file_report(&proposal(operation.clone(), "one too many"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::QuotaExceeded
    );
    registry
        .file_report(&proposal(ReportScope::Global, "another scope"))
        .unwrap();
}

fn only_a_memory_is_forgotten(store: &dyn Store) {
    let registry = store.handle();
    let mut rule = memory(ReportScope::Global, "A rule", "s", "x");
    rule.kind = "rule".to_owned();
    registry.file_report(&rule).unwrap();
    let observed = Report::new(
        "obs-2",
        ReportScope::Operation("G".to_owned()),
        "finding",
        Severity::Info,
        "t",
        "x",
        ReportFiler::Watchdog,
        1.0,
    );
    registry.file_report(&observed).unwrap();
    for id in [&rule.id, &observed.id] {
        assert!(matches!(
            registry.forget_report(id, None),
            Err(ReportWriteError::Failed(error)) if error.kind() == std::io::ErrorKind::InvalidInput
        ));
        assert!(registry.report(id).unwrap().is_some());
    }
}

fn a_known_kind_is_standing_and_an_observation_is_not(store: &dyn Store) {
    let registry = store.handle();
    let mut observed = Report::new(
        "obs-3",
        ReportScope::Operation("G".to_owned()),
        "finding",
        Severity::Info,
        "t",
        "x",
        ReportFiler::Watchdog,
        1.0,
    );
    observed.state = ReportState::Standing;
    assert_eq!(
        registry.file_report(&observed).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput,
        "an observation does not stand"
    );
    let mut person_finding = observed.clone();
    person_finding.id = "obs-4".to_owned();
    person_finding.state = ReportState::Open;
    person_finding.scope = person("ana");
    assert_eq!(
        registry.file_report(&person_finding).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput,
        "an observation is not a person's"
    );
    let mut assistant = memory(person("ana"), "t", "s", "x");
    assistant.filer = ReportFiler::Assistant {
        person: "ana".to_owned(),
    };
    registry.file_report(&assistant).unwrap();
    assert!(registry.report(&assistant.id).unwrap().unwrap().untrusted());
    assert_eq!(
        registry
            .reports(&ReportFilter::all().in_state(ReportState::Standing))
            .unwrap()
            .len(),
        1
    );
}

fn what_a_report_held_goes_when_it_ages_out_closed(store: &dyn Store) {
    use crate::bounds::TimeLimit;
    use crate::directorates::registry::limits::{RegistryLimits, ReportLimits};

    let registry = store.handle_with(RegistryLimits {
        reports: ReportLimits {
            keep_closed: TimeLimit::secs(100),
            ..ReportLimits::default()
        },
        ..RegistryLimits::default()
    });
    let mut guide = memory(ReportScope::Global, "A rule", "s", "one");
    guide.kind = "rule".to_owned();
    registry.file_report(&guide).unwrap();
    let mut next = guide.clone();
    next.text = "two".to_owned();
    registry.write_report(&next, 1).unwrap();
    assert_eq!(registry.report_versions(&guide.id).unwrap().len(), 1);
    // Retired: closed, and kept, with what it held.
    registry
        .change_report(&guide.id, &mut |report| {
            report.state = ReportState::Closed;
            report.updated = 20.0;
            Ok(())
        })
        .unwrap();
    assert!(registry.report(&guide.id).unwrap().is_some());
    assert_eq!(registry.report_versions(&guide.id).unwrap().len(), 1);
    // Past its age it goes, and so does what it held.
    let mut later = memory(ReportScope::Global, "Another", "s", "x");
    later.created = 500.0;
    later.updated = 500.0;
    registry.file_report(&later).unwrap();
    assert!(registry.report(&guide.id).unwrap().is_none());
    assert!(registry.report_versions(&guide.id).unwrap().is_empty());
}

#[test]
fn a_scope_written_as_text_reads_back_as_itself() {
    for (text, scope) in [
        ("person:ana", person("ana")),
        ("person:a:b", person("a:b")),
        ("operation:G", ReportScope::Operation("G".to_owned())),
        ("station:s", ReportScope::Station("s".to_owned())),
        ("global", ReportScope::Global),
        ("m", ReportScope::M),
    ] {
        assert_eq!(text.parse::<ReportScope>().as_ref(), Ok(&scope));
        assert_eq!(scope.to_string(), text);
        let json = serde_json::to_string(&scope).unwrap();
        assert_eq!(serde_json::from_str::<ReportScope>(&json).unwrap(), scope);
    }
    for bad in ["", "person:", "operation:", "global:x", "person"] {
        assert!(bad.parse::<ReportScope>().is_err(), "{bad}");
    }
}

#[test]
fn a_file_registry_keeps_no_revision_when_told_to_keep_none() {
    use crate::directorates::registry::limits::RegistryLimits;
    use crate::directorates::registry::store::FileRegistry;
    let dir = tempfile::tempdir().unwrap();
    let mut limits = RegistryLimits::default();
    limits.reports.versions = Limit::at(0);
    let registry = FileRegistry::open(dir.path()).with_limits(limits);
    let first = memory(person("ana"), "t", "s", "one");
    registry.file_report(&first).unwrap();
    let mut next = first.clone();
    next.text = "two".to_owned();
    registry.write_report(&next, 1).unwrap();
    assert!(registry.report_versions(&first.id).unwrap().is_empty());
}

#[test]
fn the_index_holds_each_word_once_within_its_bound() {
    let written =
        memory(person("ana"), "Tables", "tables TABLES", "Charts, tables.").on_topics(["Prefs"]);
    assert_eq!(
        index_words(&written, Limit::UNBOUNDED),
        "charts prefs tables "
    );
    assert_eq!(index_words(&written, Limit::at(7)), "charts ");
    assert_eq!(index_words(&written, Limit::at(0)), "");
}

#[test]
fn a_stored_report_without_the_knowledge_fields_reads_as_it_was() {
    let old = r#"{"id":"r","scope":"operation:G","kind":"finding","severity":"info","title":"t","text":"x","filer":{"type":"watchdog"},"links":[],"state":"open","created":1.0,"updated":1.0}"#;
    let report: Report = serde_json::from_str(old).unwrap();
    assert_eq!(report.revision, 1);
    assert!(report.summary.is_none() && report.topics.is_empty() && report.exposures.is_empty());
    assert!(
        !serde_json::to_string(&report).unwrap().contains("topics"),
        "an unset field is not written"
    );
}
