//! When a long-lived conversation (a charge's, or a standing Mission's resumed Runs) has grown
//! enough that its next turn starts a fresh session carrying a summary instead of resuming.
//!
//! No handler offers a way to compact a session that AI6 can rely on without a live model, so the
//! fallback is the only mechanism: the old session is closed and a new one starts with what the
//! Station kept of the old one.

use std::collections::BTreeSet;

use super::DaemonState;
use crate::directorates::m::roster::Agent;
use crate::directorates::medical::usage::UsageTotals;
use crate::directorates::registry::runs::RunRecord;
use crate::station::limits::HarnessLimits;

/// The size at which a conversation compacts: the smaller of `[harness] compact_at_tokens` and
/// `[harness] compact_at_cap_percent` of the route's `token_cap`; none when neither is set.
pub(super) fn threshold(limits: &HarnessLimits, cap: Option<u64>) -> Option<u64> {
    let by_cap = cap
        .zip(limits.compact_at_cap_percent.get())
        .map(|(cap, percent)| (u128::from(cap) * u128::from(percent) / 100) as u64);
    match (limits.compact_at_tokens.get(), by_cap) {
        (Some(size), Some(cap)) => Some(size.min(cap)),
        (size, cap) => size.or(cap),
    }
}

/// The smallest `token_cap` among an Agent's routes: a conversation may land on any of them.
pub(super) fn smallest_cap(agent: &Agent) -> Option<u64> {
    agent
        .route_list()
        .iter()
        .filter_map(|route| route.token_cap)
        .min()
}

/// What a turn took into the conversation's context: its prompt tokens, cached or not. For a
/// turn of several requests this counts the context once per request, so it is an upper bound.
pub(super) fn context_of(delta: &UsageTotals) -> u64 {
    delta
        .input
        .saturating_add(delta.cache_read)
        .saturating_add(delta.cache_write)
}

/// The tokens a standing Mission's conversation has taken in across the Runs that resumed one
/// provider session, ending at `last`: what each Run added (its input and output, cache reads
/// not counted again). A Run that started a session of its own ends the chain.
pub(super) fn chain_tokens(
    state: &DaemonState,
    operation: Option<&str>,
    mission_id: &str,
    last: &RunRecord,
) -> u64 {
    let Some(usage) = state
        .intake
        .mission_usage(operation, mission_id)
        .ok()
        .flatten()
    else {
        return 0;
    };
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    let mut current = last.clone();
    while seen.insert(current.id.clone()) {
        total = total.saturating_add(usage.runs.get(&current.id).map_or(0, UsageTotals::tokens));
        let Some(previous) = current
            .predecessor
            .as_deref()
            .and_then(|id| state.intake.run(id).ok().flatten())
            .filter(|previous| previous.provider_id.is_some())
            .filter(|previous| previous.provider_id == current.provider_id)
        else {
            break;
        };
        current = previous;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounds::Limit;

    fn limits(tokens: Limit, percent: Limit) -> HarnessLimits {
        HarnessLimits {
            compact_at_tokens: tokens,
            compact_at_cap_percent: percent,
            ..HarnessLimits::default()
        }
    }

    #[test]
    fn the_smaller_of_the_size_and_the_share_of_the_cap_decides() {
        let share = limits(Limit::UNBOUNDED, Limit::at(50));
        assert_eq!(threshold(&share, Some(300_000)), Some(150_000));
        assert_eq!(threshold(&share, None), None, "no cap, no size: no bound");
        let both = limits(Limit::at(100_000), Limit::at(50));
        assert_eq!(threshold(&both, Some(300_000)), Some(100_000));
        assert_eq!(threshold(&both, None), Some(100_000));
        let size = limits(Limit::at(100_000), Limit::UNBOUNDED);
        assert_eq!(threshold(&size, Some(10)), Some(100_000));
        let neither = limits(Limit::UNBOUNDED, Limit::UNBOUNDED);
        assert_eq!(threshold(&neither, Some(10)), None);
    }

    #[test]
    fn a_turns_context_counts_cached_prompt_tokens_and_not_its_output() {
        let delta = UsageTotals {
            input: 10,
            output: 1_000,
            cache_read: 200,
            cache_write: 30,
            cost_micro_usd: 0,
        };
        assert_eq!(context_of(&delta), 240);
    }
}
