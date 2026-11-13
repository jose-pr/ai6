//! The Station's charge and M's charge with a stand-in harness: an Operation's 00 asks a Station
//! for a Theater and its charge answers, a stalled Run's claim is surrendered, a frozen Source poll
//! is flagged on M's board, and a restarted Station resumes both charges. The stand-ins call the
//! real tools over the real endpoints, as a model would.

use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use super::decides::{Tools, events_of};
use super::*;
use crate::directorates::m::charges::ChargeScope;
use crate::directorates::m::mission::MissionState;
use crate::directorates::registry::reports::{ReportFilter, ReportScope};
use crate::directorates::registry::sources::SourceBatch;
use crate::station::server::head_of_station::tests::{lease_mission, start};
use crate::station::server::resources;
use crate::station::sources::SourceDesk;
use crate::{Source, SourcePoll};

/// Only one test at a time keeps M's charge: its Run name is the same in every Station.
static M_CHARGE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Every event a stand-in was woken for, with the charge's Run name, and what each tool answered.
static HEARD: Mutex<Vec<(String, Value)>> = Mutex::new(Vec::new());
static ANSWERED: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());
static TOOL_NAMES: Mutex<BTreeMap<String, Vec<String>>> = Mutex::new(BTreeMap::new());

/// Makes the charge `key` act on each event it is woken for: `handle` is given its tools and the
/// event.
fn acting(key: String, handle: impl Fn(&Tools, &Value) + Send + Sync + 'static) {
    let handle = Arc::new(handle);
    let name = key.clone();
    TURNS.lock().unwrap().insert(
        key,
        Arc::new(move |text: &str, environment: &BTreeMap<String, String>| {
            let events = events_of(text);
            if events.is_empty() {
                return;
            }
            let (key, environment, handle) = (name.clone(), environment.clone(), handle.clone());
            std::thread::spawn(move || {
                let tools = Tools::connect(&key, &environment);
                TOOL_NAMES
                    .lock()
                    .unwrap()
                    .insert(key.clone(), tools.names());
                for event in &events {
                    HEARD.lock().unwrap().push((key.clone(), event.clone()));
                    handle(&tools, event);
                }
                let _ = tools.runtime.block_on(tools.client.cancel());
            })
            .join()
            .unwrap();
        }),
    );
}

/// Calls `tool` as the charge `key` and keeps what it answered.
fn call(tools: &Tools, key: &str, tool: &str, arguments: Value) -> String {
    let answer = tools.call(tool, arguments);
    ANSWERED
        .lock()
        .unwrap()
        .push((key.to_owned(), tool.to_owned(), answer.clone()));
    answer
}

fn heard(key: &str, kind: &str) -> Vec<Value> {
    HEARD
        .lock()
        .unwrap()
        .iter()
        .filter(|(who, event)| who == key && event["kind"] == kind)
        .map(|(_, event)| event["detail"].clone())
        .collect()
}

fn answered(key: &str, tool: &str) -> Vec<String> {
    ANSWERED
        .lock()
        .unwrap()
        .iter()
        .filter(|(who, called, _)| who == key && called == tool)
        .map(|(_, _, answer)| answer.clone())
        .collect()
}

fn audits(state: &DaemonState, action: &str) -> usize {
    state
        .intake
        .audit_records()
        .unwrap()
        .iter()
        .filter(|record| record.action == action)
        .count()
}

/// A Station `station` whose Operation `TAG` has a charge, whose own charge is kept by Agent
/// `sTAG` and, with `with_m`, whose M's charge by `mTAG`. Agent `M` runs Missions on the
/// recorder; `operation` adds to the Operation's table or follows it with another.
fn kit(
    root: &Path,
    tag: &str,
    station: &str,
    (station_settings, with_m): (&str, bool),
    operation: &str,
) -> DaemonState {
    let charge_agent = |id: String| {
        format!(
            "[agent.{id}]\nhandler = \"mock-charge\"\nclearance = \"00\"\n\
             [agent.{id}.env]\nMOCK_CHARGE = \"1\"\n"
        )
    };
    let agents = format!(
        "{}{}[agent.M]\nhandler = \"mock-recorder\"\nclearance = \"field\"\n",
        charge_agent(format!("s{tag}")),
        charge_agent(format!("m{tag}")),
    );
    let mut settings = format!(
        "[charge.\"station:{station}\"]\nagent = \"s{tag}\"\nwake_every = \"none\"\n\
         {station_settings}\n"
    );
    if with_m {
        settings.push_str(&format!(
            "[charge.m]\nagent = \"m{tag}\"\nwake_every = \"none\"\npoll_frozen_after = 1\n"
        ));
    }
    station_with(
        root,
        tag,
        station,
        &settings,
        "",
        &Extra {
            agents: &agents,
            operation,
            ..Extra::default()
        },
    )
}

