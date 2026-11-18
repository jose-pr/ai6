//! Codex app-server JSON-RPC implementation of AI6's AgentHandler contract.

#![doc = include_str!("../README.md")]
#![deny(unsafe_code)]

ai6::guatiao::providers!(CodexHandler);

mod sandbox;
mod terminal;
mod tools;

pub use tools::{KEPT_FEATURES, TOOL_CHECK_ENV};

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use ai6::directorates::m::identity::RunId;
use ai6::directorates::q::terminal::{AgentTerminal, exit_code_text};
use ai6::directorates::section_00::agent::{
    AgentHandler, AgentResume, AgentStart, AssistantMode, ModelInfo, ModelListing, RunRecordFile,
    RunRecordQuery, clearance_reads_only, clearance_runs_commands,
};
use ai6::directorates::section_00::gate::{GateClient, GateVerdict, gate};
use ai6::directorates::section_00::listing;
use ai6::directorates::section_00::records;
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use ai6::{
    AgentEvent, AgentMessage, AgentSession, Bucket, BucketStatus, LIMIT_EVENT, LimitHit, Refill,
    Spend, UsageReport, UsageSource, UsageSupport, infer_limit,
};
use coqui_acp_codex::harness::PlainSpawn;
use coqui_acp_codex::harness::event::{Content, Event as CodexEvent, Outcome, PermissionRequest};
use coqui_acp_codex::{
    ClientInfo, CodexLaunch, CodexSession as AppServer, CodexSettings, Error as CodexError,
    ThreadStart,
};
use serde_json::{Value, json};

use terminal::{Link, TerminalSpawn};

/// The models in the app server's answer to `model/list`; unknown when it has none. A model the
/// catalog hides (`hidden`) is left out: Codex does not offer it.
fn models_of(answer: &Value) -> ModelListing {
    let Some(models) = answer["result"]["data"].as_array() else {
        return ModelListing::unknown("this Codex does not list its models");
    };
    ModelListing::of(
        models
            .iter()
            .filter(|model| model["hidden"] != true)
            .filter_map(|model| {
                Some(ModelInfo {
                    id: model["model"].as_str().or(model["id"].as_str())?.to_owned(),
                    name: model["displayName"].as_str().map(str::to_owned),
                    description: model["description"].as_str().map(str::to_owned),
                })
            })
            .collect(),
    )
}

/// Codex app-server AgentHandler configuration.
#[derive(Debug, Clone, ToValue, FromValue, Schema)]
#[map(crate = ::ai6::guatiao)]
pub struct CodexConfig {
    /// Executable visible to the AI6-owned terminal environment.
    #[map(default = String::from("codex"))]
    pub executable: String,
    /// Bytes of one line of the app server's output a Run holds before handing over what it
    /// has. Default 67108864 (64 MiB).
    #[map(default = 64 * 1024 * 1024)]
    pub json_line_bytes: u64,
    /// Bytes of one terminal control sequence before it is dropped, from the app server's
    /// output and from the commands of the tool check. Default 64.
    #[map(default = 64)]
    pub csi_bytes: u64,
    /// Bytes an assistant's context file may hold. Default 1048576 (1 MiB).
    #[map(default = 1024 * 1024)]
    pub context_file_bytes: u64,
    /// How many of the last non-JSON lines a failed Run quotes. Default 8.
    #[map(default = 8)]
    pub tail_lines: u64,
    /// Bytes of each of those lines a failed Run quotes. Default 512.
    #[map(default = 512)]
    pub tail_line_bytes: u64,
    /// Milliseconds a closing Run waits for the app server to have exited. Default 2000.
    #[map(default = 2_000)]
    pub close_wait_ms: u64,
    /// Milliseconds the end of the output waits for the app server's exit status. Default 1000.
    #[map(default = 1_000)]
    pub exit_wait_ms: u64,
    /// Milliseconds the app server may take to answer `initialize`. Default 20000.
    #[map(default = 20_000)]
    pub start_timeout_ms: u64,
    /// Milliseconds the app server may take to answer a thread request (`thread/start`,
    /// `thread/resume`, `config/read`). Default 30000.
    #[map(default = 30_000)]
    pub request_timeout_ms: u64,
    /// Milliseconds the sandbox probe of an assistant's scripts may run. Default 60000.
    #[map(default = 60_000)]
    pub sandbox_probe_timeout_ms: u64,
    /// Seconds the answer of that probe stands per executable and configuration environment.
    /// Default 900.
    #[map(default = 900)]
    pub sandbox_probe_lifetime_secs: u64,
    /// Characters of the probe's last output line quoted when the sandbox did not run. Default
    /// 300.
    #[map(default = 300)]
    pub sandbox_error_chars: u64,
    /// On Unix, how many times the process table is read again when the sandbox probe's
    /// process tree is killed. Default 32; Windows ignores it.
    #[map(default = 32)]
    pub tree_scans: u64,
    /// Seconds the tool check's feature list stands for the same Codex, settings and
    /// environment. Default 900.
    #[map(default = 900)]
    pub tool_check_lifetime_secs: u64,
    /// Milliseconds one of the tool check's commands may run. Default 120000.
    #[map(default = 120_000)]
    pub tool_command_timeout_ms: u64,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            executable: String::from("codex"),
            json_line_bytes: 64 * 1024 * 1024,
            csi_bytes: 64,
            context_file_bytes: 1024 * 1024,
            tail_lines: 8,
            tail_line_bytes: 512,
            close_wait_ms: 2_000,
            exit_wait_ms: 1_000,
            start_timeout_ms: 20_000,
            request_timeout_ms: 30_000,
            sandbox_probe_timeout_ms: 60_000,
            sandbox_probe_lifetime_secs: 900,
            sandbox_error_chars: 300,
            tree_scans: 32,
            tool_check_lifetime_secs: 900,
            tool_command_timeout_ms: 120_000,
        }
    }
}

impl CodexConfig {
    fn millis(&self, ms: u64) -> Duration {
        Duration::from_millis(ms)
    }
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

/// The format of Codex's rollout file.
pub const RECORD_FORMAT: &str = "codex-rollout-jsonl";

/// Codex's rollout of the thread `query` names, when it is on this host: `runs/YYYY/MM/DD/
/// rollout-<time>-<thread>.jsonl` under `CODEX_HOME` (else `~/.codex`).
fn codex_record(query: &RunRecordQuery) -> Option<RunRecordFile> {
    if !records::file_safe(&query.run_id) {
        return None;
    }
    let root = records::configured_dir(&query.environment, "CODEX_HOME", ".codex")?;
    let suffix = format!("-{}.jsonl", query.run_id);
    let path = records::find_file(&root.join("runs"), 3, &|file| {
        file.starts_with("rollout-") && file.ends_with(&suffix)
    })?;
    Some(RunRecordFile {
        path: path.to_string_lossy().into_owned(),
        format: RECORD_FORMAT.to_owned(),
    })
}

impl AgentHandler for CodexHandler {
    /// `codex app-server` speaks JSON-RPC, one message per line, over standard input and output.
    fn stdio(&self) -> Result<ai6::HarnessStdio, ProviderError> {
        Ok(ai6::HarnessStdio::Json)
    }

    fn read_only_refusal(&self) -> Result<Option<String>, ProviderError> {
        Ok(None)
    }

    fn program(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.config.executable.clone()))
    }

    /// Codex keeps each thread as a rollout file, one JSON object per line.
    fn run_record(&self, query: RunRecordQuery) -> Result<Option<RunRecordFile>, ProviderError> {
        Ok(codex_record(&query))
    }

    /// The app server's `model/list`: the models Codex offers the account behind `environment`
    /// (its `CODEX_HOME` login or API key), before any model turn. With no login it answers from
    /// the catalog it ships with, so the list is what Codex offers, not a check of the account.
    fn models(
        &self,
        environment: std::collections::BTreeMap<String, String>,
    ) -> Result<ModelListing, ProviderError> {
        let start = self.config.millis(self.config.start_timeout_ms);
        let request = self.config.millis(self.config.request_timeout_ms);
        let asked = listing::ask(
            &self.config.executable,
            &["app-server"],
            &environment,
            start + request,
            |harness| {
                harness.send(&json!({
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "clientInfo": { "name": "ai6", "version": env!("CARGO_PKG_VERSION") },
                        "capabilities": {}
                    }
                }))?;
                harness.next_where(|line| line["id"] == 1)?;
                harness.send(&json!({ "method": "initialized", "params": {} }))?;
                harness.send(&json!({ "id": 2, "method": "model/list", "params": {} }))?;
                harness.next_where(|line| line["id"] == 2)
            },
        );
        Ok(match asked {
            Ok(answer) => models_of(&answer),
            Err(reason) => ModelListing::unknown(reason),
        })
    }

    /// Codex reports each request's tokens and the account's rate-limit windows as a turn goes,
    /// and its app server answers `account/rateLimits/read` with no turn, so a Station may poll it.
    fn usage_support(&self) -> Result<UsageSupport, ProviderError> {
        Ok(UsageSupport::new(vec![UsageSource::Reported], true))
    }

    /// Starts an app server with `environment`, reads the account's rate limits and ends it: no
    /// thread is opened and no model turn is spent. A Codex that is not signed in with an account
    /// that has allowances is an error, not an empty report.
    fn usage(
        &self,
        environment: std::collections::BTreeMap<String, String>,
    ) -> Result<UsageReport, ProviderError> {
        let mut launch = CodexLaunch::new(std::env::temp_dir());
        launch.executable = self.config.executable.clone().into();
        launch.env = environment
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        launch.clear_env = true;
        let mut settings = CodexSettings::new();
        settings.initialize_timeout = self.config.millis(self.config.start_timeout_ms);
        settings.request_timeout = self.config.millis(self.config.request_timeout_ms);
        let buckets = AppServer::read_account_usage(&launch, &PlainSpawn, settings)
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error.to_string()))?;
        let mut report = UsageReport::new();
        report.sources = vec![UsageSource::Reported];
        report.buckets = buckets;
        Ok(report)
    }

    /// Asks Codex to run a command in the sandbox an assistant's scripts would get (`codex
    /// sandbox -P`, no model involved; on Windows MXC), with `environment`; the answer stands
    /// 15 minutes per executable and configuration environment.
    fn assistant_scripts(
        &self,
        environment: std::collections::BTreeMap<String, String>,
    ) -> Result<Option<String>, ProviderError> {
        Ok(sandbox::unavailable(&self.config, &environment))
    }

    fn assistant_skills(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(NO_SKILL_LIST.to_owned()))
    }

    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn ai6::AgentSession>, ProviderError> {
        let gate = station_gate(&request.environment);
        self.start_gated(request, terminal, gate)
    }

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn ai6::AgentSession>, ProviderError> {
        let profile = scripts_profile(
            &self.config.executable,
            &request.working_directory,
            &request.environment,
            request.assistant.as_ref(),
        );
        let desk = held_to_desk(
            &request.clearance,
            request.answer_only,
            request.assistant.as_ref(),
        );
        let gate = station_gate(&request.environment)
            .filter(|_| !(request.answer_only || request.assistant.is_some()));
        let run = open(Opening {
            config: &self.config,
            terminal,
            cwd: &request.working_directory,
            environment: &request.environment,
            mode: Mode {
                config: &self.config,
                project_settings: request.project_settings,
                answer_only: request.answer_only || request.assistant.is_some(),
                desk,
                assistant: request.assistant.as_ref(),
                model: request.model.as_deref(),
                profile: profile.as_ref(),
            },
            thread: ThreadStart::Resume(request.run_id.as_str().to_owned()),
            model: request.model.clone(),
            effort: request.effort,
            instructions: None,
            gate,
            restriction: restriction(
                request.answer_only,
                request.assistant.as_ref(),
                desk.then_some(&request.environment),
                clearance_reads_only(&request.clearance),
            ),
            profile: profile.as_ref(),
        })?;
        if let Some(message) = request.message
            && let Err(error) = run.send_message(AgentMessage { text: message })
        {
            let _ = run.close();
            return Err(error);
        }
        Ok(run.into_object())
    }
}

impl CodexHandler {
    /// Starts a thread whose command approvals `gate` answers.
    fn start_gated(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
        gate: Option<Gate>,
    ) -> Result<Object<dyn ai6::AgentSession>, ProviderError> {
        let scripts = Scripts::of(request.assistant.as_ref());
        // Codex 0.164.0 reads skills from its home and the project only: no setting and no
        // `skills/list` root added one, so an overlay's skill is the file a routing line names.
        let mut instructions = request
            .overlays
            .with_routing(&request.initial_context, false);
        if let Some(mode) = &request.assistant {
            instructions.extend(assistant_instructions(
                mode,
                scripts,
                self.config.context_file_bytes,
            )?);
        }
        let profile = scripts_profile(
            &self.config.executable,
            &request.working_directory,
            &request.environment,
            request.assistant.as_ref(),
        );
        let desk = held_to_desk(
            &request.clearance,
            request.answer_only,
            request.assistant.as_ref(),
        );
        let gate = gate.filter(|_| !(request.answer_only || request.assistant.is_some()));
        let run = open(Opening {
            config: &self.config,
            terminal,
            cwd: &request.working_directory,
            environment: &request.environment,
            mode: Mode {
                config: &self.config,
                project_settings: request.project_settings,
                answer_only: request.answer_only || request.assistant.is_some(),
                desk,
                assistant: request.assistant.as_ref(),
                model: request.model.as_deref(),
                profile: profile.as_ref(),
            },
            thread: ThreadStart::New,
            model: request.model.clone(),
            effort: request.effort,
            instructions: (!instructions.is_empty()).then(|| instructions.join("\n\n")),
            gate,
            restriction: restriction(
                request.answer_only,
                request.assistant.as_ref(),
                desk.then_some(&request.environment),
                clearance_reads_only(&request.clearance),
            ),
            profile: profile.as_ref(),
        })?;
        run.send_message(AgentMessage {
            text: request.prompt,
        })?;
        Ok(run.into_object())
    }
}

/// What a restricted (answer-only, assistant or `desk`) thread may use besides answering.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Restriction {
    /// The MCP servers whose tools it may call.
    servers: Vec<String>,
    /// Whether it may run commands: an assistant with scripts.
    commands: bool,
    /// Whether it may change files through Codex's own patch tool: an assistant with scripts
    /// and directories to write, or a `desk` thread working on the Station's disk.
    file_changes: bool,
    /// Whether Codex asks before each change, and only one inside the working directory is
    /// accepted: a `desk` thread, with or without a green ink gate.
    asks_file_changes: bool,
}

/// Whether a Mission's thread at `clearance` is held to `desk`: answer-only and assistant threads
/// have limits of their own.
fn held_to_desk(clearance: &str, answer_only: bool, assistant: Option<&AssistantMode>) -> bool {
    !answer_only && assistant.is_none() && !clearance_runs_commands(clearance)
}

/// AI6's own MCP servers a Run's environment names, as [`app_server_argv`] configures them.
fn station_servers(environment: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    [
        ("ai6-cables", "AI6_CABLE_MCP"),
        ("ai6-theater", "AI6_THEATER_MCP"),
    ]
    .iter()
    .filter(|(_, prefix)| {
        environment.contains_key(&format!("{prefix}_URL"))
            && environment.contains_key(&format!("{prefix}_TOKEN"))
    })
    .map(|(server, _)| (*server).to_owned())
    .collect()
}

