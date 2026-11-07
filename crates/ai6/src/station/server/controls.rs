//! Operator controls: cancel, retry, resume and interrupt, taken from Universal Exports.
//!
//! Universal Exports queues each control for the Station that holds the Mission; this Station
//! checks the Mission, its Operation and its own Session lease, applies the control through
//! the same paths its loopback API uses, and reports the outcome.

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;

use ai6_diptel::{ControlAction, MissionControl};

use super::{
    ApiErrorResponse, DaemonState, ResumeMission, TransitionMission, agents, interrupt_mission,
    is_terminal_state, lifecycle_name, medical, resume_mission, session_hold, transition_mission,
    unix_now,
};
use crate::bounds::Limit;
use crate::directorates::m::issues::OnSecret;
use crate::directorates::m::mission::{Mission, MissionState};
use crate::directorates::q::outbound::{self, LinkRule, SecretSet};
use crate::directorates::registry::sessions::{SessionStatus, SessionUpdate};
use crate::station::publisher::{ControlDelivery, ControlOutcome};

/// How often the consumer looks up from an empty queue to see whether the Station is stopping.
const STOP_CHECK: Duration = Duration::from_millis(250);

/// Starts applying this Station's controls; `None` without a Universal Exports publisher.
pub(super) fn spawn(state: &DaemonState) -> std::io::Result<Option<JoinHandle<()>>> {
    let Some(publisher) = &state.publisher else {
        return Ok(None);
    };
    let receiver = publisher.receive_controls(&state.station_id)?;
    Ok(Some(tokio::spawn(serve(state.clone(), receiver))))
}

async fn serve(state: DaemonState, mut receiver: Receiver<ControlDelivery>) {
    while !state.stopping.load(Ordering::Acquire) {
        match tokio::time::timeout(STOP_CHECK, receiver.recv()).await {
            Ok(Some(delivery)) => {
                let outcome = apply(&state.current(), &delivery.control).await;
                let _ = delivery.outcome.send(outcome);
            }
            Ok(None) => break,
            Err(_) => {}
        }
    }
}

/// Applies one control to a Mission in this Station's book and says what happened.
pub(super) async fn apply(state: &DaemonState, control: &MissionControl) -> ControlOutcome {
    let mission_id = control.mission_id().as_str();
    let mission = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned());
    // A Mission of another Operation is reported exactly like an unknown one.
    let Some(mission) = mission
        .filter(|mission| mission.operation_id.as_deref() == Some(control.operation_id().as_str()))
    else {
        return match ended_brief(state, control) {
            Some(ended) => ControlOutcome::applied(ended),
            None => ControlOutcome::rejected(None, "this Station has no such Mission"),
        };
    };
    let current = Some(mission.state.as_str());
    if let Err(reason) = check_lease(state, mission_id) {
        return ControlOutcome::rejected(current, reason);
    }
    if let Some(expected) = control.session_id()
        && mission.session_ids.last().map(|id| id.as_str()) != Some(expected.as_str())
    {
        return ControlOutcome::rejected(current, "the Mission has moved on to another Session");
    }
    let result = match control.action() {
        ControlAction::Cancel => cancel(state, &mission).await,
        ControlAction::Retry => retry(state, &mission, control.agent()).await,
        ControlAction::Resume => resume(state, &mission, control.message()).await,
        ControlAction::Interrupt => interrupt(state, &mission).await,
        _ => Err("this Station does not support the action".to_owned()),
    };
    match result {
        Ok(Applied {
            state: after,
            announced,
        }) => {
            if !announced {
                announce(state, &mission, after);
            }
            ControlOutcome::applied(after.as_str())
        }
        Err(reason) => {
            let now = state
                .missions
                .lock()
                .ok()
                .and_then(|missions| missions.get(mission_id).map(|mission| mission.state))
                .map_or(current, |after| Some(after.as_str()));
            ControlOutcome::rejected(now, reason)
        }
    }
}

/// How the Registry filed the brief of a Mission no book here holds, when a cancel asks for one that
/// ended: there is nothing left to stop.
fn ended_brief(state: &DaemonState, control: &MissionControl) -> Option<&'static str> {
    if control.action() != ControlAction::Cancel {
        return None;
    }
    let filed = state
        .intake
        .filed_brief(control.mission_id().as_str())
        .ok()??;
    if filed.brief.operation.as_deref() != Some(control.operation_id().as_str()) {
        return None;
    }
    match filed.state.as_str() {
        "done" => Some("done"),
        "failed" => Some("failed"),
        _ => None,
    }
}

