//! One running agy: its stream decoded into AI6's events, and what a Session's clearance needs
//! watched there.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ai6::directorates::m::identity::SessionId;
use ai6::directorates::q::terminal::{
    AgentTerminal, ExitInfo, JsonLines, READ_TIMED_OUT, StreamLine, TerminalSession, exit_code_text,
};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Status};
use ai6::{AgentEvent, AgentMessage, DENIED_EVENT, LIMIT_EVENT, TokenUsage, UsageLimit};
use serde_json::{Value, json};

use crate::files::SessionFiles;
use crate::{AntigravityConfig, refuse, spawn};

/// What a restricted (answer-only or assistant) Session may call.
#[derive(Debug, Clone)]
pub(crate) struct Restriction {
    pub(crate) servers: Vec<String>,
    pub(crate) tools: Vec<&'static str>,
}

/// How to run agy again after an interrupt ended it. It has no way to stop a turn and go on: on a
/// pipe a signal cannot be sent, and a `control_request` line ends the session with exit code 2.
pub(crate) struct Respawn {
    pub(crate) terminal: Object<dyn AgentTerminal>,
    pub(crate) argv: Vec<String>,
    pub(crate) working_directory: String,
    pub(crate) environment: BTreeMap<String, String>,
}

/// The `error.message` of a tool step that the permission check or a hook refused, as agy 1.4.12
/// words each: an allow rule's absence, a deny rule, a hook's denial and a hook that failed, which
/// also stops the call. `unknown tool` is not here: that tool was never offered.
fn is_denial(message: &str) -> bool {
    message.starts_with("permission check failed")
        || message.starts_with("tool call denied by pre-tool hook")
        || message.starts_with("JSON hook ")
}

pub(crate) struct AntigravitySession {
    terminal: Mutex<Arc<Object<dyn TerminalSession>>>,
    output: Mutex<JsonLines>,
    session_id: Mutex<Option<SessionId>>,
    expected_session_id: Mutex<Option<SessionId>>,
    restricted: Option<Restriction>,
    limit_phrases: Vec<String>,
    /// Messages waiting for the `init` line that says which conversation this is.
    held: Mutex<Vec<String>>,
    /// Whether an `init` line has been read from the running agy.
    initialized: AtomicBool,
    /// Events owed after the one just returned.
    pending: Mutex<VecDeque<AgentEvent>>,
    /// The text so far of the response step being read.
    response: Mutex<String>,
    /// The step of the last tool reported, so that each call is reported once.
    last_tool_step: Mutex<Option<i64>>,
    /// The calls refused in this turn, by the name [`tool_name`] gives.
    denied: Mutex<Vec<String>>,
    turn_running: AtomicBool,
    interrupting: AtomicBool,
    /// Set when agy's end has been accounted for: nothing more is reported until it runs again.
    ended: AtomicBool,
    eof: AtomicBool,
    /// Set once a restricted Session called a tool it may not, and was ended.
    tripped: AtomicBool,
    tail: Mutex<VecDeque<String>>,
    respawn: Respawn,
    files: Mutex<Option<SessionFiles>>,
    json_line_bytes: u64,
    csi_bytes: u64,
    tail_lines: usize,
    tail_line_bytes: usize,
    close_wait_ms: i64,
    exit_wait_ms: i64,
}

fn lines(json_line_bytes: u64, csi_bytes: u64) -> JsonLines {
    let mut limits = ai6::station::limits::TerminalOutputLimits::default();
    limits.json_line_bytes = ai6::Limit::at(json_line_bytes);
    limits.csi_bytes = ai6::Limit::at(csi_bytes);
    JsonLines::new(&limits)
}

