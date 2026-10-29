//! The safe houses a Station keeps: the isolated working directories of its Missions that
//! held work when they ended, or that are still in use.

use std::path::Path;
use std::time::SystemTime;

use serde::Serialize;

use super::survey::{Survey, survey};
use crate::directorates::m::mission::Mission;
use crate::directorates::q::safehouse::SafeHouseReport;
use crate::station::safehouse::{SafeHouse, SafeHouseHygiene};

use super::super::{DaemonState, is_terminal_state, open_safe_house};

/// One kept safe house as the Head of Station sees it.
#[derive(Debug, Clone, Serialize)]
pub(super) struct KeptSafeHouse {
    pub(super) mission: String,
    pub(super) operation: Option<String>,
    /// The Mission's state: `done`, `failed`, `working` and so on.
    pub(super) state: String,
    /// Whether its Mission is not over, or a Session of it still runs here.
    pub(super) live: bool,
    pub(super) path: String,
    pub(super) branch: Option<String>,
    #[serde(flatten)]
    pub(super) survey: Survey,
    /// Whether everything in it also exists elsewhere; `None` when that cannot be told.
    pub(super) exported: Option<bool>,
    /// Changes since the base that no remote holds, newest first (at most 20).
    pub(super) unexported: Vec<String>,
    /// Whether it holds uncommitted changes; `None` when that cannot be told.
    pub(super) dirty: Option<bool>,
    /// Paths version control ignores (at most 20); those outside the cache directories exist
    /// nowhere else.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) ignored: Vec<String>,
    /// Why `exported` is unknown, when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) note: Option<String>,
}

/// Whether an ignored `path` (relative, `/`-separated) lies in a cache directory the hygiene
/// names, so it can be rebuilt.
pub(super) fn rebuildable(hygiene: &SafeHouseHygiene, path: &str) -> bool {
    path.split(['/', '\\'])
        .any(|component| hygiene.is_cache(component))
}

/// Whether `mission` is over and no Session of it runs on this Station.
pub(super) fn is_over(state: &DaemonState, mission: &Mission) -> bool {
    is_terminal_state(mission.state)
        && state
            .sessions
            .lock()
            .map(|sessions| !sessions.contains_key(mission.mission_id.as_str()))
            .unwrap_or(false)
}

/// The kept safe house of a Mission, when it has one still on disk.
pub(super) fn kept_report(mission: &Mission) -> Option<&SafeHouseReport> {
    mission
        .safe_house
        .as_ref()
        .filter(|report| report.kept && Path::new(&report.path).is_dir())
}

/// This Station's Missions whose isolated safe house is kept and still on disk.
pub(super) fn kept(state: &DaemonState) -> Vec<Mission> {
    state
        .missions
        .lock()
        .map(|missions| missions.list())
        .unwrap_or_default()
        .into_iter()
        .filter(|mission| kept_report(mission).is_some())
        .collect()
}

/// Re-attaches to a Mission's kept safe house through its Operation's provider.
pub(super) fn reopen(state: &DaemonState, mission: &Mission) -> Result<SafeHouse, String> {
    let report = kept_report(mission)
        .cloned()
        .ok_or("the Mission keeps no safe house on this Station")?;
    let operation_id = mission
        .operation_id
        .as_deref()
        .ok_or("the Mission has no Operation, so its safe-house provider is unknown")?;
    let operation = state
        .roster
        .operation(operation_id)
        .map_err(|_| format!("Operation {operation_id:?} is no longer configured here"))?;
    open_safe_house(
        &state.plugins,
        operation,
        &mission.mission_id,
        &mission.prompt,
        None,
        true,
        Some(report),
        None,
    )?
    .ok_or_else(|| format!("Operation {operation_id:?} has no safe-house provider"))
}

/// Describes a kept safe house: its Mission, what is on disk and what exists only here.
pub(super) fn describe(state: &DaemonState, mission: &Mission, now: SystemTime) -> KeptSafeHouse {
    let (path, branch) = kept_report(mission)
        .map(|report| (report.path.clone(), report.branch.clone()))
        .unwrap_or_default();
    let mut described = KeptSafeHouse {
        mission: mission.mission_id.to_string(),
        operation: mission.operation_id.clone(),
        state: format!("{:?}", mission.state).to_lowercase(),
        live: !is_over(state, mission),
        survey: survey(
            Path::new(&path),
            &state.station.hygiene,
            now,
            state.station.limits().head_of_station.survey_entries,
        ),
        path,
        branch,
        exported: None,
        unexported: Vec::new(),
        dirty: None,
        ignored: Vec::new(),
        note: None,
    };
    match reopen(state, mission).and_then(|house| house.exports().map_err(|e| e.to_string())) {
        Ok(Some(exports)) => {
            let hygiene = &state.station.hygiene;
            described.exported = Some(exports.is_exported(|path| rebuildable(hygiene, path)));
            described.dirty = Some(exports.dirty);
            described.ignored = exports
                .ignored
                .into_iter()
                .take(
                    state
                        .station
                        .limits()
                        .head_of_station
                        .listed_changes
                        .or_max(),
                )
                .collect();
            described.unexported = exports
                .unexported
                .into_iter()
                .take(
                    state
                        .station
                        .limits()
                        .head_of_station
                        .listed_changes
                        .or_max(),
                )
                .collect();
        }
        Ok(None) => {
            described.note = Some(
                "its provider cannot tell what of the work exists elsewhere, so all of it counts \
                 as unexported"
                    .to_owned(),
            );
        }
        Err(error) => described.note = Some(format!("it could not be inspected: {error}")),
    }
    described
}
