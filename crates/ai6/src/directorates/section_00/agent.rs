//! The Guatiao contract for launching and driving one agent Session.

use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, ToValue};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::directorates::m::identity::{MissionId, SessionId};
use crate::directorates::q::terminal::AgentTerminal;

/// The first instruction for one agent operation in a Mission.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AgentStart {
    /// AI6's identifier for the unit of work; a Mission may have several Sessions.
    pub mission_id: MissionId,
    /// The configured Agent codename.
    pub agent_id: String,
    /// The task prompt sent to the agent.
    pub prompt: String,
    /// Context to establish before the task prompt is sent.
    pub initial_context: Vec<String>,
    /// The Operation checkout or Mission safe house.
    pub working_directory: String,
    /// The clearance selected by AI6; the handler maps it to harness policy.
    pub clearance: String,
    /// Environment values issued for this provider operation.
    pub environment: BTreeMap<String, String>,
    /// The requested model, when the Agent roster specifies one.
    pub model: Option<String>,
    /// The requested reasoning effort, when supported by the handler.
    pub effort: Option<String>,
}

/// How AI6 resumes a provider-issued Session.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AgentResume {
    /// The Mission this Session is working on.
    pub mission_id: MissionId,
    /// The provider-issued identity of the operation to resume.
    pub session_id: SessionId,
    /// A new message to submit after resuming, if any.
    pub message: Option<String>,
    /// The Mission checkout or safe house containing the saved Session.
    pub working_directory: String,
    /// Environment values issued for the resumed provider operation.
    pub environment: BTreeMap<String, String>,
    /// The requested model when the harness supports changing it on resume.
    pub model: Option<String>,
    /// The requested reasoning effort when the harness supports it on resume.
    pub effort: Option<String>,
}

/// A message AI6 sends to an already running Agent Session.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AgentMessage {
    /// Message text, kept separate from the initial context.
    pub text: String,
}

/// One provider event returned to the daemon.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Serialize)]
#[map(crate = ::guatiao)]
pub struct AgentEvent {
    /// Provider-neutral event name such as `text`, `blocked`, `tool` or `completed`.
    pub kind: String,
    /// Human-readable event text when the provider supplies it.
    pub text: Option<String>,
    /// Provider data that has no text representation.
    pub data: Option<Vec<u8>>,
}

/// Starts or resumes the configured Agent implementation.
#[guatiao::kind(name = "ai6_agent_handler")]
pub trait AgentHandler: Send + Sync {
    /// Starts an Agent Session with context established before its task prompt.
    ///
    /// `terminal` is an AI6-owned object bridge over its selected terminal provider; the
    /// handler selects its command and protocol while the provider owns process lifetime.
    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError>;

    /// Reattaches to the same provider-issued Session ID after an interruption.
    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError>;
}

/// One provider-owned, resumable agent operation.
#[guatiao::kind(object, name = "ai6_agent_session")]
pub trait AgentSession: Send + Sync {
    /// The provider-issued Session ID, once the provider has created it.
    fn session_id(&self) -> Result<Option<SessionId>, ProviderError>;

    /// Submits a message to the active operation.
    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError>;

    /// Reads one event, or returns `None` when `timeout_ms` expires.
    ///
    /// A handler must report terminal process exit as a `completed` or `failed` event;
    /// returning `None` permanently after end-of-file leaves the daemon waiting forever.
    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError>;

    /// Requests that the provider interrupt the operation.
    fn interrupt(&self) -> Result<(), ProviderError>;

    /// Closes the operation and releases provider resources.
    fn close(&self) -> Result<(), ProviderError>;
}
