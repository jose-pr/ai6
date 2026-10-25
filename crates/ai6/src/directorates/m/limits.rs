//! What M does when a provider usage limit stops a Session.

use serde::{Deserialize, Serialize};

/// How long to park a Session when the provider reports a limit without a reset time.
pub const DEFAULT_LIMIT_FALLBACK_SECS: u64 = 30 * 60;

/// Slack added after a reported reset time, so the resumed Session does not race the
/// provider's own clock.
pub const RESET_GRACE_SECS: i64 = 15;

/// An Operation's response to a provider limit (`on_limit`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnLimit {
    /// Park the Session and resume the same provider Session when the limit resets.
    #[default]
    Wait,
    /// Post the next qualified Agent as a new attempt; park only if there is none.
    Backup,
}

impl OnLimit {
    /// The configuration spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wait => "wait",
            Self::Backup => "backup",
        }
    }
}

/// When a Session stopped at `now` (Unix seconds) may resume.
///
/// A reset time in the future is honoured plus a short grace. A missing or already-past
/// one cannot be trusted (clock skew, or a limit that outlived its stated reset), so the
/// Session waits `fallback_secs` rather than retrying in a loop.
pub fn parked_until(now: i64, resets_at: Option<i64>, fallback_secs: u64) -> i64 {
    match resets_at {
        Some(reset) if reset > now => reset.saturating_add(RESET_GRACE_SECS),
        _ => now.saturating_add(i64::try_from(fallback_secs).unwrap_or(i64::MAX)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_future_reset_is_honoured_with_a_grace() {
        assert_eq!(
            parked_until(1_000, Some(5_000), 60),
            5_000 + RESET_GRACE_SECS
        );
    }

    #[test]
    fn a_missing_or_past_reset_waits_the_fallback() {
        assert_eq!(parked_until(1_000, None, 60), 1_060);
        assert_eq!(parked_until(1_000, Some(1_000), 60), 1_060);
        assert_eq!(parked_until(1_000, Some(10), 60), 1_060);
        assert_eq!(parked_until(i64::MAX, None, 60), i64::MAX);
    }

    #[test]
    fn the_policy_defaults_to_waiting_and_parses_by_name() {
        assert_eq!(OnLimit::default(), OnLimit::Wait);
        let parsed: OnLimit = serde_json::from_str("\"backup\"").unwrap();
        assert_eq!(parsed, OnLimit::Backup);
        assert!(serde_json::from_str::<OnLimit>("\"retry\"").is_err());
    }
}
