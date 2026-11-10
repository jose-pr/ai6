#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

pub mod accounts;
mod agent;
mod launch;
mod limits;
pub mod mode;
pub mod parse;
mod provider;
mod session;

use alloc::string::String;
use core::fmt;
use core::time::Duration;
use std::io;

pub use agent::{CodexAgent, DEFAULT};
pub use coqui_acp_harness::{self as harness, Launch, PlainSpawn, Spawn, Spawned};
pub use launch::{
    ClientInfo, CodexLaunch, ThreadStart, toml_value, valid_config_key, valid_thread_id,
};
pub use parse::{Parser, ParserLimits};
pub use provider::{CodexProvider, ProviderSettings};
pub use session::{
    AnswerCheck, CodexSession, CodexSettings, Model, models_of, user_input, version_of,
};

/// The Codex versions this crate was tested with. Another one runs, with a warning.
pub const TESTED_VERSIONS: &[&str] = &["0.164.0", "0.166.0"];

/// What went wrong with Codex.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// It could not be started.
    Spawn(io::Error),
    /// The request asks for what it cannot do.
    Refused(String),
    /// It did not answer in time.
    Timeout {
        /// What was waited for.
        what: &'static str,
        /// How long.
        limit: Duration,
    },
    /// It refused, failed or is gone, in its own words when it gave any.
    Harness(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "Codex could not be started: {error}"),
            Self::Refused(why) => f.write_str(why),
            Self::Timeout { what, limit } => {
                write!(f, "Codex did not answer {what} within {limit:?}")
            }
            Self::Harness(why) => f.write_str(why),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Spawn(error) => Some(error),
            _ => None,
        }
    }
}