/// The Mission's state after a control, and whether a lifecycle event already reported it.
struct Applied {
    state: MissionState,
    announced: bool,
}

/// Refuses a control while another Station holds the Mission's Session, or this one's
/// fencing token is stale.
fn check_lease(state: &DaemonState, mission_id: &str) -> Result<(), String> {
    let Some(hold) = session_hold(state, mission_id) else {
        return Ok(());
    };
    match state.intake.session(&hold.id) {
        Ok(Some(record)) if record.station != state.station_id => Err(format!(
            "Station {:?} holds this Mission's Session",
            record.station
        )),
        Ok(Some(record)) if record.fence != hold.fence => {
            Err("this Station's lease on the Session is stale".to_owned())
        }
        Ok(_) => Ok(()),
        Err(_) => Err("the Registry could not be read".to_owned()),
    }
}

async fn cancel(state: &DaemonState, mission: &Mission) -> Result<Applied, String> {
    if is_terminal_state(mission.state) {
        return Ok(Applied {
            state: mission.state,
            announced: true,
        });
    }
    let live = has_live_session(state, mission);
    let cancelled = transition_mission(
        State(state.clone()),
        Path(mission.mission_id.to_string()),
        Json(TransitionMission {
            state: MissionState::Abandoned,
        }),
    )
    .await
    .map_err(reason)?;
    Ok(Applied {
        state: cancelled.0.state,
        announced: live,
    })
}

async fn retry(
    state: &DaemonState,
    mission: &Mission,
    agent: Option<&str>,
) -> Result<Applied, String> {
    if !matches!(
        mission.state,
        MissionState::Stalled
            | MissionState::Done
            | MissionState::Failed
            | MissionState::Blown
            | MissionState::Abandoned
    ) {
        return Err(format!(
            "only a stalled or finished Mission can be retried; this one is {}",
            mission.state.as_str()
        ));
    }
    let agent = match agent {
        Some(key) => qualified_agent(state, mission, key)?,
        None => agents::retry_agent(state, mission)?,
    };
    let agent = Some(agent);
    let mission_id = mission.mission_id.as_str();
    stop_live_session(state, mission_id).await;
    if let Some(hold) = session_hold(state, mission_id) {
        // The stopped attempt is over; the next start records its replacement.
        let _ = state.intake.update_session(
            &hold.id,
            &state.station_id,
            hold.fence,
            SessionUpdate::default().status(SessionStatus::Ended),
        );
    }
    let retried = state
        .missions
        .lock()
        .map_err(|_| "the Mission book is unavailable".to_owned())?
        .retry_past_brief(mission_id, agent)
        .map_err(|error| format!("the Mission could not be queued again: {error:?}"))?;
    if let Ok(mut retries) = state.retry_after.lock() {
        retries.remove(mission_id);
    }
    Ok(Applied {
        state: retried.state,
        announced: false,
    })
}

/// The codename of an Agent that may take the Mission: configured for its Operation,
/// qualified for its role and not quarantined.
fn qualified_agent(state: &DaemonState, mission: &Mission, key: &str) -> Result<String, String> {
    let operation = mission.operation_id.as_deref();
    let agent = state
        .roster
        .agent(operation, key)
        .map_err(|_| format!("Agent {key:?} is not configured for this Operation"))?;
    if let Some(role) = &mission.role_id
        && !agent.roles.iter().any(|capability| {
            role == capability
                || role.starts_with(&format!("{}/", capability.trim_end_matches('/')))
        })
    {
        return Err(format!(
            "Agent {:?} is not qualified for role {role:?}",
            agent.id
        ));
    }
    if medical::is_quarantined(state, &agent.id, operation) {
        return Err(format!("Agent {:?} is quarantined", agent.id));
    }
    Ok(agent.id)
}

