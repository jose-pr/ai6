//! Claude Code's stream-json implementation of AI6's AgentHandler contract.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

ai6::guatiao::providers!(ClaudeHandler);

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ai6::directorates::m::identity::SessionId;
use ai6::directorates::q::terminal::{
    AgentTerminal, ExitInfo, JsonLines, READ_TIMED_OUT, STREAM_COLS, Spec, StreamLine,
    TerminalSession, exit_code_text,
};
use ai6::directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentMessage, AgentResume, AgentSession, AgentStart, AssistantMode,
    LIMIT_EVENT, QuotaReport, TokenUsage, UsageLimit,
};
use ai6::directorates::section_00::gate::GATE_PROGRAM_ENV;
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
    /// Claude Code with `-p` and stream-json reads JSON lines from standard input, which must not
    /// be a terminal: on one it exits at once.
    fn stdio(&self) -> Result<ai6::HarnessStdio, ProviderError> {
        Ok(ai6::HarnessStdio::Json)
    }

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
        let mode = Mode {
            project_settings: request.project_settings,
            project_mcp: request.project_mcp,
            answer_only: request.answer_only,
        };
        add_settings(
            &mut argv,
            &request.environment,
            mode,
            request.assistant.as_ref(),
        )?;
        let session = spawn(
            terminal,
            argv,
            &request.working_directory,
            &harness_environment(
                &request.environment,
                request.answer_only || request.assistant.is_some(),
                request.assistant.is_some(),
            ),
        )?;
        let operation = ClaudeSession::new(session, None)
            .restricted_to(restriction(request.answer_only, request.assistant.as_ref()));
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
        // Claude Code 2.1.310 reuses the system prompt it recorded with the Session
        // (`--system-prompt-snapshot`, on by default) until the Session is compacted; after
        // that, or with the snapshot off, it builds the prompt from this launch's flags.
        if !request.initial_context.is_empty() {
            argv.extend([
                "--append-system-prompt".to_string(),
                request.initial_context.join("\n\n"),
            ]);
        }
        if let Some(model) = request.model {
            argv.extend(["--model".to_string(), model]);
        }
        if let Some(effort) = request.effort {
            argv.extend(["--effort".to_string(), effort]);
        }
        add_budget(&mut argv, request.max_cost_micro_usd);
        let mode = Mode {
            project_settings: request.project_settings,
            project_mcp: request.project_mcp,
            answer_only: request.answer_only,
        };
        add_settings(
            &mut argv,
            &request.environment,
            mode,
            request.assistant.as_ref(),
        )?;
        let session = spawn(
            terminal,
            argv,
            &request.working_directory,
            &harness_environment(
                &request.environment,
                request.answer_only || request.assistant.is_some(),
                request.assistant.is_some(),
            ),
        )?;
        let operation = ClaudeSession::new(session, Some(expected_session_id))
            .restricted_to(restriction(request.answer_only, request.assistant.as_ref()));
        // With stream-json input Claude Code writes nothing, not even its stream's
        // initialization, until it has read a message: the message goes first, and the Session
        // it reports is checked as its events are read.
        if let Some(message) = request.message
            && let Err(error) = operation.send(&message)
        {
            let _ = operation.close();
            return Err(error);
        }
        Ok(operation.into_object())
    }
}

/// Claude's own shell and file tools, turned off in remote-hands mode so that nothing the
/// harness does lands on the Station.
const OWN_TOOLS: &str = "Bash,PowerShell,Edit,Write,NotebookEdit,Read,Glob,Grep";

/// Every tool an answer-only Session is denied by name, besides `--tools=` turning the
/// built-in ones off: Claude's own tools, its web and sub-agent tools, and any MCP tool.
const ANSWER_ONLY_DENIED: &str = "Bash,PowerShell,Edit,Write,NotebookEdit,Read,Glob,Grep,\
                                  WebFetch,WebSearch,Task,Agent,TodoWrite,mcp__*";

/// Turns off Claude Code's server-side advisor tool, which the API runs outside any MCP
/// server and which neither `--restricted`, `--tools=` nor a deny rule removes
/// (code.claude.com/docs/en/advisor; read by Claude Code 2.1.310 from its environment).
const DISABLE_ADVISOR_ENV: &str = "CLAUDE_CODE_DISABLE_ADVISOR_TOOL";

/// Turns off Claude Code's auto memory, which would keep notes under the user's own
/// `~/.claude/projects/` beside the assistant's memory in her repository
/// (code.claude.com/docs/en/memory, read 2026-10-27).
const DISABLE_AUTO_MEMORY_ENV: &str = "CLAUDE_CODE_DISABLE_AUTO_MEMORY";

/// The harness's environment: `environment`, with the advisor turned off for a restricted
/// (answer-only or assistant) Session, and auto memory off for an assistant.
fn harness_environment(
    environment: &BTreeMap<String, String>,
    restricted: bool,
    assistant: bool,
) -> BTreeMap<String, String> {
    let mut environment = environment.clone();
    if restricted {
        environment.insert(DISABLE_ADVISOR_ENV.to_owned(), "1".to_owned());
    }
    if assistant {
        environment.insert(DISABLE_AUTO_MEMORY_ENV.to_owned(), "1".to_owned());
    }
    environment
}

/// What a restricted (answer-only or assistant) Session may call: its MCP servers' tools, and
/// the file tools when its mode names directories to write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Allowed {
    servers: Vec<String>,
    file_tools: bool,
}

/// What a restricted Session may call, `None` for a Session that is not restricted.
fn restriction(answer_only: bool, assistant: Option<&AssistantMode>) -> Option<Allowed> {
    match assistant {
        Some(mode) => Some(Allowed {
            servers: mode
                .mcp_servers
                .iter()
                .map(|server| server.name.clone())
                .collect(),
            file_tools: !mode.files.is_empty(),
        }),
        None => answer_only.then(Allowed::default),
    }
}

/// What AI6 decided about the Session's settings.
#[derive(Clone, Copy)]
struct Mode {
    project_settings: bool,
    project_mcp: bool,
    answer_only: bool,
}

/// Every built-in tool an assistant Session is denied by name, besides `--tools=` turning the
/// built-in ones off. Unlike [`ANSWER_ONLY_DENIED`] it leaves MCP tools alone: the assistant's
/// servers are how it works.
const ASSISTANT_DENIED: &str = "Bash,PowerShell,Edit,Write,NotebookEdit,Read,Glob,Grep,\
                                WebFetch,WebSearch,Task,Agent,TodoWrite,Skill";

/// The built-in tools of an assistant whose mode names directories to write.
const ASSISTANT_FILE_TOOLS: &str = "Read,Write,Edit,Glob,Grep";

/// [`ASSISTANT_DENIED`] less [`ASSISTANT_FILE_TOOLS`]: what an assistant with file tools is
/// still denied by name.
/// The rules after the names deny writing any path with a name starting with `.` (`.git`,
/// `.gitattributes`, `.claude`), whatever the allow rules say. Measured with Claude Code
/// 2.1.310 on Windows against a loopback stand-in for the API: Write and Edit to `.git/config`,
/// `.GIT/config`, `.git./config`, `PERSONAL\.git\config`, `.gitattributes`, `notes/.hidden`
/// and `scratch/.x` were refused, and so were writes through 8.3 short names (`GIT~1\config`,
/// `GITATT~1`), even after a read.
const ASSISTANT_FILES_DENIED: &str = "Bash,PowerShell,NotebookEdit,WebFetch,WebSearch,Task,\
                                      Agent,TodoWrite,Skill,Edit(**/.*),Edit(**/.*/**)";

/// Adds the settings flags: assistant ([`add_assistant`]), answer-only ([`add_answer_only`]),
/// or the project settings AI6 allowed, the Station's MCP servers and the green ink gate.
fn add_settings(
    argv: &mut Vec<String>,
    environment: &BTreeMap<String, String>,
    mode: Mode,
    assistant: Option<&AssistantMode>,
) -> Result<(), ProviderError> {
    if let Some(assistant) = assistant {
        add_assistant(argv, environment, assistant)?;
    } else if mode.answer_only {
        add_answer_only(argv);
    } else {
        add_project_settings(argv, mode.project_settings, mode.project_mcp);
        add_mcp(argv, environment);
        add_gate(argv, environment);
    }
    Ok(())
}

