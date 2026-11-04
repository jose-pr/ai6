//! Mission-scoped leases for secret references declared by an Agent loadout, each read or leased
//! from the [`SecretProvider`](super::secret_kind::SecretProvider) its definition names.

use super::secret_kind::SecretKeeper;
use super::{Loadout, LoadoutCatalog};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zeroize::Zeroize;

use crate::bounds::TimeLimit;

/// A validated Station and Mission scope for one bounded lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretLeaseRequest {
    station: String,
    mission_id: String,
    ttl: Duration,
}

impl SecretLeaseRequest {
    /// Creates a request for a lease of `ttl`: more than zero, and a time this process can
    /// reach.
    pub fn with_ttl(
        station: impl Into<String>,
        mission_id: impl Into<String>,
        ttl: Duration,
    ) -> Result<Self, SecretError> {
        let station = station.into();
        let mission_id = mission_id.into();
        validate_identity(&station)?;
        validate_identity(&mission_id)?;
        validate_ttl(ttl, TimeLimit::UNBOUNDED)?;
        Ok(Self {
            station,
            mission_id,
            ttl,
        })
    }

    /// Returns the requesting Station.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// Returns the owning Mission ID.
    pub fn mission_id(&self) -> &str {
        &self.mission_id
    }

    /// Returns the requested lease lifetime.
    pub const fn ttl(&self) -> Duration {
        self.ttl
    }
}

/// Secret material that redacts formatting and clears its owned bytes on drop.
pub struct SecretValue(String);

impl SecretValue {
    /// Wraps a secret value returned by a trusted provider.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Exposes the value only for the duration of the supplied callback.
    pub fn with_exposed<T>(&self, use_value: impl FnOnce(&str) -> T) -> T {
        use_value(&self.0)
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue([REDACTED])")
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// A rejected, expired or failed secret lease operation.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretError {
    /// Station or Mission identity is empty, too long or contains controls.
    InvalidIdentity,
    /// Requested lifetime is zero or exceeds the broker's configured maximum.
    InvalidTtl,
    /// The provider failed; its diagnostic is withheld.
    Backend,
    /// The provider answered something that is not a secret's value.
    InvalidResponse,
    /// The reference is not declared by the Agent loadout.
    AccessDenied,
    /// The lease has expired or has been revoked.
    Expired,
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidIdentity => "invalid Station or Mission identity for secret lease",
            Self::InvalidTtl => "secret lease lifetime is outside the allowed range",
            Self::Backend => "secret backend request failed",
            Self::InvalidResponse => "secret backend returned an invalid lease response",
            Self::AccessDenied => "secret is not declared by this Agent loadout",
            Self::Expired => "secret lease is revoked or expired",
        })
    }
}

impl std::error::Error for SecretError {}

/// Finds the provider a secret names: an instance by its name, or a loaded provider with its
/// default settings. Its error is the host's own text, which never holds a value.
pub type SecretProviderLookup = dyn Fn(&str) -> Result<Arc<SecretKeeper>, String> + Send + Sync;

/// Resolves only an Agent's declared references into bounded in-memory leases, each from the
/// provider its definition names.
pub struct SecretBroker {
    catalog: Arc<LoadoutCatalog>,
    providers: Arc<SecretProviderLookup>,
    max_ttl: TimeLimit,
}

impl SecretBroker {
    /// A broker reading each secret as `catalog` defines it ([`LoadoutCatalog::secret`]), from
    /// the provider `providers` finds, leasing for as long as a request asks.
    pub fn new(
        catalog: Arc<LoadoutCatalog>,
        providers: impl Fn(&str) -> Result<Arc<SecretKeeper>, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            catalog,
            providers: Arc::new(providers),
            max_ttl: TimeLimit::UNBOUNDED,
        }
    }

    /// Refuses a lease longer than `max_ttl`.
    #[must_use]
    pub fn with_max_ttl(mut self, max_ttl: TimeLimit) -> Self {
        self.max_ttl = max_ttl;
        self
    }

    /// Acquires the loadout's named values from their providers.
    pub fn lease(
        &self,
        loadout: &Loadout,
        request: SecretLeaseRequest,
    ) -> Result<SecretLease, SecretError> {
        self.lease_names(loadout, &loadout.secrets, request)
    }

    /// Acquires some of the loadout's named values; a name the loadout does not declare is
    /// [`SecretError::AccessDenied`] and nothing is acquired. A provider that leases is asked
    /// for a lease of the request's time, and any other is read; whatever was leased is revoked
    /// when one fails.
    pub fn lease_names(
        &self,
        loadout: &Loadout,
        names: &[String],
        request: SecretLeaseRequest,
    ) -> Result<SecretLease, SecretError> {
        if names.iter().any(|name| !loadout.secrets.contains(name)) {
            return Err(SecretError::AccessDenied);
        }
        validate_ttl(request.ttl, self.max_ttl)?;
        let names: BTreeSet<&String> = names.iter().collect();
        let mut values = BTreeMap::new();
        let mut leased = Vec::new();
        for name in names {
            match self.acquire(name, &request) {
                Ok((value, lease)) => {
                    values.insert(name.clone(), value);
                    leased.extend(lease);
                }
                Err(error) => {
                    drop(values);
                    revoke_all(&mut leased);
                    return Err(error);
                }
            }
        }
        Ok(SecretLease {
            station: request.station,
            mission_id: request.mission_id,
            values: Mutex::new(LeaseState {
                values,
                revoked: false,
                expires_at: Instant::now() + request.ttl,
            }),
            leased: Mutex::new(leased),
        })
    }

    /// One secret's value, and the provider lease it came with, if any.
    fn acquire(
        &self,
        name: &str,
        request: &SecretLeaseRequest,
    ) -> Result<(SecretValue, Option<ProviderLease>), SecretError> {
        let definition = self.catalog.secret(name);
        let source = definition.source();
        let keeper = (self.providers)(&source.provider).map_err(|_| SecretError::Backend)?;
        keeper
            .check_fields(&source.fields)
            .map_err(|_| SecretError::Backend)?;
        let leased = keeper
            .lease(
                &source.key,
                &source.fields,
                request.ttl.as_secs_f64(),
                &request.station,
                &request.mission_id,
            )
            .map_err(|_| SecretError::Backend)?;
        let (value, lease) = match leased {
            Some(leased) => (
                leased.value,
                Some(ProviderLease {
                    keeper: keeper.clone(),
                    id: leased.lease,
                }),
            ),
            None => (
                keeper
                    .read(&source.key, &source.fields)
                    .map_err(|_| SecretError::Backend)?
                    .value,
                None,
            ),
        };
        if value.expose().is_empty() {
            let mut lease = lease.into_iter().collect();
            revoke_all(&mut lease);
            return Err(SecretError::InvalidResponse);
        }
        Ok((SecretValue::new(value.expose().as_str()), lease))
    }
}

