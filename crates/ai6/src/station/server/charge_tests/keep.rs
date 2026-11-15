//! Charges keep their area working: they are told of Missions that are stuck, a quiet area takes
//! no timer turn, and each kind of charge fixes what it finds with the tools of its scope.

use std::time::Duration;

use serde_json::Value;

use super::station::{M_CHARGE, acting, headquarters_until, heard, kit, names_of, station_key};
use super::*;
use crate::directorates::m::charges::ChargeScope;
use crate::directorates::registry::reports::{ReportFilter, ReportScope};
use crate::directorates::registry::runs::RunStatus;

/// A Run of Mission `mission` of `operation` on `station`, left as the Registry would find it
/// after `how`: `lost` (its lease lapsed and it was lost), `verifying` (ended, awaiting a verdict),
/// `working` (under a live lease) or `blocked` (under a live lease).
fn seed(
    state: &DaemonState,
    operation: &str,
    station: &str,
    mission: &str,
    how: &str,
) -> RunRecord {
    let id = format!("run-{mission}");
    let ttl = match how {
        "lost" => Duration::from_millis(1),
        _ => Duration::from_secs(600),
    };
    let opened = state
        .intake
        .open_run(
            NewRun::new(id.as_str(), mission, station)
                .with_operation(operation)
                .with_handle(format!("{operation}-1.1")),
            ttl,
        )
        .unwrap();
    let result = match how {
        "verifying" => "verifying",
        "blocked" => "blocked",
        _ => "working",
    };
    let mut update = RunUpdate::default().result(RunResult::new(result));
    if how == "ended" {
        update = update.status(RunStatus::Ended);
    }
    let written = state
        .intake
        .update_run(&id, station, opened.fence, update)
        .unwrap();
    if how == "lost" {
        std::thread::sleep(Duration::from_millis(20));
        return state.intake.lose_run(&id, "Station went dark").unwrap();
    }
    written
}

/// What `key` was woken with that is a `stuck` event about `mission`.
fn stuck_of(key: &str, mission: &str) -> Vec<Value> {
    heard(key, "stuck")
        .into_iter()
        .filter(|detail| detail["mission"] == mission)
        .collect()
}

