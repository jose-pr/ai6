//! `tidy_safe_house`: what a Head of Station may do to the safe houses its Station keeps.
//!
//! The limits are fixed here, not in the prompt:
//!
//! - unattended: remove the cache directories the Station's hygiene names from the safe
//!   house of a Mission that is over and has been left alone for the cache age (in a safe
//!   house with history, only a directory version control ignores whole, so nothing tracked
//!   goes); remove the safe house of a Mission that is over when its provider shows
//!   everything in it exists elsewhere, ignored files outside those caches included;
//! - green ink (kind [`SAFE_HOUSE_DELETE`]): remove a safe house that holds work existing
//!   nowhere else, or whose provider cannot tell. The request is filed under the Operation the
//!   safe house's Mission belongs to, on that Mission, so its policy and approvers decide and
//!   any later round finds the decision; no class authorisation ever approves it, and only a
//!   live, unspent approval removes the safe house;
//! - refused: a Mission this Station does not keep, one that is not over or is being started,
//!   and every other action.
//!
//! A tidy holds the Mission's start reservation from its first check to its last change, so a
//! retry or resume cannot reopen the safe house meanwhile, and checks again that the Mission is
//! over right before it removes anything. Every removal is audited.

use std::path::Path;
use std::time::SystemTime;

use serde::Serialize;

use super::safe_houses::{is_over, kept_report, rebuildable, reopen};
use super::survey::survey;
use crate::directorates::m::approvals::{ActionScope, SAFE_HOUSE_DELETE};
use crate::directorates::m::mission::Mission;
use crate::directorates::q::safehouse::{SafeHouseExports, SafeHouseReport};
use crate::directorates::registry::approvals::{ApprovalStatus, Decision};
use crate::directorates::registry::store::AuditRecord;
use crate::station::safehouse::SafeHouse;

use super::super::green_ink::{Asked, Desk as GreenInk};
use super::super::{DaemonState, StartReservation};

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

/// Who asks: the round's Mission and its provider Session.
pub(super) struct Round<'a> {
    pub(super) mission: &'a Mission,
    pub(super) session: &'a str,
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
    /// Cache directories left in place, with why.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    kept: Vec<String>,
    freed_bytes: u64,
    /// The green ink request, when the removal waits for a person.
    #[serde(skip_serializing_if = "Option::is_none")]
    green_ink: Option<serde_json::Value>,
}

impl Tidied {
    fn new(mission: &Mission, status: impl Into<String>) -> Self {
        Self {
            mission: mission.mission_id.to_string(),
            status: status.into(),
            removed: Vec::new(),
            kept: Vec::new(),
            freed_bytes: 0,
            green_ink: None,
        }
    }
}

/// Takes the start reservation of the Mission `id`: while it is held, neither a start, a retry
/// nor a resume of the Mission proceeds.
fn reserve(state: &DaemonState, id: &str) -> Result<StartReservation, String> {
    let mut starting = state
        .starting
        .lock()
        .map_err(|_| "the start reservations are unavailable".to_owned())?;
    if !starting.insert(id.to_owned()) {
        return Err(
            "the Mission is being started or resumed, so its safe house is left alone".to_owned(),
        );
    }
    Ok(StartReservation {
        starting: state.starting.clone(),
        mission_id: id.to_owned(),
    })
}

/// The Mission `id` as the book holds it now, while it is over and keeps a safe house.
fn over_and_kept(state: &DaemonState, id: &str) -> Result<(Mission, SafeHouseReport), String> {
    let mission = state
        .missions
        .lock()
        .map_err(|_| "the Mission book is unavailable".to_owned())?
        .get(id)
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
    Ok((mission, report))
}

