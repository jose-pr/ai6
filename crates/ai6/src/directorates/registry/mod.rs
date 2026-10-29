//! The Registry: the shared record every Station reads and writes. Its store keeps the
//! brief queue, held briefs and each brief's provenance ([`bona_fides`]), leases, Station
//! check-ins, the Headquarters lease, audit records and access tokens, Source cursors and
//! Sleeper approvals, green ink requests and decisions, pool accounts, Station logbooks and the activity
//! timeline; its relay carries Cables between Agents. [`backend`] names what a
//! store must do, one trait per record family, and opens a store by location: the
//! [`FileRegistry`](store::FileRegistry) directory, or with the `sqlite` or `postgres`
//! feature a SQL database (`sql::SqlRegistry`).

pub mod accounts;
pub mod agent_records;
pub mod approvals;
pub mod backend;
pub mod bona_fides;
pub mod cables;
pub mod clearance;
pub mod limits;
pub mod logbook;
pub mod quarantine;
pub mod sessions;
pub mod sleepers;
pub mod sources;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub mod sql;
pub mod store;
#[cfg(test)]
pub(crate) mod suite;
pub mod timeline;
pub mod usage;
pub mod vetting;
