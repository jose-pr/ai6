//! `tidy_safe_house`: what a Head of Station may do to the safe houses its Station keeps.
//!
//! The limits are fixed here, not in the prompt:
//!
//! - unattended: remove the cache directories the Station's hygiene names from the safe
//!   house of a Mission that is over and has been left alone for the cache age; remove the
//!   safe house of a Mission that is over when its provider shows everything in it exists
//!   elsewhere;
//! - green ink (kind `delete`): remove a safe house that holds work existing nowhere else, or
//!   whose provider cannot tell; the request is the round's, and only a live, unspent approval
//!   of it removes the safe house;
//! - refused: a Mission this Station does not keep, one that is not over, and every other
//!   action.
//!
//! Every removal is audited.

use std::path::Path;
use std::time::SystemTime;

use serde::Serialize;

use super::safe_houses::{is_over, kept_report, reopen};
use super::survey::survey;
use crate::directorates::m::approvals::ActionScope;
use crate::directorates::m::mission::Mission;
use crate::directorates::q::safehouse::SafeHouseReport;
use crate::directorates::registry::store::AuditRecord;

use super::super::DaemonState;
use super::super::green_ink::{Asked, Desk as GreenInk};

/// What a round asks to do to a safe house.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tidy {
    /// Remove its cache directories.
    Caches,
    /// Remove the safe house.
    Remove,
}

impl std::str::FromStr for Tidy {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "caches" => Ok(Self::Caches),
            "remove" => Ok(Self::Remove),
            other => Err(format!("{other:?} is not an action: use caches or remove")),
        }
    }
}

/// Who asks: the round's Mission, its provider Session and whether its exposure keeps its
/// requests from class authorisations.
pub(super) struct Round<'a> {
    pub(super) mission: &'a Mission,
    pub(super) session: &'a str,
    pub(super) exposed: bool,
}

/// What a tidy did.
#[derive(Debug, Clone, Serialize)]
pub(super) struct Tidied {
    mission: String,
    /// `caches-removed`, `removed`, or the green ink status (`pending`, `rejected`,
    /// `expired`) while the safe house stays.
    status: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    removed: Vec<String>,
    freed_bytes: u64,
    /// The green ink answer, when the removal waits for a person.
    #[serde(skip_serializing_if = "Option::is_none")]
    green_ink: Option<serde_json::Value>,
}

/// Applies `action` to the kept safe house of `target`, a Mission of this Station.
///
/// # Errors
///
/// Why the Station refuses: no such Mission here, a Mission not over, no safe house kept on
/// disk, caches not old enough, or a provider or Registry failure.
pub(super) fn tidy(
    state: &DaemonState,
    round: &Round<'_>,
    target: &str,
    action: Tidy,
    reason: Option<String>,
) -> Result<Tidied, String> {
    let mission = state
        .missions
        .lock()
        .map_err(|_| "the Mission book is unavailable".to_owned())?
        .get(target)
        .cloned()
        .ok_or("this Station keeps no such Mission")?;
    if !is_over(state, &mission) {
        return Err(
            "the Mission is not over, so its safe house is in use and is left alone".to_owned(),
        );
    }
    let report = kept_report(&mission)
        .cloned()
        .ok_or("the Mission keeps no safe house on this Station")?;
    // The provider confirms the safe house is the one the record names before anything is
    // removed from it.
    let house = reopen(state, &mission)?;
    match action {
        Tidy::Caches => remove_caches(state, round, &mission, &report),
        Tidy::Remove => {
            let exports = house.exports().map_err(|error| error.to_string())?;
            if exports
                .as_ref()
                .is_some_and(|exports| exports.is_exported())
            {
                return discard(state, round, &mission, (&house, &report), None);
            }
            remove_with_green_ink(state, round, &mission, &house, &report, exports, reason)
        }
    }
}

fn remove_caches(
    state: &DaemonState,
    round: &Round<'_>,
    mission: &Mission,
    report: &SafeHouseReport,
) -> Result<Tidied, String> {
    let hygiene = &state.station.hygiene;
    let found = survey(Path::new(&report.path), hygiene, SystemTime::now());
    let age = hygiene.cache_age().as_secs();
    let left_alone = found.age_secs.unwrap_or_default();
    if left_alone < age {
        return Err(format!(
            "its caches may go once it has been left alone for {age} seconds; it has been {left_alone}"
        ));
    }
    let mut removed = Vec::new();
    let mut freed = 0;
    for cache in &found.caches {
        // A cache is a directory the walk found inside the safe house without following a link.
        match std::fs::remove_dir_all(&cache.absolute) {
            Ok(()) => {
                removed.push(cache.path.clone());
                freed += cache.size_bytes;
            }
            Err(error) => eprintln!(
                "ai6: cache {} of Mission {} was not removed: {error}",
                cache.path, mission.mission_id
            ),
        }
    }
    if !removed.is_empty() {
        audit(
            state,
            round,
            mission,
            "head_of_station.caches_removed",
            &format!("{} {}", mission.mission_id, removed.join(" ")),
        );
    }
    Ok(Tidied {
        mission: mission.mission_id.to_string(),
        status: "caches-removed".to_owned(),
        removed,
        freed_bytes: freed,
        green_ink: None,
    })
}

