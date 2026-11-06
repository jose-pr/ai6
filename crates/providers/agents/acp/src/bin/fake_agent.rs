//! A scripted ACP agent on standard input and output, for the handler's tests. The script is
//! in its environment: `FAKE_ECHO_ENV=NAME` makes its reply carry that variable's value,
//! `FAKE_ASK=KIND` has it ask leave to use a tool of that kind and report the answer,
//! `FAKE_UNASKED=KIND` runs such a tool without asking, `FAKE_COST=AMOUNT` reports a cost,
//! `FAKE_STOP=REASON` ends the turn with that stop reason, `FAKE_ERROR=1` fails the prompt,
//! `FAKE_ERROR_DATA=JSON` fails it as an internal error carrying that `data`,
//! `FAKE_NO_HTTP=1` leaves out HTTP MCP support, `FAKE_INIT_DELAY_MS=N` delays `initialize`,
//! `FAKE_RESUME=1` advertises `session/resume`, `FAKE_DIE=1` exits mid-turn after writing
//! `FAKE_LAST_WORDS` (or its own) to standard error, and `FAKE_OPTIONS=1` says what a Session
//! can start with before any Session: a model option of two values.
//!
//! `FAKE_CABLES=JSON` has it use the `ai6-cables` MCP server its Session was given: it lists
//! the tools, then makes each call of the array (`{"tool", "args"}`), repeating one that has
//! `"until": TEXT` until the answers so far hold TEXT or `FAKE_CABLES_WAIT_MS` (30000) pass.
//! A call with `"reply_to": TEXT` is given the `cable_id` of a Cable read earlier whose body holds
//! TEXT. Its reply says `tools:NAME,...` and then `TOOL=>ANSWERS` for each call, on lines of their
//! own.

use std::sync::{Arc, Mutex};

use agent_client_protocol::{
    Agent, Client, ConnectionTo, Responder, UntypedMessage, on_receive_notification,
    on_receive_request,
};
use ai6::directorates::section_00::acp::UNBOUNDED_LINE_BYTES;
use coqui_acp::lines::over_io;
use serde_json::{Value, json};

/// The answer to a prompt `FAKE_HOLD` keeps until `session/cancel`, as a real agent's model turn,
/// and whether that has arrived already.
type Held = Arc<Mutex<(Option<Responder<Value>>, bool)>>;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// A client of one MCP server over streamable HTTP, as much of it as the fake needs.
struct Mcp {
    url: String,
    headers: Vec<(String, String)>,
    session: Option<String>,
    agent: ureq::Agent,
    next: u64,
}

impl Mcp {
    /// The server named `name` among a Session's `mcpServers`.
    fn of(servers: &Value, name: &str) -> Option<Self> {
        let server = servers
            .as_array()?
            .iter()
            .find(|server| server["name"] == name)?;
        let headers = server["headers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|header| {
                Some((
                    header["name"].as_str()?.to_owned(),
                    header["value"].as_str()?.to_owned(),
                ))
            })
            .collect();
        Some(Self {
            url: server["url"].as_str()?.to_owned(),
            headers,
            session: None,
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .into(),
            next: 1,
        })
    }

    /// Sends one JSON-RPC message; a request's answer, or `Null` for a notification.
    fn send(&mut self, method: &str, params: Value, notification: bool) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        let mut message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if !notification {
            message["id"] = json!(id);
        }
        let mut request = self
            .agent
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        if let Some(session) = &self.session {
            request = request
                .header("mcp-session-id", session)
                .header("mcp-protocol-version", "2025-06-18");
        }
        let mut response = request
            .send(message.to_string())
            .map_err(|error| format!("{method}: {error}"))?;
        if let Some(session) = response.headers().get("mcp-session-id") {
            self.session = session.to_str().ok().map(str::to_owned);
        }
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|error| format!("{method}: {error}"))?;
        if notification {
            return Ok(Value::Null);
        }
        if status != 200 {
            return Err(format!("{method}: HTTP {status}: {body}"));
        }
        let answer = body
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .chain(std::iter::once(body.as_str()))
            .filter_map(|text| serde_json::from_str::<Value>(text.trim()).ok())
            .find(|value| value["id"] == json!(id))
            .ok_or_else(|| format!("{method}: no answer in {body:?}"))?;
        match answer.get("error") {
            Some(error) => Err(format!("{method}: {error}")),
            None => Ok(answer["result"].clone()),
        }
    }

    fn connect(&mut self) -> Result<Vec<String>, String> {
        self.send(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "ai6-acp-fake-agent", "version": "0"}}),
            false,
        )?;
        self.send("notifications/initialized", json!({}), true)?;
        let listed = self.send("tools/list", json!({}), false)?;
        Ok(listed["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
            .collect())
    }

    /// The text a tool call answers, an error's included.
    fn call(&mut self, tool: &str, args: &Value) -> String {
        match self.send(
            "tools/call",
            json!({"name": tool, "arguments": args}),
            false,
        ) {
            Ok(result) => result["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join(" "),
            Err(error) => format!("ERROR {error}"),
        }
    }
}

