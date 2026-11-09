#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

mod facade;
mod kind;

pub use agent_client_protocol as sdk;
pub use facade::{
    AcpFacade, Credential, EVENT_NOTIFICATION, EVENTS_CAPABILITY, Error, FacadeAuth,
    LAUNCH_META_KEY, OptionTarget, OptionValue, SESSION_OPTIONS_CAPABILITY, SESSION_OPTIONS_METHOD,
    Settings, StartOption,
};
pub use guatiao;
pub use kind::{
    AgentEvent, AgentMessage, AgentProvider, AgentProviderVtable, AgentSession, AgentSessionVtable,
    BLOCKED_EVENT, CANCELLED_EVENT, COMPLETED_EVENT, DENIED_EVENT, FAILED_EVENT, LIMIT_EVENT,
    QuotaReport, ResumeRequest, StartRequest, TEXT_EVENT, TOOL_EVENT, TURN_CANCELLED, TokenUsage,
    UsageLimit,
};
