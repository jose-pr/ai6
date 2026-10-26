//! The Station's side of recovery ("extraction"): what the Headquarters does with a Mission
//! whose Station went dark, how a Station adopts a Session it is handed, and the notes and
//! checkpoints a later attempt's hand-off is made of.
//!
//! A Session a runner supervises on a Theater other Stations reach is adopted: another
//! Station takes its lease and re-attaches, so nothing is launched twice. Any other Session
//! ended with its Station, so it is marked lost and its brief is queued again; the next
//! attempt links to it, starts from its last checkpoint and is told what it was doing.
//!
//! A live Station gives up a Session the same way when its runner, or wrapped harness, stays
//! out of reach past the Operation's `lost_after_secs` and a last probe fails, or when a
//! re-attach keeps failing for that long ([`give_up`], [`reattach_failed`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{
    DaemonState, INTAKE_MAX_ATTEMPTS, credentials, ensure_leased_mission, queue_event, runner_link,
    session_hold, unix_now,
};
use crate::directorates::m::mission::{LaunchManifest, MissionState, SessionHold};
use crate::directorates::m::placement::{OperationPolicy, PlacementRequest, StationFacts};
use crate::directorates::m::recovery::{DEFAULT_LOST_AFTER_SECS, adopter, reattach_delay};
use crate::directorates::m::roster::OnLost;
use crate::directorates::m::theaters::TheaterAccess;
use crate::directorates::q::safehouse::{CheckpointSpec, SafeHouseCheckpoint};
use crate::directorates::registry::sessions::{
    MAX_RECENT_EVENT_CHARS, MAX_RECENT_EVENTS, SessionError, SessionLaunch, SessionRecord,
    SessionStatus, SessionUpdate,
};
use crate::directorates::registry::store::{DEFAULT_LEASE, GONE_DARK_AFTER, Lease, LeaseRecovery};
use crate::directorates::section_00::agent::{AgentEvent, AgentSession};
use crate::station::reach::ReachWatch;
use crate::{MissionId, SessionId};

/// How long a lease this Station gives up is renewed for: it lapses at once, so the
/// Registry treats the Session or brief like one whose Station stopped renewing it.
const LAPSE: Duration = Duration::from_nanos(1);
/// Tries at a Registry step that a concurrent renewal or an equal timestamp can refuse.
const GIVE_UP_TRIES: u32 = 10;

/// The brief-lease metadata key that hands a Station a Session to adopt; its value is the
/// Session record's ID.
pub(super) const ADOPT_KEY: &str = "adopt";

/// How often a Session's recent events are written to its record while they change.
const NOTES_EVERY: Duration = Duration::from_secs(15);

/// What the Headquarters does with an expired brief lease of another Station.
pub(super) enum Extraction {
    /// Nothing ran for the brief, or its last Session already ended: queue it again.
    Requeue,
    /// The lease went to a Station that re-attaches to the Session.
    HandedOver,
    /// The Session's own lease is still live; decide once it lapses too.
    Wait,
    /// The Session was lost; queue the brief again for a new attempt, or file it as failed
    /// when the Operation says so.
    Lost {
        /// The Operation's response.
        on_lost: OnLost,
    },
}

impl Extraction {
    /// The attempt limit to recover the brief lease with, or `None` to leave it alone.
    pub(super) fn max_attempts(&self) -> Option<u32> {
        match self {
            Self::Requeue
            | Self::Lost {
                on_lost: OnLost::Retry,
            } => Some(INTAKE_MAX_ATTEMPTS),
            // Recovering at a limit of zero files the brief as lost.
            Self::Lost { .. } => Some(0),
            Self::HandedOver | Self::Wait => None,
        }
    }
}

/// Decides what happens to the Mission behind an expired brief lease whose Station stopped
/// renewing it: hand it to a Station that adopts its Session, or mark the Session lost.
pub(super) fn extract(state: &DaemonState, lease: &Lease) -> Extraction {
    let Some(mission_id) = meta_str(lease, "mission_id") else {
        return Extraction::Requeue;
    };
    let record = match state.intake.latest_session(mission_id) {
        Ok(Some(record)) if !record.status.is_closed() => record,
        // A live Station that gave its Session up lets the brief lease lapse as well.
        Ok(Some(record)) if record.status == SessionStatus::Lost => {
            return Extraction::Lost {
                on_lost: operation_on_lost(state, lease),
            };
        }
        Ok(_) => return Extraction::Requeue,
        Err(error) => {
            eprintln!("ai6: Mission {mission_id:?} not recovered yet: {error}");
            return Extraction::Wait;
        }
    };
    if record.leased_at(unix_now()) {
        return Extraction::Wait;
    }
    let why = match adoption_choice(state, lease, &record) {
        Ok(station) => {
            let meta =
                BTreeMap::from([(ADOPT_KEY.to_owned(), serde_json::json!(record.session_id))]);
            match state.intake.hand_over(lease, &station, DEFAULT_LEASE, meta) {
                Ok(Some(_)) => eprintln!(
                    "ai6: Station {:?} went dark; Station {station:?} adopts Mission \
                     {mission_id:?} and re-attaches to its Session",
                    record.station
                ),
                Ok(None) => {}
                Err(error) => eprintln!("ai6: Mission {mission_id:?} not handed over: {error}"),
            }
            return Extraction::HandedOver;
        }
        Err(why) => why,
    };
    let reason = format!(
        "Station {:?} went dark{}; {why}",
        record.station,
        record
            .theater
            .as_deref()
            .map(|theater| format!(" with Theater {theater:?}"))
            .unwrap_or_default()
    );
    match state.intake.lose_session(&record.session_id, &reason) {
        Ok(_) => eprintln!("ai6: Mission {mission_id:?} lost its Session: {reason}"),
        Err(SessionError::Ended) => {}
        Err(SessionError::HeldBy(_)) => return Extraction::Wait,
        Err(error) => {
            eprintln!("ai6: Mission {mission_id:?} not recovered yet: {error}");
            return Extraction::Wait;
        }
    }
    Extraction::Lost {
        on_lost: operation_on_lost(state, lease),
    }
}

/// The `on_lost` of the Operation a brief lease names.
fn operation_on_lost(state: &DaemonState, lease: &Lease) -> OnLost {
    meta_str(lease, "operation")
        .and_then(|operation| state.roster.operation(operation).ok())
        .map_or(OnLost::default(), |operation| operation.on_lost)
}

/// The watch for a Session of `operation_id`, with that Operation's `lost_after_secs`.
pub(super) fn reach_watch(state: &DaemonState, operation_id: Option<&str>) -> ReachWatch {
    let secs = operation_id
        .and_then(|id| state.roster.operation(id).ok())
        .map_or(DEFAULT_LOST_AFTER_SECS, |operation| {
            operation.lost_after_secs
        });
    ReachWatch::new(Duration::from_secs(secs))
}

