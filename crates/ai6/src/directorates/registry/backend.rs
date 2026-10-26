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
    BriefStore
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
    let sql = location.starts_with("sqlite:")
        || location.starts_with("postgres://")
        || location.starts_with("postgresql://");
    if !sql {
        return Ok(Arc::new(FileRegistry::open(location)));
    }
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    return Ok(Arc::new(super::sql::SqlRegistry::open(location)?));
    #[cfg(not(any(feature = "sqlite", feature = "postgres")))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this build of ai6 has no SQL Registry support (cargo features `sqlite`, `postgres`)",
    ))
}

/// Opens the Registry that `AI6_REGISTRY` names (a directory or SQL location, as for
/// [`open`]), else the directory `AI6_HOME/registry` or the platform state directory.
///
/// # Errors
///
/// As [`open`], and `NotFound` when no state directory can be determined.
pub fn from_environment() -> io::Result<Arc<dyn RegistryBackend>> {
    let location = default_registry_dir()?;
    open(&location.to_string_lossy())
}

impl<T> RegistryBackend for T where
    T: BriefStore
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
