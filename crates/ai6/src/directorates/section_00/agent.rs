//! The Guatiao contract for launching and driving one agent Session.

use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, ToValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::directorates::m::identity::{MissionId, SessionId};
use crate::directorates::q::terminal::AgentTerminal;

/// The first instruction for one agent operation in a Mission.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
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
    /// Spend, in millionths of a dollar, after which the harness should stop on its own, when
    /// it can; AI6 enforces its budgets itself regardless.
    pub max_cost_micro_usd: Option<u64>,
    /// Whether the harness may load the settings its working directory carries (hooks, MCP
    /// servers, permissions), as AI6 decided from the Operation's harness policy. `false`
    /// keeps only the user's own settings and those AI6 passes itself.
    #[serde(default)]
    pub project_settings: bool,
}

/// How AI6 resumes a provider-issued Session.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
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
    /// Spend, in millionths of a dollar, after which the harness should stop on its own, when
    /// it can; AI6 enforces its budgets itself regardless.
    pub max_cost_micro_usd: Option<u64>,
    /// Whether the harness may load the settings its working directory carries, decided
    /// again on every resume.
    #[serde(default)]
    pub project_settings: bool,
}

/// A message AI6 sends to an already running Agent Session.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AgentMessage {
    /// Message text, kept separate from the initial context.
    pub text: String,
}

/// The event kind a handler emits when the provider refuses further work until a quota
/// resets. The event carries the details in [`AgentEvent::limit`].
pub const LIMIT_EVENT: &str = "limit";

/// A provider usage or rate limit that stopped the Session.
///
/// A handler emits it as an [`AgentEvent`] of kind [`LIMIT_EVENT`] in place of the
/// `completed` or `failed` event the same output would otherwise produce.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[map(crate = ::guatiao)]
pub struct UsageLimit {
    /// When the provider says the quota resets, as Unix seconds. `None` when it did not say.
    pub resets_at: Option<i64>,
    /// Which window was exhausted (`five_hour`, `weekly`, ...) when the provider names it.
    pub window: Option<String>,
}

/// Tokens and cost a provider reported for a Session, as the provider reports them.
///
/// Money is an integer count of millionths of a dollar so records never carry floats.
#[derive(
    ToValue, FromValue, Schema, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[map(crate = ::guatiao)]
pub struct TokenUsage {
    /// Prompt tokens that were not served from the cache.
    pub input: u64,
    /// Tokens the model generated.
    pub output: u64,
    /// Prompt tokens read from the provider's cache.
    pub cache_read: u64,
    /// Prompt tokens written to the provider's cache.
    pub cache_write: u64,
    /// Cost in millionths of a dollar; `0` when the provider reports no cost.
    pub cost_micro_usd: u64,
    /// Whether the figures are running totals since the provider process started (`true`)
    /// or only what the last step added (`false`).
    pub cumulative: bool,
}

/// How much of one provider quota window the Session's account has used, as the provider
/// reported it. A handler attaches one per window to the event that carried the report.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[map(crate = ::guatiao)]
pub struct QuotaReport {
    /// The window (`five_hour`, `weekly`, ...) as the provider names it, or its position
    /// (`primary`, `secondary`) when it gives no name.
    pub window: String,
    /// Thousandths of the window used, when the provider reports a figure.
    pub used_permille: Option<u16>,
    /// When the window resets, as Unix seconds, when the provider says.
    pub resets_at: Option<i64>,
    /// The provider's verdict for the window (`allowed`, `allowed_warning`, `rejected`), when
    /// it reports one.
    pub status: Option<String>,
}

/// One provider event returned to the daemon.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[map(crate = ::guatiao)]
pub struct AgentEvent {
    /// Provider-neutral event name such as `text`, `blocked`, `tool`, `completed` or `limit`.
    pub kind: String,
    /// Human-readable event text when the provider supplies it.
    pub text: Option<String>,
    /// Provider data that has no text representation.
    pub data: Option<Vec<u8>>,
    /// The limit that stopped the Session; set only on a `limit` event.
    pub limit: Option<UsageLimit>,
    /// Token usage and cost the provider reported with this event.
    pub usage: Option<TokenUsage>,
    /// Names of the tools the event calls, as the harness names them (`WebFetch`).
    pub tools: Vec<String>,
    /// Quota windows the provider reported with this event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quota: Vec<QuotaReport>,
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
