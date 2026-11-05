//! What a Mission's start and resume ask of the overlays: what its Agent's loadout and its
//! Operation issue, and reporting what was delivered.

use std::collections::BTreeMap;

use super::{DaemonState, credentials};
use crate::directorates::q::overlays::{IssuedDigest, OverlayRecord};
use crate::directorates::registry::timeline::TimelineEvent;
use crate::station::overlays::{self, Delivered, Prepared, Request};

/// The overlays a Session of `agent` in `operation` is issued, fetched and verified, or why it
/// cannot start. `None` when none is issued. Blocks on the store and on a fetch.
pub(super) fn prepare(
    state: &DaemonState,
    agent: &str,
    operation: Option<&str>,
) -> Result<Option<Prepared>, String> {
    let catalog = state.station.credentials.catalog();
    let loadout: Vec<String> = catalog
        .and_then(|catalog| catalog.for_agent(agent).ok())
        .map(|loadout| loadout.overlay_names().to_vec())
        .unwrap_or_default();
    let added: Vec<String> = operation
        .and_then(|id| state.roster.operation(id).ok())
        .map(|operation| operation.overlays.clone())
        .unwrap_or_default();
    if loadout.is_empty() && added.is_empty() {
        return Ok(None);
    }
    let no_records = BTreeMap::new();
    let records: &BTreeMap<String, OverlayRecord> =
        catalog.map_or(&no_records, |catalog| catalog.overlays());
    let state_dir = state
        .station
        .state_dir()
        .map_err(|error| error.to_string())?;
    let limits = state.station.limits();
    let secret = |name: &str| credentials::read_for_overlay(state, name);
    overlays::prepare(
        &Request {
            loadout: &loadout,
            operation: &added,
            records,
            limits: &limits.overlays,
            state_dir: &state_dir,
            base: state.station.overlay_base.as_deref(),
        },
        &secret,
    )
}

/// Reports what was delivered, each weakening on the timeline and in the log, naming the overlay
/// and the variable or path, never a value.
pub(super) fn report(
    state: &DaemonState,
    mission_id: &str,
    operation: Option<&str>,
    delivered: &Delivered,
) {
    for reported in &delivered.weakenings {
        let id = &reported.weakening.id;
        let subject = &reported.weakening.subject;
        eprintln!(
            "ai6: Mission {mission_id:?}: overlay {:?} is weakened: {id} {subject}",
            reported.overlay
        );
        if let Some(operation) = operation {
            let event =
                TimelineEvent::new("overlay", "weakened", operation, state.clock.now() as f64)
                    .station(state.station_id.clone())
                    .mission(mission_id)
                    .detail("overlay", reported.overlay.clone())
                    .detail("weakening", id.clone())
                    .detail("subject", subject.clone());
            record(state, mission_id, &event);
        }
    }
}

/// Says on the timeline that a resumed Session got other digests than it started with.
pub(super) fn report_change(
    state: &DaemonState,
    mission_id: &str,
    operation: Option<&str>,
    before: &[IssuedDigest],
    after: &[IssuedDigest],
) {
    let Some(change) = overlays::change_between(before, after) else {
        return;
    };
    eprintln!("ai6: Mission {mission_id:?} resumes with other overlays: {change}");
    if let Some(operation) = operation {
        let event = TimelineEvent::new("overlay", "changed", operation, state.clock.now() as f64)
            .station(state.station_id.clone())
            .mission(mission_id)
            .detail("change", change);
        record(state, mission_id, &event);
    }
}

fn record(state: &DaemonState, mission_id: &str, event: &TimelineEvent) {
    if let Err(error) = state.intake.record_event(event) {
        eprintln!("ai6: timeline event of Mission {mission_id:?} was not written: {error}");
    }
}
