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
//! [`FileRegistry`]: super::store::FileRegistry

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
