//! Short-lived caches in front of a directory or an authorizer, to absorb bursts.
//!
//! [`Cached`] wraps a provider and answers a repeated [`Lookup`] or authorization from memory
//! for [`CacheOptions::ttl`], or [`negative_ttl`](CacheOptions::negative_ttl) for an answer
//! of "no such person" or "denied". A credential is never cached: [`Cached`] does not
//! implement `Authenticator`. An error is never cached either.
//!
//! Needs the `cache` feature.

use core::fmt;
use core::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};

use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, Schema, ToValue};
use coqui_access::{
    AccountInfo, AccountPut, Accounts, AuthorizationDecision, AuthorizationRequest, Authorizer,
    Description, Directory, Lookup, Person, PutOutcome, Waiting,
};
use moka::Expiry;
use moka::sync::Cache;

/// How long answers are kept, and how many.
///
/// The default keeps a found person or a grant 5 s, "not found" or a denial 1 s, and 10 000
/// entries of each kind: a burst is answered once, and a change at the source is seen within
/// seconds. A removed account or a withdrawn permission can still pass for up to `ttl` after
/// the source changes unless [`Cached::invalidate`] is called; a new one can be refused for
/// up to `negative_ttl`. As configuration, the durations are seconds (`1.5` allowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao)]
#[non_exhaustive]
pub struct CacheOptions {
    /// How long a person found or a grant is kept.
    #[map(default = DEFAULT_TTL)]
    pub ttl: Duration,
    /// How long "no such person" or a denial is kept.
    #[map(default = DEFAULT_NEGATIVE_TTL)]
    pub negative_ttl: Duration,
    /// The most answers kept of each kind (lookups, decisions); `0` keeps none.
    #[map(default = DEFAULT_MAX_ENTRIES)]
    pub max_entries: u64,
}

/// [`CacheOptions::ttl`] by default: 5 s.
pub const DEFAULT_TTL: Duration = Duration::from_secs(5);
/// [`CacheOptions::negative_ttl`] by default: 1 s.
pub const DEFAULT_NEGATIVE_TTL: Duration = Duration::from_secs(1);
/// [`CacheOptions::max_entries`] by default.
pub const DEFAULT_MAX_ENTRIES: u64 = 10_000;

impl Default for CacheOptions {
    fn default() -> Self {
        Self {
            ttl: DEFAULT_TTL,
            negative_ttl: DEFAULT_NEGATIVE_TTL,
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }
}

impl CacheOptions {
    /// Nothing is kept: every call reaches the provider.
    pub const fn off() -> Self {
        Self {
            ttl: Duration::ZERO,
            negative_ttl: Duration::ZERO,
            max_entries: 0,
        }
    }

    /// These options with `ttl`.
    #[must_use]
    pub const fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// These options with `negative_ttl`.
    #[must_use]
    pub const fn negative_ttl(mut self, negative_ttl: Duration) -> Self {
        self.negative_ttl = negative_ttl;
        self
    }

    /// These options with `max_entries`.
    #[must_use]
    pub const fn max_entries(mut self, max_entries: u64) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// Whether answers are kept longer than the defaults, which widens the window in which a
    /// revoked account or permission still passes: data for the host to report, not a
    /// refusal.
    pub fn exceeds_default(&self) -> bool {
        self.ttl > DEFAULT_TTL || self.negative_ttl > DEFAULT_NEGATIVE_TTL
    }

    fn keeps_anything(&self) -> bool {
        self.max_entries > 0 && !(self.ttl.is_zero() && self.negative_ttl.is_zero())
    }
}

/// A cache as configuration: `true` (the defaults), `false` (off), or [`CacheOptions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, untagged)]
pub enum CacheSetting {
    /// On with the defaults, or off.
    Enabled(bool),
    /// On with these options.
    Tuned(CacheOptions),
}

impl Default for CacheSetting {
    fn default() -> Self {
        Self::Enabled(true)
    }
}

impl CacheSetting {
    /// The options this setting means.
    pub fn options(&self) -> CacheOptions {
        match *self {
            Self::Enabled(true) => CacheOptions::default(),
            Self::Enabled(false) => CacheOptions::off(),
            Self::Tuned(options) => options,
        }
    }
}

/// Keeps a positive answer `ttl` and a negative one `negative_ttl`.
struct Lifetimes<V> {
    ttl: Duration,
    negative_ttl: Duration,
    negative: fn(&V) -> bool,
}

impl<K, V> Expiry<K, V> for Lifetimes<V> {
    fn expire_after_create(&self, _key: &K, value: &V, _created_at: Instant) -> Option<Duration> {
        Some(if (self.negative)(value) {
            self.negative_ttl
        } else {
            self.ttl
        })
    }
}

fn cache<K, V>(options: CacheOptions, negative: fn(&V) -> bool) -> Option<Cache<K, V>>
where
    K: Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    options.keeps_anything().then(|| {
        Cache::builder()
            .max_capacity(options.max_entries)
            .expire_after(Lifetimes {
                ttl: options.ttl,
                negative_ttl: options.negative_ttl,
                negative,
            })
            .build()
    })
}

