//! Which of several accounts, or routes, takes a Run: `coqui-acp-pool` orders them and judges which
//! have room, and this module is the wiring to what AI6 keeps itself.
//!
//! AI6 keeps the configuration (a pool's strategy, an account's priority and `max_concurrent`),
//! the state in the Registry (claims, limits, what each window reports, when an account last took
//! a Run) and the scoping of pools to Operations. This module turns the state of one choice into
//! a pool of members and standings for the crate, and reads the pick back. Nothing here keeps a
//! cursor or a clock between choices: every choice is made from the Registry's state alone, so
//! two Stations that read the same state pick the same member.
//!
//! The crate's tie-break is the listed order, so a caller lists its members in the order ties go:
//! by name for accounts, as the Agent lists them for routes. An account counts as used by the most
//! used of its windows as AI6 reads them (a window that has reset counts as unused), and the crate
//! never skips a member for its use: AI6 orders by it and refuses only for a limit, a claim cap
//! and a login that cannot open here.

use coqui_acp_api::{Bucket, UsageReport};
use coqui_acp_pool::{Ask, Member, Memory, Pool, Settings, Standing, Strategy, Usage, decide};

use super::PoolStrategy;

/// One member of a choice, as the Registry's state and the configuration stand.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    /// Higher goes first under `priority`, and breaks a tie under `headroom`.
    pub(crate) priority: i64,
    /// The most Runs that may use it at once.
    pub(crate) max_concurrent: Option<usize>,
    /// The Runs using it now, not counting the asking Run's own.
    pub(crate) active: usize,
    /// Thousandths of its most used window used, as AI6 reads them; `None` when no provider
    /// reported any, which counts as the choice's `unknown_use`.
    pub(crate) used_permille: Option<u16>,
    /// When a Run last took it, as Unix seconds.
    pub(crate) last_used: Option<f64>,
    /// Why it cannot take a Run at all now (limited, or a login that cannot open here).
    pub(crate) unavailable: Option<&'static str>,
}

/// The crate's strategy for AI6's: `round_robin` is the least recently used member, which is the
/// state AI6 shares between Stations, and not a cursor held in one process.
const fn strategy_of(strategy: PoolStrategy) -> Strategy {
    match strategy {
        PoolStrategy::Priority => Strategy::Priority,
        PoolStrategy::Headroom => Strategy::Headroom,
        PoolStrategy::RoundRobin => Strategy::LeastRecent,
    }
}

/// The member of `entries`, listed in the order ties go, that takes the Run, or `None` when none
/// can. `prefer` is taken whenever it can serve, whatever the strategy. `unknown_use` is what a
/// member with no reported use counts as, in thousandths.
pub(crate) fn pick(
    strategy: PoolStrategy,
    entries: &[Entry],
    prefer: Option<usize>,
    unknown_use: u16,
) -> Option<usize> {
    if let Some(preferred) = prefer
        && entries.get(preferred).is_some()
        && decide_among(strategy, entries, &[preferred], unknown_use).is_some()
    {
        return Some(preferred);
    }
    let all: Vec<usize> = (0..entries.len()).collect();
    decide_among(strategy, entries, &all, unknown_use)
}

/// The crate's decision over the members of `entries` at `at`, as a position in `entries`.
fn decide_among(
    strategy: PoolStrategy,
    entries: &[Entry],
    at: &[usize],
    unknown_use: u16,
) -> Option<usize> {
    // Priorities are AI6's `i64`, the crate's `i32`: ranks keep their order whatever their size.
    // Under `round_robin` AI6 ignores priority, so every member ranks alike.
    let mut priorities: Vec<i64> = at.iter().map(|each| entries[*each].priority).collect();
    priorities.sort_unstable();
    priorities.dedup();
    let rank = |entry: &Entry| match strategy {
        PoolStrategy::RoundRobin => 0,
        _ => i32::try_from(priorities.binary_search(&entry.priority).unwrap_or(0)).unwrap_or(0),
    };
    let mut pool = Pool::<()>::new("pool")
        .strategy(strategy_of(strategy))
        .settings(
            Settings::new()
                .unknown_permille(unknown_use)
                .stop_permille(u16::MAX),
        );
    let mut standings = Vec::with_capacity(at.len());
    for each in at {
        let entry = &entries[*each];
        let mut member = Member::new(each.to_string(), ()).priority(rank(entry));
        if let Some(most) = entry.max_concurrent {
            member = member.max_concurrent(u32::try_from(most).unwrap_or(u32::MAX));
        }
        pool = pool.member(member);
        standings.push(standing_of(entry));
    }
    let chosen = decide(&pool, &mut Memory::default(), &standings, &Ask::new(0)).ok()?;
    at.get(chosen.index).copied()
}