/// An assistant Session, built from its mode's parts: answer-only's restrictions
/// (`--restricted`, no built-in tool, only the user's settings) with exactly the mode's MCP
/// servers connected (`--strict-mcp-config`) and their tools allowed without a prompt
/// (`--allowedTools mcp__<name>`); a print-mode run denies any other tool it would have to ask
/// for. Interpreters and skills are refused until this handler can confine them: refusing
/// fails closed, where ignoring would start a Session the Station did not ask for.
///
/// A mode with `files` gets the file tools (`--tools Read,Write,Edit,Glob,Grep`), which
/// `--restricted` confines to the working directory, under `--permission-mode dontAsk`, which
/// denies every call no rule allows: reading is allowed anywhere there, writing only under each
/// directory (`Edit(/<dir>/**)`, relative to the working directory; Edit rules cover Write),
/// and writes to `.git` and `.claude` always need a prompt, so they are denied too
/// (code.claude.com/docs/en/permissions). A `context_file` is passed with
/// `--append-system-prompt-file`.
fn add_assistant(
    argv: &mut Vec<String>,
    environment: &BTreeMap<String, String>,
    mode: &AssistantMode,
) -> Result<(), ProviderError> {
    mode.validate()
        .map_err(|reason| refuse(Status::GUATIAO_ERR_BAD_VALUE, reason))?;
    if !mode.interpreters.is_empty() || !mode.skills.is_empty() {
        return Err(refuse(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this Claude Code handler runs no interpreter or skill for an assistant",
        ));
    }
    // WebFetch and WebSearch stay denied, so the only network the Session has is its servers.
    if !mode.network.reaches_nothing() {
        return Err(refuse(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this Claude Code handler gives an assistant no network beyond its MCP servers",
        ));
    }
    let mut servers = serde_json::Map::new();
    let mut allowed = Vec::new();
    for server in &mode.mcp_servers {
        if !has_endpoint(environment, &server.env_prefix) {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!(
                    "the environment carries no URL and token for MCP server {:?}",
                    server.name
                ),
            ));
        }
        servers.insert(server.name.clone(), http_server(&server.env_prefix));
        allowed.push(format!("mcp__{}", server.name));
    }
    argv.push("--restricted".to_owned());
    if mode.files.is_empty() {
        argv.extend([
            "--tools=".to_owned(),
            "--disallowedTools".to_owned(),
            ASSISTANT_DENIED.to_owned(),
        ]);
    } else {
        argv.extend([
            "--tools".to_owned(),
            ASSISTANT_FILE_TOOLS.to_owned(),
            "--permission-mode".to_owned(),
            "dontAsk".to_owned(),
            "--disallowedTools".to_owned(),
            ASSISTANT_FILES_DENIED.to_owned(),
        ]);
        allowed.extend(["Read".to_owned(), "Glob".to_owned(), "Grep".to_owned()]);
        allowed.extend(mode.files.iter().map(|path| format!("Edit(/{path}/**)")));
    }
    argv.extend([
        "--allowedTools".to_owned(),
        allowed.join(","),
        "--setting-sources".to_owned(),
        "user".to_owned(),
        "--strict-mcp-config".to_owned(),
        "--mcp-config".to_owned(),
        json!({ "mcpServers": servers }).to_string(),
    ]);
    if let Some(file) = &mode.context_file {
        argv.extend(["--append-system-prompt-file".to_owned(), file.clone()]);
    }
    Ok(())
}

/// A Session that only reads its prompt and answers: `--restricted` (Claude Code 2.1.248 or
/// later: no shell or code tools, no WebFetch, only managed settings and AI6's), no built-in
/// tool at all (`--tools=`), the tools above denied by name, no MCP server (an empty strict
/// MCP config) and only the user's settings file besides. An older Claude Code that does not
/// know a flag refuses to start, which ends the Session without an answer.
fn add_answer_only(argv: &mut Vec<String>) {
    argv.extend([
        "--restricted".to_owned(),
        "--tools=".to_owned(),
        "--disallowedTools".to_owned(),
        ANSWER_ONLY_DENIED.to_owned(),
        "--setting-sources".to_owned(),
        "user".to_owned(),
        "--strict-mcp-config".to_owned(),
        "--mcp-config".to_owned(),
        json!({ "mcpServers": {} }).to_string(),
    ]);
}

/// Whether `environment` carries an MCP endpoint's URL and token under `prefix`.
fn has_endpoint(environment: &BTreeMap<String, String>, prefix: &str) -> bool {
    environment.contains_key(&format!("{prefix}_URL"))
        && environment.contains_key(&format!("{prefix}_TOKEN"))
}

/// One HTTP MCP server entry whose URL and bearer token come from the environment, so the
/// token never appears in the command line.
fn http_server(prefix: &str) -> Value {
    json!({
        "type": "http",
        "url": format!("${{{prefix}_URL}}"),
        "headers": { "Authorization": format!("Bearer ${{{prefix}_TOKEN}}") }
    })
}

/// Adds the Station's MCP servers: Cable tools, and, when the Station serves a Theater's
/// tools (remote-hands mode), those in place of Claude's own shell and file tools.
fn add_mcp(argv: &mut Vec<String>, environment: &BTreeMap<String, String>) {
    let mut servers = serde_json::Map::new();
    if has_endpoint(environment, "AI6_CABLE_MCP") {
        servers.insert("ai6-cables".to_owned(), http_server("AI6_CABLE_MCP"));
    }
    if has_endpoint(environment, "AI6_THEATER_MCP") {
        servers.insert("ai6-theater".to_owned(), http_server("AI6_THEATER_MCP"));
        argv.extend([
            "--disallowedTools".to_owned(),
            OWN_TOOLS.to_owned(),
            "--allowedTools".to_owned(),
            "mcp__ai6-theater".to_owned(),
        ]);
    }
    if !servers.is_empty() {
        let config = json!({ "mcpServers": servers });
        argv.extend(["--mcp-config".to_owned(), config.to_string()]);
    }
}

/// The Claude Code settings that hold a shell command taking a governed step until green ink
/// covers it: a `PreToolUse` hook on the shell tools running `PROGRAM green-ink gate
/// --cable-url URL`. The URL is in the hook's own arguments so a project's settings (`env`)
/// cannot point the gate at another endpoint; the token stays in the environment, out of every
/// command line, and a redirected token only makes the gate hold. A hook that fails to run
/// blocks the command (`onFailure`, Claude Code 2.1.295 or later), and hooks stay on whatever
/// the project's settings say.
fn gate_settings(program: &str, cable_url: &str) -> Value {
    json!({
        "disableAllHooks": false,
        "hooks": {
            "PreToolUse": [{
                "matcher": "Bash|PowerShell",
                "hooks": [{
                    "type": "command",
                    "command": program,
                    "args": ["green-ink", "gate", "--cable-url", cable_url],
                    "timeout": 60,
                    "onFailure": "block"
                }]
            }]
        }
    })
}

/// Adds the green ink gate when the Session has a Station to ask (its Cable endpoint). The
/// gate is the `ai6` program hosting this handler, or the one `AI6_GREEN_INK_GATE` names.
fn add_gate(argv: &mut Vec<String>, environment: &BTreeMap<String, String>) {
    let Some(cable_url) = environment
        .get("AI6_CABLE_MCP_URL")
        .filter(|_| has_endpoint(environment, "AI6_CABLE_MCP"))
    else {
        return;
    };
    let program = environment
        .get(GATE_PROGRAM_ENV)
        .filter(|program| !program.trim().is_empty())
        .cloned()
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        });
    if let Some(program) = program {
        argv.extend([
            "--settings".to_owned(),
            gate_settings(&program, cable_url).to_string(),
        ]);
    }
}

