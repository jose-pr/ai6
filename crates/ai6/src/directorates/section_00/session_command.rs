//! A command for a live harness session: compact its conversation in place, or clear it and
//! go on in the same process. A Run's messages are text, so a command is one text in this
//! form, which only a handler that carries commands out reads: [`SESSION_COMMAND_PREFIX`] and
//! then the JSON of a [`SessionCommand`]. The Station sends it only to a handler whose
//! [`AgentHandler::session_commands`] says it carries that command out, and a handler that
//! meets the prefix and cannot act on it refuses the message, so the form is never written to
//! a model.
//!
//! A command is a turn. It ends with the `completed` event any turn ends with, and the events
//! before that say what happened: [`COMPACTED_EVENT`], [`COMPACT_FAILED_EVENT`] or
//! [`CLEARED_EVENT`], each with a [`SessionChange`] as its data.
//!
//! [`AgentHandler::session_commands`]: super::agent::AgentHandler::session_commands

use coqui_acp_provider::AgentEvent;
use guatiao::library::ProviderError;
use guatiao::{FromValue, Schema, Status, ToValue};
use serde::{Deserialize, Serialize};

/// What begins a message in this form: a NUL, which the app's text never holds, and the form's
/// name.
pub const SESSION_COMMAND_PREFIX: &str = "\u{0}ai6-session\n";

/// The harness compacted the conversation; the data is a [`SessionChange`].
pub const COMPACTED_EVENT: &str = "compacted";

/// The harness tried to compact and could not; the text says why and the data is a
/// [`SessionChange`]. The turn ends as any turn does, and the conversation is as it was.
pub const COMPACT_FAILED_EVENT: &str = "compact_failed";

/// The harness cleared the conversation and went on in the same process; the data is a
/// [`SessionChange`] whose `session_id` is the one the conversation now has.
pub const CLEARED_EVENT: &str = "cleared";

/// One command for a live session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionCommand {
    /// Compact the conversation, keeping what `focus` names when the harness takes one.
    Compact {
        /// What the summary should keep; `None` leaves the harness's own choice.
        #[serde(default)]
        focus: Option<String>,
    },
    /// Open a fresh conversation in the same process.
    Clear,
}

impl SessionCommand {
    /// The command as a Run's message text.
    pub fn encode(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        format!("{SESSION_COMMAND_PREFIX}{json}")
    }

    /// The command a message holds: `Ok(None)` for plain text, which is a message of its own.
    ///
    /// # Errors
    ///
    /// The message begins with [`SESSION_COMMAND_PREFIX`] and is not a command, so a handler
    /// refuses it instead of passing it on as words.
    pub fn parse(message: &str) -> Result<Option<Self>, String> {
        let Some(json) = message.strip_prefix(SESSION_COMMAND_PREFIX) else {
            return Ok(None);
        };
        serde_json::from_str(json)
            .map(Some)
            .map_err(|error| format!("an unreadable session command: {error}"))
    }

    /// For a session that carries out no command: refuses `message` with `why` when it is in
    /// this form, so the form never reaches the harness as words, and passes any other text.
    ///
    /// # Errors
    ///
    /// `GUATIAO_ERR_UNSUPPORTED` for a message in this form.
    pub fn refuse_in(message: &str, why: &str) -> Result<(), ProviderError> {
        if message.starts_with(SESSION_COMMAND_PREFIX) {
            return Err(ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, why));
        }
        Ok(())
    }
}

/// What a command changed, as the data of the event that reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionChange {
    /// What began it: `manual` for a command, `auto` for the harness's own compaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// The tokens the conversation held before, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<u64>,
    /// The tokens it holds now, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_after: Option<u64>,
    /// The conversation's identity now. A harness that gives a cleared conversation a new one
    /// reports it here, and `session_id` answers with it from then on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl SessionChange {
    /// The event that reports this change: `kind` is one of the events of this module.
    pub fn event(&self, kind: &str, text: Option<String>) -> AgentEvent {
        let mut event = AgentEvent::new(kind, text);
        event.data = serde_json::to_vec(self).ok();
        event
    }

    /// The change an event reports; `None` for any other event.
    pub fn of(event: &AgentEvent) -> Option<Self> {
        matches!(
            event.kind.as_str(),
            COMPACTED_EVENT | COMPACT_FAILED_EVENT | CLEARED_EVENT
        )
        .then(|| serde_json::from_slice(event.data.as_deref()?).ok())
        .flatten()
    }
}

/// Which commands a handler carries out on a live session.
///
/// A handler that cannot tell until a session is open (an ACP agent advertises its commands
/// then) says what it may do, and refuses a command its session cannot carry out with
/// `GUATIAO_ERR_UNSUPPORTED` when it is sent.
#[derive(
    ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[map(crate = ::guatiao)]
pub struct SessionCommands {
    /// [`SessionCommand::Compact`] in the live session.
    pub compact: bool,
    /// [`SessionCommand::Clear`] in the same process.
    pub clear: bool,
    /// Why not, in a sentence, for a command it does not carry out.
    pub unsupported_because: Option<String>,
}

impl SessionCommands {
    /// A handler that carries out no command.
    pub fn none() -> Self {
        Self {
            compact: false,
            clear: false,
            unsupported_because: Some("this handler has no command for a live session".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_reads_back_and_plain_text_is_no_command() {
        for command in [
            SessionCommand::Compact { focus: None },
            SessionCommand::Compact {
                focus: Some("the open bugs".to_owned()),
            },
            SessionCommand::Clear,
        ] {
            assert_eq!(
                SessionCommand::parse(&command.encode()),
                Ok(Some(command.clone()))
            );
        }
        assert_eq!(SessionCommand::parse("/compact"), Ok(None));
        assert_eq!(SessionCommand::parse("ai6-session\n{}"), Ok(None));
    }

    #[test]
    fn a_text_that_only_looks_like_a_command_is_refused_not_passed_on() {
        for bad in ["", "{", "{\"command\":\"reboot\"}"] {
            let text = format!("{SESSION_COMMAND_PREFIX}{bad}");
            assert!(SessionCommand::parse(&text).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_change_rides_in_the_data_of_its_event() {
        let change = SessionChange {
            trigger: Some("manual".to_owned()),
            tokens_before: Some(23879),
            tokens_after: Some(2830),
            session_id: None,
        };
        let event = change.event(COMPACTED_EVENT, None);
        assert_eq!(SessionChange::of(&event), Some(change));
        assert_eq!(SessionChange::of(&AgentEvent::new("completed", None)), None);
    }
}