/// What a restricted thread may use, `None` for a thread that is not restricted. `desk` is the
/// environment of a thread held to `desk`; `reads_only` holds it further, to reading until its plan
/// passes: no file change at all, so a patch ends the thread.
fn restriction(
    answer_only: bool,
    assistant: Option<&AssistantMode>,
    desk: Option<&std::collections::BTreeMap<String, String>>,
    reads_only: bool,
) -> Option<Restriction> {
    if let Some(environment) = desk {
        let changes = !is_remote_hands(environment) && !reads_only;
        return Some(Restriction {
            servers: station_servers(environment),
            commands: false,
            file_changes: changes,
            asks_file_changes: changes,
        });
    }
    match assistant {
        Some(mode) => {
            let commands = Scripts::of(Some(mode)) != Scripts::None;
            Some(Restriction {
                servers: mode
                    .mcp_servers
                    .iter()
                    .map(|server| server.name.clone())
                    .collect(),
                commands,
                file_changes: commands && (mode.unsandboxed || !mode.files.is_empty()),
                asks_file_changes: false,
            })
        }
        None => answer_only.then(Restriction::default),
    }
}

/// Where an assistant's scripts run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scripts {
    /// No scripts: no interpreter, or no assistant.
    None,
    /// In Codex's sandbox, under [`sandbox::PROFILE`].
    Sandboxed,
    /// Without a sandbox (`AssistantMode::unsandboxed`).
    Unsandboxed,
}

impl Scripts {
    fn of(assistant: Option<&AssistantMode>) -> Self {
        match assistant {
            Some(mode) if !mode.interpreters.is_empty() && mode.unsandboxed => Self::Unsandboxed,
            Some(mode) if !mode.interpreters.is_empty() => Self::Sandboxed,
            _ => Self::None,
        }
    }
}

/// Why this handler loads no skill for an assistant.
const NO_SKILL_LIST: &str = "Codex has no list of allowed skills: it loads the skills of the \
                             project, of the user's home directory and of the system, and its own \
                             bundled ones";

/// The developer instructions an assistant thread gets besides its initial context: the text
/// of its `context_file`, and what it cannot do here that its mode suggests it could.
fn assistant_instructions(
    mode: &AssistantMode,
    scripts: Scripts,
    context_file_bytes: u64,
) -> Result<Vec<String>, ProviderError> {
    let mut parts = Vec::new();
    if let Some(file) = &mode.context_file {
        let size = std::fs::metadata(file)
            .map_err(|error| {
                refuse(
                    Status::GUATIAO_ERR_BAD_VALUE,
                    format!("context file: {error}"),
                )
            })?
            .len();
        if size > context_file_bytes {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!("the context file holds {size} bytes, more than {context_file_bytes}"),
            ));
        }
        let text = std::fs::read_to_string(file).map_err(|error| {
            refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!("context file: {error}"),
            )
        })?;
        parts.push(text);
    }
    if scripts == Scripts::None && !mode.files.is_empty() {
        parts.push(
            "You have no file tools in this conversation: you cannot create or change files, \
             your memory among them. Say so when the person asks you to save or remember \
             something."
                .to_owned(),
        );
    }
    if scripts == Scripts::Sandboxed && !mode.network.reaches_nothing() {
        eprintln!(
            "ai6: Moneypenny's scripts run with no network under Codex: its app server enforces \
             no host list for them, so they get none"
        );
        parts.push(
            "Your scripts have no network access in this conversation: say so when the person \
             asks for something that needs it."
                .to_owned(),
        );
    }
    Ok(parts)
}

/// Decides whether a command Codex asks to run may go ahead, given its working directory.
type Gate = Arc<dyn Fn(&str, Option<&Path>) -> GateVerdict + Send + Sync>;

/// The green ink gate asking the Run's Station through its Cable endpoint. A Run with
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
/// An assistant with scripts never asks either, and runs them in the sandbox its app server's
/// profile describes (no `sandbox` here: Codex does not combine the two), or with none when
/// unsandboxed. A `desk` thread has no command to ask about; `untrusted` over a read-only sandbox
/// makes Codex 0.164.0 ask about each patch, so the handler decides where it lands.
fn set_approval_policy(
    params: &mut Value,
    environment: &std::collections::BTreeMap<String, String>,
    answer_only: bool,
    scripts: Scripts,
    desk: bool,
) {
    if answer_only {
        restrict_to_answer_only(params);
        match scripts {
            Scripts::None => {}
            Scripts::Sandboxed => {
                if let Some(params) = params.as_object_mut() {
                    params.remove("sandbox");
                }
            }
            Scripts::Unsandboxed => params["sandbox"] = json!("danger-full-access"),
        }
    } else if is_remote_hands(environment) {
        restrict_to_remote_hands(params);
    } else if desk {
        params["sandbox"] = json!("read-only");
        params["approvalPolicy"] = json!("untrusted");
    } else if GateClient::from_environment(environment).is_some() {
        params["approvalPolicy"] = json!("untrusted");
    }
}

/// Everything `open` needs to start a thread.
struct Opening<'a> {
    config: &'a CodexConfig,
    terminal: Object<dyn AgentTerminal>,
    cwd: &'a str,
    environment: &'a std::collections::BTreeMap<String, String>,
    mode: Mode<'a>,
    thread: ThreadStart,
    model: Option<String>,
    effort: Option<String>,
    /// The developer instructions of a new thread.
    instructions: Option<String>,
    gate: Option<Gate>,
    /// For a restricted (answer-only, assistant or `desk`) thread, what it may use.
    restriction: Option<Restriction>,
    profile: Option<&'a sandbox::Profile>,
}

/// Starts the app server on the Station's terminal and opens the thread, through
/// `coqui-acp-codex`.
fn open(opening: Opening<'_>) -> Result<CodexRun, ProviderError> {
    let Opening {
        config,
        terminal,
        cwd,
        environment,
        mode,
        thread,
        model,
        effort,
        instructions,
        gate,
        restriction,
        profile,
    } = opening;
    let scripts = Scripts::of(mode.assistant);
    let (argv, weakening) = prepare(&terminal, &config.executable, cwd, environment, mode)?;
    let mut policy = json!({});
    set_approval_policy(
        &mut policy,
        environment,
        mode.answer_only,
        scripts,
        mode.desk,
    );
    let mut launch = CodexLaunch::new(cwd);
    launch.executable = config.executable.clone().into();
    launch.thread = thread;
    launch.model = model;
    launch.effort = effort;
    launch.developer_instructions = instructions;
    launch.approval_policy = policy["approvalPolicy"].as_str().map(str::to_owned);
    launch.sandbox = policy["sandbox"].as_str().map(str::to_owned);
    if let Some(settings) = policy["config"].as_object() {
        launch.thread_config = settings.clone();
    }
    launch.extra_args = argv.iter().skip(2).map(OsString::from).collect();
    launch.client_info = ClientInfo::new("ai6", env!("CARGO_PKG_VERSION"));

    let link = Arc::new(Link::default());
    let shared = Arc::new(Shared::new(
        gate,
        restriction,
        scripts != Scripts::None,
        Path::new(cwd).to_path_buf(),
        Arc::clone(&link),
    ));
    shared.weakened(weakening);

    let mut settings = CodexSettings::new();
    settings.max_line = config.json_line_bytes;
    settings.tail_lines = usize::try_from(config.tail_lines).unwrap_or(usize::MAX);
    settings.tail_line_bytes = usize::try_from(config.tail_line_bytes).unwrap_or(usize::MAX);
    settings.initialize_timeout = config.millis(config.start_timeout_ms);
    settings.request_timeout = config.millis(config.request_timeout_ms);
    settings.exit_wait = config.millis(config.exit_wait_ms);
    settings.close_wait = config.millis(config.close_wait_ms);
    settings.list_models = false;
    settings.version_wait = Duration::ZERO;
    {
        let shared = Arc::clone(&shared);
        settings.on_line = Some(Arc::new(move |message| shared.line(message)));
    }
    if let Some(profile) = profile {
        let (config_check, thread_check) = (profile.clone(), profile.clone());
        settings.check_config = Some(Arc::new(move |result| {
            config_check
                .check_config(result)
                .map_err(|found| found.to_string())
        }));
        settings.check_thread = Some(Arc::new(move |result| {
            thread_check
                .check_thread(result)
                .map_err(|found| found.to_string())
        }));
    }
    let mut limits = ai6::station::limits::TerminalOutputLimits::default();
    limits.json_line_bytes = ai6::Limit::at(config.json_line_bytes);
    limits.csi_bytes = ai6::Limit::at(config.csi_bytes);
    let spawner = TerminalSpawn {
        terminal,
        environment: environment.clone(),
        limits,
        link,
    };
    let sink = Arc::clone(&shared);
    let session = AppServer::start(&launch, &spawner, settings, move |event| sink.tell(event))
        .map_err(started_with)?;
    // The thread was opened with the model; a turn names none of its own.
    session.set_model(None);
    let thread_id = RunId::try_from(session.thread().to_owned())
        .map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error))?;
    let session = Arc::new(session);
    *lock(&shared.session)? = Arc::downgrade(&session);
    Ok(CodexRun {
        session,
        shared,
        thread_id,
    })
}

/// What a failed start was, in AI6's terms.
fn started_with(error: CodexError) -> ProviderError {
    match error {
        CodexError::Refused(why) => refuse(Status::GUATIAO_ERR_UNSUPPORTED, why),
        CodexError::Timeout { .. } => {
            refuse(Status::GUATIAO_ERR_INTERNAL, "Codex app-server timed out")
        }
        CodexError::Harness(why) if why.ends_with("not the one requested") => refuse(
            Status::GUATIAO_ERR_BAD_VALUE,
            "Codex resumed a different thread than requested",
        ),
        other => refuse(Status::GUATIAO_ERR_INTERNAL, polished(&other.to_string())),
    }
}

/// An exit reason in the words of a Run: how the driver words it, with a negative exit code also
/// as Windows names it.
fn polished(reason: &str) -> String {
    let mut text = reason
        .replace("before completing the session", "before completing the Run")
        .replace("closed its output", "closed its terminal");
    const CODE: &str = "exited with code ";
    if let Some(at) = text.find(CODE) {
        let start = at + CODE.len();
        let digits = text[start..]
            .char_indices()
            .find(|(index, c)| !(c.is_ascii_digit() || (*index == 0 && *c == '-')))
            .map_or(text.len(), |(index, _)| start + index);
        if let Ok(code) = text[start..digits].parse::<i64>() {
            text.replace_range(start..digits, &exit_code_text(code));
        }
    }
    text
}

/// What a Run's reader thread and its caller share.
struct Shared {
    gate: Option<Gate>,
    events: Mutex<VecDeque<AgentEvent>>,
    arrived: Condvar,
    /// The events of approval requests, told once the request is answered, after the events the
    /// answer itself gave.
    held: Mutex<Vec<AgentEvent>>,
    /// The latest quota window the account reported as used up.
    exhausted: Mutex<Option<LimitHit>>,
    /// What the thread's requests have used since this process started: Codex reports each
    /// request, and its own running total survives a resume.
    spent: Mutex<Spend>,
    /// For a restricted (answer-only, assistant or `desk`) thread, what it may use.
    restricted: Option<Restriction>,
    /// Whether a command it starts is reported as the tool `exec_command`: an assistant with
    /// scripts, whose commands may read any file of her working directory and, unsandboxed,
    /// reach any host.
    reports_commands: bool,
    /// Set once a restricted thread used a tool it may not, and was ended.
    tripped: AtomicBool,
    /// The command of each command execution item the thread started, by item id.
    started: Mutex<HashMap<String, String>>,
    /// The paths of each file change item the thread started, by item id.
    started_changes: Mutex<HashMap<String, Vec<String>>>,
    /// Where a file change is the Mission's own work.
    working_directory: std::path::PathBuf,
    /// Set once the end of the app server was reported as a `failed` event.
    ended_reported: AtomicBool,
    link: Arc<Link>,
    session: Mutex<Weak<AppServer>>,
}

fn failed(reason: String) -> AgentEvent {
    AgentEvent {
        kind: "failed".to_owned(),
        text: Some(reason),
        data: None,
        limit: None,
        tools: Vec::new(),
        report: None,
    }
}

impl Shared {
    fn new(
        gate: Option<Gate>,
        restricted: Option<Restriction>,
        reports_commands: bool,
        working_directory: std::path::PathBuf,
        link: Arc<Link>,
    ) -> Self {
        Self {
            gate,
            events: Mutex::new(VecDeque::new()),
            arrived: Condvar::new(),
            held: Mutex::new(Vec::new()),
            exhausted: Mutex::new(None),
            spent: Mutex::new(Spend::new("")),
            restricted,
            reports_commands,
            tripped: AtomicBool::new(false),
            started: Mutex::default(),
            started_changes: Mutex::default(),
            working_directory,
            ended_reported: AtomicBool::new(false),
            link,
            session: Mutex::new(Weak::new()),
        }
    }

