//! Claude Code's stream-json implementation of AI6's AgentHandler contract.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

ai6::guatiao::providers!(ClaudeHandler);

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ai6::directorates::m::assistant::SKILLS_PLUGIN;
use ai6::directorates::m::identity::SessionId;
use ai6::directorates::q::terminal::{
    AgentTerminal, ExitInfo, JsonLines, READ_TIMED_OUT, STREAM_COLS, Spec, StreamLine,
    TerminalSession, exit_code_text,
};
use ai6::directorates::section_00::agent::{
    AgentHandler, AgentResume, AgentStart, AssistantMode, ModelInfo, ModelListing, SessionOverlays,
    SessionRecordFile, SessionRecordQuery, clearance_reads_only, clearance_runs_commands,
};
use ai6::directorates::section_00::assistant::{SCRIPT_ENVIRONMENT, credential_dir_var};
use ai6::directorates::section_00::denied::DenyList;
use ai6::directorates::section_00::gate::{GATE_PROGRAM_ENV, WEB_GATE_ARGS};
use ai6::directorates::section_00::listing;
use ai6::directorates::section_00::programs;
use ai6::directorates::section_00::records;
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use ai6::micro_usd;
use ai6::{
    AgentEvent, AgentMessage, AgentSession, DENIED_EVENT, LIMIT_EVENT, QuotaReport, TokenUsage,
    UsageLimit,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Claude Code AgentHandler configuration.
#[derive(Debug, Clone, ToValue, FromValue, Schema)]
#[map(crate = ::ai6::guatiao)]
pub struct ClaudeConfig {
    /// Executable visible to the AI6-owned terminal environment.
    #[map(default = String::from("claude"))]
    pub executable: String,
    /// Bytes of one line of Claude Code's output the Session holds before handing over what it
    /// has. Default 67108864 (64 MiB).
    #[map(default = 64 * 1024 * 1024)]
    pub json_line_bytes: u64,
    /// Bytes of one terminal control sequence before it is dropped. Default 64.
    #[map(default = 64)]
    pub csi_bytes: u64,
    /// How many of the last non-JSON lines a failed Session quotes. Default 8.
    #[map(default = 8)]
    pub tail_lines: u64,
    /// Bytes of each of those lines a failed Session quotes. Default 512.
    #[map(default = 512)]
    pub tail_line_bytes: u64,
    /// Milliseconds a closing Session waits for Claude Code to have exited. Default 2000.
    #[map(default = 2_000)]
    pub close_wait_ms: u64,
    /// Milliseconds the end of the output waits for Claude Code's exit status. Default 1000.
    #[map(default = 1_000)]
    pub exit_wait_ms: u64,
    /// Milliseconds a listing of the models an account may use waits for Claude Code to answer.
    /// Default 30000.
    #[map(default = 30_000)]
    pub models_wait_ms: u64,
    /// The Agent's `handler_settings` table as TOML (`allowed_tools`, `denied_tools`), which the
    /// Station fills in. Empty keeps the default list of tools.
    #[map(default = String::new())]
    pub settings_toml: String,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            executable: String::from("claude"),
            json_line_bytes: 64 * 1024 * 1024,
            csi_bytes: 64,
            tail_lines: 8,
            tail_line_bytes: 512,
            close_wait_ms: 2_000,
            exit_wait_ms: 1_000,
            models_wait_ms: 30_000,
            settings_toml: String::new(),
        }
    }
}

/// What an Agent's `handler_settings` change about a Mission's tools, on top of [`FIELD_ALLOWED`]
/// and [`FIELD_DENIED`] and, at `desk`, [`DESK_DENIED`]. Answer-only and assistant Sessions ignore
/// them. Unknown keys are refused.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeSettings {
    /// Claude Code permission rules added to the allow list. A rule for a tool the list keeps
    /// out (`WebFetch(domain:example.com)`, or `Bash` at `desk`) offers that tool and lifts its
    /// default denial.
    #[serde(default)]
    allowed_tools: Vec<String>,
    /// Claude Code permission rules added to the deny list; a deny rule beats an allow rule.
    #[serde(default)]
    denied_tools: Vec<String>,
}

impl ClaudeSettings {
    /// Reads an Agent's settings. Each rule must start with a letter, so that none is read as
    /// another option of the variadic flag it goes in.
    fn parse(toml_text: &str) -> Result<Self, String> {
        let settings: Self = toml::from_str(toml_text).map_err(|error| error.to_string())?;
        for rule in settings.allowed_tools.iter().chain(&settings.denied_tools) {
            if !rule.starts_with(|first: char| first.is_ascii_alphabetic()) {
                return Err(format!("{rule:?} is not a Claude Code permission rule"));
            }
        }
        Ok(settings)
    }
}

/// The `request_id` of the `initialize` request that lists models.
const MODELS_REQUEST: &str = "ai6-models";

/// The format of Claude Code's Session transcript.
pub const RECORD_FORMAT: &str = "claude-code-jsonl";

/// Claude Code's transcript of the Session `query` names, when it is on this host.
fn claude_record(query: &SessionRecordQuery) -> Option<SessionRecordFile> {
    if !records::file_safe(&query.session_id) {
        return None;
    }
    let root = records::configured_dir(&query.environment, "CLAUDE_CONFIG_DIR", ".claude")?;
    let name = format!("{}.jsonl", query.session_id);
    let path = records::find_file(&root.join("projects"), 1, &|file| file == name)?;
    Some(SessionRecordFile {
        path: path.to_string_lossy().into_owned(),
        format: RECORD_FORMAT.to_owned(),
    })
}

/// The models in Claude Code's answer to `initialize`; unknown when the answer has none. The
/// answer's `account` is never read.
fn models_of(answer: &Value) -> ModelListing {
    let response = &answer["response"];
    if response["subtype"] != "success" {
        return ModelListing::unknown("Claude Code refused the request to list its models");
    }
    let Some(models) = response["response"]["models"].as_array() else {
        return ModelListing::unknown("this Claude Code does not list its models");
    };
    ModelListing::of(
        models
            .iter()
            .filter_map(|model| {
                let id = model["value"].as_str()?;
                let resolved = model["resolvedModel"]
                    .as_str()
                    .filter(|resolved| *resolved != id);
                let description = match (model["description"].as_str(), resolved) {
                    (Some(text), Some(resolved)) => {
                        Some(format!("{text} (resolves to {resolved})"))
                    }
                    (Some(text), None) => Some(text.to_owned()),
                    (None, Some(resolved)) => Some(format!("resolves to {resolved}")),
                    (None, None) => None,
                };
                Some(ModelInfo {
                    id: id.to_owned(),
                    name: model["displayName"].as_str().map(str::to_owned),
                    description,
                })
            })
            .collect(),
    )
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
    settings: ClaudeSettings,
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
        let settings = ClaudeSettings::parse(&config.settings_toml).map_err(|reason| {
            refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!("handler_settings: {reason}"),
            )
        })?;
        Ok(Self { config, settings })
    }
}

impl AgentHandler for ClaudeHandler {
    /// Claude Code with `-p` and stream-json reads JSON lines from standard input, which must not
    /// be a terminal: on one it exits at once.
    fn stdio(&self) -> Result<ai6::HarnessStdio, ProviderError> {
        Ok(ai6::HarnessStdio::Json)
    }

    fn read_only_refusal(&self) -> Result<Option<String>, ProviderError> {
        Ok(None)
    }

    fn program(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.config.executable.clone()))
    }

    /// stream-json's user message takes base64 `image` blocks beside its text.
    fn image_input(&self) -> Result<Option<String>, ProviderError> {
        Ok(None)
    }

    /// Claude Code keeps each Session as `projects/<directory>/<session>.jsonl` under its
    /// configuration directory (`CLAUDE_CONFIG_DIR`, else `~/.claude`), one JSON object per
    /// line.
    fn session_record(
        &self,
        query: SessionRecordQuery,
    ) -> Result<Option<SessionRecordFile>, ProviderError> {
        Ok(claude_record(&query))
    }

    /// Claude Code's sandbox runs on macOS (Seatbelt, built in) and on Linux and WSL2 with
    /// `bwrap` and `socat` on the harness's `PATH`; on native Windows it runs commands
    /// unsandboxed (code.claude.com/docs/en/sandboxing). WSL1 passes this check and is caught
    /// by `failIfUnavailable`, which makes Claude Code exit at start.
    fn assistant_scripts(
        &self,
        environment: BTreeMap<String, String>,
    ) -> Result<Option<String>, ProviderError> {
        Ok(script_sandbox_missing(
            std::env::consts::OS,
            environment
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
                .map(|(_, path)| std::ffi::OsString::from(path))
                .or_else(|| std::env::var_os("PATH")),
        ))
    }

    /// Under `--restricted`, Claude Code 2.1.311 loads skills only from a `--plugin-dir`
    /// plugin, which holds exactly the assistant's.
    fn assistant_skills(&self) -> Result<Option<String>, ProviderError> {
        Ok(None)
    }

    /// Each rule of `allowed_tools` that a default list does not hold. A rule the settings deny as
    /// well allows nothing, so it is not one. A rule for what only `desk` keeps out is one however
    /// the Agent is configured, since any Agent can come to work at `desk`.
    fn weakenings(&self) -> Result<Vec<String>, ProviderError> {
        let denied = &self.settings.denied_tools;
        Ok(self
            .settings
            .allowed_tools
            .iter()
            .filter(|rule| !denied.contains(rule))
            .filter_map(|rule| {
                if DESK_DENIED.contains(&rule_tool(rule)) {
                    Some(format!(
                        "allowed_tools adds {rule} to what its desk Sessions may call"
                    ))
                } else if FIELD_ALLOWED.contains(&rule.as_str()) {
                    None
                } else {
                    Some(format!(
                        "allowed_tools adds {rule} to what its field Sessions may call"
                    ))
                }
            })
            .collect())
    }

    /// Claude Code answers an `initialize` control request with the models its account may
    /// pick (`models[].value`, `displayName`, `resolvedModel`), before any model turn and with
    /// no network. The list follows the kind of login the environment gives it (an API key, a
    /// subscription, a gateway), not a check of the account's standing.
    fn models(&self, environment: BTreeMap<String, String>) -> Result<ModelListing, ProviderError> {
        let wait = std::time::Duration::from_millis(self.config.models_wait_ms);
        let asked = listing::ask(
            &self.config.executable,
            &[
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
            ],
            &environment,
            wait,
            |harness| {
                harness.send(&json!({
                    "type": "control_request",
                    "request_id": MODELS_REQUEST,
                    "request": {"subtype": "initialize"}
                }))?;
                harness.next_where(|line| {
                    line["type"] == "control_response"
                        && line["response"]["request_id"] == MODELS_REQUEST
                })
            },
        );
        Ok(match asked {
            Ok(answer) => models_of(&answer),
            Err(reason) => ModelListing::unknown(reason),
        })
    }

    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn ai6::AgentSession>, ProviderError> {
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
        add_launch_options(
            &mut argv,
            &request
                .overlays
                .with_routing(&request.initial_context, true),
            request.model.as_deref(),
            request.effort.as_deref(),
        );
        add_budget(&mut argv, request.max_cost_micro_usd);
        let mode = Mode {
            project_settings: request.project_settings,
            project_mcp: request.project_mcp,
            answer_only: request.answer_only,
            commands: clearance_runs_commands(&request.clearance),
            reads_only: clearance_reads_only(&request.clearance),
        };
        add_settings(
            &mut argv,
            &request.environment,
            &request.working_directory,
            mode,
            &self.settings,
            request.assistant.as_ref(),
            &request.overlays,
        )?;
        let session = spawn(
            terminal,
            argv,
            &request.working_directory,
            &harness_environment(
                &request.environment,
                request.answer_only || request.assistant.is_some(),
                request.assistant.as_ref(),
            ),
        )?;
        let operation = ClaudeSession::new(session, None)
            .limited_by(&self.config)
            .restricted_to(restriction(request.answer_only, request.assistant.as_ref()));
        operation.send(&request.prompt)?;
        Ok(operation.into_object())
    }

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn ai6::AgentSession>, ProviderError> {
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
        add_launch_options(
            &mut argv,
            &request
                .overlays
                .with_routing(&request.initial_context, true),
            request.model.as_deref(),
            request.effort.as_deref(),
        );
        add_budget(&mut argv, request.max_cost_micro_usd);
        // Flags, not the Session's record, decide the tools: Claude Code 2.1.316 resumed a Session
        // started with `Bash` and without it on the flags of each launch, either way round.
        let mode = Mode {
            project_settings: request.project_settings,
            project_mcp: request.project_mcp,
            answer_only: request.answer_only,
            commands: clearance_runs_commands(&request.clearance),
            reads_only: clearance_reads_only(&request.clearance),
        };
        add_settings(
            &mut argv,
            &request.environment,
            &request.working_directory,
            mode,
            &self.settings,
            request.assistant.as_ref(),
            &request.overlays,
        )?;
        let session = spawn(
            terminal,
            argv,
            &request.working_directory,
            &harness_environment(
                &request.environment,
                request.answer_only || request.assistant.is_some(),
                request.assistant.as_ref(),
            ),
        )?;
        let operation = ClaudeSession::new(session, Some(expected_session_id))
            .limited_by(&self.config)
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

/// Adds the standing context, model and effort, each as one `--flag=VALUE` argument: a value
/// that starts with `-` is then never read as another option.
fn add_launch_options(
    argv: &mut Vec<String>,
    context: &[String],
    model: Option<&str>,
    effort: Option<&str>,
) {
    if !context.is_empty() {
        argv.push(format!("--append-system-prompt={}", context.join("\n\n")));
    }
    if let Some(model) = model {
        argv.push(format!("--model={model}"));
    }
    if let Some(effort) = effort {
        argv.push(format!("--effort={effort}"));
    }
}

/// Claude's own shell and file tools, turned off in remote-hands mode so that nothing the
/// harness does lands on the Station.
const OWN_TOOLS: [&str; 8] = [
    "Bash",
    "PowerShell",
    "Edit",
    "Write",
    "NotebookEdit",
    "Read",
    "Glob",
    "Grep",
];

/// The built-in tools a Mission's Session is offered at `field` and `00` (`--tools`), and at `desk`
/// less [`DESK_DENIED`]. `dontAsk` does not stop a tool that
/// needs no approval: without this list Claude Code 2.1.316 ran `Agent`, `CronCreate` and
/// `SendMessage` under it, and fetched `docs.python.org` with `WebFetch`, its rules empty. By
/// default it offers the four `Task` tools in place of `TodoWrite`
/// (code.claude.com/docs/en/tools-reference, "Task tool availability"), so both are named.
const FIELD_TOOLS: [&str; 13] = [
    "Read",
    "Glob",
    "Grep",
    "Edit",
    "Write",
    "NotebookEdit",
    "Bash",
    "PowerShell",
    "TodoWrite",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskUpdate",
];

/// A Mission Session's allow rules at `field` and `00`, besides its own MCP servers, and at `desk`
/// less [`DESK_DENIED`]. `Edit(/**)` is rooted at the
/// working directory for a rule given as a flag, and covers `Write` and `NotebookEdit`
/// (code.claude.com/docs/en/permissions, "Read and Edit").
const FIELD_ALLOWED: [&str; 11] = [
    "Read",
    "Glob",
    "Grep",
    "Edit(/**)",
    "Bash",
    "PowerShell",
    "TodoWrite",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskUpdate",
];

/// What a field Session is denied by name, besides not being offered it: a deny rule beats any
/// allow rule the user's or a pinned project's settings hold.
const FIELD_DENIED: [&str; 3] = ["WebFetch", "WebSearch", "Agent"];

/// What a `desk` Session is denied besides [`FIELD_DENIED`]: anything that runs a command, and
/// `theater_write`, which takes any path on the Theater where `Edit(/**)` stays in the working
/// directory. A rule in `allowed_tools` lifts each, as a weakening.
const DESK_DENIED: [&str; 4] = [
    "Bash",
    "PowerShell",
    "mcp__ai6-theater__theater_run",
    "mcp__ai6-theater__theater_write",
];

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

/// Turns WebFetch off altogether (code.claude.com/docs/en/env-vars, Claude Code 2.1.285 or
/// later), beside leaving it out of `--tools`.
const DISABLE_WEB_FETCH_ENV: &str = "CLAUDE_CODE_DISABLE_WEB_FETCH";

/// The harness's environment: `environment`, with the advisor turned off for a restricted
/// (answer-only or assistant) Session, auto memory off for an assistant, and WebFetch off for
/// an assistant whose network reaches no host.
fn harness_environment(
    environment: &BTreeMap<String, String>,
    restricted: bool,
    assistant: Option<&AssistantMode>,
) -> BTreeMap<String, String> {
    let mut environment = environment.clone();
    if restricted {
        environment.insert(DISABLE_ADVISOR_ENV.to_owned(), "1".to_owned());
    }
    if let Some(mode) = assistant {
        environment.insert(DISABLE_AUTO_MEMORY_ENV.to_owned(), "1".to_owned());
        if mode.network.reaches_nothing() {
            environment.insert(DISABLE_WEB_FETCH_ENV.to_owned(), "1".to_owned());
        }
    }
    environment
}

/// What a restricted (answer-only or assistant) Session may call: its MCP servers' tools, and
/// the built-in tools its mode offers ([`offered_tools`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Allowed {
    servers: Vec<String>,
    builtins: Vec<&'static str>,
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
            builtins: offered_tools(mode),
        }),
        None => answer_only.then(Allowed::default),
    }
}

