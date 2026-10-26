//! The Station's side of Medical: recording what Sessions consume and enforcing budgets.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use super::{DaemonState, queue_event, session_hold, sync_session_record};
use crate::directorates::m::mission::{Mission, MissionState};
use crate::directorates::medical::exposure::{
    BRIEF_EXPOSURE_KEY, BRIEF_SOURCE, Exposure, ExposureGate, tool_source,
};
use crate::directorates::medical::quarantine::{Incident, IncidentKind};
use crate::directorates::medical::usage::{
    Budget, BudgetBreach, BudgetLevel, BudgetScope, MissionUsage, UsageTotals, UsageTracker,
};
use crate::directorates::registry::quarantine::Quarantine;
use crate::directorates::registry::sessions::SessionUpdate;
use crate::directorates::registry::sources::SOURCE_BRIEF_PREFIX;
use crate::directorates::registry::store::{AuditRecord, Brief};
use crate::directorates::section_00::agent::{AgentEvent, AgentSession, TokenUsage};

/// The `blocked_reason` of a Mission a budget stopped.
pub(super) const BUDGET_REASON: &str = "budget";

/// The event kind that reports a budget warning or stop.
pub(super) const BUDGET_EVENT: &str = "budget";

/// The event kind that reports a Mission stopped because its Agent is quarantined.
pub(super) const QUARANTINED_EVENT: &str = "quarantined";

/// The event kind that reports a new way a Session was exposed; its text is the source.
pub(super) const EXPOSED_EVENT: &str = "exposed";

/// What one recorded usage report left behind.
pub(super) struct Recorded {
    /// The Mission's ledger after the addition.
    pub(super) mission: MissionUsage,
    /// The reporting Session's totals after the addition.
    pub(super) session: UsageTotals,
}

/// Adds what the provider reported to the Mission's ledger in the shared Registry.
///
/// `tracker` belongs to one attached provider process, so a resumed Session is never
/// counted twice. A ledger that cannot be written is logged and the work continues:
/// measuring never stops an Agent.
pub(super) fn record_usage(
    state: &DaemonState,
    mission_id: &str,
    tracker: &mut UsageTracker,
    reported: &TokenUsage,
) -> Option<Recorded> {
    let delta = tracker.observe(reported);
    super::accounts::record_usage(state, mission_id, delta.tokens());
    let operation_id = operation_of(state, mission_id);
    let session =
        session_hold(state, mission_id).map_or_else(|| mission_id.to_owned(), |hold| hold.id);
    match state.intake.record_usage(
        operation_id.as_deref(),
        mission_id,
        &session,
        &delta,
        state.clock.now(),
    ) {
        Ok(mission) => Some(Recorded {
            session: mission.sessions.get(&session).copied().unwrap_or_default(),
            mission,
        }),
        Err(error) => {
            eprintln!("ai6: usage of Mission {mission_id:?} was not recorded: {error}");
            None
        }
    }
}

fn operation_of(state: &DaemonState, mission_id: &str) -> Option<String> {
    state
        .missions
        .lock()
        .ok()?
        .get(mission_id)
        .and_then(|mission| mission.operation_id.clone())
}

/// The limits that apply to a Mission: its Agent's over its Operation's.
pub(super) fn effective_budget(state: &DaemonState, mission_id: &str) -> Budget {
    let Some(mission) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
    else {
        return Budget::default();
    };
    let operation = mission
        .operation_id
        .as_deref()
        .and_then(|id| state.roster.operation(id).ok())
        .map(|operation| operation.budget)
        .unwrap_or_default();
    let agent = mission
        .agent_id
        .as_deref()
        .and_then(|id| state.roster.agent(mission.operation_id.as_deref(), id).ok())
        .map(|agent| agent.budget)
        .unwrap_or_default();
    operation.merge(&agent)
}

