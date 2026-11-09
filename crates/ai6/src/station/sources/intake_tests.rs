//! A Source's items as Reports for the watch: what is filed and told, what becomes a brief, and
//! that nothing becomes one by itself.

use std::sync::Mutex;

use guatiao::library::ProviderError;

use super::*;
use crate::directorates::registry::reports::{ReportFilter, ReportScope, ReportState};
use crate::directorates::registry::source_items::{
    ITEM_KIND, ItemDecision, ItemVerdict, decide_item,
};
use crate::directorates::registry::sources::{SourceBatch, SourceItem};
use crate::directorates::registry::store::FileRegistry;

/// Serves the items it is set to, every poll.
struct Items(Mutex<Vec<SourceItem>>);

impl Items {
    fn of(items: Vec<SourceItem>) -> Self {
        Self(Mutex::new(items))
    }

    fn set(&self, items: Vec<SourceItem>) {
        *self.0.lock().unwrap() = items;
    }
}

impl Source for Items {
    fn poll(&self, _request: SourcePoll) -> Result<SourceBatch, ProviderError> {
        Ok(SourceBatch {
            items: self.0.lock().unwrap().clone(),
            cursor: None,
        })
    }
}

fn item(id: &str, body: &str) -> SourceItem {
    SourceItem {
        id: id.to_owned(),
        title: format!("Item {id}"),
        body: body.to_owned(),
        url: Some(format!("https://example.invalid/{id}")),
        labels: vec!["bug".to_owned()],
        author: Some("reporter".to_owned()),
        updated: "2026-01-01T00:00:00Z".to_owned(),
        code: None,
        code_tag: None,
    }
}

/// The notices a watch was told, as (instance, body, key).
type Told = Mutex<Vec<(String, String, String)>>;

fn recorder(told: &Told) -> impl Fn(&str, &str, &str) -> Result<(), String> + '_ {
    move |instance, body, key| {
        told.lock()
            .unwrap()
            .push((instance.to_owned(), body.to_owned(), key.to_owned()));
        Ok(())
    }
}

fn poll(
    registry: &FileRegistry,
    source: &Items,
    intake: &Intake<'_>,
) -> PollSummary {
    poll_source(
        registry,
        "station-a",
        ("G", "github"),
        false,
        source,
        &Reopening::never(),
        intake,
    )
    .unwrap()
}

fn reports(registry: &FileRegistry) -> Vec<Report> {
    registry
        .reports(
            &ReportFilter::all()
                .in_scope(ReportScope::Operation("G".to_owned()))
                .of_kind(ITEM_KIND),
        )
        .unwrap()
}

fn setup() -> SourceSetup {
    SourceSetup::stand_in("github")
}

fn decide(registry: &FileRegistry, report: &str, verdict: ItemVerdict, by: &str) {
    decide_item(
        registry,
        &ItemDecision::new("G", report, verdict, "from the item", by, unix_seconds() as f64),
    )
    .unwrap();
}

#[test]
fn a_new_item_is_a_report_the_watch_is_told_of_and_no_brief_exists() {
    let state = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(state.path());
    let told = Told::default();
    let notify = recorder(&told);
    let intake = Intake {
        mode: SourceIntake::Watch,
        notify: Some(&notify),
    };
    let source = Items::of(vec![item("o/r#1", "It fails.")]);

    let summary = poll(&registry, &source, &intake);
    assert_eq!((summary.reported, summary.queued), (1, 0));
    assert!(
        registry.list_new().unwrap().is_empty(),
        "an item queues nothing until it is decided"
    );
    let found = reports(&registry);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].filer.source_key(), "source:github");
    assert!(found[0].untrusted());
    assert_eq!(found[0].text, "It fails.");
    let told_now = told.lock().unwrap().clone();
    assert_eq!(told_now.len(), 1);
    assert_eq!(told_now[0].0, "github");
    let notice: serde_json::Value = serde_json::from_str(&told_now[0].1).unwrap();
    assert_eq!(notice["source_item"]["change"], "new");
    assert_eq!(notice["source_item"]["report"], found[0].id.as_str());
    assert_eq!(notice["source_item"]["item"], "o/r#1");
    assert_eq!(notice["source_item"]["url"], "https://example.invalid/o/r#1");
}

