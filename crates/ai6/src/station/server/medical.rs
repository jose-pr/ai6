//! The Station's side of Medical: recording what Sessions consume and enforcing budgets.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use super::{DaemonState, queue_event, session_hold, sync_session_record};
use crate::directorates::medical::usage::{
    Budget, BudgetBreach, BudgetLevel, BudgetScope, MissionUsage, UsageTotals, UsageTracker,
};
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::section_00::agent::{AgentEvent, AgentSession, TokenUsage};

/// The `blocked_reason` of a Mission a budget stopped.
pub(super) const BUDGET_REASON: &str = "budget";

/// The event kind that reports a budget warning or stop.
pub(super) const BUDGET_EVENT: &str = "budget";

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
            sessions.remove(mission_id)
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
        telemetry.publish_lifecycle("blocked");
    }
    sync_session_record(state, mission_id);
}
