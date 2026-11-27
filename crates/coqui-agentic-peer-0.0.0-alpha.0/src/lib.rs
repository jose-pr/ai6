//! What an agent needs around a proven link: where this user's identity and trust store live, finding a peer on the network,
//! the browser's native-messaging companion, and offering a peer's MCP tools to a program here. Pair once, then ACP, A2A, MCP or
//! anything else runs as a capability on the same link, with no second pairing. It adds finding and reaching a peer, never a
//! dialect: what crosses a stream is that protocol's own messages.
//!
//! The proof, the pairing, the streams, the peers' grants and the service loop are `cabuya`'s, and not repeated here.
#![forbid(unsafe_code)]
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

pub mod companion;
pub mod discovery;
pub mod home;
pub mod mcp_offer;

use core::fmt;

/// What went wrong finding a peer or installing the companion.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A peer could not be advertised or searched for, or the companion could not be installed.
    Discovery(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovery(why) => write!(f, "discovery failed: {why}"),
        }
    }
}

impl std::error::Error for Error {}
