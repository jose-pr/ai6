//! The Antigravity CLI's stream-json implementation of AI6's AgentHandler contract.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

ai6::guatiao::providers!(AntigravityHandler);

mod files;
mod session;

use std::collections::BTreeMap;
use std::path::Path;

use ai6::AgentSession;
use ai6::HarnessStdio;
use ai6::directorates::q::terminal::{AgentTerminal, STREAM_COLS, Spec, TerminalSession};
use ai6::directorates::section_00::agent::{
    AgentHandler, AgentResume, AgentStart, AssistantMode, SessionOverlays, SessionRecordFile,
    SessionRecordQuery, clearance_runs_commands,
};
use ai6::directorates::section_00::gate::{CABLE_TOKEN_ENV, CABLE_URL_ENV, GATE_PROGRAM_ENV};
use ai6::directorates::section_00::records;
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use serde::Deserialize;
use serde_json::{Value, json};

use files::SessionFiles;
use session::{AntigravitySession, Respawn, Restriction};

/// Antigravity CLI AgentHandler configuration.
#[derive(Debug, Clone, ToValue, FromValue, Schema)]
#[map(crate = ::ai6::guatiao)]
pub struct AntigravityConfig {
    /// Executable visible to the AI6-owned terminal environment.
    #[map(default = String::from("agy"))]
    pub executable: String,
    /// Bytes of one line of agy's output the Session holds before handing over what it has.
    /// Default 67108864 (64 MiB).
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
    /// Milliseconds a closing Session waits for agy to have exited. Default 2000.
    #[map(default = 2_000)]
    pub close_wait_ms: u64,
    /// Milliseconds the end of the output waits for agy's exit status. Default 1000.
    #[map(default = 1_000)]
    pub exit_wait_ms: u64,
    /// Bytes of an assistant's context file read into the Session's system prompt; a larger file
    /// is refused. Default 1048576.
    #[map(default = 1024 * 1024)]
    pub context_file_bytes: u64,
    /// Seconds agy gives the green ink hook on each command; a hook past it holds the command.
    /// Default 60.
    #[map(default = 60)]
    pub gate_timeout_secs: u64,
    /// agy's `timeoutSeconds` for AI6's MCP servers: positive replaces its limits, negative turns
    /// them off, and 0 leaves its own, 300 s to connect and 180 s for a call. Default 0.
    #[map(default = 0)]
    pub mcp_timeout_secs: i64,
    /// The Agent's `handler_settings` table as TOML, which the Station fills in. Empty keeps every
    /// default.
    #[map(default = String::new())]
    pub settings_toml: String,
}

impl Default for AntigravityConfig {
    fn default() -> Self {
        Self {
            executable: String::from("agy"),
            json_line_bytes: 64 * 1024 * 1024,
            csi_bytes: 64,
            tail_lines: 8,
            tail_line_bytes: 512,
            close_wait_ms: 2_000,
            exit_wait_ms: 1_000,
            context_file_bytes: 1024 * 1024,
            gate_timeout_secs: 60,
            mcp_timeout_secs: 0,
            settings_toml: String::new(),
        }
    }
}

/// What an Agent's `handler_settings` change. Unknown keys are refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct AntigravitySettings {
    /// agy permission rules (`command(git)`, `read_url(*)`) added to the allow list, or the names
    /// of tools added to what a Mission's Session is offered (`search_web`). Answer-only and
    /// assistant Sessions ignore them.
    #[serde(default)]
    allowed_tools: Vec<String>,
    /// agy permission rules added to the deny list, or the names of tools taken out of what a
    /// Mission's Session is offered; a deny rule beats an allow rule.
    #[serde(default)]
    denied_tools: Vec<String>,
    /// Further `settings.json` keys for this Session's `--settings` (`modelProvider`), never those
    /// that decide what a Session may do ([`RESERVED_SETTINGS`]).
    #[serde(default)]
    settings: serde_json::Map<String, Value>,
    /// Lets agy update itself while a Session starts; off by default, so that a Session runs the
    /// CLI its fixtures were recorded from until an operator updates it.
    #[serde(default)]
    auto_update: bool,
    /// What marks a failed turn as a quota limit, found in its error without regard to case.
    #[serde(default = "limit_phrases")]
    limit_phrases: Vec<String>,
}