/// Keeps the working directory's own settings out of the harness unless AI6 allowed them:
/// only the user's settings file loads (`--setting-sources user`, so no project or local
/// hooks), and only the MCP servers AI6 passes (`--strict-mcp-config`, so no `.mcp.json`).
/// AI6's `--settings` and `--mcp-config` apply either way.
fn add_project_settings(argv: &mut Vec<String>, allowed: bool, mcp: bool) {
    if !allowed {
        argv.extend(["--setting-sources".to_owned(), "user".to_owned()]);
    }
    if !(allowed && mcp) {
        argv.push("--strict-mcp-config".to_owned());
    }
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
    output: Mutex<JsonLines>,
    /// The provider Session ID, once Claude has reported it.
    session_id: Mutex<Option<SessionId>>,
    expected_session_id: Option<SessionId>,
    eof: AtomicBool,
    eof_reported: AtomicBool,
    /// For a restricted (answer-only or assistant) Session, what it may call.
    restricted: Option<Allowed>,
    /// Set once a restricted Session called a tool it may not, and was ended: nothing it
    /// wrote after that is read.
    tripped: AtomicBool,
    /// The last lines Claude wrote that were not JSON: its own diagnostics, which say why it
    /// exited when it exits.
    tail: Mutex<VecDeque<String>>,
    /// Numbers the control requests written to Claude, for their `request_id`.
    requests: AtomicU64,
}

/// How many of the last non-JSON lines a `failed` event quotes, and how long each may be.
const TAIL_LINES: usize = 8;
const TAIL_LINE_BYTES: usize = 512;
/// How long `close` waits for the harness to have exited, for a terminal whose `kill` does not.
const CLOSE_WAIT_MS: i64 = 2_000;
/// How long the end of the output waits for the harness's exit status.
const EXIT_WAIT_MS: i64 = 1_000;

impl ClaudeSession {
    fn new(terminal: Object<dyn TerminalSession>, expected_session_id: Option<SessionId>) -> Self {
        Self {
            terminal,
            output: Mutex::new(JsonLines::new()),
            session_id: Mutex::new(None),
            expected_session_id,
            eof: AtomicBool::new(false),
            eof_reported: AtomicBool::new(false),
            restricted: None,
            tripped: AtomicBool::new(false),
            tail: Mutex::new(VecDeque::new()),
            requests: AtomicU64::new(0),
        }
    }

    /// How the harness ended, once its terminal says it has; `None` while it runs or when the
    /// terminal cannot tell.
    fn exited(&self, timeout_ms: i64) -> Option<ExitInfo> {
        let status = self.terminal.exit_status(timeout_ms).ok()?;
        ExitInfo::from_value(&status)
            .ok()
            .filter(|exit| !exit.running)
    }

    /// Why the Session ended without completing: the exit status and the last lines Claude
    /// wrote outside its stream.
    fn exit_reason(&self) -> String {
        let how = match self.exited(EXIT_WAIT_MS) {
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
        let mut reason = format!("Claude Code {how} before completing the Session");
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

    /// Restricts the Session to what `allowed` names: calling any other tool ends it.
    fn restricted_to(mut self, allowed: Option<Allowed>) -> Self {
        self.restricted = allowed;
        self
    }

    /// The `failed` event that ends a restricted Session whose `seen` event calls a tool outside
    /// its servers (`mcp__<server>__<tool>`) and, when it has them, its file tools, after
    /// closing it; `None` when it calls none. The flags it was started with are meant to make
    /// this impossible; this backs them up. Which paths a file tool may write is the
    /// permission rules' to enforce.
    fn tripped(&self, seen: &AgentEvent) -> Option<AgentEvent> {
        let allowed = self.restricted.as_ref()?;
        let forbidden = seen
            .tools
            .iter()
            .filter(|tool| {
                !(allowed
                    .servers
                    .iter()
                    .any(|server| tool.starts_with(&format!("mcp__{server}__")))
                    || (allowed.file_tools
                        && ASSISTANT_FILE_TOOLS.split(',').any(|file| file == *tool)))
            })
            .cloned()
            .collect::<Vec<_>>();
        if forbidden.is_empty() {
            return None;
        }
        let reason = format!(
            "the restricted Claude Code Session called {}, which it may not use; the Session \
             was ended",
            forbidden.join(", ")
        );
        eprintln!("ai6: {reason}");
        let _ = self.terminal.kill();
        self.tripped.store(true, Ordering::Release);
        Some(event("failed", Some(reason)))
    }

    /// Writes `input` as one line in a single write, so a line written from another thread (an
    /// interrupt during a message) cannot land inside it.
    fn write_line(&self, input: &impl Serialize) -> Result<(), ProviderError> {
        let mut line = serde_json::to_vec(input).map_err(internal)?;
        line.push(b'\n');
        self.terminal.write(&line)
    }

    fn send(&self, text: &str) -> Result<(), ProviderError> {
        self.write_line(&ClaudeInput::user(text))
    }

    fn read_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        if self.tripped.load(Ordering::Acquire) {
            return Ok(None);
        }
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        let line = loop {
            // What the terminal wrote before it closed is read before the exit is reported.
            let ended = self.eof.load(Ordering::Acquire);
            let next = {
                let mut output = lock(&self.output)?;
                if ended {
                    output.finish()
                } else {
                    output.next_line()
                }
            };
            if let Some(line) = next {
                break line;
            }
            if ended {
                if self.eof_reported.swap(true, Ordering::AcqRel) {
                    return Ok(None);
                }
                let reason = self.exit_reason();
                eprintln!("ai6: {reason}");
                return Ok(Some(event("failed", Some(reason))));
            }
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
                // A terminal may keep its output open after the harness exits.
                if self.exited(0).is_some() {
                    self.eof.store(true, Ordering::Release);
                    continue;
                }
                return Ok(None);
            }
            if count == 0 {
                self.eof.store(true, Ordering::Release);
                continue;
            }
            if count < 0 {
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "Claude terminal returned an invalid read count",
                ));
            }
            lock(&self.output)?.push(&bytes[..count as usize]);
        };
        let (value, line) = match line {
            StreamLine::Json { value, text, .. } => (value, text),
            StreamLine::Text(text) => {
                self.remember_output(&text);
                return Ok(Some(event("output", Some(text))));
            }
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
        let kind = value.get("type").and_then(Value::as_str);
        if kind == Some("control_response") {
            control_answered(&value);
        }
        if matches!(kind, Some("user" | "control_response")) {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            return self.read_event(remaining.as_millis().min(i64::MAX as u128) as i64);
        }
        if let Some(event) = decode_event(&value, line.clone()) {
            if let Some(ended) = self.tripped(&event) {
                return Ok(Some(ended));
            }
            return Ok(Some(event));
        }
        Ok(None)
    }
}

/// Logs a `control_response` that reports an error; a success needs nothing. Claude Code
/// 2.1.310 answers an interrupt with `success` (`{"still_queued":[]}`) even when no turn is
/// running, and a request it does not support with `error` and its reason.
fn control_answered(value: &Value) {
    let Some(response) = value.get("response") else {
        return;
    };
    if response.get("subtype").and_then(Value::as_str) == Some("error") {
        let error = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        eprintln!("ai6: Claude Code refused a control request: {error}");
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
        self.read_event(timeout_ms.max(0))
    }

    /// Stops the running turn with stream-json's `interrupt` control request, in the Agent
    /// SDK's shape; the `control_response` Claude answers with is read and dropped.
    fn interrupt(&self) -> Result<(), ProviderError> {
        let number = self.requests.fetch_add(1, Ordering::Relaxed) + 1;
        self.write_line(&json!({
            "type": "control_request",
            "request_id": format!("ai6-interrupt-{number}"),
            "request": { "subtype": "interrupt" },
        }))
    }

    fn close(&self) -> Result<(), ProviderError> {
        self.terminal.kill()?;
        // The caller may remove the working directory next, which the harness holds until it
        // has exited; a terminal that cannot tell is not waited for.
        let _ = self.terminal.exit_status(CLOSE_WAIT_MS);
        Ok(())
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
        cols: STREAM_COLS,
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
        tools: Vec::new(),
        quota: Vec::new(),
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

/// The quota a `rate_limit_event` reports, whatever its status: the window
/// (`rateLimitType`), the status, `resetsAt`, and `utilization` as thousandths when it is a
/// fraction between 0 and 1.
fn decode_quota(value: &Value) -> Vec<QuotaReport> {
    if value.get("type").and_then(Value::as_str) != Some("rate_limit_event") {
        return Vec::new();
    }
    let Some(info) = value.get("rate_limit_info") else {
        return Vec::new();
    };
    vec![QuotaReport {
        window: info
            .get("rateLimitType")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        used_permille: info
            .get("utilization")
            .and_then(Value::as_f64)
            .filter(|fraction| (0.0..=1.0).contains(fraction))
            .map(|fraction| (fraction * 1000.0).round() as u16),
        resets_at: info
            .get("resetsAt")
            .and_then(Value::as_i64)
            .map(unix_seconds),
        status: info
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }]
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
            tools: Vec::new(),
            quota: decode_quota(value),
        });
    }
    let tools = decode_tools(value);
    let kind = match event_type {
        "result" => "completed",
        "error" => "failed",
        "assistant" if !tools.is_empty() => "tool",
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
        tools,
        quota: decode_quota(value),
    })
}

