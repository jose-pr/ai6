//! Promotion at Headquarters: M puts Agents up for a higher clearance and carries out the
//! promotions a person signs.
//!
//! Each round, for every Agent posted to an Operation, M reads its record there. When it
//! meets the Operation's promotion step, M files a green ink request of kind `clearance`
//! (target `AGENT:CLEARANCE@OPERATION`) on the Agent's behalf: Mission `promotion-AGENT`,
//! Session `m`. Filing again finds the same request, so there is one per step until the
//! record meets the step's Mission count once more. A request a person approved is spent,
//! then becomes a clearance grant for the model and configured clearance the Agent has, so
//! removing the grant is final. A quarantine takes away a raised grant (the Registry removes
//! it with the quarantine; a round finding one left takes it back): the Agent goes back to its
//! configured clearance and must earn the step again. Nothing here raises a clearance without
//! a person's signature.

use super::{DaemonState, agents, medical};
use crate::directorates::m::approvals::ActionScope;
use crate::directorates::m::promotion::CLEARANCE_KIND;
use crate::directorates::m::roster::{Operation, Retirement};
use crate::directorates::registry::agent_records::NO_OPERATION;
use crate::directorates::registry::approvals::{
    ApprovalRequest, ApprovalStatus, Decision, EXPIRY_SIGNER,
};
use crate::directorates::registry::clearance::ClearanceGrant;
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::{MissionId, SessionId};

/// The Session M files promotion requests under.
pub(super) const PROMOTION_SESSION: &str = "m";
const TIMELINE_KIND: &str = "clearance";

/// The Mission a promotion request for `agent` is filed under.
pub(super) fn promotion_mission(agent: &str) -> String {
    format!("promotion-{agent}")
}

/// The green ink target of a request to raise `agent` to `clearance` in `operation`.
pub(super) fn promotion_target(agent: &str, clearance: &str, operation: &str) -> String {
    format!("{agent}:{clearance}@{operation}")
}

/// One promotion round over every Operation of the roster. Runs on the Headquarters Station.
pub(super) fn round(state: &DaemonState) {
    let records = match state.intake.agent_records() {
        Ok(records) => records,
        Err(error) => {
            eprintln!("ai6: Agent records unreadable; no promotion round: {error}");
            return;
        }
    };
    for operation in state.roster.operations() {
        let posted = state.roster.agents_for_operation(&operation.id);
        for agent in posted.unwrap_or_default() {
            let record = records.iter().find(|record| record.agent == agent.id);
            consider(state, operation, &agent, record);
        }
    }
}

fn consider(
    state: &DaemonState,
    operation: &Operation,
    agent: &crate::directorates::m::roster::Agent,
    record: Option<&crate::directorates::registry::agent_records::AgentRecord>,
) {
    let Some(grant) = agents::grant_of(state, &operation.id, &agent.id) else {
        return;
    };
    let now = state.clock.now();
    if medical::is_quarantined(state, &agent.id, Some(&operation.id)) {
        if let Some(grant) = grant.filter(ClearanceGrant::raises) {
            take_back(state, &grant, "the Agent was quarantined");
        }
        return;
    }
    // An operator's lower grant is a decision M does not argue with.
    if grant
        .as_ref()
        .is_some_and(|grant| !grant.raises() && grant.is_live(now as f64))
        || agent.retirement(now) != Retirement::Serving
    {
        return;
    }
    let Some(record) = record else {
        return;
    };
    let backed = grant
        .clone()
        .filter(|grant| agents::is_backed(state, grant));
    let current = agent.effective_clearance(operation, backed.as_ref(), now);
    // Missions count only for the model that occupies the codename now; incidents of any
    // model, and quarantines of every Operation, keep the quiet period.
    let here = record.in_operation(&operation.id);
    let Some(promotion) = operation.promotion.due(
        &current,
        &operation.clearance,
        &here.tally_of_model(agent.model.as_deref()),
        &[&here.tally, &record.in_operation(NO_OPERATION).tally],
        now as f64,
    ) else {
        return;
    };
    let target = promotion_target(&agent.id, &promotion.to, &operation.id);
    let request = match promotion_request(state, operation, &agent.id, &target, &promotion) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("ai6: promotion of Agent {:?} not filed: {error}", agent.id);
            return;
        }
    };
    match state.intake.request_approval(&request) {
        Ok(ApprovalStatus::Pending(stored)) => {
            if stored.requested_at == request.requested_at {
                eprintln!(
                    "ai6: Agent {:?} put up for {} in {:?}: green ink {}",
                    agent.id, promotion.to, operation.id, stored.id
                );
                record_event(state, &stored, "requested", "m");
                audit(state, "agent.promotion.requested", "m", &stored, now as f64);
            }
        }
        Ok(ApprovalStatus::Decided(approval)) => {
            let signed_by_a_person = approval.decision == Decision::Approved
                && approval.request.session.as_str() == PROMOTION_SESSION
                && approval.request.scope.kind == CLEARANCE_KIND
                && approval.request.scope.target == target
                && approval.signer != EXPIRY_SIGNER
                && !approval.signer.starts_with("class:");
            if !signed_by_a_person || !approval.is_live(now as f64) || approval.spent_at.is_some() {
                return;
            }
            raise(
                state,
                &approval.request,
                &promotion.to,
                &approval.signer,
                agent,
                grant,
            );
        }
        Err(error) => eprintln!("ai6: promotion of Agent {:?} not filed: {error}", agent.id),
    }
}

