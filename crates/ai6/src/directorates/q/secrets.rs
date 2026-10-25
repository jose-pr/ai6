//! Mission-scoped leases for secret references declared by an Agent loadout.

use super::Loadout;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;
use zeroize::Zeroize;

/// Default lifetime for a secret lease.
pub const DEFAULT_SECRET_LEASE_TTL: Duration = Duration::from_secs(5 * 60);
/// Longest secret lease accepted by the broker.
pub const MAX_SECRET_LEASE_TTL: Duration = Duration::from_secs(60 * 60);

/// A short-lived request to a configured secret backend.
#[derive(Clone)]
pub struct SecretRequest {
    lease_id: String,
    station: String,
    mission_id: String,
    ttl: Duration,
}

impl SecretRequest {
    /// Returns the opaque identifier used to acquire and revoke this lease.
    pub fn lease_id(&self) -> &str {
        &self.lease_id
    }

    /// Returns the Station requesting this lease.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// Returns the Mission that owns this lease.
    pub fn mission_id(&self) -> &str {
        &self.mission_id
    }

    /// Returns the requested lease lifetime.
    pub const fn ttl(&self) -> Duration {
        self.ttl
    }
}

impl fmt::Debug for SecretRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretRequest")
            .field("lease_id", &self.lease_id)
            .field("station", &self.station)
            .field("mission_id", &self.mission_id)
            .field("ttl", &self.ttl)
            .finish()
    }
}

/// A validated Station and Mission scope for one bounded lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretLeaseRequest {
    station: String,
    mission_id: String,
    ttl: Duration,
}

impl SecretLeaseRequest {
    /// Creates a request using the default lease lifetime.
    pub fn try_new(
        station: impl Into<String>,
        mission_id: impl Into<String>,
    ) -> Result<Self, SecretError> {
        Self::with_ttl(station, mission_id, DEFAULT_SECRET_LEASE_TTL)
    }

    /// Creates a request with an explicit lifetime.
    pub fn with_ttl(
        station: impl Into<String>,
        mission_id: impl Into<String>,
        ttl: Duration,
    ) -> Result<Self, SecretError> {
        let station = station.into();
        let mission_id = mission_id.into();
        validate_identity(&station)?;
        validate_identity(&mission_id)?;
        validate_ttl(ttl, MAX_SECRET_LEASE_TTL)?;
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
    /// Wraps a secret value returned by a trusted backend.
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

/// A secret retrieval backend failure without provider details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretBackendFailure;

impl SecretBackendFailure {
    /// Creates an opaque failure for a backend implementation.
    pub const fn new() -> Self {
        Self
    }
}

impl Default for SecretBackendFailure {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SecretBackendFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("secret backend request failed")
    }
}

impl std::error::Error for SecretBackendFailure {}

/// Supplies named values for a scoped lease and revokes that lease by ID.
pub trait SecretBackend: Send + Sync {
    /// Returns exactly the requested names or a backend failure.
    fn acquire(
        &self,
        request: &SecretRequest,
        names: &[String],
    ) -> Result<BTreeMap<String, SecretValue>, SecretBackendFailure>;

    /// Revokes a lease; implementations must enforce a bounded backend timeout.
    fn revoke(&self, lease_id: &str) -> Result<(), SecretBackendFailure>;
}

/// A rejected, expired or failed secret lease operation.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretError {
    /// Station or Mission identity is empty, too long or contains controls.
    InvalidIdentity,
    /// Requested lifetime is zero or exceeds the broker's configured maximum.
    InvalidTtl,
    /// The backend failed; its diagnostic is withheld.
    Backend,
    /// The backend returned values that do not match the requested names.
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

/// Resolves only an Agent's declared references into bounded in-memory leases.
pub struct SecretBroker {
    backend: Arc<dyn SecretBackend>,
    max_ttl: Duration,
}

impl SecretBroker {
    /// Creates a broker with the one-hour maximum lease lifetime.
    pub fn new(backend: impl SecretBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
            max_ttl: MAX_SECRET_LEASE_TTL,
        }
    }

    /// Sets a stricter maximum lease lifetime.
    pub fn with_max_ttl(mut self, max_ttl: Duration) -> Result<Self, SecretError> {
        validate_ttl(max_ttl, MAX_SECRET_LEASE_TTL)?;
        self.max_ttl = max_ttl;
        Ok(self)
    }