/// Gives up a Session this Station holds whose harness is gone or stayed out of reach.
///
/// The Station stops acting on it and lets its leases lapse: the Session is marked lost with
/// `reason`, and the Mission's brief goes back to the Registry, which queues it again as a new
/// attempt or files it under the Operation's `on_lost`, as for a Session lost with its
/// Station. A Mission that came from no brief has nothing to queue again and fails. `session`
/// is the harness the caller reads, when it reads one; another one is left alone.
pub(super) fn give_up(
    state: &DaemonState,
    mission_id: &str,
    session: Option<&Arc<Mutex<guatiao::library::Object<dyn AgentSession>>>>,
    reason: &str,
) {
    let operation = state.sessions.lock().ok().and_then(|mut sessions| {
        let current = sessions.get(mission_id).is_some_and(|operation| {
            session.is_none_or(|session| Arc::ptr_eq(&operation.session, session))
        });
        if current {
            sessions.remove(mission_id)
        } else {
            None
        }
    });
    if session.is_some() && operation.is_none() {
        return;
    }
    eprintln!("ai6: Mission {mission_id:?} lost its Session: {reason}");
    if let Some(operation) = &operation {
        operation.gate.close();
        // A runner that comes back keeps nothing for this Station; the next attempt ends it.
        if let Some(runner) = &operation.runner {
            runner
                .detached
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if let Ok(session) = operation.session.lock() {
            let _ = session.close();
        }
    }
    credentials::release(state, mission_id);

    // The hold goes first, so nothing renews the Session lease this Station lets lapse.
    let hold = state.missions.lock().ok().and_then(|mut missions| {
        let mut manifest = missions.operation(mission_id)?;
        let hold = manifest.session.take();
        manifest.runner = None;
        let _ = missions.set_operation(mission_id, manifest);
        hold
    });
    if let Some(hold) = &hold {
        lose_held(state, hold, reason);
    }
    let to = hand_back(state, mission_id);
    if let Some(hold) = &hold {
        super::agents::count_lost(state, mission_id, hold.attempt);
    }
    // The loss is counted and its reason queued before the Mission ends, so whoever sees it
    // ended also sees why.
    queue_event(
        state,
        mission_id,
        AgentEvent {
            kind: "lost".to_owned(),
            text: Some(reason.to_owned()),
            data: None,
            limit: None,
            usage: None,
            quota: Vec::new(),
            tools: Vec::new(),
        },
    );
    if let Ok(mut missions) = state.missions.lock()
        && let Err(error) = missions.transition(mission_id, to)
    {
        eprintln!("ai6: Mission {mission_id:?} could not be marked {to:?}: {error:?}");
    }
    #[cfg(feature = "publisher")]
    if let Some(telemetry) = operation.and_then(|operation| operation.telemetry)
        && let Some(name) = super::lifecycle_name(to)
    {
        telemetry.publish_lifecycle_for(name, None);
    }
    state.event_ready.notify_waiters();
}

/// Lets the Session lease `hold` names lapse and marks the Session lost.
fn lose_held(state: &DaemonState, hold: &SessionHold, reason: &str) {
    let station = state.station_id.as_str();
    for _ in 0..GIVE_UP_TRIES {
        match state
            .intake
            .renew_session(&hold.id, station, hold.fence, LAPSE)
        {
            Ok(_) => {}
            // Ended, or another Station holds it now: it is not this Station's to lose.
            Err(SessionError::Ended | SessionError::Stale | SessionError::HeldBy(_)) => return,
            Err(error) => {
                eprintln!("ai6: Session {:?} not marked lost: {error}", hold.id);
                return;
            }
        }
        match state.intake.lose_session(&hold.id, reason) {
            Ok(_) | Err(SessionError::Ended) => return,
            // Within the clock tick it lapsed in, the lease reads as live.
            Err(SessionError::HeldBy(holder)) if holder == station => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                eprintln!("ai6: Session {:?} not marked lost: {error}", hold.id);
                return;
            }
        }
    }
    eprintln!("ai6: Session {:?} not marked lost yet", hold.id);
}

