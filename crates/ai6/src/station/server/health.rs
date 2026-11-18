//! What a Station says of its own liveness at check-in: whether its Runs make progress, when
//! each Source's poll last finished, whether each tracker still takes writes, and the briefs it
//! gave back for making no progress. A fault in any of them makes the Station `degraded`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use super::{DaemonState, recovery, unix_now};
use crate::directorates::m::mission::MissionState;
use crate::directorates::m::placement::Degradation;
use crate::station::issues::TrackerBuilder;
use crate::station::sources::{PollHealth, SourceSetup, build_tracker, source_setups};

/// The brief keys this Station gave back for making no progress, by Station, with when the
/// Station may take them again.
static DECLINED: Mutex<BTreeMap<(String, String), f64>> = Mutex::new(BTreeMap::new());

/// What the last write check of one tracker instance found.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Canary {
    /// When it ran, as Unix seconds.
    pub(super) at: f64,
    /// Why the instance does not take writes; `None` when it does.
    pub(super) failed: Option<String>,
}

/// A brief whose claim the Registry refused with an error, and when it may be tried again.
#[derive(Debug, Clone, PartialEq)]
struct ClaimFailure {
    /// Failures in a row.
    failures: u32,
    /// Unix seconds before which the claim is not tried again.
    retry_at: f64,
    /// What the Registry said.
    reason: String,
}

/// The claims that failed on a Registry error, by Station and brief key.
static CLAIM_FAILURES: Mutex<BTreeMap<(String, String), ClaimFailure>> =
    Mutex::new(BTreeMap::new());

/// Whether the claim of brief `key` is still backing off from a Registry error.
pub(super) fn claim_backed_off(state: &DaemonState, key: &str) -> bool {
    lock(&CLAIM_FAILURES)
        .get(&(state.station_id.clone(), key.to_owned()))
        .is_some_and(|failure| failure.retry_at > unix_now())
}

/// Journals a claim that failed on a Registry error and holds it off for `[scheduler]
/// claim_backoff`, doubled for each failure in a row up to `retry_backoff_max`.
pub(super) fn claim_failed(state: &DaemonState, key: &str, error: &std::io::Error) {
    let scheduler = &state.station.limits().scheduler;
    let mut failures = lock(&CLAIM_FAILURES);
    let entry = failures
        .entry((state.station_id.clone(), key.to_owned()))
        .or_insert(ClaimFailure {
            failures: 0,
            retry_at: 0.0,
            reason: String::new(),
        });
    entry.failures = entry.failures.saturating_add(1);
    let wait = scheduler
        .claim_backoff
        .0
        .saturating_mul(2u32.saturating_pow(entry.failures - 1))
        .min(scheduler.retry_backoff_max.0.max(scheduler.claim_backoff.0));
    entry.retry_at = unix_now() + wait.as_secs_f64();
    entry.reason = error.to_string();
    eprintln!(
        "ai6: claiming brief {key} failed {} time(s) in a row, trying again in {:.0} s: {error}",
        entry.failures,
        wait.as_secs_f64()
    );
}

/// Forgets the failures of the claim of brief `key`: it was claimed.
pub(super) fn claim_succeeded(state: &DaemonState, key: &str) {
    lock(&CLAIM_FAILURES).remove(&(state.station_id.clone(), key.to_owned()));
}

/// What a Station's control loop has done, so a loop that stopped taking controls shows.
#[derive(Debug, Clone, Default, PartialEq)]
struct ControlLoop {
    /// Controls the loop finished, however they ended.
    processed: u64,
    /// Controls it gave up on and rejected for taking too long.
    failed_back: u64,
    /// When the loop last came round, as Unix seconds.
    beat: f64,
    /// The Mission of the control being applied and when it began.
    active: Option<(String, f64)>,
    /// Why the last control it gave up on did not finish; cleared by the next that does.
    stalled: Option<String>,
}

/// The control loops by Station.
static CONTROL_LOOPS: Mutex<BTreeMap<String, ControlLoop>> = Mutex::new(BTreeMap::new());