fn listen(key: &str) {
    acting(key.to_owned(), |_, _| {});
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mission_whose_run_is_lost_wakes_ms_and_its_stations_charge_once() {
    let _only = M_CHARGE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("k1", "stn-k1");
    let state = kit(
        root.path(),
        tag,
        station,
        ("stuck_run_lost_after = 0\n", true),
        "",
    );
    let (st_key, m_key, op_key) = (
        station_key(station),
        charges::run_key(&ChargeScope::M),
        key(tag),
    );
    for key in [&st_key, &m_key, &op_key] {
        listen(key);
    }
    let operation = names_of(tag);
    seed(&state, &operation, station, "lost-k1", "lost");
    // Another Station's Mission is not this Station's to be told of, but it is M's.
    seed(&state, &operation, "elsewhere", "lost-k1-far", "lost");

    headquarters_until(&state, "M and the Station are told", || {
        !stuck_of(&m_key, "lost-k1").is_empty()
            && !stuck_of(&st_key, "lost-k1").is_empty()
            && !stuck_of(&m_key, "lost-k1-far").is_empty()
    })
    .await;
    for _ in 0..40 {
        charges::tend(&state, true, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for key in [&m_key, &st_key] {
        let told = stuck_of(key, "lost-k1");
        assert_eq!(told.len(), 1, "{key}: told once: {told:?}");
        assert_eq!(told[0]["condition"], "run-lost");
        assert!(told[0]["since"].as_f64().is_some() && told[0]["why"].as_str().is_some());
        assert_eq!(told[0]["station"], station);
    }
    assert!(
        stuck_of(&st_key, "lost-k1-far").is_empty(),
        "another Station's Mission is not the Station's"
    );
    assert_eq!(stuck_of(&m_key, "lost-k1-far").len(), 1);
    // The Operation's charge is told of its own Operation's Missions too.
    assert_eq!(stuck_of(&op_key, "lost-k1").len(), 1);
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_verifying_wakes_its_operations_charge_and_m_but_not_another_stations() {
    let _only = M_CHARGE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("k2", "stn-k2");
    let state = kit(
        root.path(),
        tag,
        station,
        ("stuck_verifying_after = 0\n", true),
        "",
    );
    let (st_key, m_key, op_key) = (
        station_key(station),
        charges::run_key(&ChargeScope::M),
        key(tag),
    );
    for key in [&st_key, &m_key, &op_key] {
        listen(key);
    }
    seed(
        &state,
        &names_of(tag),
        "elsewhere",
        "verifying-k2",
        "verifying",
    );

    headquarters_until(&state, "the Operation's charge and M are told", || {
        !stuck_of(&op_key, "verifying-k2").is_empty()
            && !stuck_of(&m_key, "verifying-k2").is_empty()
    })
    .await;
    for _ in 0..20 {
        charges::tend(&state, true, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for key in [&op_key, &m_key] {
        let told = stuck_of(key, "verifying-k2");
        assert_eq!(told.len(), 1, "{key}: {told:?}");
        assert_eq!(told[0]["condition"], "verifying");
    }
    assert!(stuck_of(&st_key, "verifying-k2").is_empty());
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_threshold_of_none_turns_a_stuck_condition_off() {
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("k3", "stn-k3");
    let state = kit(
        root.path(),
        tag,
        station,
        ("stuck_run_lost_after = \"none\"\n", false),
        "",
    );
    let st_key = station_key(station);
    listen(&st_key);
    let operation = names_of(tag);
    seed(&state, &operation, station, "lost-k3", "lost");
    for _ in 0..40 {
        charges::tend(&state, false, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(stuck_of(&st_key, "lost-k3").is_empty());

    // The same Mission, with the threshold set, is flagged on the next look.
    drop(state);
    let state = kit(
        root.path(),
        tag,
        station,
        ("stuck_run_lost_after = 0\n", false),
        "",
    );
    tend_until(&state, "the Mission is flagged", || {
        !stuck_of(&st_key, "lost-k3").is_empty()
    })
    .await;
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_charge_takes_no_timer_turn_and_wakes_again_when_a_mission_opens() {
    let root = tempfile::tempdir().unwrap();
    let tag = "k4";
    // A timer of a second while open, none while idle (the default).
    let state = station(root.path(), tag, "test", "wake_every = 1\n", "");
    charges::tend(&state, false, true).await;
    assert!(state.charges.is_live(&scope(tag)));
    tend_for(&state, 150).await;
    assert!(
        calls(&key(tag)).is_empty() && turns(&state, tag) == 0,
        "nothing is open, so no timer turn: {:?}",
        calls(&key(tag))
    );

    // A Mission opens in the Operation's area.
    seed(&state, &names_of(tag), "elsewhere", "open-k4", "working");
    tend_until(&state, "a timer turn", || turns(&state, tag) == 1).await;
    let first = calls(&key(tag));
    assert!(
        first[0].text.contains("\"timer\"")
            && first[0].text.contains("\"open_work\": true")
            && first[0].text.contains("missions_by_state"),
        "{}",
        first[0].text
    );
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m_looks_at_a_charge_whose_lease_lapsed_or_whose_turn_did_not_run() {
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("k5", "stn-k5");
    let state = kit(
        root.path(),
        tag,
        station,
        (
            "stuck_charge_after = 0
",
            true,
        ),
        "",
    );
    // The Operation's charge was claimed by a Station that then went dark.
    state
        .intake
        .claim_charge(
            &scope(tag),
            &format!("k{tag}"),
            "gone-station",
            Duration::from_millis(1),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let m = state.roster.charges().get("m").cloned().unwrap();
    let looking = state.clone();
    let survey = tokio::task::spawn_blocking(move || {
        charges::area::survey(&looking, &m, &mut BTreeMap::new(), unix_now())
    })
    .await
    .unwrap();
    let lapsed: Vec<_> = survey
        .stuck
        .iter()
        .filter(|stuck| stuck.condition == "charge-lapsed")
        .collect();
    assert_eq!(lapsed.len(), 1, "{:?}", survey.stuck);
    assert_eq!(lapsed[0].charge.as_deref(), Some(scope(tag).as_str()));

    // Another charge's own look does not include the charges: only M watches them.
    let operation = state.roster.charges().get(&scope(tag)).cloned().unwrap();
    let looking = state.clone();
    let survey = tokio::task::spawn_blocking(move || {
        charges::area::survey(&looking, &operation, &mut BTreeMap::new(), unix_now())
    })
    .await
    .unwrap();
    assert!(survey.stuck.is_empty(), "{:?}", survey.stuck);
}

// ---- The tools a charge fixes its area with ----

use serde_json::json;

use super::station::{answered, audits, call};
use crate::station::server::head_of_station::tests::{lease_mission, start};

/// A Mission of `operation` whose Station went dark: a claimed brief with a lapsed lease and a
/// lost Run. Answers the Mission's key.
fn seed_dark(state: &DaemonState, operation: &str, id: &str) -> String {
    let brief: crate::directorates::registry::store::Brief = serde_json::from_value(json!({
        "title": "work", "id": id, "source": "test", "operation": operation,
    }))
    .unwrap();
    state.intake.submit_brief(&brief).unwrap();
    let key = brief.key();
    let meta = std::collections::BTreeMap::from([
        ("mission_id".to_owned(), json!(key)),
        ("operation".to_owned(), json!(operation)),
    ]);
    state
        .intake
        .claim_with_lease(Some(&key), "elsewhere", Duration::from_millis(1), meta)
        .unwrap()
        .expect("the brief is claimed");
    seed(state, operation, "elsewhere", &key, "lost");
    key
}

fn audit_actions(state: &DaemonState, actor: &str, action: &str) -> Vec<String> {
    state
        .intake
        .audit_records()
        .unwrap()
        .iter()
        .filter(|record| record.actor == actor && record.action == action)
        .map(|record| record.operation.clone())
        .collect()
}

fn tool_names(key: &str) -> Vec<String> {
    super::station::TOOL_NAMES.lock().unwrap()[key].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m_settles_lost_missions_of_any_operation_and_the_board_shows_them_settled() {
    let _only = M_CHARGE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("k6", "stn-k6");
    let state = kit(
        root.path(),
        tag,
        station,
        ("stuck_run_lost_after = 0\n", true),
        "",
    );
    let m_key = charges::run_key(&ChargeScope::M);
    let settling = m_key.clone();
    acting(m_key.clone(), move |tools, event| {
        if event["kind"] == "stuck" && event["detail"]["condition"] == "run-lost" {
            call(
                tools,
                &settling,
                "settle_mission",
                json!({
                    "mission": event["detail"]["mission"],
                    "outcome": "failed",
                    "reason": "its Station went dark",
                }),
            );
            call(tools, &settling, "board", json!({}));
        }
    });
    // Two Missions of two Operations, neither this Station's own.
    let ours = seed_dark(&state, &names_of(tag), "k6-a");
    let other = seed_dark(&state, "OTHER", "k6-b");

    headquarters_until(&state, "M settles both", || {
        answered(&m_key, "settle_mission").len() == 2
    })
    .await;
    for answer in answered(&m_key, "settle_mission") {
        assert!(answer.contains("\"settled\":\"failed\""), "{answer}");
    }
    for key in [&ours, &other] {
        let filed = state.intake.filed_brief(key).unwrap().unwrap();
        assert_eq!(filed.state, "failed", "{key}");
        assert_eq!(filed.outcome.as_ref().unwrap()["state"], "lost");
        let run = state.intake.latest_run(key).unwrap().unwrap();
        assert_eq!(run.result.unwrap().state, "failed", "{key}");
    }
    // Audited with M as the actor, in each Operation, before it was done.
    let mut operations = audit_actions(&state, "charge:m", "mission.settle");
    operations.sort();
    assert_eq!(operations, [names_of(tag), "OTHER".to_owned()]);
    // The board M read after settling lists neither.
    let board = answered(&m_key, "board").pop().unwrap();
    assert!(
        !board.contains("k6-a") && !board.contains("k6-b"),
        "{board}"
    );
    for tool in [
        "board",
        "mission_detail",
        "settle_mission",
        "retry_mission",
        "resume_mission",
        "cancel_mission",
        "wake_charge",
        "notify_person",
        "verify_mission",
        "redispatch",
        "raise_flag",
    ] {
        assert!(tool_names(&m_key).contains(&tool.to_owned()), "{tool}");
    }
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m_decides_a_verifying_mission_of_an_operation_with_no_charge_and_may_tell_a_person_instead()
 {
    let _only = M_CHARGE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("k7", "stn-k7");
    let state = kit(
        root.path(),
        tag,
        station,
        ("stuck_verifying_after = 0\n", true),
        "",
    );
    let m_key = charges::run_key(&ChargeScope::M);
    let acts = m_key.clone();
    acting(m_key.clone(), move |tools, event| {
        if event["kind"] == "stuck" && event["detail"]["condition"] == "verifying" {
            let mission = &event["detail"]["mission"];
            if mission == "v-notify" {
                call(
                    tools,
                    &acts,
                    "notify_person",
                    json!({
                        "title": "A Mission waits for its outcome",
                        "text": "Operation NOCH has no charge, so nobody verifies it.",
                    }),
                );
            } else {
                call(
                    tools,
                    &acts,
                    "verify_mission",
                    json!({ "mission": mission, "decision": "done", "reason": "read back" }),
                );
            }
        }
    });
    // An Operation with no charge on the roster.
    seed(&state, "NOCH", "elsewhere", "v-notify", "verifying");
    seed(&state, "NOCH", "elsewhere", "v-decide", "verifying");

    headquarters_until(&state, "M acts on both", || {
        answered(&m_key, "notify_person").len() == 1
            && answered(&m_key, "verify_mission").len() == 1
    })
    .await;
    // Told a person: a Report of M's own scope, and nothing confirmed.
    let notices = state
        .intake
        .reports(&ReportFilter::all().in_scope(ReportScope::M))
        .unwrap();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].title.contains("waits for its outcome"));
    let waiting = state.intake.latest_run("v-notify").unwrap().unwrap();
    assert!(waiting.verdict.is_none(), "{waiting:?}");
    // Decided: a verdict by M, which a person's replaces.
    let decided = state.intake.latest_run("v-decide").unwrap().unwrap();
    assert_eq!(decided.verdict.as_ref().unwrap().by, "charge:m");
    assert_eq!(
        audit_actions(&state, "charge:m", "mission.verify"),
        ["NOCH"]
    );
    assert_eq!(audits(&state, "charge.notified"), 1);
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_station_charge_retries_its_own_stalled_mission_and_is_refused_outside_its_station() {
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("k8", "stn-k8");
    let state = kit(root.path(), tag, station, ("", false), "");
    let st_key = station_key(station);
    let operation = names_of(tag);
    let acts = st_key.clone();
    let far = "k8-far";
    acting(st_key.clone(), move |tools, event| {
        if event["kind"] == "control" && event["detail"]["note"] == "work" {
            for (tool, arguments) in [
                ("retry_mission", json!({ "mission": "k8-own" })),
                (
                    "settle_mission",
                    json!({ "mission": far, "outcome": "failed", "reason": "x" }),
                ),
                ("retry_mission", json!({ "mission": far })),
                ("mission_detail", json!({ "mission": far })),
                (
                    "verify_mission",
                    json!({ "mission": "k8-own", "decision": "done", "reason": "x" }),
                ),
            ] {
                call(tools, &acts, tool, arguments);
            }
        }
    });
    // A stalled Mission this Station holds, and another Station's lost one.
    lease_mission(&state, &operation, "k8-own");
    start(&state, "k8-own").await;
    state
        .missions
        .lock()
        .unwrap()
        .transition(
            "k8-own",
            crate::directorates::m::mission::MissionState::Stalled,
        )
        .unwrap();
    seed(&state, &operation, "elsewhere", far, "lost");

    charges::tend(&state, false, true).await;
    super::station::wake(&state, &format!("station:{station}"), "work");
    tend_until(&state, "the Station's charge acts", || {
        answered(&st_key, "mission_detail").len() == 1
    })
    .await;
    let retried = answered(&st_key, "retry_mission");
    assert!(retried[0].contains("\"retried\":true"), "{retried:?}");
    assert_eq!(
        state.missions.lock().unwrap().get("k8-own").unwrap().state,
        crate::directorates::m::mission::MissionState::Queued,
        "the stalled Mission is queued for its next run"
    );
    assert_eq!(
        audit_actions(
            &state,
            &format!("charge:station:{station}"),
            "mission.retry"
        ),
        [operation]
    );
    // Outside its Station, every tool answers as if the Mission did not exist.
    for (tool, answer) in [
        (
            "settle_mission",
            answered(&st_key, "settle_mission").remove(0),
        ),
        ("retry_mission", retried[1].clone()),
        (
            "mission_detail",
            answered(&st_key, "mission_detail").remove(0),
        ),
    ] {
        assert!(
            answer.contains("not a Mission of your Station"),
            "{tool}: {answer}"
        );
    }
    assert!(
        state.intake.latest_run(far).unwrap().unwrap().status
            == crate::directorates::registry::runs::RunStatus::Lost
    );
    // It has no tool to decide an Operation's outcome.
    assert!(!tool_names(&st_key).contains(&"verify_mission".to_owned()));
    assert!(
        answered(&st_key, "verify_mission")
            .iter()
            .all(|a| a.starts_with("ERROR")
                || a.contains("unknown")
                || a.contains("not found")
                || a.contains("tool"))
    );
    for tool in [
        "board",
        "mission_detail",
        "settle_mission",
        "retry_mission",
        "resume_mission",
        "notify_person",
    ] {
        assert!(tool_names(&st_key).contains(&tool.to_owned()), "{tool}");
    }
    state
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_operation_charge_settles_its_own_missions_and_not_another_operations() {
    let root = tempfile::tempdir().unwrap();
    let tag = "k9";
    let state = station(root.path(), tag, "test", "stuck_run_lost_after = 0\n", "");
    let op_key = key(tag);
    let acts = op_key.clone();
    acting(op_key.clone(), move |tools, event| {
        if event["kind"] == "stuck" {
            call(
                tools,
                &acts,
                "settle_mission",
                json!({ "mission": event["detail"]["mission"], "outcome": "failed", "reason": "dark" }),
            );
        }
        if event["kind"] == "control" {
            call(
                tools,
                &acts,
                "settle_mission",
                json!({ "mission": "k9-foreign", "outcome": "failed", "reason": "dark" }),
            );
        }
    });
    let ours = seed_dark(&state, &names_of(tag), "k9-own");
    let foreign = seed_dark(&state, "OTHER", "k9-foreign");
    // The foreign key as the stand-in names it.
    assert!(foreign.contains("k9-foreign"));

    charges::tend(&state, false, true).await;
    tend_until(&state, "its own Mission is settled", || {
        state
            .intake
            .filed_brief(&ours)
            .unwrap()
            .is_some_and(|filed| filed.state == "failed")
    })
    .await;
    control(&state, tag, "try the foreign one");
    tend_until(&state, "the foreign one is refused", || {
        answered(&op_key, "settle_mission")
            .iter()
            .any(|answer| answer.contains("not a Mission of your Operation"))
    })
    .await;
    assert_eq!(
        state.intake.filed_brief(&foreign).unwrap().unwrap().state,
        "claimed",
        "another Operation's Mission is untouched"
    );
    assert!(tool_names(&op_key).contains(&"retry_mission".to_owned()));
    charges::close_all(&state).await;
}