/// Applies `action` to the kept safe house of `target`, a Mission of this Station.
///
/// # Errors
///
/// Why the Station refuses: no such Mission here, a Mission not over or being started, no
/// safe house kept on disk, caches not old enough, or a provider or Registry failure.
pub(super) fn tidy(
    state: &DaemonState,
    round: &Round<'_>,
    target: &str,
    action: Tidy,
    reason: Option<String>,
) -> Result<Tidied, String> {
    let _reservation = reserve(state, target)?;
    let (mission, _) = over_and_kept(state, target)?;
    // The provider confirms the safe house is the one the record names before anything is
    // removed from it.
    let house = reopen(state, &mission)?;
    // A provider that fails to tell reads as one that cannot: the removal waits for a person.
    let exports = house.exports().unwrap_or_else(|error| {
        eprintln!(
            "ai6: what the safe house of Mission {} holds could not be told: {error}",
            mission.mission_id
        );
        None
    });
    match action {
        Tidy::Caches => remove_caches(state, round, target, exports.as_ref()),
        Tidy::Remove => {
            let hygiene = &state.station.hygiene;
            if exports
                .as_ref()
                .is_some_and(|exports| exports.is_exported(|path| rebuildable(hygiene, path)))
            {
                return discard(state, round, target, &house, None);
            }
            remove_with_green_ink(state, round, (&mission, &house), exports, reason)
        }
    }
}

fn remove_caches(
    state: &DaemonState,
    round: &Round<'_>,
    target: &str,
    exports: Option<&SafeHouseExports>,
) -> Result<Tidied, String> {
    let (mission, report) = over_and_kept(state, target)?;
    let hygiene = &state.station.hygiene;
    let found = survey(Path::new(&report.path), hygiene, SystemTime::now());
    let age = hygiene.cache_age().as_secs();
    let left_alone = found.age_secs.unwrap_or_default();
    if left_alone < age {
        return Err(format!(
            "its caches may go once it has been left alone for {age} seconds; it has been {left_alone}"
        ));
    }
    let mut tidied = Tidied::new(&mission, "caches-removed");
    for cache in &found.caches {
        // With history, only a directory ignored whole holds nothing tracked or unrecorded.
        if let Some(exports) = exports
            && !exports.ignored.iter().any(|ignored| ignored == &cache.path)
        {
            tidied.kept.push(format!(
                "{}: it holds files version control tracks or does not ignore",
                cache.path
            ));
            continue;
        }
        // A cache is a directory the walk found inside the safe house without following a link.
        match std::fs::remove_dir_all(&cache.absolute) {
            Ok(()) => {
                tidied.removed.push(cache.path.clone());
                tidied.freed_bytes += cache.size_bytes;
            }
            Err(error) => tidied.kept.push(format!("{}: {error}", cache.path)),
        }
    }
    if !tidied.removed.is_empty() {
        audit(
            state,
            round,
            &mission,
            "head_of_station.caches_removed",
            &format!("{} {}", mission.mission_id, tidied.removed.join(" ")),
        );
    }
    Ok(tidied)
}

/// The green ink step that removing this safe house is.
fn scope(state: &DaemonState, mission: &Mission) -> Result<ActionScope, String> {
    ActionScope::try_new(
        SAFE_HOUSE_DELETE,
        format!(
            "safe house {} on Station {}",
            mission.mission_id, state.station_id
        ),
    )
    .map_err(|error| error.to_string())
}

/// What the Operation owning the safe house has decided about removing it: a live, unspent
/// approval, or the request still waiting or refused. Expired requests are asked again.
fn decided(
    state: &DaemonState,
    operation: &str,
    mission: &Mission,
    scope: &ActionScope,
) -> Result<Option<ApprovalStatus>, String> {
    let now = state.clock.now() as f64;
    let statuses = state
        .intake
        .mission_approvals(operation, mission.mission_id.as_str(), now)
        .map_err(|error| format!("green ink could not be checked: {error}"))?;
    let mut found = None;
    for status in statuses
        .into_iter()
        .filter(|status| status.request().scope.covers(scope))
    {
        match &status {
            ApprovalStatus::Decided(approval)
                if approval.is_live(now) && approval.spent_at.is_none() =>
            {
                return Ok(Some(status));
            }
            ApprovalStatus::Pending(_) => found = Some(status),
            ApprovalStatus::Decided(approval) if approval.decision == Decision::Rejected => {
                found = Some(status);
            }
            _ => {}
        }
    }
    Ok(found)
}