/// The spend ceiling to hand a harness that can enforce one itself: what the Mission may
/// still spend, never zero (a Mission already at its limit is stopped, not started).
pub(super) fn remaining_mission_cost(state: &DaemonState, mission_id: &str) -> Option<u64> {
    let limit = effective_budget(state, mission_id).mission_cost_micro_usd?;
    let spent = operation_of(state, mission_id)
        .and_then(|operation| {
            state
                .intake
                .mission_usage(Some(&operation), mission_id)
                .ok()
                .flatten()
        })
        .map_or(0, |usage| usage.total.cost_micro_usd);
    Some(limit.saturating_sub(spent).max(1))
}

/// Checks a Mission's budget after new usage was recorded.
///
/// A warning is reported once per attached provider process (`warned` remembers which),
/// as a `budget` event and an audit record. Returns the description of the first limit
/// that was reached, which the caller answers by stopping the Session.
pub(super) fn enforce_budget(
    state: &DaemonState,
    mission_id: &str,
    recorded: &Recorded,
    warned: &mut BTreeSet<(BudgetScope, bool)>,
) -> Option<String> {
    let budget = effective_budget(state, mission_id);
    if !budget.is_limited() {
        return None;
    }
    let operation_id = operation_of(state, mission_id);
    let day_cost = budget.operation_cost_micro_usd_per_day.and_then(|_| {
        state
            .intake
            .operation_cost_on_day(operation_id.as_deref(), state.clock.now())
            .map_err(|error| eprintln!("ai6: Operation spend was not read: {error}"))
            .ok()
    });
    let mut stop = None;
    for breach in budget.check(&recorded.session, &recorded.mission.total, day_cost) {
        let at_limit = breach.level == BudgetLevel::Limit;
        if warned.insert((breach.scope, at_limit)) {
            let action = if at_limit {
                "budget.limit"
            } else {
                "budget.warn"
            };
            announce(state, mission_id, operation_id.as_deref(), &breach, action);
        }
        if at_limit {
            stop.get_or_insert_with(|| breach.to_string());
        }
    }
    stop
}

fn announce(
    state: &DaemonState,
    mission_id: &str,
    operation_id: Option<&str>,
    breach: &BudgetBreach,
    action: &str,
) {
    let text = breach.to_string();
    eprintln!("ai6: Mission {mission_id:?}: {text}");
    queue_event(
        state,
        mission_id,
        AgentEvent {
            kind: BUDGET_EVENT.to_owned(),
            text: Some(text),
            data: None,
            limit: None,
            usage: None,
            tools: Vec::new(),
        },
    );
    let record = AuditRecord {
        id: format!("{action}-{}", uuid::Uuid::new_v4().simple()),
        actor: format!("station:{}", state.station_id),
        action: action.to_owned(),
        operation: operation_id.unwrap_or_default().to_owned(),
        resource: operation_id
            .is_none()
            .then(|| format!("station:{}", state.station_id)),
        target: format!("{mission_id}:{}", breach.scope.key()),
        at: state.clock.now() as f64,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: budget audit record was not written: {error}");
    }
}

/// The exposure gate for a Mission's next Session: the Operation's policy, starting from
/// what the Mission came from and what any earlier Session of it read.
pub(super) fn open_gate(state: &DaemonState, mission: &Mission) -> ExposureGate {
    let policy = mission
        .operation_id
        .as_deref()
        .and_then(|id| state.roster.operation(id).ok())
        .map(|operation| operation.exposure.clone())
        .unwrap_or_default();
    let now = state.clock.now() as f64;
    let mut inherited = state
        .intake
        .mission_exposure(mission.mission_id.as_str())
        .unwrap_or_else(|error| {
            eprintln!(
                "ai6: earlier exposure of Mission {:?} unreadable: {error}",
                mission.mission_id.as_str()
            );
            Vec::new()
        });
    for source in &mission.exposure {
        if !inherited.iter().any(|known| &known.source == source) {
            inherited.push(Exposure {
                source: source.clone(),
                at: now,
            });
        }
    }
    ExposureGate::new(policy, inherited)
}