/// `settings.json` keys `handler_settings` may not set: they decide what a Session may do, which
/// `allowed_tools` and `denied_tools` say and report as weakenings.
const RESERVED_SETTINGS: [&str; 5] = [
    "permissions",
    "toolPermission",
    "agentMode",
    "trustedWorkspaces",
    "allowNonWorkspaceAccess",
];

/// How agy 1.4.12 words a quota limit: `RESOURCE_EXHAUSTED` and `Error 429` in the `error` of a
/// turn's `result`, seen when a stand-in model answered 429 to an API key; the last two are strings
/// of its binary for a signed-in account's quota, not seen in a run. `QUOTA_EXHAUSTED` is the reason
/// Google's error details carry, which agy prints as it gets them.
fn limit_phrases() -> Vec<String> {
    [
        "RESOURCE_EXHAUSTED",
        "Error 429",
        "QUOTA_EXHAUSTED",
        "exhausted your quota",
        "individual quota reached",
    ]
    .map(str::to_owned)
    .to_vec()
}

impl AntigravitySettings {
    fn parse(toml_text: &str) -> Result<Self, String> {
        let settings: Self = toml::from_str(toml_text).map_err(|error| error.to_string())?;
        for rule in settings.allowed_tools.iter().chain(&settings.denied_tools) {
            if !valid_rule(rule) {
                return Err(format!(
                    "{rule:?} is not an agy permission rule (`action(target)`) or a tool name"
                ));
            }
        }
        if let Some(key) = RESERVED_SETTINGS
            .iter()
            .find(|key| settings.settings.contains_key(**key))
        {
            return Err(format!(
                "settings may not set {key}: allowed_tools and denied_tools say what a Session may do"
            ));
        }
        Ok(settings)
    }
}

/// A tool name or an `action(target)` rule. The action is checked because a tool name goes into
/// the agent file's YAML as it is.
fn valid_rule(rule: &str) -> bool {
    let (name, target) = match rule.split_once('(') {
        Some((name, target)) => (name, Some(target)),
        None => (rule, None),
    };
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && target.is_none_or(|target| target.ends_with(')'))
}

/// The tool a Session runs commands with, and the permission action that governs it.
const COMMAND_TOOL: &str = "run_command";
const COMMAND_ACTION: &str = "command";

/// The tools that read: `read_file` is also how a model learns an MCP tool's arguments, from
/// `<app data>/mcp/<server>/<tool>.json`, which agy lets it read without a rule.
const READ_TOOLS: [&str; 4] = ["read_file", "grep_search", "find_by_name", "list_dir"];

/// The tools that edit, which an allow rule rooted at the working directory lets run.
const EDIT_TOOLS: [&str; 2] = ["write_file", "edit_file"];

/// Paths under the working directory no file tool may write, whatever else is allowed: agy loads a
/// working directory's `.agents` and `.agent` hooks at its next start, and a planted `.git` hook
/// runs under whatever next runs git there.
const PROTECTED_DIRECTORIES: [&str; 3] = [".git", ".agents", ".agent"];

/// Files a working directory carries that run commands or connect servers in any Session started
/// there, trusted or not (agy 1.4.12: both hook files ran with the directory outside
/// `trustedWorkspaces`, and `.agents/mcp_config.json` connected). The plugin directories are from
/// agy's documentation.
const PROJECT_HOOKS: [&str; 4] = [
    ".agents/hooks.json",
    ".agent/hooks.json",
    ".agents/plugins",
    ".agent/plugins",
];
const PROJECT_MCP: [&str; 2] = [".agents/mcp_config.json", ".agent/mcp_config.json"];

