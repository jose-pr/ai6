//! The `acp` AgentHandler: any Agent Client Protocol agent runs as an Agent's handler, with
//! AI6's environment, accounts and green-ink gate.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

ai6::guatiao::providers!(AcpHandler);

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use ai6::directorates::q::terminal::{AgentTerminal, ExitInfo, STREAM_COLS, Spec, TerminalSession};
use ai6::directorates::section_00::acp::{
    AcpOptions, AcpSession, DEFAULT_MAX_TOOL_KINDS, DEFAULT_TEXT_FLUSH_BYTES, Exchange,
    GateDecider, TerminalIo, UNBOUNDED_LINE_BYTES, UNBOUNDED_MS,
};
use ai6::directorates::section_00::agent::{
    AgentHandler, AgentResume, AgentStart, AssistantMode, clearance_runs_commands,
};
use ai6::directorates::section_00::gate::{CABLE_TOKEN_ENV, CABLE_URL_ENV};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use ai6::{AgentSession, HarnessStdio, ModelInfo, ModelListing};
use coqui_acp::lines::{Diagnostics, over_io};
use coqui_acp::{Answer, DescribeLimits};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// The configuration the Station gives every handler: the Agent's `handler_settings` as TOML.
#[derive(Debug, Clone, ToValue, FromValue, Schema)]
#[map(crate = ::ai6::guatiao)]
pub struct AcpConfig {
    /// The Agent's `handler_settings` table, as TOML text; empty until an Agent sets it.
    #[map(default = String::new())]
    pub settings_toml: String,
}

/// What an Agent's `handler_settings` say about the agent to run. Unknown keys are refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcpSettings {
    /// The program that speaks ACP on its standard input and output.
    pub command: String,
    /// Its arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment added under the Session's own (the Session wins).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// A table passed beside AI6's launch data as `session/new._meta` (an adapter's options).
    #[serde(default)]
    pub meta: Map<String, Value>,
    /// The config option id for the model; default: the option of category `model`, else `model`.
    #[serde(default)]
    pub model_option: Option<String>,
    /// The config option id for the effort; default: the option of category `thought_level`,
    /// else `effort`.
    #[serde(default)]
    pub effort_option: Option<String>,
    /// The ACP version spoken; only `v1`.
    #[serde(default = "v1")]
    pub protocol: String,
    /// How long `initialize` may take, in milliseconds.
    #[serde(default = "initialize_timeout")]
    pub initialize_timeout_ms: u64,
    /// How long a request other than a prompt may take.
    #[serde(default = "request_timeout")]
    pub request_timeout_ms: u64,
    /// How long the first prompt of a façade may take to start.
    #[serde(default = "start_timeout")]
    pub start_timeout_ms: u64,
    /// How long close waits for the agent before its process is ended.
    #[serde(default = "close_grace")]
    pub close_grace_ms: u64,
    /// How many of the agent's last non-protocol output lines are kept to explain its death.
    #[serde(default = "diagnostic_lines")]
    pub diagnostic_lines: usize,
    /// How long, in milliseconds, the report of a lost agent waits for its exit status.
    #[serde(default = "exit_wait")]
    pub exit_wait_ms: i64,
    /// How often, in milliseconds, a silent agent's output is checked for a stop request.
    #[serde(default = "poll")]
    pub poll_ms: i64,
    /// Starts a `desk` Session with whatever tools the agent has: AI6 does not choose an ACP
    /// agent's tools, so it cannot keep commands from one, and without this the Session is
    /// refused. Reported as a weakening.
    #[serde(default)]
    pub unconfined_desk: bool,
    /// What marks a failed turn as a provider's usage or rate limit, found in its error without
    /// regard to case: such a Session stops as limited rather than failed, so its route or account
    /// is passed over until the limit is likely over.
    #[serde(default = "limit_phrases")]
    pub limit_phrases: Vec<String>,
    /// UTF-8 bytes of agent text gathered before they are delivered mid-message.
    #[serde(default = "text_flush")]
    pub text_flush_bytes: usize,
    /// The tool calls whose kind is kept, by which a later message about one that does not say
    /// its kind is judged; past it the oldest is forgotten, and such a message is of unknown kind.
    #[serde(default = "tool_kinds")]
    pub max_tool_kinds: usize,
    /// What asking the agent for its models, before any Session, keeps of its answer: options.
    #[serde(default = "describe_options")]
    pub describe_options: usize,
    /// Values kept of each option.
    #[serde(default = "describe_values")]
    pub describe_values: usize,
    /// Modes kept.
    #[serde(default = "describe_modes")]
    pub describe_modes: usize,
    /// Sessions kept of the agent's list, across its pages.
    #[serde(default = "describe_sessions")]
    pub describe_sessions: usize,
    /// UTF-8 bytes past which an identifier is dropped and other text cut.
    #[serde(default = "describe_text")]
    pub describe_text_bytes: usize,
    /// Pages of the agent's session list asked for.
    #[serde(default = "describe_pages")]
    pub describe_pages: usize,
}

