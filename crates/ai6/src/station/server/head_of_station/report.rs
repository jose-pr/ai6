//! `station_report`: everything a round may read about its Station, and nothing secret.
//!
//! No environment values, tokens, loadouts, Cable bodies or brief text: Missions appear by
//! ID, Operation, Agent, role and state; Theaters as they are published at check-in.

use std::time::SystemTime;

use serde::Serialize;

use super::safe_houses::{self, KeptSafeHouse};
use crate::directorates::m::placement::{self, Resources};
use crate::directorates::m::theaters::TheaterSummary;

use super::super::{DaemonState, StationRole, station_terminal};

/// What the Station tells its Head of Station about itself.
#[derive(Debug, Clone, Serialize)]
pub(in super::super) struct StationState {
    station: String,
    /// When it was read, as Unix seconds.
    at: i64,
    /// Missions it may run at once; 0 for a Station without the execution role.
    capacity: usize,
    /// Its Missions that are not over, with their Sessions.
    missions: Vec<LiveMission>,
    /// Free and total space where its state and safe houses are.
    #[serde(skip_serializing_if = "Option::is_none")]
    disk: Option<Disk>,
    resources: Resources,
    theaters: Vec<TheaterState>,
    safe_houses: Vec<KeptSafeHouse>,
    /// Provider libraries loaded although no pin or publisher accepts them.
    plugins_weakened: Vec<String>,
    /// What the Head of Station may do here, as the Station enforces it.
    limits: Limits,
}

#[derive(Debug, Clone, Serialize)]
struct LiveMission {
    mission: String,
    operation: Option<String>,
    agent: Option<String>,
    role: Option<String>,
    state: String,
    /// Whether a Session of it runs on this Station now.
    session: bool,
}

#[derive(Debug, Clone, Serialize)]
struct Disk {
    path: String,
    free_mb: u64,
    total_mb: u64,
}

#[derive(Debug, Clone, Serialize)]
struct TheaterState {
    #[serde(flatten)]
    summary: TheaterSummary,
    /// Why placement would not run a Session there now, if it would not.
    #[serde(skip_serializing_if = "Option::is_none")]
    unfit: Option<String>,
}

/// The fixed limits, so a round knows them before it asks.
#[derive(Debug, Clone, Serialize)]
pub(super) struct Limits {
    pub(super) cache_dirs: Vec<String>,
    pub(super) cache_age_secs: u64,
    pub(super) unattended: [&'static str; 4],
    pub(super) green_ink: [&'static str; 1],
    pub(super) never: [&'static str; 4],
}

pub(super) fn limits(state: &DaemonState) -> Limits {
    let hygiene = &state.station.hygiene;
    Limits {
        cache_dirs: hygiene.caches().to_vec(),
        cache_age_secs: hygiene.cache_age().as_secs(),
        unattended: [
            "read everything station_report shows",
            "remove the safe house of a Mission that is over when everything in it exists elsewhere",
            "remove the cache directories named in cache_dirs from the safe house of a Mission over for at least cache_age_secs",
            "run the provisioning steps a Theater's operator declared for it (provision_theater with apply)",
        ],
        green_ink: [
            "remove the safe house of a Mission that is over while it holds work that exists nowhere else (kind delete)",
        ],
        never: [
            "change configuration, the roster or policy",
            "read a secret",
            "act on another Station",
            "touch the safe house of a Mission that is not over",
        ],
    }
}

/// Reads the Station's state. Blocks on the filesystem and safe-house providers; call it
/// from `spawn_blocking`.
pub(in super::super) fn station_state(state: &DaemonState) -> StationState {
    let now = SystemTime::now();
    let sessions = state
        .sessions
        .lock()
        .map(|sessions| sessions.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let missions = state
        .missions
        .lock()
        .map(|missions| missions.list())
        .unwrap_or_default()
        .into_iter()
        .filter(|mission| !safe_houses::is_over(state, mission))
        .map(|mission| LiveMission {
            session: sessions.contains(&mission.mission_id.to_string()),
            mission: mission.mission_id.to_string(),
            operation: mission.operation_id,
            agent: mission.agent_id,
            role: mission.role_id,
            state: format!("{:?}", mission.state).to_lowercase(),
        })
        .collect();
    let disk = crate::station::safehouse::state_dir()
        .ok()
        .and_then(|path| {
            let free = fs2::available_space(&path).ok()?;
            let total = fs2::total_space(&path).ok()?;
            Some(Disk {
                path: path.to_string_lossy().into_owned(),
                free_mb: free / (1024 * 1024),
                total_mb: total / (1024 * 1024),
            })
        });
    let theaters = state
        .transports
        .summaries(station_terminal(state).is_ok())
        .into_iter()
        .map(|summary| TheaterState {
            unfit: summary
                .unavailable
                .clone()
                .or_else(|| placement::fitness(&summary).err()),
            summary,
        })
        .collect();
    let safe_houses = safe_houses::kept(state)
        .iter()
        .map(|mission| safe_houses::describe(state, mission, now))
        .collect();
    StationState {
        station: state.station_id.clone(),
        at: state.clock.now(),
        capacity: if state.station.roles.contains(&StationRole::Execution) {
            state.mission_capacity()
        } else {
            0
        },
        missions,
        disk,
        resources: crate::station::resources::sample(),
        theaters,
        safe_houses,
        plugins_weakened: state.plugins.scan_summary().weakened.clone(),
        limits: limits(state),
    }
}