/// What one launch of agy is told: the agent's tools, its permission rules and how the stream is
/// watched. Agents, and so tools, are fixed by the conversation's first launch (agy 1.4.12 kept a
/// conversation's tools and system prompt under another `--agent`), while `--settings` is read at
/// each launch, so what a later clearance takes away is denied by rule.
#[derive(Debug, Default)]
struct Plan {
    tools: Vec<String>,
    inherit_mcp: bool,
    allow: Vec<String>,
    deny: Vec<String>,
    /// Each MCP server's name and the prefix of its two variables.
    servers: Vec<(String, String)>,
    /// The Session's environment, where those variables are.
    endpoints: BTreeMap<String, String>,
    restriction: Option<Restriction>,
    gate: bool,
}

/// What a request says about its Session, on a start or a resume.
struct Launch<'a> {
    working_directory: &'a str,
    clearance: &'a str,
    environment: &'a BTreeMap<String, String>,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    project_settings: bool,
    project_mcp: bool,
    answer_only: bool,
    assistant: Option<&'a AssistantMode>,
    overlays: &'a SessionOverlays,
}

/// Starts and resumes Antigravity CLI streams for AI6 Agent Sessions.
#[derive(Provider)]
#[provider(
    AgentHandler,
    id = "antigravity",
    name = "Antigravity CLI",
    crate = ::ai6::guatiao,
    config = AntigravityConfig
)]
pub struct AntigravityHandler {
    config: AntigravityConfig,
    settings: AntigravitySettings,
}

impl TryFrom<AntigravityConfig> for AntigravityHandler {
    type Error = ProviderError;

    fn try_from(config: AntigravityConfig) -> Result<Self, Self::Error> {
        if config.executable.trim().is_empty() {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "the agy executable is empty",
            ));
        }
        let settings = AntigravitySettings::parse(&config.settings_toml).map_err(|reason| {
            refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!("handler_settings: {reason}"),
            )
        })?;
        Ok(Self { config, settings })
    }
}

/// The format of the Antigravity CLI's conversation transcript.
pub const RECORD_FORMAT: &str = "antigravity-jsonl";

/// The Antigravity CLI's transcript of the conversation `query` names, when it is on this
/// host: `~/.gemini/antigravity-cli/brain/<conversation>/transcript.jsonl`, the path its hooks
/// are given as `transcriptPath` (1.4.12).
fn antigravity_record(query: &SessionRecordQuery) -> Option<SessionRecordFile> {
    if !records::file_safe(&query.session_id) {
        return None;
    }
    let path = records::home(&query.environment)?
        .join(".gemini")
        .join("antigravity-cli")
        .join("brain")
        .join(&query.session_id)
        .join("transcript.jsonl");
    path.is_file().then(|| SessionRecordFile {
        path: path.to_string_lossy().into_owned(),
        format: RECORD_FORMAT.to_owned(),
    })
}

impl AgentHandler for AntigravityHandler {
    /// agy with `--input-format stream-json` reads JSON lines from standard input.
    fn stdio(&self) -> Result<HarnessStdio, ProviderError> {
        Ok(HarnessStdio::Json)
    }

