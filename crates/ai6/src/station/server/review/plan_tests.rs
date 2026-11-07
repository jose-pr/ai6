//! The plan gate: a Mission held to reading until its plan passes.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::super::tests::test_state;
use super::super::{
    DaemonState, publish_station_check_in, schedule_intake_once, schedule_queued_once,
    schedule_recovered_once,
};
use super::testing::{AUTHOR_RESUMES, AUTHOR_STARTS};
use super::*;
use crate::directorates::m::mission::{MissionState, MissionStore};
use crate::directorates::section_00::agent::READ_CLEARANCE;

const PLAN_ONE: &str = "[review.one]\npanel = [{ agent = \"r1\" }]\n";

fn station(root: &Path, agents: &str, operation: &str) -> DaemonState {
    fs::write(root.join("reviews.toml"), PLAN_ONE).unwrap();
    let mut state = test_state(root, operation, agents);
    state.missions = Arc::new(Mutex::new(
        MissionStore::open(root.join("missions.json")).unwrap(),
    ));
    publish_station_check_in(&state);
    state
}

fn agents(author: &str, reviewer: &str) -> String {
    format!(
        "[agent.007]\nhandler = {author:?}\nmodel = \"author-model\"\n\
         [agent.r1]\nhandler = \"mock-reviewer\"\nmodel = {reviewer:?}\n"
    )
}

const PLAN_GATE: &str = "[operation.G.review]\nplan = \"one\"\n";

fn submit(state: &DaemonState, mission: &str) {
    let brief: Brief = serde_json::from_value(serde_json::json!({
        "title": "Add the widget",
        "body": format!("the goals of {mission}"),
        "id": mission,
        "mission_id": mission,
        "source": "manual",
        "operation": "G",
        "agent": "007",
    }))
    .unwrap();
    state.intake.submit_brief(&brief).unwrap();
}

async fn tick(state: &DaemonState) {
    let worker = state.clone();
    let _ = tokio::task::spawn_blocking(move || schedule_intake_once(&worker)).await;
    advance(state).await;
    schedule_recovered_once(state).await;
    schedule_queued_once(state).await;
}

async fn until(state: &DaemonState, what: &str, reached: impl Fn(&DaemonState) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if reached(state) {
            return;
        }
        tick(state).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "never reached: {what}; {:#?}",
        state.missions.lock().unwrap().list()
    );
}

fn mission(state: &DaemonState, id: &str) -> Option<Mission> {
    state.missions.lock().unwrap().get(id).cloned()
}

/// Rounds until the Mission, its Session ended done, is verifying; then a person's confirmation as
/// the gateway records it, and rounds until the Station has settled it done.
async fn done(state: &DaemonState, id: &str) {
    until(state, "verifying", |state| {
        mission(state, id).is_some_and(|mission| mission.state == MissionState::Verifying)
            && state
                .intake
                .latest_session(id)
                .unwrap()
                .and_then(|record| record.result)
                .is_some_and(|result| result.state == "verifying")
    })
    .await;
    let confirmed = crate::directorates::registry::sessions::MissionVerdict::new(
        crate::directorates::registry::sessions::OutcomeVerdict::Confirmed,
        "person-test",
        state.clock.now() as f64,
        Some("checked".to_owned()),
    );
    state.intake.record_verdict("G", id, &confirmed).unwrap();
    until(state, "done", |state| {
        super::super::verdicts::settle_held(state);
        mission(state, id).is_some_and(|mission| mission.state == MissionState::Done)
    })
    .await;
}

fn plan_review(state: &DaemonState, id: &str) -> Option<MissionReview> {
    state.intake.mission_review(id, ReviewGate::Plan).unwrap()
}

fn clearances(id: &str) -> (Vec<String>, Vec<String>) {
    let starts = AUTHOR_STARTS
        .lock()
        .unwrap()
        .iter()
        .filter(|start| start.mission_id.as_str() == id)
        .map(|start| start.clearance.clone())
        .collect();
    let resumes = AUTHOR_RESUMES
        .lock()
        .unwrap()
        .iter()
        .filter(|resume| resume.mission_id.as_str() == id)
        .map(|resume| resume.clearance.clone())
        .collect();
    (starts, resumes)
}

#[tokio::test]
async fn a_plan_is_reviewed_before_the_agent_gets_its_tools_and_then_it_works() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path(), &agents("mock-author", "pass"), PLAN_GATE);
    submit(&state, "rv-plan");
    until(&state, "the plan waits", |state| {
        mission(state, "rv-plan")
            .is_some_and(|mission| mission.blocked_reason.as_deref() == Some(PLAN_REASON))
    })
    .await;
    let start = AUTHOR_STARTS
        .lock()
        .unwrap()
        .iter()
        .find(|start| start.mission_id.as_str() == "rv-plan")
        .cloned()
        .unwrap();
    assert_eq!(
        start.clearance, READ_CLEARANCE,
        "it starts holding only what reads"
    );
    assert!(!start.project_settings && !start.project_mcp);
    assert!(
        start
            .initial_context
            .iter()
            .any(|context| context.contains("submit_plan"))
    );
    assert!(
        clearances("rv-plan").1.is_empty(),
        "nothing resumed it before the pass"
    );
    done(&state, "rv-plan").await;
    let (starts, resumes) = clearances("rv-plan");
    assert_eq!(starts, [READ_CLEARANCE]);
    assert_eq!(resumes.len(), 1, "one resume, after the pass");
    assert_ne!(resumes[0], READ_CLEARANCE, "with the Mission's own tools");
    let resumed = AUTHOR_RESUMES
        .lock()
        .unwrap()
        .iter()
        .find(|resume| resume.mission_id.as_str() == "rv-plan")
        .cloned()
        .unwrap();
    assert_eq!(
        Some(&resumed.session_id),
        mission(&state, "rv-plan").unwrap().session_ids.first(),
        "the same Session, as its stream reported it"
    );
    assert!(resumed.message.unwrap().contains("passed"));
    assert_eq!(
        plan_review(&state, "rv-plan").unwrap().status,
        ReviewStatus::Passed
    );
    assert!(
        state
            .intake
            .mission_review("rv-plan", ReviewGate::Output)
            .unwrap()
            .is_none(),
        "only the plan is reviewed"
    );
    state
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
}

