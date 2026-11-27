//! A pool and its members.

use alloc::string::String;
use alloc::vec::Vec;

use crate::settings::{Settings, Strategy};

/// One choice a pool can make: an agent with an account, a model and whatever else the host picks a start by. `T` is the host's own
/// payload; the library never looks inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Member<T> {
    /// The name the member is known by in this pool; unique in it.
    pub id: String,
    /// The host's description of what starting it means.
    pub payload: T,
    /// An opaque group; a session that moves prefers members of its current member's group (the host's harness).
    pub group: String,
    /// Higher goes first under [`Strategy::Priority`], and breaks a tie under [`Strategy::Headroom`]. Default 0.
    pub priority: i32,
    /// The runs it takes at once; `None` takes [`Settings::max_concurrent`].
    pub max_concurrent: Option<u32>,
    /// The use, in thousandths, at which it stops taking turns; `None` takes [`Settings::stop_permille`].
    pub stop_permille: Option<u16>,
    /// Opaque labels the host reads back.
    pub labels: Vec<String>,
}

impl<T> Member<T> {
    /// A member named `id` with default priority, no cap and no group.
    #[must_use]
    pub fn new(id: impl Into<String>, payload: T) -> Self {
        Self {
            id: id.into(),
            payload,
            group: String::new(),
            priority: 0,
            max_concurrent: None,
            stop_permille: None,
            labels: Vec::new(),
        }
    }

    /// Sets the group.
    #[must_use]
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = group.into();
        self
    }

    /// Sets the priority.
    #[must_use]
    pub const fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Sets the cap on runs at once.
    #[must_use]
    pub const fn max_concurrent(mut self, runs: u32) -> Self {
        self.max_concurrent = Some(runs);
        self
    }

    /// Sets the use at which it stops, in thousandths.
    #[must_use]
    pub const fn stop_permille(mut self, permille: u16) -> Self {
        self.stop_permille = Some(permille);
        self
    }

    /// Adds a label.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.labels.push(label.into());
        self
    }
}

/// A named, ordered set of members and how it chooses among them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Pool<T> {
    /// The pool's name; the key of its [`Memory`](crate::Memory).
    pub name: String,
    /// How it orders the members that can serve.
    pub strategy: Strategy,
    /// Its bounds.
    pub settings: Settings,
    /// The members, in the order they are listed.
    pub members: Vec<Member<T>>,
}

impl<T> Pool<T> {
    /// An empty pool with the default strategy and settings.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            strategy: Strategy::default(),
            settings: Settings::default(),
            members: Vec::new(),
        }
    }

    /// Sets the strategy.
    #[must_use]
    pub const fn strategy(mut self, strategy: Strategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Sets the settings.
    #[must_use]
    pub fn settings(mut self, settings: Settings) -> Self {
        self.settings = settings;
        self
    }

    /// Adds a member at the end.
    #[must_use]
    pub fn member(mut self, member: Member<T>) -> Self {
        self.members.push(member);
        self
    }

    /// The position of the member `id`.
    #[must_use]
    pub fn position(&self, id: &str) -> Option<usize> {
        self.members.iter().position(|member| member.id == id)
    }

    /// The member `id`.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Member<T>> {
        self.members.iter().find(|member| member.id == id)
    }
}
