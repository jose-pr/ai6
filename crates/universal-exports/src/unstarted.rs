//! Missions that have no Run yet: queued, waiting for a place, retrying a start that failed,
//! or failed before a harness started.
//!
//! The board is built from Runs, so a Mission nobody has started would be missing from it.
//! Three records in the shared Registry say where such a Mission stands: a Station's check-in
//! (its own queue, with each entry's wait, tries and next try, and the Missions that failed
//! before starting), the briefs no Station has taken (with the reason placement last noted on
//! the timeline), and nothing else. Only what the person may read in an Operation is listed.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{Authorizer as _, Person};
use serde::Serialize;

use crate::compartments::CompartmentAccess;
use crate::tree::{OperationTree, operation_request};
use crate::{ApiError, BoardMission, BoardQueueEntry, Gateway};

/// Where a Mission with no Run stands, as the Registry last said.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardPending {
    /// `queued` (waiting for a Station to start it) or `failed` (it failed before a harness
    /// started).
    pub state: String,
    /// `unplaced` (no Station has taken its brief), `waiting` (held back, with the reason),
    /// `retrying` (a start failed and will be tried again), `recovering` (waiting to be resumed
    /// after a restart), `queued`, or `failed`.
    pub kind: String,
    /// Why it waits, why its last start failed, or why it failed.
    pub reason: Option<String>,
    /// Start tries that failed in a row.
    pub failures: u32,
    /// When the next try is due, as Unix seconds, while it waits out a backoff.
    pub retry_at: Option<f64>,
    /// The Agent assigned, when there is one.
    pub agent: Option<String>,
    /// The Station that holds it; absent for a brief no Station has taken.
    pub station: Option<String>,
    /// What to show beside its handle: the brief's name.
    pub name: Option<String>,
    /// When the Registry last said so, as Unix seconds.
    pub at: Option<f64>,
}

/// What a check-in lists of a Mission that failed before it started.
#[derive(Debug, Clone, serde::Deserialize)]
struct FailedStart {
    mission_id: String,
    operation_id: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

fn pending_mission(
    operation: &str,
    mission: &str,
    pending: BoardPending,
) -> ((String, String), BoardMission) {
    (
        (operation.to_owned(), mission.to_owned()),
        BoardMission {
            operation_id: operation.to_owned(),
            mission_id: mission.to_owned(),
            handle: mission.to_owned(),
            runs: Vec::new(),
            brief: None,
            usage: None,
            history: Vec::new(),
            pending: Some(pending),
        },
    )
}

/// The Missions with no Run that `person` may read, by Operation and Mission UID, as the
/// Registry has them now.
pub(crate) async fn unstarted_missions(
    gateway: &Gateway,
    person: Person,
) -> Result<Vec<BoardMission>, ApiError> {
    let Some(registry) = gateway.registry.clone() else {
        return Ok(Vec::new());
    };
    let authorizer = gateway.authorizer()?;
    tokio::task::spawn_blocking(move || read(&registry, authorizer.as_ref(), &person))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())
}

fn read(
    registry: &Arc<dyn RegistryBackend>,
    authorizer: &CompartmentAccess,
    person: &Person,
) -> std::io::Result<Vec<BoardMission>> {
    let stations = registry.stations(Duration::MAX)?;
    let tree = OperationTree::from_check_ins(&stations);
    let mut readable = HashMap::<String, bool>::new();
    let mut may_read = |operation: &str| {
        *readable.entry(operation.to_owned()).or_insert_with(|| {
            authorizer
                .authorize(operation_request(
                    person.clone(),
                    "mission.read",
                    operation,
                    &tree,
                ))
                .is_ok_and(|decision| decision.allowed)
        })
    };
    let mut found = BTreeMap::<(String, String), BoardMission>::new();

    // A brief nobody has taken: the reason is the latest one placement noted for it.
    let queued: Vec<_> = registry
        .list_new()?
        .into_iter()
        .filter_map(|brief| Some((brief.operation.clone()?, brief)))
        .filter(|(operation, _)| may_read(operation))
        .collect();
    let mut noted = HashMap::<String, HashMap<String, (f64, String)>>::new();
    for (operation, brief) in queued {
        if !noted.contains_key(&operation) {
            let mut reasons = HashMap::new();
            for event in registry.timeline_events(&operation)? {
                if event.action == "waiting"
                    && let (Some(key), Some(reason)) =
                        (event.detail.get("brief"), event.detail.get("reason"))
                {
                    reasons.insert(key.clone(), (event.at, reason.clone()));
                }
            }
            noted.insert(operation.clone(), reasons);
        }
        let key = brief.key();
        let said = noted.get(&operation).and_then(|reasons| reasons.get(&key));
        let pending = BoardPending {
            state: "queued".to_owned(),
            kind: if said.is_some() {
                "waiting"
            } else {
                "unplaced"
            }
            .to_owned(),
            reason: said.map(|(_, reason)| reason.clone()),
            failures: 0,
            retry_at: None,
            agent: brief.agent.clone(),
            station: None,
            name: Some(brief.display_name(256)),
            at: said.map(|(at, _)| *at),
        };
        let (index, mut mission) = pending_mission(&operation, &key, pending);
        mission.handle = brief.shown_handle();
        found.insert(index, mission);
    }

    for station in &stations {
        let seen = station.seen();
        let info = station.info();
        let failed = info
            .get("failed_starts")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| serde_json::from_value::<FailedStart>(entry.clone()).ok());
        for entry in failed.filter(|entry| may_read(&entry.operation_id)) {
            let pending = BoardPending {
                state: "failed".to_owned(),
                kind: "failed".to_owned(),
                reason: entry.reason,
                failures: 0,
                retry_at: None,
                agent: entry.agent,
                station: Some(station.station().to_owned()),
                name: name_of(registry, &entry.mission_id),
                at: Some(seen),
            };
            let (index, mission) = pending_mission(&entry.operation_id, &entry.mission_id, pending);
            found.entry(index).or_insert(mission);
        }
        let queue = info
            .get("queue")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| serde_json::from_value::<BoardQueueEntry>(entry.clone()).ok());
        for entry in queue.filter(|entry| may_read(&entry.operation_id)) {
            let kind = if entry.state == "recovering" {
                "recovering"
            } else if entry.failures > 0 || entry.retry_in_secs.is_some() {
                "retrying"
            } else if entry.waiting.is_some() {
                "waiting"
            } else {
                "queued"
            };
            let pending = BoardPending {
                state: "queued".to_owned(),
                kind: kind.to_owned(),
                reason: entry.waiting,
                failures: entry.failures,
                retry_at: entry.retry_in_secs.map(|secs| seen + secs),
                agent: entry.agent,
                station: Some(station.station().to_owned()),
                name: name_of(registry, &entry.mission_id),
                at: Some(seen),
            };
            let (index, mission) = pending_mission(&entry.operation_id, &entry.mission_id, pending);
            // The Station that holds it knows more than the brief that is still on the queue.
            found.insert(index, mission);
        }
    }
    Ok(found.into_values().collect())
}

/// What the brief filed under `key` is called, for a line beside the Mission's handle.
fn name_of(registry: &Arc<dyn RegistryBackend>, key: &str) -> Option<String> {
    registry
        .filed_brief(key)
        .ok()
        .flatten()
        .map(|filed| filed.brief.display_name(256))
}