impl AntigravitySession {
    pub(crate) fn new(
        terminal: Object<dyn TerminalSession>,
        expected_session_id: Option<SessionId>,
        restricted: Option<Restriction>,
        config: &AntigravityConfig,
        limit_phrases: Vec<String>,
        files: SessionFiles,
        respawn: Respawn,
    ) -> Self {
        Self {
            terminal: Mutex::new(Arc::new(terminal)),
            output: Mutex::new(lines(config.json_line_bytes, config.csi_bytes)),
            session_id: Mutex::new(None),
            expected_session_id: Mutex::new(expected_session_id),
            restricted,
            limit_phrases,
            held: Mutex::new(Vec::new()),
            initialized: AtomicBool::new(false),
            pending: Mutex::new(VecDeque::new()),
            response: Mutex::new(String::new()),
            last_tool_step: Mutex::new(None),
            denied: Mutex::new(Vec::new()),
            turn_running: AtomicBool::new(false),
            interrupting: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            eof: AtomicBool::new(false),
            tripped: AtomicBool::new(false),
            tail: Mutex::new(VecDeque::new()),
            respawn,
            files: Mutex::new(Some(files)),
            json_line_bytes: config.json_line_bytes,
            csi_bytes: config.csi_bytes,
            tail_lines: usize::try_from(config.tail_lines).unwrap_or(usize::MAX),
            tail_line_bytes: usize::try_from(config.tail_line_bytes).unwrap_or(usize::MAX),
            close_wait_ms: i64::try_from(config.close_wait_ms).unwrap_or(i64::MAX),
            exit_wait_ms: i64::try_from(config.exit_wait_ms).unwrap_or(i64::MAX),
        }
    }

    fn current(&self) -> Result<Arc<Object<dyn TerminalSession>>, ProviderError> {
        Ok(lock(&self.terminal)?.clone())
    }

    /// Writes one message as one line in a single write.
    pub(crate) fn send(&self, text: &str) -> Result<(), ProviderError> {
        let mut line = serde_json::to_vec(&json!({
            "event": "user",
            "message": { "content": text },
        }))
        .map_err(crate::internal)?;
        line.push(b'\n');
        self.turn_running.store(true, Ordering::Release);
        self.current()?.write(&line)
    }

    /// Keeps a message until the `init` line has been read.
    pub(crate) fn hold(&self, text: String) -> Result<(), ProviderError> {
        lock(&self.held)?.push(text);
        Ok(())
    }

    /// Runs agy again on the conversation an interrupt or a failed turn ended.
    fn run_again(&self) -> Result<(), ProviderError> {
        let _ = self.current()?.kill();
        let id = lock(&self.session_id)?.clone().ok_or_else(|| {
            refuse(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "agy was interrupted before it named its conversation, so it cannot go on",
            )
        })?;
        let mut argv: Vec<String> = self
            .respawn
            .argv
            .iter()
            .filter(|argument| {
                !argument.starts_with("--agent=") && !argument.starts_with("--conversation=")
            })
            .cloned()
            .collect();
        argv.push(format!("--conversation={id}"));
        let terminal = spawn(
            &self.respawn.terminal,
            argv,
            &self.respawn.working_directory,
            &self.respawn.environment,
        )?;
        *lock(&self.terminal)? = Arc::new(terminal);
        *lock(&self.output)? = lines(self.json_line_bytes, self.csi_bytes);
        *lock(&self.expected_session_id)? = Some(id);
        self.initialized.store(false, Ordering::Release);
        self.ended.store(false, Ordering::Release);
        self.eof.store(false, Ordering::Release);
        Ok(())
    }

    fn exited(&self, timeout_ms: i64) -> Result<Option<ExitInfo>, ProviderError> {
        let status = match self.current()?.exit_status(timeout_ms) {
            Ok(status) => status,
            Err(_) => return Ok(None),
        };
        Ok(ExitInfo::from_value(&status)
            .ok()
            .filter(|exit| !exit.running))
    }

    /// Why the Session ended without a `result`: the exit status and what agy last wrote.
    fn exit_reason(&self) -> Result<String, ProviderError> {
        let how = match self.exited(self.exit_wait_ms)? {
            Some(ExitInfo {
                code: Some(code), ..
            }) => format!("exited with code {}", exit_code_text(code)),
            Some(ExitInfo {
                signal: Some(signal),
                ..
            }) => format!("was ended by signal {signal}"),
            Some(_) => "exited".to_owned(),
            None => "closed its terminal".to_owned(),
        };
        let mut reason = format!("The Antigravity CLI {how} before completing the Session");
        let tail = lock(&self.tail)?;
        if !tail.is_empty() {
            reason.push_str("; its last output:\n");
            reason.push_str(&tail.iter().cloned().collect::<Vec<_>>().join("\n"));
        }
        Ok(reason)
    }

    fn remember_output(&self, line: &str) -> Result<(), ProviderError> {
        let mut tail = lock(&self.tail)?;
        if tail.len() >= self.tail_lines {
            tail.pop_front();
        }
        let mut end = line.len().min(self.tail_line_bytes);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        tail.push_back(line[..end].to_owned());
        Ok(())
    }

