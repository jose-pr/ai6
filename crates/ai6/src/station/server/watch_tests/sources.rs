//! A Source's items reach an Operation's watch as Cables, and the watch decides what each
//! becomes with its tools over the real endpoint, as a model would. A stand-in Source supplies
//! the items; nothing becomes a brief until the watch decides `mission`.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use super::decides::Tools;
use super::*;
use crate::directorates::m::intake::SourceIntake;
use crate::directorates::registry::source_items::ITEM_KIND;
use crate::directorates::registry::sources::{SourceBatch, SourceItem};
use crate::station::sources::{
    Intake, PollSummary, Reopening, SourceDesk, SourceSetup, poll_source, settle_items,
};
use crate::{Source, SourcePoll};

/// Serves the items it is set to, every poll.
struct Items(Mutex<Vec<SourceItem>>);

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

/// Polls `source` for the Operation of `tag` as its Station does: the watch is told by Cable.
fn poll(state: &DaemonState, tag: &str, source: &Items) -> PollSummary {
    let operation = state.roster.operation(&names(tag).0).unwrap();
    let desk = SourceDesk::new("test", Default::default()).with_cables(state.cables.clone());
    let notify = desk.notify_for(&state.roster, operation);
    assert!(notify.is_some(), "the Operation has a watch Agent");
    poll_source(
        state.intake.as_ref(),
        "test",
        (&operation.id, "github"),
        false,
        source,
        &Reopening::never(),
        &Intake {
            mode: SourceIntake::Watch,
            notify: notify.as_deref(),
        },
    )
    .unwrap()
}

/// Queues what the watch decided `mission`, as the Station that polls the Source does.
fn settle(state: &DaemonState, tag: &str) -> usize {
    let setup = SourceSetup::stand_in("github");
    settle_items(
        state.intake.as_ref(),
        "test",
        &names(tag).0,
        &[&setup],
        &Intake::held(),
    )
}

/// What a stand-in watch decides of an item, by the item's number.
fn decision_for(item: &str) -> Value {
    match item {
        "o/r#1" => json!({ "assessment": "mission", "reason": "a real fault" }),
        "o/r#2" => {
            json!({ "assessment": "duplicate", "of": "G-github-o_r_9", "reason": "the same" })
        }
        "o/r#3" => json!({ "assessment": "no_action", "reason": "a question, not work" }),
        _ => json!({ "assessment": "later", "wait_seconds": 3600, "reason": "after the release" }),
    }
}

/// How many turns each item's `new` notice has taken, so a test sees a changed one decided again.
static LISTED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static NOTICES: AtomicUsize = AtomicUsize::new(0);

/// Makes the watch of `tag` act on each source item a wake brings: it lists the open items,
/// reads the one it was told of, and decides it with `decision_for`.
fn deciding(tag: &str) {
    let key = key(tag);
    TURNS.lock().unwrap().insert(key.clone(), {
        let key = key.clone();
        Arc::new(move |text: &str, environment: &BTreeMap<String, String>| {
            let notices: Vec<Value> = super::decides::events_of(text)
                .into_iter()
                .filter(|event| event["kind"] == "cable")
                .filter_map(|event| event["detail"]["body"].as_str().map(str::to_owned))
                .filter_map(|body| serde_json::from_str::<Value>(&body).ok())
                .filter(|body| body.get("source_item").is_some())
                .collect();
            if notices.is_empty() {
                return;
            }
            let (key, environment) = (key.clone(), environment.clone());
            std::thread::spawn(move || {
                let tools = Tools::connect(&key, &environment);
                *LISTED.lock().unwrap() = tools.names();
                for notice in notices {
                    NOTICES.fetch_add(1, Ordering::SeqCst);
                    let report = notice["source_item"]["report"].as_str().unwrap();
                    if notice["source_item"]["change"] != "new" {
                        continue;
                    }
                    tools.call("source_items", json!({}));
                    tools.call("source_item", json!({ "report": report }));
                    let mut arguments =
                        decision_for(notice["source_item"]["item"].as_str().unwrap());
                    arguments["report"] = json!(report);
                    tools.call("assess_report", arguments);
                }
                let _ = tools.runtime.block_on(tools.client.cancel());
            })
            .join()
            .unwrap();
        })
    });
}

