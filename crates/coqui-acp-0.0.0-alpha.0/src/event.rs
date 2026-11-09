use alloc::string::String;
use serde_json::Value;

/// What happened on a session, whichever protocol version carried it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// Agent text, gathered up to the next tool call or the end of the turn.
    Text(String),
    /// A tool call began.
    Tool {
        /// The agent's identifier of the call.
        id: String,
        /// The human title.
        title: String,
        /// The tool's name, when the agent gave one.
        name: Option<String>,
        /// `read`, `edit`, `delete`, `move`, `search`, `execute`, `think`, `fetch`, `switch_mode` or `other`.
        kind: String,
    },
    /// The cost the agent reports so far. ACP carries no token counts.
    Cost {
        /// The amount, in `currency`.
        amount: f64,
        /// The ISO 4217 code the agent gave.
        currency: String,
    },
    /// Something worth telling a person that ends nothing.
    Notice(String),
    /// A permission request was answered.
    Permission {
        /// The tool's title.
        tool: String,
        /// Whether it may run.
        allowed: bool,
        /// Why it was refused.
        reason: Option<String>,
    },
    /// The turn finished: `end_turn`, `max_tokens` or `max_turn_requests`.
    Completed {
        /// The agent's stop reason.
        stop: String,
    },
    /// The turn failed, the agent refused, or the connection was lost.
    Failed(String),
    /// The agent ended the turn as cancelled, whether asked to or by itself.
    Cancelled,
    /// A permission request or a question could not be answered, so the agent was told it was cancelled. The turn goes on until
    /// the agent ends it.
    Blocked(String),
    /// A `session/prompt` failed while an extension owns turn endings ([`turn_end_by`]). The extension
    /// may have ended the turn already, so the caller judges this.
    ///
    /// [`turn_end_by`]: crate::Builder::turn_end_by
    PromptFailed(String),
    /// Output of the agent's was dropped before it reached this connection: `_dev.boriken.coqui.acp/output_lost`, which a bridge that
    /// kept the agent while nobody was connected sends first on the next connection. What the session shows has a gap there.
    OutputLost {
        /// Messages dropped.
        lines: u64,
        /// Their bytes.
        bytes: u64,
    },
    /// A notification whose method starts with `_`, other than [`Event::OutputLost`]: an extension, passed on untouched.
    Extension {
        /// The method.
        method: String,
        /// Its parameters.
        params: Value,
    },
}
