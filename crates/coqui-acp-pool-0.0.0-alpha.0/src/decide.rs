//! The decision: which member of a pool takes a turn.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Reverse;
use core::fmt;
use core::time::Duration;

use coqui_acp_api::CacheLifetime;

use crate::memory::Memory;
use crate::pool::Pool;
use crate::settings::Strategy;
use crate::standing::{Reason, Skipped, Standing, judge, reading};
use crate::time::{after, utc};

/// What the turn is, as the client says it.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Ask<'a> {
    /// The member the session runs on now; `None` for a session that is starting.
    pub current: Option<&'a str>,
    /// The model the turn is on, when chosen. A bucket that names models and a turn with no model do not meet.
    pub model: Option<&'a str>,
    /// How long ago the session's last turn ended; `None` for a session that is starting.
    pub idle: Option<Duration>,
    /// How long the session's prompt cache lives.
    pub lifetime: CacheLifetime,
    /// The time, in Unix seconds.
    pub now: i64,
    /// Whether a running session stays on its member while it can serve. Default true.
    pub stick: bool,
    /// The asking session, so an [`Occupancy`](crate::Occupancy) does not count its own run.
    pub session: Option<&'a str>,
}

impl<'a> Ask<'a> {
    /// A session that is starting, at `now`.
    #[must_use]
    pub const fn new(now: i64) -> Self {
        Self {
            current: None,
            model: None,
            idle: None,
            lifetime: CacheLifetime::Default,
            now,
            stick: true,
            session: None,
        }
    }

    /// The member the session runs on now.
    #[must_use]
    pub const fn current(mut self, member: &'a str) -> Self {
        self.current = Some(member);
        self
    }

    /// The model the turn is on.
    #[must_use]
    pub const fn model(mut self, model: &'a str) -> Self {
        self.model = Some(model);
        self
    }

    /// How long ago the session's last turn ended.
    #[must_use]
    pub const fn idle(mut self, idle: Duration) -> Self {
        self.idle = Some(idle);
        self
    }

    /// The session's cache lifetime.
    #[must_use]
    pub const fn lifetime(mut self, lifetime: CacheLifetime) -> Self {
        self.lifetime = lifetime;
        self
    }

    /// Whether a running session stays on its member while it can serve.
    #[must_use]
    pub const fn stick(mut self, stick: bool) -> Self {
        self.stick = stick;
        self
    }

    /// The asking session.
    #[must_use]
    pub const fn session(mut self, session: &'a str) -> Self {
        self.session = Some(session);
        self
    }
}

/// How a turn came to run where it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Why {
    /// A session that starts takes its turn from the pool.
    Start,
    /// The session keeps its member.
    Kept,
    /// The turn is the strategy's choice again: a round-robin cache that had lapsed, or a session that does not stick.
    InTurn,
    /// The member it ran on cannot serve now.
    Exhausted,
}

impl Why {
    /// `start`, `kept`, `in_turn`, `exhausted`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Kept => "kept",
            Self::InTurn => "in_turn",
            Self::Exhausted => "exhausted",
        }
    }
}

/// The member a turn is given, and whether that moves the session.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Chosen {
    /// The member's id.
    pub member: String,
    /// Its position in the pool.
    pub index: usize,
    /// Whether the session moves.
    pub moved: bool,
    /// How it came to be chosen.
    pub why: Why,
    /// For a member that was left: what held it back (not set when it was unavailable).
    pub left: Option<String>,
    /// Whether the member's usage was reported for the model. Unknown usage leaves it usable.
    pub usage_known: bool,
    /// Whether its report was asked for and has not come.
    pub usage_pending: bool,
    /// Whether the member is in the group of the one the session ran on (always true for a session that starts). A move out of the
    /// group is a move to another harness.
    pub same_group: bool,
    /// Every member that could not serve, with the reason, in the pool's order.
    pub skipped: Vec<Skipped>,
}