    fn is_limit(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.limit_phrases
            .iter()
            .any(|phrase| text.contains(&phrase.to_lowercase()))
    }

    /// What the end of agy's output means: the turn an interrupt ended, a limit its last words
    /// name, or a failure.
    fn on_end(&self) -> Result<AgentEvent, ProviderError> {
        self.ended.store(true, Ordering::Release);
        self.turn_running.store(false, Ordering::Release);
        if self.interrupting.swap(false, Ordering::AcqRel) {
            return Ok(event(
                "completed",
                Some("the turn was interrupted".to_owned()),
            ));
        }
        let reason = self.exit_reason()?;
        eprintln!("ai6: {reason}");
        if self.is_limit(&reason) {
            let mut limited = event(LIMIT_EVENT, Some(reason));
            limited.limit = Some(UsageLimit {
                resets_at: None,
                window: None,
            });
            return Ok(limited);
        }
        Ok(event("failed", Some(reason)))
    }

    fn read_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        if self.tripped.load(Ordering::Acquire) {
            return Ok(None);
        }
        if let Some(event) = lock(&self.pending)?.pop_front() {
            return Ok(Some(event));
        }
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(0));
        loop {
            let at_end = self.eof.load(Ordering::Acquire);
            let next = {
                let mut output = lock(&self.output)?;
                if at_end {
                    output.finish()
                } else {
                    output.next_line()
                }
            };
            match next {
                Some(StreamLine::Text(text)) => {
                    self.remember_output(&text)?;
                    return Ok(Some(event("output", Some(text))));
                }
                Some(StreamLine::Json { value, text, .. }) => {
                    if let Some(found) = self.on_line(&value, text)? {
                        return Ok(Some(found));
                    }
                    continue;
                }
                None => {}
            }
            if at_end {
                if self.ended.load(Ordering::Acquire) {
                    return Ok(None);
                }
                return self.on_end().map(Some);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let mut bytes = [0_u8; 8192];
            let count = self.current()?.read(
                &mut bytes,
                i64::try_from(remaining.as_millis()).unwrap_or(i64::MAX),
            )?;
            if count == READ_TIMED_OUT {
                // A terminal may keep its output open after the harness exits.
                if self.exited(0)?.is_some() {
                    self.eof.store(true, Ordering::Release);
                    continue;
                }
                return Ok(None);
            }
            if count == 0 {
                self.eof.store(true, Ordering::Release);
                continue;
            }
            let count = usize::try_from(count).map_err(|_| {
                refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "the agy terminal returned an invalid read count",
                )
            })?;
            lock(&self.output)?.push(&bytes[..count]);
        }
    }

    /// One JSON line of the stream; `None` for one that makes no event.
    fn on_line(&self, value: &Value, line: Vec<u8>) -> Result<Option<AgentEvent>, ProviderError> {
        match value.get("event").and_then(Value::as_str) {
            Some("init") => self.on_init(value, line).map(Some),
            Some("step_update") => self.on_step(value, line),
            Some("result") => self.on_result(value, line).map(Some),
            Some(other) => {
                let mut passed = event(other, None);
                passed.data = Some(line);
                Ok(Some(passed))
            }
            None => {
                let mut passed = event("output", String::from_utf8(line.clone()).ok());
                passed.data = Some(line);
                Ok(Some(passed))
            }
        }
    }

    /// Learns the conversation from its stream. A resume that names another conversation has
    /// started a new one, with none of the history asked for, so nothing is sent to it.
    fn on_init(&self, value: &Value, line: Vec<u8>) -> Result<AgentEvent, ProviderError> {
        self.check_conversation(value.get("conversation_id"))?;
        self.initialized.store(true, Ordering::Release);
        let held = std::mem::take(&mut *lock(&self.held)?);
        for message in &held {
            self.send(message)?;
        }
        let mut init = event("init", None);
        init.data = Some(line);
        Ok(init)
    }

    fn check_conversation(&self, id: Option<&Value>) -> Result<(), ProviderError> {
        let Some(id) = id.and_then(Value::as_str) else {
            return Ok(());
        };
        let session_id = SessionId::try_from(id.to_owned())
            .map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error))?;
        let different = lock(&self.expected_session_id)?
            .as_ref()
            .is_some_and(|expected| expected != &session_id);
        // Kept even when it is the wrong one: `session_id` is how the Station learns that.
        *lock(&self.session_id)? = Some(session_id);
        if different {
            lock(&self.held)?.clear();
            let _ = self.current()?.kill();
            self.ended.store(true, Ordering::Release);
            self.tripped.store(true, Ordering::Release);
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "agy resumed a different Session than requested",
            ));
        }
        Ok(())
    }

    fn on_step(&self, value: &Value, line: Vec<u8>) -> Result<Option<AgentEvent>, ProviderError> {
        let Some(step) = value.get("step_update") else {
            return Ok(None);
        };
        let state = step.get("state").and_then(Value::as_str).unwrap_or("");
        match step.get("step_type").and_then(Value::as_str) {
            Some("agent_response") => {
                let mut response = lock(&self.response)?;
                if let Some(delta) = step.get("text_delta").and_then(Value::as_str) {
                    response.push_str(delta);
                }
                if state == "ACTIVE" {
                    return Ok(None);
                }
                let text = std::mem::take(&mut *response);
                let usage = decode_usage(step.get("usage"));
                if text.trim().is_empty() && usage.is_none() {
                    return Ok(None);
                }
                let mut said = event("text", Some(text).filter(|text| !text.trim().is_empty()));
                said.usage = usage;
                said.data = Some(line);
                Ok(Some(said))
            }
            Some("tool") => self.on_tool_step(step, state, line),
            _ => Ok(None),
        }
    }

    /// A tool step: the first update of each reports the call; an `ERROR` that says the call was
    /// refused goes on the turn's list.
    fn on_tool_step(
        &self,
        step: &Value,
        state: &str,
        line: Vec<u8>,
    ) -> Result<Option<AgentEvent>, ProviderError> {
        let name = tool_name(step);
        if state == "ERROR"
            && step
                .pointer("/tool_info/error/message")
                .and_then(Value::as_str)
                .is_some_and(is_denial)
        {
            let mut denied = lock(&self.denied)?;
            if !denied.contains(&name) {
                denied.push(name.clone());
            }
        }
        let index = step.get("step_index").and_then(Value::as_i64);
        {
            let mut last = lock(&self.last_tool_step)?;
            if index.is_some() && *last == index {
                return Ok(None);
            }
            *last = index;
        }
        if let Some(ended) = self.tripped_by(step, &name)? {
            return Ok(Some(ended));
        }
        let mut called = event("tool", None);
        called.tools = vec![name];
        called.data = Some(line);
        Ok(Some(called))
    }

    /// The `failed` event that ends a restricted Session whose step calls what it may not, after
    /// closing it. The agent's tool list and the rules are meant to make this impossible; this
    /// backs them up.
    fn tripped_by(&self, step: &Value, name: &str) -> Result<Option<AgentEvent>, ProviderError> {
        let Some(restricted) = &self.restricted else {
            return Ok(None);
        };
        let tool = step
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let allowed = restricted.tools.contains(&tool)
            && match server_of(step) {
                Some(server) => restricted.servers.iter().any(|allowed| allowed == server),
                None => tool != "call_mcp_tool",
            };
        if allowed {
            return Ok(None);
        }
        let reason = format!(
            "the restricted Antigravity CLI Session called {name}, which it may not use; the \
             Session was ended"
        );
        eprintln!("ai6: {reason}");
        let _ = self.current()?.kill();
        self.tripped.store(true, Ordering::Release);
        Ok(Some(event("failed", Some(reason))))
    }

    /// A turn's end. agy words a turn it could not do as a `result` with `status` `ERROR` and
    /// `error`, and exits with code 3; a soft-denied tool leaves `status` `SUCCESS`, exit code 0 and
    /// `denied_actions`, which a deny rule's refusal is not listed in, so the turn's own steps are
    /// what the denial reports. `finished_reason` (from agy 1.4.11) marks a cut-off answer.
    fn on_result(&self, value: &Value, line: Vec<u8>) -> Result<AgentEvent, ProviderError> {
        let empty = json!({});
        let result = value.get("result").unwrap_or(&empty);
        self.check_conversation(result.get("conversation_id"))?;
        self.turn_running.store(false, Ordering::Release);
        *lock(&self.last_tool_step)? = None;
        lock(&self.response)?.clear();
        let status = result.get("status").and_then(Value::as_str).unwrap_or("");
        let reason = result.get("finished_reason").and_then(Value::as_str);
        let error = result.get("error").and_then(Value::as_str);
        let mut denied = std::mem::take(&mut *lock(&self.denied)?);
        if denied.is_empty() {
            denied.extend(
                result
                    .get("denied_actions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|action| action.get("display_name").and_then(Value::as_str))
                    .map(str::to_owned),
            );
        }
        let mut closing = if status == "SUCCESS" && reason.is_none() {
            let mut completed = event(
                "completed",
                result
                    .get("response")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            );
            completed.data = Some(line);
            completed
        } else {
            let text = error
                .map(str::to_owned)
                .or_else(|| reason.map(|reason| format!("the turn ended early: {reason}")))
                .unwrap_or_else(|| format!("the turn ended with status {status}"));
            self.ended.store(status == "ERROR", Ordering::Release);
            let mut closing = if self.is_limit(&text) {
                let mut limited = event(LIMIT_EVENT, Some(text));
                limited.limit = Some(UsageLimit {
                    resets_at: None,
                    window: None,
                });
                limited
            } else {
                event("failed", Some(text))
            };
            closing.data = Some(line);
            closing
        };
        if self.interrupting.swap(false, Ordering::AcqRel) {
            closing.kind = "completed".to_owned();
            closing.limit = None;
        }
        if denied.is_empty() {
            return Ok(closing);
        }
        lock(&self.pending)?.push_back(closing);
        Ok(event(DENIED_EVENT, Some(denied.join(", "))))
    }
}

