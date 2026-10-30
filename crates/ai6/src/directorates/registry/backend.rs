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
//! [`open`] picks the store from a location: a directory for the
//! [`FileRegistry`], `sqlite:PATH` or a `postgres://` URL for the SQL Registry.
//!
//! [`FileRegistry`]: super::store::FileRegistry

use std::io;
use std::sync::Arc;

use super::limits::RegistryLimits;
use super::store::FileRegistry;

pub use super::accounts::AccountStore;
pub use super::agent_records::AgentRecordStore;
pub use super::approvals::ApprovalStore;
pub use super::clearance::ClearanceStore;
pub use super::logbook::LogbookStore;
pub use super::mission_issues::MissionIssueStore;
pub use super::quarantine::QuarantineStore;
pub use super::sessions::SessionStore;
pub use super::sleepers::SleeperStore;
pub use super::sources::SourceStore;
pub use super::store::{
    AccessTokenStore, AgentRoleStore, AuditStore, BriefStore, CableStore, StationStore,
};
pub use super::timeline::TimelineStore;
pub use super::usage::UsageStore;
pub use super::vetting::VettingStore;

/// The bounds a Registry applies to what it keeps and accepts.
pub trait Limited {
    /// The Registry's [`RegistryLimits`], as it was opened with.
    fn limits(&self) -> &RegistryLimits;
}

impl Limited for FileRegistry {
    fn limits(&self) -> &RegistryLimits {
        &self.limits
    }
}

/// Every Registry record family. Implemented for each type that implements them all.
pub trait RegistryBackend:
    Limited
    + AccountStore
    + AgentRecordStore
    + BriefStore
    + StationStore
    + AccessTokenStore
    + CableStore
    + AuditStore
    + AgentRoleStore
    + ApprovalStore
    + ClearanceStore
    + LogbookStore
    + MissionIssueStore
    + QuarantineStore
    + SessionStore
    + SleeperStore
    + SourceStore
    + TimelineStore
    + UsageStore
    + VettingStore
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

/// As [`open`], with `options`: the limits either store applies
/// ([`SqlOptions::with_limits`](super::sql::SqlOptions::with_limits)), and a SQL Registry's
/// password and timeouts.
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
        Ok(Arc::new(
            FileRegistry::open(location).with_limits(options.limits().clone()),
        ))
    }
}

fn is_sql(location: &str) -> bool {
    location.starts_with("sqlite:")
        || location.starts_with("postgres://")
        || location.starts_with("postgresql://")
}

impl<T> RegistryBackend for T where
    T: Limited
        + AccountStore
        + AgentRecordStore
        + BriefStore
        + StationStore
        + AccessTokenStore
        + CableStore
        + AuditStore
        + AgentRoleStore
        + ApprovalStore
        + ClearanceStore
        + LogbookStore
        + MissionIssueStore
        + QuarantineStore
        + SessionStore
        + SleeperStore
        + SourceStore
        + TimelineStore
        + UsageStore
        + VettingStore
        + Send
        + Sync
{
}
