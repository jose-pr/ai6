//! Charges keep their area working: they are told of Missions that are stuck, a quiet area takes
//! no timer turn, and each kind of charge fixes what it finds with the tools of its scope.

use std::time::Duration;

use serde_json::Value;

use super::station::{M_CHARGE, acting, headquarters_until, heard, kit, names_of, station_key};
use super::*;
use crate::directorates::m::charges::ChargeScope;
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