/// How the Grok CLI words a limit (`Rate limited`, with xAI's `subscription:free-usage-exhausted`;
/// out of credits, `402 Payment Required` with `personal-team-blocked:spending-limit`) and how other
/// agents commonly do.
fn limit_phrases() -> Vec<String> {
    [
        "rate limited",
        "rate limit exceeded",
        "usage-exhausted",
        "usage limit",
        "quota exceeded",
        "spending-limit",
        "run out of credits",
        "402 payment required",
    ]
    .map(str::to_owned)
    .to_vec()
}

fn v1() -> String {
    "v1".to_owned()
}
fn initialize_timeout() -> u64 {
    20_000
}
fn request_timeout() -> u64 {
    30_000
}
fn start_timeout() -> u64 {
    60_000
}
fn close_grace() -> u64 {
    2_000
}
fn diagnostic_lines() -> usize {
    20
}
fn exit_wait() -> i64 {
    1_000
}
fn poll() -> i64 {
    200
}
fn text_flush() -> usize {
    DEFAULT_TEXT_FLUSH_BYTES
}
fn tool_kinds() -> usize {
    DEFAULT_MAX_TOOL_KINDS
}
fn describe_options() -> usize {
    DescribeLimits::new().options
}
fn describe_values() -> usize {
    DescribeLimits::new().values
}
fn describe_modes() -> usize {
    DescribeLimits::new().modes
}
fn describe_sessions() -> usize {
    DescribeLimits::new().sessions
}
fn describe_text() -> usize {
    DescribeLimits::new().text
}
fn describe_pages() -> usize {
    DescribeLimits::new().pages
}

/// Runs an ACP agent for each Session.
#[derive(Provider)]
#[provider(
    AgentHandler,
    id = "acp",
    name = "Agent Client Protocol agent",
    crate = ::ai6::guatiao,
    config = AcpConfig
)]
pub struct AcpHandler {
    settings: Option<AcpSettings>,
}

fn refuse(message: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &message.to_string())
}

impl TryFrom<AcpConfig> for AcpHandler {
    type Error = ProviderError;

    fn try_from(config: AcpConfig) -> Result<Self, Self::Error> {
        if config.settings_toml.trim().is_empty() {
            return Ok(Self { settings: None });
        }
        let settings: AcpSettings = toml::from_str(&config.settings_toml)
            .map_err(|error| refuse(format!("acp handler settings: {error}")))?;
        if settings.command.trim().is_empty() {
            return Err(refuse("acp handler settings: `command` is empty"));
        }
        if settings.protocol != "v1" {
            return Err(refuse(format!(
                "acp handler settings: protocol {:?} is not supported; only \"v1\"",
                settings.protocol
            )));
        }
        Ok(Self {
            settings: Some(settings),
        })
    }
}

impl AcpHandler {
    fn settings(&self) -> Result<&AcpSettings, ProviderError> {
        self.settings.as_ref().ok_or_else(|| {
            refuse("the acp handler needs `handler_settings` with a `command` on its Agent")
        })
    }
}

/// The lines the agent wrote outside the protocol, kept to explain its death.
fn last_words(diagnostics: &Diagnostics) -> String {
    diagnostics
        .lock()
        .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join(" | "))
        .unwrap_or_default()
}

/// What became of an agent whose connection ended: how its process ended and what it last wrote.
fn lost_agent(
    process: &Object<dyn TerminalSession>,
    diagnostics: &Diagnostics,
    exit_wait_ms: i64,
) -> Option<String> {
    let how = process
        .exit_status(exit_wait_ms)
        .ok()
        .and_then(|status| ExitInfo::from_value(&status).ok())
        .filter(|exit| !exit.running)
        .map(|exit| match (exit.code, exit.signal) {
            (Some(code), _) => format!("it exited with code {code}"),
            (None, Some(signal)) => format!("it was ended by signal {signal}"),
            (None, None) => "it exited".to_owned(),
        });
    let said = last_words(diagnostics);
    match (how, said.is_empty()) {
        (None, true) => None,
        (Some(how), true) => Some(how),
        (None, false) => Some(format!("it last wrote: {said}")),
        (Some(how), false) => Some(format!("{how}; it last wrote: {said}")),
    }
}