/// The tools an `assistant` message calls, one per tool-call content block: a `tool_use`
/// block (a tool Claude Code runs) by its `name`; any other `*_tool_use` block, which the
/// Messages API runs itself (`server_tool_use` for the advisor, web search or code execution,
/// `mcp_tool_use` for its MCP connector), as `TYPE:NAME`, which no `mcp__` rule matches.
fn decode_tools(value: &Value) -> Vec<String> {
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    value
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| {
            let kind = block.get("type").and_then(Value::as_str)?;
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            match kind {
                "tool_use" => Some(name.to_owned()),
                kind if kind.ends_with("_tool_use") => Some(format!("{kind}:{name}")),
                _ => None,
            }
        })
        .collect()
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
        /// Keeps the output open after the child exits, as ConPTY does.
        keep_open: bool,
    }

    impl ProcessTerminal {
        fn new() -> Self {
            Self {
                specs: Arc::new(Mutex::new(Vec::new())),
                keep_open: false,
            }
        }

        fn kept_open() -> Self {
            Self {
                keep_open: true,
                ..Self::new()
            }
        }
    }

    /// A line the fake child writes that the fixture hands over although it is not JSON: the
    /// harness's own diagnostics.
    const FAKE_TEXT: &str = "fake-claude: ";

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
            let held_open = self.keep_open.then(|| sender.clone());
            thread::spawn(move || forward_json_lines(stdout, sender));

            Ok(ProcessTerminalSession {
                child: Mutex::new(child),
                stdin: Mutex::new(stdin),
                receiver: Mutex::new(receiver),
                pending: Mutex::new(VecDeque::new()),
                _held_open: held_open,
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
                Ok(_)
                    if serde_json::from_slice::<Value>(&line).is_ok()
                        || line.starts_with(FAKE_TEXT.as_bytes()) =>
                {
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
        _held_open: Option<mpsc::Sender<Vec<u8>>>,
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

        fn exit_status(&self, timeout_ms: i64) -> Result<ai6::guatiao::Value, ProviderError> {
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(0) as u64);
            loop {
                if let Some(status) = self.child.lock().unwrap().try_wait().map_err(internal)? {
                    return ExitInfo {
                        running: false,
                        code: status.code().map(i64::from),
                        signal: None,
                    }
                    .to_value()
                    .map_err(internal);
                }
                if Instant::now() >= deadline {
                    return ExitInfo {
                        running: true,
                        ..ExitInfo::default()
                    }
                    .to_value()
                    .map_err(internal);
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn fake_claude_child() {
        if std::env::var_os("AI6_FAKE_CLAUDE_CHILD").is_none() {
            return;
        }
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        // Claude Code reads its first input before it writes anything (stream-json input); one
        // that cannot go on then says why and exits.
        if let Ok(code) = std::env::var("CLAUDE_FIXTURE_EXIT") {
            let mut first = String::new();
            std::io::stdin().lock().read_line(&mut first).unwrap();
            writeln!(stdout, "{FAKE_TEXT}Error: the stored login has expired").unwrap();
            stdout.flush().unwrap();
            std::process::exit(code.parse().unwrap());
        }
        let session_id = std::env::var("CLAUDE_FIXTURE_SESSION_ID").unwrap();
        let mut initialized = false;
        for line in std::io::stdin().lock().lines() {
            if !std::mem::replace(&mut initialized, true) {
                writeln!(
                    stdout,
                    "{}",
                    json!({"type":"system","subtype":"init","session_id":session_id})
                )
                .unwrap();
            }
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
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                    initial_context: Vec::new(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        let resumed_events = read_to_completion(&resumed);
        assert_eq!(resumed.session_id().unwrap(), Some(session_id.clone()));
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
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                    initial_context: Vec::new(),
                },
                terminal.into_object(),
            )
            .unwrap();

        assert_eq!(
            session.session_id().unwrap(),
            None,
            "unknown until Claude reports it"
        );
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

    /// Claude Code's stream as ConPTY renders it on Windows: mode and title sequences first, a
    /// CRLF after each line, the echoed input, and a line longer than the terminal broken
    /// with CRLF and a cursor move.
    #[test]
    fn a_stream_rendered_by_conpty_reads_as_the_lines_claude_wrote() {
        let long = "w".repeat(20_000);
        let assistant = format!(
            "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{long}\"}}]}}}}"
        );
        let (head, tail) = assistant.split_at(9_000);
        let mut input =
            b"\x1b[?9001h\x1b[?1004h\x1b[m\x1b]0;C:\\bin\\claude.exe\x07\x1b[?25h\r\n".to_vec();
        input.extend_from_slice(
            b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"claude-session-42\"}\r\n",
        );
        input.extend_from_slice(b"{\"type\":\"user\",\"message\":{\"role\":\"user\"}}\r\n");
        input.extend_from_slice(head.as_bytes());
        input.extend_from_slice(b"\r\n\x1b[23;9000H");
        input.extend_from_slice(tail.as_bytes());
        input.extend_from_slice(b"\r\n\x1b[?25l{\"type\":\"result\",\"result\":\"done\"}\r\n");
        let spec = Arc::new(Mutex::new(None));
        let terminal = ResumeTerminal {
            spec: spec.clone(),
            input,
            writes: Arc::new(Mutex::new(Vec::new())),
        };
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-test".to_owned(),
        })
        .unwrap();
        let session = handler
            .resume(
                AgentResume {
                    mission_id: "mission-42".parse().unwrap(),
                    session_id: SessionId::try_new("claude-session-42").unwrap(),
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
        let kinds_and_texts = std::iter::from_fn(|| session.next_event(50).unwrap())
            .map(|event| (event.kind, event.text))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds_and_texts,
            [
                ("system".to_owned(), None),
                ("text".to_owned(), Some(long)),
                ("completed".to_owned(), Some("done".to_owned())),
            ]
        );
        assert_eq!(spec.lock().unwrap().as_ref().unwrap().cols, STREAM_COLS);
    }

    /// An interrupt is one stream-json control request; Claude's answers to it, the idle
    /// `success` Claude Code 2.1.310 sends and an `error`, are consumed, never events.
    #[test]
    fn interrupt_writes_a_control_request_and_consumes_its_response() {
        let mut input =
            b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"claude-session-42\"}\n"
                .to_vec();
        input.extend_from_slice(
            b"{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\
              \"request_id\":\"ai6-interrupt-1\",\"response\":{\"still_queued\":[]}}}\n",
        );
        input.extend_from_slice(
            b"{\"type\":\"control_response\",\"response\":{\"subtype\":\"error\",\
              \"request_id\":\"ai6-interrupt-2\",\"error\":\"no turn to interrupt\"}}\n",
        );
        input.extend_from_slice(b"{\"type\":\"result\",\"result\":\"stopped\"}\n");
        let writes = Arc::new(Mutex::new(Vec::new()));
        let terminal = ResumeTerminal {
            spec: Arc::new(Mutex::new(None)),
            input,
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
                    session_id: SessionId::try_new("claude-session-42").unwrap(),
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

        session.interrupt().unwrap();
        session.interrupt().unwrap();

        let written = String::from_utf8(writes.lock().unwrap().clone()).unwrap();
        assert!(!written.contains('\u{3}'), "no ^C byte reaches the stream");
        let requests = written
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            requests,
            [1, 2].map(|number| json!({
                "type": "control_request",
                "request_id": format!("ai6-interrupt-{number}"),
                "request": { "subtype": "interrupt" },
            }))
        );
        assert!(written.ends_with('\n'));

        let kinds_and_texts = std::iter::from_fn(|| session.next_event(50).unwrap())
            .map(|event| (event.kind, event.text))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds_and_texts,
            [
                ("system".to_owned(), None),
                ("completed".to_owned(), Some("stopped".to_owned())),
            ]
        );
    }

    #[test]
    fn a_restricted_session_that_calls_a_tool_is_ended() {
        let call = |tool: &str| {
            format!(
                "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\
                 \"name\":\"{tool}\"}}]}}}}\n"
            )
        };
        for (answer_only, assistant, tool, ends) in [
            (true, None, "Bash", true),
            (true, None, "mcp__moneypenny__board", true),
            (
                false,
                Some(AssistantMode::moneypenny()),
                "mcp__moneypenny__board",
                false,
            ),
            (
                false,
                Some(AssistantMode::moneypenny()),
                "mcp__other__read",
                true,
            ),
            (false, None, "Bash", false),
        ] {
            let mut input =
                b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}\n".to_vec();
            input.extend_from_slice(call(tool).as_bytes());
            input.extend_from_slice(b"{\"type\":\"result\",\"result\":\"done\"}\n");
            let handler = ClaudeHandler::try_from(ClaudeConfig {
                executable: "claude-test".to_owned(),
            })
            .unwrap();
            let session = handler
                .resume(
                    AgentResume {
                        mission_id: "mission-42".parse().unwrap(),
                        session_id: SessionId::try_new("s-1").unwrap(),
                        message: None,
                        working_directory: "C:\\station\\mission-42".to_owned(),
                        environment: assistant_environment(),
                        model: None,
                        effort: None,
                        max_cost_micro_usd: None,
                        project_settings: false,
                        project_mcp: false,
                        answer_only,
                        assistant,
                        initial_context: Vec::new(),
                    },
                    ResumeTerminal {
                        spec: Arc::new(Mutex::new(None)),
                        input,
                        writes: Arc::new(Mutex::new(Vec::new())),
                    }
                    .into_object(),
                )
                .unwrap();
            let kinds = std::iter::from_fn(|| session.next_event(50).unwrap())
                .map(|event| (event.kind, event.text))
                .collect::<Vec<_>>();
            if ends {
                assert_eq!(kinds.len(), 2, "{tool}: {kinds:?}");
                assert_eq!(kinds[1].0, "failed");
                let reason = kinds[1].1.as_deref().unwrap();
                assert!(reason.contains(tool), "{reason}");
            } else {
                assert_eq!(
                    kinds.last().map(|(kind, _)| kind.as_str()),
                    Some("completed"),
                    "{tool}: {kinds:?}"
                );
            }
        }
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
        let session = handler
            .resume(
                AgentResume {
                    mission_id: "mission-42".parse().unwrap(),
                    session_id: SessionId::try_new("claude-session-42").unwrap(),
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
        let error = match session.next_event(50) {
            Ok(event) => panic!("a different provider Session ID was read as {event:?}"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("different Session"), "{error}");
        assert_eq!(session.session_id().unwrap(), None);
    }

    /// Claude Code with stream-json input writes nothing until it has read a message, so a
    /// resume that waited for its output before sending one would never see any.
    #[test]
    fn resume_sends_its_message_at_once_to_a_harness_silent_until_it_reads_input() {
        let terminal = ProcessTerminal::new();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-silent-fixture".to_owned(),
        })
        .unwrap();
        let started = Instant::now();
        let resumed = handler
            .resume(
                AgentResume {
                    mission_id: "mission-silent".parse().unwrap(),
                    session_id: SessionId::try_new("claude-silent-session").unwrap(),
                    message: Some("are you there?".to_owned()),
                    working_directory: std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    environment: BTreeMap::from([(
                        "CLAUDE_FIXTURE_SESSION_ID".to_owned(),
                        "claude-silent-session".to_owned(),
                    )]),
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
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "resume waited for output"
        );
        let events = read_to_completion(&resumed);
        assert!(events.iter().any(|event| {
            event.kind == "completed" && event.text.as_deref() == Some("are you there?")
        }));
        assert_eq!(
            resumed
                .session_id()
                .unwrap()
                .as_ref()
                .map(SessionId::as_str),
            Some("claude-silent-session")
        );
        resumed.close().unwrap();
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
    fn every_rate_limit_event_reports_its_window_as_quota() {
        let rejected = json!({
            "type": "rate_limit_event",
            "session_id": "claude-session-7",
            "rate_limit_info": {
                "status": "rejected",
                "resetsAt": 1_790_000_000,
                "rateLimitType": "five_hour"
            }
        });
        assert_eq!(
            decode_event(&rejected, b"{}".to_vec()).unwrap().quota,
            [QuotaReport {
                window: "five_hour".to_owned(),
                used_permille: None,
                resets_at: Some(1_790_000_000),
                status: Some("rejected".to_owned()),
            }]
        );
        let warning = json!({
            "type": "rate_limit_event",
            "rate_limit_info": { "status": "allowed_warning", "resetsAt": 1_790_000_000 }
        });
        let quota = decode_event(&warning, b"{}".to_vec()).unwrap().quota;
        assert_eq!(quota[0].window, "unknown");
        assert_eq!(quota[0].status.as_deref(), Some("allowed_warning"));
        // `utilization` is in the SDK's published SDKRateLimitInfo type; only a fraction is
        // taken, since its scale is not documented there.
        let measured = json!({
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": "allowed",
                "rateLimitType": "seven_day",
                "utilization": 0.25
            }
        });
        assert_eq!(
            decode_event(&measured, b"{}".to_vec()).unwrap().quota[0].used_permille,
            Some(250)
        );
        let percent = json!({
            "type": "rate_limit_event",
            "rate_limit_info": { "status": "allowed", "utilization": 25 }
        });
        assert_eq!(
            decode_event(&percent, b"{}".to_vec()).unwrap().quota[0].used_permille,
            None
        );
        let result = json!({ "type": "result", "result": "done" });
        assert!(
            decode_event(&result, b"{}".to_vec())
                .unwrap()
                .quota
                .is_empty()
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

    /// A harness that exits while its terminal keeps the output open (ConPTY) ends the Session
    /// with one `failed` event naming its exit code and quoting what it wrote last.
    #[test]
    fn a_harness_that_exits_early_fails_the_session_with_its_status_and_last_output() {
        let terminal = ProcessTerminal::kept_open();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-exit-fixture".to_owned(),
        })
        .unwrap();
        let session = handler
            .start(
                AgentStart {
                    mission_id: "mission-exit".parse().unwrap(),
                    agent_id: "007".to_owned(),
                    prompt: "hello".to_owned(),
                    initial_context: Vec::new(),
                    working_directory: std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    clearance: "field".to_owned(),
                    environment: BTreeMap::from([(
                        "CLAUDE_FIXTURE_EXIT".to_owned(),
                        "3".to_owned(),
                    )]),
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                },
                terminal.into_object(),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let failed = loop {
            assert!(Instant::now() < deadline, "the exit was never noticed");
            if let Some(event) = session.next_event(100).unwrap()
                && event.kind == "failed"
            {
                break event;
            }
        };
        let reason = failed.text.unwrap();
        assert!(reason.contains("exited with code 3"), "{reason}");
        assert!(
            reason.contains("Error: the stored login has expired"),
            "{reason}"
        );
        assert_eq!(session.next_event(100).unwrap(), None, "reported once");
        session.close().unwrap();
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

        add_mcp(&mut argv, &environment);

        assert_eq!(argv[1], "--mcp-config");
        assert!(argv[2].contains("${AI6_CABLE_MCP_URL}"));
        assert!(argv[2].contains("${AI6_CABLE_MCP_TOKEN}"));
        assert!(!argv[2].contains("session-secret-token"));
        assert!(!argv.iter().any(|argument| argument == "--disallowedTools"));
    }

    #[test]
    fn remote_hands_turns_off_claudes_own_tools_and_adds_the_theater_server() {
        let environment = BTreeMap::from([
            (
                "AI6_THEATER_MCP_URL".to_owned(),
                "http://127.0.0.1:43211/mcp".to_owned(),
            ),
            (
                "AI6_THEATER_MCP_TOKEN".to_owned(),
                "theater-secret-token".to_owned(),
            ),
        ]);
        let mut argv = vec!["claude".to_owned()];

        add_mcp(&mut argv, &environment);

        let position = |flag: &str| argv.iter().position(|argument| argument == flag).unwrap();
        let disallowed = &argv[position("--disallowedTools") + 1];
        for tool in ["Bash", "Edit", "Write", "Read", "Grep"] {
            assert!(disallowed.split(',').any(|name| name == tool), "{tool}");
        }
        assert_eq!(argv[position("--allowedTools") + 1], "mcp__ai6-theater");
        let config = &argv[position("--mcp-config") + 1];
        assert!(config.contains("ai6-theater") && !config.contains("ai6-cables"));
        assert!(config.contains("${AI6_THEATER_MCP_TOKEN}"));
        assert!(
            !argv
                .iter()
                .any(|argument| argument.contains("theater-secret-token"))
        );
        // A variadic flag is closed by the next flag, so no value runs on into another.
        assert!(position("--disallowedTools") < position("--allowedTools"));
        assert!(position("--allowedTools") < position("--mcp-config"));
    }

    fn cable_environment() -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "AI6_CABLE_MCP_URL".to_owned(),
                "http://127.0.0.1:43210/mcp".to_owned(),
            ),
            ("AI6_CABLE_MCP_TOKEN".to_owned(), "cable-token".to_owned()),
        ])
    }

    /// The hook as the Claude Code hooks reference describes a `PreToolUse` command hook: a
    /// regex matcher over tool names, exec form `command` + `args`, `timeout` in seconds.
    #[test]
    fn a_session_with_a_station_gets_the_green_ink_hook_in_its_settings() {
        let mut environment = cable_environment();
        environment.insert(GATE_PROGRAM_ENV.to_owned(), "/opt/ai6/bin/ai6".to_owned());
        let mut argv = vec!["claude".to_owned()];
        add_gate(&mut argv, &environment);

        assert_eq!(argv[1], "--settings");
        let settings: Value = serde_json::from_str(&argv[2]).unwrap();
        assert_eq!(settings["disableAllHooks"], false);
        let entry = &settings["hooks"]["PreToolUse"][0];
        let matcher = entry["matcher"].as_str().unwrap();
        for tool in ["Bash", "PowerShell"] {
            assert!(matcher.split('|').any(|name| name == tool), "{tool}");
        }
        let hook = &entry["hooks"][0];
        assert_eq!(hook["type"], "command");
        assert_eq!(hook["command"], "/opt/ai6/bin/ai6");
        assert_eq!(
            hook["args"],
            json!([
                "green-ink",
                "gate",
                "--cable-url",
                "http://127.0.0.1:43210/mcp"
            ]),
            "the endpoint is pinned in the hook itself"
        );
        assert_eq!(hook["onFailure"], "block");
        assert!(hook["timeout"].as_u64().unwrap() >= 30);
        assert!(
            !argv[2].contains("cable-token"),
            "the hook reads the token from its environment"
        );

        // Unnamed, the gate is the program hosting the handler.
        let mut argv = vec!["claude".to_owned()];
        add_gate(&mut argv, &cable_environment());
        let settings: Value = serde_json::from_str(&argv[2]).unwrap();
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            std::env::current_exe().unwrap().to_string_lossy().as_ref()
        );

        // Without a Station to ask there is no gate.
        let mut argv = vec!["claude".to_owned()];
        add_gate(&mut argv, &BTreeMap::new());
        assert_eq!(argv, ["claude"]);
    }

    #[test]
    fn start_and_resume_both_pass_the_gate() {
        let mut environment = cable_environment();
        environment.insert(
            "CLAUDE_FIXTURE_SESSION_ID".to_owned(),
            "claude-gate-session".to_owned(),
        );
        let terminal = ProcessTerminal::new();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-gate-fixture".to_owned(),
        })
        .unwrap();
        let working_directory = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let started = handler
            .start(
                AgentStart {
                    mission_id: "mission-gate".parse().unwrap(),
                    agent_id: "007".to_owned(),
                    prompt: "push when approved".to_owned(),
                    initial_context: Vec::new(),
                    working_directory: working_directory.clone(),
                    clearance: "field".to_owned(),
                    environment: environment.clone(),
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    mission_id: "mission-gate".parse().unwrap(),
                    session_id: SessionId::try_new("claude-gate-session").unwrap(),
                    message: Some("approved".to_owned()),
                    working_directory,
                    environment,
                    model: None,
                    effort: None,
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
        read_to_completion(&resumed);
        resumed.close().unwrap();
        for spec in terminal.specs.lock().unwrap().iter() {
            let position = spec
                .argv
                .iter()
                .position(|argument| argument == "--settings")
                .expect("the gate is passed");
            assert!(spec.argv[position + 1].contains("green-ink"));
        }
    }

    /// `--setting-sources` takes `user`, `project` and `local`; `--strict-mcp-config` keeps only
    /// `--mcp-config`'s servers; `--settings` sits above the setting files and is not one of
    /// them (Claude Code CLI reference and settings precedence, code.claude.com/docs).
    #[test]
    fn project_settings_are_shut_out_unless_allowed_and_the_gate_stays_either_way() {
        let mut environment = cable_environment();
        environment.insert(
            "CLAUDE_FIXTURE_SESSION_ID".to_owned(),
            "claude-settings-session".to_owned(),
        );
        let terminal = ProcessTerminal::new();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-settings-fixture".to_owned(),
        })
        .unwrap();
        let working_directory = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let start = |(project_settings, project_mcp, answer_only)| AgentStart {
            mission_id: "mission-settings".parse().unwrap(),
            agent_id: "007".to_owned(),
            prompt: "work".to_owned(),
            initial_context: Vec::new(),
            working_directory: working_directory.clone(),
            clearance: "field".to_owned(),
            environment: environment.clone(),
            model: None,
            effort: None,
            max_cost_micro_usd: None,
            project_settings,
            project_mcp,
            answer_only,
            assistant: None,
        };
        for mode in [
            (false, false, false),
            (true, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            let started = handler
                .start(start(mode), terminal.clone().into_object())
                .unwrap();
            read_to_completion(&started);
            started.close().unwrap();
        }
        let resumed = handler
            .resume(
                AgentResume {
                    mission_id: "mission-settings".parse().unwrap(),
                    session_id: SessionId::try_new("claude-settings-session").unwrap(),
                    message: None,
                    working_directory: working_directory.clone(),
                    environment: environment.clone(),
                    model: None,
                    effort: None,
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
        resumed.close().unwrap();

        let specs = terminal.specs.lock().unwrap();
        let shut_out = |argv: &[String]| {
            argv.windows(2)
                .any(|pair| pair[0] == "--setting-sources" && pair[1] == "user")
                && argv
                    .iter()
                    .any(|argument| argument == "--strict-mcp-config")
        };
        let has = |argv: &[String], flag: &str| argv.iter().any(|argument| argument == flag);
        let (ignored, loaded, pinned, answer_only, resumed) = (
            &specs[0].argv,
            &specs[1].argv,
            &specs[2].argv,
            &specs[3].argv,
            &specs[4].argv,
        );
        // Answer-only wins over everything else asked for: no tool, no server, no gate.
        assert!(shut_out(answer_only), "{answer_only:?}");
        assert!(has(answer_only, "--restricted") && has(answer_only, "--tools="));
        let position = |flag: &str| answer_only.iter().position(|a| a == flag).unwrap();
        assert_eq!(
            answer_only[position("--disallowedTools") + 1],
            ANSWER_ONLY_DENIED
        );
        assert_eq!(
            answer_only[position("--mcp-config") + 1],
            r#"{"mcpServers":{}}"#
        );
        assert!(!has(answer_only, "--settings"), "no gate: nothing to gate");
        assert!(!answer_only.iter().any(|a| a.contains("ai6-cables")));
        assert!(shut_out(ignored), "{ignored:?}");
        assert!(has(ignored, "--settings") && has(ignored, "--mcp-config"));
        assert!(!has(loaded, "--setting-sources") && !has(loaded, "--strict-mcp-config"));
        assert!(
            has(loaded, "--settings"),
            "the gate does not depend on the mode"
        );
        assert!(
            !has(pinned, "--setting-sources") && has(pinned, "--strict-mcp-config"),
            "settings without .mcp.json keep only AI6's servers: {pinned:?}"
        );
        assert!(shut_out(resumed), "{resumed:?}");
        let advisor = |index: usize| {
            specs[index]
                .env
                .as_ref()
                .unwrap()
                .get(DISABLE_ADVISOR_ENV)
                .cloned()
        };
        assert_eq!(
            advisor(3).as_deref(),
            Some("1"),
            "answer-only has no advisor"
        );
        assert_eq!(advisor(0), None, "an Operation's Session keeps its own");
    }

    /// Claude Code 2.1.310 warns that a deny rule naming `MultiEdit` or `LS` matches no known
    /// tool.
    #[test]
    fn deny_lists_name_only_tools_claude_code_knows() {
        for list in [OWN_TOOLS, ANSWER_ONLY_DENIED, ASSISTANT_DENIED] {
            for gone in ["MultiEdit", "LS"] {
                assert!(
                    !list.split(',').any(|name| name == gone),
                    "{gone} in {list}"
                );
            }
        }
    }

    fn assistant_environment() -> BTreeMap<String, String> {
        let mut environment = cable_environment();
        environment.insert(
            "AI6_MONEYPENNY_MCP_URL".to_owned(),
            "https://exports.example/v1/moneypenny/mcp".to_owned(),
        );
        environment.insert(
            "AI6_MONEYPENNY_MCP_TOKEN".to_owned(),
            "conversation-token".to_owned(),
        );
        environment.insert(
            "CLAUDE_FIXTURE_SESSION_ID".to_owned(),
            "claude-assistant-session".to_owned(),
        );
        environment
    }

    /// `--tools ""` removes every built-in tool and does not touch MCP tools;
    /// `--disallowedTools` with a bare name removes that tool; `--allowedTools` lets a tool run
    /// without a prompt; `--strict-mcp-config` keeps only `--mcp-config`'s servers;
    /// `--restricted` removes command tools and WebFetch and loads only managed settings and
    /// `--settings` (Claude Code CLI reference, code.claude.com/docs/en/cli-reference,
    /// read 2026-10-26).
    #[test]
    fn an_assistant_has_only_its_mcp_servers_and_no_built_in_tool_on_start_and_resume() {
        let environment = assistant_environment();
        let terminal = ProcessTerminal::new();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-assistant-fixture".to_owned(),
        })
        .unwrap();
        let working_directory = std::env::current_dir()
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
                    working_directory: working_directory.clone(),
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
        read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    mission_id: "assistant-conversation".parse().unwrap(),
                    session_id: SessionId::try_new("claude-assistant-session").unwrap(),
                    message: Some("And now?".to_owned()),
                    working_directory,
                    environment,
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: true,
                    project_mcp: true,
                    answer_only: false,
                    assistant: Some(AssistantMode::moneypenny()),
                    initial_context: vec!["You are Moneypenny.".to_owned()],
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        read_to_completion(&resumed);
        resumed.close().unwrap();
        let specs = terminal.specs.lock().unwrap();
        assert_eq!(specs.len(), 2);
        for spec in specs.iter() {
            let argv = &spec.argv;
            let after = |flag: &str| {
                let position = argv.iter().position(|a| a == flag).unwrap_or_else(|| {
                    panic!("{flag} missing from {argv:?}");
                });
                argv[position + 1].clone()
            };
            assert!(argv.iter().any(|a| a == "--restricted"), "{argv:?}");
            assert!(argv.iter().any(|a| a == "--tools="), "{argv:?}");
            assert!(argv.iter().any(|a| a == "--strict-mcp-config"), "{argv:?}");
            assert_eq!(after("--setting-sources"), "user");
            assert_eq!(after("--disallowedTools"), ASSISTANT_DENIED);
            assert!(!ASSISTANT_DENIED.contains("mcp__"));
            assert_eq!(
                spec.env
                    .as_ref()
                    .unwrap()
                    .get(DISABLE_ADVISOR_ENV)
                    .map(String::as_str),
                Some("1"),
                "no server-side advisor"
            );
            assert_eq!(
                spec.env
                    .as_ref()
                    .unwrap()
                    .get(DISABLE_AUTO_MEMORY_ENV)
                    .map(String::as_str),
                Some("1"),
                "no auto memory beside hers"
            );
            assert!(!argv.iter().any(|a| a == "--permission-mode"), "{argv:?}");
            assert!(!argv.iter().any(|a| a == "--append-system-prompt-file"));
            assert_eq!(after("--allowedTools"), "mcp__moneypenny");
            let servers: Value = serde_json::from_str(&after("--mcp-config")).unwrap();
            assert_eq!(
                servers,
                json!({"mcpServers": {"moneypenny": {
                    "type": "http",
                    "url": "${AI6_MONEYPENNY_MCP_URL}",
                    "headers": {"Authorization": "Bearer ${AI6_MONEYPENNY_MCP_TOKEN}"}
                }}}),
                "only Moneypenny's server, the Cable server of the environment left out"
            );
            assert_eq!(
                argv.iter().filter(|a| *a == "--mcp-config").count(),
                1,
                "{argv:?}"
            );
            assert!(!argv.iter().any(|a| a == "--settings"), "no gate to pass");
            assert_eq!(
                after("--append-system-prompt"),
                "You are Moneypenny.",
                "the instructions on start and on resume"
            );
            assert!(
                !argv.iter().any(|a| a.contains("conversation-token")),
                "the token stays in the environment"
            );
        }
    }

    /// Measured with Claude Code 2.1.310 against a loopback stand-in for the API: under these
    /// flags Write and Edit under `personal/` ran, and Write to another directory, to the
    /// working directory's own `CLAUDE.md`, to `personal/.git/hooks` and to `personal/../x`
    /// were denied, as was Read outside the working directory.
    #[test]
    fn an_assistant_with_files_gets_the_file_tools_under_dont_ask_on_start_and_resume() {
        let environment = assistant_environment();
        let terminal = ProcessTerminal::new();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-assistant-fixture".to_owned(),
        })
        .unwrap();
        let working_directory = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let context = std::env::temp_dir()
            .join("assistant-context.md")
            .to_string_lossy()
            .into_owned();
        let mode = AssistantMode {
            files: vec!["personal".to_owned(), "scratch".to_owned()],
            context_file: Some(context.clone()),
            ..AssistantMode::moneypenny()
        };
        let started = handler
            .start(
                AgentStart {
                    mission_id: "assistant-conversation".parse().unwrap(),
                    agent_id: "moneypenny".to_owned(),
                    prompt: "Remember that I like tables.".to_owned(),
                    initial_context: Vec::new(),
                    working_directory: working_directory.clone(),
                    clearance: "desk".to_owned(),
                    environment: environment.clone(),
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: Some(mode.clone()),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    mission_id: "assistant-conversation".parse().unwrap(),
                    session_id: SessionId::try_new("claude-assistant-session").unwrap(),
                    message: Some("And now?".to_owned()),
                    working_directory,
                    environment,
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: Some(mode),
                    initial_context: Vec::new(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        read_to_completion(&resumed);
        resumed.close().unwrap();
        let specs = terminal.specs.lock().unwrap();
        assert_eq!(specs.len(), 2);
        for spec in specs.iter() {
            let argv = &spec.argv;
            let after = |flag: &str| {
                let position = argv.iter().position(|a| a == flag).unwrap_or_else(|| {
                    panic!("{flag} missing from {argv:?}");
                });
                argv[position + 1].clone()
            };
            assert!(argv.iter().any(|a| a == "--restricted"), "{argv:?}");
            assert!(!argv.iter().any(|a| a == "--tools="), "{argv:?}");
            assert_eq!(after("--tools"), "Read,Write,Edit,Glob,Grep");
            assert_eq!(after("--permission-mode"), "dontAsk");
            let denied = after("--disallowedTools");
            for tool in [
                "Bash",
                "PowerShell",
                "WebFetch",
                "WebSearch",
                "Task",
                "Skill",
                "Edit(**/.*)",
                "Edit(**/.*/**)",
            ] {
                assert!(denied.split(',').any(|name| name == tool), "{denied}");
            }
            for tool in ["Read", "Write", "Edit", "Glob", "Grep"] {
                assert!(!denied.split(',').any(|name| name == tool), "{denied}");
            }
            assert_eq!(
                after("--allowedTools"),
                "mcp__moneypenny,Read,Glob,Grep,Edit(/personal/**),Edit(/scratch/**)"
            );
            assert_eq!(after("--setting-sources"), "user");
            assert_eq!(after("--append-system-prompt-file"), context);
            assert_eq!(
                spec.env
                    .as_ref()
                    .unwrap()
                    .get("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
                    .map(String::as_str),
                Some("1")
            );
        }
    }

    #[test]
    fn an_assistant_with_files_may_call_them_and_nothing_else_built_in() {
        let mode = AssistantMode {
            files: vec!["personal".to_owned()],
            ..AssistantMode::moneypenny()
        };
        for (mode, tool, ends) in [
            (mode.clone(), "Write", false),
            (mode.clone(), "Read", false),
            (mode, "Bash", true),
            (AssistantMode::moneypenny(), "Write", true),
        ] {
            let mut input =
                b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}\n".to_vec();
            input.extend_from_slice(
                format!(
                    "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\
                     \"id\":\"t\",\"name\":\"{tool}\",\"input\":{{}}}}]}}}}\n"
                )
                .as_bytes(),
            );
            input.extend_from_slice(b"{\"type\":\"result\",\"result\":\"done\"}\n");
            let session = ClaudeSession::new(
                ResumeTerminalSession {
                    input: Mutex::new(input),
                    writes: Arc::new(Mutex::new(Vec::new())),
                }
                .into_object(),
                None,
            )
            .restricted_to(restriction(false, Some(&mode)));
            let kinds = std::iter::from_fn(|| session.next_event(50).unwrap())
                .map(|event| event.kind)
                .collect::<Vec<_>>();
            assert_eq!(
                kinds.contains(&"failed".to_owned()),
                ends,
                "{tool} with files {:?}: {kinds:?}",
                mode.files
            );
        }
    }

    #[test]
    fn an_assistant_with_cable_grants_gets_the_cable_server_beside_her_own() {
        let mut environment = assistant_environment();
        let cables = AssistantMode::moneypenny().with_cables();
        let mut argv = Vec::new();
        assert!(
            add_assistant(&mut argv, &environment, &cables).is_err(),
            "no Cable endpoint in the environment"
        );
        environment.insert(
            "AI6_ASSISTANT_CABLE_MCP_URL".to_owned(),
            "http://127.0.0.1:4100/mcp".to_owned(),
        );
        environment.insert(
            "AI6_ASSISTANT_CABLE_MCP_TOKEN".to_owned(),
            "cable-token".to_owned(),
        );
        let mut argv = Vec::new();
        add_assistant(&mut argv, &environment, &cables).unwrap();
        let after = |flag: &str| {
            let position = argv.iter().position(|a| a == flag).unwrap();
            argv[position + 1].clone()
        };
        assert_eq!(after("--allowedTools"), "mcp__moneypenny,mcp__cables");
        let servers: Value = serde_json::from_str(&after("--mcp-config")).unwrap();
        assert_eq!(
            servers,
            json!({"mcpServers": {
                "moneypenny": {
                    "type": "http",
                    "url": "${AI6_MONEYPENNY_MCP_URL}",
                    "headers": {"Authorization": "Bearer ${AI6_MONEYPENNY_MCP_TOKEN}"}
                },
                "cables": {
                    "type": "http",
                    "url": "${AI6_ASSISTANT_CABLE_MCP_URL}",
                    "headers": {"Authorization": "Bearer ${AI6_ASSISTANT_CABLE_MCP_TOKEN}"}
                }
            }}),
            "a Mission's Cable server of the environment stays out"
        );
        assert!(!argv.iter().any(|a| a.contains("cable-token")));
    }

    #[test]
    fn an_assistant_mode_the_handler_cannot_confine_is_refused() {
        let environment = assistant_environment();
        let mut interpreters = AssistantMode::moneypenny();
        interpreters.interpreters.push("python3".to_owned());
        let mut open = AssistantMode::moneypenny();
        open.network.policy = "open".to_owned();
        let mut hosts = AssistantMode::moneypenny();
        hosts.network.allowed_hosts.push("example.com".to_owned());
        for mode in [interpreters, open, hosts] {
            let mut argv = Vec::new();
            assert!(add_assistant(&mut argv, &environment, &mode).is_err());
            assert!(argv.is_empty());
        }
        let mut argv = Vec::new();
        let denied = AssistantMode {
            network: ai6::AssistantNetwork {
                policy: "denied".to_owned(),
                allowed_hosts: Vec::new(),
            },
            ..AssistantMode::moneypenny()
        };
        assert!(add_assistant(&mut argv, &environment, &denied).is_ok());
        let mut argv = Vec::new();
        assert!(
            add_assistant(&mut argv, &BTreeMap::new(), &AssistantMode::moneypenny()).is_err(),
            "no endpoint in the environment"
        );
    }

    #[test]
    fn without_a_theater_server_claude_keeps_its_own_tools() {
        let mut argv = vec!["claude".to_owned()];
        add_mcp(&mut argv, &BTreeMap::new());
        assert_eq!(argv, ["claude"]);
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
    fn an_assistant_message_names_the_tools_it_calls() {
        let value = json!({
            "type": "assistant",
            "message": { "content": [
                { "type": "text", "text": "Fetching the page" },
                { "type": "tool_use", "id": "toolu_1", "name": "WebFetch", "input": {} },
                { "type": "tool_use", "id": "toolu_2", "name": "mcp__ai6-cables__read_cables", "input": {} }
            ] }
        });
        let event = decode_event(&value, b"{}".to_vec()).unwrap();
        assert_eq!(event.kind, "tool");
        assert_eq!(event.text.as_deref(), Some("Fetching the page"));
        assert_eq!(event.tools, ["WebFetch", "mcp__ai6-cables__read_cables"]);
        let plain = json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "hi" }] } });
        let event = decode_event(&plain, b"{}".to_vec()).unwrap();
        assert_eq!((event.kind.as_str(), event.tools.len()), ("text", 0));
        // Blocks the Messages API runs itself (platform.claude.com/docs, advisor tool and MCP
        // connector): `server_tool_use`, `mcp_tool_use`. Their results are not calls.
        let server = json!({
            "type": "assistant",
            "message": { "content": [
                { "type": "server_tool_use", "id": "srvtoolu_1", "name": "advisor", "input": {} },
                { "type": "advisor_tool_result", "tool_use_id": "srvtoolu_1", "content": {} },
                { "type": "mcp_tool_use", "id": "mcptoolu_1", "name": "echo", "server_name": "x" }
            ] }
        });
        let event = decode_event(&server, b"{}".to_vec()).unwrap();
        assert_eq!(event.kind, "tool");
        assert_eq!(
            event.tools,
            ["server_tool_use:advisor", "mcp_tool_use:echo"]
        );
    }

    #[test]
    fn a_server_side_tool_call_ends_a_restricted_session() {
        for block in [
            r#"{"type":"server_tool_use","id":"srvtoolu_1","name":"advisor","input":{}}"#,
            r#"{"type":"mcp_tool_use","id":"mcptoolu_1","name":"board","server_name":"moneypenny"}"#,
        ] {
            let mut input =
                b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}\n".to_vec();
            input.extend_from_slice(
                format!("{{\"type\":\"assistant\",\"message\":{{\"content\":[{block}]}}}}\n")
                    .as_bytes(),
            );
            input.extend_from_slice(b"{\"type\":\"result\",\"result\":\"done\"}\n");
            let session = ClaudeSession::new(
                ResumeTerminalSession {
                    input: Mutex::new(input),
                    writes: Arc::new(Mutex::new(Vec::new())),
                }
                .into_object(),
                None,
            )
            .restricted_to(restriction(false, Some(&AssistantMode::moneypenny())));
            let kinds = std::iter::from_fn(|| session.next_event(50).unwrap())
                .map(|event| (event.kind, event.text.unwrap_or_default()))
                .collect::<Vec<_>>();
            assert_eq!(kinds.len(), 2, "{block}: {kinds:?}");
            assert_eq!(kinds[1].0, "failed");
            assert!(kinds[1].1.contains("_tool_use:"), "{}", kinds[1].1);
        }
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
