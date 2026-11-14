#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod account;
pub mod acp;
#[cfg(feature = "antigravity")]
#[cfg_attr(docsrs, doc(cfg(feature = "antigravity")))]
pub mod antigravity;
pub mod cli;
pub mod event;
pub mod lines;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod login;
pub mod mode;
pub mod process;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod provider;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod spawner;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod stdio;
pub mod stream;
pub mod version;

pub use acp::{
    AgentInfo, ConfigOption, Conversation, EventSink, Harness, HarnessError, McpServer,
    McpTransport, Open, OpenRequest, OptionValue, Settings, Takes, UsageTracker, serve,
};
pub use event::{Event, Outcome, PermissionRequest, Responder};
pub use mode::Mode;
pub use process::{
    ErrorStream, Exit, Launch, LineWriter, PlainSpawn, Process, Spawn, Spawned, Starter,
    ending_once, exit_text, line_writer, set_starter, watch_exit,
};
#[cfg(feature = "kind")]
pub use spawner::{LocalSpawner, ThroughSpawner};
pub use stream::{Answer, LineHook, Step, Stream, Waiting};
