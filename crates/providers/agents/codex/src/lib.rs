//! Codex app-server JSON-RPC implementation of AI6's AgentHandler contract.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

ai6::guatiao::providers!(CodexHandler);

mod tools;

pub use tools::{KEPT_FEATURES, TOOL_CHECK_ENV};

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ai6::directorates::m::identity::SessionId;
use ai6::directorates::q::terminal::{
    AgentTerminal, ExitInfo, JsonLines, READ_TIMED_OUT, STREAM_COLS, Spec, StreamLine,
    TerminalSession,
};
use ai6::directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentMessage, AgentResume, AgentSession, AgentStart, AssistantMode,
    LIMIT_EVENT, QuotaReport, TokenUsage, UsageLimit,
};
use ai6::directorates::section_00::gate::{GateClient, GateVerdict, gate};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use serde_json::{Value, json};

/// Codex app-server AgentHandler configuration.
#[derive(Debug, Clone, ToValue, FromValue, Schema)]
#[map(crate = ::ai6::guatiao)]
pub struct CodexConfig {
    /// Executable visible to the AI6-owned terminal environment.
    #[map(default = String::from("codex"))]
    pub executable: String,
}

/// Starts and resumes Codex through its app-server protocol.
#[derive(Provider)]
#[provider(
    AgentHandler,
    id = "codex",
    name = "Codex",
    crate = ::ai6::guatiao,
    config = CodexConfig
)]
pub struct CodexHandler {
    config: CodexConfig,
}

impl TryFrom<CodexConfig> for CodexHandler {
    type Error = ProviderError;

    fn try_from(config: CodexConfig) -> Result<Self, Self::Error> {
        if config.executable.trim().is_empty() {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "Codex executable is empty",
            ));
        }
        Ok(Self { config })
    }
}

impl AgentHandler for CodexHandler {
    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn ai6::directorates::section_00::agent::AgentSession>, ProviderError> {
        let gate = station_gate(&request.environment);
        self.start_gated(request, terminal, gate)
    }

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn ai6::directorates::section_00::agent::AgentSession>, ProviderError> {
        let (process, weakening) = spawn(
            terminal,
            &self.config.executable,
            &request.working_directory,
            &request.environment,
            Mode {
                project_settings: request.project_settings,
                answer_only: request.answer_only || request.assistant.is_some(),
                assistant: request.assistant.as_ref(),
                model: request.model.as_deref(),
            },
        )?;
        let gate = station_gate(&request.environment)
            .filter(|_| !(request.answer_only || request.assistant.is_some()));
        let mut session = CodexSession::new(process, request.effort)
            .with_gate(gate)
            .with_weakening(weakening)
            .restricted_to(restriction(request.answer_only, request.assistant.as_ref()));
        session.request(
            "initialize",
            json!({
                "clientInfo": { "name": "ai6", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": {}
            }),
            Duration::from_secs(20),
        )?;
        session.notify("initialized", json!({}))?;
        let mut params = json!({ "threadId": request.session_id.as_str(), "excludeTurns": true });
        if let Some(model) = request.model {
            params["model"] = json!(model);
        }
        params["cwd"] = json!(request.working_directory);
        set_approval_policy(
            &mut params,
            &request.environment,
            request.answer_only || request.assistant.is_some(),
        );
        let response = match session.request("thread/resume", params, Duration::from_secs(30)) {
            Ok(response) => response,
            Err(error) => {
                let _ = session.close();
                return Err(error);
            }
        };
        let Some(resumed_thread_id) = response.pointer("/thread/id").and_then(Value::as_str) else {
            let _ = session.close();
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "Codex omitted resumed thread ID",
            ));
        };
        if resumed_thread_id != request.session_id.as_str() {
            let _ = session.close();
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "Codex resumed a different thread than requested",
            ));
        }
        session.thread_id = Some(request.session_id);
        if let Some(message) = request.message
            && let Err(error) = session.send_message(AgentMessage { text: message })
        {
            let _ = session.close();
            return Err(error);
        }
        Ok(session.into_object())
    }
}

impl CodexHandler {
    /// Starts a thread whose command approvals `gate` answers.
    fn start_gated(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
        gate: Option<Gate>,
    ) -> Result<Object<dyn ai6::directorates::section_00::agent::AgentSession>, ProviderError> {
        let (process, weakening) = spawn(
            terminal,
            &self.config.executable,
            &request.working_directory,
            &request.environment,
            Mode {
                project_settings: request.project_settings,
                answer_only: request.answer_only || request.assistant.is_some(),
                assistant: request.assistant.as_ref(),
                model: request.model.as_deref(),
            },
        )?;
        let gate = gate.filter(|_| !(request.answer_only || request.assistant.is_some()));
        let mut session = CodexSession::new(process, request.effort)
            .with_gate(gate)
            .with_weakening(weakening)
            .restricted_to(restriction(request.answer_only, request.assistant.as_ref()));
        session.request(
            "initialize",
            json!({
                "clientInfo": { "name": "ai6", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": {}
            }),
            Duration::from_secs(20),
        )?;
        session.notify("initialized", json!({}))?;
        let mut params = json!({ "cwd": request.working_directory });
        set_approval_policy(
            &mut params,
            &request.environment,
            request.answer_only || request.assistant.is_some(),
        );
        if let Some(model) = request.model {
            params["model"] = json!(model);
        }
        if !request.initial_context.is_empty() {
            params["developerInstructions"] = json!(request.initial_context.join("\n\n"));
        }
        let response = session.request("thread/start", params, Duration::from_secs(30))?;
        let thread_id = response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| refuse(Status::GUATIAO_ERR_BAD_VALUE, "Codex omitted thread ID"))?
            .to_string();
        session.thread_id = Some(
            SessionId::try_from(thread_id)
                .map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error))?,
        );
        session.send_message(AgentMessage {
            text: request.prompt,
        })?;
        Ok(session.into_object())
    }
}

/// The MCP servers a restricted (answer-only or assistant) thread may call, `None` for a thread
/// that is not restricted.
fn restriction(answer_only: bool, assistant: Option<&AssistantMode>) -> Option<Vec<String>> {
    match assistant {
        Some(mode) => Some(
            mode.mcp_servers
                .iter()
                .map(|server| server.name.clone())
                .collect(),
        ),
        None => answer_only.then(Vec::new),
    }
}

/// Decides whether a command Codex asks to run may go ahead, given its working directory.
type Gate = Arc<dyn Fn(&str, Option<&Path>) -> GateVerdict + Send + Sync>;

/// The green ink gate asking the Session's Station through its Cable endpoint. A Session with
/// no endpoint cannot ask for green ink at all, so it has no gate, as with Claude Code.
fn station_gate(environment: &std::collections::BTreeMap<String, String>) -> Option<Gate> {
    let client = GateClient::from_environment(environment)?;
    Some(Arc::new(move |command, cwd| {
        gate(command, cwd, Some(&client))
    }))
}

/// When the thread asks before running a command. Answer-only, or in remote-hands mode, it
/// never asks (its sandbox is read-only, and in remote-hands mode the Station gates
/// `theater_run`); with a green ink gate it asks (`untrusted`) for every command outside
/// Codex's own list of safe reads, so the gate sees each one.
fn set_approval_policy(
    params: &mut Value,
    environment: &std::collections::BTreeMap<String, String>,
    answer_only: bool,
) {
    if answer_only {
        restrict_to_answer_only(params);
    } else if is_remote_hands(environment) {
        restrict_to_remote_hands(params);
    } else if GateClient::from_environment(environment).is_some() {
        params["approvalPolicy"] = json!("untrusted");
    }
}

struct CodexSession {
    terminal: Object<dyn TerminalSession>,
    gate: Option<Gate>,
    output: Mutex<JsonLines>,
    events: Mutex<VecDeque<AgentEvent>>,
    thread_id: Option<SessionId>,
    request_id: AtomicU64,
    effort: Option<String>,
    /// The latest quota window the account reported as used up.
    exhausted: Mutex<Option<UsageLimit>>,
    /// For a restricted (answer-only or assistant) thread, the MCP servers it may call.
    restricted: Option<Vec<String>>,
    /// Set once a restricted thread used a tool it may not, and was ended.
    tripped: AtomicBool,
    /// The command of each command execution item the thread started, by item id.
    started: Mutex<std::collections::HashMap<String, String>>,
    /// Set once the app server's output has ended.
    ended: AtomicBool,
    /// Set once that end was reported as a `failed` event.
    ended_reported: AtomicBool,
    ended_reason: Mutex<Option<String>>,
    /// The last lines the app server wrote that were not JSON: its own diagnostics, which say
    /// why it exited when it exits.
    tail: Mutex<VecDeque<String>>,
}

/// How many of the last non-JSON lines a `failed` event quotes, and how long each may be.
const TAIL_LINES: usize = 8;
const TAIL_LINE_BYTES: usize = 512;
/// How long `close` waits for the app server to have exited, for a terminal whose `kill` does
/// not.
const CLOSE_WAIT_MS: i64 = 2_000;
/// How long the end of the output waits for the app server's exit status.
const EXIT_WAIT_MS: i64 = 1_000;

impl CodexSession {
    fn new(terminal: Object<dyn TerminalSession>, effort: Option<String>) -> Self {
        Self {
            terminal,
            gate: None,
            output: Mutex::new(JsonLines::new()),
            events: Mutex::new(VecDeque::new()),
            thread_id: None,
            request_id: AtomicU64::new(0),
            effort,
            exhausted: Mutex::new(None),
            restricted: None,
            tripped: AtomicBool::new(false),
            started: Mutex::default(),
            ended: AtomicBool::new(false),
            ended_reported: AtomicBool::new(false),
            ended_reason: Mutex::new(None),
            tail: Mutex::new(VecDeque::new()),
        }
    }

    /// How the app server ended, once its terminal says it has; `None` while it runs or when
    /// the terminal cannot tell.
    fn exited(&self, timeout_ms: i64) -> Option<ExitInfo> {
        let status = self.terminal.exit_status(timeout_ms).ok()?;
        ExitInfo::from_value(&status)
            .ok()
            .filter(|exit| !exit.running)
    }

    /// Why the app server's output ended: its exit status and the last lines it wrote outside
    /// the protocol. Worked out once.
    fn exit_reason(&self) -> String {
        if let Ok(mut reason) = self.ended_reason.lock() {
            return reason.get_or_insert_with(|| self.describe_exit()).clone();
        }
        self.describe_exit()
    }

    fn describe_exit(&self) -> String {
        let how = match self.exited(EXIT_WAIT_MS) {
            Some(ExitInfo {
                code: Some(code), ..
            }) => format!("exited with code {code}"),
            Some(ExitInfo {
                signal: Some(signal),
                ..
            }) => format!("was ended by signal {signal}"),
            Some(_) => "exited".to_owned(),
            None => "closed its terminal".to_owned(),
        };
        let mut reason = format!("Codex app-server {how} before completing the Session");
        if let Ok(tail) = self.tail.lock()
            && !tail.is_empty()
        {
            reason.push_str("; its last output:\n");
            reason.push_str(&tail.iter().cloned().collect::<Vec<_>>().join("\n"));
        }
        reason
    }

    fn remember_output(&self, line: &str) {
        if let Ok(mut tail) = self.tail.lock() {
            if tail.len() == TAIL_LINES {
                tail.pop_front();
            }
            let mut end = line.len().min(TAIL_LINE_BYTES);
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            tail.push_back(line[..end].to_owned());
        }
    }

    /// Restricts the thread to the tools of `servers`: any other tool it uses ends it.
    fn restricted_to(mut self, servers: Option<Vec<String>>) -> Self {
        self.restricted = servers;
        self
    }

    /// Ends a restricted thread that used `what`: the tool check confirmed before the start
    /// what Codex said it would offer, and this is the backstop for what it offered anyway.
    fn trip(&self, what: &str) -> Result<(), ProviderError> {
        if self.tripped.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let reason = format!(
            "the restricted Codex thread used {what}, which it may not use; the Session was ended"
        );
        eprintln!("ai6: {reason}");
        let _ = self.terminal.kill();
        lock(&self.events)?.push_back(AgentEvent {
            kind: "failed".to_owned(),
            text: Some(reason),
            data: None,
            limit: None,
            usage: None,
            tools: Vec::new(),
            quota: Vec::new(),
        });
        Ok(())
    }

    fn with_gate(mut self, gate: Option<Gate>) -> Self {
        self.gate = gate;
        self
    }

