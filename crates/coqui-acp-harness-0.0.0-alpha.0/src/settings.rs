//! The settings every harness provider's configuration shares, as sections a provider flattens into its own.
//!
//! [`ConnectionSettings`] is what one ACP connection holds to and offers; [`SessionLimits`] is how a translator
//! reads its program's output, waits for it and ends it. Every number is a setting with a default and any value is
//! taken: a value left out keeps the default, and a limit is never a ceiling the person cannot move.

use alloc::vec::Vec;
use core::time::Duration;

use coqui_acp_api::guatiao::{FromValue, Schema, ToValue};

use crate::access::Access;
use crate::acp::Settings;
use crate::mode::Mode;

/// What one ACP connection to a harness provider holds to and offers.
///
/// The provider's own settings flatten it (`#[map(flatten)]`), so its keys sit beside the provider's own. A `max_*`
/// or `*_ms` of `None` keeps [`Settings::new`]'s default.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct ConnectionSettings {
    /// The access every session starts with, in the words every harness shares: the sandbox, the network and the
    /// approvals, turned into what the harness takes (a permission mode, an approval policy, a sandbox). Absent, the
    /// harness's own configuration decides.
    pub access: Option<Access>,
    /// Whether a client may give a session its own access through `_meta` (`dev.boriken.coqui.acp/access`).
    #[map(default = false)]
    pub allow_client_access: bool,
    /// Whether the Bypass mode may be chosen: nothing is asked.
    #[map(default = false)]
    pub allow_bypass: bool,
    /// The modes a client may choose, written by id (`manual`, `accept_edits`, `auto`, `plan`, `bypass`): these widen
    /// the starting mode's own and every stricter one. Bypass is offered only with `allow_bypass`, whatever this
    /// names. Absent, a client chooses the starting mode and every stricter one.
    pub modes: Option<Vec<Mode>>,
    /// The highest ACP version `initialize` answers: 1 or 2.
    #[map(default = 2)]
    pub max_acp_version: u64,
    /// Sessions one connection may hold.
    #[map(default = 8)]
    pub max_sessions: u64,
    /// Requests one connection works on at once; a request past it is refused, and a client that sends faster than
    /// the answers are taken is held back.
    #[map(default = 32)]
    pub max_requests: u64,
    /// The largest message a connection takes, in bytes; one more ends it.
    #[map(default = 67_108_864)]
    pub max_message_bytes: u64,
    /// The largest prompt taken, as the JSON of its blocks, in bytes. Default 16 MiB.
    pub max_prompt_bytes: Option<u64>,
    /// How long a cancelled turn may take to end before it is answered `cancelled` anyway, in milliseconds. Default
    /// 10 000.
    pub cancel_grace_ms: Option<u64>,
    /// How long a permission request waits for the client before it is answered cancelled, in milliseconds. Absent,
    /// it waits for as long as the person takes.
    pub permission_timeout_ms: Option<u64>,
    /// The tool calls a session keeps track of for ACP version 1; past it the oldest is forgotten. Default 4096.
    pub max_tracked_tools: Option<u64>,
    /// Events of a session queued for the client; past it the harness's reader waits. Default 4096.
    pub max_queued_events: Option<u64>,
    /// ACP version 1 sends a tool call's content whole with each piece: the bytes kept of each. Default 1 MiB.
    pub max_tool_content_bytes: Option<u64>,
    /// The same for how many pieces. Default 256.
    pub max_tool_content_items: Option<u64>,
    /// A session's questions open at once; past it one is answered cancelled without being asked. Default 64.
    pub max_open_questions: Option<u64>,
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        Self {
            access: None,
            allow_client_access: false,
            allow_bypass: false,
            modes: None,
            max_acp_version: 2,
            max_sessions: 8,
            max_requests: 32,
            max_message_bytes: 64 * 1024 * 1024,
            max_prompt_bytes: None,
            cancel_grace_ms: None,
            permission_timeout_ms: None,
            max_tracked_tools: None,
            max_queued_events: None,
            max_tool_content_bytes: None,
            max_tool_content_items: None,
            max_open_questions: None,
        }
    }
}