fn with_loop<R>(state: &DaemonState, change: impl FnOnce(&mut ControlLoop) -> R) -> R {
    change(
        lock(&CONTROL_LOOPS)
            .entry(state.station_id.clone())
            .or_default(),
    )
}

/// Records that the control loop came round.
pub(super) fn control_beat(state: &DaemonState) {
    with_loop(state, |control| control.beat = unix_now());
}

/// Records that the loop began applying a control to `mission`.
pub(super) fn control_started(state: &DaemonState, mission: &str) {
    with_loop(state, |control| {
        control.active = Some((mission.to_owned(), unix_now()));
    });
}

/// Records that the loop finished the control it was applying.
pub(super) fn control_finished(state: &DaemonState) {
    with_loop(state, |control| {
        control.active = None;
        control.processed += 1;
        control.stalled = None;
        control.beat = unix_now();
    });
}

/// Journals a control the loop gave up on after `[scheduler] control_timeout`, and keeps the
/// Station degraded until a later control finishes.
pub(super) fn control_failed_back(state: &DaemonState, mission: &str, waited: f64) {
    let reason =
        format!("a control for Mission {mission} did not finish within {waited:.0} seconds");
    eprintln!("ai6: {reason}; it was rejected back to the operator");
    with_loop(state, |control| {
        control.active = None;
        control.processed += 1;
        control.failed_back += 1;
        control.stalled = Some(reason);
        control.beat = unix_now();
    });
}

/// Whether the control loop is stuck: a control in hand past `[scheduler] control_timeout`, or no
/// pass of the loop for that long. `None` while the loop has not started or no timeout is set.
fn control_wedge(state: &DaemonState, now: f64) -> Option<String> {
    let limit = state.station.limits().scheduler.control_timeout;
    let held = lock(&CONTROL_LOOPS).get(&state.station_id).cloned()?;
    if let Some((mission, since)) = &held.active
        && !limit.allows_secs(now - since)
    {
        return Some(format!(
            "a control for Mission {mission} has been applied for {:.0} seconds",
            now - since
        ));
    }
    (held.active.is_none() && !limit.allows_secs(now - held.beat)).then(|| {
        format!(
            "the control loop has not run for {:.0} seconds",
            now - held.beat
        )
    })
}

/// The write checks by Station and `operation/instance`.
static CANARIES: Mutex<BTreeMap<(String, String), Canary>> = Mutex::new(BTreeMap::new());

/// When this Station last looked into running its write checks, by Station.
static ASKED: Mutex<BTreeMap<String, f64>> = Mutex::new(BTreeMap::new());

/// Whether `[sources] canary_every` has passed since this Station last looked into its write
/// checks; answering yes starts the next period.
pub(super) fn canary_due(state: &DaemonState, now: f64) -> bool {
    let Some(every) = state.station.limits().sources.canary_every.get() else {
        return false;
    };
    let mut asked = lock(&ASKED);
    let due = asked
        .get(&state.station_id)
        .is_none_or(|last| *last + every.as_secs_f64() <= now);
    if due {
        asked.insert(state.station_id.clone(), now);
    }
    due
}

