//! Charges with a stand-in harness: a charge takes a turn per wake, resumes its Run across a
//! restart, starts fresh with its logbook when it cannot, moves to another Station when its own
//! goes dark, stops at its turn quota, and never becomes a Mission.

#[cfg(feature = "cables-mcp")]
mod assess;
mod attach;
#[cfg(feature = "cables-mcp")]
mod decides;
#[cfg(feature = "cables-mcp")]
mod keep;
mod sources;
#[cfg(feature = "cables-mcp")]
mod station;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use guatiao::Provider;
use guatiao::library::{Object, ProviderError};

use super::tests::test_state;
use super::*;
use crate::directorates::q::terminal::AgentTerminal;
use crate::directorates::registry::cables::{CableAction, CableGrant, CablePolicy, CableScope};
use crate::directorates::registry::charges::{ChargeRecord, ChargeUpdate};
use crate::directorates::registry::quarantine::Quarantine;
use crate::directorates::registry::runs::{NewRun, RunResult, RunUpdate};
use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart};

/// What the stand-in was asked: `start`, `resume` or `message`, under which Run name, on which
/// Run, with what text and, for a start, what context.
#[derive(Debug, Clone)]
struct Call {
    how: &'static str,
    key: String,
    run: String,
    text: String,
    context: Vec<String>,
}

static CALLS: Mutex<Vec<Call>> = Mutex::new(Vec::new());

/// What a stand-in charge does in a turn, as a model would: it is given the wake's text and the
/// environment naming its AI6 tools, and runs before the turn ends.
type Turn = Arc<dyn Fn(&str, &BTreeMap<String, String>) + Send + Sync>;

/// The turns the stand-in takes, by the charge's Run name.
static TURNS: Mutex<BTreeMap<String, Turn>> = Mutex::new(BTreeMap::new());

fn act(key: &str, text: &str, environment: &BTreeMap<String, String>) {
    let turn = TURNS.lock().unwrap().get(key).cloned();
    if let Some(turn) = turn {
        turn(text, environment);
    }
}

/// The charges, by Run name, whose turns the stand-in ends `failed` while they are listed.
static FAILING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// The charges, by Run name, whose next turn the stand-in ends at a full context window, once.
static WINDOW_ONCE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// The charges whose every turn ends at a full context window while they are listed.
static WINDOW_ALWAYS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// What a turn of the stand-in `key` says before it ends: a line and `completed`, or `failed`.
fn answer(key: &str, run: &str) -> [AgentEvent; 2] {
    let window =
        WINDOW_ONCE.lock().unwrap().remove(key) || WINDOW_ALWAYS.lock().unwrap().contains(key);
    if window {
        let mut full = event(LIMIT_EVENT, Some("prompt is too long".to_owned()));
        full.limit = Some(
            coqui_acp_api::LimitHit::new(0)
                .kind(coqui_acp_api::LimitKind::ContextWindow)
                .continuable(false),
        );
        [full, event("completed", None)]
    } else if FAILING.lock().unwrap().contains(key) {
        [
            event("text", Some(format!("noted on {run}"))),
            event("failed", Some("the model refused".to_owned())),
        ]
    } else {
        [
            event("text", Some(format!("noted on {run}"))),
            event("completed", None),
        ]
    }
}
static NEXT_RUN: AtomicUsize = AtomicUsize::new(1);

fn calls(key: &str) -> Vec<Call> {
    CALLS
        .lock()
        .unwrap()
        .iter()
        .filter(|call| call.key == key)
        .cloned()
        .collect()
}

fn event(kind: &str, text: Option<String>) -> AgentEvent {
    AgentEvent::new(kind, text)
}

/// A harness that answers each message with a line and ends its turn. Its Agent's
/// `MOCK_CHARGE_RESUME` makes a resume `refuse` (the handler errs) or `fail` (the resumed
/// Run fails before it answers, as one whose transcript is gone does).
#[derive(Default, Provider)]
#[provider(AgentHandler, id = "mock-charge", name = "Mock Charge", crate = crate::guatiao)]
pub(super) struct MockChargeHandler;

impl AgentHandler for MockChargeHandler {
    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let run = format!(
            "{}-s{}",
            request.mission_id,
            NEXT_RUN.fetch_add(1, Ordering::AcqRel)
        );
        CALLS.lock().unwrap().push(Call {
            how: "start",
            key: request.mission_id.to_string(),
            run: run.clone(),
            text: request.prompt.clone(),
            context: request.initial_context.clone(),
        });
        act(
            request.mission_id.as_str(),
            &request.prompt,
            &request.environment,
        );
        Ok(ChargeRun::answering(
            request.mission_id.to_string(),
            run,
            request.environment,
            terminal,
        )
        .into_object())
    }

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let run = request.run_id.to_string();
        CALLS.lock().unwrap().push(Call {
            how: "resume",
            key: request.mission_id.to_string(),
            run: run.clone(),
            text: request.message.clone().unwrap_or_default(),
            context: request.initial_context.clone(),
        });
        act(
            request.mission_id.as_str(),
            request.message.as_deref().unwrap_or_default(),
            &request.environment,
        );
        match request
            .environment
            .get("MOCK_CHARGE_RESUME")
            .map(String::as_str)
        {
            Some("refuse") => Err(ProviderError::new(
                guatiao::Status::GUATIAO_ERR_INTERNAL,
                "no conversation found",
            )),
            Some("fail") => Ok(ChargeRun {
                key: request.mission_id.to_string(),
                run,
                events: Mutex::new(VecDeque::from([event(
                    "failed",
                    Some("no conversation found".to_owned()),
                )])),
                environment: request.environment,
                holding: Default::default(),
                _terminal: terminal,
            }
            .into_object()),
            _ => Ok(ChargeRun::answering(
                request.mission_id.to_string(),
                run,
                request.environment,
                terminal,
            )
            .into_object()),
        }
    }
}

/// The stand-in that declares compaction and clearing of a live session, and carries each out as
/// the Agent's `MOCK_CHARGE_COMPACT` and `MOCK_CHARGE_CLEAR` say; [`MockChargeHandler`] declares
/// neither.
#[derive(Default, Provider)]
#[provider(AgentHandler, id = "mock-charge-compact", name = "Mock Charge Compact", crate = crate::guatiao)]
pub(super) struct MockCompactingChargeHandler;