impl ai6::AgentSession for AntigravitySession {
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        Ok(lock(&self.session_id)?.clone().map(String::from))
    }

    /// A message while agy runs goes to it as the next turn, once it has named its conversation.
    /// After an interrupt ended it, it is run again on the same conversation.
    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        if self.ended.load(Ordering::Acquire) && !self.tripped.load(Ordering::Acquire) {
            self.run_again()?;
            return self.hold(message.text);
        }
        if self.initialized.load(Ordering::Acquire) {
            self.send(&message.text)
        } else {
            self.hold(message.text)
        }
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        self.read_event(timeout_ms.max(0))
    }

    /// Ends agy, whose conversation stays on disk and resumes: it has no way to stop only a turn.
    /// The end of its output then reports the turn as interrupted. With no turn running it is left
    /// as it is.
    fn interrupt(&self) -> Result<(), ProviderError> {
        if !self.turn_running.load(Ordering::Acquire) {
            return Ok(());
        }
        self.interrupting.store(true, Ordering::Release);
        self.current()?.kill()
    }

    fn close(&self) -> Result<(), ProviderError> {
        let terminal = self.current()?;
        terminal.kill()?;
        // The caller may remove the working directory next; agy holds it until it has exited.
        let _ = terminal.exit_status(self.close_wait_ms);
        *lock(&self.files)? = None;
        Ok(())
    }
}

