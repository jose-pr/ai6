//! People attached to a charge's live Run: the history replayed, the live stream, a prompt that is
//! a turn of the same provider Run, answers that the first of any attached person decides, and
//! detaching that leaves the Run going.

use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};

use super::*;
use crate::bounds::{Limit, TimeLimit};
use crate::directorates::section_00::hub::{HubError, HubFrame, RunHub, RunTarget};
use crate::station::limits::AttachLimits;
use crate::station::server::attach::{HubDecider, LocalHub, RunPort};

fn target(tag: &str) -> RunTarget {
    RunTarget::Charge { scope: scope(tag) }
}

/// Reads `live` until `done` says a frame is the last wanted, and answers everything read.
fn read_until(
    live: &Receiver<HubFrame>,
    what: &str,
    mut done: impl FnMut(&HubFrame) -> bool,
) -> Vec<HubFrame> {
    tokio::task::block_in_place(|| {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut read = Vec::new();
        loop {
            assert!(Instant::now() < deadline, "never saw {what}: {read:?}");
            match live.recv_timeout(Duration::from_millis(100)) {
                Ok(frame) => {
                    let last = done(&frame);
                    read.push(frame);
                    if last {
                        return read;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => panic!("closed before {what}: {read:?}"),
            }
        }
    })
}

fn is(frame: &HubFrame, kind: &str) -> bool {
    frame.kind == kind
}

fn event_text(frame: &HubFrame) -> Option<&str> {
    (frame.kind == "event")
        .then(|| frame.data["text"].as_str())
        .flatten()
}

fn turn_end_by(frame: &HubFrame, who: &str) -> bool {
    frame.kind == "turn" && frame.data["state"] == "ended" && frame.data["by"] == who
}

/// Waits for the logbook to hold a `prompt` entry saying `text`: it is written once the turn is over.
async fn logged_prompt(state: &DaemonState, tag: &str, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let held = record(state, tag)
            .logbook
            .iter()
            .any(|entry| entry.kind == "prompt" && entry.text.contains(text));
        if held {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "never logged {text:?}: {:?}",
            record(state, tag).logbook
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A station whose charge has taken one turn, so it holds a Run, and the Run's name.
async fn charge_with_a_run(root: &Path, tag: &str) -> (DaemonState, String) {
    let state = station(root, tag, "test", "", "");
    charges::tend(&state, false, true).await;
    control(&state, tag, "first");
    tend_until(&state, "its first turn", || turns(&state, tag) == 1).await;
    let run = calls(&key(tag))[0].run.clone();
    (state, run)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attaching_replays_the_history_then_a_prompt_is_a_turn_of_the_same_run() {
    let root = tempfile::tempdir().unwrap();
    let tag = "a1";
    let (state, run) = charge_with_a_run(root.path(), tag).await;
    let hub: Arc<dyn RunHub> = state.hub.clone();

    let attachment = hub.attach(&target(tag), "c1", "alice", None).unwrap();
    let replay: Vec<&str> = attachment.replay.iter().map(|f| f.kind.as_str()).collect();
    assert_eq!(
        replay,
        ["prompt", "turn", "event", "event", "turn", "ready"],
        "{:?}",
        attachment.replay
    );
    assert_eq!(attachment.replay[0].data["from"], "station");
    assert!(
        attachment.replay[0].data["text"]
            .as_str()
            .unwrap()
            .contains("first")
    );
    assert_eq!(
        event_text(&attachment.replay[2]),
        Some(format!("noted on {run}").as_str())
    );
    assert_eq!(attachment.replay[5].data["busy"], false);
    assert_eq!(attachment.replay[5].client.as_deref(), Some("c1"));

    hub.prompt(&target(tag), "c1", "alice", "how is it going?")
        .unwrap();
    let live = read_until(&attachment.live, "the end of alice's turn", |frame| {
        turn_end_by(frame, "alice")
    });
    let kinds: Vec<&str> = live.iter().map(|f| f.kind.as_str()).collect();
    assert_eq!(
        kinds,
        ["attached", "prompt", "turn", "event", "event", "turn"],
        "{live:?}"
    );
    assert_eq!(live[1].data["from"], "alice");
    assert_eq!(live[1].data["text"], "how is it going?");

    // The turn ran in the Run the charge already had, and made no Mission.
    let made = calls(&key(tag));
    let last = made.last().unwrap();
    assert_eq!(
        (last.how, last.run.as_str(), last.text.as_str()),
        ("message", run.as_str(), "how is it going?")
    );
    assert_eq!(made.iter().filter(|call| call.how == "start").count(), 1);
    assert_eq!(
        record(&state, tag).provider_id.as_deref(),
        Some(run.as_str())
    );
    assert_no_mission(&state, tag);
    logged_prompt(&state, tag, "alice").await;
    // It is no wake: the hour's turns are not spent on a person.
    assert_eq!(turns(&state, tag), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_attached_people_see_a_turn_and_the_first_answer_to_a_request_decides() {
    let root = tempfile::tempdir().unwrap();
    let tag = "a2";
    let (state, _) = charge_with_a_run(root.path(), tag).await;
    let hub: Arc<dyn RunHub> = state.hub.clone();
    let one = hub.attach(&target(tag), "c1", "alice", None).unwrap();
    let two = hub.attach(&target(tag), "c2", "bob", None).unwrap();
    assert_eq!(two.attached.len(), 2);

    hub.prompt(&target(tag), "c1", "alice", "status?").unwrap();
    for live in [&one.live, &two.live] {
        let seen = read_until(live, "alice's turn", |frame| turn_end_by(frame, "alice"));
        assert!(
            seen.iter()
                .any(|f| is(f, "prompt") && f.data["from"] == "alice")
        );
        assert!(seen.iter().any(|f| event_text(f).is_some()));
    }

    // The Run asks leave; both see it, and bob answers before alice.
    let room = state.hub.room(&target(tag)).unwrap();
    let asking = std::thread::spawn({
        let room = room.clone();
        move || {
            room.ask(
                "Run a command",
                &[
                    ("allow".to_owned(), "Allow once".to_owned()),
                    ("refuse".to_owned(), "Refuse".to_owned()),
                ],
                &serde_json::json!({"command": "ls"}),
            )
        }
    });
    let mut asked = Vec::new();
    for live in [&one.live, &two.live] {
        let seen = read_until(live, "the request", |frame| is(frame, "permission"));
        asked.push(seen.last().unwrap().data["request"].as_u64().unwrap());
    }
    assert_eq!(asked[0], asked[1]);
    let request = asked[0];
    assert_eq!(
        hub.answer(&target(tag), "c2", "bob", request, "maybe"),
        Err(HubError::NotAnOption)
    );
    hub.answer(&target(tag), "c2", "bob", request, "allow")
        .unwrap();
    assert_eq!(
        hub.answer(&target(tag), "c1", "alice", request, "refuse"),
        Err(HubError::NoRequest),
        "the first answer decided"
    );
    assert_eq!(
        tokio::task::block_in_place(|| asking.join().unwrap()),
        Some(("bob".to_owned(), "allow".to_owned()))
    );
    for live in [&one.live, &two.live] {
        let seen = read_until(live, "the answer", |frame| is(frame, "answered"));
        let answered = seen.last().unwrap();
        assert_eq!(answered.data["by"], "bob");
        assert_eq!(answered.data["option"], "allow");
    }
    // Someone who is not attached answers nothing.
    assert_eq!(
        hub.answer(&target(tag), "c9", "mallory", request, "allow"),
        Err(HubError::NotAttached)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_during_a_wake_waits_for_the_wake_to_end() {
    let root = tempfile::tempdir().unwrap();
    let tag = "a3";
    let state = station(root.path(), tag, "test", "", "");
    let (release, gate) = channel::<()>();
    let gate = Mutex::new(gate);
    TURNS.lock().unwrap().insert(
        key(tag),
        Arc::new(move |text, _| {
            // The wake's turn is under way until the test lets it go.
            if text.contains("\"control\"") {
                let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(60));
            }
        }),
    );
    charges::tend(&state, false, true).await;
    control(&state, tag, "slow");
    tend_until(&state, "the wake to begin", || !calls(&key(tag)).is_empty()).await;
    let hub: Arc<dyn RunHub> = state.hub.clone();
    let attachment = hub.attach(&target(tag), "c1", "alice", None).unwrap();
    hub.prompt(&target(tag), "c1", "alice", "ahead of you?")
        .unwrap();
    tend_for(&state, 10).await;
    assert!(
        calls(&key(tag)).iter().all(|call| call.how != "message"),
        "it waits behind the wake: {:?}",
        calls(&key(tag))
    );

    assert_eq!(attachment.replay.last().unwrap().data["busy"], true);
    release.send(()).unwrap();
    let seen = read_until(&attachment.live, "alice's turn", |frame| {
        turn_end_by(frame, "alice")
    });
    let turns_seen: Vec<(String, String)> = seen
        .iter()
        .filter(|f| is(f, "turn"))
        .map(|f| {
            (
                f.data["state"].as_str().unwrap().to_owned(),
                f.data["by"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        turns_seen,
        [
            ("ended".to_owned(), "station".to_owned()),
            ("begun".to_owned(), "alice".to_owned()),
            ("ended".to_owned(), "alice".to_owned()),
        ],
        "the turns do not interleave: {seen:?}"
    );
    let made = calls(&key(tag));
    assert_eq!(made.last().unwrap().text, "ahead of you?");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detaching_leaves_the_run_running_and_a_later_attach_picks_it_up() {
    let root = tempfile::tempdir().unwrap();
    let tag = "a4";
    let (state, _) = charge_with_a_run(root.path(), tag).await;
    let hub: Arc<dyn RunHub> = state.hub.clone();
    let first = hub.attach(&target(tag), "c1", "alice", None).unwrap();
    let last_seen = first.replay.last().unwrap().data["last"].as_u64().unwrap();
    let room = state.hub.room(&target(tag)).unwrap();
    assert_eq!(room.attached(), 1);
    hub.detach(&target(tag), "c1");
    assert_eq!(room.attached(), 0);

    // The charge wakes with nobody attached and takes its turn.
    control(&state, tag, "second");
    tend_until(&state, "its second turn", || turns(&state, tag) == 2).await;
    assert!(state.charges.is_live(&scope(tag)));

    // A later attach, from where the first left off, is given what it missed and no more.
    let again = hub
        .attach(&target(tag), "c1", "alice", Some(last_seen))
        .unwrap();
    let kinds: Vec<&str> = again.replay.iter().map(|f| f.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "attached", "detached", "prompt", "turn", "event", "event", "turn", "ready"
        ],
        "{:?}",
        again.replay
    );
    assert!(again.replay.iter().all(|f| f.seq == 0 || f.seq > last_seen));
    // And it prompts the same Run, which went on.
    hub.prompt(&target(tag), "c1", "alice", "welcome back")
        .unwrap();
    read_until(&again.live, "the turn", |frame| turn_end_by(frame, "alice"));
    assert_eq!(
        calls(&key(tag)).iter().filter(|c| c.how == "start").count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupt_ends_the_turn_under_way_and_nothing_else() {
    let root = tempfile::tempdir().unwrap();
    let tag = "a5";
    let (state, _) = charge_with_a_run(root.path(), tag).await;
    let hub: Arc<dyn RunHub> = state.hub.clone();
    let attachment = hub.attach(&target(tag), "c1", "alice", None).unwrap();
    // Between turns an interrupt has nothing to cut.
    hub.interrupt(&target(tag), "c1", "alice").unwrap();

    hub.prompt(&target(tag), "c1", "alice", "hold").unwrap();
    read_until(&attachment.live, "the turn to begin", |frame| {
        frame.kind == "turn" && frame.data["state"] == "begun" && frame.data["by"] == "alice"
    });
    tokio::task::block_in_place(|| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !calls(&key(tag)).iter().any(|call| call.text == "hold") {
            assert!(Instant::now() < deadline, "the Run never took it");
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    hub.interrupt(&target(tag), "c1", "alice").unwrap();
    let seen = read_until(&attachment.live, "the turn to end", |frame| {
        turn_end_by(frame, "alice")
    });
    assert!(
        seen.iter()
            .any(|f| is(f, "interrupted") && f.data["by"] == "alice")
    );
    assert!(
        seen.iter()
            .any(|f| is(f, "event") && f.data["kind"] == "cancelled")
    );
    logged_prompt(&state, tag, "interrupted").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_run_tells_its_people_and_refuses_new_ones() {
    let root = tempfile::tempdir().unwrap();
    let tag = "a6";
    let (state, _) = charge_with_a_run(root.path(), tag).await;
    let hub: Arc<dyn RunHub> = state.hub.clone();
    let attachment = hub.attach(&target(tag), "c1", "alice", None).unwrap();
    charges::close_all(&state).await;
    let seen = read_until(&attachment.live, "the Run to close", |frame| {
        is(frame, "closed")
    });
    assert!(seen.last().unwrap().data["reason"].is_string());
    assert_eq!(
        hub.prompt(&target(tag), "c1", "alice", "anyone?"),
        Err(HubError::NoRun)
    );
    assert!(matches!(
        hub.attach(&target(tag), "c2", "bob", None),
        Err(HubError::NoRun)
    ));
}

/// A port that takes what it is given.
struct Quiet;

impl RunPort for Quiet {
    fn prompt(&self, _: &str, _: &str) -> Result<(), HubError> {
        Ok(())
    }

    fn interrupt(&self, _: &str) -> Result<(), HubError> {
        Ok(())
    }
}

fn quiet_hub(limits: AttachLimits) -> (LocalHub, RunTarget) {
    let hub = LocalHub::new(limits);
    let target = RunTarget::Mission {
        operation: "G".to_owned(),
        mission: "m-1".to_owned(),
    };
    hub.open(&target, Arc::new(Quiet));
    (hub, target)
}

#[test]
fn the_history_clients_and_waiting_are_bounded_by_settings() {
    let limits = AttachLimits {
        history_frames: Limit::at(3),
        clients: Limit::at(1),
        ..AttachLimits::default()
    };
    let (hub, target) = quiet_hub(limits);
    let room = hub.room(&target).unwrap();
    for number in 0..10 {
        room.publish_error(&format!("frame {number}"));
    }
    let attachment = hub.attach(&target, "c1", "alice", None).unwrap();
    // The three newest and the `ready` frame.
    let reasons: Vec<String> = attachment
        .replay
        .iter()
        .filter_map(|f| f.data["reason"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(reasons, ["frame 7", "frame 8", "frame 9"]);
    assert!(matches!(
        hub.attach(&target, "c2", "bob", None),
        Err(HubError::TooManyClients)
    ));
    // The same client attaching again is not another one.
    assert!(hub.attach(&target, "c1", "alice", None).is_ok());
}

#[test]
fn a_request_nobody_answers_is_left_to_its_asker() {
    let limits = AttachLimits {
        permission_wait: TimeLimit::at(Duration::from_millis(100)),
        ..AttachLimits::default()
    };
    let (hub, target) = quiet_hub(limits);
    let room = hub.room(&target).unwrap();
    let options = [("allow".to_owned(), "Allow".to_owned())];
    // Nobody attached: no waiting at all.
    assert_eq!(room.ask("a tool", &options, &serde_json::Value::Null), None);
    let attachment = hub.attach(&target, "c1", "alice", None).unwrap();
    assert_eq!(room.ask("a tool", &options, &serde_json::Value::Null), None);
    let kinds: Vec<String> = attachment.live.try_iter().map(|f| f.kind).collect();
    assert_eq!(kinds, ["attached", "permission", "answered"]);
    // Everyone leaving while it waits ends the wait.
    limits_unbounded_wait_ends_when_the_last_client_leaves();
}

fn limits_unbounded_wait_ends_when_the_last_client_leaves() {
    let limits = AttachLimits {
        permission_wait: TimeLimit::UNBOUNDED,
        ..AttachLimits::default()
    };
    let (hub, target) = quiet_hub(limits);
    let room = hub.room(&target).unwrap();
    hub.attach(&target, "c1", "alice", None).unwrap();
    let asking = std::thread::spawn({
        let room = room.clone();
        move || {
            room.ask(
                "a tool",
                &[("allow".to_owned(), "Allow".to_owned())],
                &serde_json::Value::Null,
            )
        }
    });
    std::thread::sleep(Duration::from_millis(100));
    hub.detach(&target, "c1");
    assert_eq!(asking.join().unwrap(), None);
}

/// An external ACP agent's request to run a command reaches the people attached, whoever of them
/// answers first decides it, and what they chose is what the agent is told.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_acp_agents_permission_request_goes_to_the_people_attached() {
    use crate::directorates::section_00::acp::{AcpOptions, AcpRun};

    let hub = Arc::new(LocalHub::default());
    let target = RunTarget::Mission {
        operation: "G".to_owned(),
        mission: "ext".to_owned(),
    };
    let room = hub.open(&target, Arc::new(Quiet));
    let attachment = hub.attach(&target, "c1", "alice", None).unwrap();
    let outcome = Arc::new(Mutex::new(None));
    let channel = crate::station::acp::testing::command_agent("git push", outcome.clone());
    let options = AcpOptions {
        prompt: Some("go".to_owned()),
        decider: HubDecider::refusing(&hub, target.clone()),
        ..AcpOptions::default()
    };
    let reading = std::thread::spawn({
        let room = room.clone();
        move || {
            let run = AcpRun::connect(channel, options, None).unwrap();
            let deadline = Instant::now() + Duration::from_secs(60);
            while Instant::now() < deadline {
                if let Some(event) = run.next_event(100).unwrap() {
                    room.publish_event(&event);
                    if event.kind == "completed" {
                        return;
                    }
                }
            }
            panic!("the agent's turn never ended");
        }
    });
    let seen = read_until(&attachment.live, "the request", |frame| {
        is(frame, "permission")
    });
    let asked = seen.last().unwrap();
    assert_eq!(asked.data["tool"], "Run a command");
    assert_eq!(asked.data["detail"]["input"]["command"], "git push");
    let request = asked.data["request"].as_u64().unwrap();
    hub.answer(&target, "c1", "alice", request, "allow")
        .unwrap();
    tokio::task::block_in_place(|| reading.join().unwrap());
    let told = outcome
        .lock()
        .unwrap()
        .clone()
        .expect("the agent was answered");
    assert_eq!(told["outcome"]["optionId"], "yes");
    read_until(&attachment.live, "the end of its turn", |frame| {
        event_text(frame).is_none() && frame.data["kind"] == "completed"
    });
}
