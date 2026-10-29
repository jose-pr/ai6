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
    SAFE_HOUSE_DELETE, classify, clearance_at_least,
};
pub use directorates::m::assistant::{
    AssistantNarrowing, AssistantSettings, NetworkPolicy, RepoKind, SharedRepo, SkillEntry,
};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use directorates::m::config::SqlConfigSource;
pub use directorates::m::config::{
    ConfigDefinitions, ConfigRecord, ConfigRevision, ConfigSnapshot, ConfigSource, ConfigStatus,
    MemoryConfigSource, TomlConfigDir,
};
pub use directorates::m::cover::{Cover, CoverError, Covers};
pub use directorates::m::harness::{
    HarnessPolicy, MAX_MANIFEST_BYTES, MAX_MANIFEST_FILE_BYTES, MAX_MANIFEST_FILES,
    PROJECT_DIRECTORIES, PROJECT_FILES, ProjectDecision, ProjectManifest, ProjectSettings,
};
pub use directorates::m::identity::{AccessTokenRecord, InvalidIdentity, MissionId, SessionId};
pub use directorates::m::placement::{
    AgentFacts, DEFAULT_MAX_CPU_PERCENT, DEFAULT_MIN_FREE_MEMORY_MB, ExecutionMode,
    OperationPolicy, Placement, PlacementChoice, PlacementRejection, PlacementRequest,
    PlacementStrategy, ResourceLimits, Resources, StationFacts, execution_mode, fitness,
    harness_verdict, place,
};
pub use directorates::m::promotion::{
    CLEARANCE_KIND, DEFAULT_00_MAX_FAILURE_RATE, DEFAULT_00_MISSIONS, DEFAULT_00_QUIET_DAYS,
    DEFAULT_FIELD_MISSIONS, DEFAULT_FIELD_QUIET_DAYS, Promotion, PromotionDefinition,
    PromotionPolicy, PromotionStep,
};
pub use directorates::m::roster::{
    Agent, AgentQuery, AgentRole, CoverIdentity, DEFAULT_CHECKPOINT_SECS,
    DEFAULT_RETIRE_WARNING_DAYS, OnLost, OnRetire, Operation, Retirement, Roster, RosterBuilder,
    RosterError, RosterErrorKind, Workspace,
};
pub use directorates::m::theaters::{
    AgentMode, DEFAULT_KEEP_FAILED, DEFAULT_KEEP_FAILED_HOURS, Egress, EgressKind, EphemeralAccess,
    LOCAL_THEATER, RunnerSettings, SshAccess, Theater, TheaterAccess, TheaterError, TheaterKind,
    TheaterSummary, Theaters, TransportAccess, is_sha256_pin,
};
pub use directorates::m::vetting::{
    CHIEF_OF_STAFF_ROLE, DEFAULT_MAX_BRIEF_BYTES, DEFAULT_MAX_PER_HOUR,
    DEFAULT_MAX_PER_SOURCE_PER_HOUR, Finding, FindingKind, KnownWork, PROPOSAL_SOURCE,
    REVIEW_DATA_KEY, REVIEW_SOURCE, ReviewTarget, Vetting, VettingContext, VettingDefinition,
    VettingPolicy, VettingScope, VettingVerdict, content_digest, is_review, is_untrusted,
    read_review, review_brief, review_target, vet, work_title,
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
    Change, ChangeLog, CheckpointFile, CheckpointSpec, SafeHouseCheckpoint, SafeHouseClone,
    SafeHouseExports, SafeHouseInfo, SafeHouseProvider, SafeHouseReport, SafeHouseSession,
    SafeHouseSpec,
};
pub use directorates::q::terminal::{
    AgentTerminal, DEFAULT_PIPE, DEFAULT_TERMINAL, ExitInfo, HarnessStdio, READ_TIMED_OUT, Spec,
    Terminal, TerminalBridge, TerminalOffer, TerminalRefusal, TerminalSession, choose_terminal,
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
pub use directorates::registry::agent_records::{
    AgentOutcome, AgentOutcomeReport, AgentRecord, AgentTally, ModelTenure, NO_OPERATION,
    OperationRecord,
};
pub use directorates::registry::approvals::{
    Approval, ApprovalRequest, ApprovalStatus, ClassAuthorization, Decision, EXPIRY_SIGNER,
    Revocation, Ruling,
};
pub use directorates::registry::backend::{
    AccessTokenStore, AccountStore, AgentRecordStore, AgentRoleStore, ApprovalStore, AuditStore,
    BriefStore, CableStore, ClearanceStore, LogbookStore, QuarantineStore, RegistryBackend,
    SessionStore, SleeperStore, SourceStore, StationStore, TimelineStore, UsageStore, VettingStore,
};
pub use directorates::registry::bona_fides::{
    LibraryPin, Origin, PinCheck, PinSet, PluginProfile, Provenance, PublisherKey, Require,
    SignatureCheck, library_pin, record_digest,
};
pub use directorates::registry::cables::{
    Cable, CableAction, CableAuditRecord, CableConfig, CableDelivery, CableDraft, CableEndpoint,
    CableError, CableGrant, CableHeldDelivery, CableHeldRecipient, CableInboundMode, CablePolicy,
    CablePrincipal, CableRecipient, CableRelay, CableRelayError, CableRelease, CableScope,
    CableSendRequest, CableState, CableTarget, DEFAULT_CABLE_TTL, MAX_CABLE_BODY_BYTES,
    MAX_CABLE_FANOUT, MAX_CABLE_TTL,
};
pub use directorates::registry::clearance::ClearanceGrant;
pub use directorates::registry::logbook::{
    HEAD_OF_STATION_ROLE, LOGBOOK_LENGTH, Severity, StationFinding, StationFindingKind,
    StationReport, is_head_of_station_role,
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
pub use directorates::registry::vetting::{
    Clearance, HeldBrief, MAX_HELD_BRIEFS, MAX_HELD_PER_OPERATION, MAX_HELD_RESULTS,
};
pub use directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentMessage, AgentResume, AgentSession, AgentStart, AssistantMode,
    AssistantNetwork, LIMIT_EVENT, McpServerRef, QuotaReport, TokenUsage, UsageLimit,
};
pub use directorates::section_00::assistant::{
    ASSISTANT_CABLE_MCP_ENV_PREFIX, ASSISTANT_CABLE_MCP_SERVER, ASSISTANT_CABLE_MCP_TOKEN_ENV,
    ASSISTANT_CABLE_MCP_URL_ENV, ASSISTANT_MCP_ENV_PREFIX, ASSISTANT_MCP_PATH,
    ASSISTANT_MCP_SERVER, ASSISTANT_MCP_TOKEN_ENV, ASSISTANT_MCP_URL_ENV,
    ASSISTANT_SHARED_CHANGE_TOOL, AssistantCable, AssistantCableDecision, AssistantCableGrant,
    AssistantChange, AssistantCommit, AssistantEvent, AssistantJob, AssistantReach,
    AssistantReport, AssistantSharedBase, AssistantWorkspace, COMMIT_MEMORY, COMMIT_SHARED,
    EVENT_ACCEPTED, EVENT_CABLE_PROPOSAL, EVENT_CABLE_SENT, EVENT_CLOSED, EVENT_COMPLETED,
    EVENT_FAILED, EVENT_INTERRUPTED, EVENT_LIMIT, EVENT_MEMORY_CHANGES, EVENT_NOTICE,
    EVENT_REJECTED, EVENT_TEXT, EVENT_TOOL, MAX_ASSISTANT_CABLE_GRANTS, MAX_ASSISTANT_CHANGE_BYTES,
    MAX_ASSISTANT_CHANGE_FILES, MAX_ASSISTANT_CONTEXT_BYTES, MAX_ASSISTANT_EVENT_BYTES,
    MAX_ASSISTANT_EVENTS, MAX_ASSISTANT_MESSAGE_BYTES, MAX_ASSISTANT_SHARED_BASES,
    MAX_MEMORY_FILES, MAX_MEMORY_LIST_BYTES, MemoryAction, MemoryAnswer, MemoryCommit, MemoryFile,
    MemoryOp, SCRIPT_ENVIRONMENT, credential_dir_var, host_pattern, interpreter_name, memory_path,
    outside_tool,
};
pub use guatiao;
#[cfg(feature = "daemon")]
pub use station::server::{
    AssistantDeskLimits, DEFAULT_CONFIG_POLL, DEFAULT_LISTEN, DEFAULT_MAX_CONCURRENT_MISSIONS,
    DEFAULT_RESOURCES_EVERY, ServeConfig, StationConfig, StationRole, serve, serve_with_capacity,
    serve_with_config_dir, serve_with_config_dir_and_capacity, serve_with_options,
    serve_with_plugins, serve_with_plugins_and_capacity,
};
