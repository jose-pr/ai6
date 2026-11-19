#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

mod account_provider;
pub mod account_usage;
pub mod accounts;
mod agent;
pub mod cli;
#[cfg(feature = "history")]
#[cfg_attr(docsrs, doc(cfg(feature = "history")))]
pub mod history;
mod launch;
mod limits;
pub mod parse;
mod provider;
#[cfg(feature = "history")]
mod provider_history;
mod replay;
mod session;
mod signin;
mod tools;

use alloc::string::String;
use core::fmt;
use core::time::Duration;
use std::io;

pub use account_provider::{AccountSettings, ClaudeAccounts};
pub use agent::{ClaudeAgent, DEFAULT, EFFORTS, FreshSessions};
pub use coqui_acp_harness::{self as harness, Launch, PlainSpawn, Spawn, Spawned};
pub use launch::{ClaudeLaunch, SessionStart, mcp_config, valid_session_id};
pub use parse::{ModeHook, ModePermit, Parser, ParserLimits};
pub use provider::{ClaudeProvider, ProviderSettings, SettingSource};
pub use replay::{config_dir, project_directory, replay, transcript};
pub use session::{ClaudeSession, ClaudeSettings, Model, models_of, user_blocks, version_of};

/// The Claude Code versions this crate was tested with. Another one runs, with a warning.
pub const TESTED_VERSIONS: &[&str] = &["2.1.318"];

/// What went wrong with Claude Code.
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
            Self::Spawn(error) => write!(f, "Claude Code could not be started: {error}"),
            Self::Refused(why) => f.write_str(why),
            Self::Timeout { what, limit } => {
                write!(f, "Claude Code did not answer {what} within {limit:?}")
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