/// The green ink step that removing this safe house is.
fn scope(state: &DaemonState, mission: &Mission) -> Result<ActionScope, String> {
    ActionScope::try_new(
        "delete",
        format!(
            "safe house {} on Station {}",
            mission.mission_id, state.station_id
        ),
    )
    .map_err(|error| error.to_string())
}

fn remove_with_green_ink(
    state: &DaemonState,
    round: &Round<'_>,
    mission: &Mission,
    house: &crate::station::safehouse::SafeHouse,
    report: &SafeHouseReport,
    exports: Option<crate::directorates::q::safehouse::SafeHouseExports>,
    reason: Option<String>,
) -> Result<Tidied, String> {
    let operation = round
        .mission
        .operation_id
        .clone()
        .ok_or("the round has no Operation, so it cannot ask for green ink")?;
    let agent = round
        .mission
        .agent_id
        .clone()
        .ok_or("the round has no Agent")?;
    let scope = scope(state, mission)?;
    let now = state.clock.now() as f64;
    let live = state
        .intake
        .live_approval(&operation, round.mission.mission_id.as_str(), &scope, now)
        .map_err(|error| format!("green ink could not be checked: {error}"))?
        .filter(|approval| approval.spent_at.is_none());
    if let Some(approval) = live {
        return discard(
            state,
            round,
            mission,
            (house, report),
            Some((&operation, &approval.request.id)),
        );
    }
    let holds = match &exports {
        Some(exports) => format!(
            "{} change(s) no remote holds{}",
            exports.unexported.len(),
            if exports.dirty {
                " and uncommitted changes"
            } else {
                ""
            }
        ),
        None => "work its provider cannot show exists elsewhere".to_owned(),
    };
    let reason = format!(
        "Remove the safe house of Mission {} ({:?}), which holds {holds}. {}",
        mission.mission_id,
        mission.state,
        reason.unwrap_or_default()
    );
    let asked = Asked {
        operation: operation.clone(),
        mission: round.mission.mission_id.to_string(),
        session: round.session.to_owned(),
        agent,
        kind: scope.kind.clone(),
        target: scope.target.clone(),
        reason: reason.trim().chars().take(2_000).collect(),
        idempotency_key: scope.target.clone(),
        exposed: round.exposed,
    };
    let answer = GreenInk::new(state.clone()).request_always(asked)?;
    // A class authorisation may have approved it at once.
    if answer.status == "approved"
        && let Ok(Some(approval)) =
            state
                .intake
                .live_approval(&operation, round.mission.mission_id.as_str(), &scope, now)
        && approval.spent_at.is_none()
    {
        return discard(
            state,
            round,
            mission,
            (house, report),
            Some((&operation, &approval.request.id)),
        );
    }
    Ok(Tidied {
        mission: mission.mission_id.to_string(),
        status: answer.status.to_owned(),
        removed: Vec::new(),
        freed_bytes: 0,
        green_ink: serde_json::to_value(&answer).ok(),
    })
}

/// Removes the safe house through its provider and records that it is gone. `approval` is
/// the Operation and ID of the green ink request that allowed it, spent here; `None` when
/// everything in it exists elsewhere.
fn discard(
    state: &DaemonState,
    round: &Round<'_>,
    mission: &Mission,
    (house, report): (&crate::station::safehouse::SafeHouse, &SafeHouseReport),
    approval: Option<(&str, &str)>,
) -> Result<Tidied, String> {
    let size = survey(
        Path::new(&report.path),
        &state.station.hygiene,
        SystemTime::now(),
    )
    .size_bytes;
    if let Some((operation, id)) = approval {
        let spent = state
            .intake
            .spend_approval(operation, id, state.clock.now() as f64)
            .map_err(|error| format!("the approval could not be spent: {error}"))?;
        if !spent {
            return Err("the approval was already used".to_owned());
        }
    }
    house.discard().map_err(|error| error.to_string())?;
    if let Ok(mut missions) = state.missions.lock() {
        let gone = SafeHouseReport {
            kept: false,
            ..report.clone()
        };
        if let Err(error) = missions.set_safe_house(mission.mission_id.as_str(), gone) {
            eprintln!(
                "ai6: Mission {} still records its removed safe house: {error:?}",
                mission.mission_id
            );
        }
    }
    let target = match approval {
        Some((_, id)) => format!("{} approved by {id}", mission.mission_id),
        None => format!("{} exported", mission.mission_id),
    };
    audit(state, round, mission, "head_of_station.removed", &target);
    Ok(Tidied {
        mission: mission.mission_id.to_string(),
        status: "removed".to_owned(),
        removed: vec![report.path.clone()],
        freed_bytes: size,
        green_ink: None,
    })
}

