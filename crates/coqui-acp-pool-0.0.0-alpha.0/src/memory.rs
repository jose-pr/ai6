//! What the pools remember between decisions: the last member each pool gave a turn to, and when each member last took one. A plain
//! value the host owns; with the `std` feature, [`SharedMemory`] holds one behind a mutex for a process.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

/// What one pool remembers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Pooled {
    last: Option<String>,
    used: BTreeMap<String, i64>,
}

/// One pool's memory, as [`Memory::export`] hands it over and [`Memory::restore`] takes it back, so a host can keep it over a
/// restart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Remembered {
    /// The pool.
    pub pool: String,
    /// The member that took the last turn, for round-robin.
    pub last: Option<String>,
    /// Each member's last use, in Unix seconds.
    pub used: Vec<(String, i64)>,
}

impl Remembered {
    /// A pool's memory with nothing in it.
    #[must_use]
    pub fn new(pool: impl Into<String>) -> Self {
        Self {
            pool: pool.into(),
            ..Self::default()
        }
    }
}

/// The round-robin cursor and each member's last use, per pool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Memory {
    pools: BTreeMap<String, Pooled>,
}

impl Memory {
    /// An empty memory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The member that took the last turn of `pool`.
    #[must_use]
    pub fn last(&self, pool: &str) -> Option<&str> {
        self.pools.get(pool)?.last.as_deref()
    }

    /// When `member` of `pool` last took a turn, in Unix seconds.
    #[must_use]
    pub fn last_used(&self, pool: &str, member: &str) -> Option<i64> {
        self.pools.get(pool)?.used.get(member).copied()
    }

    /// Notes that `member` of `pool` took a turn at `at`.
    pub fn took(&mut self, pool: &str, member: &str, at: i64) {
        let held = self.pools.entry(pool.into()).or_default();
        held.last = Some(member.into());
        held.used.insert(member.into(), at);
    }

    /// Notes that `member` of `pool` was used again at `at`, which leaves the round-robin cursor where it is.
    pub fn touch(&mut self, pool: &str, member: &str, at: i64) {
        self.pools
            .entry(pool.into())
            .or_default()
            .used
            .insert(member.into(), at);
    }

    /// Forgets `pool`, as when it is removed from the configuration.
    pub fn forget(&mut self, pool: &str) {
        self.pools.remove(pool);
    }

    /// Everything remembered, to keep.
    #[must_use]
    pub fn export(&self) -> Vec<Remembered> {
        self.pools
            .iter()
            .map(|(pool, held)| Remembered {
                pool: pool.clone(),
                last: held.last.clone(),
                used: held.used.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            })
            .collect()
    }

    /// A memory made of what [`export`](Self::export) gave.
    #[must_use]
    pub fn restore(items: impl IntoIterator<Item = Remembered>) -> Self {
        let pools = items
            .into_iter()
            .map(|item| {
                (
                    item.pool,
                    Pooled {
                        last: item.last,
                        used: item.used.into_iter().collect(),
                    },
                )
            })
            .collect();
        Self { pools }
    }
}

/// Something that lends a [`Memory`] for the length of one closure, so a decision holds it only while it runs and never while a
/// session starts.
pub trait Keeper {
    /// Runs `with` on the memory.
    fn keep(&self, with: &mut dyn FnMut(&mut Memory));
}

impl Keeper for RefCell<Memory> {
    fn keep(&self, with: &mut dyn FnMut(&mut Memory)) {
        with(&mut self.borrow_mut());
    }
}

/// One [`Memory`] behind a mutex, shared by every pool of a process.
#[cfg(feature = "std")]
#[derive(Debug, Default)]
pub struct SharedMemory(std::sync::Mutex<Memory>);

#[cfg(feature = "std")]
impl SharedMemory {
    /// An empty shared memory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `with` on the memory.
    pub fn with<R>(&self, with: impl FnOnce(&mut Memory) -> R) -> R {
        with(
            &mut self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

#[cfg(feature = "std")]
impl Keeper for SharedMemory {
    fn keep(&self, with: &mut dyn FnMut(&mut Memory)) {
        with(
            &mut self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }
}