    fn program(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.config.executable.clone()))
    }

    /// agy keeps each conversation's transcript under its home directory.
    fn session_record(
        &self,
        query: SessionRecordQuery,
    ) -> Result<Option<SessionRecordFile>, ProviderError> {
        Ok(antigravity_record(&query))
    }

    /// Each rule of `allowed_tools` no default list holds. A rule the settings deny as well allows
    /// nothing, so it is not one. A rule for a command is one however the Agent is configured,
    /// since any Agent can come to work at `desk`.
    fn weakenings(&self) -> Result<Vec<String>, ProviderError> {
        let denied = &self.settings.denied_tools;
        Ok(self
            .settings
            .allowed_tools
            .iter()
            .filter(|rule| !denied.contains(rule))
            .filter_map(|rule| {
                if rule_runs_commands(rule) {
                    Some(format!(
                        "allowed_tools adds {rule} to what its desk Sessions may call"
                    ))
                } else if READ_TOOLS.contains(&rule.as_str()) || EDIT_TOOLS.contains(&rule.as_str())
                {
                    None
                } else {
                    Some(format!(
                        "allowed_tools adds {rule} to what its field Sessions may call"
                    ))
                }
            })
            .collect())
    }

    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let launch = Launch {
            working_directory: &request.working_directory,
            clearance: &request.clearance,
            environment: &request.environment,
            model: request.model.as_deref(),
            effort: request.effort.as_deref(),
            project_settings: request.project_settings,
            project_mcp: request.project_mcp,
            answer_only: request.answer_only,
            assistant: request.assistant.as_ref(),
            overlays: &request.overlays,
        };
        let plan = self.plan(&launch)?;
        let context = self.context(
            &request
                .overlays
                .with_routing(&request.initial_context, true),
            request.assistant.as_ref(),
        )?;
        let files = SessionFiles::create(
            &request.working_directory,
            &plan,
            self.config.mcp_timeout_secs,
            self.gate_hook(&launch, &plan).as_ref(),
            &context,
            &request.overlays,
        )?;
        let argv = self.argv(&launch, &plan, &files, Some(files.agent_name()), None);
        let environment = self.environment(&request.environment);
        let session = spawn(
            &terminal,
            argv.clone(),
            &request.working_directory,
            &environment,
        )?;
        let operation = AntigravitySession::new(
            session,
            None,
            plan.restriction,
            &self.config,
            self.settings.limit_phrases.clone(),
            files,
            Respawn {
                terminal,
                argv,
                working_directory: request.working_directory.clone(),
                environment,
            },
        );
        operation.send(&request.prompt)?;
        Ok(operation.into_object())
    }

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let launch = Launch {
            working_directory: &request.working_directory,
            clearance: &request.clearance,
            environment: &request.environment,
            model: request.model.as_deref(),
            effort: request.effort.as_deref(),
            project_settings: request.project_settings,
            project_mcp: request.project_mcp,
            answer_only: request.answer_only,
            assistant: request.assistant.as_ref(),
            overlays: &request.overlays,
        };
        let plan = self.plan(&launch)?;
        // The agent and its system prompt stay as the conversation's first launch had them, so
        // this launch needs no agent file; its MCP servers, hooks and skills are this launch's.
        let files = SessionFiles::create(
            &request.working_directory,
            &plan,
            self.config.mcp_timeout_secs,
            self.gate_hook(&launch, &plan).as_ref(),
            "",
            &request.overlays,
        )?;
        let argv = self.argv(
            &launch,
            &plan,
            &files,
            None,
            Some(request.session_id.as_str()),
        );
        let environment = self.environment(&request.environment);
        let session = spawn(
            &terminal,
            argv.clone(),
            &request.working_directory,
            &environment,
        )?;
        let operation = AntigravitySession::new(
            session,
            Some(request.session_id.clone()),
            plan.restriction,
            &self.config,
            self.settings.limit_phrases.clone(),
            files,
            Respawn {
                terminal,
                argv,
                working_directory: request.working_directory.clone(),
                environment,
            },
        );
        // agy starts a new conversation for an ID it does not have and says so only on standard
        // error, so the message waits for the `init` line that names the conversation.
        if let Some(message) = request.message {
            operation.hold(message)?;
        }
        Ok(operation.into_object())
    }
}

impl AntigravityHandler {
    /// Decides what this launch may do, refusing what the handler cannot hold it to.
    fn plan(&self, launch: &Launch<'_>) -> Result<Plan, ProviderError> {
        check_project_files(
            Path::new(launch.working_directory),
            launch.project_settings,
            launch.project_mcp,
        )?;
        let mut plan = if let Some(mode) = launch.assistant {
            self.assistant_plan(launch, mode)?
        } else if launch.answer_only {
            answer_only_plan()
        } else {
            self.mission_plan(launch)?
        };
        plan.endpoints.clone_from(launch.environment);
        Ok(plan)
    }

