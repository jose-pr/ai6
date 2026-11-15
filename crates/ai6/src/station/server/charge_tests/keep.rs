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

// ---- Acts are checked before they are taken, and text does not cross unchecked ----

use crate::directorates::registry::charges::{ChargeLogEntry, ChargeUpdate};
use crate::station::server::charges::keep::AreaDesk;

/// A desk for the charge of `scope`, kept by `agent`, on `state`.
fn desk(state: &DaemonState, scope: ChargeScope, agent: &str) -> AreaDesk {
    AreaDesk::new(state.clone(), scope, agent)
}

async fn off_the_workers<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(work).await.unwrap()
}

/// A Mission of `operation` whose Station holds a brief claim that is still live and whose Run
/// is lost. Answers the Mission's key.
fn seed_live_claim(state: &DaemonState, operation: &str, id: &str) -> String {
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
        .claim_with_lease(Some(&key), "elsewhere", Duration::from_secs(600), meta)
        .unwrap()
        .expect("the brief is claimed");
    seed(state, operation, "elsewhere", &key, "lost");
    key
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settling_refuses_before_changing_anything_while_the_briefs_lease_is_live() {
    let root = tempfile::tempdir().unwrap();
    let tag = "n1";
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    let operation = names_of(tag);
    let key = seed_live_claim(&state, &operation, "n1-live");
    let own = desk(
        &state,
        ChargeScope::Operation(operation.clone()),
        &format!("k{tag}"),
    );

    let (settling, mission) = (own.clone(), key.clone());
    let refused =
        off_the_workers(move || settling.settle(&mission, "failed", "it looks dark")).await;
    let refused = refused.unwrap_err();
    assert!(refused.contains("has not lapsed"), "{refused}");
    // Nothing was audited or changed: the Run still reports its Mission queued, the brief is
    // still claimed.
    assert!(audit_actions(&state, &own.actor(), "mission.settle").is_empty());
    let run = state.intake.latest_run(&key).unwrap().unwrap();
    assert_eq!(run.result.unwrap().state, "queued");
    assert_eq!(
        state.intake.filed_brief(&key).unwrap().unwrap().state,
        "claimed"
    );
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_act_whose_audit_cannot_be_written_is_not_taken() {
    let root = tempfile::tempdir().unwrap();
    let tag = "n2";
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    let operation = names_of(tag);
    let key = seed_dark(&state, &operation, "n2-dark");
    let own = desk(
        &state,
        ChargeScope::Operation(operation),
        &format!("k{tag}"),
    );

    // The audit log cannot be written: its directory is a file.
    let audit = root.path().join("registry").join("audit");
    let _ = std::fs::remove_dir_all(&audit);
    std::fs::write(&audit, b"not a directory").unwrap();

    let (settling, mission) = (own.clone(), key.clone());
    let refused = off_the_workers(move || settling.settle(&mission, "failed", "dark")).await;
    let refused = refused.unwrap_err();
    assert!(
        refused.contains("audit record could not be written"),
        "{refused}"
    );
    let run = state.intake.latest_run(&key).unwrap().unwrap();
    assert_eq!(
        run.result.unwrap().state,
        "queued",
        "the Run was not touched"
    );
    assert_eq!(
        state.intake.filed_brief(&key).unwrap().unwrap().state,
        "claimed"
    );
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_is_refused_once_the_charge_no_longer_holds_its_lease() {
    let root = tempfile::tempdir().unwrap();
    let tag = "n3";
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    let operation = names_of(tag);
    let key = seed_dark(&state, &operation, "n3-dark");
    let own = desk(
        &state,
        ChargeScope::Operation(operation),
        &format!("k{tag}"),
    );

    // The lease lapses (the Station stopped renewing it).
    let held = record(&state, tag);
    state
        .intake
        .update_charge(
            &scope(tag),
            &held.station,
            held.fence,
            Some(Duration::from_nanos(1)),
            ChargeUpdate::default(),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(30));

    let (settling, mission) = (own.clone(), key.clone());
    let refused = off_the_workers(move || settling.settle(&mission, "failed", "dark")).await;
    assert!(refused.unwrap_err().contains("no longer holds its lease"));
    assert!(
        own.retry(&key, None)
            .await
            .unwrap_err()
            .contains("no longer holds its lease")
    );
    assert!(audit_actions(&state, &own.actor(), "mission.settle").is_empty());
    assert_eq!(
        state.intake.filed_brief(&key).unwrap().unwrap().state,
        "claimed"
    );
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_whose_holder_renewed_is_not_given_up() {
    let root = tempfile::tempdir().unwrap();
    let tag = "n4";
    let state = station(root.path(), tag, "test", "", "");
    let operation = names_of(tag);
    // The holder has renewed its lease since the Run was read as unheld.
    let run = seed(&state, &operation, "elsewhere", "n4-held", "working");
    let own = desk(
        &state,
        ChargeScope::Operation(operation),
        &format!("k{tag}"),
    );
    let refused = own.lose_unheld(&run, "looked dark").unwrap_err();
    assert!(refused.contains("could not be given up"), "{refused}");
    let after = state.intake.run(&run.id).unwrap().unwrap();
    assert_eq!(after.status, RunStatus::Active);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_from_an_operation_crosses_to_another_charge_only_as_a_code_or_through_a_compartment()
{
    let _only = M_CHARGE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("n5", "stn-n5");
    let state = kit(root.path(), tag, station, ("", true), "");
    let operation = names_of(tag);
    // The Operation has a Mission on this Station, so each may wake the other's charge.
    seed(&state, &operation, station, "n5-live", "working");
    charges::tend(&state, true, true).await;
    assert!(state.charges.is_live("m") && state.charges.is_live(&scope(tag)));
    let op_desk = desk(
        &state,
        ChargeScope::Operation(operation.clone()),
        &format!("k{tag}"),
    );
    let target = format!("station:{station}");

    // An Operation's charge wakes a Station's with no note or a code, never prose.
    let (waking, to) = (op_desk.clone(), target.clone());
    let free =
        off_the_workers(move || waking.wake(&to, Some("the build is red, please fix"), None)).await;
    assert!(
        free.unwrap_err()
            .contains("does not go to a Station's charge")
    );
    let (waking, to) = (op_desk.clone(), target.clone());
    off_the_workers(move || waking.wake(&to, Some("run-lost"), None))
        .await
        .unwrap();
    let (waking, to) = (op_desk.clone(), target.clone());
    off_the_workers(move || waking.wake(&to, None, None))
        .await
        .unwrap();

    // M relaying text it read in another Operation reaches this Operation's charge only through
    // a compartment; none is configured, so it is refused, and nothing was delivered.
    let m_desk = desk(&state, ChargeScope::M, &format!("m{tag}"));
    let before = state
        .intake
        .charge(&scope(tag))
        .unwrap()
        .unwrap()
        .controls
        .len();
    let (waking, to) = (m_desk.clone(), scope(tag));
    let refused =
        off_the_workers(move || waking.wake(&to, Some("they said X"), Some("OTHER"))).await;
    assert!(refused.unwrap_err().contains("no compartment"));
    let after = state
        .intake
        .charge(&scope(tag))
        .unwrap()
        .unwrap()
        .controls
        .len();
    assert_eq!(before, after);
    // Its own words, or text from the target's own Operation, go through, audited.
    let (waking, to, own) = (m_desk.clone(), scope(tag), operation.clone());
    off_the_workers(move || waking.wake(&to, Some("look at the board"), None))
        .await
        .unwrap();
    let (waking, to) = (m_desk.clone(), scope(tag));
    off_the_workers(move || waking.wake(&to, Some("same Operation"), Some(&own)))
        .await
        .unwrap();
    assert_eq!(audit_actions(&state, "charge:m", "charge.wake").len(), 2);
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_station_charge_is_given_no_operation_reason_in_events_or_tools() {
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("n6", "stn-n6");
    let state = kit(
        root.path(),
        tag,
        station,
        ("stuck_blocked_after = 0\nstuck_run_lost_after = 0\n", false),
        "",
    );
    let operation = names_of(tag);
    // A blocked Mission whose hold text is an Agent's words, and a lost one an earlier charge
    // settled with its reason.
    let held = seed(&state, &operation, station, "n6-held", "blocked");
    state
        .intake
        .update_run(
            &held.id,
            station,
            held.fence,
            RunUpdate::default()
                .result(RunResult::new("blocked").with_reason(Some("SECRET-HOLD-TEXT".to_owned()))),
        )
        .unwrap();
    seed(&state, &operation, station, "n6-lost", "lost");
    state
        .intake
        .settle_results(
            "n6-lost",
            &RunResult::new("stalled").with_reason(Some("SECRET-CHARGE-REASON".to_owned())),
            &RunResult::new("abandoned"),
        )
        .unwrap();

    let charge = state
        .roster
        .charges()
        .get(&format!("station:{station}"))
        .cloned()
        .unwrap();
    let looking = state.clone();
    let survey = off_the_workers(move || {
        charges::area::survey(&looking, &charge, &mut BTreeMap::new(), unix_now())
    })
    .await;
    assert!(!survey.stuck.is_empty());
    for stuck in &survey.stuck {
        let event = charges::area::stuck_detail(stuck, unix_now()).to_string();
        assert!(!event.contains("SECRET"), "{event}");
    }
    let station_desk = desk(
        &state,
        ChargeScope::Station(station.to_owned()),
        &format!("s{tag}"),
    );
    let reading = station_desk.clone();
    let board = off_the_workers(move || reading.board(None, None))
        .await
        .unwrap();
    assert!(!board.contains("SECRET"), "{board}");
    for mission in ["n6-held", "n6-lost"] {
        let (reading, mission) = (station_desk.clone(), mission.to_owned());
        let detail = off_the_workers(move || reading.state_of(&mission))
            .await
            .unwrap();
        assert!(!detail.contains("SECRET"), "{detail}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_withdraws_a_queued_mission_and_settles_one_whose_station_is_dark() {
    let root = tempfile::tempdir().unwrap();
    let tag = "n7";
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    let operation = names_of(tag);
    let own = desk(
        &state,
        ChargeScope::Operation(operation.clone()),
        &format!("k{tag}"),
    );

    let queued: crate::directorates::registry::store::Brief = serde_json::from_value(json!({
        "title": "work", "id": "n7-queued", "source": "test", "operation": operation,
    }))
    .unwrap();
    state.intake.submit_brief(&queued).unwrap();
    let answer = own.cancel(&queued.key(), "no longer wanted").await.unwrap();
    assert!(answer.contains("\"withdrawn\":true"), "{answer}");
    assert_eq!(
        state
            .intake
            .filed_brief(&queued.key())
            .unwrap()
            .unwrap()
            .state,
        "failed"
    );
    assert!(state.intake.list_new().unwrap().is_empty());
    assert_eq!(
        audit_actions(&state, &own.actor(), "mission.cancel"),
        std::slice::from_ref(&operation)
    );

    let dark = seed_dark(&state, &operation, "n7-dark");
    let answer = own.cancel(&dark, "its Station is gone").await.unwrap();
    assert!(answer.contains("\"settled\":\"failed\""), "{answer}");
    assert_eq!(
        state.intake.filed_brief(&dark).unwrap().unwrap().state,
        "failed"
    );
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m_is_told_of_a_charge_whose_last_turn_did_not_run_from_a_field_not_prose() {
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("n8", "stn-n8");
    let state = kit(
        root.path(),
        tag,
        station,
        ("stuck_charge_after = 0\n", true),
        "",
    );
    let scope_name = format!("station:{station}");
    let held = state
        .intake
        .claim_charge(
            &scope_name,
            &format!("s{tag}"),
            "elsewhere",
            Duration::from_secs(600),
        )
        .unwrap();
    let write = |kind: &str, text: &str| {
        state
            .intake
            .update_charge(
                &scope_name,
                "elsewhere",
                held.fence,
                None,
                ChargeUpdate::default().log(ChargeLogEntry::new(kind, text, 1.0)),
            )
            .unwrap();
    };
    // Prose that merely says it is not enough.
    write("turn", "woke for control but took no turn: it only says so");
    let m = state.roster.charges().get("m").cloned().unwrap();
    let looking = state.clone();
    let charge = m.clone();
    let survey = off_the_workers(move || {
        charges::area::survey(&looking, &charge, &mut BTreeMap::new(), unix_now())
    })
    .await;
    assert!(
        survey
            .stuck
            .iter()
            .all(|stuck| stuck.condition != "charge-failing")
    );
    write(
        "turn-refused",
        "woke for control but took no turn: the harness did not start",
    );
    let looking = state.clone();
    let survey = off_the_workers(move || {
        charges::area::survey(&looking, &m, &mut BTreeMap::new(), unix_now())
    })
    .await;
    assert!(
        survey
            .stuck
            .iter()
            .any(|stuck| stuck.condition == "charge-failing"
                && stuck.charge.as_deref() == Some(scope_name.as_str())),
        "{:?}",
        survey.stuck
    );
}

/// Opens a closed Run `number` of `mission` that reports `result`.
fn closed_run(state: &DaemonState, station: &str, mission: &str, number: u32, result: &str) {
    let id = format!("{mission}-{number}");
    let opened = state
        .intake
        .open_run(
            NewRun::new(id.as_str(), mission, station)
                .with_operation("OP")
                .with_number(number, None),
            Duration::from_secs(600),
        )
        .unwrap();
    state
        .intake
        .update_run(
            &id,
            station,
            opened.fence,
            RunUpdate::default()
                .result(RunResult::new(result))
                .status(RunStatus::Ended),
        )
        .unwrap();
}

fn result_of(state: &DaemonState, mission: &str, number: u32) -> String {
    state
        .intake
        .run(&format!("{mission}-{number}"))
        .unwrap()
        .unwrap()
        .result
        .unwrap()
        .state
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ended_mission_whose_earlier_runs_report_work_is_healed_at_start_and_by_the_survey() {
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("h1", "stn-h1");
    let state = kit(root.path(), tag, station, ("", true), "");
    for number in [1, 2] {
        closed_run(&state, station, "heal-start", number, "verifying");
    }
    closed_run(&state, station, "heal-start", 3, "done");

    let starting = state.clone();
    off_the_workers(move || charges::area::heal_at_start(&starting)).await;
    assert_eq!(result_of(&state, "heal-start", 1), "abandoned");
    assert_eq!(result_of(&state, "heal-start", 2), "abandoned");
    assert_eq!(result_of(&state, "heal-start", 3), "done");
    assert_eq!(
        audit_actions(&state, &format!("station:{station}"), "run.heal").len(),
        1
    );

    // A Mission that ended before the survey is healed by it, and shown as nothing open.
    for number in [1, 2] {
        closed_run(&state, station, "heal-survey", number, "verifying");
    }
    closed_run(&state, station, "heal-survey", 3, "done");
    let m = state.roster.charges().get("m").cloned().unwrap();
    let looking = state.clone();
    let survey = off_the_workers(move || {
        charges::area::survey(&looking, &m, &mut BTreeMap::new(), unix_now())
    })
    .await;
    assert!(survey.missions.is_empty(), "{:?}", survey.missions);
    assert_eq!(result_of(&state, "heal-survey", 1), "abandoned");
    assert_eq!(result_of(&state, "heal-survey", 2), "abandoned");
    assert_eq!(audit_actions(&state, "charge:m", "run.heal").len(), 1);
}
