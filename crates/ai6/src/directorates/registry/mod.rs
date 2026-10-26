//! The Registry: the shared record every Station reads and writes. Its store keeps the
//! brief queue, leases, Station check-ins, the Headquarters lease, audit records and
//! access tokens, Source cursors and Sleeper approvals, green ink requests and decisions, and
//! the activity timeline; its relay carries Cables between Agents. [`backend`] names what a
//! store must do, one trait per record family.

pub mod approvals;
pub mod backend;
pub mod cables;
pub mod quarantine;
pub mod sessions;
pub mod sleepers;
pub mod sources;
pub mod store;
pub mod timeline;
pub mod usage;