    /// A Mission's Session at its clearance. At `field` and `00`: reading, editing inside the
    /// working directory, commands and AI6's MCP servers. At `desk` and any other clearance: the
    /// same with no command tool, and commands denied by rule, since a resumed conversation keeps
    /// the tools of its first launch. In remote-hands mode only the Theater's tools.
    fn mission_plan(&self, launch: &Launch<'_>) -> Result<Plan, ProviderError> {
        let environment = launch.environment;
        let commands = clearance_runs_commands(launch.clearance);
        let remote_hands = has_endpoint(environment, "AI6_THEATER_MCP");
        let settings = &self.settings;
        let bare_denied = |tool: &str| settings.denied_tools.iter().any(|rule| rule == tool);
        let mut plan = Plan {
            inherit_mcp: true,
            gate: true,
            ..Plan::default()
        };
        if has_endpoint(environment, "AI6_CABLE_MCP") {
            plan.servers
                .push(("ai6-cables".to_owned(), "AI6_CABLE_MCP".to_owned()));
        }
        if remote_hands {
            plan.servers
                .push(("ai6-theater".to_owned(), "AI6_THEATER_MCP".to_owned()));
        }
        plan.allow.extend(
            plan.servers
                .iter()
                .map(|(name, _)| format!("mcp({name}/*)")),
        );
        let lifted = settings
            .allowed_tools
            .iter()
            .any(|rule| rule_runs_commands(rule));
        if remote_hands {
            // Nothing the harness does may land on the Station.
            plan.deny
                .extend(["read_file(*)", "write_file(*)", "command(*)"].map(str::to_owned));
            if !commands && !lifted {
                plan.deny.extend(
                    [
                        "mcp(ai6-theater/theater_run)",
                        "mcp(ai6-theater/theater_write)",
                    ]
                    .map(str::to_owned),
                );
            }
        } else {
            let root = Path::new(launch.working_directory);
            plan.tools.extend(
                READ_TOOLS
                    .iter()
                    .chain(EDIT_TOOLS.iter())
                    .filter(|tool| !bare_denied(tool))
                    .map(|tool| (*tool).to_owned()),
            );
            plan.allow.push(format!("write_file({})", display(root)));
            // agy 1.4.12 ended the turn on a read outside its workspaces under no rule.
            plan.allow.extend(
                launch
                    .overlays
                    .roots
                    .iter()
                    .map(|root| format!("read_file({root})")),
            );
            plan.deny.extend(
                PROTECTED_DIRECTORIES
                    .iter()
                    .map(|name| format!("write_file({})", display(&root.join(name)))),
            );
            if commands || lifted {
                if !bare_denied(COMMAND_TOOL) {
                    plan.tools.push(COMMAND_TOOL.to_owned());
                }
                if commands {
                    plan.allow.push(format!("{COMMAND_ACTION}(*)"));
                }
            } else {
                plan.deny.push(format!("{COMMAND_ACTION}(*)"));
            }
            for rule in &settings.allowed_tools {
                if !rule.contains('(') && !plan.tools.contains(rule) && !bare_denied(rule) {
                    plan.tools.push(rule.clone());
                }
            }
        }
        for rule in &settings.allowed_tools {
            if rule.contains('(') {
                plan.allow.push(rule.clone());
            } else if rule == COMMAND_TOOL && !remote_hands && !commands {
                plan.allow.push(format!("{COMMAND_ACTION}(*)"));
            }
        }
        plan.deny.extend(
            settings
                .denied_tools
                .iter()
                .filter(|rule| rule.contains('('))
                .cloned(),
        );
        Ok(plan)
    }