async fn resume(
    state: &DaemonState,
    mission: &Mission,
    message: Option<&str>,
) -> Result<Applied, String> {
    if !matches!(
        mission.state,
        MissionState::Parked | MissionState::Blocked | MissionState::Stalled
    ) {
        return Err(format!(
            "only a parked, blocked or stalled Mission can resume; this one is {}",
            mission.state.as_str()
        ));
    }
    let (_, Json(started)) = resume_mission(
        State(state.clone()),
        Path(mission.mission_id.to_string()),
        Json(ResumeMission {
            message: message.map(str::to_owned),
            environment: Default::default(),
        }),
    )
    .await
    .map_err(|error| reason_for(state, mission, error))?;
    Ok(Applied {
        state: started.mission.state,
        announced: true,
    })
}

async fn interrupt(state: &DaemonState, mission: &Mission) -> Result<Applied, String> {
    if mission.state != MissionState::Working || !has_live_session(state, mission) {
        return Err(format!(
            "only a working Session can be interrupted; this Mission is {}",
            mission.state.as_str()
        ));
    }
    let interrupted = interrupt_mission(State(state.clone()), Path(mission.mission_id.to_string()))
        .await
        .map_err(reason)?;
    Ok(Applied {
        state: interrupted.0.state,
        announced: true,
    })
}

fn has_live_session(state: &DaemonState, mission: &Mission) -> bool {
    state
        .sessions
        .lock()
        .is_ok_and(|sessions| sessions.contains_key(mission.mission_id.as_str()))
}

/// Closes a Mission's live provider Session and leaves its safe house for the next attempt.
async fn stop_live_session(state: &DaemonState, mission_id: &str) {
    let operation = state.sessions.lock().ok().and_then(|mut sessions| {
        sessions
            .remove(mission_id)
            .inspect(|operation| operation.gate.close())
    });
    if let Some(operation) = operation {
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(session) = operation.session.lock() {
                let _ = session.interrupt();
                let _ = session.close();
            }
        })
        .await;
    }
}

/// The reason a brief settled without its Station is filed with.
pub(super) const CANCEL_UNCONFIRMED: &str = "cancelled; the Station did not confirm";

