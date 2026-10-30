//! The `acp` AgentHandler: any Agent Client Protocol agent runs as an Agent's handler, with
//! AI6's environment, accounts and green-ink gate.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

ai6::guatiao::providers!(AcpHandler);

use std::collections::BTreeMap;
use std::sync::Arc;

use ai6::HarnessStdio;
use ai6::directorates::q::terminal::{AgentTerminal, STREAM_COLS, Spec, TerminalSession};
use ai6::directorates::section_00::acp::{
    AcpOptions, AcpSession, GateDecider, TerminalIo, lines_over_io,
};
use ai6::directorates::section_00::agent::{
    AgentHandler, AgentResume, AgentSession, AgentStart, AssistantMode,
};
use ai6::directorates::section_00::gate::{CABLE_TOKEN_ENV, CABLE_URL_ENV};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
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
    /// How often, in milliseconds, a silent agent's output is checked for a stop request.
    #[serde(default = "poll")]
    pub poll_ms: i64,
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
fn poll() -> i64 {
    200
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
        let (lines, diagnostics) = lines_over_io(io.clone(), io.clone(), settings.diagnostic_lines);
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
                let said = diagnostics
                    .lock()
                    .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join(" | "))
                    .unwrap_or_default();
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

impl AgentHandler for AcpHandler {
    /// An ACP agent speaks JSON-RPC on plain pipes.
    fn stdio(&self) -> Result<HarnessStdio, ProviderError> {
        Ok(HarnessStdio::Json)
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
                prompt: Some(briefing(
                    &request.initial_context,
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
                prompt: request.message.clone(),
                resume: Some(request.session_id.to_string()),
            },
            terminal,
        )
    }
}