    /// Reports what a `warn` tool check found, on the Station's log and as the Session's
    /// first event (kind `weakened`).
    fn with_weakening(self, weakening: Option<String>) -> Self {
        if let Some(text) = weakening {
            eprintln!("ai6: weakening: {text}");
            if let Ok(mut events) = self.events.lock() {
                events.push_back(AgentEvent {
                    kind: "weakened".to_owned(),
                    text: Some(text),
                    data: None,
                    limit: None,
                    usage: None,
                    tools: Vec::new(),
                    quota: Vec::new(),
                });
            }
        }
        self
    }

    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ProviderError> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.write_line(&json!({ "id": id, "method": method, "params": params }))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "Codex app-server timed out",
                ));
            }
            // Anything but JSON is the harness's own diagnostics, which a reply never is.
            let Some(StreamLine::Json {
                value: message,
                rebuilt,
                ..
            }) = self.read_line(remaining)?
            else {
                continue;
            };
            let ours = message.get("id").and_then(Value::as_u64) == Some(id);
            // ConPTY echoes the request itself when the app server reads its input as lines.
            if ours && message.get("method").and_then(Value::as_str) == Some(method) {
                continue;
            }
            // A server request carries its own id, which may equal ours; it has a method.
            if ours && message.get("method").is_none() {
                if let Some(error) = message.get("error") {
                    return Err(refuse(Status::GUATIAO_ERR_INTERNAL, error));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            if message.get("method").is_none() {
                continue;
            }
            self.collect_event(message, rebuilt)?;
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), ProviderError> {
        self.write_line(&json!({ "method": method, "params": params }))
    }

    fn write_line(&self, value: &Value) -> Result<(), ProviderError> {
        let mut line = serde_json::to_vec(value).map_err(internal)?;
        line.push(b'\n');
        self.terminal.write(&line)
    }

    /// The next line the app server wrote, `None` when `timeout` passes first. Once its output
    /// has ended (the terminal closed, or the app server exited while the terminal kept it
    /// open) and every line is read, an error saying how it ended.
    fn read_line(&self, timeout: Duration) -> Result<Option<StreamLine>, ProviderError> {
        let line = self.next_line(timeout)?;
        if let Some(StreamLine::Text(text)) = &line {
            self.remember_output(text);
        }
        Ok(line)
    }

    fn next_line(&self, timeout: Duration) -> Result<Option<StreamLine>, ProviderError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(line) = lock(&self.output)?.next_line() {
                return Ok(Some(line));
            }
            if self.ended.load(Ordering::Acquire) {
                if let Some(line) = lock(&self.output)?.finish() {
                    return Ok(Some(line));
                }
                return Err(refuse(Status::GUATIAO_ERR_INTERNAL, self.exit_reason()));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let mut bytes = [0_u8; 8192];
            let count = self.terminal.read(
                &mut bytes,
                remaining.as_millis().min(i64::MAX as u128) as i64,
            )?;
            if count == READ_TIMED_OUT {
                // A terminal may keep its output open after the app server exits.
                if self.exited(0).is_some() {
                    self.ended.store(true, Ordering::Release);
                    continue;
                }
                return Ok(None);
            }
            if count == 0 {
                self.ended.store(true, Ordering::Release);
                continue;
            }
            if count < 0 {
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "Codex terminal returned an invalid read count",
                ));
            }
            lock(&self.output)?.push(&bytes[..count as usize]);
        }
    }

    /// Takes one message from the app server; `rebuilt` says its line was not read as written
    /// ([`StreamLine::Json`]).
    fn collect_event(&self, message: Value, rebuilt: bool) -> Result<(), ProviderError> {
        if self.tripped.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(servers) = &self.restricted
            && let Some(what) = forbidden_use(&message, servers)
        {
            return self.trip(&what);
        }
        if let Some(id) = message.get("id")
            && message.get("method").and_then(Value::as_str) == Some(COMMAND_APPROVAL)
        {
            self.answer_command_approval(
                id,
                message.get("params").unwrap_or(&Value::Null),
                rebuilt,
            )?;
        }
        if message.get("method").and_then(Value::as_str) == Some("item/started")
            && let Some(item) = message.pointer("/params/item")
            && item.get("type").and_then(Value::as_str) == Some("commandExecution")
            && let (Some(id), Some(command)) = (
                item.get("id").and_then(Value::as_str),
                item.get("command").and_then(Value::as_str),
            )
        {
            lock(&self.started)?.insert(id.to_owned(), command.to_owned());
        }
        if let Some(exhausted) = exhausted_window(&message) {
            *lock(&self.exhausted)? = Some(exhausted);
        }
        let hint = lock(&self.exhausted)?.clone();
        if let Some(event) = decode_event(&message, hint.as_ref()) {
            lock(&self.events)?.push_back(event);
        }
        Ok(())
    }
}

/// The app-server's request to approve a command before it runs.
const COMMAND_APPROVAL: &str = "item/commandExecution/requestApproval";

/// Thread items a restricted thread may produce besides calls to its own MCP servers: what it
/// is told and says, its reasoning and plan, and the thread's own bookkeeping.
const RESTRICTED_ITEMS: [&str; 7] = [
    "userMessage",
    "agentMessage",
    "reasoning",
    "plan",
    "contextCompaction",
    "enteredReviewMode",
    "exitedReviewMode",
];

/// What `message` shows a restricted thread doing that it may not, for a thread whose only
/// tools are those of `servers`: any request from the server but token refresh and
/// attestation (approvals, tool calls, questions, elicitation), or an `item/started` or
/// `item/completed` of any other kind than [`RESTRICTED_ITEMS`] or an MCP call to one of
/// `servers`.
fn forbidden_use(message: &Value, servers: &[String]) -> Option<String> {
    let method = message.get("method")?.as_str()?;
    if message.get("id").is_some() {
        return (!matches!(
            method,
            "account/chatgptAuthTokens/refresh" | "attestation/generate"
        ))
        .then(|| format!("a {method:?} request"));
    }
    if !matches!(method, "item/started" | "item/completed") {
        return None;
    }
    let item = message.pointer("/params/item");
    let field = |name: &str| {
        item.and_then(|item| item.get(name))
            .and_then(Value::as_str)
            .unwrap_or("")
    };
    match field("type") {
        kind if RESTRICTED_ITEMS.contains(&kind) => None,
        "mcpToolCall" if servers.iter().any(|server| server == field("server")) => None,
        "mcpToolCall" => Some(format!(
            "the tool {:?} of MCP server {:?}",
            field("tool"),
            field("server")
        )),
        kind => Some(format!("a {kind:?} item")),
    }
}

impl CodexSession {
    /// Answers a command approval with the green ink gate: `accept` unless the gate holds the
    /// command, then `decline`, and the reason is steered into the running turn so the Agent
    /// knows to ask for green ink. With a gate, these are declined without asking it: input to
    /// a running command and a request that names no command (the gate cannot see what they
    /// run), a request whose line was rebuilt from a terminal's rendering (`rebuilt`: its
    /// command may not be what Codex wrote), and one with no command execution item started
    /// earlier under its item id with the same command. A decline the gate did not decide is
    /// also logged and queued as an event of kind `declined`. Without a gate (no Cable
    /// endpoint) everything is accepted.
    /// A hold the gate did not decide: logged, and queued as a `declined` event.
    fn declined(&self, reason: &str) -> Result<GateVerdict, ProviderError> {
        eprintln!("ai6: {reason}");
        lock(&self.events)?.push_back(AgentEvent {
            kind: "declined".to_owned(),
            text: Some(reason.to_owned()),
            data: None,
            limit: None,
            usage: None,
            tools: Vec::new(),
            quota: Vec::new(),
        });
        Ok(GateVerdict::Hold(reason.to_owned()))
    }

    fn answer_command_approval(
        &self,
        id: &Value,
        params: &Value,
        rebuilt: bool,
    ) -> Result<(), ProviderError> {
        let command = params.get("command").and_then(Value::as_str);
        let cwd = params.get("cwd").and_then(Value::as_str).map(Path::new);
        let item = params.get("itemId").and_then(Value::as_str);
        let started = match item {
            Some(item) => lock(&self.started)?.get(item).cloned(),
            None => None,
        };
        let unseen = |command: &str| started.as_deref() != Some(command);
        let verdict = match (
            &self.gate,
            params.get("kind").and_then(Value::as_str),
            command,
        ) {
            (None, _, _) => GateVerdict::Allow,
            (Some(_), _, _) if rebuilt => self.declined(
                "Green ink: this approval request reached AI6 rebuilt from a terminal's \
                 rendering (a line it broke, or a control inside it), so its command may not be \
                 what was asked; it is declined. Run the command again.",
            )?,
            (Some(_), kind, Some(command)) if kind != Some("writeStdin") && unseen(command) => self
                .declined(
                    "Green ink: this approval request names a command no command execution \
                     item started with before it; it is declined. Run the command again.",
                )?,
            (Some(_), Some("writeStdin"), _) => GateVerdict::Hold(
                "Green ink: input to a running command is declined, because the gate cannot see \
                 what it runs; run the command with all its arguments instead."
                    .to_owned(),
            ),
            (Some(_), _, None) => GateVerdict::Hold(
                "Green ink: an approval request that names no command is declined; run the \
                 command with all its arguments instead."
                    .to_owned(),
            ),
            (Some(gate), _, Some(command)) => gate(command, cwd),
        };
        let decision = match verdict {
            GateVerdict::Allow => "accept",
            GateVerdict::Hold(_) => "decline",
        };
        self.write_line(&json!({ "id": id, "result": { "decision": decision } }))?;
        if let GateVerdict::Hold(reason) = verdict
            && let (Some(thread), Some(turn)) = (
                params.get("threadId").and_then(Value::as_str),
                params.get("turnId").and_then(Value::as_str),
            )
        {
            self.write_line(&json!({
                "id": self.request_id.fetch_add(1, Ordering::Relaxed) + 1,
                "method": "turn/steer",
                "params": {
                    "threadId": thread,
                    "expectedTurnId": turn,
                    "input": [{ "type": "text", "text": reason }]
                }
            }))?;
        }
        Ok(())
    }
}

/// The name of a Codex quota window by its length, else its position.
fn window_name(window: &Value, position: &str) -> String {
    match window.get("windowDurationMins").and_then(Value::as_i64) {
        Some(300) => "five_hour".to_owned(),
        Some(10_080) => "weekly".to_owned(),
        _ => position.to_owned(),
    }
}

/// Every quota window an `account/rateLimits/updated` notification reports: `usedPercent`
/// as thousandths, `resetsAt` in Unix seconds.
fn decode_quota(message: &Value) -> Vec<QuotaReport> {
    if message.get("method").and_then(Value::as_str) != Some("account/rateLimits/updated") {
        return Vec::new();
    }
    let Some(limits) = message.pointer("/params/rateLimits") else {
        return Vec::new();
    };
    ["primary", "secondary"]
        .iter()
        .filter_map(|position| {
            let window = limits.get(position).filter(|window| window.is_object())?;
            Some(QuotaReport {
                window: window_name(window, position),
                used_permille: window
                    .get("usedPercent")
                    .and_then(Value::as_f64)
                    .map(|percent| (percent * 10.0).round().clamp(0.0, 1000.0) as u16),
                resets_at: window.get("resetsAt").and_then(Value::as_i64),
                status: None,
            })
        })
        .collect()
}

/// The quota window an `account/rateLimits/updated` notification reports as used up.
///
/// The usage-limit error that follows does not always repeat the reset time, so the session
/// keeps the latest exhausted window as a hint.
fn exhausted_window(message: &Value) -> Option<UsageLimit> {
    if message.get("method")?.as_str()? != "account/rateLimits/updated" {
        return None;
    }
    let limits = message.pointer("/params/rateLimits")?;
    ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let window = limits.get(key)?;
            (window.get("usedPercent")?.as_f64()? >= 100.0).then(|| UsageLimit {
                resets_at: window.get("resetsAt").and_then(Value::as_i64),
                window: match window.get("windowDurationMins").and_then(Value::as_i64) {
                    Some(300 | 10_080) => Some(window_name(window, "")),
                    _ => None,
                },
            })
        })
        // Work resumes only when every exhausted window has reset.
        .max_by_key(|limit| limit.resets_at)
}

/// Recognises the usage-limit error Codex reports on a failed turn.
///
/// The error object (`params.error`, or `params.turn.error` on `turn/failed` and a failed
/// `turn/completed`) names the limit in `codexErrorInfo`; its reset time is `resetsAt` in
/// Unix seconds when present, else the exhausted window last reported by the account.
fn decode_limit(
    message: &Value,
    hint: Option<&UsageLimit>,
) -> Option<(UsageLimit, Option<String>)> {
    let params = message.get("params")?;
    let error = match message.get("method")?.as_str()? {
        "error" => params.get("error")?,
        "turn/failed" => params
            .get("error")
            .or_else(|| params.pointer("/turn/error"))?,
        "turn/completed"
            if params.pointer("/turn/status").and_then(Value::as_str) == Some("failed") =>
        {
            params.pointer("/turn/error")?
        }
        _ => return None,
    };
    let info = error
        .get("codexErrorInfo")
        .and_then(Value::as_str)?
        .to_lowercase()
        .replace('_', "");
    if !(info.contains("usagelimit") || info.contains("ratelimit")) {
        return None;
    }
    let resets_at = error
        .get("resetsAt")
        .and_then(Value::as_i64)
        .or_else(|| hint.and_then(|hint| hint.resets_at));
    let window = hint.and_then(|hint| hint.window.clone());
    let text = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some((UsageLimit { resets_at, window }, text))
}

/// The tokens one model request used, from a `thread/tokenUsage/updated` notification.
///
/// The notification also carries the thread's running total, which survives a resume, so
/// only `last` (the latest request) is read and reported as an addition. Codex reports no
/// cost. Its `inputTokens` include the cached ones, which are split out here.
fn decode_usage(message: &Value) -> Option<TokenUsage> {
    if message.get("method")?.as_str()? != "thread/tokenUsage/updated" {
        return None;
    }
    let last = message.pointer("/params/tokenUsage/last")?;
    let count = |key: &str| last.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = count("cachedInputTokens");
    Some(TokenUsage {
        input: count("inputTokens").saturating_sub(cached),
        output: count("outputTokens"),
        cache_read: cached,
        cache_write: 0,
        cost_micro_usd: 0,
        cumulative: false,
    })
}