/// The name a tool call goes by in AI6's events: agy's own, or for a call through `call_mcp_tool`
/// the `mcp__<server>__<tool>` form AI6 names MCP tools by.
fn tool_name(step: &Value) -> String {
    let tool = step
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let called = step
        .pointer("/tool_info/parameters/ToolName")
        .and_then(Value::as_str);
    match (server_of(step), called) {
        (Some(server), Some(called)) => format!("mcp__{server}__{called}"),
        _ => tool.to_owned(),
    }
}

fn server_of(step: &Value) -> Option<&str> {
    step.pointer("/tool_info/parameters/ServerName")
        .and_then(Value::as_str)
}

/// A model call's usage, on the `DONE` update of its response step. `result` also has a total, but
/// that one runs across every launch of the conversation (agy 1.4.12), so the steps are what is
/// counted. `input_tokens` leaves out `cache_read_tokens`, and `output_tokens` holds the thinking.
fn decode_usage(usage: Option<&Value>) -> Option<TokenUsage> {
    let usage = usage?;
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    Some(TokenUsage {
        input: count("input_tokens"),
        output: count("output_tokens"),
        cache_read: count("cache_read_tokens"),
        cache_write: 0,
        cost_micro_usd: 0,
        cumulative: false,
    })
}

fn event(kind: &str, text: Option<String>) -> AgentEvent {
    AgentEvent {
        kind: kind.to_owned(),
        text,
        data: None,
        limit: None,
        usage: None,
        tools: Vec::new(),
        quota: Vec::new(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ProviderError> {
    mutex
        .lock()
        .map_err(|_| refuse(Status::GUATIAO_ERR_INTERNAL, "session lock was poisoned"))
}
