//! Briefs that find no place, harnesses that cannot start, a model sweep beside the check-in
//! beat, and Missions withdrawn, cancelled or retried from outside their Station.

use std::path::Path;

use guatiao::library::{Object, ProviderError};
use guatiao::{Provider, Status};

use super::tests::{MockSession, test_state};
use super::*;
use crate::bounds::{Limit, Seconds, TimeLimit};
use crate::directorates::q::terminal::AgentTerminal;
use crate::directorates::registry::limits::RegistryLimits;
use crate::directorates::registry::store::FileRegistry;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart, ModelListing};

/// What a [`MockProgramHandler`] is built with: an Agent's or route's `handler_settings`.
#[derive(guatiao::ToValue, guatiao::FromValue, guatiao::Schema, Debug, Clone, Default)]
#[map(crate = crate::guatiao)]
pub(super) struct MockProgramConfig {
    #[map(default = String::new())]
    settings_toml: String,
}

/// A handler whose settings name its program (`program`), how long its model listing takes
/// (`models_ms`) and a key its counts are kept under (`key`).
#[derive(Provider)]
#[provider(
    AgentHandler,
    id = "mock-program",
    name = "Mock Program",
    crate = crate::guatiao,
    config = MockProgramConfig
)]
pub(super) struct MockProgramHandler {
    program: Option<String>,
    models_ms: u64,
    key: String,
}

impl TryFrom<MockProgramConfig> for MockProgramHandler {
    type Error = ProviderError;

    fn try_from(config: MockProgramConfig) -> Result<Self, ProviderError> {
        let table: toml::Table = toml::from_str(&config.settings_toml).map_err(|error| {
            ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &error.to_string())
        })?;
        Ok(Self {
            program: table
                .get("program")
                .and_then(toml::Value::as_str)
                .map(str::to_owned),
            models_ms: table
                .get("models_ms")
                .and_then(toml::Value::as_integer)
                .map_or(0, |ms| ms.unsigned_abs()),
            key: table
                .get("key")
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }
}

/// Starts and model listings a [`MockProgramHandler`] saw, by its settings' `key`.
#[derive(Default)]
struct Counts {
    starts: usize,
    listing: usize,
    most_listing: usize,
}

static COUNTS: Mutex<BTreeMap<String, Counts>> = Mutex::new(BTreeMap::new());

fn counts<T>(key: &str, read: impl FnOnce(&mut Counts) -> T) -> T {
    read(COUNTS.lock().unwrap().entry(key.to_owned()).or_default())
}

impl AgentHandler for MockProgramHandler {
    fn start(
        &self,
        request: AgentStart,
        _terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        counts(&self.key, |counts| counts.starts += 1);
        let session = SessionId::try_new(format!("provider-session-{}", request.mission_id))
            .map_err(|error| {
                ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string())
            })?;
        Ok(MockSession::new(session, "blocked").into_object())
    }

    fn resume(
        &self,
        request: AgentResume,
        _terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        Ok(MockSession::new(request.session_id, "completed").into_object())
    }

    fn program(&self) -> Result<Option<String>, ProviderError> {
        Ok(self.program.clone())
    }

    fn models(
        &self,
        _environment: BTreeMap<String, String>,
    ) -> Result<ModelListing, ProviderError> {
        counts(&self.key, |counts| {
            counts.listing += 1;
            counts.most_listing = counts.most_listing.max(counts.listing);
        });
        std::thread::sleep(Duration::from_millis(self.models_ms));
        counts(&self.key, |counts| counts.listing -= 1);
        Ok(ModelListing::of(Vec::new()))
    }
}

/// A brief of Operation G with nothing named but the Stations it is pinned to.
pub(super) fn pinned_brief(id: &str, stations: &[&str]) -> Brief {
    serde_json::from_value(serde_json::json!({
        "title": format!("Pinned work {id}"),
        "id": id,
        "source": "manual",
        "operation": "G",
        "agent": null,
        "role": null,
        "url": null,
        "stations": stations,
    }))
    .unwrap()
}

fn waiting_events(state: &DaemonState) -> Vec<TimelineEvent> {
    state
        .intake
        .timeline_events("G")
        .unwrap()
        .into_iter()
        .filter(|event| event.action == "waiting")
        .collect()
}

fn waits_for(state: &DaemonState, key: &str) -> Vec<String> {
    waiting_events(state)
        .into_iter()
        .filter(|event| event.detail.get("brief").map(String::as_str) == Some(key))
        .filter_map(|event| event.detail.get("reason").cloned())
        .collect()
}