/// The tool an `item/started` notification begins: a web search, or an MCP or dynamic
/// tool call, named the way Claude Code names it (`WebSearch`, or the tool's own name).
fn decode_tools(message: &Value) -> Vec<String> {
    if message.get("method").and_then(Value::as_str) != Some("item/started") {
        return Vec::new();
    }
    let item = message.pointer("/params/item");
    let name = match item
        .and_then(|item| item.get("type"))
        .and_then(Value::as_str)
    {
        Some("webSearch") => Some("WebSearch"),
        Some("mcpToolCall" | "dynamicToolCall") => item
            .and_then(|item| item.get("tool"))
            .and_then(Value::as_str),
        _ => None,
    };
    name.map(str::to_owned).into_iter().collect()
}

fn decode_event(message: &Value, hint: Option<&UsageLimit>) -> Option<AgentEvent> {
    let method = message.get("method")?.as_str().unwrap_or("event");
    if let Some((limit, text)) = decode_limit(message, hint) {
        return Some(AgentEvent {
            kind: LIMIT_EVENT.to_owned(),
            text,
            data: serde_json::to_vec(message).ok(),
            limit: Some(limit),
            usage: None,
            tools: Vec::new(),
            quota: Vec::new(),
        });
    }
    let kind = match method {
        "item/agentMessage/delta" => "text",
        "turn/completed" => "completed",
        "turn/failed" => "failed",
        _ => method,
    };
    let text = message
        .pointer("/params/delta")
        .and_then(Value::as_str)
        .or_else(|| message.pointer("/params/item/text").and_then(Value::as_str))
        .map(str::to_string);
    Some(AgentEvent {
        kind: kind.to_string(),
        text,
        data: serde_json::to_vec(message).ok(),
        limit: None,
        usage: decode_usage(message),
        tools: decode_tools(message),
        quota: decode_quota(message),
    })
}

impl AgentSession for CodexSession {
    fn session_id(&self) -> Result<Option<SessionId>, ProviderError> {
        Ok(self.thread_id.clone())
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        let thread_id = self
            .thread_id
            .as_ref()
            .ok_or_else(|| refuse(Status::GUATIAO_ERR_INTERNAL, "Codex thread is not started"))?;
        let mut params = json!({
            "threadId": thread_id.as_str(),
            "input": [{ "type": "text", "text": message.text }]
        });
        if let Some(effort) = &self.effort {
            params["effort"] = json!(effort);
        }
        self.write_line(&json!({
            "id": self.request_id.fetch_add(1, Ordering::Relaxed) + 1,
            "method": "turn/start",
            "params": params
        }))
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        if let Some(event) = lock(&self.events)?.pop_front() {
            return Ok(Some(event));
        }
        let read = self.read_line(Duration::from_millis(timeout_ms.max(0) as u64));
        // The app server's exit ends the Session once, with the reason.
        let read = match read {
            Err(_) if self.ended.load(Ordering::Acquire) => {
                if self.ended_reported.swap(true, Ordering::AcqRel) {
                    return Ok(None);
                }
                let reason = self.exit_reason();
                eprintln!("ai6: {reason}");
                return Ok(Some(AgentEvent {
                    kind: "failed".to_owned(),
                    text: Some(reason),
                    data: None,
                    limit: None,
                    usage: None,
                    tools: Vec::new(),
                    quota: Vec::new(),
                }));
            }
            read => read?,
        };
        let (message, rebuilt) = match read {
            None => return Ok(None),
            Some(StreamLine::Json { value, rebuilt, .. }) => (value, rebuilt),
            Some(StreamLine::Text(text)) => {
                return Ok(Some(AgentEvent {
                    kind: "output".to_owned(),
                    text: Some(text),
                    data: None,
                    limit: None,
                    usage: None,
                    tools: Vec::new(),
                    quota: Vec::new(),
                }));
            }
        };
        if message.get("method").is_none() {
            return Ok(None);
        }
        self.collect_event(message, rebuilt)?;
        Ok(lock(&self.events)?.pop_front())
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        let thread_id = self
            .thread_id
            .as_ref()
            .ok_or_else(|| refuse(Status::GUATIAO_ERR_INTERNAL, "Codex thread is not started"))?;
        self.write_line(&json!({
            "id": self.request_id.fetch_add(1, Ordering::Relaxed) + 1,
            "method": "turn/interrupt",
            "params": { "threadId": thread_id.as_str() }
        }))
    }

    fn close(&self) -> Result<(), ProviderError> {
        self.terminal.kill()?;
        // The caller may remove the working directory next, which the app server holds until
        // it has exited; a terminal that cannot tell is not waited for.
        let _ = self.terminal.exit_status(CLOSE_WAIT_MS);
        Ok(())
    }
}

/// What AI6 decided about the thread's settings.
#[derive(Clone, Copy)]
struct Mode<'a> {
    project_settings: bool,
    /// Set for answer-only and assistant threads alike: both run read-only and never ask.
    answer_only: bool,
    assistant: Option<&'a AssistantMode>,
    /// The model the thread asks for; what Codex offers it depends on the model.
    model: Option<&'a str>,
}