/// What the crate is told of one member.
fn standing_of(entry: &Entry) -> Standing {
    let mut standing = Standing::new().active(u32::try_from(entry.active).unwrap_or(u32::MAX));
    if let Some(why) = entry.unavailable {
        standing = standing.unavailable(why);
    }
    if let Some(used) = entry.used_permille {
        let mut bucket = Bucket::new("use");
        bucket.limit = Some(1000);
        bucket.used = Some(u64::from(used));
        let mut report = UsageReport::new();
        report.buckets.push(bucket);
        standing = standing.usage(Usage::Known(report));
    }
    if let Some(at) = entry.last_used.filter(|at| at.is_finite()) {
        // Only compared with the other members' (the crate keeps no memory here), so
        // milliseconds keep the order of times that fall in one second.
        standing = standing.last_used((at * 1000.0).round() as i64);
    }
    standing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(priority: i64) -> Entry {
        Entry {
            priority,
            max_concurrent: None,
            active: 0,
            used_permille: None,
            last_used: None,
            unavailable: None,
        }
    }

    #[test]
    fn priorities_of_any_size_keep_their_order() {
        let entries = [entry(i64::MIN), entry(i64::MAX), entry(0)];
        assert_eq!(pick(PoolStrategy::Priority, &entries, None, 500), Some(1));
        let entries = [entry(5_000_000_000), entry(4_999_999_999)];
        assert_eq!(pick(PoolStrategy::Priority, &entries, None, 500), Some(0));
    }

    #[test]
    fn a_member_that_cannot_serve_is_passed_over_and_none_left_is_none() {
        let mut entries = [entry(9), entry(1)];
        entries[0].unavailable = Some("limited");
        assert_eq!(pick(PoolStrategy::Priority, &entries, None, 500), Some(1));
        entries[1].max_concurrent = Some(1);
        entries[1].active = 1;
        assert_eq!(pick(PoolStrategy::Priority, &entries, None, 500), None);
    }

    #[test]
    fn use_orders_a_member_and_never_keeps_it_out() {
        let mut entries = [entry(0), entry(0)];
        entries[0].used_permille = Some(1_000);
        entries[1].used_permille = Some(999);
        assert_eq!(pick(PoolStrategy::Headroom, &entries, None, 500), Some(1));
        entries[1].unavailable = Some("limited");
        assert_eq!(
            pick(PoolStrategy::Headroom, &entries, None, 500),
            Some(0),
            "a fully used member is still taken when it is all there is"
        );
    }

    #[test]
    fn a_member_with_no_reported_use_counts_as_the_unknown_figure() {
        let mut entries = [entry(0), entry(0)];
        entries[0].used_permille = Some(600);
        assert_eq!(pick(PoolStrategy::Headroom, &entries, None, 500), Some(1));
        assert_eq!(pick(PoolStrategy::Headroom, &entries, None, 700), Some(0));
    }

    #[test]
    fn round_robin_takes_the_longest_unused_and_ignores_priority() {
        let mut entries = [entry(9), entry(0), entry(0)];
        entries[0].last_used = Some(100.0);
        entries[1].last_used = Some(300.0);
        entries[2].last_used = Some(200.0);
        assert_eq!(pick(PoolStrategy::RoundRobin, &entries, None, 500), Some(0));
        entries[1].last_used = None;
        assert_eq!(pick(PoolStrategy::RoundRobin, &entries, None, 500), Some(1));
        entries[1].last_used = Some(100.0);
        assert_eq!(
            pick(PoolStrategy::RoundRobin, &entries, None, 500),
            Some(0),
            "equal use goes to the first listed, whatever the priority"
        );
    }

    #[test]
    fn the_preferred_member_wins_when_it_can_serve_and_is_not_forced_when_it_cannot() {
        let mut entries = [entry(9), entry(0)];
        assert_eq!(
            pick(PoolStrategy::Priority, &entries, Some(1), 500),
            Some(1)
        );
        entries[1].unavailable = Some("limited");
        assert_eq!(
            pick(PoolStrategy::Priority, &entries, Some(1), 500),
            Some(0)
        );
        assert_eq!(
            pick(PoolStrategy::Priority, &entries, Some(7), 500),
            Some(0)
        );
    }

    #[test]
    fn fractions_of_a_second_still_order_last_use() {
        let mut entries = [entry(0), entry(0)];
        entries[0].last_used = Some(10.75);
        entries[1].last_used = Some(10.25);
        assert_eq!(pick(PoolStrategy::RoundRobin, &entries, None, 500), Some(1));
    }
}

