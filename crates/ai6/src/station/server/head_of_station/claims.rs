//! `surrender_claim`: what the Station charge may do about a Run that makes no progress.
//!
//! A Mission this Station keeps whose Run has been quiet past the charge's `stall_after`, or whose
//! own watchdog already judged it stalled, may have its claim surrendered: the Station stops the
//! Run and lets its leases lapse, and the Registry queues the brief again or files it under the
//! Operation's `on_lost`, exactly as for a Run whose harness is gone. A Mission that waits on a
//! person, is over, or has made progress is refused: the Station never ends work it cannot show
//! is stuck.

use std::time::Duration;

use serde::Serialize;

use super::hygiene::Asker;
use crate::directorates::m::mission::MissionState;
use crate::directorates::registry::store::AuditRecord;

use super::super::{DaemonState, is_terminal_state, recovery};

/// What a surrender did.
#[derive(Debug, Clone, Serialize)]
pub(super) struct Surrendered {
    mission: String,
    /// What the Mission is now: `abandoned` when its brief is queued again, `failed` when it is
    /// filed under the Operation's `on_lost` or had no brief.
    state: String,
    /// Seconds the Run had made no progress, when it had a Run here.
    #[serde(skip_serializing_if = "Option::is_none")]
    quiet_secs: Option<u64>,
}

/// Why a Run does not count as stalled.
fn not_stalled(
    state: &DaemonState,
    mission_id: &str,
    mission_state: MissionState,
    quiet_after: Option<Duration>,
) -> Result<Option<Duration>, String> {
    let (quiet, stalled) = super::quiet_of(state, mission_id, mission_state, quiet_after);
    if stalled {
        return Ok(quiet);
    }
    Err(match (mission_state, quiet, quiet_after) {
        (MissionState::Working, Some(quiet), Some(after)) => format!(
            "its Run made progress {} seconds ago; it counts as stalled after {}",
            quiet.as_secs(),
            after.as_secs()
        ),
        (MissionState::Working, _, None) => "the charge sets no time after which a quiet Run is \
             stalled, and the Run's own watchdog has not judged it stalled"
            .to_owned(),
        (MissionState::Working, None, _) => "it has no Run here that can be judged".to_owned(),
        (other, _, _) => format!(
            "it is {}: it waits on someone or has ended, which is not a stall",
            other.as_str()
        ),
    })
}

/// Surrenders the claim on `mission_id`, a stalled Mission of this Station. Blocks.
///
/// # Errors
///
/// Why the Station refuses: no such Mission here, one that is over or waits on a person, or a Run
/// that made progress within `quiet_after`.
pub(super) fn surrender(
    state: &DaemonState,
    asker: &Asker<'_>,
    mission_id: &str,
    reason: Option<String>,
    quiet_after: Option<Duration>,
) -> Result<Surrendered, String> {
    let mission = state
        .missions
        .lock()
        .map_err(|_| "the Mission book is unavailable".to_owned())?
        .get(mission_id)
        .cloned()
        .ok_or("this Station keeps no such Mission")?;
    if is_terminal_state(mission.state) {
        return Err("the Mission is over".to_owned());
    }
    let quiet = not_stalled(state, mission_id, mission.state, quiet_after)?;
    let why = format!(
        "claim surrendered by the Station's charge: {}",
        reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("no progress")
    );
    // Audited before the Run is stopped: a surrender that is not on record does not happen.
    let record = AuditRecord {
        id: format!(
            "head_of_station.surrendered-{}",
            uuid::Uuid::new_v4().simple()
        ),
        actor: format!("agent:{}", asker.agent),
        action: "head_of_station.surrendered".to_owned(),
        operation: mission.operation_id.clone().unwrap_or_default(),
        resource: Some(format!("station:{}", state.station_id)),
        target: mission_id.to_owned(),
        at: state.clock.now() as f64,
    };
    state
        .intake
        .record_audit(&record)
        .map_err(|error| format!("the surrender was not audited, so the Run goes on: {error}"))?;
    recovery::give_up(state, mission_id, None, &why);
    let now = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).map(|mission| mission.state));
    Ok(Surrendered {
        mission: mission_id.to_owned(),
        state: now.unwrap_or(mission.state).as_str().to_owned(),
        quiet_secs: quiet.map(|quiet| quiet.as_secs()),
    })
}