/// A state whose Operation G is limited by `operation_settings`, with Agent 007 on the mock
/// handler, checked in as Station `test`.
fn checked_in(root: &Path, operation_settings: &str) -> DaemonState {
    let state = test_state(
        root,
        operation_settings,
        "[agent.007]\nhandler = \"mock-agent\"\n",
    );
    publish_station_check_in(&state);
    state
}

#[test]
fn a_brief_pinned_to_a_station_its_operation_excludes_says_so_once() {
    let root = tempfile::tempdir().unwrap();
    let state = checked_in(root.path(), "stations = [\"elsewhere\"]\n");
    let brief = pinned_brief("pin-excluded", &["test"]);
    state.intake.submit_brief(&brief).unwrap();

    for _ in 0..3 {
        schedule_intake_once(&state);
    }

    assert_eq!(state.intake.list_new().unwrap().len(), 1, "it stays queued");
    let reasons = waits_for(&state, &brief.key());
    assert_eq!(reasons.len(), 1, "noted once: {reasons:?}");
    assert!(
        reasons[0].contains("Station \"test\"")
            && reasons[0].contains("the Operation allows only Stations [elsewhere]"),
        "{}",
        reasons[0]
    );
}

/// What a pinned brief that never places looks like: an exact pin to a healthy Station places
/// at once, and a pin by another case, by host name, or to a Station without the Operation's
/// checkout is passed over every round. Each was silent; now each waits with its reason.
#[test]
fn a_pin_places_only_on_the_exact_id_of_a_station_that_can_take_it() {
    let root = tempfile::tempdir().unwrap();
    let state = checked_in(root.path(), "");
    // Another Station, alike but without a checkout of G, whose host is `worker`.
    let mut info = state
        .intake
        .stations(Duration::MAX)
        .unwrap()
        .into_iter()
        .find(|record| record.station() == "test")
        .unwrap()
        .info()
        .clone();
    info.insert("operations".to_owned(), serde_json::json!([]));
    info.insert("host".to_owned(), serde_json::json!("worker"));
    state.intake.check_in("worker-1a2b3c4d", info).unwrap();

    let exact = pinned_brief("pin-exact", &["test"]);
    let no_checkout = pinned_brief("pin-no-checkout", &["worker-1a2b3c4d"]);
    let other_case = pinned_brief("pin-case", &["WORKER-1A2B3C4D"]);
    let host_name = pinned_brief("pin-host", &["worker"]);
    for brief in [&exact, &no_checkout, &other_case, &host_name] {
        state.intake.submit_brief(brief).unwrap();
    }
    schedule_intake_once(&state);
    schedule_intake_once(&state);

    assert_eq!(
        state.intake.lease(&exact.key()).unwrap().unwrap().station(),
        "test"
    );
    let queued: Vec<String> = state
        .intake
        .list_new()
        .unwrap()
        .iter()
        .map(Brief::key)
        .collect();
    assert_eq!(queued.len(), 3, "{queued:?}");
    assert!(waits_for(&state, &exact.key()).is_empty());
    for (brief, expected) in [
        (
            &no_checkout,
            "this Station has no checkout of Operation \"G\"",
        ),
        (
            &other_case,
            "Station \"WORKER-1A2B3C4D\" has never checked in",
        ),
        (&host_name, "Station \"worker\" has never checked in"),
    ] {
        let reasons = waits_for(&state, &brief.key());
        assert_eq!(reasons.len(), 1, "{}: {reasons:?}", brief.id);
        assert!(reasons[0].contains(expected), "{}", reasons[0]);
        assert!(
            !reasons[0].contains("narrows placement"),
            "Stations the brief does not name are left out: {}",
            reasons[0]
        );
    }
}

#[test]
fn a_waiting_reason_longer_than_the_timeline_keeps_is_cut_and_still_recorded() {
    let root = tempfile::tempdir().unwrap();
    let state = checked_in(root.path(), "");
    let many: Vec<String> = (0..40).map(|n| format!("absent-station-{n}")).collect();
    let names: Vec<&str> = many.iter().map(String::as_str).collect();
    let brief = pinned_brief("pin-many", &names);
    state.intake.submit_brief(&brief).unwrap();

    schedule_intake_once(&state);

    let reasons = waits_for(&state, &brief.key());
    assert_eq!(reasons.len(), 1);
    let room = state.intake.limits().timeline.text_bytes.or_max();
    assert!(reasons[0].len() <= room, "{}", reasons[0].len());
    assert!(reasons[0].ends_with('…'), "{}", reasons[0]);
    assert!(reasons[0].starts_with("no place can take it: Station \"absent-station-0\""));
}