/// One HTTP MCP server entry in ACP's form.
fn http_server(name: &str, url: &str, token: &str) -> Value {
    json!({
        "type": "http",
        "name": name,
        "url": url,
        "headers": [{"name": "Authorization", "value": format!("Bearer {token}")}],
    })
}

/// The MCP servers a Session's environment names: the Cable tools and an assistant's own.
fn mcp_servers(
    environment: &BTreeMap<String, String>,
    assistant: Option<&AssistantMode>,
) -> Result<Vec<Value>, ProviderError> {
    if environment.contains_key("AI6_THEATER_MCP_URL") {
        return Err(refuse(
            "remote-hands mode replaces the agent's own shell and file tools, which an ACP agent cannot be made to do",
        ));
    }
    let mut servers = Vec::new();
    if let (Some(url), Some(token)) = (
        environment.get(CABLE_URL_ENV),
        environment.get(CABLE_TOKEN_ENV),
    ) {
        servers.push(http_server("ai6-cables", url, token));
    }
    for server in assistant.iter().flat_map(|mode| &mode.mcp_servers) {
        if let (Some(url), Some(token)) = (
            environment.get(&format!("{}_URL", server.env_prefix)),
            environment.get(&format!("{}_TOKEN", server.env_prefix)),
        ) {
            servers.push(http_server(&server.name, url, token));
        }
    }
    Ok(servers)
}

/// The text before the first prompt: ACP v1 has no system prompt, so the Agent's context and
/// the limits of a restricted Session go in front of the task.
fn briefing(context: &[String], restricted: bool, prompt: &str) -> String {
    let mut parts: Vec<String> = context.to_vec();
    if restricted {
        parts.push(
            "This is a read-only Session: answer from what you are given. Do not edit, delete or move files, and do not run commands."
                .to_owned(),
        );
    }
    parts.push(prompt.to_owned());
    parts.retain(|part| !part.trim().is_empty());
    parts.join("\n\n")
}

struct Launch<'a> {
    environment: &'a BTreeMap<String, String>,
    working_directory: &'a str,
    model: Option<&'a String>,
    effort: Option<&'a String>,
    restricted: bool,
    assistant: Option<&'a AssistantMode>,
    /// The Session is a Mission's at a clearance that may not run commands.
    desk: bool,
    prompt: Option<String>,
    resume: Option<String>,
}

impl AcpHandler {
    fn run(
        &self,
        launch: Launch<'_>,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let settings = self.settings()?;
        if launch.desk && !settings.unconfined_desk {
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "this Agent works at desk clearance, which allows no command tool, and AI6 does \
                 not choose an ACP agent's tools, so the Session was not started: give the Agent \
                 field clearance, or set unconfined_desk = true in its handler_settings to start \
                 it with whatever tools the agent has",
            ));
        }
        let mut environment = settings.env.clone();
        environment.extend(launch.environment.clone());
        let mut argv = vec![settings.command.clone()];
        argv.extend(settings.args.clone());
        let spec = Spec {
            argv,
            cwd: Some(launch.working_directory.to_owned()),
            env: Some(environment),
            rows: 24,
            cols: STREAM_COLS,
        };
        let process: Arc<Object<dyn TerminalSession>> =
            Arc::new(terminal.spawn(&spec.to_value().map_err(refuse)?)?);
        let io = TerminalIo::new(process.clone(), settings.poll_ms);
        let (lines, diagnostics) = over_io(
            io.clone(),
            io.clone(),
            settings.diagnostic_lines,
            UNBOUNDED_LINE_BYTES,
        )
        .map_err(|error| ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string()))?;
        let options = AcpOptions {
            cwd: launch.working_directory.to_owned(),
            mcp_servers: mcp_servers(launch.environment, launch.assistant)?,
            meta: settings.meta.clone(),
            model: launch.model.cloned(),
            effort: launch.effort.cloned(),
            model_option: settings.model_option.clone(),
            effort_option: settings.effort_option.clone(),
            prompt: launch.prompt,
            resume: launch.resume,
            reattach: None,
            restricted: launch.restricted,
            authenticate: None,
            initialize_timeout_ms: settings.initialize_timeout_ms,
            request_timeout_ms: settings.request_timeout_ms,
            start_timeout_ms: settings.start_timeout_ms,
            close_grace_ms: settings.close_grace_ms,
            decider: Arc::new(GateDecider::from_environment(
                launch.environment,
                launch.working_directory,
            )),
            close_on_drop: true,
            fail_on_disconnect: true,
            limit_phrases: settings.limit_phrases.clone(),
            disconnect_detail: Some({
                let (process, diagnostics) = (process.clone(), diagnostics.clone());
                let exit_wait_ms = settings.exit_wait_ms;
                Box::new(move || lost_agent(&process, &diagnostics, exit_wait_ms))
            }),
            text_flush_bytes: settings.text_flush_bytes,
            max_tool_kinds: settings.max_tool_kinds,
            extensions: None,
        };
        let ended = process.clone();
        let stopper = io.clone();
        let on_close: Box<dyn FnOnce() + Send> = Box::new(move || {
            let _ = ended.kill();
            stopper.stop();
        });
        AcpSession::connect(lines, options, Some(on_close))
            .map(AcpSession::into_handle)
            .map_err(|error| {
                let said = last_words(&diagnostics);
                if said.is_empty() {
                    error
                } else {
                    ProviderError::new(
                        Status::GUATIAO_ERR_INTERNAL,
                        &format!("{error}; the agent last wrote: {said}"),
                    )
                }
            })
    }
}

