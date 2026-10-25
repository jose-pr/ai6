//! The directorates: who decides. M sets policy and authorizes; the Registry keeps the
//! shared record (briefs, leases, check-ins, audit) and relays Cables; Q Branch issues
//! equipment; the 00 Section defines how Agents are run; Medical watches their health.
//! The node that carries out their decisions is [`crate::station`].

pub mod m;
pub mod medical;
pub mod q;
pub mod registry;
pub mod section_00;