#[test]
fn a_changed_item_reaches_the_watch_again_and_an_unchanged_one_does_not() {
    let state = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(state.path());
    let told = Told::default();
    let notify = recorder(&told);
    let intake = Intake {
        mode: SourceIntake::Watch,
        notify: Some(&notify),
    };
    let source = Items::of(vec![item("o/r#1", "It fails.")]);
    poll(&registry, &source, &intake);
    let unchanged = poll(&registry, &source, &intake);
    assert_eq!((unchanged.reported, unchanged.unchanged), (0, 1));
    assert_eq!(told.lock().unwrap().len(), 1, "unchanged: not told again");

    source.set(vec![item("o/r#1", "It fails on macOS.")]);
    poll(&registry, &source, &intake);
    source.set(vec![item("o/r#1", "It fails on macOS and Windows.")]);
    poll(&registry, &source, &intake);
    poll(&registry, &source, &intake);
    let found = reports(&registry);
    assert_eq!(found.len(), 1, "changed twice is still one Report");
    assert_eq!(found[0].text, "It fails on macOS and Windows.");
    let told_now = told.lock().unwrap().clone();
    assert_eq!(told_now.len(), 3, "told of the new item and of each change");
    let changes: Vec<String> = told_now
        .iter()
        .map(|(_, body, _)| {
            serde_json::from_str::<serde_json::Value>(body).unwrap()["source_item"]["change"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(changes, ["new", "changed", "changed"]);
    let keys: BTreeSet<_> = told_now.iter().map(|(_, _, key)| key.clone()).collect();
    assert_eq!(keys.len(), 3, "each version is its own Cable");
    assert!(registry.list_new().unwrap().is_empty());
}

#[test]
fn a_duplicate_and_a_no_action_leave_no_brief_and_record_the_decision() {
    let state = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(state.path());
    let intake = Intake::held();
    let source = Items::of(vec![item("o/r#1", "Same."), item("o/r#2", "Question.")]);
    poll(&registry, &source, &intake);
    let found = reports(&registry);
    let id = |item: &str| {
        found
            .iter()
            .find(|report| report.item.as_ref().is_some_and(|i| i.id == item))
            .unwrap()
            .id
            .clone()
    };
    decide(
        &registry,
        &id("o/r#1"),
        ItemVerdict::Duplicate {
            of: "G-github-o_r_9".to_owned(),
        },
        "watch:operation:G",
    );
    decide(&registry, &id("o/r#2"), ItemVerdict::NoAction, "watch:operation:G");
    let polled = setup();
    let queued = settle_items(&registry, "station-a", "G", &[&polled], &Intake::held());
    assert_eq!(queued, 0);
    assert!(registry.list_new().unwrap().is_empty(), "no Mission, no brief");
    for report in reports(&registry) {
        assert_eq!(report.state, ReportState::Closed);
    }
    let audited = registry
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|record| record.action == "source.item.decide")
        .count();
    assert_eq!(audited, 2);
}

#[test]
fn a_mission_decision_is_queued_by_the_desk_and_tied_to_its_report() {
    let state = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(state.path());
    let told = Told::default();
    let notify = recorder(&told);
    let intake = Intake {
        mode: SourceIntake::Watch,
        notify: Some(&notify),
    };
    let source = Items::of(vec![item("o/r#1", "It fails.")]);
    poll(&registry, &source, &intake);
    let id = reports(&registry)[0].id.clone();
    decide(&registry, &id, ItemVerdict::Mission, "watch:operation:G");
    assert!(
        registry.list_new().unwrap().is_empty(),
        "deciding records; the Station that polls queues"
    );

    let polled = setup();
    assert_eq!(
        settle_items(&registry, "station-a", "G", &[&polled], &intake),
        1
    );
    let queued = registry.list_new().unwrap();
    assert_eq!(queued.len(), 1);
    assert!(queued[0].body.contains("It fails."));
    let mission = registry
        .resolve_mission(queued[0].shown_handle().as_str())
        .unwrap()
        .unwrap()
        .mission_id;
    let report = registry.report(&id).unwrap().unwrap();
    assert!(report.about_mission(&mission), "{:?}", report.links);
    assert_eq!(report.state, ReportState::Assessed);
    let links = registry.issue_links(&mission).unwrap();
    assert!(
        links
            .iter()
            .any(|link| link.tracker == "github" && link.item == "o/r#1"),
        "the Mission is linked to the item"
    );
    // Nothing more to queue; a second pass changes nothing.
    assert_eq!(
        settle_items(&registry, "station-a", "G", &[&polled], &intake),
        0
    );

    // The item changes while its brief waits: the brief is amended, the watch told.
    source.set(vec![item("o/r#1", "It fails on macOS.")]);
    let changed = poll(&registry, &source, &intake);
    assert_eq!(changed.updated, 1);
    assert!(registry.list_new().unwrap()[0].body.contains("on macOS"));
    assert_eq!(reports(&registry).len(), 1);
    let told_now = told.lock().unwrap().clone();
    assert!(told_now.last().unwrap().1.contains("\"changed\""));
}

#[test]
fn with_no_watch_the_items_wait_and_a_persons_decision_applies() {
    let state = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(state.path());
    let intake = Intake::held();
    let source = Items::of(vec![item("o/r#1", "It fails.")]);
    poll(&registry, &source, &intake);
    poll(&registry, &source, &intake);
    let polled = setup();
    assert_eq!(settle_items(&registry, "station-a", "G", &[&polled], &intake), 0);
    assert!(
        registry.list_new().unwrap().is_empty(),
        "nothing becomes a Mission by itself"
    );
    let open = reports(&registry);
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].state, ReportState::Open);

    decide(&registry, &open[0].id, ItemVerdict::Mission, "person-1");
    assert_eq!(settle_items(&registry, "station-a", "G", &[&polled], &intake), 1);
    assert_eq!(registry.list_new().unwrap().len(), 1);
    let audit = registry
        .audit_records()
        .unwrap()
        .into_iter()
        .find(|record| record.action == "source.item.decide")
        .unwrap();
    assert_eq!(audit.actor, "person-1");
}