    /// Moneypenny's Session: reading and the mode's MCP servers, or a refusal naming what is asked
    /// for beyond them. agy has no `Bash(<interpreter> *)` rule, and its only sandbox is
    /// `--sandbox`, which AI6 has not measured, so it runs no interpreter; it cannot keep a
    /// `write_file` rule from a name starting with `.`, so it takes no `files`; `search_web` takes
    /// no host list, so it has no network.
    fn assistant_plan(
        &self,
        launch: &Launch<'_>,
        mode: &AssistantMode,
    ) -> Result<Plan, ProviderError> {
        mode.validate()
            .map_err(|reason| refuse(Status::GUATIAO_ERR_BAD_VALUE, reason))?;
        let refused = if !mode.interpreters.is_empty() {
            Some("the interpreters it names")
        } else if !mode.files.is_empty() {
            Some("the directories it names to write")
        } else if !mode.skills.is_empty() {
            Some("the skills it names")
        } else if !mode.network.reaches_nothing() {
            Some("a network besides its MCP servers")
        } else {
            None
        };
        if let Some(what) = refused {
            return Err(refuse(
                Status::GUATIAO_ERR_UNSUPPORTED,
                format!(
                    "the Antigravity CLI handler runs an assistant with reading and its MCP \
                     servers only, so it cannot give this one {what}"
                ),
            ));
        }
        let mut servers = Vec::new();
        for server in &mode.mcp_servers {
            if !has_endpoint(launch.environment, &server.env_prefix) {
                return Err(refuse(
                    Status::GUATIAO_ERR_BAD_VALUE,
                    format!(
                        "the environment carries no URL and token for MCP server {:?}",
                        server.name
                    ),
                ));
            }
            servers.push((server.name.clone(), server.env_prefix.clone()));
        }
        let allow = servers
            .iter()
            .map(|(name, _)| format!("mcp({name}/*)"))
            .collect();
        let names = servers.iter().map(|(name, _)| name.clone()).collect();
        Ok(Plan {
            tools: vec!["read_file".to_owned()],
            inherit_mcp: true,
            allow,
            deny: RESTRICTED_DENIED.map(str::to_owned).to_vec(),
            servers,
            endpoints: BTreeMap::new(),
            restriction: Some(Restriction {
                servers: names,
                tools: ASSISTANT_TOOLS.to_vec(),
            }),
            gate: false,
        })
    }

    /// The hook that holds a command green ink does not cover, for a Session with a Station to
    /// ask. It runs under `sh -c`, so each part is quoted; the token stays in the environment.
    fn gate_hook(&self, launch: &Launch<'_>, plan: &Plan) -> Option<Value> {
        if !plan.gate || !plan.tools.iter().any(|tool| tool == COMMAND_TOOL) {
            return None;
        }
        let environment = launch.environment;
        let cable_url = environment
            .get(CABLE_URL_ENV)
            .filter(|_| environment.contains_key(CABLE_TOKEN_ENV))?;
        let program = hook_program(environment)?;
        Some(json!({
            "ai6-green-ink": {
                "PreToolUse": [{
                    "matcher": format!("^{COMMAND_TOOL}$"),
                    "hooks": [{
                        "type": "command",
                        "command": format!(
                            "{} green-ink gate --cable-url {}",
                            shell_quote(&program),
                            shell_quote(cable_url)
                        ),
                        "timeout": self.config.gate_timeout_secs,
                    }]
                }]
            }
        }))
    }

    /// The Session's system prompt: the standing context and an assistant's context file.
    fn context(
        &self,
        context: &[String],
        assistant: Option<&AssistantMode>,
    ) -> Result<String, ProviderError> {
        let mut parts: Vec<String> = context.to_vec();
        if let Some(file) = assistant.and_then(|mode| mode.context_file.as_ref()) {
            let size = std::fs::metadata(file)
                .map_err(|error| {
                    refuse(
                        Status::GUATIAO_ERR_BAD_VALUE,
                        format!("the assistant's context file {file}: {error}"),
                    )
                })?
                .len();
            if size > self.config.context_file_bytes {
                return Err(refuse(
                    Status::GUATIAO_ERR_BAD_VALUE,
                    format!(
                        "the assistant's context file {file} holds {size} bytes, over \
                         context_file_bytes ({})",
                        self.config.context_file_bytes
                    ),
                ));
            }
            parts.push(std::fs::read_to_string(file).map_err(|error| {
                refuse(
                    Status::GUATIAO_ERR_BAD_VALUE,
                    format!("the assistant's context file {file}: {error}"),
                )
            })?);
        }
        Ok(parts.join("\n\n"))
    }

