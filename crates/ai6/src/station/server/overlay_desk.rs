//! What a Mission's start and resume ask of the overlays: what its Agent's loadout and its
//! Operation issue, and reporting what was delivered.

use std::collections::BTreeMap;

use super::{DaemonState, credentials};
use crate::directorates::q::overlays::{IssuedDigest, OverlayRecord};
use crate::directorates::registry::timeline::TimelineEvent;
use crate::station::overlays::{self, Delivered, Fit, Prepared, Request, Site};

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

/// Where this Session stands with its overlays: what is issued, checked against where they go.
pub(super) struct Target<'a> {
    /// The Theater the Session is placed on.
    pub(super) theater: &'a str,
    /// The Theater is the Station's own host and the harness runs on it.
    pub(super) here: bool,
    /// The harness runs where its commands do, not on the Station acting through tools.
    pub(super) harness_here: bool,
    /// The clearance runs commands.
    pub(super) commands: bool,
}

/// [`prepare`], and whether where the Session is placed has what those overlays need there. Its
/// error is what the Mission waits with.
pub(super) fn prepare_for(
    state: &DaemonState,
    agent: &str,
    operation: Option<&str>,
    environment: &BTreeMap<String, String>,
    target: &Target<'_>,
) -> Result<Option<(Prepared, Fit)>, String> {
    let Some(prepared) = prepare(state, agent, operation)? else {
        return Ok(None);
    };
    let site = site_of(state, target.theater, target.here)?;
    let limits = state.station.limits();
    let fit = prepared.fit(
        site,
        environment,
        target.commands,
        target.harness_here,
        (&state.station.overlay_keeper, limits.scheduler.check_in.0),
    )?;
    Ok(Some((prepared, fit)))
}

/// Where overlays go for a Session on `theater`.
pub(super) fn site_of(state: &DaemonState, theater: &str, here: bool) -> Result<Site, String> {
    if here {
        let state_dir = state
            .station
            .state_dir()
            .map_err(|error| error.to_string())?;
        Ok(Site::Here {
            dir: state_dir.join("overlays"),
        })
    } else {
        Site::theater(theater, state.transports.client(theater)?)
    }
}

/// Removes what `mission`'s overlays kept for it where it ran. A failure is logged and leaves an
/// empty directory behind.
pub(super) fn forget(state: &DaemonState, mission: &str, theater: &str, here: bool) {
    let removed =
        site_of(state, theater, here).and_then(|site| overlays::forget_session(&site, mission));
    if let Err(error) = removed {
        eprintln!("ai6: what Mission {mission:?}'s overlays kept was not removed: {error}");
    }
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
