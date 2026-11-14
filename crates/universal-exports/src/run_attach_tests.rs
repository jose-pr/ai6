//! People attached to a Run through the gateway, with a stand-in Station: who may attach, what
//! they are shown, what they may type, answer and interrupt, and what is audited. The Station's
//! side of the same attachment (the Run's history, its turns, the first answer deciding) is tested
//! with the Station.

use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::runs::NewRun;
use ai6::directorates::section_00::hub::{HubCommand, HubDelivery, HubFrame, HubOp, RunTarget};
use futures_util::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::controls::tests::{STATION_TOKEN, Served, served_with};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// `operator-007` may talk to Operation G's charge but not control it; `viewer-007` may only
/// read.
const CONVERSE: &str = r#"
    [role.converser]
    actions = ["charge.converse"]

    [role.viewer]
    actions = ["mission.read"]

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "converser"

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "viewer"
"#;

/// `operator-007` may also control G's Missions and Station `worker`'s charge.
const CONTROL: &str = r#"
    [role.converser]
    actions = ["charge.converse", "mission.control"]

    [role.station]
    actions = ["charge.converse", "station.configure"]

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "converser"

    [[rule]]
    subject = "person:operator-007"
    scope = "station:worker"
    role = "station"
"#;

/// A served gateway whose Operation G's charge is kept by Station `worker`.
async fn served_charge(roles: &str) -> Served {
    let served = served_with(roles).await;
    for scope in ["operation:G", "station:worker"] {
        served
            .registry
            .claim_charge(scope, "kg", "worker", Duration::from_secs(600))
            .unwrap();
    }
    served
}

fn url(served: &Served) -> String {
    format!("{}/v1/socket/attach", served.base.replacen("http", "ws", 1))
}

async fn connect(served: &Served, hello: Value) -> Socket {
    let (mut socket, _) = tokio_tungstenite::connect_async(url(served)).await.unwrap();
    socket
        .send(WsMessage::Text(hello.to_string().into()))
        .await
        .unwrap();
    socket
}

fn hello(token: &str, charge: &str) -> Value {
    json!({"type": "hello", "token": token, "charge": charge})
}

async fn receive(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), socket.next())
            .await
            .expect("the gateway answers")
            .expect("the socket is open")
            .unwrap();
        match message {
            WsMessage::Text(text) => return serde_json::from_str(&text).unwrap(),
            WsMessage::Close(_) => return json!({"type": "closed"}),
            _ => {}
        }
    }
}

/// What the Station is shown of the people attached: its poll, as the Station makes it.
async fn poll(served: &Served, wait_ms: u64) -> Vec<HubCommand> {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/v1/station/hub/next?station=worker&wait_ms={wait_ms}",
            served.base
        ))
        .bearer_auth(STATION_TOKEN)
        .send()
        .await
        .unwrap();
    if response.status() == StatusCode::NO_CONTENT {
        return Vec::new();
    }
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

async fn deliver(served: &Served, target: &RunTarget, frames: Vec<HubFrame>) -> StatusCode {
    let delivery = HubDelivery::new("worker", target.clone(), frames);
    reqwest::Client::new()
        .post(format!("{}/v1/station/hub/frames", served.base))
        .bearer_auth(STATION_TOKEN)
        .json(&delivery)
        .send()
        .await
        .unwrap()
        .status()
}

fn charge_target(scope: &str) -> RunTarget {
    RunTarget::Charge {
        scope: scope.to_owned(),
    }
}

fn audits(served: &Served, action: &str) -> Vec<ai6::AuditRecord> {
    served
        .registry
        .list_audit(100)
        .unwrap()
        .into_iter()
        .filter(|record| record.action == action)
        .collect()
}

/// Attaches `token`'s person to `charge`, takes the Attach command a Station would, and answers
/// the socket's `attached` frame with the client.
async fn attach(served: &Served, token: &str, charge: &str) -> (Socket, HubCommand) {
    let mut socket = connect(served, hello(token, charge)).await;
    let attached = receive(&mut socket).await;
    assert_eq!(attached["type"], "attached", "{attached}");
    // What waits for the Station may include other clients' detaches: this one's attach is what is asked.
    let mut commands = poll(served, 5_000).await;
    commands.retain(|command| command.client == attached["client"].as_str().unwrap_or_default());
    assert_eq!(commands.len(), 1, "{commands:?}");
    let command = commands.remove(0);
    assert_eq!(attached["client"], command.client.as_str());
    (socket, command)
}

fn frame(kind: &str, data: Value) -> HubFrame {
    HubFrame::new(kind, 1.0, data)
}

