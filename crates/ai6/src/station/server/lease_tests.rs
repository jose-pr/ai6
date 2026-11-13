//! A Station on a slow link keeps its Missions: leases last as long as the Station sets, a
//! Mission waiting for a person is never lost for want of a renewal, and a verdict settles a
//! Mission whose brief the Registry had already let go of.

use std::path::Path;
use std::time::Duration;

use super::placement_tests::{empty_book, pinned_brief};
use super::tests::{wait_for_state, wait_for_verifying};
use super::*;
use crate::bounds::Seconds;
use crate::directorates::registry::runs::{MissionVerdict, OutcomeVerdict};
use crate::directorates::registry::store::LeaseRecovery;
use crate::station::limits::StationLimits;

const AGENTS: &str = "[role.writer]\npurpose = \"Writes\"\npolicy = \"Write it.\"\n\
     [agent.007]\nhandler = \"mock-verdict\"\nmodel = \"clear\"\nroles = [\"writer\"]\n";

/// A Mission that ended its work and waits for a person's verdict, its brief claimed here.
async fn verifying(root: &Path, name: &str) -> (DaemonState, String) {
    let state = empty_book(root, AGENTS);
    let mut brief = pinned_brief(&format!("lease-{name}-{}", std::process::id()), &[]);
    brief.role = Some("writer".to_owned());
    state.intake.submit_brief(&brief).unwrap();
    schedule_intake_once(&state);
    schedule_queued_once(&state).await;
    let mission_id = brief.key();
    wait_for_verifying(&state, &mission_id).await;
    (state, mission_id)
}

/// The brief's lease runs out while its Station is away.
fn lapse_brief_lease(state: &DaemonState, mission_id: &str) -> Lease {
    let gone = Duration::from_nanos(1);
    state
        .intake
        .renew(mission_id, &state.station_id, gone)
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    state
        .intake
        .expired()
        .unwrap()
        .into_iter()
        .find(|lease| lease.key() == mission_id)
        .expect("the brief's lease ran out")
}

fn confirm(state: &DaemonState, mission_id: &str) {
    let verdict = MissionVerdict::new(
        OutcomeVerdict::Confirmed,
        "person-m",
        state.clock.now() as f64,
        None,
    );
    state
        .intake
        .record_verdict("G", mission_id, &verdict)
        .unwrap();
}

#[tokio::test]
async fn a_renewal_later_than_a_minute_after_the_last_still_finds_the_lease_held() {
    let root = tempfile::tempdir().unwrap();
    let (state, mission_id) = verifying(root.path(), "renewal").await;
    renew_run_leases(&state);
    let record = state.intake.latest_run(&mission_id).unwrap().unwrap();
    // The 60 seconds every lease once lasted had passed by the time a Station on a distant
    // Registry came round again.
    let a_slow_round = unix_now() + 90.0;
    assert!(record.leased_at(a_slow_round), "{record:?}");
    assert!(record.lease_expires - unix_now() > 170.0);
    state
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
}

#[tokio::test]
async fn the_lease_lasts_as_long_as_the_station_sets() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, mission_id) = verifying(root.path(), "setting").await;
    let mut limits = StationLimits::default();
    limits.scheduler.lease = Seconds::new(7);
    state.station = state.station.clone().with_limits(limits);
    assert_eq!(state.mission_lease(), Duration::from_secs(7));
    renew_run_leases(&state);
    let record = state.intake.latest_run(&mission_id).unwrap().unwrap();
    let left = record.lease_expires - unix_now();
    assert!((5.0..=7.5).contains(&left), "{left}");
    state
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
}

#[tokio::test]
async fn a_verifying_mission_whose_leases_ran_out_waits_for_its_verdict() {
    let root = tempfile::tempdir().unwrap();
    let (state, mission_id) = verifying(root.path(), "waits").await;
    // Its Run lease lapses as well.
    let hold = run_hold(&state, &mission_id).unwrap();
    state
        .intake
        .renew_run(
            &hold.id,
            &state.station_id,
            hold.fence,
            Duration::from_nanos(1),
        )
        .unwrap();
    let lapsed = lapse_brief_lease(&state, &mission_id);

    let extraction = recovery::extract(&state, &lapsed);
    assert!(matches!(extraction, recovery::Extraction::Wait));
    assert_eq!(extraction.max_runs(3), None);
    let record = state.intake.latest_run(&mission_id).unwrap().unwrap();
    assert!(!record.status.is_closed(), "its Run is not given up");
    assert_eq!(
        state
            .intake
            .filed_brief(&mission_id)
            .unwrap()
            .unwrap()
            .state,
        "claimed"
    );
    state
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
}

#[tokio::test]
async fn a_verdict_of_done_settles_a_mission_filed_lost_while_its_station_was_away() {
    let root = tempfile::tempdir().unwrap();
    let (state, mission_id) = verifying(root.path(), "lost").await;
    let lapsed = lapse_brief_lease(&state, &mission_id);
    // Recovery as it was before a verifying Mission waited: the last run spent.
    assert_eq!(
        state.intake.recover(&lapsed, 0, "hq").unwrap(),
        Some(LeaseRecovery::Failed)
    );
    let filed = state.intake.filed_brief(&mission_id).unwrap().unwrap();
    assert_eq!(
        (
            filed.state.as_str(),
            filed.outcome.unwrap()["state"].as_str()
        ),
        ("failed", Some("lost"))
    );

    confirm(&state, &mission_id);
    verdicts::settle_held(&state);
    wait_for_state(&state, &mission_id, MissionState::Done).await;

    let filed = state.intake.filed_brief(&mission_id).unwrap().unwrap();
    assert_eq!(filed.state, "done");
    let outcome = filed.outcome.unwrap();
    assert_eq!(outcome["state"], "done", "{outcome}");
    assert_eq!(outcome["verdict"]["verdict"], "confirmed", "{outcome}");
    state
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
}

#[tokio::test]
async fn a_verdict_of_done_settles_a_mission_whose_brief_was_queued_again() {
    let root = tempfile::tempdir().unwrap();
    let (state, mission_id) = verifying(root.path(), "requeued").await;
    let lapsed = lapse_brief_lease(&state, &mission_id);
    assert_eq!(
        state.intake.recover(&lapsed, 3, "hq").unwrap(),
        Some(LeaseRecovery::Requeued)
    );

    confirm(&state, &mission_id);
    verdicts::settle_held(&state);
    wait_for_state(&state, &mission_id, MissionState::Done).await;

    let filed = state.intake.filed_brief(&mission_id).unwrap().unwrap();
    assert_eq!(filed.state, "done", "{filed:?}");
    assert!(state.intake.list_new().unwrap().is_empty());
    state
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
}

#[test]
fn a_station_whose_check_in_is_over_a_minute_old_is_still_healthy_for_placement() {
    use crate::directorates::m::placement::StationFacts;

    let root = tempfile::tempdir().unwrap();
    let registry = crate::directorates::registry::store::FileRegistry::open(root.path());
    registry.check_in("slow", Default::default()).unwrap();
    let record = registry.stations(Duration::MAX).unwrap().remove(0);
    let window = registry.limits().stations.gone_dark_after.0;
    let late = StationFacts::from_record(&record, record.seen() + 100.0, window, 0);
    assert!(late.healthy, "silent for {}", late.silent_for);
    let gone = StationFacts::from_record(&record, record.seen() + 400.0, window, 0);
    assert!(!gone.healthy);
}