fn lock<T>(held: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    held.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The briefs `station` may not take yet, from this process's own record for this Station.
pub(super) fn declined_here(state: &DaemonState, now: f64) -> BTreeMap<String, f64> {
    lock(&DECLINED)
        .iter()
        .filter(|((station, _), until)| *station == state.station_id && **until > now)
        .map(|((_, key), until)| (key.clone(), *until))
        .collect()
}

/// How many Runs this Station holds, and how many of them made progress within
/// `[scheduler] claim_progress`.
pub(super) fn sessions(state: &DaemonState) -> (usize, usize) {
    let limit = state.station.limits().scheduler.claim_progress;
    let runs = lock_runs(state);
    let progressing = runs
        .values()
        .filter(|run| {
            run.watchdog
                .quiet_for()
                .is_none_or(|quiet| limit.allows(quiet))
        })
        .count();
    (runs.len(), progressing)
}

fn lock_runs(
    state: &DaemonState,
) -> std::sync::MutexGuard<'_, BTreeMap<String, super::AgentOperation>> {
    state
        .runs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Gives back each brief whose Run sent nothing within `[scheduler] start_deadline` of starting
/// (a failed start, tried again at once), or made no progress for `[scheduler] claim_progress`
/// (this Station is left out of its placement for `[scheduler] claim_exclusion`).
pub(super) fn surrender_stuck(state: &DaemonState) -> Vec<String> {
    let scheduler = &state.station.limits().scheduler;
    let stuck: Vec<_> = lock_runs(state)
        .iter()
        .filter_map(|(mission, run)| {
            let silent = run.watchdog.silent_since_start();
            if let (Some(silent), Some(deadline)) = (silent, scheduler.start_deadline.get())
                && silent >= deadline
            {
                return Some((
                    mission.clone(),
                    run.run.clone(),
                    Surrender::NeverStarted(deadline),
                ));
            }
            let (quiet, after) = (run.watchdog.quiet_for()?, scheduler.claim_progress.get()?);
            (quiet >= after).then(|| {
                (
                    mission.clone(),
                    run.run.clone(),
                    Surrender::NoProgress(after),
                )
            })
        })
        .collect();
    let mut given_back = Vec::new();
    for (mission_id, run, why) in stuck {
        let working = state
            .missions
            .lock()
            .ok()
            .and_then(|missions| missions.get(&mission_id).map(|mission| mission.state))
            == Some(MissionState::Working);
        if !working {
            continue;
        }
        if let Surrender::NoProgress(_) = why {
            let Some(key) = state.intake.leases().ok().and_then(|leases| {
                leases.into_iter().find_map(|lease| {
                    (lease.station() == state.station_id
                        && lease.meta().get("mission_id").and_then(|id| id.as_str())
                            == Some(mission_id.as_str()))
                    .then(|| lease.key().to_owned())
                })
            }) else {
                continue;
            };
            lock(&DECLINED).insert(
                (state.station_id.clone(), key),
                unix_now() + scheduler.claim_exclusion.0.as_secs_f64(),
            );
        }
        recovery::give_up(state, &mission_id, Some(&run), &why.reason());
        given_back.push(mission_id);
    }
    given_back
}

/// Why a Run's brief is given back.
enum Surrender {
    /// The Run sent no output and no event with content within the start deadline.
    NeverStarted(std::time::Duration),
    /// The Run made no progress for the claim progress limit.
    NoProgress(std::time::Duration),
}

impl Surrender {
    fn reason(&self) -> String {
        match self {
            Self::NeverStarted(after) => format!(
                "its Run sent nothing within {:.0} seconds of starting, so this Station gave the brief back to start again",
                after.as_secs_f64()
            ),
            Self::NoProgress(after) => format!(
                "its Run made no progress for {:.0} seconds, so this Station gave the brief back",
                after.as_secs_f64()
            ),
        }
    }
}

/// The orphans already journaled, by Station, so each is named once.
static ORPHANS: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());

/// The Missions this Station holds that should have a harness running but have none: working or
/// blocked, with no live Run, not starting, and not waiting to be re-attached. Their claims are
/// not renewed, and the check-in names them so an operator can settle them.
pub(super) fn orphaned(state: &DaemonState) -> BTreeSet<String> {
    let live: BTreeSet<String> = lock_runs(state).keys().cloned().collect();
    let mut waiting: BTreeSet<String> = state
        .starting
        .lock()
        .map(|starting| starting.clone())
        .unwrap_or_default();
    waiting.extend(
        state
            .startup_recovery
            .lock()
            .map(|pending| pending.clone())
            .unwrap_or_default(),
    );
    let found: BTreeSet<String> = state
        .missions
        .lock()
        .map(|missions| {
            missions
                .list()
                .into_iter()
                .filter(|mission| {
                    matches!(mission.state, MissionState::Working | MissionState::Blocked)
                })
                .map(|mission| mission.mission_id.to_string())
                .filter(|id| !live.contains(id) && !waiting.contains(id))
                .collect()
        })
        .unwrap_or_default();
    let mut journaled = lock(&ORPHANS);
    journaled.retain(|(station, id)| *station != state.station_id || found.contains(id));
    for id in &found {
        if journaled.insert((state.station_id.clone(), id.clone())) {
            eprintln!(
                "ai6: Mission {id:?} is held here with no live Run; its claim is no longer renewed"
            );
        }
    }
    found
}