/// A decision's key: who (id, name, groups in order), what and on what.
type DecisionKey = (String, String, Vec<String>, String, String, Vec<String>);

/// A provider with its lookups and decisions kept for a short while.
///
/// It implements `Directory` and `Authorizer` for a provider that does, answering from memory
/// within the [`CacheOptions`], and `Accounts` by delegating, clearing every kept answer when
/// an account is put or removed. It holds the provider in an [`Arc`], so a host wraps a
/// loaded plugin's handle. `Debug` shows counts only.
///
/// ```
/// use std::sync::Arc;
/// use coqui_access::{Directory, Lookup, Person};
/// use coqui_access::guatiao::library::ProviderError;
/// use coqui_access_kit::{CacheOptions, Cached};
///
/// struct One;
/// impl Directory for One {
///     fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
///         Ok((lookup.value == "alice").then(|| Person { id: "1".into(), ..Person::default() }))
///     }
/// }
///
/// let cached = Cached::new(Arc::new(One), CacheOptions::default());
/// let alice = Lookup { by: "name".into(), value: "alice".into() };
/// assert_eq!(cached.lookup(alice.clone()).unwrap().unwrap().id, "1");
/// cached.invalidate(&alice);
/// ```
pub struct Cached<T: ?Sized> {
    inner: Arc<T>,
    options: CacheOptions,
    lookups: Option<Cache<(String, String), Option<Person>>>,
    decisions: Option<Cache<DecisionKey, AuthorizationDecision>>,
}

impl<T: ?Sized> fmt::Debug for Cached<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = |cache: Option<u64>| cache.unwrap_or(0);
        formatter
            .debug_struct("Cached")
            .field("options", &self.options)
            .field(
                "lookups",
                &count(self.lookups.as_ref().map(Cache::entry_count)),
            )
            .field(
                "decisions",
                &count(self.decisions.as_ref().map(Cache::entry_count)),
            )
            .finish_non_exhaustive()
    }
}

impl<T: ?Sized> Cached<T> {
    /// `inner`, with its answers kept as `options` say. Nothing is asked yet.
    pub fn new(inner: Arc<T>, options: CacheOptions) -> Self {
        Self {
            inner,
            options,
            lookups: cache(options, Option::is_none),
            decisions: cache(options, |decision: &AuthorizationDecision| {
                !decision.allowed
            }),
        }
    }

    /// The provider behind the cache.
    pub fn inner(&self) -> &Arc<T> {
        &self.inner
    }

    /// The options in force.
    pub fn options(&self) -> CacheOptions {
        self.options
    }

    /// Forgets what is kept for `lookup`, so the next one asks the provider.
    pub fn invalidate(&self, lookup: &Lookup) {
        if let Some(lookups) = &self.lookups {
            lookups.invalidate(&(lookup.by.clone(), lookup.value.clone()));
        }
    }

    /// Forgets every lookup and decision kept.
    pub fn invalidate_all(&self) {
        if let Some(lookups) = &self.lookups {
            lookups.invalidate_all();
        }
        if let Some(decisions) = &self.decisions {
            decisions.invalidate_all();
        }
    }
}

impl<T: Directory + ?Sized> Directory for Cached<T> {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        let Some(lookups) = &self.lookups else {
            return self.inner.lookup(lookup);
        };
        let key = (lookup.by.clone(), lookup.value.clone());
        if let Some(kept) = lookups.get(&key) {
            return Ok(kept);
        }
        let found = self.inner.lookup(lookup)?;
        lookups.insert(key, found.clone());
        Ok(found)
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Directory::describe(&*self.inner)
    }
}

impl<T: Authorizer + ?Sized> Authorizer for Cached<T> {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        let Some(decisions) = &self.decisions else {
            return self.inner.authorize(request);
        };
        let mut groups = request.person.groups.clone();
        groups.sort();
        let key = (
            request.person.id.clone(),
            request.person.name.clone(),
            groups,
            request.action.clone(),
            request.resource.clone(),
            // The same resource under another parent may be decided otherwise.
            request.ancestors.clone(),
        );
        if let Some(kept) = decisions.get(&key) {
            return Ok(kept);
        }
        let decided = self.inner.authorize(request)?;
        decisions.insert(key, decided.clone());
        Ok(decided)
    }
}

impl<T: Accounts + ?Sized> Accounts for Cached<T> {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError> {
        self.inner.accounts()
    }

    fn put(&self, account: AccountPut) -> Result<PutOutcome, ProviderError> {
        let outcome = self.inner.put(account);
        self.invalidate_all();
        outcome
    }

    fn remove(&self, name: String) -> Result<(), ProviderError> {
        let removed = self.inner.remove(name);
        self.invalidate_all();
        removed
    }

    fn waiting(&self) -> Result<Vec<Waiting>, ProviderError> {
        self.inner.waiting()
    }
}

#[cfg(test)]
mod tests;