impl fmt::Debug for SecretBroker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretBroker")
            .field("max_ttl", &self.max_ttl)
            .finish_non_exhaustive()
    }
}

/// A lease a provider issued, revoked at that provider.
struct ProviderLease {
    keeper: Arc<SecretKeeper>,
    id: String,
}

/// Revokes each of `leases` at its provider, keeping those that failed for another try.
fn revoke_all(leases: &mut Vec<ProviderLease>) -> bool {
    leases.retain(|lease| lease.keeper.revoke(&lease.id).is_err());
    leases.is_empty()
}

struct LeaseState {
    values: BTreeMap<String, SecretValue>,
    revoked: bool,
    expires_at: Instant,
}

/// Active Mission-scoped secret references held only in process memory.
pub struct SecretLease {
    station: String,
    mission_id: String,
    values: Mutex<LeaseState>,
    /// What the providers leased and have not yet revoked.
    leased: Mutex<Vec<ProviderLease>>,
}

impl SecretLease {
    /// Returns the Station that owns the lease.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// Returns the Mission ID that owns the lease.
    pub fn mission_id(&self) -> &str {
        &self.mission_id
    }

    /// Returns the monotonic expiry deadline for this process.
    pub fn expires_at(&self) -> Instant {
        self.lock_state().expires_at
    }

    /// Returns whether values can currently be accessed.
    pub fn is_active(&self) -> bool {
        let state = self.lock_state();
        !state.revoked && Instant::now() < state.expires_at
    }

    /// Returns declared reference names, never their values.
    pub fn names(&self) -> Vec<String> {
        self.lock_state().values.keys().cloned().collect()
    }

    /// Uses one declared value while holding the lease lock against concurrent revocation.
    ///
    /// The callback should be short and must not re-enter this lease.
    pub fn with_secret<T>(
        &self,
        name: &str,
        use_value: impl FnOnce(&str) -> T,
    ) -> Result<T, SecretError> {
        let mut state = self.lock_state();
        if state.revoked {
            return Err(SecretError::Expired);
        }
        if Instant::now() >= state.expires_at {
            state.revoked = true;
            state.values.clear();
            drop(state);
            self.revoke_backend()?;
            return Err(SecretError::Expired);
        }
        let value = state.values.get(name).ok_or(SecretError::AccessDenied)?;
        Ok(value.with_exposed(use_value))
    }

    /// Clears local values and revokes each provider lease; safe to retry after failure.
    pub fn revoke(&self) -> Result<(), SecretError> {
        let mut state = self.lock_state();
        state.revoked = true;
        state.values.clear();
        drop(state);
        self.revoke_backend()
    }

    fn revoke_backend(&self) -> Result<(), SecretError> {
        let mut leased = self
            .leased
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if revoke_all(&mut leased) {
            Ok(())
        } else {
            Err(SecretError::Backend)
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, LeaseState> {
        self.values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl fmt::Debug for SecretLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretLease")
            .field("station", &self.station())
            .field("mission_id", &self.mission_id())
            .field("names", &self.names())
            .field("expires_at", &self.expires_at())
            .field("active", &self.is_active())
            .finish()
    }
}

fn validate_identity(value: &str) -> Result<(), SecretError> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        Err(SecretError::InvalidIdentity)
    } else {
        Ok(())
    }
}

fn validate_ttl(value: Duration, maximum: TimeLimit) -> Result<(), SecretError> {
    if value.is_zero() || !maximum.allows(value) || Instant::now().checked_add(value).is_none() {
        Err(SecretError::InvalidTtl)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
