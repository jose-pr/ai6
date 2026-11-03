//! M's round over the scorecard, at Headquarters: proposals a person decides, and holds.
//!
//! Each round, for every Operation that lets M propose, M reads the scorecard against the
//! Agents posted there ([`crate::directorates::m::stewardship::propose`]) and files each
//! proposal as a green ink request of kind `hold` or `prefer` under Session `m`, with the numbers
//! it rests on in the reason. Filing again finds the same request, so there is one per step until
//! the sample has grown by a small sample more. A `hold` a person signs becomes a hold, in the
//! person's name; a `prefer` is advice the person carries out in the roster, and signing it only
//! closes the request. With `auto_hold` M puts a hold on by itself, audited, unless it would leave
//! a role with no Agent, which is proposed instead. Nothing here grants a role, a clearance or an
//! account, and no Session already running is touched.

use super::{DaemonState, agents, medical};
use crate::bounds::Limit;
use crate::directorates::m::approvals::ActionScope;
use crate::directorates::m::placement::AgentFacts;
use crate::directorates::m::roster::{Operation, RouteRef};
use crate::directorates::m::scorecard::from_registry;
use crate::directorates::m::stewardship::{Proposal, Step, propose, roles_left_empty};
use crate::directorates::registry::approvals::{
    ApprovalRequest, ApprovalStatus, Decision, EXPIRY_SIGNER,
};
use crate::directorates::registry::holds::{AUTOMATIC, RosterHold};
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::{MissionId, SessionId};

/// The Session M files proposals under.
const SESSION: &str = "m";
const TIMELINE_KIND: &str = "hold";

/// The Agents of `operation` held whole, for selection to pass over.
pub(super) fn held_agents(state: &DaemonState, operation: Option<&str>) -> Vec<String> {
    let Some(operation) = operation else {
        return Vec::new();
    };
    state
        .intake
        .holds(operation)
        .unwrap_or_default()
        .into_iter()
        .filter(RosterHold::holds_agent)
        .map(|hold| hold.agent)
        .collect()
}

/// The hold that keeps `route` of `agent` out of use in `operation`, if one does.
pub(super) fn hold_on(
    state: &DaemonState,
    operation: Option<&str>,
    agent: &str,
    route: &RouteRef,
) -> Option<RosterHold> {
    state
        .intake
        .holds(operation?)
        .ok()?
        .into_iter()
        .find(|hold| hold.agent == agent && hold.covers(route))
}

/// The Mission a proposal is filed under.
fn mission_of(kind: &str, agent: &str) -> String {
    format!("{kind}-{agent}")
}

/// One round over every Operation of the roster. Runs on the Headquarters Station.
pub(super) fn round(state: &DaemonState) {
    for operation in state.roster.operations() {
        if operation.stewardship.propose {
            consider(state, operation);
        }
    }
}

fn consider(state: &DaemonState, operation: &Operation) {
    let now = state.clock.now() as f64;
    let policy = &operation.stewardship;
    let card = match from_registry(
        state.intake.as_ref(),
        &operation.id,
        &operation.scorecard,
        now,
    ) {
        Ok(card) => card,
        Err(error) => {
            eprintln!(
                "ai6: scorecard of {:?} unreadable; no proposals: {error}",
                operation.id
            );
            return;
        }
    };
    let Ok(posted) = state.roster.agents_for_operation(&operation.id) else {
        return;
    };
    let facts: Vec<AgentFacts> = posted.iter().map(AgentFacts::from).collect();
    let held = state.intake.holds(&operation.id).unwrap_or_default();
    for proposal in propose(&card, &facts, &held, policy) {
        if policy.auto_hold
            && let Step::Hold { agent, route } = &proposal.step
        {
            // Read again: a hold put on earlier in this round counts.
            let held = state.intake.holds(&operation.id).unwrap_or_default();
            let empty = roles_left_empty(
                &facts,
                agent,
                route.as_ref(),
                &unavailable(state, operation, &held),
                &held,
            );
            if empty.is_empty() {
                let reason = format!("{} Held by M.", proposal.reason);
                apply(
                    state,
                    operation,
                    agent,
                    route.as_ref(),
                    &reason,
                    AUTOMATIC,
                    None,
                );
                continue;
            }
            let reason = format!(
                "{} A hold by M would leave {} with no Agent, so it is proposed.",
                proposal.reason,
                empty.join(", ")
            );
            file(state, operation, &Proposal { reason, ..proposal }, &facts);
            continue;
        }
        file(state, operation, &proposal, &facts);
    }
}