fn station_key(station: &str) -> String {
    charges::run_key(&ChargeScope::Station(station.to_owned()))
}

fn wake(state: &DaemonState, scope: &str, note: &str) {
    state
        .intake
        .request_charge_wake(scope, "person-1", note)
        .unwrap();
}

fn turns_of(state: &DaemonState, scope: &str) -> usize {
    state.intake.charge(scope).unwrap().map_or(0, |record| {
        record
            .logbook
            .iter()
            .filter(|entry| entry.kind == "turn" && !entry.text.contains("took no turn"))
            .count()
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_00_asks_the_station_for_a_theater_and_its_charge_answers_with_the_theater_or_the_reason()
{
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("r1", "stn-r1");
    let operation = "[operation.OUT]\nstations = [\"elsewhere\"]\n";
    let state = kit(root.path(), tag, station, ("", false), operation);
    crate::station::server::publish_station_check_in(&state);
    let (op_key, st_key) = (key(tag), station_key(station));

    // The 00 asks when a person's word says what to ask; the Station's charge provides what the
    // Station's own check says it can, and gives the check's reason for what it cannot.
    let asker = op_key.clone();
    acting(op_key.clone(), move |tools, event| {
        if event["kind"] != "control" {
            return;
        }
        let note = event["detail"]["note"].as_str().unwrap_or_default();
        if let Some(spec) = note.strip_prefix("ask ") {
            call(
                tools,
                &asker,
                "request_resources",
                serde_json::from_str(spec).unwrap(),
            );
        }
    });
    let answerer = st_key.clone();
    acting(st_key.clone(), move |tools, event| {
        if event["kind"] != "request" {
            return;
        }
        let (request, check) = (
            &event["detail"]["request"],
            &event["detail"]["station_check"],
        );
        let provide = check.get("can_provide").is_some();
        call(
            tools,
            &answerer,
            "answer_request",
            json!({
                "request": request["id"], "operation": request["operation"],
                "kind": request["kind"], "name": request["name"],
                "decision": if provide { "provide" } else { "refuse" },
                "reason": check["cannot_provide"],
            }),
        );
    });

    charges::tend(&state, false, true).await;
    assert!(
        state.charges.is_live(&scope(tag)) && state.charges.is_live(&format!("station:{station}"))
    );
    wake(
        &state,
        &scope(tag),
        r#"ask {"kind":"theater","name":"local","detail":"to build","idempotency_key":"ask-1"}"#,
    );
    tend_until(&state, "the 00 is told the Theater", || {
        !heard(&op_key, "resources").is_empty()
    })
    .await;
    let told = &heard(&op_key, "resources")[0]["answer"];
    assert_eq!(told["answer"], "provided", "{told}");
    assert_eq!(told["provided"]["theater"], "local", "{told}");
    assert_eq!(told["request"], "ask-1");
    assert_eq!(told["station"], station);

    // A Theater the Station does not reach is refused, with why.
    wake(
        &state,
        &scope(tag),
        r#"ask {"kind":"theater","name":"gpu-box","detail":"to train","idempotency_key":"ask-2"}"#,
    );
    tend_until(&state, "the 00 is told why not", || {
        heard(&op_key, "resources").len() == 2
    })
    .await;
    let told = &heard(&op_key, "resources")[1]["answer"];
    assert_eq!(told["answer"], "refused", "{told}");
    assert!(
        told["reason"].as_str().unwrap().contains("not fitted"),
        "{told}"
    );
    assert_eq!(told["request"], "ask-2");

    // Both are audited, and each tool exists only for its own charge.
    assert_eq!(audits(&state, "resources.requested"), 2);
    assert_eq!(audits(&state, "resources.answered"), 2);
    let names = |key: &str| TOOL_NAMES.lock().unwrap()[key].clone();
    assert!(names(&op_key).contains(&"request_resources".to_owned()));
    assert!(!names(&op_key).contains(&"answer_request".to_owned()));
    for tool in ["answer_request", "surrender_claim", "station_report"] {
        assert!(names(&st_key).contains(&tool.to_owned()), "{tool}");
    }
    assert!(!names(&st_key).contains(&"request_resources".to_owned()));
    assert_no_mission(&state, tag);

    // What the Station cannot give, it says so before it promises: outside the Operation's
    // allowance, an account no pool serves, and no place at all.
    let outside = resources::check(&state, "OUT", "theater", None).unwrap_err();
    assert!(
        outside.contains("outside the Operation's allowance"),
        "{outside}"
    );
    let agent = format!("k{tag}");
    let account = resources::check(&state, &names_of(tag), "account", Some(&agent)).unwrap();
    assert_eq!(account["pools"], json!([]), "{account}");
    let unknown = resources::check(&state, "NOPE", "capacity", None).unwrap_err();
    assert!(unknown.contains("not on the roster"), "{unknown}");
    charges::close_all(&state).await;
}

/// [`tend_until`] on the Headquarters Station, which keeps M's charge.
async fn headquarters_until(state: &DaemonState, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(90);
    while !done() {
        assert!(Instant::now() < deadline, "never happened: {what}");
        charges::tend(state, true, true).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn names_of(tag: &str) -> String {
    names(tag).0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_station_with_no_room_says_busy_and_a_charge_that_promises_more_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("r5", "stn-r5");
    let mut state = kit(root.path(), tag, station, ("", false), "");
    state.max_concurrent_missions = 0;
    crate::station::server::publish_station_check_in(&state);
    let busy = resources::check(&state, &names_of(tag), "capacity", None).unwrap_err();
    assert!(!busy.is_empty());

    // The charge says "provide"; the Station checks and answers a refusal with its own reason.
    charges::tend(&state, false, true).await;
    let desk = crate::station::server::head_of_station::Desk::new(state.clone(), format!("s{tag}"));
    let answered = tokio::task::spawn_blocking({
        let operation = names_of(tag);
        move || {
            desk.answer(resources::Answering {
                request: "r".to_owned(),
                operation,
                kind: "capacity".to_owned(),
                name: None,
                decision: "provide".to_owned(),
                reason: Some("sure".to_owned()),
            })
        }
    })
    .await
    .unwrap();
    // The Operation's charge has run, so the answer is left for it.
    let answered = answered.unwrap();
    assert_eq!(answered["answer"], "refused", "{answered}");
    assert_eq!(answered["reason"], json!(busy));
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_planted_stall_is_surrendered_by_the_station_charge() {
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("r2", "stn-r2");
    let state = kit(root.path(), tag, station, ("", false), "");
    let st_key = station_key(station);
    let acting_key = st_key.clone();
    acting(st_key.clone(), move |tools, event| {
        if event["kind"] == "health" && event["detail"]["flag"] == "run-stalled" {
            call(
                tools,
                &acting_key,
                "surrender_claim",
                json!({ "mission": event["detail"]["mission"], "reason": "no output for an hour" }),
            );
        }
    });
    let operation = names_of(tag);
    let (stuck, working) = ("stall-stuck-r2", "stall-working-r2");
    for id in [stuck, working] {
        lease_mission(&state, &operation, id);
        start(&state, id).await;
        state
            .missions
            .lock()
            .unwrap()
            .transition(id, MissionState::Working)
            .unwrap();
    }
    // One Run has made no progress for an hour; the other has just spoken.
    state.runs.lock().unwrap()[stuck]
        .watchdog
        .backdate(Duration::from_secs(3_600));

    charges::tend(&state, false, true).await;
    tend_until(&state, "the stall is surrendered", || {
        !answered(&st_key, "surrender_claim").is_empty()
    })
    .await;
    let said = answered(&st_key, "surrender_claim").remove(0);
    assert!(said.contains(stuck) && !said.contains("refused"), "{said}");
    assert!(
        !state.runs.lock().unwrap().contains_key(stuck),
        "its Run stopped"
    );
    let after = state.missions.lock().unwrap().get(stuck).unwrap().state;
    assert_eq!(
        after,
        MissionState::Abandoned,
        "its brief goes back to the queue"
    );
    assert!(
        state
            .intake
            .list_new()
            .unwrap()
            .iter()
            .any(|brief| brief.id == stuck),
        "queued again for placement"
    );
    assert_eq!(audits(&state, "head_of_station.surrendered"), 1);
    // The Run that made progress was never told of, and the Station refuses to end it.
    assert!(
        heard(&st_key, "health")
            .iter()
            .all(|event| event["mission"] != working)
    );
    let desk = crate::station::server::head_of_station::Desk::new(state.clone(), format!("s{tag}"));
    let refused = tokio::task::spawn_blocking(move || desk.surrender(working, None))
        .await
        .unwrap()
        .unwrap_err();
    assert!(refused.contains("made progress"), "{refused}");
    assert!(state.runs.lock().unwrap().contains_key(working));
    state.stopping.store(true, Ordering::Release);
    charges::close_all(&state).await;
}

/// A Source that does not answer until it is let go.
struct Frozen(Arc<AtomicBool>);

impl Source for Frozen {
    fn poll(&self, _request: SourcePoll) -> Result<SourceBatch, ProviderError> {
        while !self.0.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(SourceBatch {
            items: Vec::new(),
            cursor: None,
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frozen_source_poll_is_flagged_on_ms_board_by_its_charge() {
    let _only = M_CHARGE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("r3", "stn-r3");
    let sources = "sources = { tracker = { provider = \"tracker\", interval_secs = 3600 } }\n";
    let state = kit(root.path(), tag, station, ("", true), sources);
    let m_key = charges::run_key(&ChargeScope::M);
    let flagging = m_key.clone();
    acting(m_key.clone(), move |tools, event| {
        if event["kind"] == "health" && event["detail"]["flag"] == "source-poll-frozen" {
            call(
                tools,
                &flagging,
                "raise_flag",
                json!({
                    "title": format!("Source {} is not polling", event["detail"]["source"]),
                    "text": event["detail"]["why"],
                    "severity": "warning",
                }),
            );
        }
    });

    // A poll that never returns holds up the desk, as a hung tracker does.
    let release = Arc::new(AtomicBool::new(false));
    let polling = {
        let (state, release) = (state.clone(), release.clone());
        std::thread::spawn(move || {
            let desk =
                SourceDesk::new("test", Default::default()).with_board(state.charges.polls.clone());
            let source = release.clone();
            let build = move |_: &str, _: &toml::Table| -> Result<Box<dyn Source>, String> {
                Ok(Box::new(Frozen(source.clone())))
            };
            let no_tracker = |_: &str, _: &toml::Table| Err("no tracker".to_owned());
            desk.run_with(&state.roster, state.intake.as_ref(), &build, &no_tracker);
        })
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while state
        .charges
        .polls
        .frozen(unix_now() + 3.0, Duration::from_secs(1))
        .is_empty()
    {
        assert!(Instant::now() < deadline, "the poll never began");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    charges::tend(&state, true, true).await;
    assert!(
        state.charges.is_live("m"),
        "M's charge runs on the Headquarters Station"
    );
    headquarters_until(&state, "M's charge flags the poll", || {
        !answered(&m_key, "raise_flag").is_empty()
    })
    .await;
    let flags = state
        .intake
        .reports(&ReportFilter::all().in_scope(ReportScope::M))
        .unwrap();
    assert_eq!(flags.len(), 1, "{flags:?}");
    assert!(flags[0].title.contains("R3/tracker"), "{}", flags[0].title);
    assert!(TOOL_NAMES.lock().unwrap()[&m_key].contains(&"raise_flag".to_owned()));
    assert_eq!(audits(&state, "flag.raised"), 1);
    // It acts through the flag alone: no Mission, brief or placement came of it.
    assert!(state.intake.list_new().unwrap().is_empty());

    release.store(true, Ordering::Release);
    polling.join().unwrap();
    assert!(
        state
            .charges
            .polls
            .frozen(unix_now(), Duration::from_secs(1))
            .is_empty(),
        "a poll that returned is not frozen"
    );
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_station_resumes_both_charges() {
    let _only = M_CHARGE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let (tag, station) = ("r4", "stn-r4");
    let (st_scope, st_key) = (format!("station:{station}"), station_key(station));
    let m_key = charges::run_key(&ChargeScope::M);

    let before = kit(root.path(), tag, station, ("", true), "");
    charges::tend(&before, true, true).await;
    wake(&before, &st_scope, "first");
    wake(&before, "m", "first");
    headquarters_until(&before, "a turn of each", || {
        turns_of(&before, &st_scope) == 1 && turns_of(&before, "m") == 1
    })
    .await;
    let station_run = before
        .intake
        .charge(&st_scope)
        .unwrap()
        .unwrap()
        .provider_id
        .unwrap();
    let m_run = before
        .intake
        .charge("m")
        .unwrap()
        .unwrap()
        .provider_id
        .unwrap();
    drop(before);

    let after = kit(root.path(), tag, station, ("", true), "");
    charges::tend(&after, true, true).await;
    wake(&after, &st_scope, "after the restart");
    wake(&after, "m", "after the restart");
    headquarters_until(&after, "a turn of each after the restart", || {
        turns_of(&after, &st_scope) == 2 && turns_of(&after, "m") == 2
    })
    .await;
    for (key, run) in [(&st_key, &station_run), (&m_key, &m_run)] {
        let resumed: Vec<Call> = calls(key)
            .into_iter()
            .filter(|call| call.how == "resume")
            .collect();
        assert_eq!(resumed.len(), 1, "{key}: {resumed:?}");
        assert_eq!(&resumed[0].run, run, "{key} resumes its own Run");
        assert!(resumed[0].text.contains("after the restart"));
    }
    for scope in [&st_scope, "m"] {
        let kept = after.intake.charge(scope).unwrap().unwrap();
        assert_eq!(kept.fence, 1, "the same Station renews under its own token");
        assert!(kept.logbook.iter().any(|entry| entry.kind == "resumed"));
    }
    charges::close_all(&after).await;
}