fn reports(state: &DaemonState, tag: &str) -> Vec<crate::directorates::registry::reports::Report> {
    use crate::directorates::registry::reports::{ReportFilter, ReportScope};
    state
        .intake
        .reports(
            &ReportFilter::all()
                .in_scope(ReportScope::Operation(names(tag).0))
                .of_kind(ITEM_KIND),
        )
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_source_item_reaches_the_watch_as_a_cable_and_waits_for_its_decision() {
    let root = tempfile::tempdir().unwrap();
    let tag = "s1";
    let state = station_with(
        root.path(),
        tag,
        "test",
        "",
        "",
        &Extra {
            operation_cables: true,
            ..Extra::default()
        },
    );
    watches::tend(&state, false, true).await;
    // A watch that only takes the notice.
    TURNS.lock().unwrap().remove(&key(tag));
    let source = Items(Mutex::new(vec![item("o/r#1", "It fails.")]));

    let summary = poll(&state, tag, &source);
    assert_eq!((summary.reported, summary.queued), (1, 0));
    tend_until(&state, "a turn for the notice", || turns(&state, tag) == 1).await;
    let wake = &calls(&key(tag))[0].text;
    for expected in ["\"cable\"", "source_item", "untrusted"] {
        assert!(wake.contains(expected), "{expected}: {wake}");
    }
    assert!(
        state.intake.list_new().unwrap().is_empty(),
        "no brief exists until the watch decides mission"
    );
    assert_eq!(reports(&state, tag).len(), 1);
    assert_no_mission(&state, tag);
    assert!(state.intake.runs().unwrap().is_empty());

    // Polled unchanged, it is not told again.
    let again = poll(&state, tag, &source);
    assert_eq!((again.reported, again.unchanged), (0, 1));
    tend_for(&state, 10).await;
    assert_eq!(turns(&state, tag), 1, "an unchanged item wakes nobody");
    watches::close_all(&state).await;
    state.stopping.store(true, Ordering::Release);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_watch_decides_each_item_and_only_a_mission_becomes_a_brief() {
    let root = tempfile::tempdir().unwrap();
    let tag = "s2";
    let state = station_with(
        root.path(),
        tag,
        "test",
        "",
        "",
        &Extra {
            operation_cables: true,
            ..Extra::default()
        },
    );
    deciding(tag);
    watches::tend(&state, false, true).await;
    let source = Items(Mutex::new(vec![
        item("o/r#1", "It fails."),
        item("o/r#2", "Same as before."),
        item("o/r#3", "How do I?"),
        item("o/r#4", "Not now."),
    ]));
    poll(&state, tag, &source);

    // The stand-in reads and decides every item it was told of, with the real tools.
    tend_until(&state, "all four decided", || {
        reports(&state, tag)
            .iter()
            .all(|report| report.assessment.is_some())
            && reports(&state, tag).len() == 4
    })
    .await;
    let tools = LISTED.lock().unwrap().clone();
    for name in ["source_items", "source_item", "assess_report"] {
        assert!(tools.iter().any(|tool| tool == name), "{name}: {tools:?}");
    }
    assert!(
        state.intake.list_new().unwrap().is_empty(),
        "deciding queues nothing itself"
    );

    // The duplicate and the no-action leave no Mission and record why.
    let by_item = |id: &str| {
        reports(&state, tag)
            .into_iter()
            .find(|report| report.item.as_ref().is_some_and(|item| item.id == id))
            .unwrap()
    };
    let duplicate = by_item("o/r#2");
    assert_eq!(duplicate.assessment.as_ref().unwrap().verdict, "duplicate");
    assert_eq!(
        duplicate.assessment.as_ref().unwrap().by,
        format!("watch:operation:{}", names(tag).0)
    );
    assert_eq!(
        duplicate.resolution.as_ref().unwrap().note.as_deref(),
        Some("duplicate of G-github-o_r_9")
    );
    let none = by_item("o/r#3");
    assert_eq!(none.assessment.as_ref().unwrap().verdict, "no_action");
    assert_eq!(
        none.assessment.as_ref().unwrap().comment.as_deref(),
        Some("a question, not work")
    );
    let later = by_item("o/r#4");
    assert_eq!(later.assessment.as_ref().unwrap().verdict, "later");
    assert!(later.deadline.is_some());
    let audited: Vec<_> = state
        .intake
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|record| record.action == "source.item.decide")
        .collect();
    assert_eq!(audited.len(), 4, "each decision is audited");

    // The Station that polls the Source queues the one decided mission, linked to its item.
    assert_eq!(settle(&state, tag), 1);
    let queued = state.intake.list_new().unwrap();
    assert_eq!(queued.len(), 1);
    assert!(queued[0].body.contains("It fails."));
    let mission = state
        .intake
        .resolve_mission(queued[0].shown_handle().as_str())
        .unwrap()
        .unwrap()
        .mission_id;
    assert!(by_item("o/r#1").about_mission(&mission));
    assert!(
        state
            .intake
            .issue_links(&mission)
            .unwrap()
            .iter()
            .any(|link| link.item == "o/r#1")
    );
    assert_no_watch_mission(&state);

    // A changed item reaches the watch again, and the decided Mission's brief follows it.
    let notices = NOTICES.load(Ordering::SeqCst);
    *source.0.lock().unwrap() = vec![item("o/r#1", "It fails on macOS.")];
    let changed = poll(&state, tag, &source);
    assert_eq!(changed.updated, 1, "the queued brief is amended");
    tend_until(&state, "a turn for the change", || {
        NOTICES.load(Ordering::SeqCst) > notices
    })
    .await;
    assert!(
        state.intake.list_new().unwrap()[0]
            .body
            .contains("on macOS")
    );
    assert_eq!(reports(&state, tag).len(), 4, "still one Report per item");
    watches::close_all(&state).await;
    state.stopping.store(true, Ordering::Release);
}

/// The watch has no Mission, brief or placement of its own: only the item it queued exists.
fn assert_no_watch_mission(state: &DaemonState) {
    assert!(
        state
            .intake
            .runs()
            .unwrap()
            .iter()
            .all(|run| !run.mission_id.starts_with("watch--")),
        "no Run record of a watch"
    );
}