/// The account path still notes each new reason, its figures included.
#[test]
fn an_account_hold_is_noted_again_when_its_reason_changes() {
    let root = tempfile::tempdir().unwrap();
    let state = checked_in(root.path(), "");
    accounts::note_waiting(&state, "k", "G", Some("frees at 100"));
    accounts::note_waiting(&state, "k", "G", Some("frees at 100"));
    accounts::note_waiting(&state, "k", "G", Some("frees at 200"));
    assert_eq!(waits_for(&state, "k"), ["frees at 100", "frees at 200"]);
    // A placement reason whose figures alone changed is the same reason.
    accounts::note_unplaced(&state, "p", "G", "Station \"a\": silent for 10 seconds");
    accounts::note_unplaced(&state, "p", "G", "Station \"a\": silent for 12 seconds");
    accounts::note_unplaced(&state, "p", "G", "Station \"b\": silent for 12 seconds");
    assert_eq!(waits_for(&state, "p").len(), 2);
}

/// Agent 007 on the mock program handler, with `settings` as its `handler_settings` and
/// nothing else: no loadout, no `env`.
fn program_agent(settings: &str) -> String {
    format!("[agent.007]\nhandler = \"mock-program\"\n[agent.007.handler_settings]\n{settings}\n")
}

/// A Station whose book holds no Mission yet.
pub(super) fn empty_book(root: &Path, agents: &str) -> DaemonState {
    let mut state = test_state(root, "", agents);
    state.missions = Arc::new(Mutex::new(MissionStore::default()));
    publish_station_check_in(&state);
    state
}

fn named_brief(id: &str, agent: &str) -> Brief {
    let mut brief = pinned_brief(id, &[]);
    brief.agent = Some(agent.to_owned());
    brief
}

/// A Station whose Mission book is a file, as a real Station's is, with Agent 007 posted to `G`
/// through `G`'s roster, and the brief `id` placed on it: a Mission queued and not yet started.
fn placed_on_a_durable_book(root: &Path, id: &str) -> (DaemonState, String) {
    let mut state = test_state(
        root,
        "roster = [\"007\"]\n",
        "[agent.007]\nhandler = \"mock-agent\"\n[agent.008]\nhandler = \"mock-agent\"\n",
    );
    state.missions = Arc::new(Mutex::new(
        MissionStore::open(root.join("missions.json")).unwrap(),
    ));
    publish_station_check_in(&state);
    let brief = named_brief(id, "007");
    let key = brief.key();
    state.intake.submit_brief(&brief).unwrap();
    assert!(schedule_intake_once(&state));
    assert_eq!(
        super::tests::mission_state(&state, &key).state,
        MissionState::Queued
    );
    (state, key)
}

/// The roster as it is once Agent 007 is no longer posted to `G`.
fn unpost_the_agent(state: &mut DaemonState, root: &Path) {
    fs::write(
        root.join("operations.toml"),
        format!(
            "[operation.G]\npath = {:?}\nroster = [\"008\"]\n",
            root.join("checkout").to_string_lossy()
        ),
    )
    .unwrap();
    state.roster = Arc::new(Roster::load(root).unwrap());
}

/// The Agent is taken off the Operation, a restart lets the brief's lease lapse, and intake files
/// the brief failed because it names that Agent, with the Mission still queued in the Station's
/// book. Nothing else would ever settle that Mission.
#[tokio::test]
async fn a_settled_brief_stops_retrying_and_its_mission_is_dropped_across_a_restart() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, key) = placed_on_a_durable_book(root.path(), "pinned-settled");
    unpost_the_agent(&mut state, root.path());
    assert!(
        state
            .intake
            .renew(&key, "test", Duration::from_nanos(1))
            .unwrap()
    );
    let lapsed = loop {
        if let Some(lease) = state.intake.expired().unwrap().into_iter().next() {
            break lease;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    state.intake.recover(&lapsed, 5, "test").unwrap().unwrap();
    assert!(schedule_intake_once(&state));
    let filed = state.intake.filed_brief(&key).unwrap().unwrap();
    assert_eq!(filed.state, "failed");
    assert!(
        filed.outcome.unwrap()["reason"]
            .as_str()
            .unwrap()
            .contains("not posted"),
    );

    schedule_queued_once(&state).await;
    assert!(
        !state.retry_after.lock().unwrap().contains_key(&key),
        "a settled brief is not tried again"
    );
    assert!(
        state.missions.lock().unwrap().get(&key).is_none(),
        "its stale Mission is dropped"
    );
    let dropped = state
        .intake
        .timeline_events("G")
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "mission" && event.action == "dropped")
        .count();
    assert_eq!(dropped, 1);

    // After a restart the book has nothing to start.
    let book = MissionStore::open(root.path().join("missions.json")).unwrap();
    assert!(book.get(&key).is_none());
}

