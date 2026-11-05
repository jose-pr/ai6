//! A standing Sleeper's runs: each window is a new attempt of the Sleeper's one Mission, and one
//! that resumes takes up the provider Session the last attempt left.

use super::DaemonState;
use crate::directorates::m::vetting::sleeper_of;
use crate::directorates::registry::sessions::SessionRecord;
use crate::directorates::registry::sleepers::window_of;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::{AgentResume, AgentStart, SessionId};

/// The window a standing Sleeper's run answers, and whether it resumes the last attempt.
pub(super) struct WindowRun {
    /// The window, as its label (`20261025T0300Z`).
    pub(super) window: String,
    /// Whether the Sleeper says `resume = true`.
    pub(super) resume: bool,
}

/// The standing Sleeper window the Mission's claimed brief answers, when the Registry kept a
/// Sleeper provenance for that brief; `None` for any other Mission.
pub(super) fn of(state: &DaemonState, mission_id: &str) -> Option<WindowRun> {
    let brief = state.intake.read_claimed(mission_id).ok()??;
    sleeper_of(&brief)?;
    let origin = brief.data.get("sleeper")?;
    let flag = |key: &str| origin.get(key).and_then(serde_json::Value::as_bool) == Some(true);
    if !flag("standing") {
        return None;
    }
    Some(WindowRun {
        window: window_of(&brief)?.to_owned(),
        resume: flag("resume"),
    })
}

/// The provider Session a run that resumes takes up: the last attempt's, when that attempt ran
/// on `handler` and its provider gave it an ID. Which it does goes on the timeline.
pub(super) fn resume_from(
    state: &DaemonState,
    (operation, mission_id): (Option<&str>, &str),
    run: &WindowRun,
    earlier: Option<&SessionRecord>,
    handler: &str,
) -> Option<SessionId> {
    if !run.resume {
        return None;
    }
    let found = earlier
        .filter(|record| {
            record
                .launch
                .as_ref()
                .is_some_and(|launch| launch.handler == handler)
        })
        .and_then(|record| record.provider_session_id.clone())
        .and_then(|id| SessionId::try_new(id).ok());
    let (action, detail) = match &found {
        Some(id) => (
            "resumed",
            format!(
                "the window resumes the last attempt's provider Session {}",
                id.as_str()
            ),
        ),
        None => (
            "started",
            "the window starts a new Session: the last attempt left none its handler can resume"
                .to_owned(),
        ),
    };
    if let Some(operation) = operation {
        let event = TimelineEvent::new("sleeper", action, operation, state.clock.now() as f64)
            .mission(mission_id)
            .detail("window", run.window.clone())
            .detail("detail", detail);
        let _ = state.intake.record_event(&event);
    }
    found
}

/// `start` as a resume of the provider Session `session_id`, the window's brief its message.
pub(super) fn resume_request(start: AgentStart, session_id: SessionId) -> AgentResume {
    AgentResume {
        mission_id: start.mission_id,
        session_id,
        message: Some(start.prompt),
        working_directory: start.working_directory,
        clearance: start.clearance,
        environment: start.environment,
        model: start.model,
        effort: start.effort,
        max_cost_micro_usd: start.max_cost_micro_usd,
        project_settings: start.project_settings,
        project_mcp: start.project_mcp,
        answer_only: start.answer_only,
        assistant: start.assistant,
        initial_context: start.initial_context,
    }
}

