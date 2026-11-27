//! The switchboard: assigns a member to a new session, and moves a running one on when its turn ends out of usage.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::time::Duration;

use coqui_acp_api::{CacheLifetime, LimitHit};
use coqui_acp_history::{LossReport, TurnOutcome, TurnRecord};

use crate::clean::Resume;
use crate::decide::{Ask, Chosen, NoneLeft, Why, decide};
use crate::memory::Keeper;
use crate::pool::{Member, Pool};
use crate::seams::{Availability, Carry, Holds, Ledger, Occupancy, Started, Starter, UsageSource};
use crate::standing::{Standing, Usage};

/// A member left out for the length of one operation, with the words that say why.
type Excluded = Vec<(String, String)>;

/// A pool with the seams that fill it in. Build it with [`Switchboard::new`] and add the seams a host has.
pub struct Switchboard<'a, T> {
    pool: &'a Pool<T>,
    memory: &'a dyn Keeper,
    availability: &'a dyn Availability<T>,
    starter: &'a dyn Starter<T>,
    usage: Option<&'a dyn UsageSource<T>>,
    occupancy: Option<&'a dyn Occupancy<T>>,
    holds: Option<&'a dyn Holds<T>>,
    ledger: Option<&'a dyn Ledger>,
}

impl<T> fmt::Debug for Switchboard<'_, T> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Switchboard")
            .field("pool", &self.pool.name)
            .finish_non_exhaustive()
    }
}

/// A session that was started.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Assigned {
    /// The member that was chosen and started.
    pub chosen: Chosen,
    /// What the host started.
    pub started: Started,
}

/// A new session that could not be assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Unassigned(pub NoneLeft);

impl fmt::Display for Unassigned {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(out)
    }
}

impl core::error::Error for Unassigned {}

/// How a turn ended, as the host read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HowEnded {
    /// A limit ended the turn (the agent's error or refusal named one, or a usage notice showed a bucket used up before it closed
    /// with a failure). `until` is when the limit lifts, when the text said.
    Limit {
        /// When it lifts, in Unix seconds.
        until: Option<i64>,
    },
    /// The turn ended well. The member may still have a bucket used up now, which the standing says.
    Finished,
    /// It ended for another reason (cancelled, out of context tokens, a plain error): not a limit, and nothing moves.
    Other,
}

/// A turn that ended, for [`Switchboard::at_turn`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Ended<'a> {
    /// The session.
    pub session: &'a str,
    /// The member it ran on.
    pub current: &'a str,
    /// The model it ran on.
    pub model: Option<&'a str>,
    /// The time, in Unix seconds.
    pub now: i64,
    /// What ended it.
    pub how: HowEnded,
    /// The role's own continuation text, when it set one.
    pub resume_text: Option<&'a str>,
    /// What the harness reported of the limit: its kind, when it lifts, whether the work can go on elsewhere and which messages
    /// are the limit's words. It dates the member's hold when `how` names no time, stops a move that cannot help (a context
    /// window), reaches `carry` for the cleaner and the ledger.
    pub report: Option<LimitHit>,
    /// What the host knows of the turn: agent, account, effort, role, profile, tokens, cost, duration, tags. The switchboard adds
    /// the pool, member, why and the move.
    pub record: TurnRecord,
}

impl<'a> Ended<'a> {
    /// A turn of `session` on `current` that ended `how` at `now`.
    #[must_use]
    pub fn new(session: &'a str, current: &'a str, how: HowEnded, now: i64) -> Self {
        Self {
            session,
            current,
            model: None,
            now,
            how,
            resume_text: None,
            report: None,
            record: TurnRecord::new(now),
        }
    }

    /// The model it ran on.
    #[must_use]
    pub const fn model(mut self, model: &'a str) -> Self {
        self.model = Some(model);
        self
    }

    /// The role's own continuation text.
    #[must_use]
    pub const fn resume_text(mut self, text: &'a str) -> Self {
        self.resume_text = Some(text);
        self
    }

    /// What the harness reported of the limit.
    #[must_use]
    pub fn report(mut self, report: LimitHit) -> Self {
        self.report = Some(report);
        self
    }

