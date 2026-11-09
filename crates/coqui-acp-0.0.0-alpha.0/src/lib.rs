#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

mod client;
mod decider;
mod describe;
mod elicit;
mod error;
mod event;
pub mod lines;
mod state;

pub use agent_client_protocol as sdk;
pub use client::{Authenticate, Builder, Client, Exchange, RpcFailure, Session};
pub use decider::{Decider, Decision, PermissionOption, PermissionRequest, RefuseAll};
pub use describe::{
    Answer, DescribeLimits, Description, SESSION_OPTIONS_CAPABILITY, SESSION_OPTIONS_METHOD,
    SessionList,
};
pub use elicit::{ChoiceOption, Elicitation, ElicitationAnswer, Elicitor, Field, FieldKind};
pub use error::Error;
pub use event::Event;

/// An in-memory pair of connected transports, for an agent in the same process.
#[must_use]
pub fn duplex() -> (sdk::Channel, sdk::Channel) {
    sdk::Channel::duplex()
}