#[tokio::test]
async fn an_agent_not_posted_to_the_operation_fails_the_brief_once_naming_it() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, key) = placed_on_a_durable_book(root.path(), "pinned-unposted");
    unpost_the_agent(&mut state, root.path());

    schedule_queued_once(&state).await;
    let mission = super::tests::mission_state(&state, &key);
    assert_eq!(mission.state, MissionState::Failed);
    let reason = mission.failure.unwrap();
    assert!(
        reason.contains("\"007\"") && reason.contains("not posted"),
        "{reason}"
    );
    assert!(state.retry_after.lock().unwrap().is_empty());
    assert!(schedule_intake_once(&state));
    let filed = state.intake.filed_brief(&key).unwrap().unwrap();
    assert_eq!(filed.state, "failed");
    assert_eq!(filed.outcome.unwrap()["reason"], reason.as_str());

    // Nothing is left to try.
    schedule_queued_once(&state).await;
    assert!(state.retry_after.lock().unwrap().is_empty());
    assert_eq!(
        super::tests::mission_state(&state, &key).state,
        MissionState::Failed
    );
}

/// A Mission whose start keeps failing, with its brief still claimed, is ended by a cancel.
#[cfg(feature = "publisher")]
#[tokio::test]
async fn a_cancel_ends_a_mission_looping_in_its_start() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, key) = placed_on_a_durable_book(root.path(), "pinned-looping");
    // A handler is a thing a roster can lose and get back, so this start is tried again.
    fs::write(root.path().join("agents.toml"), "[agent.007]\n").unwrap();
    state.roster = Arc::new(Roster::load(root.path()).unwrap());
    schedule_queued_once(&state).await;
    assert!(state.retry_after.lock().unwrap().contains_key(&key));
    assert_eq!(
        super::tests::mission_state(&state, &key).state,
        MissionState::Queued
    );

    let control = ai6_diptel::MissionControl::new(
        ai6_diptel::ControlId::new("c-1").unwrap(),
        ai6_diptel::OperationId::new("G").unwrap(),
        ai6_diptel::MissionId::new(key.clone()).unwrap(),
        ai6_diptel::ControlAction::Cancel,
    );
    assert_eq!(
        controls::apply(&state, &control).await,
        crate::station::publisher::ControlOutcome::applied("abandoned")
    );
    assert!(schedule_intake_once(&state));
    assert_eq!(
        state.intake.filed_brief(&key).unwrap().unwrap().state,
        "failed"
    );
    state.retry_after.lock().unwrap().clear();
    schedule_queued_once(&state).await;
    assert!(
        state.retry_after.lock().unwrap().is_empty(),
        "nothing is tried again"
    );
}

/// A Mission AI6 failed itself stays failed, though its last Session on record was lost: its
/// reason would fail the next try alike, and each try would queue the one after.
#[tokio::test]
async fn a_mission_ai6_failed_is_not_taken_for_a_lost_attempt_to_try_again() {
    use crate::directorates::registry::sessions::{NewSession, SessionError};

    let root = tempfile::tempdir().unwrap();
    let (state, key) = placed_on_a_durable_book(root.path(), "pinned-lost");
    state
        .intake
        .open_session(
            NewSession::new("lost-run", key.as_str(), "elsewhere").with_operation("G"),
            Duration::from_nanos(1),
        )
        .unwrap();
    loop {
        match state
            .intake
            .lose_session("lost-run", "its Station went dark")
        {
            Ok(_) => break,
            Err(SessionError::HeldBy(_)) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => panic!("{error}"),
        }
    }
    state
        .missions
        .lock()
        .unwrap()
        .fail_for(&key, "it cannot be started")
        .unwrap();
    assert!(schedule_intake_once(&state));
    assert_eq!(
        super::tests::mission_state(&state, &key).state,
        MissionState::Failed
    );
    assert_eq!(
        state.intake.filed_brief(&key).unwrap().unwrap().state,
        "failed"
    );
}