fn promotion_request(
    state: &DaemonState,
    operation: &Operation,
    agent: &str,
    target: &str,
    promotion: &crate::directorates::m::promotion::Promotion,
) -> std::io::Result<ApprovalRequest> {
    let invalid = |_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid identity");
    let request = ApprovalRequest::try_new(
        operation.id.clone(),
        MissionId::try_new(promotion_mission(agent)).map_err(invalid)?,
        SessionId::try_new(PROMOTION_SESSION).map_err(invalid)?,
        agent,
        ActionScope::try_new(CLEARANCE_KIND, target)?,
        promotion.reason.clone(),
        &format!("{target}#{}", promotion.round),
        &state.intake.limits().approvals,
    )?;
    Ok(request.expiring(state.clock.now() as f64, operation.green_ink.request_ttl()))
}

/// Spends the approval, then writes the grant it earned for the Agent's current occupant, but
/// only if the Agent's grant is still `current`, the one this round read: a lower an operator
/// wrote meanwhile stands. Spending first means a grant removed later (revoked, taken back by
/// a quarantine) is never written again.
fn raise(
    state: &DaemonState,
    request: &ApprovalRequest,
    clearance: &str,
    signer: &str,
    agent: &crate::directorates::m::roster::Agent,
    current: Option<ClearanceGrant>,
) {
    let now = state.clock.now() as f64;
    match state
        .intake
        .spend_approval(&request.operation, &request.id, now)
    {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            eprintln!(
                "ai6: promotion approval {:?} not spent: {error}",
                request.id
            );
            return;
        }
    }
    let grant = ClearanceGrant::raise(
        request.agent.clone(),
        request.operation.clone(),
        clearance,
        request.id.clone(),
        signer,
        now,
    )
    .for_occupant(agent.model.clone(), agent.clearance.clone())
    .because(request.reason.clone());
    match state
        .intake
        .replace_clearance_grant(current.as_ref(), &grant)
    {
        Ok(true) => {
            eprintln!(
                "ai6: Agent {:?} raised to {clearance} in {:?}, signed by {signer}",
                request.agent, request.operation
            );
            record_event(state, request, "raised", signer);
            audit(state, "agent.clearance.raised", signer, request, now);
            // A quarantine stored after this write removes the raise itself; one stored
            // between this round's check and the write is caught here.
            if medical::is_quarantined(state, &request.agent, Some(&request.operation)) {
                take_back(state, &grant, "the Agent was quarantined");
            }
        }
        Ok(false) => eprintln!(
            "ai6: Agent {:?} not raised in {:?}: its clearance grant changed meanwhile",
            request.agent, request.operation
        ),
        Err(error) => eprintln!(
            "ai6: clearance of Agent {:?} not raised: {error}",
            request.agent
        ),
    }
}

/// Removes a raised grant, and withdraws its approval while that is still live.
fn take_back(state: &DaemonState, grant: &ClearanceGrant, why: &str) {
    let by = format!("station:{}", state.station_id);
    let now = state.clock.now() as f64;
    match state
        .intake
        .remove_clearance_grant(&grant.operation, &grant.agent)
    {
        Ok(Some(_)) => {}
        Ok(None) => return,
        Err(error) => {
            eprintln!(
                "ai6: raised clearance of Agent {:?} not taken back: {error}",
                grant.agent
            );
            return;
        }
    }
    if let Some(approval) = &grant.approval {
        let _ = state
            .intake
            .revoke_approval(&grant.operation, approval, &by, now);
    }
    eprintln!(
        "ai6: Agent {:?} back at its configured clearance in {:?}: {why}",
        grant.agent, grant.operation
    );
    let record = AuditRecord {
        id: format!("agent.clearance.revoked-{}", uuid::Uuid::new_v4().simple()),
        actor: by,
        action: "agent.clearance.revoked".to_owned(),
        operation: grant.operation.clone(),
        resource: None,
        target: format!("{} {}", grant.agent, grant.clearance),
        at: now,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: clearance audit record was not written: {error}");
    }
}

fn record_event(state: &DaemonState, request: &ApprovalRequest, action: &str, actor: &str) {
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
        eprintln!("ai6: clearance timeline event was not written: {error}");
    }
}