/// Lets this Station's brief lease on the Mission lapse and recovers it under the
/// Operation's `on_lost`. Answers the state the Mission ends in here: `Abandoned` when its
/// brief is queued again, `Failed` when it is filed or there was no brief.
fn hand_back(state: &DaemonState, mission_id: &str) -> MissionState {
    let station = state.station_id.as_str();
    let ours = || {
        state.intake.leases().ok().and_then(|leases| {
            leases.into_iter().find(|lease| {
                lease.station() == station && meta_str(lease, "mission_id") == Some(mission_id)
            })
        })
    };
    let Some(first) = ours() else {
        return MissionState::Failed;
    };
    let on_lost = operation_on_lost(state, &first);
    let handed = match on_lost {
        OnLost::Retry => MissionState::Abandoned,
        OnLost::Fail => MissionState::Failed,
    };
    let max_attempts = Extraction::Lost { on_lost }
        .max_attempts()
        .unwrap_or(INTAKE_MAX_ATTEMPTS);
    for _ in 0..GIVE_UP_TRIES {
        let Some(lease) = ours() else {
            // Another Headquarters recovered it once it lapsed.
            return handed;
        };
        if let Err(error) = state.intake.renew(lease.key(), station, LAPSE) {
            eprintln!("ai6: Mission {mission_id:?} brief not handed back yet: {error}");
        }
        // Read back: recovering compares the lease as stored.
        if let Ok(Some(lapsed)) = state.intake.lease(lease.key())
            && lapsed.station() == station
        {
            match state.intake.recover(&lapsed, max_attempts, station) {
                Ok(Some(LeaseRecovery::Requeued)) => return MissionState::Abandoned,
                Ok(Some(_)) => return MissionState::Failed,
                Ok(None) => {}
                Err(error) => {
                    eprintln!("ai6: Mission {mission_id:?} brief not handed back yet: {error}")
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    eprintln!(
        "ai6: Mission {mission_id:?} brief not handed back; Headquarters recovers it once its \
         lease lapses"
    );
    handed
}

/// Whether a lost attempt at a Mission this Station had left behind is what a new brief lease
/// asks it to run: the Mission ended here, holds no Session, and its latest Session is lost.
pub(super) fn retries_lost_attempt(state: &DaemonState, mission_id: &str) -> bool {
    session_hold(state, mission_id).is_none()
        && state
            .intake
            .latest_session(mission_id)
            .ok()
            .flatten()
            .is_some_and(|record| record.status == SessionStatus::Lost)
}

/// Re-attach tries that failed in a row, and since when, by Station and Mission.
static REATTACHING: Mutex<BTreeMap<(String, String), (u32, Instant)>> = Mutex::new(BTreeMap::new());

/// A try at re-attaching to (or resuming) a Mission's Session after a restart or adoption
/// failed. Answers when to try again; once the tries have failed for the Operation's
/// `lost_after_secs`, the Session's runner or Theater is probed once more, and if that fails
/// too the Session is given up as lost and `None` is answered.
pub(super) fn reattach_failed(state: &DaemonState, mission_id: &str) -> Option<Instant> {
    let key = (state.station_id.clone(), mission_id.to_owned());
    let now = Instant::now();
    let (failures, since) = {
        let mut tries = REATTACHING.lock().unwrap_or_else(|e| e.into_inner());
        let entry = tries.entry(key.clone()).or_insert((0, now));
        entry.0 += 1;
        *entry
    };
    let operation_id = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id)?.operation_id.clone());
    let grace = reach_watch(state, operation_id.as_deref()).grace();
    if now.duration_since(since) >= grace {
        match probe_harness(state, mission_id) {
            Some(Err(why)) => {
                reattached(state, mission_id);
                give_up(
                    state,
                    mission_id,
                    None,
                    &format!(
                        "its Session could not be re-attached for {}s and a probe through the \
                         Theater's transport failed: {why}",
                        grace.as_secs()
                    ),
                );
                return None;
            }
            // It answers, or there is nothing remote to probe: keep trying, from a new grace.
            _ => {
                if let Ok(mut tries) = REATTACHING.lock() {
                    tries.insert(key, (failures, Instant::now()));
                }
            }
        }
    }
    Some(now + reattach_delay(failures))
}

/// Forgets the failed re-attach tries of a Mission, once one succeeds or it is given up.
pub(super) fn reattached(state: &DaemonState, mission_id: &str) {
    if let Ok(mut tries) = REATTACHING.lock() {
        tries.remove(&(state.station_id.clone(), mission_id.to_owned()));
    }
}

/// Probes what a Mission's Session ran on: its runner when one supervised it, else the
/// Theater's transport. `None` when the harness ran on this Station's own host.
fn probe_harness(state: &DaemonState, mission_id: &str) -> Option<Result<(), String>> {
    let manifest = state.missions.lock().ok()?.operation(mission_id)?;
    let theater_id = manifest.theater_id.clone()?;
    if let Some(key) = &manifest.runner {
        let link = match runner_link(state, &theater_id) {
            Ok(Some(link)) => link,
            Ok(None) => return Some(Err(format!("Theater {theater_id:?} has no runner now"))),
            Err(why) => return Some(Err(why)),
        };
        return Some(match link.status(key) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(format!("the runner no longer has Session {key:?}")),
            Err(why) => Err(why),
        });
    }
    let theater = state.transports.theater(&theater_id)?;
    if theater.access == TheaterAccess::Local {
        return None;
    }
    let client = match state.transports.client(&theater_id) {
        Ok(client) => client,
        Err(why) => return Some(Err(why)),
    };
    Some(
        client
            .run(&crate::directorates::q::transport::RunSpec {
                argv: vec!["true".to_owned()],
                timeout_ms: 30_000,
                ..Default::default()
            })
            .map_err(|error| crate::directorates::q::transport::describe(&error))
            .and_then(|output| match output.code {
                Some(0) => Ok(()),
                code => Err(format!("`true` exited with {code:?} on the Theater")),
            }),
    )
}

/// The Station that should adopt `record`, or why none may.
fn adoption_choice(
    state: &DaemonState,
    lease: &Lease,
    record: &SessionRecord,
) -> Result<String, String> {
    let operation = meta_str(lease, "operation").ok_or("its brief names no Operation")?;
    let policy =
        OperationPolicy::from_roster(&state.roster, operation).map_err(|e| e.to_string())?;
    let stations = state
        .intake
        .stations(Duration::MAX)
        .map_err(|error| error.to_string())?;
    let leases = state.intake.leases().unwrap_or_default();
    let now = unix_now();
    let facts: Vec<StationFacts> = stations
        .iter()
        .map(|station| {
            let load = leases
                .iter()
                .filter(|lease| lease.station() == station.station())
                .count();
            StationFacts::from_record(station, now, GONE_DARK_AFTER, load)
        })
        .collect();
    let request = PlacementRequest {
        stations: state
            .intake
            .read_claimed(lease.key())
            .ok()
            .flatten()
            .map(|brief| brief.stations)
            .unwrap_or_default(),
        agent: meta_str(lease, "agent").map(str::to_owned),
        deciding_station: Some(state.station_id.clone()),
        ..PlacementRequest::default()
    };
    adopter(&policy, &request, &facts, record).map(|choice| choice.station)
}

/// What became of a brief lease that may ask this Station to adopt a Session.
pub(super) enum Adoption {
    /// The lease asks for nothing to be adopted, or the Mission is already here.
    NotAsked,
    /// The Session cannot be taken yet; try again on the next pass.
    Pending,
    /// The Session is now this Station's and is queued to re-attach.
    Adopted,
}

/// Takes over the Session a brief lease handed to this Station: takes its lease, records the
/// Mission as its last Station launched it, and queues the re-attach. A Session that ended
/// in the meantime is not adopted, so the Mission starts as a new attempt instead.
pub(super) fn adopt(state: &DaemonState, lease: &Lease) -> Adoption {
    let (Some(session_id), Some(mission_id)) =
        (meta_str(lease, ADOPT_KEY), meta_str(lease, "mission_id"))
    else {
        return Adoption::NotAsked;
    };
    let here = state
        .missions
        .lock()
        .map(|missions| missions.get(mission_id).is_some())
        .unwrap_or(true);
    if here {
        return Adoption::NotAsked;
    }
    let launch = match state.intake.session(session_id) {
        Ok(Some(record)) if !record.status.is_closed() => match record.launch {
            Some(launch) => launch,
            None => return Adoption::NotAsked,
        },
        Ok(_) => return Adoption::NotAsked,
        Err(error) => {
            eprintln!("ai6: Session {session_id:?} not adopted yet: {error}");
            return Adoption::Pending;
        }
    };
    let record = match state
        .intake
        .take_session(session_id, &state.station_id, DEFAULT_LEASE)
    {
        Ok(record) => record,
        Err(SessionError::Ended | SessionError::NotFound) => return Adoption::NotAsked,
        Err(error) => {
            eprintln!("ai6: Session {session_id:?} not adopted yet: {error}");
            return Adoption::Pending;
        }
    };
    if let Err(error) = ensure_leased_mission(state, lease) {
        eprintln!("ai6: Mission {mission_id:?} not adopted: {error}");
        return Adoption::Pending;
    }
    match record_adopted(state, mission_id, &record, launch) {
        Ok(()) => {
            eprintln!(
                "ai6: adopted Mission {mission_id:?}; re-attaching to its Session on Theater {:?}",
                record.theater.as_deref().unwrap_or_default()
            );
            Adoption::Adopted
        }
        Err(error) => {
            eprintln!("ai6: Mission {mission_id:?} not adopted: {error}");
            Adoption::Pending
        }
    }
}

/// Records an adopted Session in this Station's Mission book as its last Station launched
/// it, and queues the re-attach (or the resume, for a parked Session).
fn record_adopted(
    state: &DaemonState,
    mission_id: &str,
    record: &SessionRecord,
    launch: SessionLaunch,
) -> Result<(), String> {
    let manifest = LaunchManifest {
        handler_provider: launch.handler,
        terminal_provider: launch.terminal,
        working_directory: launch.working_directory,
        model: launch.model,
        effort: launch.effort,
        cover: None,
        station_id: Some(state.station_id.clone()),
        theater_id: record.theater.clone(),
        mode: launch.mode,
        session: Some(SessionHold {
            id: record.session_id.clone(),
            attempt: record.attempt,
            fence: record.fence,
        }),
        runner: launch.runner,
    };
    let mut missions = state
        .missions
        .lock()
        .map_err(|_| "Mission book lock was poisoned".to_owned())?;
    missions
        .set_operation(mission_id, manifest)
        .map_err(|error| format!("{error:?}"))?;
    if let Some(provider) = record
        .provider_session_id
        .as_deref()
        .and_then(|id| SessionId::try_new(id).ok())
    {
        missions
            .attach_session(mission_id, provider)
            .map_err(|error| format!("{error:?}"))?;
    }
    missions
        .transition(mission_id, MissionState::Working)
        .map_err(|error| format!("{error:?}"))?;
    match (record.status, record.parked_until) {
        (SessionStatus::Parked, Some(until)) => {
            missions
                .park(mission_id, until as i64)
                .map_err(|error| format!("{error:?}"))?;
        }
        _ => {
            missions
                .transition(mission_id, MissionState::Blocked)
                .map_err(|error| format!("{error:?}"))?;
            drop(missions);
            if let Ok(mut pending) = state.startup_recovery.lock() {
                pending.insert(mission_id.to_owned());
            }
        }
    }
    Ok(())
}

/// The latest earlier attempt at a Mission recorded in the shared Registry, which a new
/// attempt replaces.
pub(super) fn predecessor(state: &DaemonState, mission_id: &MissionId) -> Option<SessionRecord> {
    state
        .intake
        .latest_session(mission_id.as_str())
        .unwrap_or_else(|error| {
            eprintln!("ai6: earlier attempts at Mission {mission_id} unreadable: {error}");
            None
        })
}

/// Where a new attempt finds the work of the lost attempt it replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkFrom {
    /// Its working directory starts from the lost attempt's last checkpoint.
    Checkpoint,
    /// It works in the directory the lost attempt worked in, as that attempt left it.
    Place,
    /// Nothing of the lost attempt's work is here.
    Nothing,
}