/// Each Source's last finished poll, for the check-in.
pub(super) fn source_polls(state: &DaemonState) -> Vec<serde_json::Value> {
    state
        .charges
        .polls
        .snapshot()
        .into_iter()
        .map(|(source, health)| {
            serde_json::json!({
                "source": source,
                "every": health.every,
                "last_completed": health.completed,
            })
        })
        .collect()
}

/// The Sources that went `n` intervals without a finished poll, with how long it has been.
pub(super) fn missed_polls(polls: &[(String, PollHealth)], now: f64, n: u64) -> Vec<(String, f64)> {
    polls
        .iter()
        .filter_map(|(source, health)| {
            let last = health.completed.unwrap_or(health.seen);
            let silent = now - last;
            (silent >= health.every * n as f64).then(|| (source.clone(), silent))
        })
        .collect()
}

/// Everything that keeps this Station from being whole.
pub(super) fn degradations(state: &DaemonState) -> Vec<Degradation> {
    let now = unix_now();
    let limits = state.station.limits();
    let agents: Vec<_> = state
        .roster
        .agent_ids()
        .filter_map(|id| state.roster.agent(None, id).ok())
        .collect();
    let mut found = crate::station::degraded::refused_libraries(
        &state.plugins.scan_summary().found,
        &agents,
        &super::available_handlers(state),
    );
    if let Some(n) = limits.sources.missed_polls.get() {
        for (source, silent) in missed_polls(&state.charges.polls.snapshot(), now, n) {
            found.push(Degradation::new(
                "source-poll",
                source,
                format!("no poll has finished for {silent:.0} seconds ({n} intervals)"),
                vec!["new work from this Source".to_owned()],
            ));
        }
    }
    for ((station, source), canary) in lock(&CANARIES).iter() {
        if *station == state.station_id
            && let Some(why) = &canary.failed
        {
            found.push(Degradation::new(
                "source-write",
                source.clone(),
                why.clone(),
                vec!["reports and comments to this tracker".to_owned()],
            ));
        }
    }
    found.extend(state.station.bootstrap_refused.iter().cloned());
    found.extend(super::reload::record_degradations(state));
    for ((station, key), failure) in lock(&CLAIM_FAILURES).iter() {
        if *station == state.station_id {
            found.push(Degradation::new(
                "claim",
                key.clone(),
                format!(
                    "the Registry failed its claim {} time(s) in a row: {}",
                    failure.failures, failure.reason
                ),
                vec!["this brief being placed".to_owned()],
            ));
        }
    }
    for (scope, failed, ended) in state.charges.failing() {
        found.push(Degradation::new(
            "charge",
            scope,
            format!("its last {failed} turns failed in a row: {ended}"),
            vec!["its area being kept by its charge".to_owned()],
        ));
    }
    for id in orphaned(state) {
        found.push(Degradation::new(
            "orphaned-claim",
            id,
            "the claim is held with no live Run behind it".to_owned(),
            vec!["this Mission making progress".to_owned()],
        ));
    }
    let stalled = lock(&CONTROL_LOOPS)
        .get(&state.station_id)
        .and_then(|control| control.stalled.clone());
    if let Some(why) = control_wedge(state, now).or(stalled) {
        found.push(Degradation::new(
            "controls",
            "control loop",
            why,
            vec!["operator controls (cancel, retry, resume, interrupt)".to_owned()],
        ));
    }
    let (running, progressing) = sessions(state);
    if running > 0 && progressing == 0 {
        found.push(Degradation::new(
            "sessions",
            "runs",
            format!("none of its {running} Runs has made progress recently"),
            Vec::new(),
        ));
    }
    found
}