/// A brief filed before an operator asks again stays filed, and the retry is what runs.
#[cfg(feature = "publisher")]
#[tokio::test]
async fn a_retry_an_operator_asked_for_runs_though_its_brief_is_settled() {
    let root = tempfile::tempdir().unwrap();
    let (state, key) = placed_on_a_durable_book(root.path(), "pinned-retried");
    state
        .missions
        .lock()
        .unwrap()
        .fail_for(&key, "it did not work")
        .unwrap();
    assert!(schedule_intake_once(&state));
    assert_eq!(
        state.intake.filed_brief(&key).unwrap().unwrap().state,
        "failed"
    );
    let control = ai6_diptel::MissionControl::new(
        ai6_diptel::ControlId::new("c-1").unwrap(),
        ai6_diptel::OperationId::new("G").unwrap(),
        ai6_diptel::MissionId::new(key.clone()).unwrap(),
        ai6_diptel::ControlAction::Retry,
    );
    assert_eq!(controls::apply(&state, &control).await.outcome, "applied");

    schedule_queued_once(&state).await;
    assert_ne!(
        super::tests::mission_state(&state, &key).state,
        MissionState::Queued,
        "it started"
    );
    state.stopping.store(true, Ordering::Release);
}

#[tokio::test]
async fn an_agent_with_no_path_fails_its_brief_after_one_attempt_and_says_why() {
    let root = tempfile::tempdir().unwrap();
    let key = "no-loadout";
    let state = empty_book(
        root.path(),
        &program_agent(&format!(
            "program = \"ai6-test-harness-nowhere\"\nkey = \"{key}\""
        )),
    );
    let brief = named_brief("harness-missing", "007");
    state.intake.submit_brief(&brief).unwrap();

    schedule_intake_once(&state);
    schedule_queued_once(&state).await;
    let mission = super::tests::mission_state(&state, &brief.key());
    assert_eq!(mission.state, MissionState::Failed);
    let reason = mission.failure.clone().unwrap();
    assert_eq!(
        reason,
        "Agent \"007\" has no PATH in its environment (no `[loadout.007] env`), so its harness \
         \"ai6-test-harness-nowhere\" cannot be found"
    );
    assert!(
        !state.retry_after.lock().unwrap().contains_key(&brief.key()),
        "it is not tried again"
    );
    schedule_queued_once(&state).await;
    assert_eq!(
        counts(key, |counts| counts.starts),
        0,
        "no harness was started"
    );

    schedule_intake_once(&state);
    let filed = state.intake.filed_brief(&brief.key()).unwrap().unwrap();
    assert_eq!(filed.state, "failed");
    assert_eq!(filed.attempts, 1);
    let outcome = filed.outcome.unwrap();
    assert_eq!(outcome["state"], "failed");
    assert_eq!(outcome["reason"], reason.as_str());
}

#[tokio::test]
async fn a_harness_on_the_agents_own_path_starts() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let file = if cfg!(windows) {
        "ai6-test-harness.exe"
    } else {
        "ai6-test-harness"
    };
    fs::write(bin.join(file), "").unwrap();
    let key = "on-path";
    let agents = format!(
        "[agent.007]\nhandler = \"mock-program\"\nenv = {{ PATH = {:?} }}\n\
         [agent.007.handler_settings]\nprogram = \"ai6-test-harness\"\nkey = \"{key}\"\n",
        bin.to_string_lossy()
    );
    let state = empty_book(root.path(), &agents);
    let brief = named_brief("harness-found", "007");
    state.intake.submit_brief(&brief).unwrap();
    schedule_intake_once(&state);
    schedule_queued_once(&state).await;
    super::tests::wait_for_state(&state, &brief.key(), MissionState::Blocked).await;
    assert_eq!(counts(key, |counts| counts.starts), 1);
    state.stopping.store(true, Ordering::Release);
}

/// A Station whose check-in comes every 100 ms and is dark after a second of silence, with a
/// route whose handler takes `models_ms` to list its models.
fn slow_listing_state(root: &Path, key: &str, models_ms: u64) -> DaemonState {
    slow_listing_state_dark_after(root, key, models_ms, Duration::from_secs(1))
}

