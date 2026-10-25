//! Claude Code's stream-json implementation of AI6's AgentHandler contract.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

ai6::guatiao::providers!(ClaudeHandler);

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ai6::directorates::m::identity::SessionId;
use ai6::directorates::q::terminal::{AgentTerminal, READ_TIMED_OUT, Spec, TerminalSession};
use ai6::directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentMessage, AgentResume, AgentSession, AgentStart, LIMIT_EVENT,
    TokenUsage, UsageLimit,
};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use ai6::micro_usd;
use serde::Serialize;
use serde_json::{Value, json};

/// Claude Code AgentHandler configuration.
#[derive(Debug, Clone, ToValue, FromValue, Schema)]
#[map(crate = ::ai6::guatiao)]
pub struct ClaudeConfig {
    /// Executable visible to the AI6-owned terminal environment.
    #[map(default = String::from("claude"))]
    pub executable: String,
}

/// Starts and resumes Claude Code streams for AI6 Agent Sessions.
#[derive(Provider)]
#[provider(
    AgentHandler,
    id = "claude",
    name = "Claude Code",
    crate = ::ai6::guatiao,
    config = ClaudeConfig
)]
pub struct ClaudeHandler {
    config: ClaudeConfig,
}

impl TryFrom<ClaudeConfig> for ClaudeHandler {
    type Error = ProviderError;

    fn try_from(config: ClaudeConfig) -> Result<Self, Self::Error> {
        if config.executable.trim().is_empty() {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "Claude executable is empty",
            ));
        }
        Ok(Self { config })
    }
}

impl AgentHandler for ClaudeHandler {
    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn ai6::directorates::section_00::agent::AgentSession>, ProviderError> {
        let mut argv = vec![
            self.config.executable.clone(),
            "-p".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--replay-user-messages".to_string(),
        ];
        if !request.initial_context.is_empty() {
            argv.extend([
                "--append-system-prompt".to_string(),
                request.initial_context.join("\n\n"),
            ]);
        }
        if let Some(model) = request.model.as_ref() {
            argv.extend(["--model".to_string(), model.clone()]);
        }
        if let Some(effort) = request.effort.as_ref() {
            argv.extend(["--effort".to_string(), effort.clone()]);
        }
        add_budget(&mut argv, request.max_cost_micro_usd);
        add_cables_mcp(&mut argv, &request.environment);
        let session = spawn(
            terminal,
            argv,
            &request.working_directory,
            &request.environment,
        )?;
        let operation = ClaudeSession::new(session, None);
        operation.send(&request.prompt)?;
        Ok(operation.into_object())
    }

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn ai6::directorates::section_00::agent::AgentSession>, ProviderError> {
        let expected_session_id = request.session_id.clone();
        let mut argv = vec![
            self.config.executable.clone(),
            "-p".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--replay-user-messages".to_string(),
            "--resume".to_string(),
            request.session_id.to_string(),
        ];
        if let Some(model) = request.model {
            argv.extend(["--model".to_string(), model]);
        }
        if let Some(effort) = request.effort {
            argv.extend(["--effort".to_string(), effort]);
        }
        add_budget(&mut argv, request.max_cost_micro_usd);
        add_cables_mcp(&mut argv, &request.environment);
        let session = spawn(
            terminal,
            argv,
            &request.working_directory,
            &request.environment,
        )?;
        let operation = ClaudeSession::new(session, Some(expected_session_id));
        if let Err(error) = operation.prime_session_id(std::time::Duration::from_secs(30)) {
            let _ = operation.close();
            return Err(error);
        }
        if let Some(message) = request.message {
            operation.send(&message)?;
        }
        Ok(operation.into_object())
    }
}

fn add_cables_mcp(argv: &mut Vec<String>, environment: &BTreeMap<String, String>) {
    if !environment.contains_key("AI6_CABLE_MCP_URL")
        || !environment.contains_key("AI6_CABLE_MCP_TOKEN")
    {
        return;
    }
    let config = json!({
        "mcpServers": {
            "ai6-cables": {
                "type": "http",
                "url": "${AI6_CABLE_MCP_URL}",
                "headers": {
                    "Authorization": "Bearer ${AI6_CABLE_MCP_TOKEN}"
                }
            }
        }
    });
    argv.extend(["--mcp-config".to_owned(), config.to_string()]);
}

