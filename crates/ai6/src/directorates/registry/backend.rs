//! Where the Registry keeps its records: one trait per record family, and
//! [`RegistryBackend`] for all of them together.
//!
//! The Station and Universal Exports hold an `Arc<dyn RegistryBackend>`, so any type that
//! implements every family serves as the Registry. [`FileRegistry`] keeps them in a shared
//! directory. Bring the traits into scope with `use ai6::directorates::registry::backend::*`
//! to call them through the trait object.
//!
//! The traits are synchronous: callers on an async runtime run them on a blocking thread.
//!
//! [`open`] and [`from_environment`] pick the store from a location: a directory for the
//! [`FileRegistry`], `sqlite:PATH` or a `postgres://` URL for the SQL Registry.
//!
//! [`FileRegistry`]: super::store::FileRegistry

use std::io;
use std::sync::Arc;

use super::store::{FileRegistry, default_registry_dir};

pub use super::accounts::AccountStore;
pub use super::approvals::ApprovalStore;
pub use super::quarantine::QuarantineStore;
pub use super::sessions::SessionStore;
pub use super::sleepers::SleeperStore;
pub use super::sources::SourceStore;
pub use super::store::{
    AccessTokenStore, AgentRoleStore, AuditStore, BriefStore, CableStore, StationStore,
};
pub use super::timeline::TimelineStore;
pub use super::usage::UsageStore;

/// Every Registry record family. Implemented for each type that implements them all.
pub trait RegistryBackend:
    AccountStore
    + BriefStore
    + StationStore
    + AccessTokenStore
    + CableStore
    + AuditStore
    + AgentRoleStore
    + ApprovalStore
    + QuarantineStore
    + SessionStore
    + SleeperStore
    + SourceStore
    + TimelineStore
    + UsageStore
    + Send
    + Sync
{
}

/// Opens the Registry at `location`: `sqlite:PATH` (or `sqlite://PATH`) for a SQLite file,
/// a `postgres://` or `postgresql://` URL for PostgreSQL, anything else a Registry
/// directory.
///
/// # Errors
///
/// `Unsupported` for a SQL location when this build lacks its cargo feature (`sqlite`,
/// `postgres`); the SQL Registry's errors when its database cannot be opened. A directory
/// is not touched until the Registry is used.
pub fn open(location: &str) -> io::Result<Arc<dyn RegistryBackend>> {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    return open_with(location, &super::sql::SqlOptions::default());
    #[cfg(not(any(feature = "sqlite", feature = "postgres")))]
    if is_sql(location) {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this build of ai6 has no SQL Registry support (cargo features `sqlite`, `postgres`)",
        ))
    } else {
        Ok(Arc::new(FileRegistry::open(location)))
    }
}

/// As [`open`], connecting a SQL Registry with `options` (password, timeouts).
///
/// # Errors
///
/// As [`open`].
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub fn open_with(
    location: &str,
    options: &super::sql::SqlOptions,
) -> io::Result<Arc<dyn RegistryBackend>> {
    if is_sql(location) {
        Ok(Arc::new(super::sql::SqlRegistry::open_with(
            location, options,
        )?))
    } else {
        Ok(Arc::new(FileRegistry::open(location)))
    }
}

fn is_sql(location: &str) -> bool {
    location.starts_with("sqlite:")
        || location.starts_with("postgres://")
        || location.starts_with("postgresql://")
}

/// Opens the Registry that `AI6_REGISTRY` names (a directory or SQL location, as for
/// [`open`]), else the directory `AI6_HOME/registry` or the platform state directory. A
/// PostgreSQL location without a password takes `AI6_REGISTRY_PASSWORD`.
///
/// # Errors
///
/// As [`open`], and `NotFound` when no state directory can be determined.
pub fn from_environment() -> io::Result<Arc<dyn RegistryBackend>> {
    let location = default_registry_dir()?;
    let location = location.to_string_lossy();
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let mut options = super::sql::SqlOptions::default();
        if let Some(password) = std::env::var("AI6_REGISTRY_PASSWORD")
            .ok()
            .filter(|password| !password.is_empty())
        {
            options = options.with_password(password);
        }
        open_with(&location, &options)
    }
    #[cfg(not(any(feature = "sqlite", feature = "postgres")))]
    open(&location)
}

impl<T> RegistryBackend for T where
    T: AccountStore
        + BriefStore
        + StationStore
        + AccessTokenStore
        + CableStore
        + AuditStore
        + AgentRoleStore
        + ApprovalStore
        + QuarantineStore
        + SessionStore
        + SleeperStore
        + SourceStore
        + TimelineStore
        + UsageStore
        + Send
        + Sync
{
}