#[tokio::test]
async fn a_plan_sent_back_is_redone_still_holding_only_what_reads() {
    let root = tempfile::tempdir().unwrap();
    let state = station(
        root.path(),
        &agents("mock-author", "strict"),
        "[operation.G.review]\nplan = \"one\"\n",
    );
    submit(&state, "rv-plan-redo");
    done(&state, "rv-plan-redo").await;
    let (starts, resumes) = clearances("rv-plan-redo");
    assert_eq!(starts, [READ_CLEARANCE]);
    assert_eq!(resumes.len(), 2, "sent back once, then on after the pass");
    assert_eq!(
        resumes[0], READ_CLEARANCE,
        "a plan sent back is redone reading"
    );
    assert_ne!(resumes[1], READ_CLEARANCE);
    let review = plan_review(&state, "rv-plan-redo").unwrap();
    assert_eq!(review.sent_back, 1);
    assert_eq!(review.status, ReviewStatus::Passed);
    state
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
}

#[tokio::test]
async fn a_handler_that_cannot_hold_a_session_to_reading_never_starts_it() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path(), &agents("mock-agent", "pass"), PLAN_GATE);
    submit(&state, "rv-plan-refused");
    until(&state, "it waits, with the reason", |state| {
        mission(state, "rv-plan-refused").is_some_and(|mission| {
            mission
                .waiting
                .as_deref()
                .is_some_and(|why| why.contains("plan is reviewed"))
        })
    })
    .await;
    for _ in 0..5 {
        tick(&state).await;
    }
    let mission = mission(&state, "rv-plan-refused").unwrap();
    assert_eq!(mission.state, MissionState::Queued, "it never started");
    assert!(mission.session_ids.is_empty());
    state
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
}

#[tokio::test]
async fn the_submitted_plan_is_what_the_panel_reads() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path(), &agents("mock-author", "silent"), PLAN_GATE);
    // The author ends its turn only after it has submitted, as a real one does.
    super::testing::HELD
        .lock()
        .unwrap()
        .insert("rv-plan-tool".to_owned());
    submit(&state, "rv-plan-tool");
    until(&state, "running", |state| {
        mission(state, "rv-plan-tool").is_some_and(|mission| !mission.session_ids.is_empty())
    })
    .await;
    // What the submit_plan tool does with what the Agent sends.
    let answer = PlanDesk::new(state.clone(), "rv-plan-tool".to_owned())
        .submit("1. read the code\n2. add the widget, with a test");
    assert!(answer.contains("End your turn"), "{answer}");
    super::testing::HELD.lock().unwrap().remove("rv-plan-tool");
    until(&state, "the plan is up", |state| {
        plan_review(state, "rv-plan-tool").is_some()
    })
    .await;
    let review = plan_review(&state, "rv-plan-tool").unwrap();
    assert!(
        review
            .material
            .submitted
            .contains("add the widget, with a test"),
        "{:?}",
        review.material
    );
    assert!(review.material.diff.is_empty(), "a plan has no change yet");
    // The plan is forgotten once it is up, just before the Mission is blocked.
    until(&state, "blocked", |state| {
        mission(state, "rv-plan-tool")
            .is_some_and(|mission| mission.blocked_reason.as_deref() == Some(PLAN_REASON))
    })
    .await;
    assert_eq!(
        mission(&state, "rv-plan-tool").unwrap().plan,
        None,
        "kept once"
    );
    state
        .stopping
        .store(true, std::sync::atomic::Ordering::Release);
}

#[test]
fn a_handler_says_it_cannot_hold_reading_unless_it_says_it_can() {
    use crate::directorates::section_00::agent::AgentHandler;
    struct Silent;
    impl AgentHandler for Silent {
        fn start(
            &self,
            _: crate::directorates::section_00::agent::AgentStart,
            _: crate::guatiao::library::Object<dyn crate::directorates::q::terminal::AgentTerminal>,
        ) -> Result<
            crate::guatiao::library::Object<dyn coqui_acp_provider::AgentSession>,
            crate::guatiao::library::ProviderError,
        > {
            unreachable!()
        }
        fn resume(
            &self,
            _: crate::directorates::section_00::agent::AgentResume,
            _: crate::guatiao::library::Object<dyn crate::directorates::q::terminal::AgentTerminal>,
        ) -> Result<
            crate::guatiao::library::Object<dyn coqui_acp_provider::AgentSession>,
            crate::guatiao::library::ProviderError,
        > {
            unreachable!()
        }
    }
    let refused = plan_stage_refusal(&Silent, false).unwrap();
    assert!(refused.contains("cannot hold"), "{refused}");
    let remote = plan_stage_refusal(&super::testing::MockAuthorHandler, true).unwrap();
    assert!(remote.contains("remote-hands"), "{remote}");
    assert_eq!(
        plan_stage_refusal(&super::testing::MockAuthorHandler, false),
        None
    );
}