/// A wait in milliseconds as `coqui_acp` takes it: [`UNBOUNDED_MS`] is none.
fn wait_of(ms: u64) -> Option<Duration> {
    (ms != UNBOUNDED_MS).then(|| Duration::from_millis(ms))
}

/// The models the agent's option of category `model` offers, from its values or its groups'.
fn models_of(options: &[Value]) -> ModelListing {
    let Some(option) = options
        .iter()
        .find(|option| option["category"] == "model" && option["type"] == "select")
    else {
        return ModelListing::unknown("the agent offers no model to choose before a Session");
    };
    let value = |entry: &Value| {
        Some(ModelInfo {
            id: entry["value"].as_str()?.to_owned(),
            name: entry["name"].as_str().map(str::to_owned),
            description: entry["description"].as_str().map(str::to_owned),
        })
    };
    let models = option["options"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| match entry["options"].as_array() {
            Some(group) => group.iter().filter_map(value).collect(),
            None => value(entry).into_iter().collect::<Vec<_>>(),
        })
        .collect();
    ModelListing::of(models)
}

impl AcpHandler {
    /// Starts the agent with exactly `environment` over the settings' own, as a Session would
    /// be, and asks what it offers before any Session; the agent is ended once it has answered.
    fn describe(&self, environment: BTreeMap<String, String>) -> Result<ModelListing, String> {
        let settings = self.settings().map_err(|error| error.to_string())?;
        let mut merged = settings.env.clone();
        merged.extend(environment);
        let mut child = Command::new(&settings.command)
            .args(&settings.args)
            .env_clear()
            .envs(&merged)
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("{} could not be started: {error}", settings.command))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{} has no standard input or output",
                settings.command
            ));
        };
        let described = over_io(stdout, stdin, 0, UNBOUNDED_LINE_BYTES)
            .map_err(|error| error.to_string())
            .and_then(|(lines, _)| {
                Exchange::new()
                    .client_info("ai6", env!("CARGO_PKG_VERSION"))
                    .initialize_timeout(wait_of(settings.initialize_timeout_ms))
                    .request_timeout(wait_of(settings.request_timeout_ms))
                    .describe_limits(
                        DescribeLimits::new()
                            .options(settings.describe_options)
                            .values(settings.describe_values)
                            .modes(settings.describe_modes)
                            .sessions(settings.describe_sessions)
                            .text(settings.describe_text_bytes)
                            .pages(settings.describe_pages),
                    )
                    .describe(lines)
                    .map_err(|error| format!("the agent could not be asked: {error}"))
            });
        let _ = child.kill();
        let _ = child.wait();
        Ok(match described?.options {
            Answer::Known(options) => models_of(
                &options
                    .iter()
                    .filter_map(|option| serde_json::to_value(option).ok())
                    .collect::<Vec<_>>(),
            ),
            Answer::Unknown(reason) => ModelListing::unknown(reason),
            Answer::NotSupported => {
                ModelListing::unknown("the agent does not say what it offers before a Session")
            }
        })
    }
}

impl AgentHandler for AcpHandler {
    /// An ACP agent speaks JSON-RPC on plain pipes.
    fn stdio(&self) -> Result<HarnessStdio, ProviderError> {
        Ok(HarnessStdio::Json)
    }

