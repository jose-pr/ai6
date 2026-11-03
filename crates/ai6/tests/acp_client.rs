//! The ACP client's own bookkeeping, against an agent that leaves, stays silent, cancels on its
//! own, runs out of credits or lost output: what the Station is told, and what it acknowledges.

use agent_client_protocol::{
    Agent, Channel, Client, ConnectionTo, Responder, UntypedMessage, on_receive_notification,
    on_receive_request,
};
use ai6::directorates::section_00::acp::{AcpOptions, AcpSession, duplex};
use coqui_acp_provider::{AgentEvent, AgentSession, CANCELLED_EVENT, LIMIT_EVENT, TURN_CANCELLED};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum Behaviour {
    /// Speaks AI6's event extension, ends the turn with `completed`, then hangs up.
    EndsThenLeaves,
    /// Speaks the extension, says some text, then hangs up with the turn unfinished.
    LeavesMidTurn,
    /// Speaks the extension, never says the operation started and stays connected.
    NeverStarts,
    /// A plain ACP agent that answers its prompt as cancelled.
    CancelsByItself,
    /// A plain ACP agent that hangs up with its turn unfinished.
    PlainLeavesMidTurn,
    /// A plain ACP agent that fails its prompt as the Grok CLI does when its account is out of
    /// credits: an internal error whose reason is in an object.
    OutOfCredits,
    /// A plain ACP agent that says a bridge lost some of its output, then ends the turn.
    LosesOutput,
}

#[derive(Default)]
struct Seen {
    acks: Mutex<Vec<u64>>,
    closes: AtomicUsize,
}

fn extension_event(seq: u64, kind: &str, text: Option<&str>) -> Value {
    let event = AgentEvent {
        kind: kind.to_owned(),
        text: text.map(str::to_owned),
        data: None,
        limit: None,
        usage: None,
        tools: Vec::new(),
        quota: Vec::new(),
    };
    json!({
        "sessionId": "ext-1",
        "providerSessionId": "provider-1",
        "seq": seq,
        "event": serde_json::to_value(event).unwrap(),
    })
}

fn tell(cx: &ConnectionTo<Client>, params: Value) {
    cx.send_notification(UntypedMessage::new("_ai6/event", params).unwrap())
        .unwrap();
}

fn serve(behaviour: Behaviour, seen: Arc<Seen>, transport: Channel) {
    std::thread::spawn(move || {
        let (hang_up, hung_up) = futures::channel::oneshot::channel::<()>();
        let hang_up = Arc::new(Mutex::new(Some(hang_up)));
        let for_requests = seen.clone();
        let for_notifications = seen.clone();
        let _ = futures::executor::block_on(
            Agent
                .builder()
                .on_receive_request(
                    async move |message: UntypedMessage,
                                responder: Responder<Value>,
                                cx: ConnectionTo<Client>| {
                        match message.method() {
                            "initialize" => {
                                let meta = if matches!(
                                    behaviour,
                                    Behaviour::CancelsByItself
                                        | Behaviour::PlainLeavesMidTurn
                                        | Behaviour::OutOfCredits
                                        | Behaviour::LosesOutput
                                ) {
                                    json!({})
                                } else {
                                    json!({"dev.coqui.ai6/events": true})
                                };
                                responder
                                    .respond(json!({
                                        "protocolVersion": 1,
                                        "agentCapabilities": {"_meta": meta},
                                        "authMethods": [],
                                    }))
                                    .unwrap();
                            }
                            "session/new" => {
                                responder.respond(json!({"sessionId": "ext-1"})).unwrap();
                            }
                            "session/close" => {
                                for_requests.closes.fetch_add(1, Ordering::SeqCst);
                                responder.respond(json!({})).unwrap();
                            }
                            "session/prompt" => {
                                let hang_up = hang_up.clone();
                                let seen = for_requests.clone();
                                std::thread::spawn(move || {
                                    let leave = || {
                                        if let Some(hang_up) = hang_up.lock().unwrap().take() {
                                            let _ = hang_up.send(());
                                        }
                                    };
                                    match behaviour {
                                        Behaviour::EndsThenLeaves => {
                                            tell(
                                                &cx,
                                                json!({
                                                    "sessionId": "ext-1",
                                                    "providerSessionId": "provider-1",
                                                }),
                                            );
                                            tell(&cx, extension_event(1, "text", Some("hi")));
                                            tell(&cx, extension_event(2, "completed", None));
                                            // Leaves once the client has acknowledged the first, so the acknowledgement had a connection.
                                            let deadline =
                                                Instant::now() + Duration::from_secs(120);
                                            while seen.acks.lock().unwrap().is_empty()
                                                && Instant::now() < deadline
                                            {
                                                std::thread::sleep(Duration::from_millis(10));
                                            }
                                            leave();
                                        }
                                        Behaviour::LeavesMidTurn => {
                                            tell(
                                                &cx,
                                                json!({
                                                    "sessionId": "ext-1",
                                                    "providerSessionId": "provider-1",
                                                }),
                                            );
                                            tell(&cx, extension_event(1, "text", Some("hi")));
                                            leave();
                                        }
                                        Behaviour::NeverStarts => {
                                            let _ = &responder;
                                            std::thread::sleep(Duration::from_secs(600));
                                        }
                                        Behaviour::CancelsByItself => {
                                            responder
                                                .respond(json!({"stopReason": "cancelled"}))
                                                .unwrap();
                                        }
                                        Behaviour::PlainLeavesMidTurn => {
                                            let _ = &responder;
                                            leave();
                                        }
                                        Behaviour::OutOfCredits => {
                                            responder
                                                .respond_with_error(
                                                    agent_client_protocol::Error::internal_error()
                                                        .data(json!({
                                                            "message": "API error (status 402 Payment Required): personal-team-blocked:spending-limit: You have run out of credits. Add credits to continue.",
                                                            "http_status": 402,
                                                        })),
                                                )
                                                .unwrap();
                                        }
                                        Behaviour::LosesOutput => {
                                            cx.send_notification(
                                                UntypedMessage::new(
                                                    "_dev.boriken.coqui.acp/output_lost",
                                                    json!({"lines": 3, "bytes": 120}),
                                                )
                                                .unwrap(),
                                            )
                                            .unwrap();
                                            responder
                                                .respond(json!({"stopReason": "end_turn"}))
                                                .unwrap();
                                        }
                                    }
                                });
                            }
                            _ => {
                                responder
                                    .respond_with_error(
                                        agent_client_protocol::Error::method_not_found(),
                                    )
                                    .unwrap();
                            }
                        }
                        Ok(())
                    },
                    on_receive_request!(),
                )
                .on_receive_notification(
                    async move |message: UntypedMessage, _cx: ConnectionTo<Client>| {
                        if message.method() == "_ai6/ack"
                            && let Some(seq) = message.params()["seq"].as_u64()
                        {
                            for_notifications.acks.lock().unwrap().push(seq);
                        }
                        Ok(())
                    },
                    on_receive_notification!(),
                )
                .connect_with(transport, async move |_cx| {
                    let _ = hung_up.await;
                    Ok(())
                }),
        );
    });
}

