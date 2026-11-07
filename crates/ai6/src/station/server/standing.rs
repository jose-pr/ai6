//! A standing Sleeper's runs: each window is a new attempt of the Sleeper's one Mission, and one
//! that resumes takes up the provider Session the last attempt left. A Sleeper's attempt that fails
//! says so on the timeline and the board, and a window that finds its order's attempt stalled
//! supersedes it.

use super::{DaemonState, is_terminal_state, session_hold};
use crate::directorates::m::mission::{Mission, MissionState};
use crate::directorates::m::vetting::sleeper_of;
use crate::directorates::registry::bona_fides::Origin;
use crate::directorates::registry::sessions::{
    SessionRecord, SessionResult, SessionStatus, SessionUpdate,
};
use crate::directorates::registry::sleepers::{window_label, window_of};
use crate::directorates::registry::store::{AuditRecord, Brief};
use crate::directorates::registry::timeline::TimelineEvent;
use crate::{AgentResume, AgentStart, SessionId};

/// The key an attempt's outcome sets when its `reason` is what its harness said, which may quote
/// anything and stays off the timeline.
pub(super) const HARNESS_SAID: &str = "reason_from_harness";

/// The reason the board gets with a Sleeper's failed attempt.
#[cfg(feature = "publisher")]
pub(super) const ORDER_FAILED_REASON: &str = "order-attempt-failed";

/// The reason on the Session record of an attempt a later window superseded.
const SUPERSEDED: &str = "superseded";

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

/// The provider Session a run that resumes takes up: the last attempt's, when that attempt ran on
/// `handler`, did not fail, and its provider gave it an ID, unless the provider `refused` it this
/// attempt. A failed attempt's Session may be why it failed, and resuming it could fail every
/// window alike. That it failed is read from its Session record, or `after_failure` from this
/// Station's book, which can say so first. Which it does goes on the timeline.
pub(super) fn resume_from(
    state: &DaemonState,
    (operation, mission_id): (Option<&str>, &str),
    run: &WindowRun,
    earlier: Option<&SessionRecord>,
    (handler, refused, after_failure): (&str, bool, bool),
) -> Option<SessionId> {
    if !run.resume {
        return None;
    }
    let failed = after_failure
        || earlier
            .and_then(|record| record.result.as_ref())
            .is_some_and(|result| {
                result.state == "failed" || result.reason.as_deref() == Some(SUPERSEDED)
            });
    let found = earlier
        .filter(|_| !refused && !failed)
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
            if refused {
                "the window starts a new Session: the last attempt's could not be resumed"
            } else if failed {
                "the window starts a new Session: the last attempt failed, so its is not taken up"
            } else {
                "the window starts a new Session: the last attempt left none its handler can resume"
            }
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
        overlays: start.overlays,
    }
}

/// Notes that the provider could not resume the Session this attempt would have taken up: the
/// scheduler's next try starts a new one, rather than asking again for what will not come back.
pub(super) fn resume_refused(state: &DaemonState, mission_id: &str, why: &str) {
    eprintln!("ai6: Mission {mission_id:?} could not resume its last Session: {why}");
    if let Ok(mut missions) = state.missions.lock()
        && let Err(error) = missions.refuse_resume(mission_id, why)
    {
        eprintln!("ai6: Mission {mission_id:?}: the refused resume is not recorded: {error:?}");
    }
}

/// Puts a failed attempt of a Sleeper on the timeline, needing no one's approval: the order stands
/// and its next window fires, so nothing else would tell of it. A cancel is an operator's own doing.
pub(super) fn attempt_failed(state: &DaemonState, brief: &Brief, outcome: &serde_json::Value) {
    if outcome["state"] == "abandoned" {
        return;
    }
    let (Some((operation, name)), Some(window)) = (sleeper_of(brief), window_of(brief)) else {
        return;
    };
    let reason = if outcome[HARNESS_SAID] == true {
        "its harness reported a failure; its words are in the attempt's outcome"
    } else {
        outcome["reason"]
            .as_str()
            .unwrap_or("no reason was recorded")
    };
    let limit = state.intake.limits().timeline.text_bytes.or_max();
    let mut kept = String::new();
    for character in reason.chars().filter(|character| !character.is_control()) {
        if kept.len() + character.len_utf8() > limit {
            break;
        }
        kept.push(character);
    }
    let mut event = TimelineEvent::new(
        "sleeper",
        "attempt-failed",
        operation,
        state.clock.now() as f64,
    )
    .station(state.station_id.clone())
    .mission(brief.key())
    .detail("sleeper", name)
    .detail("window", window.to_owned());
    if !kept.trim().is_empty() {
        event = event.detail("reason", kept);
    }
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!(
            "ai6: timeline event of Mission {:?} was not written: {error}",
            brief.key()
        );
    }
}

