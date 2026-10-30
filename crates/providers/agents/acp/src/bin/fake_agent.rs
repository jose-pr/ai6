//! A scripted ACP agent on standard input and output, for the handler's tests. The script is
//! in its environment: `FAKE_ECHO_ENV=NAME` makes its reply carry that variable's value,
//! `FAKE_ASK=KIND` has it ask leave to use a tool of that kind and report the answer,
//! `FAKE_UNASKED=KIND` runs such a tool without asking, `FAKE_COST=AMOUNT` reports a cost,
//! `FAKE_STOP=REASON` ends the turn with that stop reason, `FAKE_ERROR=1` fails the prompt,
//! `FAKE_NO_HTTP=1` leaves out HTTP MCP support, `FAKE_INIT_DELAY_MS=N` delays `initialize`,
//! `FAKE_RESUME=1` advertises `session/resume`.

use std::sync::Arc;

use agent_client_protocol::{
    Agent, Client, ConnectionTo, Responder, UntypedMessage, on_receive_request,
};
use ai6::directorates::section_00::acp::lines_over_io;
use serde_json::{Value, json};

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn update(cx: &ConnectionTo<Client>, update: Value) {
    if let Ok(message) = UntypedMessage::new(
        "session/update",
        json!({"sessionId": "fake-1", "update": update}),
    ) {
        let _ = cx.send_notification(message);
    }
}

fn handle(
    message: UntypedMessage,
    responder: Responder<Value>,
    cx: ConnectionTo<Client>,
    cwd: &Arc<std::sync::Mutex<String>>,
) {
    let params = message.params().clone();
    match message.method() {
        "initialize" => {
            let delay = var("FAKE_INIT_DELAY_MS")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(delay));
                let _ = responder.respond(json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {
                        "mcpCapabilities": {"http": var("FAKE_NO_HTTP").is_none()},
                        "sessionCapabilities": {
                            "resume": if var("FAKE_RESUME").is_some() { json!({}) } else { Value::Null },
                        },
                    },
                }));
            });
        }
        "session/new" | "session/resume" => {
            *cwd.lock().unwrap() = params["cwd"].as_str().unwrap_or_default().to_owned();
            let _ = responder.respond(json!({"sessionId": "fake-1"}));
        }
        "session/close" => {
            let _ = responder.respond(json!({}));
        }
        "session/prompt" => {
            let cwd = cwd.lock().unwrap().clone();
            std::thread::spawn(move || {
                let prompt = params["prompt"][0]["text"].as_str().unwrap_or_default();
                if var("FAKE_ERROR").is_some() {
                    let _ = responder.respond_with_error(
                        agent_client_protocol::Error::internal_error()
                            .data("the fake model is down"),
                    );
                    return;
                }
                let mut reply = format!("cwd:{cwd} prompt:{prompt}");
                if let Some(name) = var("FAKE_ECHO_ENV") {
                    reply.push_str(&format!(
                        " env:{}",
                        std::env::var(&name).unwrap_or_else(|_| "unset".to_owned())
                    ));
                }
                if let Some(kind) = var("FAKE_UNASKED") {
                    update(
                        &cx,
                        json!({"sessionUpdate": "tool_call", "toolCallId": "u1", "title": "Change a file",
                               "kind": kind, "status": "completed"}),
                    );
                }
                if let Some(kind) = var("FAKE_ASK") {
                    let asked = UntypedMessage::new(
                        "session/request_permission",
                        json!({
                            "sessionId": "fake-1",
                            "toolCall": {"toolCallId": "t1", "title": "Use a tool", "kind": kind,
                                         "status": "pending", "rawInput": {"command": "git push origin main"}},
                            "options": [
                                {"optionId": "yes", "name": "Allow", "kind": "allow_once"},
                                {"optionId": "no", "name": "Reject", "kind": "reject_once"},
                            ],
                        }),
                    )
                    .expect("a well-formed request");
                    let answer = futures::executor::block_on(cx.send_request(asked).block_task());
                    let chosen = answer
                        .ok()
                        .and_then(|value| value["outcome"]["optionId"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| "cancelled".to_owned());
                    reply.push_str(&format!(" asked:{chosen}"));
                }
                update(
                    &cx,
                    json!({"sessionUpdate": "agent_message_chunk",
                           "content": {"type": "text", "text": reply}}),
                );
                if let Some(amount) = var("FAKE_COST").and_then(|value| value.parse::<f64>().ok()) {
                    update(
                        &cx,
                        json!({"sessionUpdate": "usage_update", "used": 1, "size": 100,
                               "cost": {"amount": amount, "currency": "USD"}}),
                    );
                }
                let stop = var("FAKE_STOP").unwrap_or_else(|| "end_turn".to_owned());
                let _ = responder.respond(json!({"stopReason": stop}));
            });
        }
        _ => {
            let _ = responder.respond_with_error(agent_client_protocol::Error::method_not_found());
        }
    }
}

fn main() {
    let (lines, _) = lines_over_io(std::io::stdin(), std::io::stdout(), 0);
    let cwd = Arc::new(std::sync::Mutex::new(String::new()));
    let _ = futures::executor::block_on(
        Agent
            .builder()
            .on_receive_request(
                async move |message: UntypedMessage,
                            responder: Responder<Value>,
                            cx: ConnectionTo<Client>| {
                    handle(message, responder, cx, &cwd);
                    Ok(())
                },
                on_receive_request!(),
            )
            .connect_to(lines),
    );
}