/// [`slow_listing_state`] with the Station dark after `dark` of silence.
fn slow_listing_state_dark_after(
    root: &Path,
    key: &str,
    models_ms: u64,
    dark: Duration,
) -> DaemonState {
    let agents = format!(
        "[agent.009]\n[[agent.009.route]]\nhandler = \"mock-program\"\nmodel = \"slow\"\n\
         handler_settings = {{ models_ms = {models_ms}, key = \"{key}\" }}\n"
    );
    let mut state = test_state(root, "", &agents);
    state.missions = Arc::new(Mutex::new(MissionStore::default()));
    let mut limits = StationLimits::default();
    limits.scheduler.check_in = Seconds(Duration::from_millis(100));
    limits.scheduler.interval = Seconds(Duration::from_millis(20));
    limits.accounts.model_probe_timeout = TimeLimit::UNBOUNDED;
    state.station = state.station.clone().with_limits(limits);
    let mut registry = RegistryLimits::default();
    registry.stations.gone_dark_after = Seconds(dark);
    state.intake = Arc::new(FileRegistry::open(root.join("registry")).with_limits(registry));
    state
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_listing_slower_than_going_dark_leaves_the_check_ins_on_time() {
    let root = tempfile::tempdir().unwrap();
    // The listing outlasts going dark by far, so a beat it blocked goes about 7 s stale, while
    // a loaded machine delays an unblocked 100 ms beat by about 2 s at most.
    let state =
        slow_listing_state_dark_after(root.path(), "slow-beat", 7_000, Duration::from_secs(3));
    let running = tokio::spawn(scheduler_loop(state.clone()));
    let started = Instant::now();
    let mut stalest = 0.0_f64;
    let mut found = false;
    while started.elapsed() < Duration::from_secs(20) && !found {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(record) = state
            .intake
            .stations(Duration::MAX)
            .unwrap()
            .into_iter()
            .find(|record| record.station() == "test")
        {
            stalest = stalest.max(unix_now() - record.seen());
        }
        found = state
            .models
            .found()
            .iter()
            .any(|found| found.agent.as_deref() == Some("009"));
    }
    state.stopping.store(true, Ordering::Release);
    let _ = running.await;
    assert!(found, "the look finished");
    assert!(
        started.elapsed() >= Duration::from_secs(7),
        "the listing took its seven seconds"
    );
    let dark = state
        .intake
        .limits()
        .stations
        .gone_dark_after
        .0
        .as_secs_f64();
    assert!(
        stalest < dark,
        "the check-in went {stalest:.2} s stale while the models were listed"
    );
}

#[test]
fn a_listing_that_does_not_answer_in_time_keeps_what_was_found_before() {
    let root = tempfile::tempdir().unwrap();
    let mut state = slow_listing_state(root.path(), "slow-kept", 400);
    models::look(&state);
    let before = state.models.found();
    assert_eq!(before.len(), 1);
    assert!(before[0].listing.known);

    let mut limits = state.station.limits().clone();
    limits.accounts.model_probe_timeout = TimeLimit::at(Duration::from_millis(50));
    state.station = state.station.clone().with_limits(limits);
    let asked = std::time::Instant::now();
    models::look(&state);
    assert!(
        asked.elapsed() < Duration::from_millis(400),
        "it did not wait"
    );
    assert_eq!(state.models.found(), before, "the list it had is kept");

    // Nothing found before, and nothing in time: unknown, saying so.
    let other = tempfile::tempdir().unwrap();
    let mut fresh = slow_listing_state(other.path(), "slow-unknown", 400);
    let mut limits = fresh.station.limits().clone();
    limits.accounts.model_probe_timeout = TimeLimit::at(Duration::from_millis(50));
    fresh.station = fresh.station.clone().with_limits(limits);
    models::look(&fresh);
    let found = fresh.models.found();
    assert!(!found[0].listing.known);
    assert!(
        found[0]
            .listing
            .unknown_because
            .as_deref()
            .is_some_and(|why| why.contains("did not answer within")),
        "{found:?}"
    );
}

#[test]
fn a_look_asks_no_more_handlers_at_once_than_model_probes() {
    let root = tempfile::tempdir().unwrap();
    let key = "probes-at-once";
    let routes: String = (0..5)
        .map(|n| {
            format!(
                "[[agent.009.route]]\nhandler = \"mock-program\"\nmodel = \"m{n}\"\n\
                 handler_settings = {{ models_ms = 200, key = \"{key}\" }}\n"
            )
        })
        .collect();
    let mut state = test_state(root.path(), "", &format!("[agent.009]\n{routes}"));
    let mut limits = StationLimits::default();
    limits.accounts.model_probes = Limit::at(2);
    state.station = state.station.clone().with_limits(limits);
    models::look(&state);
    assert_eq!(state.models.found().len(), 5);
    assert_eq!(counts(key, |counts| counts.most_listing), 2);
}

/// A Station whose Headquarters settles an unconfirmed cancel after `settle_after`.
#[cfg(feature = "publisher")]
fn settling_state(root: &Path, settle_after: Duration) -> DaemonState {
    let mut state = empty_book(root, "[agent.007]\nhandler = \"mock-agent\"\n");
    let mut registry = RegistryLimits::default();
    registry.briefs.cancel_settle_after = Seconds(settle_after);
    state.intake = Arc::new(FileRegistry::open(root.join("registry")).with_limits(registry));
    state
}

/// What a wedged Mission's brief looks like: its Station renews the lease every pass for as long
/// as its Mission has not ended, and nothing else ends it. A cancel the Station never confirms is
/// settled by Headquarters once `cancel_settle_after` has passed, and the Station, finding the
/// brief settled, stops the Mission.
#[cfg(feature = "publisher")]
#[tokio::test]
async fn a_cancel_its_station_never_confirms_settles_the_brief_after_the_setting() {
    let root = tempfile::tempdir().unwrap();
    let state = settling_state(root.path(), Duration::from_secs(300));
    let brief = named_brief("wedged", "007");
    let key = brief.key();
    state.intake.submit_brief(&brief).unwrap();
    schedule_intake_once(&state);
    state
        .missions
        .lock()
        .unwrap()
        .transition(&key, MissionState::Working)
        .unwrap();
    let first = state.intake.lease(&key).unwrap().unwrap().expires();
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(20));
        schedule_intake_once(&state);
    }
    assert!(
        state.intake.lease(&key).unwrap().unwrap().expires() >= first,
        "its Station keeps the lease"
    );
    assert_eq!(
        state.intake.filed_brief(&key).unwrap().unwrap().state,
        "claimed"
    );

    // Asked a moment ago: not yet.
    assert!(state.intake.request_cancel(&key, unix_now()).unwrap());
    schedule_intake_once(&state);
    assert_eq!(
        state.intake.filed_brief(&key).unwrap().unwrap().state,
        "claimed"
    );

    // The first request is the one that counts; one that old is settled.
    let other = tempfile::tempdir().unwrap();
    let state = settling_state(other.path(), Duration::from_secs(300));
    state.intake.submit_brief(&brief).unwrap();
    schedule_intake_once(&state);
    state
        .missions
        .lock()
        .unwrap()
        .transition(&key, MissionState::Working)
        .unwrap();
    assert!(
        state
            .intake
            .request_cancel(&key, unix_now() - 301.0)
            .unwrap()
    );
    assert!(state.intake.request_cancel(&key, unix_now()).unwrap());
    schedule_intake_once(&state);
    let filed = state.intake.filed_brief(&key).unwrap().unwrap();
    assert_eq!(filed.state, "failed");
    let outcome = filed.outcome.unwrap();
    assert_eq!(outcome["reason"], controls::CANCEL_UNCONFIRMED);
    assert_eq!(outcome["station"], "test");
    assert!(
        state.intake.lease(&key).unwrap().is_none(),
        "the lease is released"
    );
    assert!(state.intake.cancel_requests().unwrap().is_empty());

    controls::stop_settled(&state).await;
    assert_eq!(
        super::tests::mission_state(&state, &key).state,
        MissionState::Abandoned
    );
}