    fn push(&self, event: AgentEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push_back(event);
            self.arrived.notify_all();
        }
    }

    fn flush_held(&self) {
        let held = self
            .held
            .lock()
            .map(|mut held| std::mem::take(&mut *held))
            .unwrap_or_default();
        for event in held {
            self.push(event);
        }
    }

    /// Reports what a `warn` tool check found, on the Station's log and as the Run's
    /// first event (kind `weakened`).
    fn weakened(&self, weakening: Option<String>) {
        if let Some(text) = weakening {
            eprintln!("ai6: weakening: {text}");
            self.push(AgentEvent {
                kind: "weakened".to_owned(),
                text: Some(text),
                data: None,
                limit: None,
                tools: Vec::new(),
                report: None,
            });
        }
    }

    /// Ends a restricted thread that used `what`: the tool check confirmed before the start
    /// what Codex said it would offer, and this is the backstop for what it offered anyway.
    fn trip(&self, what: &str) {
        if self.tripped.swap(true, Ordering::AcqRel) {
            return;
        }
        let reason = format!(
            "the restricted Codex thread used {what}, which it may not use; the Run was ended"
        );
        eprintln!("ai6: {reason}");
        self.link.kill();
        self.ended_reported.store(true, Ordering::Release);
        self.push(failed(reason));
    }

    /// Takes one message the app server wrote, before the driver reads it; `false` ends the app
    /// server.
    fn line(&self, message: &Value) -> bool {
        self.flush_held();
        // An answer to one of the driver's own requests.
        if message.get("method").is_none() {
            return true;
        }
        if self.tripped.load(Ordering::Acquire) {
            return false;
        }
        if let Some(restriction) = &self.restricted
            && let Some(what) = forbidden_use(message, restriction)
        {
            self.trip(&what);
            return false;
        }
        if message.get("method").and_then(Value::as_str) == Some("item/started")
            && let Some(item) = message.pointer("/params/item")
            && let Some(id) = item.get("id").and_then(Value::as_str)
        {
            match item.get("type").and_then(Value::as_str) {
                Some("fileChange") => {
                    let paths = item
                        .get("changes")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|change| change.get("path").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect();
                    if let Ok(mut started) = self.started_changes.lock() {
                        started.insert(id.to_owned(), paths);
                    }
                }
                Some("commandExecution") => {
                    if let Some(command) = item.get("command").and_then(Value::as_str)
                        && let Ok(mut started) = self.started.lock()
                    {
                        started.insert(id.to_owned(), command.to_owned());
                    }
                }
                _ => {}
            }
        }
        if let Some(exhausted) = exhausted_window(message)
            && let Ok(mut hint) = self.exhausted.lock()
        {
            *hint = Some(exhausted);
        }
        let hint = self.exhausted.lock().ok().and_then(|hint| hint.clone());
        let Some(mut event) = decode_event(message, hint.as_ref()) else {
            return true;
        };
        if self.reports_commands && started_item(message) == Some("commandExecution") {
            event.tools.push(COMMAND_TOOL.to_owned());
        }
        if let Some(spend) = event
            .report
            .as_mut()
            .and_then(|report| report.spend.as_mut())
            && let Ok(mut spent) = self.spent.lock()
        {
            spent.input = spent.input.saturating_add(spend.input);
            spent.output = spent.output.saturating_add(spend.output);
            spent.cache_read = spent.cache_read.saturating_add(spend.cache_read);
            spent.cache_write = spent.cache_write.saturating_add(spend.cache_write);
            *spend = spent.clone();
        }
        let asks = message.get("id").is_some()
            && matches!(
                message.get("method").and_then(Value::as_str),
                Some(COMMAND_APPROVAL | FILE_APPROVAL)
            );
        if asks {
            if let Ok(mut held) = self.held.lock() {
                held.push(event);
            }
        } else {
            self.push(event);
        }
        true
    }

    /// Takes what the driver tells of the app server.
    fn tell(&self, event: CodexEvent) -> bool {
        match event {
            CodexEvent::Permission(request) => self.permission(request),
            CodexEvent::Output(text) => self.push(AgentEvent {
                kind: "output".to_owned(),
                text: Some(text),
                data: None,
                limit: None,
                tools: Vec::new(),
                report: None,
            }),
            CodexEvent::Exited(reason) => {
                self.flush_held();
                // The app server's exit ends the Run once, with the reason.
                if !self.ended_reported.swap(true, Ordering::AcqRel) {
                    let reason = polished(&reason);
                    eprintln!("ai6: {reason}");
                    self.push(failed(reason));
                }
            }
            _ => {}
        }
        true
    }

    /// Answers what the app server asks leave for. A question that is not a command or a file
    /// change is left unanswered, which the driver reports to the app server as nobody having
    /// chosen.
    fn permission(&self, request: PermissionRequest) {
        match request
            .details
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .as_deref()
        {
            Some(COMMAND_APPROVAL) => self.answer_command_approval(request),
            Some(FILE_APPROVAL) => self.answer_file_approval(request),
            _ => {}
        }
        self.flush_held();
    }

    /// A hold the gate did not decide: logged, and queued as a `declined` event.
    fn declined(&self, reason: &str) -> GateVerdict {
        eprintln!("ai6: {reason}");
        self.push(AgentEvent {
            kind: "declined".to_owned(),
            text: Some(reason.to_owned()),
            data: None,
            limit: None,
            tools: Vec::new(),
            report: None,
        });
        GateVerdict::Hold(reason.to_owned())
    }

    /// Answers a command approval with the green ink gate: `accept` unless the gate holds the
    /// command, then `decline`, and the reason is steered into the running turn so the Agent
    /// knows to ask for green ink. With a gate, these are declined without asking it: input to
    /// a running command and a request that names no command (the gate cannot see what they
    /// run), a request whose line was rebuilt from a terminal's rendering (`rebuilt`: its
    /// command may not be what Codex wrote), and one with no command execution item started
    /// earlier under its item id with the same command. A decline the gate did not decide is
    /// also logged and queued as an event of kind `declined`. Without a gate (no Cable
    /// endpoint) everything is accepted.
    fn answer_command_approval(&self, request: PermissionRequest) {
        let input = request.tool.input.clone().unwrap_or(Value::Null);
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .filter(|command| !command.is_empty());
        let cwd = input.get("cwd").and_then(Value::as_str).map(Path::new);
        let kind = request.details.get("kind").and_then(Value::as_str);
        let started = request
            .details
            .get("itemId")
            .and_then(Value::as_str)
            .and_then(|item| self.started.lock().ok()?.get(item).cloned());
        let rebuilt = self.link.rebuilt.load(Ordering::Acquire);
        let unseen = |command: &str| started.as_deref() != Some(command);
        let verdict = match (&self.gate, kind, command) {
            (None, _, _) => GateVerdict::Allow,
            (Some(_), _, _) if rebuilt => self.declined(
                "Green ink: this approval request reached AI6 rebuilt from a terminal's \
                 rendering (a line it broke, or a control inside it), so its command may not be \
                 what was asked; it is declined. Run the command again.",
            ),
            (Some(_), kind, Some(command)) if kind != Some("writeStdin") && unseen(command) => self
                .declined(
                    "Green ink: this approval request names a command no command execution \
                     item started with before it; it is declined. Run the command again.",
                ),
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
        self.answer_approval(request, verdict);
    }

    /// Answers a file change the way the Claude handler's gate treats an edit: green ink governs
    /// shell commands, so an edit inside the working directory is the Mission's own work and
    /// `accept`ed. Codex 0.164.0 applies what is accepted wherever it points, even from a
    /// read-only thread, so a gated thread is declined a path outside the working directory, a
    /// `grantRoot`, a request whose line was `rebuilt`, and one with no file change item started
    /// earlier under its item id, and so is a `desk` thread, gated or not. Otherwise, without a
    /// gate, everything is accepted.
    fn answer_file_approval(&self, request: PermissionRequest) {
        let paths = request
            .details
            .get("itemId")
            .and_then(Value::as_str)
            .and_then(|item| self.started_changes.lock().ok()?.get(item).cloned());
        let granted = request
            .details
            .get("grantRoot")
            .is_some_and(|root| !root.is_null());
        let judged = self.gate.is_some()
            || self
                .restricted
                .as_ref()
                .is_some_and(|allowed| allowed.asks_file_changes);
        let rebuilt = self.link.rebuilt.load(Ordering::Acquire);
        let root = &self.working_directory;
        let verdict = match (judged, paths) {
            (false, _) => GateVerdict::Allow,
            _ if rebuilt => self.declined(
                "Green ink: this file change request reached AI6 rebuilt from a terminal's \
                 rendering, so what it changes may not be what was asked; it is declined. Apply \
                 the change again.",
            ),
            _ if granted => self.declined(
                "Green ink: a file change may not widen where this thread writes; it is \
                 declined. Keep the change inside the working directory.",
            ),
            (true, Some(paths))
                if !paths.is_empty() && paths.iter().all(|path| inside(root, path)) =>
            {
                GateVerdict::Allow
            }
            (true, Some(_)) => self.declined(
                "Green ink: this file change reaches outside the working directory; it is \
                 declined. Keep the change inside the working directory.",
            ),
            (true, None) => self.declined(
                "Green ink: this approval request names a change no file change item started \
                 with before it; it is declined. Apply the change again.",
            ),
        };
        self.answer_approval(request, verdict);
    }

    /// Writes the decision, and steers a held step's reason into the running turn.
    fn answer_approval(&self, request: PermissionRequest, verdict: GateVerdict) {
        let turn = request
            .details
            .get("turnId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let chosen = match verdict {
            GateVerdict::Allow => coqui_acp_codex::parse::ALLOW_ONCE,
            GateVerdict::Hold(_) => coqui_acp_codex::parse::REJECT,
        };
        let _ = request
            .responder
            .answer(Outcome::Selected(chosen.to_owned()));
        if let GateVerdict::Hold(reason) = verdict
            && let Some(turn) = turn
            && let Some(session) = self
                .session
                .lock()
                .ok()
                .and_then(|session| session.upgrade())
        {
            let _ = session.steer(&turn, &[Content::Text(reason)]);
        }
    }
}

/// A started Codex thread, as one of AI6's Runs.
struct CodexRun {
    session: Arc<AppServer>,
    shared: Arc<Shared>,
    thread_id: RunId,
}

/// The tool name a scripts thread's command executions are reported under.
const COMMAND_TOOL: &str = "exec_command";

/// The type of the item an `item/started` notification starts.
fn started_item(message: &Value) -> Option<&str> {
    (message.get("method")?.as_str()? == "item/started")
        .then(|| message.pointer("/params/item/type")?.as_str())
        .flatten()
}

/// The app-server's request to approve a command before it runs.
const COMMAND_APPROVAL: &str = "item/commandExecution/requestApproval";

/// The app-server's request to approve a file change before it is applied.
const FILE_APPROVAL: &str = "item/fileChange/requestApproval";

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

/// What `message` shows a restricted thread doing that it may not: any request from the server
/// but token refresh, attestation and, as `restriction` allows, a file change (approvals, tool
/// calls, questions, elicitation), or an `item/started` or `item/completed` of any other kind than
/// [`RESTRICTED_ITEMS`], an MCP call to one of its servers, and, as `restriction` allows, a command
/// execution or a file change.
fn forbidden_use(message: &Value, restriction: &Restriction) -> Option<String> {
    let servers = &restriction.servers;
    let method = message.get("method")?.as_str()?;
    if message.get("id").is_some() {
        if method == FILE_APPROVAL && restriction.asks_file_changes {
            return None;
        }
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
        "commandExecution" if restriction.commands => None,
        "fileChange" if restriction.file_changes => None,
        "mcpToolCall" if servers.iter().any(|server| server == field("server")) => None,
        "mcpToolCall" => Some(format!(
            "the tool {:?} of MCP server {:?}",
            field("tool"),
            field("server")
        )),
        kind => Some(format!("a {kind:?} item")),
    }
}

/// Whether `path` lies under `root`, as far as the file system resolves either: a link inside the
/// directory that points out of it counts as outside.
fn inside(root: &Path, path: &str) -> bool {
    let path = Path::new(path);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if joined
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return false;
    }
    resolved(&joined).starts_with(resolved(root))
}

/// `path` with its longest existing ancestor made canonical; what does not exist yet follows it.
fn resolved(path: &Path) -> std::path::PathBuf {
    let mut existing = path;
    let mut rest = Vec::new();
    loop {
        if let Ok(found) = existing.canonicalize() {
            return rest.iter().rev().fold(found, |path, part| path.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
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

/// The bucket one Codex rate-limit window is: `usedPercent` as thousandths, `resetsAt` in Unix
/// seconds, exhausted once it is used up.
fn bucket_of(window: &Value, position: &str) -> Bucket {
    let mut bucket = Bucket::new(window_name(window, position));
    if let Some(percent) = window.get("usedPercent").and_then(Value::as_f64) {
        bucket.limit = Some(1000);
        bucket.used = Some((percent * 10.0).round().clamp(0.0, 1000.0) as u64);
        if percent >= 100.0 {
            bucket.status = BucketStatus::Exhausted;
        }
    }
    bucket.refill = Refill::FixedWindow {
        length: window
            .get("windowDurationMins")
            .and_then(Value::as_u64)
            .map(|minutes| std::time::Duration::from_secs(minutes.saturating_mul(60))),
        resets_at: window.get("resetsAt").and_then(Value::as_i64),
    };
    bucket
}

/// Every quota window an `account/rateLimits/updated` notification reports: `usedPercent`
/// as thousandths, `resetsAt` in Unix seconds.
fn decode_quota(message: &Value) -> Vec<Bucket> {
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
            Some(bucket_of(window, position))
        })
        .collect()
}

/// The quota window an `account/rateLimits/updated` notification reports as used up.
///
/// The usage-limit error that follows does not always repeat the reset time, so the run
/// keeps the latest exhausted window as a hint.
fn exhausted_window(message: &Value) -> Option<LimitHit> {
    if message.get("method")?.as_str()? != "account/rateLimits/updated" {
        return None;
    }
    let limits = message.pointer("/params/rateLimits")?;
    ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let window = limits.get(key)?;
            (window.get("usedPercent")?.as_f64()? >= 100.0).then(|| {
                let mut hit = LimitHit::new(0);
                hit.resets_at = window.get("resetsAt").and_then(Value::as_i64);
                hit.bucket = match window.get("windowDurationMins").and_then(Value::as_i64) {
                    Some(300 | 10_080) => Some(window_name(window, "")),
                    _ => None,
                };
                hit
            })
        })
        // Work resumes only when every exhausted window has reset.
        .max_by_key(|hit| hit.resets_at)
}

/// Recognises the usage-limit error Codex reports on a failed turn.
///
/// The error object (`params.error`, or `params.turn.error` on `turn/failed` and a failed
/// `turn/completed`) names the limit in `codexErrorInfo`; its reset time is `resetsAt` in
/// Unix seconds when present, else the exhausted window last reported by the account.
fn decode_limit(message: &Value, hint: Option<&LimitHit>) -> Option<(LimitHit, Option<String>)> {
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
    let text = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut hit = LimitHit::new(0);
    hit.resets_at = resets_at;
    hit.bucket = hint.and_then(|hint| hint.bucket.clone());
    // A wait the message states (`try again in 90 seconds`) is the refusal's own.
    hit.retry_after = text
        .as_deref()
        .and_then(|text| infer_limit(text, &["limit"]))
        .and_then(|stated| stated.retry_after);
    Some((hit, text))
}

/// The tokens one model request used, from a `thread/tokenUsage/updated` notification.
///
/// The notification also carries the thread's running total, which survives a resume, so
/// only `last` (the latest request) is read and reported as an addition. Codex reports no
/// cost. Its `inputTokens` include the cached ones, which are split out here.
fn decode_usage(message: &Value) -> Option<Spend> {
    if message.get("method")?.as_str()? != "thread/tokenUsage/updated" {
        return None;
    }
    let last = message.pointer("/params/tokenUsage/last")?;
    let count = |key: &str| last.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = count("cachedInputTokens");
    let mut spend = Spend::new("");
    spend.input = count("inputTokens").saturating_sub(cached);
    spend.output = count("outputTokens");
    spend.cache_read = cached;
    Some(spend)
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

/// What a notification tells of the account's usage: one request's spend (an addition, which
/// the run turns into a running total) and the rate-limit windows.
fn decode_report(message: &Value) -> Option<UsageReport> {
    let spend = decode_usage(message);
    let buckets = decode_quota(message);
    if spend.is_none() && buckets.is_empty() {
        return None;
    }
    let mut report = UsageReport::new();
    report.spend = spend;
    report.buckets = buckets;
    Some(report)
}

fn decode_event(message: &Value, hint: Option<&LimitHit>) -> Option<AgentEvent> {
    let method = message.get("method")?.as_str().unwrap_or("event");
    if let Some((limit, text)) = decode_limit(message, hint) {
        return Some(AgentEvent {
            kind: LIMIT_EVENT.to_owned(),
            text,
            data: serde_json::to_vec(message).ok(),
            limit: Some(limit),
            tools: Vec::new(),
            report: None,
        });
    }
    // Codex retries a gateway's refusal while `willRetry` is set; a token-cap refusal is the same
    // every time, so the turn ends with the gateway's own words.
    if method == "error"
        && let Some(words) = message.get("params").and_then(ai6::token_cap_refusal)
    {
        return Some(AgentEvent {
            kind: "failed".to_owned(),
            text: Some(words),
            data: serde_json::to_vec(message).ok(),
            limit: None,
            tools: Vec::new(),
            report: None,
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
        tools: decode_tools(message),
        report: decode_report(message),
    })
}

impl AgentSession for CodexRun {
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.thread_id.clone().into()))
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        self.session
            .prompt(&[Content::Text(message.text)])
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(0) as u64);
        let mut events = lock(&self.shared.events)?;
        loop {
            if let Some(event) = events.pop_front() {
                return Ok(Some(event));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            events = self
                .shared
                .arrived
                .wait_timeout(events, remaining)
                .map_err(|_| refuse(Status::GUATIAO_ERR_INTERNAL, "run lock was poisoned"))?
                .0;
        }
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        self.session
            .interrupt()
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))
    }

    /// Ends the app server. The caller may remove the working directory next, which the app
    /// server holds until it has exited, so this waits for that.
    fn close(&self) -> Result<(), ProviderError> {
        self.session.close();
        Ok(())
    }
}