/// `text` cut to at most `max` bytes on a character boundary.
fn bounded(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn audit(state: &DaemonState, round: &Round<'_>, mission: &Mission, action: &str, target: &str) {
    let record = AuditRecord {
        id: format!("{action}-{}", uuid::Uuid::new_v4().simple()),
        actor: format!(
            "agent:{}",
            round.mission.agent_id.as_deref().unwrap_or("unknown")
        ),
        action: action.to_owned(),
        operation: mission.operation_id.clone().unwrap_or_default(),
        resource: Some(format!("station:{}", state.station_id)),
        target: bounded(target, 500),
        at: state.clock.now() as f64,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: Head of Station audit record {action} was not written: {error}");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use super::super::tests::{call, kept_safe_house, lease_round, start, station};
    use super::*;
    use crate::directorates::m::mission::MissionState;
    use crate::directorates::m::roster::Roster;
    use crate::directorates::registry::approvals::{ApprovalStatus, Ruling};
    use crate::directorates::registry::logbook::HEAD_OF_STATION_ROLE;
    use crate::station::safehouse::SafeHouseHygiene;

    /// The Station of `super::super::tests::station`, whose housekeeping Operation `H` would let
    /// a `field` Agent delete without green ink, keeping caches `left_alone` long.
    fn lenient_station(root: &Path, left_alone: Duration) -> DaemonState {
        let mut state = station(root);
        let checkout = root.join("checkout");
        std::fs::write(
            root.join("operations.toml"),
            format!(
                "[operation.G]\npath = {checkout:?}\nworkspace = {{ provider = \"mock-safehouse\" }}\n\
                 [operation.H]\npath = {checkout:?}\n\
                 [operation.H.green_ink]\nunattended = {{ field = [\"delete\"] }}\n",
                checkout = checkout.to_string_lossy()
            ),
        )
        .unwrap();
        state.roster = Arc::new(Roster::load(root).unwrap());
        state.station = state
            .station
            .clone()
            .with_safe_house_hygiene(SafeHouseHygiene::default().with_cache_age(left_alone));
        state
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap_or_else(|_| panic!("not JSON: {text}"))
    }

    fn audited(state: &DaemonState, action: &str) -> Vec<AuditRecord> {
        state
            .intake
            .audit_records()
            .unwrap()
            .into_iter()
            .filter(|record| record.action == action)
            .collect()
    }

    #[test]
    fn caches_go_only_from_a_safe_house_left_alone_long_enough_by_a_mission_that_is_over() {
        let root = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let round_id = format!("hos-caches-round-{pid}");
        let done_id = format!("hos-caches-done-{pid}");
        let live_id = format!("hos-caches-live-{pid}");

        // Left alone for less than the cache age: the caches stay.
        let state = lenient_station(root.path(), Duration::from_secs(7 * 24 * 3600));
        lease_round(&state, "H", &round_id, HEAD_OF_STATION_ROLE, &["test"]);
        let done = kept_safe_house(&state, root.path(), &done_id, MissionState::Done, &[]);
        let live = kept_safe_house(&state, root.path(), &live_id, MissionState::Working, &[]);
        let round_mission = state.missions.lock().unwrap().get(&round_id).cloned();
        let round_mission = round_mission.unwrap();
        let round = Round {
            mission: &round_mission,
            session: "provider-session-hos",
            exposed: false,
        };
        let young = tidy(&state, &round, &done_id, Tidy::Caches, None).unwrap_err();
        assert!(young.contains("left alone"), "{young}");
        assert!(done.join("target").exists());

        // Old enough: only the named caches go, and the removal is audited.
        let mut state = state;
        state.station = state
            .station
            .clone()
            .with_safe_house_hygiene(SafeHouseHygiene::default().with_cache_age(Duration::ZERO));
        let tidied = tidy(&state, &round, &done_id, Tidy::Caches, None).unwrap();
        assert_eq!(tidied.status, "caches-removed");
        assert_eq!(tidied.removed, ["target"]);
        assert_eq!(tidied.freed_bytes, 64);
        assert!(!done.join("target").exists());
        assert!(done.join("main.rs").exists(), "work is never a cache");
        let records = audited(&state, "head_of_station.caches_removed");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].actor, "agent:M");
        assert_eq!(records[0].resource.as_deref(), Some("station:test"));
        assert_eq!(records[0].target, format!("{done_id} target"));

        // A Mission that is not over, one this Station does not keep, the round itself and an
        // unknown action are refused, and nothing is touched.
        let refused = tidy(&state, &round, &live_id, Tidy::Caches, None).unwrap_err();
        assert!(refused.contains("not over"), "{refused}");
        assert!(live.join("target").exists());
        let unknown = tidy(&state, &round, "elsewhere-1", Tidy::Remove, None).unwrap_err();
        assert!(unknown.contains("no such Mission"), "{unknown}");
        assert!(tidy(&state, &round, &round_id, Tidy::Remove, None).is_err());
        assert!("chmod".parse::<Tidy>().is_err());
        assert!(live.join("main.rs").exists());
    }

    #[tokio::test]
    async fn a_round_removes_exported_work_unattended_and_asks_green_ink_for_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let state = lenient_station(root.path(), Duration::ZERO);
        let exported_id = format!("hos-exported-{pid}");
        let unexported_id = format!("hos-unexported-{pid}");
        let live_id = format!("hos-live-{pid}");
        let exported = kept_safe_house(&state, root.path(), &exported_id, MissionState::Done, &[]);
        let unexported = kept_safe_house(
            &state,
            root.path(),
            &unexported_id,
            MissionState::Failed,
            &["abc1234 add the parser"],
        );
        let live = kept_safe_house(&state, root.path(), &live_id, MissionState::Working, &[]);
        let round = format!("hos-tidy-round-{pid}");
        lease_round(&state, "H", &round, HEAD_OF_STATION_ROLE, &["test"]);
        start(&state, &round).await;

        // Everything in it exists elsewhere: removed at once, recorded and audited.
        let removed = json(
            &call(
                &state,
                &round,
                "tidy_safe_house",
                serde_json::json!({ "mission": exported_id, "action": "remove" }),
            )
            .await
            .unwrap(),
        );
        assert_eq!(removed["status"], "removed", "{removed}");
        assert!(!exported.exists());
        let record = state.missions.lock().unwrap().get(&exported_id).cloned();
        assert!(!record.unwrap().safe_house.unwrap().kept);
        assert_eq!(
            audited(&state, "head_of_station.removed")[0].target,
            format!("{exported_id} exported")
        );

        // Unexported work: a pending green ink request, although the Operation's policy would
        // let the Agent delete unattended, and the safe house survives.
        let asked = json(
            &call(
                &state,
                &round,
                "tidy_safe_house",
                serde_json::json!({
                    "mission": unexported_id, "action": "remove", "reason": "disk is low"
                }),
            )
            .await
            .unwrap(),
        );
        assert_eq!(asked["status"], "pending", "{asked}");
        let request_id = asked["green_ink"]["request_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(unexported.join("main.rs").exists());
        let now = state.clock.now() as f64;
        let pending = state.intake.pending_approvals("H", now).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].scope.kind, "delete");
        assert_eq!(
            pending[0].scope.target,
            format!("safe house {unexported_id} on Station test")
        );
        assert!(pending[0].reason.contains("1 change(s) no remote holds"));
        assert!(pending[0].reason.contains("disk is low"));
        assert_eq!(pending[0].mission.as_str(), round);
        let again = json(
            &call(
                &state,
                &round,
                "tidy_safe_house",
                serde_json::json!({ "mission": unexported_id, "action": "remove" }),
            )
            .await
            .unwrap(),
        );
        assert_eq!(again["status"], "pending");
        assert!(unexported.exists(), "asking again removes nothing");

        // A person approves: the next call removes it and spends the approval.
        state
            .intake
            .decide_approval(
                "H",
                &request_id,
                &Ruling::approve("operator-m", now, now + 600.0),
            )
            .unwrap();
        let approved = json(
            &call(
                &state,
                &round,
                "tidy_safe_house",
                serde_json::json!({ "mission": unexported_id, "action": "remove" }),
            )
            .await
            .unwrap(),
        );
        assert_eq!(approved["status"], "removed", "{approved}");
        assert!(!unexported.exists());
        let Some(ApprovalStatus::Decided(approval)) =
            state.intake.approval("H", &request_id, now).unwrap()
        else {
            panic!("the request is decided");
        };
        assert!(approval.spent_at.is_some());
        let targets = audited(&state, "head_of_station.removed")
            .into_iter()
            .map(|record| record.target)
            .collect::<Vec<_>>();
        assert!(
            targets.contains(&format!("{unexported_id} approved by {request_id}")),
            "{targets:?}"
        );

        // A live Mission's safe house is refused, whatever the action.
        for action in ["remove", "caches"] {
            let refused = call(
                &state,
                &round,
                "tidy_safe_house",
                serde_json::json!({ "mission": live_id, "action": action }),
            )
            .await
            .unwrap();
            assert!(refused.contains("not over"), "{refused}");
        }
        assert!(live.join("target").exists() && live.join("main.rs").exists());
        state.stopping.store(true, Ordering::Release);
    }
}
