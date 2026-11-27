//! The bounds and waits of a Claude Code session, as the settings a provider's configuration holds.

use coqui_acp_api::guatiao::{FromValue, Schema, ToValue};
use coqui_acp_harness::SessionLimits;

use crate::session::ClaudeSettings;

/// How a Claude Code session reads its program's output, waits for it and ends it: [`SessionLimits`] and what only
/// this translator bounds. Every value is optional and any value is taken; absent, [`ClaudeSettings::new`]'s.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct SessionSettings {
    /// The limits every translator has.
    #[map(flatten)]
    pub limits: SessionLimits,
    /// The largest transcript a load replays, in bytes. Default 64 MiB.
    pub max_transcript_bytes: Option<u64>,
    /// The longest title made from a tool's input, in bytes. Default 256.
    pub max_tool_title_bytes: Option<u64>,
    /// The messages remembered as streamed, so their whole copy is not told again. Default 64.
    pub max_streamed_messages: Option<u64>,
    /// The entries of the plan the Task tools keep, the oldest dropped past it. Default 1024.
    pub max_tasks: Option<u64>,
    /// The longest task or result text a subagent's report carries, in bytes. Default 16384.
    pub max_child_text_bytes: Option<u64>,
}

impl SessionSettings {
    /// The session's settings: [`ClaudeSettings::new`]'s, with the values given here over them.
    #[must_use]
    pub fn settings(&self) -> ClaudeSettings {
        let mut settings = ClaudeSettings::new();
        let limits = &self.limits;
        SessionLimits::bytes(limits.max_line_bytes, &mut settings.max_line);
        SessionLimits::count(limits.tail_lines, &mut settings.tail_lines);
        SessionLimits::count(limits.tail_line_bytes, &mut settings.tail_line_bytes);
        SessionLimits::millis(
            limits.initialize_timeout_ms,
            &mut settings.initialize_timeout,
        );
        SessionLimits::millis(limits.request_timeout_ms, &mut settings.control_timeout);
        SessionLimits::millis(limits.exit_wait_ms, &mut settings.exit_wait);
        SessionLimits::millis(limits.exit_poll_ms, &mut settings.exit_poll);
        SessionLimits::millis(limits.close_wait_ms, &mut settings.close_wait);
        SessionLimits::count(limits.max_tools, &mut settings.max_tools);
        SessionLimits::count(limits.max_queued_writes, &mut settings.max_queued_writes);
        SessionLimits::count(limits.title_bytes, &mut settings.title_bytes);
        SessionLimits::millis(limits.version_wait_ms, &mut settings.version_wait);
        SessionLimits::bytes(limits.version_bytes, &mut settings.version_bytes);
        SessionLimits::bytes(
            self.max_transcript_bytes,
            &mut settings.max_transcript_bytes,
        );
        SessionLimits::count(
            self.max_tool_title_bytes,
            &mut settings.parser.max_title_bytes,
        );
        SessionLimits::count(
            self.max_streamed_messages,
            &mut settings.parser.max_streamed,
        );
        SessionLimits::count(self.max_tasks, &mut settings.parser.max_tasks);
        SessionLimits::count(
            self.max_child_text_bytes,
            &mut settings.parser.max_child_text_bytes,
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
        let defaults = ClaudeSettings::new();
        let none = SessionSettings::default().settings();
        assert_eq!(none.initialize_timeout, defaults.initialize_timeout);
        assert_eq!(none.max_transcript_bytes, defaults.max_transcript_bytes);
        assert_eq!(none.parser, defaults.parser);
        let mut given = SessionSettings::default();
        given.limits.initialize_timeout_ms = Some(7);
        given.limits.request_timeout_ms = Some(8);
        given.limits.version_wait_ms = Some(0);
        given.max_transcript_bytes = Some(9);
        given.max_tasks = Some(10);
        let settings = given.settings();
        assert_eq!(settings.initialize_timeout, Duration::from_millis(7));
        assert_eq!(settings.control_timeout, Duration::from_millis(8));
        assert_eq!(settings.version_wait, Duration::ZERO);
        assert_eq!(settings.max_transcript_bytes, 9);
        assert_eq!(settings.parser.max_tasks, 10);
        assert_eq!(settings.parser.max_streamed, defaults.parser.max_streamed);
    }
}
