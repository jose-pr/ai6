//! The assistant Session contract between Universal Exports and a Station.
//!
//! Moneypenny, the operator's assistant, runs as an Agent Session on a Station through the
//! same AgentHandler providers as every Agent, started with [`AgentStart::assistant`]. Its
//! tools are not on the Station: Universal Exports serves them on an MCP endpoint of its own,
//! authenticated by a token bound to the person and the conversation, and every tool runs as
//! that person. The Station takes [`AssistantJob`]s from Universal Exports (one per message
//! the person sends), hands the harness the endpoint's URL and token in its environment, and
//! reports what the harness says as [`AssistantReport`]s.
//!
//! [`AgentStart::assistant`]: super::agent::AgentStart::assistant

use std::fmt;

use serde::{Deserialize, Serialize};

use super::agent::{AssistantMode, AssistantNetwork, McpServerRef};

/// The prefix of the environment variables holding the assistant's MCP endpoint
/// (`AI6_MONEYPENNY_MCP_URL`, `AI6_MONEYPENNY_MCP_TOKEN`).
pub const ASSISTANT_MCP_ENV_PREFIX: &str = "AI6_MONEYPENNY_MCP";
/// The environment variable holding the URL of the assistant's MCP endpoint.
pub const ASSISTANT_MCP_URL_ENV: &str = "AI6_MONEYPENNY_MCP_URL";
/// The environment variable holding the bearer token of the assistant's MCP endpoint.
pub const ASSISTANT_MCP_TOKEN_ENV: &str = "AI6_MONEYPENNY_MCP_TOKEN";
/// The name the harness knows the endpoint by: Claude Code calls its tools
/// `mcp__moneypenny__<tool>`.
pub const ASSISTANT_MCP_SERVER: &str = "moneypenny";

impl AssistantMode {
    /// The mode of a Moneypenny Session that has only her own tools: the Universal Exports
    /// endpoint, no interpreter and no skill.
    pub fn moneypenny() -> Self {
        Self {
            mcp_servers: vec![McpServerRef {
                name: ASSISTANT_MCP_SERVER.to_owned(),
                env_prefix: ASSISTANT_MCP_ENV_PREFIX.to_owned(),
            }],
            interpreters: Vec::new(),
            skills: Vec::new(),
            network: AssistantNetwork::default(),
        }
    }

    /// Checks the parts: at least one server; server names of letters, digits, `-` and `_`;
    /// environment prefixes of capitals, digits and `_`; interpreter and skill names of
    /// letters, digits, `.`, `-` and `_`. A handler checks this before building settings
    /// from the names.
    ///
    /// # Errors
    ///
    /// A message naming the first part out of bounds.
    pub fn validate(&self) -> Result<(), String> {
        let word = |value: &str, extra: &[char]| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
        };
        if self.mcp_servers.is_empty() {
            return Err("an assistant needs at least one MCP server".to_owned());
        }
        for server in &self.mcp_servers {
            if !word(&server.name, &['-', '_']) {
                return Err(format!("MCP server name {:?} is not allowed", server.name));
            }
            if !word(&server.env_prefix, &['_'])
                || server.env_prefix.chars().any(|c| c.is_ascii_lowercase())
            {
                return Err(format!(
                    "MCP environment prefix {:?} is not allowed",
                    server.env_prefix
                ));
            }
        }
        for name in self.interpreters.iter().chain(&self.skills) {
            if !word(name, &['.', '-', '_']) {
                return Err(format!(
                    "{name:?} is not an allowed interpreter or skill name"
                ));
            }
        }
        if !matches!(self.network.policy.as_str(), "limited" | "denied" | "open") {
            return Err(format!(
                "network policy {:?} is not limited, denied or open",
                self.network.policy
            ));
        }
        for host in &self.network.allowed_hosts {
            if !word(host, &['.', '-', '*']) || host.len() > 253 {
                return Err(format!("{host:?} is not a host name"));
            }
        }
        Ok(())
    }
}
/// The endpoint's path on Universal Exports, relative to its origin.
pub const ASSISTANT_MCP_PATH: &str = "v1/moneypenny/mcp";
/// The longest message a job carries, in bytes.
pub const MAX_ASSISTANT_MESSAGE_BYTES: usize = 32 * 1024;
/// The most initial context a job carries, in bytes, all items together.
pub const MAX_ASSISTANT_CONTEXT_BYTES: usize = 24 * 1024;
/// The longest text one reported event carries, in bytes; longer text is cut.
pub const MAX_ASSISTANT_EVENT_BYTES: usize = 64 * 1024;
/// The most events one report carries.
pub const MAX_ASSISTANT_EVENTS: usize = 64;