/// No member can take the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NoneLeft {
    /// The pool.
    pub pool: String,
    /// The model, when the turn named one.
    pub model: Option<String>,
    /// The reason of each member, in the pool's order.
    pub reasons: Vec<Skipped>,
    /// When the soonest allowance refills, in Unix seconds, among the members an allowance holds back.
    pub soonest: Option<i64>,
    /// The time of the decision.
    pub now: i64,
}

impl NoneLeft {
    /// The refusal in words, calling a member `noun` (`account`, `member`).
    #[must_use]
    pub fn message(&self, noun: &str) -> String {
        if self.reasons.is_empty() {
            return format!("pool {} holds no {noun} this connection may use", self.pool);
        }
        let what = self.model.as_deref().map_or_else(
            || "a turn".to_string(),
            |model| format!("a turn on {model:?}"),
        );
        let reasons: Vec<String> = self
            .reasons
            .iter()
            .map(|skipped| format!("{}: {}", skipped.member, skipped.reason.text()))
            .collect();
        let mut said = format!(
            "no {noun} of pool {} can take {what}: {}",
            self.pool,
            reasons.join("; ")
        );
        if self.reasons.iter().any(|each| each.reason.is_allowance()) {
            match self.soonest {
                Some(at) => said.push_str(&format!(
                    "; the soonest allowance refills {} (at {})",
                    after(at - self.now),
                    utc(at)
                )),
                None => said.push_str("; nothing says when an allowance refills"),
            }
        }
        said
    }
}

impl fmt::Display for NoneLeft {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(&self.message("member"))
    }
}

impl core::error::Error for NoneLeft {}

/// How long a prompt cache lives, as each lifetime says: none for off, about five minutes for the short one and the agent's own
/// default, and about an hour for the long one.
#[must_use]
pub const fn lifetime_span(lifetime: CacheLifetime) -> Duration {
    match lifetime {
        CacheLifetime::Off => Duration::ZERO,
        CacheLifetime::Long => Duration::from_secs(3600),
        _ => Duration::from_secs(300),
    }
}

