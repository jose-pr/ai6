// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The AI6 daemon and public contracts.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod directorates;
pub mod station;

pub use directorates::m::access::{
    ACTIONS, AccessPolicyError, AccountInfo, AccountPut, Accounts, Attempt, Authenticator,
    AuthorizationDecision, AuthorizationRequest, Authorizer, Description, Directory, KIND_PREFIX,
    Lookup, Person, PutOutcome, Role, RoleAuthorizer, RoleAuthorizerBuilder, RolePolicy,
    RolePolicyError, Rule, SCOPE_KINDS, Verdict, Waiting, load_role_authorizer, role_authorizer,
    role_authorizer_from_toml,
};
pub use directorates::m::approvals::{
    ActionScope, BUILTIN_KINDS, DEFAULT_APPROVAL_TTL, DEFAULT_PROTECTED, DEFAULT_REQUEST_TTL,
    GREEN_INK_REASON, GreenInkDefinition, GreenInkPolicy, GreenInkPreset, Requirement, classify,
    clearance_at_least,
};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use directorates::m::config::SqlConfigSource;
pub use directorates::m::config::{
    ConfigDefinitions, ConfigRecord, ConfigRevision, ConfigSnapshot, ConfigSource, ConfigStatus,
    MemoryConfigSource, TomlConfigDir,
};
pub use directorates::m::cover::{Cover, CoverError, Covers};
pub use directorates::m::identity::{AccessTokenRecord, InvalidIdentity, MissionId, SessionId};
pub use directorates::m::placement::{
    AgentFacts, DEFAULT_MAX_CPU_PERCENT, DEFAULT_MIN_FREE_MEMORY_MB, ExecutionMode,
    OperationPolicy, Placement, PlacementChoice, PlacementRejection, PlacementRequest,
    PlacementStrategy, ResourceLimits, Resources, StationFacts, execution_mode, fitness,
    harness_verdict, place,
};
pub use directorates::m::roster::{
    Agent, AgentRole, CoverIdentity, DEFAULT_CHECKPOINT_SECS, OnLost, Operation, Roster,
    RosterBuilder, RosterError, RosterErrorKind, Workspace,
};
pub use directorates::m::theaters::{
    AgentMode, DEFAULT_KEEP_FAILED, DEFAULT_KEEP_FAILED_HOURS, Egress, EgressKind, EphemeralAccess,
    LOCAL_THEATER, RunnerSettings, SshAccess, Theater, TheaterAccess, TheaterError, TheaterKind,
    TheaterSummary, Theaters, TransportAccess, is_sha256_pin,
};
pub use directorates::medical::exposure::{
    BRIEF_EXPOSURE_KEY, BRIEF_REGISTRY_DROP, BRIEF_SOURCE, DEFAULT_EXPOSING_TOOLS, Exposure,
    ExposureDefinition, ExposureGate, ExposurePolicy, ExposurePreset, REVIEW, Restriction,
    cable_source, tool_source,
};
pub use directorates::medical::quarantine::{
    DEFAULT_BUDGET_STOPS, DEFAULT_STALLS, DEFAULT_WINDOW_SECS, Incident, IncidentKind,
    QuarantineDefinition, QuarantinePolicy,
};
pub use directorates::medical::usage::{
    Budget, BudgetBreach, BudgetDefinition, BudgetLevel, BudgetPreset, BudgetScope,
    DEFAULT_WARN_AT_PERMILLE, MissionUsage, UsageTotals, UsageTracker, micro_usd, utc_day,
};
pub use directorates::medical::watchdog::{
    DEFAULT_BLOCKED_AFTER, DEFAULT_LINE_HISTORY, DEFAULT_LOOP_MIN_LINES, DEFAULT_STALL_TIMEOUT,
    Watchdog, WatchdogConfig, WatchdogOverrides,
};
pub use directorates::q::ephemeral::{
    MadeTheater, MadeTheaters, TheaterMaker, TheaterProvider, TheaterProviderConfig, TheaterRequest,
};
pub use directorates::q::provision::{
    DEFAULT_INSTALL_TIMEOUT_SECS, ProbeReport, ProvisionSpec, ProvisionStep, RequiredProgram,
    Requirements, StepStatus, Unmet, probe_resources,
};
pub use directorates::q::safehouse::{
    Change, ChangeLog, CheckpointSpec, SafeHouseCheckpoint, SafeHouseInfo, SafeHouseProvider,
    SafeHouseReport, SafeHouseSession, SafeHouseSpec,
};
pub use directorates::q::terminal::{
    AgentTerminal, ExitInfo, READ_TIMED_OUT, Spec, Terminal, TerminalBridge, TerminalSession,
};
pub use directorates::q::transport::{
    Capabilities, DEFAULT_MAX_OUTPUT, DEFAULT_READ_LIMIT, DirEntry, DirListing, FileData, Forward,
    ForwardInfo, ForwardSpec, Output, ReadSpec, RunSpec, Transport, TransportClient,
    TransportConfig, TransportForward, WriteSpec, describe, is_unsupported, unsupported,
};
pub use directorates::q::{
    Account, AccountKind, AccountLogin, AccountPool, CREDENTIAL_VARIABLES, CommandLogin,
    PoolChoice, PoolStrategy, choose_pools,
};
pub use directorates::q::{
    DEFAULT_SECRET_LEASE_TTL, Loadout, LoadoutCatalog, MAX_LOADOUT_BYTES, MAX_SECRET_LEASE_TTL,
    McpServer, QError, QErrorKind, SandboxError, SandboxProfile, SandboxRequest, SecretBackend,
    SecretBackendFailure, SecretBroker, SecretDefinition, SecretError, SecretLease,
    SecretLeaseRequest, SecretRequest, SecretSource, SecretValue, StationSecrets,
    claude_mcp_config,
};
pub use directorates::registry::accounts::{
    AccountCandidate, AccountClaim, AccountClaimRequest, AccountProfile, AccountRef, AccountState,
    QuotaWindow, UNKNOWN_USE_PERMILLE, choose_account,
};
pub use directorates::registry::approvals::{
    Approval, ApprovalRequest, ApprovalStatus, ClassAuthorization, Decision, EXPIRY_SIGNER,
    Revocation, Ruling,
};
pub use directorates::registry::backend::{
    AccessTokenStore, AccountStore, AgentRoleStore, ApprovalStore, AuditStore, BriefStore,
    CableStore, QuarantineStore, RegistryBackend, SessionStore, SleeperStore, SourceStore,
    StationStore, TimelineStore, UsageStore,
};
pub use directorates::registry::bona_fides::{
    LibraryPin, Origin, PinCheck, PinSet, PluginProfile, Provenance, PublisherKey, Require,
    SignatureCheck, brief_digest, library_pin,
};
pub use directorates::registry::cables::{
    Cable, CableAction, CableAuditRecord, CableConfig, CableDelivery, CableDraft, CableEndpoint,
    CableError, CableGrant, CableHeldDelivery, CableHeldRecipient, CableInboundMode, CablePolicy,
    CablePrincipal, CableRecipient, CableRelay, CableRelayError, CableRelease, CableScope,
    CableSendRequest, CableState, CableTarget, DEFAULT_CABLE_TTL, MAX_CABLE_BODY_BYTES,
    MAX_CABLE_FANOUT, MAX_CABLE_TTL,
};
pub use directorates::registry::quarantine::Quarantine;
pub use directorates::registry::sessions::{
    MAX_RECENT_EVENT_CHARS, MAX_RECENT_EVENTS, NewSession, SessionError, SessionLaunch,
    SessionRecord, SessionStatus, SessionUpdate,
};
pub use directorates::registry::sleepers::{
    Cron, CronError, Fire, SLEEPER_BRIEF_SOURCE, Sleeper, SleeperAuthorization,
};
pub use directorates::registry::sources::{
    CxReport, FinishedSourceBrief, Ingest, SOURCE_BRIEF_PREFIX, Source, SourceBatch, SourceConfig,
    SourceItem, SourcePoll, source_brief,
};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use directorates::registry::sql::{SqlOptions, SqlRegistry};
pub use directorates::registry::store::{
    AuditRecord, Brief, DEFAULT_LEASE, FileRegistry, GONE_DARK_AFTER, HeadquartersLease, Lease,
    LeaseRecovery, MAX_STATION_MISSION_CAPACITY, MissionProposal, StationRecord, StationSettings,
};
pub use directorates::registry::timeline::{MAX_TIMELINE_EVENTS, TimelineEvent};
pub use directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentMessage, AgentResume, AgentSession, AgentStart, LIMIT_EVENT,
    QuotaReport, TokenUsage, UsageLimit,
};
pub use guatiao;
#[cfg(feature = "daemon")]
pub use station::server::{
    DEFAULT_CONFIG_POLL, DEFAULT_LISTEN, DEFAULT_MAX_CONCURRENT_MISSIONS, DEFAULT_RESOURCES_EVERY,
    DEFAULT_TERMINAL, ServeConfig, StationConfig, StationRole, serve, serve_with_capacity,
    serve_with_config_dir, serve_with_config_dir_and_capacity, serve_with_options,
    serve_with_plugins, serve_with_plugins_and_capacity,
};
