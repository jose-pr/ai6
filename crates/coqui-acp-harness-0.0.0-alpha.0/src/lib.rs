#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

pub mod acp;
#[cfg(feature = "antigravity")]
#[cfg_attr(docsrs, doc(cfg(feature = "antigravity")))]
pub mod antigravity;
pub mod event;
pub mod lines;
pub mod mode;
pub mod process;
#[cfg(feature = "kind")]
#[cfg_attr(docsrs, doc(cfg(feature = "kind")))]
pub mod spawner;
pub mod stream;
pub mod version;

pub use acp::{
    AgentInfo, ConfigOption, Conversation, EventSink, Harness, HarnessError, McpServer,
    McpTransport, Open, OpenRequest, OptionValue, Settings, Takes, UsageTracker, serve,
};
pub use event::{Event, Outcome, PermissionRequest, Responder};
pub use mode::Mode;
pub use process::{
    Exit, Launch, LineWriter, PlainSpawn, Process, Spawn, Spawned, ending_once, exit_text,
    line_writer, watch_exit,
};
#[cfg(feature = "kind")]
pub use spawner::{LocalSpawner, ThroughSpawner};
pub use stream::{Answer, LineHook, Step, Stream, Waiting};