#[cfg(feature = "publisher")]
#[tokio::test]
async fn a_mission_retried_after_its_settlement_runs_on() {
    let root = tempfile::tempdir().unwrap();
    let state = settling_state(root.path(), Duration::ZERO);
    let brief = named_brief("retried", "007");
    let key = brief.key();
    state.intake.submit_brief(&brief).unwrap();
    schedule_intake_once(&state);
    assert!(state.intake.request_cancel(&key, unix_now()).unwrap());
    schedule_intake_once(&state);
    assert_eq!(
        state.intake.filed_brief(&key).unwrap().unwrap().state,
        "failed"
    );
    // The Mission had a Session the settlement did not name: a later attempt.
    {
        let mut missions = state.missions.lock().unwrap();
        missions.transition(&key, MissionState::Working).unwrap();
        missions
            .attach_session(&key, SessionId::try_new("provider-session-later").unwrap())
            .unwrap();
    }
    controls::stop_settled(&state).await;
    assert_eq!(
        super::tests::mission_state(&state, &key).state,
        MissionState::Working
    );
}

#[cfg(feature = "publisher")]
const ROLE_AGENTS: &str = "[agent.007]\nhandler = \"mock-agent\"\npriority = 5\n\
                           roles = [\"case-officer\"]\n\
                           [agent.008]\nhandler = \"mock-agent\"\nroles = [\"case-officer\"]\n";