/// Checks that `setup`'s tracker still takes writes: its provider builds, answers what it can do,
/// and says it can comment.
fn check_write(build: &TrackerBuilder<'_>, setup: &SourceSetup) -> Option<String> {
    let tracker = match build(&setup.provider, &setup.settings) {
        Ok(tracker) => tracker,
        Err(error) => return Some(format!("its provider could not be built: {error}")),
    };
    match tracker.capabilities(None) {
        Ok(capabilities) if capabilities.comment => None,
        Ok(_) => Some("its credential cannot post comments".to_owned()),
        Err(error) => Some(format!("it did not answer what it can do: {error}")),
    }
}

/// Runs the write check of every tracker instance not checked within `[sources] canary_every`.
/// Only the Headquarters Station holds tracker tokens; any other leaves it alone.
pub(super) fn run_canaries(state: &DaemonState, now: f64) {
    let plugins = state.plugins.clone();
    run_canaries_with(state, now, &|provider: &str, settings: &toml::Table| {
        build_tracker(&plugins, provider, settings)
    });
}

/// [`run_canaries`] with the trackers built by `build`.
fn run_canaries_with(state: &DaemonState, now: f64, build: &TrackerBuilder<'_>) {
    let limits = state.station.limits();
    let Some(every) = limits.sources.canary_every.get() else {
        return;
    };
    for operation in state.roster.operations() {
        for setup in source_setups(operation, &limits.sources)
            .into_iter()
            .flatten()
        {
            let key = (
                state.station_id.clone(),
                format!("{}/{}", operation.id, setup.name),
            );
            let due = lock(&CANARIES)
                .get(&key)
                .is_none_or(|last| last.at + every.as_secs_f64() <= now);
            if !due {
                continue;
            }
            let failed = check_write(build, &setup);
            lock(&CANARIES).insert(key, Canary { at: now, failed });
        }
    }
}