/// The built-in tools an assistant's mode offers: the file tools with directories to write,
/// `Bash` with interpreters, `WebFetch` when the network reaches a host, `WebSearch` only
/// when it is open (a search takes no host list), and `Skill` with skills.
fn offered_tools(mode: &AssistantMode) -> Vec<&'static str> {
    let mut tools = Vec::new();
    if !mode.files.is_empty() {
        tools.extend(ASSISTANT_FILE_TOOLS.split(','));
    }
    if !mode.interpreters.is_empty() {
        tools.push("Bash");
    }
    if !mode.network.reaches_nothing() {
        tools.push("WebFetch");
    }
    if mode.network.open() {
        tools.push("WebSearch");
    }
    if !mode.skills.is_empty() {
        tools.push(SKILL_TOOL);
    }
    tools
}

/// The built-in tool that loads a skill, offered to an assistant with skills.
const SKILL_TOOL: &str = "Skill";

/// What AI6 decided about the Session's settings.
#[derive(Clone, Copy)]
struct Mode {
    project_settings: bool,
    project_mcp: bool,
    answer_only: bool,
    /// Whether the clearance allows a command tool ([`clearance_runs_commands`]).
    commands: bool,
    /// Whether the Session is held to reading until its plan passes ([`clearance_reads_only`]).
    reads_only: bool,
}

/// Every built-in tool an assistant Session is denied by name, besides `--tools=` turning the
/// built-in ones off. Unlike [`ANSWER_ONLY_DENIED`] it leaves MCP tools alone: the assistant's
/// servers are how it works.
const ASSISTANT_DENIED: &str = "Bash,PowerShell,Edit,Write,NotebookEdit,Read,Glob,Grep,\
                                WebFetch,WebSearch,Task,Agent,TodoWrite,Skill";

/// The built-in tools of an assistant whose mode names directories to write.
const ASSISTANT_FILE_TOOLS: &str = "Read,Write,Edit,Glob,Grep";

/// The rules that deny writing any path with a name starting with `.` (`.git`,
/// `.gitattributes`, `.claude`), whatever the allow rules say. Measured with Claude Code
/// 2.1.310 on Windows against a loopback stand-in for the API: Write and Edit to `.git/config`,
/// `.GIT/config`, `.git./config`, `PERSONAL\.git\config`, `.gitattributes`, `notes/.hidden`
/// and `scratch/.x` were refused, and so were writes through 8.3 short names (`GIT~1\config`,
/// `GITATT~1`), even after a read.
const DOT_PATHS_DENIED: [&str; 2] = ["Edit(**/.*)", "Edit(**/.*/**)"];

/// Model credentials Claude Code reads (code.claude.com/docs/en/env-vars), removed from every
/// script's environment even when the harness's own environment does not name them.
const MODEL_CREDENTIALS: [&str; 3] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
];

/// Adds the settings flags: assistant ([`add_assistant`]), answer-only ([`add_answer_only`]),
/// or the project settings AI6 allowed, the Station's MCP servers and the green ink gate.
fn add_settings(
    argv: &mut Vec<String>,
    environment: &BTreeMap<String, String>,
    working_directory: &str,
    mode: Mode,
    settings: &ClaudeSettings,
    assistant: Option<&AssistantMode>,
    overlays: &SessionOverlays,
) -> Result<(), ProviderError> {
    if let Some(assistant) = assistant {
        add_assistant(argv, environment, working_directory, assistant)?;
    } else if mode.answer_only {
        add_answer_only(argv);
    } else if mode.reads_only {
        add_project_settings(argv, false, false);
        add_read_only_tools(argv, environment);
        add_mcp(argv, environment);
        add_overlay_skills(argv, overlays);
        add_mission_settings(argv, environment, overlays);
    } else {
        add_project_settings(argv, mode.project_settings, mode.project_mcp);
        add_field_tools(argv, environment, settings, mode.commands);
        add_mcp(argv, environment);
        add_overlay_skills(argv, overlays);
        add_mission_settings(argv, environment, overlays);
    }
    Ok(())
}

/// Whether the launch has a native overlay skill to load: a plugin directory and a skill for it.
fn loads_overlay_skills(overlays: &SessionOverlays) -> bool {
    overlays.plugin_dir.is_some() && overlays.skills.iter().any(|skill| skill.native)
}

/// Loads a Mission's overlay skills: `Skill` joins `--tools`, closed by the next flag, each skill is
/// allowed as `Skill(ai6:<name>)`, and the Station's plugin directory is `--plugin-dir`. Claude Code
/// 2.1.318 listed and launched the plugin's skills with this under `dontAsk`, at `field` and `desk`,
/// and with no `Skill` in `--tools` it answered `No such tool available`.
fn add_overlay_skills(argv: &mut Vec<String>, overlays: &SessionOverlays) {
    let Some(dir) = overlays
        .plugin_dir
        .as_ref()
        .filter(|_| loads_overlay_skills(overlays))
    else {
        return;
    };
    let mut add_to = |flag: &str, added: &str| {
        if let Some(at) = argv.iter().position(|arg| arg == flag)
            && let Some(list) = argv.get_mut(at + 1)
        {
            if !list.is_empty() {
                list.push(',');
            }
            list.push_str(added);
        }
    };
    add_to("--tools", SKILL_TOOL);
    let rules: Vec<String> = overlays
        .skills
        .iter()
        .filter(|skill| skill.native)
        .map(|skill| format!("{SKILL_TOOL}({SKILLS_PLUGIN}:{})", skill.name))
        .collect();
    add_to("--allowedTools", &rules.join(","));
    argv.extend(["--plugin-dir".to_owned(), dir.clone()]);
}

/// The one `--settings` a Mission's Session gets: the green ink gate when it has a Station to ask,
/// and, with overlay skills, no skill's injected command: a skill Claude Code ships or the user's
/// directory holds is not one the Station checked, and its command would run before the gate saw it.
fn add_mission_settings(
    argv: &mut Vec<String>,
    environment: &BTreeMap<String, String>,
    overlays: &SessionOverlays,
) {
    let mut settings = gate_of(environment).unwrap_or_else(|| json!({}));
    if loads_overlay_skills(overlays) {
        settings["disableSkillShellExecution"] = json!(true);
    }
    if settings.as_object().is_some_and(|map| !map.is_empty()) {
        argv.extend(["--settings".to_owned(), settings.to_string()]);
    }
}

/// An assistant Session, built from its mode's parts: answer-only's restrictions
/// (`--restricted`, only the user's settings) with exactly the mode's MCP servers connected
/// (`--strict-mcp-config`) and their tools allowed without a prompt (`--allowedTools
/// mcp__<name>`), and only the built-in tools [`offered_tools`] names (`--tools`; none is
/// `--tools=`), each other one denied by name.
///
/// With any built-in tool the Session runs under `--permission-mode dontAsk`, which denies
/// every call no rule allows (code.claude.com/docs/en/permissions):
///
/// - `files`: the file tools, which `--restricted` confines to the working directory; reading
///   is allowed anywhere there, writing only under each directory (`Edit(/<dir>/**)`, relative
///   to the working directory; Edit rules cover Write), never to a path with a name starting
///   with `.`.
/// - `interpreters`: `Bash`, allowed only as `Bash(<interpreter> *)`, inside the sandbox
///   [`script_settings`] describes.
/// - network `limited` with hosts: `WebFetch`, allowed as `WebFetch(domain:<host>)` for each
///   host (which also lets the sandbox's scripts reach it); `open`: `WebFetch(domain:*)` and
///   `WebSearch`. WebFetch runs outside the sandbox, so its rules are its only limit.
/// - `skills`: `Skill`, each skill allowed as `Skill(ai6:<name>)`, and the mode's `skills_dir`
///   as `--plugin-dir`: under `--restricted`, Claude Code 2.1.311 loads no skill from the
///   working directory, the user's directory or an installed plugin, and does load a
///   `--plugin-dir` plugin's. Only the skills installed there can load, since an allow rule
///   does not keep another skill out. Without `skill_commands`, `disableSkillShellExecution`
///   keeps a skill's injected commands from running.
///
/// Every setting the parts call for goes in one `--settings` value. A `context_file` is passed
/// with `--append-system-prompt-file`.
fn add_assistant(
    argv: &mut Vec<String>,
    environment: &BTreeMap<String, String>,
    working_directory: &str,
    mode: &AssistantMode,
) -> Result<(), ProviderError> {
    mode.validate()
        .map_err(|reason| refuse(Status::GUATIAO_ERR_BAD_VALUE, reason))?;
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
    let offered = offered_tools(mode);
    if let Some(found) = managed_settings_at(std::env::consts::OS, std::path::Path::exists) {
        eprintln!(
            "ai6: Claude Code managed settings at {} apply to Moneypenny's harness too: they \
             merge with AI6's sandbox and permissions, and `allowManagedHooksOnly` there would \
             drop the WebFetch gate",
            found.display()
        );
    }
    // Under `dontAsk` Claude Code 2.1.310 still fetches its own documentation domains
    // (docs.python.org, developer.mozilla.org) when no rule names them, and WebFetch runs
    // outside the sandbox, so a hook gates every WebFetch: a `limited` mode to its listed
    // hosts, both off the deny list, by name and by the addresses a name resolves to.
    let web_gate = if offered.contains(&"WebFetch") {
        Some(hook_program(environment).ok_or_else(|| {
            refuse(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "no program to run as the assistant's WebFetch gate",
            )
        })?)
    } else {
        None
    };
    let mut denied: Vec<String> = ASSISTANT_DENIED
        .split(',')
        .filter(|tool| !offered.contains(tool))
        .map(str::to_owned)
        .collect();
    argv.push("--restricted".to_owned());
    if offered.is_empty() {
        argv.push("--tools=".to_owned());
    } else {
        argv.extend([
            "--tools".to_owned(),
            offered.join(","),
            "--permission-mode".to_owned(),
            "dontAsk".to_owned(),
        ]);
    }
    if !mode.files.is_empty() {
        denied.extend(DOT_PATHS_DENIED.map(str::to_owned));
        allowed.extend(["Read".to_owned(), "Glob".to_owned(), "Grep".to_owned()]);
        allowed.extend(mode.files.iter().map(|path| format!("Edit(/{path}/**)")));
    }
    allowed.extend(
        mode.interpreters
            .iter()
            .map(|interpreter| format!("Bash({interpreter} *)")),
    );
    if mode.network.open() {
        allowed.extend(["WebFetch(domain:*)".to_owned(), "WebSearch".to_owned()]);
    } else if !mode.network.reaches_nothing() {
        allowed.extend(
            mode.network
                .allowed_hosts
                .iter()
                .map(|host| format!("WebFetch(domain:{host})")),
        );
    }
    allowed.extend(
        mode.skills
            .iter()
            .map(|name| format!("{SKILL_TOOL}({SKILLS_PLUGIN}:{name})")),
    );
    argv.extend([
        "--disallowedTools".to_owned(),
        denied.join(","),
        "--allowedTools".to_owned(),
        allowed.join(","),
        "--setting-sources".to_owned(),
        "user".to_owned(),
        "--strict-mcp-config".to_owned(),
        "--mcp-config".to_owned(),
        json!({ "mcpServers": servers }).to_string(),
    ]);
    let skills_dir = mode.skills_dir.as_ref().filter(|_| !mode.skills.is_empty());
    if let Some(dir) = skills_dir {
        argv.extend(["--plugin-dir".to_owned(), dir.clone()]);
    }
    let mut settings = serde_json::Map::new();
    if !mode.interpreters.is_empty() {
        settings.extend(script_settings(mode, working_directory, environment)?);
    }
    if skills_dir.is_some() && !mode.skill_commands {
        settings.insert("disableSkillShellExecution".to_owned(), Value::Bool(true));
    }
    if let Some(program) = web_gate {
        // A limited network allows exactly its hosts; an open one allows any but the denied.
        let allow = if mode.network.open() {
            Vec::new()
        } else {
            mode.network.allowed_hosts.clone()
        };
        settings.extend(web_gate_settings(&program, &allow, &mode.network.denied));
    }
    if !settings.is_empty() {
        argv.extend(["--settings".to_owned(), Value::Object(settings).to_string()]);
    }
    if let Some(file) = &mode.context_file {
        argv.extend(["--append-system-prompt-file".to_owned(), file.clone()]);
    }
    Ok(())
}

/// The paths a sandboxed script may read on `os`, besides the working directory and the
/// install directories of its interpreters: the system's programs and libraries, only the
/// files under `/etc` that an interpreter, its resolver and TLS need, and the device files a
/// program opens. Anything else stays unreadable under `denyRead: ["/"]`: the Station's
/// configuration and secrets under `/etc`, `/opt`, `/var`, the home directory, `/proc` of other
/// processes. Measured for `python3` (stdlib, `ssl`, `sqlite3`, `os.urandom`, HTTPS through the
/// sandbox's proxy) on Linux with Claude Code 2.1.310; the macOS list is inferred. Other
/// platforms have no sandbox and never reach here.
fn script_read_roots(os: &str) -> &'static [&'static str] {
    match os {
        "macos" => &[
            "/usr",
            "/bin",
            "/sbin",
            "/System",
            "/Library/Developer/CommandLineTools",
            "/private/var/db/xcode_select_link",
            "/private/var/select",
            "/private/etc/ssl",
            "/private/etc/hosts",
            "/private/etc/resolv.conf",
            "/private/etc/localtime",
            "/private/etc/passwd",
            "/private/etc/group",
            "/private/etc/services",
            "/private/etc/protocols",
            "/dev/null",
            "/dev/zero",
            "/dev/random",
            "/dev/urandom",
            "/dev/tty",
        ],
        // Linux and WSL2. The two `/tmp` entries are the sandbox's own network relay sockets (a
        // script reaches its proxy only through them; measured with Claude Code 2.1.310), where
        // they are when the Session has no `CLAUDE_CODE_TMPDIR` of its own: re-opened by name,
        // so the rest of `/tmp`, AI6's own IPC sockets among it, stays unreadable.
        "linux" => &[
            "/usr",
            "/bin",
            "/sbin",
            "/lib",
            "/lib64",
            "/etc/ld.so.cache",
            "/etc/ld.so.conf",
            "/etc/ld.so.conf.d",
            "/etc/ssl",
            "/etc/pki",
            "/etc/crypto-policies",
            "/etc/ca-certificates",
            "/etc/localtime",
            "/etc/hosts",
            "/etc/resolv.conf",
            "/etc/nsswitch.conf",
            "/etc/host.conf",
            "/etc/gai.conf",
            "/etc/services",
            "/etc/protocols",
            "/etc/passwd",
            "/etc/group",
            "/etc/alternatives",
            "/dev/null",
            "/dev/zero",
            "/dev/random",
            "/dev/urandom",
            "/dev/tty",
            "/tmp/claude-http-*.sock",
            "/tmp/srt-mux-*.sock",
        ],
        _ => &[],
    }
}