/// The brief a new attempt is given about the lost attempt it replaces.
pub(super) fn hand_off(lost: &SessionRecord, work: WorkFrom) -> String {
    let mut lines = vec![format!(
        "This Mission continues attempt {}, which was lost: {}.",
        lost.attempt,
        lost.lost
            .as_deref()
            .unwrap_or("its Station went dark")
            .trim_end_matches('.')
    )];
    match (&lost.checkpoint, work) {
        (Some(checkpoint), WorkFrom::Checkpoint) => lines.push(format!(
            "Its work was last saved as commit {} on branch {} of {}; your working directory \
             starts from it. Inspect it before you continue: changes made after that save are \
             gone.",
            short(&checkpoint.commit),
            checkpoint.branch,
            checkpoint.remote
        )),
        (_, WorkFrom::Place) => lines.push(
            "You work in the directory it worked in, as it left it. Inspect it before you \
             continue: it may have stopped in the middle of a change."
                .to_owned(),
        ),
        _ => lines.push(
            "None of its work could be recovered here: start from the brief, and check \
             whether any of it was already published."
                .to_owned(),
        ),
    }
    if !lost.recent.is_empty() {
        lines.push("What it was last seen doing, oldest first:".to_owned());
        lines.extend(lost.recent.iter().map(|line| format!("- {line}")));
    }
    lines.join("\n")
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// How this Station launched a Mission's Session, as another Station needs it to re-attach.
pub(super) fn launch_of(manifest: &LaunchManifest) -> SessionLaunch {
    let mut launch = SessionLaunch::new(
        manifest.handler_provider.clone(),
        manifest.terminal_provider.clone(),
        manifest.working_directory.clone(),
    )
    .with_model(manifest.model.clone(), manifest.effort.clone())
    .with_mode(manifest.mode);
    if let Some(runner) = &manifest.runner {
        launch = launch.with_runner(runner.clone());
    }
    launch
}

/// Writes how the Mission's Session is launched now into its record, after a resume or a
/// re-attach changed it.
pub(super) fn record_launch(state: &DaemonState, mission_id: &str) {
    let Some(manifest) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.operation(mission_id))
    else {
        return;
    };
    let Some(hold) = manifest.session.clone() else {
        return;
    };
    let update = SessionUpdate::default().launch(launch_of(&manifest));
    if let Err(error) = state
        .intake
        .update_session(&hold.id, &state.station_id, hold.fence, update)
    {
        eprintln!(
            "ai6: Session record {:?} launch not recorded: {error}",
            hold.id
        );
    }
}

/// What a Session has recently done, kept in its record so a later attempt can be told if
/// this one is lost. The event pump owns one per Session.
#[derive(Default)]
pub(super) struct Notes {
    lines: VecDeque<String>,
    changed: bool,
    written: Option<Instant>,
}

