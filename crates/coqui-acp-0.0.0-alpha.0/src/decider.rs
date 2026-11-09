use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;
use serde_json::Value;

/// One way a permission request can be answered, as the agent offered it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionOption {
    /// The agent's identifier of the option.
    pub option_id: String,
    /// `allow_once`, `allow_always`, `reject_once`, `reject_always` or `other`.
    pub kind: String,
}

/// An agent asking leave to run a tool.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PermissionRequest {
    /// The session it belongs to.
    pub session_id: String,
    /// The agent's identifier of the tool call.
    pub tool_call_id: String,
    /// The tool's title.
    pub title: Option<String>,
    /// The tool kind.
    pub kind: Option<String>,
    /// The tool's input as the agent sent it. Untrusted: it is what the agent wants to do.
    pub raw_input: Option<Value>,
    /// What the agent lets the client answer.
    pub options: Vec<PermissionOption>,
}

/// The answer to a [`PermissionRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decision {
    /// Let the tool run, once.
    Allow,
    /// Refuse it, for this reason.
    Deny(String),
}

/// Judges an agent's permission requests. A decider may block, for instance to ask a person: it runs on
/// a thread of its own. A session with none refuses everything.
pub trait Decider: Send + Sync {
    /// Decides one request.
    fn decide(&self, request: &PermissionRequest) -> Decision;
}

/// Refuses every request.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefuseAll;

impl Decider for RefuseAll {
    fn decide(&self, _request: &PermissionRequest) -> Decision {
        Decision::Deny("no permission decider is configured".to_owned())
    }
}
