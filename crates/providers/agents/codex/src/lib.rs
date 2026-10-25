//! Codex app-server JSON-RPC implementation of AI6's AgentHandler contract.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

ai6::guatiao::providers!(CodexHandler);

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ai6::directorates::m::identity::SessionId;
use ai6::directorates::q::terminal::{AgentTerminal, READ_TIMED_OUT, Spec, TerminalSession};
use ai6::directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentMessage, AgentResume, AgentSession, AgentStart,
};
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
        let process = spawn(
            terminal,
            &self.config.executable,
            &request.working_directory,
            &request.environment,
        )?;
        let mut session = CodexSession::new(process, request.effort);
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

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn ai6::directorates::section_00::agent::AgentSession>, ProviderError> {
        let process = spawn(
            terminal,
            &self.config.executable,
            &request.working_directory,
            &request.environment,
        )?;
        let mut session = CodexSession::new(process, request.effort);
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

struct CodexSession {
    terminal: Object<dyn TerminalSession>,
    output: Mutex<Vec<u8>>,
    events: Mutex<VecDeque<AgentEvent>>,
    thread_id: Option<SessionId>,
    request_id: AtomicU64,
    effort: Option<String>,
}

impl CodexSession {
    fn new(terminal: Object<dyn TerminalSession>, effort: Option<String>) -> Self {
        Self {
            terminal,
            output: Mutex::new(Vec::new()),
            events: Mutex::new(VecDeque::new()),
            thread_id: None,
            request_id: AtomicU64::new(0),
            effort,
        }
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
            let Some(line) = self.read_line(remaining)? else {
                continue;
            };
            let message: Value = serde_json::from_slice(&line).map_err(internal)?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if message.get("method").and_then(Value::as_str) == Some(method) {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    return Err(refuse(Status::GUATIAO_ERR_INTERNAL, error));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            if message.get("method").is_none() {
                continue;
            }
            self.collect_event(message)?;
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

    fn read_line(&self, timeout: Duration) -> Result<Option<Vec<u8>>, ProviderError> {
        let deadline = Instant::now() + timeout;
        loop {
            let mut output = lock(&self.output)?;
            if let Some(end) = output.iter().position(|byte| *byte == b'\n') {
                let remainder = output.split_off(end + 1);
                let line = std::mem::replace(&mut *output, remainder);
                return Ok(Some(line));
            }
            drop(output);
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
                return Ok(None);
            }
            if count == 0 {
                let mut output = lock(&self.output)?;
                if !output.is_empty() {
                    return Ok(Some(std::mem::take(&mut *output)));
                }
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "Codex app-server closed its terminal",
                ));
            }
            if count < 0 {
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "Codex terminal returned an invalid read count",
                ));
            }
            lock(&self.output)?.extend_from_slice(&bytes[..count as usize]);
        }
    }

    fn collect_event(&self, message: Value) -> Result<(), ProviderError> {
        if let Some(event) = decode_event(&message) {
            lock(&self.events)?.push_back(event);
        }
        Ok(())
    }
}

fn decode_event(message: &Value) -> Option<AgentEvent> {
    let method = message.get("method")?.as_str().unwrap_or("event");
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
        let Some(line) = self.read_line(Duration::from_millis(timeout_ms.max(0) as u64))? else {
            return Ok(None);
        };
        let message: Value = serde_json::from_slice(&line).map_err(internal)?;
        if message.get("method").is_none() {
            return Ok(None);
        }
        self.collect_event(message)?;
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
        self.terminal.kill()
    }
}

fn spawn(
    terminal: Object<dyn AgentTerminal>,
    executable: &str,
    cwd: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<Object<dyn TerminalSession>, ProviderError> {
    let argv = app_server_argv(executable, environment);
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

fn app_server_argv(
    executable: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    let mut argv = vec![executable.to_string(), "app-server".to_string()];
    if let Some(url) = environment.get("AI6_CABLE_MCP_URL")
        && environment.contains_key("AI6_CABLE_MCP_TOKEN")
    {
        argv.extend([
            "-c".to_owned(),
            format!("mcp_servers.ai6-cables.url=\"{url}\""),
            "-c".to_owned(),
            "mcp_servers.ai6-cables.bearer_token_env_var=\"AI6_CABLE_MCP_TOKEN\"".to_owned(),
        ]);
    }
    argv
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
    use std::sync::atomic::AtomicBool;
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
            thread::spawn(move || forward_json_lines(stdout, sender));

            Ok(ProcessTerminalSession {
                child: Mutex::new(child),
                stdin: Mutex::new(stdin),
                receiver: Mutex::new(receiver),
                pending: Mutex::new(std::collections::VecDeque::new()),
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
        let thread_id = std::env::var("CODEX_FIXTURE_THREAD_ID").unwrap();
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        for line in std::io::stdin().lock().lines() {
            let message: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let method = message.get("method").and_then(Value::as_str).unwrap_or("");
            if let Some(id) = message.get("id") {
                match method {
                    "initialize" => {
                        writeln!(stdout, "{}", json!({"id":id,"result":{}})).unwrap();
                        stdout.flush().unwrap();
                    }
                    "thread/start" | "thread/resume" => {
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
        assert_eq!(specs[0].argv, ["codex-process-fixture", "app-server"]);
        assert_eq!(specs[1].argv, ["codex-process-fixture", "app-server"]);
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
                },
                terminal.into_object(),
            )
            .unwrap();

        assert_eq!(session.session_id().unwrap(), Some(session_id));

        let spec = spec.lock().unwrap().clone().unwrap();
        assert_eq!(spec.argv, ["codex-test", "app-server"]);
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
        let event = decode_event(&message).unwrap();
        assert_eq!(event.kind, "text");
        assert_eq!(event.text.as_deref(), Some("hello from Codex"));
        assert_eq!(
            serde_json::from_slice::<Value>(event.data.as_deref().unwrap()).unwrap(),
            message
        );
    }

    #[test]
    fn turn_completed_fixture_maps_to_completed() {
        let event = decode_event(&json!({ "method": "turn/completed", "params": {} })).unwrap();
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
}
