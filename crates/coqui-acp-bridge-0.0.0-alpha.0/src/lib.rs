//! Puts a stdio Agent Client Protocol agent on the network: a peer proves itself, then complete messages pass between it
//! and the agent's standard input and output, unread. What the agent is allowed to do stays with the agent, and whatever the
//! client allows stays with the client. Proof, pairing and the link are `cabuya`'s; discovery and the home are `coqui_agentic_peer`'s.
//!
//! The commands a program offers are here as modules, each with its arguments as a `clap::Args` type and a `run` function:
//! [`serve`], [`dial`], [`forward`], [`pairing`], [`grant`], [`discover`], [`keygen`] and the standing door's [`standing`]. A
//! program that embeds them names itself in a [`Helper`](helper::Helper).
#![forbid(unsafe_code)]
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

pub mod agent;
mod common;
pub mod dial;
pub mod discover;
pub mod forward;
pub mod grant;
pub mod helper;
pub mod keygen;
pub mod linger;
pub mod mcp_shim;
pub mod pairing;
pub mod relay;
pub mod serve;
pub mod standing;

pub use common::{MethodArgs, SessionArgs, valid_origin};