/// The Claude Code settings that confine an assistant's scripts (code.claude.com/docs/en/
/// sandboxing and /settings-reference): the sandbox on, Claude Code exiting at start when it
/// cannot run it (`failIfUnavailable`), every command still checked against the allow rules
/// (`autoAllowBashIfSandboxed: false`) and none retried outside it
/// (`allowUnsandboxedCommands: false`). Reads are denied everywhere (`denyRead: ["/"]`) but the
/// working directory, the system paths an interpreter needs ([`script_read_roots`]) and the
/// interpreters' own install directories, with the Station's own paths denied again
/// ([`programs::station_paths`]). Scripts write only the mode's directories, never `read_only`, `.claude`
/// or a `.git` under the working directory (which an unsandboxed git could act on); reach only
/// the mode's hosts (every host under `open`) less the deny list's names and addresses
/// (`deniedDomains`); see no environment variable but [`SCRIPT_ENVIRONMENT`]
/// (`credentials.envVars`); and read no directory a credential variable names
/// (`credentials.files`). The file tools are kept to the working directory too
/// (`blockReadsOutsideWorkingDirectories`). With `unsandboxed` the sandbox is off and none of
/// these applies to scripts.
///
/// Refused: a deny list with a network or a category wider than one address while scripts
/// reach a host, since the sandbox's proxy matches `deniedDomains` by name and by single
/// address, not by network; and a variable of the environment `credentials.envVars` cannot
/// name, which would reach scripts.
fn script_settings(
    mode: &AssistantMode,
    working_directory: &str,
    environment: &BTreeMap<String, String>,
) -> Result<serde_json::Map<String, Value>, ProviderError> {
    let permissions = json!({ "blockReadsOutsideWorkingDirectories": true });
    if mode.unsandboxed {
        return Ok(object(
            json!({ "sandbox": { "enabled": false }, "permissions": permissions }),
        ));
    }
    let denied = DenyList::parse(&mode.network.denied)
        .map_err(|reason| refuse(Status::GUATIAO_ERR_BAD_VALUE, reason))?;
    let ranges = denied.ranges();
    if !mode.network.reaches_nothing() && !ranges.is_empty() {
        let named: Vec<String> = ranges.iter().map(|entry| format!("{entry:?}")).collect();
        return Err(refuse(
            Status::GUATIAO_ERR_UNSUPPORTED,
            format!(
                "Claude Code's sandbox proxy refuses hosts by name and by single address, not by \
                 network, so scripts cannot be kept from {}; deny single addresses and host \
                 names, or run no interpreter",
                named.join(", ")
            ),
        ));
    }
    if let Some(name) = environment.keys().find(|name| !variable_name(name)) {
        return Err(refuse(
            Status::GUATIAO_ERR_BAD_VALUE,
            format!(
                "the harness's environment has {name:?}, which Claude Code's sandbox cannot hide \
                 from scripts (credentials.envVars takes letters, digits and '_')"
            ),
        ));
    }
    let root = std::path::Path::new(working_directory);
    let under = |path: &str| root.join(path).to_string_lossy().into_owned();
    let mut deny_write: Vec<String> = mode.read_only.iter().map(|path| under(path)).collect();
    deny_write.push(under(".claude"));
    // A script could plant a `.git/config` with `core.fsmonitor` or a hook that the Station's
    // own unsandboxed git would run; her repositories' git directories live outside the
    // working directory, so nothing here needs a writable `.git`.
    deny_write.push(under(".git"));
    deny_write.extend(mode.files.iter().map(|dir| under(&format!("{dir}/.git"))));
    let interpreters: Vec<&str> = mode.interpreters.iter().map(String::as_str).collect();
    let program_dirs = programs::program_dirs(&interpreters, root, environment);
    let mut allow_read: Vec<String> = vec![working_directory.to_owned()];
    allow_read.extend(
        script_read_roots(std::env::consts::OS)
            .iter()
            .map(|dir| (*dir).to_owned()),
    );
    allow_read.extend(
        program_dirs
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned()),
    );
    // With a temporary directory of the Session's own, Claude Code 2.1.310 puts the sandbox's
    // network relay sockets there instead of `/tmp`: they are re-opened by name, since
    // re-opening the whole directory read-only covers the writable `$TMPDIR` under it and
    // `bwrap` then fails to start.
    if let Some(temp) = environment
        .get("CLAUDE_CODE_TMPDIR")
        .filter(|temp| std::path::Path::new(temp.as_str()).is_absolute())
    {
        let temp = std::path::Path::new(temp.as_str());
        for socket in ["claude-http-*.sock", "srt-mux-*.sock"] {
            allow_read.push(temp.join(socket).to_string_lossy().into_owned());
        }
    }
    let mut deny_read = vec!["/".to_owned()];
    deny_read.extend(
        programs::station_paths(root, &program_dirs, &mode.station_paths)
            .iter()
            .map(|path| path.to_string_lossy().into_owned()),
    );
    // Claude Code 2.1.310 does not open the sandbox to `WebFetch(domain:*)` given in
    // `--allowedTools`, so an open network names every host here itself.
    let hosts: Vec<&str> = match mode.network.policy.as_str() {
        "open" => vec!["*"],
        "limited" => mode
            .network
            .allowed_hosts
            .iter()
            .map(String::as_str)
            .collect(),
        _ => Vec::new(),
    };
    let mut hidden: std::collections::BTreeSet<&str> = MODEL_CREDENTIALS.into_iter().collect();
    hidden.extend(environment.keys().map(String::as_str).filter(|name| {
        !SCRIPT_ENVIRONMENT
            .iter()
            .any(|kept| kept.eq_ignore_ascii_case(name))
    }));
    let files: Vec<Value> = environment
        .iter()
        .filter(|(name, _)| credential_dir_var(name))
        .map(|(_, path)| json!({ "path": path, "mode": "deny" }))
        .collect();
    Ok(object(json!({
        "sandbox": {
            "enabled": true,
            "failIfUnavailable": true,
            "autoAllowBashIfSandboxed": false,
            "allowUnsandboxedCommands": false,
            "filesystem": {
                "allowWrite": mode.files.iter().map(|path| under(path)).collect::<Vec<_>>(),
                "denyWrite": deny_write,
                "denyRead": deny_read,
                "allowRead": allow_read,
            },
            "network": {
                "allowedDomains": hosts,
                "deniedDomains": denied.names_and_addresses(),
                "strictAllowlist": true,
            },
            "credentials": {
                "envVars": hidden
                    .into_iter()
                    .map(|name| json!({ "name": name, "mode": "deny" }))
                    .collect::<Vec<_>>(),
                "files": files,
            },
        },
        "permissions": permissions,
    })))
}

/// The members of a JSON object built with `json!`.
fn object(value: Value) -> serde_json::Map<String, Value> {
    match value {
        Value::Object(members) => members,
        _ => serde_json::Map::new(),
    }
}

/// The Claude Code settings that keep an assistant's WebFetch to `allow` and off `deny`: a
/// `PreToolUse` hook on `WebFetch` running `PROGRAM assistant web-gate [--hosts …] [--deny …]`,
/// which denies a fetch the deployment does not allow
/// (`ai6::directorates::section_00::gate::web_fetch_gate`), with hooks on whatever the user's
/// settings say and a hook that fails to run blocking the fetch (`onFailure`, Claude Code
/// 2.1.295 or later), as the green ink gate's. An empty `allow` (an open network) allows every
/// host but the denied ones.
fn web_gate_settings(
    program: &str,
    allow: &[String],
    deny: &[String],
) -> serde_json::Map<String, Value> {
    let mut args: Vec<String> = WEB_GATE_ARGS.map(str::to_owned).to_vec();
    if !allow.is_empty() {
        args.extend(["--hosts".to_owned(), allow.join(",")]);
    }
    if !deny.is_empty() {
        args.extend(["--deny".to_owned(), deny.join(",")]);
    }
    object(json!({
        "disableAllHooks": false,
        "hooks": {
            "PreToolUse": [{
                "matcher": "WebFetch",
                "hooks": [{
                    "type": "command",
                    "command": program,
                    "args": args,
                    "timeout": 60,
                    "onFailure": "block"
                }]
            }]
        }
    }))
}

/// The program a harness hook runs as AI6's gate: `AI6_GREEN_INK_GATE` from the Session
/// environment (which only the Agent's roster can set) when not blank, else the executable
/// hosting this handler (`ai6` on a Station or field runner).
fn hook_program(environment: &BTreeMap<String, String>) -> Option<String> {
    environment
        .get(GATE_PROGRAM_ENV)
        .filter(|program| !program.trim().is_empty())
        .cloned()
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        })
}

/// The file-based Claude Code managed settings on `os` that `exists` finds, the first of
/// `managed-settings.json` and `managed-settings.d` in its system directory
/// (code.claude.com/docs/en/managed-settings). Settings delivered by MDM or the claude.ai
/// console are not files here and are not found.
fn managed_settings_at(
    os: &str,
    exists: impl Fn(&std::path::Path) -> bool,
) -> Option<std::path::PathBuf> {
    let dir = match os {
        "macos" => "/Library/Application Support/ClaudeCode",
        "linux" => "/etc/claude-code",
        "windows" => r"C:\Program Files\ClaudeCode",
        _ => return None,
    };
    ["managed-settings.json", "managed-settings.d"]
        .into_iter()
        .map(|name| std::path::Path::new(dir).join(name))
        .find(|path| exists(path))
}

/// Why Claude Code's sandbox cannot run on `os` with programs found on `path`, or `None` when
/// it can.
fn script_sandbox_missing(os: &str, path: Option<std::ffi::OsString>) -> Option<String> {
    match os {
        "macos" => None,
        "linux" => {
            let missing: Vec<&str> = ["bwrap", "socat"]
                .into_iter()
                .filter(|program| {
                    !path.as_ref().is_some_and(|path| {
                        std::env::split_paths(path).any(|dir| dir.join(program).is_file())
                    })
                })
                .collect();
            (!missing.is_empty()).then(|| {
                format!(
                    "Claude Code's sandbox needs bubblewrap (bwrap) and socat on the harness's \
                     PATH, and {} is not there",
                    missing.join(" and ")
                )
            })
        }
        "windows" => Some(
            "Claude Code runs commands unsandboxed on native Windows; its sandbox runs on \
             macOS, Linux and WSL2"
                .to_owned(),
        ),
        other => Some(format!("Claude Code has no sandbox on {other}")),
    }
}

/// Whether `name` is a variable `credentials.envVars` accepts: a letter or `_`, then
/// letters, digits and `_`.
fn variable_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
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

/// The tool a permission rule is for: `WebFetch` for `WebFetch(domain:example.com)`.
fn rule_tool(rule: &str) -> &str {
    rule.split('(').next().unwrap_or(rule).trim()
}

/// A field Session's tools, each list closed by the next flag since all are variadic: under
/// `--permission-mode dontAsk` a call no rule allows is denied, with no prompt nobody is there to
/// answer (code.claude.com/docs/en/permissions). In remote-hands mode, when the Station serves a
/// Theater's tools, Claude's own shell and file tools are neither offered nor allowed and stay
/// denied whatever the settings say, so that nothing the harness does lands on the Station. With
/// `commands` false, the clearance's, the tools of [`DESK_DENIED`] are neither offered nor allowed
/// and are denied by name, unless a rule of the settings names one.
fn add_field_tools(
    argv: &mut Vec<String>,
    environment: &BTreeMap<String, String>,
    settings: &ClaudeSettings,
    commands: bool,
) {
    let remote_hands = has_endpoint(environment, "AI6_THEATER_MCP");
    let own = |tool: &str| remote_hands && OWN_TOOLS.contains(&tool);
    let kept_out = |tool: &str| !commands && DESK_DENIED.contains(&tool);
    let mut offered: Vec<&str> = FIELD_TOOLS
        .iter()
        .copied()
        .filter(|tool| !own(tool) && !kept_out(tool))
        .collect();
    for tool in settings.allowed_tools.iter().map(|rule| rule_tool(rule)) {
        if !tool.starts_with("mcp__") && !own(tool) && !offered.contains(&tool) {
            offered.push(tool);
        }
    }
    let desk_denied = DESK_DENIED.iter().copied().filter(|tool| {
        kept_out(tool) && !own(tool) && (remote_hands || !tool.starts_with("mcp__"))
    });
    let mut denied: Vec<&str> = FIELD_DENIED
        .iter()
        .copied()
        .chain(desk_denied)
        .filter(|tool| {
            !settings
                .allowed_tools
                .iter()
                .any(|rule| rule_tool(rule) == *tool)
        })
        .collect();
    if remote_hands {
        denied.extend(OWN_TOOLS);
    }
    denied.extend(settings.denied_tools.iter().map(String::as_str));
    let mut allowed = Vec::new();
    for (prefix, rule) in [
        ("AI6_CABLE_MCP", "mcp__ai6-cables"),
        ("AI6_THEATER_MCP", "mcp__ai6-theater"),
    ] {
        if has_endpoint(environment, prefix) {
            allowed.push(rule);
        }
    }
    allowed.extend(
        FIELD_ALLOWED
            .iter()
            .copied()
            .filter(|rule| !own(rule_tool(rule)) && !kept_out(rule_tool(rule))),
    );
    allowed.extend(settings.allowed_tools.iter().map(String::as_str));
    argv.extend([
        "--tools".to_owned(),
        offered.join(","),
        "--permission-mode".to_owned(),
        "dontAsk".to_owned(),
        "--disallowedTools".to_owned(),
        denied.join(","),
        "--allowedTools".to_owned(),
        allowed.join(","),
    ]);
}

/// The tools of a Session held to reading until its plan passes: reading and searching, and AI6's
/// Cable server, whose only tool it then has is `submit_plan`. What could change anything is denied
/// by name, and no setting adds to the list: `allowed_tools` is a field Session's. A resume at the
/// Mission's clearance builds the field list again, since each launch passes its own flags
/// (`--resume` keeps the conversation, not the tools).
fn add_read_only_tools(argv: &mut Vec<String>, environment: &BTreeMap<String, String>) {
    let mut allowed = vec!["Read", "Glob", "Grep"];
    if has_endpoint(environment, "AI6_CABLE_MCP") {
        allowed.insert(0, "mcp__ai6-cables");
    }
    let denied = FIELD_DENIED
        .iter()
        .chain(READ_ONLY_DENIED.iter())
        .copied()
        .collect::<Vec<_>>();
    argv.extend([
        "--tools".to_owned(),
        "Read,Glob,Grep".to_owned(),
        "--permission-mode".to_owned(),
        "dontAsk".to_owned(),
        "--disallowedTools".to_owned(),
        denied.join(","),
        "--allowedTools".to_owned(),
        allowed.join(","),
    ]);
}

/// What a Session held to reading is denied besides [`FIELD_DENIED`]: everything that writes or runs.
const READ_ONLY_DENIED: [&str; 12] = [
    "Edit",
    "Write",
    "NotebookEdit",
    "Bash",
    "PowerShell",
    "TodoWrite",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskUpdate",
    "mcp__ai6-theater__theater_run",
    "mcp__ai6-theater__theater_write",
];