/// [`attempt_failed`] for a brief a recovered lease filed, when its Station was lost.
pub(super) fn recovered_failed(state: &DaemonState, key: &str) {
    if let Ok(Some(filed)) = state.intake.filed_brief(key)
        && let Some(outcome) = &filed.outcome
    {
        attempt_failed(state, &filed.brief, outcome);
    }
}

/// An attempt a later window takes the place of.
struct Stalled {
    mission_id: String,
    operation: String,
    /// The window the attempt answers.
    window: String,
    /// The window that supersedes it.
    next: String,
    why: String,
    /// Whether a stall of its Agent's was already counted for this attempt.
    counted: bool,
}

/// Why `mission`, an attempt held by this Station, cannot be expected to end by itself. The stall
/// bound is its Agent's `stall_timeout`, which is what makes it `stalled`. A Mission waiting for a
/// person, a provider's limit or an operator's release is not stalled, nor is one a re-attach may
/// still bring back.
fn why_stalled(state: &DaemonState, mission: &Mission) -> Option<String> {
    let id = mission.mission_id.as_str();
    let failed_retry = state
        .retry_after
        .lock()
        .ok()
        .is_some_and(|retries| retries.contains_key(id));
    match mission.state {
        MissionState::Stalled => Some("its Session stalled".to_owned()),
        MissionState::Queued => failed_retry.then(|| "it did not start".to_owned()),
        MissionState::Working | MissionState::Blocked => {
            let live = state
                .sessions
                .lock()
                .ok()
                .is_none_or(|sessions| sessions.contains_key(id))
                || state
                    .starting
                    .lock()
                    .ok()
                    .is_none_or(|starting| starting.contains(id));
            let reattaching = state
                .startup_recovery
                .lock()
                .ok()
                .is_none_or(|pending| pending.contains(id))
                && !failed_retry;
            (!live && !reattaching && mission.blocked_reason.is_none())
                .then(|| format!("no live Session on Station {}", state.station_id))
        }
        _ => None,
    }
}

/// The attempts in this Station's book whose order's next window has come while they cannot end.
fn stalled_attempts(state: &DaemonState) -> Vec<Stalled> {
    let book: Vec<Mission> = state
        .missions
        .lock()
        .map(|missions| {
            missions
                .list()
                .into_iter()
                .filter(|mission| {
                    !is_terminal_state(mission.state)
                        && mission
                            .provenance
                            .as_ref()
                            .is_some_and(|provenance| provenance.origin == Origin::Sleeper)
                })
                .collect()
        })
        .unwrap_or_default();
    let limits = &state.intake.limits().sleepers;
    let now = state.clock.now();
    book.iter()
        .filter_map(|mission| {
            let why = why_stalled(state, mission)?;
            let id = mission.mission_id.as_str();
            let brief = state.intake.read_claimed(id).ok()??;
            let (operation, name) = sleeper_of(&brief)?;
            let run = of(state, id)?;
            let lease = state.intake.lease(id).ok()??;
            if lease.station() != state.station_id {
                return None;
            }
            let sleeper = state
                .roster
                .operation(&operation)
                .ok()?
                .sleepers
                .get(&name)?;
            let approval = state
                .intake
                .sleeper_authorization(&operation, &name)
                .ok()??;
            if approval.fingerprint != sleeper.fingerprint() {
                return None;
            }
            let since = approval.mark.map_or(approval.at, |mark| mark.since);
            let next = window_label(sleeper.due_window(now, since, limits)?);
            (next > run.window).then(|| Stalled {
                mission_id: id.to_owned(),
                operation,
                window: run.window,
                next,
                why,
                counted: mission.state == MissionState::Stalled,
            })
        })
        .collect()
}