impl AgentHandler for MockCompactingChargeHandler {
    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        MockChargeHandler.start(request, terminal)
    }

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        MockChargeHandler.resume(request, terminal)
    }

    fn session_commands(&self) -> Result<crate::SessionCommands, ProviderError> {
        Ok(crate::SessionCommands {
            compact: true,
            clear: true,
            unsupported_because: None,
        })
    }
}

struct ChargeRun {
    key: String,
    run: String,
    environment: BTreeMap<String, String>,
    events: Mutex<VecDeque<AgentEvent>>,
    /// The message `hold` was taken: the turn goes on until it is interrupted.
    holding: std::sync::atomic::AtomicBool,
    _terminal: Object<dyn AgentTerminal>,
}

impl ChargeRun {
    /// A Run answering the message it was started or resumed with.
    fn answering(
        key: String,
        run: String,
        environment: BTreeMap<String, String>,
        terminal: Object<dyn AgentTerminal>,
    ) -> Self {
        Self {
            environment,
            events: Mutex::new(VecDeque::from(answer(&key, &run))),
            key,
            run,
            holding: Default::default(),
            _terminal: terminal,
        }
    }
}

impl ChargeRun {
    /// A session command, as the Agent's `MOCK_CHARGE_COMPACT` (`ok`, `fail` or `big`: compacts
    /// to a size still over the setting) and `MOCK_CHARGE_CLEAR` (`ok`) say; unset, the
    /// command is unsupported.
    fn command(&self, command: &crate::SessionCommand) -> Result<(), ProviderError> {
        use crate::{CLEARED_EVENT, COMPACT_FAILED_EVENT, COMPACTED_EVENT, SessionChange};
        let (how, setting) = match command {
            crate::SessionCommand::Compact { .. } => ("compact", "MOCK_CHARGE_COMPACT"),
            crate::SessionCommand::Clear => ("clear", "MOCK_CHARGE_CLEAR"),
        };
        let change = |before, after| SessionChange {
            tokens_before: before,
            tokens_after: after,
            ..SessionChange::default()
        };
        let reported = match (how, self.environment.get(setting).map(String::as_str)) {
            ("compact", Some("ok")) => change(Some(1_000), Some(200)).event(COMPACTED_EVENT, None),
            ("compact", Some("big")) => {
                change(Some(2_000), Some(1_500)).event(COMPACTED_EVENT, None)
            }
            ("compact", Some("fail")) => SessionChange::default().event(
                COMPACT_FAILED_EVENT,
                Some("no room to summarise".to_owned()),
            ),
            // Declared, and the turn ends with no boundary.
            ("compact" | "clear", Some("silent")) => event("completed", None),
            // Declared, and nothing ever comes back.
            ("compact", Some("hang")) => {
                CALLS.lock().unwrap().push(Call {
                    how,
                    key: self.key.clone(),
                    run: self.run.clone(),
                    text: String::new(),
                    context: Vec::new(),
                });
                return Ok(());
            }
            ("clear", Some("ok")) => change(None, None).event(CLEARED_EVENT, None),
            _ => {
                return Err(ProviderError::new(
                    guatiao::Status::GUATIAO_ERR_UNSUPPORTED,
                    "the stand-in does not do that",
                ));
            }
        };
        CALLS.lock().unwrap().push(Call {
            how,
            key: self.key.clone(),
            run: self.run.clone(),
            text: String::new(),
            context: Vec::new(),
        });
        // A command that reports nothing is a turn that ends, once.
        let mut events = self.events.lock().unwrap();
        let silent = reported.kind == "completed";
        events.push_back(reported);
        if !silent {
            events.push_back(event("completed", None));
        }
        Ok(())
    }
}