/// A count as the platform's size: a value past it takes the most there is.
fn size(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

impl ConnectionSettings {
    /// The ACP versions the connection answers: 1 and 2, or 1 alone, as `max_acp_version` says.
    #[must_use]
    pub fn versions(&self) -> Vec<u16> {
        if self.max_acp_version >= 2 {
            alloc::vec![1, 2]
        } else {
            alloc::vec![1]
        }
    }

    /// The largest message a connection takes, as the platform's size.
    #[must_use]
    pub fn message_limit(&self) -> usize {
        size(self.max_message_bytes)
    }

    /// The server settings of one connection, over [`Settings::new`]'s defaults. What a provider sets besides (the
    /// working roots, the account's id, whether a client may give MCP servers) is left to it.
    #[must_use]
    pub fn serving(&self) -> Settings {
        let mut settings = Settings::new();
        settings.max_sessions = size(self.max_sessions);
        settings.max_requests = size(self.max_requests);
        settings.max_version = if self.max_acp_version >= 2 { 2 } else { 1 };
        if let Some(bytes) = self.max_prompt_bytes {
            settings.max_prompt_bytes = size(bytes);
        }
        if let Some(ms) = self.cancel_grace_ms {
            settings.cancel_grace = Duration::from_millis(ms);
        }
        settings.permission_timeout = self.permission_timeout_ms.map(Duration::from_millis);
        if let Some(count) = self.max_tracked_tools {
            settings.max_tools = size(count);
        }
        if let Some(count) = self.max_queued_events {
            settings.max_queued_events = size(count);
        }
        if let Some(bytes) = self.max_tool_content_bytes {
            settings.max_tool_content_bytes = size(bytes);
        }
        if let Some(count) = self.max_tool_content_items {
            settings.max_tool_content_items = size(count);
        }
        if let Some(count) = self.max_open_questions {
            settings.max_open_questions = size(count);
        }
        settings
    }
}

/// How a translator reads its program's output, waits for it and ends it, the same for every translator.
///
/// A translator's provider and account provider both flatten it (inside the translator's own session section), so a
/// limit is set once for the sessions that serve a client and for the ones that list models or renew a login. Every
/// value is optional: absent, the translator's own default.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct SessionLimits {
    /// The longest line of the program's output read, in bytes; past it the session ends. Default 64 MiB.
    pub max_line_bytes: Option<u64>,
    /// How many of the last lines that were not protocol a failure quotes. Default 8.
    pub tail_lines: Option<u64>,
    /// The bytes of each of those lines. Default 512.
    pub tail_line_bytes: Option<u64>,
    /// How long the program may take to answer its first request, in milliseconds. Default 30 000 (Codex: 20 000).
    pub initialize_timeout_ms: Option<u64>,
    /// How long it may take to answer any other request, in milliseconds. Default 30 000.
    pub request_timeout_ms: Option<u64>,
    /// How long the end of the output waits for the exit status, in milliseconds. Default 1 000.
    pub exit_wait_ms: Option<u64>,
    /// How often the program is asked whether it has exited while its output stays open, in milliseconds. Default 100.
    pub exit_poll_ms: Option<u64>,
    /// How long a close waits for it to have ended, in milliseconds. Default 2 000.
    pub close_wait_ms: Option<u64>,
    /// The tool calls followed at once. Default 4096.
    pub max_tools: Option<u64>,
    /// The lines waiting to be written to it; past it a write fails. Default 1024.
    pub max_queued_writes: Option<u64>,
    /// The bytes of a session's title, from its first prompt. Default 80.
    pub title_bytes: Option<u64>,
    /// How long `<program> --version` may take, in milliseconds; 0 does not ask. Default 10 000.
    pub version_wait_ms: Option<u64>,
    /// How much of its answer is read, in bytes. Default 4096.
    pub version_bytes: Option<u64>,
}

impl SessionLimits {
    /// `slot` as `value` milliseconds, when one is given.
    pub fn millis(value: Option<u64>, slot: &mut Duration) {
        if let Some(ms) = value {
            *slot = Duration::from_millis(ms);
        }
    }

    /// `slot` as `value` counted in the platform's size, when one is given.
    pub fn count(value: Option<u64>, slot: &mut usize) {
        if let Some(count) = value {
            *slot = size(count);
        }
    }

    /// `slot` as `value`, when one is given.
    pub fn bytes(value: Option<u64>, slot: &mut u64) {
        if let Some(bytes) = value {
            *slot = bytes;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_servers_and_a_value_given_replaces_one() {
        let defaults = ConnectionSettings::default().serving();
        assert_eq!(defaults.max_prompt_bytes, Settings::new().max_prompt_bytes);
        assert_eq!(defaults.cancel_grace, Settings::new().cancel_grace);
        assert_eq!(defaults.permission_timeout, None);
        let mut given = ConnectionSettings {
            max_prompt_bytes: Some(1),
            cancel_grace_ms: Some(0),
            permission_timeout_ms: Some(5),
            max_open_questions: Some(u64::MAX),
            ..ConnectionSettings::default()
        };
        let settings = given.serving();
        assert_eq!(settings.max_prompt_bytes, 1);
        assert_eq!(settings.cancel_grace, Duration::ZERO);
        assert_eq!(settings.permission_timeout, Some(Duration::from_millis(5)));
        assert_eq!(settings.max_open_questions, usize::MAX);
        given.max_acp_version = 1;
        assert_eq!(given.serving().max_version, 1);
        assert_eq!(given.versions(), [1]);
        assert_eq!(ConnectionSettings::default().versions(), [1, 2]);
    }

    #[test]
    fn a_session_limit_left_out_keeps_the_translators_own() {
        let mut duration = Duration::from_secs(20);
        SessionLimits::millis(None, &mut duration);
        assert_eq!(duration, Duration::from_secs(20));
        SessionLimits::millis(Some(1500), &mut duration);
        assert_eq!(duration, Duration::from_millis(1500));
        let mut count = 8;
        SessionLimits::count(Some(0), &mut count);
        assert_eq!(count, 0);
    }
}
