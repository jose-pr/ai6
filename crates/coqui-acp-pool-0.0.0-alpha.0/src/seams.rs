//! The seams a host implements. The library does no I/O: usage, runs, holds, availability, starting and moving a session and the
//! record of what happened are all the host's, behind these traits. Every method answers at once; none is awaited.

use alloc::string::String;
use alloc::vec::Vec;

use coqui_acp_api::LimitHit;
use coqui_acp_history::{LossReport, TurnRecord};
use serde_json::Value;

use crate::decide::Ask;
use crate::pool::Member;
use crate::standing::{Hold, Usage};

/// Where a member's usage comes from. Canoa's reads its providers' reports (cached); AI6's reads a local report or asks another
/// station and answers [`Usage::Pending`] until the answer comes. The library never waits.
pub trait UsageSource<T> {
    /// What `member`'s provider reports of its allowances.
    fn report(&self, member: &Member<T>) -> Usage;
}

/// How busy each member is.
pub trait Occupancy<T> {
    /// The runs on `member`, not counting the session `excluding`, which is the one asking.
    fn active(&self, member: &Member<T>, excluding: Option<&str>) -> u32;

    /// When `member` last took a turn, in Unix seconds.
    fn last_used(&self, member: &Member<T>) -> Option<i64>;
}

/// What holds a member back besides usage, such as a spending limit's stop. The host decides the scope (account, profile, operation,
/// a glob); the library filters only by what it is given.
pub trait Holds<T> {
    /// The holds on `member` at `now`.
    fn holds(&self, member: &Member<T>, now: i64) -> Vec<Hold>;
}

/// What the host knows of a member that is not usage, runs or a hold.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Basics {
    /// Why it cannot serve whatever it has left (program not installed, account missing or not signed in, held by another
    /// connection, model not listed or lacking a needed capability, not offered to this connection).
    pub unavailable: Option<String>,
    /// Until when a turn a limit refused holds it back, in Unix seconds; `i64::MAX` when the refusal said no time.
    pub limited_until: Option<i64>,
    /// Whether a running session's history can move onto it.
    pub carries: bool,
}

impl Basics {
    /// A member that can serve and takes a running session's history.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            unavailable: None,
            limited_until: None,
            carries: true,
        }
    }

    /// The same, unavailable for `reason`.
    #[must_use]
    pub fn unavailable(mut self, reason: impl Into<String>) -> Self {
        self.unavailable = Some(reason.into());
        self
    }

    /// The same, held back until `until`.
    #[must_use]
    pub const fn limited_until(mut self, until: i64) -> Self {
        self.limited_until = Some(until);
        self
    }

    /// The same, with a harness that cannot take a running session's history.
    #[must_use]
    pub const fn cannot_carry(mut self) -> Self {
        self.carries = false;
        self
    }
}

impl Default for Basics {
    fn default() -> Self {
        Self::new()
    }
}

/// The host's facts about each member.
pub trait Availability<T> {
    /// What is known of `member` for the turn `ask` describes.
    fn availability(&self, member: &Member<T>, ask: &Ask<'_>) -> Basics;
}

/// A session the host started.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Started {
    /// The session's id as the host knows it.
    pub session: String,
}

impl Started {
    /// A started session.
    #[must_use]
    pub fn new(session: impl Into<String>) -> Self {
        Self {
            session: session.into(),
        }
    }
}

/// What a move carried. The host's `carry` does the export, the cleaning, and the import; this is what it says of them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Carried {
    /// What the import could not carry.
    pub loss: LossReport,
    /// Whether the interrupted turn held any work of the agent's after cleaning ([`Cleaned::had_work`](crate::Cleaned)).
    pub had_work: bool,
    /// The interrupted turn's prompt blocks, to send again when it had no work.
    pub prompt: Option<Vec<Value>>,
    /// Whether a limit's text could not be recognised and stayed in the history.
    pub limit_text_left: bool,
}

impl Carried {
    /// A move that lost `loss` and found `had_work` in the interrupted turn.
    #[must_use]
    pub const fn new(loss: LossReport, had_work: bool) -> Self {
        Self {
            loss,
            had_work,
            prompt: None,
            limit_text_left: false,
        }
    }

    /// The same, with the prompt to send again.
    #[must_use]
    pub fn prompt(mut self, prompt: Vec<Value>) -> Self {
        self.prompt = Some(prompt);
        self
    }

    /// The same, saying the limit's text stayed.
    #[must_use]
    pub const fn limit_text_left(mut self) -> Self {
        self.limit_text_left = true;
        self
    }
}

/// How a session moved.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Carry {
    /// The whole session was exported and imported.
    Whole(Carried),
    /// Only the newest turns that fit went, as context in the first prompt of a session of the new member's own.
    Replayed(Carried),
    /// It could not move; the member is left out for this session.
    Refused(String),
}

/// Starts sessions and moves them. Canoa's runs a move in the host (`history/account`, `history/move`, the cleaner) and its
/// delegation one opens a child session; AI6's starts a run.
pub trait Starter<T> {
    /// Starts a session on `member` for the turn `ask` describes.
    ///
    /// # Errors
    ///
    /// Why it could not start; the pool excludes the member and decides again.
    fn start(&self, member: &Member<T>, ask: &Ask<'_>) -> Result<Started, String>;

    /// Moves the running `session` from one member to another: export, clean ([`clean`](crate::clean), given `report`, which names
    /// the messages that are the limit's own words), import.
    fn carry(
        &self,
        session: &str,
        from: &Member<T>,
        to: &Member<T>,
        report: Option<&LimitHit>,
    ) -> Carry;

    /// Sends the new member what continues the work.
    ///
    /// # Errors
    ///
    /// Why it could not be sent.
    fn resume(
        &self,
        session: &str,
        to: &Member<T>,
        resume: &crate::clean::Resume,
    ) -> Result<(), String>;
}

/// Where the record of each served turn or start goes. Storing and querying records is the host's.
pub trait Ledger {
    /// Takes one record.
    fn record(&self, record: &TurnRecord);
}