#[cfg(test)]
mod equivalence {
    use super::*;

    /// The order AI6 chose by before the crate did: the members with room, the preferred one when
    /// it has room, else the least by the strategy's keys, ties to the first listed.
    fn before(
        strategy: PoolStrategy,
        entries: &[Entry],
        prefer: Option<usize>,
        unknown_use: u16,
    ) -> Option<usize> {
        let room = |entry: &Entry| {
            entry.unavailable.is_none()
                && entry.max_concurrent.is_none_or(|most| entry.active < most)
        };
        let open: Vec<usize> = (0..entries.len())
            .filter(|at| room(&entries[*at]))
            .collect();
        if let Some(at) = prefer.filter(|at| open.contains(at)) {
            return Some(at);
        }
        let last = |entry: &Entry| entry.last_used.unwrap_or(f64::NEG_INFINITY);
        open.into_iter().min_by(|left, right| {
            let (l, r) = (&entries[*left], &entries[*right]);
            let by_last = last(l).total_cmp(&last(r));
            let by_priority = r.priority.cmp(&l.priority);
            match strategy {
                PoolStrategy::RoundRobin => by_last,
                PoolStrategy::Priority => by_priority,
                PoolStrategy::Headroom => l
                    .used_permille
                    .unwrap_or(unknown_use)
                    .cmp(&r.used_permille.unwrap_or(unknown_use))
                    .then(l.active.cmp(&r.active))
                    .then(by_last)
                    .then(by_priority),
            }
            .then(left.cmp(right))
        })
    }

    fn variants(full: bool) -> Vec<Entry> {
        let mut all = Vec::new();
        for priority in [0, 1] {
            for active in [0, 1] {
                for used_permille in [None, Some(300), Some(700)] {
                    for last_used in [None, Some(1.0), Some(2.5)] {
                        let caps: &[Option<usize>] = if full { &[None, Some(1)] } else { &[None] };
                        let outs: &[Option<&'static str>] = if full {
                            &[None, Some("limited")]
                        } else {
                            &[None]
                        };
                        for max_concurrent in caps {
                            for unavailable in outs {
                                all.push(Entry {
                                    priority,
                                    max_concurrent: *max_concurrent,
                                    active,
                                    used_permille,
                                    last_used,
                                    unavailable: *unavailable,
                                });
                            }
                        }
                    }
                }
            }
        }
        all
    }

    #[test]
    fn the_crate_picks_the_member_the_earlier_order_did_for_every_state_tried() {
        let strategies = [
            PoolStrategy::Headroom,
            PoolStrategy::RoundRobin,
            PoolStrategy::Priority,
        ];
        let mut tried = 0_u32;
        let mut check = |entries: &[Entry]| {
            for strategy in strategies {
                for prefer in [None, Some(1)] {
                    for unknown in [0, 500, 1_000] {
                        assert_eq!(
                            pick(strategy, entries, prefer, unknown),
                            before(strategy, entries, prefer, unknown),
                            "{strategy:?} prefer {prefer:?} unknown {unknown}: {entries:?}"
                        );
                        tried += 1;
                    }
                }
            }
        };
        let pairs = variants(true);
        for first in &pairs {
            for second in &pairs {
                check(&[first.clone(), second.clone()]);
            }
        }
        let triples = variants(false);
        for first in &triples {
            for second in triples.iter().step_by(5) {
                for third in triples.iter().step_by(7) {
                    check(&[first.clone(), second.clone(), third.clone()]);
                }
            }
        }
        assert!(tried > 100_000, "{tried}");
    }
}