    /// Acquires the loadout's named values from the backend.
    pub fn lease(
        &self,
        loadout: &Loadout,
        request: SecretLeaseRequest,
    ) -> Result<SecretLease, SecretError> {
        validate_ttl(request.ttl, self.max_ttl)?;
        let lease_id = Uuid::new_v4().simple().to_string();
        let backend_request = SecretRequest {
            lease_id,
            station: request.station,
            mission_id: request.mission_id,
            ttl: request.ttl,
        };
        let names = loadout.secrets.clone();
        let values = if names.is_empty() {
            BTreeMap::new()
        } else {
            match self.backend.acquire(&backend_request, &names) {
                Ok(values) => values,
                Err(_) => {
                    let _ = self.backend.revoke(backend_request.lease_id());
                    return Err(SecretError::Backend);
                }
            }
        };
        let expected: BTreeSet<_> = names.iter().collect();
        let actual: BTreeSet<_> = values.keys().collect();
        if expected != actual {
            drop(values);
            let _ = self.backend.revoke(backend_request.lease_id());
            return Err(SecretError::InvalidResponse);
        }
        Ok(SecretLease {
            backend: Arc::clone(&self.backend),
            request: backend_request,
            values: Mutex::new(LeaseState {
                values,
                revoked: false,
                expires_at: Instant::now() + request.ttl,
            }),
            backend_lease: !names.is_empty(),
            backend_revoked: Mutex::new(false),
        })
    }
}

impl fmt::Debug for SecretBroker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretBroker")
            .field("max_ttl", &self.max_ttl)
            .finish_non_exhaustive()
    }
}

struct LeaseState {
    values: BTreeMap<String, SecretValue>,
    revoked: bool,
    expires_at: Instant,
}

/// Active Mission-scoped secret references held only in process memory.
pub struct SecretLease {
    backend: Arc<dyn SecretBackend>,
    request: SecretRequest,
    values: Mutex<LeaseState>,
    backend_lease: bool,
    backend_revoked: Mutex<bool>,
}

impl SecretLease {
    /// Returns the Station that owns the lease.
    pub fn station(&self) -> &str {
        self.request.station()
    }

