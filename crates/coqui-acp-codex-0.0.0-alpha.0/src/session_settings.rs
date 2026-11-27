//! The bounds and waits of a Codex session, as the settings a provider's configuration holds.

use coqui_acp_api::guatiao::{FromValue, Schema, ToValue};
use coqui_acp_harness::SessionLimits;

use crate::session::CodexSettings;

/// How a Codex session reads the app server's output, waits for it and ends it: [`SessionLimits`] and what only this
/// translator bounds. Every value is optional and any value is taken; absent, [`CodexSettings::new`]'s.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct SessionSettings {
    /// The limits every translator has.
    #[map(flatten)]
    pub limits: SessionLimits,
    /// The pages of `model/list` asked for. Default 10.
    pub max_model_pages: Option<u64>,
    /// The earlier turns a load replays, the latest kept. Default 1000.
    pub max_history_turns: Option<u64>,
    /// Whether to ask for the models when starting. Off, or when Codex does not answer, they are unknown. Default on.
    pub list_models: Option<bool>,
    /// The longest title made from a command or a query, in bytes. Default 256.
    pub max_tool_title_bytes: Option<u64>,
    /// The most of a command a question's description tells, or of a subagent's task or result, in bytes. Default
    /// 16384.
    pub max_told_bytes: Option<u64>,
    /// The messages remembered as streamed, so their whole copy is not told again. Default 64.
    pub max_streamed_messages: Option<u64>,
}

impl SessionSettings {
    /// The session's settings: [`CodexSettings::new`]'s, with the values given here over them.
    #[must_use]
    pub fn settings(&self) -> CodexSettings {
        let mut settings = CodexSettings::new();
        let limits = &self.limits;
        SessionLimits::bytes(limits.max_line_bytes, &mut settings.max_line);
        SessionLimits::count(limits.tail_lines, &mut settings.tail_lines);
        SessionLimits::count(limits.tail_line_bytes, &mut settings.tail_line_bytes);
        SessionLimits::millis(
            limits.initialize_timeout_ms,
            &mut settings.initialize_timeout,
        );
        SessionLimits::millis(limits.request_timeout_ms, &mut settings.request_timeout);
        SessionLimits::millis(limits.exit_wait_ms, &mut settings.exit_wait);
        SessionLimits::millis(limits.exit_poll_ms, &mut settings.exit_poll);
        SessionLimits::millis(limits.close_wait_ms, &mut settings.close_wait);
        SessionLimits::count(limits.max_tools, &mut settings.max_tools);
        SessionLimits::count(limits.max_queued_writes, &mut settings.max_queued_writes);
        SessionLimits::count(limits.title_bytes, &mut settings.title_bytes);
        SessionLimits::millis(limits.version_wait_ms, &mut settings.version_wait);
        SessionLimits::bytes(limits.version_bytes, &mut settings.version_bytes);
        SessionLimits::count(self.max_model_pages, &mut settings.max_model_pages);
        SessionLimits::count(self.max_history_turns, &mut settings.max_history_turns);
        if let Some(list) = self.list_models {
            settings.list_models = list;
        }
        SessionLimits::count(
            self.max_tool_title_bytes,
            &mut settings.parser.max_title_bytes,
        );
        SessionLimits::count(self.max_told_bytes, &mut settings.parser.max_told_bytes);
        SessionLimits::count(
            self.max_streamed_messages,
            &mut settings.parser.max_streamed,
        );
        settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::time::Duration;

    /// A session section with nothing in it is the session defaults; each value given reaches its own limit.
    #[test]
    fn nothing_given_keeps_the_defaults_and_each_value_reaches_its_limit() {
        let defaults = CodexSettings::new();
        let none = SessionSettings::default().settings();
        assert_eq!(none.initialize_timeout, Duration::from_secs(20));
        assert_eq!(none.max_history_turns, defaults.max_history_turns);
        assert_eq!(none.parser, defaults.parser);
        assert!(none.list_models);
        let mut given = SessionSettings::default();
        given.limits.initialize_timeout_ms = Some(7);
        given.limits.request_timeout_ms = Some(8);
        given.max_history_turns = Some(9);
        given.list_models = Some(false);
        given.max_told_bytes = Some(10);
        let settings = given.settings();
        assert_eq!(settings.initialize_timeout, Duration::from_millis(7));
        assert_eq!(settings.request_timeout, Duration::from_millis(8));
        assert_eq!(settings.max_history_turns, 9);
        assert!(!settings.list_models);
        assert_eq!(settings.parser.max_told_bytes, 10);
        assert_eq!(settings.parser.max_streamed, defaults.parser.max_streamed);
    }
}