/// Starts the app server, and returns what a `warn` tool check found. A restricted (answer-only
/// or assistant) app server starts only after its tool check ([`tools`]).
fn spawn(
    terminal: Object<dyn AgentTerminal>,
    executable: &str,
    cwd: &str,
    environment: &std::collections::BTreeMap<String, String>,
    mode: Mode<'_>,
) -> Result<(Object<dyn TerminalSession>, Option<String>), ProviderError> {
    let mut argv = if let Some(assistant) = mode.assistant {
        assistant_argv(executable, environment, assistant)?
    } else if mode.answer_only {
        answer_only_argv(executable)
    } else {
        app_server_argv(executable, environment)
    };
    if !mode.project_settings || mode.answer_only {
        argv.extend(untrusted_project(cwd));
    }
    let mut weakening = None;
    if mode.answer_only {
        let servers = mode
            .assistant
            .map(|assistant| {
                assistant
                    .mcp_servers
                    .iter()
                    .map(|server| server.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let checked = tools::Probe {
            terminal: &terminal,
            executable,
            overrides: &argv[2..],
            cwd,
            environment,
            model: mode.model,
            servers: &servers,
        }
        .check()?;
        argv.extend(checked.overrides);
        weakening = checked.weakening;
    }
    let spec = Spec {
        argv,
        cwd: Some(cwd.to_string()),
        env: Some(environment.clone()),
        rows: 24,
        cols: STREAM_COLS,
    };
    let value = spec.to_value().map_err(internal)?;
    Ok((terminal.spawn(&value)?, weakening))
}

fn app_server_argv(
    executable: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    let mut argv = vec![executable.to_string(), "app-server".to_string()];
    for (server, prefix) in [
        ("ai6-cables", "AI6_CABLE_MCP"),
        ("ai6-theater", "AI6_THEATER_MCP"),
    ] {
        if let Some(url) = environment.get(&format!("{prefix}_URL"))
            && environment.contains_key(&format!("{prefix}_TOKEN"))
        {
            argv.extend([
                "-c".to_owned(),
                format!("mcp_servers.{server}.url=\"{url}\""),
                "-c".to_owned(),
                format!("mcp_servers.{server}.bearer_token_env_var=\"{prefix}_TOKEN\""),
            ]);
        }
    }
    argv
}

/// The settings that leave an answer-only or assistant thread no built-in tool that reads,
/// runs or reaches anything: no command execution (`features.shell_tool`; Codex ignores a
/// user's `features.unified_exec = false`), no web search, no sub-agents
/// (`features.multi_agent`, `agents.enabled`), no app connectors, a skills catalog too small to
/// list one (`skills.max_context_tokens`, which takes no 0), and no questions to the user. Names
/// and meanings from the Codex configuration reference
/// (learn.chatgpt.com/docs/config-file/config-reference), but for
/// `tools.experimental_request_user_input`: switched off, it removes `request_user_input` and
/// `request_user_input_async`, which no feature does (Codex 0.164.0). Every other
/// feature Codex enables, and every MCP server of the user's, is switched off by the tool check
/// ([`tools`]), and the read-only sandbox and approval policy `never` stay as a further line.
const NO_BUILT_IN_TOOLS: [&str; 7] = [
    "features.shell_tool=false",
    "web_search=\"disabled\"",
    "features.multi_agent=false",
    "agents.enabled=false",
    "features.apps=false",
    "skills.max_context_tokens=1",
    "tools.experimental_request_user_input={enabled=false}",
];

/// `-c` overrides for [`NO_BUILT_IN_TOOLS`].
fn no_built_in_tools() -> Vec<String> {
    NO_BUILT_IN_TOOLS
        .iter()
        .flat_map(|setting| ["-c".to_owned(), (*setting).to_owned()])
        .collect()
}

/// The app server of an answer-only thread: an empty `mcp_servers` table, AI6's servers left
/// out, and no built-in tool ([`NO_BUILT_IN_TOOLS`]). Codex merges the table into the user's
/// own servers, which the tool check switches off by name ([`tools`]).
fn answer_only_argv(executable: &str) -> Vec<String> {
    let mut argv = vec![
        executable.to_owned(),
        "app-server".to_owned(),
        "-c".to_owned(),
        "mcp_servers={}".to_owned(),
    ];
    argv.extend(no_built_in_tools());
    argv
}

/// The app server of an assistant thread, built from its mode's parts: `mcp_servers` set to
/// one inline table holding exactly the mode's servers, which Codex merges into the user's own
/// (the tool check switches those off by name, [`tools`]), each with its URL and the name of
/// the variable holding its bearer token, and no built-in tool ([`NO_BUILT_IN_TOOLS`]): no
/// shell, no file or image reading, no web. The
/// thread itself runs as an answer-only one ([`restrict_to_answer_only`]). Interpreters,
/// skills and any network beyond the servers are refused until this handler can confine them.
fn assistant_argv(
    executable: &str,
    environment: &std::collections::BTreeMap<String, String>,
    mode: &AssistantMode,
) -> Result<Vec<String>, ProviderError> {
    mode.validate()
        .map_err(|reason| refuse(Status::GUATIAO_ERR_BAD_VALUE, reason))?;
    if !mode.interpreters.is_empty() || !mode.skills.is_empty() {
        return Err(refuse(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this Codex handler runs no interpreter or skill for an assistant",
        ));
    }
    if !mode.network.reaches_nothing() {
        return Err(refuse(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this Codex handler gives an assistant no network beyond its MCP servers",
        ));
    }
    let mut servers = Vec::new();
    for server in &mode.mcp_servers {
        let prefix = &server.env_prefix;
        let (Some(url), true) = (
            environment.get(&format!("{prefix}_URL")),
            environment.contains_key(&format!("{prefix}_TOKEN")),
        ) else {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!(
                    "the environment carries no URL and token for MCP server {:?}",
                    server.name
                ),
            ));
        };
        // A JSON string is a valid TOML basic string.
        let url = serde_json::to_string(url).map_err(internal)?;
        let token = serde_json::to_string(&format!("{prefix}_TOKEN")).map_err(internal)?;
        servers.push(format!(
            "{}={{url={url},bearer_token_env_var={token}}}",
            server.name
        ));
    }
    let mut argv = vec![
        executable.to_owned(),
        "app-server".to_owned(),
        "-c".to_owned(),
        format!("mcp_servers={{{}}}", servers.join(",")),
    ];
    argv.extend(no_built_in_tools());
    Ok(argv)
}

/// Marks the working directory an untrusted project for this process, so Codex skips its
/// project `.codex/` layer: project config, hooks and rules. `projects` is set as one inline
/// table because a path holds dots, which a dotted `-c` key would split; it replaces the
/// user's own `projects` table for this process only.
fn untrusted_project(cwd: &str) -> [String; 2] {
    let path = serde_json::to_string(cwd).unwrap_or_else(|_| "\"\"".to_owned());
    [
        "-c".to_owned(),
        format!("projects={{{path}={{trust_level=\"untrusted\"}}}}"),
    ]
}

/// Whether the Station serves a Theater's tools, which is remote-hands mode.
fn is_remote_hands(environment: &std::collections::BTreeMap<String, String>) -> bool {
    environment.contains_key("AI6_THEATER_MCP_URL")
        && environment.contains_key("AI6_THEATER_MCP_TOKEN")
}

/// Codex cannot switch its shell tool off, so in remote-hands mode it runs in a read-only
/// sandbox that asks nothing: work cannot land on the Station, and the Theater tools are
/// how it changes anything.
fn restrict_to_remote_hands(params: &mut Value) {
    params["sandbox"] = json!("read-only");
    params["approvalPolicy"] = json!("never");
}

/// A thread that only reads its prompt and answers: its app server has no built-in tool
/// ([`NO_BUILT_IN_TOOLS`]), and the thread also runs in the read-only sandbox (no writes;
/// Codex gives that sandbox no network), never asks, and has web search off.
fn restrict_to_answer_only(params: &mut Value) {
    restrict_to_remote_hands(params);
    params["config"] = json!({ "web_search": "disabled" });
}

fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, ProviderError> {
    mutex
        .lock()
        .map_err(|_| refuse(Status::GUATIAO_ERR_INTERNAL, "session lock was poisoned"))
}

fn internal(error: impl std::fmt::Display) -> ProviderError {
    refuse(Status::GUATIAO_ERR_INTERNAL, error)
}

fn refuse(status: Status, error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(status, &error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    struct ProcessTerminal {
        specs: Arc<Mutex<Vec<Spec>>>,
    }

    impl ProcessTerminal {
        fn new() -> Self {
            Self {
                specs: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl AgentTerminal for ProcessTerminal {
        fn spawn(
            &self,
            value: &ai6::guatiao::Value,
        ) -> Result<Object<dyn TerminalSession>, ProviderError> {
            let spec = Spec::from_value(value).map_err(internal)?;
            self.specs.lock().unwrap().push(spec.clone());

            let mut child = Command::new(std::env::current_exe().map_err(internal)?)
                .args(["--exact", "tests::fake_codex_child", "--nocapture"])
                .env("AI6_FAKE_CODEX_CHILD", "1")
                .env("AI6_FAKE_CODEX_ARGV", spec.argv[1..].join("\u{1f}"))
                .envs(spec.env.unwrap_or_default())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(internal)?;
            let stdin = child.stdin.take().ok_or_else(|| {
                refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "fake Codex stdin is unavailable",
                )
            })?;
            let stdout = child.stdout.take().ok_or_else(|| {
                refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "fake Codex stdout is unavailable",
                )
            })?;
            let (sender, receiver) = mpsc::channel();
            // The tool check's commands get everything, libtest's lines included.
            let json_only = spec
                .argv
                .get(1)
                .is_some_and(|command| command == "app-server");
            thread::spawn(move || forward_json_lines(stdout, sender, json_only));

            Ok(ProcessTerminalSession {
                child: Mutex::new(child),
                stdin: Mutex::new(stdin),
                receiver: Mutex::new(receiver),
                pending: Mutex::new(std::collections::VecDeque::new()),
            }
            .into_object())
        }
    }

    /// Marks where a fake tool-check command's own output starts and ends.
    const FAKE_BEGIN: &str = "AI6-FAKE-OUTPUT-BEGIN";
    const FAKE_END: &str = "AI6-FAKE-OUTPUT-END";

    /// Hands over the fake app server's JSON lines, or with `json_only` false a tool-check
    /// command's output between [`FAKE_BEGIN`] and [`FAKE_END`].
    fn forward_json_lines(mut stdout: impl Read, sender: mpsc::Sender<Vec<u8>>, json_only: bool) {
        let mut reader = BufReader::new(&mut stdout);
        let mut line = Vec::new();
        let mut inside = false;
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) if !json_only => {
                    let text = String::from_utf8_lossy(&line);
                    if text.trim() == FAKE_BEGIN {
                        inside = true;
                    } else if text.trim() == FAKE_END
                        || (inside && sender.send(line.clone()).is_err())
                    {
                        return;
                    }
                }
                Ok(_) if serde_json::from_slice::<Value>(&line).is_ok() => {
                    if sender.send(line.clone()).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
            }
        }
    }

    struct ProcessTerminalSession {
        child: Mutex<Child>,
        stdin: Mutex<ChildStdin>,
        receiver: Mutex<Receiver<Vec<u8>>>,
        pending: Mutex<std::collections::VecDeque<u8>>,
    }

    impl TerminalSession for ProcessTerminalSession {
        fn pid(&self) -> i64 {
            i64::from(self.child.lock().unwrap().id())
        }

        fn read(&self, dst: &mut [u8], timeout_ms: i64) -> Result<i64, ProviderError> {
            let mut pending = self.pending.lock().unwrap();
            if pending.is_empty() {
                match self
                    .receiver
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_millis(timeout_ms.max(0) as u64))
                {
                    Ok(bytes) => pending.extend(bytes),
                    Err(mpsc::RecvTimeoutError::Timeout) => return Ok(READ_TIMED_OUT),
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
                }
            }
            let count = dst.len().min(pending.len());
            for byte in &mut dst[..count] {
                *byte = pending
                    .pop_front()
                    .expect("count is bounded by pending bytes");
            }
            Ok(count as i64)
        }

        fn write(&self, data: &[u8]) -> Result<(), ProviderError> {
            self.stdin.lock().unwrap().write_all(data).map_err(internal)
        }

        fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
            Ok(())
        }

        fn kill(&self) -> Result<(), ProviderError> {
            let mut child = self.child.lock().unwrap();
            match child.kill() {
                Ok(()) => child.wait().map(|_| ()).map_err(internal),
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
                Err(error) => Err(internal(error)),
            }
        }

        fn exit_status(&self, _timeout_ms: i64) -> Result<ai6::guatiao::Value, ProviderError> {
            Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                "process fixture does not expose exit metadata",
            ))
        }
    }

    #[test]
    fn fake_codex_child() {
        if std::env::var_os("AI6_FAKE_CODEX_CHILD").is_none() {
            return;
        }
        let argv = std::env::var("AI6_FAKE_CODEX_ARGV").unwrap_or_default();
        let argv = argv.split('\u{1f}').collect::<Vec<_>>();
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        // The tool check's commands, as Codex 0.164.0 answers them, between markers that
        // keep libtest's own lines out of what the fake terminal hands over.
        let answer = |stdout: &mut std::io::StdoutLock<'_>, output: &str| -> ! {
            writeln!(stdout, "\n{FAKE_BEGIN}\n{output}\n{FAKE_END}").unwrap();
            stdout.flush().unwrap();
            std::process::exit(0)
        };
        match argv.first().copied() {
            Some("--version") => answer(&mut stdout, "codex-cli 0.164.0"),
            Some("features") => {
                let output = std::env::var("CODEX_FIXTURE_FEATURES").unwrap_or_else(|_| {
                    [
                        "code_mode_host                           stable             true",
                        "memories                                 stable             false",
                        "secret_auth_storage                      stable             true",
                        "tool_suggest                             under development  true",
                    ]
                    .join("\n")
                });
                answer(&mut stdout, &output)
            }
            Some("mcp") => {
                let output =
                    std::env::var("CODEX_FIXTURE_MCP_LIST").unwrap_or_else(|_| "[]".to_owned());
                answer(&mut stdout, &output)
            }
            Some("debug") => {
                let output = std::env::var("CODEX_FIXTURE_PROMPT_INPUT").unwrap_or_else(|_| {
                    let servers = argv
                        .iter()
                        .any(|argument| argument.starts_with("mcp_servers={moneypenny="));
                    if servers {
                        "[{\"type\":\"additional_tools\",\"tools\":[{\"type\":\"namespace\",\
                         \"name\":\"mcp__moneypenny\",\"tools\":[{\"type\":\"function\",\
                         \"name\":\"board\"}]}]},{\"type\":\"message\",\"role\":\"user\",\
                         \"content\":[]}]"
                            .to_owned()
                    } else {
                        "[{\"type\":\"message\",\"role\":\"user\",\"content\":[]}]".to_owned()
                    }
                });
                answer(&mut stdout, &output)
            }
            _ => {}
        }
        let thread_id = std::env::var("CODEX_FIXTURE_THREAD_ID").unwrap();
        // A turn whose text is `run: COMMAND` asks the client to approve COMMAND, as the
        // app-server does before running a command, and reports the decision it receives.
        let mut asked = 900_u64;
        let mut pending = None;
        for line in std::io::stdin().lock().lines() {
            let message: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let method = message.get("method").and_then(Value::as_str).unwrap_or("");
            if pending.is_some() && message.get("id").and_then(Value::as_u64) == pending {
                pending = None;
                let decision = message
                    .pointer("/result/decision")
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                let delta = format!("decision: {decision}");
                writeln!(
                    stdout,
                    "{}",
                    json!({"method":"item/agentMessage/delta","params":{"delta":delta}})
                )
                .unwrap();
                if decision == "accept" {
                    writeln!(stdout, "{}", json!({"method":"turn/completed","params":{}})).unwrap();
                }
                stdout.flush().unwrap();
                continue;
            }
            if method == "turn/steer" {
                assert_eq!(
                    message.pointer("/params/expectedTurnId"),
                    Some(&json!("turn-7"))
                );
                let text = message
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let delta = format!("steered: {text}");
                writeln!(
                    stdout,
                    "{}",
                    json!({"method":"item/agentMessage/delta","params":{"delta":delta}})
                )
                .unwrap();
                writeln!(stdout, "{}", json!({"method":"turn/completed","params":{}})).unwrap();
                stdout.flush().unwrap();
                continue;
            }
            // `stdin: TEXT` asks to write to a running `cat`; `nameless` names no command.
            let text = message
                .pointer("/params/input/0/text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            // `item: TYPE [SERVER]` starts a thread item of that type, as a turn using a tool.
            if method == "turn/start"
                && let Some(item) = text.strip_prefix("item: ")
            {
                let mut parts = item.split(' ');
                let kind = parts.next().unwrap_or_default();
                let server = parts.next().unwrap_or_default();
                for line in [
                    json!({"method":"item/started","params":{"item":{
                        "type": kind, "id": "item-9", "server": server, "tool": "board"
                    }}}),
                    json!({"method":"item/agentMessage/delta","params":{"delta":"after"}}),
                    json!({"method":"turn/completed","params":{}}),
                ] {
                    writeln!(stdout, "{line}").unwrap();
                }
                stdout.flush().unwrap();
                continue;
            }
            let asks = if let Some(command) = text.strip_prefix("run: ") {
                Some(("command", json!(command)))
            } else if text.starts_with("stdin: ") {
                Some(("writeStdin", json!("cat")))
            } else if text == "nameless" {
                Some(("command", Value::Null))
            } else {
                None
            };
            if method == "turn/start"
                && let Some((kind, command)) = asks
            {
                asked += 1;
                pending = Some(asked);
                // The command execution item the approval is for starts first, as Codex's
                // app server announces an item before asking about it.
                writeln!(
                    stdout,
                    "{}",
                    json!({"method":"item/started","params":{"item":{
                        "type":"commandExecution","id":"item-1","command":command
                    }}})
                )
                .unwrap();
                writeln!(
                    stdout,
                    "{}",
                    json!({
                        "id": asked,
                        "method": "item/commandExecution/requestApproval",
                        "params": {
                            "threadId": thread_id, "turnId": "turn-7", "itemId": "item-1",
                            "startedAtMs": 1_790_000_000_000_u64, "command": command,
                            "cwd": "/work", "kind": kind
                        }
                    })
                )
                .unwrap();
                stdout.flush().unwrap();
                continue;
            }
            if let Some(id) = message.get("id") {
                match method {
                    "initialize" => {
                        writeln!(stdout, "{}", json!({"id":id,"result":{}})).unwrap();
                        stdout.flush().unwrap();
                    }
                    "thread/start" | "thread/resume" => {
                        // Reports the thread's policy as text, for tests that check it.
                        if std::env::var_os("CODEX_FIXTURE_SHOW_POLICY").is_some() {
                            let policy = json!({
                                "sandbox": message.pointer("/params/sandbox"),
                                "approvalPolicy": message.pointer("/params/approvalPolicy"),
                                "config": message.pointer("/params/config"),
                            });
                            writeln!(
                                stdout,
                                "{}",
                                json!({"method":"item/agentMessage/delta",
                                       "params":{"delta":format!("policy: {policy}")}})
                            )
                            .unwrap();
                        }
                        writeln!(
                            stdout,
                            "{}",
                            json!({"id":id,"result":{"thread":{"id":thread_id}}})
                        )
                        .unwrap();
                        stdout.flush().unwrap();
                    }
                    _ => {}
                }
            }
            if method == "turn/start" {
                let text = message
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                writeln!(
                    stdout,
                    "{}",
                    json!({"method":"item/agentMessage/delta","params":{"delta":text}})
                )
                .unwrap();
                writeln!(stdout, "{}", json!({"method":"turn/completed","params":{}})).unwrap();
                stdout.flush().unwrap();
            }
        }
    }

    fn read_to_completion(session: &Object<dyn AgentSession>) -> Vec<AgentEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Some(event) = session
                .next_event(remaining.as_millis().min(i64::MAX as u128) as i64)
                .unwrap()
            {
                let completed = event.kind == "completed";
                events.push(event);
                if completed {
                    return events;
                }
            }
        }
        panic!("fake Codex did not complete the provider Session")
    }

    #[test]
    fn process_session_restarts_and_resumes_with_the_same_provider_thread() {
        let terminal = ProcessTerminal::new();
        let session_id = SessionId::try_new("codex-process-thread-42").unwrap();
        let environment =
            BTreeMap::from([("CODEX_FIXTURE_THREAD_ID".to_owned(), session_id.to_string())]);
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-process-fixture".to_owned(),
        })
        .unwrap();

        let started = handler
            .start(
                AgentStart {
                    mission_id: "mission-process-42".parse().unwrap(),
                    agent_id: "007".to_owned(),
                    prompt: "first process turn".to_owned(),
                    initial_context: vec!["context before the prompt".to_owned()],
                    working_directory: std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    clearance: "developer".to_owned(),
                    environment: environment.clone(),
                    model: Some("gpt-5".to_owned()),
                    effort: Some("high".to_owned()),
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        let first_events = read_to_completion(&started);
        assert_eq!(started.session_id().unwrap(), Some(session_id.clone()));
        assert!(first_events.iter().any(|event| {
            event.kind == "text" && event.text.as_deref() == Some("first process turn")
        }));
        assert!(first_events.iter().any(|event| event.kind == "completed"));
        started.close().unwrap();

        let resumed = handler
            .resume(
                AgentResume {
                    mission_id: "mission-process-42".parse().unwrap(),
                    session_id: session_id.clone(),
                    message: Some("resumed process turn".to_owned()),
                    working_directory: std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    environment,
                    model: Some("gpt-5".to_owned()),
                    effort: Some("high".to_owned()),
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                    initial_context: Vec::new(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        assert_eq!(resumed.session_id().unwrap(), Some(session_id.clone()));
        let resumed_events = read_to_completion(&resumed);
        assert!(resumed_events.iter().any(|event| {
            event.kind == "text" && event.text.as_deref() == Some("resumed process turn")
        }));
        assert!(resumed_events.iter().any(|event| event.kind == "completed"));
        resumed.close().unwrap();

        let specs = terminal.specs.lock().unwrap();
        assert_eq!(specs.len(), 2);
        let cwd = std::env::current_dir().unwrap();
        let expected = [
            vec!["codex-process-fixture".to_owned(), "app-server".to_owned()],
            untrusted_project(&cwd.to_string_lossy()).to_vec(),
        ]
        .concat();
        assert_eq!(
            specs[0].argv, expected,
            "start ignores the project's settings"
        );
        assert_eq!(specs[1].argv, expected, "and so does resume");
    }

    /// A gate that holds every governed step until an approval in `approved` covers it, as
    /// the Station answers through `check_approval`.
    /// A repository where every name is a branch and nothing is configured.
    struct Branches;

    impl ai6::directorates::m::approvals::Repository for Branches {
        fn full_name(
            &self,
            _at: &ai6::directorates::m::approvals::GitContext,
            name: &str,
        ) -> Result<Option<String>, String> {
            Ok(Some(format!("refs/heads/{name}")))
        }

        fn config(
            &self,
            _at: &ai6::directorates::m::approvals::GitContext,
            _key: &str,
        ) -> Result<Vec<String>, String> {
            Ok(Vec::new())
        }
    }

    fn approvals_gate(approved: Arc<Mutex<Vec<ai6::ActionScope>>>) -> Option<Gate> {
        Some(Arc::new(move |command, _cwd| {
            match ai6::classify(command, &Branches) {
                Err(reason) => GateVerdict::Hold(reason),
                Ok(steps) => {
                    let approved = approved.lock().unwrap();
                    match steps
                        .iter()
                        .find(|step| !approved.iter().any(|approval| approval.covers(step)))
                    {
                        None => GateVerdict::Allow,
                        Some(step) => GateVerdict::Hold(format!(
                            "call request_approval with kind {} and target {}",
                            step.kind, step.target
                        )),
                    }
                }
            }
        }))
    }

    fn texts_until_completed(session: &Object<dyn AgentSession>) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut texts = Vec::new();
        while Instant::now() < deadline {
            if let Some(event) = session.next_event(1_000).unwrap() {
                if event.kind == "completed" {
                    return texts;
                }
                texts.extend(event.text);
            }
        }
        panic!("fake Codex did not complete the turn: {texts:?}")
    }

    #[test]
    fn a_governed_command_is_declined_until_green_ink_covers_it() {
        let approved = Arc::new(Mutex::new(Vec::new()));
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-gate-fixture".to_owned(),
        })
        .unwrap();
        let session = handler
            .start_gated(
                AgentStart {
                    mission_id: "mission-gate".parse().unwrap(),
                    agent_id: "007".to_owned(),
                    prompt: "run: git push origin main".to_owned(),
                    initial_context: Vec::new(),
                    working_directory: std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    clearance: "field".to_owned(),
                    environment: BTreeMap::from([(
                        "CODEX_FIXTURE_THREAD_ID".to_owned(),
                        "codex-gate-thread".to_owned(),
                    )]),
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                },
                ProcessTerminal::new().into_object(),
                approvals_gate(approved.clone()),
            )
            .unwrap();

        let declined = texts_until_completed(&session);
        assert_eq!(declined[0], "decision: decline", "{declined:?}");
        assert!(
            declined[1].starts_with("steered: ")
                && declined[1].contains("request_approval")
                && declined[1].contains("origin:refs/heads/main"),
            "the Agent is told why: {declined:?}"
        );

        approved
            .lock()
            .unwrap()
            .push(ai6::ActionScope::try_new("push", "origin:main").unwrap());
        session
            .send_message(AgentMessage {
                text: "run: git push origin main".to_owned(),
            })
            .unwrap();
        assert_eq!(texts_until_completed(&session), ["decision: accept"]);

        session
            .send_message(AgentMessage {
                text: "run: cargo test --workspace".to_owned(),
            })
            .unwrap();
        assert_eq!(texts_until_completed(&session), ["decision: accept"]);

        // The gate cannot see what these run, so they are declined and the Agent told why.
        for (text, why) in [
            ("stdin: git push origin main", "input to a running command"),
            ("nameless", "names no command"),
        ] {
            session
                .send_message(AgentMessage {
                    text: text.to_owned(),
                })
                .unwrap();
            let declined = texts_until_completed(&session);
            assert_eq!(declined[0], "decision: decline", "{text}: {declined:?}");
            assert!(declined[1].contains(why), "{text}: {declined:?}");
        }
        session.close().unwrap();
    }

    /// What a gated session answers to `stream`: the decision written back, and the events.
    fn answered(stream: &[u8]) -> (Value, Vec<AgentEvent>) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let session = CodexSession::new(
            ResumeTerminalSession {
                input: Mutex::new(stream.to_vec()),
                writes: writes.clone(),
                killed: Arc::new(AtomicBool::new(false)),
            }
            .into_object(),
            None,
        )
        .with_gate(approvals_gate(Arc::new(Mutex::new(Vec::new()))));
        let mut events = Vec::new();
        while let Some(event) = session.next_event(20).unwrap() {
            events.push(event);
        }
        while let Ok(Some(event)) = session.next_event(20) {
            events.push(event);
        }
        let written = String::from_utf8(writes.lock().unwrap().clone()).unwrap();
        let decision = written
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|message| message.pointer("/result/decision").is_some())
            .expect("the approval was answered");
        (decision["result"]["decision"].clone(), events)
    }

    fn started(command: &str) -> String {
        json!({"method":"item/started","params":{"item":{
            "type":"commandExecution","id":"item-1","command":command}}})
        .to_string()
    }

    fn approval(command: &str) -> String {
        json!({"id": 901, "method": COMMAND_APPROVAL, "params": {
            "threadId": "t", "turnId": "u", "itemId": "item-1",
            "command": command, "cwd": "/work", "kind": "command"}})
        .to_string()
    }

    #[test]
    fn an_approval_is_declined_unless_read_as_written_after_its_item_started() {
        let declined_event = |events: &[AgentEvent]| {
            events
                .iter()
                .find(|event| event.kind == "declined")
                .and_then(|event| event.text.clone())
        };
        let whole = format!(
            "{}\r\n{}\r\n",
            started("cargo test"),
            approval("cargo test")
        );
        let (decision, events) = answered(whole.as_bytes());
        assert_eq!(decision, "accept", "{events:?}");

        let (decision, events) = answered(format!("{}\r\n", approval("cargo test")).as_bytes());
        assert_eq!(decision, "decline");
        assert!(
            declined_event(&events)
                .unwrap()
                .contains("no command execution item")
        );

        let other = format!(
            "{}\r\n{}\r\n",
            started("cargo build"),
            approval("cargo test")
        );
        assert_eq!(answered(other.as_bytes()).0, "decline", "another command");

        let line = approval("cargo test");
        let (head, tail) = line.split_at(line.find("cargo").unwrap());
        let broken = format!(
            "{}\r\n{head}\r\n\x1b[23;80H{tail}\r\n",
            started("cargo test")
        );
        let (decision, events) = answered(broken.as_bytes());
        assert_eq!(decision, "decline", "a line the terminal broke");
        assert!(declined_event(&events).unwrap().contains("rebuilt"));

        let inside = format!("{}\r\n{head}\x1b[?25l{tail}\r\n", started("cargo test"));
        assert_eq!(
            answered(inside.as_bytes()).0,
            "decline",
            "a control other than colour inside the line"
        );
        let colour = format!("{}\r\n{head}\x1b[0m{tail}\r\n", started("cargo test"));
        assert_eq!(answered(colour.as_bytes()).0, "accept", "colour alone");
    }

    #[test]
    fn a_gated_session_asks_before_every_untrusted_command() {
        let cable = std::collections::BTreeMap::from([
            (
                "AI6_CABLE_MCP_URL".to_owned(),
                "http://127.0.0.1:1/mcp".to_owned(),
            ),
            ("AI6_CABLE_MCP_TOKEN".to_owned(), "token".to_owned()),
        ]);
        let mut params = json!({ "cwd": "/work" });
        set_approval_policy(&mut params, &cable, false);
        assert_eq!(params["approvalPolicy"], "untrusted");

        let mut params = json!({ "cwd": "/work" });
        set_approval_policy(&mut params, &cable, true);
        assert_eq!(
            params["sandbox"], "read-only",
            "answer-only wins over the gate"
        );
        assert_eq!(params["approvalPolicy"], "never");
        assert_eq!(params["config"]["web_search"], "disabled");

        let mut remote = cable.clone();
        remote.insert(
            "AI6_THEATER_MCP_URL".to_owned(),
            "http://127.0.0.1:2/mcp".to_owned(),
        );
        remote.insert("AI6_THEATER_MCP_TOKEN".to_owned(), "theater".to_owned());
        let mut params = json!({ "cwd": "/work" });
        set_approval_policy(&mut params, &remote, false);
        assert_eq!(
            params["approvalPolicy"], "never",
            "the Station gates theater_run"
        );

        let mut params = json!({ "cwd": "/work" });
        set_approval_policy(&mut params, &std::collections::BTreeMap::new(), false);
        assert!(
            params.get("approvalPolicy").is_none(),
            "no gate, Codex's own policy"
        );
    }

    struct EofTerminalSession;

    impl TerminalSession for EofTerminalSession {
        fn pid(&self) -> i64 {
            1
        }

        fn read(&self, _dst: &mut [u8], _timeout_ms: i64) -> Result<i64, ProviderError> {
            Ok(0)
        }

        fn write(&self, _data: &[u8]) -> Result<(), ProviderError> {
            Ok(())
        }

        fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
            Ok(())
        }

        fn kill(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn exit_status(&self, _timeout_ms: i64) -> Result<ai6::guatiao::Value, ProviderError> {
            Err(refuse(Status::GUATIAO_ERR_INTERNAL, "unused in EOF test"))
        }
    }

    struct ResumeTerminal {
        spec: Arc<Mutex<Option<Spec>>>,
        input: Vec<u8>,
        writes: Arc<Mutex<Vec<u8>>>,
        killed: Arc<AtomicBool>,
    }

    impl AgentTerminal for ResumeTerminal {
        fn spawn(
            &self,
            value: &ai6::guatiao::Value,
        ) -> Result<Object<dyn TerminalSession>, ProviderError> {
            *self.spec.lock().unwrap() = Some(Spec::from_value(value).map_err(internal)?);
            Ok(ResumeTerminalSession {
                input: Mutex::new(self.input.clone()),
                writes: self.writes.clone(),
                killed: self.killed.clone(),
            }
            .into_object())
        }
    }

    struct ResumeTerminalSession {
        input: Mutex<Vec<u8>>,
        writes: Arc<Mutex<Vec<u8>>>,
        killed: Arc<AtomicBool>,
    }

    impl TerminalSession for ResumeTerminalSession {
        fn pid(&self) -> i64 {
            8
        }

        fn read(&self, dst: &mut [u8], _timeout_ms: i64) -> Result<i64, ProviderError> {
            let mut input = self.input.lock().unwrap();
            if input.is_empty() {
                return Ok(READ_TIMED_OUT);
            }
            let count = dst.len().min(input.len());
            dst[..count].copy_from_slice(&input[..count]);
            input.drain(..count);
            Ok(count as i64)
        }

        fn write(&self, data: &[u8]) -> Result<(), ProviderError> {
            self.writes.lock().unwrap().extend_from_slice(data);
            Ok(())
        }

        fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
            Ok(())
        }

        fn kill(&self) -> Result<(), ProviderError> {
            self.killed.store(true, Ordering::Release);
            Ok(())
        }

        fn exit_status(&self, _timeout_ms: i64) -> Result<ai6::guatiao::Value, ProviderError> {
            Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                "unused in resume test",
            ))
        }
    }

    #[test]
    fn resume_reattaches_and_submits_a_turn_to_the_same_provider_thread() {
        let session_id = SessionId::try_new("codex-thread-42").unwrap();
        let spec = Arc::new(Mutex::new(None));
        let writes = Arc::new(Mutex::new(Vec::new()));
        let killed = Arc::new(AtomicBool::new(false));
        let terminal = ResumeTerminal {
            spec: spec.clone(),
            input: b"{\"id\":1,\"result\":{}}\n{\"id\":2,\"result\":{\"thread\":{\"id\":\"codex-thread-42\"}}}\n".to_vec(),
            writes: writes.clone(),
            killed: killed.clone(),
        };
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-test".to_owned(),
        })
        .unwrap();
        let session = handler
            .resume(
                AgentResume {
                    mission_id: "mission-42".parse().unwrap(),
                    session_id: session_id.clone(),
                    message: Some("continue the same Session".to_owned()),
                    working_directory: "C:\\station\\mission-42".to_owned(),
                    environment: BTreeMap::from([("TOKEN".to_owned(), "fresh".to_owned())]),
                    model: Some("gpt-5".to_owned()),
                    effort: Some("high".to_owned()),
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                    initial_context: Vec::new(),
                },
                terminal.into_object(),
            )
            .unwrap();

        assert_eq!(session.session_id().unwrap(), Some(session_id));

        let spec = spec.lock().unwrap().clone().unwrap();
        assert_eq!(
            spec.argv,
            [
                vec!["codex-test".to_owned(), "app-server".to_owned()],
                untrusted_project("C:\\station\\mission-42").to_vec(),
            ]
            .concat()
        );
        assert_eq!(spec.cwd.as_deref(), Some("C:\\station\\mission-42"));
        assert_eq!(
            spec.env.as_ref().unwrap().get("TOKEN").map(String::as_str),
            Some("fresh")
        );

        let written = String::from_utf8(writes.lock().unwrap().clone()).unwrap();
        let messages = written
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(messages[0]["method"], "initialize");
        assert_eq!(messages[1]["method"], "initialized");
        assert_eq!(messages[2]["method"], "thread/resume");
        assert_eq!(messages[2]["params"]["threadId"], "codex-thread-42");
        assert_eq!(messages[2]["params"]["excludeTurns"], true);
        assert_eq!(messages[2]["params"]["model"], "gpt-5");
        assert_eq!(messages[2]["params"]["cwd"], "C:\\station\\mission-42");
        assert_eq!(messages[3]["method"], "turn/start");
        assert_eq!(messages[3]["params"]["threadId"], "codex-thread-42");
        assert_eq!(messages[3]["params"]["effort"], "high");
        assert_eq!(
            messages[3]["params"]["input"][0]["text"],
            "continue the same Session"
        );
        assert!(!killed.load(Ordering::Acquire));
    }

    /// The app server's replies as ConPTY renders them on Windows: mode and title sequences
    /// first, CRLF line ends, a cursor move between lines, and a reply longer than the
    /// terminal broken with CRLF and a cursor move.
    #[test]
    fn replies_rendered_by_conpty_read_as_the_lines_codex_wrote() {
        let delta = "d".repeat(5_000);
        let notification = format!(
            "{{\"method\":\"item/agentMessage/delta\",\"params\":{{\"delta\":\"{delta}\"}}}}"
        );
        let (head, tail) = notification.split_at(2_000);
        let mut input =
            b"\x1b[?9001h\x1b[?1004h\x1b[m\x1b]0;C:\\bin\\codex.exe\x07\x1b[?25h\r\n".to_vec();
        input.extend_from_slice(b"{\"id\":1,\"result\":{}}\r\n\x1b[?25l");
        input.extend_from_slice(
            b"\x1b[5;1H{\"id\":2,\"result\":{\"thread\":{\"id\":\"codex-thread-42\"}}}\r\n",
        );
        input.extend_from_slice(head.as_bytes());
        input.extend_from_slice(b"\r\n\x1b[23;2000H");
        input.extend_from_slice(tail.as_bytes());
        input.extend_from_slice(b"\r\n");
        let spec = Arc::new(Mutex::new(None));
        let terminal = ResumeTerminal {
            spec: spec.clone(),
            input,
            writes: Arc::new(Mutex::new(Vec::new())),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-test".to_owned(),
        })
        .unwrap();
        let session = handler
            .resume(
                AgentResume {
                    mission_id: "mission-42".parse().unwrap(),
                    session_id: SessionId::try_new("codex-thread-42").unwrap(),
                    message: None,
                    working_directory: "C:\\station\\mission-42".to_owned(),
                    environment: BTreeMap::new(),
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                    initial_context: Vec::new(),
                },
                terminal.into_object(),
            )
            .unwrap();
        let event = session.next_event(50).unwrap().unwrap();
        assert_eq!(
            (event.kind.as_str(), event.text.as_deref()),
            ("text", Some(&*delta))
        );
        assert_eq!(spec.lock().unwrap().as_ref().unwrap().cols, STREAM_COLS);
    }

    #[test]
    fn resume_rejects_a_different_thread_and_closes_the_provider_process() {
        let spec = Arc::new(Mutex::new(None));
        let writes = Arc::new(Mutex::new(Vec::new()));
        let killed = Arc::new(AtomicBool::new(false));
        let terminal = ResumeTerminal {
            spec,
            input: b"{\"id\":1,\"result\":{}}\n{\"id\":2,\"result\":{\"thread\":{\"id\":\"different-thread\"}}}\n".to_vec(),
            writes,
            killed: killed.clone(),
        };
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-test".to_owned(),
        })
        .unwrap();

        let result = handler.resume(
            AgentResume {
                mission_id: "mission-42".parse().unwrap(),
                session_id: SessionId::try_new("codex-thread-42").unwrap(),
                message: None,
                working_directory: "C:\\station\\mission-42".to_owned(),
                environment: BTreeMap::new(),
                model: None,
                effort: None,
                max_cost_micro_usd: None,
                project_settings: false,
                project_mcp: false,
                answer_only: false,
                assistant: None,
                initial_context: Vec::new(),
            },
            terminal.into_object(),
        );

        assert!(result.is_err());
        assert!(killed.load(Ordering::Acquire));
    }

    #[test]
    fn delta_fixture_maps_to_text_without_losing_provider_data() {
        let message = json!({
            "method": "item/agentMessage/delta",
            "params": { "delta": "hello from Codex" }
        });
        let event = decode_event(&message, None).unwrap();
        assert_eq!(event.kind, "text");
        assert_eq!(event.text.as_deref(), Some("hello from Codex"));
        assert_eq!(
            serde_json::from_slice::<Value>(event.data.as_deref().unwrap()).unwrap(),
            message
        );
    }

    #[test]
    fn a_usage_limit_error_becomes_a_limit_with_the_reset_it_names() {
        let message = json!({
            "method": "error",
            "params": {
                "error": {
                    "message": "You've hit your usage limit.",
                    "codexErrorInfo": "usageLimitExceeded",
                    "resetsAt": 1_790_000_000
                },
                "willRetry": false
            }
        });
        let event = decode_event(&message, None).unwrap();
        assert_eq!(event.kind, LIMIT_EVENT);
        assert_eq!(event.text.as_deref(), Some("You've hit your usage limit."));
        assert_eq!(event.limit.unwrap().resets_at, Some(1_790_000_000));
    }

    #[test]
    fn a_failed_turn_with_a_usage_limit_uses_the_exhausted_window_for_the_reset() {
        let snapshot = json!({
            "method": "account/rateLimits/updated",
            "params": { "rateLimits": {
                "primary": { "usedPercent": 100.0, "windowDurationMins": 300, "resetsAt": 1_790_000_000 },
                "secondary": { "usedPercent": 40.0, "windowDurationMins": 10_080, "resetsAt": 1_790_500_000 }
            } }
        });
        let hint = exhausted_window(&snapshot).unwrap();
        assert_eq!(hint.window.as_deref(), Some("five_hour"));

        let failed = json!({
            "method": "turn/completed",
            "params": { "turn": { "status": "failed", "error": {
                "message": "usage limit",
                "codexErrorInfo": "usage_limit_exceeded"
            } } }
        });
        let event = decode_event(&failed, Some(&hint)).unwrap();
        assert_eq!(event.kind, LIMIT_EVENT);
        let limit = event.limit.unwrap();
        assert_eq!(limit.resets_at, Some(1_790_000_000));
        assert_eq!(limit.window.as_deref(), Some("five_hour"));
        assert_eq!(
            decode_event(&failed, None)
                .unwrap()
                .limit
                .unwrap()
                .resets_at,
            None
        );
    }

    #[test]
    fn every_rate_limit_snapshot_reports_its_windows_as_quota() {
        let snapshot = json!({
            "method": "account/rateLimits/updated",
            "params": { "rateLimits": {
                "primary": { "usedPercent": 42.5, "windowDurationMins": 300, "resetsAt": 1_790_000_000 },
                "secondary": { "usedPercent": 40.0, "windowDurationMins": 10_080, "resetsAt": 1_790_500_000 }
            } }
        });
        let event = decode_event(&snapshot, None).unwrap();
        assert_eq!(event.kind, "account/rateLimits/updated");
        assert_eq!(
            event.quota,
            [
                QuotaReport {
                    window: "five_hour".to_owned(),
                    used_permille: Some(425),
                    resets_at: Some(1_790_000_000),
                    status: None,
                },
                QuotaReport {
                    window: "weekly".to_owned(),
                    used_permille: Some(400),
                    resets_at: Some(1_790_500_000),
                    status: None,
                },
            ]
        );
        let partial = json!({
            "method": "account/rateLimits/updated",
            "params": { "rateLimits": { "primary": { "usedPercent": 99.0, "resetsAt": 1 } } }
        });
        let quota = decode_event(&partial, None).unwrap().quota;
        assert_eq!(quota.len(), 1);
        assert_eq!(
            quota[0].window, "primary",
            "an unnamed window keeps its position"
        );
        assert_eq!(quota[0].used_permille, Some(990));
        let other = json!({ "method": "turn/completed", "params": {} });
        assert!(decode_event(&other, None).unwrap().quota.is_empty());
    }

    #[test]
    fn a_usage_snapshot_below_the_limit_and_other_errors_are_not_limits() {
        let below = json!({
            "method": "account/rateLimits/updated",
            "params": { "rateLimits": { "primary": { "usedPercent": 99.0, "resetsAt": 1 } } }
        });
        assert_eq!(exhausted_window(&below), None);
        let other = json!({
            "method": "error",
            "params": { "error": { "message": "boom", "codexErrorInfo": "internalServerError" } }
        });
        let event = decode_event(&other, None).unwrap();
        assert_eq!(event.kind, "error");
        assert_eq!(event.limit, None);
    }

    #[test]
    fn turn_completed_fixture_maps_to_completed() {
        let event =
            decode_event(&json!({ "method": "turn/completed", "params": {} }), None).unwrap();
        assert_eq!(event.kind, "completed");
    }

    #[test]
    fn terminal_eof_is_a_provider_failure_not_an_idle_timeout() {
        let session = CodexSession::new(EofTerminalSession.into_object(), None);
        let error = session
            .read_line(Duration::from_millis(10))
            .unwrap_err()
            .to_string();

        assert!(error.contains("closed its terminal"));
    }

    /// An app server that exited, behind a terminal that keeps its output open (ConPTY).
    struct ExitedTerminalSession {
        output: Mutex<Vec<u8>>,
    }

    impl TerminalSession for ExitedTerminalSession {
        fn pid(&self) -> i64 {
            1
        }

        fn read(&self, dst: &mut [u8], _timeout_ms: i64) -> Result<i64, ProviderError> {
            let mut output = self.output.lock().unwrap();
            if output.is_empty() {
                return Ok(READ_TIMED_OUT);
            }
            let count = dst.len().min(output.len());
            dst[..count].copy_from_slice(&output[..count]);
            output.drain(..count);
            Ok(count as i64)
        }

        fn write(&self, _data: &[u8]) -> Result<(), ProviderError> {
            Ok(())
        }

        fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
            Ok(())
        }

        fn kill(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn exit_status(&self, _timeout_ms: i64) -> Result<ai6::guatiao::Value, ProviderError> {
            ExitInfo {
                running: false,
                code: Some(2),
                signal: None,
            }
            .to_value()
            .map_err(internal)
        }
    }

    #[test]
    fn an_app_server_that_exits_fails_the_session_with_its_status_and_last_output() {
        let session = CodexSession::new(
            ExitedTerminalSession {
                output: Mutex::new(b"error: unknown model \"nope\"\r\n".to_vec()),
            }
            .into_object(),
            None,
        );
        let output = session.next_event(100).unwrap().unwrap();
        assert_eq!(output.kind, "output");
        let failed = session.next_event(100).unwrap().unwrap();
        assert_eq!(failed.kind, "failed");
        let reason = failed.text.unwrap();
        assert!(reason.contains("exited with code 2"), "{reason}");
        assert!(reason.contains("unknown model"), "{reason}");
        assert_eq!(session.next_event(100).unwrap(), None, "reported once");
        let error = session
            .request("thread/start", json!({}), Duration::from_millis(100))
            .unwrap_err()
            .to_string();
        assert!(error.contains("exited with code 2"), "{error}");
    }

    #[test]
    fn remote_hands_adds_the_theater_server_and_a_read_only_sandbox() {
        let environment = std::collections::BTreeMap::from([
            (
                "AI6_THEATER_MCP_URL".to_owned(),
                "http://127.0.0.1:43211/mcp".to_owned(),
            ),
            (
                "AI6_THEATER_MCP_TOKEN".to_owned(),
                "theater-secret-token".to_owned(),
            ),
        ]);
        assert!(is_remote_hands(&environment));
        assert!(!is_remote_hands(&std::collections::BTreeMap::new()));

        let argv = app_server_argv("codex", &environment);
        assert!(argv.iter().any(|argument| {
            argument == "mcp_servers.ai6-theater.url=\"http://127.0.0.1:43211/mcp\""
        }));
        assert!(argv.iter().any(|argument| {
            argument == "mcp_servers.ai6-theater.bearer_token_env_var=\"AI6_THEATER_MCP_TOKEN\""
        }));
        assert!(
            argv.iter()
                .all(|argument| !argument.contains("theater-secret-token"))
        );

        let mut params = json!({ "cwd": "/scratch" });
        restrict_to_remote_hands(&mut params);
        assert_eq!(params["sandbox"], "read-only");
        assert_eq!(params["approvalPolicy"], "never");
    }

    /// Codex reads `projects.<path>.trust_level` (`trusted` or `untrusted`), and an untrusted
    /// project skips its `.codex/` layer: config, hooks and rules (Codex config reference).
    #[test]
    fn an_ignored_project_is_marked_untrusted_in_one_inline_table() {
        let [flag, value] = untrusted_project("C:\\safe houses\\m.1");
        assert_eq!(flag, "-c");
        let (key, table) = value.split_once('=').unwrap();
        assert_eq!(key, "projects", "no dotted key for a path that holds dots");
        let parsed: toml::Table = toml::from_str(&format!("projects = {table}")).unwrap();
        assert_eq!(
            parsed["projects"]["C:\\safe houses\\m.1"]["trust_level"].as_str(),
            Some("untrusted")
        );
    }

    /// `mcp_servers.<name>.url` and `bearer_token_env_var` are Codex's streamable HTTP server
    /// keys, and `sandbox`, `approvalPolicy` and `config` are `thread/start` and
    /// `thread/resume` parameters, as the app-server schema this handler already speaks has
    /// them.
    #[test]
    fn an_assistant_thread_has_only_its_mcp_servers_read_only_on_start_and_resume() {
        let terminal = ProcessTerminal::new();
        let session_id = SessionId::try_new("codex-assistant-thread").unwrap();
        let environment = BTreeMap::from([
            ("CODEX_FIXTURE_THREAD_ID".to_owned(), session_id.to_string()),
            ("CODEX_FIXTURE_SHOW_POLICY".to_owned(), "1".to_owned()),
            (
                "AI6_MONEYPENNY_MCP_URL".to_owned(),
                "https://exports.example/v1/moneypenny/mcp".to_owned(),
            ),
            (
                "AI6_MONEYPENNY_MCP_TOKEN".to_owned(),
                "conversation-token".to_owned(),
            ),
            (
                "AI6_CABLE_MCP_URL".to_owned(),
                "http://127.0.0.1:1/mcp".to_owned(),
            ),
            ("AI6_CABLE_MCP_TOKEN".to_owned(), "cable-token".to_owned()),
            (
                "CODEX_FIXTURE_MCP_LIST".to_owned(),
                MCP_LIST_ASSISTANT.replace('\n', ""),
            ),
        ]);
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-assistant-fixture".to_owned(),
        })
        .unwrap();
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let started = handler
            .start(
                AgentStart {
                    mission_id: "assistant-conversation".parse().unwrap(),
                    agent_id: "moneypenny".to_owned(),
                    prompt: "Which Stations are busy?".to_owned(),
                    initial_context: vec!["You are Moneypenny.".to_owned()],
                    working_directory: cwd.clone(),
                    clearance: "desk".to_owned(),
                    environment: environment.clone(),
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: true,
                    project_mcp: true,
                    answer_only: false,
                    assistant: Some(AssistantMode::moneypenny()),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        let started_events = read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    mission_id: "assistant-conversation".parse().unwrap(),
                    session_id,
                    message: Some("And now?".to_owned()),
                    working_directory: cwd.clone(),
                    environment,
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: true,
                    project_mcp: true,
                    answer_only: false,
                    assistant: Some(AssistantMode::moneypenny()),
                    initial_context: Vec::new(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        let resumed_events = read_to_completion(&resumed);
        resumed.close().unwrap();
        for events in [started_events, resumed_events] {
            let policy = events
                .iter()
                .find_map(|event| event.text.as_deref()?.strip_prefix("policy: "))
                .expect("the thread's policy was reported");
            let policy: Value = serde_json::from_str(policy).unwrap();
            assert_eq!(policy["sandbox"], "read-only");
            assert_eq!(policy["approvalPolicy"], "never");
            assert_eq!(policy["config"]["web_search"], "disabled");
        }
        let all_specs = terminal.specs.lock().unwrap();
        let specs = all_specs
            .iter()
            .filter(|spec| spec.argv[1] == "app-server")
            .collect::<Vec<_>>();
        assert_eq!(specs.len(), 2);
        let settings = [
            vec![
                "-c".to_owned(),
                "mcp_servers={moneypenny={url=\"https://exports.example/v1/moneypenny/mcp\",\
                 bearer_token_env_var=\"AI6_MONEYPENNY_MCP_TOKEN\"}}"
                    .to_owned(),
            ],
            no_built_in_tools(),
            untrusted_project(&cwd).to_vec(),
        ]
        .concat();
        // The fixture's feature list enables code_mode_host, secret_auth_storage and
        // tool_suggest; the kept one stays on. Codex lists the user's node_repl beside the
        // thread's own server.
        let features_off = [
            "-c",
            "features.code_mode_host=false",
            "-c",
            "features.tool_suggest=false",
        ]
        .map(str::to_owned);
        let switched_off = [
            features_off.to_vec(),
            vec![
                "-c".to_owned(),
                "mcp_servers.node_repl.enabled=false".to_owned(),
            ],
        ]
        .concat();
        let listed = all_specs
            .iter()
            .find(|spec| spec.argv[1] == "mcp")
            .expect("the MCP servers were listed");
        assert_eq!(
            listed.argv[1..],
            [
                vec!["mcp".to_owned(), "list".to_owned(), "--json".to_owned()],
                settings.clone(),
                features_off.to_vec()
            ]
            .concat(),
            "with the thread's settings and features"
        );
        let expected = [
            vec![
                "codex-assistant-fixture".to_owned(),
                "app-server".to_owned(),
            ],
            settings.clone(),
            switched_off.to_vec(),
        ]
        .concat();
        let checked = all_specs
            .iter()
            .find(|spec| spec.argv.len() > 2 && spec.argv[1..3] == ["debug", "prompt-input"])
            .expect("the tools were checked");
        assert_eq!(
            checked.argv[3..],
            [settings, switched_off.to_vec()].concat(),
            "with the app server's own settings"
        );
        assert_eq!(checked.env, specs[0].env, "and environment");
        assert_eq!(checked.cwd, specs[0].cwd, "and working directory");
        assert_no_built_in_tools(&specs[0].argv);
        assert_no_built_in_tools(&specs[1].argv);
        assert_eq!(
            specs[0].argv, expected,
            "only Moneypenny's server, no Cables"
        );
        assert_eq!(specs[1].argv, expected, "and so on resume");
        let (key, value) = expected[3].split_once('=').unwrap();
        let parsed: toml::Table = toml::from_str(&format!("{key} = {value}")).unwrap();
        let servers = parsed["mcp_servers"].as_table().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(
            servers["moneypenny"]["bearer_token_env_var"].as_str(),
            Some("AI6_MONEYPENNY_MCP_TOKEN")
        );
        assert!(all_specs.iter().all(|spec| {
            spec.argv
                .iter()
                .all(|argument| !argument.contains("conversation-token"))
        }));
    }

    fn answer_only_start(environment: BTreeMap<String, String>) -> AgentStart {
        let mut environment = environment;
        environment.insert(
            "CODEX_FIXTURE_THREAD_ID".to_owned(),
            "codex-review-thread".to_owned(),
        );
        AgentStart {
            mission_id: "review--brief".parse().unwrap(),
            agent_id: "m".to_owned(),
            prompt: "Review this.".to_owned(),
            initial_context: Vec::new(),
            working_directory: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            clearance: "desk".to_owned(),
            environment,
            model: Some("gpt-6.1-sol".to_owned()),
            effort: None,
            max_cost_micro_usd: None,
            project_settings: false,
            project_mcp: false,
            answer_only: true,
            assistant: None,
        }
    }

    /// Codex 0.164.0's own answer with the settings this handler passed before the tool check.
    const OFFERS_EXEC: &str =
        include_str!("../tests/fixtures/codex-0.164.0/prompt-input-main.json");
    /// Codex 0.164.0's answer with the settings it passes now: the model's catalog entry still
    /// brings `functions.exec` and `functions.wait`.
    const ANSWER_ONLY_PROMPT: &str =
        include_str!("../tests/fixtures/codex-0.164.0/prompt-input-answer-only.json");
    /// `codex mcp list --json` of Codex 0.164.0 for a user with one stdio server, `node_repl`,
    /// alone and beside an assistant's `moneypenny`.
    const MCP_LIST: &str = include_str!("../tests/fixtures/codex-0.164.0/mcp-list.json");
    const MCP_LIST_ASSISTANT: &str =
        include_str!("../tests/fixtures/codex-0.164.0/mcp-list-assistant.json");

    #[test]
    fn an_answer_only_thread_codex_still_gives_exec_and_wait_is_not_started() {
        let terminal = ProcessTerminal::new();
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-catalog-fixture".to_owned(),
        })
        .unwrap();
        let environment = BTreeMap::from([
            (
                "CODEX_FIXTURE_PROMPT_INPUT".to_owned(),
                ANSWER_ONLY_PROMPT.replace('\n', ""),
            ),
            (
                "CODEX_FIXTURE_FEATURES".to_owned(),
                include_str!("../tests/fixtures/codex-0.164.0/features-list.txt").to_owned(),
            ),
            (
                "CODEX_FIXTURE_MCP_LIST".to_owned(),
                MCP_LIST.replace('\n', ""),
            ),
        ]);
        let refused = match handler.start(
            answer_only_start(environment),
            terminal.clone().into_object(),
        ) {
            Ok(_) => panic!("a thread offered functions.exec and functions.wait started"),
            Err(error) => error.to_string(),
        };
        assert!(
            refused.contains("functions.exec, functions.wait"),
            "{refused}"
        );
        let specs = terminal.specs.lock().unwrap();
        assert!(specs.iter().all(|spec| spec.argv[1] != "app-server"));
        let checked = specs
            .iter()
            .find(|spec| spec.argv.len() > 2 && spec.argv[1..3] == ["debug", "prompt-input"])
            .unwrap();
        let has = |setting: &str| checked.argv.iter().any(|argument| argument == setting);
        assert!(
            has("mcp_servers.node_repl.enabled=false") && has("features.code_mode_host=false"),
            "rendered with the user's server and the features off: {:?}",
            checked.argv
        );
        assert!(
            !checked
                .argv
                .iter()
                .any(|argument| argument.starts_with("features.incremental_tools")),
            "kept on, or prompt-input renders no tool"
        );
    }

    #[test]
    fn the_users_mcp_servers_are_switched_off_on_the_app_server() {
        let terminal = ProcessTerminal::new();
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-user-servers-fixture".to_owned(),
        })
        .unwrap();
        let environment = BTreeMap::from([(
            "CODEX_FIXTURE_MCP_LIST".to_owned(),
            MCP_LIST.replace('\n', ""),
        )]);
        let session = handler
            .start(
                answer_only_start(environment),
                terminal.clone().into_object(),
            )
            .unwrap();
        session.close().unwrap();
        let specs = terminal.specs.lock().unwrap();
        let server = specs
            .iter()
            .find(|spec| spec.argv[1] == "app-server")
            .unwrap();
        assert!(
            server
                .argv
                .windows(2)
                .any(|pair| pair == ["-c", "mcp_servers.node_repl.enabled=false"]),
            "{:?}",
            server.argv
        );
    }

    #[test]
    fn an_mcp_list_it_cannot_read_refuses_unless_warned() {
        for (index, output, found) in [
            (
                0,
                "Error: failed to load bootstrap configuration",
                "none of the user's is switched off",
            ),
            (
                1,
                "[{\"name\":\"we.ird\",\"enabled\":true}]",
                "\"we.ird\" of Codex's configuration cannot be switched off",
            ),
        ] {
            // An executable of its own, so the warned start reads its own list.
            let handler = CodexHandler::try_from(CodexConfig {
                executable: format!("codex-mcp-unread-fixture-{index}"),
            })
            .unwrap();
            let mut environment =
                BTreeMap::from([("CODEX_FIXTURE_MCP_LIST".to_owned(), output.to_owned())]);
            let terminal = ProcessTerminal::new();
            let refused = match handler.start(
                answer_only_start(environment.clone()),
                terminal.clone().into_object(),
            ) {
                Ok(_) => panic!("{output}: started"),
                Err(error) => error.to_string(),
            };
            assert!(refused.contains(found), "{refused}");
            assert!(
                terminal
                    .specs
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|spec| spec.argv[1] != "app-server" && spec.argv[1] != "debug"),
                "refused before anything else ran"
            );
            environment.insert(TOOL_CHECK_ENV.to_owned(), "warn".to_owned());
            let session = handler
                .start(
                    answer_only_start(environment),
                    ProcessTerminal::new().into_object(),
                )
                .unwrap();
            let first = session.next_event(1_000).unwrap().unwrap();
            assert_eq!(first.kind, "weakened");
            let text = first.text.unwrap();
            assert!(text.contains(found), "{text}");
            session.close().unwrap();
        }
    }

    #[test]
    fn an_answer_only_thread_offered_extra_tools_is_not_started() {
        let terminal = ProcessTerminal::new();
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-strict-fixture".to_owned(),
        })
        .unwrap();
        let environment = BTreeMap::from([(
            "CODEX_FIXTURE_PROMPT_INPUT".to_owned(),
            OFFERS_EXEC.replace('\n', ""),
        )]);
        let refused = match handler.start(
            answer_only_start(environment),
            terminal.clone().into_object(),
        ) {
            Ok(_) => panic!("a thread offered functions.exec started"),
            Err(error) => error.to_string(),
        };
        assert!(refused.contains("functions.exec"), "{refused}");
        let specs = terminal.specs.lock().unwrap();
        assert!(
            specs.iter().all(|spec| spec.argv[1] != "app-server"),
            "no app server was started"
        );
        let checked = specs
            .iter()
            .find(|spec| spec.argv.len() > 2 && spec.argv[1..3] == ["debug", "prompt-input"])
            .unwrap();
        assert!(
            checked
                .argv
                .ends_with(&["-c".to_owned(), "model=\"gpt-6.1-sol\"".to_owned()]),
            "what Codex offers depends on the model: {:?}",
            checked.argv
        );
    }

    #[test]
    fn a_warned_thread_starts_and_reports_the_extra_tools_first() {
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-warn-fixture".to_owned(),
        })
        .unwrap();
        let environment = BTreeMap::from([
            (
                "CODEX_FIXTURE_PROMPT_INPUT".to_owned(),
                OFFERS_EXEC.replace('\n', ""),
            ),
            (TOOL_CHECK_ENV.to_owned(), "warn".to_owned()),
        ]);
        let session = handler
            .start(
                answer_only_start(environment),
                ProcessTerminal::new().into_object(),
            )
            .unwrap();
        let first = session.next_event(1_000).unwrap().unwrap();
        assert_eq!(first.kind, "weakened");
        let text = first.text.unwrap();
        assert!(
            text.contains("functions.exec") && text.contains("mcp__cua_repl.js"),
            "{text}"
        );
        session.close().unwrap();
    }

    #[test]
    fn an_answer_only_thread_with_no_tool_passes_and_keeps_its_plumbing() {
        let terminal = ProcessTerminal::new();
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-pass-fixture".to_owned(),
        })
        .unwrap();
        let session = handler
            .start(
                answer_only_start(BTreeMap::new()),
                terminal.clone().into_object(),
            )
            .unwrap();
        session.close().unwrap();
        let specs = terminal.specs.lock().unwrap();
        let server = specs
            .iter()
            .find(|spec| spec.argv[1] == "app-server")
            .unwrap();
        assert!(
            server
                .argv
                .iter()
                .any(|a| a == "features.code_mode_host=false")
        );
        assert!(
            !server
                .argv
                .iter()
                .any(|a| a.starts_with("features.secret_auth_storage"))
        );
    }

    #[test]
    fn the_feature_list_is_reused_only_for_the_same_codex_home_and_the_tools_rendered_every_time() {
        let terminal = ProcessTerminal::new();
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-cache-fixture".to_owned(),
        })
        .unwrap();
        for home in ["C:\\agents\\one", "C:\\agents\\one", "C:\\agents\\two"] {
            let environment = BTreeMap::from([("CODEX_HOME".to_owned(), home.to_owned())]);
            let session = handler
                .start(
                    answer_only_start(environment),
                    terminal.clone().into_object(),
                )
                .unwrap();
            session.close().unwrap();
        }
        let specs = terminal.specs.lock().unwrap();
        let count = |command: &str| {
            specs
                .iter()
                .filter(|spec| spec.argv.get(1).map(String::as_str) == Some(command))
                .count()
        };
        assert_eq!(count("--version"), 3, "the version is asked every time");
        assert_eq!(
            count("features"),
            2,
            "another CODEX_HOME is another configuration"
        );
        assert_eq!(
            count("mcp"),
            2,
            "the MCP servers are reused with the features"
        );
        assert_eq!(
            count("debug"),
            3,
            "what Codex offers is rendered on every start"
        );
        assert_eq!(count("app-server"), 3);
    }

    #[test]
    fn a_feature_list_with_lines_it_cannot_read_refuses_unless_warned() {
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-unread-fixture".to_owned(),
        })
        .unwrap();
        let features = "code_mode_host stable true\nWARNING: the plugin cache is stale";
        let mut environment =
            BTreeMap::from([("CODEX_FIXTURE_FEATURES".to_owned(), features.to_owned())]);
        let refused = match handler.start(
            answer_only_start(environment.clone()),
            ProcessTerminal::new().into_object(),
        ) {
            Ok(_) => panic!("a feature list with a line it cannot read was accepted"),
            Err(error) => error.to_string(),
        };
        assert!(refused.contains("plugin cache is stale"), "{refused}");
        environment.insert(TOOL_CHECK_ENV.to_owned(), "warn".to_owned());
        let session = handler
            .start(
                answer_only_start(environment),
                ProcessTerminal::new().into_object(),
            )
            .unwrap();
        let first = session.next_event(1_000).unwrap().unwrap();
        assert_eq!(first.kind, "weakened");
        assert!(first.text.unwrap().contains("not a feature"));
        session.close().unwrap();
    }

    /// Events until the Session ends (`completed` or `failed`), at most five seconds.
    fn until_end(session: &Object<dyn AgentSession>) -> Vec<AgentEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            match session.next_event(200) {
                Ok(Some(event)) => {
                    let end = matches!(event.kind.as_str(), "completed" | "failed");
                    events.push(event);
                    if end {
                        break;
                    }
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
        events
    }

    #[test]
    fn a_restricted_thread_that_uses_a_tool_is_ended() {
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-trip-fixture".to_owned(),
        })
        .unwrap();
        for item in [
            "commandExecution",
            "fileChange",
            "webSearch",
            "mcpToolCall other",
        ] {
            let mut start = answer_only_start(BTreeMap::new());
            start.prompt = format!("item: {item}");
            let session = handler
                .start(start, ProcessTerminal::new().into_object())
                .unwrap();
            let events = until_end(&session);
            let last = events.last().expect("the Session ended");
            assert_eq!(last.kind, "failed", "{item}: {events:?}");
            let reason = last.text.as_deref().unwrap_or_default();
            assert!(reason.contains("restricted Codex thread used"), "{reason}");
            assert!(
                !events
                    .iter()
                    .any(|event| event.text.as_deref() == Some("after")),
                "nothing after the tool use is read: {events:?}"
            );
        }
    }

    #[test]
    fn a_restricted_thread_may_call_its_own_servers_and_talk() {
        assert_eq!(
            forbidden_use(
                &json!({"method":"item/started","params":{"item":{
                    "type":"mcpToolCall","server":"moneypenny","tool":"board"}}}),
                &["moneypenny".to_owned()]
            ),
            None
        );
        for kind in ["agentMessage", "reasoning", "userMessage"] {
            let message = json!({"method":"item/completed","params":{"item":{"type":kind}}});
            assert_eq!(forbidden_use(&message, &[]), None, "{kind}");
        }
        let unknown = json!({"method":"item/started","params":{"item":{"type":"hologram"}}});
        assert!(
            forbidden_use(&unknown, &[]).is_some(),
            "an unknown item is a tool"
        );
        let question = json!({"id": 4, "method": "item/tool/requestUserInput", "params": {}});
        assert!(forbidden_use(&question, &[]).is_some());
        let refresh = json!({"id": 5, "method": "account/chatgptAuthTokens/refresh"});
        assert_eq!(forbidden_use(&refresh, &[]), None, "plumbing is not a tool");
    }

    #[test]
    fn an_assistant_mode_the_codex_handler_cannot_confine_is_refused() {
        let environment = BTreeMap::from([
            (
                "AI6_MONEYPENNY_MCP_URL".to_owned(),
                "https://exports.example/v1/moneypenny/mcp".to_owned(),
            ),
            ("AI6_MONEYPENNY_MCP_TOKEN".to_owned(), "token".to_owned()),
        ]);
        let mut skills = AssistantMode::moneypenny();
        skills.skills.push("pdf".to_owned());
        let mut open = AssistantMode::moneypenny();
        open.network.policy = "open".to_owned();
        for mode in [skills, open] {
            assert!(assistant_argv("codex", &environment, &mode).is_err());
        }
        assert!(
            assistant_argv("codex", &BTreeMap::new(), &AssistantMode::moneypenny()).is_err(),
            "no endpoint in the environment"
        );
    }

    #[test]
    fn an_answer_only_app_server_has_no_mcp_server_at_all() {
        let argv = answer_only_argv("codex");
        assert_eq!(argv[..4], ["codex", "app-server", "-c", "mcp_servers={}"]);
        let (key, value) = argv[3].split_once('=').unwrap();
        let parsed: toml::Table = toml::from_str(&format!("{key} = {value}")).unwrap();
        assert!(parsed["mcp_servers"].as_table().unwrap().is_empty());
        assert_no_built_in_tools(&argv);
    }

    /// Asserts the `-c` overrides that leave Codex no shell, no exec, no image reading, no web
    /// search, no sub-agents and no app connectors, each parsed as Codex parses a `-c` value.
    fn assert_no_built_in_tools(argv: &[String]) {
        let mut settings = toml::Table::new();
        for pair in argv.windows(2).filter(|pair| pair[0] == "-c") {
            let parsed: toml::Table = toml::from_str(&pair[1]).unwrap();
            for (key, value) in parsed {
                match (settings.get_mut(&key), value) {
                    (Some(toml::Value::Table(merged)), toml::Value::Table(more)) => {
                        merged.extend(more);
                    }
                    (_, value) => {
                        settings.insert(key, value);
                    }
                }
            }
        }
        let off = |table: &str, key: &str| settings[table][key].as_bool();
        assert_eq!(off("features", "shell_tool"), Some(false), "{argv:?}");
        assert_eq!(
            settings["tools"]["experimental_request_user_input"]["enabled"].as_bool(),
            Some(false),
            "{argv:?}"
        );
        assert!(
            settings["tools"].get("view_image").is_none(),
            "Codex 0.164.0 ignores it as unrecognized: {argv:?}"
        );
        assert_eq!(off("features", "multi_agent"), Some(false), "{argv:?}");
        assert_eq!(off("agents", "enabled"), Some(false), "{argv:?}");
        assert_eq!(off("features", "apps"), Some(false), "{argv:?}");
        assert_eq!(
            settings["skills"]["max_context_tokens"].as_integer(),
            Some(1),
            "{argv:?}"
        );
        assert_eq!(
            settings["web_search"].as_str(),
            Some("disabled"),
            "{argv:?}"
        );
    }

    #[test]
    fn cable_mcp_config_passes_url_and_token_name_without_token_value() {
        let environment = std::collections::BTreeMap::from([
            (
                "AI6_CABLE_MCP_URL".to_owned(),
                "http://127.0.0.1:43210/mcp".to_owned(),
            ),
            (
                "AI6_CABLE_MCP_TOKEN".to_owned(),
                "session-secret-token".to_owned(),
            ),
        ]);

        let argv = app_server_argv("codex", &environment);
        assert!(argv.iter().any(|argument| {
            argument == "mcp_servers.ai6-cables.url=\"http://127.0.0.1:43210/mcp\""
        }));
        assert!(argv.iter().any(|argument| {
            argument == "mcp_servers.ai6-cables.bearer_token_env_var=\"AI6_CABLE_MCP_TOKEN\""
        }));
        assert!(
            argv.iter()
                .all(|argument| !argument.contains("session-secret-token"))
        );
    }

    /// The `thread/tokenUsage/updated` notification as the app-server protocol's schema
    /// describes it; the figures are invented.
    fn token_usage_notification() -> Value {
        json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "tokenUsage": {
                    "total": { "totalTokens": 9000, "inputTokens": 8000, "cachedInputTokens": 3000,
                               "outputTokens": 1000, "reasoningOutputTokens": 200 },
                    "last": { "totalTokens": 1500, "inputTokens": 1200, "cachedInputTokens": 700,
                              "outputTokens": 300, "reasoningOutputTokens": 50 },
                    "modelContextWindow": 272000
                }
            }
        })
    }

    #[test]
    fn a_token_usage_notification_reports_the_last_request_as_an_addition() {
        let event = decode_event(&token_usage_notification(), None).unwrap();
        assert_eq!(event.kind, "thread/tokenUsage/updated");
        assert_eq!(
            event.usage,
            Some(TokenUsage {
                input: 500,
                output: 300,
                cache_read: 700,
                cache_write: 0,
                cost_micro_usd: 0,
                cumulative: false,
            })
        );
    }

    #[test]
    fn a_started_web_search_or_tool_call_names_its_tool() {
        let search = json!({
            "method": "item/started",
            "params": { "item": { "type": "webSearch", "id": "i1", "query": "rust" } }
        });
        assert_eq!(decode_event(&search, None).unwrap().tools, ["WebSearch"]);
        let call = json!({
            "method": "item/started",
            "params": { "item": { "type": "mcpToolCall", "server": "docs", "tool": "fetch_page" } }
        });
        assert_eq!(decode_event(&call, None).unwrap().tools, ["fetch_page"]);
        let command = json!({
            "method": "item/started",
            "params": { "item": { "type": "commandExecution", "command": "ls" } }
        });
        assert!(decode_event(&command, None).unwrap().tools.is_empty());
    }

    #[test]
    fn other_notifications_and_a_total_without_last_carry_no_usage() {
        let delta = json!({ "method": "item/agentMessage/delta", "params": { "delta": "hi" } });
        assert_eq!(decode_event(&delta, None).unwrap().usage, None);
        let total_only = json!({
            "method": "thread/tokenUsage/updated",
            "params": { "tokenUsage": { "total": { "totalTokens": 1 } } }
        });
        assert_eq!(decode_event(&total_only, None).unwrap().usage, None);
    }
}