/// A Station took the job: the harness has the message. `text` is unset.
pub const EVENT_ACCEPTED: &str = "accepted";
/// A Station refused the job; `text` says why, in AI6's words. Universal Exports may offer
/// the job to another Station.
pub const EVENT_REJECTED: &str = "rejected";
/// Text the assistant wrote.
pub const EVENT_TEXT: &str = "text";
/// The assistant called tools, named in [`AssistantEvent::tools`].
pub const EVENT_TOOL: &str = "tool";
/// The assistant finished answering the message; `text` is its final answer when the
/// harness reports one.
pub const EVENT_COMPLETED: &str = "completed";
/// The harness stopped before finishing; `text` says why when it can.
pub const EVENT_FAILED: &str = "failed";
/// The model's provider refused more work until a quota resets.
pub const EVENT_LIMIT: &str = "limit";
/// The Station closed the conversation's harness (idle, closed by request, or stopping). The
/// provider Session can be resumed by a later job.
pub const EVENT_CLOSED: &str = "closed";

/// One message for the assistant of one conversation, queued for a Station.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantJob {
    /// Universal Exports' identity for the job.
    pub job_id: String,
    /// The conversation the message belongs to; the Station keeps one harness per
    /// conversation.
    pub conversation_id: String,
    /// The roster Agent that is the assistant; its handler, model, effort and environment
    /// apply as for any Agent.
    pub agent_id: String,
    /// What the person wrote, or what the app reports on the person's behalf.
    pub message: String,
    /// Context a harness started for this job is given before the message, and a resumed one
    /// again where the harness does not keep it ([`AgentResume::initial_context`]); unused
    /// when the conversation's harness is running.
    ///
    /// [`AgentResume::initial_context`]: crate::directorates::section_00::agent::AgentResume::initial_context
    #[serde(default)]
    pub context: Vec<String>,
    /// The bearer token of the conversation's MCP endpoint, bound to the person and the
    /// conversation. Never logged.
    pub mcp_token: String,
    /// When `mcp_token` stops being accepted, as Unix seconds. A harness started with an older
    /// token is restarted (resumed) with this one before the token it has expires.
    pub token_expires_at: f64,
    /// The provider Session the conversation last ran, which a new harness resumes.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Close the conversation's harness instead of sending a message; `message` is empty.
    #[serde(default)]
    pub close: bool,
}

impl AssistantJob {
    /// Checks the job's bounds: identities of 1 to 256 bytes without control characters, a
    /// message within [`MAX_ASSISTANT_MESSAGE_BYTES`] (empty only to close), context within
    /// [`MAX_ASSISTANT_CONTEXT_BYTES`] and a token of 32 to 512 bytes.
    ///
    /// # Errors
    ///
    /// A message naming the first field out of bounds.
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("job_id", self.job_id.as_str()),
            ("conversation_id", self.conversation_id.as_str()),
            ("agent_id", self.agent_id.as_str()),
        ] {
            if !valid_identity(value) {
                return Err(format!(
                    "{name} is empty, too long or has control characters"
                ));
            }
        }
        if let Some(session) = &self.session_id
            && !valid_identity(session)
        {
            return Err("session_id is empty, too long or has control characters".to_owned());
        }
        if self.message.len() > MAX_ASSISTANT_MESSAGE_BYTES
            || (self.message.trim().is_empty() && !self.close)
        {
            return Err("message is empty or too long".to_owned());
        }
        if self.context.iter().map(String::len).sum::<usize>() > MAX_ASSISTANT_CONTEXT_BYTES {
            return Err("context is too long".to_owned());
        }
        if !(32..=512).contains(&self.mcp_token.len())
            || self
                .mcp_token
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err("mcp_token is not a valid token".to_owned());
        }
        if !self.token_expires_at.is_finite() || self.token_expires_at <= 0.0 {
            return Err("token_expires_at is not a time".to_owned());
        }
        Ok(())
    }
}