#[test]
fn a_deferred_item_is_told_to_the_watch_again_when_it_is_due() {
    let state = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(state.path());
    let told = Told::default();
    let notify = recorder(&told);
    let intake = Intake {
        mode: SourceIntake::Watch,
        notify: Some(&notify),
    };
    let source = Items::of(vec![item("o/r#1", "After the release.")]);
    poll(&registry, &source, &intake);
    let id = reports(&registry)[0].id.clone();
    let now = unix_seconds() as f64;
    decide_item(
        &registry,
        &ItemDecision::new(
            "G",
            &id,
            ItemVerdict::Later { until: now + 1.0 },
            "after the release",
            "watch:operation:G",
            now,
        ),
    )
    .unwrap();
    let polled = setup();
    settle_items(&registry, "station-a", "G", &[&polled], &intake);
    assert_eq!(told.lock().unwrap().len(), 1, "not yet due");
    std::thread::sleep(Duration::from_millis(1_200));
    settle_items(&registry, "station-a", "G", &[&polled], &intake);
    let told_now = told.lock().unwrap().clone();
    assert_eq!(told_now.len(), 2);
    assert!(told_now[1].1.contains("\"due\""), "{}", told_now[1].1);
    assert_eq!(
        registry.report(&id).unwrap().unwrap().state,
        ReportState::Open
    );
}

#[test]
fn a_direct_intake_is_a_reported_non_default_and_a_watch_one_is_not() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("operations.toml"),
        "[operation.G]\nintake = \"direct\"\nsources = { fake = { ids = [] } }\n\
         [operation.H]\nsources = { fake = { ids = [] } }\n\
         [operation.I]\nintake = \"direct\"\n",
    )
    .unwrap();
    let roster = Roster::load(dir.path()).unwrap();
    assert_eq!(roster.operation("H").unwrap().intake, SourceIntake::Watch);
    let lines = intake_non_defaults(&roster);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("\"G\"") && lines[0].contains("intake = \"direct\""),
        "{lines:?}"
    );
}
