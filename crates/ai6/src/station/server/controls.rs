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
    ApiErrorResponse, DaemonState, ResumeMission, TransitionMission, interrupt_mission,
    is_terminal_state, lifecycle_name, medical, resume_mission, session_hold, transition_mission,
};
use crate::directorates::m::mission::{Mission, MissionState};
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
                let outcome = apply(&state, &delivery.control).await;
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
        return ControlOutcome::rejected(None, "this Station has no such Mission");
    };
    let current = Some(state_name(mission.state));
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
            ControlOutcome::applied(state_name(after))
        }
        Err(reason) => {
            let now = state
                .missions
                .lock()
                .ok()
                .and_then(|missions| missions.get(mission_id).map(|mission| mission.state))
                .map_or(current, |after| Some(state_name(after)));
            ControlOutcome::rejected(now, reason)
        }
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
    if mission.state == MissionState::Abandoned {
        return Ok(Applied {
            state: MissionState::Abandoned,
            announced: true,
        });
    }
    if is_terminal_state(mission.state) {
        return Err(format!(
            "the Mission already ended ({})",
            state_name(mission.state)
        ));
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
            state_name(mission.state)
        ));
    }
    let agent = agent
        .map(|key| qualified_agent(state, mission, key))
        .transpose()?;
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
        .retry(mission_id, agent)
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
            state_name(mission.state)
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
    .map_err(reason)?;
    Ok(Applied {
        state: started.mission.state,
        announced: true,
    })
}

async fn interrupt(state: &DaemonState, mission: &Mission) -> Result<Applied, String> {
    if mission.state != MissionState::Working || !has_live_session(state, mission) {
        return Err(format!(
            "only a working Session can be interrupted; this Mission is {}",
            state_name(mission.state)
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
        "the provider refused the request".to_owned()
    } else {
        error.error
    }
}

/// The Mission state as Universal Exports and its clients spell it.
fn state_name(state: MissionState) -> &'static str {
    match state {
        MissionState::Queued => "queued",
        MissionState::Working => "working",
        MissionState::Blocked => "blocked",
        MissionState::Stalled => "stalled",
        MissionState::Done => "done",
        MissionState::Failed => "failed",
        MissionState::Blown => "blown",
        MissionState::Abandoned => "abandoned",
        MissionState::Parked => "parked",
        MissionState::Quarantined => "quarantined",
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path as FsPath;

    use ai6_diptel::{ControlId, MissionId, OperationId, SessionId};

    use super::super::tests::{mission_state, start_mission_by_id, test_state, wait_for_state};
    use super::super::{DEFAULT_LEASE, schedule_queued_once, unix_now};
    use super::*;

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
        wait_for_state(&state, "mission-1", MissionState::Done).await;
        let mission = mission_state(&state, "mission-1");
        assert_eq!(mission.session_ids.len(), 1);
        assert_eq!(mission.session_ids[0].as_str(), PROVIDER_SESSION);
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
        state
            .intake
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