/// Gives Claude Code its own spend ceiling for this process (print mode only).
fn add_budget(argv: &mut Vec<String>, max_cost_micro_usd: Option<u64>) {
    if let Some(micro) = max_cost_micro_usd {
        argv.extend([
            "--max-budget-usd".to_owned(),
            format!("{}.{:06}", micro / 1_000_000, micro % 1_000_000),
        ]);
    }
}

/// The usage a `result` event reports: `usage` token counts and `total_cost_usd`, both
/// running totals for the Claude Code process.
fn decode_usage(value: &Value) -> Option<TokenUsage> {
    if value.get("type").and_then(Value::as_str)? != "result" {
        return None;
    }
    let usage = value.get("usage");
    let cost = value.get("total_cost_usd").and_then(Value::as_f64);
    if usage.is_none() && cost.is_none() {
        return None;
    }
    let count = |key: &str| {
        usage
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Some(TokenUsage {
        input: count("input_tokens"),
        output: count("output_tokens"),
        cache_read: count("cache_read_input_tokens"),
        cache_write: count("cache_creation_input_tokens"),
        cost_micro_usd: cost.map_or(0, micro_usd),
        cumulative: true,
    })
}

struct ClaudeSession {
    terminal: Object<dyn TerminalSession>,
    output: Mutex<Vec<u8>>,
    events: Mutex<VecDeque<AgentEvent>>,
    session_id: Mutex<Option<SessionId>>,
    expected_session_id: Option<SessionId>,
    eof: AtomicBool,
    eof_reported: AtomicBool,
}

impl ClaudeSession {
    fn new(terminal: Object<dyn TerminalSession>, expected_session_id: Option<SessionId>) -> Self {
        Self {
            terminal,
            output: Mutex::new(Vec::new()),
            events: Mutex::new(VecDeque::new()),
            session_id: Mutex::new(None),
            expected_session_id,
            eof: AtomicBool::new(false),
            eof_reported: AtomicBool::new(false),
        }
    }

    fn send(&self, text: &str) -> Result<(), ProviderError> {
        let line = serde_json::to_vec(&ClaudeInput::user(text)).map_err(internal)?;
        self.terminal.write(&line)?;
        self.terminal.write(b"\n")
    }

    fn prime_session_id(&self, timeout: std::time::Duration) -> Result<(), ProviderError> {
        let deadline = std::time::Instant::now() + timeout;
        while self.session_id()?.is_none() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "Claude did not report a Session ID while resuming",
                ));
            }
            let event = self
                .read_event(remaining.as_millis().min(i64::MAX as u128) as i64)?
                .ok_or_else(|| {
                    refuse(
                        Status::GUATIAO_ERR_INTERNAL,
                        "Claude did not report a Session ID while resuming",
                    )
                })?;
            let ended = matches!(event.kind.as_str(), "completed" | "failed" | LIMIT_EVENT);
            lock(&self.events)?.push_back(event);
            if ended && self.session_id()?.is_none() {
                return Err(refuse(
                    Status::GUATIAO_ERR_BAD_VALUE,
                    "Claude ended the process before identifying the resumed Session",
                ));
            }
        }
        Ok(())
    }

    fn read_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        if self.eof.load(Ordering::Acquire) {
            return Ok((!self.eof_reported.swap(true, Ordering::AcqRel)).then(|| {
                event(
                    "failed",
                    Some("Claude process exited before completing the Session".to_owned()),
                )
            }));
        }
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        let line = loop {
            let mut output = lock(&self.output)?;
            if let Some(end) = output.iter().position(|byte| *byte == b'\n') {
                let remainder = output.split_off(end + 1);
                let line = std::mem::replace(&mut *output, remainder);
                break line;
            }
            drop(output);
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let mut bytes = [0_u8; 8192];
            let count = self.terminal.read(
                &mut bytes,
                remaining.as_millis().min(i64::MAX as u128) as i64,
            )?;
            if count == READ_TIMED_OUT {
                return Ok(None);
            }
            if count == 0 {
                self.eof.store(true, Ordering::Release);
                let mut output = lock(&self.output)?;
                let tail = std::mem::take(&mut *output);
                drop(output);
                if tail.is_empty() {
                    self.eof_reported.store(true, Ordering::Release);
                    return Ok(Some(event(
                        "failed",
                        Some("Claude process exited before completing the Session".to_owned()),
                    )));
                }
                break tail;
            }
            if count < 0 {
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "Claude terminal returned an invalid read count",
                ));
            }
            lock(&self.output)?.extend_from_slice(&bytes[..count as usize]);
        };
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            return Ok(Some(event(
                "output",
                Some(String::from_utf8_lossy(&line).trim().to_string()),
            )));
        };
        if let Some(id) = value.get("session_id").and_then(Value::as_str) {
            let session_id = SessionId::try_from(id.to_string())
                .map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error))?;
            if self
                .expected_session_id
                .as_ref()
                .is_some_and(|expected| expected != &session_id)
            {
                return Err(refuse(
                    Status::GUATIAO_ERR_BAD_VALUE,
                    "Claude resumed a different Session than requested",
                ));
            }
            *lock(&self.session_id)? = Some(session_id);
        }
        if let Some(event) = decode_event(&value, line.clone()) {
            return Ok(Some(event));
        }
        if value.get("type").and_then(Value::as_str) == Some("user") {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            return self.read_event(remaining.as_millis().min(i64::MAX as u128) as i64);
        }
        Ok(None)
    }
}