    fn argv(
        &self,
        launch: &Launch<'_>,
        plan: &Plan,
        files: &SessionFiles,
        agent: Option<&str>,
        conversation: Option<&str>,
    ) -> Vec<String> {
        let mut argv = vec![
            self.config.executable.clone(),
            "--input-format".to_owned(),
            "stream-json".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--print-timeout=0s".to_owned(),
            "--add-dir".to_owned(),
            files.path_text(),
        ];
        if let Some(skills) = files.skills_text() {
            argv.extend(["--add-dir".to_owned(), skills]);
        }
        if let Some(agent) = agent {
            argv.push(format!("--agent={agent}"));
        }
        if let Some(conversation) = conversation {
            argv.push(format!("--conversation={conversation}"));
        }
        add_model(&mut argv, launch.model, launch.effort);
        argv.extend([
            "--settings".to_owned(),
            self.session_settings(plan, files).to_string(),
        ]);
        argv
    }

    /// The `--settings` value: never saved by agy, so nothing here reaches the user's own file. It
    /// names no secret; the Session's own directory is denied to the file tools, since it holds
    /// the MCP servers' tokens.
    fn session_settings(&self, plan: &Plan, files: &SessionFiles) -> Value {
        let mut deny = plan.deny.clone();
        deny.push(format!("read_file({})", files.path_text()));
        deny.push(format!("write_file({})", files.path_text()));
        let mut settings = self.settings.settings.clone();
        settings.insert(
            "permissions".to_owned(),
            json!({ "allow": plan.allow, "deny": deny }),
        );
        Value::Object(settings)
    }

    /// The harness's environment: the Session's, with agy's updater off unless the Agent lets it
    /// run. Only the value `true` turns it off (agy 1.4.12 went on updating with `1`).
    fn environment(&self, environment: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut environment = environment.clone();
        if !self.settings.auto_update {
            environment.insert(AUTO_UPDATE_OFF_ENV.to_owned(), "true".to_owned());
        }
        environment
    }
}

/// The variable that keeps agy from starting its updater.
const AUTO_UPDATE_OFF_ENV: &str = "AGY_CLI_DISABLE_AUTO_UPDATE";

/// The tools a restricted Session is not to reach by rule, should its offer ever widen.
const RESTRICTED_DENIED: [&str; 4] = [
    "write_file(*)",
    "command(*)",
    "read_url(*)",
    "execute_url(*)",
];

/// The tools an assistant Session may call: reading, the MCP tools (checked by server), and
/// `manage_task`, which agy offers every agent and which has no background task to act on here.
const ASSISTANT_TOOLS: [&str; 5] = [
    "read_file",
    "call_mcp_tool",
    "list_resources",
    "read_resource",
    "manage_task",
];

/// A Session that only reads its prompt and answers. agy offers `list_resources`, `read_resource`
/// and `manage_task` to any agent; the rules and the tripwire keep the first two away.
fn answer_only_plan() -> Plan {
    let mut deny: Vec<String> = RESTRICTED_DENIED.map(str::to_owned).to_vec();
    deny.extend(["read_file(*)", "mcp(*)"].map(str::to_owned));
    Plan {
        tools: Vec::new(),
        inherit_mcp: false,
        allow: Vec::new(),
        deny,
        servers: Vec::new(),
        endpoints: BTreeMap::new(),
        restriction: Some(Restriction {
            servers: Vec::new(),
            tools: vec!["manage_task"],
        }),
        gate: false,
    }
}

/// Whether a rule lets a Session run commands.
fn rule_runs_commands(rule: &str) -> bool {
    rule == COMMAND_TOOL || rule.starts_with("command(")
}

