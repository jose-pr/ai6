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
    Attempt, Authenticator, AuthorizationDecision, AuthorizationRequest, Authorizer, Directory,
    Lookup, Person, RoleAuthorizer, RoleAuthorizerConfig, RoleAuthorizerProvider, RolePolicyError,
    Verdict,
};
pub use directorates::m::identity::{AccessTokenRecord, InvalidIdentity, MissionId, SessionId};
pub use directorates::m::roster::{
    AgentRole, CoverIdentity, Operation, Roster, RosterError, Workspace,
};
pub use directorates::m::theaters::{
    LOCAL_THEATER, SshAccess, Theater, TheaterAccess, TheaterError, TheaterKind, TheaterSummary,
    Theaters,
};
pub use directorates::medical::watchdog::{
    DEFAULT_BLOCKED_AFTER, DEFAULT_LINE_HISTORY, DEFAULT_LOOP_MIN_LINES, DEFAULT_STALL_TIMEOUT,
    Watchdog, WatchdogConfig, WatchdogOverrides,
};
pub use directorates::q::safehouse::{
    Change, ChangeLog, SafeHouseInfo, SafeHouseProvider, SafeHouseReport, SafeHouseSession,
    SafeHouseSpec,
};
pub use directorates::q::terminal::{
    AgentTerminal, ExitInfo, READ_TIMED_OUT, Spec, Terminal, TerminalBridge, TerminalSession,
};
pub use directorates::q::{
    DEFAULT_SECRET_LEASE_TTL, Loadout, LoadoutCatalog, MAX_LOADOUT_BYTES, MAX_SECRET_LEASE_TTL,
    McpServer, QError, QErrorKind, SandboxError, SandboxProfile, SandboxRequest, SecretBackend,
    SecretBackendFailure, SecretBroker, SecretError, SecretLease, SecretLeaseRequest,
    SecretRequest, SecretValue, claude_mcp_config,
};
pub use directorates::registry::cables::{
    Cable, CableAction, CableAuditRecord, CableConfig, CableDelivery, CableDraft, CableEndpoint,
    CableError, CableGrant, CableHeldDelivery, CableHeldRecipient, CableInboundMode, CablePolicy,
    CablePrincipal, CableRecipient, CableRelay, CableRelayError, CableRelease, CableScope,
    CableSendRequest, CableState, CableTarget, DEFAULT_CABLE_TTL, MAX_CABLE_BODY_BYTES,
    MAX_CABLE_FANOUT, MAX_CABLE_TTL,
};
pub use directorates::registry::sleepers::{
    Cron, CronError, Fire, SLEEPER_BRIEF_SOURCE, Sleeper, SleeperAuthorization,
};
pub use directorates::registry::sources::{
    CxReport, FinishedSourceBrief, Ingest, SOURCE_BRIEF_PREFIX, Source, SourceBatch, SourceConfig,
    SourceItem, SourcePoll, source_brief,
};
pub use directorates::registry::store::{
    AuditRecord, Brief, DEFAULT_LEASE, FileRegistry, HeadquartersLease, Lease, LeaseRecovery,
    MAX_STATION_MISSION_CAPACITY, MissionProposal, StationRecord, StationSettings,
};
pub use directorates::section_00::agent::{
    AgentEvent, AgentHandler, AgentMessage, AgentResume, AgentSession, AgentStart,
};
pub use guatiao;
#[cfg(feature = "station-tls")]
pub use station::identity::{
    PeerTrustStore, StationFingerprint, StationIdentity, StationIdentityError, TlsTrust,
};
#[cfg(feature = "daemon")]
pub use station::server::{
    DEFAULT_LISTEN, DEFAULT_MAX_CONCURRENT_MISSIONS, DEFAULT_TERMINAL, ServeConfig, StationConfig,
    StationRole, serve, serve_with_capacity, serve_with_config_dir,
    serve_with_config_dir_and_capacity, serve_with_options, serve_with_plugins,
    serve_with_plugins_and_capacity,
};