/// What a Mission made from `brief` has already read: the Source item's text, or the
/// exposure the proposing Session carried into the proposal.
pub(super) fn brief_exposure(brief: &Brief) -> Vec<String> {
    let mut sources = Vec::new();
    if brief.source.starts_with(SOURCE_BRIEF_PREFIX) {
        sources.push(BRIEF_SOURCE.to_owned());
    }
    if let Some(inherited) = brief
        .data
        .get(BRIEF_EXPOSURE_KEY)
        .and_then(serde_json::Value::as_array)
    {
        sources.extend(
            inherited
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter(|source| !source.is_empty() && source.len() <= 128)
                .map(str::to_owned),
        );
    }
    sources.truncate(32);
    sources
}

/// Marks the Session exposed by any tool in `event` the Operation lists.
pub(super) fn expose_by_tools(state: &DaemonState, gate: &ExposureGate, event: &AgentEvent) {
    for tool in &event.tools {
        if gate.policy().exposed_by_tool(tool) {
            gate.mark(&tool_source(tool), state.clock.now() as f64);
        }
    }
}

/// Writes exposure that changed to the Session's Registry record and announces each new
/// source as an `exposed` event. `announced` counts the sources already announced.
pub(super) fn sync_exposure(
    state: &DaemonState,
    mission_id: &str,
    gate: &ExposureGate,
    announced: &mut usize,
) {
    let Some(exposures) = gate.take_changes() else {
        return;
    };
    for exposure in exposures.iter().skip(*announced) {
        queue_event(
            state,
            mission_id,
            AgentEvent {
                kind: EXPOSED_EVENT.to_owned(),
                text: Some(exposure.source.clone()),
                data: None,
                limit: None,
                usage: None,
                tools: Vec::new(),
            },
        );
    }
    *announced = exposures.len();
    persist_exposure(state, mission_id, &exposures);
}

/// Adds `exposures` to the Mission's Session record, when this Station holds one.
pub(super) fn persist_exposure(state: &DaemonState, mission_id: &str, exposures: &[Exposure]) {
    if exposures.is_empty() {
        return;
    }
    let Some(hold) = session_hold(state, mission_id) else {
        return;
    };
    if let Err(error) = state.intake.update_session(
        &hold.id,
        &state.station_id,
        hold.fence,
        SessionUpdate::default().exposed(exposures.to_vec()),
    ) {
        eprintln!(
            "ai6: exposure of Session {:?} not recorded: {error}",
            hold.id
        );
    }
}

/// Stops a Session whose budget is spent and blocks its Mission with reason `budget`.
///
/// The safe house and provider Session stay as they are, so an operator who raises the
/// limit can resume the same work.
pub(super) fn stop_for_budget(
    state: &DaemonState,
    mission_id: &str,
    session: &Arc<Mutex<guatiao::library::Object<dyn AgentSession>>>,
) {
    let operation = state.sessions.lock().ok().and_then(|mut sessions| {
        if sessions
            .get(mission_id)
            .is_some_and(|operation| Arc::ptr_eq(&operation.session, session))
        {
            sessions
                .remove(mission_id)
                .inspect(|operation| operation.gate.close())
        } else {
            None
        }
    });
    let Some(operation) = operation else {
        return;
    };
    if let Ok(session) = operation.session.lock() {
        let _ = session.interrupt();
        let _ = session.close();
    }
    let blocked = state
        .missions
        .lock()
        .ok()
        .is_some_and(|mut missions| missions.block_for(mission_id, BUDGET_REASON).is_ok());
    if !blocked {
        return;
    }
    #[cfg(feature = "publisher")]
    if let Some(telemetry) = &operation.telemetry {
        telemetry.publish_lifecycle_for("blocked", Some(BUDGET_REASON));
    }
    sync_session_record(state, mission_id);
    report_incident(state, mission_id, IncidentKind::Budget);
}

