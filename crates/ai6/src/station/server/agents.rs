//! The Station's side of the roster lifecycle: choosing Agents that are neither quarantined
//! nor retired, the clearance each works at, and counting each Mission's outcome and each
//! incident charged to its Agent.

use super::{DaemonState, medical};
use crate::directorates::m::mission::{Mission, MissionState};
use crate::directorates::m::roster::{Agent, AgentQuery, Retirement, Roster, RosterError};
use crate::directorates::registry::agent_records::{AgentOutcome, AgentOutcomeReport};

/// The highest-priority Agent posted to `operation` for `role` and `tags` whose handler is
/// among `handlers` (any when `None`), passing over quarantined and retired Agents and those
/// in `excluded`.
pub(super) fn select(
    state: &DaemonState,
    operation: Option<&str>,
    role: Option<&str>,
    tags: &[String],
    handlers: Option<&[String]>,
    excluded: &[String],
) -> Result<Agent, RosterError> {
    let mut passed_over = medical::quarantined_agents(state, operation);
    passed_over.extend_from_slice(excluded);
    state.roster.select_agent(
        &AgentQuery::new()
            .operation(operation)
            .role(role)
            .tags(tags)
            .handlers(handlers)
            .excluding(&passed_over)
            .at(state.clock.now()),
    )
}

/// The Agent as it works in `operation` now: configured, with the clearance it holds there.
pub(super) fn effective(
    state: &DaemonState,
    operation: Option<&str>,
    key: &str,
) -> Result<Agent, RosterError> {
    let mut agent = state.roster.agent(operation, key)?;
    if let Some(operation) = operation {
        let operation = state.roster.operation(operation)?;
        agent.clearance = agent.effective_clearance(operation, state.clock.now());
    }
    Ok(agent)
}

/// Why `agent` may take no new work now, when it is retired.
pub(super) fn retired(state: &DaemonState, agent: &Agent) -> Option<String> {
    agent.is_retired(state.clock.now()).then(|| {
        format!(
            "Agent {:?} retired on {}",
            agent.id,
            agent.retire_on.as_deref().unwrap_or_default()
        )
    })
}

/// Gives a queued Mission whose Agent is quarantined or retired to the next qualified Agent.
///
/// Returns `false` when it must wait: its Agent may not work and no other can take it.
pub(super) fn reassign_if_unavailable(state: &DaemonState, mission: &Mission) -> bool {
    let Some(agent) = mission.agent_id.as_deref() else {
        return true;
    };
    let operation = mission.operation_id.as_deref();
    let retired = state
        .roster
        .agent(operation, agent)
        .is_ok_and(|agent| agent.is_retired(state.clock.now()));
    if !retired && !medical::is_quarantined(state, agent, operation) {
        return true;
    }
    let handlers = super::available_handlers(state);
    let Ok(backup) = select(
        state,
        operation,
        mission.role_id.as_deref(),
        &[],
        Some(&handlers),
        &[],
    ) else {
        return false;
    };
    state.missions.lock().ok().is_some_and(|mut missions| {
        missions
            .reassign(mission.mission_id.as_str(), backup.id)
            .is_ok()
    })
}

/// The codenames of the roster's retired Agents, as Operation `operation` resolves them.
pub(super) fn retired_agents(state: &DaemonState, operation: &str) -> Vec<String> {
    let now = state.clock.now();
    let posted = state.roster.agents_for_operation(operation);
    posted
        .unwrap_or_default()
        .into_iter()
        .filter(|agent| agent.is_retired(now))
        .map(|agent| agent.id)
        .collect()
}

/// The roster's Agents whose retirement is due or near at `now`, for the check-in and the
/// log: `{agent, retire_on, on_retire, status}` each.
pub(super) fn retirements(roster: &Roster, now: i64) -> Vec<serde_json::Value> {
    roster
        .agent_ids()
        .filter_map(|id| roster.agent(None, id).ok())
        .filter_map(|agent| {
            let status = agent.retirement(now);
            (status != Retirement::Serving).then(|| {
                serde_json::json!({
                    "agent": agent.id,
                    "retire_on": agent.retire_on,
                    "on_retire": agent.on_retire.as_str(),
                    "status": status.as_str(),
                })
            })
        })
        .collect()
}

