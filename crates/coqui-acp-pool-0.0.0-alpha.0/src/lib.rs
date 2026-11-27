//! The policy that picks which agent, account and model serves a session or a task from a pool, and moves a running session on when
//! its usage runs out. Pure: it does no I/O, reads no file, holds no secret and has no clock (every time is an argument, in Unix
//! seconds). A host builds [`Pool`]s from its own configuration and implements the seams ([`UsageSource`], [`Occupancy`], [`Holds`],
//! [`Availability`], [`Starter`], [`Ledger`]); the library decides.
//!
//! - [`decide`] takes a [`Pool`], a [`Memory`], one [`Standing`] for each member and an [`Ask`], and answers a [`Chosen`] or
//!   [`NoneLeft`]. [`Strategy`] orders the members that can serve: `priority` (the default), `headroom`, `round_robin` and `least_recent`; members equal on every key of a strategy go in the pool's listed order.
//! - [`Switchboard`] runs the seams around it: [`Switchboard::assign`] starts a new session, [`Switchboard::at_turn`] moves a running
//!   one when its turn ends out of usage (the same group first), with the history cleaned by [`clean`] and a continuation by
//!   [`resume`].
//! - [`TurnRecord`] (from `coqui-acp-history`) is what a [`Ledger`] receives for each served turn.
//! - [`accounts`] and [`check_pool`] are the pure parts of account pools; [`fits`] checks a model's capabilities; [`Cache`] keeps what
//!   a provider listed for a while.
//!
//! Every bound is a [`Settings`] value with a default and no floor; [`Settings::non_defaults`] shows a changed one.
//!
//! ```
//! use coqui_acp_pool::{Ask, Member, Memory, Pool, Standing, Strategy, decide};
//!
//! let pool = Pool::new("planner")
//!     .strategy(Strategy::Priority)
//!     .member(Member::new("claude", ()).group("claude"))
//!     .member(Member::new("codex", ()).group("codex"));
//! let standings = [Standing::new().unavailable("not signed in"), Standing::new()];
//! let chosen = decide(&pool, &mut Memory::new(), &standings, &Ask::new(1_800_000_000))?;
//! assert_eq!(chosen.member, "codex");
//! # Ok::<(), coqui_acp_pool::NoneLeft>(())
//! ```

#![forbid(unsafe_code)]
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod accounts;
mod cache;
mod check;
mod clean;
mod decide;
mod memory;
mod needs;
mod pool;
mod seams;
mod settings;
mod standing;
mod switchboard;
mod time;

#[cfg(feature = "std")]
pub use cache::SharedCache;
pub use cache::{Cache, DEFAULT_MAX_ENTRIES};
pub use check::{Known, PoolIssue, check_pool};
pub use clean::{
    Cleaned, Cleaning, DEFAULT_CONTINUATION, DEFAULT_MAX_WINDOW, Removed, Resume, clean, resume,
};
pub use decide::{Ask, Chosen, NoneLeft, Why, decide, lifetime_span};
#[cfg(feature = "std")]
pub use memory::SharedMemory;
pub use memory::{Keeper, Memory, Remembered};
pub use needs::{Fit, fits};
pub use pool::{Member, Pool};
pub use seams::{
    Availability, Basics, Carried, Carry, Holds, Ledger, Occupancy, Started, Starter, UsageSource,
};
pub use settings::{NonDefault, Settings, Strategy, UnknownStrategy};
pub use standing::{Hold, Reason, Skipped, Standing, Usage};
pub use switchboard::{Assigned, AtTurn, Ended, HowEnded, Moved, Switchboard, Unassigned};
pub use time::{after, utc};

pub use coqui_acp_history::{Cost, TagError, Tokens, TurnOutcome, TurnRecord};
