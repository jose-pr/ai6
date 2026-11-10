#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

#[cfg(feature = "contract")]
#[cfg_attr(docsrs, doc(cfg(feature = "contract")))]
pub mod contract;
mod environment;
mod kind;
mod models;
mod requirement;
mod roots;
mod spawner;
pub mod usage;

pub use guatiao;
pub use guatiao_channel::{Channel, ChannelSink, ChannelSinkVtable, ChannelVtable};

pub use environment::Environment;
pub use kind::{AcpHarness, AcpHarnessVtable};
pub use models::{
    DEFAULT_PAGE_BYTES, ExportPage, HarnessInfo, HistoryLoss, ImportReport, LINE_TOO_LARGE,
    ModelInfo, ModelListing, NonDefault, SeedPoint, SessionExport, SessionImport, SessionQuery,
    SessionRecordFile, StartRequest, StartRequestBuilder, Takes, line_too_large,
};
pub use requirement::{
    ANSWER_ONLY, DENIED_PATHS, FILES_UNDER, NAMES, NETWORK, NO_COMMANDS, NO_PROJECT_MCP,
    NO_PROJECT_SETTINGS, Narrowing, ONLY_SKILLS, READ_ONLY, READ_ONLY_PATHS, Requirement,
    RequirementError, SANDBOXED_SCRIPTS,
};
pub use roots::{PathRefusal, PathStyle, WorkingRoots, is_plain_absolute, is_session_id};
pub use spawner::{
    ErrorOutput, LinkState, ProcessExit, ProcessSpawner, ProcessSpawnerVtable, READ_LINK_DOWN,
    READ_TIMED_OUT, SpawnSpec, SpawnedProcess, SpawnedProcessVtable, SpawnerInfo, TerminalSize,
};
pub use usage::{
    Bucket, Coverage, DEFAULT_LIMIT_PHRASES, LIMIT_META_KEY, LimitHit, ModelSpend, Refill, Source,
    Spend, Status, USAGE_CAPABILITY, USAGE_METHOD, USAGE_NOTIFICATION, USAGE_VERSION, Unit,
    UsageQuery, UsageReport, UsageSupport, infer_limit,
};