fn options() -> AcpOptions {
    AcpOptions {
        cwd: "/work".to_owned(),
        prompt: Some("go".to_owned()),
        ..AcpOptions::default()
    }
}

fn connect(
    behaviour: Behaviour,
    options: AcpOptions,
) -> (
    Result<AcpSession, ai6::guatiao::library::ProviderError>,
    Arc<Seen>,
) {
    let seen = Arc::new(Seen::default());
    let (agent_end, client_end) = duplex();
    serve(behaviour, seen.clone(), agent_end);
    (AcpSession::connect(client_end, options, None), seen)
}

/// Events until `done` says so, or the agent has left and nothing more comes, within a deadline.
fn events_until(session: &AcpSession, done: impl Fn(&[AgentEvent]) -> bool) -> Vec<AgentEvent> {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Some(event) = session.next_event(100).unwrap() {
            seen.push(event);
        }
        if done(&seen) {
            return seen;
        }
    }
    panic!("the events never satisfied the test; saw {seen:?}");
}

fn left(session: &AcpSession) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !session.connection_lost() {
        assert!(Instant::now() < deadline, "the agent never left");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn exit_detail() -> Option<ai6::directorates::section_00::acp::DisconnectDetail> {
    Some(Box::new(|| Some("it exited with code 7".to_owned())))
}

#[test]
fn a_turn_the_agent_ended_is_not_failed_by_the_agent_leaving_and_what_was_taken_is_acknowledged() {
    let (session, seen) = connect(
        Behaviour::EndsThenLeaves,
        AcpOptions {
            disconnect_detail: exit_detail(),
            ..options()
        },
    );
    let session = session.unwrap();
    assert_eq!(
        session.session_id().unwrap().unwrap().as_str(),
        "provider-1",
        "the operation said who it is before the Session was handed out"
    );
    let events = events_until(&session, |events| {
        events.iter().any(|event| event.kind == "completed")
    });
    left(&session);
    // Asking again says the caller dealt with the last one; the connection is gone by now.
    let after = session.next_event(500).unwrap();
    assert_eq!(after, None, "no failure after a finished turn");
    assert!(
        events.iter().all(|event| event.kind != "failed"),
        "{events:?}"
    );
    assert_eq!(session.last_seq(), 2);
    assert!(
        seen.acks.lock().unwrap().contains(&1),
        "the first event taken was acknowledged: {:?}",
        seen.acks.lock()
    );
}

#[test]
fn a_turn_left_unfinished_by_the_agent_leaving_fails_with_what_became_of_it() {
    let (session, _) = connect(
        Behaviour::LeavesMidTurn,
        AcpOptions {
            disconnect_detail: exit_detail(),
            ..options()
        },
    );
    let events = events_until(&session.unwrap(), |events| {
        events.iter().any(|event| event.kind == "failed")
    });
    let failed = events.last().unwrap().text.as_deref().unwrap();
    assert!(
        failed.contains("the agent closed its connection"),
        "{failed}"
    );
    assert!(failed.contains("it exited with code 7"), "{failed}");
    assert_eq!(
        events[0].text.as_deref(),
        Some("hi"),
        "its words come first"
    );
}

#[test]
fn a_connection_that_may_drop_without_failing_the_turn_says_only_that_it_dropped() {
    let (session, _) = connect(
        Behaviour::LeavesMidTurn,
        AcpOptions {
            fail_on_disconnect: false,
            disconnect_detail: exit_detail(),
            ..options()
        },
    );
    let session = session.unwrap();
    left(&session);
    let mut seen = Vec::new();
    while let Some(event) = session.next_event(500).unwrap() {
        seen.push(event);
    }
    assert!(seen.iter().all(|event| event.kind != "failed"), "{seen:?}");
    assert!(session.connection_lost());
}

#[test]
fn an_operation_that_never_starts_is_a_start_failure_with_its_own_limit() {
    let started = Instant::now();
    let (session, _) = connect(
        Behaviour::NeverStarts,
        AcpOptions {
            start_timeout_ms: 300,
            ..options()
        },
    );
    let error = session.err().expect("the start times out");
    assert!(
        error.to_string().contains("did not start within 300 ms"),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn an_interrupted_turn_completes_and_one_the_agent_cancels_by_itself_is_cancelled() {
    let (session, _) = connect(Behaviour::CancelsByItself, options());
    let session = session.unwrap();
    session.interrupt().unwrap();
    let events = events_until(&session, |events| !events.is_empty());
    assert_eq!(events.last().unwrap().kind, "completed");

    let (session, _) = connect(Behaviour::CancelsByItself, options());
    let events = events_until(&session.unwrap(), |events| !events.is_empty());
    let last = events.last().unwrap();
    assert_eq!(last.kind, CANCELLED_EVENT);
    assert_eq!(last.text.as_deref(), Some(TURN_CANCELLED));
}

#[test]
fn a_prompt_error_with_its_reason_in_an_object_is_a_limit_when_its_words_say_so() {
    let (session, _) = connect(
        Behaviour::OutOfCredits,
        AcpOptions {
            limit_phrases: vec!["spending-limit".to_owned()],
            ..options()
        },
    );
    let events = events_until(&session.unwrap(), |events| !events.is_empty());
    let last = events.last().unwrap();
    assert_eq!(last.kind, LIMIT_EVENT, "{events:?}");
    let text = last.text.as_deref().unwrap();
    assert!(
        text.starts_with("Internal error: API error (status 402 Payment Required)"),
        "the reason in the error's object is kept: {text}"
    );
    assert!(last.limit.is_some());
}

#[test]
fn a_lost_connection_is_never_read_as_a_limit_whatever_the_agent_last_wrote() {
    let (session, _) = connect(
        Behaviour::PlainLeavesMidTurn,
        AcpOptions {
            limit_phrases: vec!["rate limited".to_owned()],
            disconnect_detail: Some(Box::new(|| Some("it last wrote: Rate limited".to_owned()))),
            ..options()
        },
    );
    let events = events_until(&session.unwrap(), |events| !events.is_empty());
    let last = events.last().unwrap();
    assert_eq!(last.kind, "failed", "{events:?}");
    let text = last.text.as_deref().unwrap();
    assert!(text.contains("the agent closed its connection"), "{text}");
    assert!(text.contains("Rate limited"), "{text}");
    assert!(last.limit.is_none());
}

#[test]
fn output_a_bridge_lost_fails_the_turn_with_what_was_lost() {
    let (session, _) = connect(Behaviour::LosesOutput, options());
    let events = events_until(&session.unwrap(), |events| {
        events.iter().any(|event| event.kind == "completed")
    });
    let failed = events
        .iter()
        .position(|event| event.kind == "failed")
        .expect("the lost output fails the turn");
    let completed = events
        .iter()
        .position(|event| event.kind == "completed")
        .unwrap();
    assert!(failed < completed, "{events:?}");
    let text = events[failed].text.as_deref().unwrap();
    assert!(
        text.contains("3 lines") && text.contains("120 bytes"),
        "{text}"
    );
}