/// Codenames selection already passes over in `operation`.
fn unavailable(state: &DaemonState, operation: &Operation, held: &[RosterHold]) -> Vec<String> {
    let mut passed = medical::quarantined_agents(state, Some(&operation.id));
    passed.extend(agents::retired_agents(state, &operation.id));
    passed.extend(
        held.iter()
            .filter(|hold| hold.holds_agent())
            .map(|hold| hold.agent.clone()),
    );
    passed
}

/// Cuts `text` to the longest prefix `limit` allows, on a character boundary.
fn within(text: &str, limit: Limit) -> String {
    if limit.allows(text.len()) {
        return text.to_owned();
    }
    let mut end = limit.or_max().min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn file(state: &DaemonState, operation: &Operation, proposal: &Proposal, _facts: &[AgentFacts]) {
    let agent = match &proposal.step {
        Step::Hold { agent, .. } | Step::Prefer { agent, .. } => agent,
    };
    let kind = proposal.kind();
    let round = proposal.judged / operation.scorecard.small_sample.max(1);
    let limits = state.intake.limits().approvals.clone();
    let invalid = |_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid identity");
    let request = (|| {
        ApprovalRequest::try_new(
            operation.id.clone(),
            MissionId::try_new(mission_of(kind, agent)).map_err(invalid)?,
            SessionId::try_new(SESSION).map_err(invalid)?,
            agent.as_str(),
            ActionScope::try_new(kind, &proposal.target)?,
            within(&proposal.reason, limits.reason_bytes),
            &format!("{}#{round}", proposal.target),
            &limits,
        )
    })();
    let request = match request {
        Ok(request) => {
            request.expiring(state.clock.now() as f64, operation.green_ink.request_ttl())
        }
        Err(error) => {
            eprintln!("ai6: proposal for Agent {agent:?} not filed: {error}");
            return;
        }
    };
    match state.intake.request_approval(&request) {
        Ok(ApprovalStatus::Pending(stored)) => {
            if stored.requested_at == request.requested_at {
                eprintln!(
                    "ai6: proposes {kind} for Agent {agent:?} in {:?}: green ink {}",
                    operation.id, stored.id
                );
                event(state, &stored, "proposed", AUTOMATIC);
                audit(
                    state,
                    "agent.hold.proposed",
                    AUTOMATIC,
                    &stored.operation,
                    &stored.scope.target,
                    state.clock.now() as f64,
                );
            }
        }
        Ok(ApprovalStatus::Decided(approval)) => {
            let now = state.clock.now() as f64;
            let by_a_person = approval.decision == Decision::Approved
                && approval.request.session.as_str() == SESSION
                && approval.request.scope.kind == kind
                && approval.request.scope.target == proposal.target
                && approval.signer != EXPIRY_SIGNER
                && !approval.signer.starts_with("class:");
            if !by_a_person || !approval.is_live(now) || approval.spent_at.is_some() {
                return;
            }
            match state.intake.spend_approval(
                &approval.request.operation,
                &approval.request.id,
                now,
            ) {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => {
                    eprintln!("ai6: approval {:?} not spent: {error}", approval.request.id);
                    return;
                }
            }
            match &proposal.step {
                Step::Hold { agent, route } => apply(
                    state,
                    operation,
                    agent,
                    route.as_ref(),
                    &proposal.reason,
                    &approval.signer,
                    Some(&approval.request.id),
                ),
                Step::Prefer { .. } => {
                    event(state, &approval.request, "acknowledged", &approval.signer);
                }
            }
        }
        Err(error) => eprintln!("ai6: proposal for Agent {agent:?} not filed: {error}"),
    }
}

/// Puts a hold on, in `by`'s name, and says so on the audit log and the timeline.
fn apply(
    state: &DaemonState,
    operation: &Operation,
    agent: &str,
    route: Option<&RouteRef>,
    reason: &str,
    by: &str,
    approval: Option<&str>,
) {
    let limits = state.intake.limits().holds.clone();
    let now = state.clock.now() as f64;
    let mut hold = RosterHold::new(
        operation.id.clone(),
        agent,
        route.cloned(),
        within(reason, limits.text_bytes),
        by,
        now,
    );
    if let Some(approval) = approval {
        hold = hold.signed_through(approval);
    }
    if let Err(error) = state.intake.put_hold(&hold) {
        eprintln!("ai6: hold on Agent {agent:?} not put on: {error}");
        return;
    }
    eprintln!(
        "ai6: Agent {agent:?} held in {:?}{}: {reason}",
        operation.id,
        route
            .map(|route| format!(" on {route}"))
            .unwrap_or_default()
    );
    let target = crate::directorates::m::stewardship::hold_target(agent, route, &operation.id);
    audit(state, "agent.hold.applied", by, &operation.id, &target, now);
    let event = TimelineEvent::new(TIMELINE_KIND, "applied", &operation.id, now)
        .actor(by)
        .detail("agent", agent)
        .detail("target", target);
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: hold timeline event was not written: {error}");
    }
}

