//! What M does when a provider usage limit stops a Run.

use serde::{Deserialize, Serialize};

/// How long to park a Run when the provider reports a limit without a reset time.
pub const DEFAULT_LIMIT_FALLBACK_SECS: u64 = 30 * 60;

/// An Operation's response to a provider limit (`on_limit`).
#[non_exhaustive]
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    guatiao::ToValue,
    guatiao::FromValue,
    guatiao::Schema,
)]
#[serde(rename_all = "lowercase")]
#[map(rename_all = "lowercase")]
pub enum OnLimit {
    /// Move the Run to another account of its pools that has headroom: the same provider
    /// Run where its history goes with it (Claude's environment credentials), else a new
    /// run with a hand-off. Without a pool account to move to, the same as `Wait`.
    #[default]
    #[schema(label = "Move to another account in its pools")]
    Rotate,
    /// Park the Run and resume the same provider Run when the limit resets.
    #[schema(label = "Wait for the limit to reset")]
    Wait,
    /// Post the next qualified Agent as a new run; park only if there is none.
    #[schema(label = "Hand over to the next qualified Agent")]
    Backup,
}

impl OnLimit {
    /// The configuration spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rotate => "rotate",
            Self::Wait => "wait",
            Self::Backup => "backup",
        }
    }
}

/// When a Run stopped at `now` (Unix seconds) may resume.
///
/// A reset time in the future is honoured plus `grace_secs`, so the resumed Run does not
/// race the provider's own clock. Without one, the wait the provider stated (`retry_after_secs`,
/// a 429's `retry-after`) is used as it is. A missing or already-past reset with no stated wait
/// cannot be trusted (clock skew, or a limit that outlived its stated reset), so the Run
/// waits `fallback_secs` rather than retrying in a loop.
pub fn parked_until(
    now: i64,
    resets_at: Option<i64>,
    retry_after_secs: Option<u64>,
    fallback_secs: u64,
    grace_secs: i64,
) -> i64 {
    let after = |secs: u64| now.saturating_add(i64::try_from(secs).unwrap_or(i64::MAX));
    match (resets_at, retry_after_secs) {
        (Some(reset), _) if reset > now => reset.saturating_add(grace_secs),
        (_, Some(wait)) => after(wait),
        _ => after(fallback_secs),
    }
}

/// When a limit lifts, as far as the hit says: its reset time, else `now` plus the wait it
/// states. `None` when it states neither.
pub fn lifts_at(now: i64, hit: &coqui_acp_api::LimitHit) -> Option<i64> {
    hit.resets_at.or_else(|| {
        let wait = i64::try_from(hit.retry_after?.as_secs()).ok()?;
        Some(now.saturating_add(wait))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_future_reset_is_honoured_with_a_grace() {
        assert_eq!(parked_until(1_000, Some(5_000), None, 60, 15), 5_000 + 15);
    }

    #[test]
    fn a_missing_or_past_reset_waits_the_fallback() {
        assert_eq!(parked_until(1_000, None, None, 60, 15), 1_060);
        assert_eq!(parked_until(1_000, Some(1_000), None, 60, 15), 1_060);
        assert_eq!(parked_until(1_000, Some(10), None, 60, 15), 1_060);
        assert_eq!(parked_until(i64::MAX, None, None, 60, 15), i64::MAX);
    }

    #[test]
    fn a_stated_wait_beats_the_fallback_and_a_future_reset_beats_both() {
        assert_eq!(parked_until(1_000, None, Some(90), 1_800, 15), 1_090);
        assert_eq!(parked_until(1_000, Some(10), Some(90), 1_800, 15), 1_090);
        assert_eq!(parked_until(1_000, Some(5_000), Some(90), 1_800, 15), 5_015);
        let mut hit = coqui_acp_api::LimitHit::new(1_000);
        assert_eq!(lifts_at(1_000, &hit), None);
        hit.retry_after = Some(std::time::Duration::from_secs(90));
        assert_eq!(lifts_at(1_000, &hit), Some(1_090));
        hit.resets_at = Some(4_000);
        assert_eq!(lifts_at(1_000, &hit), Some(4_000));
    }

    #[test]
    fn the_policy_defaults_to_rotating_and_parses_by_name() {
        assert_eq!(OnLimit::default(), OnLimit::Rotate);
        let parsed: OnLimit = serde_json::from_str("\"backup\"").unwrap();
        assert_eq!(parsed, OnLimit::Backup);
        let parsed: OnLimit = serde_json::from_str("\"wait\"").unwrap();
        assert_eq!(parsed, OnLimit::Wait);
        assert!(serde_json::from_str::<OnLimit>("\"retry\"").is_err());
    }
}