/// Logs each Agent whose retirement is due or near at `now`.
pub(super) fn log_retirements(roster: &Roster, now: i64) {
    for notice in retirements(roster, now) {
        eprintln!(
            "ai6: Agent {} {} on {} (on_retire {})",
            notice["agent"],
            match notice["status"].as_str() {
                Some("retiring") => "retires",
                _ => "retired",
            },
            notice["retire_on"].as_str().unwrap_or_default(),
            notice["on_retire"].as_str().unwrap_or_default(),
        );
    }
}

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

/// Counts `outcome` for `agent` in `operation`, with the clearance it works at and the model
/// it is configured with; `key` names the event. A record that cannot be written is logged: counting never
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
    if let Ok(configured) = effective(state, operation, agent) {
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
    use std::sync::Arc;

    use super::super::tests::test_state;
    use super::super::{Clock, apply_event_state, resolve_brief_assignment};
    use crate::MissionId;
    use crate::directorates::m::mission::MissionState;
    use crate::directorates::registry::store::Brief;
    use crate::directorates::section_00::agent::AgentEvent;

    /// 2027-01-31T00:01:00Z, a minute into the retirement day.
    const AFTER_RETIREMENT: i64 = 1_801_353_660;

    const RETIRING: &str = "[role.case-officer]\npurpose = \"Run a Mission.\"\npolicy = \"Report back.\"\n[agent.007]\nhandler = \"mock-agent\"\nroles = [\"case-officer\"]\nclearance = \"00\"\npriority = 10\nretire_on = \"2027-01-31\"\non_retire = \"retired\"\n[agent.008]\nhandler = \"mock-agent\"\nroles = [\"case-officer\"]\nclearance = \"field\"\npriority = 5\nretire_on = \"2027-01-31\"\n";

    fn brief(role: &str, agent: Option<&str>) -> Brief {
        serde_json::from_value(serde_json::json!({
            "title": "Inspect", "id": "b-1", "operation": "G", "role": role, "agent": agent,
        }))
        .unwrap()
    }

    #[test]
    fn retired_agents_pass_their_work_to_the_backup_and_desk_duty_caps_clearance() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "clearance = \"00\"\n", RETIRING);
        state.clock = Clock(Arc::new(|| AFTER_RETIREMENT));

        let mission = state
            .missions
            .lock()
            .unwrap()
            .get("mission-1")
            .cloned()
            .unwrap();
        assert!(super::reassign_if_unavailable(&state, &mission));
        let reassigned = state
            .missions
            .lock()
            .unwrap()
            .get("mission-1")
            .cloned()
            .unwrap();
        assert_eq!(reassigned.agent_id.as_deref(), Some("008"));
        assert_eq!(
            super::effective(&state, Some("G"), "008")
                .unwrap()
                .clearance,
            "desk",
            "008 is on desk duty"
        );
        assert_eq!(
            resolve_brief_assignment(&state, &brief("case-officer", Some("007"))).unwrap_err(),
            "Agent \"007\" retired on 2027-01-31"
        );
        let statuses: Vec<_> = super::retirements(&state.roster, state.clock.now())
            .into_iter()
            .map(|notice| notice["status"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(statuses, ["retired", "desk_duty"]);

        // A role whose only Agent is retired fails assignment, naming it.
        let alone = tempfile::tempdir().unwrap();
        let only = RETIRING.split("[agent.008]").next().unwrap();
        let mut state = test_state(alone.path(), "clearance = \"00\"\n", only);
        state.clock = Clock(Arc::new(|| AFTER_RETIREMENT));
        assert_eq!(
            resolve_brief_assignment(&state, &brief("case-officer", None)).unwrap_err(),
            "no qualified Agent for role \"case-officer\"; 007 retired on 2027-01-31"
        );
        let mission = state
            .missions
            .lock()
            .unwrap()
            .get("mission-1")
            .cloned()
            .unwrap();
        assert!(
            !super::reassign_if_unavailable(&state, &mission),
            "its queued Mission waits"
        );
    }

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
