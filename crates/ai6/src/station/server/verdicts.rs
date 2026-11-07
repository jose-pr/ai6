//! What a Mission's end leaves for a person to check: the Agent's final message is filed, a role
//! that requires a decided green ink request holds a `done` that has none, and a person's verdict,
//! written to the Registry by Universal Exports, settles what is held or being verified.

use coqui_acp_provider::AgentEvent;

use super::{
    DaemonState, FinishOutcome, publish_finished_event, review, session_hold, verification, vetting,
};
use crate::directorates::m::approvals::NO_APPROVAL_REASON;
use crate::directorates::m::mission::{Mission, MissionState};
use crate::directorates::registry::approvals::{ApprovalStatus, Decision};
use crate::directorates::registry::sessions::{FinalReport, MissionVerdict, OutcomeVerdict};

/// The event kind that holds a Mission for a person's verdict.
pub(super) const HELD_EVENT: &str = "held";

/// The reason a held Mission's lifecycle is published with: a wire reason has no spaces.
#[cfg(feature = "publisher")]
pub(super) const NO_APPROVAL_CODE: &str = "no-approval";

/// Files the message the Agent ended its turn with, on the Session this Station holds. A review
/// may keep the Mission from ending here, and a later turn replaces it.
pub(super) fn file_final_report(state: &DaemonState, mission_id: &str, text: Option<&str>) {
    let (Some(text), Some(hold)) = (
        text.filter(|text| !text.trim().is_empty()),
        session_hold(state, mission_id),
    ) else {
        return;
    };
    let report = FinalReport::new(
        hold.id,
        mission_id,
        text,
        state.clock.now() as f64,
        state.intake.limits().sessions.final_report_bytes,
    );
    if let Err(error) = state.intake.file_final_report(&report) {
        eprintln!("ai6: Mission {mission_id:?}: its final report was not kept: {error}");
    }
}

/// Whether a Mission's `done` stands in the way of its role's requirement: the role wants a green
/// ink request decided in this attempt, and none was. A request that expired was not decided. What
/// cannot be read counts as none: a person's look costs less than a false `done`.
fn lacks_decision(state: &DaemonState, mission: &Mission) -> bool {
    if vetting::is_review_mission(mission) || review::is_panel_mission(mission) {
        return false;
    }
    let (Some(role), Some(operation)) =
        (mission.role_id.as_deref(), mission.operation_id.as_deref())
    else {
        return false;
    };
    if !state
        .roster
        .role(role)
        .is_ok_and(|role| role.require_approval_before_done)
    {
        return false;
    }
    let Some(session) = mission.session_ids.last() else {
        return true;
    };
    let now = state.clock.now() as f64;
    let decided = state
        .intake
        .mission_approvals(operation, mission.mission_id.as_str(), now)
        .is_ok_and(|statuses| {
            statuses.iter().any(|status| {
                matches!(
                    status,
                    ApprovalStatus::Decided(approval)
                        if approval.decision != Decision::Expired
                            && approval.request.session.as_str() == session.as_str()
                )
            })
        });
    !decided
}

/// `event`, or for a `completed` that [`lacks_decision`], the event that holds the Mission instead.
/// A Mission already held is ending by a person's verdict.
pub(super) fn hold_unapproved(
    state: &DaemonState,
    mission_id: &str,
    mut event: AgentEvent,
) -> AgentEvent {
    let mission = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned());
    if event.kind == "completed"
        && mission.is_some_and(|mission| {
            !matches!(mission.state, MissionState::Held | MissionState::Verifying)
                && lacks_decision(state, &mission)
        })
    {
        event.kind = HELD_EVENT.to_owned();
        event.text = Some(NO_APPROVAL_REASON.to_owned());
    }
    event
}

/// Settles each held or verifying Mission a person gave a verdict on: `done` when they confirmed
/// it, `failed` otherwise, and for a verifying one asked to be redispatched, its next attempt. Its
/// brief is filed on the next intake pass.
pub(super) fn settle_held(state: &DaemonState) {
    let waiting = state
        .missions
        .lock()
        .map(|missions| missions.list())
        .unwrap_or_default()
        .into_iter()
        .filter(|mission| matches!(mission.state, MissionState::Held | MissionState::Verifying));
    for mission in waiting {
        let mission_id = mission.mission_id.as_str();
        let verdict = session_hold(state, mission_id)
            .and_then(|hold| state.intake.session(&hold.id).ok().flatten())
            .and_then(|record| record.verdict);
        match verdict {
            Some(verdict)
                if verdict.verdict == OutcomeVerdict::Redispatch
                    && mission.state == MissionState::Verifying =>
            {
                verification::redispatch(state, &mission, &verdict);
            }
            // Only a verifying Mission is redispatched: a held one has no work to try again.
            Some(verdict) if verdict.verdict != OutcomeVerdict::Redispatch => {
                settle(state, mission_id, &verdict);
            }
            _ => {}
        }
    }
}