/// Settles each attempt in this Station's book whose order's next window finds it stalled: it fails
/// with that reason, audited, its harness stopped, and the next intake pass files its brief, which
/// lets the window fire as the next attempt. Without this the order waits for a person.
pub(super) async fn supersede_stalled(state: &DaemonState) {
    let reading = state.clone();
    let stalled = tokio::task::spawn_blocking(move || stalled_attempts(&reading))
        .await
        .unwrap_or_default();
    for attempt in stalled {
        supersede(state, attempt).await;
    }
}

async fn supersede(state: &DaemonState, attempt: Stalled) {
    let id = attempt.mission_id.as_str();
    let reason = format!("superseded by window {}: {}", attempt.next, attempt.why);
    let hold = session_hold(state, id);
    let failed = state
        .missions
        .lock()
        .is_ok_and(|mut missions| missions.fail_for(id, &reason).is_ok());
    if !failed {
        return;
    }
    eprintln!("ai6: Mission {id:?}: {reason}");
    if let Ok(mut retries) = state.retry_after.lock() {
        retries.remove(id);
    }
    if let Ok(mut pending) = state.startup_recovery.lock() {
        pending.remove(id);
    }
    super::recovery::reattached(state, id);
    if let Some(hold) = &hold {
        // Not the Agent's failure: what stopped it is a stall already counted, or the Station.
        let update = SessionUpdate::default()
            .result(SessionResult::new("abandoned").with_reason(Some(SUPERSEDED.to_owned())))
            .status(SessionStatus::Ended);
        if let Err(error) =
            state
                .intake
                .update_session(&hold.id, &state.station_id, hold.fence, update)
        {
            eprintln!("ai6: Session record {:?} not updated: {error}", hold.id);
        }
        if !attempt.counted {
            super::agents::count_lost(state, id, hold.attempt);
        }
    }
    super::publish_lifecycle(state, id, MissionState::Failed);
    let at = state.clock.now() as f64;
    let record = AuditRecord {
        id: format!("sleeper-superseded-{}", uuid::Uuid::new_v4().simple()),
        actor: "m".to_owned(),
        action: "sleeper.superseded".to_owned(),
        operation: attempt.operation.clone(),
        resource: None,
        target: format!("{id} {}", attempt.window),
        at,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: the supersede of Mission {id:?} was not audited: {error}");
    }
    let event = TimelineEvent::new("sleeper", "superseded", &attempt.operation, at)
        .station(state.station_id.clone())
        .mission(id)
        .detail("window", attempt.window.clone())
        .detail("by", attempt.next.clone())
        .detail("why", attempt.why.clone());
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: timeline event of Mission {id:?} was not written: {error}");
    }
    if super::finalize_mission_resources(state, id, true, false)
        .await
        .is_err()
    {
        eprintln!("ai6: Mission {id:?}: the superseded attempt's resources were not released");
    }
}

