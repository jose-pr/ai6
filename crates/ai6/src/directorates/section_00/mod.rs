//! Provider-neutral agent harness and Session contracts.

#[cfg(feature = "acp")]
pub mod acp;
pub mod agent;
pub mod assistant;
pub mod denied;
pub mod gate;
pub mod programs;