impl Notes {
    /// Notes what `event` said or did, one line each; a line that repeats the last one is not
    /// noted again.
    pub(super) fn record(&mut self, event: &AgentEvent) {
        let text = event
            .text
            .as_deref()
            .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|text| !text.is_empty());
        let line = match (text, event.tools.is_empty()) {
            (Some(text), _) => format!("{}: {text}", event.kind),
            (None, false) => format!("{}: {}", event.kind, event.tools.join(", ")),
            // A bare kind says something only when it is a state the Session stopped in.
            (None, true) if matches!(event.kind.as_str(), "blocked" | "stalled") => {
                event.kind.clone()
            }
            (None, true) => return,
        };
        if self.lines.back() == Some(&line) {
            return;
        }
        if self.lines.len() == MAX_RECENT_EVENTS {
            self.lines.pop_front();
        }
        self.lines
            .push_back(line.chars().take(MAX_RECENT_EVENT_CHARS).collect());
        self.changed = true;
    }

    /// Writes the notes to the Session record when they changed and the last write is old
    /// enough, so a busy Session costs a Registry write every few seconds at most.
    pub(super) fn write_if_due(&mut self, state: &DaemonState, mission_id: &str) {
        if self.written.is_some_and(|at| at.elapsed() < NOTES_EVERY) {
            return;
        }
        self.write_now(state, mission_id);
    }

    /// Writes the notes to the Session record now if they changed, as before it is given up.
    pub(super) fn write_now(&mut self, state: &DaemonState, mission_id: &str) {
        if !self.changed {
            return;
        }
        let Some(hold) = session_hold(state, mission_id) else {
            return;
        };
        let update = SessionUpdate::default().recent(self.lines.iter().cloned().collect());
        match state
            .intake
            .update_session(&hold.id, &state.station_id, hold.fence, update)
        {
            Ok(_) | Err(SessionError::Ended) => {}
            Err(error) => eprintln!(
                "ai6: Session record {:?} notes not written: {error}",
                hold.id
            ),
        }
        self.changed = false;
        self.written = Some(Instant::now());
    }
}