#[cfg(test)]
pub(super) mod testing {
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::directorates::m::identity::SessionId;
    use crate::directorates::q::terminal::AgentTerminal;
    use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart};
    use crate::guatiao::library::{Object, ProviderError};
    use crate::guatiao::{Provider, Status};
    use coqui_acp_provider::{AgentEvent, AgentSession};

    use super::super::tests::MockSession;

    /// Each resume the handler was asked for, as (Mission, provider Session).
    pub(in crate::station::server) static RESUMED: Mutex<Vec<(String, String)>> =
        Mutex::new(Vec::new());
    static STARTS: AtomicUsize = AtomicUsize::new(0);
    /// The brief context of each start or resume, as (Mission, context), in the order they came.
    pub(in crate::station::server) static BRIEFS: Mutex<Vec<(String, Vec<String>)>> =
        Mutex::new(Vec::new());
    /// The Missions whose first attempt has gone the way its prompt asks.
    static FIRST_DONE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

    fn event(kind: &str, text: &str) -> AgentEvent {
        AgentEvent {
            kind: kind.to_owned(),
            text: Some(text.to_owned()),
            data: None,
            limit: None,
            usage: None,
            quota: Vec::new(),
            tools: Vec::new(),
        }
    }

    /// A harness that resumes by provider Session ID, as Claude Code does: each start is a new
    /// provider Session and each turn completes the Mission. A word in the prompt changes that:
    /// `first-attempt-fails`, `first-attempt-stalls` and `first-attempt-blocks` for a Mission's
    /// first start, `resume-refused` and `resume-dead` for every resume.
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
            request: AgentStart,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            let started = STARTS.fetch_add(1, Ordering::Relaxed);
            BRIEFS.lock().unwrap().push((
                request.mission_id.as_str().to_owned(),
                request.initial_context.clone(),
            ));
            let session =
                SessionId::try_new(format!("provider-standing-{started}")).map_err(|error| {
                    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string())
                })?;
            let first = FIRST_DONE
                .lock()
                .unwrap()
                .insert(request.mission_id.as_str().to_owned());
            let asked = |word: &str| first && request.prompt.contains(word);
            let session = if asked("first-attempt-fails") {
                MockSession::with_event(session, event("failed", "the model refused the turn"))
            } else if asked("first-attempt-stalls") {
                MockSession::new(session, "stalled")
            } else if asked("first-attempt-blocks") {
                MockSession::new(session, "blocked")
            } else {
                MockSession::new(session, "completed")
            };
            Ok(session.into_object())
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
            BRIEFS.lock().unwrap().push((
                request.mission_id.as_str().to_owned(),
                request.initial_context.clone(),
            ));
            let message = request.message.as_deref().unwrap_or_default();
            if message.contains("resume-refused") {
                return Err(ProviderError::new(
                    Status::GUATIAO_ERR_INTERNAL,
                    "no conversation found",
                ));
            }
            let session = if message.contains("resume-dead") {
                MockSession::with_event(
                    request.session_id,
                    event("failed", "no conversation found"),
                )
            } else {
                MockSession::new(request.session_id, "completed")
            };
            Ok(session.into_object())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::tests::{test_state, wait_for_done, wait_for_state};
    use super::super::{publish_station_check_in, schedule_intake_once, schedule_queued_once};
    use super::*;
    use crate::directorates::m::mission::MissionState;
    use crate::directorates::registry::mission_ids::MissionEntry;
    use crate::station::sources::fire_due_sleepers;

    /// 2026-10-25T03:00:00Z, a window of the `0 3 * * *` Sleeper below.
    const FIRST: i64 = 1_792_897_200;
    const DAY: i64 = 86_400;

    /// A Station whose Operation `G` has the authorized standing Sleeper `nightly`.
    fn standing_state(root: &std::path::Path, resume: bool) -> DaemonState {
        order_state(root, "Tidy the tree.", resume)
    }

    /// [`standing_state`], the Sleeper's prompt saying how [`testing::MockStandingHandler`] goes.
    fn order_state(root: &std::path::Path, prompt: &str, resume: bool) -> DaemonState {
        let mut state = test_state(
            root,
            &format!(
                "[operation.G.sleepers.nightly]\nschedule = \"0 3 * * *\"\ntitle = \"Tidy\"\n\
                 prompt = \"{prompt}\"\nagent = \"007\"\nstanding = true\nresume = {resume}\n"
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
        wait_for_done(state, &mission).await;
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

    /// Each window's brief carries the Cables sent to the Agent since the last one, once.
    #[cfg(feature = "cables-mcp")]
    #[tokio::test]
    async fn a_windows_brief_carries_the_cables_sent_since_the_last_window() {
        use crate::directorates::registry::cables::{
            CablePrincipal, CableScope, CableSendRequest, CableState, CableTarget,
        };

        let root = tempfile::tempdir().unwrap();
        let mut state = standing_state(root.path(), false);
        super::super::cable_wake_tests::open_cables(&mut state);
        let mut warned = BTreeSet::new();
        let sender = CablePrincipal::try_new(
            "008".to_owned(),
            "G".to_owned(),
            None,
            "test".to_owned(),
            None,
            None,
        )
        .unwrap();
        let send = |body: &str| {
            state
                .cables
                .send(
                    &sender,
                    CableSendRequest::new(
                        CableTarget::try_new(CableScope::Agent, Some("007".to_owned())).unwrap(),
                        body.to_owned(),
                        format!("key-{body}"),
                    ),
                )
                .unwrap()
                .id()
                .to_owned()
        };
        let before = send("sent before the first window");
        let mission = {
            assert_eq!(
                fire_due_sleepers(
                    state.intake.as_ref(),
                    &state.roster,
                    "test",
                    FIRST + 60,
                    &mut warned
                ),
                1
            );
            run_attempt(&state).await
        };
        let between = send("sent between the windows");
        run_window(&state, FIRST + DAY, &mut warned).await;

        let briefs: Vec<String> = testing::BRIEFS
            .lock()
            .unwrap()
            .iter()
            .filter(|(of, _)| *of == mission)
            .map(|(_, context)| {
                context.join(
                    "
",
                )
            })
            .collect();
        assert_eq!(briefs.len(), 2, "{briefs:?}");
        assert!(briefs[0].contains("sent before the first window"));
        assert!(!briefs[0].contains("sent between the windows"));
        assert!(briefs[1].contains("sent between the windows"));
        assert!(
            !briefs[1].contains("sent before the first window"),
            "a Cable is in one brief"
        );
        for cable in [before, between] {
            let held = state.intake.get_cable(&cable).unwrap().unwrap();
            assert_eq!(held.recipients()[0].state(), CableState::Delivered);
        }
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
            "the Cables made no Mission: one standing Mission on the board"
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
        wait_for_done(&state, &mission).await;

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

    /// Fires what is due at `now`.
    fn fire_at(state: &DaemonState, now: i64, warned: &mut BTreeSet<String>) -> usize {
        fire_due_sleepers(state.intake.as_ref(), &state.roster, "test", now, warned)
    }

    /// The Sleeper events named `action` on the timeline.
    fn told(state: &DaemonState, action: &str) -> Vec<TimelineEvent> {
        state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "sleeper" && event.action == action)
            .collect()
    }

    fn stop(state: &DaemonState) {
        state
            .stopping
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// The Station's clock stands at `now`.
    fn at(state: &mut DaemonState, now: i64) {
        state.clock = super::super::Clock(std::sync::Arc::new(move || now));
    }

    #[tokio::test]
    async fn a_failed_attempt_leaves_the_order_standing_and_says_why() {
        let root = tempfile::tempdir().unwrap();
        let state = order_state(root.path(), "first-attempt-fails the tree.", false);
        let mut warned = BTreeSet::new();
        assert_eq!(fire_at(&state, FIRST + 60, &mut warned), 1);
        assert!(schedule_intake_once(&state));
        let mission = standing_mission(&state);
        schedule_queued_once(&state).await;
        wait_for_state(&state, &mission, MissionState::Failed).await;
        schedule_intake_once(&state);

        let filed = state.intake.filed_brief(&mission).unwrap().unwrap();
        assert_eq!(filed.state, "failed");
        let outcome = filed.outcome.unwrap();
        assert_eq!(outcome["reason"], "the model refused the turn");

        #[cfg(feature = "publisher")]
        {
            let reason = super::super::order_reason(&state, &mission);
            assert_eq!(reason, Some(ORDER_FAILED_REASON));
            assert!(ai6_diptel::valid_reason(ORDER_FAILED_REASON));
            assert_eq!(
                super::super::order_reason(&state, "mission-nobody"),
                None,
                "nothing else is told as an order"
            );
        }
        let alerts = told(&state, "attempt-failed");
        assert_eq!(alerts.len(), 1, "one alert for the attempt");
        assert_eq!(alerts[0].mission.as_deref(), Some(mission.as_str()));
        assert_eq!(alerts[0].detail["sleeper"], "nightly");
        assert_eq!(alerts[0].detail["window"], window_label(FIRST));
        assert!(
            !alerts[0].detail["reason"].contains("the model refused"),
            "the timeline keeps what a harness says out: {:?}",
            alerts[0].detail
        );
        assert!(
            state
                .intake
                .audit_records()
                .unwrap()
                .iter()
                .all(|record| !record.action.starts_with("green_ink")),
            "no green ink is asked for"
        );

        assert_eq!(
            run_window(&state, FIRST + DAY, &mut warned).await,
            mission,
            "the next window fires as the next attempt"
        );
        let runs = runs(&state, &mission);
        assert_eq!(
            runs.iter().map(|run| run.attempt).collect::<Vec<_>>(),
            [1, 2]
        );
        stop(&state);
    }

    /// A cancelled attempt is an operator's own doing, and asks for no alert.
    #[test]
    fn a_cancelled_attempt_raises_no_alert() {
        let root = tempfile::tempdir().unwrap();
        let state = standing_state(root.path(), false);
        let sleeper = state.roster.operation("G").unwrap().sleepers["nightly"].clone();
        let brief = crate::directorates::registry::sleepers::sleeper_brief(
            "G", "nightly", &sleeper, FIRST, None,
        );
        attempt_failed(
            &state,
            &brief,
            &serde_json::json!({"state": "abandoned", "reason": "cancelled"}),
        );
        assert!(told(&state, "attempt-failed").is_empty());
    }

    /// A provider that refuses the resume leaves the attempt to start a new Session, and the
    /// order does not sit in the start's retries.
    #[tokio::test]
    async fn a_resume_the_provider_refuses_falls_back_to_a_new_session_and_says_so() {
        let root = tempfile::tempdir().unwrap();
        let state = order_state(root.path(), "resume-refused the tree.", true);
        let mut warned = BTreeSet::new();
        let mission = run_window(&state, FIRST, &mut warned).await;

        assert_eq!(fire_at(&state, FIRST + DAY + 60, &mut warned), 1);
        assert!(schedule_intake_once(&state));
        schedule_queued_once(&state).await;
        state.retry_after.lock().unwrap().clear();
        schedule_queued_once(&state).await;
        assert_ne!(
            super::super::tests::mission_state(&state, &mission).state,
            MissionState::Queued,
            "the attempt must not keep asking for a Session that cannot resume"
        );
        wait_for_done(&state, &mission).await;
        schedule_intake_once(&state);

        let runs = runs(&state, &mission);
        assert_eq!(
            runs.iter().map(|run| run.attempt).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_ne!(runs[0].provider_session_id, runs[1].provider_session_id);
        let said: Vec<(String, String)> = state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "sleeper" && event.mission.as_deref() == Some(&mission))
            .map(|event| (event.action, event.detail["detail"].clone()))
            .collect();
        assert_eq!(
            said.iter()
                .map(|(action, _)| action.as_str())
                .collect::<Vec<_>>(),
            ["started", "resumed", "started"]
        );
        assert!(said[2].1.contains("could not be resumed"), "{said:?}");
        stop(&state);
    }

    /// The book says an attempt failed before its Session record does: the record's result is
    /// written after the Mission's state, so a start in between still reads the attempt as done.
    #[tokio::test]
    async fn a_failure_the_book_holds_keeps_the_next_window_off_its_session() {
        let root = tempfile::tempdir().unwrap();
        let state = order_state(root.path(), "Tidy the tree.", true);
        let mut warned = BTreeSet::new();
        let mission = run_window(&state, FIRST, &mut warned).await;
        // Its result may not be written yet; its provider Session is, from when it opened.
        let done = runs(&state, &mission).remove(0);
        let run = WindowRun {
            window: window_label(FIRST + DAY),
            resume: true,
        };
        let handler = "mock-standing";
        let at = (Some("G"), mission.as_str());
        assert_eq!(
            resume_from(&state, at, &run, Some(&done), (handler, false, false)),
            done.provider_session_id
                .clone()
                .map(|id| SessionId::try_new(id).unwrap())
        );
        assert_eq!(
            resume_from(&state, at, &run, Some(&done), (handler, false, true)),
            None
        );
        let said = told(&state, "started");
        assert!(
            said.last().unwrap().detail["detail"].contains("the last attempt failed"),
            "{said:?}"
        );
        stop(&state);
    }

    /// A harness can take the resume and then fail on it, each window alike; only a new Session
    /// gets the order out of that.
    #[tokio::test]
    async fn a_session_whose_attempt_failed_is_not_resumed_by_the_next_window() {
        let root = tempfile::tempdir().unwrap();
        let state = order_state(root.path(), "resume-dead the tree.", true);
        let mut warned = BTreeSet::new();
        let mission = run_window(&state, FIRST, &mut warned).await;

        assert_eq!(fire_at(&state, FIRST + DAY + 60, &mut warned), 1);
        assert!(schedule_intake_once(&state));
        schedule_queued_once(&state).await;
        wait_for_state(&state, &mission, MissionState::Failed).await;
        schedule_intake_once(&state);
        let filed = state.intake.filed_brief(&mission).unwrap().unwrap();
        assert_eq!(
            filed.outcome.unwrap()["reason"],
            "no conversation found",
            "the attempt's outcome carries the reason"
        );

        assert_eq!(
            run_window(&state, FIRST + 2 * DAY, &mut warned).await,
            mission,
            "the third window runs on a new Session"
        );
        let runs = runs(&state, &mission);
        assert_eq!(runs.len(), 3);
        assert_ne!(runs[2].provider_session_id, runs[0].provider_session_id);
        stop(&state);
    }

    #[tokio::test]
    async fn a_window_supersedes_a_stalled_attempt_and_runs_as_the_next_attempt() {
        let root = tempfile::tempdir().unwrap();
        let mut state = order_state(root.path(), "first-attempt-stalls the tree.", false);
        let mut warned = BTreeSet::new();
        at(&mut state, FIRST + 60);
        assert_eq!(fire_at(&state, FIRST + 60, &mut warned), 1);
        assert!(schedule_intake_once(&state));
        let mission = standing_mission(&state);
        schedule_queued_once(&state).await;
        wait_for_state(&state, &mission, MissionState::Stalled).await;

        // Its own window has not passed: it is left alone.
        supersede_stalled(&state).await;
        assert_eq!(
            super::super::tests::mission_state(&state, &mission).state,
            MissionState::Stalled
        );

        at(&mut state, FIRST + DAY + 60);
        assert_eq!(fire_at(&state, FIRST + DAY + 60, &mut warned), 0);
        supersede_stalled(&state).await;
        let superseded = super::super::tests::mission_state(&state, &mission);
        assert_eq!(superseded.state, MissionState::Failed);
        let reason = superseded.failure.unwrap();
        assert!(
            reason.contains(&window_label(FIRST + DAY)) && reason.contains("stalled"),
            "{reason}"
        );
        assert!(
            !state.sessions.lock().unwrap().contains_key(&mission),
            "its harness is stopped"
        );
        schedule_intake_once(&state);
        let filed = state.intake.filed_brief(&mission).unwrap().unwrap();
        assert_eq!(filed.state, "failed");
        assert_eq!(filed.outcome.unwrap()["reason"], reason.as_str());
        assert!(
            state
                .intake
                .audit_records()
                .unwrap()
                .iter()
                .any(|record| record.action == "sleeper.superseded"
                    && record.target.starts_with(&mission)),
            "the supersede is audited"
        );
        assert_eq!(told(&state, "superseded").len(), 1);

        assert_eq!(fire_at(&state, FIRST + DAY + 80, &mut warned), 1);
        run_attempt(&state).await;
        let runs = runs(&state, &mission);
        assert_eq!(
            runs.iter().map(|run| run.attempt).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(
            crate::directorates::m::scorecard::judge(&runs[0]),
            crate::directorates::m::scorecard::Judgment::Withdrawn,
            "a stall already counted is not counted again as the Agent's failure"
        );
        stop(&state);
    }

    /// A restart leaves the Mission blocked and waiting to re-attach. While a re-attach has not
    /// failed it may still come back; a harness that cannot be re-attached wedges the order until
    /// the next window.
    #[tokio::test]
    async fn a_window_supersedes_an_attempt_with_no_live_session_after_a_restart() {
        let root = tempfile::tempdir().unwrap();
        let mut state = order_state(root.path(), "first-attempt-blocks the tree.", false);
        let mut warned = BTreeSet::new();
        at(&mut state, FIRST + 60);
        assert_eq!(fire_at(&state, FIRST + 60, &mut warned), 1);
        assert!(schedule_intake_once(&state));
        let mission = standing_mission(&state);
        schedule_queued_once(&state).await;
        wait_for_state(&state, &mission, MissionState::Blocked).await;

        // Blocked with its Session live is waiting, not stalled.
        at(&mut state, FIRST + DAY + 60);
        supersede_stalled(&state).await;
        assert_eq!(
            super::super::tests::mission_state(&state, &mission).state,
            MissionState::Blocked
        );

        // The restart: nothing live here and a re-attach pending, which has not failed yet.
        state.sessions.lock().unwrap().remove(&mission);
        state
            .startup_recovery
            .lock()
            .unwrap()
            .insert(mission.clone());
        supersede_stalled(&state).await;
        assert_eq!(
            super::super::tests::mission_state(&state, &mission).state,
            MissionState::Blocked
        );

        state
            .retry_after
            .lock()
            .unwrap()
            .insert(mission.clone(), tokio::time::Instant::now());
        supersede_stalled(&state).await;
        let superseded = super::super::tests::mission_state(&state, &mission);
        assert_eq!(superseded.state, MissionState::Failed);
        assert!(
            superseded.failure.unwrap().contains("no live Session"),
            "the reason names what was found"
        );
        assert!(!state.startup_recovery.lock().unwrap().contains(&mission));
        schedule_intake_once(&state);
        assert_eq!(fire_at(&state, FIRST + DAY + 80, &mut warned), 1);
        stop(&state);
    }
}
