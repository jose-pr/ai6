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
    GREEN_INK_REASON, GreenInkDefinition, GreenInkPolicy, GreenInkPreset, Requirement,
    clearance_at_least,
};
pub use directorates::m::identity::{AccessTokenRecord, InvalidIdentity, MissionId, SessionId};
pub use directorates::m::placement::{
    AgentFacts, ExecutionMode, OperationPolicy, Placement, PlacementChoice, PlacementRejection,
    PlacementRequest, StationFacts, execution_mode, fitness, harness_verdict, place,
};
pub use directorates::m::roster::{
    AgentRole, CoverIdentity, DEFAULT_CHECKPOINT_SECS, OnLost, Operation, Roster, RosterError,
    Workspace,
};
pub use directorates::m::theaters::{
    AgentMode, Egress, EgressKind, LOCAL_THEATER, RunnerSettings, SshAccess, Theater,
    TheaterAccess, TheaterError, TheaterKind, TheaterSummary, Theaters, TransportAccess,
    is_sha256_pin,
};
pub use directorates::medical::exposure::{
    BRIEF_EXPOSURE_KEY, BRIEF_SOURCE, DEFAULT_EXPOSING_TOOLS, Exposure, ExposureDefinition,
    ExposureGate, ExposurePolicy, ExposurePreset, REVIEW, Restriction, cable_source, tool_source,
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
pub use directorates::q::provision::{
    DEFAULT_INSTALL_TIMEOUT_SECS, ProbeReport, ProvisionSpec, ProvisionStep, RequiredProgram,
    Requirements, StepStatus, Unmet,
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
    DEFAULT_SECRET_LEASE_TTL, Loadout, LoadoutCatalog, MAX_LOADOUT_BYTES, MAX_SECRET_LEASE_TTL,
    McpServer, QError, QErrorKind, SandboxError, SandboxProfile, SandboxRequest, SecretBackend,
    SecretBackendFailure, SecretBroker, SecretDefinition, SecretError, SecretLease,
    SecretLeaseRequest, SecretRequest, SecretSource, SecretValue, StationSecrets,
    claude_mcp_config,
};
pub use directorates::registry::approvals::{
    Approval, ApprovalRequest, ApprovalStatus, ClassAuthorization, Decision, EXPIRY_SIGNER,
    Revocation, Ruling,
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
pub use directorates::registry::store::{
    AuditRecord, Brief, DEFAULT_LEASE, FileRegistry, GONE_DARK_AFTER, HeadquartersLease, Lease,
    LeaseRecovery, MAX_STATION_MISSION_CAPACITY, MissionProposal, StationRecord, StationSettings,
};
pub use directorates::registry::timeline::{MAX_TIMELINE_EVENTS, TimelineEvent};
pub use directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentMessage, AgentResume, AgentSession, AgentStart, LIMIT_EVENT,
    TokenUsage, UsageLimit,
};
pub use guatiao;
#[cfg(feature = "daemon")]
pub use station::server::{
    DEFAULT_LISTEN, DEFAULT_MAX_CONCURRENT_MISSIONS, DEFAULT_TERMINAL, ServeConfig, StationConfig,
    StationRole, serve, serve_with_capacity, serve_with_config_dir,
    serve_with_config_dir_and_capacity, serve_with_options, serve_with_plugins,
    serve_with_plugins_and_capacity,
};