fn event(state: &DaemonState, request: &ApprovalRequest, action: &str, actor: &str) {
    let event = TimelineEvent::new(
        TIMELINE_KIND,
        action,
        &request.operation,
        state.clock.now() as f64,
    )
    .actor(actor)
    .detail("agent", &request.agent)
    .detail("target", &request.scope.target)
    .detail("request", &request.id);
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: hold timeline event was not written: {error}");
    }
}

fn audit(state: &DaemonState, action: &str, actor: &str, operation: &str, target: &str, at: f64) {
    let record = AuditRecord {
        id: format!("{action}-{}", uuid::Uuid::new_v4().simple()),
        actor: actor.to_owned(),
        action: action.to_owned(),
        operation: operation.to_owned(),
        resource: None,
        target: target.to_owned(),
        at,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: hold audit record {action} was not written: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_state;
    use super::*;
    use crate::directorates::m::stewardship::{HOLD_KIND, PREFER_KIND};
    use crate::directorates::registry::approvals::Ruling;
    use crate::directorates::registry::sessions::{
        NewSession, SessionLaunch, SessionResult, SessionStatus, SessionUpdate,
    };
    use std::time::Duration;

    const SETTINGS: &str = "clearance = \"00\"\n";
    const AGENTS: &str = concat!(
        "[role.case-officer]\npurpose = \"Runs cases\"\npolicy = \"Follow the Operation policy\"\n",
        "[agent.007]\nhandler = \"mock-agent\"\nclearance = \"field\"\nroles = [\"case-officer\"]\npriority = 5\n",
        "[agent.008]\nhandler = \"mock-agent\"\nclearance = \"field\"\nroles = [\"case-officer\"]\npriority = 1\n",
        "[agent.fable]\nhandler = \"mock-agent\"\nmodel = \"claude-fable-5-1\"\nclearance = \"field\"\nroles = []\n",
    );

    /// The Operation's settings, with `rules` under `[operation.G.scorecard]`.
    fn settings(rules: &str) -> String {
        format!("{SETTINGS}[operation.G.scorecard]\nsmall_sample = 3\n{rules}\n")
    }

    /// Ends `n` attempts of `agent` in `state` ("done" or "failed").
    fn attempts(state: &DaemonState, agent: &str, done: usize, failed: usize) {
        for (index, outcome) in std::iter::repeat_n("done", done)
            .chain(std::iter::repeat_n("failed", failed))
            .enumerate()
        {
            let mission = format!("{agent}-{index}");
            let record = state
                .intake
                .open_session(
                    NewSession::new(format!("{mission}.1"), &mission, "test")
                        .with_operation("G")
                        .with_launch(
                            SessionLaunch::new("mock-agent", "mock-terminal", "/w")
                                .with_agent(Some(agent.to_owned())),
                        ),
                    Duration::from_secs(600),
                )
                .unwrap();
            state
                .intake
                .update_session(
                    &record.session_id,
                    "test",
                    record.fence,
                    SessionUpdate::default()
                        .result(SessionResult::new(outcome))
                        .status(SessionStatus::Ended),
                )
                .unwrap();
        }
    }

    fn pending(state: &DaemonState) -> Vec<ApprovalRequest> {
        state
            .intake
            .pending_approvals("G", state.clock.now() as f64)
            .unwrap()
    }

    fn sign(state: &DaemonState, request: &ApprovalRequest, signer: &str) {
        let now = state.clock.now() as f64;
        state
            .intake
            .decide_approval(
                &request.operation,
                &request.id,
                &Ruling::approve(signer, now, now + 3600.0),
            )
            .unwrap();
    }

    #[test]
    fn m_proposes_a_hold_with_the_evidence_and_applies_it_only_when_a_person_signs() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), &settings("hold_above = 0.5"), AGENTS);
        attempts(&state, "007", 1, 5);
        attempts(&state, "008", 5, 0);
        round(&state);

        let waiting = pending(&state);
        let [request] = waiting.as_slice() else {
            panic!("{waiting:?}");
        };
        assert_eq!(request.scope.kind, HOLD_KIND);
        assert_eq!(request.scope.target, "007@G");
        assert_eq!(request.session.as_str(), "m");
        assert!(request.reason.contains("5 of 6"), "{}", request.reason);
        assert!(
            request.reason.contains("007-1"),
            "the Missions: {}",
            request.reason
        );
        assert!(
            state.intake.holds("G").unwrap().is_empty(),
            "nothing is held unsigned"
        );
        round(&state);
        assert_eq!(pending(&state).len(), 1, "one request, however many rounds");

        sign(&state, request, "alice");
        round(&state);
        let holds = state.intake.holds("G").unwrap();
        let [hold] = holds.as_slice() else {
            panic!("{holds:?}");
        };
        assert_eq!((hold.agent.as_str(), hold.by.as_str()), ("007", "alice"));
        assert_eq!(hold.approval.as_deref(), Some(request.id.as_str()));
        assert!(hold.reason.contains("5 of 6"));
        assert!(
            state
                .intake
                .list_audit(50)
                .unwrap()
                .iter()
                .any(|record| record.action == "agent.hold.applied" && record.actor == "alice")
        );
        round(&state);
        assert!(
            pending(&state).is_empty(),
            "a held Agent is not proposed again, nor is the spent request reused"
        );
    }

    #[test]
    fn a_hold_stops_new_work_for_the_agent_and_a_person_lifts_it() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), &settings(""), AGENTS);
        let pick = || {
            agents::select(&state, Some("G"), Some("case-officer"), &[], None, &[])
                .map(|agent| agent.id)
                .unwrap()
        };
        assert_eq!(pick(), "007", "the higher priority");
        state
            .intake
            .put_hold(&RosterHold::new("G", "007", None, "failing", "alice", 1.0))
            .unwrap();
        assert_eq!(pick(), "008", "007 is passed over while held");
        state.intake.lift_hold("G", "007", None).unwrap();
        assert_eq!(pick(), "007");
    }

    #[test]
    fn auto_hold_holds_by_itself_audited_and_refuses_to_empty_a_role() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(
            root.path(),
            &settings("hold_above = 0.5\nauto_hold = true"),
            AGENTS,
        );
        attempts(&state, "007", 0, 5);
        attempts(&state, "008", 0, 5);
        round(&state);

        let holds = state.intake.holds("G").unwrap();
        assert_eq!(
            holds.len(),
            1,
            "the second would leave case-officer empty: {holds:?}"
        );
        assert_eq!(
            (holds[0].agent.as_str(), holds[0].by.as_str()),
            ("007", AUTOMATIC)
        );
        let waiting = pending(&state);
        let [request] = waiting.as_slice() else {
            panic!("{waiting:?}");
        };
        assert_eq!(request.scope.target, "008@G");
        assert!(
            request.reason.contains("case-officer"),
            "it says why it asks: {}",
            request.reason
        );
        assert!(
            state
                .intake
                .list_audit(50)
                .unwrap()
                .iter()
                .any(|record| record.action == "agent.hold.applied" && record.actor == AUTOMATIC)
        );
    }

    #[test]
    fn nothing_here_grants_a_role_a_clearance_or_an_account() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(
            root.path(),
            &settings("hold_above = 0.5\nreorder_gap = 0.2\nauto_hold = true"),
            AGENTS,
        );
        attempts(&state, "007", 0, 5);
        attempts(&state, "008", 5, 0);
        let before = state.roster.agents_for_operation("G").unwrap();
        round(&state);
        round(&state);
        assert_eq!(
            state.roster.agents_for_operation("G").unwrap(),
            before,
            "the roster"
        );
        assert!(state.intake.clearance_grants("G").unwrap().is_empty());
        assert!(state.intake.agent_records().unwrap().is_empty());
        assert!(state.intake.accounts().unwrap().is_empty());
        let kinds: Vec<_> = pending(&state)
            .into_iter()
            .map(|request| request.scope.kind)
            .collect();
        assert!(
            kinds
                .iter()
                .all(|kind| kind == HOLD_KIND || kind == PREFER_KIND),
            "{kinds:?}"
        );
    }

    #[test]
    fn an_agent_with_no_roles_is_never_held_or_put_ahead() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(
            root.path(),
            &settings("hold_above = 0.5\nreorder_gap = 0.2\nauto_hold = true"),
            AGENTS,
        );
        attempts(&state, "fable", 0, 5);
        attempts(&state, "008", 0, 5);
        round(&state);
        assert!(
            state
                .intake
                .holds("G")
                .unwrap()
                .iter()
                .all(|hold| hold.agent != "fable"),
            "used only when named"
        );
        assert!(
            pending(&state)
                .iter()
                .all(|request| request.agent != "fable")
        );
    }

    #[test]
    fn nothing_is_proposed_by_default_or_when_proposing_is_off() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), &settings(""), AGENTS);
        attempts(&state, "007", 0, 9);
        round(&state);
        assert!(pending(&state).is_empty() && state.intake.holds("G").unwrap().is_empty());

        let root = tempfile::tempdir().unwrap();
        let state = test_state(
            root.path(),
            &settings("hold_above = 0.1\npropose = false"),
            AGENTS,
        );
        attempts(&state, "007", 0, 9);
        round(&state);
        assert!(pending(&state).is_empty());
    }
}
