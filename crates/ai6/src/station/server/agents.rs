//! The Station's side of Agent records: counting each Mission's outcome and each incident
//! charged to its Agent.

use super::DaemonState;
use crate::directorates::m::mission::MissionState;
use crate::directorates::registry::agent_records::{AgentOutcome, AgentOutcomeReport};

/// The outcome a Mission's state counts as, if any.
fn mission_outcome(state: MissionState) -> Option<AgentOutcome> {
    Some(match state {
        MissionState::Done => AgentOutcome::Done,
        MissionState::Failed => AgentOutcome::Failed,
        MissionState::Blown => AgentOutcome::Blown,
        MissionState::Abandoned => AgentOutcome::Abandoned,
        MissionState::Stalled => AgentOutcome::Stalled,
        _ => return None,
    })
}

/// Counts the Mission's state into its Agent's record when it is a finished or stalled one.
/// The Mission's revision names the event, so a state seen twice counts once.
pub(super) fn count_mission(state: &DaemonState, mission_id: &str) {
    let Some(mission) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
    else {
        return;
    };
    let (Some(outcome), Some(agent)) = (mission_outcome(mission.state), &mission.agent_id) else {
        return;
    };
    count(
        state,
        agent,
        mission.operation_id.as_deref(),
        outcome,
        format!("{mission_id}#{}", mission.revision),
    );
}

/// Counts `outcome` for `agent` in `operation`, with the clearance and model it is configured
/// with; `key` names the event. A record that cannot be written is logged: counting never
/// stops work.
pub(super) fn count(
    state: &DaemonState,
    agent: &str,
    operation: Option<&str>,
    outcome: AgentOutcome,
    key: String,
) {
    let mut report = AgentOutcomeReport::new(
        agent,
        operation.map(str::to_owned),
        outcome,
        key,
        state.clock.now() as f64,
    );
    if let Ok(configured) = state.roster.agent(operation, agent) {
        report = report
            .at_clearance(configured.clearance)
            .with_model(configured.model);
    }
    if let Err(error) = state.intake.record_agent_outcome(&report) {
        eprintln!(
            "ai6: {} of Agent {agent:?} was not counted: {error}",
            outcome.as_str()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::apply_event_state;
    use super::super::tests::test_state;
    use crate::MissionId;
    use crate::directorates::m::mission::MissionState;
    use crate::directorates::section_00::agent::AgentEvent;

    const AGENTS: &str =
        "[agent.007]\nhandler = \"mock-agent\"\nmodel = \"opus\"\nclearance = \"field\"\n";

    fn event(kind: &str) -> AgentEvent {
        AgentEvent {
            kind: kind.to_owned(),
            text: None,
            data: None,
            limit: None,
            usage: None,
            tools: Vec::new(),
            quota: Vec::new(),
        }
    }

    fn work(state: &super::DaemonState, mission: &str) {
        state
            .missions
            .lock()
            .unwrap()
            .transition(mission, MissionState::Working)
            .unwrap();
    }

    #[test]
    fn finished_and_stalled_missions_count_once_and_a_new_model_opens_a_tenure() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        work(&state, "mission-1");
        apply_event_state(&state, "mission-1", &event("completed"));
        work(&state, "mission-2");
        apply_event_state(&state, "mission-2", &event("stalled"));
        apply_event_state(&state, "mission-2", &event("completed"));
        super::count_mission(&state, "mission-2");

        let record = state.intake.agent_record("007").unwrap().unwrap();
        let tally = record.in_operation("G");
        assert_eq!((tally.done, tally.stalled, tally.failed), (2, 1, 0));
        assert_eq!(record.by_clearance["field"].done, 2);
        assert!(record.last_mission_at.is_some());
        assert_eq!(record.model_history.len(), 1);

        // The codename gets a new occupant: the next Mission starts a second tenure.
        let state = test_state(root.path(), "", &AGENTS.replace("opus", "sonnet"));
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new("mission-3").unwrap(),
                Some("G".to_owned()),
                "more work".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        work(&state, "mission-3");
        apply_event_state(&state, "mission-3", &event("completed"));
        let record = state.intake.agent_record("007").unwrap().unwrap();
        assert_eq!(record.in_operation("G").done, 3);
        let models: Vec<_> = record
            .model_history
            .iter()
            .map(|tenure| (tenure.model.as_deref(), tenure.to.is_some()))
            .collect();
        assert_eq!(models, [(Some("opus"), true), (Some("sonnet"), false)]);
    }
}