/// The member of `pool` that takes the turn `ask` describes. `standings` holds one [`Standing`] for each member, in the pool's
/// order; a missing one counts as a member with nothing known against it.
///
/// A member is eligible when, in this order, it is not unavailable, not at its cap of runs, has no refused turn that has not
/// lifted, has no bucket covering the model that is exhausted or at its stop point with its window still open, and has no hold. Usage
/// that is not reported leaves it eligible. A session that runs on an eligible member stays when `ask.stick` (for
/// [`Strategy::RoundRobin`], also while its cache is warm); any other turn takes the first eligible member in the strategy's order,
/// the current member's group before the others.
///
/// # Errors
///
/// [`NoneLeft`] when no member is eligible.
pub fn decide<T>(
    pool: &Pool<T>,
    memory: &mut Memory,
    standings: &[Standing],
    ask: &Ask<'_>,
) -> Result<Chosen, NoneLeft> {
    let fallback = Standing::new();
    let standing = |at: usize| standings.get(at).unwrap_or(&fallback);
    let reasons: Vec<Option<Reason>> = pool
        .members
        .iter()
        .enumerate()
        .map(|(at, member)| judge(member, standing(at), &pool.settings, ask.model, ask.now))
        .collect();
    let eligible = |at: usize| reasons[at].is_none();
    let current = ask
        .current
        .and_then(|name| pool.members.iter().position(|each| each.id == name));
    let cold = ask
        .idle
        .is_none_or(|idle| idle >= lifetime_span(ask.lifetime));
    let group = current.map(|at| pool.members[at].group.as_str());
    let finish = |memory: &mut Memory, taken: usize, why: Why, left: Option<String>| {
        let read = reading(standing(taken), ask.model, ask.now);
        let member = &pool.members[taken];
        if why == Why::Kept {
            memory.touch(&pool.name, &member.id, ask.now);
        } else {
            memory.took(&pool.name, &member.id, ask.now);
        }
        Chosen {
            member: member.id.clone(),
            index: taken,
            moved: Some(taken) != current,
            why,
            left,
            usage_known: read.permille.is_some(),
            usage_pending: read.pending,
            same_group: group.is_none_or(|group| group == member.group),
            skipped: reasons
                .iter()
                .enumerate()
                .filter_map(|(at, reason)| {
                    reason.as_ref().map(|reason| Skipped {
                        member: pool.members[at].id.clone(),
                        reason: reason.clone(),
                    })
                })
                .collect(),
        }
    };
    if let Some(at) = current
        && eligible(at)
        && ask.stick
        && (pool.strategy != Strategy::RoundRobin || !cold)
    {
        return Ok(finish(memory, at, Why::Kept, None));
    }
    let mut order = order_of(pool, memory, standings, ask, current);
    if let Some(group) = group {
        // Stable: the strategy's order holds inside each part.
        order.sort_by_key(|at| pool.members[*at].group != group);
    }
    let Some(taken) = order.into_iter().find(|at| eligible(*at)) else {
        return Err(NoneLeft {
            pool: pool.name.clone(),
            model: ask.model.map(str::to_string),
            reasons: reasons
                .iter()
                .enumerate()
                .map(|(at, reason)| Skipped {
                    member: pool.members[at].id.clone(),
                    reason: reason
                        .clone()
                        .unwrap_or_else(|| Reason::Unavailable("nothing holds it back".into())),
                })
                .collect(),
            soonest: reasons
                .iter()
                .flatten()
                .filter(|reason| reason.is_allowance())
                .filter_map(Reason::until)
                .min(),
            now: ask.now,
        });
    };
    let left = current
        .and_then(|at| reasons[at].as_ref())
        .filter(|reason| !matches!(reason, Reason::Unavailable(_)))
        .map(Reason::text);
    let why = match current {
        None => Why::Start,
        Some(at) if !eligible(at) => Why::Exhausted,
        Some(_) => Why::InTurn,
    };
    Ok(finish(memory, taken, why, left))
}

/// Every member's position, in the order the strategy would take them.
fn order_of<T>(
    pool: &Pool<T>,
    memory: &Memory,
    standings: &[Standing],
    ask: &Ask<'_>,
    current: Option<usize>,
) -> Vec<usize> {
    let count = pool.members.len();
    let mut order: Vec<usize> = (0..count).collect();
    let fallback = Standing::new();
    match pool.strategy {
        Strategy::Priority => {
            order.sort_by_key(|at| (Reverse(pool.members[*at].priority), *at));
        }
        Strategy::Headroom => {
            let key = |at: usize| {
                let standing = standings.get(at).unwrap_or(&fallback);
                let member = &pool.members[at];
                let used = reading(standing, ask.model, ask.now)
                    .permille
                    .unwrap_or(pool.settings.unknown_permille);
                let last = standing
                    .last_used
                    .into_iter()
                    .chain(memory.last_used(&pool.name, &member.id))
                    .max();
                (used, standing.active, last, Reverse(member.priority), at)
            };
            order.sort_by_key(|at| key(*at));
        }
        Strategy::LeastRecent => {
            let key = |at: usize| {
                let standing = standings.get(at).unwrap_or(&fallback);
                let member = &pool.members[at];
                let last = standing
                    .last_used
                    .into_iter()
                    .chain(memory.last_used(&pool.name, &member.id))
                    .max();
                (last, Reverse(member.priority), at)
            };
            order.sort_by_key(|at| key(*at));
        }
        Strategy::RoundRobin => {
            let after = memory
                .last(&pool.name)
                .and_then(|name| pool.members.iter().position(|each| each.id == name))
                .or(current);
            if let Some(after) = after {
                order = (1..=count).map(|step| (after + step) % count).collect();
            }
        }
    }
    order
}