/// The facts of this Station's liveness the check-in carries.
pub(super) fn check_in_facts(state: &DaemonState, info: &mut BTreeMap<String, serde_json::Value>) {
    let (running, progressing) = sessions(state);
    info.insert(
        "sessions".to_owned(),
        serde_json::json!({"running": running, "progressing": progressing}),
    );
    info.insert(
        "source_polls".to_owned(),
        serde_json::json!(source_polls(state)),
    );
    if let Some(control) = lock(&CONTROL_LOOPS).get(&state.station_id) {
        info.insert(
            "controls".to_owned(),
            serde_json::json!({
                "processed": control.processed,
                "failed_back": control.failed_back,
                "heartbeat": control.beat,
            }),
        );
    }
    let orphans = orphaned(state);
    if !orphans.is_empty() {
        info.insert("orphaned".to_owned(), serde_json::json!(orphans));
    }
    let declined = declined_here(state, unix_now());
    if !declined.is_empty() {
        info.insert("declined".to_owned(), serde_json::json!(declined));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use guatiao::Status;
    use guatiao::library::ProviderError;

    use super::*;
    use crate::directorates::registry::tracker::{
        IssueSpec, PostedComment, ReplyBatch, ReplyPoll, Tracker, TrackerCapabilities,
        TrackerIssue, TrackerLink, TrackerPost, TrackerUpdate,
    };
    use crate::station::server::tests::test_state;

    /// A tracker whose credential reads and cannot comment.
    struct ReadOnly;

    impl Tracker for ReadOnly {
        fn capabilities(&self, _: Option<String>) -> Result<TrackerCapabilities, ProviderError> {
            Ok(TrackerCapabilities::default())
        }
        fn issue(&self, _: String) -> Result<TrackerIssue, ProviderError> {
            Err(ProviderError::new(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "read only",
            ))
        }
        fn replies(&self, _: ReplyPoll) -> Result<ReplyBatch, ProviderError> {
            Ok(ReplyBatch::default())
        }
        fn post(&self, _: TrackerPost) -> Result<PostedComment, ProviderError> {
            Err(ProviderError::new(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "read only",
            ))
        }
        fn update(&self, _: TrackerUpdate) -> Result<(), ProviderError> {
            Err(ProviderError::new(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "read only",
            ))
        }
        fn create(&self, _: IssueSpec) -> Result<TrackerIssue, ProviderError> {
            Err(ProviderError::new(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "read only",
            ))
        }
        fn link(&self, _: TrackerLink) -> Result<(), ProviderError> {
            Err(ProviderError::new(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "read only",
            ))
        }
    }

    const SOURCES: &str = "sources = { gh = { provider = \"github\", repo = \"o/r\", token_env = \"AI6_TEST_NONE\" } }\n";

    fn station(root: &std::path::Path) -> DaemonState {
        test_state(
            root,
            &format!("{SOURCES}[operation.G.issues]\n"),
            "[agent.007]\nhandler = \"mock-recorder\"\n",
        )
    }

    #[test]
    fn a_read_only_credential_fails_the_write_check_and_degrades_the_station_naming_the_source() {
        let root = tempfile::tempdir().unwrap();
        let state = station(root.path());
        assert!(
            degradations(&state)
                .iter()
                .all(|found| found.kind != "source-write")
        );
        run_canaries_with(&state, 1_000.0, &|_: &str, _: &toml::Table| {
            Ok(Box::new(ReadOnly) as Box<dyn Tracker>)
        });
        let degraded = degradations(&state);
        let write = degraded
            .iter()
            .find(|found| found.kind == "source-write")
            .unwrap_or_else(|| panic!("{degraded:?}"));
        assert_eq!(write.subject, "G/gh");
        assert!(write.reason.contains("cannot post comments"), "{write:?}");
    }

    #[test]
    fn a_source_whose_polls_stopped_for_missed_polls_intervals_degrades_the_station() {
        let root = tempfile::tempdir().unwrap();
        let state = station(root.path());
        let long_ago = unix_now() - 1_000.0;
        state
            .charges
            .polls
            .begin("G/gh", Duration::from_secs(60), long_ago);
        state.charges.polls.end("G/gh", long_ago + 1.0, None);
        let degraded = degradations(&state);
        let poll = degraded
            .iter()
            .find(|found| found.kind == "source-poll")
            .unwrap_or_else(|| panic!("{degraded:?}"));
        assert_eq!(poll.subject, "G/gh");
        let facts = source_polls(&state);
        assert_eq!(facts[0]["source"], "G/gh");
        assert!(facts[0]["last_completed"].as_f64().is_some());

        state.charges.polls.end("G/gh", unix_now(), None);
        assert!(
            degradations(&state)
                .iter()
                .all(|found| found.kind != "source-poll"),
            "a poll that finished clears it"
        );
    }

    #[test]
    fn a_control_loop_that_stopped_coming_round_degrades_the_station_until_it_does() {
        let root = tempfile::tempdir().unwrap();
        let mut state = station(root.path());
        state.station_id = "loop-heartbeat".to_owned();
        let found = |state: &DaemonState| {
            degradations(state)
                .into_iter()
                .find(|found| found.kind == "controls")
        };
        assert!(found(&state).is_none(), "no loop, nothing to report");

        control_beat(&state);
        assert!(found(&state).is_none());
        with_loop(&state, |control| control.beat -= 600.0);
        let stuck = found(&state).expect("a loop silent past the timeout is degraded");
        assert!(stuck.reason.contains("has not run"), "{}", stuck.reason);

        std::sync::Arc::make_mut(&mut state.station.limits)
            .scheduler
            .control_timeout = crate::bounds::TimeLimit::UNBOUNDED;
        assert!(found(&state).is_none(), "`none` never degrades");
        std::sync::Arc::make_mut(&mut state.station.limits)
            .scheduler
            .control_timeout = crate::bounds::TimeLimit::secs(120);
        control_beat(&state);
        assert!(found(&state).is_none());
    }
}
