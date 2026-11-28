use alloc::string::String;
use serde_json::Value;

/// A command the agent advertises with `available_commands_update`, such as a slash command.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AdvertisedCommand {
    /// The command's name, without a leading slash.
    pub name: String,
    /// What the command does, in the agent's words.
    pub description: String,
    /// The hint the agent gives for the text typed after the name, when the command takes any.
    pub input_hint: Option<String>,
}

impl AdvertisedCommand {
    /// A command with no input hint.
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_hint: None,
        }
    }

    /// Sets the hint for the text typed after the name.
    #[must_use]
    pub fn input_hint(mut self, hint: impl Into<String>) -> Self {
        self.input_hint = Some(hint.into());
        self
    }
}

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
    /// The agent's list of commands, in full: each `available_commands_update` replaces the one before.
    Commands(Vec<AdvertisedCommand>),
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
    /// The agent stated the limit that refused the turn, in the standard form: the `LimitHit` (as the JSON object the agent sent,
    /// which `coqui_acp_api::LimitHit` reads) of the answer's `_meta` under [`LIMIT_META_KEY`](crate::LIMIT_META_KEY), or of the
    /// `_meta` of a refused request's error data. It comes just before the [`Event::Failed`] (or [`Event::PromptFailed`]) the
    /// refusal also is, which keeps its text, so a host reads the kind, the reset and whether another account can go on from
    /// this and never from the words.
    Limited(Value),
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
