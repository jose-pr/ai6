//! The AI6 launch extension: what ACP has no field for, carried in `_meta`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

use crate::directorates::m::identity::{MissionId, SessionId};
use crate::directorates::section_00::agent::{AgentResume, AgentStart, AssistantMode};

/// The handler provider a field runner starts, by id and Guatiao configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HandlerSpec {
    /// The provider id (`claude`).
    pub id: String,
    /// Its configuration; `null` for the defaults.
    #[serde(default)]
    pub config: Value,
}

impl HandlerSpec {
    /// A handler by provider id and configuration.
    pub fn new(id: impl Into<String>, config: Value) -> Self {
        Self {
            id: id.into(),
            config,
        }
    }
}

/// Every [`AgentStart`] field ACP has no home for. The working directory is ACP's `cwd` and
/// the task prompt is `session/prompt`, so neither is here.
///
/// `environment` holds secrets: its `Debug` names the variables and never shows a value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LaunchMeta {
    /// AI6's identifier for the unit of work.
    pub mission_id: MissionId,
    /// The configured Agent codename.
    pub agent_id: String,
    /// Context to establish before the task prompt.
    pub initial_context: Vec<String>,
    /// The clearance AI6 selected.
    pub clearance: String,
    /// Environment issued for this operation.
    pub environment: BTreeMap<String, String>,
    /// The requested model.
    pub model: Option<String>,
    /// The requested reasoning effort.
    pub effort: Option<String>,
    /// Spend, in millionths of a dollar, after which the harness should stop.
    pub max_cost_micro_usd: Option<u64>,
    /// Whether the harness may load its working directory's settings.
    #[serde(default)]
    pub project_settings: bool,
    /// Whether it may connect the MCP servers its working directory names.
    #[serde(default)]
    pub project_mcp: bool,
    /// Whether the Session only reads and answers.
    #[serde(default)]
    pub answer_only: bool,
    /// The assistant mode.
    #[serde(default)]
    pub assistant: Option<AssistantMode>,
    /// The handler a field runner starts; absent where the façade serves one handler.
    #[serde(default)]
    pub handler: Option<HandlerSpec>,
    /// The terminal provider the Station pinned.
    #[serde(default)]
    pub terminal: Option<String>,
    /// The name the Station gave the Session on a field runner; it is the ACP session ID there.
    #[serde(default)]
    pub key: Option<String>,
}

impl LaunchMeta {
    /// The part of `start` that ACP does not carry itself.
    pub fn from_start(start: &AgentStart) -> Self {
        Self {
            mission_id: start.mission_id.clone(),
            agent_id: start.agent_id.clone(),
            initial_context: start.initial_context.clone(),
            clearance: start.clearance.clone(),
            environment: start.environment.clone(),
            model: start.model.clone(),
            effort: start.effort.clone(),
            max_cost_micro_usd: start.max_cost_micro_usd,
            project_settings: start.project_settings,
            project_mcp: start.project_mcp,
            answer_only: start.answer_only,
            assistant: start.assistant.clone(),
            handler: None,
            terminal: None,
            key: None,
        }
    }

    /// Names the handler provider a field runner starts.
    #[must_use]
    pub fn with_handler(mut self, handler: HandlerSpec) -> Self {
        self.handler = Some(handler);
        self
    }

    /// Pins the terminal provider.
    #[must_use]
    pub fn with_terminal(mut self, terminal: Option<String>) -> Self {
        self.terminal = terminal;
        self
    }

    /// Names the Session on a field runner.
    #[must_use]
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// The [`AgentStart`] this launch and ACP's `cwd` and prompt describe.
    pub fn into_start(self, prompt: String, working_directory: String) -> AgentStart {
        AgentStart {
            mission_id: self.mission_id,
            agent_id: self.agent_id,
            prompt,
            initial_context: self.initial_context,
            working_directory,
            clearance: self.clearance,
            environment: self.environment,
            model: self.model,
            effort: self.effort,
            max_cost_micro_usd: self.max_cost_micro_usd,
            project_settings: self.project_settings,
            project_mcp: self.project_mcp,
            answer_only: self.answer_only,
            assistant: self.assistant,
        }
    }