#[tokio::test]
async fn a_person_without_charge_converse_on_the_scope_is_refused() {
    let served = served_charge(CONVERSE).await;
    // Reading is not conversing.
    let mut viewer = connect(&served, hello(&served.viewer, "operation:G")).await;
    let refused = receive(&mut viewer).await;
    assert_eq!(refused["type"], "error");
    assert_eq!(refused["code"], "forbidden");
    assert_eq!(receive(&mut viewer).await["type"], "closed");
    // Nor is the action on one scope the action on another.
    for scope in ["operation:L", "station:worker", "m"] {
        let mut operator = connect(&served, hello(&served.operator, scope)).await;
        assert_eq!(receive(&mut operator).await["code"], "forbidden", "{scope}");
    }
    // Nothing was passed to the Station or audited as an attachment.
    assert!(poll(&served, 0).await.is_empty());
    assert!(audits(&served, "charge.attach").is_empty());
    // A token that is nobody's, and a frame that is not a hello, get nothing either.
    let mut stranger = connect(&served, hello("not-a-token", "operation:G")).await;
    assert_eq!(receive(&mut stranger).await["code"], "unauthorized");
    let (mut mute, _) = tokio_tungstenite::connect_async(url(&served))
        .await
        .unwrap();
    mute.send(WsMessage::Text("{}".into())).await.unwrap();
    assert_eq!(receive(&mut mute).await["type"], "closed");
}