    /// What the host knows of the turn.
    #[must_use]
    pub fn record(mut self, record: TurnRecord) -> Self {
        self.record = record;
        self
    }
}

/// A session that moved.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Moved {
    /// The member it left.
    pub from: String,
    /// The member it is on now.
    pub chosen: Chosen,
    /// Whether it went as a replay in the first prompt, not as the whole session.
    pub replayed: bool,
    /// What the import could not carry.
    pub loss: LossReport,
    /// What was sent to continue.
    pub resume: Resume,
    /// Why the continuation could not be sent, when it could not.
    pub resume_error: Option<String>,
    /// Whether a limit's text stayed in the history.
    pub limit_text_left: bool,
}

/// What happened at a turn's end.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AtTurn {
    /// The session stays where it is.
    Stayed,
    /// It moved.
    Moved(Box<Moved>),
    /// A limit ended the turn that no other member helps with (a context window): the session stays, and the report says why.
    NotContinuable(Box<LimitHit>),
    /// It cannot move: no member can take it. The session stays, and the person's next prompt is refused with this.
    NoneLeft(NoneLeft),
    /// The member the session ran on is not in the pool.
    Unknown(String),
}

impl<'a, T> Switchboard<'a, T> {
    /// A switchboard over `pool` that keeps its round-robin cursor in `memory`, asks `availability` of each member and starts and
    /// moves sessions through `starter`.
    #[must_use]
    pub fn new(
        pool: &'a Pool<T>,
        memory: &'a dyn Keeper,
        availability: &'a dyn Availability<T>,
        starter: &'a dyn Starter<T>,
    ) -> Self {
        Self {
            pool,
            memory,
            availability,
            starter,
            usage: None,
            occupancy: None,
            holds: None,
            ledger: None,
        }
    }

    /// Reads usage from `source`; without one every member's usage is unknown.
    #[must_use]
    pub fn usage(mut self, source: &'a dyn UsageSource<T>) -> Self {
        self.usage = Some(source);
        self
    }

    /// Counts runs through `occupancy`; without one no member is busy.
    #[must_use]
    pub fn occupancy(mut self, occupancy: &'a dyn Occupancy<T>) -> Self {
        self.occupancy = Some(occupancy);
        self
    }

    /// Reads holds from `holds`; without one there are none.
    #[must_use]
    pub fn holds(mut self, holds: &'a dyn Holds<T>) -> Self {
        self.holds = Some(holds);
        self
    }

    /// Sends every record to `ledger`.
    #[must_use]
    pub fn ledger(mut self, ledger: &'a dyn Ledger) -> Self {
        self.ledger = Some(ledger);
        self
    }

    /// The standing of each member for `ask`. `moving` is a running session's move, which only a member whose history can be carried
    /// may take.
    fn standings(&self, ask: &Ask<'_>, moving: bool, excluded: &Excluded) -> Vec<Standing> {
        self.pool
            .members
            .iter()
            .map(|member| {
                let basics = self.availability.availability(member, ask);
                let mut standing = Standing::new();
                standing.unavailable = basics.unavailable;
                standing.limited_until = basics.limited_until;
                standing.carries = basics.carries;
                if moving
                    && !basics.carries
                    && standing.unavailable.is_none()
                    && ask.current != Some(member.id.as_str())
                {
                    standing.unavailable =
                        Some("its harness cannot take a running session's history".to_string());
                }
                if let Some((_, why)) = excluded.iter().find(|(id, _)| *id == member.id)
                    && standing.unavailable.is_none()
                {
                    standing.unavailable = Some(why.clone());
                }
                if standing.unavailable.is_none() {
                    standing.usage = self
                        .usage
                        .map_or(Usage::Unknown, |source| source.report(member));
                    standing.holds = self
                        .holds
                        .map(|holds| holds.holds(member, ask.now))
                        .unwrap_or_default();
                    if let Some(occupancy) = self.occupancy {
                        standing.active = occupancy.active(member, ask.session);
                        standing.last_used = occupancy.last_used(member);
                    }
                }
                standing
            })
            .collect()
    }