/// Whether `agent` may not work in `operation` because it is quarantined.
///
/// An unreadable quarantine list counts as quarantined: work that cannot be shown to be
/// allowed does not start.
pub(super) fn is_quarantined(state: &DaemonState, agent: &str, operation: Option<&str>) -> bool {
    state
        .intake
        .is_quarantined(agent, operation)
        .unwrap_or_else(|error| {
            eprintln!(
                "ai6: quarantine list unreadable, treating {agent:?} as quarantined: {error}"
            );
            true
        })
}

/// The Agents quarantined for `operation`, for the roster to skip.
pub(super) fn quarantined_agents(state: &DaemonState, operation: Option<&str>) -> Vec<String> {
    state
        .intake
        .quarantines()
        .unwrap_or_default()
        .into_iter()
        .filter(|record| record.covers(&record.agent, operation))
        .map(|record| record.agent)
        .collect()
}

/// Records an incident against the Mission's Agent and quarantines the Agent when its
/// Operation's policy says its recent history warrants it.
pub(super) fn report_incident(state: &DaemonState, mission_id: &str, kind: IncidentKind) {
    let Some(mission) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
    else {
        return;
    };
    let Some(agent) = mission.agent_id.clone() else {
        return;
    };
    let policy = mission
        .operation_id
        .as_deref()
        .and_then(|id| state.roster.operation(id).ok())
        .map(|operation| operation.quarantine)
        .unwrap_or_default();
    let now = state.clock.now() as f64;
    let incident = Incident {
        kind,
        at: now,
        operation: mission.operation_id.clone(),
        mission: mission_id.to_owned(),
    };
    let mut history = match state.intake.record_incident(&agent, incident) {
        Ok(history) => history,
        Err(error) => {
            eprintln!("ai6: incident of Agent {agent:?} was not recorded: {error}");
            return;
        }
    };
    // An Operation's policy judges what happened in that Operation.
    history.retain(|known| known.operation == mission.operation_id);
    if let Some(reason) = policy.reason(&history, now) {
        quarantine_agent(
            state,
            &agent,
            mission.operation_id.as_deref(),
            &reason,
            &format!("station:{}", state.station_id),
        );
    }
}

/// Quarantines an Agent in the Registry and stops its work on this Station.
pub(super) fn quarantine_agent(
    state: &DaemonState,
    agent: &str,
    operation: Option<&str>,
    reason: &str,
    by: &str,
) {
    let record = Quarantine::new(
        agent,
        operation.map(str::to_owned),
        reason,
        by,
        state.clock.now() as f64,
    );
    match state.intake.quarantine_agent(&record) {
        Ok(_) => {
            eprintln!("ai6: Agent {agent:?} is quarantined: {reason}");
            let audit = AuditRecord {
                id: format!("agent.quarantine-{}", uuid::Uuid::new_v4().simple()),
                actor: by.to_owned(),
                action: "agent.quarantine".to_owned(),
                operation: operation.unwrap_or_default().to_owned(),
                resource: operation
                    .is_none()
                    .then(|| format!("station:{}", state.station_id)),
                target: agent.to_owned(),
                at: record.at,
            };
            if let Err(error) = state.intake.record_audit(&audit) {
                eprintln!("ai6: quarantine audit record was not written: {error}");
            }
        }
        Err(error) => eprintln!("ai6: Agent {agent:?} was not quarantined: {error}"),
    }
    enforce_quarantines(state);
}