impl ai6::directorates::section_00::agent::AgentSession for ClaudeSession {
    fn session_id(&self) -> Result<Option<SessionId>, ProviderError> {
        Ok(lock(&self.session_id)?.clone())
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        self.send(&message.text)
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        if let Some(event) = lock(&self.events)?.pop_front() {
            return Ok(Some(event));
        }
        self.read_event(timeout_ms.max(0))
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        self.terminal.write(&[3])
    }

    fn close(&self) -> Result<(), ProviderError> {
        self.terminal.kill()
    }
}

#[derive(Serialize)]
struct ClaudeInput<'a> {
    r#type: &'static str,
    message: ClaudeMessage<'a>,
}

#[derive(Serialize)]
struct ClaudeMessage<'a> {
    role: &'static str,
    content: [ClaudeText<'a>; 1],
}

#[derive(Serialize)]
struct ClaudeText<'a> {
    r#type: &'static str,
    text: &'a str,
}

impl<'a> ClaudeInput<'a> {
    fn user(text: &'a str) -> Self {
        Self {
            r#type: "user",
            message: ClaudeMessage {
                role: "user",
                content: [ClaudeText {
                    r#type: "text",
                    text,
                }],
            },
        }
    }
}

fn spawn(
    terminal: Object<dyn AgentTerminal>,
    argv: Vec<String>,
    cwd: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<Object<dyn TerminalSession>, ProviderError> {
    let spec = Spec {
        argv,
        cwd: Some(cwd.to_string()),
        env: Some(environment.clone()),
        rows: 24,
        cols: 80,
    };
    let value = spec.to_value().map_err(internal)?;
    terminal.spawn(&value)
}

fn event(kind: &str, text: Option<String>) -> AgentEvent {
    AgentEvent {
        kind: kind.to_string(),
        text,
        data: None,
        limit: None,
        usage: None,
    }
}

/// Recognises Claude Code's refusal to continue until a quota resets.
///
/// Two stream shapes carry it: a `rate_limit_event` whose status is `rejected` (with
/// `resetsAt` in Unix seconds), and an error `result` whose text names the limit, either as
/// `... usage limit reached|<unix seconds>` or as prose with only a local clock time, which
/// leaves `resets_at` unset rather than guessing a date.
fn decode_limit(value: &Value) -> Option<(UsageLimit, Option<String>)> {
    match value.get("type").and_then(Value::as_str)? {
        "rate_limit_event" => {
            let info = value.get("rate_limit_info")?;
            if info.get("status").and_then(Value::as_str) != Some("rejected") {
                return None;
            }
            let limit = UsageLimit {
                resets_at: info
                    .get("resetsAt")
                    .and_then(Value::as_i64)
                    .map(unix_seconds),
                window: info
                    .get("rateLimitType")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            Some((limit, None))
        }
        "result" if value.get("is_error").and_then(Value::as_bool) == Some(true) => {
            let text = value.get("result").and_then(Value::as_str)?;
            let lower = text.to_lowercase();
            let limited = ["usage limit", "limit reached", "hit your limit"]
                .iter()
                .any(|phrase| lower.contains(phrase));
            if !limited {
                return None;
            }
            let resets_at = text
                .rsplit_once('|')
                .and_then(|(_, tail)| tail.trim().parse::<i64>().ok())
                .map(unix_seconds);
            let window = if lower.contains("5-hour") || lower.contains("five-hour") {
                Some("five_hour".to_owned())
            } else if lower.contains("weekly") || lower.contains("7-day") {
                Some("weekly".to_owned())
            } else {
                None
            };
            Some((UsageLimit { resets_at, window }, Some(text.to_owned())))
        }
        _ => None,
    }
}

/// A Unix timestamp that may be in seconds or milliseconds.
fn unix_seconds(value: i64) -> i64 {
    if value > 100_000_000_000 {
        value / 1000
    } else {
        value
    }
}

fn decode_event(value: &Value, line: Vec<u8>) -> Option<AgentEvent> {
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("event");
    if event_type == "user" {
        return None;
    }
    if let Some((limit, text)) = decode_limit(value) {
        return Some(AgentEvent {
            kind: LIMIT_EVENT.to_owned(),
            text,
            data: Some(line),
            limit: Some(limit),
            usage: decode_usage(value),
        });
    }
    let kind = match event_type {
        "result" => "completed",
        "error" => "failed",
        "assistant" | "stream_event" => "text",
        other => other,
    };
    let text = value
        .pointer("/result")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .pointer("/message/content/0/text")
                .and_then(Value::as_str)
        })
        .or_else(|| value.pointer("/event/delta/text").and_then(Value::as_str))
        .map(str::to_string);
    Some(AgentEvent {
        kind: kind.to_string(),
        text,
        data: Some(line),
        limit: None,
        usage: decode_usage(value),
    })
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
    use std::collections::VecDeque;
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
                .args(["--exact", "tests::fake_claude_child", "--nocapture"])
                .env("AI6_FAKE_CLAUDE_CHILD", "1")
                .envs(spec.env.unwrap_or_default())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(internal)?;
            let stdin = child.stdin.take().ok_or_else(|| {
                refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "fake Claude stdin is unavailable",
                )
            })?;
            let stdout = child.stdout.take().ok_or_else(|| {
                refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "fake Claude stdout is unavailable",
                )
            })?;
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || forward_json_lines(stdout, sender));

            Ok(ProcessTerminalSession {
                child: Mutex::new(child),
                stdin: Mutex::new(stdin),
                receiver: Mutex::new(receiver),
                pending: Mutex::new(VecDeque::new()),
            }
            .into_object())
        }
    }

    fn forward_json_lines(mut stdout: impl Read, sender: mpsc::Sender<Vec<u8>>) {
        let mut reader = BufReader::new(&mut stdout);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
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
        pending: Mutex<VecDeque<u8>>,
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
    fn fake_claude_child() {
        if std::env::var_os("AI6_FAKE_CLAUDE_CHILD").is_none() {
            return;
        }
        let session_id = std::env::var("CLAUDE_FIXTURE_SESSION_ID").unwrap();
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        writeln!(
            stdout,
            "{}",
            json!({"type":"system","subtype":"init","session_id":session_id})
        )
        .unwrap();
        stdout.flush().unwrap();

        for line in std::io::stdin().lock().lines() {
            let message: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let Some(text) = message
                .pointer("/message/content/0/text")
                .and_then(Value::as_str)
            else {
                continue;
            };
            writeln!(
                stdout,
                "{}",
                json!({"type":"assistant","session_id":session_id,"message":{"content":[{"type":"text","text":text}]}})
            )
            .unwrap();
            writeln!(
                stdout,
                "{}",
                json!({"type":"result","session_id":session_id,"result":text})
            )
            .unwrap();
            stdout.flush().unwrap();
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
        panic!("fake Claude did not complete the provider Session")
    }

    #[test]
    fn process_session_restarts_and_resumes_with_the_same_provider_id() {
        let terminal = ProcessTerminal::new();
        let session_id = SessionId::try_new("claude-process-session-42").unwrap();
        let environment = BTreeMap::from([(
            "CLAUDE_FIXTURE_SESSION_ID".to_owned(),
            session_id.to_string(),
        )]);
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-process-fixture".to_owned(),
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
                    model: Some("sonnet".to_owned()),
                    effort: Some("high".to_owned()),
                    max_cost_micro_usd: None,
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        let first_events = read_to_completion(&started);
        assert_eq!(started.session_id().unwrap(), Some(session_id.clone()));
        assert!(first_events.iter().any(|event| {
            event.kind == "completed" && event.text.as_deref() == Some("first process turn")
        }));
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
                    model: Some("sonnet".to_owned()),
                    effort: Some("high".to_owned()),
                    max_cost_micro_usd: None,
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        assert_eq!(resumed.session_id().unwrap(), Some(session_id.clone()));
        let resumed_events = read_to_completion(&resumed);
        assert!(resumed_events.iter().any(|event| {
            event.kind == "completed" && event.text.as_deref() == Some("resumed process turn")
        }));
        resumed.close().unwrap();

        let specs = terminal.specs.lock().unwrap();
        assert_eq!(specs.len(), 2);
        assert!(
            specs[0]
                .argv
                .windows(2)
                .any(|pair| { pair == ["--append-system-prompt", "context before the prompt"] })
        );
        assert!(
            specs[1]
                .argv
                .windows(2)
                .any(|pair| { pair == ["--resume", session_id.as_str()] })
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
            }
            .into_object())
        }
    }

    struct ResumeTerminalSession {
        input: Mutex<Vec<u8>>,
        writes: Arc<Mutex<Vec<u8>>>,
    }

    impl TerminalSession for ResumeTerminalSession {
        fn pid(&self) -> i64 {
            7
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
    fn resume_uses_the_provider_session_and_stream_json_message() {
        let session_id = SessionId::try_new("claude-session-42").unwrap();
        let spec = Arc::new(Mutex::new(None));
        let writes = Arc::new(Mutex::new(Vec::new()));
        let terminal = ResumeTerminal {
            spec: spec.clone(),
            input:
                b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"claude-session-42\"}\n"
                    .to_vec(),
            writes: writes.clone(),
        };
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-test".to_owned(),
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
                    model: Some("sonnet".to_owned()),
                    effort: Some("high".to_owned()),
                    max_cost_micro_usd: None,
                },
                terminal.into_object(),
            )
            .unwrap();

        assert_eq!(session.session_id().unwrap(), Some(session_id.clone()));
        let event = session.next_event(50).unwrap().unwrap();
        assert_eq!(event.kind, "system");
        assert_eq!(session.session_id().unwrap(), Some(session_id));

        let spec = spec.lock().unwrap().clone().unwrap();
        assert!(
            spec.argv
                .windows(2)
                .any(|pair| pair == ["--resume", "claude-session-42"])
        );
        assert!(
            spec.argv
                .windows(2)
                .any(|pair| pair == ["--model", "sonnet"])
        );
        assert!(
            spec.argv
                .windows(2)
                .any(|pair| pair == ["--effort", "high"])
        );
        assert_eq!(spec.cwd.as_deref(), Some("C:\\station\\mission-42"));
        assert_eq!(
            spec.env.as_ref().unwrap().get("TOKEN").map(String::as_str),
            Some("fresh")
        );

        let written = String::from_utf8(writes.lock().unwrap().clone()).unwrap();
        let message: Value = serde_json::from_str(written.trim()).unwrap();
        assert_eq!(message["type"], "user");
        assert_eq!(
            message["message"]["content"][0]["text"],
            "continue the same Session"
        );
    }

    #[test]
    fn resume_rejects_a_different_session_reported_by_claude() {
        let terminal = ResumeTerminal {
            spec: Arc::new(Mutex::new(None)),
            input:
                b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"different-session\"}\n"
                    .to_vec(),
            writes: Arc::new(Mutex::new(Vec::new())),
        };
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-test".to_owned(),
        })
        .unwrap();
        let error = match handler.resume(
            AgentResume {
                mission_id: "mission-42".parse().unwrap(),
                session_id: SessionId::try_new("claude-session-42").unwrap(),
                message: None,
                working_directory: "C:\\station\\mission-42".to_owned(),
                environment: BTreeMap::new(),
                model: None,
                effort: None,
                max_cost_micro_usd: None,
            },
            terminal.into_object(),
        ) {
            Ok(_) => panic!("resume should reject a different provider Session ID"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("different Session"));
    }

    #[test]
    fn input_fixture_keeps_the_provider_stream_json_message_shape() {
        let encoded = serde_json::to_value(ClaudeInput::user("continue the Mission")).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{ "type": "text", "text": "continue the Mission" }]
                }
            })
        );
    }

    #[test]
    fn result_fixture_maps_to_a_completed_event() {
        let value = serde_json::json!({
            "type": "result",
            "session_id": "claude-session-7",
            "result": "Mission work finished"
        });
        let event = decode_event(&value, br#"{"type":"result"}"#.to_vec()).unwrap();
        assert_eq!(event.kind, "completed");
        assert_eq!(event.text.as_deref(), Some("Mission work finished"));
        assert_eq!(value["session_id"], "claude-session-7");
    }

    #[test]
    fn a_rejected_rate_limit_event_becomes_a_limit_with_its_reset_time() {
        let value = json!({
            "type": "rate_limit_event",
            "session_id": "claude-session-7",
            "rate_limit_info": {
                "status": "rejected",
                "resetsAt": 1_790_000_000,
                "rateLimitType": "five_hour"
            }
        });
        let event = decode_event(&value, b"{}".to_vec()).unwrap();
        assert_eq!(event.kind, LIMIT_EVENT);
        assert_eq!(
            event.limit,
            Some(UsageLimit {
                resets_at: Some(1_790_000_000),
                window: Some("five_hour".to_owned()),
            })
        );
    }

    #[test]
    fn an_allowed_rate_limit_event_is_not_a_limit() {
        let value = json!({
            "type": "rate_limit_event",
            "rate_limit_info": { "status": "allowed_warning", "resetsAt": 1_790_000_000 }
        });
        let event = decode_event(&value, b"{}".to_vec()).unwrap();
        assert_eq!(event.kind, "rate_limit_event");
        assert_eq!(event.limit, None);
    }

    #[test]
    fn a_usage_limit_result_carries_the_reset_time_after_the_pipe() {
        for (reset, expected) in [
            ("1790000000", 1_790_000_000),
            ("1790000000000", 1_790_000_000),
        ] {
            let value = json!({
                "type": "result",
                "is_error": true,
                "result": format!("Claude AI usage limit reached|{reset}")
            });
            let event = decode_event(&value, b"{}".to_vec()).unwrap();
            assert_eq!(
                event.kind, LIMIT_EVENT,
                "not completed: the Session did not finish"
            );
            assert_eq!(event.limit.unwrap().resets_at, Some(expected));
            assert!(event.text.unwrap().contains("usage limit reached"));
        }
    }

    #[test]
    fn a_limit_without_a_parsable_reset_time_still_parks_the_session() {
        let value = json!({
            "type": "result",
            "is_error": true,
            "result": "5-hour limit reached \u{2219} resets 3pm"
        });
        let limit = decode_event(&value, b"{}".to_vec()).unwrap().limit.unwrap();
        assert_eq!(limit.resets_at, None);
        assert_eq!(limit.window.as_deref(), Some("five_hour"));
    }

    #[test]
    fn an_ordinary_error_result_is_not_mistaken_for_a_limit() {
        let value = json!({
            "type": "result",
            "is_error": true,
            "result": "Tool call failed"
        });
        let event = decode_event(&value, b"{}".to_vec()).unwrap();
        assert_eq!(event.kind, "completed");
        assert_eq!(event.limit, None);
    }

    #[test]
    fn terminal_eof_emits_one_failed_event_instead_of_polling_forever() {
        let session = ClaudeSession::new(EofTerminalSession.into_object(), None);

        let event = session.next_event(100).unwrap().unwrap();
        assert_eq!(event.kind, "failed");
        assert_eq!(session.next_event(100).unwrap(), None);
    }

    #[test]
    fn cable_mcp_config_uses_environment_placeholders_for_auth() {
        let environment = BTreeMap::from([
            (
                "AI6_CABLE_MCP_URL".to_owned(),
                "http://127.0.0.1:43210/mcp".to_owned(),
            ),
            (
                "AI6_CABLE_MCP_TOKEN".to_owned(),
                "session-secret-token".to_owned(),
            ),
        ]);
        let mut argv = vec!["claude".to_owned()];

        add_cables_mcp(&mut argv, &environment);

        assert_eq!(argv[1], "--mcp-config");
        assert!(argv[2].contains("${AI6_CABLE_MCP_URL}"));
        assert!(argv[2].contains("${AI6_CABLE_MCP_TOKEN}"));
        assert!(!argv[2].contains("session-secret-token"));
    }

    /// The shape Claude Code 2.1 prints for a finished turn. The field names follow its
    /// stream-json output; the figures are invented.
    fn result_with_usage() -> Value {
        json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "done",
            "session_id": "claude-session-7",
            "total_cost_usd": 0.0123456,
            "usage": {
                "input_tokens": 12,
                "output_tokens": 345,
                "cache_creation_input_tokens": 6789,
                "cache_read_input_tokens": 101112
            }
        })
    }

    #[test]
    fn a_result_reports_tokens_and_cost_as_running_totals() {
        let event = decode_event(&result_with_usage(), b"{}".to_vec()).unwrap();
        assert_eq!(event.kind, "completed");
        assert_eq!(
            event.usage,
            Some(TokenUsage {
                input: 12,
                output: 345,
                cache_read: 101_112,
                cache_write: 6_789,
                cost_micro_usd: 12_346,
                cumulative: true,
            })
        );
    }

    #[test]
    fn only_a_result_carries_usage_and_a_cost_alone_is_enough() {
        let assistant = json!({ "type": "assistant", "usage": { "input_tokens": 5 } });
        assert_eq!(
            decode_event(&assistant, b"{}".to_vec()).unwrap().usage,
            None
        );
        let bare = json!({ "type": "result", "result": "ok" });
        assert_eq!(decode_event(&bare, b"{}".to_vec()).unwrap().usage, None);
        let cost_only = json!({ "type": "result", "total_cost_usd": 1.5 });
        let usage = decode_event(&cost_only, b"{}".to_vec())
            .unwrap()
            .usage
            .unwrap();
        assert_eq!((usage.input, usage.cost_micro_usd), (0, 1_500_000));
    }

    #[test]
    fn a_limit_error_result_keeps_its_usage() {
        let mut value = result_with_usage();
        value["is_error"] = json!(true);
        value["result"] = json!("Claude AI usage limit reached|1790000000");
        let event = decode_event(&value, b"{}".to_vec()).unwrap();
        assert_eq!(event.kind, LIMIT_EVENT);
        assert_eq!(event.usage.unwrap().output, 345);
    }

    #[test]
    fn a_spend_ceiling_is_passed_as_max_budget_usd() {
        let mut argv = vec!["claude".to_owned()];
        add_budget(&mut argv, Some(2_500_007));
        assert_eq!(argv[1..], ["--max-budget-usd", "2.500007"]);
        let mut none = vec!["claude".to_owned()];
        add_budget(&mut none, None);
        assert_eq!(none.len(), 1);
    }
}