/// What AI6 decided about the thread's settings.
#[derive(Clone, Copy)]
struct Mode<'a> {
    project_settings: bool,
    /// Set for answer-only and assistant threads alike: both run read-only and never ask.
    answer_only: bool,
    /// Set for a Mission's thread held to `desk` ([`held_to_desk`]).
    desk: bool,
    assistant: Option<&'a AssistantMode>,
    /// The model the thread asks for; what Codex offers it depends on the model.
    model: Option<&'a str>,
    /// The sandbox of an assistant's scripts, when they run in one.
    profile: Option<&'a sandbox::Profile>,
    /// The handler's configuration: the tool check's and the sandbox probe's bounds.
    config: &'a CodexConfig,
}

/// The sandbox `assistant`'s scripts run in, `None` when they run in none.
fn scripts_profile(
    executable: &str,
    cwd: &str,
    environment: &std::collections::BTreeMap<String, String>,
    assistant: Option<&AssistantMode>,
) -> Option<sandbox::Profile> {
    assistant
        .filter(|mode| Scripts::of(Some(mode)) == Scripts::Sandboxed)
        .map(|mode| sandbox::Profile::new(executable, cwd, environment, mode))
}

/// The app server's command line, and what a `warn` tool check found. A restricted (answer-only,
/// assistant or `desk`) app server starts only after its tool check ([`tools`]).
fn prepare(
    terminal: &Object<dyn AgentTerminal>,
    executable: &str,
    cwd: &str,
    environment: &std::collections::BTreeMap<String, String>,
    mode: Mode<'_>,
) -> Result<(Vec<String>, Option<String>), ProviderError> {
    let mut argv = if let Some(assistant) = mode.assistant {
        assistant_argv(executable, environment, assistant, mode.profile)?
    } else if mode.answer_only {
        answer_only_argv(executable)
    } else if mode.desk {
        desk_argv(executable, environment)
    } else {
        app_server_argv(executable, environment)
    };
    if !mode.project_settings || mode.answer_only {
        argv.extend(untrusted_project(cwd));
    }
    let scripts = Scripts::of(mode.assistant);
    let mut weakening = None;
    if mode.answer_only || mode.desk {
        let servers = if mode.desk {
            station_servers(environment)
        } else {
            mode.assistant
                .map(|assistant| {
                    assistant
                        .mcp_servers
                        .iter()
                        .map(|server| server.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let commands = scripts != Scripts::None;
        let checked = tools::Probe {
            terminal,
            executable,
            overrides: &argv[2..],
            cwd,
            environment,
            model: mode.model,
            servers: &servers,
            code_mode: commands || mode.desk,
            commands,
            config: mode.config,
            // `apply_patch` is nested in `exec` in remote-hands mode too, where a read-only sandbox
            // that never asks refuses it.
            files: mode.desk
                || restriction(mode.answer_only, mode.assistant, None, false)
                    .is_some_and(|allowed| allowed.file_changes),
        }
        .check()?;
        argv.extend(checked.overrides);
        weakening = checked.weakening;
    }
    if let Some(assistant) = mode.assistant.filter(|_| scripts != Scripts::None) {
        let admitted = format!(
            "Codex gives Moneypenny's thread functions.exec and functions.wait (code mode): her \
             scripts may run any command {}, not only {}",
            if scripts == Scripts::Sandboxed {
                "inside Codex's sandbox"
            } else {
                "with no sandbox"
            },
            assistant.interpreters.join(", ")
        );
        weakening = Some(match weakening {
            Some(found) => format!("{admitted}; {found}"),
            None => admitted,
        });
    }
    Ok((argv, weakening))
}

/// The app server of a Mission, with AI6's servers its environment names, their tools approved
/// without asking: Codex otherwise asks about each MCP call, which fails it under approval `never`
/// and leaves it unanswered under `untrusted`. The Station checks what those tools do. In
/// remote-hands mode it has no command tool: the harness runs on the Station, where nothing it does
/// is to land, and Codex's read-only sandbox lets a command read any file there (Codex 0.164.0 on
/// Linux).
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
                "-c".to_owned(),
                format!("mcp_servers.{server}.default_tools_approval_mode=\"approve\""),
            ]);
            if server == "ai6-theater"
                && let Some(seconds) = theater_tool_timeout_secs(environment)
            {
                argv.extend([
                    "-c".to_owned(),
                    format!("mcp_servers.{server}.tool_timeout_sec={seconds}"),
                ]);
            }
        }
    }
    if is_remote_hands(environment) {
        argv.extend(["-c".to_owned(), "features.shell_tool=false".to_owned()]);
    }
    argv
}

/// Seconds Codex's MCP client waits for a Theater tool call (its default is 60 s, which would end a
/// longer `theater_run`): the Station's `AI6_THEATER_MCP_TIMEOUT_MS`, rounded up.
fn theater_tool_timeout_secs(
    environment: &std::collections::BTreeMap<String, String>,
) -> Option<u64> {
    let millis: u64 = environment
        .get("AI6_THEATER_MCP_TIMEOUT_MS")?
        .parse()
        .ok()?;
    Some(millis.div_ceil(1000).max(1))
}

/// The Theater tools a `desk` thread may not call: one runs a command, the other writes any path on
/// the Theater. Codex 0.164.0 leaves a tool named here out of `exec`.
const THEATER_TOOLS_OFF: &str = "mcp_servers.ai6-theater.disabled_tools=[\"theater_run\",\
                                 \"theater_write\"]";

/// The app server of a Mission's thread held to `desk`: AI6's servers, and no built-in tool
/// ([`NO_BUILT_IN_TOOLS`]). With `features.shell_tool` off Codex 0.164.0 offers `functions.exec`
/// and `functions.wait` with no command tool nested, and `exec`'s JavaScript has no file system,
/// process or network of its own; the tool check ([`tools`]) confirms what a release offers.
/// That leaves the thread nothing that reads a file, since Codex reads through commands.
fn desk_argv(
    executable: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    let mut argv = app_server_argv(executable, environment);
    argv.extend(no_built_in_tools(false));
    if is_remote_hands(environment) {
        argv.extend(["-c".to_owned(), THEATER_TOOLS_OFF.to_owned()]);
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

/// `-c` overrides for [`NO_BUILT_IN_TOOLS`]; with `shell`, the command tools stay on.
fn no_built_in_tools(shell: bool) -> Vec<String> {
    NO_BUILT_IN_TOOLS
        .iter()
        .filter(|setting| !(shell && setting.starts_with("features.shell_tool=")))
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
    argv.extend(no_built_in_tools(false));
    argv
}

/// The app server of an assistant thread, built from its mode's parts: `mcp_servers` set to
/// one inline table holding exactly the mode's servers, which Codex merges into the user's own
/// (the tool check switches those off by name, [`tools`]), each with its URL, the name of the
/// variable holding its bearer token and its tools approved without asking (under approval
/// `never` Codex 0.164.0 fails every MCP call it would ask about); no built-in tool
/// ([`NO_BUILT_IN_TOOLS`]); and no project instructions file (`project_doc_max_bytes=0`), since
/// its standing context is the mode's `context_file`, which the thread gets as developer
/// instructions.
///
/// With interpreters the command tools stay on and the thread's commands run under `profile`
/// ([`sandbox::Profile`]): the mode's `files` directories writable, nothing outside the working
/// directory readable, no network; unsandboxed, under none. Without interpreters `exec` is not
/// offered, so `files` gives no tool to write with. Skills are refused: Codex keeps no list of
/// allowed skills.
fn assistant_argv(
    executable: &str,
    environment: &std::collections::BTreeMap<String, String>,
    mode: &AssistantMode,
    profile: Option<&sandbox::Profile>,
) -> Result<Vec<String>, ProviderError> {
    mode.validate()
        .map_err(|reason| refuse(Status::GUATIAO_ERR_BAD_VALUE, reason))?;
    if !mode.skills.is_empty() {
        return Err(refuse(Status::GUATIAO_ERR_UNSUPPORTED, NO_SKILL_LIST));
    }
    let scripts = Scripts::of(Some(mode));
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
            "{}={{url={url},bearer_token_env_var={token},\
             default_tools_approval_mode=\"approve\"}}",
            server.name
        ));
    }
    let mut argv = vec![
        executable.to_owned(),
        "app-server".to_owned(),
        "-c".to_owned(),
        format!("mcp_servers={{{}}}", servers.join(",")),
    ];
    argv.extend(no_built_in_tools(scripts != Scripts::None));
    argv.extend(["-c".to_owned(), "project_doc_max_bytes=0".to_owned()]);
    if let Some(profile) = profile {
        argv.extend(profile.overrides());
    }
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

