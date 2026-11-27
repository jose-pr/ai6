//! A map whose entries stand for a while, with the time passed in. A pool asked at every turn keeps what a provider listed and
//! reported for a little, so no program starts for each member each time.

use alloc::collections::BTreeMap;
use alloc::string::String;

/// The most entries a cache holds unless told otherwise.
pub const DEFAULT_MAX_ENTRIES: usize = 1024;

/// Values by key, each good for `life` seconds from when it was put.
#[derive(Debug, Clone)]
pub struct Cache<T> {
    life: i64,
    max_entries: usize,
    held: BTreeMap<String, (i64, T)>,
}

impl<T> Cache<T> {
    /// A cache whose entries stand for `life` seconds.
    #[must_use]
    pub const fn new(life: i64) -> Self {
        Self {
            life,
            max_entries: DEFAULT_MAX_ENTRIES,
            held: BTreeMap::new(),
        }
    }

    /// Sets the most entries held; the oldest go first.
    #[must_use]
    pub const fn max_entries(mut self, max: usize) -> Self {
        self.max_entries = max;
        self
    }

    /// The value under `key`, when it was put less than `life` seconds before `now`.
    #[must_use]
    pub fn get(&self, key: &str, now: i64) -> Option<&T> {
        self.held
            .get(key)
            .filter(|(at, _)| now.saturating_sub(*at) < self.life)
            .map(|(_, value)| value)
    }

    /// Keeps `value` under `key`, put at `now`.
    pub fn put(&mut self, key: impl Into<String>, value: T, now: i64) {
        self.held.insert(key.into(), (now, value));
        while self.held.len() > self.max_entries {
            let oldest = self
                .held
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => self.held.remove(&key),
                None => break,
            };
        }
    }

    /// Drops every entry, returning how many there were.
    pub fn forget(&mut self) -> usize {
        let count = self.held.len();
        self.held.clear();
        count
    }

    /// How many entries are held, stale ones included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

/// A [`Cache`] behind a mutex, for a process.
#[cfg(feature = "std")]
#[derive(Debug)]
pub struct SharedCache<T>(std::sync::Mutex<Cache<T>>);

#[cfg(feature = "std")]
impl<T: Clone> SharedCache<T> {
    /// A shared cache whose entries stand for `life` seconds.
    #[must_use]
    pub const fn new(life: i64) -> Self {
        Self(std::sync::Mutex::new(Cache::new(life)))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cache<T>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A copy of the value under `key` that is still good at `now`.
    #[must_use]
    pub fn get(&self, key: &str, now: i64) -> Option<T> {
        self.lock().get(key, now).cloned()
    }

    /// Keeps `value` under `key`.
    pub fn put(&self, key: impl Into<String>, value: T, now: i64) {
        self.lock().put(key, value, now);
    }

    /// Drops every entry, returning how many there were.
    pub fn forget(&self) -> usize {
        self.lock().forget()
    }
}