/// What `FAKE_CABLES` asks of the Cable tools, in the reply's words.
fn use_cables(servers: &Value, script: &str) -> String {
    let Ok(Value::Array(steps)) = serde_json::from_str::<Value>(script) else {
        return "cables:unreadable script".to_owned();
    };
    let Some(mut mcp) = Mcp::of(servers, "ai6-cables") else {
        return "cables:no ai6-cables server".to_owned();
    };
    let tools = match mcp.connect() {
        Ok(tools) => tools,
        Err(error) => return format!("cables:{error}"),
    };
    let wait = std::time::Duration::from_millis(
        var("FAKE_CABLES_WAIT_MS")
            .and_then(|value| value.parse().ok())
            .unwrap_or(30_000),
    );
    let mut lines = vec![format!("tools:{}", tools.join(","))];
    // A read hands each Cable over once, so a later step may wait for what an earlier one read.
    let mut seen = String::new();
    for step in steps {
        let tool = step["tool"].as_str().unwrap_or_default();
        let mut args = step.get("args").cloned().unwrap_or_else(|| json!({}));
        if let Some(text) = step["reply_to"].as_str()
            && let Some(cable) = cable_read(&lines, text)
        {
            args["cable_id"] = json!(cable);
        }
        let deadline = std::time::Instant::now() + wait;
        let mut answers = vec![mcp.call(tool, &args)];
        seen.push_str(&answers[0]);
        if let Some(until) = step["until"].as_str() {
            while !seen.contains(until) && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let answer = mcp.call(tool, &args);
                seen.push_str(&answer);
                answers.push(answer);
            }
        }
        answers.retain(|answer| answer != "[]");
        lines.push(format!("{tool}=>{}", answers.join(" | ")));
    }
    lines.join("\n")
}