/// On the Headquarters Station: files each claimed brief whose cancel was asked for at least
/// `[registry.limits.briefs] cancel_settle_after` ago as failed, releasing its lease, when its
/// Station has not finished it. The outcome names the Mission's Session at the time, which tells
/// the Station holding the Mission which attempt to stop.
pub(super) fn settle_unconfirmed_cancels(state: &DaemonState) {
    let Ok(requests) = state.intake.cancel_requests() else {
        return;
    };
    let after = state
        .intake
        .limits()
        .briefs
        .cancel_settle_after
        .0
        .as_secs_f64();
    let now = unix_now();
    for (key, asked) in requests {
        if now - asked < after {
            continue;
        }
        let (Ok(Some(lease)), Ok(Some(brief))) =
            (state.intake.lease(&key), state.intake.read_claimed(&key))
        else {
            continue;
        };
        let mission_id = lease
            .meta()
            .get("mission_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&key)
            .to_owned();
        let session = state
            .intake
            .latest_session(&mission_id)
            .ok()
            .flatten()
            .and_then(|record| record.provider_session_id);
        let outcome = serde_json::json!({
            "state": "failed",
            "mission_id": mission_id,
            "reason": CANCEL_UNCONFIRMED,
            "station": lease.station(),
            "session_id": session,
            "cancel_requested_at": asked,
        });
        if state
            .intake
            .finish(&brief, &outcome, false, None, Some(lease.station()))
            .unwrap_or(false)
        {
            eprintln!(
                "ai6: Mission {mission_id:?} settled: cancelled {:.0} s ago and Station {:?} \
                 did not confirm",
                now - asked,
                lease.station()
            );
        }
    }
}

/// Stops each Mission in this Station's book whose brief Headquarters settled after an
/// unconfirmed cancel, when it is still on the attempt the settlement names. A Mission retried
/// since is queued, or on a new Session, and runs on.
pub(super) async fn stop_settled(state: &DaemonState) {
    let candidates: Vec<Mission> = state
        .missions
        .lock()
        .map(|missions| {
            missions
                .list()
                .into_iter()
                .filter(|mission| {
                    !is_terminal_state(mission.state) && mission.state != MissionState::Queued
                })
                .collect()
        })
        .unwrap_or_default();
    if candidates.is_empty() {
        return;
    }
    let reading = state.clone();
    let ids: Vec<String> = candidates
        .iter()
        .map(|mission| mission.mission_id.to_string())
        .collect();
    let settled = tokio::task::spawn_blocking(move || {
        let station = reading.station_id.as_str();
        let held: Vec<String> = reading
            .intake
            .leases()
            .map(|leases| {
                leases
                    .into_iter()
                    .filter(|lease| lease.station() == station)
                    .map(|lease| lease.key().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        ids.into_iter()
            .filter(|id| !held.contains(id))
            .filter_map(|id| {
                let filed = reading.intake.filed_brief(&id).ok().flatten()?;
                let outcome = filed.outcome?;
                (filed.state == "failed" && outcome["reason"] == CANCEL_UNCONFIRMED)
                    .then(|| (id, outcome["session_id"].as_str().map(str::to_owned)))
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    for (id, session) in settled {
        let Some(mission) = candidates
            .iter()
            .find(|mission| mission.mission_id.as_str() == id)
        else {
            continue;
        };
        if mission.session_ids.last().map(|id| id.as_str()) != session.as_deref() {
            continue;
        }
        eprintln!("ai6: Mission {id:?} is stopped: its brief was settled as {CANCEL_UNCONFIRMED}");
        if let Ok(Applied {
            state: after,
            announced: false,
        }) = cancel(state, mission).await
        {
            announce(state, mission, after);
        }
    }
}

/// Publishes the Mission's new state on its latest provider Session, for a control that
/// changed it while no Session was live to report it.
fn announce(state: &DaemonState, mission: &Mission, after: MissionState) {
    let (Some(publisher), Some(name), Some(operation), Some(session)) = (
        &state.publisher,
        lifecycle_name(after),
        &mission.operation_id,
        mission.session_ids.last(),
    ) else {
        return;
    };
    let telemetry = publisher.session(operation.clone(), mission.mission_id.clone());
    telemetry.bind(session.clone());
    telemetry.publish_lifecycle(name);
}

/// A refused internal request as a reason the operator may see: a provider's own words stay
/// out of it.
fn reason(error: ApiErrorResponse) -> String {
    let ApiErrorResponse(status, Json(error)) = error;
    if status == StatusCode::BAD_GATEWAY {
        REFUSED.to_owned()
    } else {
        error.error
    }
}

/// What an operator is told when the provider refused the request.
const REFUSED: &str = "the provider refused the request";

/// Why a control on `mission` failed. A provider's words say what is wrong, but they are not AI6's
/// and may quote its command line or environment: only the first line is passed on, and only when
/// it holds nothing shaped like a credential and none of the Agent's secret settings; the Station's
/// log has the whole text.
fn reason_for(state: &DaemonState, mission: &Mission, error: ApiErrorResponse) -> String {
    let ApiErrorResponse(status, Json(error)) = error;
    if status != StatusCode::BAD_GATEWAY {
        return error.error;
    }
    let cause = error.error.lines().next().unwrap_or_default().trim();
    let secrets = SecretSet::new(
        mission
            .agent_id
            .as_deref()
            .and_then(|agent| {
                state
                    .roster
                    .agent(mission.operation_id.as_deref(), agent)
                    .ok()
            })
            .into_iter()
            .flat_map(|agent| agent.env.into_iter())
            .filter(|(name, _)| {
                let name = name.to_uppercase();
                ["TOKEN", "KEY", "SECRET", "PASSWORD", "CREDENTIAL"]
                    .iter()
                    .any(|word| name.contains(word))
            })
            .map(|(_, value)| value),
        state.station.limits().secrets.min_matched_bytes,
    );
    match outbound::check(
        cause,
        &secrets,
        &LinkRule::any(),
        Limit::UNBOUNDED,
        OnSecret::Refuse,
    ) {
        Ok(_) => format!("{REFUSED}: {cause}"),
        Err(_) => format!("{REFUSED}; the Station's log says why"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path as FsPath;

    use ai6_diptel::{ControlId, MissionId, OperationId, SessionId};

    use super::super::tests::{
        mission_state, start_mission_by_id, test_state, wait_for_done, wait_for_state,
    };
    use super::super::{DEFAULT_LEASE, schedule_queued_once, unix_now};
    use super::*;
    use crate::directorates::registry::store::FileRegistry;

    /// 008's mock provider issues its own Session ID and then stays quiet, working.
    const TWO_AGENTS: &str = "[agent.007]\nhandler = \"mock-agent\"\nroles = [\"case-officer\"]\n\
                              [agent.008]\nhandler = \"mock-spend\"\nroles = [\"case-officer\"]\n\
                              [agent.009]\nhandler = \"mock-agent\"\n";
    const PROVIDER_SESSION: &str = "provider-session-007";

    fn control(mission: &str, action: ControlAction) -> MissionControl {
        MissionControl::new(
            ControlId::new(format!("control-{mission}-{}", action.as_str())).unwrap(),
            OperationId::new("G").unwrap(),
            MissionId::new(mission).unwrap(),
            action,
        )
    }

    fn on_session(control: MissionControl) -> MissionControl {
        control.with_session(SessionId::new(PROVIDER_SESSION).unwrap())
    }

    fn state_in(root: &FsPath, agents: &str) -> DaemonState {
        test_state(root, "", agents)
    }

    /// Ends the event pumps of Sessions a test leaves live.
    fn stop(state: &DaemonState) {
        state.stopping.store(true, Ordering::Release);
    }

    /// Starts `mission-1`; the mock provider reports it blocked with its Session still live.
    async fn blocked_mission(state: &DaemonState) {
        start_mission_by_id(state, "mission-1").await;
        wait_for_state(state, "mission-1", MissionState::Blocked).await;
    }

    #[tokio::test]
    async fn cancel_stops_the_live_session_and_abandons_the_mission_once() {
        let root = tempfile::tempdir().unwrap();
        let state = state_in(root.path(), TWO_AGENTS);
        blocked_mission(&state).await;
        let hold = session_hold(&state, "mission-1").unwrap();

        let outcome = apply(
            &state,
            &on_session(control("mission-1", ControlAction::Cancel)),
        )
        .await;
        assert_eq!(outcome, ControlOutcome::applied("abandoned"));
        let mission = mission_state(&state, "mission-1");
        assert_eq!(mission.state, MissionState::Abandoned);
        assert_eq!(mission.session_ids[0].as_str(), PROVIDER_SESSION);
        assert!(state.sessions.lock().unwrap().is_empty(), "no live Session");
        assert_eq!(
            state.intake.session(&hold.id).unwrap().unwrap().status,
            SessionStatus::Ended
        );

        // Repeating the request is harmless; a queued Mission with no Session is abandoned too.
        let again = apply(&state, &control("mission-1", ControlAction::Cancel)).await;
        assert_eq!(again, ControlOutcome::applied("abandoned"));
        let queued = apply(&state, &control("mission-2", ControlAction::Cancel)).await;
        assert_eq!(queued, ControlOutcome::applied("abandoned"));
        assert!(mission_state(&state, "mission-2").session_ids.is_empty());
    }

    #[tokio::test]
    async fn a_cancel_of_a_mission_that_ended_is_applied_and_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let state = state_in(root.path(), TWO_AGENTS);
        state
            .missions
            .lock()
            .unwrap()
            .fail_for("mission-1", "it did not work")
            .unwrap();
        let outcome = apply(&state, &control("mission-1", ControlAction::Cancel)).await;
        assert_eq!(outcome, ControlOutcome::applied("failed"));
        assert_eq!(
            mission_state(&state, "mission-1").state,
            MissionState::Failed
        );
    }

    /// A brief settled while no Station holds its Mission: Headquarters is asked, has no Mission
    /// to stop, and the cancel is what the Registry already says.
    #[tokio::test]
    async fn a_cancel_of_a_brief_that_ended_with_no_mission_here_is_applied() {
        let root = tempfile::tempdir().unwrap();
        let state = state_in(root.path(), TWO_AGENTS);
        let brief: crate::directorates::registry::store::Brief =
            serde_json::from_value(serde_json::json!({
                "title": "gone",
                "id": "gone",
                "source": "manual",
                "operation": "G",
            }))
            .unwrap();
        state.intake.submit_brief(&brief).unwrap();
        let claimed = state.intake.claim_new(Some(&brief.key())).unwrap().unwrap();
        state
            .intake
            .finish(
                &claimed,
                &serde_json::json!({"state": "failed", "reason": "no"}),
                false,
                None,
                None,
            )
            .unwrap();
        let outcome = apply(&state, &control(&brief.key(), ControlAction::Cancel)).await;
        assert_eq!(outcome, ControlOutcome::applied("failed"));

        let elsewhere = MissionControl::new(
            ai6_diptel::ControlId::new("c-2").unwrap(),
            ai6_diptel::OperationId::new("L").unwrap(),
            ai6_diptel::MissionId::new(brief.key()).unwrap(),
            ControlAction::Cancel,
        );
        assert_eq!(apply(&state, &elsewhere).await.outcome, "rejected");
        let unknown = apply(&state, &control("mission-nobody", ControlAction::Cancel)).await;
        assert_eq!(unknown.outcome, "rejected");
    }

    #[tokio::test]
    async fn retry_queues_a_new_attempt_for_another_qualified_agent() {
        let root = tempfile::tempdir().unwrap();
        let state = state_in(root.path(), TWO_AGENTS);
        blocked_mission(&state).await;
        let first = session_hold(&state, "mission-1").unwrap();

        // A live Mission is not retried.
        let refused = apply(&state, &control("mission-1", ControlAction::Retry)).await;
        assert_eq!(refused.outcome, "rejected");
        assert_eq!(refused.state, Some("blocked"));

        apply(&state, &control("mission-1", ControlAction::Cancel)).await;
        let unknown = control("mission-1", ControlAction::Retry)
            .with_agent("006")
            .unwrap();
        assert_eq!(apply(&state, &unknown).await.outcome, "rejected");

        let retry = control("mission-1", ControlAction::Retry)
            .with_agent("008")
            .unwrap();
        assert_eq!(
            apply(&state, &retry).await,
            ControlOutcome::applied("queued")
        );
        let queued = mission_state(&state, "mission-1");
        assert_eq!(queued.agent_id.as_deref(), Some("008"));
        assert_eq!(queued.session_ids[0].as_str(), PROVIDER_SESSION);

        // The scheduler starts it as the second attempt, continuing the first.
        schedule_queued_once(&state).await;
        wait_for_state(&state, "mission-1", MissionState::Working).await;
        let second = session_hold(&state, "mission-1").unwrap();
        let record = state.intake.session(&second.id).unwrap().unwrap();
        assert_eq!(record.attempt, 2);
        assert_eq!(record.predecessor.as_deref(), Some(first.id.as_str()));
        assert_eq!(record.station, "test");
        assert_eq!(
            record.provider_session_id.as_deref(),
            Some("provider-session-spend")
        );
        let ids = mission_state(&state, "mission-1").session_ids;
        assert_eq!(
            ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            [PROVIDER_SESSION, "provider-session-spend"]
        );
        stop(&state);
    }

    #[tokio::test]
    async fn retry_refuses_an_agent_without_the_missions_role() {
        let root = tempfile::tempdir().unwrap();
        let state = state_in(root.path(), TWO_AGENTS);
        {
            let mut missions = state.missions.lock().unwrap();
            missions
                .create(
                    crate::MissionId::try_new("mission-role").unwrap(),
                    Some("G".to_owned()),
                    "review".to_owned(),
                    Some("007".to_owned()),
                    Some("case-officer".to_owned()),
                )
                .unwrap();
            missions
                .transition("mission-role", MissionState::Failed)
                .unwrap();
        }
        let retry = control("mission-role", ControlAction::Retry)
            .with_agent("009")
            .unwrap();
        let outcome = apply(&state, &retry).await;
        assert_eq!(outcome.outcome, "rejected");
        assert_eq!(outcome.state, Some("failed"));
        assert!(outcome.reason.unwrap().contains("not qualified"));
    }

    #[tokio::test]
    async fn resume_continues_the_same_provider_session_with_the_message() {
        let root = tempfile::tempdir().unwrap();
        let state = state_in(root.path(), TWO_AGENTS);
        let queued = apply(&state, &control("mission-1", ControlAction::Resume)).await;
        assert_eq!(queued.outcome, "rejected");
        assert_eq!(queued.state, Some("queued"));

        blocked_mission(&state).await;
        let resume = on_session(control("mission-1", ControlAction::Resume))
            .with_message("the review is unblocked; carry on")
            .unwrap();
        assert_eq!(
            apply(&state, &resume).await,
            ControlOutcome::applied("working")
        );
        // The mock's resumed turn completes on the provider Session it was given.
        wait_for_done(&state, "mission-1").await;
        let mission = mission_state(&state, "mission-1");
        assert_eq!(mission.session_ids.len(), 1);
        assert_eq!(mission.session_ids[0].as_str(), PROVIDER_SESSION);
    }

    /// A refused resume has told the gateway of the Session again, which then shows it working.
    #[cfg(feature = "publisher")]
    #[tokio::test]
    async fn a_refused_resume_publishes_the_missions_state() {
        use super::super::tests::{recording_gateway, wait_for_publication};
        use crate::station::publisher::{Config, Publisher};
        let root = tempfile::tempdir().unwrap();
        let mut state = state_in(root.path(), TWO_AGENTS);
        let (origin, requests) = recording_gateway();
        state.publisher =
            Some(Publisher::start(Config::try_new(&origin, "test-token").unwrap()).unwrap());
        blocked_mission(&state).await;
        let registered = |path: &str, _: &serde_json::Value| path == "/v1/station/sessions";
        wait_for_publication(&requests, |path, body| {
            path.contains("/lifecycle/") && body["state"] == "blocked"
        })
        .expect("the start publishes the block");
        // A handler that does not build, which a resume finds out after it binds its telemetry.
        state
            .sessions
            .lock()
            .unwrap()
            .get_mut("mission-1")
            .unwrap()
            .handler_provider = "no-such-handler".to_owned();

        let refused = apply(
            &state,
            &on_session(control("mission-1", ControlAction::Resume)),
        )
        .await;

        assert_eq!(refused.outcome, "rejected");
        assert_eq!(refused.state, Some("blocked"));
        wait_for_publication(&requests, registered).expect("the resume binds the Session again");
        let lifecycle = wait_for_publication(&requests, |path, _| path.contains("/lifecycle/"))
            .expect("the Mission's state is published after that");
        assert_eq!(lifecycle["state"], "blocked");
        stop(&state);
    }

    #[tokio::test]
    async fn interrupt_stops_the_turn_and_keeps_the_session() {
        let root = tempfile::tempdir().unwrap();
        let state = state_in(root.path(), "[agent.007]\nhandler = \"mock-spend\"\n");
        let idle = apply(&state, &control("mission-1", ControlAction::Interrupt)).await;
        assert_eq!(idle.outcome, "rejected");

        start_mission_by_id(&state, "mission-1").await;
        wait_for_state(&state, "mission-1", MissionState::Working).await;
        let interrupt = control("mission-1", ControlAction::Interrupt)
            .with_session(SessionId::new("provider-session-spend").unwrap());
        assert_eq!(
            apply(&state, &interrupt).await,
            ControlOutcome::applied("blocked")
        );
        assert!(state.sessions.lock().unwrap().contains_key("mission-1"));
        assert_eq!(
            mission_state(&state, "mission-1").state,
            MissionState::Blocked
        );
        stop(&state);
    }

    #[tokio::test]
    async fn a_control_is_refused_for_another_operation_session_or_station() {
        let root = tempfile::tempdir().unwrap();
        let state = state_in(root.path(), TWO_AGENTS);
        blocked_mission(&state).await;

        let other_operation = MissionControl::new(
            ControlId::new("control-x").unwrap(),
            OperationId::new("Q").unwrap(),
            MissionId::new("mission-1").unwrap(),
            ControlAction::Cancel,
        );
        let outcome = apply(&state, &other_operation).await;
        assert_eq!(
            outcome,
            ControlOutcome::rejected(None, "this Station has no such Mission")
        );

        let stale = control("mission-1", ControlAction::Cancel)
            .with_session(SessionId::new("provider-session-old").unwrap());
        assert_eq!(apply(&state, &stale).await.outcome, "rejected");

        // Another Station took the Session's lease: this one may not act on it.
        let hold = session_hold(&state, "mission-1").unwrap();
        FileRegistry::open(root.path().join("registry"))
            .take_session_at(
                &hold.id,
                "other-station",
                DEFAULT_LEASE,
                unix_now() + 3_600.0,
            )
            .unwrap();
        let fenced = apply(&state, &control("mission-1", ControlAction::Cancel)).await;
        assert_eq!(fenced.outcome, "rejected");
        assert!(fenced.reason.unwrap().contains("other-station"));
        assert_eq!(
            mission_state(&state, "mission-1").state,
            MissionState::Blocked
        );
        stop(&state);
    }
}