    /// Returns the Mission ID that owns the lease.
    pub fn mission_id(&self) -> &str {
        self.request.mission_id()
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

    /// Clears local values and requests backend revocation; safe to retry after failure.
    pub fn revoke(&self) -> Result<(), SecretError> {
        let mut state = self.lock_state();
        state.revoked = true;
        state.values.clear();
        drop(state);
        self.revoke_backend()
    }

    fn revoke_backend(&self) -> Result<(), SecretError> {
        if !self.backend_lease {
            return Ok(());
        }
        let mut revoked = self
            .backend_revoked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *revoked {
            return Ok(());
        }
        self.backend
            .revoke(self.request.lease_id())
            .map_err(|_| SecretError::Backend)?;
        *revoked = true;
        Ok(())
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

fn validate_ttl(value: Duration, maximum: Duration) -> Result<(), SecretError> {
    if value.is_zero() || value > maximum {
        Err(SecretError::InvalidTtl)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_SECRET_LEASE_TTL, SecretBackend, SecretBackendFailure, SecretBroker, SecretError,
        SecretLeaseRequest, SecretRequest, SecretValue,
    };
    use crate::directorates::q::LoadoutCatalog;
    use std::collections::BTreeMap;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct BackendState {
        revoked: Vec<String>,
        fail_acquire: bool,
        fail_revoke: bool,
        invalid_response: bool,
    }

    #[derive(Clone, Default)]
    struct Backend(Arc<Mutex<BackendState>>);

    impl SecretBackend for Backend {
        fn acquire(
            &self,
            request: &SecretRequest,
            names: &[String],
        ) -> Result<BTreeMap<String, SecretValue>, SecretBackendFailure> {
            if self.0.lock().expect("test backend lock").fail_acquire {
                return Err(SecretBackendFailure::new());
            }
            assert!(!request.lease_id().is_empty());
            assert_eq!(request.station(), "station-a");
            assert_eq!(request.mission_id(), "mission-1");
            assert_eq!(request.ttl(), DEFAULT_SECRET_LEASE_TTL);
            let invalid = self.0.lock().expect("test backend lock").invalid_response;
            let selected = if invalid { "undeclared" } else { &names[0] };
            Ok(BTreeMap::from([(
                selected.to_owned(),
                SecretValue::new("TOP-SECRET"),
            )]))
        }

        fn revoke(&self, lease_id: &str) -> Result<(), SecretBackendFailure> {
            let mut state = self.0.lock().expect("test backend lock");
            if state.fail_revoke {
                return Err(SecretBackendFailure::new());
            }
            state.revoked.push(lease_id.to_owned());
            Ok(())
        }
    }

    fn loadout() -> crate::directorates::q::Loadout {
        LoadoutCatalog::from_str("[loadout.007]\nsecrets = ['token']")
            .expect("valid test loadout")
            .for_agent("007")
            .expect("test Agent")
            .clone()
    }

    fn request() -> SecretLeaseRequest {
        SecretLeaseRequest::try_new("station-a", "mission-1").expect("valid lease request")
    }

    #[test]
    fn secret_values_are_scoped_and_debug_redacts_material() {
        let backend = Backend::default();
        let lease = SecretBroker::new(backend.clone())
            .lease(&loadout(), request())
            .expect("backend grants the declared secret");
        assert!(lease.is_active());
        assert_eq!(lease.names(), ["token"]);
        assert_eq!(
            lease.with_secret("token", str::to_owned).unwrap(),
            "TOP-SECRET"
        );
        assert_eq!(
            lease.with_secret("other", str::to_owned),
            Err(SecretError::AccessDenied)
        );
        assert!(!format!("{lease:?}").contains("TOP-SECRET"));
        assert!(!format!("{:?}", SecretValue::new("TOP-SECRET")).contains("TOP-SECRET"));
        lease.revoke().unwrap();
        lease.revoke().unwrap();
        assert!(lease.names().is_empty());
        assert_eq!(backend.0.lock().unwrap().revoked.len(), 1);
    }

    #[test]
    fn backend_response_must_match_requested_references() {
        let backend = Backend::default();
        backend.0.lock().unwrap().invalid_response = true;
        assert_eq!(
            SecretBroker::new(backend.clone())
                .lease(&loadout(), request())
                .err(),
            Some(SecretError::InvalidResponse)
        );
        assert_eq!(backend.0.lock().unwrap().revoked.len(), 1);
    }

    #[test]
    fn failed_backend_acquisition_attempts_revocation_and_hides_details() {
        let backend = Backend::default();
        backend.0.lock().unwrap().fail_acquire = true;
        let error = SecretBroker::new(backend.clone())
            .lease(&loadout(), request())
            .unwrap_err();
        assert_eq!(error, SecretError::Backend);
        assert!(!error.to_string().contains("TOP-SECRET"));
        assert_eq!(backend.0.lock().unwrap().revoked.len(), 1);
    }

    #[test]
    fn expired_lease_clears_values_and_revokes_backend() {
        let backend = Backend::default();
        let lease = SecretBroker::new(backend.clone())
            .lease(&loadout(), request())
            .unwrap();
        assert!(lease.is_active());
        lease.values.lock().unwrap().expires_at = std::time::Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            lease.with_secret("token", str::to_owned),
            Err(SecretError::Expired)
        );
        assert!(lease.names().is_empty());
        assert_eq!(backend.0.lock().unwrap().revoked.len(), 1);
    }

    #[test]
    fn invalid_lifetimes_and_backend_revocation_fail_closed() {
        assert_eq!(
            SecretLeaseRequest::with_ttl("station-a", "mission-1", Duration::ZERO).err(),
            Some(SecretError::InvalidTtl)
        );
        let backend = Backend::default();
        let lease = SecretBroker::new(backend.clone())
            .lease(&loadout(), request())
            .unwrap();
        backend.0.lock().unwrap().fail_revoke = true;
        assert_eq!(lease.revoke(), Err(SecretError::Backend));
        assert!(lease.names().is_empty());
        backend.0.lock().unwrap().fail_revoke = false;
        lease.revoke().unwrap();
        assert_eq!(backend.0.lock().unwrap().revoked.len(), 1);
    }

    #[test]
    fn empty_loadout_does_not_create_backend_lease() {
        let empty = LoadoutCatalog::from_str("[loadout.007]")
            .unwrap()
            .for_agent("007")
            .unwrap()
            .clone();
        let backend = Backend::default();
        let lease = SecretBroker::new(backend.clone())
            .lease(&empty, request())
            .unwrap();
        lease.revoke().unwrap();
        assert!(backend.0.lock().unwrap().revoked.is_empty());
    }

    #[test]
    fn secret_lease_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SecretBroker>();
        assert_send_sync::<super::SecretLease>();
        assert_send_sync::<SecretValue>();
    }
}