fn remove_with_green_ink(
    state: &DaemonState,
    round: &Round<'_>,
    (mission, house): (&Mission, &SafeHouse),
    exports: Option<SafeHouseExports>,
    reason: Option<String>,
) -> Result<Tidied, String> {
    let operation = mission
        .operation_id
        .clone()
        .ok_or("the Mission has no Operation to ask for green ink in")?;
    let agent = round
        .mission
        .agent_id
        .clone()
        .ok_or("the round has no Agent")?;
    let scope = scope(state, mission)?;
    let status = match decided(state, &operation, mission, &scope)? {
        Some(ApprovalStatus::Decided(approval)) if approval.decision == Decision::Approved => {
            return discard(
                state,
                round,
                mission.mission_id.as_str(),
                house,
                Some((&operation, &approval.request.id)),
            );
        }
        Some(status) => status,
        None => {
            let holds = match &exports {
                Some(exports) => format!(
                    "{} change(s) no remote holds{}{}",
                    exports.unexported.len(),
                    if exports.dirty {
                        ", uncommitted changes"
                    } else {
                        ""
                    },
                    if exports.ignored.is_empty() {
                        String::new()
                    } else {
                        format!(", ignored files ({})", exports.ignored.join(", "))
                    }
                ),
                None => "work its provider cannot show exists elsewhere".to_owned(),
            };
            let reason = format!(
                "Remove the safe house of Mission {} ({:?}), which holds {holds}. Asked by the Head \
                 of Station round {}. {}",
                mission.mission_id,
                mission.state,
                round.mission.mission_id,
                reason.unwrap_or_default()
            );
            let asked = Asked {
                operation: operation.clone(),
                mission: mission.mission_id.to_string(),
                session: round.session.to_owned(),
                agent,
                kind: scope.kind.clone(),
                target: scope.target.clone(),
                reason: bounded(reason.trim(), 2_000),
                idempotency_key: scope.target.clone(),
                // Never a class authorisation: a person of the owning Operation decides.
                exposed: true,
            };
            GreenInk::new(state.clone()).request_always(asked)?;
            decided(state, &operation, mission, &scope)?
                .ok_or("the green ink request was not recorded")?
        }
    };
    let mut tidied = Tidied::new(mission, status.name());
    let request = status.request();
    tidied.green_ink = Some(serde_json::json!({
        "request_id": request.id,
        "operation": request.operation,
        "status": status.name(),
        "expires_at": request.expires_at,
    }));
    Ok(tidied)
}