    /// The `command` of its settings; without settings there is none to name.
    fn program(&self) -> Result<Option<String>, ProviderError> {
        Ok(self
            .settings
            .as_ref()
            .map(|settings| settings.command.clone()))
    }

    /// What the agent's model option offers, asked before any Session; an agent that does not
    /// say is unknown, with its reason.
    fn models(&self, environment: BTreeMap<String, String>) -> Result<ModelListing, ProviderError> {
        Ok(self
            .describe(environment)
            .unwrap_or_else(ModelListing::unknown))
    }

    /// AI6 does not choose an ACP agent's tools, so it cannot keep commands from a `desk` Session.
    fn clearance_refusal(&self, clearance: String) -> Result<Option<String>, ProviderError> {
        let settings = self.settings()?;
        Ok((!clearance_runs_commands(&clearance) && !settings.unconfined_desk).then(|| {
            "AI6 does not choose an ACP agent's tools, so it cannot keep commands from a Session \
             at this clearance, and the route's unconfined_desk is not set"
                .to_owned()
        }))
    }

    /// `unconfined_desk`: a `desk` Session runs with the agent's own tools.
    fn weakenings(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self
            .settings
            .as_ref()
            .filter(|settings| settings.unconfined_desk)
            .map(|_| {
                "unconfined_desk starts its desk Sessions with whatever tools the ACP agent has, \
                 commands among them"
                    .to_owned()
            })
            .into_iter()
            .collect())
    }

    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let restricted = request.answer_only || request.assistant.is_some();
        self.run(
            Launch {
                environment: &request.environment,
                working_directory: &request.working_directory,
                model: request.model.as_ref(),
                effort: request.effort.as_ref(),
                restricted,
                assistant: request.assistant.as_ref(),
                desk: !restricted && !clearance_runs_commands(&request.clearance),
                prompt: Some(briefing(
                    &request
                        .overlays
                        .with_routing(&request.initial_context, false),
                    restricted,
                    &request.prompt,
                )),
                resume: None,
            },
            terminal,
        )
    }

    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError> {
        let restricted = request.answer_only || request.assistant.is_some();
        self.run(
            Launch {
                environment: &request.environment,
                working_directory: &request.working_directory,
                model: request.model.as_ref(),
                effort: request.effort.as_ref(),
                restricted,
                assistant: request.assistant.as_ref(),
                desk: !restricted && !clearance_runs_commands(&request.clearance),
                prompt: request.message.clone(),
                resume: Some(request.session_id.to_string()),
            },
            terminal,
        )
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;

    fn handler(settings: &str) -> AcpHandler {
        AcpHandler::try_from(AcpConfig {
            settings_toml: settings.to_owned(),
        })
        .unwrap()
    }

    #[test]
    fn it_cannot_hold_a_session_to_a_plan_however_it_is_set() {
        for settings in [
            "command = \"grok\"
",
            "command = \"grok\"
unconfined_desk = true
",
        ] {
            let refused = handler(settings).read_only_refusal().unwrap();
            assert!(refused.is_some(), "{settings}");
        }
    }

    #[test]
    fn it_refuses_desk_unless_unconfined_and_keeps_its_limit_words() {
        let confined = handler("command = \"grok\"\n");
        assert!(
            confined
                .clearance_refusal("desk".to_owned())
                .unwrap()
                .is_some()
        );
        assert_eq!(
            confined.clearance_refusal("field".to_owned()).unwrap(),
            None
        );
        let unconfined = handler("command = \"grok\"\nunconfined_desk = true\n");
        assert_eq!(
            unconfined.clearance_refusal("desk".to_owned()).unwrap(),
            None
        );
        assert!(
            confined
                .settings()
                .unwrap()
                .limit_phrases
                .iter()
                .any(|phrase| phrase == "rate limited"),
            "the Grok CLI's words count by default"
        );
        let session = |text: &str| {
            confined
                .settings()
                .unwrap()
                .limit_phrases
                .iter()
                .any(|phrase| text.to_lowercase().contains(phrase))
        };
        assert!(
            session(
                "Internal error: API error (status 402 Payment Required): \
                 personal-team-blocked:spending-limit: You have run out of credits"
            ),
            "Grok out of credits is a limit"
        );
        let own = handler("command = \"x\"\nlimit_phrases = [\"out of credits\"]\n");
        assert_eq!(
            own.settings().unwrap().limit_phrases,
            ["out of credits"],
            "a setting replaces them"
        );
    }
}