impl fmt::Debug for AssistantJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssistantJob")
            .field("job_id", &self.job_id)
            .field("conversation_id", &self.conversation_id)
            .field("agent_id", &self.agent_id)
            .field("message_bytes", &self.message.len())
            .field("context_items", &self.context.len())
            .field("mcp_token", &"[REDACTED]")
            .field("token_expires_at", &self.token_expires_at)
            .field("session_id", &self.session_id)
            .field("close", &self.close)
            .finish()
    }
}

/// What a Station reports about one conversation's harness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantReport {
    /// The reporting Station; Stations share one credential, so this is its own word.
    pub station_id: String,
    /// The conversation the events belong to.
    pub conversation_id: String,
    /// The job the events answer, when they answer one.
    #[serde(default)]
    pub job_id: Option<String>,
    /// The provider Session the harness runs, once the provider has issued it.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The events, oldest first; at most [`MAX_ASSISTANT_EVENTS`].
    pub events: Vec<AssistantEvent>,
}

/// One thing the assistant's harness did, in provider-neutral terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantEvent {
    /// One of the `EVENT_*` kinds; a Station may report others, which Universal Exports
    /// ignores.
    pub kind: String,
    /// The event's text, at most [`MAX_ASSISTANT_EVENT_BYTES`].
    #[serde(default)]
    pub text: Option<String>,
    /// The tools a [`EVENT_TOOL`] event called, as the harness names them.
    #[serde(default)]
    pub tools: Vec<String>,
}

impl AssistantEvent {
    /// An event of `kind` with `text`, cut to [`MAX_ASSISTANT_EVENT_BYTES`] on a character
    /// boundary.
    pub fn new(kind: &str, text: Option<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            text: text.map(|text| cut(text, MAX_ASSISTANT_EVENT_BYTES)),
            tools: Vec::new(),
        }
    }
}

/// Whether `value` is 1 to 256 bytes without control characters.
pub fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn cut(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> AssistantJob {
        AssistantJob {
            job_id: "job-1".to_owned(),
            conversation_id: "conversation-1".to_owned(),
            agent_id: "moneypenny".to_owned(),
            message: "Which Stations are busy?".to_owned(),
            context: vec!["You are Moneypenny.".to_owned()],
            mcp_token: "t".repeat(43),
            token_expires_at: 1_800_000_000.0,
            session_id: None,
            close: false,
        }
    }

    #[test]
    fn a_job_is_bounded_and_its_token_never_printed() {
        assert_eq!(job().validate(), Ok(()));
        let shown = format!("{:?}", job());
        assert!(!shown.contains(&"t".repeat(43)), "{shown}");
        let mut blank = job();
        blank.message = "  ".to_owned();
        assert!(blank.validate().is_err());
        blank.close = true;
        assert_eq!(blank.validate(), Ok(()), "a close carries no message");
        let mut long = job();
        long.message = "x".repeat(MAX_ASSISTANT_MESSAGE_BYTES + 1);
        assert!(long.validate().is_err());
        let mut token = job();
        token.mcp_token = "short".to_owned();
        assert!(token.validate().is_err());
        let mut control = job();
        control.conversation_id = "a\nb".to_owned();
        assert!(control.validate().is_err());
    }

    #[test]
    fn event_text_is_cut_on_a_character_boundary() {
        let text = "é".repeat(MAX_ASSISTANT_EVENT_BYTES);
        let event = AssistantEvent::new(EVENT_TEXT, Some(text));
        let kept = event.text.unwrap();
        assert!(kept.len() <= MAX_ASSISTANT_EVENT_BYTES);
        assert!(kept.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_job_round_trips_and_refuses_unknown_fields() {
        let encoded = serde_json::to_value(job()).unwrap();
        let decoded: AssistantJob = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded, job());
        let mut extra = encoded;
        extra["shell"] = serde_json::json!(true);
        assert!(serde_json::from_value::<AssistantJob>(extra).is_err());
    }
}