/// Sessions whose checkpoint is being saved now, by Session record ID.
static CHECKPOINTING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Saves a checkpoint of every running Session's safe house whose Operation pushes them
/// and whose last one is older than the Operation's interval. Each runs on a blocking
/// worker; one Session never has two at once.
pub(super) fn checkpoint_due(state: &DaemonState) {
    let houses = state
        .sessions
        .lock()
        .map(|sessions| {
            sessions
                .iter()
                .filter_map(|(mission_id, operation)| {
                    let house = operation.safe_house.clone()?;
                    house.is_isolated().then(|| (mission_id.clone(), house))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let now = unix_now();
    for (mission_id, house) in houses {
        let Some(operation) = state
            .missions
            .lock()
            .ok()
            .and_then(|missions| missions.get(&mission_id)?.operation_id.clone())
            .and_then(|id| state.roster.operation(&id).ok())
        else {
            continue;
        };
        let Some(remote) = operation.workspace.checkpoint_remote.clone() else {
            continue;
        };
        let Some(hold) = session_hold(state, &mission_id) else {
            continue;
        };
        let due = state
            .intake
            .session(&hold.id)
            .ok()
            .flatten()
            .is_some_and(|record| {
                record
                    .checkpointed_at
                    .is_none_or(|at| now - at >= operation.workspace.checkpoint_secs as f64)
            });
        let Ok(cover) = state.roster.cover(&operation.cover) else {
            continue;
        };
        if !due
            || !CHECKPOINTING
                .lock()
                .is_ok_and(|mut running| running.insert(hold.id.clone()))
        {
            continue;
        }
        let spec = CheckpointSpec {
            remote,
            message: cover.checkpoint_message(),
            author_name: operation.cover_identity.as_ref().map(|i| i.name.clone()),
            author_email: operation.cover_identity.as_ref().map(|i| i.email.clone()),
        };
        let state = state.clone();
        tokio::task::spawn_blocking(move || {
            match house.checkpoint(&spec) {
                Ok(checkpoint) => save_checkpoint(&state, &hold, checkpoint),
                Err(error) => {
                    eprintln!("ai6: Mission {mission_id:?} checkpoint not saved: {error}")
                }
            }
            if let Ok(mut running) = CHECKPOINTING.lock() {
                running.remove(&hold.id);
            }
        });
    }
}

fn save_checkpoint(state: &DaemonState, hold: &SessionHold, checkpoint: SafeHouseCheckpoint) {
    let update = SessionUpdate::default().checkpoint(checkpoint, unix_now());
    if let Err(error) = state
        .intake
        .update_session(&hold.id, &state.station_id, hold.fence, update)
    {
        eprintln!(
            "ai6: Session record {:?} checkpoint not recorded: {error}",
            hold.id
        );
    }
}

fn meta_str<'a>(lease: &'a Lease, key: &str) -> Option<&'a str> {
    lease.meta().get(key).and_then(serde_json::Value::as_str)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};

    use super::super::tests::{
        DRIP_INTERVAL, DRIP_TICKS, RECORDED_STARTS, SAFE_HOUSE_OPENS, start_test_runner,
        test_state, ticks, wait_for_state,
    };
    use super::super::{
        StationConfig, mission_id_for_brief, plugins, pop_event, publish_station_check_in,
        schedule_intake_once, schedule_queued_once, schedule_recovered_once,
    };
    use super::*;
    use crate::directorates::m::mission::MissionStore;
    use crate::directorates::m::theaters::{Theater, TheaterAccess, Theaters};
    use crate::directorates::registry::backend::*;
    use crate::directorates::registry::store::{Brief, FileRegistry};
    use crate::station::runner::client::RunnerLink;
    use crate::station::transports::Transports;

    /// Station `id`, one of several sharing the Registry at `registry`, with an empty Mission
    /// book, Operation `G` (`operation` adds its settings) and the Theater `theater`.
    fn station(
        root: &Path,
        id: &str,
        registry: &Path,
        operation: &str,
        agents: &str,
        theater: Option<Theater>,
    ) -> DaemonState {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        let mut state = test_state(&dir, operation, agents);
        state.station_id = id.to_owned();
        state.intake = Arc::new(FileRegistry::open(registry));
        state.missions = Arc::new(Mutex::new(MissionStore::default()));
        let mut theaters = Theaters::default();
        if let Some(theater) = theater {
            theaters.insert(theater).unwrap();
        }
        state.station = StationConfig::default().with_terminal("mock-terminal");
        state.station.theaters = theaters.clone();
        state.transports = Arc::new(Transports::build(&theaters, &plugins::Plugins::scan(&[])));
        state
    }

    /// Queues a brief for Agent 007 of Operation G and answers its Mission ID.
    fn submit(registry: &dyn RegistryBackend, id: &str) -> String {
        let brief: Brief = serde_json::from_value(serde_json::json!({
            "title": "say every tick",
            "id": id,
            "source": "manual",
            "operation": "G",
            "agent": "007",
        }))
        .unwrap();
        registry.submit_brief(&brief).unwrap();
        mission_id_for_brief(&brief).unwrap().to_string()
    }

    /// The Station dies: it lets go of nothing and tells no one, and its leases run out.
    fn go_dark(state: &DaemonState, mission_id: &str) {
        let crashed = state.sessions.lock().unwrap().remove(mission_id);
        drop(crashed);
        let gone = Duration::from_nanos(1);
        let station = state.station_id.as_str();
        let hold = session_hold(state, mission_id).unwrap();
        state
            .intake
            .renew_session(&hold.id, station, hold.fence, gone)
            .unwrap();
        for lease in state.intake.leases().unwrap() {
            if lease.station() == station {
                state.intake.renew(lease.key(), station, gone).unwrap();
            }
        }
        state.intake.elect(station, gone).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }

    /// The Theater `field`: this host, supervised by the runner serving `runner_dir`.
    fn field_theater(root: &Path, runner_dir: &Path) -> Theater {
        Theater::new("field", TheaterAccess::Local).with_runner(
            crate::RunnerSettings::default()
                .with_state_dir(runner_dir.to_string_lossy().into_owned())
                // The test's own runner serves; never start this test binary as one.
                .with_command(root.join("no-runner-command").to_string_lossy()),
        )
    }

    /// The runner serving `runner_dir` dies as far as any Station can tell: its record points
    /// at a port nothing listens on. Answers the record that brings it back.
    fn kill_runner(runner_dir: &Path) -> Vec<u8> {
        let path = runner_dir.join("runner.json");
        let saved = fs::read(&path).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut info: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        info["url"] = serde_json::json!(format!("http://127.0.0.1:{port}"));
        fs::write(&path, serde_json::to_vec(&info).unwrap()).unwrap();
        saved
    }

    fn mission_state(state: &DaemonState, mission_id: &str) -> Option<MissionState> {
        Some(state.missions.lock().unwrap().get(mission_id)?.state)
    }

    /// The hand-off the latest start of `mission_id` was briefed with.
    fn latest_hand_off(mission_id: &str) -> Option<String> {
        RECORDED_STARTS
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|start| start.mission_id.as_str() == mission_id)?
            .initial_context
            .iter()
            .find(|text| text.starts_with("This Mission continues attempt 1, which was lost"))
            .cloned()
    }

    async fn until(what: &str, mut done: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_runner_session_of_a_station_that_goes_dark_is_adopted_by_another_without_a_second_launch()
     {
        let root = tempfile::tempdir().unwrap();
        let registry = root.path().join("registry");
        let runner_dir = root.path().join("runner");
        let _runner = start_test_runner(&runner_dir).await;
        // Both Stations reach the same runner as Theater `field`.
        let field = field_theater(root.path(), &runner_dir);
        let agents = "[agent.007]\nhandler = \"mock-drip\"\n";
        let operation = "theater = \"field\"\n";
        let a = station(
            root.path(),
            "station-a",
            &registry,
            operation,
            agents,
            Some(field.clone()),
        );
        let b = station(
            root.path(),
            "station-b",
            &registry,
            operation,
            agents,
            Some(field),
        );
        let mission_id = submit(a.intake.as_ref(), &format!("adopt-{}", std::process::id()));

        // Station A is Headquarters, places the brief on itself and starts the harness.
        publish_station_check_in(&a);
        publish_station_check_in(&b);
        assert!(schedule_intake_once(&a));
        schedule_queued_once(&a).await;
        let mut seen = Vec::new();
        until("ticks on station-a", || {
            while let Some(event) = pop_event(&a, &mission_id) {
                seen.push(event);
            }
            ticks(&seen).0.len() >= 5
        })
        .await;
        let first = session_hold(&a, &mission_id).unwrap();
        let record = a.intake.session(&first.id).unwrap().unwrap();
        let key = record
            .launch
            .as_ref()
            .and_then(|launch| launch.runner.clone())
            .expect("the runner Session is in the shared record");
        assert_eq!(record.theater.as_deref(), Some("field"));

        go_dark(&a, &mission_id);
        // What station-a took counts as seen, though the test had not read all of it yet.
        tokio::time::sleep(DRIP_INTERVAL * 2).await;
        while let Some(event) = pop_event(&a, &mission_id) {
            seen.push(event);
        }
        let link = RunnerLink::local(runner_dir.clone(), PathBuf::from("unused"));
        let at_dark = link.status(&key).unwrap().unwrap().last;
        tokio::time::sleep(DRIP_INTERVAL * 10).await;
        assert!(link.status(&key).unwrap().unwrap().last >= at_dark + 5);

        // Station B becomes Headquarters, is handed the brief and adopts the Session.
        publish_station_check_in(&b);
        assert!(schedule_intake_once(&b));
        let lease = b.intake.leases().unwrap().pop().unwrap();
        assert_eq!(lease.station(), "station-b");
        assert_eq!(lease.meta()[ADOPT_KEY], first.id.as_str());
        let adopted = session_hold(&b, &mission_id).expect("station-b holds the Session");
        assert_eq!(
            (adopted.id.as_str(), adopted.attempt),
            (first.id.as_str(), 1)
        );
        assert!(adopted.fence > first.fence, "station-a is fenced out");
        // The queue starts nothing for it: the adopted Mission only re-attaches.
        schedule_queued_once(&b).await;
        schedule_recovered_once(&b).await;
        assert!(
            b.sessions
                .lock()
                .unwrap()
                .get(&mission_id)
                .and_then(|operation| operation.runner.as_ref().map(|r| r.key.clone()))
                == Some(key.clone()),
            "station-b re-attached to the same runner Session"
        );

        let mut replayed = Vec::new();
        until("the rest of the Session on station-b", || {
            while let Some(event) = pop_event(&b, &mission_id) {
                replayed.push(event);
            }
            ticks(&replayed).1
        })
        .await;
        wait_for_state(&b, &mission_id, MissionState::Done).await;
        let (before, _) = ticks(&seen);
        let (after, completed) = ticks(&replayed);
        assert!(completed);
        let last_before = *before.last().unwrap();
        assert!(
            after[0] >= last_before && after[0] <= last_before + 1,
            "the replay starts after the last tick station-a took ({last_before}): {after:?}"
        );
        assert_eq!(after, (after[0]..=DRIP_TICKS).collect::<Vec<_>>());

        // One harness ran from start to end: nothing was launched again.
        let (status, sessions) = link.call("GET", "/v1/sessions", None).unwrap();
        assert_eq!(status, 200);
        assert_eq!(sessions.as_array().map(Vec::len), Some(1), "{sessions}");
        let record = b.intake.session(&first.id).unwrap().unwrap();
        assert_eq!(record.station, "station-b");
        assert_eq!(record.status, SessionStatus::Ended);
        a.stopping.store(true, Ordering::Release);
        b.stopping.store(true, Ordering::Release);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_lost_with_its_station_is_tried_again_elsewhere_from_its_checkpoint() {
        let root = tempfile::tempdir().unwrap();
        let registry = root.path().join("registry");
        let operation = "workspace = { provider = \"mock-safehouse\", checkpoint_remote = \"origin\", checkpoint_secs = 1 }\n";
        // Each Station posts 007 on its own handler, so the attempts get distinct provider IDs.
        let a = station(
            root.path(),
            "station-a",
            &registry,
            operation,
            "[agent.007]\nhandler = \"mock-agent\"\n",
            None,
        );
        let b = station(
            root.path(),
            "station-b",
            &registry,
            operation,
            "[agent.007]\nhandler = \"mock-recorder\"\n",
            None,
        );
        let mission_id = submit(a.intake.as_ref(), &format!("retry-{}", std::process::id()));
        publish_station_check_in(&a);
        publish_station_check_in(&b);
        assert!(schedule_intake_once(&a));
        schedule_queued_once(&a).await;
        wait_for_state(&a, &mission_id, MissionState::Blocked).await;
        let first = session_hold(&a, &mission_id).unwrap();

        // What the Session did and where its work is saved reach the shared record.
        until("station-a's notes", || {
            a.intake
                .session(&first.id)
                .unwrap()
                .is_some_and(|record| record.recent == ["blocked"])
        })
        .await;
        checkpoint_due(&a);
        until("station-a's checkpoint", || {
            a.intake
                .session(&first.id)
                .unwrap()
                .is_some_and(|record| record.checkpoint.is_some())
        })
        .await;
        let saved = a
            .intake
            .session(&first.id)
            .unwrap()
            .unwrap()
            .checkpoint
            .unwrap();
        assert_eq!(saved.remote, "origin");

        // Station A was also the Theater: nothing can re-attach, so the Session is lost and
        // the brief goes to Station B as a second attempt.
        go_dark(&a, &mission_id);
        publish_station_check_in(&b);
        assert!(schedule_intake_once(&b));
        let lost = b.intake.session(&first.id).unwrap().unwrap();
        assert_eq!(lost.status, SessionStatus::Lost);
        let reason = lost.lost.clone().unwrap();
        assert!(
            reason.contains("Station \"station-a\" went dark with Theater \"local\"")
                && reason.contains("own host"),
            "{reason}"
        );
        let lease = b.intake.leases().unwrap().pop().unwrap();
        assert_eq!((lease.station(), lease.attempt()), ("station-b", 2));
        assert!(!lease.meta().contains_key(ADOPT_KEY));

        schedule_queued_once(&b).await;
        wait_for_state(&b, &mission_id, MissionState::Blocked).await;
        let second = session_hold(&b, &mission_id).unwrap();
        let record = b.intake.session(&second.id).unwrap().unwrap();
        assert_eq!(
            (record.attempt, record.predecessor.as_deref()),
            (2, Some(first.id.as_str()))
        );
        assert_eq!(record.station, "station-b");
        let opened = SAFE_HOUSE_OPENS
            .lock()
            .unwrap()
            .iter()
            .filter(|(mission, _)| *mission == mission_id)
            .map(|(_, checkpoint)| checkpoint.clone())
            .collect::<Vec<_>>();
        assert_eq!(opened, [None, Some(saved.clone())]);
        let started = RECORDED_STARTS
            .lock()
            .unwrap()
            .iter()
            .find(|start| start.mission_id.as_str() == mission_id)
            .cloned()
            .expect("station-b started the second attempt");
        let hand_off = started
            .initial_context
            .iter()
            .find(|text| text.starts_with("This Mission continues attempt 1, which was lost"))
            .expect("the second attempt is briefed on the first");
        assert!(hand_off.contains(&reason), "{hand_off}");
        assert!(hand_off.contains(&saved.branch), "{hand_off}");
        assert!(hand_off.contains("starts from it"), "{hand_off}");
        assert!(hand_off.ends_with("- blocked"), "{hand_off}");
        a.stopping.store(true, Ordering::Release);
        b.stopping.store(true, Ordering::Release);
    }

    /// Pops `mission_id`'s events on `state` until it completes, and answers its ticks.
    async fn finish(state: &DaemonState, mission_id: &str) -> Vec<u32> {
        let mut events = Vec::new();
        until("the Session completes", || {
            while let Some(event) = pop_event(state, mission_id) {
                events.push(event);
            }
            ticks(&events).1
        })
        .await;
        wait_for_state(state, mission_id, MissionState::Done).await;
        ticks(&events).0
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_runner_that_dies_under_a_live_station_loses_its_session_and_the_mission_is_tried_again()
     {
        let root = tempfile::tempdir().unwrap();
        let registry = root.path().join("registry");
        let runner_dir = root.path().join("runner");
        let _runner = start_test_runner(&runner_dir).await;
        let a = station(
            root.path(),
            "station-a",
            &registry,
            "theater = \"field\"\nlost_after_secs = 1\n",
            "[agent.007]\nhandler = \"mock-drip\"\n",
            Some(field_theater(root.path(), &runner_dir)),
        );
        let mission_id = submit(
            a.intake.as_ref(),
            &format!("runner-loss-{}", std::process::id()),
        );
        publish_station_check_in(&a);
        assert!(schedule_intake_once(&a));
        schedule_queued_once(&a).await;
        let mut seen = Vec::new();
        until("ticks before the runner dies", || {
            while let Some(event) = pop_event(&a, &mission_id) {
                seen.push(event);
            }
            ticks(&seen).0.len() >= 3
        })
        .await;
        let first = session_hold(&a, &mission_id).unwrap();

        // The runner dies and the Station lives: it waits out the grace, probes, gives up.
        let saved = kill_runner(&runner_dir);
        until("the Mission handed back", || {
            mission_state(&a, &mission_id) == Some(MissionState::Abandoned)
        })
        .await;
        let lost = a.intake.session(&first.id).unwrap().unwrap();
        assert_eq!(lost.status, SessionStatus::Lost);
        let reason = lost.lost.clone().unwrap();
        assert!(reason.contains("out of reach for 1s"), "{reason}");
        assert!(
            lost.recent
                .iter()
                .any(|line| line.starts_with("text: tick")),
            "its last events are kept for the next attempt: {:?}",
            lost.recent
        );
        assert!(!a.sessions.lock().unwrap().contains_key(&mission_id));
        while let Some(event) = pop_event(&a, &mission_id) {
            seen.push(event);
        }
        assert!(
            seen.iter()
                .any(|event| event.kind == "lost" && event.text.as_deref() == Some(&reason)),
            "the operator is told why"
        );
        assert!(a.intake.leases().unwrap().is_empty(), "the brief went back");
        assert_eq!(a.intake.list_new().unwrap().len(), 1);

        // The runner answers again; Headquarters posts the brief anew as a second attempt.
        fs::write(runner_dir.join("runner.json"), saved).unwrap();
        publish_station_check_in(&a);
        assert!(schedule_intake_once(&a));
        let lease = a.intake.leases().unwrap().pop().unwrap();
        assert_eq!((lease.station(), lease.attempt()), ("station-a", 2));
        schedule_queued_once(&a).await;
        let second = session_hold(&a, &mission_id).expect("the second attempt holds a Session");
        let record = a.intake.session(&second.id).unwrap().unwrap();
        assert_eq!(
            (record.attempt, record.predecessor.as_deref()),
            (2, Some(first.id.as_str()))
        );
        let hand_off = latest_hand_off(&mission_id).expect("the second attempt is briefed");
        assert!(hand_off.contains(&reason), "{hand_off}");
        assert!(hand_off.contains("- text: tick"), "{hand_off}");
        assert_eq!(finish(&a, &mission_id).await.len(), DRIP_TICKS as usize);
        a.stopping.store(true, Ordering::Release);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_station_that_adopts_a_session_whose_runner_is_gone_gives_it_up_instead_of_waiting() {
        let root = tempfile::tempdir().unwrap();
        let registry = root.path().join("registry");
        let runner_dir = root.path().join("runner");
        let _runner = start_test_runner(&runner_dir).await;
        let operation = "theater = \"field\"\nlost_after_secs = 1\n";
        let agents = "[agent.007]\nhandler = \"mock-drip\"\n";
        let [a, b] = ["station-a", "station-b"].map(|id| {
            station(
                root.path(),
                id,
                &registry,
                operation,
                agents,
                Some(field_theater(root.path(), &runner_dir)),
            )
        });
        let mission_id = submit(
            a.intake.as_ref(),
            &format!("adopt-loss-{}", std::process::id()),
        );
        publish_station_check_in(&a);
        publish_station_check_in(&b);
        assert!(schedule_intake_once(&a));
        schedule_queued_once(&a).await;
        let mut seen = Vec::new();
        until("ticks on station-a", || {
            while let Some(event) = pop_event(&a, &mission_id) {
                seen.push(event);
            }
            ticks(&seen).0.len() >= 3
        })
        .await;
        let first = session_hold(&a, &mission_id).unwrap();

        // Station A goes dark and its runner dies with it; Station B adopts the Session.
        go_dark(&a, &mission_id);
        let saved = kill_runner(&runner_dir);
        publish_station_check_in(&b);
        assert!(schedule_intake_once(&b));
        assert_eq!(session_hold(&b, &mission_id).unwrap().id, first.id);

        // The re-attach fails; the next try waits the first backoff, not a fixed 5 s loop.
        let tried = tokio::time::Instant::now();
        schedule_recovered_once(&b).await;
        let retry_at = b.retry_after.lock().unwrap().get(&mission_id).copied();
        assert!(
            retry_at.is_some_and(|at| at >= tried + Duration::from_secs(4)),
            "{retry_at:?}"
        );
        assert_eq!(mission_state(&b, &mission_id), Some(MissionState::Blocked));

        // Past the grace, a failed try probes the runner, finds it gone and gives up.
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        b.retry_after.lock().unwrap().clear();
        schedule_recovered_once(&b).await;
        let lost = b.intake.session(&first.id).unwrap().unwrap();
        assert_eq!(lost.status, SessionStatus::Lost);
        let reason = lost.lost.clone().unwrap();
        assert!(
            reason.contains("could not be re-attached for 1s"),
            "{reason}"
        );
        assert_eq!(
            mission_state(&b, &mission_id),
            Some(MissionState::Abandoned)
        );
        assert!(!b.startup_recovery.lock().unwrap().contains(&mission_id));
        assert_eq!(b.intake.list_new().unwrap().len(), 1, "the brief went back");
        while pop_event(&b, &mission_id).is_some() {}

        // With its runner back, the Mission runs again as a second attempt on Station B.
        fs::write(runner_dir.join("runner.json"), saved).unwrap();
        publish_station_check_in(&b);
        assert!(schedule_intake_once(&b));
        let lease = b.intake.leases().unwrap().pop().unwrap();
        assert_eq!((lease.station(), lease.attempt()), ("station-b", 2));
        schedule_queued_once(&b).await;
        let second = session_hold(&b, &mission_id).expect("the second attempt holds a Session");
        let record = b.intake.session(&second.id).unwrap().unwrap();
        assert_eq!(
            (record.attempt, record.predecessor.as_deref()),
            (2, Some(first.id.as_str()))
        );
        assert!(
            latest_hand_off(&mission_id).is_some_and(|hand_off| hand_off.contains(&reason)),
            "the second attempt is briefed on the first"
        );
        assert_eq!(finish(&b, &mission_id).await.len(), DRIP_TICKS as usize);
        a.stopping.store(true, Ordering::Release);
        b.stopping.store(true, Ordering::Release);
    }

    #[test]
    fn an_operation_that_fails_lost_work_files_the_brief() {
        let fail = Extraction::Lost {
            on_lost: OnLost::Fail,
        };
        assert_eq!(fail.max_attempts(), Some(0));
        let retry = Extraction::Lost {
            on_lost: OnLost::Retry,
        };
        assert_eq!(retry.max_attempts(), Some(INTAKE_MAX_ATTEMPTS));
        assert_eq!(Extraction::Wait.max_attempts(), None);
        assert_eq!(Extraction::HandedOver.max_attempts(), None);
    }

    #[test]
    fn notes_keep_the_last_events_once_each() {
        let event = |kind: &str, text: Option<&str>, tools: &[&str]| AgentEvent {
            kind: kind.to_owned(),
            text: text.map(str::to_owned),
            data: None,
            limit: None,
            usage: None,
            tools: tools.iter().map(|tool| (*tool).to_owned()).collect(),
            quota: Vec::new(),
        };
        let mut notes = Notes::default();
        notes.record(&event("text", Some("  editing\n the   parser "), &[]));
        notes.record(&event("text", Some("editing the parser"), &[]));
        notes.record(&event("tool", None, &["Edit", "Bash"]));
        notes.record(&event("blocked", None, &[]));
        notes.record(&event("system", None, &[]));
        assert_eq!(
            notes.lines,
            ["text: editing the parser", "tool: Edit, Bash", "blocked"]
        );
        for index in 0..MAX_RECENT_EVENTS {
            notes.record(&event("text", Some(&format!("line {index}")), &[]));
        }
        assert_eq!(notes.lines.len(), MAX_RECENT_EVENTS);
        assert_eq!(notes.lines[0], "text: line 0");
    }
}