/// Brings this Station's Missions in line with the Registry's quarantine list: the work of
/// a quarantined Agent stops, and a Mission whose Agent was released goes back to the queue,
/// with the next qualified Agent when there is one.
pub(super) fn enforce_quarantines(state: &DaemonState) {
    let records = match state.intake.quarantines() {
        Ok(records) => records,
        Err(error) => {
            eprintln!("ai6: quarantine list unreadable: {error}");
            return;
        }
    };
    let missions = state
        .missions
        .lock()
        .map(|missions| missions.list())
        .unwrap_or_default();
    for mission in missions {
        let Some(agent) = mission.agent_id.as_deref() else {
            continue;
        };
        let covered = records
            .iter()
            .any(|record| record.covers(agent, mission.operation_id.as_deref()));
        match (covered, mission.state) {
            (
                true,
                MissionState::Working
                | MissionState::Blocked
                | MissionState::Stalled
                | MissionState::Parked,
            ) => stop_for_quarantine(state, &mission, &records),
            (false, MissionState::Quarantined) => release_mission(state, &mission),
            _ => {}
        }
    }
}

fn stop_for_quarantine(state: &DaemonState, mission: &Mission, records: &[Quarantine]) {
    let mission_id = mission.mission_id.as_str();
    let operation = state
        .sessions
        .lock()
        .ok()
        .and_then(|mut sessions| sessions.remove(mission_id))
        .inspect(|operation| operation.gate.close());
    if let Some(operation) = &operation
        && let Ok(session) = operation.session.lock()
    {
        let _ = session.interrupt();
        let _ = session.close();
    }
    let quarantined = state
        .missions
        .lock()
        .ok()
        .is_some_and(|mut missions| missions.quarantine(mission_id).is_ok());
    if !quarantined {
        return;
    }
    #[cfg(feature = "publisher")]
    if let Some(telemetry) = operation
        .as_ref()
        .and_then(|operation| operation.telemetry.as_ref())
    {
        telemetry.publish_lifecycle("quarantined");
    }
    sync_session_record(state, mission_id);
    let reason = records
        .iter()
        .find(|record| {
            mission
                .agent_id
                .as_deref()
                .is_some_and(|agent| record.covers(agent, mission.operation_id.as_deref()))
        })
        .map_or("quarantined", |record| record.reason.as_str());
    queue_event(
        state,
        mission_id,
        AgentEvent {
            kind: QUARANTINED_EVENT.to_owned(),
            text: Some(format!("Agent quarantined: {reason}")),
            data: None,
            limit: None,
            usage: None,
            tools: Vec::new(),
        },
    );
}

fn release_mission(state: &DaemonState, mission: &Mission) {
    let Some(agent) = mission.agent_id.clone() else {
        return;
    };
    let handlers = super::available_handlers(state);
    let backup = state
        .roster
        .select_agent_excluding(
            mission.operation_id.as_deref(),
            mission.role_id.as_deref(),
            &[],
            Some(&handlers),
            std::slice::from_ref(&agent),
        )
        .map_or(agent, |backup| backup.id);
    let requeued = state.missions.lock().ok().is_some_and(|mut missions| {
        missions
            .requeue_for(mission.mission_id.as_str(), backup)
            .is_ok()
    });
    if requeued {
        sync_session_record(state, mission.mission_id.as_str());
    }
}

/// Gives a queued Mission whose Agent is quarantined to the next qualified Agent.
///
/// Returns `false` when it must wait: its Agent is quarantined and no other can take it.
pub(super) fn reassign_if_quarantined(state: &DaemonState, mission: &Mission) -> bool {
    let Some(agent) = mission.agent_id.as_deref() else {
        return true;
    };
    if !is_quarantined(state, agent, mission.operation_id.as_deref()) {
        return true;
    }
    let handlers = super::available_handlers(state);
    let excluded = quarantined_agents(state, mission.operation_id.as_deref());
    let Ok(backup) = state.roster.select_agent_excluding(
        mission.operation_id.as_deref(),
        mission.role_id.as_deref(),
        &[],
        Some(&handlers),
        &excluded,
    ) else {
        return false;
    };
    state.missions.lock().ok().is_some_and(|mut missions| {
        missions
            .reassign(mission.mission_id.as_str(), backup.id)
            .is_ok()
    })
}
