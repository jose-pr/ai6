//! Watches with a stand-in harness: a watch takes a turn per wake, resumes its Session across a
//! restart, starts fresh with its logbook when it cannot, moves to another Station when its own
//! goes dark, stops at its turn quota, and never becomes a Mission.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

use guatiao::Provider;
use guatiao::library::{Object, ProviderError};

use super::tests::test_state;
use super::*;
use crate::directorates::q::terminal::AgentTerminal;
use crate::directorates::registry::cables::{CableAction, CableGrant, CablePolicy, CableScope};
use crate::directorates::registry::quarantine::Quarantine;
use crate::directorates::registry::sessions::{NewSession, SessionResult, SessionUpdate};
use crate::directorates::registry::watches::WatchRecord;
use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart};

/// What the stand-in was asked: `start`, `resume` or `message`, under which Session name, on which
/// provider Session, with what text and, for a start, what context.
#[derive(Debug, Clone)]
struct Call {
    how: &'static str,
    key: String,
    session: String,
    text: String,
    context: Vec<String>,
}

static CALLS: Mutex<Vec<Call>> = Mutex::new(Vec::new());
static NEXT_SESSION: AtomicUsize = AtomicUsize::new(1);

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
/// `MOCK_WATCH_RESUME` makes a resume `refuse` (the handler errs) or `fail` (the resumed
/// Session fails before it answers, as one whose transcript is gone does).
#[derive(Default, Provider)]
#[provider(AgentHandler, id = "mock-watch", name = "Mock Watch", crate = crate::guatiao)]
pub(super) struct MockWatchHandler;

impl AgentHandler for MockWatchHandler {
    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let session = format!(
            "{}-s{}",
            request.mission_id,
            NEXT_SESSION.fetch_add(1, Ordering::AcqRel)
        );
        CALLS.lock().unwrap().push(Call {
            how: "start",
            key: request.mission_id.to_string(),
            session: session.clone(),
            text: request.prompt.clone(),
            context: request.initial_context.clone(),
        });
        Ok(
            WatchSession::answering(request.mission_id.to_string(), session, terminal)
                .into_object(),
        )
    }

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let session = request.session_id.to_string();
        CALLS.lock().unwrap().push(Call {
            how: "resume",
            key: request.mission_id.to_string(),
            session: session.clone(),
            text: request.message.clone().unwrap_or_default(),
            context: request.initial_context.clone(),
        });
        match request
            .environment
            .get("MOCK_WATCH_RESUME")
            .map(String::as_str)
        {
            Some("refuse") => Err(ProviderError::new(
                guatiao::Status::GUATIAO_ERR_INTERNAL,
                "no conversation found",
            )),
            Some("fail") => Ok(WatchSession {
                key: request.mission_id.to_string(),
                session,
                events: Mutex::new(VecDeque::from([event(
                    "failed",
                    Some("no conversation found".to_owned()),
                )])),
                _terminal: terminal,
            }
            .into_object()),
            _ => Ok(
                WatchSession::answering(request.mission_id.to_string(), session, terminal)
                    .into_object(),
            ),
        }
    }
}

struct WatchSession {
    key: String,
    session: String,
    events: Mutex<VecDeque<AgentEvent>>,
    _terminal: Object<dyn AgentTerminal>,
}

impl WatchSession {
    /// A Session answering the message it was started or resumed with.
    fn answering(key: String, session: String, terminal: Object<dyn AgentTerminal>) -> Self {
        Self {
            key,
            events: Mutex::new(VecDeque::from([
                event("text", Some(format!("noted on {session}"))),
                event("completed", None),
            ])),
            session,
            _terminal: terminal,
        }
    }
}