fn audit(state: &DaemonState, action: &str, actor: &str, request: &ApprovalRequest, at: f64) {
    let record = AuditRecord {
        id: format!("{action}-{}", uuid::Uuid::new_v4().simple()),
        actor: actor.to_owned(),
        action: action.to_owned(),
        operation: request.operation.clone(),
        resource: None,
        target: format!("{} {}", request.id, request.scope.target),
        at,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: clearance audit record {action} was not written: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_state;
    use super::super::{DaemonState, agents, medical};
    use crate::directorates::registry::agent_records::{AgentOutcome, AgentOutcomeReport};
    use crate::directorates::registry::approvals::Ruling;
    use crate::directorates::registry::clearance::ClearanceGrant;

    const SETTINGS: &str = "clearance = \"00\"\n[operation.L]\nclearance = \"00\"\n";
    const AGENTS: &str = "[agent.007]\nhandler = \"mock-agent\"\nclearance = \"desk\"\n";

    fn done(state: &DaemonState, missions: u32) {
        let now = state.clock.now() as f64;
        for mission in 0..missions {
            state
                .intake
                .record_agent_outcome(&AgentOutcomeReport::new(
                    "007",
                    Some("G".to_owned()),
                    AgentOutcome::Done,
                    format!("m-{mission}#3"),
                    now - 100.0,
                ))
                .unwrap();
        }
    }

    fn clearance_in(state: &DaemonState, operation: &str) -> String {
        agents::effective(state, Some(operation), "007")
            .unwrap()
            .clearance
    }

    fn pending(state: &DaemonState) -> Vec<String> {
        state
            .intake
            .pending_approvals("G", state.clock.now() as f64)
            .unwrap()
            .into_iter()
            .map(|request| request.scope.target)
            .collect()
    }

    #[test]
    fn a_record_that_meets_the_step_files_one_request_and_only_a_signature_raises() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), SETTINGS, AGENTS);
        done(&state, 9);
        super::round(&state);
        assert!(pending(&state).is_empty(), "nine Missions are not enough");

        done(&state, 10);
        super::round(&state);
        super::round(&state);
        assert_eq!(pending(&state), ["007:field@G"], "one request, filed once");
        assert_eq!(clearance_in(&state, "G"), "desk", "nothing is automatic");

        let now = state.clock.now() as f64;
        let request = state.intake.pending_approvals("G", now).unwrap().remove(0);
        assert_eq!(request.mission.as_str(), "promotion-007");
        state
            .intake
            .decide_approval(
                "G",
                &request.id,
                &Ruling::approve("alice", now, now + 3600.0),
            )
            .unwrap();
        super::round(&state);
        let grant = state.intake.clearance_grant("G", "007").unwrap().unwrap();
        assert_eq!(
            (grant.clearance.as_str(), grant.signer.as_str()),
            ("field", "alice")
        );
        assert_eq!(clearance_in(&state, "G"), "field");
        assert_eq!(clearance_in(&state, "L"), "desk", "in that Operation only");

        // Revoking returns the configured clearance, and the spent approval raises nothing.
        state.intake.remove_clearance_grant("G", "007").unwrap();
        super::round(&state);
        assert_eq!(clearance_in(&state, "G"), "desk");
        assert!(state.intake.clearance_grant("G", "007").unwrap().is_none());
    }

    /// Earns 007 its `field` step in G, signed by alice, and returns the grant it got.
    fn promoted(state: &DaemonState) -> ClearanceGrant {
        done(state, 10);
        super::round(state);
        let now = state.clock.now() as f64;
        let request = state.intake.pending_approvals("G", now).unwrap().remove(0);
        state
            .intake
            .decide_approval(
                "G",
                &request.id,
                &Ruling::approve("alice", now, now + 3600.0),
            )
            .unwrap();
        super::round(state);
        state.intake.clearance_grant("G", "007").unwrap().unwrap()
    }

    #[test]
    fn a_quarantine_takes_a_raise_back_at_once_and_a_lower_grant_holds() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), SETTINGS, AGENTS);
        promoted(&state);
        assert_eq!(clearance_in(&state, "G"), "field");
        medical::quarantine_agent(&state, "007", Some("G"), "looping", "operator");
        // Gone with the quarantine, before any round and whether or not it is released.
        assert!(state.intake.clearance_grant("G", "007").unwrap().is_none());
        assert!(state.intake.release_quarantine("007", Some("G")).unwrap());
        super::round(&state);
        assert_eq!(clearance_in(&state, "G"), "desk");

        // An operator's lower grant only lowers, and M files no promotion past it.
        let fresh = tempfile::tempdir().unwrap();
        let state = test_state(
            fresh.path(),
            SETTINGS,
            &AGENTS.replace("\"desk\"", "\"field\""),
        );
        let now = state.clock.now() as f64;
        state
            .intake
            .put_clearance_grant(&ClearanceGrant::lower("007", "L", "desk", "bob", now))
            .unwrap();
        assert_eq!(clearance_in(&state, "L"), "desk");
        assert_eq!(clearance_in(&state, "G"), "field");
        state
            .intake
            .put_clearance_grant(&ClearanceGrant::lower("007", "G", "00", "bob", now))
            .unwrap();
        assert_eq!(
            clearance_in(&state, "G"),
            "field",
            "a lower grant never raises"
        );
        done(&state, 30);
        super::round(&state);
        assert!(pending(&state).is_empty());
        state.intake.remove_clearance_grant("G", "007").unwrap();
        super::round(&state);
        assert_eq!(
            pending(&state),
            ["007:00@G"],
            "without it, the record earns 00"
        );
    }

    #[test]
    fn a_raise_belongs_to_its_occupant_and_needs_its_signed_approval() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), SETTINGS, AGENTS);
        let grant = promoted(&state);
        assert_eq!(
            (grant.model.as_deref(), grant.base.as_deref()),
            (None, Some("desk"))
        );
        assert_eq!(clearance_in(&state, "G"), "field");

        // A new model in the slot does not inherit the raise, and starts its count afresh.
        let state = test_state(
            root.path(),
            SETTINGS,
            &format!("{AGENTS}model = \"next\"\n"),
        );
        assert_eq!(clearance_in(&state, "G"), "desk");
        super::round(&state);
        assert!(
            pending(&state).is_empty(),
            "the next model has done nothing yet"
        );

        // Another configured clearance takes it away too: the configured one applies.
        let state = test_state(root.path(), SETTINGS, AGENTS);
        assert_eq!(clearance_in(&state, "G"), "field");
        let state = test_state(
            root.path(),
            "clearance = \"00\"\n",
            &AGENTS.replace("\"desk\"", "\"field\""),
        );
        assert_eq!(
            clearance_in(&state, "G"),
            "field",
            "configured, not the raise"
        );

        // A raise its approval does not back counts for nothing.
        let forged = tempfile::tempdir().unwrap();
        let state = test_state(forged.path(), SETTINGS, AGENTS);
        let now = state.clock.now() as f64;
        state
            .intake
            .put_clearance_grant(
                &ClearanceGrant::raise("007", "G", "00", "green-ink-x", "alice", now)
                    .for_occupant(None, "desk"),
            )
            .unwrap();
        assert_eq!(clearance_in(&state, "G"), "desk");
    }

    #[test]
    fn a_quarantine_stored_between_the_check_and_the_raise_still_takes_it_back() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), SETTINGS, AGENTS);
        done(&state, 10);
        super::round(&state);
        let now = state.clock.now() as f64;
        let request = state.intake.pending_approvals("G", now).unwrap().remove(0);
        state
            .intake
            .decide_approval(
                "G",
                &request.id,
                &Ruling::approve("alice", now, now + 3600.0),
            )
            .unwrap();
        // The round found 007 free; the quarantine lands before the raise is written.
        medical::quarantine_agent(&state, "007", Some("G"), "looping", "operator");
        let agent = state.roster.agent(Some("G"), "007").unwrap();
        super::raise(&state, &request, "field", "alice", &agent, None);
        assert!(state.intake.clearance_grant("G", "007").unwrap().is_none());
        assert!(state.intake.release_quarantine("007", Some("G")).unwrap());
        assert_eq!(clearance_in(&state, "G"), "desk");
    }

    #[test]
    fn a_raise_does_not_overwrite_a_lower_written_meanwhile_and_is_applied_once() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), SETTINGS, AGENTS);
        done(&state, 10);
        super::round(&state);
        let now = state.clock.now() as f64;
        let request = state.intake.pending_approvals("G", now).unwrap().remove(0);
        state
            .intake
            .decide_approval(
                "G",
                &request.id,
                &Ruling::approve("alice", now, now + 3600.0),
            )
            .unwrap();
        // The round read no grant; an operator lowers 007 before the raise is written.
        let lower = ClearanceGrant::lower("007", "G", "desk", "bob", now);
        state.intake.put_clearance_grant(&lower).unwrap();
        let agent = state.roster.agent(Some("G"), "007").unwrap();
        super::raise(&state, &request, "field", "alice", &agent, None);
        assert_eq!(
            state.intake.clearance_grant("G", "007").unwrap(),
            Some(lower)
        );
        // Its approval is spent: removing the lower does not bring the raise back.
        state.intake.remove_clearance_grant("G", "007").unwrap();
        super::round(&state);
        assert!(state.intake.clearance_grant("G", "007").unwrap().is_none());
        assert_eq!(clearance_in(&state, "G"), "desk");
    }
}