#[cfg(test)]
pub(super) mod testing {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::directorates::m::identity::SessionId;
    use crate::directorates::q::terminal::AgentTerminal;
    use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart};
    use crate::guatiao::library::{Object, ProviderError};
    use crate::guatiao::{Provider, Status};
    use coqui_acp_provider::AgentSession;

    use super::super::tests::MockSession;

    /// Each resume the handler was asked for, as (Mission, provider Session).
    pub(in crate::station::server) static RESUMED: Mutex<Vec<(String, String)>> =
        Mutex::new(Vec::new());
    static STARTS: AtomicUsize = AtomicUsize::new(0);

    /// A harness that resumes by provider Session ID, as Claude Code does: each start is a new
    /// provider Session and each turn completes the Mission.
    #[derive(Default, Provider)]
    #[provider(
        AgentHandler,
        id = "mock-standing",
        name = "Mock Standing",
        crate = crate::guatiao
    )]
    pub(in crate::station::server) struct MockStandingHandler;

    impl AgentHandler for MockStandingHandler {
        fn start(
            &self,
            _request: AgentStart,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            let started = STARTS.fetch_add(1, Ordering::Relaxed);
            let session =
                SessionId::try_new(format!("provider-standing-{started}")).map_err(|error| {
                    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string())
                })?;
            Ok(MockSession::new(session, "completed").into_object())
        }

        fn resume(
            &self,
            request: AgentResume,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            RESUMED.lock().unwrap().push((
                request.mission_id.as_str().to_owned(),
                request.session_id.as_str().to_owned(),
            ));
            Ok(MockSession::new(request.session_id, "completed").into_object())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::tests::{test_state, wait_for_state};
    use super::super::{publish_station_check_in, schedule_intake_once, schedule_queued_once};
    use super::*;
    use crate::directorates::m::mission::MissionState;
    use crate::directorates::registry::mission_ids::MissionEntry;
    use crate::directorates::registry::sleepers::window_label;
    use crate::station::sources::fire_due_sleepers;

    /// 2026-10-25T03:00:00Z, a window of the `0 3 * * *` Sleeper below.
    const FIRST: i64 = 1_792_897_200;
    const DAY: i64 = 86_400;

    /// A Station whose Operation `G` has the authorized standing Sleeper `nightly`.
    fn standing_state(root: &std::path::Path, resume: bool) -> DaemonState {
        let mut state = test_state(
            root,
            &format!(
                "[operation.G.sleepers.nightly]\nschedule = \"0 3 * * *\"\ntitle = \"Tidy\"\n\
                 prompt = \"Tidy the tree.\"\nagent = \"007\"\nstanding = true\nresume = {resume}\n"
            ),
            "[agent.007]\nhandler = \"mock-standing\"\n",
        );
        // Only the Sleeper's Mission, on a Station that has checked in as placement reads it.
        state.missions = std::sync::Arc::new(std::sync::Mutex::new(Default::default()));
        publish_station_check_in(&state);
        let sleeper = state.roster.operation("G").unwrap().sleepers["nightly"].clone();
        state
            .intake
            .authorize_sleeper("G", "nightly", &sleeper, "operator", FIRST - DAY)
            .unwrap();
        state
    }

    /// The standing Mission's UID.
    fn standing_mission(state: &DaemonState) -> String {
        state
            .intake
            .entered_mission_id(&MissionEntry::standing("G", "nightly", None))
            .unwrap()
            .expect("the standing Mission entered")
    }

    /// Runs the queued attempt of the standing Mission to its end and files it.
    async fn run_attempt(state: &DaemonState) -> String {
        assert!(schedule_intake_once(state));
        let mission = standing_mission(state);
        schedule_queued_once(state).await;
        wait_for_state(state, &mission, MissionState::Done).await;
        schedule_intake_once(state);
        assert_eq!(
            state.intake.filed_brief(&mission).unwrap().unwrap().state,
            "done"
        );
        mission
    }

    /// Fires the window at `window` and runs its attempt to its end; the standing Mission.
    async fn run_window(state: &DaemonState, window: i64, warned: &mut BTreeSet<String>) -> String {
        assert_eq!(
            fire_due_sleepers(
                state.intake.as_ref(),
                &state.roster,
                "test",
                window + 60,
                warned
            ),
            1,
            "window {}",
            window_label(window)
        );
        run_attempt(state).await
    }

    fn runs(state: &DaemonState, mission: &str) -> Vec<SessionRecord> {
        let mut runs: Vec<SessionRecord> = state
            .intake
            .sessions()
            .unwrap()
            .into_iter()
            .filter(|record| record.mission_id == mission)
            .collect();
        runs.sort_by_key(|record| record.attempt);
        runs
    }

    #[tokio::test]
    async fn three_windows_are_attempts_of_one_mission_and_an_overlapping_one_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        let state = standing_state(root.path(), false);
        let mut warned = BTreeSet::new();
        let fire = |now: i64, warned: &mut BTreeSet<String>| {
            fire_due_sleepers(state.intake.as_ref(), &state.roster, "test", now, warned)
        };

        assert_eq!(fire(FIRST + 60, &mut warned), 1);
        assert_eq!(
            fire(FIRST + DAY + 60, &mut warned),
            0,
            "the next window comes while the first attempt is queued"
        );
        let skipped = format!("skipped/G/nightly//{}", window_label(FIRST + DAY));
        assert!(warned.contains(&skipped), "{warned:?}");
        let mission = run_attempt(&state).await;

        for window in [FIRST + DAY, FIRST + 2 * DAY] {
            assert_eq!(run_window(&state, window, &mut warned).await, mission);
        }
        let runs = runs(&state, &mission);
        let labels = [FIRST, FIRST + DAY, FIRST + 2 * DAY].map(window_label);
        assert_eq!(
            runs.iter()
                .map(|run| (run.attempt, run.handle.as_deref(), run.window.as_deref()))
                .collect::<Vec<_>>(),
            [
                (1, Some("G-nightly-1.1"), Some(labels[0].as_str())),
                (2, Some("G-nightly-1.2"), Some(labels[1].as_str())),
                (3, Some("G-nightly-1.3"), Some(labels[2].as_str())),
            ]
        );
        assert_eq!(
            runs[1].predecessor.as_deref(),
            Some(runs[0].session_id.as_str())
        );
        assert!(
            testing::RESUMED
                .lock()
                .unwrap()
                .iter()
                .all(|(resumed, _)| *resumed != mission),
            "without resume each attempt starts a new Session"
        );
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .list()
                .iter()
                .filter(|listed| listed.handle.as_deref() == Some("G-nightly-1"))
                .count(),
            1,
            "one standing Mission on the board"
        );
        state
            .stopping
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// A lease that lapsed after its Mission ended here, recovered and claimed again, is the
    /// same attempt: its outcome is filed and nothing runs twice.
    #[tokio::test]
    async fn the_same_brief_claimed_again_after_a_lapse_is_filed_and_not_run_again() {
        let root = tempfile::tempdir().unwrap();
        let state = standing_state(root.path(), false);
        let mut warned = BTreeSet::new();
        fire_due_sleepers(
            state.intake.as_ref(),
            &state.roster,
            "test",
            FIRST + 60,
            &mut warned,
        );
        assert!(schedule_intake_once(&state));
        let mission = standing_mission(&state);
        schedule_queued_once(&state).await;
        wait_for_state(&state, &mission, MissionState::Done).await;

        assert!(
            state
                .intake
                .renew(&mission, "test", std::time::Duration::from_nanos(1))
                .unwrap()
        );
        let lapsed = loop {
            if let Some(lease) = state.intake.expired().unwrap().into_iter().next() {
                break lease;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        assert_eq!(lapsed.attempt(), 1);
        state.intake.recover(&lapsed, 5, "other").unwrap().unwrap();

        assert!(schedule_intake_once(&state));
        assert!(schedule_intake_once(&state));
        assert_eq!(
            state.intake.filed_brief(&mission).unwrap().unwrap().state,
            "done"
        );
        schedule_queued_once(&state).await;
        assert_eq!(runs(&state, &mission).len(), 1, "it ran once");
        state
            .stopping
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[tokio::test]
    async fn a_resuming_window_takes_up_the_last_attempts_provider_session() {
        let root = tempfile::tempdir().unwrap();
        let state = standing_state(root.path(), true);
        let mut warned = BTreeSet::new();
        let mission = run_window(&state, FIRST, &mut warned).await;
        assert_eq!(run_window(&state, FIRST + DAY, &mut warned).await, mission);
        let runs = runs(&state, &mission);
        assert_eq!(runs.len(), 2);
        let first = runs[0]
            .provider_session_id
            .clone()
            .expect("a provider Session");
        assert_eq!(runs[1].provider_session_id.as_deref(), Some(first.as_str()));
        assert_eq!(
            testing::RESUMED
                .lock()
                .unwrap()
                .iter()
                .filter(|(resumed, _)| *resumed == mission)
                .map(|(_, session)| session.clone())
                .collect::<Vec<_>>(),
            [first]
        );
        let said: Vec<String> = state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "sleeper" && event.mission.as_deref() == Some(&mission))
            .map(|event| event.action)
            .collect();
        assert_eq!(said, ["started", "resumed"]);
        state
            .stopping
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
