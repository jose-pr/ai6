#![doc = include_str!("../README.md")]
#![deny(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

pub mod access;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod account;
pub mod acp;
#[cfg(feature = "antigravity")]
#[cfg_attr(docsrs, doc(cfg(feature = "antigravity")))]
pub mod antigravity;
pub mod cli;
pub mod event;
#[cfg(feature = "keyring")]
#[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
pub mod keyring;
pub mod lines;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod login;
pub mod mode;
pub mod process;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod provider;
pub mod settings;
pub mod signed_in;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod spawner;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod stdio;
pub mod stream;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod vendors;
pub mod version;
pub mod warm;

/// What the macros of this crate name.
#[cfg(feature = "kind")]
#[doc(hidden)]
pub mod __private {
    pub extern crate alloc;
    pub use coqui_acp_api as api;
}

pub use access::{ACCESS_META, Access, Approvals, Part, Sandbox, Target, Translation};
pub use acp::{
    AgentInfo, ConfigOption, Conversation, EventSink, Harness, HarnessError, McpServer,
    McpTransport, Open, OpenRequest, OptionValue, Settings, Takes, UsageTracker, serve,
};
pub use event::{Event, Outcome, PermissionRequest, Responder};
pub use mode::Mode;
pub use process::{
    ErrorStream, Exit, Launch, LineWriter, PlainSpawn, Process, Spawn, Spawned, Starter,
    ending_once, exit_text, line_writer, set_program_dirs, set_starter, watch_exit,
};
pub use settings::{ConnectionSettings, SessionLimits};
#[cfg(feature = "kind")]
pub use spawner::{LocalSpawner, ThroughSpawner};
pub use stream::{Answer, LineHook, Step, Stream, Waiting};
pub use warm::{Maker, Relay, WarmKey, WarmPolicy, WarmPool, Warmed, drop_all};