/// Refuses a working directory whose own files agy would load though AI6 did not allow them.
fn check_project_files(root: &Path, settings: bool, mcp: bool) -> Result<(), ProviderError> {
    let found =
        |names: &[&'static str]| names.iter().find(|name| root.join(name).exists()).copied();
    let carried = if settings {
        None
    } else {
        found(&PROJECT_HOOKS)
    };
    let carried = carried.or_else(|| {
        if settings && mcp {
            None
        } else {
            found(&PROJECT_MCP)
        }
    });
    match carried {
        Some(name) => Err(refuse(
            Status::GUATIAO_ERR_UNSUPPORTED,
            format!(
                "the working directory carries {name}, which the Antigravity CLI loads in every \
                 Session started there and no flag keeps out, and the Operation does not allow \
                 its project settings: remove it, or allow them with harness.project_settings"
            ),
        )),
        None => Ok(()),
    }
}

/// Adds the model and effort, each as one `--flag=VALUE` argument: a value that starts with `-` is
/// then never read as another option. A model name that ends in an effort (`gemini-3.8-flash-low`)
/// already says one, and agy refuses `--effort` beside it when they differ, so the name wins.
fn add_model(argv: &mut Vec<String>, model: Option<&str>, effort: Option<&str>) {
    if let Some(model) = model {
        argv.push(format!("--model={model}"));
    }
    let named = model.is_some_and(|model| {
        ["low", "medium", "high", "xhigh", "max"]
            .iter()
            .any(|effort| model.ends_with(&format!("-{effort}")))
    });
    if let Some(effort) = effort.filter(|_| !named) {
        argv.push(format!("--effort={effort}"));
    }
}

/// The program a hook runs as AI6's gate: `AI6_GREEN_INK_GATE` from the Session environment when
/// not blank, else the executable hosting this handler.
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

/// One word for `sh`.
fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// Whether `environment` carries an MCP endpoint's URL and token under `prefix`.
fn has_endpoint(environment: &BTreeMap<String, String>, prefix: &str) -> bool {
    environment.contains_key(&format!("{prefix}_URL"))
        && environment.contains_key(&format!("{prefix}_TOKEN"))
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn spawn(
    terminal: &Object<dyn AgentTerminal>,
    argv: Vec<String>,
    cwd: &str,
    environment: &BTreeMap<String, String>,
) -> Result<Object<dyn TerminalSession>, ProviderError> {
    let spec = Spec {
        argv,
        cwd: Some(cwd.to_owned()),
        env: Some(environment.clone()),
        rows: 24,
        cols: STREAM_COLS,
    };
    let value = spec.to_value().map_err(internal)?;
    terminal.spawn(&value)
}

fn internal(error: impl std::fmt::Display) -> ProviderError {
    refuse(Status::GUATIAO_ERR_INTERNAL, error)
}

fn refuse(status: Status, error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(status, &error.to_string())
}

#[cfg(test)]
mod record_tests {
    use super::*;

    #[test]
    fn a_conversations_transcript_is_found_under_its_home() {
        let home = tempfile::tempdir().unwrap();
        let brain = home
            .path()
            .join(".gemini")
            .join("antigravity-cli")
            .join("brain")
            .join("c-1");
        std::fs::create_dir_all(&brain).unwrap();
        std::fs::write(brain.join("transcript.jsonl"), "{}\n").unwrap();
        let query = |conversation: &str| SessionRecordQuery {
            session_id: conversation.to_owned(),
            working_directory: "/work".to_owned(),
            environment: BTreeMap::from([(
                "HOME".to_owned(),
                home.path().to_string_lossy().into_owned(),
            )]),
        };
        let found = antigravity_record(&query("c-1")).unwrap();
        assert_eq!(
            std::path::PathBuf::from(found.path),
            brain.join("transcript.jsonl")
        );
        assert_eq!(found.format, RECORD_FORMAT);
        assert!(antigravity_record(&query("c-2")).is_none());
        assert!(antigravity_record(&query("..")).is_none());
    }
}
