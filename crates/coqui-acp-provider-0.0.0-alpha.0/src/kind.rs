//! The contract a provider serves: [`AgentProvider`] starts or resumes an [`AgentSession`], and a session
//! reports what happens as [`AgentEvent`]s.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use coqui_acp_api::{LimitHit, UsageReport};
use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, Status, ToValue, Value};
use serde::{Deserialize, Serialize};

/// Agent text, in [`AgentEvent::text`]. Text of one message may arrive in any number of events.
pub const TEXT_EVENT: &str = "text";

/// The agent used a tool. [`AgentEvent::text`] is its title for people, [`AgentEvent::tools`] its name.
pub const TOOL_EVENT: &str = "tool";

/// The agent stopped short of what it was asked, and [`AgentEvent::text`] says why: a permission was
/// refused, say. The turn goes on unless the text is [`TURN_CANCELLED`].
pub const BLOCKED_EVENT: &str = "blocked";

/// The turn finished, and the agent can take another message. Also how a harness that exited cleanly is
/// reported.
pub const COMPLETED_EVENT: &str = "completed";

/// The turn failed. [`AgentEvent::text`] says why, and for a harness that exited, how it ended and what it
/// wrote last.
pub const FAILED_EVENT: &str = "failed";

/// The turn stopped because [`AgentSession::interrupt`] was called or the agent gave it up. An addition to
/// `blocked` with [`TURN_CANCELLED`], which means the same and stays for consumers that match it.
pub const CANCELLED_EVENT: &str = "cancelled";

/// The provider refuses further work until a quota resets. Emitted in place of the `completed` or `failed`
/// event the same output would otherwise produce, with the details in [`AgentEvent::limit`].
pub const LIMIT_EVENT: &str = "limit";

/// The harness refused calls in this turn by its permission rules. Emitted before the turn's `completed` or
/// `failed` event. The event's text names the tools, separated by `, `; [`AgentEvent::tools`] stays empty,
/// since these calls did not run.
pub const DENIED_EVENT: &str = "denied";

/// The text of the `blocked` event for a turn the agent itself ended as cancelled, as the Grok CLI does when
/// one of its tools is refused. No `completed` event follows it.
pub const TURN_CANCELLED: &str = "the agent cancelled the turn";

/// A message for a running session.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AgentMessage {
    /// Message text, kept separate from the initial context.
    pub text: String,
}

/// One thing that happened in a session.
///
/// Consumers match [`kind`](Self::kind) as text, so it is an open vocabulary: the constants of this crate
/// name the part with a fixed meaning, and a consumer passes over a kind it does not know.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[map(crate = ::guatiao)]
pub struct AgentEvent {
    /// The event's name: [`TEXT_EVENT`], [`TOOL_EVENT`], [`COMPLETED_EVENT`] and the others of this crate.
    pub kind: String,
    /// Human-readable event text when the provider supplies it.
    pub text: Option<String>,
    /// Provider data that has no text representation.
    pub data: Option<Vec<u8>>,
    /// The limit that stopped the session; set only on a `limit` event.
    pub limit: Option<LimitHit>,
    /// Names of the tools the event calls, as the harness names them (`WebFetch`).
    #[map(default)]
    pub tools: Vec<String>,
    /// What the provider reported of the account's usage with this event: its buckets, and the
    /// session's spend. A reader keeps the newest by `revision` and reads each bucket's `observed_at`.
    #[serde(default)]
    pub report: Option<UsageReport>,
}

impl AgentEvent {
    /// An event of `kind` with `text`, and nothing else.
    #[must_use]
    pub fn new(kind: impl Into<String>, text: Option<String>) -> Self {
        Self {
            kind: kind.into(),
            text,
            data: None,
            limit: None,
            tools: Vec::new(),
            report: None,
        }
    }
}

/// What a provider starts a session from.
#[derive(ToValue, FromValue, Schema, Clone, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct StartRequest {
    /// The absolute directory the agent works in.
    pub working_directory: String,
    /// The first message.
    pub prompt: String,
    /// Context to establish before the prompt is sent.
    #[map(default)]
    pub context: Vec<String>,
    /// Environment values for what the provider runs. They may hold credentials, so a provider logs and
    /// reports none of them.
    #[map(default)]
    #[schema(sensitive)]
    pub environment: BTreeMap<String, String>,
    /// The requested model, when one was asked for.
    pub model: Option<String>,
    /// The requested reasoning effort, when one was asked for.
    pub effort: Option<String>,
    /// Anything else, for the provider alone: nothing between the client and the provider reads it, so a
    /// provider treats it as untrusted input.
    pub meta: Option<Value>,
}

