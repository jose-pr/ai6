#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

mod account;
mod account_provider;
mod address;
pub mod batch;
#[cfg(feature = "contract")]
#[cfg_attr(docsrs, doc(cfg(feature = "contract")))]
pub mod check;
#[cfg(feature = "contract")]
#[cfg_attr(docsrs, doc(cfg(feature = "contract")))]
pub mod contract;
mod environment;
pub mod files;
#[cfg(feature = "reader")]
#[cfg_attr(docsrs, doc(cfg(feature = "reader")))]
pub mod key_usage;
mod kind;
mod models;
#[cfg(feature = "reader")]
#[cfg_attr(docsrs, doc(cfg(feature = "reader")))]
pub mod reader;
mod requirement;
mod roots;
mod spawner;
pub mod usage;

pub use guatiao;
pub use guatiao_channel::{Channel, ChannelSink, ChannelSinkVtable, ChannelVtable};

pub use account::{
    ACCOUNT_ID_MAX, ASK, AUTH, Accepted, AcceptedSlot, AccountCatalog, AccountGrant, AccountKeeper,
    AccountKeeperVtable, AccountKindInfo, AccountLoss, AccountRef, AccountRenewal, Credentials,
    DEFAULT_LOGIN_TIMEOUT_MS, EndpointSlot, HARNESS_LOGIN, LOCAL_ACCOUNT, LoginAnswer, LoginForm,
    LoginMethodInfo, LoginOutcome, LoginPrompter, LoginPrompterVtable, LoginRequest, MODEL_API,
    OPEN_URL, OpenUrl, PASTE_CODE, PROVIDES_BY_VALUE, PROXY, RenewReport, RenewRequest, USER_CODE,
    is_account_id,
};
pub use account_provider::{
    AccountModelsQuery, AccountOffer, AccountProvider, AccountProviderInfo, AccountProviderVtable,
    AccountUsage, AccountUsageQuery, AccountWatch, AccountWatchVtable, SignedInCheck,
    SignedInQuery, SignedInState, WatchRequest,
};
pub use address::is_http_url;
pub use environment::Environment;
pub mod standard;
pub use files::PrivateDir;
pub use kind::{AcpHarness, AcpHarnessVtable};
pub use models::{
    AccountsLink, CacheLifetime, CapabilitiesSource, DEFAULT_PAGE_BYTES, ExportPage, HarnessInfo,
    HistoryLoss, ImportReport, LINE_TOO_LARGE, ModelInfo, ModelLimits, ModelListing, NonDefault,
    REPLAYED_AS_PROMPT, SeedPoint, SessionExport, SessionImport, SessionQuery, SessionRecordFile,
    StartRequest, StartRequestBuilder, Takes, UnknownKind, line_too_large,
};
pub use requirement::{
    ANSWER_ONLY, DENIED_PATHS, FILES_UNDER, NAMES, NETWORK, NO_COMMANDS, NO_PROJECT_MCP,
    NO_PROJECT_SETTINGS, Narrowing, ONLY_SKILLS, READ_ONLY, READ_ONLY_PATHS, Requirement,
    RequirementError, SANDBOXED_SCRIPTS,
};
pub use roots::{PathRefusal, PathStyle, WorkingRoots, is_plain_absolute, is_session_id};
pub use spawner::{
    ErrorOutput, FileInfo, HarnessNotice, LinkState, NoticeKind, ProcessExit, ProcessSpawner,
    ProcessSpawnerVtable, READ_LINK_DOWN, READ_TIMED_OUT, SecretBytes, SpawnSpec, SpawnedProcess,
    SpawnedProcessVtable, SpawnerInfo, TerminalSize,
};
pub use usage::{
    Bucket, Coverage, DEFAULT_LIMIT_PHRASES, LIMIT_META_KEY, LimitHit, LimitKind, ModelSpend,
    Refill, Source, Spend, Status, USAGE_CAPABILITY, USAGE_METHOD, USAGE_NOTIFICATION,
    USAGE_VERSION, Unit, UsageReport, UsageSupport, infer_limit, limit_kind_of,
};
