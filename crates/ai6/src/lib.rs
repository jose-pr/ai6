// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The AI6 daemon and public contracts.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod bootstrap;
pub mod bounds;
pub mod settings;
#[cfg(feature = "tls-roots")]
pub mod tls;

pub use bounds::{Limit, Rate, Seconds, TimeLimit};
pub mod directorates;
pub mod station;

pub use coqui_acp_provider::{
    AgentEvent, AgentMessage, AgentSession, DENIED_EVENT, LIMIT_EVENT, QuotaReport, TokenUsage,
    UsageLimit,
};
pub use directorates::m::access::{
    ACTIONS, AccessPolicy, AccessPolicyError, AccessSet, AccountInfo, AccountPut, Accounts,
    Attempt, Authenticator, AuthorizationDecision, AuthorizationRequest, Authorizer, CHIEF_ROLE,
    CONFIG_ACCESS, CONFIG_ACCOUNTS, Description, Directory, Editor, EffectiveAccess, Explanation,
    KIND_PREFIX, LimitsDef, Lineage, Lookup, NOT_DELEGABLE, OPERATION_ACTIONS, OperationAccess,
    Orphan, Person, PutOutcome, RESERVED_ROLES, Refusal, Role, RoleAuthorizer,
    RoleAuthorizerBuilder, RoleDef, RolePolicy, RolePolicyError, Rule, RuleDef, SCOPE_KINDS,
    STATION_ACTIONS, Subject, TierKind, Verdict, Waiting, access_from_toml, check_change,
    check_operation_change, compile, compile_with_operations, default_delegable, load_access,
    load_policy, role_authorizer, seeded_policy, validate_operation_access,
};
pub use directorates::m::approvals::{
    ActionScope, BUILTIN_KINDS, DEFAULT_APPROVAL_TTL, DEFAULT_PROTECTED, DEFAULT_REQUEST_TTL,
    GREEN_INK_REASON, GreenInkDefinition, GreenInkPolicy, GreenInkPreset, NO_APPROVAL_REASON,
    Requirement, SAFE_HOUSE_DELETE, classify, clearance_at_least,
};
pub use directorates::m::assistant::{
    AssistantDeskLimits, AssistantGatewayLimits, AssistantLimits, AssistantNarrowing,
    AssistantPageLimits, AssistantSettings, AssistantViewLimits, NetworkPolicy, PageNetwork,
    RepoKind, SharedRepo, SkillEntry,
};
pub use directorates::m::compartments::{
    Compartment, CompartmentAction, CompartmentCover, CompartmentLimits, CompartmentResource,
    CompartmentSettings, Compartments, CompiledCompartments, CoverTarget, FieldPrincipal,
    HeldCompartment, ReadIn, ReadInSubject,
};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use directorates::m::config::SqlProvider;
pub use directorates::m::config::{
    ConfigDefinitions, ConfigLimits, ConfigProvider, ConfigRecord, ConfigRevision, ConfigScope,
    ConfigSnapshot, ConfigStatus, FileProvider, MemoryProvider, ProviderConfig, Settings,
    StationRuntime, UnexRecord,
};
pub use directorates::m::cover::{Cover, CoverError, Covers};
pub use directorates::m::harness::{
    HarnessPolicy, PROJECT_DIRECTORIES, PROJECT_FILES, ProjectDecision, ProjectManifest,
    ProjectSettings,
};
pub use directorates::m::identity::{AccessTokenRecord, InvalidIdentity, MissionId, SessionId};
pub use directorates::m::issues::{SourcePage, source_pages};
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
pub use directorates::m::records::{
    RecordForm, RecordFormError, RecordKey, from_json as read_definition,
};
pub use directorates::m::review::{
    AppliedPolicy, Ballot, BriefAuthority, BriefNaming, DEFAULT_REVIEWER_TIMEOUT_SECS,
    Decision as ReviewDecision, IgnoredNaming, NO_REVIEW, OnNoAgreement, PanelCandidate,
    PanelRefusal, PanelSeat, PolicyLookup, PolicySource, ReviewChoice, ReviewGate, ReviewPolicy,
    ReviewRefusal, Reviewer, Step as ReviewStep, Vote, VoteAnswer, applicable as applicable_review,
    choose_panel, decide as decide_review, models_of, next_step as review_next_step,
    out_of_budget as review_out_of_budget, read_vote,
};
pub use directorates::m::roster::{
    Agent, AgentQuery, AgentRole, AgentRoute, CoverIdentity, DEFAULT_CHECKPOINT_SECS,
    DEFAULT_MAX_OPERATION_DEPTH, DEFAULT_RETIRE_WARNING_DAYS, OnLost, OnRetire, Operation,
    Retirement, Roster, RosterBuilder, RosterError, RosterErrorKind, RosterSettings, RouteBalance,
    RouteRef, Workspace,
};
pub use directorates::m::scorecard::{
    AgentCard, AttemptRef, DEFAULT_SMALL_SAMPLE, DEFAULT_WINDOW_DAYS, Judgment, ModelCard,
    RouteCard, Scorecard, ScorecardDefinition, ScorecardPolicy, Standing,
    from_registry as scorecard_from, judge,
};
pub use directorates::m::stewardship::{
    HOLD_KIND, PREFER_KIND, Proposal, Step as ProposedStep, StewardshipPolicy, hold_target,
    propose as propose_from_scorecard, roles_left_empty, route_name,
};
pub use directorates::m::theaters::{
    AgentMode, CableReach, DEFAULT_KEEP_FAILED, DEFAULT_KEEP_FAILED_HOURS, Egress, EgressKind,
    EphemeralAccess, LOCAL_THEATER, RunnerSettings, SshAccess, Theater, TheaterAccess,
    TheaterError, TheaterKind, TheaterSummary, Theaters, TransportAccess, is_sha256_pin,
};
pub use directorates::m::unex_limits::{
    ControlLimits, LoginTable, TextLimits, UnexGatewayLimits, UnexLimits, UnexTimelineLimits,
};
pub use directorates::m::vetting::{
    CHIEF_OF_STAFF_ROLE, DEFAULT_MAX_BRIEF_BYTES, DEFAULT_MAX_PER_HOUR,
    DEFAULT_MAX_PER_SOURCE_PER_HOUR, Finding, FindingKind, KnownWork, PROPOSAL_SOURCE,
    REVIEW_DATA_KEY, REVIEW_SOURCE, ReviewTarget, Vetting, VettingContext, VettingDefinition,
    VettingPolicy, VettingScope, VettingVerdict, content_digest, is_review, is_untrusted,
    read_review, review_brief, review_target, vet, work_title,
};
pub use directorates::m::watches::{Watch, WatchDefinition, WatchScope};
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
    PoolChoice, PoolStrategy, RouteStanding, choose_pools, choose_route,
};
pub use directorates::q::{
    Loadout, LoadoutCatalog, McpServer, QError, QErrorKind, SandboxError, SandboxProfile,
    SandboxRequest, SecretBroker, SecretDefinition, SecretError, SecretFields, SecretInstance,
    SecretKeeper, SecretLease, SecretLeaseRequest, SecretProvider, SecretSource, SecretValue,
    SessionEnv, claude_mcp_config,
};
pub use directorates::registry::accounts::{
    AccountCandidate, AccountClaim, AccountClaimRequest, AccountProfile, AccountRef, AccountState,
    LoginState, QuotaWindow, RenewalLease, choose_account,
};
pub use directorates::registry::agent_records::{
    AgentOutcome, AgentOutcomeReport, AgentRecord, AgentTally, ModelTenure, NO_OPERATION,
    OperationRecord,
};
pub use directorates::registry::approvals::{
    Approval, ApprovalLink, ApprovalRequest, ApprovalStatus, ClassAuthorization, Decision,
    EXPIRY_SIGNER, Rearming, Revocation, Ruling,
};
pub use directorates::registry::assistant_log::{
    AssistantConversation, AssistantLogEntry, AssistantTurn,
};
pub use directorates::registry::backend::{
    AccessTokenStore, AccountStore, AgentRecordStore, AgentRoleStore, ApprovalStore,
    AssistantLogStore, AuditStore, BriefStore, CableStore, ClearanceStore, Limited, LogbookStore,
    QuarantineStore, RegistryBackend, SealedStore, SessionContextStore, SessionStore, SleeperStore,
    SourceStore, StationStore, TimelineStore, UsageStore, VettingStore,
};
pub use directorates::registry::bona_fides::{
    KeepVerified, LibraryPin, Origin, PinCheck, PinSet, PluginProfile, Provenance, PublisherKey,
    Require, SignatureCheck, library_pin, record_digest,
};
pub use directorates::registry::cables::{
    Cable, CableAction, CableAuditRecord, CableConfig, CableDelivery, CableDraft, CableEndpoint,
    CableError, CableGrant, CableHeldDelivery, CableHeldRecipient, CableInboundMode, CablePolicy,
    CablePrincipal, CableRecipient, CableRelay, CableRelayError, CableRelease, CableScope,
    CableSendRequest, CableState, CableTarget, MAX_CABLE_FANOUT,
};
pub use directorates::registry::clearance::ClearanceGrant;
pub use directorates::registry::contexts::{
    ContextOwner, ContextSource, EVENTS_FORMAT as CONTEXT_EVENTS_FORMAT, SessionContext,
};
pub use directorates::registry::holds::{AUTOMATIC as AUTOMATIC_HOLD, HoldStore, RosterHold};
pub use directorates::registry::limits::{
    AccountLimits, AgentLimits, AgentRoleLimits, ApprovalLimits, AssistantLogLimits, AuditLimits,
    BriefLimits, CableLimits, ClearanceLimits, ContextLimits, FileLimits, HeldLimits, HoldLimits,
    LogbookLimits, MemoryLimits, ProposalLimits, QuarantineLimits, RegistryLimits, SessionLimits,
    SleeperLimits, SourceLimits, StationLimits, TimelineLimits, TokenLimits, UsageLimits,
};
pub use directorates::registry::logbook::{
    HEAD_OF_STATION_ROLE, Severity, StationFinding, StationFindingKind, StationReport,
    is_head_of_station_role,
};
pub use directorates::registry::memories::{
    MemoryEntry, MemoryError, MemoryHeader, MemoryProposal, MemoryScope, MemorySource, MemoryStore,
};
pub use directorates::registry::mission_ids::{
    MissionEntry, MissionHandle, is_uid, new_uid, run_handle, valid_handle,
};
pub use directorates::registry::quarantine::Quarantine;
pub use directorates::registry::sessions::{
    Evidence, FinalReport, MissionVerdict, NewSession, OutcomeVerdict, ProbeResult, SessionError,
    SessionLaunch, SessionRecord, SessionResult, SessionStatus, SessionUpdate,
};
pub use directorates::registry::sleepers::{
    Cron, CronError, Fire, SLEEPER_BRIEF_SOURCE, Sleeper, SleeperAuthorization, SleeperMark,
};
pub use directorates::registry::sources::{
    FinishedSourceBrief, Ingest, SOURCE_BRIEF_PREFIX, Source, SourceBatch, SourceConfig,
    SourceItem, SourcePoll, settled_cursor, source_brief,
};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use directorates::registry::sql::{SqlOptions, SqlRegistry};
pub use directorates::registry::store::{
    AuditRecord, Brief, DEFAULT_LEASE, FileRegistry, FiledBrief, HeadquartersLease, Lease,
    LeaseRecovery, MissionProposal, Reentry, StationRecord, StationSettings,
};
pub use directorates::registry::timeline::TimelineEvent;
pub use directorates::registry::tracker::{
    IssueSpec, PostedComment, Reply, ReplyBatch, ReplyPoll, SourceRead, SourceReadAnswer,
    SourceReadComment, SourceReadItem, Tracker, TrackerCapabilities, TrackerIssue, TrackerLink,
    TrackerPost, TrackerUpdate, settings_from_table,
};
pub use directorates::registry::vetting::{Clearance, HeldBrief};
pub use directorates::section_00::agent::{
    AgentHandler, AgentResume, AgentStart, AssistantMode, AssistantNetwork, McpServerRef,
    ModelInfo, ModelListing, OverlaySkill, READ_CLEARANCE, SessionOverlays, SessionRecordFile,
    SessionRecordQuery, clearance_reads_only, clearance_runs_commands,
};
pub use directorates::section_00::assistant::{
    ASSISTANT_CABLE_MCP_ENV_PREFIX, ASSISTANT_CABLE_MCP_SERVER, ASSISTANT_CABLE_MCP_TOKEN_ENV,
    ASSISTANT_CABLE_MCP_URL_ENV, ASSISTANT_MCP_ENV_PREFIX, ASSISTANT_MCP_PATH,
    ASSISTANT_MCP_SERVER, ASSISTANT_MCP_TOKEN_ENV, ASSISTANT_MCP_URL_ENV,
    ASSISTANT_PUBLISH_PAGE_TOOL, ASSISTANT_SHARED_CHANGE_TOOL, AssistantCable,
    AssistantCableDecision, AssistantCableGrant, AssistantChange, AssistantCommit, AssistantEvent,
    AssistantImage, AssistantJob, AssistantPageFile, AssistantReach, AssistantReport, AssistantRun,
    AssistantSharedBase, AssistantWorkspace, EVENT_ACCEPTED, EVENT_CABLE_PROPOSAL,
    EVENT_CABLE_SENT, EVENT_CLOSED, EVENT_COMPLETED, EVENT_FAILED, EVENT_INTERRUPTED, EVENT_LIMIT,
    EVENT_NOTICE, EVENT_REJECTED, EVENT_TEXT, EVENT_TOOL, SCRIPT_ENVIRONMENT, credential_dir_var,
    host_pattern, interpreter_name, page_call_digest,
};
pub use directorates::section_00::content::{MessageContent, MessageImage};
pub use guatiao;
#[cfg(feature = "daemon")]
pub use station::server::{
    DEFAULT_CONFIG_POLL, DEFAULT_LISTEN, DEFAULT_MAX_CONCURRENT_MISSIONS, DEFAULT_RESOURCES_EVERY,
    ServeConfig, StationConfig, StationRole, serve, serve_with_capacity, serve_with_config_dir,
    serve_with_config_dir_and_capacity, serve_with_options, serve_with_plugins,
    serve_with_plugins_and_capacity,
};