/// The `cable_id` of a Cable among the reply lines' reads whose body holds `text`.
fn cable_read(lines: &[String], text: &str) -> Option<String> {
    lines
        .iter()
        .filter_map(|line| line.strip_prefix("read_cables=>"))
        .flat_map(|answers| answers.split(" | "))
        .filter_map(|answer| serde_json::from_str::<Vec<Value>>(answer).ok())
        .flatten()
        .find(|cable| {
            cable["body"]
                .as_str()
                .is_some_and(|body| body.contains(text))
        })
        .and_then(|cable| cable["cable_id"].as_str().map(str::to_owned))
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
    held: &Held,
    servers: &Arc<Mutex<Value>>,
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
                        "_meta": {
                            "dev.boriken.coqui.acp/session_options": var("FAKE_OPTIONS").is_some(),
                        },
                    },
                }));
            });
        }
        "session/new" | "session/resume" => {
            *cwd.lock().unwrap() = params["cwd"].as_str().unwrap_or_default().to_owned();
            *servers.lock().unwrap() = params["mcpServers"].clone();
            let _ = responder.respond(json!({"sessionId": "fake-1"}));
        }
        "session/close" => {
            let _ = responder.respond(json!({}));
        }
        "_coqui_acp/session_options" if var("FAKE_OPTIONS").is_some() => {
            let _ = responder.respond(json!({
                "configOptions": [{
                    "id": "model",
                    "name": "Model",
                    "category": "model",
                    "type": "select",
                    "currentValue": "fast",
                    "options": [
                        {"value": "fast", "name": "Fast"},
                        {"value": "deep", "name": "Deep", "description": "Thinks longer"},
                    ],
                }],
                "unknown": {"modes": "the fake agent has no modes"},
            }));
        }
        "session/prompt" => {
            let cwd = cwd.lock().unwrap().clone();
            let held = held.clone();
            let servers = servers.lock().unwrap().clone();
            std::thread::spawn(move || {
                let prompt = params["prompt"][0]["text"].as_str().unwrap_or_default();
                if let Some(data) = var("FAKE_ERROR_DATA")
                    .and_then(|data| serde_json::from_str::<Value>(&data).ok())
                {
                    let _ = responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(data),
                    );
                    return;
                }
                if var("FAKE_ERROR").is_some() {
                    let _ = responder.respond_with_error(
                        agent_client_protocol::Error::internal_error()
                            .data("the fake model is down"),
                    );
                    return;
                }
                if var("FAKE_HOLD").is_some() {
                    let mut held = held.lock().unwrap();
                    if held.1 {
                        let _ = responder.respond(json!({"stopReason": "cancelled"}));
                    } else {
                        held.0 = Some(responder);
                    }
                    return;
                }
                if var("FAKE_DIE").is_some() {
                    eprintln!(
                        "{}",
                        var("FAKE_LAST_WORDS")
                            .unwrap_or_else(|| "the fake agent ran out of road".to_owned())
                    );
                    std::process::exit(3);
                }
                let mut reply = format!("cwd:{cwd} prompt:{prompt}");
                if let Some(name) = var("FAKE_ECHO_ENV") {
                    reply.push_str(&format!(
                        " env:{}",
                        std::env::var(&name).unwrap_or_else(|_| "unset".to_owned())
                    ));
                }
                if let Some(script) = var("FAKE_CABLES") {
                    reply.push('\n');
                    reply.push_str(&use_cables(&servers, &script));
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
                                         "status": "pending", "rawInput": var("FAKE_ASK_INPUT")
                                             .and_then(|input| serde_json::from_str::<Value>(&input).ok())
                                             .unwrap_or_else(|| json!({"command": "git push origin main"}))},
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
    let Ok((lines, _)) = over_io(std::io::stdin(), std::io::stdout(), 0, UNBOUNDED_LINE_BYTES)
    else {
        return;
    };
    let cwd = Arc::new(std::sync::Mutex::new(String::new()));
    let servers = Arc::new(Mutex::new(Value::Null));
    let held: Held = Arc::default();
    let for_prompts = held.clone();
    let _ = futures::executor::block_on(
        Agent
            .builder()
            .on_receive_request(
                async move |message: UntypedMessage,
                            responder: Responder<Value>,
                            cx: ConnectionTo<Client>| {
                    handle(message, responder, cx, &cwd, &for_prompts, &servers);
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_notification(
                async move |message: UntypedMessage, _cx: ConnectionTo<Client>| {
                    if message.method() == "session/cancel" {
                        let mut held = held.lock().unwrap();
                        held.1 = true;
                        if let Some(responder) = held.0.take() {
                            let _ = responder.respond(json!({"stopReason": "cancelled"}));
                        }
                    }
                    Ok(())
                },
                on_receive_notification!(),
            )
            .connect_to(lines),
    );
}