#[tokio::test]
async fn attaching_replays_the_history_and_then_streams_what_the_run_shows() {
    let served = served_charge(CONVERSE).await;
    let target = charge_target("operation:G");
    let (mut socket, command) = attach(&served, &served.operator, "operation:G").await;
    assert_eq!(command.target, target);
    assert_eq!(command.person, "operator-007");
    assert_eq!(command.op, HubOp::Attach { after: None });
    assert_eq!(audits(&served, "charge.attach").len(), 1);

    // The Station replays what the Run showed, addressed to the client, then goes live.
    let replay = vec![
        frame("prompt", json!({"from": "station", "text": "wake"})).to(&command.client),
        frame("event", json!({"kind": "text", "text": "noted"})).to(&command.client),
        frame("ready", json!({"busy": false})).to(&command.client),
    ];
    assert_eq!(
        deliver(&served, &target, replay).await,
        StatusCode::NO_CONTENT
    );
    let live = vec![
        frame("event", json!({"kind": "tool", "text": "Run a command"})),
        frame("event", json!({"kind": "plan", "text": "1. look"})),
        frame(
            "permission",
            json!({"request": 1, "tool": "Run a command", "options": []}),
        ),
    ];
    assert_eq!(
        deliver(&served, &target, live).await,
        StatusCode::NO_CONTENT
    );
    let mut seen = Vec::new();
    for _ in 0..6 {
        let shown = receive(&mut socket).await;
        assert_eq!(shown["type"], "frame");
        seen.push(shown["frame"]["kind"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        seen,
        ["prompt", "event", "ready", "event", "event", "permission"]
    );
}

#[tokio::test]
async fn a_prompt_is_stamped_with_who_sent_it_and_audited_and_makes_no_mission() {
    let served = served_charge(CONVERSE).await;
    let (mut socket, attached) = attach(&served, &served.operator, "operation:G").await;
    socket
        .send(WsMessage::Text(
            json!({"type": "prompt", "text": "what is the state of G-1?"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let commands = poll(&served, 5_000).await;
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!(commands[0].person, "operator-007");
    assert_eq!(commands[0].client, attached.client);
    assert_eq!(
        commands[0].op,
        HubOp::Prompt {
            text: "what is the state of G-1?".to_owned()
        }
    );
    let audited = audits(&served, "charge.prompt");
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].actor, "operator-007");
    assert_eq!(audited[0].target, "charge:operation:G");
    assert!(
        !audited[0].target.contains("state of"),
        "never the prompt itself"
    );
    // It is a turn of a Run that already exists: no Mission or brief was made for it.
    assert!(served.registry.list_new().unwrap().is_empty());
    // A blank prompt, or one past the limit, is refused here.
    socket
        .send(WsMessage::Text(
            json!({"type": "prompt", "text": "  "}).to_string().into(),
        ))
        .await
        .unwrap();
    assert_eq!(receive(&mut socket).await["code"], "bad_request");
    assert!(poll(&served, 0).await.is_empty());
}

#[tokio::test]
async fn two_attached_people_both_see_a_frame_and_both_answers_reach_the_station_in_order() {
    let served = served_charge(CONVERSE).await;
    let target = charge_target("operation:G");
    let (mut first, one) = attach(&served, &served.operator, "operation:G").await;
    let (mut second, two) = attach(&served, &served.operator, "operation:G").await;
    assert_ne!(one.client, two.client);

    // A frame for nobody in particular is shown to everybody attached.
    let shown = vec![frame("event", json!({"kind": "text", "text": "both"}))];
    assert_eq!(
        deliver(&served, &target, shown).await,
        StatusCode::NO_CONTENT
    );
    for socket in [&mut first, &mut second] {
        let seen = receive(socket).await;
        assert_eq!(seen["frame"]["data"]["text"], "both");
    }
    // A frame for one is shown to that one.
    let alone = vec![frame("error", json!({"reason": "yours"})).to(&two.client)];
    assert_eq!(
        deliver(&served, &target, alone).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        receive(&mut second).await["frame"]["data"]["reason"],
        "yours"
    );

    // Either may answer; the gateway passes each to the Station, which decides the first.
    for (socket, option) in [(&mut second, "allow"), (&mut first, "refuse")] {
        socket
            .send(WsMessage::Text(
                json!({"type": "answer", "request": 7, "option": option})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut answers = poll(&served, 5_000).await;
    while answers.len() < 2 {
        answers.extend(poll(&served, 5_000).await);
    }
    let heard: Vec<(String, HubOp)> = answers
        .into_iter()
        .map(|command| (command.client, command.op))
        .collect();
    assert_eq!(
        heard,
        [
            (
                two.client.clone(),
                HubOp::Answer {
                    request: 7,
                    option: "allow".to_owned()
                }
            ),
            (
                one.client.clone(),
                HubOp::Answer {
                    request: 7,
                    option: "refuse".to_owned()
                }
            ),
        ]
    );
    assert_eq!(audits(&served, "charge.answer").len(), 2);
    // A frame for a client that is not attached is refused, so a Station stops sending to it.
    let gone = vec![frame("event", json!({})).to("nobody")];
    assert_eq!(deliver(&served, &target, gone).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn detaching_tells_the_station_and_a_later_attach_is_a_new_request_for_the_same_run() {
    let served = served_charge(CONVERSE).await;
    let (mut socket, first) = attach(&served, &served.operator, "operation:G").await;
    socket
        .send(WsMessage::Text(
            json!({"type": "detach"}).to_string().into(),
        ))
        .await
        .unwrap();
    let commands = poll(&served, 5_000).await;
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!(commands[0].op, HubOp::Detach);
    assert_eq!(commands[0].client, first.client);
    // A socket that just closes is a detach too.
    let (socket, second) = attach(&served, &served.operator, "operation:G").await;
    drop(socket);
    let commands = poll(&served, 5_000).await;
    assert_eq!(commands[0].op, HubOp::Detach);
    assert_eq!(commands[0].client, second.client);
    // The Run was not touched: attaching again asks for where it left off.
    let mut again = connect(
        &served,
        json!({"type": "hello", "token": served.operator, "charge": "operation:G", "after": 41}),
    )
    .await;
    assert_eq!(receive(&mut again).await["type"], "attached");
    let commands = poll(&served, 5_000).await;
    assert_eq!(commands[0].op, HubOp::Attach { after: Some(41) });
    // The audit of a socket that just closed is written as the gateway notices.
    for _ in 0..100 {
        if audits(&served, "charge.detach").len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(audits(&served, "charge.detach").len(), 2);
}

#[tokio::test]
async fn interrupting_needs_control_on_the_scope_and_not_only_the_right_to_converse() {
    let served = served_charge(CONVERSE).await;
    let (mut socket, _) = attach(&served, &served.operator, "operation:G").await;
    socket
        .send(WsMessage::Text(
            json!({"type": "interrupt"}).to_string().into(),
        ))
        .await
        .unwrap();
    assert_eq!(receive(&mut socket).await["code"], "forbidden");
    assert!(poll(&served, 0).await.is_empty());
    assert!(audits(&served, "charge.interrupt").is_empty());

    // With `mission.control` on the Operation, and `station.configure` on a Station's charge.
    let served = served_charge(CONTROL).await;
    let mut open = Vec::new();
    for scope in ["operation:G", "station:worker"] {
        let (mut socket, attached) = attach(&served, &served.operator, scope).await;
        socket
            .send(WsMessage::Text(
                json!({"type": "interrupt"}).to_string().into(),
            ))
            .await
            .unwrap();
        let commands = poll(&served, 5_000).await;
        assert_eq!(commands.len(), 1, "{scope}: {commands:?}");
        assert_eq!(commands[0].op, HubOp::Interrupt);
        assert_eq!(commands[0].client, attached.client);
        open.push(socket);
    }
    assert_eq!(audits(&served, "charge.interrupt").len(), 2);
}

#[tokio::test]
async fn a_missions_run_is_attached_under_control_of_its_operation() {
    let mission = |token: &str| {
        json!({"type": "hello", "token": token,
               "mission": {"operation": "G", "mission": "mission-g"}})
    };
    // Conversing with G's charge is not controlling its Missions.
    let served = served_charge(CONVERSE).await;
    let mut refused = connect(&served, mission(&served.operator)).await;
    assert_eq!(receive(&mut refused).await["code"], "forbidden");

    let served = served_charge(CONTROL).await;
    let mut socket = connect(&served, mission(&served.operator)).await;
    assert_eq!(receive(&mut socket).await["type"], "attached");
    let commands = poll(&served, 5_000).await;
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!(
        commands[0].target,
        RunTarget::Mission {
            operation: "G".to_owned(),
            mission: "mission-g".to_owned()
        }
    );
    assert_eq!(audits(&served, "mission.attach").len(), 1);
    socket
        .send(WsMessage::Text(
            json!({"type": "prompt", "text": "carry on"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let commands = poll(&served, 5_000).await;
    assert!(matches!(&commands[0].op, HubOp::Prompt { text } if text == "carry on"));
    assert_eq!(audits(&served, "mission.prompt").len(), 1);
    // A Mission no Station runs a Run of is not attachable.
    let mut none = connect(
        &served,
        json!({"type": "hello", "token": served.operator,
               "mission": {"operation": "G", "mission": "mission-none"}}),
    )
    .await;
    assert_eq!(receive(&mut none).await["code"], "no_run");
}

#[tokio::test]
async fn only_the_station_may_poll_and_deliver() {
    let served = served_charge(CONVERSE).await;
    let client = reqwest::Client::new();
    let polled = client
        .get(format!(
            "{}/v1/station/hub/next?station=worker&wait_ms=0",
            served.base
        ))
        .bearer_auth(&served.operator)
        .send()
        .await
        .unwrap();
    assert_eq!(polled.status(), StatusCode::UNAUTHORIZED);
    let delivery = HubDelivery::new(
        "worker",
        charge_target("operation:G"),
        vec![frame("event", json!({}))],
    );
    let delivered = client
        .post(format!("{}/v1/station/hub/frames", served.base))
        .bearer_auth(&served.operator)
        .json(&delivery)
        .send()
        .await
        .unwrap();
    assert_eq!(delivered.status(), StatusCode::UNAUTHORIZED);
    // A charge no Station holds the lease of is not attachable.
    let served = served_with(CONVERSE).await;
    let mut socket = connect(&served, hello(&served.operator, "operation:G")).await;
    assert_eq!(receive(&mut socket).await["code"], "no_run");
}

/// A Run of `mission` with no Operation on its record, held by Station `worker`, and the
/// brief of the Mission naming an Operation when one is given.
fn operationless_run(served: &Served, mission: &str, brief_operation: Option<&str>) {
    served
        .registry
        .open_run(
            NewRun::new(format!("record-{mission}"), mission, "worker"),
            Duration::from_secs(600),
        )
        .unwrap();
    if let Some(operation) = brief_operation {
        let brief: ai6::Brief = serde_json::from_value(json!({
            "title": "work", "id": mission, "mission_id": mission, "operation": operation,
        }))
        .unwrap();
        served.registry.submit_brief(&brief).unwrap();
    }
}

fn mission_hello(token: &str, operation: &str, mission: &str) -> Value {
    json!({"type": "hello", "token": token,
           "mission": {"operation": operation, "mission": mission}})
}

#[tokio::test]
async fn a_missions_operation_is_the_registrys_and_not_the_hellos() {
    let served = served_charge(CONTROL).await;
    // The person controls G only. A Run whose record names no Operation, with a brief that names
    // L, is L's: naming G in the hello does not make it theirs.
    operationless_run(&served, "mission-l2", Some("L"));
    let mut wrong = connect(&served, mission_hello(&served.operator, "G", "mission-l2")).await;
    assert_eq!(receive(&mut wrong).await["code"], "no_run");
    // With nothing in the Registry that says whose it is, it is refused too.
    operationless_run(&served, "mission-nobody", None);
    let mut unknown = connect(
        &served,
        mission_hello(&served.operator, "G", "mission-nobody"),
    )
    .await;
    assert_eq!(receive(&mut unknown).await["code"], "no_run");
    assert!(poll(&served, 0).await.is_empty());
    assert!(audits(&served, "mission.attach").is_empty());
    // Where the brief agrees with the hello, and the person controls that Operation, it attaches
    // and the audit names the Operation the Registry gave.
    operationless_run(&served, "mission-g2", Some("G"));
    let mut socket = connect(&served, mission_hello(&served.operator, "G", "mission-g2")).await;
    assert_eq!(receive(&mut socket).await["type"], "attached");
    let audited = audits(&served, "mission.attach");
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].operation, "G");
}

/// Limits that check an attached person's sign-in every 200 ms.
fn quick_checks() -> ai6::UnexLimits {
    let mut limits = ai6::UnexLimits::default();
    limits.attach.recheck_every = ai6::Seconds(Duration::from_millis(200));
    limits
}

async fn token_for(served: &Served, ttl: Duration) -> (String, String) {
    let issued = crate::issue_access_token_record(
        std::sync::Arc::new(served.registry.clone()),
        "operator-007".to_owned(),
        ttl,
        ai6::TimeLimit::UNBOUNDED,
    )
    .await
    .ok()
    .unwrap();
    (issued.token, issued.token_id)
}

#[tokio::test]
async fn a_sign_in_that_ends_mid_attachment_ends_the_attachment() {
    let served =
        crate::controls::tests::served_with_unex(CONVERSE, None, Some(quick_checks())).await;
    served
        .registry
        .claim_charge("operation:G", "kg", "worker", Duration::from_secs(600))
        .unwrap();
    // A token that expires: the socket is closed when it does, not when the person next speaks.
    let (token, _) = token_for(&served, Duration::from_secs(1)).await;
    let (mut socket, _) = attach(&served, &token, "operation:G").await;
    let ended = receive(&mut socket).await;
    assert_eq!(ended["code"], "unauthorized", "{ended}");
    assert_eq!(receive(&mut socket).await["type"], "closed");

    // One revoked is refused at its next act.
    let (token, id) = token_for(&served, Duration::from_secs(600)).await;
    let (mut acting, _) = attach(&served, &token, "operation:G").await;
    served.registry.revoke_access_token(&id).unwrap();
    acting
        .send(WsMessage::Text(
            json!({"type": "prompt", "text": "still there?"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let refused = receive(&mut acting).await;
    assert_eq!(refused["code"], "unauthorized", "{refused}");
    assert_eq!(receive(&mut acting).await["type"], "closed");
    assert!(
        poll(&served, 0)
            .await
            .iter()
            .all(|command| !matches!(command.op, HubOp::Prompt { .. }))
    );

    // And one revoked while the person says nothing is closed at the next look.
    let (token, id) = token_for(&served, Duration::from_secs(600)).await;
    let (mut idle, _) = attach(&served, &token, "operation:G").await;
    served.registry.revoke_access_token(&id).unwrap();
    assert_eq!(receive(&mut idle).await["code"], "unauthorized");
    assert_eq!(receive(&mut idle).await["type"], "closed");
}

#[tokio::test]
async fn a_prompt_within_the_limit_fits_a_frame_however_it_is_escaped() {
    let mut limits = ai6::UnexLimits::default();
    limits.attach.prompt_bytes = ai6::Limit::at(1_024);
    limits.attach.frame_slack = ai6::Limit::at(100);
    let served = crate::controls::tests::served_with_unex(CONVERSE, None, Some(limits)).await;
    served
        .registry
        .claim_charge("operation:G", "kg", "worker", Duration::from_secs(600))
        .unwrap();
    let (mut socket, _) = attach(&served, &served.operator, "operation:G").await;
    // Every byte a control character: six bytes each once written as JSON.
    let heavy = "\u{1}".repeat(1_024);
    let sent = json!({"type": "prompt", "text": heavy}).to_string();
    assert!(sent.len() > 6 * 1_024, "{}", sent.len());
    socket.send(WsMessage::Text(sent.into())).await.unwrap();
    let commands = poll(&served, 5_000).await;
    assert!(matches!(&commands[0].op, HubOp::Prompt { text } if text.len() == 1_024));
    // A frame past what such a prompt takes is answered, and the socket stays open.
    socket
        .send(WsMessage::Text(
            json!({"type": "prompt", "text": "x".repeat(8_000)})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(receive(&mut socket).await["code"], "bad_request");
    socket
        .send(WsMessage::Text(
            json!({"type": "prompt", "text": "still open"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let commands = poll(&served, 5_000).await;
    assert!(matches!(&commands[0].op, HubOp::Prompt { text } if text == "still open"));
}
