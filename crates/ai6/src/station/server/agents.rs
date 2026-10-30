//! The Station's side of the roster lifecycle: choosing Agents that are neither quarantined
//! nor retired, the clearance each works at, and counting each Mission's outcome and each
//! incident charged to its Agent.

use super::promotion::{PROMOTION_SESSION, promotion_mission, promotion_target};
use super::{DaemonState, medical};
use crate::directorates::m::mission::{Mission, MissionState};
use crate::directorates::m::promotion::CLEARANCE_KIND;
use crate::directorates::m::roster::{Agent, AgentQuery, Retirement, Roster, RosterError};
use crate::directorates::registry::agent_records::{AgentOutcome, AgentOutcomeReport};
use crate::directorates::registry::approvals::{ApprovalStatus, Decision, EXPIRY_SIGNER};
use crate::directorates::registry::clearance::ClearanceGrant;
use crate::directorates::registry::timeline::TimelineEvent;

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
///
/// A grant that cannot be read counts as one holding the Agent at `desk`, and a raise its
/// approval does not back counts for nothing: a clearance that cannot be shown is not assumed.
pub(super) fn effective(
    state: &DaemonState,
    operation: Option<&str>,
    key: &str,
) -> Result<Agent, RosterError> {
    let mut agent = state.roster.agent(operation, key)?;
    if let Some(operation) = operation {
        let operation = state.roster.operation(operation)?;
        let now = state.clock.now();
        let grant = working_grant(state, &operation.id, &agent.id);
        agent.clearance = agent.effective_clearance(operation, grant.as_ref(), now);
    }
    Ok(agent)
}

/// The grant the Station works the Agent under in `operation`: its grant when it can be read
/// and, for a raise, is backed by its approval ([`is_backed`]); a `desk` lower when it cannot
/// be read; `None` otherwise.
fn working_grant(state: &DaemonState, operation: &str, agent: &str) -> Option<ClearanceGrant> {
    match grant_of(state, operation, agent) {
        Some(grant) => grant.filter(|grant| is_backed(state, grant)),
        None => Some(unreadable(state, operation, agent)),
    }
}

/// The grant an unreadable one counts as: the Agent held at `desk`.
fn unreadable(state: &DaemonState, operation: &str, agent: &str) -> ClearanceGrant {
    ClearanceGrant::lower(
        agent,
        operation,
        "desk",
        format!("station:{}", state.station_id),
        state.clock.now() as f64,
    )
    .because("grant unreadable")
}

/// Whether a grant may be applied: a lower always; a raise only when its approval is M's
/// promotion request for exactly this Agent, clearance and Operation, approved by a person
/// (not a class authorisation, not an expiry), spent, and not revoked.
pub(super) fn is_backed(state: &DaemonState, grant: &ClearanceGrant) -> bool {
    let Some(id) = &grant.approval else {
        return true;
    };
    let now = state.clock.now() as f64;
    let Ok(Some(ApprovalStatus::Decided(approval))) =
        state.intake.approval(&grant.operation, id, now)
    else {
        return false;
    };
    let request = &approval.request;
    approval.decision == Decision::Approved
        && approval.revoked.is_none()
        && approval.spent_at.is_some()
        && approval.signer != EXPIRY_SIGNER
        && !approval.signer.starts_with("class:")
        && request.id == *id
        && request.agent == grant.agent
        && request.operation == grant.operation
        && request.session.as_str() == PROMOTION_SESSION
        && request.mission.as_str() == promotion_mission(&grant.agent)
        && request.scope.kind == CLEARANCE_KIND
        && request.scope.target
            == promotion_target(&grant.agent, &grant.clearance, &grant.operation)
}

/// The Agent's grant in `operation`; `None` (logged) when it cannot be read. A codename that
/// is not a Registry key has no grant.
pub(super) fn grant_of(
    state: &DaemonState,
    operation: &str,
    agent: &str,
) -> Option<Option<ClearanceGrant>> {
    match state.intake.clearance_grant(operation, agent) {
        Ok(grant) => Some(grant),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Some(None),
        Err(error) => {
            eprintln!(
                "ai6: clearance grant of Agent {agent:?} in {operation:?} unreadable: {error}"
            );
            None
        }
    }
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
        .ok()
        .and_then(|agent| retired(state, &agent));
    let unavailable = retired.or_else(|| {
        medical::is_quarantined(state, agent, operation)
            .then(|| format!("Agent {agent:?} is quarantined"))
    });
    let Some(unavailable) = unavailable else {
        if mission.waiting.is_some() {
            note_waiting(state, mission, None);
        }
        return true;
    };
    let handlers = super::available_handlers(state);
    let Ok(backup) = select(
        state,
        operation,
        mission.role_id.as_deref(),
        &[],
        Some(&handlers),
        &[],
    ) else {
        note_waiting(
            state,
            mission,
            Some(format!(
                "{unavailable}; no other qualified Agent can take it"
            )),
        );
        return false;
    };
    state.missions.lock().ok().is_some_and(|mut missions| {
        missions
            .reassign(mission.mission_id.as_str(), backup.id)
            .is_ok()
    })
}

