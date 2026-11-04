//! Briefs that find no place, harnesses that cannot start, a model sweep beside the check-in
//! beat, and Missions withdrawn, cancelled or retried from outside their Station.

use std::path::Path;

use super::tests::test_state;
use super::*;
use crate::directorates::registry::timeline::TimelineEvent;

/// A brief of Operation G with nothing named but the Stations it is pinned to.
pub(super) fn pinned_brief(id: &str, stations: &[&str]) -> Brief {
    serde_json::from_value(serde_json::json!({
        "title": format!("Pinned work {id}"),
        "id": id,
        "source": "manual",
        "operation": "G",
        "agent": null,
        "role": null,
        "url": null,
        "stations": stations,
    }))
    .unwrap()
}

fn waiting_events(state: &DaemonState) -> Vec<TimelineEvent> {
    state
        .intake
        .timeline_events("G")
        .unwrap()
        .into_iter()
        .filter(|event| event.action == "waiting")
        .collect()
}

fn waits_for(state: &DaemonState, key: &str) -> Vec<String> {
    waiting_events(state)
        .into_iter()
        .filter(|event| event.detail.get("brief").map(String::as_str) == Some(key))
        .filter_map(|event| event.detail.get("reason").cloned())
        .collect()
}

/// A state whose Operation G is limited by `operation_settings`, with Agent 007 on the mock
/// handler, checked in as Station `test`.
fn checked_in(root: &Path, operation_settings: &str) -> DaemonState {
    let state = test_state(
        root,
        operation_settings,
        "[agent.007]\nhandler = \"mock-agent\"\n",
    );
    publish_station_check_in(&state);
    state
}

#[test]
fn a_brief_pinned_to_a_station_its_operation_excludes_says_so_once() {
    let root = tempfile::tempdir().unwrap();
    let state = checked_in(root.path(), "stations = [\"elsewhere\"]\n");
    let brief = pinned_brief("pin-excluded", &["test"]);
    state.intake.submit_brief(&brief).unwrap();

    for _ in 0..3 {
        schedule_intake_once(&state);
    }

    assert_eq!(state.intake.list_new().unwrap().len(), 1, "it stays queued");
    let reasons = waits_for(&state, &brief.key());
    assert_eq!(reasons.len(), 1, "noted once: {reasons:?}");
    assert!(
        reasons[0].contains("Station \"test\"")
            && reasons[0].contains("the Operation allows only Stations [elsewhere]"),
        "{}",
        reasons[0]
    );
}

/// What a pinned brief that never places looks like: an exact pin to a healthy Station places
/// at once, and a pin by another case, by host name, or to a Station without the Operation's
/// checkout is passed over every round. Each was silent; now each waits with its reason.
#[test]
fn a_pin_places_only_on_the_exact_id_of_a_station_that_can_take_it() {
    let root = tempfile::tempdir().unwrap();
    let state = checked_in(root.path(), "");
    // Another Station, alike but without a checkout of G, whose host is `worker`.
    let mut info = state
        .intake
        .stations(Duration::MAX)
        .unwrap()
        .into_iter()
        .find(|record| record.station() == "test")
        .unwrap()
        .info()
        .clone();
    info.insert("operations".to_owned(), serde_json::json!([]));
    info.insert("host".to_owned(), serde_json::json!("worker"));
    state.intake.check_in("worker-1a2b3c4d", info).unwrap();

    let exact = pinned_brief("pin-exact", &["test"]);
    let no_checkout = pinned_brief("pin-no-checkout", &["worker-1a2b3c4d"]);
    let other_case = pinned_brief("pin-case", &["WORKER-1A2B3C4D"]);
    let host_name = pinned_brief("pin-host", &["worker"]);
    for brief in [&exact, &no_checkout, &other_case, &host_name] {
        state.intake.submit_brief(brief).unwrap();
    }
    schedule_intake_once(&state);
    schedule_intake_once(&state);

    assert_eq!(
        state.intake.lease(&exact.key()).unwrap().unwrap().station(),
        "test"
    );
    let queued: Vec<String> = state
        .intake
        .list_new()
        .unwrap()
        .iter()
        .map(Brief::key)
        .collect();
    assert_eq!(queued.len(), 3, "{queued:?}");
    assert!(waits_for(&state, &exact.key()).is_empty());
    for (brief, expected) in [
        (
            &no_checkout,
            "this Station has no checkout of Operation \"G\"",
        ),
        (
            &other_case,
            "Station \"WORKER-1A2B3C4D\" has never checked in",
        ),
        (&host_name, "Station \"worker\" has never checked in"),
    ] {
        let reasons = waits_for(&state, &brief.key());
        assert_eq!(reasons.len(), 1, "{}: {reasons:?}", brief.id);
        assert!(reasons[0].contains(expected), "{}", reasons[0]);
        assert!(
            !reasons[0].contains("narrows placement"),
            "Stations the brief does not name are left out: {}",
            reasons[0]
        );
    }
}

#[test]
fn a_waiting_reason_longer_than_the_timeline_keeps_is_cut_and_still_recorded() {
    let root = tempfile::tempdir().unwrap();
    let state = checked_in(root.path(), "");
    let many: Vec<String> = (0..40).map(|n| format!("absent-station-{n}")).collect();
    let names: Vec<&str> = many.iter().map(String::as_str).collect();
    let brief = pinned_brief("pin-many", &names);
    state.intake.submit_brief(&brief).unwrap();

    schedule_intake_once(&state);

    let reasons = waits_for(&state, &brief.key());
    assert_eq!(reasons.len(), 1);
    let room = state.intake.limits().timeline.text_bytes.or_max();
    assert!(reasons[0].len() <= room, "{}", reasons[0].len());
    assert!(reasons[0].ends_with('…'), "{}", reasons[0]);
    assert!(reasons[0].starts_with("no place can take it: Station \"absent-station-0\""));
}

/// The account path still notes each new reason, its figures included.
#[test]
fn an_account_hold_is_noted_again_when_its_reason_changes() {
    let root = tempfile::tempdir().unwrap();
    let state = checked_in(root.path(), "");
    accounts::note_waiting(&state, "k", "G", Some("frees at 100"));
    accounts::note_waiting(&state, "k", "G", Some("frees at 100"));
    accounts::note_waiting(&state, "k", "G", Some("frees at 200"));
    assert_eq!(waits_for(&state, "k"), ["frees at 100", "frees at 200"]);
    // A placement reason whose figures alone changed is the same reason.
    accounts::note_unplaced(&state, "p", "G", "Station \"a\": silent for 10 seconds");
    accounts::note_unplaced(&state, "p", "G", "Station \"a\": silent for 12 seconds");
    accounts::note_unplaced(&state, "p", "G", "Station \"b\": silent for 12 seconds");
    assert_eq!(waits_for(&state, "p").len(), 2);
}
