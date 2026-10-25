//! The Registry: the shared record every Station reads and writes. Its store keeps the
//! brief queue, leases, Station check-ins, the Headquarters lease, audit records and
//! access tokens, Source cursors and Sleeper approvals; its relay carries Cables between Agents.

pub mod cables;
pub mod sleepers;
pub mod sources;
pub mod store;