impl AgentSession for ChargeRun {
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.run.clone()))
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        if let Some(command) = crate::SessionCommand::parse(&message.text).unwrap() {
            return self.command(&command);
        }
        CALLS.lock().unwrap().push(Call {
            how: "message",
            key: self.key.clone(),
            run: self.run.clone(),
            text: message.text.clone(),
            context: Vec::new(),
        });
        act(&self.key, &message.text, &self.environment);
        if message.text == "hold" {
            self.holding.store(true, Ordering::Release);
            return Ok(());
        }
        self.events
            .lock()
            .unwrap()
            .extend(answer(&self.key, &self.run));
        Ok(())
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(0).unsigned_abs());
        loop {
            if let Some(event) = self.events.lock().unwrap().pop_front() {
                return Ok(Some(event));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        if self.holding.swap(false, Ordering::AcqRel) {
            self.events
                .lock()
                .unwrap()
                .push_back(event("cancelled", None));
        }
        Ok(())
    }

    fn close(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

/// The Operation and charge Agent of a test's `tag`.
fn names(tag: &str) -> (String, String) {
    (tag.to_uppercase(), format!("k{tag}"))
}

/// A Station `station` sharing `root`'s Registry and configuration: Operation `TAG` and its
/// charge kept by Agent `kTAG` on the stand-in, with `settings` added to the charge's table and
/// `env` to the Agent's environment. The relay lets 007 and the charge's Agent exchange Cables.
fn station(root: &Path, tag: &str, station: &str, settings: &str, env: &str) -> DaemonState {
    station_with(root, tag, station, settings, env, &Extra::default())
}

/// What a test adds to its Station: Agent tables, Operation settings, and the Cable grants of the
/// Operation scope (007 sends to the Operation, its charge's Agent receives).
#[derive(Default)]
struct Extra<'a> {
    /// Settings of the charge's Agent itself.
    charge_agent: &'a str,
    /// The charge Agent's handler; the stand-in that declares no session command by default.
    handler: &'a str,
    agents: &'a str,
    operation: &'a str,
    operation_cables: bool,
}

fn station_with(
    root: &Path,
    tag: &str,
    station: &str,
    settings: &str,
    env: &str,
    extra: &Extra<'_>,
) -> DaemonState {
    let (operation, agent) = names(tag);
    std::fs::write(
        root.join("charges.toml"),
        format!(
            "[charge.\"operation:{operation}\"]\nagent = \"{agent}\"\n{}{settings}",
            if settings.contains("wake_every = ") {
                ""
            } else {
                "wake_every = \"none\"\n"
            }
        ),
    )
    .unwrap();
    let checkout = root.join("checkout");
    let mut state = test_state(
        root,
        &format!(
            "\n[operation.{operation}]\npath = {:?}\nclearance = \"00\"\n{}",
            checkout.to_string_lossy(),
            extra.operation
        ),
        &format!(
            "[agent.007]\nhandler = \"mock-agent\"\n\
             [agent.{agent}]\nhandler = \"{}\"\nclearance = \"00\"\n{}\
             [agent.{agent}.env]\nMOCK_CHARGE = \"1\"\n{env}\n{}",
            if extra.handler.is_empty() {
                "mock-charge"
            } else {
                extra.handler
            },
            extra.charge_agent,
            extra.agents
        ),
    );
    state.station_id = station.to_owned();
    state.station = state
        .station
        .clone()
        .with_state_dir(root.join(format!("state-{station}")));
    let mut grants: Vec<CableGrant> = ["007", agent.as_str()]
        .into_iter()
        .flat_map(|who| {
            let target =
                CableTarget::try_new(CableScope::Station, Some(station.to_owned())).unwrap();
            [CableAction::Send, CableAction::Receive]
                .map(|action| CableGrant::try_new(who.to_owned(), action, target.clone()).unwrap())
        })
        .collect();
    if extra.operation_cables {
        let target = CableTarget::try_new(CableScope::Operation, Some(operation.clone())).unwrap();
        for (who, action) in [
            ("007", CableAction::Send),
            (agent.as_str(), CableAction::Receive),
        ] {
            grants.push(CableGrant::try_new(who.to_owned(), action, target.clone()).unwrap());
        }
    }
    let (roster, missions, runs, station_id) = (
        state.roster.clone(),
        state.missions.clone(),
        state.runs.clone(),
        state.station_id.clone(),
    );
    let (resident_roster, resident_site) = (state.roster.clone(), state.station_id.clone());
    state.cables = Arc::new(
        CableRelay::new(
            state.intake.clone(),
            CablePolicy::try_new(grants, BTreeMap::new()).unwrap(),
            move || cable_endpoints(&roster, &missions, &runs, &station_id),
            BTreeMap::new(),
        )
        .with_residents(move |target| cable_residents(&resident_roster, &resident_site, target))
        .with_cross_operation(CrossOperation::StationGrants)
        .with_durable_audit(|_| Ok(())),
    );
    state
}

fn scope(tag: &str) -> String {
    format!("operation:{}", names(tag).0)
}

fn key(tag: &str) -> String {
    format!("charge--operation-{}", names(tag).0)
}

fn record(state: &DaemonState, tag: &str) -> ChargeRecord {
    state.intake.charge(&scope(tag)).unwrap().unwrap()
}

/// Turns the charge took, by its logbook. A wake whose turn was refused (a loaded machine can
/// refuse one, and the wake is retried) is logged as a `turn` too, and is not counted.
fn turns(state: &DaemonState, tag: &str) -> usize {
    state
        .intake
        .charge(&scope(tag))
        .unwrap()
        .map_or(0, |record| {
            record
                .logbook
                .iter()
                .filter(|entry| entry.kind == "turn" && !entry.text.contains("took no turn"))
                .count()
        })
}

/// Tends `state`'s charges until `done`, with a wide margin for a loaded machine.
async fn tend_until(state: &DaemonState, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(90);
    while !done() {
        assert!(Instant::now() < deadline, "never happened: {what}");
        charges::tend(state, false, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Tends `state`'s charges for a while, for what must not happen.
async fn tend_for(state: &DaemonState, rounds: usize) {
    for _ in 0..rounds {
        charges::tend(state, false, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn control(state: &DaemonState, tag: &str, note: &str) {
    state
        .intake
        .request_charge_wake(&scope(tag), "person-1", note)
        .unwrap();
}

/// Nothing about the charge is a Mission: no Mission, brief, placement or Run record names it.
fn assert_no_mission(state: &DaemonState, tag: &str) {
    let key = key(tag);
    assert!(state.intake.list_new().unwrap().is_empty(), "no brief");
    assert!(state.intake.leases().unwrap().is_empty(), "no placement");
    assert!(
        state
            .intake
            .runs()
            .unwrap()
            .iter()
            .all(|run| !run.mission_id.starts_with("charge--")),
        "no Run record of a charge"
    );
    assert!(state.intake.resolve_mission(&key).unwrap().is_none());
    assert!(
        state
            .missions
            .lock()
            .unwrap()
            .list()
            .iter()
            .all(|mission| !mission.mission_id.as_str().starts_with("charge--")),
        "no Mission"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_takes_one_turn_per_wake_and_is_no_mission() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w1";
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    assert!(state.charges.is_live(&scope(tag)));
    let held = record(&state, tag);
    assert_eq!((held.station.as_str(), held.fence), ("test", 1));
    tend_for(&state, 5).await;
    assert!(calls(&key(tag)).is_empty(), "nothing woke it");

    control(&state, tag, "look at the board");
    tend_until(&state, "a turn for the control", || turns(&state, tag) == 1).await;
    let first = calls(&key(tag));
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].how, "start");
    assert!(first[0].text.contains("\"control\"") && first[0].text.contains("look at the board"));
    assert!(first[0].text.contains("untrusted"), "{}", first[0].text);
    assert!(
        first[0]
            .context
            .iter()
            .any(|line| line.contains("You are not a Mission")),
        "{:?}",
        first[0].context
    );
    let provider = first[0].run.clone();
    assert_eq!(
        record(&state, tag).provider_id.as_deref(),
        Some(provider.as_str())
    );
    assert!(
        record(&state, tag).controls.is_empty(),
        "the control was taken"
    );

    // Another Station's Mission of the Operation enters verifying.
    let (operation, _) = names(tag);
    let run = state
        .intake
        .open_run(
            NewRun::new("run-v1", "mission-v1", "elsewhere")
                .with_operation(operation.clone())
                .with_handle("V-1"),
            Duration::from_secs(600),
        )
        .unwrap();
    state
        .intake
        .update_run(
            "run-v1",
            "elsewhere",
            run.fence,
            RunUpdate::default().result(RunResult::new("verifying")),
        )
        .unwrap();
    tend_until(&state, "a turn for verifying", || turns(&state, tag) == 2).await;
    tend_for(&state, 10).await;
    assert_eq!(turns(&state, tag), 2, "an event is told once");

    #[cfg_attr(not(feature = "cables-mcp"), allow(unused_mut))]
    let mut expected = 2;
    #[cfg(feature = "cables-mcp")]
    {
        // A Cable to the charge's Agent, which it takes in its own Run.
        let from = CablePrincipal::try_new(
            "007".to_owned(),
            operation.clone(),
            None,
            "test".to_owned(),
            None,
            None,
        )
        .unwrap();
        state
            .cables
            .send(
                &from,
                CableSendRequest::new(
                    CableTarget::try_new(CableScope::Agent, Some(names(tag).1)).unwrap(),
                    "the build is red".to_owned(),
                    "charge-cable-1".to_owned(),
                ),
            )
            .unwrap();
        expected += 1;
        tend_until(&state, "a turn for the Cable", || {
            turns(&state, tag) == expected
        })
        .await;
    }

    // A health flag: one of the Operation's Agents is quarantined.
    state
        .intake
        .quarantine_agent(&Quarantine::new(
            "007",
            Some(operation.clone()),
            "spent too much",
            "station:test",
            unix_now(),
        ))
        .unwrap();
    expected += 1;
    tend_until(&state, "a turn for the health flag", || {
        turns(&state, tag) == expected
    })
    .await;

    let all = calls(&key(tag));
    let messages: Vec<&Call> = all.iter().filter(|call| call.how == "message").collect();
    assert_eq!(
        all.iter().filter(|call| call.how != "message").count(),
        1,
        "{all:?}"
    );
    assert!(messages.iter().all(|call| call.run == provider), "one Run");
    assert!(messages[0].text.contains("\"verifying\"") && messages[0].text.contains("V-1"));
    #[cfg(feature = "cables-mcp")]
    assert!(
        messages[1].text.contains("\"cable\"") && messages[1].text.contains("the build is red")
    );
    let health = messages.last().unwrap();
    assert!(health.text.contains("\"health\"") && health.text.contains("quarantine"));
    assert_no_mission(&state, tag);
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_control_wakes_the_charge_once_though_it_is_tended_during_the_turn() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w7";
    TURNS.lock().unwrap().insert(
        key(tag),
        Arc::new(|_: &str, _: &BTreeMap<String, String>| {
            std::thread::sleep(Duration::from_millis(500));
        }),
    );
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    control(&state, tag, "once");
    tend_until(&state, "the turn", || turns(&state, tag) == 1).await;
    tend_for(&state, 25).await;
    let all = calls(&key(tag));
    assert_eq!(all.len(), 1, "one turn for one control: {all:?}");
    assert_eq!(turns(&state, tag), 1);
    TURNS.lock().unwrap().remove(&key(tag));
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wake_now_runs_the_next_turn_and_one_asked_during_it_is_reported_running() {
    use crate::directorates::registry::charges::ChargeWake;

    let root = tempfile::tempdir().unwrap();
    let tag = "w8";
    TURNS.lock().unwrap().insert(
        key(tag),
        Arc::new(|_: &str, _: &BTreeMap<String, String>| {
            std::thread::sleep(Duration::from_millis(600));
        }),
    );
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    let ask = |who: &str| state.intake.wake_charge_now(&scope(tag), who).unwrap();
    assert!(matches!(ask("person-1"), ChargeWake::Requested(_)));

    // The turn is under way once the record says so; a second ask then adds nothing.
    tend_until(&state, "the turn begins", || {
        record(&state, tag).running_since.is_some()
    })
    .await;
    assert!(matches!(ask("person-2"), ChargeWake::Running { .. }));
    assert_eq!(
        record(&state, tag).controls.len(),
        1,
        "only the wake the turn answers waits; it is taken off when the turn ends"
    );

    // It ends, is logged once, and leaves nothing running; the next ask is taken.
    tend_until(&state, "the turn ends", || turns(&state, tag) == 1).await;
    tend_for(&state, 25).await;
    assert_eq!(calls(&key(tag)).len(), 1, "one turn for two asks");
    assert!(record(&state, tag).running_since.is_none());
    assert!(matches!(ask("person-1"), ChargeWake::Requested(_)));
    tend_until(&state, "the second turn", || turns(&state, tag) == 2).await;
    TURNS.lock().unwrap().remove(&key(tag));
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_left_recorded_by_a_station_that_restarted_is_cleared() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w9";
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    let held = record(&state, tag);
    state
        .intake
        .update_charge(
            &scope(tag),
            "test",
            held.fence,
            None,
            ChargeUpdate::default().running(1.0),
        )
        .unwrap();
    charges::tend(&state, false, true).await;
    assert!(
        record(&state, tag).running_since.is_none(),
        "no turn runs here, so none is recorded"
    );
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_station_resumes_the_charge_run() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w2";
    let before = station(root.path(), tag, "test", "", "");
    charges::tend(&before, false, true).await;
    control(&before, tag, "first");
    tend_until(&before, "the first turn", || turns(&before, tag) == 1).await;
    let provider = record(&before, tag).provider_id.unwrap();
    drop(before);

    let after = station(root.path(), tag, "test", "", "");
    charges::tend(&after, false, true).await;
    control(&after, tag, "after the restart");
    tend_until(&after, "a turn after the restart", || {
        turns(&after, tag) == 2
    })
    .await;
    let resumed: Vec<Call> = calls(&key(tag))
        .into_iter()
        .filter(|call| call.how == "resume")
        .collect();
    assert_eq!(resumed.len(), 1, "{resumed:?}");
    assert_eq!(resumed[0].run, provider);
    assert!(
        resumed[0].text.contains("after the restart"),
        "the wake is its message"
    );
    let kept = record(&after, tag);
    assert_eq!(kept.provider_id.as_deref(), Some(provider.as_str()));
    assert_eq!(kept.fence, 1, "the same Station renews under its own token");
    assert!(kept.logbook.iter().any(|entry| entry.kind == "resumed"));
    assert_no_mission(&after, tag);
    charges::close_all(&after).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_that_cannot_resume_starts_fresh_with_its_logbook() {
    for (tag, mode) in [("w3", "refuse"), ("w4", "fail")] {
        let root = tempfile::tempdir().unwrap();
        let env = format!("MOCK_CHARGE_RESUME = \"{mode}\"\n");
        let before = station(root.path(), tag, "test", "", &env);
        charges::tend(&before, false, true).await;
        control(&before, tag, "first");
        tend_until(&before, "the first turn", || turns(&before, tag) == 1).await;
        let lost = record(&before, tag).provider_id.unwrap();
        drop(before);

        let after = station(root.path(), tag, "test", "", &env);
        charges::tend(&after, false, true).await;
        control(&after, tag, "after the restart");
        tend_until(&after, "a fresh turn", || turns(&after, tag) == 2).await;
        let all = calls(&key(tag));
        let hows: Vec<&str> = all.iter().map(|call| call.how).collect();
        assert_eq!(
            hows,
            ["start", "resume", "start"],
            "{mode}: {all:?}\n{:?}",
            record(&after, tag).logbook
        );
        let fresh = &all[2];
        assert_ne!(fresh.run, lost);
        assert!(
            fresh.text.contains("after the restart"),
            "{mode}: the wake is its prompt"
        );
        let logbook = fresh
            .context
            .iter()
            .find(|line| line.contains("Your logbook"))
            .unwrap_or_else(|| panic!("{mode}: no logbook in {:?}", fresh.context));
        assert!(logbook.contains("woke for control"), "{mode}: {logbook}");
        let kept = record(&after, tag);
        assert_eq!(kept.provider_id.as_deref(), Some(fresh.run.as_str()));
        assert!(
            kept.logbook.iter().any(|entry| entry.kind == "fresh"),
            "{mode}: {:?}",
            kept.logbook
        );
        assert_no_mission(&after, tag);
        charges::close_all(&after).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_moves_to_another_station_when_its_station_goes_dark() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w5";
    let lease = "lease = 1\n";
    let first = station(root.path(), tag, "station-a", lease, "");
    let second = station(root.path(), tag, "station-b", lease, "");
    charges::tend(&first, false, true).await;
    control(&first, tag, "on a");
    tend_until(&first, "a turn on a", || turns(&first, tag) == 1).await;
    let provider = record(&first, tag).provider_id.unwrap();
    charges::tend(&second, false, true).await;
    assert!(!second.charges.is_live(&scope(tag)), "a's lease is live");

    // Station a goes dark: it stops renewing, and b takes the charge once the lease lapses.
    tend_until(&second, "b takes the charge over", || {
        second.charges.is_live(&scope(tag))
    })
    .await;
    let moved = record(&second, tag);
    assert_eq!((moved.station.as_str(), moved.fence), ("station-b", 2));
    assert!(moved.logbook.iter().any(|entry| entry.kind == "station"));
    charges::tend(&second, false, true).await;
    control(&second, tag, "on b");
    tend_until(&second, "a turn on b", || turns(&second, tag) == 2).await;
    let resumed: Vec<Call> = calls(&key(tag))
        .into_iter()
        .filter(|call| call.how == "resume")
        .collect();
    assert_eq!(
        resumed.len(),
        1,
        "{resumed:?}\n{:?}",
        record(&second, tag).logbook
    );
    assert_eq!(resumed[0].run, provider, "b resumes a's Run");

    // a comes back, finds b holds the charge, and lets it go.
    charges::tend(&first, false, true).await;
    assert!(!first.charges.is_live(&scope(tag)));
    assert_eq!(record(&second, tag).station, "station-b");
    assert_no_mission(&second, tag);
    charges::close_all(&second).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quotas_stop_a_storm() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w6";
    let state = station(
        root.path(),
        tag,
        "test",
        "max_turns_per_hour = 2\nmax_events_per_turn = 3\nmax_waiting_events = 5\n",
        "",
    );
    charges::tend(&state, false, true).await;
    for at in 0..20 {
        control(&state, tag, &format!("storm {at}"));
    }
    tend_until(&state, "two turns", || turns(&state, tag) == 2).await;
    tend_until(&state, "the quota is noted", || {
        record(&state, tag)
            .logbook
            .iter()
            .any(|entry| entry.kind == "quota" && entry.text.contains("max_turns_per_hour"))
    })
    .await;
    tend_for(&state, 50).await;
    assert_eq!(turns(&state, tag), 2, "the hour's turns are spent");
    let all = calls(&key(tag));
    assert_eq!(all.len(), 2, "{all:?}");
    for call in &all {
        assert_eq!(
            call.text.matches("\"control\"").count(),
            3,
            "three events a turn"
        );
    }
    let kept = record(&state, tag);
    assert!(
        kept.logbook
            .iter()
            .any(|entry| entry.kind == "quota" && entry.text.contains("max_waiting_events")),
        "{:?}",
        kept.logbook
    );
    assert_eq!(
        kept.controls.len(),
        14,
        "what was not told waits on the record"
    );
    assert_no_mission(&state, tag);
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_turn_on_an_idle_charge_is_retried_once_and_an_acknowledgement_ends_the_count() {
    let root = tempfile::tempdir().unwrap();
    let tag = "f1";
    FAILING.lock().unwrap().insert(key(tag));
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    control(&state, tag, "look at the board");
    tend_until(&state, "the failed turn", || turns(&state, tag) == 1).await;
    assert_eq!(record(&state, tag).failed_turns, 1);

    // Nothing else is due, and the charge is idle: the Station wakes it once more by itself.
    tend_until(&state, "the retry", || turns(&state, tag) == 2).await;
    let all = calls(&key(tag));
    assert!(
        all.last().unwrap().text.contains("\"retry\""),
        "{}",
        all.last().unwrap().text
    );
    assert_eq!(record(&state, tag).failed_turns, 2);

    // The retry failed too: it is not tried again until something wakes the charge.
    tend_for(&state, 350).await;
    assert_eq!(turns(&state, tag), 2, "one retry, not a loop");

    let acknowledged = state
        .intake
        .acknowledge_charge(&scope(tag), "person-1")
        .unwrap();
    assert_eq!(acknowledged.failed_turns, 0);
    FAILING.lock().unwrap().remove(&key(tag));
    tend_for(&state, 25).await;
    assert_eq!(
        turns(&state, tag),
        2,
        "an acknowledged failure is not retried"
    );
    control(&state, tag, "again");
    tend_until(&state, "a turn that ends well", || turns(&state, tag) == 3).await;
    assert_eq!(record(&state, tag).failed_turns, 0);
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retry_that_ends_well_clears_the_failure_and_is_not_followed_by_another() {
    let root = tempfile::tempdir().unwrap();
    let tag = "f2";
    FAILING.lock().unwrap().insert(key(tag));
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    control(&state, tag, "look");
    tend_until(&state, "the failed turn", || turns(&state, tag) == 1).await;
    FAILING.lock().unwrap().remove(&key(tag));
    tend_until(&state, "the retry", || turns(&state, tag) == 2).await;
    assert_eq!(record(&state, tag).failed_turns, 0);
    tend_for(&state, 350).await;
    assert_eq!(turns(&state, tag), 2);
    charges::close_all(&state).await;
}

fn charge_degradations(state: &DaemonState) -> Vec<crate::directorates::m::placement::Degradation> {
    health::degradations(state)
        .into_iter()
        .filter(|found| found.kind == "charge")
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turns_that_keep_failing_degrade_the_station_naming_the_charge_until_it_is_acknowledged() {
    let root = tempfile::tempdir().unwrap();
    let tag = "f3";
    FAILING.lock().unwrap().insert(key(tag));
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    control(&state, tag, "one");
    tend_until(&state, "the first failure", || turns(&state, tag) == 1).await;
    tend_until(&state, "the retry", || turns(&state, tag) == 2).await;
    assert!(
        charge_degradations(&state).is_empty(),
        "two failures are under the setting"
    );
    control(&state, tag, "three");
    tend_until(&state, "the third failure", || turns(&state, tag) == 3).await;
    let found = charge_degradations(&state);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].subject, scope(tag));
    assert!(
        found[0].reason.contains("3 turns") && found[0].reason.contains("the model refused"),
        "{}",
        found[0].reason
    );

    state
        .intake
        .acknowledge_charge(&scope(tag), "person-1")
        .unwrap();
    FAILING.lock().unwrap().remove(&key(tag));
    tend_for(&state, 5).await;
    assert!(charge_degradations(&state).is_empty());
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_after_of_none_never_flags_the_charge() {
    let root = tempfile::tempdir().unwrap();
    let tag = "f4";
    FAILING.lock().unwrap().insert(key(tag));
    let state = station(root.path(), tag, "test", "failing_after = \"none\"\n", "");
    charges::tend(&state, false, true).await;
    control(&state, tag, "one");
    tend_until(&state, "a failure", || turns(&state, tag) == 1).await;
    assert_eq!(record(&state, tag).failed_turns, 1);
    assert!(charge_degradations(&state).is_empty());
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_whose_conversation_reached_its_setting_starts_a_fresh_run_with_its_logbook() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w9";
    let mut state = station(root.path(), tag, "test", "", "");
    let mut limits = state.station.limits().clone();
    limits.harness.compact_at_tokens = crate::Limit::at(1_000);
    limits.harness.compact_at_cap_percent = crate::Limit::UNBOUNDED;
    state.station = state.station.clone().with_limits(limits);
    charges::tend(&state, false, true).await;
    control(&state, tag, "first");
    tend_until(&state, "the first turn", || turns(&state, tag) == 1).await;
    let first = record(&state, tag);
    let started = first.provider_id.clone().unwrap();

    // Below the setting the same Run takes the next wake.
    let update = ChargeUpdate {
        context_tokens: Some(999),
        ..Default::default()
    };
    state
        .intake
        .update_charge(&scope(tag), "test", first.fence, None, update)
        .unwrap();
    control(&state, tag, "second");
    tend_until(&state, "the second turn", || turns(&state, tag) == 2).await;
    assert_eq!(
        record(&state, tag).provider_id.as_deref(),
        Some(started.as_str())
    );
    assert!(
        !record(&state, tag)
            .logbook
            .iter()
            .any(|entry| matches!(entry.kind.as_str(), "compacted" | "cleared" | "renewed"))
    );

    // At it, the next wake starts a fresh Run that is given the logbook.
    let update = ChargeUpdate {
        context_tokens: Some(1_000),
        ..Default::default()
    };
    state
        .intake
        .update_charge(&scope(tag), "test", first.fence, None, update)
        .unwrap();
    control(&state, tag, "third");
    tend_until(&state, "the third turn", || turns(&state, tag) == 3).await;
    let all = calls(&key(tag));
    let hows: Vec<&str> = all.iter().map(|call| call.how).collect();
    assert_eq!(hows, ["start", "message", "start"], "{all:?}");
    assert_ne!(all[2].run, started);
    assert!(
        all[2]
            .context
            .iter()
            .any(|line| line.contains("Your logbook")),
        "{:?}",
        all[2].context
    );
    let kept = record(&state, tag);
    let said = kept
        .logbook
        .iter()
        .find(|entry| entry.kind == "renewed")
        .unwrap();
    assert!(said.text.contains("1000 tokens"), "{}", said.text);
    assert_eq!(kept.provider_id.as_deref(), Some(all[2].run.as_str()));
    assert_eq!(kept.context_tokens, Some(0), "the fresh Run starts small");
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_whose_turn_fills_the_context_window_starts_a_fresh_run_and_takes_the_turn_again()
{
    let root = tempfile::tempdir().unwrap();
    let tag = "w10";
    let state = station(root.path(), tag, "test", "", "");
    charges::tend(&state, false, true).await;
    control(&state, tag, "first");
    tend_until(&state, "the first turn", || turns(&state, tag) == 1).await;
    let started = record(&state, tag).provider_id.clone().unwrap();

    WINDOW_ONCE.lock().unwrap().insert(key(tag));
    control(&state, tag, "second");
    tend_until(&state, "the second turn", || turns(&state, tag) == 2).await;
    let all = calls(&key(tag));
    let hows: Vec<&str> = all.iter().map(|call| call.how).collect();
    assert_eq!(hows, ["start", "message", "start"], "{all:?}");
    assert_eq!(all[2].text, all[1].text, "the same wake is taken again");
    assert!(
        all[2]
            .context
            .iter()
            .any(|line| line.contains("Your logbook")),
        "{:?}",
        all[2].context
    );
    let kept = record(&state, tag);
    assert_ne!(kept.provider_id.as_deref(), Some(started.as_str()));
    assert_eq!(kept.failed_turns, 0);
    assert!(
        kept.logbook
            .iter()
            .any(|entry| entry.kind == "renewed" && entry.text.contains("context window")),
        "{:?}",
        kept.logbook
    );
    charges::close_all(&state).await;
}

/// A Station whose charge compacts at 1,000 tokens, with `mode` for `[harness] compaction`.
fn compacting_station(
    root: &Path,
    tag: &str,
    env: &str,
    mode: crate::station::limits::CompactionMode,
) -> DaemonState {
    compacting_station_on(root, tag, "mock-charge-compact", env, mode)
}

/// As [`compacting_station`], with the charge Agent on `handler`.
fn compacting_station_on(
    root: &Path,
    tag: &str,
    handler: &str,
    env: &str,
    mode: crate::station::limits::CompactionMode,
) -> DaemonState {
    let extra = Extra {
        handler,
        ..Extra::default()
    };
    let mut state = station_with(root, tag, "test", "", env, &extra);
    let mut limits = state.station.limits().clone();
    limits.harness.compact_at_tokens = crate::Limit::at(1_000);
    limits.harness.compact_at_cap_percent = crate::Limit::UNBOUNDED;
    limits.harness.compaction = mode;
    state.station = state.station.clone().with_limits(limits);
    state
}

/// Takes a first turn, then sets the charge's conversation to `tokens` and takes a second.
async fn grown_to(state: &DaemonState, tag: &str, tokens: u64) -> ChargeRecord {
    charges::tend(state, false, true).await;
    control(state, tag, "first");
    tend_until(state, "the first turn", || turns(state, tag) == 1).await;
    let first = record(state, tag);
    let update = ChargeUpdate {
        context_tokens: Some(tokens),
        ..Default::default()
    };
    state
        .intake
        .update_charge(&scope(tag), "test", first.fence, None, update)
        .unwrap();
    control(state, tag, "second");
    tend_until(state, "the second turn", || turns(state, tag) == 2).await;
    first
}

fn hows(tag: &str) -> Vec<&'static str> {
    calls(&key(tag)).iter().map(|call| call.how).collect()
}

fn logged<'a>(kept: &'a ChargeRecord, kind: &str) -> &'a str {
    &kept
        .logbook
        .iter()
        .find(|entry| entry.kind == kind)
        .unwrap_or_else(|| panic!("no {kind} entry in {:?}", kept.logbook))
        .text
}

/// The first step: the warm session compacts and takes the wake, with no new Run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_at_its_setting_compacts_in_its_session_and_starts_no_new_run() {
    use crate::station::limits::CompactionMode;
    let root = tempfile::tempdir().unwrap();
    let tag = "c1";
    let state = compacting_station(
        root.path(),
        tag,
        "MOCK_CHARGE_COMPACT = \"ok\"\n",
        CompactionMode::InSession,
    );
    let first = grown_to(&state, tag, 1_000).await;
    assert_eq!(hows(tag), ["start", "compact", "message"]);
    let kept = record(&state, tag);
    assert_eq!(kept.provider_id, first.provider_id, "the same Run");
    let said = logged(&kept, "compacted");
    assert!(said.contains("1000 tokens before, 200 after"), "{said}");
    assert!(!kept.logbook.iter().any(|entry| entry.kind == "renewed"));
    charges::close_all(&state).await;
}

/// The second step, because compaction fails: the session is cleared and the wake carries the
/// logbook.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_whose_compaction_fails_is_cleared_and_given_its_logbook() {
    use crate::station::limits::CompactionMode;
    let root = tempfile::tempdir().unwrap();
    let tag = "c2";
    let state = compacting_station(
        root.path(),
        tag,
        "MOCK_CHARGE_COMPACT = \"fail\"\nMOCK_CHARGE_CLEAR = \"ok\"\n",
        CompactionMode::InSession,
    );
    let first = grown_to(&state, tag, 1_000).await;
    assert_eq!(hows(tag), ["start", "compact", "clear", "message"]);
    let all = calls(&key(tag));
    assert!(all[3].text.contains("Your logbook"), "{}", all[3].text);
    assert!(
        all[3].text.contains("second"),
        "the wake still comes: {}",
        all[3].text
    );
    let kept = record(&state, tag);
    assert_eq!(kept.provider_id, first.provider_id, "the same Run");
    let said = logged(&kept, "cleared");
    assert!(
        said.contains("compaction failed: no room to summarise"),
        "{said}"
    );
    assert!(said.contains("1000 tokens before, 0 after"), "{said}");
    charges::close_all(&state).await;
}

/// The second step, because compaction leaves the conversation over the setting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_whose_compaction_leaves_it_over_its_setting_is_cleared() {
    use crate::station::limits::CompactionMode;
    let root = tempfile::tempdir().unwrap();
    let tag = "c3";
    let state = compacting_station(
        root.path(),
        tag,
        "MOCK_CHARGE_COMPACT = \"big\"\nMOCK_CHARGE_CLEAR = \"ok\"\n",
        CompactionMode::InSession,
    );
    grown_to(&state, tag, 1_000).await;
    assert_eq!(hows(tag), ["start", "compact", "clear", "message"]);
    let said = logged(&record(&state, tag), "cleared").to_owned();
    assert!(said.contains("left 1500 tokens, at or past 1000"), "{said}");
    charges::close_all(&state).await;
}

/// The third step: a handler that declares neither is sent nothing, even though its session would
/// have answered, and gets a fresh Run; the logbook says why.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_whose_handler_declares_neither_is_sent_no_command_and_gets_a_fresh_run() {
    use crate::station::limits::CompactionMode;
    let root = tempfile::tempdir().unwrap();
    let tag = "c4";
    let state = compacting_station_on(
        root.path(),
        tag,
        "mock-charge",
        "MOCK_CHARGE_COMPACT = \"ok\"\nMOCK_CHARGE_CLEAR = \"ok\"\n",
        CompactionMode::InSession,
    );
    let first = grown_to(&state, tag, 1_000).await;
    assert_eq!(hows(tag), ["start", "start"], "no command was sent");
    let kept = record(&state, tag);
    assert_ne!(kept.provider_id, first.provider_id);
    let said = logged(&kept, "renewed");
    assert!(
        said.contains("declares no compaction") && said.contains("declares no clear"),
        "{said}"
    );
    charges::close_all(&state).await;
}

/// A handler that declares compaction and ends the turn with no boundary has failed it: the
/// logbook says so, and the clear step follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declared_compaction_that_ends_with_no_boundary_fails_and_the_session_is_cleared() {
    use crate::station::limits::CompactionMode;
    let root = tempfile::tempdir().unwrap();
    let tag = "c8";
    let state = compacting_station(
        root.path(),
        tag,
        "MOCK_CHARGE_COMPACT = \"silent\"\nMOCK_CHARGE_CLEAR = \"ok\"\n",
        CompactionMode::InSession,
    );
    let first = grown_to(&state, tag, 1_000).await;
    assert_eq!(
        hows(tag),
        ["start", "compact", "clear", "message"],
        "{:?}",
        record(&state, tag).logbook
    );
    let kept = record(&state, tag);
    assert_eq!(kept.provider_id, first.provider_id, "the same Run");
    let said = logged(&kept, "cleared");
    assert!(
        said.contains("compaction failed: the turn ended without a compaction report"),
        "{said}"
    );
    charges::close_all(&state).await;
}

/// One whose report never comes is cut off at `[harness] compact_wait`, and the clear step
/// follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declared_compaction_that_never_reports_fails_at_the_wait_and_the_session_is_cleared() {
    use crate::station::limits::CompactionMode;
    let root = tempfile::tempdir().unwrap();
    let tag = "c9";
    let mut state = compacting_station(
        root.path(),
        tag,
        "MOCK_CHARGE_COMPACT = \"hang\"\nMOCK_CHARGE_CLEAR = \"ok\"\n",
        CompactionMode::InSession,
    );
    let mut limits = state.station.limits().clone();
    limits.harness.compact_wait = crate::TimeLimit::secs(1);
    state.station = state.station.clone().with_limits(limits);
    grown_to(&state, tag, 1_000).await;
    assert_eq!(hows(tag), ["start", "compact", "clear", "message"]);
    let said = logged(&record(&state, tag), "cleared").to_owned();
    assert!(said.contains("no compaction report in 1 s"), "{said}");
    charges::close_all(&state).await;
}

/// `clear` skips the compaction; `fresh` touches nothing in the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_compaction_setting_chooses_where_the_order_begins() {
    use crate::station::limits::CompactionMode;
    let env = "MOCK_CHARGE_COMPACT = \"ok\"\nMOCK_CHARGE_CLEAR = \"ok\"\n";
    let root = tempfile::tempdir().unwrap();
    let state = compacting_station(root.path(), "c5", env, CompactionMode::Clear);
    grown_to(&state, "c5", 1_000).await;
    assert_eq!(hows("c5"), ["start", "clear", "message"]);
    charges::close_all(&state).await;

    let root = tempfile::tempdir().unwrap();
    let state = compacting_station(root.path(), "c6", env, CompactionMode::Fresh);
    let first = grown_to(&state, "c6", 1_000).await;
    assert_eq!(hows("c6"), ["start", "start"]);
    assert_ne!(record(&state, "c6").provider_id, first.provider_id);
    charges::close_all(&state).await;
}

/// A context-window limit: the turn is retried once in the session after it compacts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_whose_turn_fills_the_window_compacts_in_session_and_takes_the_turn_again() {
    use crate::station::limits::CompactionMode;
    let root = tempfile::tempdir().unwrap();
    let tag = "c7";
    let state = compacting_station(
        root.path(),
        tag,
        "MOCK_CHARGE_COMPACT = \"ok\"\n",
        CompactionMode::InSession,
    );
    charges::tend(&state, false, true).await;
    control(&state, tag, "first");
    tend_until(&state, "the first turn", || turns(&state, tag) == 1).await;
    let started = record(&state, tag).provider_id.clone();
    WINDOW_ONCE.lock().unwrap().insert(key(tag));
    control(&state, tag, "second");
    tend_until(&state, "the second turn", || turns(&state, tag) == 2).await;
    assert_eq!(
        hows(tag),
        ["start", "message", "compact", "message"],
        "{:?}",
        record(&state, tag).logbook
    );
    let all = calls(&key(tag));
    assert_eq!(all[3].text, all[1].text, "the same wake is taken again");
    let kept = record(&state, tag);
    assert_eq!(kept.provider_id, started, "no new Run");
    assert_eq!(kept.failed_turns, 0);
    let said = logged(&kept, "compacted");
    assert!(said.contains("context window"), "{said}");
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_run_that_fills_the_window_too_fails_the_turn_once_and_does_not_start_a_third() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w11";
    let state = station(
        root.path(),
        tag,
        "test",
        "failing_after = \"none\"
",
        "",
    );
    charges::tend(&state, false, true).await;
    control(&state, tag, "first");
    tend_until(&state, "the first turn", || turns(&state, tag) == 1).await;

    WINDOW_ALWAYS.lock().unwrap().insert(key(tag));
    control(&state, tag, "second");
    tend_until(&state, "the failed turn", || {
        record(&state, tag).failed_turns == 1
    })
    .await;
    let starts = calls(&key(tag))
        .iter()
        .filter(|call| call.how == "start")
        .count();
    assert_eq!(starts, 2, "one fresh Run, not a third");
    WINDOW_ALWAYS.lock().unwrap().remove(&key(tag));
    charges::close_all(&state).await;
}