/// A failed Mission of Agent 007 in role `case-officer`.
#[cfg(feature = "publisher")]
fn failed_case(state: &DaemonState) {
    let mut missions = state.missions.lock().unwrap();
    missions
        .create(
            MissionId::try_new("mission-case").unwrap(),
            Some("G".to_owned()),
            "work the case".to_owned(),
            Some("007".to_owned()),
            Some("case-officer".to_owned()),
        )
        .unwrap();
    missions
        .transition("mission-case", MissionState::Failed)
        .unwrap();
}

#[cfg(feature = "publisher")]
fn retry_case() -> ai6_diptel::MissionControl {
    ai6_diptel::MissionControl::new(
        ai6_diptel::ControlId::new("control-retry").unwrap(),
        ai6_diptel::OperationId::new("G").unwrap(),
        ai6_diptel::MissionId::new("mission-case").unwrap(),
        ai6_diptel::ControlAction::Retry,
    )
}

#[cfg(feature = "publisher")]
#[tokio::test]
async fn a_retry_goes_to_the_agent_the_roster_gives_now() {
    let root = tempfile::tempdir().unwrap();
    let mut state = test_state(root.path(), "", ROLE_AGENTS);
    failed_case(&state);
    // 007 is stripped of the role and drops to priority 1.
    fs::write(
        root.path().join("agents.toml"),
        ROLE_AGENTS.replace(
            "priority = 5\nroles = [\"case-officer\"]",
            "priority = 1\nroles = []",
        ),
    )
    .unwrap();
    state.roster = Arc::new(Roster::load(root.path()).unwrap());

    let outcome = controls::apply(&state, &retry_case()).await;
    assert_eq!(outcome.outcome, "applied", "{:?}", outcome.reason);
    let retried = super::tests::mission_state(&state, "mission-case");
    assert_eq!(retried.state, MissionState::Queued);
    assert_eq!(retried.agent_id.as_deref(), Some("008"));
}

#[cfg(feature = "publisher")]
#[tokio::test]
async fn a_retry_keeps_its_agent_while_it_ranks_first_and_names_one_when_asked() {
    let root = tempfile::tempdir().unwrap();
    let state = test_state(root.path(), "", ROLE_AGENTS);
    failed_case(&state);
    assert_eq!(
        controls::apply(&state, &retry_case()).await.outcome,
        "applied"
    );
    assert_eq!(
        super::tests::mission_state(&state, "mission-case")
            .agent_id
            .as_deref(),
        Some("007")
    );

    state
        .missions
        .lock()
        .unwrap()
        .transition("mission-case", MissionState::Failed)
        .unwrap();
    let named = retry_case().with_agent("008").unwrap();
    assert_eq!(controls::apply(&state, &named).await.outcome, "applied");
    assert_eq!(
        super::tests::mission_state(&state, "mission-case")
            .agent_id
            .as_deref(),
        Some("008"),
        "a named Agent is taken as named"
    );
}

#[cfg(feature = "publisher")]
#[tokio::test]
async fn a_retry_no_agent_can_take_is_refused_with_the_reason() {
    let root = tempfile::tempdir().unwrap();
    let mut state = test_state(root.path(), "", ROLE_AGENTS);
    failed_case(&state);
    fs::write(
        root.path().join("agents.toml"),
        "[agent.007]\nhandler = \"mock-agent\"\n",
    )
    .unwrap();
    state.roster = Arc::new(Roster::load(root.path()).unwrap());
    let outcome = controls::apply(&state, &retry_case()).await;
    assert_eq!(outcome.outcome, "rejected");
    assert!(
        outcome
            .reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("no Agent can take the retry")),
        "{:?}",
        outcome.reason
    );
}