    fn decide_with(
        &self,
        standings: &[Standing],
        ask: &Ask<'_>,
        preview: bool,
    ) -> Result<Chosen, NoneLeft> {
        let mut result = None;
        self.memory.keep(&mut |memory| {
            result = Some(if preview {
                decide(self.pool, &mut memory.clone(), standings, ask)
            } else {
                decide(self.pool, memory, standings, ask)
            });
        });
        result.unwrap_or_else(|| {
            Err(NoneLeft {
                pool: self.pool.name.clone(),
                model: None,
                reasons: Vec::new(),
                soonest: None,
                now: ask.now,
            })
        })
    }

    /// What `ask` would be given, with nothing started and nothing remembered: the answer of `roles/resolve`. The skipped members and
    /// their reasons are in [`Chosen::skipped`].
    ///
    /// # Errors
    ///
    /// [`NoneLeft`] when no member can serve.
    pub fn resolve(&self, ask: &Ask<'_>) -> Result<Chosen, NoneLeft> {
        let moving = ask.current.is_some();
        self.decide_with(&self.standings(ask, moving, &Vec::new()), ask, true)
    }

    /// Chooses a member for a new session and starts it. A member that cannot start is left out and the pool decides again, once
    /// for each member. `record` is what the host knows of the turn; the pool, member, why and usage are added before it goes to the
    /// ledger.
    ///
    /// # Errors
    ///
    /// [`Unassigned`] when no member is left; each member's reason is in it, those that failed to start included.
    pub fn assign(&self, ask: &Ask<'_>, record: TurnRecord) -> Result<Assigned, Unassigned> {
        let mut excluded = Excluded::new();
        for _ in 0..=self.pool.members.len() {
            let standings = self.standings(ask, false, &excluded);
            let chosen = self
                .decide_with(&standings, ask, false)
                .map_err(Unassigned)?;
            let member = &self.pool.members[chosen.index];
            match self.starter.start(member, ask) {
                Ok(started) => {
                    let mut record = record;
                    record.session = Some(started.session.clone());
                    self.write(record, member, ask, &chosen, None);
                    return Ok(Assigned { chosen, started });
                }
                Err(why) => excluded.push((chosen.member, format!("could not start: {why}"))),
            }
        }
        let standings = self.standings(ask, false, &excluded);
        Err(Unassigned(
            self.decide_with(&standings, ask, true)
                .expect_err("every member was left out"),
        ))
    }

    /// The member the session would move to now that its turn ended, with nothing carried: for a host that asks the person first
    /// (`handover = ask`, a move out of [`Chosen::same_group`]). `Ok(None)` when it stays.
    ///
    /// # Errors
    ///
    /// [`NoneLeft`] when it cannot stay and no member can take it.
    pub fn choose_next(&self, ended: &Ended<'_>) -> Result<Option<Chosen>, NoneLeft> {
        self.next(ended, &Excluded::new(), true)
    }

    fn next(
        &self,
        ended: &Ended<'_>,
        excluded: &Excluded,
        preview: bool,
    ) -> Result<Option<Chosen>, NoneLeft> {
        // A turn's end moves a session only when its member cannot serve: a warm cache and the stick rule hold it otherwise.
        let mut ask = Ask::new(ended.now)
            .current(ended.current)
            .idle(Duration::ZERO)
            .lifetime(CacheLifetime::Long)
            .session(ended.session);
        ask.model = ended.model;
        let mut standings = self.standings(&ask, true, excluded);
        if let (HowEnded::Limit { until }, Some(at)) =
            (ended.how, self.pool.position(ended.current))
        {
            // The time the text gave, else the harness's own: its reset, or the wait it asked for.
            let said = ended.report.as_ref().and_then(|report| {
                report.resets_at.or_else(|| {
                    report.retry_after.map(|wait| {
                        ended
                            .now
                            .saturating_add(i64::try_from(wait.as_secs()).unwrap_or(i64::MAX))
                    })
                })
            });
            let until = until
                .or(said)
                .filter(|until| *until > ended.now)
                .unwrap_or(i64::MAX);
            if standings[at].unavailable.is_none() {
                standings[at].limited_until = Some(until);
            }
        }
        let chosen = self.decide_with(&standings, &ask, preview)?;
        Ok((chosen.why != Why::Kept).then_some(chosen))
    }