/// Records on the Mission why it waits, and tells the timeline once per new reason.
fn note_waiting(state: &DaemonState, mission: &Mission, reason: Option<String>) {
    let mission_id = mission.mission_id.as_str();
    let changed = state
        .missions
        .lock()
        .ok()
        .and_then(|mut missions| missions.set_waiting(mission_id, reason.clone()).ok())
        .unwrap_or(false);
    let (true, Some(reason), Some(operation)) = (changed, reason, &mission.operation_id) else {
        return;
    };
    eprintln!("ai6: Mission {mission_id:?} waits: {reason}");
    let event = TimelineEvent::new("mission", "waiting", operation, state.clock.now() as f64)
        .station(state.station_id.clone())
        .mission(mission_id)
        .detail("reason", reason);
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: timeline event of Mission {mission_id:?} was not written: {error}");
    }
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

/// The clearance each posted Agent works at in each Operation now, beside its configured
/// one, with its aliases, for the check-in: `{OPERATION: {AGENT: {clearance, configured,
/// aliases, unreadable?}}}`. A grant that cannot be read shows `desk` and `unreadable: true`,
/// as the Station works the Agent.
pub(super) fn clearances(state: &DaemonState) -> serde_json::Value {
    let now = state.clock.now();
    let mut operations = serde_json::Map::new();
    for operation in state.roster.operations() {
        let posted = state.roster.agents_for_operation(&operation.id);
        let mut agents = serde_json::Map::new();
        for agent in posted.unwrap_or_default() {
            let read = grant_of(state, &operation.id, &agent.id);
            let grant = match &read {
                Some(grant) => grant.clone().filter(|grant| is_backed(state, grant)),
                None => Some(unreadable(state, &operation.id, &agent.id)),
            };
            let mut entry = serde_json::json!({
                "clearance": agent.effective_clearance(operation, grant.as_ref(), now),
                "configured": agent.clearance,
                "aliases": agent.aliases,
            });
            if read.is_none() {
                entry["unreadable"] = serde_json::json!(true);
            }
            agents.insert(agent.id.clone(), entry);
        }
        operations.insert(operation.id.clone(), serde_json::Value::Object(agents));
    }
    serde_json::Value::Object(operations)
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

/// The Mission and the model its Session was launched with.
fn launched(state: &DaemonState, mission_id: &str) -> Option<(Mission, Option<String>)> {
    let missions = state.missions.lock().ok()?;
    let mission = missions.get(mission_id).cloned()?;
    let model = missions
        .operation(mission_id)
        .and_then(|manifest| manifest.model);
    Some((mission, model))
}

/// Counts the Mission's state into its Agent's record when it is a finished or stalled one,
/// for Session `attempt`, which this Station holds. The Mission, the attempt and the outcome
/// name the event, so a state seen twice, or by the Station that adopted the Session, counts
/// once. Chief of Staff reviews are not the Agent's own work and are not counted.
pub(super) fn count_mission(state: &DaemonState, mission_id: &str, attempt: u32) {
    let Some((mission, launched)) = launched(state, mission_id) else {
        return;
    };
    if super::vetting::is_review_mission(&mission) {
        return;
    }
    let (Some(outcome), Some(agent)) = (mission_outcome(mission.state), &mission.agent_id) else {
        return;
    };
    count(
        state,
        agent,
        mission.operation_id.as_deref(),
        outcome,
        format!("{mission_id}#{attempt}#{}", outcome.as_str()),
        launched,
    );
}

/// Counts Session `attempt` of a Mission as lost with its Station or runner: the
/// infrastructure's doing, never charged as the Agent's failure, even when the Mission fails.
pub(super) fn count_lost(state: &DaemonState, mission_id: &str, attempt: u32) {
    let Some((mission, launched)) = launched(state, mission_id) else {
        return;
    };
    let Some(agent) = &mission.agent_id else {
        return;
    };
    if super::vetting::is_review_mission(&mission) {
        return;
    }
    count(
        state,
        agent,
        mission.operation_id.as_deref(),
        AgentOutcome::Lost,
        format!("{mission_id}#{attempt}#lost"),
        launched,
    );
}

/// Counts `outcome` for `agent` in `operation`, at the clearance it works at, for a Session
/// launched with `model` (`None`: the handler's default); `key` names the event. A record that
/// cannot be written is logged: counting never stops work.
pub(super) fn count(
    state: &DaemonState,
    agent: &str,
    operation: Option<&str>,
    outcome: AgentOutcome,
    key: String,
    model: Option<String>,
) {
    let mut report = AgentOutcomeReport::new(
        agent,
        operation.map(str::to_owned),
        outcome,
        key,
        state.clock.now() as f64,
    )
    .with_model(model);
    if let Ok(working) = effective(state, operation, agent) {
        report = report.at_clearance(working.clearance);
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
    use crate::directorates::m::mission::{LaunchManifest, MissionState, SessionHold};
    use crate::directorates::registry::sessions::NewSession;
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
    fn a_brief_for_an_operation_with_nowhere_to_work_is_refused_naming_it() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "parent = \"P\"\n[operation.P]\n", RETIRING);
        let for_parent: Brief = serde_json::from_value(serde_json::json!({
            "title": "Inspect", "id": "b-2", "operation": "P", "role": "case-officer",
        }))
        .unwrap();
        assert_eq!(
            resolve_brief_assignment(&state, &for_parent).unwrap_err(),
            "Operation \"P\" has nowhere to work"
        );
        assert_eq!(state.roster.parent("G"), Some("P"));
        assert!(resolve_brief_assignment(&state, &brief("case-officer", None)).is_ok());
    }

    #[test]
    fn a_check_in_carries_every_operations_lineage() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(
            root.path(),
            "parent = \"P\"\n[operation.P]\nparent = \"ROOT\"\n[operation.ROOT]\n",
            RETIRING,
        );
        super::super::publish_station_check_in(&state);
        let stations = state.intake.stations(std::time::Duration::MAX).unwrap();
        assert_eq!(
            stations[0].info()["operation_lineage"],
            serde_json::json!({"G": ["P", "ROOT"], "P": ["ROOT"], "ROOT": []})
        );
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
        let waiting = state
            .missions
            .lock()
            .unwrap()
            .get("mission-1")
            .cloned()
            .unwrap();
        assert_eq!(
            waiting.waiting.as_deref(),
            Some("Agent \"007\" retired on 2027-01-31; no other qualified Agent can take it")
        );
        assert!(!super::reassign_if_unavailable(&state, &waiting));
        let told: Vec<_> = state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "mission" && event.action == "waiting")
            .collect();
        assert_eq!(told.len(), 1, "told once per reason");
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

    /// Gives `mission` a Session record held by this Station, attempt `attempt`, launched with
    /// `model`.
    fn launch(state: &super::DaemonState, mission: &str, model: &str, attempt: u32) {
        let record = state
            .intake
            .open_session(
                NewSession::new(format!("s-{mission}-{attempt}"), mission, "test")
                    .with_attempt(attempt, None),
                std::time::Duration::from_secs(600),
            )
            .unwrap();
        let manifest = LaunchManifest {
            handler_provider: "mock-agent".to_owned(),
            terminal_provider: "mock-terminal".to_owned(),
            working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
            model: Some(model.to_owned()),
            effort: None,
            cover: None,
            station_id: Some("test".to_owned()),
            theater_id: None,
            mode: Default::default(),
            session: Some(SessionHold {
                id: record.session_id,
                attempt: record.attempt,
                fence: record.fence,
            }),
            runner: None,
        };
        state
            .missions
            .lock()
            .unwrap()
            .set_operation(mission, manifest)
            .unwrap();
    }

    fn create(state: &super::DaemonState, mission: &str, role: Option<&str>) {
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(mission).unwrap(),
                Some("G".to_owned()),
                "more work".to_owned(),
                Some("007".to_owned()),
                role.map(str::to_owned),
            )
            .unwrap();
    }

    fn tally(
        state: &super::DaemonState,
    ) -> crate::directorates::registry::agent_records::OperationRecord {
        state
            .intake
            .agent_record("007")
            .unwrap()
            .unwrap_or_else(|| {
                crate::directorates::registry::agent_records::AgentRecord::new("007")
            })
            .in_operation("G")
    }

    #[test]
    fn a_held_session_counts_once_per_attempt_and_outcome_under_its_launch_model() {
        let root = tempfile::tempdir().unwrap();
        // The roster now names sonnet; the Sessions were launched with opus.
        let state = test_state(root.path(), "", &AGENTS.replace("opus", "sonnet"));
        launch(&state, "mission-1", "opus", 1);
        work(&state, "mission-1");
        apply_event_state(&state, "mission-1", &event("completed"));
        launch(&state, "mission-2", "opus", 1);
        work(&state, "mission-2");
        apply_event_state(&state, "mission-2", &event("stalled"));
        apply_event_state(&state, "mission-2", &event("completed"));
        super::count_mission(&state, "mission-2", 1);

        let record = tally(&state);
        assert_eq!(
            (record.tally.done, record.tally.stalled, record.tally.failed),
            (2, 1, 0)
        );
        assert_eq!(record.by_clearance["field"].done, 2);
        assert!(record.last_mission_at.is_some());
        let models: Vec<_> = record
            .model_history
            .iter()
            .map(|tenure| tenure.model.as_deref())
            .collect();
        assert_eq!(models, [Some("opus")], "the launch record names the model");

        // A Mission this Station holds no Session for, or no longer holds, counts nothing.
        create(&state, "mission-3", None);
        work(&state, "mission-3");
        apply_event_state(&state, "mission-3", &event("completed"));
        create(&state, "mission-4", None);
        launch(&state, "mission-4", "opus", 1);
        {
            let mut missions = state.missions.lock().unwrap();
            let mut manifest = missions.operation("mission-4").unwrap();
            let hold = manifest.session.as_mut().unwrap();
            hold.fence += 1;
            missions.set_operation("mission-4", manifest).unwrap();
        }
        work(&state, "mission-4");
        apply_event_state(&state, "mission-4", &event("completed"));
        assert_eq!(
            tally(&state).tally.done,
            2,
            "fenced out or unheld: not counted"
        );

        // The next occupant's Sessions open a second tenure.
        create(&state, "mission-5", None);
        launch(&state, "mission-5", "sonnet", 1);
        work(&state, "mission-5");
        apply_event_state(&state, "mission-5", &event("completed"));
        let record = tally(&state);
        assert_eq!(record.tally.done, 3);
        assert_eq!(record.tally_of_model(Some("sonnet")).done, 1);
    }

    #[test]
    fn an_unreadable_grant_is_desk_in_the_check_in_as_at_work() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        let grants = root.path().join("registry/clearance-grants/G");
        std::fs::create_dir_all(&grants).unwrap();
        std::fs::write(grants.join("007.json"), b"{ not a grant").unwrap();
        assert_eq!(
            super::effective(&state, Some("G"), "007")
                .unwrap()
                .clearance,
            "desk"
        );
        let shown = &super::clearances(&state)["G"]["007"];
        assert_eq!(shown["clearance"], "desk");
        assert_eq!(shown["configured"], "field");
        assert_eq!(shown["unreadable"], true);
    }

    #[test]
    fn a_lost_session_is_not_the_agents_failure_and_reviews_do_not_count() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        launch(&state, "mission-1", "opus", 2);
        work(&state, "mission-1");
        super::super::recovery::give_up(&state, "mission-1", None, "runner gone");
        let failed = state
            .missions
            .lock()
            .unwrap()
            .get("mission-1")
            .cloned()
            .unwrap();
        assert_eq!(
            failed.state,
            MissionState::Failed,
            "no brief to queue again"
        );
        let record = tally(&state);
        assert_eq!((record.tally.lost, record.tally.failed), (1, 0));
        assert_eq!(record.tally.failure_rate(), None);

        let review = format!("{}--b-1", crate::directorates::m::vetting::REVIEW_SOURCE);
        create(
            &state,
            &review,
            Some(crate::directorates::m::vetting::CHIEF_OF_STAFF_ROLE),
        );
        launch(&state, &review, "opus", 1);
        work(&state, &review);
        apply_event_state(&state, &review, &event("completed"));
        assert_eq!(
            tally(&state).tally.done,
            0,
            "a review is not the Agent's own work"
        );
    }
}