/// In remote-hands mode the Theater tools are how a thread changes anything. Its command tool is off
/// ([`app_server_argv`]), and its read-only sandbox that asks nothing refuses Codex's patches, so
/// no work lands on the Station.
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
        .map_err(|_| refuse(Status::GUATIAO_ERR_INTERNAL, "run lock was poisoned"))
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
    use ai6::directorates::q::terminal::{
        ExitInfo, READ_TIMED_OUT, STREAM_COLS, Spec, TerminalRun,
    };
    use ai6::guatiao::{FromValue, ToValue};
    use std::collections::BTreeMap;

    #[test]
    fn a_threads_rollout_is_found_under_codex_home() {
        let home = tempfile::tempdir().unwrap();
        let day = home.path().join("runs").join("2026").join("11").join("04");
        std::fs::create_dir_all(&day).unwrap();
        let file = day.join("rollout-2026-11-04T10-00-00-0195-thread.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        let query = |thread: &str| RunRecordQuery {
            run_id: thread.to_owned(),
            working_directory: "/work".to_owned(),
            environment: BTreeMap::from([(
                "CODEX_HOME".to_owned(),
                home.path().to_string_lossy().into_owned(),
            )]),
        };
        let found = codex_record(&query("0195-thread")).unwrap();
        assert_eq!(std::path::PathBuf::from(found.path), file);
        assert_eq!(found.format, RECORD_FORMAT);
        assert!(codex_record(&query("other")).is_none());
        assert!(codex_record(&query("../0195-thread")).is_none());
    }

    #[test]
    fn codexs_usage_is_read_with_no_turn_and_a_codex_that_cannot_run_is_an_error_not_an_empty_report()
     {
        let config = CodexConfig {
            executable: "ai6-no-such-codex".to_owned(),
            ..CodexConfig::default()
        };
        let handler = CodexHandler::try_from(config).unwrap();
        let support = handler.usage_support().unwrap();
        assert!(!support.needs_turn && !support.needs_session);
        assert_eq!(support.sources, [UsageSource::Reported]);
        assert!(handler.usage(Default::default()).is_err());
    }

    #[test]
    fn the_models_codex_lists_are_read_from_model_list_and_hidden_ones_are_left_out() {
        let answer = json!({"id": 2, "result": {"data": [
            {"id": "gpt-6.1-sol", "model": "gpt-6.1-sol", "displayName": "GPT-6.1-Sol",
             "description": "Latest workhorse model", "hidden": false, "isDefault": true},
            {"id": "internal", "model": "internal", "hidden": true},
            {"id": "gpt-6-astra", "displayName": "Astra"}
        ]}});
        let listing = models_of(&answer);
        assert!(listing.known);
        let ids: Vec<_> = listing
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        assert_eq!(ids, ["gpt-6.1-sol", "gpt-6-astra"]);
        assert_eq!(listing.models[0].name.as_deref(), Some("GPT-6.1-Sol"));
    }

    #[test]
    fn an_answer_without_data_is_unknown_never_empty() {
        let listing =
            models_of(&json!({"id": 2, "error": {"code": -32601, "message": "no such method"}}));
        assert!(!listing.known && listing.models.is_empty());
        assert!(listing.unknown_because.is_some());
    }

    /// Against the real Codex on this machine, with an empty `CODEX_HOME`, no login and no
    /// network: the app server answers `model/list` from its own catalog. Run with
    /// `AI6_TEST_LIVE_HARNESSES=1`.
    #[test]
    #[ignore = "needs Codex installed; set AI6_TEST_LIVE_HARNESSES=1 and run with --ignored"]
    fn live_codex_lists_models_with_no_login_and_no_turn() {
        assert!(
            std::env::var_os("AI6_TEST_LIVE_HARNESSES").is_some(),
            "set AI6_TEST_LIVE_HARNESSES=1"
        );
        let home = tempfile::tempdir().unwrap();
        let environment = BTreeMap::from([
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            (
                "SYSTEMROOT".to_owned(),
                std::env::var("SYSTEMROOT").unwrap_or_default(),
            ),
            ("HOME".to_owned(), home.path().display().to_string()),
            ("USERPROFILE".to_owned(), home.path().display().to_string()),
            ("APPDATA".to_owned(), home.path().display().to_string()),
            ("LOCALAPPDATA".to_owned(), home.path().display().to_string()),
            (
                "CODEX_HOME".to_owned(),
                home.path().join(".codex").display().to_string(),
            ),
            ("HTTPS_PROXY".to_owned(), "http://127.0.0.1:9".to_owned()),
        ]);
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        let handler = CodexHandler::try_from(CodexConfig::default()).unwrap();
        let listing = handler.models(environment).unwrap();
        assert!(listing.known, "{listing:?}");
        assert!(!listing.models.is_empty(), "{listing:?}");
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
        /// The command word of each command run through `spawn_private`.
        private: Arc<Mutex<Vec<String>>>,
    }

    impl ProcessTerminal {
        fn new() -> Self {
            Self {
                specs: Arc::new(Mutex::new(Vec::new())),
                private: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl AgentTerminal for ProcessTerminal {
        fn spawn_private(
            &self,
            value: &ai6::guatiao::Value,
        ) -> Result<Object<dyn TerminalRun>, ProviderError> {
            let spec = Spec::from_value(value).map_err(internal)?;
            self.private.lock().unwrap().push(spec.argv[1].clone());
            self.spawn(value)
        }

        fn spawn(
            &self,
            value: &ai6::guatiao::Value,
        ) -> Result<Object<dyn TerminalRun>, ProviderError> {
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

            Ok(ProcessTerminalRun {
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

    struct ProcessTerminalRun {
        child: Mutex<Child>,
        stdin: Mutex<ChildStdin>,
        receiver: Mutex<Receiver<Vec<u8>>>,
        pending: Mutex<std::collections::VecDeque<u8>>,
    }

    impl TerminalRun for ProcessTerminalRun {
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
            // Codex answers a turn at once, with the turn's id, before it streams anything.
            if method == "turn/start"
                && let Some(id) = message.get("id")
            {
                writeln!(
                    stdout,
                    "{}",
                    json!({"id": id, "result": {"turn": {"id": "turn-7"}}})
                )
                .unwrap();
                stdout.flush().unwrap();
            }
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
                    "config/read" => {
                        let config = fake_config(&argv);
                        writeln!(stdout, "{}", json!({"id":id,"result":{"config":config}}))
                            .unwrap();
                        stdout.flush().unwrap();
                    }
                    "thread/start" | "thread/resume" => {
                        // Reports the thread's policy as text, for tests that check it.
                        if std::env::var_os("CODEX_FIXTURE_SHOW_POLICY").is_some() {
                            let policy = json!({
                                "sandbox": message.pointer("/params/sandbox"),
                                "approvalPolicy": message.pointer("/params/approvalPolicy"),
                                "config": message.pointer("/params/config"),
                                "developerInstructions":
                                    message.pointer("/params/developerInstructions"),
                            });
                            writeln!(
                                stdout,
                                "{}",
                                json!({"method":"item/agentMessage/delta",
                                       "params":{"delta":format!("policy: {policy}")}})
                            )
                            .unwrap();
                        }
                        let config = fake_config(&argv);
                        let network = config.pointer("/permissions/ai6_assistant/network/enabled")
                            == Some(&json!(true));
                        let profile = config
                            .get("default_permissions")
                            .map(|name| json!({ "id": name }));
                        writeln!(
                            stdout,
                            "{}",
                            json!({"id":id,"result":{
                                "thread":{"id":thread_id},
                                "activePermissionProfile": profile,
                                "sandbox": {"networkAccess": network}
                            }})
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

    /// The fake app server's effective configuration, as Codex 0.164.0 makes it: the sandbox
    /// settings its `-c` arguments give, merged over `CODEX_FIXTURE_USER_CONFIG` (JSON standing
    /// for the user's `config.toml`) table by table.
    fn fake_config(argv: &[&str]) -> Value {
        fn merge(into: &mut Value, from: Value) {
            match (into, from) {
                (Value::Object(into), Value::Object(from)) => {
                    for (key, value) in from {
                        merge(into.entry(key).or_insert(Value::Null), value);
                    }
                }
                (slot, value) => *slot = value,
            }
        }
        let mut config = std::env::var("CODEX_FIXTURE_USER_CONFIG")
            .map(|text| serde_json::from_str(&text).unwrap())
            .unwrap_or_else(|_| json!({}));
        for pair in argv.windows(2).filter(|pair| pair[0] == "-c") {
            let sandbox = [
                "permissions.",
                "default_permissions=",
                "shell_environment_policy.",
            ];
            if sandbox.iter().any(|prefix| pair[1].starts_with(prefix)) {
                let table: toml::Table = toml::from_str(pair[1]).unwrap();
                merge(&mut config, serde_json::to_value(table).unwrap());
            }
        }
        config
    }

    fn read_to_completion(run: &Object<dyn AgentSession>) -> Vec<AgentEvent> {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Some(event) = run
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
        panic!("fake Codex did not complete the Run")
    }

    #[test]
    fn process_run_restarts_and_resumes_with_the_same_provider_thread() {
        let terminal = ProcessTerminal::new();
        let run_id = RunId::try_new("codex-process-thread-42").unwrap();
        let environment =
            BTreeMap::from([("CODEX_FIXTURE_THREAD_ID".to_owned(), run_id.to_string())]);
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-process-fixture".to_owned(),
            ..CodexConfig::default()
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
                    model: Some("gpt-5".to_owned()),
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
        assert_eq!(started.session_id().unwrap(), Some(run_id.to_string()));
        assert!(first_events.iter().any(|event| {
            event.kind == "text" && event.text.as_deref() == Some("first process turn")
        }));
        assert!(first_events.iter().any(|event| event.kind == "completed"));
        started.close().unwrap();

        let resumed = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
                    mission_id: "mission-process-42".parse().unwrap(),
                    run_id: run_id.clone(),
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
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        assert_eq!(resumed.session_id().unwrap(), Some(run_id.to_string()));
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
            match ai6::classify(command, &Branches, 4) {
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

    fn texts_until_completed(run: &Object<dyn AgentSession>) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut texts = Vec::new();
        while Instant::now() < deadline {
            if let Some(event) = run.next_event(1_000).unwrap() {
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
            ..CodexConfig::default()
        })
        .unwrap();
        let run = handler
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
                    overlays: Default::default(),
                },
                ProcessTerminal::new().into_object(),
                approvals_gate(approved.clone()),
            )
            .unwrap();

        let declined = texts_until_completed(&run);
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
        run.send_message(AgentMessage {
            text: "run: git push origin main".to_owned(),
        })
        .unwrap();
        assert_eq!(texts_until_completed(&run), ["decision: accept"]);

        run.send_message(AgentMessage {
            text: "run: cargo test --workspace".to_owned(),
        })
        .unwrap();
        assert_eq!(texts_until_completed(&run), ["decision: accept"]);

        // The gate cannot see what these run, so they are declined and the Agent told why.
        for (text, why) in [
            ("stdin: git push origin main", "input to a running command"),
            ("nameless", "names no command"),
        ] {
            run.send_message(AgentMessage {
                text: text.to_owned(),
            })
            .unwrap();
            let declined = texts_until_completed(&run);
            assert_eq!(declined[0], "decision: decline", "{text}: {declined:?}");
            assert!(declined[1].contains(why), "{text}: {declined:?}");
        }
        run.close().unwrap();
    }

    /// What a gated run answers to `stream`: the decision written back, and the events.
    fn answered(stream: &[u8]) -> (Value, Vec<AgentEvent>) {
        let (run, writes) = scripted_run(
            stream,
            "/work",
            approvals_gate(Arc::new(Mutex::new(Vec::new()))),
            None,
        );
        let events = events_until_quiet(&run);
        let (decision, _) = written_decision(&writes).expect("the approval was answered");
        (decision, events)
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

    /// The `item/started` and request Codex 0.164.0 sends on an `apply_patch` in a gated
    /// (`approvalPolicy` untrusted) thread, field for field, for `path`.
    fn file_change(path: &str, grant_root: Value) -> String {
        format!(
            "{}\r\n{}\r\n",
            json!({"method": "item/started", "params": {
                "threadId": "01a19a10-25e3-74e3-8584-f85797bbe59f",
                "turnId": "01a19a10-2834-7e90-b669-8320d3df8e45",
                "item": {
                    "type": "fileChange",
                    "id": "exec-43c043eb-e8cf-4952-ab6b-7f8306cf9b5e",
                    "changes": [{"path": path, "kind": {"type": "add"}, "diff": "x\n"}],
                    "status": "inProgress",
                },
            }}),
            json!({"method": FILE_APPROVAL, "id": 0, "params": {
                "threadId": "01a19a10-25e3-74e3-8584-f85797bbe59f",
                "turnId": "01a19a10-2834-7e90-b669-8320d3df8e45",
                "itemId": "exec-43c043eb-e8cf-4952-ab6b-7f8306cf9b5e",
                "startedAtMs": 1_793_586_113_669_u64,
                "reason": null,
                "grantRoot": grant_root,
            }}),
        )
    }

    /// What a gated run working in `directory` answers to `stream`, and what it told the turn.
    fn answered_in(directory: &Path, stream: &str) -> (Value, String) {
        let (run, writes) = scripted_run(
            stream.as_bytes(),
            &directory.to_string_lossy(),
            approvals_gate(Arc::new(Mutex::new(Vec::new()))),
            None,
        );
        let _ = events_until_quiet(&run);
        written_decision(&writes).expect("the file change was answered, not left waiting")
    }

    #[test]
    fn a_file_change_inside_the_working_directory_is_accepted_and_any_other_declined() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir_all(work.join("notes")).unwrap();
        let path = |path: &Path| path.to_string_lossy().into_owned();

        let (decision, written) = answered_in(
            &work,
            &file_change(&path(&work.join("notes").join("p.md")), Value::Null),
        );
        assert_eq!(decision, "accept", "{written}");
        assert!(!written.contains("turn/steer"), "{written}");
        let (decision, _) = answered_in(
            &work,
            &file_change(
                &path(&work.join("new").join("deep").join("p.md")),
                Value::Null,
            ),
        );
        assert_eq!(decision, "accept", "a directory the change makes");

        for (why, stream) in [
            (
                "outside",
                file_change(&path(&root.path().join("outside.md")), Value::Null),
            ),
            (
                "climbing out",
                file_change(&format!("{}/../outside.md", path(&work)), Value::Null),
            ),
            (
                "a wider root",
                file_change(&path(&work.join("p.md")), json!(path(root.path()))),
            ),
        ] {
            let (decision, written) = answered_in(&work, &stream);
            assert_eq!(decision, "decline", "{why}");
            assert!(written.contains("turn/steer"), "{why}: the Agent is told");
        }

        let unseen = json!({"method": FILE_APPROVAL, "id": 0, "params": {
            "threadId": "t", "turnId": "u", "itemId": "never-started",
            "reason": null, "grantRoot": null}});
        let (decision, _) = answered_in(&work, &format!("{unseen}\r\n"));
        assert_eq!(decision, "decline", "no item started");
    }

    /// What a `desk` run working in `directory`, without a gate, answers to `stream`.
    fn answered_at_desk(directory: &Path, stream: &str) -> Value {
        let (run, writes) = scripted_run(
            stream.as_bytes(),
            &directory.to_string_lossy(),
            None,
            restriction(false, None, Some(&BTreeMap::new()), false),
        );
        let _ = events_until_quiet(&run);
        written_decision(&writes)
            .expect("the file change was answered, not left waiting")
            .0
    }

    #[test]
    fn a_thread_held_to_its_plan_changes_no_file_and_one_at_desk_or_field_may() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let inside = file_change(&work.join("p.md").to_string_lossy(), Value::Null);
        let answered = |reads_only: bool| {
            let (run, writes) = scripted_run(
                inside.as_bytes(),
                &work.to_string_lossy(),
                None,
                restriction(false, None, Some(&BTreeMap::new()), reads_only),
            );
            let events = events_until_quiet(&run);
            (written_decision(&writes).map(|written| written.0), events)
        };
        let (decision, events) = answered(true);
        assert_ne!(decision, Some(json!("accept")), "{events:?}");
        assert!(
            events.iter().any(|event| event.kind == "failed"),
            "a patch ends a thread held to its plan: {events:?}"
        );
        assert_eq!(
            answered(false).0,
            Some(json!("accept")),
            "after the pass it may"
        );
        assert!(
            held_to_desk(ai6::READ_CLEARANCE, false, None),
            "and it never runs a command"
        );
    }

    #[test]
    fn this_handler_can_hold_a_thread_to_its_plan() {
        let handler = CodexHandler::try_from(CodexConfig::default()).unwrap();
        assert_eq!(handler.read_only_refusal().unwrap(), None);
    }

    #[test]
    fn a_desk_thread_without_a_gate_may_change_files_only_inside_its_directory() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let path = |path: &Path| path.to_string_lossy().into_owned();
        assert_eq!(
            answered_at_desk(&work, &file_change(&path(&work.join("p.md")), Value::Null)),
            "accept"
        );
        for (why, stream) in [
            (
                "outside",
                file_change(&path(&root.path().join("outside.md")), Value::Null),
            ),
            (
                "climbing out",
                file_change(&format!("{}/../outside.md", path(&work)), Value::Null),
            ),
            (
                "a wider root",
                file_change(&path(&work.join("p.md")), json!(path(root.path()))),
            ),
        ] {
            assert_eq!(answered_at_desk(&work, &stream), "decline", "{why}");
        }
    }

    /// The settings a start, then a resume, at `clearance` hand the app server, and the policy each
    /// thread was started with.
    fn launched_at(
        clearance: &str,
        executable: &str,
        environment: BTreeMap<String, String>,
    ) -> Launched {
        let mut environment = environment;
        environment.insert(
            "CODEX_FIXTURE_THREAD_ID".to_owned(),
            "codex-clearance-thread".to_owned(),
        );
        environment.insert("CODEX_FIXTURE_SHOW_POLICY".to_owned(), "1".to_owned());
        let terminal = ProcessTerminal::new();
        let handler = CodexHandler::try_from(CodexConfig {
            executable: executable.to_owned(),
            ..CodexConfig::default()
        })
        .unwrap();
        let cwd = std::env::current_dir()
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
                    working_directory: cwd.clone(),
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
        let first = read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    mission_id: "mission-clearance".parse().unwrap(),
                    run_id: RunId::try_new("codex-clearance-thread").unwrap(),
                    message: Some("more".to_owned()),
                    working_directory: cwd,
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
        let second = read_to_completion(&resumed);
        resumed.close().unwrap();
        let policy = |events: &[AgentEvent]| -> Value {
            let text = events
                .iter()
                .find_map(|event| event.text.as_deref()?.strip_prefix("policy: "))
                .expect("the thread's policy was reported");
            serde_json::from_str(text).unwrap()
        };
        let policies = [policy(&first), policy(&second)];
        let specs = terminal.specs.lock().unwrap();
        let servers: Vec<_> = specs
            .iter()
            .filter(|spec| spec.argv[1] == "app-server")
            .map(|spec| spec.argv[2..].to_vec())
            .collect();
        let rendered = specs
            .iter()
            .filter(|spec| spec.argv.len() > 2 && spec.argv[1..3] == ["debug", "prompt-input"])
            .count();
        assert_eq!(servers.len(), 2);
        Launched {
            servers,
            policies,
            rendered,
        }
    }

    struct Launched {
        servers: Vec<Vec<String>>,
        policies: [Value; 2],
        /// How many times `codex debug prompt-input` ran.
        rendered: usize,
    }

    fn cable() -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "AI6_CABLE_MCP_URL".to_owned(),
                "http://127.0.0.1:1/mcp".to_owned(),
            ),
            ("AI6_CABLE_MCP_TOKEN".to_owned(), "token".to_owned()),
        ])
    }

    /// The `-c` values of a list of arguments, with the working directory's `projects` entry left
    /// out, since it holds a path.
    fn settings(arguments: &[String]) -> Vec<&str> {
        arguments
            .iter()
            .filter(|argument| *argument != "-c" && !argument.starts_with("projects="))
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn a_threads_settings_follow_its_clearance_on_start_and_resume() {
        let station = [
            "mcp_servers.ai6-cables.url=\"http://127.0.0.1:1/mcp\"",
            "mcp_servers.ai6-cables.bearer_token_env_var=\"AI6_CABLE_MCP_TOKEN\"",
            "mcp_servers.ai6-cables.default_tools_approval_mode=\"approve\"",
        ];
        for (index, clearance) in ["field", "00"].iter().enumerate() {
            let launched = launched_at(
                clearance,
                &format!("codex-field-clearance-fixture-{index}"),
                cable(),
            );
            assert_eq!(launched.rendered, 0, "{clearance}: no tool check");
            for server in &launched.servers {
                assert_eq!(settings(server), station, "{clearance}");
            }
            for policy in &launched.policies {
                assert_eq!(policy["sandbox"], Value::Null, "{clearance}");
                assert_eq!(policy["approvalPolicy"], "untrusted", "{clearance}");
            }
        }
        // A value the handler does not know, an empty one included, gets the least it can give.
        // The fake lists `code_mode_host`, kept, and `tool_suggest`, switched off.
        let desk: Vec<&str> = station
            .iter()
            .copied()
            .chain(NO_BUILT_IN_TOOLS)
            .chain(["features.tool_suggest=false"])
            .collect();
        for (index, clearance) in ["desk", "developer", ""].iter().enumerate() {
            let launched = launched_at(
                clearance,
                &format!("codex-desk-clearance-fixture-{index}"),
                cable(),
            );
            assert_eq!(launched.rendered, 2, "{clearance:?}: checked each time");
            for server in &launched.servers {
                assert_eq!(settings(server), desk, "{clearance:?}");
            }
            for policy in &launched.policies {
                assert_eq!(policy["sandbox"], "read-only", "{clearance:?}");
                assert_eq!(policy["approvalPolicy"], "untrusted", "{clearance:?}");
            }
        }
    }

    #[test]
    fn the_theater_server_waits_as_long_as_the_station_says_a_call_may_take() {
        let mut environment = cable();
        environment.extend([
            (
                "AI6_THEATER_MCP_URL".to_owned(),
                "http://127.0.0.1:2/mcp".to_owned(),
            ),
            ("AI6_THEATER_MCP_TOKEN".to_owned(), "token".to_owned()),
        ]);
        let without = app_server_argv("codex", &environment);
        assert!(
            !without
                .iter()
                .any(|argument| argument.contains("tool_timeout_sec")),
            "no timeout named, none set"
        );
        environment.insert(
            "AI6_THEATER_MCP_TIMEOUT_MS".to_owned(),
            "3630001".to_owned(),
        );
        let argv = app_server_argv("codex", &environment);
        let timeouts: Vec<&String> = argv
            .iter()
            .filter(|argument| argument.contains("tool_timeout_sec"))
            .collect();
        assert_eq!(timeouts, ["mcp_servers.ai6-theater.tool_timeout_sec=3631"]);
        let cables_only = app_server_argv("codex", &cable());
        assert!(
            !cables_only
                .iter()
                .any(|argument| argument.contains("tool_timeout_sec"))
        );
    }

    #[test]
    fn a_desk_thread_on_a_remote_theater_has_neither_theater_run_nor_theater_write() {
        let mut environment = cable();
        environment.extend([
            (
                "AI6_THEATER_MCP_URL".to_owned(),
                "http://127.0.0.1:2/mcp".to_owned(),
            ),
            ("AI6_THEATER_MCP_TOKEN".to_owned(), "token".to_owned()),
        ]);
        let launched = launched_at("desk", "codex-desk-remote-fixture", environment.clone());
        for server in &launched.servers {
            assert!(settings(server).contains(&THEATER_TOOLS_OFF), "{server:?}");
        }
        for policy in &launched.policies {
            assert_eq!(policy["sandbox"], "read-only");
            assert_eq!(
                policy["approvalPolicy"], "never",
                "nothing lands on the Station"
            );
        }
        let launched = launched_at("field", "codex-field-remote-fixture", environment);
        for server in &launched.servers {
            assert!(!settings(server).contains(&THEATER_TOOLS_OFF), "{server:?}");
        }
    }

    /// `codex debug prompt-input` of Codex 0.164.0 for a `desk` thread. `SCRIPTS_PROMPT` stands for
    /// a thread whose `exec` still nests the command tools.
    const DESK_PROMPT: &str =
        include_str!("../tests/fixtures/codex-0.164.0/prompt-input-desk.json");

    fn desk_start(
        executable: &str,
        prompt: &str,
        extra: &[(&str, &str)],
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        desk_start_on(ProcessTerminal::new(), executable, prompt, extra)
    }

    fn desk_start_on(
        terminal: ProcessTerminal,
        executable: &str,
        prompt: &str,
        extra: &[(&str, &str)],
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let mut environment = cable();
        environment.insert(
            "CODEX_FIXTURE_THREAD_ID".to_owned(),
            "codex-desk-thread".to_owned(),
        );
        environment.extend(
            extra
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
        );
        CodexHandler::try_from(CodexConfig {
            executable: executable.to_owned(),
            ..CodexConfig::default()
        })
        .unwrap()
        .start(
            AgentStart {
                mission_id: "mission-desk".parse().unwrap(),
                agent_id: "007".to_owned(),
                prompt: prompt.to_owned(),
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
                answer_only: false,
                assistant: None,
                overlays: Default::default(),
            },
            terminal.into_object(),
        )
    }

    /// `codex mcp list --json` prints each server's environment values: only the app server, the
    /// Run itself, may run where its viewers read it.
    #[test]
    fn the_tool_check_runs_out_of_the_viewers_sight_and_the_app_server_in_it() {
        let terminal = ProcessTerminal::new();
        let desk = DESK_PROMPT.replace('\n', "");
        let run = desk_start_on(
            terminal.clone(),
            "codex-private-probe-fixture",
            "work",
            &[("CODEX_FIXTURE_PROMPT_INPUT", &desk)],
        )
        .unwrap();
        read_to_completion(&run);
        run.close().unwrap();
        let private = terminal.private.lock().unwrap();
        for command in ["--version", "features", "mcp", "debug"] {
            assert!(private.iter().any(|word| word == command), "{private:?}");
        }
        assert!(
            !private.iter().any(|word| word == "app-server"),
            "{private:?}"
        );
        assert!(
            terminal
                .specs
                .lock()
                .unwrap()
                .iter()
                .any(|spec| spec.argv[1] == "app-server")
        );
    }

    #[test]
    fn a_desk_thread_starts_only_if_codex_gives_it_no_command_tool() {
        let desk = DESK_PROMPT.replace('\n', "");
        let run = desk_start(
            "codex-desk-offered-fixture",
            "work",
            &[("CODEX_FIXTURE_PROMPT_INPUT", &desk)],
        )
        .unwrap();
        let events = read_to_completion(&run);
        assert!(
            !events.iter().any(|event| event.kind == "weakened"),
            "{events:?}"
        );
        run.close().unwrap();

        let nests_commands = SCRIPTS_PROMPT.replace('\n', "");
        let refused = match desk_start(
            "codex-desk-commands-fixture",
            "work",
            &[("CODEX_FIXTURE_PROMPT_INPUT", &nests_commands)],
        ) {
            Ok(_) => panic!("a desk thread with exec_command started"),
            Err(error) => error.to_string(),
        };
        assert!(
            refused.contains("exec_command") && refused.contains(TOOL_CHECK_ENV),
            "{refused}"
        );
        let run = desk_start(
            "codex-desk-commands-warned-fixture",
            "work",
            &[
                ("CODEX_FIXTURE_PROMPT_INPUT", &nests_commands),
                (TOOL_CHECK_ENV, "warn"),
            ],
        )
        .unwrap();
        let events = read_to_completion(&run);
        assert!(
            events.iter().any(|event| event.kind == "weakened"
                && event.text.as_deref().unwrap().contains("exec_command")),
            "{events:?}"
        );
        run.close().unwrap();
    }

    #[test]
    fn a_desk_thread_that_runs_a_command_anyway_is_ended() {
        for (executable, prompt) in [
            ("codex-desk-item-fixture", "item: commandExecution"),
            ("codex-desk-ask-fixture", "run: rm -rf /"),
        ] {
            let run = desk_start(executable, prompt, &[]).unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            let reason = loop {
                assert!(Instant::now() < deadline, "{prompt}: never ended");
                if let Some(event) = run.next_event(100).unwrap()
                    && event.kind == "failed"
                {
                    break event.text.unwrap();
                }
            };
            assert!(reason.contains("restricted Codex thread used"), "{reason}");
            run.close().unwrap();
        }
    }

    #[test]
    fn a_gated_run_asks_before_every_untrusted_command() {
        let cable = std::collections::BTreeMap::from([
            (
                "AI6_CABLE_MCP_URL".to_owned(),
                "http://127.0.0.1:1/mcp".to_owned(),
            ),
            ("AI6_CABLE_MCP_TOKEN".to_owned(), "token".to_owned()),
        ]);
        let mut params = json!({ "cwd": "/work" });
        set_approval_policy(&mut params, &cable, false, Scripts::None, false);
        assert_eq!(params["approvalPolicy"], "untrusted");

        let mut params = json!({ "cwd": "/work" });
        set_approval_policy(&mut params, &cable, true, Scripts::None, false);
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
        set_approval_policy(&mut params, &remote, false, Scripts::None, false);
        assert_eq!(
            params["approvalPolicy"], "never",
            "the Station gates theater_run"
        );

        let mut params = json!({ "cwd": "/work" });
        set_approval_policy(
            &mut params,
            &std::collections::BTreeMap::new(),
            false,
            Scripts::None,
            false,
        );
        assert!(
            params.get("approvalPolicy").is_none(),
            "no gate, Codex's own policy"
        );
    }

    /// How a scripted app server's output ends once everything scripted was read.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Ending {
        /// Never: the terminal keeps its output open.
        Open,
        /// The terminal closes its output.
        Eof,
        /// The program exits with this code and the terminal keeps its output open.
        Exited(i64),
    }

    /// A terminal whose app server answers by script: the n-th request written (a message with an
    /// id and a method) releases the n-th chunk of output, and the request `handshake` releases
    /// every chunk left, so a stream is read only once the driver is waiting for it.
    struct ScriptedTerminal {
        spec: Arc<Mutex<Option<Spec>>>,
        chunks: Vec<Vec<u8>>,
        handshake: usize,
        ending: Ending,
        writes: Arc<Mutex<Vec<u8>>>,
        killed: Arc<AtomicBool>,
    }

    impl ScriptedTerminal {
        fn new(chunks: Vec<Vec<u8>>, handshake: usize) -> Self {
            Self {
                spec: Arc::new(Mutex::new(None)),
                chunks,
                handshake,
                ending: Ending::Open,
                writes: Arc::new(Mutex::new(Vec::new())),
                killed: Arc::new(AtomicBool::new(false)),
            }
        }

        fn ending(mut self, ending: Ending) -> Self {
            self.ending = ending;
            self
        }
    }

    impl AgentTerminal for ScriptedTerminal {
        fn spawn_private(
            &self,
            value: &ai6::guatiao::Value,
        ) -> Result<Object<dyn TerminalRun>, ProviderError> {
            self.spawn(value)
        }

        fn spawn(
            &self,
            value: &ai6::guatiao::Value,
        ) -> Result<Object<dyn TerminalRun>, ProviderError> {
            *self.spec.lock().unwrap() = Some(Spec::from_value(value).map_err(internal)?);
            Ok(ScriptedRun {
                script: Mutex::new(Script {
                    output: VecDeque::new(),
                    chunks: self.chunks.iter().cloned().collect(),
                    requests: 0,
                }),
                arrived: Condvar::new(),
                handshake: self.handshake,
                ending: self.ending,
                writes: self.writes.clone(),
                killed: self.killed.clone(),
            }
            .into_object())
        }
    }

    struct Script {
        output: VecDeque<u8>,
        chunks: VecDeque<Vec<u8>>,
        requests: usize,
    }

    struct ScriptedRun {
        script: Mutex<Script>,
        arrived: Condvar,
        handshake: usize,
        ending: Ending,
        writes: Arc<Mutex<Vec<u8>>>,
        killed: Arc<AtomicBool>,
    }

    impl TerminalRun for ScriptedRun {
        fn pid(&self) -> i64 {
            8
        }

        fn read(&self, dst: &mut [u8], timeout_ms: i64) -> Result<i64, ProviderError> {
            let mut script = self.script.lock().unwrap();
            if script.output.is_empty() {
                script = self
                    .arrived
                    .wait_timeout(script, Duration::from_millis(timeout_ms.max(0) as u64))
                    .unwrap()
                    .0;
            }
            if script.output.is_empty() {
                let finished = script.chunks.is_empty() && script.requests > 0;
                return Ok(if self.ending == Ending::Eof && finished {
                    0
                } else {
                    READ_TIMED_OUT
                });
            }
            let count = dst.len().min(script.output.len());
            for byte in &mut dst[..count] {
                *byte = script.output.pop_front().unwrap();
            }
            Ok(count as i64)
        }

        fn write(&self, data: &[u8]) -> Result<(), ProviderError> {
            self.writes.lock().unwrap().extend_from_slice(data);
            for line in data.split(|byte| *byte == b'\n') {
                let Ok(message) = serde_json::from_slice::<Value>(line) else {
                    continue;
                };
                if message.get("id").is_none() || message.get("method").is_none() {
                    continue;
                }
                let mut script = self.script.lock().unwrap();
                script.requests += 1;
                let released = if script.requests >= self.handshake {
                    script.chunks.drain(..).collect::<Vec<_>>()
                } else {
                    script.chunks.pop_front().into_iter().collect()
                };
                for chunk in released {
                    script.output.extend(chunk);
                }
                self.arrived.notify_all();
            }
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
            let script = self.script.lock().unwrap();
            match self.ending {
                Ending::Exited(code)
                    if script.requests > 0
                        && script.chunks.is_empty()
                        && script.output.is_empty() =>
                {
                    ExitInfo {
                        running: false,
                        code: Some(code),
                        signal: None,
                    }
                    .to_value()
                    .map_err(internal)
                }
                Ending::Exited(_) => ExitInfo {
                    running: true,
                    code: None,
                    signal: None,
                }
                .to_value()
                .map_err(internal),
                _ => Err(refuse(Status::GUATIAO_ERR_INTERNAL, "unused in this test")),
            }
        }
    }

    /// A run on a scripted app server that opens a thread, takes a turn, and then writes `stream`;
    /// what it wrote back is in the second value.
    fn scripted_run(
        stream: &[u8],
        directory: &str,
        gate: Option<Gate>,
        restriction: Option<Restriction>,
    ) -> (CodexRun, Arc<Mutex<Vec<u8>>>) {
        let mut turn = b"{\"id\":3,\"result\":{\"turn\":{\"id\":\"u\"}}}\n".to_vec();
        turn.extend_from_slice(stream);
        let terminal = ScriptedTerminal::new(
            vec![
                b"{\"id\":1,\"result\":{}}\n".to_vec(),
                b"{\"id\":2,\"result\":{\"thread\":{\"id\":\"t\"}}}\n".to_vec(),
                turn,
            ],
            3,
        );
        let writes = terminal.writes.clone();
        let config = CodexConfig::default();
        let run = open(Opening {
            config: &config,
            terminal: terminal.into_object(),
            cwd: directory,
            environment: &BTreeMap::new(),
            mode: Mode {
                config: &config,
                project_settings: true,
                answer_only: false,
                desk: false,
                assistant: None,
                model: None,
                profile: None,
            },
            thread: ThreadStart::New,
            model: None,
            effort: None,
            instructions: None,
            gate,
            restriction,
            profile: None,
        })
        .unwrap();
        run.send_message(AgentMessage {
            text: "go".to_owned(),
        })
        .unwrap();
        (run, writes)
    }

    /// Every event the run tells until it has been quiet for a moment.
    fn events_until_quiet(run: &CodexRun) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Ok(Some(event)) = run.next_event(400) {
            events.push(event);
        }
        events
    }

    /// Opens a thread on `terminal` with the settings of an ungated Mission's.
    fn open_on(
        terminal: ScriptedTerminal,
        config: &CodexConfig,
    ) -> Result<CodexRun, ProviderError> {
        open(Opening {
            config,
            terminal: terminal.into_object(),
            cwd: "/work",
            environment: &BTreeMap::new(),
            mode: Mode {
                config,
                project_settings: true,
                answer_only: false,
                desk: false,
                assistant: None,
                model: None,
                profile: None,
            },
            thread: ThreadStart::New,
            model: None,
            effort: None,
            instructions: None,
            gate: None,
            restriction: None,
            profile: None,
        })
    }

    /// The decision the app server was sent, once it has been.
    fn written_decision(writes: &Arc<Mutex<Vec<u8>>>) -> Option<(Value, String)> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let written = String::from_utf8(writes.lock().unwrap().clone()).unwrap();
            if let Some(decision) = written
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find_map(|message| message.pointer("/result/decision").cloned())
            {
                // What follows a decision (the steer) is written in the same moment.
                std::thread::sleep(Duration::from_millis(100));
                let written = String::from_utf8(writes.lock().unwrap().clone()).unwrap();
                return Some((decision, written));
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn resume_reattaches_and_submits_a_turn_to_the_same_provider_thread() {
        let run_id = RunId::try_new("codex-thread-42").unwrap();
        let terminal = ScriptedTerminal::new(
            vec![
                b"{\"id\":1,\"result\":{}}\n".to_vec(),
                b"{\"id\":2,\"result\":{\"thread\":{\"id\":\"codex-thread-42\"}}}\n".to_vec(),
                b"{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-1\"}}}\n".to_vec(),
            ],
            3,
        );
        let (spec, writes, killed) = (
            terminal.spec.clone(),
            terminal.writes.clone(),
            terminal.killed.clone(),
        );
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-test".to_owned(),
            ..CodexConfig::default()
        })
        .unwrap();
        let run = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
                    mission_id: "mission-42".parse().unwrap(),
                    run_id: run_id.clone(),
                    message: Some("continue the same Run".to_owned()),
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
                    overlays: Default::default(),
                },
                terminal.into_object(),
            )
            .unwrap();

        assert_eq!(run.session_id().unwrap(), Some(run_id.to_string()));

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
            "continue the same Run"
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
        let input =
            b"\x1b[?9001h\x1b[?1004h\x1b[m\x1b]0;C:\\bin\\codex.exe\x07\x1b[?25h\r\n".to_vec();
        let mut first = input;
        let mut second =
            b"\x1b[5;1H{\"id\":2,\"result\":{\"thread\":{\"id\":\"codex-thread-42\"}}}\r\n"
                .to_vec();
        second.extend_from_slice(head.as_bytes());
        second.extend_from_slice(b"\r\n\x1b[23;2000H");
        second.extend_from_slice(tail.as_bytes());
        second.extend_from_slice(b"\r\n");
        first.extend_from_slice(b"{\"id\":1,\"result\":{}}\r\n\x1b[?25l");
        let terminal = ScriptedTerminal::new(vec![first, second], 2);
        let spec = terminal.spec.clone();
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-test".to_owned(),
            ..CodexConfig::default()
        })
        .unwrap();
        let run = handler
            .resume(
                AgentResume {
                    clearance: "field".to_owned(),
                    mission_id: "mission-42".parse().unwrap(),
                    run_id: RunId::try_new("codex-thread-42").unwrap(),
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
        let event = run.next_event(50).unwrap().unwrap();
        assert_eq!(
            (event.kind.as_str(), event.text.as_deref()),
            ("text", Some(&*delta))
        );
        assert_eq!(spec.lock().unwrap().as_ref().unwrap().cols, STREAM_COLS);
    }

    #[test]
    fn resume_rejects_a_different_thread_and_closes_the_provider_process() {
        let terminal = ScriptedTerminal::new(
            vec![
                b"{\"id\":1,\"result\":{}}\n".to_vec(),
                b"{\"id\":2,\"result\":{\"thread\":{\"id\":\"different-thread\"}}}\n".to_vec(),
            ],
            2,
        );
        let killed = terminal.killed.clone();
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-test".to_owned(),
            ..CodexConfig::default()
        })
        .unwrap();

        let result = handler.resume(
            AgentResume {
                clearance: "field".to_owned(),
                mission_id: "mission-42".parse().unwrap(),
                run_id: RunId::try_new("codex-thread-42").unwrap(),
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
        assert_eq!(hint.bucket.as_deref(), Some("five_hour"));

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
        assert_eq!(limit.bucket.as_deref(), Some("five_hour"));
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
        let quota = event.report.unwrap().buckets;
        assert_eq!(
            quota
                .iter()
                .map(|bucket| (
                    bucket.id.as_str(),
                    UsageReport::fraction_permille(bucket),
                    bucket.refill.clone()
                ))
                .collect::<Vec<_>>(),
            [
                (
                    "five_hour",
                    Some(425),
                    Refill::FixedWindow {
                        length: Some(std::time::Duration::from_secs(300 * 60)),
                        resets_at: Some(1_790_000_000)
                    }
                ),
                (
                    "weekly",
                    Some(400),
                    Refill::FixedWindow {
                        length: Some(std::time::Duration::from_secs(10_080 * 60)),
                        resets_at: Some(1_790_500_000)
                    }
                ),
            ]
        );
        let partial = json!({
            "method": "account/rateLimits/updated",
            "params": { "rateLimits": { "primary": { "usedPercent": 99.0, "resetsAt": 1 } } }
        });
        let quota = decode_event(&partial, None)
            .unwrap()
            .report
            .unwrap()
            .buckets;
        assert_eq!(quota.len(), 1);
        assert_eq!(
            quota[0].id, "primary",
            "an unnamed window keeps its position"
        );
        assert_eq!(UsageReport::fraction_permille(&quota[0]), Some(990));
        let other = json!({ "method": "turn/completed", "params": {} });
        assert!(decode_event(&other, None).unwrap().report.is_none());
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
    fn a_token_cap_refusal_that_will_retry_fails_the_turn_with_the_gateways_words() {
        let words = "400 Request rejected: Current cap is 300000 tokens";
        let capped = json!({
            "method": "error",
            "params": {
                "error": { "message": words, "codexErrorInfo": "badRequest" },
                "willRetry": true
            }
        });
        let event = decode_event(&capped, None).unwrap();
        assert_eq!(event.kind, "failed");
        assert_eq!(event.text.as_deref(), Some(words));
        let transient = json!({
            "method": "error",
            "params": {
                "error": { "message": "stream disconnected", "codexErrorInfo": "other" },
                "willRetry": true
            }
        });
        assert_eq!(decode_event(&transient, None).unwrap().kind, "error");
    }

    #[test]
    fn turn_completed_fixture_maps_to_completed() {
        let event =
            decode_event(&json!({ "method": "turn/completed", "params": {} }), None).unwrap();
        assert_eq!(event.kind, "completed");
    }

    #[test]
    fn terminal_eof_is_a_provider_failure_not_an_idle_timeout() {
        let config = CodexConfig::default();
        let terminal = ScriptedTerminal::new(Vec::new(), 1).ending(Ending::Eof);
        let error = open_on(terminal, &config)
            .err()
            .expect("an app server with no output is not a thread")
            .to_string();

        assert!(error.contains("closed its terminal"), "{error}");
    }

    #[test]
    fn an_app_server_that_exits_fails_the_run_with_its_status_and_last_output() {
        // Behind a terminal that keeps its output open (ConPTY): it exits after its thread opened.
        let terminal = ScriptedTerminal::new(
            vec![
                b"{\"id\":1,\"result\":{}}\n".to_vec(),
                b"{\"id\":2,\"result\":{\"thread\":{\"id\":\"t\"}}}\nerror: unknown model \"nope\"\r\n"
                    .to_vec(),
            ],
            2,
        )
        .ending(Ending::Exited(2));
        let config = CodexConfig::default();
        let run = open_on(terminal, &config).unwrap();
        let output = run.next_event(2_000).unwrap().unwrap();
        assert_eq!(output.kind, "output");
        let failed = run.next_event(2_000).unwrap().unwrap();
        assert_eq!(failed.kind, "failed");
        let reason = failed.text.unwrap();
        assert!(reason.contains("exited with code 2"), "{reason}");
        assert!(reason.contains("unknown model"), "{reason}");
        assert_eq!(run.next_event(100).unwrap(), None, "reported once");

        // And one that exits before its thread opened is why the start failed.
        let terminal =
            ScriptedTerminal::new(vec![b"error: unknown model \"nope\"\r\n".to_vec()], 1)
                .ending(Ending::Exited(2));
        let error = open_on(terminal, &config)
            .err()
            .expect("no thread opened")
            .to_string();
        assert!(error.contains("exited with code 2"), "{error}");
        assert!(error.contains("unknown model"), "{error}");
    }

    #[test]
    fn the_configured_tail_bounds_what_a_failed_run_quotes() {
        let config = CodexConfig {
            tail_lines: 2,
            tail_line_bytes: 4,
            ..CodexConfig::default()
        };
        let terminal =
            ScriptedTerminal::new(vec![b"one one\r\ntwo two\r\nthree three\r\n".to_vec()], 1)
                .ending(Ending::Exited(1));
        let reason = open_on(terminal, &config)
            .err()
            .expect("no thread opened")
            .to_string();
        let reason = reason.trim_end_matches(" (GUATIAO_ERR_INTERNAL)");
        assert!(reason.ends_with("its last output:\ntwo \nthre"), "{reason}");
    }

    #[test]
    fn the_configured_size_bounds_an_assistants_context_file() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("context.md");
        std::fs::write(&file, "twelve bytes").unwrap();
        let mut mode = AssistantMode::moneypenny();
        mode.context_file = Some(file.to_string_lossy().into_owned());

        let error = assistant_instructions(&mode, Scripts::None, 11).unwrap_err();
        assert!(error.to_string().contains("more than 11"), "{error}");
        let told = assistant_instructions(&mode, Scripts::None, 12).unwrap();
        assert!(told.iter().any(|part| part == "twelve bytes"), "{told:?}");
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
        assert!(argv.iter().any(|argument| {
            argument == "mcp_servers.ai6-theater.default_tools_approval_mode=\"approve\""
        }));
        assert!(
            argv.iter()
                .all(|argument| !argument.contains("theater-secret-token"))
        );

        assert!(
            argv.iter()
                .any(|argument| argument == "features.shell_tool=false"),
            "on Linux its read-only sandbox lets a command read any file of the Station: {argv:?}"
        );
        assert!(
            !app_server_argv("codex", &std::collections::BTreeMap::new())
                .iter()
                .any(|argument| argument == "features.shell_tool=false"),
            "a field Run on the Station keeps its commands"
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
        let run_id = RunId::try_new("codex-assistant-thread").unwrap();
        let environment = BTreeMap::from([
            ("CODEX_FIXTURE_THREAD_ID".to_owned(), run_id.to_string()),
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
            ..CodexConfig::default()
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
                    overlays: Default::default(),
                },
                terminal.clone().into_object(),
            )
            .unwrap();
        let started_events = read_to_completion(&started);
        started.close().unwrap();
        let resumed = handler
            .resume(
                AgentResume {
                    clearance: "desk".to_owned(),
                    mission_id: "assistant-conversation".parse().unwrap(),
                    run_id,
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
                    overlays: Default::default(),
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
                 bearer_token_env_var=\"AI6_MONEYPENNY_MCP_TOKEN\",\
                 default_tools_approval_mode=\"approve\"}}"
                    .to_owned(),
            ],
            no_built_in_tools(false),
            vec!["-c".to_owned(), "project_doc_max_bytes=0".to_owned()],
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

    /// `codex debug prompt-input` of Codex 0.164.0 for an assistant with scripts.
    const SCRIPTS_PROMPT: &str =
        include_str!("../tests/fixtures/codex-0.164.0/prompt-input-scripts.json");

    /// An assistant Run's environment and start request, with `mode`.
    fn assistant_start(mode: AssistantMode, prompt: &str) -> AgentStart {
        let environment = BTreeMap::from([
            (
                "CODEX_FIXTURE_THREAD_ID".to_owned(),
                "codex-scripts-thread".to_owned(),
            ),
            ("CODEX_FIXTURE_SHOW_POLICY".to_owned(), "1".to_owned()),
            (
                "CODEX_FIXTURE_PROMPT_INPUT".to_owned(),
                SCRIPTS_PROMPT.replace('\n', ""),
            ),
            (
                "AI6_MONEYPENNY_MCP_URL".to_owned(),
                "https://exports.example/v1/moneypenny/mcp".to_owned(),
            ),
            (
                "AI6_MONEYPENNY_MCP_TOKEN".to_owned(),
                "conversation-token".to_owned(),
            ),
        ]);
        AgentStart {
            mission_id: "assistant--scripts".parse().unwrap(),
            agent_id: "moneypenny".to_owned(),
            prompt: prompt.to_owned(),
            initial_context: vec!["You are Moneypenny.".to_owned()],
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
            answer_only: false,
            assistant: Some(mode),
            overlays: Default::default(),
        }
    }

    fn reported_policy(events: &[AgentEvent]) -> Value {
        let policy = events
            .iter()
            .find_map(|event| event.text.as_deref()?.strip_prefix("policy: "))
            .expect("the thread's policy was reported");
        serde_json::from_str(policy).unwrap()
    }

    #[test]
    fn an_assistant_with_scripts_runs_them_in_the_sandbox_profile_as_a_weakening() {
        let context =
            std::env::temp_dir().join(format!("ai6-codex-context-{}.md", std::process::id()));
        std::fs::write(&context, "CONTEXT-FILE-MARKER: her memory\n").unwrap();
        let mut mode = AssistantMode::moneypenny();
        mode.interpreters.push("python3".to_owned());
        mode.files = vec!["personal".to_owned(), "scratch".to_owned()];
        mode.context_file = Some(context.to_string_lossy().into_owned());
        mode.network.allowed_hosts.push("example.com".to_owned());
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-scripts-fixture".to_owned(),
            ..CodexConfig::default()
        })
        .unwrap();
        let terminal = ProcessTerminal::new();
        let start = assistant_start(mode.clone(), "Chart it.");
        let run = handler
            .start(start.clone(), terminal.clone().into_object())
            .unwrap();
        let events = read_to_completion(&run);
        run.close().unwrap();
        let _ = std::fs::remove_file(&context);
        assert_eq!(events[0].kind, "weakened", "{events:?}");
        let weakening = events[0].text.clone().unwrap();
        assert!(
            weakening.contains("functions.exec and functions.wait")
                && weakening.contains("inside Codex's sandbox, not only python3"),
            "{weakening}"
        );
        let policy = reported_policy(&events);
        assert_eq!(
            policy["sandbox"],
            Value::Null,
            "the profile decides: {policy}"
        );
        assert_eq!(policy["approvalPolicy"], "never");
        let instructions = policy["developerInstructions"].as_str().unwrap();
        assert!(
            instructions.starts_with("You are Moneypenny.")
                && instructions.contains("CONTEXT-FILE-MARKER: her memory")
                && instructions.contains("no network access"),
            "{instructions}"
        );

        let specs = terminal.specs.lock().unwrap();
        let server = specs
            .iter()
            .find(|spec| spec.argv[1] == "app-server")
            .unwrap();
        let has = |setting: &str| server.argv.iter().any(|argument| argument == setting);
        let starts = |prefix: &str| {
            server
                .argv
                .iter()
                .any(|argument| argument.starts_with(prefix))
        };
        assert!(
            starts("permissions.ai6_assistant={filesystem={")
                && has("default_permissions=\"ai6_assistant\"")
                && starts("shell_environment_policy.include_only=[")
                && has("project_doc_max_bytes=0"),
            "{:?}",
            server.argv
        );
        assert_eq!(has("windows.sandbox=\"mxc\""), cfg!(windows));
        assert!(
            !has("features.shell_tool=false") && !has("features.code_mode_host=false"),
            "exec and its command tools stay on: {:?}",
            server.argv
        );
        assert!(has("features.tool_suggest=false"), "other features go");
        let checked = specs
            .iter()
            .find(|spec| spec.argv.len() > 2 && spec.argv[1..3] == ["debug", "prompt-input"])
            .unwrap();
        assert!(
            !checked
                .argv
                .iter()
                .any(|a| a == "features.shell_tool=false")
        );
        drop(specs);

        let resumed = handler
            .resume(
                AgentResume {
                    clearance: "desk".to_owned(),
                    mission_id: start.mission_id.clone(),
                    run_id: RunId::try_new("codex-scripts-thread").unwrap(),
                    message: Some("Again.".to_owned()),
                    working_directory: start.working_directory.clone(),
                    environment: start.environment.clone(),
                    model: start.model.clone(),
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
        let events = read_to_completion(&resumed);
        resumed.close().unwrap();
        let policy = reported_policy(&events);
        assert_eq!(policy["sandbox"], Value::Null);
        assert_eq!(policy["approvalPolicy"], "never");
    }

    #[test]
    fn a_users_codex_configuration_that_widens_the_scripts_sandbox_refuses_the_thread() {
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-widened-fixture".to_owned(),
            ..CodexConfig::default()
        })
        .unwrap();
        let mut mode = AssistantMode::moneypenny();
        mode.interpreters.push("python3".to_owned());
        mode.files = vec!["personal".to_owned()];
        for user in [
            json!({"permissions": {"ai6_assistant": {"network": {"enabled": true}}}}),
            json!({"shell_environment_policy": {"set": {"TMPDIR": "/elsewhere"}}}),
        ] {
            let mut start = assistant_start(mode.clone(), "Chart it.");
            start
                .environment
                .insert("CODEX_FIXTURE_USER_CONFIG".to_owned(), user.to_string());
            let terminal = ProcessTerminal::new();
            let refused = handler
                .start(start.clone(), terminal.clone().into_object())
                .err()
                .unwrap_or_else(|| panic!("started with {user}"));
            assert!(
                refused.to_string().contains("changes the sandbox"),
                "{refused}"
            );
            let refused = handler
                .resume(
                    AgentResume {
                        clearance: "desk".to_owned(),
                        mission_id: start.mission_id.clone(),
                        run_id: RunId::try_new("codex-scripts-thread").unwrap(),
                        message: None,
                        working_directory: start.working_directory.clone(),
                        environment: start.environment.clone(),
                        model: start.model.clone(),
                        effort: None,
                        max_cost_micro_usd: None,
                        project_settings: false,
                        project_mcp: false,
                        answer_only: false,
                        assistant: Some(mode.clone()),
                        initial_context: Vec::new(),
                        overlays: Default::default(),
                    },
                    terminal.into_object(),
                )
                .err()
                .unwrap_or_else(|| panic!("resumed with {user}"));
            assert!(
                refused.to_string().contains("changes the sandbox"),
                "{refused}"
            );
        }
    }

    #[test]
    fn scripted_commands_run_and_are_reported_as_a_tool() {
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-commands-fixture".to_owned(),
            ..CodexConfig::default()
        })
        .unwrap();
        for unsandboxed in [false, true] {
            let mut mode = AssistantMode::moneypenny();
            mode.interpreters.push("python3".to_owned());
            mode.files = vec!["personal".to_owned()];
            mode.unsandboxed = unsandboxed;
            let terminal = ProcessTerminal::new();
            let run = handler
                .start(
                    assistant_start(mode, "item: commandExecution"),
                    terminal.clone().into_object(),
                )
                .unwrap();
            let events = until_end(&run);
            run.close().unwrap();
            assert_eq!(events.last().unwrap().kind, "completed", "{events:?}");
            let reported = events
                .iter()
                .any(|event| event.tools.iter().any(|tool| tool == "exec_command"));
            assert!(reported, "a script may read her files: {events:?}");
            let policy = reported_policy(&events);
            let specs = terminal.specs.lock().unwrap();
            let server = specs
                .iter()
                .find(|spec| spec.argv[1] == "app-server")
                .unwrap();
            let profiled = server
                .argv
                .iter()
                .any(|argument| argument.starts_with("permissions."));
            if unsandboxed {
                assert_eq!(policy["sandbox"], "danger-full-access");
                assert!(!profiled);
            } else {
                assert_eq!(policy["sandbox"], Value::Null);
                assert!(profiled);
            }
        }
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
            overlays: Default::default(),
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
            ..CodexConfig::default()
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
            ..CodexConfig::default()
        })
        .unwrap();
        let environment = BTreeMap::from([(
            "CODEX_FIXTURE_MCP_LIST".to_owned(),
            MCP_LIST.replace('\n', ""),
        )]);
        let run = handler
            .start(
                answer_only_start(environment),
                terminal.clone().into_object(),
            )
            .unwrap();
        run.close().unwrap();
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
                ..CodexConfig::default()
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
            let run = handler
                .start(
                    answer_only_start(environment),
                    ProcessTerminal::new().into_object(),
                )
                .unwrap();
            let first = run.next_event(1_000).unwrap().unwrap();
            assert_eq!(first.kind, "weakened");
            let text = first.text.unwrap();
            assert!(text.contains(found), "{text}");
            run.close().unwrap();
        }
    }

    #[test]
    fn an_answer_only_thread_offered_extra_tools_is_not_started() {
        let terminal = ProcessTerminal::new();
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-strict-fixture".to_owned(),
            ..CodexConfig::default()
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
            ..CodexConfig::default()
        })
        .unwrap();
        let environment = BTreeMap::from([
            (
                "CODEX_FIXTURE_PROMPT_INPUT".to_owned(),
                OFFERS_EXEC.replace('\n', ""),
            ),
            (TOOL_CHECK_ENV.to_owned(), "warn".to_owned()),
        ]);
        let run = handler
            .start(
                answer_only_start(environment),
                ProcessTerminal::new().into_object(),
            )
            .unwrap();
        let first = run.next_event(1_000).unwrap().unwrap();
        assert_eq!(first.kind, "weakened");
        let text = first.text.unwrap();
        assert!(
            text.contains("functions.exec") && text.contains("mcp__cua_repl.js"),
            "{text}"
        );
        run.close().unwrap();
    }

    #[test]
    fn an_answer_only_thread_with_no_tool_passes_and_keeps_its_plumbing() {
        let terminal = ProcessTerminal::new();
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-pass-fixture".to_owned(),
            ..CodexConfig::default()
        })
        .unwrap();
        let run = handler
            .start(
                answer_only_start(BTreeMap::new()),
                terminal.clone().into_object(),
            )
            .unwrap();
        run.close().unwrap();
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
            ..CodexConfig::default()
        })
        .unwrap();
        for home in ["C:\\agents\\one", "C:\\agents\\one", "C:\\agents\\two"] {
            let environment = BTreeMap::from([("CODEX_HOME".to_owned(), home.to_owned())]);
            let run = handler
                .start(
                    answer_only_start(environment),
                    terminal.clone().into_object(),
                )
                .unwrap();
            run.close().unwrap();
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
            ..CodexConfig::default()
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
        let run = handler
            .start(
                answer_only_start(environment),
                ProcessTerminal::new().into_object(),
            )
            .unwrap();
        let first = run.next_event(1_000).unwrap().unwrap();
        assert_eq!(first.kind, "weakened");
        assert!(first.text.unwrap().contains("not a feature"));
        run.close().unwrap();
    }

    /// Events until the Run ends (`completed` or `failed`), at most a minute.
    fn until_end(run: &Object<dyn AgentSession>) -> Vec<AgentEvent> {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            match run.next_event(200) {
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
            ..CodexConfig::default()
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
            let run = handler
                .start(start, ProcessTerminal::new().into_object())
                .unwrap();
            let events = until_end(&run);
            let last = events.last().expect("the Run ended");
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
        let nothing = Restriction::default();
        assert_eq!(
            forbidden_use(
                &json!({"method":"item/started","params":{"item":{
                    "type":"mcpToolCall","server":"moneypenny","tool":"board"}}}),
                &Restriction {
                    servers: vec!["moneypenny".to_owned()],
                    ..Restriction::default()
                }
            ),
            None
        );
        for kind in ["agentMessage", "reasoning", "userMessage"] {
            let message = json!({"method":"item/completed","params":{"item":{"type":kind}}});
            assert_eq!(forbidden_use(&message, &nothing), None, "{kind}");
        }
        let unknown = json!({"method":"item/started","params":{"item":{"type":"hologram"}}});
        assert!(
            forbidden_use(&unknown, &nothing).is_some(),
            "an unknown item is a tool"
        );
        let question = json!({"id": 4, "method": "item/tool/requestUserInput", "params": {}});
        assert!(forbidden_use(&question, &nothing).is_some());
        let refresh = json!({"id": 5, "method": "account/chatgptAuthTokens/refresh"});
        assert_eq!(
            forbidden_use(&refresh, &nothing),
            None,
            "plumbing is not a tool"
        );
    }

    #[test]
    fn scripts_admit_commands_and_only_with_directories_file_changes() {
        let item = |kind: &str| json!({"method":"item/started","params":{"item":{"type":kind}}});
        let mut mode = AssistantMode::moneypenny();
        mode.interpreters.push("python3".to_owned());
        let scripts = restriction(false, Some(&mode), None, false).unwrap();
        assert_eq!(forbidden_use(&item("commandExecution"), &scripts), None);
        assert!(
            forbidden_use(&item("fileChange"), &scripts).is_some(),
            "no directory to write"
        );
        assert!(forbidden_use(&item("webSearch"), &scripts).is_some());
        mode.files.push("personal".to_owned());
        let files = restriction(false, Some(&mode), None, false).unwrap();
        assert_eq!(forbidden_use(&item("fileChange"), &files), None);
        let approval = json!({"id": 3, "method": COMMAND_APPROVAL, "params": {}});
        assert!(
            forbidden_use(&approval, &files).is_some(),
            "a thread that never asks has nothing to approve"
        );
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
        skills.skills_dir = Some(
            std::env::temp_dir()
                .join(".claude")
                .to_string_lossy()
                .into_owned(),
        );
        let refused = assistant_argv("codex", &environment, &skills, None).unwrap_err();
        assert_eq!(refused.status, Status::GUATIAO_ERR_UNSUPPORTED);
        assert!(
            refused.to_string().contains("no list of allowed skills"),
            "{refused}"
        );
        assert!(
            assistant_argv(
                "codex",
                &BTreeMap::new(),
                &AssistantMode::moneypenny(),
                None
            )
            .is_err(),
            "no endpoint in the environment"
        );
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex".to_owned(),
            ..CodexConfig::default()
        })
        .unwrap();
        assert!(
            handler.assistant_skills().unwrap().is_some(),
            "a Station installs no skill for it"
        );
    }

    #[test]
    fn without_interpreters_files_and_a_network_add_no_tool_and_she_is_told() {
        let environment = BTreeMap::from([
            (
                "AI6_MONEYPENNY_MCP_URL".to_owned(),
                "https://exports.example/v1/moneypenny/mcp".to_owned(),
            ),
            ("AI6_MONEYPENNY_MCP_TOKEN".to_owned(), "token".to_owned()),
        ]);
        let mut mode = AssistantMode::moneypenny();
        mode.files.push("personal".to_owned());
        mode.network.policy = "open".to_owned();
        let argv = assistant_argv("codex", &environment, &mode, None).unwrap();
        assert_no_built_in_tools(&argv);
        assert!(
            !argv
                .iter()
                .any(|argument| argument.starts_with("permissions.")),
            "no sandbox profile without scripts: {argv:?}"
        );
        assert_eq!(
            restriction(false, Some(&mode), None, false),
            Some(Restriction {
                servers: vec!["moneypenny".to_owned()],
                commands: false,
                file_changes: false,
                asks_file_changes: false,
            })
        );
        let told = assistant_instructions(
            &mode,
            Scripts::None,
            CodexConfig::default().context_file_bytes,
        )
        .unwrap();
        assert!(
            told.iter().any(|line| line.contains("no file tools")),
            "{told:?}"
        );
        let mut params = json!({ "cwd": "/work" });
        set_approval_policy(&mut params, &environment, true, Scripts::None, false);
        assert_eq!(params["sandbox"], "read-only");
    }

    #[test]
    fn an_assistant_with_cable_grants_gets_both_servers() {
        let environment = BTreeMap::from([
            (
                "AI6_MONEYPENNY_MCP_URL".to_owned(),
                "https://exports.example/v1/moneypenny/mcp".to_owned(),
            ),
            ("AI6_MONEYPENNY_MCP_TOKEN".to_owned(), "token".to_owned()),
            (
                "AI6_ASSISTANT_CABLE_MCP_URL".to_owned(),
                "http://127.0.0.1:4100/mcp".to_owned(),
            ),
            (
                "AI6_ASSISTANT_CABLE_MCP_TOKEN".to_owned(),
                "cable-token".to_owned(),
            ),
        ]);
        let argv = assistant_argv(
            "codex",
            &environment,
            &AssistantMode::moneypenny().with_cables(),
            None,
        )
        .unwrap();
        let (key, value) = argv[3].split_once('=').unwrap();
        let parsed: toml::Table = toml::from_str(&format!("{key} = {value}")).unwrap();
        assert_eq!(
            parsed["mcp_servers"],
            toml::Value::Table(
                toml::from_str(
                    "moneypenny = { url = \"https://exports.example/v1/moneypenny/mcp\", \
                     bearer_token_env_var = \"AI6_MONEYPENNY_MCP_TOKEN\", \
                     default_tools_approval_mode = \"approve\" }\n\
                     cables = { url = \"http://127.0.0.1:4100/mcp\", \
                     bearer_token_env_var = \"AI6_ASSISTANT_CABLE_MCP_TOKEN\", \
                     default_tools_approval_mode = \"approve\" }"
                )
                .unwrap()
            )
        );
        assert!(!argv.iter().any(|a| a.contains("cable-token")));
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
                "run-secret-token".to_owned(),
            ),
        ]);

        let argv = app_server_argv("codex", &environment);
        assert!(argv.iter().any(|argument| {
            argument == "mcp_servers.ai6-cables.url=\"http://127.0.0.1:43210/mcp\""
        }));
        assert!(argv.iter().any(|argument| {
            argument == "mcp_servers.ai6-cables.bearer_token_env_var=\"AI6_CABLE_MCP_TOKEN\""
        }));
        assert!(argv.iter().any(|argument| {
            argument == "mcp_servers.ai6-cables.default_tools_approval_mode=\"approve\""
        }));
        assert!(
            argv.iter()
                .all(|argument| !argument.contains("run-secret-token"))
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
        let spend = event.report.unwrap().spend.unwrap();
        assert_eq!(
            (
                spend.input,
                spend.output,
                spend.cache_read,
                spend.cache_write
            ),
            (500, 300, 700, 0)
        );
        assert_eq!(spend.cost_micro_usd, None, "Codex reports no cost");
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
        assert!(decode_event(&delta, None).unwrap().report.is_none());
        let total_only = json!({
            "method": "thread/tokenUsage/updated",
            "params": { "tokenUsage": { "total": { "totalTokens": 1 } } }
        });
        assert!(decode_event(&total_only, None).unwrap().report.is_none());
    }

    /// Real Codex 0.164.0 listed skills of its home and of the project only: `skills.*` settings and
    /// `skills/list` with extra roots added none, so the routing line is the delivery.
    #[test]
    fn an_overlays_skill_is_a_routing_line_in_the_developer_instructions() {
        let handler = CodexHandler::try_from(CodexConfig {
            executable: "codex-overlay-fixture".to_owned(),
            ..CodexConfig::default()
        })
        .unwrap();
        let terminal = ProcessTerminal::new();
        let mut start = assistant_start(AssistantMode::moneypenny(), "Do it.");
        start.assistant = None;
        start.clearance = "field".to_owned();
        start.overlays.skills.push(ai6::OverlaySkill {
            name: "review".to_owned(),
            overlay: "tools".to_owned(),
            description: "Reviews a change.".to_owned(),
            path: "/store/tools/skills/review".to_owned(),
            native: true,
        });
        let run = handler.start(start, terminal.into_object()).unwrap();
        let events = read_to_completion(&run);
        let instructions = reported_policy(&events)["developerInstructions"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            instructions.starts_with("You are Moneypenny."),
            "{instructions}"
        );
        assert!(
            instructions.contains("Skill `review` (overlay `tools`)"),
            "{instructions}"
        );
        assert!(
            instructions.contains("/store/tools/skills/review/SKILL.md"),
            "{instructions}"
        );
        run.close().unwrap();
    }
}