    /// Called when a turn ends. A turn that ended for another reason moves nothing. Otherwise the pool decides with the session's
    /// member left out for what ran out; on a move the host's [`Starter::carry`] runs (export, clean, import), a member whose carry is
    /// refused is left out and the pool decides again, and the continuation is sent. The move is recorded.
    pub fn at_turn(&self, ended: &Ended<'_>) -> AtTurn {
        let Some(from) = self.pool.get(ended.current) else {
            return AtTurn::Unknown(ended.current.to_string());
        };
        if ended.how == HowEnded::Other {
            return AtTurn::Stayed;
        }
        if let Some(report) = ended
            .report
            .as_ref()
            .filter(|report| !report.can_continue())
            && matches!(ended.how, HowEnded::Limit { .. })
        {
            return AtTurn::NotContinuable(Box::new(report.clone()));
        }
        let mut excluded = Excluded::new();
        for _ in 0..=self.pool.members.len() {
            let chosen = match self.next(ended, &excluded, false) {
                Ok(Some(chosen)) => chosen,
                Ok(None) => return AtTurn::Stayed,
                Err(none) => return AtTurn::NoneLeft(none),
            };
            let to = &self.pool.members[chosen.index];
            let (carried, replayed) =
                match self
                    .starter
                    .carry(ended.session, from, to, ended.report.as_ref())
                {
                    Carry::Whole(carried) => (carried, false),
                    Carry::Replayed(carried) => (carried, true),
                    Carry::Refused(why) => {
                        excluded.push((
                            chosen.member,
                            format!("its history cannot be carried: {why}"),
                        ));
                        continue;
                    }
                };
            let by_limit = matches!(ended.how, HowEnded::Limit { .. });
            let resume = crate::clean::resume(
                ended.resume_text,
                (carried.had_work, carried.prompt.as_deref()),
                by_limit,
            );
            let resume_error = match &resume {
                Resume::Nothing => None,
                _ => self.starter.resume(ended.session, to, &resume).err(),
            };
            let mut record = ended.record.clone();
            record.session = Some(ended.session.to_string());
            let ask = Ask::new(ended.now);
            self.write(
                record,
                to,
                &ask,
                &chosen,
                Some((
                    &from.id,
                    by_limit,
                    ended.report.as_ref().filter(|_| by_limit),
                )),
            );
            return AtTurn::Moved(Box::new(Moved {
                from: from.id.clone(),
                chosen,
                replayed,
                loss: carried.loss,
                resume,
                resume_error,
                limit_text_left: carried.limit_text_left,
            }));
        }
        match self.next(ended, &excluded, true) {
            Err(none) => AtTurn::NoneLeft(none),
            Ok(_) => AtTurn::Stayed,
        }
    }

    /// Sends the record of one served turn or start to the ledger, completed with what the pool knows.
    fn write(
        &self,
        mut record: TurnRecord,
        member: &Member<T>,
        ask: &Ask<'_>,
        chosen: &Chosen,
        moved: Option<(&str, bool, Option<&LimitHit>)>,
    ) {
        let Some(ledger) = self.ledger else {
            return;
        };
        record.at = ask.now;
        record.pool = Some(self.pool.name.clone());
        record.member = Some(member.id.clone());
        if record.harness.is_none() && !member.group.is_empty() {
            record.harness = Some(member.group.clone());
        }
        if record.model.is_none() {
            record.model = ask.model.map(str::to_string);
        }
        record.why = Some(chosen.why.name().to_string());
        record.usage_known = Some(chosen.usage_known);
        if let Some((from, by_limit, limit)) = moved {
            record.moved_from = Some(from.to_string());
            let outcome = record.outcome.get_or_insert_with(TurnOutcome::default);
            outcome.moved_on = true;
            outcome.limit = by_limit;
            if let Some(limit) = limit {
                // Tagged by this crate, so usage can be summed by what kind of limit moved a session.
                if let Some(kind) = &limit.kind {
                    let _ = record.tag("pool:limit_kind", kind.as_str());
                }
                if let Some(bucket) = &limit.bucket {
                    let _ = record.tag("pool:limit_bucket", bucket.as_str());
                }
            }
        }
        ledger.record(&record);
    }
}