impl AgentSession for WatchSession {
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.session.clone()))
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        CALLS.lock().unwrap().push(Call {
            how: "message",
            key: self.key.clone(),
            session: self.session.clone(),
            text: message.text,
            context: Vec::new(),
        });
        let mut events = self.events.lock().unwrap();
        events.push_back(event("text", Some(format!("noted on {}", self.session))));
        events.push_back(event("completed", None));
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
        Ok(())
    }

    fn close(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

/// The Operation and watch Agent of a test's `tag`.
fn names(tag: &str) -> (String, String) {
    (tag.to_uppercase(), format!("k{tag}"))
}

/// A Station `station` sharing `root`'s Registry and configuration: Operation `TAG` and its
/// watch kept by Agent `kTAG` on the stand-in, with `settings` added to the watch's table and
/// `env` to the Agent's environment. The relay lets 007 and the watch's Agent exchange Cables.
fn station(root: &Path, tag: &str, station: &str, settings: &str, env: &str) -> DaemonState {
    let (operation, agent) = names(tag);
    std::fs::write(
        root.join("watches.toml"),
        format!("[watch.\"operation:{operation}\"]\nagent = \"{agent}\"\nwake_every = \"none\"\n{settings}"),
    )
    .unwrap();
    let checkout = root.join("checkout");
    let mut state = test_state(
        root,
        &format!(
            "\n[operation.{operation}]\npath = {:?}\n",
            checkout.to_string_lossy()
        ),
        &format!(
            "[agent.007]\nhandler = \"mock-agent\"\n[agent.{agent}]\nhandler = \"mock-watch\"\n\
             [agent.{agent}.env]\nMOCK_WATCH = \"1\"\n{env}"
        ),
    );
    state.station_id = station.to_owned();
    state.station = state
        .station
        .clone()
        .with_state_dir(root.join(format!("state-{station}")));
    let grants = ["007", agent.as_str()]
        .into_iter()
        .flat_map(|who| {
            let target =
                CableTarget::try_new(CableScope::Station, Some(station.to_owned())).unwrap();
            [CableAction::Send, CableAction::Receive]
                .map(|action| CableGrant::try_new(who.to_owned(), action, target.clone()).unwrap())
        })
        .collect();
    let (roster, missions, sessions, station_id) = (
        state.roster.clone(),
        state.missions.clone(),
        state.sessions.clone(),
        state.station_id.clone(),
    );
    let (resident_roster, resident_site) = (state.roster.clone(), state.station_id.clone());
    state.cables = Arc::new(
        CableRelay::new(
            state.intake.clone(),
            CablePolicy::try_new(grants, BTreeMap::new()).unwrap(),
            move || cable_endpoints(&roster, &missions, &sessions, &station_id),
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
    format!("watch--operation-{}", names(tag).0)
}

fn record(state: &DaemonState, tag: &str) -> WatchRecord {
    state.intake.watch(&scope(tag)).unwrap().unwrap()
}

/// Turns the watch's logbook records.
fn turns(state: &DaemonState, tag: &str) -> usize {
    state
        .intake
        .watch(&scope(tag))
        .unwrap()
        .map_or(0, |record| {
            record
                .logbook
                .iter()
                .filter(|entry| entry.kind == "turn")
                .count()
        })
}

/// Tends `state`'s watches until `done`, with a wide margin for a loaded machine.
async fn tend_until(state: &DaemonState, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(90);
    while !done() {
        assert!(Instant::now() < deadline, "never happened: {what}");
        watches::tend(state, false, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Tends `state`'s watches for a while, for what must not happen.
async fn tend_for(state: &DaemonState, rounds: usize) {
    for _ in 0..rounds {
        watches::tend(state, false, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn control(state: &DaemonState, tag: &str, note: &str) {
    state
        .intake
        .request_watch_wake(&scope(tag), "person-1", note)
        .unwrap();
}

/// Nothing about the watch is a Mission: no Mission, brief, placement or Session record names it.
fn assert_no_mission(state: &DaemonState, tag: &str) {
    let key = key(tag);
    assert!(state.intake.list_new().unwrap().is_empty(), "no brief");
    assert!(state.intake.leases().unwrap().is_empty(), "no placement");
    assert!(
        state
            .intake
            .sessions()
            .unwrap()
            .iter()
            .all(|session| !session.mission_id.starts_with("watch--")),
        "no Session record of a watch"
    );
    assert!(state.intake.resolve_mission(&key).unwrap().is_none());
    assert!(
        state
            .missions
            .lock()
            .unwrap()
            .list()
            .iter()
            .all(|mission| !mission.mission_id.as_str().starts_with("watch--")),
        "no Mission"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watch_takes_one_turn_per_wake_and_is_no_mission() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w1";
    let state = station(root.path(), tag, "test", "", "");
    watches::tend(&state, false, true).await;
    assert!(state.watches.is_live(&scope(tag)));
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
    let provider = first[0].session.clone();
    assert_eq!(
        record(&state, tag).provider_session_id.as_deref(),
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
        .open_session(
            NewSession::new("run-v1", "mission-v1", "elsewhere")
                .with_operation(operation.clone())
                .with_handle("V-1"),
            Duration::from_secs(600),
        )
        .unwrap();
    state
        .intake
        .update_session(
            "run-v1",
            "elsewhere",
            run.fence,
            SessionUpdate::default().result(SessionResult::new("verifying")),
        )
        .unwrap();
    tend_until(&state, "a turn for verifying", || turns(&state, tag) == 2).await;
    tend_for(&state, 10).await;
    assert_eq!(turns(&state, tag), 2, "an event is told once");

    #[cfg_attr(not(feature = "cables-mcp"), allow(unused_mut))]
    let mut expected = 2;
    #[cfg(feature = "cables-mcp")]
    {
        // A Cable to the watch's Agent, which it takes in its own Session.
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
                    "watch-cable-1".to_owned(),
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
    assert!(
        messages.iter().all(|call| call.session == provider),
        "one provider Session"
    );
    assert!(messages[0].text.contains("\"verifying\"") && messages[0].text.contains("V-1"));
    #[cfg(feature = "cables-mcp")]
    assert!(
        messages[1].text.contains("\"cable\"") && messages[1].text.contains("the build is red")
    );
    let health = messages.last().unwrap();
    assert!(health.text.contains("\"health\"") && health.text.contains("quarantine"));
    assert_no_mission(&state, tag);
    watches::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_station_resumes_the_watch_session() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w2";
    let before = station(root.path(), tag, "test", "", "");
    watches::tend(&before, false, true).await;
    control(&before, tag, "first");
    tend_until(&before, "the first turn", || turns(&before, tag) == 1).await;
    let provider = record(&before, tag).provider_session_id.unwrap();
    drop(before);

    let after = station(root.path(), tag, "test", "", "");
    watches::tend(&after, false, true).await;
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
    assert_eq!(resumed[0].session, provider);
    assert!(
        resumed[0].text.contains("after the restart"),
        "the wake is its message"
    );
    let kept = record(&after, tag);
    assert_eq!(kept.provider_session_id.as_deref(), Some(provider.as_str()));
    assert_eq!(kept.fence, 1, "the same Station renews under its own token");
    assert!(kept.logbook.iter().any(|entry| entry.kind == "resumed"));
    assert_no_mission(&after, tag);
    watches::close_all(&after).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watch_that_cannot_resume_starts_fresh_with_its_logbook() {
    for (tag, mode) in [("w3", "refuse"), ("w4", "fail")] {
        let root = tempfile::tempdir().unwrap();
        let env = format!("MOCK_WATCH_RESUME = \"{mode}\"\n");
        let before = station(root.path(), tag, "test", "", &env);
        watches::tend(&before, false, true).await;
        control(&before, tag, "first");
        tend_until(&before, "the first turn", || turns(&before, tag) == 1).await;
        let lost = record(&before, tag).provider_session_id.unwrap();
        drop(before);

        let after = station(root.path(), tag, "test", "", &env);
        watches::tend(&after, false, true).await;
        control(&after, tag, "after the restart");
        tend_until(&after, "a fresh turn", || turns(&after, tag) == 2).await;
        let all = calls(&key(tag));
        let hows: Vec<&str> = all.iter().map(|call| call.how).collect();
        assert_eq!(hows, ["start", "resume", "start"], "{mode}: {all:?}");
        let fresh = &all[2];
        assert_ne!(fresh.session, lost);
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
        assert_eq!(
            kept.provider_session_id.as_deref(),
            Some(fresh.session.as_str())
        );
        assert!(
            kept.logbook.iter().any(|entry| entry.kind == "fresh"),
            "{mode}: {:?}",
            kept.logbook
        );
        assert_no_mission(&after, tag);
        watches::close_all(&after).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watch_moves_to_another_station_when_its_station_goes_dark() {
    let root = tempfile::tempdir().unwrap();
    let tag = "w5";
    let lease = "lease = 1\n";
    let first = station(root.path(), tag, "station-a", lease, "");
    let second = station(root.path(), tag, "station-b", lease, "");
    watches::tend(&first, false, true).await;
    control(&first, tag, "on a");
    tend_until(&first, "a turn on a", || turns(&first, tag) == 1).await;
    let provider = record(&first, tag).provider_session_id.unwrap();
    watches::tend(&second, false, true).await;
    assert!(!second.watches.is_live(&scope(tag)), "a's lease is live");

    // Station a goes dark: it stops renewing, and b takes the watch once the lease lapses.
    tend_until(&second, "b takes the watch over", || {
        second.watches.is_live(&scope(tag))
    })
    .await;
    let moved = record(&second, tag);
    assert_eq!((moved.station.as_str(), moved.fence), ("station-b", 2));
    assert!(moved.logbook.iter().any(|entry| entry.kind == "station"));
    watches::tend(&second, false, true).await;
    control(&second, tag, "on b");
    tend_until(&second, "a turn on b", || turns(&second, tag) == 2).await;
    let resumed: Vec<Call> = calls(&key(tag))
        .into_iter()
        .filter(|call| call.how == "resume")
        .collect();
    assert_eq!(resumed.len(), 1, "{resumed:?}");
    assert_eq!(
        resumed[0].session, provider,
        "b resumes a's provider Session"
    );

    // a comes back, finds b holds the watch, and lets it go.
    watches::tend(&first, false, true).await;
    assert!(!first.watches.is_live(&scope(tag)));
    assert_eq!(record(&second, tag).station, "station-b");
    assert_no_mission(&second, tag);
    watches::close_all(&second).await;
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
    watches::tend(&state, false, true).await;
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
    watches::close_all(&state).await;
}