/// Removes the safe house through its provider and records that it is gone. `approval` is
/// the Operation and ID of the green ink request that allowed it, spent here; `None` when
/// everything in it exists elsewhere. The Mission is checked again right before, under the
/// reservation the caller holds.
fn discard(
    state: &DaemonState,
    round: &Round<'_>,
    target: &str,
    house: &SafeHouse,
    approval: Option<(&str, &str)>,
) -> Result<Tidied, String> {
    let (mission, report) = over_and_kept(state, target)?;
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
    let audited = match approval {
        Some((_, id)) => format!("{} approved by {id}", mission.mission_id),
        None => format!("{} exported", mission.mission_id),
    };
    audit(state, round, &mission, "head_of_station.removed", &audited);
    let mut tidied = Tidied::new(&mission, "removed");
    tidied.removed.push(report.path.clone());
    tidied.freed_bytes = size;
    Ok(tidied)
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

    use super::super::tests::{
        call, kept_safe_house, lease_round, lease_sleeper_round, start, station,
    };
    use super::*;
    use crate::directorates::m::mission::MissionState;
    use crate::directorates::m::roster::Roster;
    use crate::directorates::registry::approvals::{ClassAuthorization, Ruling};
    use crate::directorates::registry::logbook::HEAD_OF_STATION_ROLE;
    use crate::station::safehouse::SafeHouseHygiene;

    /// The Station of `super::super::tests::station`, whose Operation `G`, which owns the
    /// safe houses, would let a `field` Agent delete them without green ink, keeping caches
    /// `left_alone` long.
    fn lenient_station(root: &Path, left_alone: Duration) -> DaemonState {
        let mut state = station(root);
        let checkout = root.join("checkout");
        std::fs::write(
            root.join("operations.toml"),
            format!(
                "[operation.G]\npath = {checkout:?}\nworkspace = {{ provider = \"mock-safehouse\" }}\n\
                 [operation.G.green_ink]\nunattended = {{ field = [\"safe-house-delete\"] }}\n\
                 [operation.H]\npath = {checkout:?}\n\
                 [operation.H.sleepers.rounds]\nschedule = \"0 * * * *\"\ntitle = \"Station round\"\n\
                 role = \"head-of-station\"\neach_station = true\n",
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

    fn round_of<'a>(mission: &'a Mission) -> Round<'a> {
        Round {
            mission,
            session: "provider-session-hos",
        }
    }

    #[test]
    fn caches_go_only_from_a_safe_house_left_alone_long_enough_by_a_mission_that_is_over() {
        let root = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let round_id = format!("hos-caches-round-{pid}");
        let done_id = format!("hos-caches-done-{pid}");
        let live_id = format!("hos-caches-live-{pid}");
        let tracked_id = format!("hos-caches-tracked-{pid}");

        // Left alone for less than the cache age: the caches stay.
        let state = lenient_station(root.path(), Duration::from_secs(7 * 24 * 3600));
        lease_round(&state, "H", &round_id, HEAD_OF_STATION_ROLE, &["test"]);
        let done = kept_safe_house(&state, root.path(), &done_id, MissionState::Done, &[]);
        std::fs::write(done.join(".mock-ignored"), "target\n").unwrap();
        let live = kept_safe_house(&state, root.path(), &live_id, MissionState::Working, &[]);
        // Its target/ holds a file version control tracks: it is not ignored whole.
        let tracked = kept_safe_house(&state, root.path(), &tracked_id, MissionState::Done, &[]);
        let round_mission = state.missions.lock().unwrap().get(&round_id).cloned();
        let round_mission = round_mission.unwrap();
        let round = round_of(&round_mission);
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

        // A cache directory version control does not ignore whole stays.
        let kept = tidy(&state, &round, &tracked_id, Tidy::Caches, None).unwrap();
        assert!(kept.removed.is_empty(), "{kept:?}");
        assert!(
            kept.kept[0].starts_with("target: it holds files"),
            "{kept:?}"
        );
        assert!(tracked.join("target/debug/app").exists());

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

    #[test]
    fn a_safe_house_whose_mission_is_being_started_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let state = lenient_station(root.path(), Duration::ZERO);
        let round_id = format!("hos-race-round-{pid}");
        let target = format!("hos-race-{pid}");
        lease_round(&state, "H", &round_id, HEAD_OF_STATION_ROLE, &["test"]);
        let house = kept_safe_house(&state, root.path(), &target, MissionState::Failed, &[]);
        let round_mission = state
            .missions
            .lock()
            .unwrap()
            .get(&round_id)
            .cloned()
            .unwrap();
        // A retry holds the Mission's start reservation, as start_mission does.
        state.starting.lock().unwrap().insert(target.clone());
        for action in [Tidy::Remove, Tidy::Caches] {
            let refused =
                tidy(&state, &round_of(&round_mission), &target, action, None).unwrap_err();
            assert!(refused.contains("being started"), "{refused}");
        }
        assert!(house.join("main.rs").exists() && house.join("target").exists());
        // While a tidy holds it, a start of the Mission is refused in turn.
        state.starting.lock().unwrap().remove(&target);
        let held = reserve(&state, &target).unwrap();
        assert!(!state.starting.lock().unwrap().insert(target.clone()));
        drop(held);
        assert!(state.starting.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_round_removes_exported_work_unattended_and_asks_the_owning_operation_for_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let state = lenient_station(root.path(), Duration::ZERO);
        let exported_id = format!("hos-exported-{pid}");
        let secrets_id = format!("hos-secrets-{pid}");
        let unexported_id = format!("hos-unexported-{pid}");
        let live_id = format!("hos-live-{pid}");
        let exported = kept_safe_house(&state, root.path(), &exported_id, MissionState::Done, &[]);
        std::fs::write(exported.join(".mock-ignored"), "target\nweb/node_modules\n").unwrap();
        // Its only unrecorded file is an ignored `.env`, which no cache list names.
        let secrets = kept_safe_house(&state, root.path(), &secrets_id, MissionState::Done, &[]);
        std::fs::write(secrets.join(".mock-ignored"), "target\n.env\n").unwrap();
        let unexported = kept_safe_house(
            &state,
            root.path(),
            &unexported_id,
            MissionState::Failed,
            &["abc1234 add the parser"],
        );
        let live = kept_safe_house(&state, root.path(), &live_id, MissionState::Working, &[]);
        // A class authorisation of the housekeeping Operation, and one of `G` itself.
        let now = state.clock.now() as f64;
        for operation in ["H", "G"] {
            state
                .intake
                .put_class_authorization(
                    &ClassAuthorization::new(
                        operation,
                        vec![SAFE_HOUSE_DELETE.to_owned()],
                        "*",
                        "operator-m",
                        now,
                        now + 3600.0,
                    )
                    .for_agents(vec!["*".to_owned()]),
                )
                .unwrap();
        }
        let round = format!("hos-tidy-round-{pid}");
        lease_sleeper_round(&state, &round);
        start(&state, &round).await;
        let tidy_call = |mission: String| {
            let (state, round) = (state.clone(), round.clone());
            async move {
                call(
                    &state,
                    &round,
                    "tidy_safe_house",
                    serde_json::json!({ "mission": mission, "action": "remove", "reason": "disk is low" }),
                )
                .await
                .unwrap()
            }
        };

        // Everything in it exists elsewhere or is a cache: removed at once and audited.
        let removed = json(&tidy_call(exported_id.clone()).await);
        assert_eq!(removed["status"], "removed", "{removed}");
        assert!(!exported.exists());
        let record = state.missions.lock().unwrap().get(&exported_id).cloned();
        assert!(!record.unwrap().safe_house.unwrap().kept);
        assert_eq!(
            audited(&state, "head_of_station.removed")[0].target,
            format!("{exported_id} exported")
        );

        // An ignored file outside the caches exists nowhere else: a person decides.
        let asked = json(&tidy_call(secrets_id.clone()).await);
        assert_eq!(asked["status"], "pending", "{asked}");
        assert!(secrets.join("main.rs").exists());

        // Unexported work: a request under `G`, the Operation that owns the safe house, on its
        // Mission; neither the policy's unattended list nor a class authorisation approves it.
        let asked = json(&tidy_call(unexported_id.clone()).await);
        assert_eq!(asked["status"], "pending", "{asked}");
        assert_eq!(asked["green_ink"]["operation"], "G");
        let request_id = asked["green_ink"]["request_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(unexported.join("main.rs").exists());
        let pending = state.intake.pending_approvals("G", now).unwrap();
        let request = pending
            .iter()
            .find(|request| request.id == request_id)
            .expect("filed under the owning Operation");
        assert_eq!(request.scope.kind, SAFE_HOUSE_DELETE);
        assert_eq!(
            request.scope.target,
            format!("safe house {unexported_id} on Station test")
        );
        assert_eq!(request.mission.as_str(), unexported_id);
        assert!(request.reason.contains("1 change(s) no remote holds"));
        assert!(request.reason.contains("disk is low"));
        assert!(state.intake.pending_approvals("H", now).unwrap().is_empty());
        let again = json(&tidy_call(unexported_id.clone()).await);
        assert_eq!(again["status"], "pending");
        assert_eq!(
            again["green_ink"]["request_id"],
            request_id.as_str(),
            "asked once"
        );
        assert!(unexported.exists(), "asking again removes nothing");

        // A person of `G` approves: a later round removes it and spends the approval.
        state
            .intake
            .decide_approval(
                "G",
                &request_id,
                &Ruling::approve("operator-g", now, now + 600.0),
            )
            .unwrap();
        let later = format!("hos-tidy-later-{pid}");
        lease_sleeper_round(&state, &later);
        start(&state, &later).await;
        let approved = json(
            &call(
                &state,
                &later,
                "tidy_safe_house",
                serde_json::json!({ "mission": unexported_id, "action": "remove" }),
            )
            .await
            .unwrap(),
        );
        assert_eq!(approved["status"], "removed", "{approved}");
        assert!(!unexported.exists());
        let Some(ApprovalStatus::Decided(approval)) =
            state.intake.approval("G", &request_id, now).unwrap()
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
