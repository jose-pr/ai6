#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

#[cfg(feature = "sealed")]
#[cfg_attr(docsrs, doc(cfg(feature = "sealed")))]
pub mod auth;
#[cfg(feature = "forward")]
#[cfg_attr(docsrs, doc(cfg(feature = "forward")))]
pub mod forward;
pub mod link;
pub mod mux;
#[cfg(feature = "sealed")]
#[cfg_attr(docsrs, doc(cfg(feature = "sealed")))]
pub mod pairing;
pub mod peers;

#[cfg(test)]
mod deadline;

use core::fmt;

/// What went wrong on a link.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The network channel failed.
    Channel(coqui_transport::Failure),
    /// A peer failed its identity proof, or its proof could not be completed.
    Proof(String),
    /// A key, identity file or trust store could not be used.
    Identity(String),
    /// The peer's first message shows it proves by another method than this endpoint, which never tries that one.
    #[cfg(feature = "sealed")]
    OtherMethod {
        /// The method the peer's message shows.
        peer: auth::Method,
        /// This endpoint's.
        ours: auth::Method,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Channel(failure) => write!(f, "the channel failed: {failure}"),
            Self::Proof(why) => write!(f, "the peer was not proven: {why}"),
            Self::Identity(why) => write!(f, "the identity could not be used: {why}"),
            #[cfg(feature = "sealed")]
            Self::OtherMethod { peer, ours } => write!(
                f,
                "the peer was not proven: the peer proves by {peer}, and this endpoint only by {ours}: a method is chosen per endpoint, never per connection"
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<coqui_transport::Failure> for Error {
    fn from(failure: coqui_transport::Failure) -> Self {
        Self::Channel(failure)
    }
}