/// Adds the Station's MCP servers: Cable tools, and, when the Station serves a Theater's tools
/// (remote-hands mode), those. [`add_field_tools`] allows them by server.
fn add_mcp(argv: &mut Vec<String>, environment: &BTreeMap<String, String>) {
    let mut servers = serde_json::Map::new();
    if has_endpoint(environment, "AI6_CABLE_MCP") {
        servers.insert("ai6-cables".to_owned(), http_server("AI6_CABLE_MCP"));
    }
    if has_endpoint(environment, "AI6_THEATER_MCP") {
        servers.insert("ai6-theater".to_owned(), http_server("AI6_THEATER_MCP"));
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

/// The green ink gate when the Session has a Station to ask (its Cable endpoint). The gate is the
/// `ai6` program hosting this handler, or the one `AI6_GREEN_INK_GATE` names.
fn gate_of(environment: &BTreeMap<String, String>) -> Option<Value> {
    let cable_url = environment
        .get("AI6_CABLE_MCP_URL")
        .filter(|_| has_endpoint(environment, "AI6_CABLE_MCP"))?;
    hook_program(environment).map(|program| gate_settings(&program, cable_url))
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
    /// The event a `result` line owes after the [`DENIED_EVENT`] read from the same line.
    pending: Mutex<Option<AgentEvent>>,
    /// Numbers the control requests written to Claude, for their `request_id`.
    requests: AtomicU64,
    /// How many of the last non-JSON lines a `failed` event quotes, and how long each may be.
    tail_lines: usize,
    tail_line_bytes: usize,
    /// How long `close` waits for the harness to have exited, for a terminal whose `kill` does
    /// not.
    close_wait_ms: i64,
    /// How long the end of the output waits for the harness's exit status.
    exit_wait_ms: i64,
}

impl ClaudeSession {
    fn new(terminal: Object<dyn TerminalSession>, expected_session_id: Option<SessionId>) -> Self {
        Self {
            terminal,
            output: Mutex::new(JsonLines::new(&Default::default())),
            session_id: Mutex::new(None),
            expected_session_id,
            eof: AtomicBool::new(false),
            eof_reported: AtomicBool::new(false),
            restricted: None,
            tripped: AtomicBool::new(false),
            tail: Mutex::new(VecDeque::new()),
            pending: Mutex::new(None),
            requests: AtomicU64::new(0),
            tail_lines: 8,
            tail_line_bytes: 512,
            close_wait_ms: 2_000,
            exit_wait_ms: 1_000,
        }
    }

    /// Takes the bounds and waits from the handler's configuration.
    fn limited_by(mut self, config: &ClaudeConfig) -> Self {
        let mut limits = ai6::station::limits::TerminalOutputLimits::default();
        limits.json_line_bytes = ai6::Limit::at(config.json_line_bytes);
        limits.csi_bytes = ai6::Limit::at(config.csi_bytes);
        self.output = Mutex::new(JsonLines::new(&limits));
        self.tail_lines = usize::try_from(config.tail_lines).unwrap_or(usize::MAX);
        self.tail_line_bytes = usize::try_from(config.tail_line_bytes).unwrap_or(usize::MAX);
        self.close_wait_ms = i64::try_from(config.close_wait_ms).unwrap_or(i64::MAX);
        self.exit_wait_ms = i64::try_from(config.exit_wait_ms).unwrap_or(i64::MAX);
        self
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
        let how = match self.exited(self.exit_wait_ms) {
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
            if tail.len() >= self.tail_lines {
                tail.pop_front();
            }
            let mut end = line.len().min(self.tail_line_bytes);
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
    /// its servers (`mcp__<server>__<tool>`) and the built-in tools its mode offers, after
    /// closing it; `None` when it calls none. The flags it was started with are meant to make
    /// this impossible; this backs them up. Which paths, commands and hosts a built-in tool may
    /// reach is the permission rules' and the sandbox's to enforce.
    fn tripped(&self, seen: &AgentEvent, value: &Value) -> Option<AgentEvent> {
        let allowed = self.restricted.as_ref()?;
        let mut forbidden = seen
            .tools
            .iter()
            .filter(|tool| {
                !(allowed
                    .servers
                    .iter()
                    .any(|server| tool.starts_with(&format!("mcp__{server}__")))
                    || allowed.builtins.iter().any(|builtin| builtin == *tool))
            })
            .cloned()
            .collect::<Vec<_>>();
        for block in unexpected_blocks(value) {
            if !forbidden.contains(&block) {
                forbidden.push(block);
            }
        }
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
        self.write_line(&user_input(text))
    }

    fn read_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        if self.tripped.load(Ordering::Acquire) {
            return Ok(None);
        }
        if let Some(event) = lock(&self.pending)?.take() {
            return Ok(Some(event));
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
            let different = self
                .expected_session_id
                .as_ref()
                .is_some_and(|expected| expected != &session_id);
            // Kept even when it is the wrong one: `session_id` is how the Station learns that.
            *lock(&self.session_id)? = Some(session_id);
            if different {
                return Err(refuse(
                    Status::GUATIAO_ERR_BAD_VALUE,
                    "Claude resumed a different Session than requested",
                ));
            }
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
            if let Some(ended) = self.tripped(&event, &value) {
                return Ok(Some(ended));
            }
            if let Some(denied) = decode_denials(&value) {
                *lock(&self.pending)? = Some(event);
                return Ok(Some(denied));
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

impl ai6::AgentSession for ClaudeSession {
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        Ok(lock(&self.session_id)?.clone().map(String::from))
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
        let _ = self.terminal.exit_status(self.close_wait_ms);
        Ok(())
    }
}

/// A user message in stream-json's shape: its text, after the images a message in AI6's
/// content form carries, each as a base64 `image` block.
fn user_input(text: &str) -> Value {
    let content: Vec<Value> = match ai6::MessageContent::decode(text) {
        Some(content) => content
            .images
            .iter()
            .map(|image| {
                json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": image.media_type,
                        "data": image.data,
                    },
                })
            })
            .chain([json!({"type": "text", "text": content.text})])
            .collect(),
        None => vec![json!({"type": "text", "text": text})],
    };
    json!({"type": "user", "message": {"role": "user", "content": content}})
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
            // Past its retries Claude Code words a gateway's refusal `API Error: Request rejected
            // (429)`, and says the status beside it (`api_error_status`, 2.1.316).
            let limited = value.get("api_error_status").and_then(Value::as_i64) == Some(429)
                || ["usage limit", "limit reached", "hit your limit"]
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
    let error_result = event_type == "result" && result_is_error(value);
    let kind = match event_type {
        "result" if error_result => "failed",
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
        .map(str::to_string)
        .or_else(|| error_result.then(|| result_errors(value)).flatten());
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

/// The event for the calls a `result` lists under `permission_denials`, a turn that otherwise
/// reads as a success: its text names each tool once, in the order first refused. A call's input
/// stays out, since it may hold anything the model wrote.
fn decode_denials(value: &Value) -> Option<AgentEvent> {
    if value.get("type").and_then(Value::as_str) != Some("result") {
        return None;
    }
    let mut tools: Vec<&str> = Vec::new();
    for denial in value.get("permission_denials")?.as_array()? {
        if let Some(tool) = denial.get("tool_name").and_then(Value::as_str)
            && !tools.contains(&tool)
        {
            tools.push(tool);
        }
    }
    (!tools.is_empty()).then(|| event(DENIED_EVENT, Some(tools.join(", "))))
}

/// Whether a `result` reports that the turn did not do its work: `is_error`, which Claude Code
/// 2.1.316 sets with `subtype: success` when no login reached a model, or an `error_*` subtype.
fn result_is_error(value: &Value) -> bool {
    value.get("is_error").and_then(Value::as_bool) == Some(true)
        || value
            .get("subtype")
            .and_then(Value::as_str)
            .is_some_and(|subtype| subtype.starts_with("error"))
}

/// The `errors` of a `result` with no `result` text, one per line.
fn result_errors(value: &Value) -> Option<String> {
    let errors: Vec<&str> = value
        .get("errors")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    (!errors.is_empty()).then(|| errors.join("\n"))
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

/// The content block types an `assistant` message of a restricted Session may hold: prose,
/// reasoning, and a call of a tool Claude Code runs (checked by name).
const EXPECTED_BLOCKS: [&str; 4] = ["text", "thinking", "redacted_thinking", "tool_use"];

/// Every content block of an `assistant` message whose type is not [`EXPECTED_BLOCKS`], as
/// `TYPE` or `TYPE:NAME`: a call the Messages API runs itself (`server_tool_use`,
/// `mcp_tool_use`), its result (`web_search_tool_result`, `mcp_tool_result`), a block with no
/// type, or one of a type Claude Code adds later.
fn unexpected_blocks(value: &Value) -> Vec<String> {
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    value
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| {
            let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
            if EXPECTED_BLOCKS.contains(&kind) {
                return None;
            }
            let kind = if kind.is_empty() { "untyped" } else { kind };
            Some(match block.get("name").and_then(Value::as_str) {
                Some(name) => format!("{kind}:{name}"),
                None => kind.to_owned(),
            })
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

    #[test]
    fn a_sessions_transcript_is_found_under_its_configuration_directory() {
        let config = tempfile::tempdir().unwrap();
        let project = config.path().join("projects").join("-work-repo");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("abc-123.jsonl"), "{}\n").unwrap();
        let query = |session: &str| SessionRecordQuery {
            session_id: session.to_owned(),
            working_directory: "/work/repo".to_owned(),
            environment: BTreeMap::from([(
                "CLAUDE_CONFIG_DIR".to_owned(),
                config.path().to_string_lossy().into_owned(),
            )]),
        };
        let found = claude_record(&query("abc-123")).unwrap();
        assert_eq!(
            std::path::PathBuf::from(found.path),
            project.join("abc-123.jsonl")
        );
        assert_eq!(found.format, RECORD_FORMAT);
        assert!(claude_record(&query("other")).is_none());
        assert!(claude_record(&query("../abc-123")).is_none());
    }

    /// Claude Code 2.1.316's answer to `initialize`, cut to what the listing reads, with the
    /// `account` it also carries: that is never read.
    fn initialize_answer() -> Value {
        json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": "ai6-models",
                "response": {
                    "account": {"email": "someone@example.invalid"},
                    "models": [
                        {"value": "default", "resolvedModel": "claude-opus-5-5",
                         "displayName": "Default (recommended)",
                         "description": "Use the default model", "supportsEffort": true},
                        {"value": "claude-haiku-5-5", "resolvedModel": "claude-haiku-5-5",
                         "displayName": "Haiku"},
                        {"displayName": "no value: not a model"}
                    ]
                }
            }
        })
    }

    #[test]
    fn the_models_claude_code_lists_are_read_from_its_initialize_answer() {
        let listing = models_of(&initialize_answer());
        assert!(listing.known);
        let ids: Vec<_> = listing
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        assert_eq!(ids, ["default", "claude-haiku-5-5"]);
        assert_eq!(
            listing.models[0].name.as_deref(),
            Some("Default (recommended)")
        );
        assert_eq!(
            listing.models[0].description.as_deref(),
            Some("Use the default model (resolves to claude-opus-5-5)")
        );
        assert_eq!(
            listing.models[1].description, None,
            "an id that resolves to itself"
        );
        assert!(
            !format!("{listing:?}").contains("someone@example"),
            "the account is never read"
        );
    }

    #[test]
    fn an_answer_without_models_or_a_refusal_is_unknown_never_empty() {
        for answer in [
            json!({"type": "control_response", "response": {"subtype": "error", "request_id": "ai6-models"}}),
            json!({"type": "control_response", "response": {"subtype": "success", "response": {"commands": []}}}),
        ] {
            let listing = models_of(&answer);
            assert!(!listing.known && listing.models.is_empty(), "{listing:?}");
            assert!(listing.unknown_because.is_some());
        }
    }

    /// Against the real Claude Code on this machine, with no login and no network: the harness
    /// answers `initialize` before any turn. Run with `AI6_TEST_LIVE_HARNESSES=1`.
    #[test]
    #[ignore = "needs Claude Code installed; set AI6_TEST_LIVE_HARNESSES=1 and run with --ignored"]
    fn live_claude_code_lists_models_with_no_login_and_no_turn() {
        assert!(
            std::env::var_os("AI6_TEST_LIVE_HARNESSES").is_some(),
            "set AI6_TEST_LIVE_HARNESSES=1"
        );
        let home = tempfile::tempdir().unwrap();
        let path = std::env::var("PATH").unwrap_or_default();
        let environment = BTreeMap::from([
            ("PATH".to_owned(), path),
            (
                "SYSTEMROOT".to_owned(),
                std::env::var("SYSTEMROOT").unwrap_or_default(),
            ),
            ("HOME".to_owned(), home.path().display().to_string()),
            ("USERPROFILE".to_owned(), home.path().display().to_string()),
            ("APPDATA".to_owned(), home.path().display().to_string()),
            ("LOCALAPPDATA".to_owned(), home.path().display().to_string()),
            (
                "CLAUDE_CONFIG_DIR".to_owned(),
                home.path().join(".claude").display().to_string(),
            ),
            ("ANTHROPIC_API_KEY".to_owned(), "dummy-not-a-key".to_owned()),
            (
                "ANTHROPIC_BASE_URL".to_owned(),
                "http://127.0.0.1:9".to_owned(),
            ),
        ]);
        let handler = ClaudeHandler::try_from(ClaudeConfig::default()).unwrap();
        let listing = handler.models(environment).unwrap();
        assert!(listing.known, "{listing:?}");
        assert!(
            listing.models.iter().any(|model| model.id == "opus"),
            "{listing:?}"
        );
    }
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
        fn spawn_private(
            &self,
            value: &ai6::guatiao::Value,
        ) -> Result<Object<dyn TerminalSession>, ProviderError> {
            self.spawn(value)
        }

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
            ..ClaudeConfig::default()
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
                    clearance: "field".to_owned(),
                    environment: environment.clone(),
                    model: Some("sonnet".to_owned()),
                    effort: Some("high".to_owned()),
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        let first_events = read_to_completion(&started);
        assert_eq!(started.session_id().unwrap(), Some(session_id.to_string()));
        assert!(first_events.iter().any(|event| {
            event.kind == "completed" && event.text.as_deref() == Some("first process turn")
        }));
        started.close().unwrap();

        let resumed = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
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
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        let resumed_events = read_to_completion(&resumed);
        assert_eq!(resumed.session_id().unwrap(), Some(session_id.to_string()));
        assert!(resumed_events.iter().any(|event| {
            event.kind == "completed" && event.text.as_deref() == Some("resumed process turn")
        }));
        resumed.close().unwrap();

        let specs = terminal.specs.lock().unwrap();
        assert_eq!(specs.len(), 2);
        assert!(
            specs[0]
                .argv
                .iter()
                .any(|arg| arg == "--append-system-prompt=context before the prompt")
        );
        assert!(
            specs[1]
                .argv
                .windows(2)
                .any(|pair| { pair == ["--resume", session_id.as_str()] })
        );
    }

    /// A model, effort or context starting with `-` stays the value of its own option.
    #[test]
    fn launch_option_values_are_never_read_as_options() {
        let mut argv = Vec::new();
        add_launch_options(
            &mut argv,
            &["- a bullet".to_owned()],
            Some("--dangerously-skip-permissions"),
            Some("-p"),
        );
        assert_eq!(
            argv,
            [
                "--append-system-prompt=- a bullet",
                "--model=--dangerously-skip-permissions",
                "--effort=-p",
            ]
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
        fn spawn_private(
            &self,
            value: &ai6::guatiao::Value,
        ) -> Result<Object<dyn TerminalSession>, ProviderError> {
            self.spawn(value)
        }

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
            ..ClaudeConfig::default()
        })
        .unwrap();
        let session = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
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
                    overlays: Default::default(),
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
        assert_eq!(session.session_id().unwrap(), Some(session_id.to_string()));

        let spec = spec.lock().unwrap().clone().unwrap();
        assert!(
            spec.argv
                .windows(2)
                .any(|pair| pair == ["--resume", "claude-session-42"])
        );
        assert!(spec.argv.iter().any(|arg| arg == "--model=sonnet"));
        assert!(spec.argv.iter().any(|arg| arg == "--effort=high"));
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
            ..ClaudeConfig::default()
        })
        .unwrap();
        let session = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
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
                    overlays: Default::default(),
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
            ..ClaudeConfig::default()
        })
        .unwrap();
        let session = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
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
                    overlays: Default::default(),
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
                ..ClaudeConfig::default()
            })
            .unwrap();
            let session = handler
                .resume(
                    AgentResume {
                        clearance: "desk".to_owned(),
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
                        overlays: Default::default(),
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
            ..ClaudeConfig::default()
        })
        .unwrap();
        let session = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
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
                    overlays: Default::default(),
                },
                terminal.into_object(),
            )
            .unwrap();
        let error = match session.next_event(50) {
            Ok(event) => panic!("a different provider Session ID was read as {event:?}"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("different Session"), "{error}");
        assert_eq!(
            session.session_id().unwrap().as_deref(),
            Some("different-session")
        );
    }

    /// Claude Code with stream-json input writes nothing until it has read a message, so a
    /// resume that waited for its output before sending one would never see any.
    #[test]
    fn resume_sends_its_message_at_once_to_a_harness_silent_until_it_reads_input() {
        let terminal = ProcessTerminal::new();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-silent-fixture".to_owned(),
            ..ClaudeConfig::default()
        })
        .unwrap();
        let started = Instant::now();
        let resumed = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
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
                    overlays: Default::default(),
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
            resumed.session_id().unwrap().as_deref(),
            Some("claude-silent-session")
        );
        resumed.close().unwrap();
    }

    #[test]
    fn a_message_with_images_reaches_claude_as_image_blocks_before_its_text() {
        let message = ai6::MessageContent {
            text: "What is wrong here?".to_owned(),
            images: vec![ai6::MessageImage {
                media_type: "image/png".to_owned(),
                data: "iVBORw0KGgo=".to_owned(),
            }],
        }
        .encode();
        assert_eq!(
            user_input(&message),
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [
                        {"type": "image", "source": {"type": "base64",
                            "media_type": "image/png", "data": "iVBORw0KGgo="}},
                        {"type": "text", "text": "What is wrong here?"}
                    ]
                }
            })
        );
        assert_eq!(
            ClaudeHandler::try_from(ClaudeConfig::default())
                .unwrap()
                .image_input()
                .unwrap(),
            None
        );
    }

    #[test]
    fn input_fixture_keeps_the_provider_stream_json_message_shape() {
        let encoded = user_input("continue the Mission");
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

    /// As Claude Code 2.1.316 reports a 429 from a LiteLLM gateway, past its retries.
    #[test]
    fn a_gateway_refusing_for_quota_is_a_limit_with_no_reset_time() {
        let (limit, text) = decode_limit(&serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "api_error_status": 429,
            "terminal_reason": "api_error",
            "result": "API Error: Request rejected (429) · litellm.RateLimitError: GeminiException \
                       - quota exceeded. Received Model Group=gemini-free",
        }))
        .expect("a limit");
        assert_eq!(limit.resets_at, None);
        assert!(text.unwrap().contains("Request rejected (429)"));
        assert!(
            decode_limit(&serde_json::json!({
                "type": "result",
                "is_error": true,
                "api_error_status": 500,
                "result": "API Error: 500 upstream",
            }))
            .is_none(),
            "another status is no limit"
        );
    }

    #[test]
    fn an_ordinary_error_result_is_not_mistaken_for_a_limit() {
        let value = json!({
            "type": "result",
            "is_error": true,
            "result": "Tool call failed"
        });
        let event = decode_event(&value, b"{}".to_vec()).unwrap();
        assert_eq!(event.kind, "failed");
        assert_eq!(event.text.as_deref(), Some("Tool call failed"));
        assert_eq!(event.limit, None);
    }

    /// As Claude Code 2.1.316 writes them. One that never logged in reaches no model and still
    /// says `success`; an interrupted turn has no `result` at all, only `errors`.
    #[test]
    fn a_result_reporting_an_error_does_not_complete_the_turn() {
        let not_logged_in = json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "result": "Not logged in \u{b7} Please run /login",
            "stop_reason": "stop_sequence",
            "terminal_reason": "api_error",
            "session_id": "c75d9e6e-f5fe-45d2-b05d-f2c88f0d132b",
            "total_cost_usd": 0,
        });
        let event = decode_event(&not_logged_in, b"{}".to_vec()).unwrap();
        assert_eq!(event.kind, "failed");
        assert_eq!(
            event.text.as_deref(),
            Some("Not logged in \u{b7} Please run /login")
        );

        let interrupted = json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "result": null,
            "errors": ["[ede_diagnostic] result_type=user last_content_type=n/a stop_reason=null"],
            "terminal_reason": "aborted_streaming",
            "stop_reason": null,
            "session_id": "b46e80da-b2eb-4336-b717-a18df95b965b",
        });
        let event = decode_event(&interrupted, b"{}".to_vec()).unwrap();
        assert_eq!(event.kind, "failed");
        assert!(event.text.unwrap().contains("ede_diagnostic"));

        let done =
            json!({"type": "result", "subtype": "success", "is_error": false, "result": "ok"});
        assert_eq!(
            decode_event(&done, b"{}".to_vec()).unwrap().kind,
            "completed"
        );
    }

    /// A harness that exits while its terminal keeps the output open (ConPTY) ends the Session
    /// with one `failed` event naming its exit code and quoting what it wrote last.
    #[test]
    fn a_harness_that_exits_early_fails_the_session_with_its_status_and_last_output() {
        let terminal = ProcessTerminal::kept_open();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-exit-fixture".to_owned(),
            ..ClaudeConfig::default()
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
                    overlays: Default::default(),
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

        let position = |flag: &str| argv.iter().position(|argument| argument == flag).unwrap();
        let config = &argv[position("--mcp-config") + 1];
        assert!(config.contains("${AI6_CABLE_MCP_URL}"));
        assert!(config.contains("${AI6_CABLE_MCP_TOKEN}"));
        assert!(!config.contains("session-secret-token"));
    }

    #[test]
    fn a_session_with_both_servers_allows_exactly_their_tools() {
        let environment = BTreeMap::from([
            ("AI6_CABLE_MCP_URL".to_owned(), "http://a/mcp".to_owned()),
            ("AI6_CABLE_MCP_TOKEN".to_owned(), "a".to_owned()),
            ("AI6_THEATER_MCP_URL".to_owned(), "http://b/mcp".to_owned()),
            ("AI6_THEATER_MCP_TOKEN".to_owned(), "b".to_owned()),
        ]);
        let mut argv = vec!["claude".to_owned()];
        add_field_tools(&mut argv, &environment, &ClaudeSettings::default(), true);
        let allowed = argv
            .iter()
            .position(|argument| argument == "--allowedTools")
            .unwrap();
        // Under dontAsk a tool no rule allows is denied, and an MCP tool is no exception. No
        // other server is named, so another server's tools stay denied.
        let rules: Vec<&str> = argv[allowed + 1].split(',').collect();
        assert_eq!(&rules[..2], ["mcp__ai6-cables", "mcp__ai6-theater"]);
        assert_eq!(
            rules
                .iter()
                .filter(|rule| rule.starts_with("mcp__"))
                .count(),
            2
        );
        assert_eq!(
            argv.iter()
                .filter(|argument| *argument == "--allowedTools")
                .count(),
            1
        );
    }

    #[test]
    fn a_session_without_ai6_servers_allows_no_server() {
        let mut argv = vec!["claude".to_owned()];
        add_mcp(&mut argv, &BTreeMap::new());
        assert_eq!(argv, ["claude"]);
        add_field_tools(
            &mut argv,
            &BTreeMap::new(),
            &ClaudeSettings::default(),
            true,
        );
        assert!(!argv.iter().any(|argument| argument.contains("mcp__")));
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
        let settings = ClaudeSettings {
            allowed_tools: vec!["Bash".to_owned(), "Edit(/src/**)".to_owned()],
            denied_tools: Vec::new(),
        };

        add_field_tools(&mut argv, &environment, &settings, true);
        add_mcp(&mut argv, &environment);

        let position = |flag: &str| argv.iter().position(|argument| argument == flag).unwrap();
        let disallowed = &argv[position("--disallowedTools") + 1];
        for tool in ["Bash", "Edit", "Write", "Read", "Grep"] {
            assert!(disallowed.split(',').any(|name| name == tool), "{tool}");
        }
        let offered = &argv[position("--tools") + 1];
        for tool in OWN_TOOLS {
            assert!(!offered.split(',').any(|name| name == tool), "{offered}");
        }
        // A setting's rule goes on the list, and the deny rule beats it.
        let allowed = &argv[position("--allowedTools") + 1];
        assert_eq!(
            allowed,
            "mcp__ai6-theater,TodoWrite,TaskCreate,TaskGet,TaskList,TaskUpdate,Bash,Edit(/src/**)"
        );
        let config = &argv[position("--mcp-config") + 1];
        assert!(config.contains("ai6-theater") && !config.contains("ai6-cables"));
        assert!(config.contains("${AI6_THEATER_MCP_TOKEN}"));
        assert!(
            !argv
                .iter()
                .any(|argument| argument.contains("theater-secret-token"))
        );
        // A variadic flag is closed by the next flag, so no value runs on into another.
        assert!(position("--tools") < position("--permission-mode"));
        assert!(position("--disallowedTools") < position("--allowedTools"));
        assert!(position("--allowedTools") < position("--mcp-config"));
    }

    /// What a field Session's tool flags say, as `(offered, denied, allowed)`, after a start.
    fn field_flags(argv: &[String]) -> (String, String, String) {
        assert_eq!(flag(argv, "--permission-mode").as_deref(), Some("dontAsk"));
        (
            flag(argv, "--tools").unwrap(),
            flag(argv, "--disallowedTools").unwrap(),
            flag(argv, "--allowedTools").unwrap(),
        )
    }

    const DEFAULT_OFFERED: &str = "Read,Glob,Grep,Edit,Write,NotebookEdit,Bash,PowerShell,\
                                   TodoWrite,TaskCreate,TaskGet,TaskList,TaskUpdate";
    const DEFAULT_ALLOWED: &str = "Read,Glob,Grep,Edit(/**),Bash,PowerShell,\
                                   TodoWrite,TaskCreate,TaskGet,TaskList,TaskUpdate";

    #[test]
    fn a_field_session_runs_by_the_explicit_list_on_start_and_resume() {
        let mut environment = cable_environment();
        environment.insert(
            "CLAUDE_FIXTURE_SESSION_ID".to_owned(),
            "claude-field-session".to_owned(),
        );
        let terminal = ProcessTerminal::new();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-field-fixture".to_owned(),
            ..ClaudeConfig::default()
        })
        .unwrap();
        let working_directory = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let started = handler
            .start(
                AgentStart {
                    mission_id: "mission-field".parse().unwrap(),
                    agent_id: "007".to_owned(),
                    prompt: "work".to_owned(),
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
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
                    mission_id: "mission-field".parse().unwrap(),
                    session_id: SessionId::try_new("claude-field-session").unwrap(),
                    message: None,
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
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        resumed.close().unwrap();
        let specs = terminal.specs.lock().unwrap();
        for spec in specs.iter() {
            let (offered, denied, allowed) = field_flags(&spec.argv);
            assert_eq!(offered, DEFAULT_OFFERED);
            assert_eq!(denied, "WebFetch,WebSearch,Agent");
            assert_eq!(allowed, format!("mcp__ai6-cables,{DEFAULT_ALLOWED}"));
            // A gated Session keeps the same list: the gate decides each command.
            let settings = settings_of(&spec.argv).expect("the gate");
            let hook = &settings["hooks"]["PreToolUse"][0];
            assert_eq!(hook["matcher"], "Bash|PowerShell");
        }
        assert_eq!(specs.len(), 2);
    }

    #[test]
    fn a_field_session_without_a_station_keeps_the_list_and_has_no_gate() {
        let mut argv = Vec::new();
        add_settings(
            &mut argv,
            &BTreeMap::new(),
            "wd",
            Mode {
                project_settings: false,
                project_mcp: false,
                answer_only: false,
                commands: true,
                reads_only: false,
            },
            &ClaudeSettings::default(),
            None,
            &SessionOverlays::default(),
        )
        .unwrap();
        let (offered, _, allowed) = field_flags(&argv);
        assert_eq!(offered, DEFAULT_OFFERED);
        assert_eq!(allowed, DEFAULT_ALLOWED);
        assert!(!argv.iter().any(|argument| argument == "--settings"));
    }

    #[test]
    fn an_agents_settings_change_the_list_and_a_tool_they_add_is_offered() {
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            settings_toml: "allowed_tools = [\"WebFetch(domain:example.com)\", \"Skill\",                             \"mcp__docs\"]\ndenied_tools = [\"Bash(rm *)\"]\n"
                .to_owned(),
            ..ClaudeConfig::default()
        })
        .unwrap();
        let mut argv = Vec::new();
        add_field_tools(&mut argv, &BTreeMap::new(), &handler.settings, true);
        let (offered, denied, allowed) = field_flags(&argv);
        assert_eq!(offered, format!("{DEFAULT_OFFERED},WebFetch,Skill"));
        assert_eq!(
            denied, "WebSearch,Agent,Bash(rm *)",
            "the rule lifts its denial"
        );
        assert_eq!(
            allowed,
            format!("{DEFAULT_ALLOWED},WebFetch(domain:example.com),Skill,mcp__docs")
        );
        assert_eq!(
            handler.weakenings().unwrap(),
            [
                "allowed_tools adds WebFetch(domain:example.com) to what its field Sessions may call",
                "allowed_tools adds Skill to what its field Sessions may call",
                "allowed_tools adds mcp__docs to what its field Sessions may call",
            ]
        );
    }

    #[test]
    fn only_a_rule_beyond_the_default_list_weakens_a_session() {
        let weakenings = |settings_toml: &str| {
            ClaudeHandler::try_from(ClaudeConfig {
                settings_toml: settings_toml.to_owned(),
                ..ClaudeConfig::default()
            })
            .unwrap()
            .weakenings()
            .unwrap()
        };
        assert!(weakenings("").is_empty());
        assert!(weakenings("denied_tools = [\"Bash\", \"Read\"]").is_empty());
        assert!(weakenings("allowed_tools = [\"Read\", \"Edit(/**)\"]").is_empty());
        assert!(
            weakenings("allowed_tools = [\"WebFetch\"]\ndenied_tools = [\"WebFetch\"]").is_empty(),
            "a deny rule beats the allow rule"
        );
        assert_eq!(
            weakenings("allowed_tools = [\"Write\"]").len(),
            1,
            "a bare Write reaches outside the working directory"
        );
    }

    const DESK_OFFERED: &str = "Read,Glob,Grep,Edit,Write,NotebookEdit,\
                                TodoWrite,TaskCreate,TaskGet,TaskList,TaskUpdate";
    const DESK_ALLOWED: &str = "Read,Glob,Grep,Edit(/**),\
                                TodoWrite,TaskCreate,TaskGet,TaskList,TaskUpdate";

    /// The tool flags of a start, then a resume, at `clearance` under `settings_toml`.
    fn flags_at(clearance: &str, settings_toml: &str) -> Vec<(String, String, String)> {
        let mut environment = cable_environment();
        environment.insert(
            "CLAUDE_FIXTURE_SESSION_ID".to_owned(),
            "claude-clearance-session".to_owned(),
        );
        let terminal = ProcessTerminal::new();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-field-fixture".to_owned(),
            settings_toml: settings_toml.to_owned(),
            ..ClaudeConfig::default()
        })
        .unwrap();
        let working_directory = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let started = handler
            .start(
                AgentStart {
                    mission_id: "mission-clearance".parse().unwrap(),
                    agent_id: "007".to_owned(),
                    prompt: "work".to_owned(),
                    initial_context: Vec::new(),
                    working_directory: working_directory.clone(),
                    clearance: clearance.to_owned(),
                    environment: environment.clone(),
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    mission_id: "mission-clearance".parse().unwrap(),
                    session_id: SessionId::try_new("claude-clearance-session").unwrap(),
                    message: None,
                    working_directory,
                    clearance: clearance.to_owned(),
                    environment,
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                    initial_context: Vec::new(),
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        resumed.close().unwrap();
        let specs = terminal.specs.lock().unwrap();
        assert_eq!(specs.len(), 2);
        specs.iter().map(|spec| field_flags(&spec.argv)).collect()
    }

    #[test]
    fn a_sessions_tools_follow_its_clearance_on_start_and_resume() {
        let field = (
            DEFAULT_OFFERED.to_owned(),
            "WebFetch,WebSearch,Agent".to_owned(),
            format!("mcp__ai6-cables,{DEFAULT_ALLOWED}"),
        );
        let desk = (
            DESK_OFFERED.to_owned(),
            "WebFetch,WebSearch,Agent,Bash,PowerShell".to_owned(),
            format!("mcp__ai6-cables,{DESK_ALLOWED}"),
        );
        for clearance in ["field", "00"] {
            assert_eq!(flags_at(clearance, ""), [field.clone(), field.clone()]);
        }
        // A value the handler does not know, an empty one included, gets the least it can give.
        for clearance in ["desk", "developer", ""] {
            assert_eq!(
                flags_at(clearance, ""),
                [desk.clone(), desk.clone()],
                "{clearance:?}"
            );
        }
    }

    #[test]
    fn a_desk_session_is_given_a_command_tool_only_by_a_rule_reported_as_a_weakening() {
        let settings = "allowed_tools = [\"Bash(git status)\"]\n";
        let flags = flags_at("desk", settings);
        assert_eq!(
            flags[0],
            (
                format!("{DESK_OFFERED},Bash"),
                "WebFetch,WebSearch,Agent,PowerShell".to_owned(),
                format!("mcp__ai6-cables,{DESK_ALLOWED},Bash(git status)"),
            ),
            "the tool it names, and no other"
        );
        assert_eq!(flags[1], flags[0]);
        let weakenings = |settings_toml: &str| {
            ClaudeHandler::try_from(ClaudeConfig {
                settings_toml: settings_toml.to_owned(),
                ..ClaudeConfig::default()
            })
            .unwrap()
            .weakenings()
            .unwrap()
        };
        assert_eq!(
            weakenings(settings),
            ["allowed_tools adds Bash(git status) to what its desk Sessions may call"]
        );
        assert_eq!(
            weakenings("allowed_tools = [\"PowerShell\"]"),
            ["allowed_tools adds PowerShell to what its desk Sessions may call"]
        );
        assert_eq!(
            weakenings("allowed_tools = [\"mcp__ai6-theater__theater_run\"]"),
            ["allowed_tools adds mcp__ai6-theater__theater_run to what its desk Sessions may call"]
        );
        assert!(
            weakenings("allowed_tools = [\"Bash\"]\ndenied_tools = [\"Bash\"]").is_empty(),
            "a deny rule beats the allow rule"
        );
    }

    #[test]
    fn a_desk_session_on_a_remote_theater_may_read_it_but_not_run_or_write() {
        let environment = BTreeMap::from([
            ("AI6_THEATER_MCP_URL".to_owned(), "http://b/mcp".to_owned()),
            ("AI6_THEATER_MCP_TOKEN".to_owned(), "b".to_owned()),
        ]);
        let flags = |commands: bool, settings: &ClaudeSettings| {
            let mut argv = Vec::new();
            add_field_tools(&mut argv, &environment, settings, commands);
            field_flags(&argv)
        };
        let tasks = "TodoWrite,TaskCreate,TaskGet,TaskList,TaskUpdate";
        let own = "Bash,PowerShell,Edit,Write,NotebookEdit,Read,Glob,Grep";
        assert_eq!(
            flags(false, &ClaudeSettings::default()),
            (
                tasks.to_owned(),
                format!(
                    "WebFetch,WebSearch,Agent,mcp__ai6-theater__theater_run,\
                     mcp__ai6-theater__theater_write,{own}"
                ),
                format!("mcp__ai6-theater,{tasks}"),
            )
        );
        assert_eq!(
            flags(true, &ClaudeSettings::default()).1,
            format!("WebFetch,WebSearch,Agent,{own}"),
            "field keeps every Theater tool"
        );
        let consent = ClaudeSettings {
            allowed_tools: vec![
                "mcp__ai6-theater__theater_run".to_owned(),
                "Bash".to_owned(),
            ],
            denied_tools: Vec::new(),
        };
        let (offered, denied, _) = flags(false, &consent);
        assert_eq!(
            denied,
            format!("WebFetch,WebSearch,Agent,mcp__ai6-theater__theater_write,{own}"),
            "a rule lifts the Theater tool it names, and never Claude's own shell on the Station"
        );
        assert_eq!(offered, tasks);
    }

    #[test]
    fn a_session_held_to_its_plan_reads_and_calls_ai6_and_a_resume_at_field_has_its_tools() {
        let flags = flags_at(
            ai6::READ_CLEARANCE,
            "allowed_tools = [\"Bash\", \"Edit(/**)\"]
",
        );
        assert_eq!(flags[0].0, "Read,Glob,Grep", "nothing offered but reading");
        assert_eq!(
            flags[0].2, "mcp__ai6-cables,Read,Glob,Grep",
            "no setting adds to it"
        );
        for denied in [
            "Edit",
            "Write",
            "NotebookEdit",
            "Bash",
            "PowerShell",
            "TodoWrite",
        ] {
            assert!(
                flags[0].1.split(',').any(|tool| tool == denied),
                "{denied} denied by name: {}",
                flags[0].1
            );
        }
        assert_eq!(
            flags[1], flags[0],
            "a resume held to the plan is held the same"
        );
        let field = flags_at("field", "");
        assert_eq!(
            field[1].0, DEFAULT_OFFERED,
            "a resume at field gets the Mission's tools"
        );
        assert!(field[1].2.contains("Edit(/**)") && field[1].2.contains("Bash"));
    }

    #[test]
    fn this_handler_can_hold_a_session_to_its_plan() {
        let handler = ClaudeHandler::try_from(ClaudeConfig::default()).unwrap();
        assert_eq!(handler.read_only_refusal().unwrap(), None);
    }

    #[test]
    fn a_desk_session_without_a_station_has_no_server_and_no_command_tool() {
        let mut argv = Vec::new();
        add_settings(
            &mut argv,
            &BTreeMap::new(),
            "wd",
            Mode {
                project_settings: false,
                project_mcp: false,
                answer_only: false,
                commands: false,
                reads_only: false,
            },
            &ClaudeSettings::default(),
            None,
            &SessionOverlays::default(),
        )
        .unwrap();
        let (offered, denied, allowed) = field_flags(&argv);
        assert_eq!(offered, DESK_OFFERED);
        assert_eq!(denied, "WebFetch,WebSearch,Agent,Bash,PowerShell");
        assert_eq!(allowed, DESK_ALLOWED);
    }

    #[test]
    fn answer_only_and_assistant_flags_do_not_depend_on_the_clearance() {
        let flags = |commands: bool, answer_only: bool, assistant: Option<&AssistantMode>| {
            let mut argv = Vec::new();
            add_settings(
                &mut argv,
                &assistant_environment(),
                "wd",
                Mode {
                    project_settings: false,
                    project_mcp: false,
                    answer_only,
                    commands,
                    reads_only: false,
                },
                &ClaudeSettings::default(),
                assistant,
                &SessionOverlays::default(),
            )
            .unwrap();
            argv
        };
        assert_eq!(flags(true, true, None), flags(false, true, None));
        let mode = AssistantMode::moneypenny();
        assert_eq!(
            flags(true, false, Some(&mode)),
            flags(false, false, Some(&mode))
        );
    }

    #[test]
    fn handler_settings_it_cannot_read_are_refused() {
        for settings_toml in [
            "allowed = [\"Bash\"]",
            "allowed_tools = \"Bash\"",
            "allowed_tools = [\"--dangerously-skip-permissions\"]",
            "denied_tools = [\"\"]",
        ] {
            let error = ClaudeHandler::try_from(ClaudeConfig {
                settings_toml: settings_toml.to_owned(),
                ..ClaudeConfig::default()
            })
            .err()
            .unwrap_or_else(|| panic!("{settings_toml} was accepted"));
            assert_eq!(
                error.status,
                Status::GUATIAO_ERR_BAD_VALUE,
                "{settings_toml}"
            );
        }
    }

    #[test]
    fn answer_only_and_assistant_sessions_ignore_the_settings() {
        let settings = ClaudeSettings {
            allowed_tools: vec!["WebFetch".to_owned()],
            denied_tools: vec!["Read".to_owned()],
        };
        let flags =
            |answer_only: bool, assistant: Option<&AssistantMode>, settings: &ClaudeSettings| {
                let mut argv = Vec::new();
                add_settings(
                    &mut argv,
                    &assistant_environment(),
                    "wd",
                    Mode {
                        project_settings: false,
                        project_mcp: false,
                        answer_only,
                        commands: true,
                        reads_only: false,
                    },
                    settings,
                    assistant,
                    &SessionOverlays::default(),
                )
                .unwrap();
                argv
            };
        let answer_only = flags(true, None, &settings);
        assert_eq!(answer_only, flags(true, None, &ClaudeSettings::default()));
        assert!(
            !answer_only
                .iter()
                .any(|argument| argument == "--permission-mode")
        );
        let (_, _, mode) = script_mode("denied", &[], false);
        assert_eq!(
            flags(false, Some(&mode), &settings),
            flags(false, Some(&mode), &ClaudeSettings::default())
        );
    }

    /// Terminal output holding one `result` line, as a resumed Session reads it.
    fn result_session(result: Value) -> Object<dyn AgentSession> {
        let mut input = result.to_string().into_bytes();
        input.push(b'\n');
        let terminal = ResumeTerminal {
            spec: Arc::new(Mutex::new(None)),
            input,
            writes: Arc::new(Mutex::new(Vec::new())),
        };
        ClaudeHandler::try_from(ClaudeConfig::default())
            .unwrap()
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
                    mission_id: "mission-denied".parse().unwrap(),
                    session_id: SessionId::try_new("claude-session-denied").unwrap(),
                    message: None,
                    working_directory: "wd".to_owned(),
                    environment: BTreeMap::new(),
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                    initial_context: Vec::new(),
                    overlays: Default::default(),
                },
                terminal.into_object(),
            )
            .unwrap()
    }

    /// The shape Claude Code 2.1.316 writes after a turn whose calls were refused: a success.
    #[test]
    fn a_denied_call_is_reported_before_the_turn_ends_and_its_input_stays_out() {
        let session = result_session(json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "finished",
            "permission_denials": [
                {"tool_name": "Write", "tool_use_id": "t1", "tool_input": {"content": "secret"}},
                {"tool_name": "Bash", "tool_use_id": "t2", "tool_input": {"command": "secret"}},
                {"tool_name": "Write", "tool_use_id": "t3", "tool_input": {"content": "secret"}},
            ],
        }));
        let events = std::iter::from_fn(|| session.next_event(50).unwrap()).collect::<Vec<_>>();
        let kinds_and_texts = events
            .iter()
            .map(|event| (event.kind.as_str(), event.text.as_deref()))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds_and_texts,
            [
                (DENIED_EVENT, Some("Write, Bash")),
                ("completed", Some("finished")),
            ]
        );
        assert!(events[0].tools.is_empty(), "a refused call did not run");
        assert_eq!(events[0].data, None, "the raw line holds the calls' input");
    }

    #[test]
    fn a_turn_with_nothing_denied_reports_only_its_end() {
        for denials in [json!([]), Value::Null] {
            let session = result_session(json!({
                "type": "result",
                "subtype": "success",
                "result": "finished",
                "permission_denials": denials,
            }));
            let kinds = std::iter::from_fn(|| session.next_event(50).unwrap())
                .map(|event| event.kind)
                .collect::<Vec<_>>();
            assert_eq!(kinds, ["completed"]);
        }
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
        add_mission_settings(&mut argv, &environment, &SessionOverlays::default());

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
        add_mission_settings(&mut argv, &cable_environment(), &SessionOverlays::default());
        let settings: Value = serde_json::from_str(&argv[2]).unwrap();
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            std::env::current_exe().unwrap().to_string_lossy().as_ref()
        );

        // Without a Station to ask there is no gate.
        let mut argv = vec!["claude".to_owned()];
        add_mission_settings(&mut argv, &BTreeMap::new(), &SessionOverlays::default());
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
            ..ClaudeConfig::default()
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
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
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
                    overlays: Default::default(),
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
            ..ClaudeConfig::default()
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
            overlays: Default::default(),
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
                    clearance: "field".to_owned(),
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
                    overlays: Default::default(),
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
        assert!(
            !answer_only.iter().any(|a| a.contains("dontAsk")),
            "answer-only has no mode"
        );
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
        let lists = [
            OWN_TOOLS.join(","),
            ANSWER_ONLY_DENIED.to_owned(),
            ASSISTANT_DENIED.to_owned(),
            FIELD_DENIED.join(","),
        ];
        for list in lists {
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
            ..ClaudeConfig::default()
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
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    clearance: "desk".to_owned(),
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
                    overlays: Default::default(),
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
            assert!(
                argv.iter()
                    .any(|a| a == "--append-system-prompt=You are Moneypenny."),
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
            ..ClaudeConfig::default()
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
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    clearance: "desk".to_owned(),
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
                    overlays: Default::default(),
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
            add_assistant(&mut argv, &environment, "/wd", &cables).is_err(),
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
        add_assistant(&mut argv, &environment, "/wd", &cables).unwrap();
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

    /// Measured with Claude Code 2.1.311 against a loopback stand-in for the API: under
    /// `--restricted` only a `--plugin-dir` plugin's skills loaded, and with
    /// `disableSkillShellExecution` a skill's injected command was replaced, not run.
    #[test]
    fn an_assistant_with_skills_loads_only_its_plugin_directory_and_runs_no_command() {
        let environment = assistant_environment();
        let dir = std::env::temp_dir()
            .join("conversation")
            .join(".claude")
            .to_string_lossy()
            .into_owned();
        let skills = AssistantMode {
            skills: vec!["report-writer".to_owned(), "charts".to_owned()],
            skills_dir: Some(dir.clone()),
            ..AssistantMode::moneypenny()
        };
        let with_files = AssistantMode {
            files: vec!["personal".to_owned()],
            ..skills.clone()
        };
        let commands = AssistantMode {
            skill_commands: true,
            ..skills.clone()
        };
        for (mode, tools, allowed) in [
            (
                &skills,
                "Skill",
                "mcp__moneypenny,Skill(ai6:report-writer),Skill(ai6:charts)",
            ),
            (
                &with_files,
                "Read,Write,Edit,Glob,Grep,Skill",
                "mcp__moneypenny,Read,Glob,Grep,Edit(/personal/**),Skill(ai6:report-writer),\
                 Skill(ai6:charts)",
            ),
            (
                &commands,
                "Skill",
                "mcp__moneypenny,Skill(ai6:report-writer),Skill(ai6:charts)",
            ),
        ] {
            let mut argv = Vec::new();
            add_assistant(&mut argv, &environment, "/wd", mode).unwrap();
            let after = |flag: &str| {
                let position = argv.iter().position(|a| a == flag);
                position.map(|position| argv[position + 1].clone())
            };
            assert!(argv.iter().any(|a| a == "--restricted"), "{argv:?}");
            assert_eq!(after("--tools").as_deref(), Some(tools));
            assert_eq!(after("--permission-mode").as_deref(), Some("dontAsk"));
            let denied = after("--disallowedTools").unwrap();
            assert!(!denied.split(',').any(|tool| tool == "Skill"), "{denied}");
            for tool in ["Bash", "WebFetch", "Task", "Agent"] {
                assert!(denied.split(',').any(|name| name == tool), "{denied}");
            }
            assert_eq!(after("--allowedTools").as_deref(), Some(allowed));
            assert_eq!(after("--setting-sources").as_deref(), Some("user"));
            assert_eq!(after("--plugin-dir").as_deref(), Some(dir.as_str()));
            let settings = after("--settings");
            if mode.skill_commands {
                assert_eq!(settings, None, "{argv:?}");
            } else {
                assert_eq!(
                    serde_json::from_str::<Value>(&settings.unwrap()).unwrap(),
                    json!({"disableSkillShellExecution": true})
                );
            }
        }
        let mut nowhere = skills.clone();
        nowhere.skills_dir = None;
        let mut argv = Vec::new();
        assert!(add_assistant(&mut argv, &environment, "/wd", &nowhere).is_err());
        assert!(argv.is_empty());

        // The tripwire lets `Skill` through only for a mode with skills.
        for (mode, ends) in [(&skills, false), (&AssistantMode::moneypenny(), true)] {
            let mut input =
                b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}\n".to_vec();
            input.extend_from_slice(
                b"{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\
                  \"id\":\"t\",\"name\":\"Skill\",\"input\":{\"skill\":\"ai6:charts\"}}]}}\n",
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
            .restricted_to(restriction(false, Some(mode)));
            let kinds = std::iter::from_fn(|| session.next_event(50).unwrap())
                .map(|event| event.kind)
                .collect::<Vec<_>>();
            assert_eq!(kinds.contains(&"failed".to_owned()), ends, "{kinds:?}");
        }
    }

    #[test]
    fn skills_beside_scripts_share_one_settings_value() {
        let environment = assistant_environment();
        let mode = AssistantMode {
            interpreters: vec!["python3".to_owned()],
            files: vec!["personal".to_owned()],
            skills: vec!["charts".to_owned()],
            skills_dir: Some(
                std::env::temp_dir()
                    .join("conversation")
                    .join(".claude")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ..AssistantMode::moneypenny()
        };
        let mut argv = Vec::new();
        add_assistant(&mut argv, &environment, "/wd", &mode).unwrap();
        let flags: Vec<usize> = argv
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--settings")
            .map(|(at, _)| at)
            .collect();
        assert_eq!(flags.len(), 1, "one --settings: {argv:?}");
        let settings: Value = serde_json::from_str(&argv[flags[0] + 1]).unwrap();
        assert_eq!(settings["sandbox"]["enabled"], true);
        assert_eq!(settings["disableSkillShellExecution"], true);
        let tools = &argv[argv.iter().position(|a| a == "--tools").unwrap() + 1];
        assert_eq!(tools, "Read,Write,Edit,Glob,Grep,Bash,Skill");
    }

    #[test]
    fn an_assistant_mode_the_handler_cannot_confine_is_refused() {
        let environment = assistant_environment();
        let mut skills = AssistantMode::moneypenny();
        skills.skills.push("pdf".to_owned());
        let mut wildcard = AssistantMode::moneypenny();
        wildcard.network.allowed_hosts.push("*".to_owned());
        let mut path = AssistantMode::moneypenny();
        path.interpreters.push("/bin/sh".to_owned());
        for mode in [skills, wildcard, path] {
            let mut argv = Vec::new();
            assert!(add_assistant(&mut argv, &environment, "/wd", &mode).is_err());
            assert!(argv.is_empty());
        }
        let mut argv = Vec::new();
        assert!(
            add_assistant(
                &mut argv,
                &BTreeMap::new(),
                "/wd",
                &AssistantMode::moneypenny()
            )
            .is_err(),
            "no endpoint in the environment"
        );
    }

    /// The working directory, an unreadable directory and the mode of a fixture: `policy` with
    /// `hosts`, `python3` when `scripts`, and her workspace's directories.
    #[allow(clippy::type_complexity)]
    fn script_mode(policy: &str, hosts: &[&str], scripts: bool) -> (String, String, AssistantMode) {
        let root = std::env::temp_dir().join("ai6-claude-fixture");
        let working_directory = root.join("conversation").to_string_lossy().into_owned();
        let home = root.join("home").to_string_lossy().into_owned();
        let mode = AssistantMode {
            interpreters: if scripts {
                vec!["python3".to_owned()]
            } else {
                Vec::new()
            },
            network: ai6::AssistantNetwork {
                policy: policy.to_owned(),
                allowed_hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
                ..Default::default()
            },
            files: vec!["personal".to_owned(), "scratch".to_owned()],
            read_only: vec![
                "shared".to_owned(),
                "CLAUDE.md".to_owned(),
                "AGENTS.md".to_owned(),
            ],
            ..AssistantMode::moneypenny()
        };
        (working_directory, home, mode)
    }

    /// The argv and environment Claude Code is started with for `mode`.
    fn started_with(
        working_directory: &str,
        mode: AssistantMode,
    ) -> (Vec<String>, BTreeMap<String, String>) {
        let mut environment = assistant_environment();
        environment.insert("ANTHROPIC_API_KEY".to_owned(), "model-key".to_owned());
        environment.insert("LANG".to_owned(), "C".to_owned());
        // The desk gives each conversation its own configuration directory; the handler denies
        // reading it and hides the variable from scripts.
        environment.insert(
            "CLAUDE_CONFIG_DIR".to_owned(),
            format!("{working_directory}.config"),
        );
        let terminal = ProcessTerminal::new();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude-assistant-fixture".to_owned(),
            ..ClaudeConfig::default()
        })
        .unwrap();
        let started = handler
            .start(
                AgentStart {
                    mission_id: "assistant-conversation".parse().unwrap(),
                    agent_id: "moneypenny".to_owned(),
                    prompt: "Work it out.".to_owned(),
                    initial_context: Vec::new(),
                    working_directory: working_directory.to_owned(),
                    clearance: "desk".to_owned(),
                    environment,
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: Some(mode),
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        read_to_completion(&started);
        started.close().unwrap();
        let spec = terminal.specs.lock().unwrap().remove(0);
        (spec.argv, spec.env.unwrap())
    }

    fn flag(argv: &[String], flag: &str) -> Option<String> {
        argv.iter()
            .position(|a| a == flag)
            .map(|position| argv[position + 1].clone())
    }

    /// The `--settings` Claude Code is started with, parsed; `None` when none is passed.
    fn settings_of(argv: &[String]) -> Option<Value> {
        flag(argv, "--settings").map(|settings| serde_json::from_str(&settings).unwrap())
    }

    /// The string members of a JSON array.
    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// Each network policy with and without scripts, as code.claude.com/docs/en/sandboxing,
    /// /permissions and /settings-reference describe the flags and keys. Reads are denied
    /// everywhere but the working directory and the platform's paths an interpreter needs, and
    /// with no deny list nothing is refused, under `open` too.
    #[test]
    fn scripts_and_the_network_policy_give_exactly_their_tools_rules_and_sandbox() {
        let files = "mcp__moneypenny,Read,Glob,Grep,Edit(/personal/**),Edit(/scratch/**)";
        let dots = "Edit(**/.*),Edit(**/.*/**)";
        // (policy, hosts, scripts, --tools, --disallowedTools, --allowedTools, WebFetch off)
        let cases = [
            (
                "limited",
                &["docs.python.org", "*.pypi.org"][..],
                true,
                "Read,Write,Edit,Glob,Grep,Bash,WebFetch",
                format!("PowerShell,NotebookEdit,WebSearch,Task,Agent,TodoWrite,Skill,{dots}"),
                format!(
                    "{files},Bash(python3 *),WebFetch(domain:docs.python.org),\
                     WebFetch(domain:*.pypi.org)"
                ),
                false,
            ),
            (
                "limited",
                &["docs.python.org", "*.pypi.org"][..],
                false,
                "Read,Write,Edit,Glob,Grep,WebFetch",
                format!("Bash,PowerShell,NotebookEdit,WebSearch,Task,Agent,TodoWrite,Skill,{dots}"),
                format!("{files},WebFetch(domain:docs.python.org),WebFetch(domain:*.pypi.org)"),
                false,
            ),
            (
                "denied",
                &[][..],
                true,
                "Read,Write,Edit,Glob,Grep,Bash",
                format!(
                    "PowerShell,NotebookEdit,WebFetch,WebSearch,Task,Agent,TodoWrite,Skill,{dots}"
                ),
                format!("{files},Bash(python3 *)"),
                true,
            ),
            (
                "denied",
                &[][..],
                false,
                "Read,Write,Edit,Glob,Grep",
                format!(
                    "Bash,PowerShell,NotebookEdit,WebFetch,WebSearch,Task,Agent,TodoWrite,Skill,\
                     {dots}"
                ),
                files.to_owned(),
                true,
            ),
            (
                "open",
                &[][..],
                true,
                "Read,Write,Edit,Glob,Grep,Bash,WebFetch,WebSearch",
                format!("PowerShell,NotebookEdit,Task,Agent,TodoWrite,Skill,{dots}"),
                format!("{files},Bash(python3 *),WebFetch(domain:*),WebSearch"),
                false,
            ),
            (
                "open",
                &[][..],
                false,
                "Read,Write,Edit,Glob,Grep,WebFetch,WebSearch",
                format!("Bash,PowerShell,NotebookEdit,Task,Agent,TodoWrite,Skill,{dots}"),
                format!("{files},WebFetch(domain:*),WebSearch"),
                false,
            ),
        ];
        for (policy, hosts, scripts, tools, denied, allowed, fetch_off) in cases {
            let case = format!("{policy} {hosts:?} scripts={scripts}");
            let (working_directory, _home, mode) = script_mode(policy, hosts, scripts);
            let (argv, environment) = started_with(&working_directory, mode);
            assert_eq!(flag(&argv, "--tools").as_deref(), Some(tools), "{case}");
            assert_eq!(flag(&argv, "--permission-mode").as_deref(), Some("dontAsk"));
            assert_eq!(flag(&argv, "--disallowedTools"), Some(denied), "{case}");
            assert_eq!(flag(&argv, "--allowedTools"), Some(allowed), "{case}");
            assert_eq!(
                environment.get(DISABLE_WEB_FETCH_ENV).map(String::as_str),
                fetch_off.then_some("1"),
                "{case}"
            );
            // `--settings` is present when there is a sandbox (scripts) or a WebFetch gate
            // (any policy but `denied`); `denied` with no scripts has neither.
            let has_settings = scripts || policy != "denied";
            let settings = settings_of(&argv);
            assert_eq!(settings.is_some(), has_settings, "{case}");
            let Some(settings) = settings else {
                continue;
            };
            let under = |path: &str| {
                std::path::Path::new(&working_directory)
                    .join(path)
                    .to_string_lossy()
                    .into_owned()
            };
            if scripts {
                let fs = &settings["sandbox"]["filesystem"];
                assert_eq!(settings["sandbox"]["enabled"], json!(true), "{case}");
                assert_eq!(
                    settings["sandbox"]["failIfUnavailable"],
                    json!(true),
                    "{case}"
                );
                let deny_read = strings(&fs["denyRead"]);
                assert_eq!(deny_read[0], "/", "reads denied everywhere: {case}");
                let station =
                    programs::station_paths(std::path::Path::new(&working_directory), &[], &[]);
                assert!(!station.is_empty(), "{case}");
                for path in station {
                    let path = path.to_string_lossy().into_owned();
                    assert!(deny_read.contains(&path), "{case}: {path} denied again");
                }
                let allow_read = strings(&fs["allowRead"]);
                assert!(
                    allow_read.contains(&working_directory),
                    "{case}: {allow_read:?}"
                );
                for root in script_read_roots(std::env::consts::OS) {
                    assert!(allow_read.contains(&(*root).to_owned()), "{case}: {root}");
                }
                for wide in [
                    "/etc",
                    "/opt",
                    "/dev",
                    "/var",
                    "/Library",
                    "/proc",
                    "/private/var/db",
                ] {
                    assert!(
                        !allow_read.iter().any(|dir| dir == wide),
                        "{case}: {wide} is not re-opened whole"
                    );
                }
                assert_eq!(
                    strings(&fs["allowWrite"]),
                    [under("personal"), under("scratch")],
                    "{case}"
                );
                let deny_write = strings(&fs["denyWrite"]);
                for path in [
                    under("shared"),
                    under("CLAUDE.md"),
                    under("AGENTS.md"),
                    under(".claude"),
                    under(".git"),
                    under("personal/.git"),
                    under("scratch/.git"),
                ] {
                    assert!(deny_write.contains(&path), "{case}: {path}");
                }
                let listed: &[&str] = match policy {
                    "limited" => hosts,
                    "open" => &["*"],
                    _ => &[],
                };
                assert_eq!(
                    strings(&settings["sandbox"]["network"]["allowedDomains"]),
                    listed,
                    "{case}"
                );
                assert!(
                    strings(&settings["sandbox"]["network"]["deniedDomains"]).is_empty(),
                    "{case}: no deny list, nothing denied"
                );
                assert_eq!(
                    settings["sandbox"]["network"]["strictAllowlist"],
                    json!(true),
                    "{case}"
                );
                let hidden: Vec<String> = settings["sandbox"]["credentials"]["envVars"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| entry["name"].as_str().unwrap().to_owned())
                    .collect();
                for name in [
                    "ANTHROPIC_API_KEY",
                    "AI6_MONEYPENNY_MCP_TOKEN",
                    "CLAUDE_CONFIG_DIR",
                ] {
                    assert!(hidden.contains(&name.to_owned()), "{case}: {name} hidden");
                }
                let files: Vec<String> = settings["sandbox"]["credentials"]["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| entry["path"].as_str().unwrap().to_owned())
                    .collect();
                assert!(
                    files.iter().any(|path| path.ends_with(".config")),
                    "{case}: the config dir is a denied file: {files:?}"
                );
                assert_eq!(
                    settings["permissions"]["blockReadsOutsideWorkingDirectories"],
                    json!(true),
                    "{case}"
                );
            } else {
                assert!(
                    settings.get("sandbox").is_none(),
                    "no sandbox without scripts: {case}"
                );
            }
            // Every policy but `denied` gates WebFetch; with no deny list it passes none.
            let hooks = settings.pointer("/hooks/PreToolUse");
            if policy == "denied" {
                assert!(hooks.is_none(), "no WebFetch under denied: {case}");
            } else {
                let args = strings(&hooks.unwrap()[0]["hooks"][0]["args"]);
                assert_eq!(&args[..2], ["assistant", "web-gate"], "{case}");
                assert!(
                    !args.iter().any(|arg| arg == "--deny"),
                    "{case}: no deny list"
                );
                assert_eq!(
                    args.iter().any(|arg| arg == "--hosts"),
                    policy == "limited",
                    "{case}: an allow list only under limited"
                );
            }
            assert!(
                !argv
                    .iter()
                    .any(|a| a.contains("conversation-token") || a.contains("model-key")),
                "secrets stay out of the command line: {case}"
            );
        }
    }

    /// Without a workspace, scripts and the web still get their tools, and an assistant whose
    /// network reaches no host and runs no script keeps `--tools=` with no permission mode.
    #[test]
    fn scripts_without_a_workspace_and_a_mode_with_nothing_built_in() {
        let mode = AssistantMode {
            interpreters: vec!["python3".to_owned(), "node".to_owned()],
            network: ai6::AssistantNetwork {
                policy: "limited".to_owned(),
                allowed_hosts: vec!["docs.python.org".to_owned()],
                ..Default::default()
            },
            ..AssistantMode::moneypenny()
        };
        let (argv, environment) = started_with("/conversation", mode);
        assert_eq!(flag(&argv, "--tools").as_deref(), Some("Bash,WebFetch"));
        assert_eq!(
            flag(&argv, "--allowedTools").as_deref(),
            Some("mcp__moneypenny,Bash(python3 *),Bash(node *),WebFetch(domain:docs.python.org)")
        );
        assert!(
            !flag(&argv, "--disallowedTools")
                .unwrap()
                .contains("Edit(**/.*)")
        );
        let settings: Value = serde_json::from_str(&flag(&argv, "--settings").unwrap()).unwrap();
        // No workspace: nothing writable, but reads are still denied everywhere but the system
        // directories, and the WebFetch gate still applies.
        assert_eq!(settings["sandbox"]["filesystem"]["allowWrite"], json!([]));
        assert_eq!(settings["sandbox"]["filesystem"]["denyRead"][0], json!("/"));
        let allow_read = strings(&settings["sandbox"]["filesystem"]["allowRead"]);
        for root in script_read_roots(std::env::consts::OS) {
            assert!(allow_read.contains(&(*root).to_owned()), "{root}");
        }
        assert!(
            settings.pointer("/hooks/PreToolUse").is_some(),
            "the web gate"
        );
        assert_eq!(environment.get(DISABLE_WEB_FETCH_ENV), None);

        let (argv, environment) = started_with("/conversation", AssistantMode::moneypenny());
        assert!(argv.iter().any(|a| a == "--tools="), "{argv:?}");
        assert_eq!(flag(&argv, "--permission-mode"), None);
        assert_eq!(flag(&argv, "--settings"), None);
        assert_eq!(
            environment.get(DISABLE_WEB_FETCH_ENV).map(String::as_str),
            Some("1"),
            "limited with no host reaches nothing"
        );
    }

    #[test]
    fn managed_settings_are_found_in_each_platforms_system_directory() {
        let only = |wanted: &'static str| move |path: &std::path::Path| path.ends_with(wanted);
        assert_eq!(
            managed_settings_at("linux", only("managed-settings.json")),
            Some(std::path::PathBuf::from(
                "/etc/claude-code/managed-settings.json"
            ))
        );
        assert_eq!(
            managed_settings_at("macos", only("managed-settings.d")),
            Some(
                std::path::Path::new("/Library/Application Support/ClaudeCode")
                    .join("managed-settings.d")
            )
        );
        assert!(managed_settings_at("windows", only("managed-settings.json")).is_some());
        assert_eq!(
            managed_settings_at("linux", |_: &std::path::Path| false),
            None
        );
        assert_eq!(
            managed_settings_at("freebsd", |_: &std::path::Path| true),
            None
        );
    }

    /// Reads are denied everywhere but the sandbox's own relay sockets in `/tmp`, and, with a
    /// temporary directory of the Session's own, the relay sockets there, by name: the whole
    /// directory is not re-opened (that covers the writable `$TMPDIR` read-only and `bwrap`
    /// fails to start).
    #[test]
    fn the_network_relay_sockets_are_the_only_temp_paths_reopened() {
        let (working_directory, _, mode) = script_mode("limited", &["docs.python.org"], true);
        let mut environment = assistant_environment();
        let base = std::env::temp_dir()
            .join("conv.t")
            .to_string_lossy()
            .into_owned();
        environment.insert("CLAUDE_CODE_TMPDIR".to_owned(), base.clone());
        let settings =
            Value::Object(script_settings(&mode, &working_directory, &environment).unwrap());
        let allow_read = strings(&settings["sandbox"]["filesystem"]["allowRead"]);
        let under = |name: &str| {
            std::path::Path::new(&base)
                .join(name)
                .to_string_lossy()
                .into_owned()
        };
        assert!(
            allow_read.contains(&under("claude-http-*.sock")),
            "{allow_read:?}"
        );
        assert!(
            allow_read.contains(&under("srt-mux-*.sock")),
            "{allow_read:?}"
        );
        assert!(
            !allow_read.contains(&base),
            "not the whole directory: {allow_read:?}"
        );
        if cfg!(target_os = "linux") {
            assert!(allow_read.contains(&"/tmp/claude-http-*.sock".to_owned()));
            assert!(!allow_read.contains(&"/tmp".to_owned()), "not all of /tmp");
        }
    }

    /// A deny list reaches the sandbox as the names and single addresses it holds and the
    /// WebFetch gate whole; with scripts that reach a host, a network or a range category is
    /// refused, since the sandbox's proxy matches names and single addresses only (measured with
    /// Claude Code 2.1.310: `127.0.0.0/8` in `deniedDomains` left `127.0.0.2` reachable).
    #[test]
    fn a_deny_list_reaches_the_sandbox_and_the_gate_and_a_range_with_scripts_is_refused() {
        let (working_directory, _, mut mode) = script_mode("open", &[], true);
        mode.network.denied = vec![
            "metadata".to_owned(),
            "intranet.example".to_owned(),
            "192.0.2.7".to_owned(),
        ];
        let (argv, _) = started_with(&working_directory, mode.clone());
        let settings = settings_of(&argv).unwrap();
        assert_eq!(
            strings(&settings["sandbox"]["network"]["deniedDomains"]),
            [
                "169.254.169.254",
                "192.0.2.7",
                "[fd00:ec2::254]",
                "intranet.example",
                "metadata.google.internal",
            ]
        );
        let args = strings(&settings.pointer("/hooks/PreToolUse").unwrap()[0]["hooks"][0]["args"]);
        let at = args.iter().position(|arg| arg == "--deny").unwrap();
        assert_eq!(args[at + 1], "metadata,intranet.example,192.0.2.7");

        for range in ["loopback", "private", "10.0.0.0/8"] {
            let mut ranged = mode.clone();
            ranged.network.denied = vec![range.to_owned()];
            let mut argv = Vec::new();
            let error = add_assistant(
                &mut argv,
                &assistant_environment(),
                &working_directory,
                &ranged,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("by name and by single address"),
                "{range}: {error}"
            );
            // Without scripts only the gate holds the list, and it resolves names itself.
            ranged.interpreters.clear();
            let mut argv = Vec::new();
            add_assistant(
                &mut argv,
                &assistant_environment(),
                &working_directory,
                &ranged,
            )
            .unwrap();
            // A network that reaches nothing cannot reach a range either.
            let mut quiet = mode.clone();
            quiet.network.policy = "denied".to_owned();
            quiet.network.denied = vec![range.to_owned()];
            let mut argv = Vec::new();
            add_assistant(
                &mut argv,
                &assistant_environment(),
                &working_directory,
                &quiet,
            )
            .unwrap();
        }
    }

    /// A variable `credentials.envVars` cannot name would reach scripts, so the start is
    /// refused rather than the variable silently left visible.
    #[test]
    fn an_environment_variable_the_sandbox_cannot_hide_is_refused() {
        let (working_directory, _, mode) = script_mode("limited", &[], true);
        let mut environment = assistant_environment();
        environment.insert("SECRET-TOKEN".to_owned(), "x".to_owned());
        let mut argv = Vec::new();
        let error = add_assistant(&mut argv, &environment, &working_directory, &mode).unwrap_err();
        assert!(error.to_string().contains("SECRET-TOKEN"), "{error}");
        let mut unsandboxed = mode.clone();
        unsandboxed.unsandboxed = true;
        let mut argv = Vec::new();
        add_assistant(&mut argv, &environment, &working_directory, &unsandboxed).unwrap();
    }

    /// The configuration directory the desk sets is denied to scripts twice over: it is not in
    /// `allowRead` (reads are denied everywhere else), and it is a `credentials.files` deny
    /// entry, so a stored login and other conversations' transcripts stay unreadable.
    #[test]
    fn the_config_directory_is_denied_to_scripts() {
        let (working_directory, _, mode) = script_mode("denied", &[], true);
        let (argv, _) = started_with(&working_directory, mode);
        let settings: Value = serde_json::from_str(&flag(&argv, "--settings").unwrap()).unwrap();
        let allow_read = strings(&settings["sandbox"]["filesystem"]["allowRead"]);
        let config = format!("{working_directory}.config");
        assert!(
            !allow_read.contains(&config),
            "the config dir is not readable: {allow_read:?}"
        );
        let files: Vec<String> = settings["sandbox"]["credentials"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["path"].as_str().unwrap().to_owned())
            .collect();
        assert!(
            files.contains(&config),
            "the config dir is a denied file: {files:?}"
        );
    }

    /// With `unsandboxed`, scripts run with the sandbox off; Bash is still allowed only for
    /// the interpreters.
    #[test]
    fn unsandboxed_scripts_turn_the_sandbox_off_and_keep_the_interpreter_rules() {
        let (working_directory, _, mut mode) = script_mode("limited", &[], true);
        mode.unsandboxed = true;
        let (argv, _) = started_with(&working_directory, mode);
        let settings: Value = serde_json::from_str(&flag(&argv, "--settings").unwrap()).unwrap();
        assert_eq!(
            settings,
            json!({
                "sandbox": { "enabled": false },
                "permissions": { "blockReadsOutsideWorkingDirectories": true }
            })
        );
        assert!(
            flag(&argv, "--allowedTools")
                .unwrap()
                .contains("Bash(python3 *)")
        );
    }

    #[test]
    fn the_sandbox_is_found_on_macos_and_on_linux_with_bwrap_and_socat() {
        let bin = std::env::temp_dir().join(format!("ai6-claude-sandbox-{}", std::process::id()));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("bwrap"), "").unwrap();
        let path = Some(std::env::join_paths([&bin]).unwrap());
        let missing = script_sandbox_missing("linux", path.clone()).unwrap();
        assert!(
            missing.contains("socat") && !missing.contains("bwrap and"),
            "{missing}"
        );
        std::fs::write(bin.join("socat"), "").unwrap();
        assert_eq!(script_sandbox_missing("linux", path.clone()), None);
        assert!(script_sandbox_missing("linux", None).is_some());
        assert_eq!(script_sandbox_missing("macos", None), None);
        let windows = script_sandbox_missing("windows", path).unwrap();
        assert!(windows.contains("native Windows"), "{windows}");
        assert!(script_sandbox_missing("freebsd", None).is_some());
        std::fs::remove_dir_all(&bin).unwrap();
        let handler = ClaudeHandler::try_from(ClaudeConfig {
            executable: "claude".to_owned(),
            ..ClaudeConfig::default()
        })
        .unwrap();
        let here = handler.assistant_scripts(BTreeMap::new()).unwrap();
        if cfg!(windows) {
            assert!(here.is_some());
        }
    }

    #[test]
    fn an_assistant_may_call_the_built_in_tools_its_mode_offers_and_no_other() {
        let (_, _, scripts) = script_mode("limited", &["docs.python.org"], true);
        let (_, _, denied) = script_mode("denied", &[], false);
        let (_, _, open) = script_mode("open", &[], false);
        for (mode, tool, ends) in [
            (scripts.clone(), "Bash", false),
            (scripts.clone(), "WebFetch", false),
            (scripts, "WebSearch", true),
            (denied.clone(), "Bash", true),
            (denied, "WebFetch", true),
            (open.clone(), "WebSearch", false),
            (open, "PowerShell", true),
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
                "{tool} under {:?} with {:?}: {kinds:?}",
                mode.network.policy,
                mode.interpreters
            );
        }
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
        for (block, named) in [
            (
                r#"{"type":"server_tool_use","id":"srvtoolu_1","name":"advisor","input":{}}"#,
                "server_tool_use:advisor",
            ),
            (
                r#"{"type":"mcp_tool_use","id":"mcptoolu_1","name":"board","server_name":"moneypenny"}"#,
                "mcp_tool_use:board",
            ),
            (
                r#"{"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[]}"#,
                "web_search_tool_result",
            ),
            (
                r#"{"type":"code_execution_tool_result","tool_use_id":"srvtoolu_2"}"#,
                "code_execution_tool_result",
            ),
            (
                r#"{"type":"container_upload","file_id":"f"}"#,
                "container_upload",
            ),
            (r#"{"text":"no type"}"#, "untyped"),
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
            assert!(kinds[1].1.contains(named), "{}", kinds[1].1);
        }
    }

    /// Prose, reasoning and an allowed call pass the tripwire.
    #[test]
    fn expected_blocks_keep_a_restricted_session_going() {
        let message = json!({"type": "assistant", "message": {"content": [
            {"type": "thinking", "thinking": "hm"},
            {"type": "redacted_thinking", "data": "x"},
            {"type": "text", "text": "ok"},
            {"type": "tool_use", "id": "t", "name": "mcp__moneypenny__board", "input": {}},
        ]}});
        assert!(unexpected_blocks(&message).is_empty());
        let session = ClaudeSession::new(
            ResumeTerminalSession {
                input: Mutex::new(Vec::new()),
                writes: Arc::new(Mutex::new(Vec::new())),
            }
            .into_object(),
            None,
        )
        .restricted_to(restriction(false, Some(&AssistantMode::moneypenny())));
        let event = decode_event(&message, Vec::new()).unwrap();
        assert!(session.tripped(&event, &message).is_none());
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

    fn overlay_skill(name: &str, native: bool) -> ai6::OverlaySkill {
        ai6::OverlaySkill {
            name: name.to_owned(),
            overlay: "tools".to_owned(),
            description: "Does a thing.".to_owned(),
            path: format!("/store/tools/skills/{name}"),
            native,
        }
    }

    fn mission_argv(overlays: &SessionOverlays, commands: bool) -> Vec<String> {
        let mut argv = Vec::new();
        add_settings(
            &mut argv,
            &cable_environment(),
            "wd",
            Mode {
                project_settings: false,
                project_mcp: false,
                answer_only: false,
                commands,
                reads_only: false,
            },
            &ClaudeSettings::default(),
            None,
            overlays,
        )
        .unwrap();
        argv
    }

    /// Measured with Claude Code 2.1.318 against a loopback stand-in for the API: under these flags
    /// the plugin's skills were listed in the request and launched, at `field` and at `desk`.
    #[test]
    fn a_missions_overlay_skills_load_from_the_plugin_directory_under_its_rules() {
        let overlays = SessionOverlays {
            roots: vec!["/store/tools".to_owned()],
            skills: vec![overlay_skill("review", true), overlay_skill("audit", false)],
            plugin_dir: Some("/store/plugin".to_owned()),
        };
        for commands in [true, false] {
            let argv = mission_argv(&overlays, commands);
            let (offered, _, allowed) = field_flags(&argv);
            assert!(offered.split(',').any(|tool| tool == "Skill"), "{offered}");
            assert!(allowed.contains("Skill(ai6:review)"), "{allowed}");
            assert!(
                !allowed.contains("audit"),
                "a routed skill is no rule: {allowed}"
            );
            assert_eq!(
                flag(&argv, "--plugin-dir").as_deref(),
                Some("/store/plugin")
            );
            let settings = settings_of(&argv).unwrap();
            assert_eq!(settings["disableSkillShellExecution"], json!(true));
            assert!(settings["hooks"]["PreToolUse"].is_array(), "the gate stays");
        }
        // What it does not load itself is named in its context.
        let context = overlays.with_routing(&["standing".to_owned()], true);
        assert_eq!(context.len(), 2);
        assert!(
            context[1].contains("/store/tools/skills/audit/SKILL.md"),
            "{context:?}"
        );
        assert!(!context[1].contains("review"), "{context:?}");
    }

    #[test]
    fn a_mission_with_no_overlay_skill_is_offered_no_skill_tool() {
        for overlays in [
            SessionOverlays::default(),
            SessionOverlays {
                roots: vec!["/store/tools".to_owned()],
                skills: vec![overlay_skill("audit", false)],
                plugin_dir: None,
            },
        ] {
            let argv = mission_argv(&overlays, true);
            let (offered, _, allowed) = field_flags(&argv);
            assert!(!offered.contains("Skill"), "{offered}");
            assert!(!allowed.contains("Skill"), "{allowed}");
            assert_eq!(flag(&argv, "--plugin-dir"), None);
            assert_eq!(
                settings_of(&argv).unwrap()["disableSkillShellExecution"],
                Value::Null
            );
        }
    }
}
