//! The Agent Client Protocol (ACP) wire AI6 drives agent Sessions over.
//!
//! [`AcpSession`] is an ACP client that implements [`AgentSession`]: the Station and the `acp`
//! handler provider use it to run any ACP agent. [`AcpFacade`] is the agent side of the same
//! protocol serving any [`AgentHandler`]: the native Claude and Codex providers keep their
//! restricted modes, hooks and limit detection, and reach the client over ACP. The two meet in
//! one process over [`duplex`], or across a process boundary over any newline-delimited stream
//! ([`lines_over_io`]).
//!
//! ACP has no field for a process environment, an Agent's model and effort, or AI6's policy
//! flags, so the client sends them in `_meta[`[`LAUNCH_META_KEY`]`]` ([`LaunchMeta`],
//! [`ResumeMeta`]) and the façade also sends every [`AgentEvent`] verbatim as an
//! [`EVENT_NOTIFICATION`] notification, which an agent advertises with [`EVENTS_CAPABILITY`].
//! An agent that does not is driven by the standard `session/update` stream alone.
//!
//! [`AgentSession`]: super::agent::AgentSession
//! [`AgentHandler`]: super::agent::AgentHandler
//! [`AgentEvent`]: super::agent::AgentEvent

mod client;
mod facade;
#[cfg(feature = "gate")]
mod gate_decider;
mod lines;
mod meta;

pub use client::{
    AcpOptions, AcpSession, Authenticate, PermissionDecider, PermissionDecision, PermissionOption,
    PermissionRequest, RefuseAll, RpcFailure, exchange,
};
pub use facade::{AcpFacade, Credential, FacadeAuth, TerminalFactory};
#[cfg(feature = "gate")]
pub use gate_decider::GateDecider;
pub use lines::{IoLines, TerminalIo, lines_over_io};
pub use meta::{HandlerSpec, LaunchMeta, ResumeMeta};

/// The `_meta` key of `session/new`, `session/load` and `session/resume` that carries
/// [`LaunchMeta`] or [`ResumeMeta`].
pub const LAUNCH_META_KEY: &str = "dev.coqui.ai6/launch";

/// The `agentCapabilities._meta` key with which an agent says it sends [`EVENT_NOTIFICATION`].
pub const EVENTS_CAPABILITY: &str = "dev.coqui.ai6/events";

/// The notification carrying one [`AgentEvent`](super::agent::AgentEvent) verbatim, beside the
/// provider Session ID: `{"sessionId", "providerSessionId", "event"}`; no `event` marks the
/// start of the operation.
pub const EVENT_NOTIFICATION: &str = "_ai6/event";

/// A timeout that never fires: what a caller sets where a limit would be new behaviour.
pub const UNBOUNDED_MS: u64 = u64::MAX;

/// The authentication method a field runner advertises: its bearer token, in
/// `authenticate._meta.token`.
pub const RUNNER_TOKEN_METHOD: &str = "ai6-runner-token";

pub use agent_client_protocol::Channel;

/// An in-memory pair of connected transports: serve one end with
/// [`AcpFacade::serve`] and connect an [`AcpSession`] over the other, with no extra process.
pub fn duplex() -> (Channel, Channel) {
    Channel::duplex()
}

/// Tool kinds that change something or run something: what a restricted Session refuses.
pub(crate) const RESTRICTED_KINDS: [&str; 4] = ["edit", "delete", "move", "execute"];