fn settle(state: &DaemonState, mission_id: &str, verdict: &MissionVerdict) {
    let spoken = format!("{} by {}", verdict.verdict.as_str(), verdict.by);
    let kind = if verdict.verdict == OutcomeVerdict::Confirmed {
        "completed"
    } else {
        let failed = state
            .missions
            .lock()
            .is_ok_and(|mut missions| missions.fail_for(mission_id, &spoken).is_ok());
        if !failed {
            return;
        }
        "failed"
    };
    let event = AgentEvent {
        kind: kind.to_owned(),
        text: Some(spoken),
        data: None,
        limit: None,
        usage: None,
        tools: Vec::new(),
        quota: Vec::new(),
    };
    let finished = FinishOutcome {
        cover_blown: false,
        fail_on_blow: false,
        failure: None,
        #[cfg(feature = "publisher")]
        telemetry: review::published(state, mission_id),
    };
    publish_finished_event(state, mission_id, event, Some(finished));
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::tests::{
        start_mission_by_id, test_state, wait_for_done, wait_for_state, wait_for_verifying,
    };
    use super::*;
    use crate::directorates::m::approvals::ActionScope;
    use crate::directorates::m::identity::{MissionId, SessionId};
    use crate::directorates::m::scorecard::{self, Judgment, ScorecardPolicy};
    use crate::directorates::registry::approvals::{ApprovalRequest, Ruling};

    /// Agents that finish at once with `All good.`, for a role that requires a decision and for
    /// one that does not.
    const AGENTS: &str = "[role.releaser]\npurpose = \"Releases\"\npolicy = \"Release it.\"\n\
         require_approval_before_done = true\n\
         [role.writer]\npurpose = \"Writes\"\npolicy = \"Write it.\"\n\
         [agent.007]\nhandler = \"mock-verdict\"\nmodel = \"clear\"\n\
         roles = [\"releaser\", \"writer\"]\n";

    fn mission_for(state: &DaemonState, name: &str, role: &str) -> String {
        let mission_id = format!("verdict-{name}-{}", std::process::id());
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(&mission_id).unwrap(),
                Some("G".to_owned()),
                "do the work".to_owned(),
                Some("007".to_owned()),
                Some(role.to_owned()),
            )
            .unwrap();
        mission_id
    }

    fn outcome_of(root: &Path, directory: &str, key: &str) -> serde_json::Value {
        let path = root
            .join("registry")
            .join(directory)
            .join(format!("{key}.outcome.json"));
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn stop(state: &DaemonState) {
        state
            .stopping
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[tokio::test]
    async fn the_final_message_is_in_the_outcome_of_a_brief() {
        let root = tempfile::tempdir().unwrap();
        let state = super::super::placement_tests::empty_book(root.path(), AGENTS);
        let key = format!("final-report-{}", std::process::id());
        let mut brief = super::super::placement_tests::pinned_brief(&key, &[]);
        brief.role = Some("writer".to_owned());
        state.intake.submit_brief(&brief).unwrap();
        super::super::schedule_intake_once(&state);
        let mission_id = brief.key();
        super::super::schedule_queued_once(&state).await;
        wait_for_verifying(&state, &mission_id).await;
        // Its brief is not filed while the Mission waits for a decision.
        super::super::schedule_intake_once(&state);
        assert_eq!(
            state
                .intake
                .filed_brief(&mission_id)
                .unwrap()
                .unwrap()
                .state,
            "claimed"
        );

        wait_for_done(&state, &mission_id).await;
        super::super::schedule_intake_once(&state);
        let outcome = outcome_of(root.path(), "done", &brief.key());
        assert_eq!(outcome["state"], "done");
        assert!(
            outcome["final_report"]
                .as_str()
                .is_some_and(|report| report.starts_with("All good.")),
            "{outcome}"
        );
        assert_eq!(outcome["verdict"]["verdict"], "confirmed", "{outcome}");
        stop(&state);
    }

    #[tokio::test]
    async fn a_role_that_requires_a_decision_holds_a_done_session_that_filed_no_request() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        let mission_id = mission_for(&state, "held", "releaser");
        start_mission_by_id(&state, &mission_id).await;
        wait_for_state(&state, &mission_id, MissionState::Held).await;

        let mission = state
            .missions
            .lock()
            .unwrap()
            .get(&mission_id)
            .cloned()
            .unwrap();
        assert_eq!(mission.blocked_reason.as_deref(), Some(NO_APPROVAL_REASON));
        let record = state.intake.latest_session(&mission_id).unwrap().unwrap();
        let result = record.result.clone().unwrap();
        assert_eq!(
            (result.state.as_str(), result.reason.as_deref()),
            ("held", Some(NO_APPROVAL_REASON))
        );
        assert_eq!(scorecard::judge(&record), Judgment::WaitingOnPerson);
        assert!(!record.status.is_closed(), "a person settles it");

        // The same Agent under a role with no such requirement ends done.
        let free = mission_for(&state, "free", "writer");
        start_mission_by_id(&state, &free).await;
        wait_for_done(&state, &free).await;
        stop(&state);
    }

    #[tokio::test]
    async fn a_verdict_settles_a_held_mission_and_counts_on_the_scorecard() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        let confirmed = mission_for(&state, "confirmed", "releaser");
        let disputed = mission_for(&state, "disputed", "releaser");
        for mission_id in [&confirmed, &disputed] {
            start_mission_by_id(&state, mission_id).await;
            wait_for_state(&state, mission_id, MissionState::Held).await;
        }
        let now = state.clock.now() as f64;
        settle_held(&state);
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get(&confirmed)
                .unwrap()
                .state,
            MissionState::Held,
            "no verdict yet"
        );

        let verdict = |verdict| MissionVerdict::new(verdict, "person-m", now, None);
        state
            .intake
            .record_verdict("G", &confirmed, &verdict(OutcomeVerdict::Confirmed))
            .unwrap();
        state
            .intake
            .record_verdict("G", &disputed, &verdict(OutcomeVerdict::FailedVerification))
            .unwrap();
        settle_held(&state);
        wait_for_state(&state, &confirmed, MissionState::Done).await;
        wait_for_state(&state, &disputed, MissionState::Failed).await;
        let failed = state
            .missions
            .lock()
            .unwrap()
            .get(&disputed)
            .cloned()
            .unwrap();
        assert_eq!(
            failed.failure.as_deref(),
            Some("failed-verification by person-m")
        );

        let card = scorecard::from_registry(
            state.intake.as_ref(),
            "G",
            &ScorecardPolicy::default(),
            now + 1.0,
        )
        .unwrap();
        let standing = &card.agents[0].standing;
        assert_eq!(standing.count(Judgment::Done), 1);
        assert_eq!(standing.count(Judgment::Disputed), 1);
        assert_eq!(standing.failure_rate, Some(0.5));
        stop(&state);
    }

    /// One Mission of Agent 007 that has reported Session `session`.
    fn with_session(state: &DaemonState, session: &str) -> Mission {
        let mission_id = mission_for(state, session, "releaser");
        let mut missions = state.missions.lock().unwrap();
        missions
            .transition(&mission_id, MissionState::Working)
            .unwrap();
        missions
            .attach_session(&mission_id, SessionId::try_new(session).unwrap())
            .unwrap();
        missions.get(&mission_id).cloned().unwrap()
    }

    fn asked(state: &DaemonState, mission: &Mission, session: &str, ttl: u64) -> ApprovalRequest {
        let now = state.clock.now() as f64;
        let request = ApprovalRequest::try_new(
            "G",
            mission.mission_id.clone(),
            SessionId::try_new(session).unwrap(),
            "007",
            ActionScope::try_new("push", "origin:refs/heads/main").unwrap(),
            "the release is green",
            session,
            &Default::default(),
        )
        .unwrap()
        .expiring(now - 60.0, std::time::Duration::from_secs(ttl));
        state.intake.request_approval(&request).unwrap();
        request
    }

    #[test]
    fn only_a_decision_in_this_attempt_lets_a_done_stand() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        let now = state.clock.now() as f64;

        let none = with_session(&state, "session-none");
        assert!(lacks_decision(&state, &none), "no request at all");

        let approved = with_session(&state, "session-approved");
        let request = asked(&state, &approved, "session-approved", 3_600);
        assert!(lacks_decision(&state, &approved), "still undecided");
        state
            .intake
            .decide_approval("G", &request.id, &Ruling::approve("p", now, now + 60.0))
            .unwrap();
        assert!(!lacks_decision(&state, &approved));

        let refused = with_session(&state, "session-refused");
        let request = asked(&state, &refused, "session-refused", 3_600);
        state
            .intake
            .decide_approval("G", &request.id, &Ruling::reject("p", now, None))
            .unwrap();
        assert!(!lacks_decision(&state, &refused), "a person decided");

        let lapsed = with_session(&state, "session-lapsed");
        asked(&state, &lapsed, "session-lapsed", 1);
        assert!(lacks_decision(&state, &lapsed), "an expiry is no decision");

        let earlier = with_session(&state, "session-earlier");
        let request = asked(&state, &earlier, "session-earlier", 3_600);
        state
            .intake
            .decide_approval("G", &request.id, &Ruling::approve("p", now, now + 60.0))
            .unwrap();
        state
            .missions
            .lock()
            .unwrap()
            .attach_session(
                earlier.mission_id.as_str(),
                SessionId::try_new("session-later").unwrap(),
            )
            .unwrap();
        let later = state
            .missions
            .lock()
            .unwrap()
            .get(earlier.mission_id.as_str())
            .cloned()
            .unwrap();
        assert!(lacks_decision(&state, &later), "another attempt's decision");
    }
}