impl fmt::Debug for StartRequest {
    /// Names the environment and shows none of its values, and leaves out what only the provider reads.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartRequest")
            .field("working_directory", &self.working_directory)
            .field("prompt", &self.prompt)
            .field("context", &self.context)
            .field("environment", &self.environment.keys())
            .field("model", &self.model)
            .field("effort", &self.effort)
            .finish_non_exhaustive()
    }
}

impl StartRequest {
    /// A request with nothing but its working directory and first message.
    #[must_use]
    pub fn new(working_directory: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            working_directory: working_directory.into(),
            prompt: prompt.into(),
            context: Vec::new(),
            environment: BTreeMap::new(),
            model: None,
            effort: None,
            meta: None,
        }
    }
}

/// What a provider resumes a session from.
#[derive(ToValue, FromValue, Schema, Clone, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct ResumeRequest {
    /// The identity [`AgentSession::session_id`] issued. It may come from outside, so a provider checks it.
    pub session_id: String,
    /// The absolute directory the saved session works in.
    pub working_directory: String,
    /// A message to submit after resuming, if any.
    pub prompt: Option<String>,
    /// The standing part of [`StartRequest::context`], for a harness that may not keep its start-up
    /// context in the session it resumes; never a recap of the conversation the session already holds.
    #[map(default)]
    pub context: Vec<String>,
    /// As [`StartRequest::environment`].
    #[map(default)]
    #[schema(sensitive)]
    pub environment: BTreeMap<String, String>,
    /// The requested model, when the harness can change it on resume.
    pub model: Option<String>,
    /// The requested reasoning effort, when the harness can change it on resume.
    pub effort: Option<String>,
    /// As [`StartRequest::meta`].
    pub meta: Option<Value>,
}

impl fmt::Debug for ResumeRequest {
    /// Names the environment and shows none of its values, and leaves out what only the provider reads.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResumeRequest")
            .field("session_id", &self.session_id)
            .field("working_directory", &self.working_directory)
            .field("prompt", &self.prompt)
            .field("context", &self.context)
            .field("environment", &self.environment.keys())
            .field("model", &self.model)
            .field("effort", &self.effort)
            .finish_non_exhaustive()
    }
}

impl ResumeRequest {
    /// A request with nothing but the session and its working directory.
    #[must_use]
    pub fn new(session_id: impl Into<String>, working_directory: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            working_directory: working_directory.into(),
            prompt: None,
            context: Vec::new(),
            environment: BTreeMap::new(),
            model: None,
            effort: None,
            meta: None,
        }
    }
}

/// Starts or resumes agent sessions. A provider owns whatever process it runs.
#[guatiao::kind(name = "agent_provider")]
pub trait AgentProvider: Send + Sync {
    /// Starts a session with context established before its first message.
    ///
    /// It may block until the harness runs; the caller gives it a thread and a time limit.
    fn start(&self, request: StartRequest) -> Result<Object<dyn AgentSession>, ProviderError>;

    /// Reattaches to a session this provider issued, as long as it still exists. The default: this
    /// provider resumes nothing.
    fn resume(&self, request: ResumeRequest) -> Result<Object<dyn AgentSession>, ProviderError> {
        drop(request);
        Err(ProviderError::new(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this provider resumes no session",
        ))
    }
}

/// One provider-owned, resumable agent operation.
///
/// Every method may be called while another runs, from another thread: `next_event` blocks for its
/// timeout, and `interrupt` and `close` have to reach it.
#[guatiao::kind(object, name = "agent_session")]
pub trait AgentSession: Send + Sync {
    /// The provider-issued session identity, once the provider has one.
    fn session_id(&self) -> Result<Option<String>, ProviderError>;

    /// Submits a message to the running operation.
    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError>;

    /// Reads one event, or returns `None` when `timeout_ms` expires.
    ///
    /// A harness's exit must be reported as a `completed` or `failed` event, the `failed` text saying how
    /// it ended and what it wrote last: a provider that only returns `None` after end-of-file leaves its
    /// consumer waiting forever. A method that takes much longer than `timeout_ms` to return counts as
    /// hung.
    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError>;

    /// Asks the provider to interrupt the running turn, which then ends with an event as any turn does.
    fn interrupt(&self) -> Result<(), ProviderError>;

    /// Closes the operation and releases what it holds. It returns once the harness has exited, or after a
    /// short wait when that cannot be told, since the caller may remove the working directory next.
    fn close(&self) -> Result<(), ProviderError>;
}