    /// This launch as the JSON value of `_meta[LAUNCH_META_KEY]`.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

impl fmt::Debug for LaunchMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchMeta")
            .field("mission_id", &self.mission_id)
            .field("agent_id", &self.agent_id)
            .field("clearance", &self.clearance)
            .field("environment", &self.environment.keys().collect::<Vec<_>>())
            .field("model", &self.model)
            .field("effort", &self.effort)
            .field("answer_only", &self.answer_only)
            .field("handler", &self.handler.as_ref().map(|handler| &handler.id))
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

/// Every [`AgentResume`] field ACP has no home for; the working directory is `cwd` and the
/// provider Session ID is the ACP session ID.
///
/// A `message` here is submitted by the handler as it resumes; a client that sends it as
/// `session/prompt` instead leaves it `None`. `environment` is never shown by `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ResumeMeta {
    /// The Mission the Session is working on.
    pub mission_id: MissionId,
    /// The provider-issued Session ID to resume.
    pub session_id: SessionId,
    /// A message to submit after resuming.
    pub message: Option<String>,
    /// Environment issued for the resumed operation.
    pub environment: BTreeMap<String, String>,
    /// The requested model.
    pub model: Option<String>,
    /// The requested reasoning effort.
    pub effort: Option<String>,
    /// Spend, in millionths of a dollar, after which the harness should stop.
    pub max_cost_micro_usd: Option<u64>,
    /// Whether the harness may load its working directory's settings.
    #[serde(default)]
    pub project_settings: bool,
    /// Whether it may connect the MCP servers its working directory names.
    #[serde(default)]
    pub project_mcp: bool,
    /// Whether the Session only reads and answers.
    #[serde(default)]
    pub answer_only: bool,
    /// The assistant mode.
    #[serde(default)]
    pub assistant: Option<AssistantMode>,
    /// The standing part of the start-up context.
    #[serde(default)]
    pub initial_context: Vec<String>,
    /// The handler a field runner starts.
    #[serde(default)]
    pub handler: Option<HandlerSpec>,
    /// The terminal provider the Station pinned.
    #[serde(default)]
    pub terminal: Option<String>,
    /// The name the Station gave the Session on a field runner.
    #[serde(default)]
    pub key: Option<String>,
}

impl ResumeMeta {
    /// The part of `resume` that ACP does not carry itself.
    pub fn from_resume(resume: &AgentResume) -> Self {
        Self {
            mission_id: resume.mission_id.clone(),
            session_id: resume.session_id.clone(),
            message: resume.message.clone(),
            environment: resume.environment.clone(),
            model: resume.model.clone(),
            effort: resume.effort.clone(),
            max_cost_micro_usd: resume.max_cost_micro_usd,
            project_settings: resume.project_settings,
            project_mcp: resume.project_mcp,
            answer_only: resume.answer_only,
            assistant: resume.assistant.clone(),
            initial_context: resume.initial_context.clone(),
            handler: None,
            terminal: None,
            key: None,
        }
    }

    /// Names the handler provider a field runner starts.
    #[must_use]
    pub fn with_handler(mut self, handler: HandlerSpec) -> Self {
        self.handler = Some(handler);
        self
    }

    /// Pins the terminal provider.
    #[must_use]
    pub fn with_terminal(mut self, terminal: Option<String>) -> Self {
        self.terminal = terminal;
        self
    }

    /// Names the Session on a field runner.
    #[must_use]
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// The [`AgentResume`] this resume and ACP's `cwd` describe.
    pub fn into_resume(self, working_directory: String) -> AgentResume {
        AgentResume {
            mission_id: self.mission_id,
            session_id: self.session_id,
            message: self.message,
            working_directory,
            environment: self.environment,
            model: self.model,
            effort: self.effort,
            max_cost_micro_usd: self.max_cost_micro_usd,
            project_settings: self.project_settings,
            project_mcp: self.project_mcp,
            answer_only: self.answer_only,
            assistant: self.assistant,
            initial_context: self.initial_context,
        }
    }

    /// This resume as the JSON value of `_meta[LAUNCH_META_KEY]`.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

impl fmt::Debug for ResumeMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResumeMeta")
            .field("mission_id", &self.mission_id)
            .field("session_id", &self.session_id)
            .field("environment", &self.environment.keys().collect::<Vec<_>>())
            .field("model", &self.model)
            .field("effort", &self.effort)
            .field("answer_only", &self.answer_only)
            .field("handler", &self.handler.as_ref().map(|handler| &handler.id))
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}
