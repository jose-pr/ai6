#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

mod access_admin;
mod accounts;
mod agents;
mod compartments;
mod controls;
#[cfg(test)]
mod follow_up_tests;
mod green_ink;
mod held;
mod issues;
pub mod liaison;
mod login_limit;
mod memories;
mod models;
mod moneypenny;
mod page_access;
mod pools;
#[cfg(test)]
mod profile_tests;
#[cfg(test)]
mod rename_tests;
mod reports;
mod reviews;
mod roster;
mod source_reads;
mod timeline;
mod tls;
mod transcripts;
mod tree;
mod verdicts;

use core::future::Future;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::mission_issues::{IssueLink, LinkRelation, LinkState};
use ai6::{
    AccessTokenRecord, AgentRole, Attempt, AuditRecord, Authenticator, AuthorizationRequest,
    Authorizer, Brief, ConfigProvider, Directory, Lookup, MissionHandle, MissionProposal,
    MissionUsage, Origin, Person, Provenance, Quarantine, StationSettings, TimeLimit,
    TimelineEvent, Verdict,
};

/// How a brief's provenance names the gateway as the place it was accepted.
const GATEWAY_RECEIVER: &str = "universal-exports";
use ai6_diptel::{
    FrameReceiver, IncomingFrame, LifecycleEvent, MissionId, OperationId, SessionId, SessionState,
    TerminalData, TerminalDirection, TerminalResize, lifecycle_frames, resize_frames,
    terminal_frames,
};
use axum::Extension;
use axum::body::{Body, Bytes};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::header::{AUTHORIZATION, COOKIE, ORIGIN, RETRY_AFTER, SET_COOKIE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use coqui_peer::PeerId;
use futures_util::{SinkExt, StreamExt};
use guatiao::{Map, TryAsRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;
use tokio::sync::{Mutex, broadcast, mpsc};
use tree::{OperationTree, operation_request, resource_request};
use uuid::Uuid;

pub use access_admin::{EitherAuthorizer, LiveAccess};
pub use agents::{AgentRoute, AgentView, RetirementNotice};
use controls::ControlBook;
pub use controls::ControlStatus;
pub use held::{HeldBriefDecision, HeldBriefEntry};
pub use issues::{EntryView, IssueView, IssueWrite, MissionIssues, QuestionView, ReplyCounts};
use login_limit::LoginGuard;
pub use login_limit::LoginLimits;
pub use moneypenny::{AssistantConfig, PagesConfig, ViewLimits};
pub use pools::{BoardAccount, BoardClaim, BoardWindow, PoolBoard};
pub use roster::{EffectiveOperation, RosterChange, RosterRecord, RosterRecords};
pub use timeline::{TimelineItem, TimelinePage};
pub use tls::tls_server_config;

/// Bytes a frame's length field can grow by past an empty frame's.
const LENGTH_GROWTH: usize = 10;

/// Limits and trusted origin for one gateway instance.
///
/// The Station token is deliberately omitted from `Debug`; callers should load it
/// from their scoped credential source and never log it.
pub struct GatewayConfig {
    public_origin: String,
    station_token: String,
    limits: GatewayLimits,
    attach_token_ttl: Duration,
    write_lease_ttl: Duration,
    login: LoginLimits,
    unex: ai6::UnexLimits,
}

/// Finite state, payload, fanout and input limits for one gateway.
#[derive(Debug, Clone, Copy)]
pub struct GatewayLimits {
    /// Maximum number of live provider Sessions.
    pub max_sessions: usize,
    /// Maximum simultaneous browser connections to one provider Session.
    pub max_clients_per_session: usize,
    /// Number of value frames retained per provider Session.
    pub scrollback_events: usize,
    /// Maximum bytes in one guatiao WebSocket or ingress frame.
    pub max_frame_bytes: usize,
    /// Maximum browser input frames accepted per connection per second.
    pub max_input_frames_per_second: u32,
    /// Capacity of the bounded queue between the gateway and its Stations.
    pub input_queue_capacity: usize,
    /// Capacity of each per-Session viewer fanout queue.
    pub fanout_capacity: usize,
    /// Bytes the gateway may hold in buffers across every Session and connection; the
    /// configuration is refused when its limits could exceed it.
    pub buffer_budget_bytes: ai6::Limit,
    /// Connections the gateway keeps slots for; the configuration is refused when its
    /// Sessions times connections per Session could exceed it.
    pub client_slots: ai6::Limit,
    /// The fewest bytes the Station token may hold.
    pub station_token_min_bytes: usize,
    /// The most bytes the public origin may hold.
    pub origin_bytes: ai6::Limit,
    /// Bytes the write buffer of a browser's socket holds at least.
    pub socket_buffer_bytes: usize,
}

/// User identity providers used to authenticate and authorize Universal Exports requests.
#[derive(Clone)]
pub struct AccessProviders {
    authenticator: Option<Arc<dyn Authenticator>>,
    directory: Arc<dyn Directory>,
    authorizer: Arc<dyn Authorizer>,
    proxy_identity: Option<ProxyIdentity>,
    /// Names the person asking for a page from the request itself; read only by the page
    /// routes.
    page_authenticator: Option<Arc<dyn Authenticator>>,
}

#[derive(Clone)]
struct ProxyIdentity {
    header: HeaderName,
    lookup_by: String,
    /// What the proxy sends in [`PROXY_SECRET_HEADER`]; without it no asserted identity counts.
    secret: Option<String>,
}

/// The header in which the trusted reverse proxy proves it sent a request that asserts an
/// identity: anything that reaches the gateway's port directly can set the identity header too.
pub const PROXY_SECRET_HEADER: &str = "x-ai6-proxy-secret";

impl AccessProviders {
    /// The authorizer as configured, without compartments.
    fn authorizer(&self) -> Arc<dyn Authorizer> {
        self.authorizer.clone()
    }

    /// Combines the configured authentication, identity and authorization providers.
    pub fn new(
        authenticator: Arc<dyn Authenticator>,
        directory: Arc<dyn Directory>,
        authorizer: Arc<dyn Authorizer>,
    ) -> Self {
        Self {
            authenticator: Some(authenticator),
            directory,
            authorizer,
            proxy_identity: None,
            page_authenticator: None,
        }
    }

    /// Uses a sanitized identity header supplied by the trusted reverse proxy.
    ///
    /// The proxy must remove any client-provided value for this header and set
    /// it only after authenticating the request. `lookup_by` selects the
    /// Directory attribute used to resolve that identity (commonly `name` or
    /// `email`).
    pub fn from_reverse_proxy(
        directory: Arc<dyn Directory>,
        authorizer: Arc<dyn Authorizer>,
        header: HeaderName,
        lookup_by: impl Into<String>,
    ) -> Self {
        Self {
            authenticator: None,
            directory,
            authorizer,
            proxy_identity: Some(ProxyIdentity {
                header,
                lookup_by: lookup_by.into(),
                secret: None,
            }),
            page_authenticator: None,
        }
    }

    /// Names the person asking for one of Moneypenny's pages by `authenticator`, a provider that
    /// reads the request itself (`coqui_access_mtls`, `coqui_access_proxy`): it is given the
    /// request's headers but cookies, its client's address, and the certificates the client
    /// proved on [`Gateway::serve_tls`]. Only the page routes ask it; no `/v1` route does.
    #[must_use]
    pub fn with_page_authenticator(mut self, authenticator: Arc<dyn Authenticator>) -> Self {
        self.page_authenticator = Some(authenticator);
        self
    }

    /// Adds trusted reverse-proxy identity as a login method alongside password login.
    pub fn with_proxy_identity(mut self, header: HeaderName, lookup_by: impl Into<String>) -> Self {
        self.proxy_identity = Some(ProxyIdentity {
            header,
            lookup_by: lookup_by.into(),
            secret: None,
        });
        self
    }

    /// The secret the reverse proxy sends in [`PROXY_SECRET_HEADER`] with every request; an
    /// asserted identity is accepted only beside it, so a client that reaches the gateway's port
    /// without the proxy (a local process among them) cannot assert one. Without a secret no
    /// asserted identity is accepted.
    ///
    /// # Errors
    ///
    /// A message when `secret` is shorter than `min_bytes` or there is no proxy
    /// identity to guard.
    pub fn with_proxy_secret(
        mut self,
        secret: impl Into<String>,
        min_bytes: usize,
    ) -> Result<Self, String> {
        let secret = secret.into();
        if secret.len() < min_bytes {
            return Err(format!(
                "the proxy secret must be at least {min_bytes} characters"
            ));
        }
        let proxy = self
            .proxy_identity
            .as_mut()
            .ok_or("a proxy secret needs a proxy identity header")?;
        proxy.secret = Some(secret);
        Ok(self)
    }
}

impl fmt::Debug for AccessProviders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccessProviders([REDACTED])")
    }
}

impl Default for GatewayLimits {
    fn default() -> Self {
        Self::from(&ai6::UnexGatewayLimits::default())
    }
}

impl From<&ai6::UnexGatewayLimits> for GatewayLimits {
    /// The record's `[gateway]` table, an absent bound being as large as the type holds.
    fn from(table: &ai6::UnexGatewayLimits) -> Self {
        Self {
            max_sessions: table.sessions.or_max(),
            max_clients_per_session: table.clients_per_session.or_max(),
            scrollback_events: table.scrollback_events.or_max(),
            max_frame_bytes: table.frame_bytes.or_max(),
            max_input_frames_per_second: table
                .input_frames_per_second
                .get()
                .map_or(u32::MAX, |rate| u32::try_from(rate).unwrap_or(u32::MAX)),
            input_queue_capacity: usize::try_from(table.input_queue).unwrap_or(usize::MAX),
            fanout_capacity: usize::try_from(table.fanout).unwrap_or(usize::MAX),
            buffer_budget_bytes: table.buffer_budget_bytes,
            client_slots: table.client_slots,
            socket_buffer_bytes: table.socket_buffer_bytes.or_max(),
            station_token_min_bytes: usize::try_from(table.station_token_min_bytes)
                .unwrap_or(usize::MAX),
            origin_bytes: table.origin_bytes,
        }
    }
}

impl GatewayConfig {
    /// Creates a gateway config and rejects limits that cannot work together: a zero where
    /// nothing could pass, and Sessions, connections and frames that could hold more than
    /// `buffer_budget_bytes` or `client_slots` allow.
    pub fn new(
        public_origin: impl Into<String>,
        station_token: impl Into<String>,
        limits: GatewayLimits,
        attach_token_ttl: Duration,
        write_lease_ttl: Duration,
    ) -> Result<Self, GatewayError> {
        let public_origin = public_origin.into();
        let station_token = station_token.into();
        let authority = public_origin.strip_prefix("https://").unwrap_or_default();
        let retained_bytes = limits.scrollback_events.checked_mul(limits.max_frame_bytes);
        let fanout_bytes = limits.fanout_capacity.checked_mul(limits.max_frame_bytes);
        let input_bytes = limits
            .input_queue_capacity
            .checked_mul(limits.max_frame_bytes);
        let history_budget = limits
            .max_sessions
            .checked_mul(retained_bytes.unwrap_or(usize::MAX));
        let fanout_budget = limits
            .max_sessions
            .checked_mul(fanout_bytes.unwrap_or(usize::MAX));
        let socket_buffer = limits
            .max_frame_bytes
            .checked_mul(2)
            .map(|bytes| bytes.max(limits.socket_buffer_bytes));
        let socket_budget =
            client_slots_or_max(limits).checked_mul(socket_buffer.unwrap_or(usize::MAX));
        let over_budget = |bytes: Option<usize>| match limits.buffer_budget_bytes.get() {
            Some(budget) => bytes.is_none_or(|bytes| bytes as u64 > budget),
            None => false,
        };
        let over_slots = |slots: Option<usize>| match limits.client_slots.get() {
            Some(budget) => slots.is_none_or(|slots| slots as u64 > budget),
            None => false,
        };
        let total_buffers = history_budget
            .and_then(|history| fanout_budget.and_then(|fanout| history.checked_add(fanout)))
            .and_then(|buffers| buffers.checked_add(input_bytes.unwrap_or(usize::MAX)))
            .and_then(|buffers| socket_budget.and_then(|sockets| buffers.checked_add(sockets)));
        let client_slots = limits
            .max_sessions
            .checked_mul(limits.max_clients_per_session);
        if authority.is_empty()
            || authority.contains(['/', '?', '#', '@'])
            || !limits.origin_bytes.allows(public_origin.len())
            || station_token.len() < limits.station_token_min_bytes
            || limits.max_sessions == 0
            || limits.max_clients_per_session == 0
            || limits.scrollback_events == 0
            || limits.max_frame_bytes == 0
            || limits.max_input_frames_per_second == 0
            || limits.input_queue_capacity == 0
            || limits.fanout_capacity == 0
            || over_slots(client_slots)
            || over_budget(total_buffers)
            || attach_token_ttl.is_zero()
            || write_lease_ttl.is_zero()
        {
            return Err(GatewayError::InvalidConfiguration);
        }
        Ok(Self {
            public_origin: public_origin.trim_end_matches('/').to_owned(),
            station_token,
            limits,
            attach_token_ttl,
            write_lease_ttl,
            login: LoginLimits::default(),
            unex: ai6::UnexLimits::default(),
        })
    }

    /// Replaces the default `[controls]`, `[timeline]` and `[text]` limits (and `[login]`'s
    /// pace) with the `unex` record's; the `[gateway]` table is `limits` of [`GatewayConfig::new`].
    #[must_use]
    pub fn with_unex_limits(mut self, limits: ai6::UnexLimits) -> Self {
        self.unex = limits;
        self
    }

    /// The `unex` record's limit tables this gateway runs by.
    pub(crate) fn unex(&self) -> &ai6::UnexLimits {
        &self.unex
    }

    /// Replaces the default limits on password and key tries at the login route.
    #[must_use]
    pub fn with_login_limits(mut self, login: LoginLimits) -> Self {
        self.login = login;
        self
    }

    /// The origin browser requests must match exactly.
    pub fn public_origin(&self) -> &str {
        &self.public_origin
    }
}

impl fmt::Debug for GatewayConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayConfig")
            .field("public_origin", &self.public_origin)
            .field("station_token", &"[REDACTED]")
            .field("limits", &self.limits)
            .field("attach_token_ttl", &self.attach_token_ttl)
            .field("write_lease_ttl", &self.write_lease_ttl)
            .field("login", &self.login)
            .field("unex", &self.unex)
            .finish()
    }
}

/// Whether an attachment may only watch or may send terminal input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AttachMode {
    /// Observe board state and terminal output.
    Read,
    /// Observe output and request terminal input through an expiring lease.
    Write,
}

/// An authorization grant scoped to one Station, Mission, Session and mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachGrant {
    /// The project that owns this work.
    pub operation_id: OperationId,
    /// The unit of work that groups Sessions.
    pub mission_id: MissionId,
    /// The provider-issued resumable operation.
    pub session_id: SessionId,
    /// Read or write rights granted by M.
    pub mode: AttachMode,
}

/// A queued terminal input frame sent by an attached viewer.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentInput {
    /// The Operation containing the Mission.
    pub operation_id: OperationId,
    /// The unit of work that groups Sessions.
    pub mission_id: MissionId,
    /// The provider-issued Agent Session receiving input.
    pub session_id: SessionId,
    /// Validated guatiao channel frame, kept as bytes.
    pub frame: Vec<u8>,
}

impl fmt::Debug for AgentInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentInput")
            .field("operation_id", &self.operation_id)
            .field("mission_id", &self.mission_id)
            .field("session_id", &self.session_id)
            .field(
                "frame",
                &format_args!("[REDACTED; {} bytes]", self.frame.len()),
            )
            .finish()
    }
}

/// A successfully consumed viewer attachment.
pub struct Attachment {
    /// Read or write access for this connection.
    pub mode: AttachMode,
    /// Schema frames and scrollback frames to send before live fanout.
    pub replay: Vec<Vec<u8>>,
    /// Live event stream. A lagged receiver must be disconnected.
    pub events: broadcast::Receiver<Vec<u8>>,
    session_id: SessionId,
    connection_id: String,
    expires: Instant,
}

impl fmt::Debug for Attachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attachment")
            .field("mode", &self.mode)
            .field("replay_frames", &self.replay.len())
            .field("session_id", &self.session_id)
            .field("connection_id", &"[REDACTED]")
            .field("expires", &self.expires)
            .finish()
    }
}

/// Failures returned by gateway policy and bounded-state operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GatewayError {
    /// A required setting is absent or outside its accepted bound.
    InvalidConfiguration,
    /// The caller did not authenticate as a Station.
    Unauthorized,
    /// A grant does not match the registered Station, Mission or Session.
    Forbidden,
    /// A requested Mission or Session has not been registered.
    NotFound,
    /// A viewer, writer or prior event already occupies this bounded slot.
    Conflict,
    /// A frame or history event exceeded a configured bound.
    TooLarge,
    /// Guatiao rejected a malformed or schema-invalid frame.
    InvalidFrame,
    /// The one bounded queue for agent input has no capacity.
    Backpressure,
    /// An attachment credential is unknown, expired or already consumed.
    InvalidCredential,
}

impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidConfiguration => "gateway configuration is invalid",
            Self::Unauthorized => "Station authentication is required",
            Self::Forbidden => "the grant does not authorize this Station and Session",
            Self::NotFound => "the Station or Session was not found",
            Self::Conflict => "a bounded gateway resource is already occupied",
            Self::TooLarge => "the frame exceeds the configured bound",
            Self::InvalidFrame => "the guatiao frame is invalid",
            Self::Backpressure => "the bounded input queue is full",
            Self::InvalidCredential => "the attachment credential is invalid or expired",
        };
        f.write_str(message)
    }
}

impl std::error::Error for GatewayError {}

/// Gateway state shared by the HTTP handler and the Station channel.
#[derive(Clone)]
pub struct Gateway {
    config: std::sync::Arc<GatewayConfig>,
    inner: std::sync::Arc<Mutex<Inner>>,
    input_tx: mpsc::Sender<AgentInput>,
    input_rx: std::sync::Arc<Mutex<mpsc::Receiver<AgentInput>>>,
    registry: Option<Arc<dyn RegistryBackend>>,
    access: Option<AccessProviders>,
    roster: Option<Arc<dyn ConfigProvider>>,
    access_admin: Option<Arc<access_admin::LiveAccess>>,
    compartment_reads: compartments::AuditMemory,
    accounts: Option<Arc<dyn ai6::Accounts>>,
    login_guard: std::sync::Arc<LoginGuard>,
    /// Refused resumes, apart from `login_guard` so they cost nobody a try at their password.
    resume_guard: std::sync::Arc<LoginGuard>,
    controls: Arc<std::sync::Mutex<ControlBook>>,
    control_ready: Arc<tokio::sync::Notify>,
    assistant: Option<Arc<moneypenny::Assistant>>,
    job_poll_max_wait: Duration,
    pages: Option<Arc<moneypenny::pages::PageStore>>,
    pages_config: Option<PagesConfig>,
    /// The person each live [`StandIn`] reads as, by the SHA-256 of its bearer.
    stand_ins: Arc<std::sync::Mutex<HashMap<String, String>>>,
    /// Live tracker reads waiting for the Headquarters Station.
    source_reads: Arc<source_reads::SourceReads>,
    /// The images people send Moneypenny, kept beside the pages.
    images: Option<Arc<moneypenny::images::ImageStore>>,
    /// The peers Liaison serves A2A to, on links and on `/v1/a2a`.
    liaison: Option<Arc<liaison::LiaisonConfig>>,
}

/// A bearer the gateway makes for itself to read, in this process, as a person who proved who
/// they are some other way (a page's viewer). It is never stored or sent anywhere, and it stops
/// working when dropped.
pub(crate) struct StandIn {
    token: String,
    digest: String,
    stand_ins: Arc<std::sync::Mutex<HashMap<String, String>>>,
}

impl StandIn {
    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.stand_ins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.digest);
    }
}

impl fmt::Debug for Gateway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gateway")
            .field("config", &self.config)
            .field("state", &"[REDACTED]")
            .finish()
    }
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<SessionId, SessionRecord>,
    missions: HashMap<(OperationId, MissionId), LaunchManifest>,
    attach_tokens: HashMap<String, TimedGrant>,
    connection_tickets: HashMap<String, TimedGrant>,
    connections: HashMap<String, ActiveConnection>,
}

struct TimedGrant {
    grant: AttachGrant,
    expires: Instant,
}

struct ActiveConnection {
    session_id: SessionId,
    mode: AttachMode,
    expires: Instant,
    input_window_started: Instant,
    input_window_count: u32,
    last_input_sequence: u64,
}

struct SessionRecord {
    operation_id: OperationId,
    mission_id: MissionId,
    lifecycle_state: Option<String>,
    lifecycle_reason: Option<String>,
    lifecycle_sequence: u64,
    lifecycle_at: Option<f64>,
    history: VecDeque<(u64, Vec<u8>)>,
    schemas: HashMap<u64, Vec<u8>>,
    last_sequence: u64,
    events: broadcast::Sender<Vec<u8>>,
    lease: Option<WriteLease>,
    active_clients: usize,
}

struct WriteLease {
    connection_id: String,
    expires: Instant,
}

impl Gateway {
    /// Creates the bounded gateway state and the one authenticated Station input queue.
    pub fn new(config: GatewayConfig) -> Self {
        let (input_tx, input_rx) = mpsc::channel(config.limits.input_queue_capacity);
        let login_guard = std::sync::Arc::new(LoginGuard::new(&config.login));
        let resume_guard = std::sync::Arc::new(LoginGuard::new(&config.login));
        let controls = ControlBook::new(config.unex().controls.clone());
        Self {
            config: std::sync::Arc::new(config),
            inner: std::sync::Arc::new(Mutex::new(Inner::default())),
            input_tx,
            input_rx: std::sync::Arc::new(Mutex::new(input_rx)),
            registry: None,
            access: None,
            roster: None,
            access_admin: None,
            compartment_reads: Default::default(),
            accounts: None,
            login_guard,
            resume_guard,
            controls: Arc::new(std::sync::Mutex::new(controls)),
            control_ready: Arc::default(),
            assistant: None,
            job_poll_max_wait: ai6::AssistantGatewayLimits::default().max_wait.0,
            pages: None,
            pages_config: None,
            stand_ins: Arc::default(),
            source_reads: Arc::default(),
            images: None,
            liaison: None,
        }
    }

    /// A [`StandIn`] reading as `person_id` until it is dropped.
    pub(crate) fn stand_in(&self, person_id: &str) -> StandIn {
        let token = new_secret();
        let digest = access_token_digest(&token);
        self.stand_ins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(digest.clone(), person_id.to_owned());
        StandIn {
            token,
            digest,
            stand_ins: self.stand_ins.clone(),
        }
    }

    /// Creates a gateway backed by the shared Registry for user access tokens.
    pub fn with_registry(config: GatewayConfig, registry: Arc<dyn RegistryBackend>) -> Self {
        let mut gateway = Self::new(config);
        gateway.registry = Some(registry);
        gateway
    }

    /// Creates a gateway backed by the shared Registry and configured user access providers.
    pub fn with_access(
        config: GatewayConfig,
        registry: Arc<dyn RegistryBackend>,
        access: AccessProviders,
    ) -> Self {
        let mut gateway = Self::with_registry(config, registry);
        gateway.access = Some(access);
        gateway
    }

    /// Serves A2A to `config`'s peers: on the HTTP JSON-RPC binding at `/v1/a2a` to a person
    /// signed in as a peer's principal, and on links through [`Gateway::liaison`] and a
    /// [`liaison::LinkListener`]. Without it `/v1/a2a` answers 404.
    #[must_use]
    pub fn with_liaison(mut self, config: liaison::LiaisonConfig) -> Self {
        self.liaison = Some(Arc::new(config));
        self
    }

    /// Liaison's A2A agent over this gateway, when [`Gateway::with_liaison`] set one up.
    pub fn liaison(&self) -> Option<liaison::Liaison> {
        let config = self.liaison.clone()?;
        Some(liaison::Liaison::shared(self.clone(), config))
    }

    /// Serves roster administration (`/v1/user/roster/...`) from `source`, the roster every
    /// Station reads. Edits need a source that takes changes, such as a SQL roster; without a
    /// source those routes answer 503.
    #[must_use]
    pub fn with_roster(mut self, source: Arc<dyn ConfigProvider>) -> Self {
        self.roster = Some(source);
        self
    }

    /// What this request is allowed: the access policy with what compartments add, what they
    /// add read once, and each read they alone allow written to the audit log.
    fn authorizer(&self) -> Result<Arc<compartments::CompartmentAccess>, ApiError> {
        let every = self.config.unex().gateway.compartment_audit.0.as_secs_f64();
        self.compartment_access(Some(every))
    }

    /// [`authorizer`](Self::authorizer) for what only tells a person what they could read,
    /// which reads nothing and writes nothing.
    fn probing_authorizer(&self) -> Result<Arc<compartments::CompartmentAccess>, ApiError> {
        self.compartment_access(None)
    }

    fn compartment_access(
        &self,
        audit_every: Option<f64>,
    ) -> Result<Arc<compartments::CompartmentAccess>, ApiError> {
        let access = self.access.as_ref().ok_or_else(ApiError::unavailable)?;
        Ok(Arc::new(compartments::CompartmentAccess::new(
            access.authorizer(),
            self.roster.clone(),
            self.registry.clone(),
            self.compartment_reads.clone(),
            audit_every,
        )))
    }

    /// Serves the access administration routes (`/v1/user/access`, an Operation's `access`) over
    /// `access`, the policy in force.
    #[must_use]
    pub fn with_access_admin(mut self, access: Arc<access_admin::LiveAccess>) -> Self {
        self.access_admin = Some(access);
        self
    }

    /// Serves the account routes (`/v1/user/accounts`) over `accounts`, the provider that keeps
    /// the accounts people sign in with.
    #[must_use]
    pub fn with_accounts(mut self, accounts: Arc<dyn ai6::Accounts>) -> Self {
        self.accounts = Some(accounts);
        self
    }

    /// Serves Moneypenny, the operator's assistant: the app's conversation socket, her tools'
    /// MCP endpoint and the Stations' assistant routes. Her model runs on a Station as the
    /// roster Agent the config names. Without it the socket answers 503.
    #[must_use]
    pub fn with_assistant(mut self, config: AssistantConfig) -> Self {
        self.assistant = Some(Arc::new(moneypenny::Assistant::new(config)));
        self
    }

    /// How long a Station's assistant job poll is held while Moneypenny is not served
    /// (`[assistant.gateway] max_wait`); once she is, her config decides.
    #[must_use]
    pub fn with_job_poll_max_wait(mut self, max_wait: Duration) -> Self {
        self.job_poll_max_wait = max_wait;
        self
    }

    /// Keeps the pages Moneypenny publishes in `config.dir` and serves them: under `/pages/` on
    /// the API's own origin, or, with `config.origin`, only on the pages' own listener
    /// ([`Gateway::pages_router`]). Needs [`Gateway::with_assistant`] to publish anything.
    ///
    /// # Errors
    ///
    /// The directory cannot be made or read, or the origin is not an origin.
    pub fn with_pages(mut self, config: PagesConfig) -> Result<Self, GatewayError> {
        if let Some(origin) = &config.origin {
            let plain = origin.trim_end_matches('/');
            if !(plain.starts_with("https://") || plain.starts_with("http://"))
                || plain.contains(['?', '#'])
                || plain.matches('/').count() != 2
            {
                return Err(GatewayError::InvalidConfiguration);
            }
        }
        let store = moneypenny::pages::PageStore::open(config.dir.clone())
            .map_err(|_| GatewayError::InvalidConfiguration)?;
        let images = moneypenny::images::ImageStore::open(config.dir.join(".images"))
            .map_err(|_| GatewayError::InvalidConfiguration)?;
        self.images = Some(Arc::new(images));
        self.pages = Some(Arc::new(store));
        self.pages_config = Some(config);
        Ok(self)
    }

    /// The router of the pages' own listener: page routes only, no `/v1`. `None` unless
    /// [`Gateway::with_pages`] gave an origin.
    pub fn pages_router(&self) -> Option<Router> {
        self.pages_config.as_ref()?.origin.as_ref()?;
        self.start_sweeper();
        Some(
            moneypenny::pages::pages_router()
                .layer(axum::extract::DefaultBodyLimit::max(0))
                .with_state(self.clone()),
        )
    }

    /// Serves the pages' own listener on `address` until `shutdown`.
    ///
    /// # Errors
    ///
    /// The address cannot be bound, or there is no pages origin.
    pub async fn serve_pages<F>(
        self,
        address: SocketAddr,
        shutdown: F,
    ) -> Result<(), std::io::Error>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let router = self
            .pages_router()
            .ok_or_else(|| std::io::Error::other("pages have no origin of their own"))?;
        let listener = tokio::net::TcpListener::bind(address).await?;
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
    }

    /// Starts the page sweeper once, when a runtime runs.
    fn start_sweeper(&self) {
        let Some(store) = &self.pages else {
            return;
        };
        if store.start_sweeper()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(moneypenny::page_tools::sweeper(self.clone()));
        }
    }

    /// Checks whether a presented secret is the configured Station channel credential.
    pub fn authenticate_station(&self, presented: &str) -> Result<(), GatewayError> {
        if constant_time_eq(self.config.station_token.as_bytes(), presented.as_bytes()) {
            Ok(())
        } else {
            Err(GatewayError::Unauthorized)
        }
    }

    /// Registers one active provider Session after authenticating the Station.
    pub async fn register_session(
        &self,
        station_token: &str,
        operation_id: OperationId,
        mission_id: MissionId,
        session_id: SessionId,
    ) -> Result<(), GatewayError> {
        self.authenticate_station(station_token)?;
        let mut inner = self.inner.lock().await;
        if let Some(record) = inner.sessions.get(&session_id) {
            if record.operation_id == operation_id && record.mission_id == mission_id {
                return Ok(());
            }
            return Err(GatewayError::Conflict);
        }
        if inner.sessions.len() >= self.config.limits.max_sessions {
            return Err(GatewayError::Conflict);
        }
        let (events, _) = broadcast::channel(self.config.limits.fanout_capacity);
        inner.sessions.insert(
            session_id.clone(),
            SessionRecord {
                operation_id,
                mission_id,
                lifecycle_state: None,
                lifecycle_reason: None,
                lifecycle_sequence: 0,
                lifecycle_at: None,
                history: VecDeque::new(),
                schemas: HashMap::new(),
                last_sequence: 0,
                events,
                lease: None,
                active_clients: 0,
            },
        );
        Ok(())
    }

    /// Returns the next event cursor for an authenticated registered Session.
    pub async fn next_sequence(
        &self,
        station_token: &str,
        session_id: &SessionId,
    ) -> Result<u64, GatewayError> {
        self.authenticate_station(station_token)?;
        let inner = self.inner.lock().await;
        let session = inner
            .sessions
            .get(session_id)
            .ok_or(GatewayError::NotFound)?;
        session
            .last_sequence
            .checked_add(1)
            .ok_or(GatewayError::Conflict)
    }

    /// Returns the current provider Sessions, their latest lifecycle states and the Agent of
    /// each one's Mission brief.
    pub async fn board_sessions(&self) -> Vec<BoardSession> {
        let inner = self.inner.lock().await;
        let mut sessions = inner
            .sessions
            .iter()
            .map(|(session_id, session)| BoardSession {
                operation_id: session.operation_id.to_string(),
                mission_id: session.mission_id.to_string(),
                session_id: session_id.to_string(),
                state: session.lifecycle_state.clone(),
                reason: session.lifecycle_reason.clone(),
                sequence: session.lifecycle_sequence,
                updated_at: session.lifecycle_at,
                exposure: Vec::new(),
                station_id: None,
                agent: inner
                    .missions
                    .get(&(session.operation_id.clone(), session.mission_id.clone()))
                    .and_then(|brief| brief.agent_id.clone()),
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| {
            (&left.operation_id, &left.mission_id, &left.session_id).cmp(&(
                &right.operation_id,
                &right.mission_id,
                &right.session_id,
            ))
        });
        sessions
    }

    /// Returns registered Missions grouped by Station with their provider Sessions.
    pub async fn board_missions(&self) -> Vec<BoardMission> {
        self.enrich_missions(group_missions(self.board_sessions().await))
            .await
    }

    async fn publish_mission_brief(
        &self,
        station_token: &str,
        operation_id: OperationId,
        mission_id: MissionId,
        brief: LaunchManifest,
    ) -> Result<(), GatewayError> {
        self.authenticate_station(station_token)?;
        let mut inner = self.inner.lock().await;
        // AI6 is authoritative: a resumed operation can publish a refreshed manifest.
        inner.missions.insert((operation_id, mission_id), brief);
        Ok(())
    }

    async fn enrich_missions(&self, mut missions: Vec<BoardMission>) -> Vec<BoardMission> {
        if let Some(registry) = self.registry.clone() {
            missions = tokio::task::spawn_blocking(move || {
                let mut attempts = registry
                    .sessions()
                    .map(attempts_by_mission)
                    .unwrap_or_default();
                for mission in &mut missions {
                    mission.usage = registry
                        .mission_usage(Some(&mission.operation_id), &mission.mission_id)
                        .ok()
                        .flatten();
                    mission.attempts = attempts.remove(&mission.mission_id).unwrap_or_default();
                    if let Ok(Some(found)) = registry.resolve_mission(&mission.mission_id)
                        && found.mission_id == mission.mission_id
                    {
                        mission.handle = found.handle;
                    }
                }
                missions
            })
            .await
            .unwrap_or_default();
        }
        let inner = self.inner.lock().await;
        for mission in &mut missions {
            mission.brief = inner.missions.iter().find_map(|((station, id), brief)| {
                (station.as_str() == mission.operation_id && id.as_str() == mission.mission_id)
                    .then(|| brief.clone())
            });
        }
        missions
    }

    /// Encodes and publishes one lifecycle event for a registered provider Session.
    pub async fn publish_lifecycle(
        &self,
        station_token: &str,
        event: LifecycleEvent,
    ) -> Result<(), GatewayError> {
        let (schema, value) = lifecycle_frames(&event).map_err(|_| GatewayError::InvalidFrame)?;
        self.ingest(station_token, &event.session_id, 0, schema)
            .await?;
        self.ingest(station_token, &event.session_id, event.sequence, value)
            .await
    }

    /// Encodes and publishes raw terminal output without requiring the Station to encode guatiao.
    pub async fn publish_terminal_output(
        &self,
        station_token: &str,
        session_id: SessionId,
        sequence: u64,
        bytes: Vec<u8>,
    ) -> Result<(), GatewayError> {
        let terminal = TerminalData::new(
            session_id.clone(),
            sequence,
            TerminalDirection::Output,
            bytes,
        )
        .map_err(|_| GatewayError::TooLarge)?;
        let (schema, value) = terminal_frames(&terminal).map_err(|_| GatewayError::InvalidFrame)?;
        self.ingest(station_token, &session_id, 0, schema).await?;
        self.ingest(station_token, &session_id, sequence, value)
            .await
    }

    /// The most terminal output that fits one frame of `session_id` at any sequence, and that the
    /// wire contract allows: what a Station is told, so that it splits longer output itself.
    pub fn terminal_bytes(&self, session_id: &SessionId) -> usize {
        let header = TerminalData::new(
            session_id.clone(),
            u64::MAX,
            TerminalDirection::Output,
            Vec::new(),
        )
        .ok()
        .and_then(|empty| terminal_frames(&empty).ok())
        .map_or(usize::MAX, |(_, frame)| {
            frame.len().saturating_add(LENGTH_GROWTH)
        });
        self.config
            .limits
            .max_frame_bytes
            .saturating_sub(header)
            .min(ai6_diptel::MAX_TERMINAL_BYTES)
    }

    /// Mints an opaque, short-lived, single-use token for an existing Session.
    pub async fn issue_attach_token(
        &self,
        station_token: &str,
        grant: AttachGrant,
    ) -> Result<String, GatewayError> {
        self.authenticate_station(station_token)?;
        self.store_attach_token(grant).await
    }

    async fn store_attach_token(&self, grant: AttachGrant) -> Result<String, GatewayError> {
        let mut inner = self.inner.lock().await;
        let session = inner
            .sessions
            .get(&grant.session_id)
            .ok_or(GatewayError::NotFound)?;
        if session.operation_id != grant.operation_id || session.mission_id != grant.mission_id {
            return Err(GatewayError::Forbidden);
        }
        prune_expired(&mut inner);
        if inner.attach_tokens.len() + inner.connection_tickets.len()
            >= pending_credential_capacity(&self.config)
        {
            return Err(GatewayError::Conflict);
        }
        let token = Uuid::new_v4().simple().to_string();
        inner.attach_tokens.insert(
            token.clone(),
            TimedGrant {
                grant,
                expires: Instant::now() + self.config.attach_token_ttl,
            },
        );
        Ok(token)
    }

    /// Exchanges an attach token for a cookie-only, one-connection ticket.
    pub async fn exchange_attach_token(&self, token: &str) -> Result<String, GatewayError> {
        let mut inner = self.inner.lock().await;
        prune_expired(&mut inner);
        if inner.attach_tokens.len() + inner.connection_tickets.len()
            >= pending_credential_capacity(&self.config)
        {
            return Err(GatewayError::Conflict);
        }
        let timed = inner
            .attach_tokens
            .remove(token)
            .ok_or(GatewayError::InvalidCredential)?;
        let ticket = Uuid::new_v4().simple().to_string();
        inner.connection_tickets.insert(ticket.clone(), timed);
        Ok(ticket)
    }

    /// Consumes a connection ticket and grants one bounded viewer slot.
    ///
    /// The returned replay starts with the latest schema announcement for each
    /// channel, followed by scrollback newer than `after_sequence`. Where that scrollback starts
    /// later than `after_sequence` or skips a sequence, a [`TerminalGap`](ai6_diptel::TerminalGap)
    /// says so, ahead of the frame after it.
    pub async fn attach(
        &self,
        session_id: &SessionId,
        ticket: &str,
        after_sequence: u64,
    ) -> Result<Attachment, GatewayError> {
        let mut inner = self.inner.lock().await;
        prune_expired(&mut inner);
        let timed = inner
            .connection_tickets
            .remove(ticket)
            .ok_or(GatewayError::InvalidCredential)?;
        if &timed.grant.session_id != session_id {
            return Err(GatewayError::Forbidden);
        }
        let session = inner
            .sessions
            .get_mut(session_id)
            .ok_or(GatewayError::NotFound)?;
        if session.operation_id != timed.grant.operation_id
            || session.mission_id != timed.grant.mission_id
        {
            return Err(GatewayError::Forbidden);
        }
        if session.active_clients >= self.config.limits.max_clients_per_session {
            return Err(GatewayError::Conflict);
        }
        let now = Instant::now();
        if timed.grant.mode == AttachMode::Write
            && session
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires > now)
        {
            return Err(GatewayError::Conflict);
        }
        let connection_id = Uuid::new_v4().simple().to_string();
        let connection_expires = timed.expires;
        let lease_expires = (now + self.config.write_lease_ttl).min(connection_expires);
        if timed.grant.mode == AttachMode::Write {
            session.lease = Some(WriteLease {
                connection_id: connection_id.clone(),
                expires: lease_expires,
            });
        }
        let mut replay: Vec<_> = session.schemas.values().cloned().collect();
        let mut position = after_sequence;
        for (sequence, frame) in session
            .history
            .iter()
            .filter(|(sequence, _)| *sequence > after_sequence)
        {
            if let Some([schema, gap]) = gap_between(session_id, position, *sequence)? {
                replay.extend([schema, gap]);
            }
            replay.push(frame.clone());
            position = *sequence;
        }
        let events = session.events.subscribe();
        session.active_clients += 1;
        inner.connections.insert(
            connection_id.clone(),
            ActiveConnection {
                session_id: session_id.clone(),
                mode: timed.grant.mode,
                expires: connection_expires,
                input_window_started: now,
                input_window_count: 0,
                last_input_sequence: 0,
            },
        );
        Ok(Attachment {
            mode: timed.grant.mode,
            replay,
            events,
            session_id: session_id.clone(),
            connection_id,
            expires: connection_expires,
        })
    }

    /// Validates and records a Station-originated guatiao frame, then fans it out.
    pub async fn ingest(
        &self,
        station_token: &str,
        session_id: &SessionId,
        sequence: u64,
        frame: Vec<u8>,
    ) -> Result<(), GatewayError> {
        self.authenticate_station(station_token)?;
        if frame.len() > self.config.limits.max_frame_bytes {
            return Err(GatewayError::TooLarge);
        }
        let mut inner = self.inner.lock().await;
        let transition = self.ingest_locked(&mut inner, session_id, sequence, frame)?;
        drop(inner);
        if let Some(transition) = transition {
            timeline::record_transition(self, transition).await;
        }
        Ok(())
    }

    /// Validates, records and fans out one frame; returns the lifecycle transition it made.
    fn ingest_locked(
        &self,
        inner: &mut Inner,
        session_id: &SessionId,
        sequence: u64,
        frame: Vec<u8>,
    ) -> Result<Option<timeline::Transition>, GatewayError> {
        let session = inner
            .sessions
            .get_mut(session_id)
            .ok_or(GatewayError::NotFound)?;
        let mut receiver = guatiao::value::wire::channel::Receiver::new();
        for schema in session.schemas.values() {
            receiver
                .accept(schema)
                .map_err(|_| GatewayError::InvalidFrame)?;
        }
        let received = receiver
            .accept(&frame)
            .map_err(|_| GatewayError::InvalidFrame)?;
        let channel = match received {
            guatiao::value::wire::channel::Received::Schema(channel) => channel,
            guatiao::value::wire::channel::Received::Value { channel, .. } => channel,
            guatiao::value::wire::channel::Received::Closed(channel) => channel,
        };
        if !matches!(channel, 1 | 2) {
            return Err(GatewayError::InvalidFrame);
        }
        let is_value = matches!(
            &received,
            guatiao::value::wire::channel::Received::Value { .. }
        );
        if (is_value && (sequence == 0 || sequence <= session.last_sequence))
            || (!is_value && sequence != 0)
        {
            return Err(GatewayError::Conflict);
        }
        if matches!(
            &received,
            guatiao::value::wire::channel::Received::Schema(_)
        ) && session.schemas.get(&channel) == Some(&frame)
        {
            return Ok(None);
        }
        match &received {
            guatiao::value::wire::channel::Received::Value { channel: 1, value } => {
                validate_lifecycle_value(
                    value,
                    &session.operation_id,
                    &session.mission_id,
                    session_id,
                    sequence,
                )?
            }
            guatiao::value::wire::channel::Received::Value { channel: 2, value } => {
                validate_terminal_output(
                    value,
                    session_id,
                    sequence,
                    self.config.limits.max_frame_bytes,
                )?
            }
            _ => {}
        }
        let lifecycle_state = match &received {
            guatiao::value::wire::channel::Received::Value { channel: 1, value } => {
                let map: &Map = value.try_as_ref().ok_or(GatewayError::InvalidFrame)?;
                let reason = match map.get("reason") {
                    Some(reason) => Some(
                        <&str>::try_from(reason)
                            .ok()
                            .filter(|reason| ai6_diptel::valid_reason(reason))
                            .ok_or(GatewayError::InvalidFrame)?
                            .to_owned(),
                    ),
                    None => None,
                };
                Some((read_text(map, "state")?.to_owned(), reason))
            }
            _ => None,
        };
        match received {
            guatiao::value::wire::channel::Received::Schema(_) => {
                session.schemas.insert(channel, frame.clone());
            }
            guatiao::value::wire::channel::Received::Closed(_) => {
                session.schemas.remove(&channel);
            }
            guatiao::value::wire::channel::Received::Value { .. } => {}
        }
        let mut transition = None;
        if is_value {
            // A viewer that is past the gap's start ignores it, so there is no need to know here
            // what each has seen.
            if let Some(frames) = gap_between(session_id, session.last_sequence, sequence)? {
                for frame in frames {
                    let _ = session.events.send(frame);
                }
            }
            session.last_sequence = sequence;
            if let Some((state, reason)) = lifecycle_state {
                let at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0.0, |elapsed| elapsed.as_secs_f64());
                // A repeated state is a re-publication, not a transition.
                if session.lifecycle_state.as_deref() != Some(state.as_str()) {
                    transition = Some(timeline::Transition {
                        operation: session.operation_id.to_string(),
                        mission: session.mission_id.to_string(),
                        session: session_id.to_string(),
                        state: state.clone(),
                        station: None,
                        at,
                    });
                }
                session.lifecycle_state = Some(state);
                session.lifecycle_reason = reason;
                session.lifecycle_sequence = sequence;
                session.lifecycle_at = Some(at);
            }
            session.history.push_back((sequence, frame.clone()));
            while session.history.len() > self.config.limits.scrollback_events {
                session.history.pop_front();
            }
        }
        let _ = session.events.send(frame);
        if let Some(transition) = &mut transition {
            transition.station = inner
                .missions
                .iter()
                .find(|((operation, mission), _)| {
                    operation.as_str() == transition.operation
                        && mission.as_str() == transition.mission
                })
                .map(|(_, brief)| brief.station_id.clone());
        }
        Ok(transition)
    }

    /// Queues one browser frame for the Station after confirming an active write lease.
    pub async fn submit_input(
        &self,
        session_id: &SessionId,
        connection_id: &str,
        frame: Vec<u8>,
    ) -> Result<(), GatewayError> {
        if frame.len() > self.config.limits.max_frame_bytes {
            return Err(GatewayError::TooLarge);
        }
        let input_sequence = validate_input_frame(session_id, &frame)?;
        let mut inner = self.inner.lock().await;
        let now = Instant::now();
        let connection = inner
            .connections
            .get(connection_id)
            .ok_or(GatewayError::InvalidCredential)?;
        if &connection.session_id != session_id
            || connection.mode != AttachMode::Write
            || connection.expires <= now
        {
            return Err(GatewayError::Forbidden);
        }
        if input_sequence <= connection.last_input_sequence {
            return Err(GatewayError::Conflict);
        }
        if let Some(session) = inner.sessions.get(session_id)
            && !session
                .lease
                .as_ref()
                .is_some_and(|lease| lease.connection_id == connection_id && lease.expires > now)
        {
            return Err(GatewayError::Forbidden);
        }
        let connection = inner
            .connections
            .get_mut(connection_id)
            .ok_or(GatewayError::InvalidCredential)?;
        if now.duration_since(connection.input_window_started) >= Duration::from_secs(1) {
            connection.input_window_started = now;
            connection.input_window_count = 0;
        }
        if connection.input_window_count >= self.config.limits.max_input_frames_per_second {
            return Err(GatewayError::Backpressure);
        }
        connection.input_window_count += 1;
        connection.last_input_sequence = input_sequence;
        let session = inner
            .sessions
            .get(session_id)
            .ok_or(GatewayError::NotFound)?;
        let input = AgentInput {
            operation_id: session.operation_id.clone(),
            mission_id: session.mission_id.clone(),
            session_id: session_id.clone(),
            frame,
        };
        self.input_tx
            .try_send(input)
            .map_err(|_| GatewayError::Backpressure)
    }

    /// Releases a viewer slot and its write lease when a socket closes.
    pub async fn disconnect(&self, attachment: Attachment) {
        self.disconnect_connection(&attachment.session_id, &attachment.connection_id)
            .await;
    }

    /// Releases an active connection by its non-secret internal handle.
    pub async fn disconnect_connection(&self, session_id: &SessionId, connection_id: &str) {
        let mut inner = self.inner.lock().await;
        if inner.connections.remove(connection_id).is_none() {
            return;
        }
        if let Some(session) = inner.sessions.get_mut(session_id) {
            session.active_clients = session.active_clients.saturating_sub(1);
            if session
                .lease
                .as_ref()
                .is_some_and(|lease| lease.connection_id == connection_id)
            {
                session.lease = None;
            }
        }
    }

    /// Waits for the next terminal input from an authenticated Station connection.
    pub async fn receive_agent_input(&self) -> Option<AgentInput> {
        self.input_rx.lock().await.recv().await
    }

    /// Waits at most `wait` for the next input; `None` when none came.
    pub async fn receive_agent_input_within(&self, wait: Duration) -> Option<AgentInput> {
        tokio::time::timeout(wait, self.receive_agent_input())
            .await
            .ok()
            .flatten()
    }

    /// Returns the configured public HTTPS origin for same-origin checks.
    pub fn public_origin(&self) -> &str {
        self.config.public_origin()
    }

    /// Returns the Axum service for the Station channel, browser attach and terminal sockets.
    pub fn router(self) -> Router {
        let max_frame_bytes = self.config.limits.max_frame_bytes;
        self.start_sweeper();
        if let (Some(assistant), Some(registry)) = (&self.assistant, &self.registry) {
            assistant.start_log(registry.clone());
        }
        let own_origin = self
            .pages_config
            .as_ref()
            .is_some_and(|config| config.origin.is_some());
        let pages = (self.pages.is_some() && !own_origin)
            .then(|| Router::new().nest("/pages", moneypenny::pages::pages_router()));
        let base = Router::new()
            .route(
                "/v1/station/assistant/pages/{digest}/{*path}",
                axum::routing::put(moneypenny::page_tools::upload)
                    .layer(axum::extract::DefaultBodyLimit::disable()),
            )
            .route(
                "/v1/station/assistant/pages/{digest}/done",
                post(moneypenny::page_tools::done),
            )
            .route("/v1/user/pages", get(moneypenny::page_tools::list))
            .route(
                "/v1/user/pages/{id}",
                axum::routing::delete(moneypenny::page_tools::delete),
            )
            .route(
                "/v1/user/pages/{id}/open",
                post(moneypenny::page_tools::open),
            )
            .route(
                "/v1/user/pages/{id}/remember",
                post(moneypenny::page_tools::remember),
            )
            .route(
                "/v1/user/pages/{id}/shares",
                axum::routing::put(moneypenny::page_tools::share),
            )
            .route(
                "/v1/user/pages/{id}/public",
                axum::routing::put(moneypenny::page_tools::public),
            );
        let router = Router::new()
            .route("/health", get(health))
            .route("/v1/board", get(board))
            .route("/v1/stations", get(stations))
            .route(
                "/v1/user/stations/{station_id}/settings",
                get(get_user_station_settings).put(put_user_station_settings),
            )
            .route("/v1/missions", get(missions))
            .route(
                "/v1/missions/{id}/products",
                get(compartments::mission_products),
            )
            .route("/v1/missions/{id}/reports", get(reports::of_mission))
            .route("/v1/reports", get(reports::list).post(reports::file))
            .route("/v1/reports/clearances", get(reports::clearances))
            .route(
                "/v1/reports/clearances/{subject}",
                axum::routing::put(reports::grant).delete(reports::revoke),
            )
            .route("/v1/reports/{id}", get(reports::show))
            .route(
                "/v1/reports/{id}/markings",
                axum::routing::put(reports::mark),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions",
                post(create_user_mission),
            )
            .route(
                "/v1/user/operations/{operation_id}/mission-proposals",
                get(list_user_mission_proposals),
            )
            .route(
                "/v1/user/operations/{operation_id}/placement-preview",
                get(placement_preview),
            )
            .route(
                "/v1/user/operations/{operation_id}/mission-proposals/{proposal_id}/approve",
                post(approve_user_mission_proposal),
            )
            .route(
                "/v1/user/operations/{operation_id}/mission-proposals/{proposal_id}/reject",
                post(reject_user_mission_proposal),
            )
            .route(
                "/v1/user/operations/{operation_id}/source-item",
                get(source_reads::item),
            )
            .route(
                "/v1/user/operations/{operation_id}/source-items",
                get(source_reads::items),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/issues",
                get(issues::list).post(issues::link),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/issues/{reference}",
                axum::routing::delete(issues::unlink),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/issues/{reference}/accept",
                post(issues::accept),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/issues/{reference}/comments",
                post(issues::comment),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/issues/{reference}/update",
                post(issues::update),
            )
            .route(
                "/v1/user/operations/{operation_id}/issues",
                post(issues::create),
            )
            .route(
                "/v1/user/operations/{operation_id}/issue-proposals",
                get(issues::proposals),
            )
            .route(
                "/v1/user/operations/{operation_id}/issue-proposals/{id}/approve",
                post(issues::approve),
            )
            .route(
                "/v1/user/operations/{operation_id}/issue-proposals/{id}/reject",
                post(issues::reject),
            )
            .route(
                "/v1/user/operations/{operation_id}/held-briefs",
                get(held::list),
            )
            .route(
                "/v1/user/operations/{operation_id}/held-briefs/{key}/release",
                post(held::release),
            )
            .route(
                "/v1/user/operations/{operation_id}/held-briefs/{key}/refuse",
                post(held::refuse),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/reviews",
                get(reviews::list),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/reviews/{gate}/overrule",
                post(reviews::overrule),
            )
            .route(
                "/v1/user/operations/{operation_id}/agent-roles",
                get(list_user_agent_roles),
            )
            .route(
                "/v1/user/operations/{operation_id}/agent-roles/{role_id}",
                axum::routing::put(put_user_agent_role).delete(delete_user_agent_role),
            )
            .route(
                "/v1/user/operations/{operation_id}/agents",
                get(agents::list),
            )
            .route(
                "/v1/user/operations/{operation_id}/agents/scorecard",
                get(agents::scorecard),
            )
            .route(
                "/v1/user/operations/{operation_id}/agents/{agent}/hold",
                axum::routing::delete(agents::lift_hold),
            )
            .route(
                "/v1/user/operations/{operation_id}/agents/{agent}/clearance",
                axum::routing::put(agents::lower).delete(agents::restore),
            )
            .route(
                "/v1/user/operations/{operation_id}/quarantine",
                get(list_user_quarantines),
            )
            .route(
                "/v1/user/operations/{operation_id}/quarantine/{agent_id}",
                post(quarantine_user_agent),
            )
            .route(
                "/v1/user/operations/{operation_id}/quarantine/{agent_id}/release",
                post(release_user_quarantine),
            )
            .route(
                "/v1/operations/{operation_id}/missions/{mission_id}",
                get(mission_detail),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/actions",
                post(controls::request_control),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/withdraw",
                post(held::withdraw),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/verdict",
                post(verdicts::give),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/verify",
                post(verdicts::verify),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/probe",
                post(verdicts::probe),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/rename",
                post(rename_user_mission),
            )
            .route(
                "/v1/user/operations/{operation_id}/missions/{mission_id}/actions/{control_id}",
                get(controls::control_status),
            )
            .route(
                "/v1/user/operations/{operation_id}/timeline",
                get(timeline::operation_timeline),
            )
            .route(
                "/v1/user/operations/{operation_id}/pools",
                get(pools::operation_pools),
            )
            .route(
                "/v1/user/operations/{operation_id}/pools/{pool}/{account}/clear-limit",
                post(pools::clear_limit),
            )
            .route("/v1/user/access", get(access_admin::read_global_policy))
            .route(
                "/v1/user/access/roles/{role}",
                axum::routing::put(access_admin::put_role).delete(access_admin::delete_role),
            )
            .route(
                "/v1/user/access/rules",
                axum::routing::put(access_admin::put_rules),
            )
            .route(
                "/v1/user/access/delegable",
                axum::routing::put(access_admin::put_delegable),
            )
            .route(
                "/v1/user/access/chief",
                axum::routing::put(access_admin::put_chief),
            )
            .route("/v1/user/access/check", get(access_admin::check))
            .route(
                "/v1/user/operations/{operation_id}/access",
                get(access_admin::read_operation_policy),
            )
            .route(
                "/v1/user/operations/{operation_id}/access/roles/{role}",
                axum::routing::put(access_admin::put_operation_role)
                    .delete(access_admin::delete_operation_role),
            )
            .route(
                "/v1/user/operations/{operation_id}/access/rules",
                axum::routing::put(access_admin::put_operation_rules),
            )
            .route("/v1/user/compartments", get(compartments::list))
            .route("/v1/user/compartment-form", get(compartments::form))
            .route(
                "/v1/user/compartments/{id}",
                get(compartments::show)
                    .put(compartments::put)
                    .delete(compartments::remove),
            )
            .route(
                "/v1/user/compartments/{id}/covers/{resource}",
                axum::routing::put(compartments::put_cover).delete(compartments::remove_cover),
            )
            .route(
                "/v1/user/compartments/{id}/read-in/{subject}",
                axum::routing::put(compartments::put_read_in)
                    .delete(compartments::remove_read_in),
            )
            .route("/v1/user/accounts", get(accounts::list))
            .route("/v1/user/models", get(models::list))
            .route(
                "/v1/user/accounts/{name}",
                axum::routing::put(accounts::put_account),
            )
            .route(
                "/v1/user/accounts/{name}/secret",
                post(accounts::reset_secret),
            )
            .route("/v1/user/roster/{kind}", get(roster::records))
            .route(
                "/v1/user/roster/operation/{id}/effective",
                get(roster::effective),
            )
            .route(
                "/v1/user/roster/{kind}/schema",
                get(roster::form)
                    .put(roster::put_record_keyed_schema)
                    .delete(roster::remove_record_keyed_schema),
            )
            .route(
                "/v1/user/roster/{kind}/{key}",
                axum::routing::put(roster::put_record).delete(roster::remove_record),
            )
            .route(
                "/v1/user/operations/{operation_id}/green-ink",
                get(green_ink::pending),
            )
            .route(
                "/v1/user/operations/{operation_id}/green-ink/expired",
                get(green_ink::expired),
            )
            .route(
                "/v1/user/operations/{operation_id}/green-ink/sources",
                get(green_ink::sources),
            )
            .route(
                "/v1/user/operations/{operation_id}/green-ink/{request_id}/rearm",
                post(green_ink::rearm),
            )
            .route(
                "/v1/user/operations/{operation_id}/green-ink/{request_id}/approve",
                post(green_ink::approve),
            )
            .route(
                "/v1/user/operations/{operation_id}/green-ink/{request_id}/reject",
                post(green_ink::reject),
            )
            .route(
                "/v1/user/operations/{operation_id}/green-ink/{request_id}/revoke",
                post(green_ink::revoke),
            )
            .route(
                "/v1/user/operations/{operation_id}/class-authorizations",
                get(green_ink::list_classes).post(green_ink::create_class),
            )
            .route(
                "/v1/user/operations/{operation_id}/class-authorizations/{id}",
                axum::routing::delete(green_ink::revoke_class),
            )
            .route("/v1/user/attach-grants", post(issue_user_attach_token))
            .route("/v1/user/moneypenny", get(moneypenny::socket))
            .route(
                "/v1/user/moneypenny/images/{sha256}",
                get(moneypenny::user_image),
            )
            .route(
                "/v1/station/assistant/images/{sha256}",
                get(moneypenny::station_image),
            )
            .route("/v1/moneypenny/mcp", post(moneypenny::mcp::endpoint))
            .route("/v1/user/memories", get(memories::list).post(memories::add))
            .route("/v1/user/memories/proposals", get(memories::proposals))
            .route(
                "/v1/user/memories/proposals/{id}",
                post(memories::decide),
            )
            .route(
                "/v1/user/memories/{id}",
                get(memories::read)
                    .put(memories::change)
                    .delete(memories::forget),
            )
            .route("/v1/user/memories/{id}/versions", get(memories::versions))
            .route(
                "/v1/user/assistant/memory",
                get(moneypenny::memory::list).post(moneypenny::memory::add),
            )
            .route(
                "/v1/user/assistant/memory/{id}",
                get(moneypenny::memory::read)
                    .put(moneypenny::memory::change)
                    .delete(moneypenny::memory::forget),
            )
            .route(
                "/v1/user/assistant/memory/{id}/versions",
                get(moneypenny::memory::versions),
            )
            .route("/v1/station/assistant/next", get(moneypenny::next))
            .route("/v1/station/assistant/reports", post(moneypenny::reports))
            .route(
                "/v1/station/assistant/cables",
                get(moneypenny::cable_standing),
            )
            .route("/v1/auth/methods", get(auth_methods))
            .route("/v1/auth/login", post(login))
            .route("/v1/auth/logout", post(logout))
            .route("/v1/auth/renew", post(renew))
            .route("/v1/auth/resume", post(resume))
            .route("/v1/auth/session", get(access_session))
            .route("/v1/user/profile", get(user_profile))
            .route("/v1/user/audit", get(user_audit))
            .route(
                "/v1/user/operations/{operation_id}/sessions/{session_id}/context",
                get(transcripts::mission_context),
            )
            .route(
                "/v1/user/assistant/conversations",
                get(transcripts::conversations),
            )
            .route(
                "/v1/user/assistant/conversations/{id}/log",
                get(transcripts::conversation_log),
            )
            .route(
                "/v1/user/assistant/conversations/{id}/context",
                get(transcripts::conversation_context),
            )
            .route("/v1/station/source-reads/next", get(source_reads::next))
            .route(
                "/v1/station/source-reads/{id}/result",
                post(source_reads::result),
            )
            .route("/v1/station/sessions", post(register_session))
            .route(
                "/v1/station/sessions/{session_id}/lifecycle/{sequence}",
                post(publish_lifecycle),
            )
            .route(
                "/v1/station/sessions/{session_id}/terminal/{sequence}",
                post(publish_terminal_output),
            )
            .route(
                "/v1/station/sessions/{session_id}/events/{sequence}",
                post(ingest_event),
            )
            .route("/v1/station/attach-grants", post(issue_attach_token))
            .route("/v1/station/access-tokens", post(issue_access_token))
            .route(
                "/v1/station/access-tokens/{token_id}",
                axum::routing::delete(revoke_access_token),
            )
            .route("/v1/station/inputs/next", get(next_agent_input))
            .route(
                "/v1/station/controls/next",
                get(controls::next_station_control),
            )
            .route(
                "/v1/station/controls/{control_id}/result",
                post(controls::report_station_control),
            )
            .route("/v1/attach", post(exchange_attach_token))
            .route("/v1/socket/{session_id}", get(websocket_attach))
            .merge(liaison::routes())
            .layer(axum::extract::DefaultBodyLimit::max(max_frame_bytes))
            .merge(base)
            .with_state(self.clone());
        match pages {
            Some(pages) => router.merge(pages.with_state(self)),
            None => router,
        }
    }

    /// Serves the gateway on loopback so browsers reach it only through the
    /// configured HTTPS reverse proxy. Once `shutdown` resolves and the open requests end, it
    /// waits up to `[assistant.gateway] log_flush_timeout` for Moneypenny's conversation log.
    pub async fn serve_loopback<F>(self, port: u16, shutdown: F) -> Result<(), std::io::Error>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let assistant = self.assistant.clone();
        let served = axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await;
        if let Some(assistant) = assistant {
            assistant.flush_log().await;
        }
        served
    }
}

async fn board(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<Vec<BoardSession>>, ApiError> {
    Ok(Json(authorized_board_sessions(&gateway, &headers).await?))
}

async fn stations(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<StationBoard>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let authorizer = gateway.authorizer()?;
    let (station_records, headquarters, dark) = tokio::task::spawn_blocking(move || {
        let stations = registry
            .stations(Duration::MAX)
            .map_err(|_| ApiError::internal())?;
        let headquarters = registry.headquarters().map_err(|_| ApiError::internal())?;
        let dark = registry.limits().stations.gone_dark_after.0;
        Ok::<_, ApiError>((stations, headquarters, dark))
    })
    .await
    .map_err(|_| ApiError::internal())??;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ApiError::internal())?
        .as_secs_f64();
    let tree = OperationTree::from_check_ins(&station_records);
    let visible = tokio::task::spawn_blocking(move || {
        let mut stations = Vec::new();
        let mut readable = BTreeSet::new();
        for station in station_records {
            let reads_station = authorizer
                .authorize(AuthorizationRequest {
                    person: person.clone(),
                    action: "station.read".to_owned(),
                    resource: format!("station:{}", station.station()),
                    ancestors: Vec::new(),
                })
                .map_err(|_| ApiError::internal())?
                .allowed;
            let operation_values = station
                .info()
                .get("operations")
                .and_then(serde_json::Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            let mut operations = Vec::new();
            for operation_value in operation_values {
                let Some(operation_id) = operation_value.as_str() else {
                    continue;
                };
                let Ok(operation_id) = OperationId::new(operation_id.to_owned()) else {
                    continue;
                };
                let allowed = authorizer
                    .authorize(operation_request(
                        person.clone(),
                        "mission.read",
                        operation_id.as_str(),
                        &tree,
                    ))
                    .map_err(|_| ApiError::internal())?
                    .allowed;
                if allowed {
                    operations.push(operation_id.to_string());
                }
            }
            // A control Station holds no checkout, so an Operation alone would never show it.
            if operations.is_empty() && !reads_station {
                continue;
            }
            operations.sort();
            operations.dedup();
            let mut board = board_station(&station, &operations, now, headquarters.as_ref(), dark);
            // The report covers every Operation on the Station: it is the Station's to read.
            if !reads_station {
                board.head_of_station = None;
            }
            stations.push(board);
        }
        stations.sort_by(|left: &BoardStation, right| left.station_id.cmp(&right.station_id));
        // The tree's Operations the person may read, each under its nearest readable ancestor.
        let named: BTreeSet<String> = stations
            .iter()
            .flat_map(|station| station.operations.iter().cloned())
            .chain(tree.operations().map(str::to_owned))
            .collect();
        for operation in named {
            if authorizer
                .authorize(operation_request(
                    person.clone(),
                    "mission.read",
                    &operation,
                    &tree,
                ))
                .map_err(|_| ApiError::internal())?
                .allowed
            {
                readable.insert(operation);
            }
        }
        let operations = readable
            .iter()
            .map(|operation| BoardOperation {
                operation_id: operation.clone(),
                parent: tree.visible_parent(operation, |id| readable.contains(id)),
            })
            .collect();
        let headquarters_station = headquarters
            .filter(|lease| {
                lease.expires() > now
                    && stations
                        .iter()
                        .any(|station| station.station_id == lease.station())
            })
            .map(|lease| lease.station().to_owned());
        Ok::<_, ApiError>(StationBoard {
            stations,
            headquarters_station,
            operations,
        })
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(visible))
}

fn board_station(
    record: &ai6::StationRecord,
    operations: &[String],
    now: f64,
    headquarters: Option<&ai6::HeadquartersLease>,
    gone_dark_after: Duration,
) -> BoardStation {
    let info = record.info();
    let string_list = |key: &str| {
        info.get(key)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let usize_value = |key: &str| {
        info.get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or_default()
    };
    BoardStation {
        station_id: record.station().to_owned(),
        healthy: now - record.seen() <= gone_dark_after.as_secs_f64(),
        headquarters: headquarters
            .is_some_and(|lease| lease.station() == record.station() && lease.expires() > now),
        seen: record.seen(),
        roles: string_list("roles"),
        handlers: string_list("handlers"),
        running: usize_value("running"),
        capacity: usize_value("capacity"),
        operations: operations.to_vec(),
        theaters: station_theaters(info),
        resources: info
            .get("resources")
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
        config: info
            .get("config")
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
        issues_weakened: info
            .get("issues_weakened")
            .and_then(serde_json::Value::as_array)
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter(|line| {
                        operations
                            .iter()
                            .any(|operation| line.starts_with(&format!("Operation {operation:?} ")))
                    })
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        head_of_station: info
            .get("head_of_station")
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
    }
}

/// The Theaters a Station published; one without the key reaches only its own host.
fn station_theaters(info: &BTreeMap<String, serde_json::Value>) -> Vec<BoardTheater> {
    let published = info
        .get("theaters")
        .and_then(|value| serde_json::from_value::<Vec<ai6::TheaterSummary>>(value.clone()).ok())
        .unwrap_or_else(|| vec![ai6::TheaterSummary::local()]);
    published
        .into_iter()
        .map(|theater| BoardTheater {
            id: theater.id,
            access: theater.access.as_str().to_owned(),
            host: theater.host,
            provider: theater.provider,
            labels: theater.labels,
            capabilities: theater.capabilities,
            agents: theater.agents.as_str().to_owned(),
            egress: theater.egress.as_str().to_owned(),
            harnesses: theater.harnesses,
            workdir: theater.workdir,
            unavailable: theater.unavailable,
            weakened: theater.weakened,
            requires: theater.requires,
            report: theater.report,
            made_for: theater.made_for,
            resources: theater.resources,
            sessions: theater.sessions,
        })
        .collect()
}

/// `GET /v1/missions` filters: one Operation, or with `subtree` that Operation and every
/// Operation under it.
#[derive(Debug, Default, Deserialize)]
struct MissionsQuery {
    operation: Option<String>,
    #[serde(default)]
    subtree: bool,
    /// What one compartment covers, of what the person may read.
    compartment: Option<String>,
    /// One Mission: its UID or any handle it had.
    mission: Option<String>,
}

async fn missions(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<MissionsQuery>,
) -> Result<Json<Vec<BoardMission>>, ApiError> {
    let mut sessions = authorized_board_sessions(&gateway, &headers).await?;
    if let Some(mission) = &query.mission {
        let uid = mission_key(&gateway, mission).await?;
        sessions.retain(|session| session.mission_id == uid);
    }
    if let Some(operation) = &query.operation {
        let tree = if query.subtree {
            operation_tree(&gateway).await?
        } else {
            OperationTree::default()
        };
        sessions.retain(|session| tree.in_subtree(&session.operation_id, operation));
    }
    if let Some(compartment) = query.compartment {
        let person = resolve_user(&gateway, bearer(&headers)?).await?;
        let access = gateway.probing_authorizer()?;
        sessions = tokio::task::spawn_blocking(move || {
            let compiled = access.compiled();
            let readable = access
                .authorize(AuthorizationRequest {
                    person: person.clone(),
                    action: "compartment.read".to_owned(),
                    resource: format!("compartment:{compartment}"),
                    ancestors: Vec::new(),
                })
                .is_ok_and(|decision| decision.allowed);
            let holds =
                compiled.is_some_and(|compiled| compiled.person_holds(&compartment, &person));
            match compiled.filter(|_| readable || holds) {
                Some(compiled) => sessions
                    .into_iter()
                    .filter(|session| {
                        compartments::in_compartment(
                            compiled,
                            &compartment,
                            &session.operation_id,
                            &session.mission_id,
                        )
                    })
                    .collect(),
                None => Vec::new(),
            }
        })
        .await
        .map_err(|_| ApiError::internal())?;
    }
    Ok(Json(
        gateway.enrich_missions(group_missions(sessions)).await,
    ))
}

async fn mission_detail(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
) -> Result<Json<BoardMission>, ApiError> {
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::not_found())?;
    let named = named_mission(&gateway, &mission_id).await?;
    let mission_id = MissionId::new(named.mission_id).map_err(|_| ApiError::not_found())?;
    let visible = authorized_board_sessions(&gateway, &headers).await?;
    let sessions = visible
        .into_iter()
        .filter(|session| {
            session.operation_id == operation_id.as_str()
                && session.mission_id == mission_id.as_str()
        })
        .collect::<Vec<_>>();
    let mut mission = group_missions(sessions)
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)?;
    mission.handle = named.handle;
    mission.brief = gateway
        .inner
        .lock()
        .await
        .missions
        .get(&(operation_id, mission_id))
        .cloned();
    if let Some(registry) = gateway.registry.clone() {
        let (operation, mission_id) = (mission.operation_id.clone(), mission.mission_id.clone());
        let (usage, mut attempts) = tokio::task::spawn_blocking(move || {
            let mut attempts = attempts_by_mission(registry.sessions()?);
            for attempt in attempts.get_mut(&mission_id).into_iter().flatten() {
                attempt.final_report = registry
                    .final_report(&attempt.session_id)?
                    .map(|report| report.text);
            }
            Ok::<_, std::io::Error>((
                registry.mission_usage(Some(&operation), &mission_id)?,
                attempts,
            ))
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
        mission.usage = usage;
        mission.attempts = attempts.remove(&mission.mission_id).unwrap_or_default();
    }
    Ok(Json(mission))
}

/// Whether a title and instructions are written, and within `[text]`.
fn mission_text_fits(title: &str, prompt: &str, limits: &ai6::TextLimits) -> bool {
    !title.trim().is_empty()
        && limits.mission_title_chars.allows(title.chars().count())
        && !prompt.trim().is_empty()
        && limits.mission_prompt_bytes.allows(prompt.len())
}

/// How many audit records a request gets: what it asks for, within `[timeline]`.
fn audit_page(asked: Option<usize>, pages: &ai6::UnexTimelineLimits) -> Option<usize> {
    let limit = asked.unwrap_or(usize::try_from(pages.audit_page).unwrap_or(usize::MAX));
    (limit != 0 && pages.max_audit_page.allows(limit)).then_some(limit)
}

async fn create_user_mission(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    Json(request): Json<CreateUserMissionRequest>,
) -> Result<(StatusCode, Json<CreateUserMissionResponse>), ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    if !mission_text_fits(&request.title, &request.prompt, &gateway.config.unex().text)
        || request
            .agent_id
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 256)
        || request
            .role
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 128)
        // A Head of Station round comes only from an authorized Sleeper.
        || request
            .role
            .as_deref()
            .is_some_and(ai6::is_head_of_station_role)
        || request.tags.len() > 32
        || request
            .tags
            .iter()
            .any(|tag| tag.trim().is_empty() || tag.len() > 64)
        || !valid_station_list(&request.stations)
    {
        return Err(ApiError::bad_request());
    }
    let authorizer = gateway.authorizer()?;
    let actor_id = person.id.clone();
    let check = operation_request(
        person.clone(),
        "mission.create",
        operation_id.as_str(),
        &operation_tree(&gateway).await?,
    );
    let decision = tokio::task::spawn_blocking(move || authorizer.authorize(check))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    if !decision.allowed {
        return Err(ApiError::forbidden());
    }
    // A person's naming may weaken a review, so it takes the action that decides over reviews.
    if let Some(review) = &request.review {
        if review
            .named()
            .chain(
                review
                    .plan
                    .iter()
                    .chain(review.output.iter())
                    .map(String::as_str),
            )
            .any(|name| name.trim().is_empty() || name.len() > 128)
        {
            return Err(ApiError::bad_request());
        }
        authorize_operation_action(&gateway, person.clone(), &operation_id, "mission.review")
            .await?;
    }
    if request.deliverables.len() > 32 || request.deliverables.iter().any(|text| text.len() > 512) {
        return Err(ApiError::bad_request());
    }
    if let Err(invalid) = ai6::directorates::m::deliverables::parse_all(&request.deliverables) {
        return Err(ApiError::invalid(invalid.to_string()));
    }
    let reference = |text: &str| {
        !text.trim().is_empty() && text.len() <= 256 && !text.chars().any(char::is_control)
    };
    if (request.issue.is_some() && request.create_issue.is_some())
        || request
            .issue
            .as_ref()
            .is_some_and(|issue| !reference(&issue.tracker) || !reference(&issue.item))
    {
        return Err(ApiError::bad_request());
    }

    let created = utc_timestamp()?;
    let received_at = unix_timestamp()? as i64;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    if !request.stations.is_empty() {
        let (lookup, named, operation) = (
            registry.clone(),
            request.stations.clone(),
            operation_id.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let records = lookup
                .stations(Duration::MAX)
                .map_err(|_| ApiError::internal())?;
            let headquarters = lookup.headquarters().map_err(|_| ApiError::internal())?;
            let policy = published_policy(
                &records,
                headquarters.as_ref().map(|lease| lease.station()),
                &operation,
            )
            .map(|(_, policy)| policy);
            match station_pin_refusal(&named, &records, policy.as_ref(), &operation) {
                Some(refusal) => Err(ApiError::invalid(refusal)),
                None => Ok(()),
            }
        })
        .await
        .map_err(|_| ApiError::internal())??;
    }
    let mut data = request
        .review
        .as_ref()
        .map(|review| {
            BTreeMap::from([(
                ai6::directorates::m::review::BRIEF_REVIEW_KEY.to_owned(),
                serde_json::to_value(review).unwrap_or_default(),
            )])
        })
        .unwrap_or_default();
    if !request.deliverables.is_empty() {
        data.insert(
            ai6::directorates::m::deliverables::BRIEF_DELIVERABLES_KEY.to_owned(),
            serde_json::json!(request.deliverables),
        );
    }
    if let Some(issue) = &request.issue {
        data.insert(
            "follow_up".to_owned(),
            serde_json::json!({ "tracker": issue.tracker, "item": issue.item }),
        );
    }
    let mut brief = Brief {
        title: request.title,
        body: request.prompt,
        id: ai6::new_uid(),
        mission_id: None,
        handle: None,
        name: None,
        source: "universal-exports".to_owned(),
        operation: Some(operation_id.to_string()),
        agent: request.agent_id,
        role: request.role,
        tags: request.tags,
        stations: request.stations,
        priority: 0,
        url: None,
        data,
        created,
        version: 1,
        provenance: None,
    };
    if let Some(create) = request.create_issue {
        return create_with_issue(&gateway, registry, (&person, &operation_id), create, brief)
            .await;
    }
    let book = registry.clone();
    let entry = match &request.issue {
        Some(issue) => MissionEntry::follow_up(operation_id.as_str(), &issue.tracker, &issue.item),
        None => MissionEntry::person(Some(operation_id.as_str())),
    };
    let entered = tokio::task::spawn_blocking(move || book.allot_mission_id(&entry))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|error| match (&request.issue, error.kind()) {
            (Some(issue), std::io::ErrorKind::NotFound) => ApiError::invalid(format!(
                "item {:?} of {:?} has no Mission in Operation {operation_id}: only an item AI6 \
                 has taken in can be followed up",
                issue.item, issue.tracker
            )),
            _ => ApiError::internal(),
        })?;
    brief.id = entered.mission_id.clone();
    brief.mission_id = Some(entered.mission_id);
    brief.handle = Some(entered.handle.clone());
    brief.provenance = Some(
        Provenance::new(Origin::Operator, &brief, GATEWAY_RECEIVER, received_at)
            .with_principal(actor_id.clone()),
    );
    let mission_id = MissionId::new(brief.key()).map_err(|_| ApiError::internal())?;
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: actor_id,
        action: "mission.create.requested".to_owned(),
        operation: operation_id.to_string(),
        resource: None,
        target: mission_id.to_string(),
        at: unix_timestamp()?,
    };
    let audit_registry = registry.clone();
    tokio::task::spawn_blocking(move || audit_registry.record_audit(&audit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    let followed = request.issue.map(|issue| IssueLink {
        operation: operation_id.to_string(),
        mission_id: mission_id.to_string(),
        tracker: issue.tracker,
        item: issue.item,
        url: None,
        relation: LinkRelation::Related,
        state: LinkState::Active,
        linked_by: format!("person:{}", person.id),
        linked_at: received_at as f64,
        status_comment: None,
        phase: None,
        snapshot: None,
    });
    tokio::task::spawn_blocking(move || {
        registry.submit_brief(&brief)?;
        if let Some(link) = followed {
            registry.link_issue(&link)?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateUserMissionResponse {
            operation_id: operation_id.to_string(),
            mission_id: Some(mission_id.to_string()),
            handle: Some(entered.handle),
            proposal: None,
            state: "queued",
        }),
    ))
}

/// The rest of a Mission create that asks for its issue: the issue goes to the Headquarters
/// Station as an approved creation carrying the brief, and the Mission enters, numbered by the
/// issue, once it exists. Needs `issue.create` as well.
async fn create_with_issue(
    gateway: &Gateway,
    registry: Arc<dyn RegistryBackend>,
    (person, operation_id): (&Person, &OperationId),
    create: NewMissionIssue,
    mut brief: Brief,
) -> Result<(StatusCode, Json<CreateUserMissionResponse>), ApiError> {
    if create.tracker.trim().is_empty() || create.tracker.len() > 256 {
        return Err(ApiError::bad_request());
    }
    authorize_operation_action(gateway, person.clone(), operation_id, "issue.create").await?;
    let received_at = unix_timestamp()?;
    brief.provenance = Some(
        Provenance::new(
            Origin::Operator,
            &brief,
            GATEWAY_RECEIVER,
            received_at as i64,
        )
        .with_principal(person.id.clone()),
    );
    let title = create.title.unwrap_or_else(|| brief.title.clone());
    let body = create.body.unwrap_or_else(|| brief.body.clone());
    let proposal = issues::create_for_mission(
        gateway,
        &registry,
        (person, operation_id),
        (&create.tracker, &title, &body),
        brief,
    )
    .await?;
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: "mission.create.requested".to_owned(),
        operation: operation_id.to_string(),
        resource: None,
        target: format!("issue-proposal {proposal}"),
        at: received_at,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&audit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateUserMissionResponse {
            operation_id: operation_id.to_string(),
            mission_id: None,
            handle: None,
            proposal: Some(proposal),
            state: "awaiting_issue",
        }),
    ))
}

fn valid_station_list(stations: &[String]) -> bool {
    stations.len() <= 64
        && stations.iter().all(|station| {
            !station.is_empty() && station.len() <= 256 && !station.chars().any(char::is_control)
        })
}

/// Why a Mission may not be pinned to `named`, if it may not: one names no Station that ever
/// checked in, or one the Operation's own `stations` leaves out. Pins match Station IDs exactly,
/// so an unknown one is told the IDs it equals ignoring case, and those of Stations on a host of
/// that name.
fn station_pin_refusal(
    named: &[String],
    records: &[ai6::StationRecord],
    policy: Option<&ai6::OperationPolicy>,
    operation: &str,
) -> Option<String> {
    for station in named {
        if !records.iter().any(|record| record.station() == station) {
            let near: Vec<&str> = records
                .iter()
                .filter(|record| {
                    record.station().eq_ignore_ascii_case(station)
                        || record
                            .info()
                            .get("host")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|host| host.eq_ignore_ascii_case(station))
                })
                .map(ai6::StationRecord::station)
                .collect();
            let hint = if near.is_empty() {
                String::new()
            } else {
                format!("; Stations it may mean: {}", near.join(", "))
            };
            return Some(format!(
                "no Station {station:?} has checked in (a pin is an exact Station ID){hint}"
            ));
        }
        if let Some(policy) = policy
            && !policy.stations.is_empty()
            && !policy.stations.contains(station)
        {
            return Some(format!(
                "Operation {operation:?} does not run on Station {station:?}: its stations are [{}]",
                policy.stations.join(", ")
            ));
        }
    }
    None
}

/// The Operation's placement policy as a Station published it at check-in: the Headquarters
/// Station's copy when it publishes one, else the first Station's that does.
fn published_policy<'a>(
    records: &'a [ai6::StationRecord],
    headquarters: Option<&str>,
    operation: &str,
) -> Option<(&'a ai6::StationRecord, ai6::OperationPolicy)> {
    let published = |record: &ai6::StationRecord| {
        record
            .info()
            .get("operation_policies")
            .and_then(|policies| policies.get(operation))
            .and_then(|policy| serde_json::from_value::<ai6::OperationPolicy>(policy.clone()).ok())
    };
    headquarters
        .and_then(|station| records.iter().find(|record| record.station() == station))
        .and_then(|record| published(record).map(|policy| (record, policy)))
        .or_else(|| {
            records
                .iter()
                .find_map(|record| published(record).map(|policy| (record, policy)))
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlacementPreviewQuery {
    agent: Option<String>,
    role: Option<String>,
    /// Comma-separated Station IDs the Mission would narrow placement to.
    stations: Option<String>,
    /// Comma-separated tags the Agent must carry.
    tags: Option<String>,
}

/// Where a Mission for the Operation would run, and why each other place would not.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlacementPreview {
    /// The Operation previewed.
    pub operation_id: String,
    /// The Station whose copy of the Operation's policy was used: the Headquarters Station
    /// when it publishes one, otherwise the first Station that does.
    pub policy_station: String,
    /// Eligible (Station, Theater, Agent) choices, best first.
    pub choices: Vec<ai6::PlacementChoice>,
    /// Everything ruled out, each with its reason.
    pub rejections: Vec<ai6::PlacementRejection>,
}

fn comma_list(value: Option<String>) -> Vec<String> {
    value
        .iter()
        .flat_map(|list| list.split(','))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

async fn placement_preview(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    Query(query): Query<PlacementPreviewQuery>,
) -> Result<Json<PlacementPreview>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person, &operation_id, "mission.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let mut request = ai6::PlacementRequest::default();
    request.stations = comma_list(query.stations);
    request.agent = query.agent.filter(|agent| !agent.trim().is_empty());
    request.role = query.role.filter(|role| !role.trim().is_empty());
    request.tags = comma_list(query.tags);
    if !valid_station_list(&request.stations) || request.tags.len() > 32 {
        return Err(ApiError::bad_request());
    }
    let operation = operation_id.to_string();
    let preview = tokio::task::spawn_blocking(move || {
        let records = registry
            .stations(Duration::MAX)
            .map_err(|_| ApiError::internal())?;
        let headquarters = registry.headquarters().map_err(|_| ApiError::internal())?;
        let leases = registry.leases().map_err(|_| ApiError::internal())?;
        let (source, policy) = published_policy(
            &records,
            headquarters.as_ref().map(|lease| lease.station()),
            &operation,
        )
        .ok_or_else(|| ApiError::from(GatewayError::NotFound))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ApiError::internal())?
            .as_secs_f64();
        let stations = records
            .iter()
            .map(|record| {
                let load = leases
                    .iter()
                    .filter(|lease| lease.station() == record.station())
                    .count();
                ai6::StationFacts::from_record(
                    record,
                    now,
                    registry.limits().stations.gone_dark_after.0,
                    load,
                )
            })
            .collect::<Vec<_>>();
        let placement = ai6::place(&policy, &request, &stations);
        Ok::<_, ApiError>(PlacementPreview {
            operation_id: operation,
            policy_station: source.station().to_owned(),
            choices: placement.choices,
            rejections: placement.rejections,
        })
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(preview))
}

async fn list_user_mission_proposals(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<Vec<MissionProposal>>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person, &operation_id, "mission.create").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation_id).await?;
    let operation = operation_id.to_string();
    let proposals =
        tokio::task::spawn_blocking(move || registry.list_mission_proposals(&operation, 100))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|_| ApiError::internal())?;
    Ok(Json(proposals))
}

async fn approve_user_mission_proposal(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, proposal_id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<MissionProposalDecisionResponse>), ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    validate_proposal_path_id(&proposal_id)?;
    authorize_operation_action(&gateway, person.clone(), &operation_id, "mission.create").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation_id).await?;
    let operation = operation_id.to_string();
    let requested_id = proposal_id.clone();
    let approver = person.id.clone();
    let now = unix_timestamp()?;
    // The operator saw the proposal's vetting: only a malformed brief is stopped again. The
    // brief is promoted and cleared in one step, so no intake round sees it uncleared.
    let cleared = ai6::FindingKind::ALL
        .into_iter()
        .filter(|kind| *kind != ai6::FindingKind::Malformed)
        .collect::<Vec<_>>();
    let brief = tokio::task::spawn_blocking(move || {
        registry.promote_cleared(
            &operation,
            &requested_id,
            GATEWAY_RECEIVER,
            &cleared,
            &approver,
            now,
        )
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(proposal_registry_error)?;
    let mission_id = MissionId::new(brief.key()).map_err(|_| ApiError::internal())?;
    record_proposal_audit(
        &gateway,
        person,
        &operation_id,
        &proposal_id,
        "mission.proposal.approved",
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(MissionProposalDecisionResponse {
            operation_id: operation_id.to_string(),
            proposal_id,
            mission_id: Some(mission_id.to_string()),
            handle: Some(brief.shown_handle()),
            state: "queued",
        }),
    ))
}

async fn reject_user_mission_proposal(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, proposal_id)): Path<(String, String)>,
) -> Result<Json<MissionProposalDecisionResponse>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    validate_proposal_path_id(&proposal_id)?;
    authorize_operation_action(&gateway, person.clone(), &operation_id, "mission.create").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation_id).await?;
    let operation = operation_id.to_string();
    let requested_id = proposal_id.clone();
    tokio::task::spawn_blocking(move || {
        registry.reject_mission_proposal(&operation, &requested_id)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(proposal_registry_error)?;
    record_proposal_audit(
        &gateway,
        person,
        &operation_id,
        &proposal_id,
        "mission.proposal.rejected",
    )
    .await?;
    Ok(Json(MissionProposalDecisionResponse {
        operation_id: operation_id.to_string(),
        proposal_id,
        mission_id: None,
        handle: None,
        state: "rejected",
    }))
}

fn validate_proposal_path_id(proposal_id: &str) -> Result<(), ApiError> {
    if proposal_id.is_empty()
        || proposal_id.len() > 128
        || proposal_id
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(ApiError::bad_request());
    }
    Ok(())
}

fn proposal_registry_error(error: std::io::Error) -> ApiError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ApiError::not_found(),
        std::io::ErrorKind::AlreadyExists => ApiError::from(GatewayError::Conflict),
        std::io::ErrorKind::InvalidInput => ApiError::bad_request(),
        _ => ApiError::internal(),
    }
}

async fn record_proposal_audit(
    gateway: &Gateway,
    person: Person,
    operation_id: &OperationId,
    proposal_id: &str,
    action: &'static str,
) -> Result<(), ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let entry = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id,
        action: action.to_owned(),
        operation: operation_id.to_string(),
        resource: None,
        target: proposal_id.to_owned(),
        at: unix_timestamp()?,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&entry))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(())
}

async fn authorized_board_sessions(
    gateway: &Gateway,
    headers: &HeaderMap,
) -> Result<Vec<BoardSession>, ApiError> {
    let person = resolve_user(gateway, bearer(headers)?).await?;
    let authorizer = gateway.authorizer()?;
    let sessions = gateway.board_sessions().await;
    let registry = gateway.registry.clone();
    let tree = operation_tree(gateway).await?;
    let visible = tokio::task::spawn_blocking(move || {
        // Exposure and the holding Station live in the shared Session records, keyed by the
        // provider's Session ID.
        let records: BTreeMap<String, (String, Vec<String>)> = registry
            .and_then(|registry| registry.sessions().ok())
            .into_iter()
            .flatten()
            .filter_map(|record| {
                let sources = record.exposure.into_iter().map(|e| e.source).collect();
                Some((record.provider_session_id?, (record.station, sources)))
            })
            .collect();
        let mut visible = Vec::new();
        for mut session in sessions {
            if let Some((station, sources)) = records.get(&session.session_id) {
                session.station_id = Some(station.clone());
                session.exposure.clone_from(sources);
            }
            let decision = authorizer
                .authorize(operation_request(
                    person.clone(),
                    "mission.read",
                    &session.operation_id,
                    &tree,
                ))
                .map_err(|_| ())?;
            // A compartment may cover this Mission alone, not its Operation.
            if decision.allowed
                || authorizer.allows_mission(
                    &person,
                    ai6::CompartmentAction::MissionRead,
                    &session.mission_id,
                    &session.operation_id,
                )
            {
                visible.push(session);
            }
        }
        Ok::<_, ()>(visible)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    Ok(visible)
}

fn group_missions(sessions: Vec<BoardSession>) -> Vec<BoardMission> {
    let mut missions = BTreeMap::<(String, String), Vec<BoardSession>>::new();
    for session in sessions {
        missions
            .entry((session.operation_id.clone(), session.mission_id.clone()))
            .or_default()
            .push(session);
    }
    missions
        .into_iter()
        .map(|((operation_id, mission_id), sessions)| BoardMission {
            operation_id,
            handle: mission_id.clone(),
            mission_id,
            sessions,
            brief: None,
            usage: None,
            attempts: Vec::new(),
        })
        .collect()
}

/// The Mission a route names by `id`: its UID, the key of a brief dropped by hand, or any
/// handle it was ever given, compared without case, with the handle it goes by now. An ID the
/// Registry does not know, a Mission started on a Station rather than from the queue, is taken
/// as it is.
pub(crate) async fn named_mission(gateway: &Gateway, id: &str) -> Result<MissionHandle, ApiError> {
    let Some(registry) = gateway.registry.clone() else {
        return Ok(MissionHandle::new(id, id));
    };
    let wanted = id.to_owned();
    let found = tokio::task::spawn_blocking(move || registry.resolve_mission(&wanted))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(found.unwrap_or_else(|| MissionHandle::new(id, id)))
}

/// The UID of the Mission a route names by `id`, as [`named_mission`] finds it.
pub(crate) async fn mission_key(gateway: &Gateway, id: &str) -> Result<String, ApiError> {
    Ok(named_mission(gateway, id).await?.mission_id)
}

/// `POST /v1/user/operations/{operation_id}/missions/{mission_id}/rename`: gives the Mission
/// the handle `{"handle": …}`, under `mission.control`. Its UID, and everything keyed by it,
/// stays; the old handle still names it. A refusal is `{"error": TEXT}`.
async fn rename_user_mission(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
    Json(request): Json<RenameMissionRequest>,
) -> Result<Json<RenamedMission>, memories::Refused> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation_id, "mission.control").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let named = named_mission(&gateway, &mission_id).await?;
    let (store, uid, operation, handle) = (
        registry.clone(),
        named.mission_id.clone(),
        operation_id.to_string(),
        request.handle,
    );
    let renamed = tokio::task::spawn_blocking(move || {
        let filed = store.filed_brief(&uid)?;
        if filed.is_none_or(|filed| filed.brief.operation.as_deref() != Some(operation.as_str())) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the Operation has no such Mission",
            ));
        }
        store.rename_mission(&uid, &handle)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|error| {
        let status = match error.kind() {
            std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            std::io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
            std::io::ErrorKind::AlreadyExists => StatusCode::CONFLICT,
            _ => return memories::Refused::from(ApiError::internal()),
        };
        memories::refused(status, error.to_string())
    })?;
    let at = unix_timestamp()?;
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: "mission.rename".to_owned(),
        operation: operation_id.to_string(),
        resource: Some(format!("mission:{}", renamed.mission_id)),
        target: renamed.mission_id.clone(),
        at,
    };
    let event = TimelineEvent::new("control", "rename.done", operation_id.to_string(), at)
        .mission(renamed.mission_id.clone())
        .actor(person.id)
        .detail("handle", renamed.handle.clone())
        .detail("previous", named.handle.clone());
    tokio::task::spawn_blocking(move || {
        registry.record_audit(&audit)?;
        registry.record_event(&event)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    Ok(Json(RenamedMission {
        operation_id: operation_id.to_string(),
        mission_id: renamed.mission_id,
        handle: renamed.handle,
        previous: named.handle,
    }))
}

async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct IssueAccessTokenRequest {
    person_id: String,
    ttl_seconds: u64,
}

#[derive(Deserialize)]
struct LoginRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    secret: String,
    #[serde(default)]
    ttl_seconds: Option<u64>,
    /// `false` asks a browser's sign-in not to get a session cookie.
    #[serde(default)]
    session_cookie: Option<bool>,
}

#[derive(Serialize)]
struct IssuedAccessToken {
    token_id: String,
    token: String,
    expires_at: f64,
    /// Seconds it has left, so a client schedules by this rather than by its own clock.
    expires_in: f64,
    /// When renewing stops ([`session_end`]).
    ends_at: Option<f64>,
}

/// What `POST /v1/auth/renew` answers: the token's times, and no secret.
#[derive(Serialize)]
struct RenewedAccessToken {
    token_id: String,
    expires_at: f64,
    /// Seconds it has left, so a client schedules by this rather than by its own clock.
    expires_in: f64,
    /// When renewing stops ([`session_end`]).
    ends_at: Option<f64>,
}

/// When renewing a token stops: `session_max` after it was issued, or its own expiry when that
/// is later (a lifetime asked for at sign-in, or renewal turned off). `None` with no bound.
fn session_end(issued_at: f64, expires_at: f64, session_max: TimeLimit) -> Option<f64> {
    session_max
        .get()
        .map(|max| (issued_at + max.as_secs_f64()).max(expires_at))
}

/// How long a token is issued for: `asked`, seconds within `access_token_max_ttl`, else the
/// gateway's `access_token_ttl`.
fn access_token_ttl(gateway: &Gateway, asked: Option<u64>) -> Result<Duration, ApiError> {
    let table = &gateway.config.unex().gateway;
    let Some(asked) = asked else {
        return Ok(table.access_token_ttl.0);
    };
    let asked = Duration::from_secs(asked);
    if !table.access_token_max_ttl.allows(asked) {
        return Err(ApiError::bad_request());
    }
    Ok(asked)
}

#[derive(Serialize)]
struct AccessSession {
    token_id: String,
    person_id: String,
    expires_at: f64,
}

#[derive(Serialize)]
struct UserProfile {
    person_id: String,
    name: String,
    display_name: Option<String>,
    groups: Vec<String>,
    expires_at: f64,
    operations: Vec<UserOperationAccess>,
    stations: Vec<UserStationAccess>,
    /// The actions the person holds on the roster (`config:roster`).
    roster: Vec<String>,
    /// The actions the person holds on the global access policy (`access.read`, `access.manage`),
    /// the accounts (`account.manage`), other people's memory (`assistant.memory.admin`) and the
    /// global memory (`memory.write`).
    global_actions: Vec<String>,
    /// Whether this gateway serves Moneypenny, whom Outer Office offers only when it does.
    moneypenny: bool,
}

#[derive(Serialize)]
struct UserOperationAccess {
    operation_id: String,
    /// The nearest Operation above it the person may read; omitted for a root or when every
    /// ancestor is hidden from them.
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    actions: Vec<String>,
    /// The compartments that give the person something here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    compartments: Vec<String>,
}

#[derive(Serialize)]
struct UserStationAccess {
    station_id: String,
    actions: Vec<String>,
}

#[derive(Serialize)]
struct StationSettingsResponse {
    station_id: String,
    max_concurrent_missions: usize,
    min_free_memory_mb: u64,
    max_cpu_percent: u32,
    configured: bool,
    execution_enabled: bool,
}

/// A settings change; an absent resource limit keeps its current value.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateStationSettings {
    max_concurrent_missions: usize,
    #[serde(default)]
    min_free_memory_mb: Option<u64>,
    #[serde(default)]
    max_cpu_percent: Option<u32>,
}

#[derive(Serialize)]
struct AgentRoleResponse {
    #[serde(flatten)]
    role: AgentRole,
    operation_override: bool,
}

#[derive(Serialize)]
struct RevokedAccessToken {
    revoked: bool,
}

async fn issue_access_token(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(request): Json<IssueAccessTokenRequest>,
) -> Result<(StatusCode, Json<IssuedAccessToken>), ApiError> {
    gateway
        .authenticate_station(bearer(&headers)?)
        .map_err(ApiError::from)?;
    let ttl = access_token_ttl(&gateway, Some(request.ttl_seconds))?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::internal)?;
    let session_max = gateway.config.unex().gateway.session_max;
    let issued = issue_access_token_record(registry, request.person_id, ttl, session_max).await?;
    Ok((StatusCode::CREATED, Json(issued)))
}

/// What `POST /v1/auth/login` accepts here, so a sign-in page asks for nothing it cannot use.
/// Says nothing about any account, so it needs no token.
#[derive(Serialize)]
struct AuthMethods {
    /// `password`: a name and a secret. `proxy`: an identity the reverse proxy asserts.
    methods: Vec<&'static str>,
}

async fn auth_methods(State(gateway): State<Gateway>) -> Result<Json<AuthMethods>, ApiError> {
    let mut methods = Vec::new();
    if let Some(providers) = &gateway.access {
        if let Some(authenticator) = providers.authenticator.clone() {
            let described = tokio::task::spawn_blocking(move || authenticator.describe())
                .await
                .map_err(|_| ApiError::internal())?;
            // An authenticator that says nothing, or cannot, is still asked for a name and a
            // secret by `login`.
            let takes = described.ok().and_then(|description| description.takes);
            if takes.as_deref() != Some(ai6::directorates::m::access::takes::REQUEST) {
                methods.push("password");
            }
        }
        if providers.proxy_identity.is_some() {
            methods.push("proxy");
        }
    }
    Ok(Json(AuthMethods { methods }))
}

async fn login(
    State(gateway): State<Gateway>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    // A browser always names the page's origin on a POST, and nothing else this gateway serves
    // does, so that is what tells a web sign-in from a desktop app's or an API client's.
    let from_browser = headers.contains_key(ORIGIN);
    if from_browser {
        require_origin(&gateway, &headers)?;
    }
    let ttl = access_token_ttl(&gateway, request.ttl_seconds)?;
    let wants_cookie = from_browser && request.session_cookie != Some(false);
    let providers = gateway.access.clone().ok_or_else(ApiError::unavailable)?;
    let person = match proxy_person(&gateway, &providers, &headers).await? {
        Some(person) => person,
        None => authenticate_password(&gateway, &providers, &headers, connection, request).await?,
    };
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let table = &gateway.config.unex().gateway;
    let session_max = table.session_max;
    let issued_at = unix_timestamp()?;
    let cookie = if wants_cookie && session_cookies_on(table) {
        Some(store_session_cookie(registry.clone(), &person.id, issued_at, session_max).await?)
    } else {
        None
    };
    let sign_in = cookie.as_ref().map(|cookie| cookie.sign_in.clone());
    let issued = store_bearer(
        registry,
        person.id,
        Times {
            issued_at,
            expires_at: issued_at + ttl.as_secs_f64(),
            now: issued_at,
        },
        session_max,
        sign_in.clone(),
    )
    .await?;
    let page_cookie = from_browser
        .then(|| page_cookie_for(&gateway, &issued, sign_in.as_deref()))
        .flatten();
    let mut response = (StatusCode::CREATED, Json(issued)).into_response();
    if let Some(cookie) = cookie {
        response.headers_mut().insert(SET_COOKIE, cookie.header);
    }
    page_access::append(response.headers_mut(), page_cookie);
    Ok(response)
}

/// The page cookie of a browser's sign-in that `issued` was issued in: it names the sign-in
/// (`sign_in`, the session cookie's record) when there is one, else the token itself, and lasts as
/// long as what it names may.
fn page_cookie_for(
    gateway: &Gateway,
    issued: &IssuedAccessToken,
    sign_in: Option<&str>,
) -> Option<HeaderValue> {
    let seconds = match sign_in {
        Some(_) => issued
            .ends_at
            .map_or(f64::MAX, |ends| ends - issued_now(issued)),
        None => issued.expires_in,
    };
    page_access::set_page_cookie(gateway, sign_in.unwrap_or(&issued.token_id), seconds)
}

/// The moment `issued` was answered at, by its own times.
fn issued_now(issued: &IssuedAccessToken) -> f64 {
    issued.expires_at - issued.expires_in
}

/// The person a trusted reverse proxy asserts on this request; `None` when it asserts nobody.
async fn proxy_person(
    gateway: &Gateway,
    providers: &AccessProviders,
    headers: &HeaderMap,
) -> Result<Option<Person>, ApiError> {
    let Some(proxy) = &providers.proxy_identity else {
        return Ok(None);
    };
    let Some(value) = headers.get(&proxy.header) else {
        return Ok(None);
    };
    require_origin(gateway, headers)?;
    let sent = headers
        .get(PROXY_SECRET_HEADER)
        .map(|value| value.as_bytes())
        .unwrap_or_default();
    let proven = proxy
        .secret
        .as_ref()
        .is_some_and(|secret| constant_time_eq(secret.as_bytes(), sent));
    if !proven {
        return Err(ApiError::unauthorized());
    }
    let value = value.to_str().map_err(|_| ApiError::unauthorized())?;
    let value = value.trim();
    if value.is_empty() || value.len() > 512 || value.contains(',') {
        return Err(ApiError::unauthorized());
    }
    let directory = providers.directory.clone();
    let lookup = Lookup {
        by: proxy.lookup_by.clone(),
        value: value.to_owned(),
    };
    tokio::task::spawn_blocking(move || directory.lookup(lookup))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::unauthorized)
        .map(Some)
}

/// `POST /v1/auth/resume`: a fresh access token for the sign-in a session cookie holds, so a
/// reloaded page need not ask for the password. The sign-in's time keeps running from when it
/// was made. It answers as `login` does; a cookie that does not stand is cleared.
async fn resume(
    State(gateway): State<Gateway>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // Unlike a bearer, a cookie goes along by itself, so the origin is not optional here.
    require_origin(&gateway, &headers)?;
    let value = session_cookie(&headers).ok_or_else(ApiError::unauthorized)?;
    if let Some((issued, sign_in)) = resumed_token(&gateway, &headers, value).await? {
        let page_cookie = page_cookie_for(&gateway, &issued, Some(&sign_in));
        let mut response = (StatusCode::CREATED, Json(issued)).into_response();
        page_access::append(response.headers_mut(), page_cookie);
        return Ok(response);
    }
    // Judged before being counted: the value cannot be guessed, so the count only slows a client
    // hammering this route, and must not keep out a good cookie behind the same address.
    let source = gateway.resume_guard.source(
        &headers,
        connection.map(|Extension(ConnectInfo(address))| address),
    );
    let refusal = match gateway.resume_guard.tried_address(source) {
        Some(wait) => ApiError::too_many_attempts(wait),
        None => ApiError::unauthorized(),
    };
    let mut response = refusal.into_response();
    response.headers_mut().append(SET_COOKIE, ended_cookie()?);
    Ok(response)
}

/// The token a session cookie's value gets, with the ID of its sign-in; `None` when it gets none: the setting is off, the
/// value is not a standing cookie, its time is up, its person has left the directory, or a
/// front proxy now says this browser is someone else.
async fn resumed_token(
    gateway: &Gateway,
    headers: &HeaderMap,
    value: &str,
) -> Result<Option<(IssuedAccessToken, String)>, ApiError> {
    let table = &gateway.config.unex().gateway;
    if !session_cookies_on(table) {
        return Ok(None);
    }
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let digest = session_cookie_digest(value);
    let found = registry.clone();
    let cookie = tokio::task::spawn_blocking(move || found.find_session_cookie(&digest))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    let Some(cookie) = cookie else {
        return Ok(None);
    };
    let providers = gateway.access.clone().ok_or_else(ApiError::unavailable)?;
    let directory = providers.directory.clone();
    let person_id = cookie.person_id().to_owned();
    let person = tokio::task::spawn_blocking(move || {
        directory.lookup(Lookup {
            by: "id".to_owned(),
            value: person_id,
        })
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    let Some(person) = person else {
        return Ok(None);
    };
    let asserted = proxy_person(gateway, &providers, headers).await?;
    if asserted.is_some_and(|asserted| asserted.id != person.id) {
        return Ok(None);
    }
    let now = unix_timestamp()?;
    // The sign-in's end as it was set, or sooner if `session_max` has been lowered since.
    let ends = cookie
        .expires_at()
        .min(cookie.issued_at() + table.session_max.as_secs_f64());
    if ends <= now {
        return Ok(None);
    }
    store_bearer(
        registry,
        person.id,
        Times {
            issued_at: cookie.issued_at(),
            expires_at: (now + table.access_token_ttl.0.as_secs_f64()).min(ends),
            now,
        },
        table.session_max,
        Some(cookie.token_id().to_owned()),
    )
    .await
    .map(|issued| Some((issued, cookie.token_id().to_owned())))
}

/// `POST /v1/auth/renew`: moves the presented token's expiry on by `access_token_ttl`, up to
/// `session_max` after it was issued. The value does not change, so what is bound to it keeps
/// working; the person must still be in the directory, and it is no try at a password.
async fn renew(State(gateway): State<Gateway>, headers: HeaderMap) -> Result<Response, ApiError> {
    if headers.contains_key(ORIGIN) {
        require_origin(&gateway, &headers)?;
    }
    let token = bearer(&headers)?;
    resolve_user(&gateway, token).await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let digest = access_token_digest(token);
    let record = tokio::task::spawn_blocking(move || registry.find_access_token(&digest))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::unauthorized)?;
    let table = &gateway.config.unex().gateway;
    let session_max = table.session_max;
    let expires_at = (unix_timestamp()? + table.access_token_ttl.0.as_secs_f64())
        .min(record.issued_at() + session_max.as_secs_f64());
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let token_id = record.token_id().to_owned();
    let extended =
        tokio::task::spawn_blocking(move || registry.extend_access_token(&token_id, expires_at))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|_| ApiError::internal())?
            .ok_or_else(ApiError::unauthorized)?;
    let renewed = RenewedAccessToken {
        token_id: extended.token_id().to_owned(),
        expires_at: extended.expires_at(),
        expires_in: extended.expires_at() - unix_timestamp()?,
        ends_at: session_end(extended.issued_at(), extended.expires_at(), session_max),
    };
    // A browser's renewal keeps its page cookie as long as the sign-in it names.
    let page_cookie = headers.contains_key(ORIGIN).then(|| {
        let seconds = match extended.sign_in() {
            Some(_) => renewed.ends_at.unwrap_or(f64::MAX) - unix_timestamp().unwrap_or(0.0),
            None => renewed.expires_in,
        };
        page_access::set_page_cookie(
            &gateway,
            extended.sign_in().unwrap_or(extended.token_id()),
            seconds,
        )
    });
    let mut response = Json(renewed).into_response();
    page_access::append(response.headers_mut(), page_cookie.flatten());
    Ok(response)
}

async fn authenticate_password(
    gateway: &Gateway,
    providers: &AccessProviders,
    headers: &HeaderMap,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    request: LoginRequest,
) -> Result<Person, ApiError> {
    let authenticator = providers
        .authenticator
        .clone()
        .ok_or_else(ApiError::unauthorized)?;
    let source = gateway.login_guard.source(
        headers,
        connection.map(|Extension(ConnectInfo(address))| address),
    );
    if let Some(wait) = gateway.login_guard.tried(source, &request.name) {
        return Err(ApiError::too_many_attempts(wait));
    }
    let attempt = Attempt {
        name: Some(request.name),
        secret: Some(request.secret),
        peer: Some(source.to_string()),
        secure: Some(true),
        ..Attempt::default()
    };
    let verdict = tokio::task::spawn_blocking(move || authenticator.authenticate(attempt))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    let Verdict::Valid(lookup) = verdict else {
        return Err(ApiError::unauthorized());
    };
    let directory = providers.directory.clone();
    tokio::task::spawn_blocking(move || directory.lookup(lookup))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::unauthorized)
}

async fn issue_access_token_record(
    registry: Arc<dyn RegistryBackend>,
    person_id: String,
    ttl: Duration,
    session_max: TimeLimit,
) -> Result<IssuedAccessToken, ApiError> {
    let issued_at = unix_timestamp()?;
    store_bearer(
        registry,
        person_id,
        Times {
            issued_at,
            expires_at: issued_at + ttl.as_secs_f64(),
            now: issued_at,
        },
        session_max,
        None,
    )
    .await
}

/// A bearer token's times. A resumed token's `issued_at` is the sign-in's, which is what keeps
/// its renewals within `session_max` of that sign-in.
struct Times {
    issued_at: f64,
    expires_at: f64,
    now: f64,
}

/// A new secret. Two independent UUIDs provide 256 random bits without a separate token crate.
fn new_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Stores a new bearer token, of the sign-in holding the session cookie `sign_in` when one is
/// named.
async fn store_bearer(
    registry: Arc<dyn RegistryBackend>,
    person_id: String,
    times: Times,
    session_max: TimeLimit,
    sign_in: Option<String>,
) -> Result<IssuedAccessToken, ApiError> {
    let Times {
        issued_at,
        expires_at,
        now,
    } = times;
    let token_id = Uuid::new_v4().to_string();
    let token = new_secret();
    let digest = access_token_digest(&token);
    let mut record = AccessTokenRecord::try_new(
        token_id.clone(),
        digest,
        person_id,
        issued_at,
        expires_at,
        None,
    )
    .map_err(|_| ApiError::bad_request())?;
    if let Some(sign_in) = sign_in {
        record = record
            .in_sign_in(sign_in)
            .map_err(|_| ApiError::internal())?;
    }
    tokio::task::spawn_blocking(move || registry.store_access_token(&record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(IssuedAccessToken {
        token_id,
        token,
        expires_at,
        expires_in: expires_at - now,
        ends_at: session_end(issued_at, expires_at, session_max),
    })
}

/// A session cookie just stored: its record's ID and the header that hands it to the browser.
struct NewSessionCookie {
    sign_in: String,
    header: HeaderValue,
}

/// Whether a browser's sign-in gets a session cookie: with renewal off the sign-in ends with its
/// first token, and there is nothing to come back to.
fn session_cookies_on(table: &ai6::UnexGatewayLimits) -> bool {
    table.session_cookie && table.session_max.get() != Some(Duration::ZERO)
}

/// Stores a sign-in's session cookie, which ends `session_max` after `issued_at` and never later.
async fn store_session_cookie(
    registry: Arc<dyn RegistryBackend>,
    person_id: &str,
    issued_at: f64,
    session_max: TimeLimit,
) -> Result<NewSessionCookie, ApiError> {
    let lifetime = session_max.as_secs_f64();
    // No bound leaves nothing to write down but the largest time there is. Past about a year a
    // browser keeps a cookie only as long as it chooses, whatever Max-Age says.
    let expires_at = if lifetime.is_finite() {
        issued_at + lifetime
    } else {
        f64::MAX
    };
    let max_age = lifetime.ceil().min(f64::from(u32::MAX)) as u64;
    let value = new_secret();
    let sign_in = Uuid::new_v4().to_string();
    let record = AccessTokenRecord::try_new(
        sign_in.clone(),
        session_cookie_digest(&value),
        person_id,
        issued_at,
        expires_at,
        None,
    )
    .map_err(|_| ApiError::bad_request())?
    .into_session_cookie();
    tokio::task::spawn_blocking(move || registry.store_access_token(&record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(NewSessionCookie {
        sign_in,
        header: session_cookie_header(&value, max_age)?,
    })
}

async fn resolve_user(gateway: &Gateway, token: &str) -> Result<Person, ApiError> {
    resolve_digest(gateway, access_token_digest(token)).await
}

/// The person whose access token, or live [`StandIn`], has the digest `digest`.
async fn resolve_digest(gateway: &Gateway, digest: String) -> Result<Person, ApiError> {
    let standing_in = gateway
        .stand_ins
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&digest)
        .cloned();
    let person_id = match standing_in {
        Some(person_id) => person_id,
        None => {
            let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
            tokio::task::spawn_blocking(move || registry.find_access_token(&digest))
                .await
                .map_err(|_| ApiError::internal())?
                .map_err(|_| ApiError::internal())?
                .ok_or_else(ApiError::unauthorized)?
                .person_id()
                .to_owned()
        }
    };
    person_by_id(gateway, person_id).await
}

/// The person with ID `person_id` in the directory, now.
async fn person_by_id(gateway: &Gateway, person_id: String) -> Result<Person, ApiError> {
    let directory = gateway
        .access
        .as_ref()
        .ok_or_else(ApiError::unavailable)?
        .directory
        .clone();
    tokio::task::spawn_blocking(move || {
        directory.lookup(Lookup {
            by: "id".to_owned(),
            value: person_id,
        })
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::unauthorized)
}

async fn issue_user_attach_token(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(request): Json<GrantRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let mode = match request.mode.as_str() {
        "read" => AttachMode::Read,
        "write" => AttachMode::Write,
        _ => return Err(ApiError::bad_request()),
    };
    let grant = AttachGrant {
        operation_id: OperationId::new(request.operation_id)
            .map_err(|_| ApiError::bad_request())?,
        mission_id: MissionId::new(request.mission_id).map_err(|_| ApiError::bad_request())?,
        session_id: SessionId::new(request.session_id).map_err(|_| ApiError::bad_request())?,
        mode,
    };
    let session = gateway
        .inner
        .lock()
        .await
        .sessions
        .get(&grant.session_id)
        .map(|record| (record.operation_id.clone(), record.mission_id.clone()))
        .ok_or_else(|| ApiError::from(GatewayError::NotFound))?;
    if session != (grant.operation_id.clone(), grant.mission_id.clone()) {
        return Err(ApiError::forbidden());
    }
    let providers = gateway.access.clone().ok_or_else(ApiError::unavailable)?;
    let action = match grant.mode {
        AttachMode::Read => "session.read",
        AttachMode::Write => "session.input",
    };
    let request = operation_request(
        person,
        action,
        grant.operation_id.as_str(),
        &operation_tree(&gateway).await?,
    );
    let decision = tokio::task::spawn_blocking(move || providers.authorizer.authorize(request))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    if !decision.allowed {
        return Err(ApiError::forbidden());
    }
    Ok(Json(TokenResponse {
        token: gateway.store_attach_token(grant).await?,
    }))
}

async fn access_session(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<AccessSession>, ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::internal)?;
    let token = bearer(&headers)?;
    let digest = access_token_digest(token);
    let record = tokio::task::spawn_blocking(move || registry.find_access_token(&digest))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::unauthorized)?;
    Ok(Json(AccessSession {
        token_id: record.token_id().to_owned(),
        person_id: record.person_id().to_owned(),
        expires_at: record.expires_at(),
    }))
}

async fn user_profile(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<UserProfile>, ApiError> {
    let token = bearer(&headers)?;
    let person = resolve_user(&gateway, token).await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let digest = access_token_digest(token);
    let record = tokio::task::spawn_blocking(move || registry.find_access_token(&digest))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::unauthorized)?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let station_records = tokio::task::spawn_blocking(move || registry.stations(Duration::MAX))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    let authorization_person = person.clone();
    let tree = OperationTree::from_check_ins(&station_records);
    observe_tree(&gateway, &tree);
    // The profile says what could be read and reads nothing.
    let authorizer = gateway.probing_authorizer()?;
    let (operations, stations, roster, global_actions) = tokio::task::spawn_blocking(move || {
        let mut operation_ids: BTreeSet<String> = tree.operations().map(str::to_owned).collect();
        let mut stations = Vec::new();
        for station in &station_records {
            let station_id = station.station().to_owned();
            let mut actions = Vec::new();
            for &action in ai6::STATION_ACTIONS {
                let allowed = authorizer
                    .authorize(AuthorizationRequest {
                        person: authorization_person.clone(),
                        action: action.to_owned(),
                        resource: format!("station:{station_id}"),
                        ancestors: Vec::new(),
                    })
                    .map_err(|_| ApiError::internal())?
                    .allowed;
                if allowed {
                    actions.push(action.to_owned());
                }
            }
            if !actions.is_empty() {
                stations.push(UserStationAccess {
                    station_id,
                    actions,
                });
            }
            let Some(values) = station
                .info()
                .get("operations")
                .and_then(serde_json::Value::as_array)
            else {
                continue;
            };
            for value in values {
                if let Some(value) = value.as_str()
                    && let Ok(operation_id) = OperationId::new(value.to_owned())
                {
                    operation_ids.insert(operation_id.to_string());
                }
            }
        }
        let mut operations = Vec::new();
        for operation_id in operation_ids {
            let mut actions = Vec::new();
            for &action in ai6::OPERATION_ACTIONS {
                let allowed = authorizer
                    .authorize(operation_request(
                        authorization_person.clone(),
                        action,
                        &operation_id,
                        &tree,
                    ))
                    .map_err(|_| ApiError::internal())?
                    .allowed;
                if allowed {
                    actions.push(action.to_owned());
                }
            }
            let compartments: Vec<String> = authorizer
                .compiled()
                .map(|compiled| {
                    compiled
                        .person_compartments(&authorization_person, &operation_id)
                        .into_iter()
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if !actions.is_empty() {
                operations.push(UserOperationAccess {
                    operation_id,
                    parent: None,
                    actions,
                    compartments,
                });
            }
        }
        // An ancestor is named only when the person may read its Missions.
        let readable: BTreeSet<String> = operations
            .iter()
            .filter(|access| access.actions.iter().any(|action| action == "mission.read"))
            .map(|access| access.operation_id.clone())
            .collect();
        for access in &mut operations {
            access.parent = tree.visible_parent(&access.operation_id, |id| readable.contains(id));
        }
        stations.sort_by(|left, right| left.station_id.cmp(&right.station_id));
        let mut roster = Vec::new();
        for action in ["roster.read", "roster.manage", "audit.read"] {
            let allowed = authorizer
                .authorize(AuthorizationRequest {
                    person: authorization_person.clone(),
                    action: action.to_owned(),
                    resource: roster::ROSTER_RESOURCE.to_owned(),
                    ancestors: Vec::new(),
                })
                .map_err(|_| ApiError::internal())?
                .allowed;
            if allowed {
                roster.push(action.to_owned());
            }
        }
        let mut global_actions = Vec::new();
        for (action, resource) in [
            ("access.read", ai6::CONFIG_ACCESS),
            ("access.manage", ai6::CONFIG_ACCESS),
            ("account.manage", ai6::CONFIG_ACCOUNTS),
            ("assistant.memory.admin", "config:assistant"),
            ("compartment.manage", "compartment:*"),
            ("memory.write", "config:memory"),
        ] {
            let allowed = authorizer
                .authorize(AuthorizationRequest {
                    person: authorization_person.clone(),
                    action: action.to_owned(),
                    resource: resource.to_owned(),
                    ancestors: Vec::new(),
                })
                .map_err(|_| ApiError::internal())?
                .allowed;
            if allowed {
                global_actions.push(action.to_owned());
            }
        }
        Ok::<_, ApiError>((operations, stations, roster, global_actions))
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(UserProfile {
        person_id: person.id,
        name: person.name,
        display_name: person.display_name,
        groups: person.groups,
        expires_at: record.expires_at(),
        operations,
        stations,
        roster,
        global_actions,
        moneypenny: gateway.assistant.is_some(),
    }))
}

async fn authorize_operation_action(
    gateway: &Gateway,
    person: Person,
    operation_id: &OperationId,
    action: &'static str,
) -> Result<(), ApiError> {
    authorize_resource_action(
        gateway,
        person,
        tree::operation_resource(operation_id.as_str()),
        action,
    )
    .await
}

/// The tree of Operations from the Stations' latest check-ins; empty without a Registry.
async fn operation_tree(gateway: &Gateway) -> Result<OperationTree, ApiError> {
    let Some(registry) = gateway.registry.clone() else {
        return Ok(OperationTree::default());
    };
    let tree = tokio::task::spawn_blocking(move || registry.stations(Duration::MAX))
        .await
        .map_err(|_| ApiError::internal())?
        .map(|records| OperationTree::from_check_ins(&records))
        .map_err(|_| ApiError::internal())?;
    observe_tree(gateway, &tree);
    Ok(tree)
}

/// Every Operation the gateway knows of: from the Stations' check-ins and its roster.
async fn known_operations(gateway: &Gateway) -> Vec<String> {
    let mut known: BTreeSet<String> = BTreeSet::new();
    if let Ok(tree) = operation_tree(gateway).await {
        known.extend(tree.operations().map(str::to_owned));
    }
    if let Some(registry) = gateway.registry.clone()
        && let Ok(Ok(stations)) =
            tokio::task::spawn_blocking(move || registry.stations(Duration::MAX)).await
    {
        for station in &stations {
            if let Some(operations) = station
                .info()
                .get("operations")
                .and_then(serde_json::Value::as_array)
            {
                known.extend(
                    operations
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned),
                );
            }
        }
    }
    if let Some(source) = gateway.roster.clone()
        && let Ok(Ok(definitions)) = tokio::task::spawn_blocking(move || {
            source.definitions(ai6::ConfigRecord::Operation, &ai6::ConfigScope::Global)
        })
        .await
    {
        known.extend(definitions.records.into_iter().map(|(id, _)| id));
    }
    known.into_iter().collect()
}

/// Tells the access policy the tree the Stations last published, so Operation policies are
/// checked against it.
fn observe_tree(gateway: &Gateway, tree: &OperationTree) {
    if let Some(access) = &gateway.access_admin {
        access.observe_lineage(access_admin::tree_lineage(tree));
    }
}

async fn authorize_resource_action(
    gateway: &Gateway,
    person: Person,
    resource: String,
    action: &'static str,
) -> Result<(), ApiError> {
    if resource_action_allowed(gateway, person, resource, action).await? {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

/// Whether `person` holds `action` on `resource`, for what a view only shows to who does.
async fn resource_action_allowed(
    gateway: &Gateway,
    person: Person,
    resource: String,
    action: &'static str,
) -> Result<bool, ApiError> {
    let authorizer = gateway.authorizer()?;
    let request = resource_request(person, action, resource, &operation_tree(gateway).await?);
    let decision = tokio::task::spawn_blocking(move || authorizer.authorize(request))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(decision.allowed)
}

async fn ensure_station_registered(
    registry: &Arc<dyn RegistryBackend>,
    station_id: &str,
) -> Result<ai6::StationRecord, ApiError> {
    let registry = registry.clone();
    let station_id = station_id.to_owned();
    tokio::task::spawn_blocking(move || {
        registry
            .stations(Duration::MAX)
            .map_err(|_| ApiError::internal())?
            .into_iter()
            .find(|station| station.station() == station_id)
            .ok_or_else(|| ApiError::from(GatewayError::NotFound))
    })
    .await
    .map_err(|_| ApiError::internal())?
}

async fn get_user_station_settings(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(station_id): Path<String>,
) -> Result<Json<StationSettingsResponse>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    StationSettings::try_new(station_id.clone(), 1).map_err(|_| ApiError::bad_request())?;
    authorize_resource_action(
        &gateway,
        person,
        format!("station:{station_id}"),
        "station.read",
    )
    .await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let station = ensure_station_registered(&registry, &station_id).await?;
    let execution_enabled = station
        .info()
        .get("roles")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|roles| roles.iter().any(|role| role.as_str() == Some("execution")));
    let station_key = station_id.clone();
    let registry = registry.clone();
    let settings = tokio::task::spawn_blocking(move || registry.station_settings(&station_key))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    let (max_concurrent_missions, limits, configured) = match settings {
        Some(settings) => (
            settings.max_concurrent_missions(),
            settings.resource_limits(),
            true,
        ),
        None => (
            station
                .info()
                .get("capacity")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or_default(),
            ai6::ResourceLimits::default(),
            false,
        ),
    };
    Ok(Json(StationSettingsResponse {
        station_id,
        max_concurrent_missions,
        min_free_memory_mb: limits.min_free_memory_mb,
        max_cpu_percent: limits.max_cpu_percent,
        configured,
        execution_enabled,
    }))
}

async fn put_user_station_settings(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(station_id): Path<String>,
    Json(request): Json<UpdateStationSettings>,
) -> Result<StatusCode, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let settings = StationSettings::try_new(station_id.clone(), request.max_concurrent_missions)
        .map_err(|_| ApiError::bad_request())?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        format!("station:{station_id}"),
        "station.configure",
    )
    .await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let station = ensure_station_registered(&registry, &station_id).await?;
    let execution_enabled = station
        .info()
        .get("roles")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|roles| roles.iter().any(|role| role.as_str() == Some("execution")));
    if !execution_enabled {
        return Err(ApiError::from(GatewayError::Conflict));
    }
    let station = station_id.clone();
    let write_registry = registry.clone();
    let mut changed = vec!["max_concurrent_missions"];
    if request.min_free_memory_mb.is_some() {
        changed.push("min_free_memory_mb");
    }
    if request.max_cpu_percent.is_some() {
        changed.push("max_cpu_percent");
    }
    tokio::task::spawn_blocking(move || {
        let current = write_registry
            .station_settings(&station)?
            .map(|settings| settings.resource_limits())
            .unwrap_or_default();
        let limits = ai6::ResourceLimits::new(
            request
                .min_free_memory_mb
                .unwrap_or(current.min_free_memory_mb),
            request.max_cpu_percent.unwrap_or(current.max_cpu_percent),
        );
        write_registry.set_station_settings(&station, &settings.with_resource_limits(limits)?)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::bad_request())?;
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id,
        action: "station.settings.update".to_owned(),
        operation: String::new(),
        resource: Some(format!("station:{station_id}")),
        target: changed.join(", "),
        at: unix_timestamp()?,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&audit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_operation_registered(
    registry: &Arc<dyn RegistryBackend>,
    operation_id: &OperationId,
) -> Result<(), ApiError> {
    let registry = registry.clone();
    let operation_id = operation_id.to_string();
    let registered = tokio::task::spawn_blocking(move || {
        registry
            .stations(Duration::MAX)
            .map(|stations| {
                stations.iter().any(|station| {
                    station
                        .info()
                        .get("operations")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|operations| {
                            operations
                                .iter()
                                .any(|value| value.as_str() == Some(&operation_id))
                        })
                })
            })
            .map_err(|_| ApiError::internal())
    })
    .await
    .map_err(|_| ApiError::internal())??;
    if registered {
        Ok(())
    } else {
        Err(ApiError::from(GatewayError::NotFound))
    }
}

async fn list_user_agent_roles(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<Vec<AgentRoleResponse>>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person, &operation_id, "agent.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation_id).await?;
    let operation = operation_id.to_string();
    let roles = tokio::task::spawn_blocking(move || {
        let mut roles = BTreeMap::new();
        let headquarters = registry.headquarters().map_err(|_| ApiError::internal())?;
        if let Some(headquarters) = headquarters {
            let stations = registry
                .stations(Duration::MAX)
                .map_err(|_| ApiError::internal())?;
            if let Some(station) = stations
                .iter()
                .find(|station| station.station() == headquarters.station())
                && let Some(values) = station
                    .info()
                    .get("agent_roles")
                    .and_then(serde_json::Value::as_array)
            {
                for value in values {
                    if let Ok(role) = serde_json::from_value::<AgentRole>(value.clone())
                        && role.validate().is_ok()
                    {
                        roles.insert(role.id.clone(), (role, false));
                    }
                }
            }
        }
        for role in registry
            .list_agent_roles(&operation)
            .map_err(|_| ApiError::internal())?
        {
            roles.insert(role.id.clone(), (role, true));
        }
        Ok::<_, ApiError>(
            roles
                .into_values()
                .map(|(role, operation_override)| AgentRoleResponse {
                    role,
                    operation_override,
                })
                .collect::<Vec<_>>(),
        )
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(roles))
}

async fn put_user_agent_role(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, role_id)): Path<(String, String)>,
    Json(role): Json<AgentRole>,
) -> Result<StatusCode, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    if role.id != role_id || role.validate().is_err() {
        return Err(ApiError::bad_request());
    }
    authorize_operation_action(&gateway, person.clone(), &operation_id, "agent.configure").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation_id).await?;
    let operation = operation_id.to_string();
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id,
        action: "agent.role.update".to_owned(),
        operation: operation.clone(),
        resource: None,
        target: role.id.clone(),
        at: unix_timestamp()?,
    };
    tokio::task::spawn_blocking(move || registry.put_agent_role(&operation, &role))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::bad_request())?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    tokio::task::spawn_blocking(move || registry.record_audit(&audit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_user_agent_role(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, role_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    if role_id.is_empty()
        || role_id.len() > 128
        || !role_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(ApiError::bad_request());
    }
    authorize_operation_action(&gateway, person.clone(), &operation_id, "agent.configure").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation_id).await?;
    let operation = operation_id.to_string();
    let role = role_id.clone();
    let removed =
        tokio::task::spawn_blocking(move || registry.remove_agent_role(&operation, &role))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|_| ApiError::internal())?;
    if !removed {
        return Err(ApiError::from(GatewayError::NotFound));
    }
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id,
        action: "agent.role.remove".to_owned(),
        operation: operation_id.to_string(),
        resource: None,
        target: role_id,
        at: unix_timestamp()?,
    };
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    tokio::task::spawn_blocking(move || registry.record_audit(&audit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuarantineRequest {
    reason: String,
}

fn valid_agent_id(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && agent_id.len() <= 128
        && agent_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

async fn list_user_quarantines(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<Vec<Quarantine>>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person, &operation_id, "agent.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let operation = operation_id.to_string();
    let records = tokio::task::spawn_blocking(move || registry.quarantines())
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(Json(
        records
            .into_iter()
            .filter(|record| {
                record
                    .operation
                    .as_deref()
                    .is_none_or(|own| own == operation)
            })
            .collect(),
    ))
}

async fn quarantine_user_agent(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, agent_id)): Path<(String, String)>,
    Json(request): Json<QuarantineRequest>,
) -> Result<StatusCode, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    let reason = request.reason.trim().to_owned();
    if !valid_agent_id(&agent_id)
        || reason.is_empty()
        || reason.chars().count() > 512
        || reason.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request());
    }
    authorize_operation_action(&gateway, person.clone(), &operation_id, "agent.configure").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation_id).await?;
    let at = unix_timestamp()?;
    let record = Quarantine::new(
        agent_id.clone(),
        Some(operation_id.to_string()),
        reason,
        person.id.clone(),
        at,
    );
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id,
        action: "agent.quarantine".to_owned(),
        operation: operation_id.to_string(),
        resource: None,
        target: agent_id,
        at,
    };
    tokio::task::spawn_blocking(move || {
        registry.quarantine_agent(&record)?;
        registry.record_audit(&audit)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::CREATED)
}

async fn release_user_quarantine(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, agent_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    if !valid_agent_id(&agent_id) {
        return Err(ApiError::bad_request());
    }
    authorize_operation_action(&gateway, person.clone(), &operation_id, "agent.configure").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let operation = operation_id.to_string();
    let target = agent_id.clone();
    // Only this Operation's own record is lifted here; a quarantine of every Operation is
    // released by an operator on a Station (`ai6 quarantine release --all`).
    let released =
        tokio::task::spawn_blocking(move || registry.release_quarantine(&target, Some(&operation)))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|_| ApiError::internal())?;
    if !released {
        return Err(ApiError::from(GatewayError::NotFound));
    }
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id,
        action: "agent.quarantine.release".to_owned(),
        operation: operation_id.to_string(),
        resource: None,
        target: agent_id,
        at: unix_timestamp()?,
    };
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    tokio::task::spawn_blocking(move || registry.record_audit(&audit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn user_audit(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<UserAuditQuery>,
) -> Result<Json<Vec<AuditRecord>>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_filter = query
        .operation_id
        .map(OperationId::new)
        .transpose()
        .map_err(|_| ApiError::bad_request())?
        .map(|operation| operation.to_string());
    if operation_filter.is_some() && query.station_id.is_some() {
        return Err(ApiError::bad_request());
    }
    let station_filter = query
        .station_id
        .map(|station_id| {
            StationSettings::try_new(station_id.clone(), 1)
                .map(|_| station_id)
                .map_err(|_| ApiError::bad_request())
        })
        .transpose()?;
    let limit = audit_page(query.limit, &gateway.config.unex().timeline)
        .ok_or_else(ApiError::bad_request)?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let authorizer = gateway.authorizer()?;
    let tree = operation_tree(&gateway).await?;
    if let Some(operation_id) = &operation_filter {
        let scoped_authorizer = authorizer.clone();
        let request = operation_request(person.clone(), "audit.read", operation_id, &tree);
        let allowed = tokio::task::spawn_blocking(move || scoped_authorizer.authorize(request))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|_| ApiError::internal())?
            .allowed;
        if !allowed {
            return Err(ApiError::forbidden());
        }
    } else if let Some(station_id) = &station_filter {
        let registry_check = registry.clone();
        ensure_station_registered(&registry_check, station_id).await?;
        let scoped_authorizer = authorizer.clone();
        let scoped_person = person.clone();
        let resource = format!("station:{station_id}");
        let allowed = tokio::task::spawn_blocking(move || {
            scoped_authorizer.authorize(AuthorizationRequest {
                person: scoped_person,
                action: "audit.read".to_owned(),
                resource,
                ancestors: Vec::new(),
            })
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .allowed;
        if !allowed {
            return Err(ApiError::forbidden());
        }
    }
    let entries = tokio::task::spawn_blocking(move || {
        let records = registry.list_audit(500).map_err(|_| ApiError::internal())?;
        let mut visible = Vec::new();
        for record in records {
            // A crossing belongs to both sides: the reader's Operation and what it read.
            let crossed = record
                .resource
                .as_deref()
                .and_then(|resource| resource.strip_prefix("operation:"));
            if operation_filter.as_ref().is_some_and(|operation_id| {
                operation_id != &record.operation && crossed != Some(operation_id.as_str())
            }) {
                continue;
            }
            let resource = record
                .resource
                .clone()
                .filter(|resource| !resource.starts_with("mission:"))
                .unwrap_or_else(|| tree::operation_resource(&record.operation));
            if station_filter
                .as_ref()
                .is_some_and(|station_id| resource != format!("station:{station_id}"))
            {
                continue;
            }
            let allowed = authorizer
                .authorize(resource_request(
                    person.clone(),
                    "audit.read",
                    resource,
                    &tree,
                ))
                .map_err(|_| ApiError::internal())?
                .allowed
                || (crossed.is_some()
                    && authorizer
                        .authorize(operation_request(
                            person.clone(),
                            "audit.read",
                            &record.operation,
                            &tree,
                        ))
                        .map_err(|_| ApiError::internal())?
                        .allowed);
            if allowed {
                visible.push(record);
                if visible.len() == limit {
                    break;
                }
            }
        }
        Ok::<_, ApiError>(visible)
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(entries))
}

/// `POST /v1/auth/logout`: ends the sign-in of the presented token, of the session cookie when
/// the app's own page sends one, or both. The cookie counts by itself because a token can be over
/// by then, and signing out must still end what a reload would come back to.
async fn logout(State(gateway): State<Gateway>, headers: HeaderMap) -> Result<Response, ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let cookie = session_cookie(&headers);
    let mut ended = false;
    if let Some(value) = cookie.filter(|_| require_origin(&gateway, &headers).is_ok()) {
        let digest = session_cookie_digest(value);
        let found = registry.clone();
        let record = tokio::task::spawn_blocking(move || found.find_session_cookie(&digest))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|_| ApiError::internal())?;
        if let Some(record) = record {
            ended |= revoke_token_record(&registry, record).await?;
        }
    }
    if let Ok(token) = bearer(&headers) {
        let digest = access_token_digest(token);
        let found = registry.clone();
        let record = tokio::task::spawn_blocking(move || found.find_access_token(&digest))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|_| ApiError::internal())?;
        if let Some(record) = record {
            ended |= revoke_token_record(&registry, record).await?;
        }
    }
    let mut response = if ended {
        StatusCode::NO_CONTENT.into_response()
    } else {
        ApiError::unauthorized().into_response()
    };
    if cookie.is_some() {
        response.headers_mut().append(SET_COOKIE, ended_cookie()?);
    }
    // A browser's page cookie named this sign-in, which no longer stands either way.
    if headers.contains_key(ORIGIN) {
        page_access::append(
            response.headers_mut(),
            page_access::ended_page_cookie(&gateway),
        );
    }
    Ok(response)
}

async fn revoke_token_record(
    registry: &Arc<dyn RegistryBackend>,
    record: AccessTokenRecord,
) -> Result<bool, ApiError> {
    let registry = registry.clone();
    let token_id = record.token_id().to_owned();
    tokio::task::spawn_blocking(move || registry.revoke_access_token(&token_id))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())
}

async fn revoke_access_token(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(token_id): Path<String>,
) -> Result<Json<RevokedAccessToken>, ApiError> {
    gateway
        .authenticate_station(bearer(&headers)?)
        .map_err(ApiError::from)?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::internal)?;
    let revoked = tokio::task::spawn_blocking(move || registry.revoke_access_token(&token_id))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(Json(RevokedAccessToken { revoked }))
}

fn access_token_digest(token: &str) -> String {
    secret_digest(b"ai6-access-token\0", token)
}

/// Under its own prefix, so a cookie's value presented as a bearer token looks up a digest that
/// no record has.
fn session_cookie_digest(value: &str) -> String {
    secret_digest(b"ai6-session-cookie\0", value)
}

fn secret_digest(purpose: &[u8], secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(purpose);
    hasher.update(secret.as_bytes());
    PeerId::from_bytes(hasher.finalize().into()).hex()
}

fn unix_timestamp() -> Result<f64, ApiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs_f64())
        .map_err(|_| ApiError::internal())
}

fn utc_timestamp() -> Result<String, ApiError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ApiError::internal())?
        .as_secs();
    let at = time::OffsetDateTime::from_unix_timestamp(
        i64::try_from(seconds).map_err(|_| ApiError::internal())?,
    )
    .map_err(|_| ApiError::internal())?;
    at.format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| ApiError::internal())
}

#[derive(Deserialize)]
struct RegisterSessionRequest {
    operation_id: String,
    mission_id: String,
    session_id: String,
    mission: Option<LaunchManifest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateUserMissionRequest {
    title: String,
    prompt: String,
    agent_id: Option<String>,
    role: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    stations: Vec<String>,
    /// The review policy per gate, `none` included; naming one needs `mission.review`.
    #[serde(default)]
    review: Option<ai6::ReviewChoice>,
    /// An item of one of the Operation's Sources, with a Mission of its own, that this Mission
    /// follows up: it is numbered by the item and linked to it.
    #[serde(default)]
    issue: Option<FollowedIssue>,
    /// An issue to create for this Mission, which then enters numbered by it and linked to it;
    /// needs `issue.create`.
    #[serde(default)]
    create_issue: Option<NewMissionIssue>,
    /// What the Mission is to deliver, each `kind` or `kind:target`, beyond its role's.
    #[serde(default)]
    deliverables: Vec<String>,
}

/// The item a Mission follows up.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FollowedIssue {
    /// The tracker instance, a key under the Operation's `sources`.
    tracker: String,
    /// The provider-local item ID.
    item: String,
}

/// The issue a Mission is created with.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewMissionIssue {
    /// The tracker instance to create it in.
    tracker: String,
    /// Its title; the Mission's title when left out.
    #[serde(default)]
    title: Option<String>,
    /// Its body; the Mission's instructions when left out.
    #[serde(default)]
    body: Option<String>,
}

#[derive(Serialize)]
struct CreateUserMissionResponse {
    operation_id: String,
    /// The Mission's UID; none yet while it waits for the issue it is created with.
    #[serde(skip_serializing_if = "Option::is_none")]
    mission_id: Option<String>,
    /// What it is called; none yet while it waits for the issue it is created with.
    #[serde(skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    /// The approved issue proposal the Headquarters Station creates the issue from.
    #[serde(skip_serializing_if = "Option::is_none")]
    proposal: Option<String>,
    state: &'static str,
}

#[derive(Serialize)]
struct MissionProposalDecisionResponse {
    operation_id: String,
    proposal_id: String,
    mission_id: Option<String>,
    handle: Option<String>,
    state: &'static str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameMissionRequest {
    handle: String,
}

#[derive(Serialize)]
struct RenamedMission {
    operation_id: String,
    /// The Mission's UID, which the rename leaves as it is.
    mission_id: String,
    /// What it is called now.
    handle: String,
    /// What it was called; it still names the Mission.
    previous: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserAuditQuery {
    operation_id: Option<String>,
    station_id: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct LifecycleRequest {
    operation_id: String,
    mission_id: String,
    state: String,
    #[serde(default)]
    reason: Option<String>,
}

/// One provider Session entry returned by the public board API.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardSession {
    /// Project that owns the Mission.
    pub operation_id: String,
    /// AI6 Mission ID.
    pub mission_id: String,
    /// Provider-issued resumable Session ID.
    pub session_id: String,
    /// Latest Mission state, absent until the first lifecycle event.
    pub state: Option<String>,
    /// Why AI6 holds the Session in that state, when it says: `green-ink` while a step
    /// waits for a person's approval, `budget` after a spending limit.
    pub reason: Option<String>,
    /// Sequence of the latest lifecycle event, or zero before one arrives.
    pub sequence: u64,
    /// When the latest lifecycle event arrived, as Unix seconds; the most recently updated
    /// Session of a Mission carries the Mission's current state.
    pub updated_at: Option<f64>,
    /// The untrusted input the Session has read (`brief:source`, `cable:SENDER`,
    /// `tool:NAME`, `review`), from its Registry record; empty when it has read none.
    pub exposure: Vec<String>,
    /// The Station that holds, or last held, the Session, from its Registry record; absent
    /// without a Registry or before the Station records the Session.
    pub station_id: Option<String>,
    /// The Agent the Mission's published brief names; absent before the brief arrives or
    /// when it names none. Every Session of a Mission carries the same one.
    pub agent: Option<String>,
}

/// One registered Mission with the provider Sessions known to Universal Exports.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardMission {
    /// Project that owns the Mission.
    pub operation_id: String,
    /// AI6 Mission ID: the Mission's UID when it came from the Registry.
    pub mission_id: String,
    /// What the Mission is called now; its ID when the Registry names it no other way.
    pub handle: String,
    /// Provider Sessions registered for this Mission.
    pub sessions: Vec<BoardSession>,
    /// Exact Mission brief and sanitized launch manifest published by AI6, when known.
    pub brief: Option<LaunchManifest>,
    /// Tokens and cost the Mission's Sessions consumed, from the shared Registry ledger;
    /// absent until a provider reports usage.
    pub usage: Option<MissionUsage>,
    /// Every attempt at the Mission, first first, from the shared Session records; empty
    /// when no Session was recorded.
    pub attempts: Vec<MissionAttempt>,
}

/// One attempt at a Mission: which Station ran it on which Theater, and how it stands.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MissionAttempt {
    /// Which try at the Mission this is, starting at 1.
    pub attempt: u32,
    /// AI6's Session ID: the run's UID.
    pub session_id: String,
    /// The run's handle, `<mission handle>.<attempt>`, as the Mission was called when it
    /// opened.
    pub handle: Option<String>,
    /// The provider's Session ID, once issued.
    pub provider_session_id: Option<String>,
    /// The AI6 Session ID of the attempt this one replaced.
    pub predecessor: Option<String>,
    /// The Station that holds, or last held, the Session.
    pub station: String,
    /// The Theater the harness ran in.
    pub theater: Option<String>,
    /// `active`, `parked`, `ended` or `lost`.
    pub status: String,
    /// Why the attempt was lost; set only when `status` is `lost`.
    pub lost: Option<String>,
    /// The branch its work was last checkpointed to, when it was.
    pub checkpoint_branch: Option<String>,
    /// The handler, model and pool it ran on, once it was launched.
    pub route: Option<ai6::RouteRef>,
    /// The account it drew from, when it drew from a pool.
    pub account: Option<ai6::AccountRef>,
    /// What a person found when they checked how it ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<ai6::MissionVerdict>,
    /// What the Mission was to deliver, as `kind` or `kind:target`, once the attempt ended done
    /// and went to be verified.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deliverables: Vec<String>,
    /// What the Station's probes found of those, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<ai6::Evidence>,
    /// When a probe was asked for, as Unix seconds, until the Station has run it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_requested: Option<f64>,
    /// The last message its Agent ended a turn with, as data. Only one Mission's detail has it:
    /// each can be as long as `[registry.limits.sessions] final_report_bytes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_report: Option<String>,
}

impl MissionAttempt {
    fn from_record(record: &ai6::SessionRecord) -> Self {
        Self {
            route: record.launch.as_ref().map(|launch| {
                ai6::RouteRef::new(
                    launch.handler.clone(),
                    launch.model.clone(),
                    launch.pool.clone(),
                )
            }),
            account: record.account.clone(),
            verdict: record.verdict.clone(),
            deliverables: record
                .deliverables
                .iter()
                .map(ToString::to_string)
                .collect(),
            evidence: record.evidence.clone(),
            probe_requested: record.probe_requested,
            final_report: None,
            attempt: record.attempt,
            session_id: record.session_id.clone(),
            handle: record.handle.clone(),
            provider_session_id: record.provider_session_id.clone(),
            predecessor: record.predecessor.clone(),
            station: record.station.clone(),
            theater: record.theater.clone(),
            status: serde_json::to_value(record.status)
                .ok()
                .and_then(|status| status.as_str().map(str::to_owned))
                .unwrap_or_default(),
            lost: record.lost.clone(),
            checkpoint_branch: record
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.branch.clone()),
        }
    }
}

/// The attempts at each Mission among `records`, in attempt order.
fn attempts_by_mission(records: Vec<ai6::SessionRecord>) -> BTreeMap<String, Vec<MissionAttempt>> {
    let mut attempts = BTreeMap::<String, Vec<MissionAttempt>>::new();
    for record in &records {
        attempts
            .entry(record.mission_id.clone())
            .or_default()
            .push(MissionAttempt::from_record(record));
    }
    for list in attempts.values_mut() {
        list.sort_by_key(|attempt| attempt.attempt);
    }
    attempts
}

/// AI6 Station overview derived from the shared check-in Registry.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StationBoard {
    /// Registered daemon Stations visible through an authorized Operation.
    pub stations: Vec<BoardStation>,
    /// Current elected Headquarters Station, when visible and its lease is live.
    pub headquarters_station: Option<String>,
    /// The Operations the person may read, from the Stations and the tree of Operations they
    /// publish, each with its nearest readable ancestor.
    pub operations: Vec<BoardOperation>,
}

/// One Operation on the board and where it sits in the tree as the person sees it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardOperation {
    /// Operation codename.
    pub operation_id: String,
    /// The nearest Operation above it the person may read; `None` (omitted) for a root or
    /// when every ancestor is hidden from them, which are never named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// One daemon Station and the Operations it hosts for this user.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardStation {
    /// Daemon Station identity.
    pub station_id: String,
    /// The Agent handlers it has loaded, as it reported at check-in.
    pub handlers: Vec<String>,
    /// Whether the latest check-in is within the 45-second health window.
    pub healthy: bool,
    /// Whether this Station currently holds the elected Headquarters lease.
    pub headquarters: bool,
    /// Unix timestamp of the last Station check-in.
    pub seen: f64,
    /// Station roles reported at check-in.
    pub roles: Vec<String>,
    /// Active sessions reported at check-in.
    pub running: usize,
    /// Mission execution capacity reported at check-in.
    pub capacity: usize,
    /// Operations visible to this user that the Station can host.
    pub operations: Vec<String>,
    /// Theaters the Station reaches; its own host is always the `local` Theater.
    pub theaters: Vec<BoardTheater>,
    /// The host's CPUs, load and memory as the Station last sampled them, with the sample
    /// time; absent from a Station that does not report them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ai6::Resources>,
    /// The configuration the Station runs: its roster revision, what it refused and which
    /// changed files wait for a restart; absent from a Station that does not report it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<ai6::ConfigStatus>,
    /// The latest report its Head of Station filed: a summary and typed findings; absent
    /// until one is filed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_of_station: Option<ai6::StationReport>,
    /// Each Operation's issue settings looser than the default, one line each.
    pub issues_weakened: Vec<String>,
}

/// One Theater a Station reaches, as the Station published it at check-in.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardTheater {
    /// Theater identifier.
    pub id: String,
    /// How the Station reaches it: `local`, `ssh` or `transport`.
    pub access: String,
    /// SSH host, for an `ssh` Theater.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The transport provider, for a `transport` Theater.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Theater labels.
    pub labels: BTreeMap<String, String>,
    /// What the Station's transport can do on the Theater: `terminal`, `run`, `files`,
    /// `forward`, and the host's `os` and `arch` when known. Absent from a Station that
    /// does not report it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ai6::Capabilities>,
    /// Where the harness may run: `auto`, `local` or `remote-hands`.
    pub agents: String,
    /// How the Theater reaches the provider's API: `direct`, `proxy`, `station` or `none`.
    pub egress: String,
    /// Harnesses installed on the Theater, as declared.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<String>,
    /// The directory a remote-hands Mission works in when its Operation names no path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Why the Station cannot reach the Theater now, when it cannot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// Settings looser than their defaults, and credentials found on the Theater outside
    /// AI6's leases.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub weakened: Vec<String>,
    /// What the Theater must have before Sessions run there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires: Option<ai6::Requirements>,
    /// What the Station's last probe found there: versions, free space, unmet requirements.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<ai6::ProbeReport>,
    /// The Mission an ephemeral Theater was made for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub made_for: Option<String>,
    /// Its CPUs, load and memory as last sampled, with the sample time; absent until a
    /// sample succeeds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ai6::Resources>,
    /// Sessions the Station is running there; omitted when none.
    #[serde(skip_serializing_if = "is_zero")]
    pub sessions: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// Mission instructions and non-secret launch metadata published by AI6.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LaunchManifest {
    /// What to show beside the Mission ID: a person's title, or an issue's title as data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Exact task instructions provided to the configured Agent.
    pub prompt: String,
    /// Configured AI6 Agent identity, if assigned.
    pub agent_id: Option<String>,
    /// Configured Agent role assigned to the Mission, if specified.
    pub role_id: Option<String>,
    /// Guatiao AgentHandler provider identifier.
    pub handler_provider: String,
    /// Guatiao terminal provider identifier.
    pub terminal_provider: String,
    /// Absolute working directory used for the operation.
    pub working_directory: String,
    /// Model selected by the Agent configuration, if specified.
    pub model: Option<String>,
    /// Reasoning effort selected by the Agent configuration, if specified.
    pub effort: Option<String>,
    /// Environment variable names only; values are never published.
    pub environment_keys: Vec<String>,
    /// The Station that supervises the Session.
    pub station_id: String,
    /// The Theater the harness runs in, when the Station knows it.
    pub theater_id: Option<String>,
    /// Whether the harness runs on the Theater (`field`) or on the Station, acting on the
    /// Theater through its transport (`remote-hands`).
    pub mode: String,
    /// Where the Mission's brief came from (`origin` `operator`, `source`, `sleeper`,
    /// `proposal` or `registry_drop`, with who asked), as the Station recorded it; absent for
    /// a Mission started directly on a Station.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

#[derive(Deserialize)]
struct GrantRequest {
    operation_id: String,
    mission_id: String,
    session_id: String,
    mode: String,
}

#[derive(Serialize)]
struct TokenResponse {
    token: String,
}

#[derive(Deserialize)]
struct ExchangeRequest {
    token: String,
}

#[derive(Debug, Deserialize)]
struct AfterQuery {
    after: Option<u64>,
}

async fn register_session(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(request): Json<RegisterSessionRequest>,
) -> Result<(StatusCode, Json<RegisterSessionResponse>), ApiError> {
    let token = bearer(&headers)?;
    let session_id = SessionId::new(request.session_id).map_err(|_| ApiError::bad_request())?;
    let operation_id =
        OperationId::new(request.operation_id).map_err(|_| ApiError::bad_request())?;
    let mission_id = MissionId::new(request.mission_id).map_err(|_| ApiError::bad_request())?;
    gateway
        .register_session(
            token,
            operation_id.clone(),
            mission_id.clone(),
            session_id.clone(),
        )
        .await?;
    if let Some(brief) = request.mission {
        gateway
            .publish_mission_brief(token, operation_id, mission_id, brief)
            .await?;
    }
    let next_sequence = gateway.next_sequence(token, &session_id).await?;
    let terminal_bytes = gateway.terminal_bytes(&session_id);
    Ok((
        StatusCode::CREATED,
        Json(RegisterSessionResponse {
            next_sequence,
            terminal_bytes,
        }),
    ))
}

#[derive(Serialize)]
struct RegisterSessionResponse {
    next_sequence: u64,
    /// The most terminal output one event of this Session may carry.
    terminal_bytes: usize,
}

async fn publish_lifecycle(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((session, sequence)): Path<(String, u64)>,
    Json(request): Json<LifecycleRequest>,
) -> Result<StatusCode, ApiError> {
    let state = SessionState::parse(&request.state).ok_or_else(ApiError::bad_request)?;
    if request
        .reason
        .as_deref()
        .is_some_and(|reason| !ai6_diptel::valid_reason(reason))
    {
        return Err(ApiError::bad_request());
    }
    gateway
        .publish_lifecycle(
            bearer(&headers)?,
            LifecycleEvent {
                operation_id: OperationId::new(request.operation_id)
                    .map_err(|_| ApiError::bad_request())?,
                mission_id: MissionId::new(request.mission_id)
                    .map_err(|_| ApiError::bad_request())?,
                session_id: SessionId::new(session).map_err(|_| ApiError::bad_request())?,
                state,
                reason: request.reason,
                sequence,
            },
        )
        .await?;
    Ok(StatusCode::ACCEPTED)
}

async fn publish_terminal_output(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((session, sequence)): Path<(String, u64)>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    gateway
        .publish_terminal_output(
            bearer(&headers)?,
            SessionId::new(session).map_err(|_| ApiError::bad_request())?,
            sequence,
            body.to_vec(),
        )
        .await?;
    Ok(StatusCode::ACCEPTED)
}

async fn ingest_event(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((session, sequence)): Path<(String, u64)>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    gateway
        .ingest(
            bearer(&headers)?,
            &SessionId::new(session).map_err(|_| ApiError::bad_request())?,
            sequence,
            body.to_vec(),
        )
        .await?;
    Ok(StatusCode::ACCEPTED)
}

async fn issue_attach_token(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(request): Json<GrantRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let mode = match request.mode.as_str() {
        "read" => AttachMode::Read,
        "write" => AttachMode::Write,
        _ => return Err(ApiError::bad_request()),
    };
    let token = gateway
        .issue_attach_token(
            bearer(&headers)?,
            AttachGrant {
                operation_id: OperationId::new(request.operation_id)
                    .map_err(|_| ApiError::bad_request())?,
                mission_id: MissionId::new(request.mission_id)
                    .map_err(|_| ApiError::bad_request())?,
                session_id: SessionId::new(request.session_id)
                    .map_err(|_| ApiError::bad_request())?,
                mode,
            },
        )
        .await?;
    Ok(Json(TokenResponse { token }))
}

async fn exchange_attach_token(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(request): Json<ExchangeRequest>,
) -> Result<Response, ApiError> {
    require_origin(&gateway, &headers)?;
    let ticket = gateway.exchange_attach_token(&request.token).await?;
    let cookie = format!(
        "ai6_attach={ticket}; Path=/v1/socket; HttpOnly; Secure; SameSite=Strict; Max-Age={}",
        gateway.config.attach_token_ttl.as_secs().max(1)
    );
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| ApiError::internal())?,
    );
    Ok(response)
}

/// How long an input poll is held: `wait_ms`, else the `controls` table's wait.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputPoll {
    wait_ms: Option<u64>,
}

async fn next_agent_input(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(poll): Query<InputPoll>,
) -> Result<Response, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    let table = &gateway.config.unex().controls;
    let wait = poll
        .wait_ms
        .map_or(table.poll_wait.0, Duration::from_millis);
    let wait = table
        .max_poll_wait
        .get()
        .map_or(wait, |most| wait.min(most));
    let Some(input) = gateway.receive_agent_input_within(wait).await else {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .map_err(|_| ApiError::internal());
    };
    let mut response = Response::new(Body::from(input.frame));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("application/vnd.ai6.guatiao"),
    );
    response.headers_mut().insert(
        "x-ai6-operation-id",
        HeaderValue::from_str(input.operation_id.as_str()).map_err(|_| ApiError::internal())?,
    );
    response.headers_mut().insert(
        "x-ai6-mission-id",
        HeaderValue::from_str(input.mission_id.as_str()).map_err(|_| ApiError::internal())?,
    );
    response.headers_mut().insert(
        "x-ai6-session-id",
        HeaderValue::from_str(input.session_id.as_str()).map_err(|_| ApiError::internal())?,
    );
    Ok(response)
}

async fn websocket_attach(
    State(gateway): State<Gateway>,
    Path(session): Path<String>,
    Query(after): Query<AfterQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    require_origin(&gateway, &headers)?;
    let cookie = headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(attach_cookie)
        .ok_or_else(ApiError::unauthorized)?;
    let session_id = SessionId::new(session).map_err(|_| ApiError::bad_request())?;
    let attachment = gateway
        .attach(&session_id, cookie, after.after.unwrap_or(0))
        .await?;
    let socket_origin = gateway.public_origin().to_owned();
    let max_write_buffer = gateway
        .config
        .limits
        .max_frame_bytes
        .saturating_mul(2)
        .max(gateway.config.limits.socket_buffer_bytes);
    Ok(upgrade
        .max_message_size(gateway.config.limits.max_frame_bytes)
        .max_frame_size(gateway.config.limits.max_frame_bytes)
        .max_write_buffer_size(max_write_buffer)
        .protocols(["ai6.guatiao"])
        .on_upgrade(move |socket| {
            serve_socket(socket, gateway, attachment, session_id, socket_origin)
        }))
}

async fn serve_socket(
    socket: WebSocket,
    gateway: Gateway,
    attachment: Attachment,
    session_id: SessionId,
    _expected_origin: String,
) {
    let Attachment {
        mode,
        replay,
        mut events,
        connection_id,
        expires,
        ..
    } = attachment;
    let (mut sender, mut receiver) = socket.split();
    for frame in replay {
        if sender.send(Message::Binary(frame.into())).await.is_err() {
            gateway
                .disconnect_connection(&session_id, &connection_id)
                .await;
            return;
        }
    }
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(expires)) => {
                let _ = sender.send(Message::Close(Some(CloseFrame {
                    code: 1008,
                    reason: "attachment credential expired".into(),
                }))).await;
                break;
            }
            message = receiver.next() => match message {
                Some(Ok(Message::Binary(frame))) if mode == AttachMode::Write => {
                    if gateway.submit_input(&session_id, &connection_id, frame.to_vec()).await.is_err() {
                        let _ = sender.send(close_policy()).await;
                        break;
                    }
                }
                Some(Ok(Message::Binary(_))) | Some(Ok(Message::Text(_))) => {
                    let _ = sender.send(close_policy()).await;
                    break;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(Message::Ping(payload))) => {
                    if sender.send(Message::Pong(payload)).await.is_err() { break; }
                }
                Some(Ok(Message::Pong(_))) => {}
            },
            event = events.recv() => match event {
                Ok(frame) => {
                    if sender.send(Message::Binary(frame.into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = sender.send(Message::Close(Some(CloseFrame {
                        code: 1013,
                        reason: "viewer fell behind; reconnect with a cursor".into(),
                    }))).await;
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
    gateway
        .disconnect_connection(&session_id, &connection_id)
        .await;
}

fn close_policy() -> Message {
    Message::Close(Some(CloseFrame {
        code: 1008,
        reason: "write authorization is required".into(),
    }))
}

fn require_origin(gateway: &Gateway, headers: &HeaderMap) -> Result<(), ApiError> {
    let origin = headers
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(ApiError::forbidden)?;
    let forwarded_proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok());
    if origin != gateway.public_origin() || forwarded_proto != Some("https") {
        return Err(ApiError::forbidden());
    }
    Ok(())
}

fn bearer(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .ok_or_else(ApiError::unauthorized)
}

fn attach_cookie(cookie: &str) -> Option<&str> {
    named_cookie(cookie, "ai6_attach")
}

fn named_cookie<'a>(cookie: &'a str, wanted: &str) -> Option<&'a str> {
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == wanted && !value.is_empty()).then_some(value)
    })
}

/// The cookie that resumes a sign-in; only `resume` and `logout` read it.
const SESSION_COOKIE: &str = "ai6_session";

/// A client speaking HTTP/2 may split `Cookie` over several headers.
fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| named_cookie(cookie, SESSION_COOKIE))
}

fn session_cookie_header(value: &str, max_age: u64) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={value}; Path=/v1/auth; HttpOnly; Secure; SameSite=Strict; \
         Max-Age={max_age}"
    ))
    .map_err(|_| ApiError::internal())
}

/// Tells the browser to drop the cookie.
fn ended_cookie() -> Result<HeaderValue, ApiError> {
    session_cookie_header("", 0)
}

struct ApiError {
    status: StatusCode,
    message: std::borrow::Cow<'static, str>,
    retry_after_seconds: Option<u64>,
}

impl ApiError {
    /// A 400 that says what was wrong with the request.
    fn invalid(message: String) -> Self {
        Self::refused(StatusCode::BAD_REQUEST, message)
    }

    /// A refusal with `status` that says why.
    fn refused(status: StatusCode, message: String) -> Self {
        Self {
            status,
            message: std::borrow::Cow::Owned(message),
            retry_after_seconds: None,
        }
    }

    fn payload_too_large() -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: std::borrow::Cow::Borrowed("request body is too large"),
            retry_after_seconds: None,
        }
    }

    fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: std::borrow::Cow::Borrowed("resource is not available"),
            retry_after_seconds: None,
        }
    }

    fn bad_request() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: std::borrow::Cow::Borrowed("request is invalid"),
            retry_after_seconds: None,
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: std::borrow::Cow::Borrowed("authentication is required"),
            retry_after_seconds: None,
        }
    }

    fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: std::borrow::Cow::Borrowed("request is not authorized"),
            retry_after_seconds: None,
        }
    }

    fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: std::borrow::Cow::Borrowed("gateway operation failed"),
            retry_after_seconds: None,
        }
    }

    fn unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: std::borrow::Cow::Borrowed("user access providers are unavailable"),
            retry_after_seconds: None,
        }
    }

    fn too_many_attempts(wait: Duration) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: std::borrow::Cow::Borrowed("too many login attempts; try again later"),
            retry_after_seconds: Some(wait.as_secs().max(1)),
        }
    }
}

impl From<GatewayError> for ApiError {
    fn from(error: GatewayError) -> Self {
        let status = match error {
            GatewayError::Unauthorized | GatewayError::InvalidCredential => {
                StatusCode::UNAUTHORIZED
            }
            GatewayError::Forbidden => StatusCode::FORBIDDEN,
            GatewayError::NotFound => StatusCode::NOT_FOUND,
            GatewayError::Conflict | GatewayError::Backpressure => StatusCode::CONFLICT,
            GatewayError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            GatewayError::InvalidFrame | GatewayError::InvalidConfiguration => {
                StatusCode::BAD_REQUEST
            }
        };
        Self {
            status,
            message: std::borrow::Cow::Borrowed("gateway request was refused"),
            retry_after_seconds: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, self.message).into_response();
        if let Some(seconds) = self.retry_after_seconds {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }
}

fn prune_expired(inner: &mut Inner) {
    let now = Instant::now();
    inner.attach_tokens.retain(|_, timed| timed.expires > now);
    inner
        .connection_tickets
        .retain(|_, timed| timed.expires > now);
    for session in inner.sessions.values_mut() {
        if session
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires <= now)
        {
            session.lease = None;
        }
    }
}

fn pending_credential_capacity(config: &GatewayConfig) -> usize {
    config
        .limits
        .max_sessions
        .saturating_mul(config.limits.max_clients_per_session)
        .clamp(16, 65_536)
}

fn client_slots_or_max(limits: GatewayLimits) -> usize {
    limits
        .max_sessions
        .saturating_mul(limits.max_clients_per_session)
}

/// Whether `left` and `right` are equal, in time that does not depend on where they differ.
/// A different length is refused at once: the length of a stored secret or digest is fixed by
/// its format and is not itself secret.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

fn validate_input_frame(session_id: &SessionId, frame: &[u8]) -> Result<u64, GatewayError> {
    let template = TerminalData::new(session_id.clone(), 1, TerminalDirection::Input, Vec::new())
        .map_err(|_| GatewayError::InvalidFrame)?;
    let resize_template = TerminalResize::new(session_id.clone(), 1, 80, 24)
        .map_err(|_| GatewayError::InvalidFrame)?;
    let (terminal_schema, _) =
        terminal_frames(&template).map_err(|_| GatewayError::InvalidFrame)?;
    let (resize_schema, _) =
        resize_frames(&resize_template).map_err(|_| GatewayError::InvalidFrame)?;
    let mut receiver = FrameReceiver::new();
    receiver
        .accept(&terminal_schema)
        .map_err(|_| GatewayError::InvalidFrame)?;
    receiver
        .accept(&resize_schema)
        .map_err(|_| GatewayError::InvalidFrame)?;
    match receiver
        .accept(frame)
        .map_err(|_| GatewayError::InvalidFrame)?
        .ok_or(GatewayError::InvalidFrame)?
    {
        IncomingFrame::Terminal(data)
            if data.session_id() == session_id
                && data.direction() == TerminalDirection::Input
                && data.sequence() > 0 =>
        {
            Ok(data.sequence())
        }
        IncomingFrame::Resize(resize)
            if resize.session_id() == session_id && resize.sequence() > 0 =>
        {
            Ok(resize.sequence())
        }
        IncomingFrame::Terminal(_) | IncomingFrame::Resize(_) => Err(GatewayError::Forbidden),
        IncomingFrame::Lifecycle(_)
        | IncomingFrame::Control(_)
        | IncomingFrame::Gap(_)
        | IncomingFrame::Closed(_) => Err(GatewayError::InvalidFrame),
    }
}

fn validate_lifecycle_value(
    value: &guatiao::Value,
    operation_id: &OperationId,
    mission_id: &MissionId,
    session_id: &SessionId,
    expected_sequence: u64,
) -> Result<(), GatewayError> {
    let map: &Map = value.try_as_ref().ok_or(GatewayError::InvalidFrame)?;
    let station = read_text(map, "operation_id")?;
    let mission = read_text(map, "mission_id")?;
    let session = read_text(map, "session_id")?;
    let state = read_text(map, "state")?;
    let sequence = read_sequence(map)?;
    if station != operation_id.as_str()
        || mission != mission_id.as_str()
        || session != session_id.as_str()
        || SessionState::parse(state).is_none()
        || sequence != expected_sequence
    {
        return Err(GatewayError::Forbidden);
    }
    Ok(())
}

/// The schema and value of a gap, when a sequence lies between `after` and `before`.
fn gap_between(
    session_id: &SessionId,
    after: u64,
    before: u64,
) -> Result<Option<[Vec<u8>; 2]>, GatewayError> {
    let Ok(gap) = ai6_diptel::TerminalGap::new(session_id.clone(), after, before) else {
        return Ok(None);
    };
    let (schema, value) = ai6_diptel::gap_frames(&gap).map_err(|_| GatewayError::InvalidFrame)?;
    Ok(Some([schema, value]))
}

fn validate_terminal_output(
    value: &guatiao::Value,
    session_id: &SessionId,
    expected_sequence: u64,
    max_frame_bytes: usize,
) -> Result<(), GatewayError> {
    let map: &Map = value.try_as_ref().ok_or(GatewayError::InvalidFrame)?;
    let session = read_text(map, "session_id")?;
    let direction = read_text(map, "direction")?;
    let sequence = read_sequence(map)?;
    let bytes: &[u8] = map
        .required("bytes")
        .map_err(|_| GatewayError::InvalidFrame)?
        .try_into()
        .map_err(|_| GatewayError::InvalidFrame)?;
    if session != session_id.as_str() || direction != "output" || sequence != expected_sequence {
        return Err(GatewayError::Forbidden);
    }
    if bytes.len() > max_frame_bytes {
        return Err(GatewayError::TooLarge);
    }
    Ok(())
}

fn read_text<'a>(map: &'a Map, key: &str) -> Result<&'a str, GatewayError> {
    <&str>::try_from(map.required(key).map_err(|_| GatewayError::InvalidFrame)?)
        .map_err(|_| GatewayError::InvalidFrame)
}

fn read_sequence(map: &Map) -> Result<u64, GatewayError> {
    u64::try_from(
        map.required("sequence")
            .map_err(|_| GatewayError::InvalidFrame)?,
    )
    .map_err(|_| GatewayError::InvalidFrame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_compare_equal_only_when_every_byte_and_the_length_match() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"station-token", b"station-token"));
        assert!(!constant_time_eq(b"station-token", b"station-tokeN"));
        assert!(!constant_time_eq(b"station-token", b"station-toke"));
        assert!(!constant_time_eq(b"station-toke", b"station-token"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    const PROXY_TEST_SECRET: &str = "proxy-secret-that-is-long-enough-for-tests";
    use ai6::{FileRegistry, RoleAuthorizer};
    use ai6_diptel::{
        LifecycleEvent, SessionState, TerminalData, TerminalDirection, TerminalResize,
        lifecycle_frames, resize_frames, terminal_frames,
    };
    use guatiao::library::ProviderError;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    fn test_limits() -> GatewayLimits {
        GatewayLimits {
            max_sessions: 4,
            max_clients_per_session: 3,
            scrollback_events: 2,
            max_frame_bytes: 1024,
            max_input_frames_per_second: 20,
            input_queue_capacity: 2,
            fanout_capacity: 2,
            ..GatewayLimits::default()
        }
    }

    #[test]
    fn reverse_proxy_access_can_be_configured_without_a_password_authenticator() {
        struct DirectoryStub;

        impl Directory for DirectoryStub {
            fn lookup(&self, _lookup: Lookup) -> Result<Option<Person>, ProviderError> {
                Ok(None)
            }
        }

        let access = AccessProviders::from_reverse_proxy(
            Arc::new(DirectoryStub),
            Arc::new(RoleAuthorizer::default()),
            HeaderName::from_static("x-forwarded-user"),
            "name",
        );

        assert!(access.authenticator.is_none());
        assert_eq!(
            access
                .proxy_identity
                .as_ref()
                .map(|proxy| (proxy.header.as_str(), proxy.lookup_by.as_str())),
            Some(("x-forwarded-user", "name"))
        );
    }

    fn gateway() -> Gateway {
        Gateway::new(
            GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                test_limits(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
        )
    }

    async fn register(gateway: &Gateway) -> (OperationId, MissionId, SessionId) {
        let station = OperationId::new("station-a").unwrap();
        let mission = MissionId::new("mission-a").unwrap();
        let session = SessionId::new("provider-session-a").unwrap();
        gateway
            .register_session(
                "m-token-that-is-long-enough-for-testing",
                station.clone(),
                mission.clone(),
                session.clone(),
            )
            .await
            .unwrap();
        (station, mission, session)
    }

    #[tokio::test]
    async fn board_missions_group_provider_sessions_by_station_and_mission() {
        let gateway = gateway();
        let (station, mission, _) = register(&gateway).await;
        gateway
            .register_session(
                "m-token-that-is-long-enough-for-testing",
                station,
                mission,
                SessionId::new("provider-session-b").unwrap(),
            )
            .await
            .unwrap();

        let missions = gateway.board_missions().await;

        assert_eq!(missions.len(), 1);
        assert_eq!(missions[0].operation_id, "station-a");
        assert_eq!(missions[0].mission_id, "mission-a");
        assert_eq!(missions[0].sessions.len(), 2);
    }

    async fn grant(gateway: &Gateway, mode: AttachMode) -> String {
        let (station, mission, session) = register(gateway).await;
        gateway
            .issue_attach_token(
                "m-token-that-is-long-enough-for-testing",
                AttachGrant {
                    operation_id: station,
                    mission_id: mission,
                    session_id: session,
                    mode,
                },
            )
            .await
            .unwrap()
    }

    fn output(session: &SessionId, sequence: u64) -> (Vec<u8>, Vec<u8>) {
        let event = TerminalData::new(
            session.clone(),
            sequence,
            TerminalDirection::Output,
            vec![0xff, sequence as u8],
        )
        .unwrap();
        terminal_frames(&event).unwrap()
    }

    #[tokio::test]
    async fn m_authentication_and_station_scoping_are_enforced() {
        let gateway = gateway();
        assert_eq!(
            gateway.authenticate_station("wrong").unwrap_err(),
            GatewayError::Unauthorized
        );
        let (_, _, session) = register(&gateway).await;
        let result = gateway
            .issue_attach_token(
                "m-token-that-is-long-enough-for-testing",
                AttachGrant {
                    operation_id: OperationId::new("other-station").unwrap(),
                    mission_id: MissionId::new("mission-a").unwrap(),
                    session_id: session,
                    mode: AttachMode::Read,
                },
            )
            .await;
        assert_eq!(result.unwrap_err(), GatewayError::Forbidden);
    }

    #[tokio::test]
    async fn the_board_shows_why_a_session_is_held_until_its_next_state() {
        let gateway = gateway();
        let (operation, mission, session) = register(&gateway).await;
        let event = |state, reason: Option<&str>, sequence| LifecycleEvent {
            operation_id: operation.clone(),
            mission_id: mission.clone(),
            session_id: session.clone(),
            state,
            reason: reason.map(str::to_owned),
            sequence,
        };
        let token = "m-token-that-is-long-enough-for-testing";
        gateway
            .publish_lifecycle(token, event(SessionState::Blocked, Some("green-ink"), 1))
            .await
            .unwrap();
        let board = gateway.board_sessions().await;
        assert_eq!(
            (board[0].state.as_deref(), board[0].reason.as_deref()),
            (Some("blocked"), Some("green-ink"))
        );
        gateway
            .publish_lifecycle(token, event(SessionState::Working, None, 2))
            .await
            .unwrap();
        assert_eq!(gateway.board_sessions().await[0].reason, None);
        assert!(
            gateway
                .publish_lifecycle(token, event(SessionState::Blocked, Some("Green Ink"), 3))
                .await
                .is_err(),
            "a reason is a name"
        );
    }

    #[tokio::test]
    async fn typed_m_publisher_encodes_failed_state_and_binary_terminal_output() {
        let gateway = gateway();
        let (station, mission, session) = register(&gateway).await;
        assert_eq!(gateway.board_sessions().await[0].updated_at, None);
        gateway
            .publish_lifecycle(
                "m-token-that-is-long-enough-for-testing",
                LifecycleEvent {
                    operation_id: station,
                    mission_id: mission,
                    session_id: session.clone(),
                    state: SessionState::Failed,
                    reason: None,
                    sequence: 1,
                },
            )
            .await
            .unwrap();
        let board = gateway.board_sessions().await;
        assert_eq!(board[0].state.as_deref(), Some("failed"));
        assert!(board[0].updated_at.is_some_and(|at| at > 0.0));
        gateway
            .publish_terminal_output(
                "m-token-that-is-long-enough-for-testing",
                session.clone(),
                2,
                vec![0xff, 0x00, 0x80],
            )
            .await
            .unwrap();

        let token = grant(&gateway, AttachMode::Read).await;
        let ticket = gateway.exchange_attach_token(&token).await.unwrap();
        let attachment = gateway.attach(&session, &ticket, 0).await.unwrap();
        assert_eq!(attachment.replay.len(), 4);
        let mut receiver = guatiao::value::wire::channel::Receiver::new();
        for frame in &attachment.replay {
            match receiver.accept(frame).unwrap() {
                guatiao::value::wire::channel::Received::Value { channel: 1, value } => {
                    let map: &Map = value.try_as_ref().unwrap();
                    assert_eq!(read_text(map, "state").unwrap(), "failed");
                    assert_eq!(read_text(map, "mission_id").unwrap(), "mission-a");
                }
                guatiao::value::wire::channel::Received::Value { channel: 2, value } => {
                    let map: &Map = value.try_as_ref().unwrap();
                    let bytes: &[u8] = map.required("bytes").unwrap().try_into().unwrap();
                    assert_eq!(bytes, [0xff, 0x00, 0x80]);
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn attach_tokens_are_single_use_and_write_lease_is_exclusive() {
        let gateway = gateway();
        let token = grant(&gateway, AttachMode::Write).await;
        let cookie = gateway.exchange_attach_token(&token).await.unwrap();
        assert_eq!(
            gateway.exchange_attach_token(&token).await.unwrap_err(),
            GatewayError::InvalidCredential
        );
        let session = SessionId::new("provider-session-a").unwrap();
        let attachment = gateway.attach(&session, &cookie, 0).await.unwrap();

        let second_token = grant(&gateway, AttachMode::Write).await;
        let second_cookie = gateway.exchange_attach_token(&second_token).await.unwrap();
        assert_eq!(
            gateway
                .attach(&session, &second_cookie, 0)
                .await
                .unwrap_err(),
            GatewayError::Conflict
        );
        gateway.disconnect(attachment).await;
        let next = grant(&gateway, AttachMode::Write).await;
        let next = gateway.exchange_attach_token(&next).await.unwrap();
        assert!(gateway.attach(&session, &next, 0).await.is_ok());
    }

    #[tokio::test]
    async fn credentials_and_write_leases_expire() {
        let gateway = Gateway::new(
            GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                test_limits(),
                Duration::from_millis(40),
                Duration::from_millis(10),
            )
            .unwrap(),
        );
        let (_, _, session) = register(&gateway).await;
        let token = grant(&gateway, AttachMode::Read).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            gateway.exchange_attach_token(&token).await.unwrap_err(),
            GatewayError::InvalidCredential
        );

        let old_token = grant(&gateway, AttachMode::Write).await;
        let old_ticket = gateway.exchange_attach_token(&old_token).await.unwrap();
        let old_writer = gateway.attach(&session, &old_ticket, 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(15)).await;
        let new_token = grant(&gateway, AttachMode::Write).await;
        let new_ticket = gateway.exchange_attach_token(&new_token).await.unwrap();
        let new_writer = gateway.attach(&session, &new_ticket, 0).await.unwrap();
        assert_eq!(new_writer.mode, AttachMode::Write);
        let stale_input =
            TerminalData::new(session.clone(), 1, TerminalDirection::Input, vec![b'x']).unwrap();
        let (_, stale_frame) = terminal_frames(&stale_input).unwrap();
        assert_eq!(
            gateway
                .submit_input(&session, &old_writer.connection_id, stale_frame)
                .await
                .unwrap_err(),
            GatewayError::Forbidden,
            "an expired writer cannot submit after another writer takes the lease"
        );
        gateway.disconnect(old_writer).await;
        gateway.disconnect(new_writer).await;
    }

    #[tokio::test]
    async fn attachment_ticket_is_session_scoped_and_read_is_default() {
        let gateway = gateway();
        let token = grant(&gateway, AttachMode::Read).await;
        let cookie = gateway.exchange_attach_token(&token).await.unwrap();
        assert_eq!(
            gateway
                .attach(&SessionId::new("other").unwrap(), &cookie, 0)
                .await
                .unwrap_err(),
            GatewayError::Forbidden
        );

        let token = grant(&gateway, AttachMode::Read).await;
        let cookie = gateway.exchange_attach_token(&token).await.unwrap();
        let attachment = gateway
            .attach(&SessionId::new("provider-session-a").unwrap(), &cookie, 0)
            .await
            .unwrap();
        assert_eq!(attachment.mode, AttachMode::Read);
        gateway.disconnect(attachment).await;
    }

    #[tokio::test]
    async fn scrollback_is_bounded_and_replay_honors_the_sequence_cursor() {
        let gateway = gateway();
        let (_, _, session) = register(&gateway).await;
        let (schema, first) = output(&session, 1);
        gateway
            .ingest(
                "m-token-that-is-long-enough-for-testing",
                &session,
                0,
                schema,
            )
            .await
            .unwrap();
        gateway
            .ingest(
                "m-token-that-is-long-enough-for-testing",
                &session,
                1,
                first,
            )
            .await
            .unwrap();
        let (_, second) = output(&session, 2);
        gateway
            .ingest(
                "m-token-that-is-long-enough-for-testing",
                &session,
                2,
                second,
            )
            .await
            .unwrap();
        let (_, third) = output(&session, 3);
        gateway
            .ingest(
                "m-token-that-is-long-enough-for-testing",
                &session,
                3,
                third,
            )
            .await
            .unwrap();

        let token = grant(&gateway, AttachMode::Read).await;
        let ticket = gateway.exchange_attach_token(&token).await.unwrap();
        let mut attachment = gateway.attach(&session, &ticket, 1).await.unwrap();
        assert_eq!(
            attachment.replay.len(),
            3,
            "current schema plus both retained events newer than the cursor"
        );
        assert!(matches!(
            attachment.events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        gateway.disconnect(attachment).await;
    }

    #[tokio::test]
    async fn what_a_station_is_told_fits_one_frame_at_any_sequence() {
        let gateway = gateway();
        let longest = SessionId::new("s".repeat(ai6_diptel::MAX_ID_BYTES)).unwrap();
        let token = "m-token-that-is-long-enough-for-testing";
        gateway
            .register_session(
                token,
                OperationId::new("station-a").unwrap(),
                MissionId::new("mission-a").unwrap(),
                longest.clone(),
            )
            .await
            .unwrap();
        let stated = gateway.terminal_bytes(&longest);
        assert!(stated > 0, "the test's frames hold more than a header");
        gateway
            .publish_terminal_output(token, longest.clone(), u64::MAX, vec![7; stated])
            .await
            .expect("as much as it states fits, at the longest sequence");
        // A frame's length field grows by at most this much, so this is past the frame.
        let over = stated + LENGTH_GROWTH + 1;
        assert!(matches!(
            gateway
                .publish_terminal_output(token, longest, u64::MAX, vec![7; over])
                .await,
            Err(GatewayError::TooLarge)
        ));
    }

    #[test]
    fn the_wire_limit_caps_what_a_station_is_told() {
        let gateway = Gateway::new(
            GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                GatewayLimits {
                    max_frame_bytes: 4 * ai6_diptel::MAX_TERMINAL_BYTES,
                    ..test_limits()
                },
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
        );
        assert_eq!(
            gateway.terminal_bytes(&SessionId::new("session").unwrap()),
            ai6_diptel::MAX_TERMINAL_BYTES
        );
    }

    /// The gaps in `frames`, as (after, before), and the sequences of its other values, in order.
    fn gaps_and_values(frames: &[Vec<u8>]) -> (Vec<(u64, u64)>, Vec<u64>) {
        let mut receiver = FrameReceiver::new();
        let (mut gaps, mut values) = (Vec::new(), Vec::new());
        for frame in frames {
            match receiver.accept(frame).unwrap() {
                Some(IncomingFrame::Gap(gap)) => {
                    gaps.push((gap.after(), gap.before()));
                    // Where it falls: nothing older follows.
                    values.push(gap.before() * 1000);
                }
                Some(IncomingFrame::Terminal(data)) => values.push(data.sequence()),
                _ => {}
            }
        }
        (gaps, values)
    }

    /// Publishes output at each of `sequences`, then attaches after `after`.
    async fn attach_after(
        gateway: &Gateway,
        session: &SessionId,
        sequences: &[u64],
        after: u64,
    ) -> Attachment {
        let token = "m-token-that-is-long-enough-for-testing";
        for sequence in sequences {
            gateway
                .publish_terminal_output(token, session.clone(), *sequence, vec![1])
                .await
                .unwrap();
        }
        let grant = grant(gateway, AttachMode::Read).await;
        let ticket = gateway.exchange_attach_token(&grant).await.unwrap();
        gateway.attach(session, &ticket, after).await.unwrap()
    }

    #[tokio::test]
    async fn a_viewer_whose_position_was_trimmed_is_told_of_the_gap() {
        let gateway = gateway();
        let (_, _, session) = register(&gateway).await;
        // The test's scrollback holds two events.
        let attachment = attach_after(&gateway, &session, &[1, 2, 3, 4, 5], 1).await;
        let (gaps, values) = gaps_and_values(&attachment.replay);
        assert_eq!(gaps, [(1, 4)], "2 and 3 are gone");
        assert_eq!(
            values,
            [4000, 4, 5],
            "the gap comes ahead of what follows it"
        );
        gateway.disconnect(attachment).await;

        let attachment = attach_after(&gateway, &session, &[], 0).await;
        assert_eq!(
            gaps_and_values(&attachment.replay).0,
            [(0, 4)],
            "from the start too"
        );
        gateway.disconnect(attachment).await;

        let attachment = attach_after(&gateway, &session, &[], 3).await;
        assert_eq!(
            gaps_and_values(&attachment.replay).0,
            [],
            "nothing it lacks is gone"
        );
        gateway.disconnect(attachment).await;

        let attachment = attach_after(&gateway, &session, &[], 5).await;
        let (gaps, values) = gaps_and_values(&attachment.replay);
        assert_eq!(
            (gaps, values),
            (Vec::new(), Vec::new()),
            "it has everything"
        );
        gateway.disconnect(attachment).await;
    }

    #[tokio::test]
    async fn a_hole_in_the_sequence_is_a_gap_on_replay_and_live() {
        // Room for a gap's two frames and the one after them in a viewer's queue.
        let gateway = Gateway::new(
            GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                GatewayLimits {
                    fanout_capacity: 8,
                    ..test_limits()
                },
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
        );
        let (_, _, session) = register(&gateway).await;
        let mut attachment = attach_after(&gateway, &session, &[1, 4], 0).await;
        let (gaps, values) = gaps_and_values(&attachment.replay);
        assert_eq!(gaps, [(1, 4)], "the Station dropped 2 and 3");
        assert_eq!(values, [1, 4000, 4]);

        let token = "m-token-that-is-long-enough-for-testing";
        gateway
            .publish_terminal_output(token, session.clone(), 7, vec![1])
            .await
            .unwrap();
        let mut live = attachment.replay.clone();
        while let Ok(frame) = attachment.events.try_recv() {
            live.push(frame);
        }
        assert_eq!(
            gaps_and_values(&live).0,
            [(1, 4), (4, 7)],
            "a viewer already attached hears of it before the frame that follows it"
        );
        gateway.disconnect(attachment).await;
    }

    #[tokio::test]
    async fn a_gateway_that_restarted_tells_a_new_viewer_and_spares_one_that_has_it_all() {
        let gateway = gateway();
        let (_, _, session) = register(&gateway).await;
        // A Station's first event after this gateway restarted, its sequence going on from 40.
        let attachment = attach_after(&gateway, &session, &[41], 0).await;
        assert_eq!(gaps_and_values(&attachment.replay).0, [(0, 41)]);
        gateway.disconnect(attachment).await;
        let attachment = attach_after(&gateway, &session, &[], 40).await;
        assert_eq!(gaps_and_values(&attachment.replay).0, []);
        gateway.disconnect(attachment).await;
    }

    #[tokio::test]
    async fn slow_viewers_are_bounded_and_report_lag() {
        let gateway = gateway();
        let (_, _, session) = register(&gateway).await;
        let (schema, value) = output(&session, 1);
        gateway
            .ingest(
                "m-token-that-is-long-enough-for-testing",
                &session,
                0,
                schema,
            )
            .await
            .unwrap();
        let token = grant(&gateway, AttachMode::Read).await;
        let ticket = gateway.exchange_attach_token(&token).await.unwrap();
        let mut attachment = gateway.attach(&session, &ticket, 0).await.unwrap();
        for sequence in 1..=3 {
            let (_, frame) = output(&session, sequence);
            gateway
                .ingest(
                    "m-token-that-is-long-enough-for-testing",
                    &session,
                    sequence,
                    frame,
                )
                .await
                .unwrap();
        }
        assert!(matches!(
            attachment.events.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
        gateway.disconnect(attachment).await;
        assert!(!value.is_empty());
    }

    #[tokio::test]
    async fn only_the_write_lease_can_submit_valid_input_to_m() {
        let gateway = gateway();
        let (_, _, session) = register(&gateway).await;
        let reader_token = grant(&gateway, AttachMode::Read).await;
        let reader_ticket = gateway.exchange_attach_token(&reader_token).await.unwrap();
        let reader = gateway.attach(&session, &reader_ticket, 0).await.unwrap();

        let input = TerminalData::new(
            session.clone(),
            1,
            TerminalDirection::Input,
            vec![0xff, 0x00],
        )
        .unwrap();
        let (_, input_frame) = terminal_frames(&input).unwrap();
        let resize = TerminalResize::new(session.clone(), 2, 120, 40).unwrap();
        let (_, resize_frame) = resize_frames(&resize).unwrap();
        assert_eq!(
            gateway
                .submit_input(&session, &reader.connection_id, input_frame.clone())
                .await
                .unwrap_err(),
            GatewayError::Forbidden
        );
        assert_eq!(
            gateway
                .submit_input(&session, &reader.connection_id, resize_frame.clone())
                .await
                .unwrap_err(),
            GatewayError::Forbidden
        );
        gateway.disconnect(reader).await;

        let writer_token = grant(&gateway, AttachMode::Write).await;
        let writer_ticket = gateway.exchange_attach_token(&writer_token).await.unwrap();
        let writer = gateway.attach(&session, &writer_ticket, 0).await.unwrap();
        gateway
            .submit_input(&session, &writer.connection_id, input_frame)
            .await
            .unwrap();
        gateway
            .submit_input(&session, &writer.connection_id, resize_frame)
            .await
            .unwrap();
        let queued = gateway.receive_agent_input().await.unwrap();
        assert_eq!(queued.session_id, session);
        assert!(!queued.frame.is_empty());
        assert!(format!("{queued:?}").contains("[REDACTED;"));
        assert!(!format!("{queued:?}").contains("ff00"));
        let queued_resize = gateway.receive_agent_input().await.unwrap();
        assert_eq!(queued_resize.session_id, session);
        assert!(!queued_resize.frame.is_empty());

        let stale_resize =
            TerminalResize::new(queued_resize.session_id.clone(), 1, 80, 24).unwrap();
        let (_, stale_resize_frame) = resize_frames(&stale_resize).unwrap();
        assert_eq!(
            gateway
                .submit_input(
                    &queued_resize.session_id,
                    &writer.connection_id,
                    stale_resize_frame
                )
                .await
                .unwrap_err(),
            GatewayError::Conflict
        );

        let duplicate = TerminalData::new(
            SessionId::new("provider-session-a").unwrap(),
            1,
            TerminalDirection::Input,
            vec![0xff, 0x00],
        )
        .unwrap();
        let (_, duplicate_frame) = terminal_frames(&duplicate).unwrap();
        assert_eq!(
            gateway
                .submit_input(
                    &SessionId::new("provider-session-a").unwrap(),
                    &writer.connection_id,
                    duplicate_frame
                )
                .await
                .unwrap_err(),
            GatewayError::Conflict
        );

        let (_, output_frame) = output(&queued.session_id, 3);
        assert_eq!(
            gateway
                .submit_input(&session, &writer.connection_id, output_frame)
                .await
                .unwrap_err(),
            GatewayError::Forbidden
        );
        gateway.disconnect(writer).await;
    }

    #[tokio::test]
    async fn http_m_publisher_accepts_lifecycle_and_binary_output() {
        let gateway = gateway();
        let observer = gateway.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, gateway.router()).await });
        let base = format!("http://{address}");
        let client = reqwest::Client::new();
        let token = "m-token-that-is-long-enough-for-testing";

        let registered = client
            .post(format!("{base}/v1/station/sessions"))
            .bearer_auth(token)
            .json(&serde_json::json!({
                "operation_id": "station-a",
                "mission_id": "mission-a",
                "session_id": "provider-session-a",
                "mission": {
                    "prompt": "Inspect this change",
                    "agent_id": "analyst",
                    "handler_provider": "claude",
                    "terminal_provider": "pty",
                    "working_directory": "C:/stations/a",
                    "model": "sonnet",
                    "effort": "high",
                    "environment_keys": ["API_TOKEN"],
                    "station_id": "station-a",
                    "theater_id": "local",
                    "mode": "field"
                }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(registered.status(), StatusCode::CREATED);
        assert_eq!(
            registered.json::<serde_json::Value>().await.unwrap()["next_sequence"],
            1
        );
        let mission = observer.board_missions().await.remove(0);
        let brief = mission.brief.unwrap();
        assert_eq!(brief.prompt, "Inspect this change");
        assert_eq!(brief.environment_keys, ["API_TOKEN"]);
        assert_eq!(brief.station_id, "station-a");

        let lifecycle = client
            .post(format!(
                "{base}/v1/station/sessions/provider-session-a/lifecycle/1"
            ))
            .bearer_auth(token)
            .json(&serde_json::json!({
                "operation_id": "station-a",
                "mission_id": "mission-a",
                "state": "failed"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(lifecycle.status(), StatusCode::ACCEPTED);

        let board = client.get(format!("{base}/v1/board")).send().await.unwrap();
        assert_eq!(board.status(), StatusCode::UNAUTHORIZED);

        let terminal = client
            .post(format!(
                "{base}/v1/station/sessions/provider-session-a/terminal/2"
            ))
            .bearer_auth(token)
            .body(vec![0xff, 0x00, 0x80])
            .send()
            .await
            .unwrap();
        assert_eq!(terminal.status(), StatusCode::ACCEPTED);

        let resumed = client
            .post(format!("{base}/v1/station/sessions"))
            .bearer_auth(token)
            .json(&serde_json::json!({
                "operation_id": "station-a",
                "mission_id": "mission-a",
                "session_id": "provider-session-a"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resumed.status(), StatusCode::CREATED);
        assert_eq!(
            resumed.json::<serde_json::Value>().await.unwrap()["next_sequence"],
            3
        );

        // A Session a provider limit paused is a board state of its own.
        let parked = client
            .post(format!(
                "{base}/v1/station/sessions/provider-session-a/lifecycle/3"
            ))
            .bearer_auth(token)
            .json(&serde_json::json!({
                "operation_id": "station-a",
                "mission_id": "mission-a",
                "state": "parked"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(parked.status(), StatusCode::ACCEPTED);
        let session = observer.board_missions().await.remove(0).sessions.remove(0);
        assert_eq!(session.state.as_deref(), Some("parked"));
        let unknown = client
            .post(format!(
                "{base}/v1/station/sessions/provider-session-a/lifecycle/4"
            ))
            .bearer_auth(token)
            .json(&serde_json::json!({
                "operation_id": "station-a",
                "mission_id": "mission-a",
                "state": "paused"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);

        let denied = client
            .post(format!(
                "{base}/v1/station/sessions/provider-session-a/lifecycle/4"
            ))
            .json(&serde_json::json!({
                "operation_id": "station-a",
                "mission_id": "mission-a",
                "state": "done"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        server.abort();
    }

    struct OneAccount;

    impl Authenticator for OneAccount {
        fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
            Ok(
                if attempt.name.as_deref() == Some("alice")
                    && attempt.secret.as_deref() == Some("right")
                {
                    Verdict::Valid(Lookup {
                        by: "name".to_owned(),
                        value: "alice".to_owned(),
                    })
                } else {
                    Verdict::Invalid
                },
            )
        }
    }

    impl Directory for OneAccount {
        fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
            Ok((lookup.value == "alice").then(|| Person {
                id: "alice-id".to_owned(),
                name: "alice".to_owned(),
                ..Person::default()
            }))
        }
    }

    async fn login_gateway(
        limits: LoginLimits,
    ) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
        let config = GatewayConfig::new(
            "https://localhost",
            "m-token-that-is-long-enough-for-testing",
            test_limits(),
            Duration::from_secs(30),
            Duration::from_secs(30),
        )
        .unwrap()
        .with_login_limits(limits);
        let directory = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(directory.path());
        let gateway = Gateway::with_access(
            config,
            Arc::new(registry),
            AccessProviders::new(
                Arc::new(OneAccount),
                Arc::new(OneAccount),
                Arc::new(RoleAuthorizer::default()),
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                gateway
                    .router()
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        (base, server, directory)
    }

    async fn try_login(
        client: &reqwest::Client,
        base: &str,
        name: &str,
        secret: &str,
        client_address: Option<&str>,
    ) -> reqwest::Response {
        let mut request = client
            .post(format!("{base}/v1/auth/login"))
            .json(&serde_json::json!({"name": name, "secret": secret}));
        if let Some(address) = client_address {
            request = request.header("x-forwarded-for", address);
        }
        request.send().await.unwrap()
    }

    /// An authenticator that reads the request and takes no name or secret.
    struct ByRequest;

    impl Authenticator for ByRequest {
        fn authenticate(&self, _: Attempt) -> Result<Verdict, ProviderError> {
            Ok(Verdict::Invalid)
        }

        fn describe(&self) -> Result<ai6::Description, ProviderError> {
            Ok(ai6::Description {
                takes: Some(ai6::directorates::m::access::takes::REQUEST.to_owned()),
                ..ai6::Description::default()
            })
        }
    }

    #[tokio::test]
    async fn auth_methods_say_what_login_accepts_and_need_no_token() {
        let header = || HeaderName::from_static("x-forwarded-user");
        let provided = |access: AccessProviders| async move {
            let config = GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                test_limits(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap();
            let directory = tempfile::tempdir().unwrap();
            let registry = Arc::new(FileRegistry::open(directory.path()));
            let gateway = Gateway::with_access(config, registry, access);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let _ = axum::serve(listener, gateway.router()).await;
            });
            let answer: serde_json::Value = reqwest::get(format!("{base}/v1/auth/methods"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            server.abort();
            answer["methods"].clone()
        };
        let authorizer = || Arc::new(RoleAuthorizer::default());
        assert_eq!(
            provided(AccessProviders::new(
                Arc::new(OneAccount),
                Arc::new(OneAccount),
                authorizer(),
            ))
            .await,
            serde_json::json!(["password"])
        );
        assert_eq!(
            provided(
                AccessProviders::new(Arc::new(OneAccount), Arc::new(OneAccount), authorizer())
                    .with_proxy_identity(header(), "name")
            )
            .await,
            serde_json::json!(["password", "proxy"])
        );
        assert_eq!(
            provided(AccessProviders::from_reverse_proxy(
                Arc::new(OneAccount),
                authorizer(),
                header(),
                "name",
            ))
            .await,
            serde_json::json!(["proxy"])
        );
        assert_eq!(
            provided(AccessProviders::new(
                Arc::new(ByRequest),
                Arc::new(OneAccount),
                authorizer(),
            ))
            .await,
            serde_json::json!([])
        );
    }

    /// Alice, until `gone` is set: the directory answers for her no longer.
    struct Leaving {
        gone: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Authenticator for Leaving {
        fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
            OneAccount.authenticate(attempt)
        }
    }

    impl Directory for Leaving {
        fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
            if lookup.value == "bob" {
                return Ok(Some(Person {
                    id: "bob-id".to_owned(),
                    name: "bob".to_owned(),
                    ..Person::default()
                }));
            }
            let known = ["alice", "alice-id"].contains(&lookup.value.as_str());
            Ok(
                (known && !self.gone.load(std::sync::atomic::Ordering::SeqCst)).then(|| Person {
                    id: "alice-id".to_owned(),
                    name: "alice".to_owned(),
                    ..Person::default()
                }),
            )
        }
    }

    /// A gateway under `change`d `[gateway]` limits, for what signing in and renewing do.
    struct Signing {
        base: String,
        registry: FileRegistry,
        gone: Arc<std::sync::atomic::AtomicBool>,
        server: tokio::task::JoinHandle<()>,
        _directory: tempfile::TempDir,
    }

    impl Drop for Signing {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    #[test]
    fn what_a_gateway_accepts_of_a_mission_and_an_audit_page_are_settings() {
        let text = ai6::TextLimits::default();
        assert!(mission_text_fits(
            &"t".repeat(512),
            &"p".repeat(65_536),
            &text
        ));
        assert!(!mission_text_fits(&"t".repeat(513), "p", &text));
        assert!(!mission_text_fits("t", &"p".repeat(65_537), &text));
        assert!(!mission_text_fits(" ", "p", &text));
        let mut wider = text.clone();
        wider.mission_title_chars = ai6::Limit::UNBOUNDED;
        wider.mission_prompt_bytes = ai6::Limit::at(1);
        assert!(mission_text_fits(&"t".repeat(5_000), "p", &wider));
        assert!(!mission_text_fits("t", "pp", &wider));

        let mut pages = ai6::UnexTimelineLimits::default();
        assert_eq!(audit_page(None, &pages), Some(100));
        assert_eq!(audit_page(Some(500), &pages), Some(500));
        assert_eq!(audit_page(Some(501), &pages), None);
        assert_eq!(audit_page(Some(0), &pages), None);
        pages.audit_page = 7;
        pages.max_audit_page = ai6::Limit::UNBOUNDED;
        assert_eq!(audit_page(None, &pages), Some(7));
        assert_eq!(audit_page(Some(9_000), &pages), Some(9_000));
    }

    #[test]
    fn the_gateway_s_own_limits_default_to_the_record_s() {
        let limits = GatewayLimits::default();
        assert_eq!(limits.max_sessions, 16);
        assert_eq!(limits.max_clients_per_session, 12);
        assert_eq!(limits.scrollback_events, 64);
        assert_eq!(limits.max_frame_bytes, 256 * 1024);
        assert_eq!(limits.max_input_frames_per_second, 40);
        assert_eq!(limits.input_queue_capacity, 64);
        assert_eq!(limits.fanout_capacity, 8);
        assert_eq!(
            limits.buffer_budget_bytes,
            ai6::Limit::at(512 * 1024 * 1024)
        );
        assert_eq!(limits.client_slots, ai6::Limit::at(100_000));
        assert_eq!(limits.socket_buffer_bytes, 256 * 1024);
        assert_eq!(limits.station_token_min_bytes, 32);
        assert_eq!(limits.origin_bytes, ai6::Limit::at(2048));
        let login = LoginLimits::default();
        assert_eq!(login.name_bytes, 64);
        assert_eq!(login.forget_every, 256);
        assert_eq!(login.address_window, Duration::from_secs(60));
        assert_eq!(login.name_window, Duration::from_secs(600));
    }

    impl Signing {
        async fn new(change: impl FnOnce(&mut ai6::UnexGatewayLimits)) -> Self {
            Self::built(change, LoginLimits::default(), |providers| providers).await
        }

        async fn built(
            change: impl FnOnce(&mut ai6::UnexGatewayLimits),
            login: LoginLimits,
            providers: impl FnOnce(AccessProviders) -> AccessProviders,
        ) -> Self {
            let mut limits = ai6::UnexLimits::default();
            change(&mut limits.gateway);
            let config = GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                test_limits(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap()
            .with_unex_limits(limits)
            .with_login_limits(login);
            let directory = tempfile::tempdir().unwrap();
            let registry = FileRegistry::open(directory.path());
            let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let people = Arc::new(Leaving { gone: gone.clone() });
            let gateway = Gateway::with_access(
                config,
                Arc::new(registry.clone()),
                providers(AccessProviders::new(
                    people.clone(),
                    people,
                    Arc::new(RoleAuthorizer::default()),
                )),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let _ = axum::serve(
                    listener,
                    gateway
                        .router()
                        .into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await;
            });
            Self {
                base,
                registry,
                gone,
                server,
                _directory: directory,
            }
        }

        /// Signs in as Alice, naming the lifetime when `ttl_seconds` does.
        async fn login(&self, ttl_seconds: Option<u64>) -> reqwest::Response {
            let mut body = serde_json::json!({"name": "alice", "secret": "right"});
            if let Some(ttl_seconds) = ttl_seconds {
                body["ttl_seconds"] = ttl_seconds.into();
            }
            reqwest::Client::new()
                .post(format!("{}/v1/auth/login", self.base))
                .json(&body)
                .send()
                .await
                .unwrap()
        }

        async fn renew(&self, token: &str) -> reqwest::Response {
            reqwest::Client::new()
                .post(format!("{}/v1/auth/renew", self.base))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
        }

        /// A token of Alice's with these times, returning its value.
        fn token(&self, issued_at: f64, expires_at: f64) -> String {
            let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
            let record = AccessTokenRecord::try_new(
                Uuid::new_v4().to_string(),
                access_token_digest(&token),
                "alice-id",
                issued_at,
                expires_at,
                None,
            )
            .unwrap();
            self.registry.store_access_token(&record).unwrap();
            token
        }

        async fn session(&self, token: &str) -> reqwest::Response {
            reqwest::Client::new()
                .get(format!("{}/v1/auth/session", self.base))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
        }

        /// A POST as the app's own page sends one.
        fn page_post(&self, path: &str) -> reqwest::RequestBuilder {
            reqwest::Client::new()
                .post(format!("{}{path}", self.base))
                .header(ORIGIN, "https://localhost")
                .header("x-forwarded-proto", "https")
        }

        /// Signs in as Alice from the app's page, returning the answer's body and the cookie as
        /// a browser would send it back.
        async fn web_login(&self) -> (serde_json::Value, String) {
            let response = self
                .page_post("/v1/auth/login")
                .json(&serde_json::json!({"name": "alice", "secret": "right"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
            let cookie = set_cookie_of(&response)
                .expect("a web sign-in gets a cookie")
                .split(';')
                .next()
                .unwrap()
                .to_owned();
            (response.json().await.unwrap(), cookie)
        }

        async fn resume(&self, cookie: &str) -> reqwest::Response {
            self.page_post("/v1/auth/resume")
                .header(COOKIE, cookie)
                .send()
                .await
                .unwrap()
        }

        /// A session cookie of Alice's with these times, returning it as a browser sends it.
        fn cookie(&self, issued_at: f64, expires_at: f64) -> String {
            let value = new_secret();
            let record = AccessTokenRecord::try_new(
                Uuid::new_v4().to_string(),
                session_cookie_digest(&value),
                "alice-id",
                issued_at,
                expires_at,
                None,
            )
            .unwrap()
            .into_session_cookie();
            self.registry.store_access_token(&record).unwrap();
            format!("{SESSION_COOKIE}={value}")
        }

        /// The record of a token value, while it stands.
        fn record_of(&self, token: &str) -> Option<AccessTokenRecord> {
            self.registry
                .find_access_token(&access_token_digest(token))
                .unwrap()
        }
    }

    fn set_cookie_of(response: &reqwest::Response) -> Option<String> {
        response
            .headers()
            .get(SET_COOKIE)
            .map(|value| value.to_str().unwrap().to_owned())
    }

    /// Says the response tells the browser to drop the session cookie.
    fn assert_clears_cookie(response: &reqwest::Response) {
        let set = set_cookie_of(response).expect("the cookie is cleared");
        assert!(set.starts_with("ai6_session=;"), "{set}");
        assert!(set.contains("Path=/v1/auth"), "{set}");
        assert!(set.contains("Max-Age=0"), "{set}");
    }

    async fn body_of(response: reqwest::Response) -> serde_json::Value {
        assert_eq!(response.status(), StatusCode::OK);
        response.json().await.unwrap()
    }

    fn time_in(body: &serde_json::Value, key: &str) -> f64 {
        body[key]
            .as_f64()
            .unwrap_or_else(|| panic!("{key} in {body}"))
    }

    #[tokio::test]
    async fn renewal_keeps_the_same_token_up_to_the_cap() {
        let signing = Signing::new(|gateway| {
            gateway.access_token_ttl = ai6::Seconds::new(3_600);
            gateway.session_max = TimeLimit::secs(7_200);
        })
        .await;
        let before = unix_timestamp().ok().unwrap();

        // Signing in says when the token ends and when renewing does.
        let signed_in: serde_json::Value = signing.login(None).await.json().await.unwrap();
        let expires_at = time_in(&signed_in, "expires_at");
        assert!(expires_at >= before + 3_600.0, "{signed_in}");
        assert!(
            (time_in(&signed_in, "expires_in") - 3_600.0).abs() < 1e-3,
            "{signed_in}"
        );
        assert!(
            (time_in(&signed_in, "ends_at") - (expires_at + 3_600.0)).abs() < 1e-3,
            "{signed_in}"
        );

        // A renewal gives a whole lifetime from now, with the same token and no secret.
        let token = signing.token(before - 10.0, before + 120.0);
        let id = body_of(signing.session(&token).await).await["token_id"].clone();
        let renewed = body_of(signing.renew(&token).await).await;
        assert_eq!(renewed["token_id"], id);
        assert!(renewed.get("token").is_none(), "{renewed}");
        let renewed_until = time_in(&renewed, "expires_at");
        assert!(renewed_until >= before + 3_600.0, "{renewed}");
        let left = time_in(&renewed, "expires_in");
        assert!((3_590.0..=3_600.0).contains(&left), "{renewed}");
        assert!(
            (time_in(&renewed, "ends_at") - (before - 10.0 + 7_200.0)).abs() < 1e-3,
            "{renewed}"
        );
        let session = body_of(signing.session(&token).await).await;
        assert_eq!(session["token_id"], id);
        assert!((time_in(&session, "expires_at") - renewed_until).abs() < 1e-3);

        // Near the cap it stops there, and again it stays there.
        let issued_at = before - 5_000.0;
        let token = signing.token(issued_at, before + 120.0);
        for _ in 0..2 {
            let capped = body_of(signing.renew(&token).await).await;
            assert!(
                (time_in(&capped, "expires_at") - (issued_at + 7_200.0)).abs() < 1e-3,
                "{capped}"
            );
            assert!(
                (time_in(&capped, "ends_at") - (issued_at + 7_200.0)).abs() < 1e-3,
                "{capped}"
            );
        }

        // A browser's renewal must come from the public origin, like its sign-in.
        let foreign = reqwest::Client::new()
            .post(format!("{}/v1/auth/renew", signing.base))
            .bearer_auth(&token)
            .header(ORIGIN, "https://elsewhere.example")
            .header("x-forwarded-proto", "https")
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        let own = reqwest::Client::new()
            .post(format!("{}/v1/auth/renew", signing.base))
            .bearer_auth(&token)
            .header(ORIGIN, "https://localhost")
            .header("x-forwarded-proto", "https")
            .send()
            .await
            .unwrap();
        assert_eq!(own.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn session_max_turns_renewal_off_or_lets_it_run_on() {
        let off = Signing::new(|gateway| gateway.session_max = TimeLimit::secs(0)).await;
        let signed_in: serde_json::Value = off.login(None).await.json().await.unwrap();
        let expires_at = time_in(&signed_in, "expires_at");
        assert!(
            (time_in(&signed_in, "ends_at") - expires_at).abs() < 1e-3,
            "it ends when the token does: {signed_in}"
        );
        let token = signed_in["token"].as_str().unwrap();
        let renewed = body_of(off.renew(token).await).await;
        assert!((time_in(&renewed, "expires_at") - expires_at).abs() < 1e-3);

        let unbound = Signing::new(|gateway| gateway.session_max = TimeLimit::UNBOUNDED).await;
        let now = unix_timestamp().ok().unwrap();
        let token = unbound.token(now - 1_000_000.0, now + 120.0);
        let renewed = body_of(unbound.renew(&token).await).await;
        assert!(renewed["ends_at"].is_null(), "{renewed}");
        assert!(
            time_in(&renewed, "expires_at") >= now + 3_600.0,
            "{renewed}"
        );
    }

    #[tokio::test]
    async fn a_requested_lifetime_is_bound_by_the_setting_and_has_no_floor() {
        let signing = Signing::new(|gateway| {
            gateway.access_token_max_ttl = TimeLimit::secs(100);
            gateway.access_token_ttl = ai6::Seconds::new(50);
        })
        .await;
        let before = unix_timestamp().ok().unwrap();
        let default: serde_json::Value = signing.login(None).await.json().await.unwrap();
        assert!(
            time_in(&default, "expires_at") < before + 100.0,
            "{default}"
        );
        assert_eq!(signing.login(Some(1)).await.status(), StatusCode::CREATED);
        assert_eq!(signing.login(Some(100)).await.status(), StatusCode::CREATED);
        assert_eq!(
            signing.login(Some(101)).await.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            signing.login(Some(0)).await.status(),
            StatusCode::BAD_REQUEST
        );
        let issued = reqwest::Client::new()
            .post(format!("{}/v1/station/access-tokens", signing.base))
            .bearer_auth("m-token-that-is-long-enough-for-testing")
            .json(&serde_json::json!({"person_id": "alice-id", "ttl_seconds": 101}))
            .send()
            .await
            .unwrap();
        assert_eq!(issued.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_revoked_or_expired_token_cannot_renew() {
        let signing = Signing::new(|_| {}).await;
        let now = unix_timestamp().ok().unwrap();
        let expired = signing.token(now - 7_200.0, now - 3_600.0);
        assert_eq!(
            signing.renew(&expired).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            signing.session(&expired).await.status(),
            StatusCode::UNAUTHORIZED,
            "and it is still over"
        );

        let signed_in: serde_json::Value = signing.login(None).await.json().await.unwrap();
        let token = signed_in["token"].as_str().unwrap();
        assert_eq!(signing.renew(token).await.status(), StatusCode::OK);
        let signed_out = reqwest::Client::new()
            .post(format!("{}/v1/auth/logout", signing.base))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(signed_out.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            signing.renew(token).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            signing.session(token).await.status(),
            StatusCode::UNAUTHORIZED,
            "and it stays revoked"
        );

        assert_eq!(
            signing.renew("made-up").await.status(),
            StatusCode::UNAUTHORIZED
        );
        let no_token = reqwest::Client::new()
            .post(format!("{}/v1/auth/renew", signing.base))
            .send()
            .await
            .unwrap();
        assert_eq!(no_token.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_person_gone_from_the_directory_cannot_renew() {
        let signing = Signing::new(|_| {}).await;
        let now = unix_timestamp().ok().unwrap();
        let first = now + 600.0;
        let token = signing.token(now - 10.0, first);
        assert_eq!(signing.renew(&token).await.status(), StatusCode::OK);
        let held = time_in(&body_of(signing.session(&token).await).await, "expires_at");

        signing
            .gone
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            signing.renew(&token).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let unmoved = time_in(&body_of(signing.session(&token).await).await, "expires_at");
        assert!((unmoved - held).abs() < 1e-3, "nothing moved");
    }

    #[tokio::test]
    async fn a_web_login_sets_exactly_the_session_cookie_and_keeps_only_its_digest() {
        let signing = Signing::new(|gateway| gateway.session_max = TimeLimit::secs(7_200)).await;
        let response = signing
            .page_post("/v1/auth/login")
            .json(&serde_json::json!({"name": "alice", "secret": "right"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let set = set_cookie_of(&response).unwrap();
        let mut parts = set.split("; ");
        let (name, value) = parts.next().unwrap().split_once('=').unwrap();
        assert_eq!(name, "ai6_session");
        assert_eq!(value.len(), 64, "{set}");
        assert_eq!(
            parts.collect::<Vec<_>>(),
            [
                "Path=/v1/auth",
                "HttpOnly",
                "Secure",
                "SameSite=Strict",
                "Max-Age=7200"
            ]
        );
        let body: serde_json::Value = response.json().await.unwrap();
        let token = body["token"].as_str().unwrap();
        assert_ne!(token, value);

        // Its record ends with the sign-in and holds only the digest, and the bearer names it.
        let record = signing
            .registry
            .find_session_cookie(&session_cookie_digest(value))
            .unwrap()
            .unwrap();
        assert!((record.expires_at() - (record.issued_at() + 7_200.0)).abs() < 1e-3);
        let bearer = signing.record_of(token).unwrap();
        assert_eq!(bearer.sign_in(), Some(record.token_id()));
        assert!((bearer.issued_at() - record.issued_at()).abs() < 1e-6);
        for entry in std::fs::read_dir(signing._directory.path().join("access-tokens")).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!text.contains(value), "the value is never written");
        }

        // A client that is no browser, and a page that asks not to, get none.
        let api = signing.login(None).await;
        assert_eq!(api.status(), StatusCode::CREATED);
        assert!(set_cookie_of(&api).is_none());
        let api: serde_json::Value = api.json().await.unwrap();
        let bearer = signing.record_of(api["token"].as_str().unwrap()).unwrap();
        assert_eq!(bearer.sign_in(), None);
        let declined = signing
            .page_post("/v1/auth/login")
            .json(&serde_json::json!({
                "name": "alice", "secret": "right", "session_cookie": false
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(declined.status(), StatusCode::CREATED);
        assert!(set_cookie_of(&declined).is_none());

        // A failed sign-in gets none.
        let refused = signing
            .page_post("/v1/auth/login")
            .json(&serde_json::json!({"name": "alice", "secret": "wrong"}))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        assert!(set_cookie_of(&refused).is_none());
    }

    #[tokio::test]
    async fn a_resume_answers_as_login_does_with_a_new_token_of_the_same_sign_in() {
        let signing = Signing::new(|gateway| {
            gateway.access_token_ttl = ai6::Seconds::new(3_600);
            gateway.session_max = TimeLimit::secs(7_200);
        })
        .await;
        let (signed_in, cookie) = signing.web_login().await;
        let first = signed_in["token"].as_str().unwrap();

        let resumed = signing.resume(&cookie).await;
        assert_eq!(resumed.status(), StatusCode::CREATED);
        assert!(
            set_cookie_of(&resumed).is_none(),
            "the cookie stays as it is"
        );
        let resumed: serde_json::Value = resumed.json().await.unwrap();
        let second = resumed["token"].as_str().unwrap();
        assert_ne!(second, first);
        assert!((3_590.0..=3_600.0).contains(&time_in(&resumed, "expires_in")));
        assert!(time_in(&resumed, "expires_at") > time_in(&signed_in, "expires_at") - 1e-3);
        assert!(
            (time_in(&resumed, "ends_at") - time_in(&signed_in, "ends_at")).abs() < 1e-3,
            "it ends when the sign-in does: {resumed}"
        );
        let session = body_of(signing.session(second).await).await;
        assert_eq!(session["person_id"], "alice-id");
        assert_eq!(session["token_id"], resumed["token_id"]);
        assert_eq!(
            signing.session(first).await.status(),
            StatusCode::OK,
            "another tab's token carries on"
        );
        let first = signing.record_of(first).unwrap();
        let second = signing.record_of(second).unwrap();
        assert_eq!(second.sign_in(), first.sign_in());
        assert!((second.issued_at() - first.issued_at()).abs() < 1e-6);
    }

    #[tokio::test]
    async fn a_resume_cannot_pass_the_cap_of_the_original_sign_in() {
        let signing = Signing::new(|gateway| {
            gateway.access_token_ttl = ai6::Seconds::new(3_600);
            gateway.session_max = TimeLimit::secs(7_200);
        })
        .await;
        let now = unix_timestamp().ok().unwrap();

        // A hundred seconds of the sign-in left: the token gets those and no more, however many
        // times it is resumed, and renewing it cannot move it past them.
        let issued_at = now - 7_100.0;
        let ends = issued_at + 7_200.0;
        let cookie = signing.cookie(issued_at, ends);
        for _ in 0..2 {
            let resumed = signing.resume(&cookie).await;
            assert_eq!(resumed.status(), StatusCode::CREATED);
            let resumed: serde_json::Value = resumed.json().await.unwrap();
            assert!(
                (time_in(&resumed, "expires_at") - ends).abs() < 1e-3,
                "{resumed}"
            );
            assert!(
                (time_in(&resumed, "ends_at") - ends).abs() < 1e-3,
                "{resumed}"
            );
            assert!(time_in(&resumed, "expires_in") <= 100.0, "{resumed}");
            let renewed = body_of(signing.renew(resumed["token"].as_str().unwrap()).await).await;
            assert!(
                (time_in(&renewed, "expires_at") - ends).abs() < 1e-3,
                "{renewed}"
            );
        }

        // Past it: refused and cleared.
        let over = signing.cookie(now - 7_300.0, now - 100.0);
        let refused = signing.resume(&over).await;
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        assert_clears_cookie(&refused);

        // A cookie written to last longer stops when `session_max` now says, lowered since.
        let lowered = signing.cookie(now - 7_300.0, now + 10_000.0);
        assert_eq!(
            signing.resume(&lowered).await.status(),
            StatusCode::UNAUTHORIZED
        );

        // With no bound there is nothing to pass, and the cookie says the longest it can.
        let unbound = Signing::new(|gateway| gateway.session_max = TimeLimit::UNBOUNDED).await;
        let response = unbound
            .page_post("/v1/auth/login")
            .json(&serde_json::json!({"name": "alice", "secret": "right"}))
            .send()
            .await
            .unwrap();
        let set = set_cookie_of(&response).unwrap();
        assert!(set.ends_with(&format!("Max-Age={}", u32::MAX)), "{set}");
        let cookie = set.split(';').next().unwrap();
        let resumed = unbound.resume(cookie).await;
        assert_eq!(resumed.status(), StatusCode::CREATED);
        let resumed: serde_json::Value = resumed.json().await.unwrap();
        assert!(resumed["ends_at"].is_null(), "{resumed}");
    }

    #[tokio::test]
    async fn a_revoked_sign_in_or_a_person_gone_cannot_resume() {
        let signing = Signing::new(|_| {}).await;
        let (signed_in, cookie) = signing.web_login().await;
        let resumed: serde_json::Value = signing.resume(&cookie).await.json().await.unwrap();

        // What Stations revoke by: the ID of any token of the sign-in.
        let revoked = reqwest::Client::new()
            .delete(format!(
                "{}/v1/station/access-tokens/{}",
                signing.base,
                signed_in["token_id"].as_str().unwrap()
            ))
            .bearer_auth("m-token-that-is-long-enough-for-testing")
            .send()
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::OK);
        let refused = signing.resume(&cookie).await;
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        assert_clears_cookie(&refused);
        assert_eq!(
            signing
                .session(resumed["token"].as_str().unwrap())
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "the token a resume gave goes with it"
        );

        let (_, cookie) = signing.web_login().await;
        signing
            .gone
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let refused = signing.resume(&cookie).await;
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        assert_clears_cookie(&refused);
    }

    #[tokio::test]
    async fn a_resume_needs_the_public_origin() {
        let signing = Signing::new(|_| {}).await;
        let (_, cookie) = signing.web_login().await;
        let post = || {
            reqwest::Client::new()
                .post(format!("{}/v1/auth/resume", signing.base))
                .header(COOKIE, &cookie)
        };
        for (request, what) in [
            (
                post()
                    .header(ORIGIN, "https://elsewhere.example")
                    .header("x-forwarded-proto", "https"),
                "another site",
            ),
            (
                post()
                    .header(ORIGIN, "null")
                    .header("x-forwarded-proto", "https"),
                "a sandboxed page of Moneypenny's",
            ),
            (post().header("x-forwarded-proto", "https"), "no origin"),
            (post().header(ORIGIN, "https://localhost"), "not over HTTPS"),
            (
                post()
                    .header(ORIGIN, "https://localhost")
                    .header("x-forwarded-proto", "http"),
                "said to be HTTP",
            ),
        ] {
            let refused = request.send().await.unwrap();
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{what}");
            assert!(set_cookie_of(&refused).is_none(), "{what} clears nothing");
            assert!(!refused.text().await.unwrap().contains("token"), "{what}");
        }
        assert_eq!(
            signing.resume(&cookie).await.status(),
            StatusCode::CREATED,
            "and it still works from the page"
        );
        let none = signing.page_post("/v1/auth/resume").send().await.unwrap();
        assert_eq!(none.status(), StatusCode::UNAUTHORIZED, "no cookie");
        assert!(set_cookie_of(&none).is_none());
    }

    #[tokio::test]
    async fn the_cookie_is_no_bearer_and_no_other_route_takes_it() {
        let signing = Signing::new(|_| {}).await;
        let (signed_in, cookie) = signing.web_login().await;
        let value = cookie.split_once('=').unwrap().1;
        let client = reqwest::Client::new();

        // Its value as a bearer token, everywhere one is taken.
        assert_eq!(
            signing.session(value).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            signing.renew(value).await.status(),
            StatusCode::UNAUTHORIZED
        );
        for (path, post) in [
            ("/v1/user/profile", false),
            ("/v1/stations", false),
            ("/v1/auth/logout", true),
        ] {
            let url = format!("{}{path}", signing.base);
            let request = if post {
                client.post(url)
            } else {
                client.get(url)
            };
            let answer = request.bearer_auth(value).send().await.unwrap();
            assert_eq!(answer.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
        // The cookie alone, as a route that wants a bearer sees it.
        for (path, post) in [
            ("/v1/auth/session", false),
            ("/v1/auth/renew", true),
            ("/v1/user/profile", false),
            ("/v1/stations", false),
        ] {
            let url = format!("{}{path}", signing.base);
            let request = if post {
                client.post(url)
            } else {
                client.get(url)
            };
            let answer = request
                .header(COOKIE, &cookie)
                .header(ORIGIN, "https://localhost")
                .header("x-forwarded-proto", "https")
                .send()
                .await
                .unwrap();
            assert_eq!(answer.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
        // A bearer token is no cookie either.
        let token = signed_in["token"].as_str().unwrap();
        let as_cookie = format!("{SESSION_COOKIE}={token}");
        assert_eq!(
            signing.resume(&as_cookie).await.status(),
            StatusCode::UNAUTHORIZED
        );
        // None of that ended anything.
        assert_eq!(signing.session(token).await.status(), StatusCode::OK);
        assert_eq!(signing.resume(&cookie).await.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn signing_out_ends_the_cookie_and_every_token_of_that_sign_in() {
        let signing = Signing::new(|_| {}).await;
        let (signed_in, cookie) = signing.web_login().await;
        let first = signed_in["token"].as_str().unwrap();
        let resumed: serde_json::Value = signing.resume(&cookie).await.json().await.unwrap();
        let second = resumed["token"].as_str().unwrap();
        let (other, other_cookie) = signing.web_login().await;

        let signed_out = signing
            .page_post("/v1/auth/logout")
            .header(COOKIE, &cookie)
            .bearer_auth(second)
            .send()
            .await
            .unwrap();
        assert_eq!(signed_out.status(), StatusCode::NO_CONTENT);
        assert_clears_cookie(&signed_out);
        for token in [first, second] {
            assert_eq!(
                signing.session(token).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            signing.resume(&cookie).await.status(),
            StatusCode::UNAUTHORIZED
        );

        // That sign-in alone: one elsewhere carries on.
        assert_eq!(
            signing
                .session(other["token"].as_str().unwrap())
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            signing.resume(&other_cookie).await.status(),
            StatusCode::CREATED
        );
    }

    #[tokio::test]
    async fn signing_out_with_a_token_past_its_time_still_ends_the_cookie() {
        let signing = Signing::new(|_| {}).await;
        let (signed_in, cookie) = signing.web_login().await;
        let token = signed_in["token"].as_str().unwrap();

        // From another site the cookie counts for nothing, and the person stays signed in.
        let foreign = reqwest::Client::new()
            .post(format!("{}/v1/auth/logout", signing.base))
            .header(COOKIE, &cookie)
            .header(ORIGIN, "https://elsewhere.example")
            .header("x-forwarded-proto", "https")
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(signing.session(token).await.status(), StatusCode::OK);

        let signed_out = signing
            .page_post("/v1/auth/logout")
            .header(COOKIE, &cookie)
            .bearer_auth("a-token-long-over")
            .send()
            .await
            .unwrap();
        assert_eq!(signed_out.status(), StatusCode::NO_CONTENT);
        assert_clears_cookie(&signed_out);
        assert_eq!(
            signing.resume(&cookie).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            signing.session(token).await.status(),
            StatusCode::UNAUTHORIZED,
            "and the token that was still good goes with it"
        );
    }

    #[tokio::test]
    async fn with_the_setting_off_a_sign_in_lives_in_memory_alone() {
        for off in [
            Signing::new(|gateway| gateway.session_cookie = false).await,
            Signing::new(|gateway| gateway.session_max = TimeLimit::secs(0)).await,
        ] {
            let response = off
                .page_post("/v1/auth/login")
                .json(&serde_json::json!({"name": "alice", "secret": "right"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
            assert!(set_cookie_of(&response).is_none());
            let body: serde_json::Value = response.json().await.unwrap();
            let token = body["token"].as_str().unwrap();
            assert_eq!(off.record_of(token).unwrap().sign_in(), None);

            // One from before the setting changed is refused and cleared.
            let now = unix_timestamp().ok().unwrap();
            let refused = off.resume(&off.cookie(now - 10.0, now + 3_600.0)).await;
            assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
            assert_clears_cookie(&refused);
        }
    }

    #[tokio::test]
    async fn refused_resumes_are_counted_apart_from_login_tries() {
        let signing = Signing::built(
            |_| {},
            LoginLimits {
                per_address: std::num::NonZeroU32::new(3).unwrap(),
                per_name: std::num::NonZeroU32::new(3).unwrap(),
                address_header: None,
                ..LoginLimits::default()
            },
            |providers| providers,
        )
        .await;
        let (_, good) = signing.web_login().await;

        // No cookie is what every signed-out page sends: it is not counted.
        for _ in 0..5 {
            let none = signing.page_post("/v1/auth/resume").send().await.unwrap();
            assert_eq!(none.status(), StatusCode::UNAUTHORIZED);
        }
        let bad = format!("{SESSION_COOKIE}=made-up");
        for _ in 0..3 {
            assert_eq!(
                signing.resume(&bad).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let held = signing.resume(&bad).await;
        assert_eq!(held.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(held.headers().contains_key(RETRY_AFTER));
        assert_clears_cookie(&held);

        // Nobody behind that address pays for it: a good cookie resumes, the person's password
        // tries are untouched, and resuming spends none of them.
        for _ in 0..4 {
            assert_eq!(signing.resume(&good).await.status(), StatusCode::CREATED);
        }
        for _ in 0..2 {
            let (_, again) = signing.web_login().await;
            assert_eq!(signing.resume(&again).await.status(), StatusCode::CREATED);
        }
    }

    #[tokio::test]
    async fn a_resume_stops_when_the_front_proxy_says_this_is_someone_else() {
        let signing = Signing::built(
            |_| {},
            LoginLimits::default(),
            |providers| {
                providers
                    .with_proxy_identity(HeaderName::from_static("x-forwarded-user"), "name")
                    .with_proxy_secret(PROXY_TEST_SECRET, 32)
                    .unwrap()
            },
        )
        .await;
        let (_, cookie) = signing.web_login().await;
        let as_user = |user: &'static str| {
            signing
                .page_post("/v1/auth/resume")
                .header(COOKIE, &cookie)
                .header("x-forwarded-user", user)
                .header(PROXY_SECRET_HEADER, PROXY_TEST_SECRET)
        };
        let same = as_user("alice").send().await.unwrap();
        assert_eq!(same.status(), StatusCode::CREATED);
        let other = as_user("bob").send().await.unwrap();
        assert_eq!(other.status(), StatusCode::UNAUTHORIZED);
        assert_clears_cookie(&other);
        // A claim nobody can prove means nothing, and costs the person nothing.
        let forged = signing
            .page_post("/v1/auth/resume")
            .header(COOKIE, &cookie)
            .header("x-forwarded-user", "bob")
            .send()
            .await
            .unwrap();
        assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);
        assert!(set_cookie_of(&forged).is_none());
        assert_eq!(signing.resume(&cookie).await.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn login_is_refused_without_being_judged_past_the_tries_of_an_address() {
        let (base, server, _directory) = login_gateway(LoginLimits {
            per_address: std::num::NonZeroU32::new(3).unwrap(),
            per_name: std::num::NonZeroU32::new(100).unwrap(),
            address_header: None,
            ..LoginLimits::default()
        })
        .await;
        let client = reqwest::Client::new();
        for _ in 0..3 {
            let wrong = try_login(&client, &base, "alice", "wrong", None).await;
            assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        }
        // The right secret is refused too: past the count nothing is judged.
        let limited = try_login(&client, &base, "alice", "right", None).await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        let wait: u64 = limited
            .headers()
            .get(RETRY_AFTER)
            .expect("a refused try says how long to wait")
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=60).contains(&wait));
        server.abort();
    }

    #[tokio::test]
    async fn login_counts_a_name_across_addresses_and_addresses_the_proxy_names() {
        let (base, server, _directory) = login_gateway(LoginLimits {
            per_address: std::num::NonZeroU32::new(100).unwrap(),
            per_name: std::num::NonZeroU32::new(2).unwrap(),
            address_header: Some(HeaderName::from_static("x-forwarded-for")),
            ..LoginLimits::default()
        })
        .await;
        let client = reqwest::Client::new();
        for address in ["198.51.100.1", "198.51.100.2"] {
            let wrong = try_login(&client, &base, "alice", "wrong", Some(address)).await;
            assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        }
        let by_name = try_login(&client, &base, "ALICE", "right", Some("198.51.100.3")).await;
        assert_eq!(by_name.status(), StatusCode::TOO_MANY_REQUESTS);
        // Another account from a fresh address is unaffected, and a right secret still works.
        let other = try_login(&client, &base, "bob", "wrong", Some("198.51.100.4")).await;
        assert_eq!(other.status(), StatusCode::UNAUTHORIZED);
        server.abort();

        let (base, server, _directory) = login_gateway(LoginLimits {
            per_address: std::num::NonZeroU32::new(1).unwrap(),
            per_name: std::num::NonZeroU32::new(100).unwrap(),
            address_header: Some(HeaderName::from_static("x-forwarded-for")),
            ..LoginLimits::default()
        })
        .await;
        let first = try_login(&client, &base, "alice", "right", Some("198.51.100.1")).await;
        assert_eq!(first.status(), StatusCode::CREATED);
        let same = try_login(&client, &base, "alice", "right", Some("198.51.100.1")).await;
        assert_eq!(same.status(), StatusCode::TOO_MANY_REQUESTS);
        let elsewhere = try_login(&client, &base, "alice", "right", Some("198.51.100.9")).await;
        assert_eq!(elsewhere.status(), StatusCode::CREATED);
        server.abort();
    }

    #[tokio::test]
    async fn user_access_filters_stations_and_scopes_attach_grants() {
        struct TestAuthenticator;

        impl Authenticator for TestAuthenticator {
            fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
                match (attempt.name.as_deref(), attempt.secret.as_deref()) {
                    (Some("viewer"), Some("secret")) | (Some("operator"), Some("secret")) => {
                        Ok(Verdict::Valid(Lookup {
                            by: "name".to_owned(),
                            value: attempt.name.unwrap(),
                        }))
                    }
                    _ => Ok(Verdict::Invalid),
                }
            }
        }

        struct TestDirectory;

        impl Directory for TestDirectory {
            fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
                let person = match lookup.value.as_str() {
                    "viewer" => Some(("viewer-007", "viewer", vec!["field"])),
                    "operator" => Some(("operator-007", "operator", Vec::new())),
                    "viewer-007" => Some(("viewer-007", "viewer", vec!["field"])),
                    "operator-007" => Some(("operator-007", "operator", Vec::new())),
                    _ => None,
                };
                Ok(person.map(|(id, name, groups)| Person {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    display_name: None,
                    email: None,
                    groups: groups.into_iter().map(str::to_owned).collect(),
                    attributes: BTreeMap::new(),
                }))
            }
        }

        let authorizer = ai6::access_from_toml(
            r#"
                [role.viewer]
                actions = ["mission.read", "session.read"]

                [role.operator]
                actions = ["mission.read", "mission.create", "session.read", "session.input", "audit.read", "agent.read", "agent.configure"]

                [role.station-operator]
                actions = ["station.read", "station.configure", "audit.read"]

                [[rule]]
                subject = "person:viewer-007"
                scope = "operation:G"
                role = "viewer"

                [[rule]]
                subject = "person:operator-007"
                scope = "operation:G"
                role = "operator"

                [[rule]]
                subject = "person:operator-007"
                scope = "station:worker"
                role = "station-operator"

                [[rule]]
                subject = "person:operator-007"
                scope = "station:hq"
                role = "station-operator"
            "#, ai6::Limit::UNBOUNDED,
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(directory.path());
        let mut hq_info = BTreeMap::new();
        hq_info.insert("operations".to_owned(), serde_json::json!([]));
        hq_info.insert(
            "agent_roles".to_owned(),
            serde_json::json!([{
                "id": "case-officer",
                "purpose": "Coordinate routine work.",
                "policy": "Follow the global policy."
            }]),
        );
        hq_info.insert("roles".to_owned(), serde_json::json!(["control"]));
        hq_info.insert("capacity".to_owned(), serde_json::json!(0));
        hq_info.insert("running".to_owned(), serde_json::json!(0));
        hq_info.insert("host".to_owned(), serde_json::json!("private-host"));
        hq_info.insert("process_id".to_owned(), serde_json::json!(7132));
        registry.check_in("hq", hq_info).unwrap();
        registry.elect("hq", Duration::from_secs(120)).unwrap();
        let mut worker_info = BTreeMap::new();
        worker_info.insert("operations".to_owned(), serde_json::json!(["G", "L"]));
        worker_info.insert("roles".to_owned(), serde_json::json!(["execution"]));
        worker_info.insert("capacity".to_owned(), serde_json::json!(4));
        worker_info.insert("running".to_owned(), serde_json::json!(2));
        worker_info.insert(
            "config".to_owned(),
            serde_json::json!({
                "revision": "18446744073709551615",
                "refused": ["roster revision 7: Agent \"009\": clearance"],
                "needs_restart": ["theater"]
            }),
        );
        worker_info.insert(
            "resources".to_owned(),
            serde_json::json!({
                "cpus": 8.0, "cpu_percent": 25.0, "load_1m": 1.5, "memory_total_mb": 16384,
                "memory_available_mb": 12288, "sampled_at": 1_700_000_000
            }),
        );
        worker_info.insert(
            "theaters".to_owned(),
            serde_json::json!([
                {
                    "id": "local", "access": "local", "labels": {},
                    "agents": "auto", "egress": "direct"
                },
                {
                    "id": "build-box", "access": "ssh", "host": "build.example.net",
                    "labels": {"os": "linux"}, "agents": "auto", "egress": "direct",
                    "capabilities": {
                        "terminal": true, "run": true, "files": true, "forward": true,
                        "os": "linux", "arch": "x86_64"
                    },
                    "harnesses": ["claude"],
                    "resources": {
                        "cpus": 1.5, "load_1m": 0.5, "memory_total_mb": 4096,
                        "memory_available_mb": 2048, "sampled_at": 1_700_000_010
                    },
                    "sessions": 1
                }
            ]),
        );
        worker_info.insert("handlers".to_owned(), serde_json::json!(["claude"]));
        worker_info.insert(
            "head_of_station".to_owned(),
            serde_json::json!({
                "station": "worker", "at": 1_700_000_100,
                "summary": "Disk at 12% free; two safe houses hold 40 GB of caches.",
                "findings": [{
                    "kind": "disk", "severity": "warning",
                    "text": "12% free under the state directory"
                }],
                "mission": "round-1", "agent": "M"
            }),
        );
        worker_info.insert(
            "operation_policies".to_owned(),
            serde_json::json!({"G": {
                "operation": "G",
                "checkout": true,
                "agents": [
                    {"id": "007", "handler": "claude", "priority": 10, "roles": ["case-officer"]},
                    {"id": "008", "handler": "codex", "priority": 5, "roles": ["case-officer"]}
                ]
            }}),
        );
        registry.check_in("worker", worker_info).unwrap();
        let make_proposal = |retry_key: &str, title: &str| {
            MissionProposal::try_new(
                "G",
                ai6::directorates::m::identity::MissionId::try_new("parent-mission").unwrap(),
                "coordinator",
                ai6::directorates::m::identity::SessionId::try_new("provider-session").unwrap(),
                retry_key,
                Brief {
                    title: title.to_owned(),
                    body: "Review this delegated work.".to_owned(),
                    id: "proposal".to_owned(),
                    mission_id: None,
                    handle: None,
                    name: None,
                    source: "agent".to_owned(),
                    operation: None,
                    agent: None,
                    role: Some("case-officer".to_owned()),
                    tags: vec!["review".to_owned()],
                    stations: Vec::new(),
                    priority: 1,
                    url: None,
                    data: BTreeMap::new(),
                    created: String::new(),
                    version: 1,
                    provenance: None,
                },
                &Default::default(),
            )
            .unwrap()
        };
        let proposal_approval = make_proposal("approval-test", "Approve this delegated work");
        let proposal_rejection = make_proposal("rejection-test", "Reject this delegated work");
        registry
            .submit_mission_proposal(&proposal_approval)
            .unwrap();
        registry
            .submit_mission_proposal(&proposal_rejection)
            .unwrap();
        let approval_proposal_id = proposal_approval.id.clone();
        let rejection_proposal_id = proposal_rejection.id.clone();
        let gateway = Gateway::with_access(
            GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                test_limits(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
            Arc::new(registry),
            AccessProviders::new(
                Arc::new(TestAuthenticator),
                Arc::new(TestDirectory),
                Arc::new(authorizer),
            )
            .with_proxy_identity(HeaderName::from_static("x-forwarded-user"), "name")
            .with_proxy_secret(PROXY_TEST_SECRET, 32)
            .unwrap(),
        );
        for (station, mission, session) in [
            ("G", "mission-g", "session-g"),
            ("L", "mission-l", "session-l"),
        ] {
            gateway
                .register_session(
                    "m-token-that-is-long-enough-for-testing",
                    OperationId::new(station).unwrap(),
                    MissionId::new(mission).unwrap(),
                    SessionId::new(session).unwrap(),
                )
                .await
                .unwrap();
        }
        gateway
            .publish_mission_brief(
                "m-token-that-is-long-enough-for-testing",
                OperationId::new("G").unwrap(),
                MissionId::new("mission-g").unwrap(),
                LaunchManifest {
                    name: None,
                    prompt: "Review the requested change".to_owned(),
                    agent_id: Some("technical-officer".to_owned()),
                    role_id: Some("case-officer".to_owned()),
                    handler_provider: "claude".to_owned(),
                    terminal_provider: "pty".to_owned(),
                    working_directory: "C:/stations/field".to_owned(),
                    model: Some("claude-sonnet".to_owned()),
                    effort: Some("high".to_owned()),
                    environment_keys: vec!["API_TOKEN".to_owned()],
                    station_id: "worker".to_owned(),
                    theater_id: Some("local".to_owned()),
                    mode: "remote-hands".to_owned(),
                    provenance: Some(Provenance::new(
                        Origin::RegistryDrop,
                        &serde_json::from_value::<Brief>(
                            serde_json::json!({"title": "Review the requested change"}),
                        )
                        .unwrap(),
                        "worker",
                        1_792_922_400,
                    )),
                },
            )
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, gateway.router()).await });
        let base = format!("http://{address}");
        let client = reqwest::Client::new();
        let login = |name: &'static str| {
            client
                .post(format!("{base}/v1/auth/login"))
                .header("origin", "https://localhost")
                .header("x-forwarded-proto", "https")
                .json(&serde_json::json!({"name": name, "secret": "secret"}))
        };

        // A client that reaches the port without the proxy sets every header but the secret.
        for secret in [None, Some("wrong"), Some(&PROXY_TEST_SECRET[1..])] {
            let mut forged = client
                .post(format!("{base}/v1/auth/login"))
                .header("origin", "https://localhost")
                .header("x-forwarded-proto", "https")
                .header("x-forwarded-user", "viewer");
            if let Some(secret) = secret {
                forged = forged.header(PROXY_SECRET_HEADER, secret);
            }
            let forged = forged.json(&serde_json::json!({})).send().await.unwrap();
            assert_eq!(forged.status(), StatusCode::UNAUTHORIZED, "{secret:?}");
        }
        let proxy_login = client
            .post(format!("{base}/v1/auth/login"))
            .header("origin", "https://localhost")
            .header("x-forwarded-proto", "https")
            .header("x-forwarded-user", "viewer")
            .header(PROXY_SECRET_HEADER, PROXY_TEST_SECRET)
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(proxy_login.status(), StatusCode::CREATED);
        let proxy_token: String = proxy_login.json::<serde_json::Value>().await.unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        let proxy_board = client
            .get(format!("{base}/v1/board"))
            .bearer_auth(&proxy_token)
            .send()
            .await
            .unwrap();
        assert_eq!(proxy_board.status(), StatusCode::OK);
        let proxy_board: serde_json::Value = proxy_board.json().await.unwrap();
        assert_eq!(proxy_board.as_array().unwrap().len(), 1);
        assert_eq!(proxy_board[0]["operation_id"], "G");
        let unknown_proxy_identity = client
            .post(format!("{base}/v1/auth/login"))
            .header("origin", "https://localhost")
            .header("x-forwarded-proto", "https")
            .header("x-forwarded-user", "unknown-account")
            .header(PROXY_SECRET_HEADER, PROXY_TEST_SECRET)
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(unknown_proxy_identity.status(), StatusCode::UNAUTHORIZED);
        let proxy_identity_without_origin = client
            .post(format!("{base}/v1/auth/login"))
            .header("x-forwarded-user", "viewer")
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            proxy_identity_without_origin.status(),
            StatusCode::FORBIDDEN
        );

        let viewer_login = login("viewer").send().await.unwrap();
        assert_eq!(viewer_login.status(), StatusCode::CREATED);
        let viewer_token: String = viewer_login.json::<serde_json::Value>().await.unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        let viewer_profile = client
            .get(format!("{base}/v1/user/profile"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_profile.status(), StatusCode::OK);
        let viewer_profile: serde_json::Value = viewer_profile.json().await.unwrap();
        assert_eq!(viewer_profile["person_id"], "viewer-007");
        assert_eq!(viewer_profile["operations"][0]["operation_id"], "G");
        assert_eq!(
            viewer_profile["operations"][0]["actions"],
            serde_json::json!(["mission.read", "session.read"])
        );
        assert!(viewer_profile["stations"].as_array().unwrap().is_empty());
        assert_eq!(viewer_profile["moneypenny"], false);
        let viewer_settings = client
            .get(format!("{base}/v1/user/stations/worker/settings"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_settings.status(), StatusCode::FORBIDDEN);
        let viewer_create = client
            .post(format!("{base}/v1/user/operations/G/missions"))
            .bearer_auth(&viewer_token)
            .json(&serde_json::json!({
                "title": "Unauthorized brief",
                "prompt": "This must not enter the queue"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_create.status(), StatusCode::FORBIDDEN);

        let operator_login = login("operator").send().await.unwrap();
        assert_eq!(operator_login.status(), StatusCode::CREATED);
        let operator_token: String =
            operator_login.json::<serde_json::Value>().await.unwrap()["token"]
                .as_str()
                .unwrap()
                .to_owned();
        let operator_profile = client
            .get(format!("{base}/v1/user/profile"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(operator_profile.status(), StatusCode::OK);
        let operator_profile: serde_json::Value = operator_profile.json().await.unwrap();
        assert_eq!(operator_profile["person_id"], "operator-007");
        assert_eq!(operator_profile["operations"][0]["operation_id"], "G");
        assert_eq!(
            operator_profile["operations"][0]["actions"],
            serde_json::json!([
                "agent.configure",
                "agent.read",
                "audit.read",
                "mission.create",
                "mission.read",
                "session.input",
                "session.read",
            ])
        );
        let station_access = operator_profile["stations"].as_array().unwrap();
        let worker_access = station_access
            .iter()
            .find(|station| station["station_id"] == "worker")
            .unwrap();
        assert_eq!(
            worker_access["actions"],
            serde_json::json!(["audit.read", "station.configure", "station.read"])
        );
        let initial_settings = client
            .get(format!("{base}/v1/user/stations/worker/settings"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(initial_settings.status(), StatusCode::OK);
        let initial_settings: serde_json::Value = initial_settings.json().await.unwrap();
        assert_eq!(initial_settings["max_concurrent_missions"], 4);
        assert_eq!(initial_settings["configured"], false);
        assert_eq!(initial_settings["execution_enabled"], true);
        let control_station_settings = client
            .get(format!("{base}/v1/user/stations/hq/settings"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(control_station_settings.status(), StatusCode::OK);
        let control_station_settings: serde_json::Value =
            control_station_settings.json().await.unwrap();
        assert_eq!(control_station_settings["execution_enabled"], false);
        let control_station_update = client
            .put(format!("{base}/v1/user/stations/hq/settings"))
            .bearer_auth(&operator_token)
            .json(&serde_json::json!({"max_concurrent_missions": 8}))
            .send()
            .await
            .unwrap();
        assert_eq!(control_station_update.status(), StatusCode::CONFLICT);
        let invalid_settings = client
            .put(format!("{base}/v1/user/stations/worker/settings"))
            .bearer_auth(&operator_token)
            .json(&serde_json::json!({"max_concurrent_missions": 0}))
            .send()
            .await
            .unwrap();
        assert_eq!(invalid_settings.status(), StatusCode::BAD_REQUEST);
        let updated_settings = client
            .put(format!("{base}/v1/user/stations/worker/settings"))
            .bearer_auth(&operator_token)
            .json(&serde_json::json!({"max_concurrent_missions": 8}))
            .send()
            .await
            .unwrap();
        assert_eq!(updated_settings.status(), StatusCode::NO_CONTENT);
        let updated_settings = client
            .get(format!("{base}/v1/user/stations/worker/settings"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        let updated_settings: serde_json::Value = updated_settings.json().await.unwrap();
        assert_eq!(updated_settings["max_concurrent_missions"], 8);
        assert_eq!(updated_settings["configured"], true);
        assert_eq!(
            (
                &updated_settings["min_free_memory_mb"],
                &updated_settings["max_cpu_percent"]
            ),
            (&serde_json::json!(512), &serde_json::json!(95)),
            "resource limits start at their defaults"
        );
        let put_settings = |body: serde_json::Value| {
            client
                .put(format!("{base}/v1/user/stations/worker/settings"))
                .bearer_auth(&operator_token)
                .json(&body)
                .send()
        };
        assert_eq!(
            put_settings(serde_json::json!({"max_concurrent_missions": 8, "max_cpu_percent": 101}))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            put_settings(
                serde_json::json!({"max_concurrent_missions": 8, "min_free_memory_mb": 0})
            )
            .await
            .unwrap()
            .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            put_settings(serde_json::json!({"max_concurrent_missions": 6, "max_cpu_percent": 100}))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let loosened: serde_json::Value = client
            .get(format!("{base}/v1/user/stations/worker/settings"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            (
                &loosened["max_concurrent_missions"],
                &loosened["min_free_memory_mb"],
                &loosened["max_cpu_percent"]
            ),
            (
                &serde_json::json!(6),
                &serde_json::json!(0),
                &serde_json::json!(100)
            ),
            "a limit left out keeps the value set before"
        );
        let station_audit = client
            .get(format!("{base}/v1/user/audit?station_id=worker"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(station_audit.status(), StatusCode::OK);
        let station_audit: serde_json::Value = station_audit.json().await.unwrap();
        assert_eq!(station_audit[0]["action"], "station.settings.update");
        assert_eq!(station_audit[0]["resource"], "station:worker");
        assert_eq!(station_audit[0]["operation"], "");
        let created_mission = client
            .post(format!("{base}/v1/user/operations/G/missions"))
            .bearer_auth(&operator_token)
            .json(&serde_json::json!({
                "title": "Inspect the board",
                "prompt": "Check operation context and report findings."
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(created_mission.status(), StatusCode::ACCEPTED);
        for role in ["head-of-station", "head-of-station/disk"] {
            let round = client
                .post(format!("{base}/v1/user/operations/G/missions"))
                .bearer_auth(&operator_token)
                .json(&serde_json::json!({
                    "title": "Tidy the Station", "prompt": "Remove every safe house.",
                    "role": role, "stations": ["worker"]
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(
                round.status(),
                StatusCode::BAD_REQUEST,
                "only a Sleeper queues a Head of Station round"
            );
        }
        let created: serde_json::Value = created_mission.json().await.unwrap();
        assert_eq!(created["operation_id"], "G");
        assert_eq!(created["state"], "queued");
        let queued = FileRegistry::open(directory.path()).list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].title, "Inspect the board");
        assert_eq!(
            queued[0].body,
            "Check operation context and report findings."
        );
        assert_eq!(queued[0].operation.as_deref(), Some("G"));
        assert_eq!(queued[0].source, "universal-exports");
        assert!(queued[0].created.ends_with('Z'));
        let provenance = queued[0]
            .provenance
            .as_ref()
            .expect("the gateway records the asker");
        assert_eq!(provenance.origin, Origin::Operator);
        assert_eq!(provenance.received_by, GATEWAY_RECEIVER);
        assert!(provenance.principal.is_some());
        assert!(provenance.matches(&queued[0]));
        assert_eq!(created["mission_id"], queued[0].key());
        assert!(!created.to_string().contains("Check operation context"));

        let operator_audit = client
            .get(format!("{base}/v1/user/audit?operation_id=G"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(operator_audit.status(), StatusCode::OK);
        let operator_audit: serde_json::Value = operator_audit.json().await.unwrap();
        assert_eq!(operator_audit.as_array().unwrap().len(), 1);
        assert_eq!(operator_audit[0]["actor"], "operator-007");
        assert_eq!(operator_audit[0]["action"], "mission.create.requested");
        assert_eq!(operator_audit[0]["operation"], "G");
        assert_eq!(operator_audit[0]["target"], created["mission_id"]);
        assert!(operator_audit[0]["at"].as_f64().unwrap() > 0.0);
        assert!(
            !operator_audit
                .to_string()
                .contains("Check operation context")
        );

        let viewer_audit = client
            .get(format!("{base}/v1/user/audit?operation_id=G"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_audit.status(), StatusCode::FORBIDDEN);

        let viewer_proposals = client
            .get(format!("{base}/v1/user/operations/G/mission-proposals"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_proposals.status(), StatusCode::FORBIDDEN);
        let viewer_approval = client
            .post(format!(
                "{base}/v1/user/operations/G/mission-proposals/{approval_proposal_id}/approve"
            ))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_approval.status(), StatusCode::FORBIDDEN);

        let proposals = client
            .get(format!("{base}/v1/user/operations/G/mission-proposals"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(proposals.status(), StatusCode::OK);
        let proposals: serde_json::Value = proposals.json().await.unwrap();
        assert_eq!(proposals.as_array().unwrap().len(), 2);
        assert!(
            proposals
                .to_string()
                .contains("Review this delegated work.")
        );
        assert!(!proposals.to_string().contains("operator secret"));

        let approved = client
            .post(format!(
                "{base}/v1/user/operations/G/mission-proposals/{approval_proposal_id}/approve"
            ))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(approved.status(), StatusCode::ACCEPTED);
        let approved: serde_json::Value = approved.json().await.unwrap();
        assert_eq!(approved["proposal_id"], approval_proposal_id);
        assert_eq!(approved["state"], "queued");
        assert_eq!(
            FileRegistry::open(directory.path())
                .list_new()
                .unwrap()
                .len(),
            2
        );
        let cleared = FileRegistry::open(directory.path())
            .clearance(approved["mission_id"].as_str().unwrap())
            .unwrap()
            .expect("an operator's approval clears the promoted brief");
        assert!(cleared.kinds.contains(&ai6::FindingKind::Screening));
        assert!(!cleared.kinds.contains(&ai6::FindingKind::Malformed));
        assert_eq!(cleared.by, "operator-007");
        let rejected = client
            .post(format!(
                "{base}/v1/user/operations/G/mission-proposals/{rejection_proposal_id}/reject"
            ))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::OK);
        let rejected: serde_json::Value = rejected.json().await.unwrap();
        assert_eq!(rejected["state"], "rejected");
        assert!(
            FileRegistry::open(directory.path())
                .list_mission_proposals("G", 10)
                .unwrap()
                .is_empty()
        );
        let proposal_audit = client
            .get(format!("{base}/v1/user/audit?operation_id=G"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(proposal_audit.status(), StatusCode::OK);
        let proposal_audit: serde_json::Value = proposal_audit.json().await.unwrap();
        for (action, proposal_id) in [
            ("mission.proposal.approved", approval_proposal_id.as_str()),
            ("mission.proposal.rejected", rejection_proposal_id.as_str()),
        ] {
            assert!(proposal_audit.as_array().unwrap().iter().any(|entry| {
                entry["actor"] == "operator-007"
                    && entry["action"] == action
                    && entry["target"] == proposal_id
                    && entry["at"].as_f64().is_some_and(|at| at > 0.0)
            }));
        }

        let role = serde_json::json!({
            "id": "reviewer",
            "purpose": "Review Operation changes",
            "policy": "Follow the configured Operation rules."
        });
        let viewer_role_update = client
            .put(format!("{base}/v1/user/operations/G/agent-roles/reviewer"))
            .bearer_auth(&viewer_token)
            .json(&role)
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_role_update.status(), StatusCode::FORBIDDEN);
        let viewer_role_list = client
            .get(format!("{base}/v1/user/operations/G/agent-roles"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_role_list.status(), StatusCode::FORBIDDEN);

        let operator_role_update = client
            .put(format!("{base}/v1/user/operations/G/agent-roles/reviewer"))
            .bearer_auth(&operator_token)
            .json(&role)
            .send()
            .await
            .unwrap();
        assert_eq!(operator_role_update.status(), StatusCode::NO_CONTENT);
        let roles = client
            .get(format!("{base}/v1/user/operations/G/agent-roles"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(roles.status(), StatusCode::OK);
        let roles: serde_json::Value = roles.json().await.unwrap();
        assert_eq!(roles.as_array().unwrap().len(), 2);
        assert_eq!(roles[0]["id"], "case-officer");
        assert_eq!(roles[0]["operation_override"], false);
        assert_eq!(roles[1]["id"], "reviewer");
        assert_eq!(roles[1]["operation_override"], true);
        assert!(
            roles
                .to_string()
                .contains("Follow the configured Operation rules.")
        );

        let role_audit = client
            .get(format!("{base}/v1/user/audit?operation_id=G"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(role_audit.status(), StatusCode::OK);
        let role_audit: serde_json::Value = role_audit.json().await.unwrap();
        assert!(role_audit.as_array().unwrap().iter().any(|entry| {
            entry["actor"] == "operator-007"
                && entry["action"] == "agent.role.update"
                && entry["target"] == "reviewer"
                && entry["at"].as_f64().is_some_and(|at| at > 0.0)
        }));
        assert!(
            !role_audit
                .to_string()
                .contains("Follow the configured Operation rules.")
        );

        let role_delete = client
            .delete(format!("{base}/v1/user/operations/G/agent-roles/reviewer"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(role_delete.status(), StatusCode::NO_CONTENT);
        let roles_after_delete = client
            .get(format!("{base}/v1/user/operations/G/agent-roles"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        let roles_after_delete: serde_json::Value = roles_after_delete.json().await.unwrap();
        assert_eq!(roles_after_delete.as_array().unwrap().len(), 1);
        assert_eq!(roles_after_delete[0]["id"], "case-officer");

        let quarantine = format!("{base}/v1/user/operations/G/quarantine");
        let forbidden = client
            .post(format!("{quarantine}/007"))
            .bearer_auth(&viewer_token)
            .json(&serde_json::json!({ "reason": "looping" }))
            .send()
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        let quarantined = client
            .post(format!("{quarantine}/007"))
            .bearer_auth(&operator_token)
            .json(&serde_json::json!({ "reason": "looping" }))
            .send()
            .await
            .unwrap();
        assert_eq!(quarantined.status(), StatusCode::CREATED);
        let listed: serde_json::Value = client
            .get(&quarantine)
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(listed[0]["agent"], "007");
        assert_eq!(listed[0]["operation"], "G");
        assert_eq!(listed[0]["by"], "operator-007");
        let viewer_release = client
            .post(format!("{quarantine}/007/release"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_release.status(), StatusCode::FORBIDDEN);
        let released = client
            .post(format!("{quarantine}/007/release"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(released.status(), StatusCode::NO_CONTENT);
        let again = client
            .post(format!("{quarantine}/007/release"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::NOT_FOUND);
        let empty: serde_json::Value = client
            .get(&quarantine)
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(empty.as_array().unwrap().is_empty());

        let anonymous_board = client.get(format!("{base}/v1/board")).send().await.unwrap();
        assert_eq!(anonymous_board.status(), StatusCode::UNAUTHORIZED);
        let anonymous_missions = client
            .get(format!("{base}/v1/missions"))
            .send()
            .await
            .unwrap();
        assert_eq!(anonymous_missions.status(), StatusCode::UNAUTHORIZED);
        let anonymous_stations = client
            .get(format!("{base}/v1/stations"))
            .send()
            .await
            .unwrap();
        assert_eq!(anonymous_stations.status(), StatusCode::UNAUTHORIZED);
        let anonymous_profile = client
            .get(format!("{base}/v1/user/profile"))
            .send()
            .await
            .unwrap();
        assert_eq!(anonymous_profile.status(), StatusCode::UNAUTHORIZED);
        let anonymous_audit = client
            .get(format!("{base}/v1/user/audit"))
            .send()
            .await
            .unwrap();
        assert_eq!(anonymous_audit.status(), StatusCode::UNAUTHORIZED);

        let station_board = client
            .get(format!("{base}/v1/stations"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(station_board.status(), StatusCode::OK);
        let station_board: serde_json::Value = station_board.json().await.unwrap();
        assert!(station_board["headquarters_station"].is_null());
        assert_eq!(station_board["stations"].as_array().unwrap().len(), 1);
        assert_eq!(station_board["stations"][0]["station_id"], "worker");
        assert_eq!(station_board["stations"][0]["healthy"], true);
        assert_eq!(station_board["stations"][0]["headquarters"], false);
        assert_eq!(station_board["stations"][0]["operations"][0], "G");
        assert!(
            station_board["stations"][0]["head_of_station"].is_null(),
            "the report covers every Operation: a viewer of one does not see it"
        );
        let operator_board: serde_json::Value = client
            .get(format!("{base}/v1/stations"))
            .bearer_auth(&operator_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let worker = operator_board["stations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|station| station["station_id"] == "worker")
            .unwrap();
        let control = operator_board["stations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|station| station["station_id"] == "hq")
            .expect("station.read shows a Station that holds no checkout");
        assert_eq!(control["operations"], serde_json::json!([]));
        assert_eq!(operator_board["headquarters_station"], "hq");
        assert_eq!(
            worker["head_of_station"],
            serde_json::json!({
                "station": "worker", "at": 1_700_000_100,
                "summary": "Disk at 12% free; two safe houses hold 40 GB of caches.",
                "findings": [{
                    "kind": "disk", "severity": "warning",
                    "text": "12% free under the state directory"
                }],
                "mission": "round-1", "agent": "M"
            }),
            "the Station's latest Head of Station report"
        );

        let anonymous_preview = client
            .get(format!("{base}/v1/user/operations/G/placement-preview"))
            .send()
            .await
            .unwrap();
        assert_eq!(anonymous_preview.status(), StatusCode::UNAUTHORIZED);
        let unauthorized_preview = client
            .get(format!("{base}/v1/user/operations/L/placement-preview"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(unauthorized_preview.status(), StatusCode::FORBIDDEN);
        let preview = client
            .get(format!("{base}/v1/user/operations/G/placement-preview"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(preview.status(), StatusCode::OK);
        let preview: serde_json::Value = preview.json().await.unwrap();
        assert_eq!(preview["operation_id"], "G");
        assert_eq!(preview["policy_station"], "worker");
        assert_eq!(
            preview["choices"],
            serde_json::json!([{
                "station": "worker", "theater": "local", "agent": "007", "handler": "claude",
                "mode": "field", "score": 1.0
            }]),
            "half its capacity running, a quarter of its CPU busy and its memory in use"
        );
        let rejections = preview["rejections"].as_array().unwrap();
        assert!(rejections.iter().any(|r| {
            r["theater"] == "build-box"
                && r["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("has no `workdir`"))
        }));
        assert!(
            rejections
                .iter()
                .any(|r| r["station"] == "hq" && r["reason"] == "it has no execution role")
        );
        assert!(rejections.iter().any(|r| r["station"] == "worker"
            && r["agent"] == "008"
            && r["reason"] == "handler \"codex\" is not available on this Station"));
        let narrowed = client
            .get(format!(
                "{base}/v1/user/operations/G/placement-preview?stations=hq&agent=007"
            ))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        let narrowed: serde_json::Value = narrowed.json().await.unwrap();
        assert!(narrowed["choices"].as_array().unwrap().is_empty());
        assert!(narrowed["rejections"].as_array().unwrap().iter().any(|r| {
            r["station"] == "worker"
                && r["reason"] == "the Mission narrows placement to Stations [hq]"
        }));
        let bad_query = client
            .get(format!(
                "{base}/v1/user/operations/G/placement-preview?unknown=1"
            ))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(bad_query.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            station_board["stations"][0]["theaters"],
            serde_json::json!([
                {"id": "local", "access": "local", "labels": {}, "agents": "auto", "egress": "direct"},
                {
                    "id": "build-box", "access": "ssh", "host": "build.example.net",
                    "labels": {"os": "linux"},
                    "capabilities": {
                        "terminal": true, "run": true, "files": true, "forward": true,
                        "os": "linux", "arch": "x86_64"
                    },
                    "agents": "auto", "egress": "direct", "harnesses": ["claude"],
                    "resources": {
                        "cpus": 1.5, "load_1m": 0.5, "memory_total_mb": 4096,
                        "memory_available_mb": 2048, "sampled_at": 1_700_000_010
                    },
                    "sessions": 1
                }
            ]),
            "a Theater without resources or Sessions carries neither key"
        );
        assert_eq!(
            station_board["stations"][0]["resources"],
            serde_json::json!({
                "cpus": 8.0, "cpu_percent": 25.0, "load_1m": 1.5, "memory_total_mb": 16384,
                "memory_available_mb": 12288, "sampled_at": 1_700_000_000
            })
        );
        assert_eq!(
            station_board["stations"][0]["config"],
            serde_json::json!({
                "revision": "18446744073709551615",
                "refused": ["roster revision 7: Agent \"009\": clearance"],
                "needs_restart": ["theater"]
            }),
            "the configuration a Station runs, as it reported it"
        );
        assert!(
            !serde_json::to_string(&station_board)
                .unwrap()
                .contains("private-host")
        );
        // A substring test would also match the digits of a live timestamp.
        fn holds_number(value: &serde_json::Value, wanted: u64) -> bool {
            match value {
                serde_json::Value::Number(number) => number.as_u64() == Some(wanted),
                serde_json::Value::Array(items) => {
                    items.iter().any(|item| holds_number(item, wanted))
                }
                serde_json::Value::Object(fields) => {
                    fields.values().any(|item| holds_number(item, wanted))
                }
                _ => false,
            }
        }
        assert!(
            !serde_json::to_string(&station_board)
                .unwrap()
                .contains("process_id")
        );
        assert!(!holds_number(&station_board, 7132));

        FileRegistry::open(directory.path())
            .record_usage(
                Some("G"),
                "mission-g",
                "session-g",
                &ai6::UsageTotals {
                    input: 120,
                    output: 30,
                    cost_micro_usd: 4_500,
                    ..ai6::UsageTotals::default()
                },
                1_790_000_000,
            )
            .unwrap();
        // The first attempt was lost with its Station; the second runs elsewhere.
        let registry = FileRegistry::open(directory.path());
        registry
            .open_session(
                ai6::NewSession::new("attempt-1", "mission-g", "hq").with_theater("local"),
                Duration::from_nanos(1),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        registry
            .lose_session("attempt-1", "Station \"hq\" went dark")
            .unwrap();
        registry
            .open_session(
                ai6::NewSession::new("attempt-2", "mission-g", "worker")
                    .with_theater("local")
                    .with_provider_session("session-g")
                    .with_attempt(2, Some("attempt-1".to_owned()))
                    .with_account(ai6::AccountRef::new("gateway", "key"))
                    .with_launch(
                        ai6::SessionLaunch::new("claude", "pipe", "/work")
                            .with_model(Some("gemini-free".to_owned()), None)
                            .with_pool(Some("gateway".to_owned())),
                    ),
                Duration::from_secs(60),
            )
            .unwrap();
        let mission_detail = client
            .get(format!("{base}/v1/operations/G/missions/mission-g"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(mission_detail.status(), StatusCode::OK);
        let mission_detail: serde_json::Value = mission_detail.json().await.unwrap();
        assert_eq!(
            mission_detail["attempts"],
            serde_json::json!([
                {
                    "attempt": 1, "session_id": "attempt-1", "handle": null,
                    "provider_session_id": null,
                    "predecessor": null, "station": "hq", "theater": "local",
                    "status": "lost", "lost": "Station \"hq\" went dark",
                    "checkpoint_branch": null, "route": null, "account": null
                },
                {
                    "attempt": 2, "session_id": "attempt-2", "handle": null,
                    "provider_session_id": "session-g",
                    "predecessor": "attempt-1", "station": "worker", "theater": "local",
                    "status": "active", "lost": null, "checkpoint_branch": null,
                    "route": {"handler": "claude", "model": "gemini-free", "pool": "gateway"},
                    "account": {"pool": "gateway", "account": "key"}
                }
            ])
        );
        assert_eq!(mission_detail["usage"]["total"]["input"], 120);
        assert_eq!(mission_detail["usage"]["total"]["cost_micro_usd"], 4_500);
        assert_eq!(
            mission_detail["usage"]["sessions"]["session-g"]["output"],
            30
        );
        assert_eq!(
            mission_detail["brief"]["prompt"],
            "Review the requested change"
        );
        assert_eq!(mission_detail["brief"]["role_id"], "case-officer");
        assert_eq!(mission_detail["brief"]["environment_keys"][0], "API_TOKEN");
        assert_eq!(mission_detail["brief"]["station_id"], "worker");
        assert_eq!(mission_detail["brief"]["theater_id"], "local");
        assert_eq!(mission_detail["brief"]["mode"], "remote-hands");
        assert_eq!(
            mission_detail["brief"]["provenance"]["origin"],
            "registry_drop"
        );
        assert_eq!(
            mission_detail["brief"]["provenance"]["received_by"],
            "worker"
        );
        assert!(
            !serde_json::to_string(&mission_detail)
                .unwrap()
                .contains("secret-value")
        );
        let hidden_mission = client
            .get(format!("{base}/v1/operations/L/missions/mission-l"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(hidden_mission.status(), StatusCode::NOT_FOUND);

        let registry = FileRegistry::open(directory.path());
        registry
            .open_session(
                ai6::NewSession::new("record-g", "mission-g", "worker")
                    .with_provider_session("session-g")
                    .with_attempt(3, None)
                    .with_exposure(vec![ai6::Exposure {
                        source: "tool:WebFetch".to_owned(),
                        at: 1_790_000_000.0,
                    }]),
                std::time::Duration::from_secs(60),
            )
            .unwrap();
        let viewer_board = client
            .get(format!("{base}/v1/board"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_board.status(), StatusCode::OK);
        let viewer_board: serde_json::Value = viewer_board.json().await.unwrap();
        assert_eq!(viewer_board.as_array().unwrap().len(), 1);
        assert_eq!(viewer_board[0]["operation_id"], "G");
        assert_eq!(
            viewer_board[0]["exposure"],
            serde_json::json!(["tool:WebFetch"])
        );
        assert_eq!(
            viewer_board[0]["station_id"], "worker",
            "from its Registry record"
        );
        assert_eq!(
            viewer_board[0]["agent"], "technical-officer",
            "from its Mission's brief"
        );

        let viewer_missions = client
            .get(format!("{base}/v1/missions"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_missions.status(), StatusCode::OK);
        let viewer_missions: serde_json::Value = viewer_missions.json().await.unwrap();
        assert_eq!(viewer_missions.as_array().unwrap().len(), 1);
        assert_eq!(viewer_missions[0]["operation_id"], "G");
        assert_eq!(viewer_missions[0]["mission_id"], "mission-g");
        assert_eq!(viewer_missions[0]["sessions"][0]["session_id"], "session-g");

        let viewer_read = client
            .post(format!("{base}/v1/user/attach-grants"))
            .bearer_auth(&viewer_token)
            .json(&serde_json::json!({
                "operation_id": "G",
                "mission_id": "mission-g",
                "session_id": "session-g",
                "mode": "read"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_read.status(), StatusCode::OK);
        let read_token = viewer_read.json::<serde_json::Value>().await.unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        let exchanged = client
            .post(format!("{base}/v1/attach"))
            .header("origin", "https://localhost")
            .header("x-forwarded-proto", "https")
            .json(&serde_json::json!({"token": read_token.clone()}))
            .send()
            .await
            .unwrap();
        assert_eq!(exchanged.status(), StatusCode::NO_CONTENT);
        let set_cookie = exchanged
            .headers()
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("Secure"));
        assert!(set_cookie.contains("SameSite=Strict"));
        let cookie = set_cookie.split(';').next().unwrap().to_owned();
        let replayed = client
            .post(format!("{base}/v1/attach"))
            .header("origin", "https://localhost")
            .header("x-forwarded-proto", "https")
            .json(&serde_json::json!({"token": read_token}))
            .send()
            .await
            .unwrap();
        assert_eq!(replayed.status(), StatusCode::UNAUTHORIZED);
        let mut request = format!("ws://{address}/v1/socket/session-g?after=0")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("https://localhost"));
        request
            .headers_mut()
            .insert("x-forwarded-proto", HeaderValue::from_static("https"));
        request
            .headers_mut()
            .insert(COOKIE, HeaderValue::from_str(&cookie).unwrap());
        let (mut socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        socket.close(None).await.unwrap();

        let viewer_write = client
            .post(format!("{base}/v1/user/attach-grants"))
            .bearer_auth(&viewer_token)
            .json(&serde_json::json!({
                "operation_id": "G",
                "mission_id": "mission-g",
                "session_id": "session-g",
                "mode": "write"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(viewer_write.status(), StatusCode::FORBIDDEN);

        let operator_login = login("operator").send().await.unwrap();
        assert_eq!(operator_login.status(), StatusCode::CREATED);
        let operator_token: String =
            operator_login.json::<serde_json::Value>().await.unwrap()["token"]
                .as_str()
                .unwrap()
                .to_owned();
        let operator_g_write = client
            .post(format!("{base}/v1/user/attach-grants"))
            .bearer_auth(&operator_token)
            .json(&serde_json::json!({
                "operation_id": "G",
                "mission_id": "mission-g",
                "session_id": "session-g",
                "mode": "write"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(operator_g_write.status(), StatusCode::OK);
        let operator_l_write = client
            .post(format!("{base}/v1/user/attach-grants"))
            .bearer_auth(&operator_token)
            .json(&serde_json::json!({
                "operation_id": "L",
                "mission_id": "mission-l",
                "session_id": "session-l",
                "mode": "write"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(operator_l_write.status(), StatusCode::FORBIDDEN);

        let signed_out = client
            .post(format!("{base}/v1/auth/logout"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(signed_out.status(), StatusCode::NO_CONTENT);
        let expired_board = client
            .get(format!("{base}/v1/board"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(expired_board.status(), StatusCode::UNAUTHORIZED);

        server.abort();
    }

    #[tokio::test]
    async fn issued_access_token_is_shared_across_gateways_and_revocation_propagates() {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(directory.path());
        let first = Gateway::with_registry(
            GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                test_limits(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
            Arc::new(registry.clone()),
        );
        let second = Gateway::with_registry(
            GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                test_limits(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
            Arc::new(registry),
        );
        let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_a = format!("http://{}", listener_a.local_addr().unwrap());
        let base_b = format!("http://{}", listener_b.local_addr().unwrap());
        let server_a = tokio::spawn(async move { axum::serve(listener_a, first.router()).await });
        let server_b = tokio::spawn(async move { axum::serve(listener_b, second.router()).await });
        let client = reqwest::Client::new();
        let station_token = "m-token-that-is-long-enough-for-testing";

        let issued = client
            .post(format!("{base_a}/v1/station/access-tokens"))
            .bearer_auth(station_token)
            .json(&serde_json::json!({"person_id":"person-007","ttl_seconds":600}))
            .send()
            .await
            .unwrap();
        assert_eq!(issued.status(), StatusCode::CREATED);
        let issued: serde_json::Value = issued.json().await.unwrap();
        let token_id = issued["token_id"].as_str().unwrap().to_owned();
        let secret = issued["token"].as_str().unwrap().to_owned();

        let session = client
            .get(format!("{base_b}/v1/auth/session"))
            .bearer_auth(&secret)
            .send()
            .await
            .unwrap();
        assert_eq!(session.status(), StatusCode::OK);
        let session: serde_json::Value = session.json().await.unwrap();
        assert_eq!(session["token_id"], token_id);
        assert_eq!(session["person_id"], "person-007");

        let revoked = client
            .delete(format!("{base_a}/v1/station/access-tokens/{token_id}"))
            .bearer_auth(station_token)
            .send()
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::OK);
        assert_eq!(
            revoked.json::<serde_json::Value>().await.unwrap()["revoked"],
            true
        );

        let rejected = client
            .get(format!("{base_b}/v1/auth/session"))
            .bearer_auth(&secret)
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        server_a.abort();
        server_b.abort();
    }

    #[tokio::test]
    async fn http_ingress_and_websocket_replay_work_over_loopback() {
        let gateway = gateway();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observed = gateway.clone();
        let server = tokio::spawn(async move { axum::serve(listener, gateway.router()).await });
        let base = format!("http://{address}");
        let client = reqwest::Client::new();

        let unauthorized = client
            .post(format!("{base}/v1/station/sessions"))
            .json(&serde_json::json!({"operation_id":"station-a","mission_id":"mission-a","session_id":"provider-session-a"}))
            .send()
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let authorized = client
            .post(format!("{base}/v1/station/sessions"))
            .bearer_auth("m-token-that-is-long-enough-for-testing")
            .json(&serde_json::json!({"operation_id":"station-a","mission_id":"mission-a","session_id":"provider-session-a"}))
            .send()
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::CREATED);

        let grant = client
            .post(format!("{base}/v1/station/attach-grants"))
            .bearer_auth("m-token-that-is-long-enough-for-testing")
            .json(&serde_json::json!({"operation_id":"station-a","mission_id":"mission-a","session_id":"provider-session-a","mode":"read"}))
            .send()
            .await
            .unwrap();
        assert_eq!(grant.status(), StatusCode::OK);
        let token: serde_json::Value = grant.json().await.unwrap();

        let wrong_origin = client
            .post(format!("{base}/v1/attach"))
            .header("origin", "https://other.example")
            .header("x-forwarded-proto", "https")
            .json(&serde_json::json!({"token":token["token"]}))
            .send()
            .await
            .unwrap();
        assert_eq!(wrong_origin.status(), StatusCode::FORBIDDEN);

        let exchanged = client
            .post(format!("{base}/v1/attach"))
            .header("origin", "https://localhost")
            .header("x-forwarded-proto", "https")
            .json(&serde_json::json!({"token":token["token"]}))
            .send()
            .await
            .unwrap();
        assert_eq!(exchanged.status(), StatusCode::NO_CONTENT);
        let cookie = exchanged
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Path=/v1/socket"));
        let cookie = cookie.split(';').next().unwrap();

        let session = SessionId::new("provider-session-a").unwrap();
        let event = LifecycleEvent {
            operation_id: OperationId::new("station-a").unwrap(),
            mission_id: MissionId::new("mission-a").unwrap(),
            session_id: session.clone(),
            state: SessionState::Working,
            reason: None,
            sequence: 1,
        };
        let (schema, value) = lifecycle_frames(&event).unwrap();
        let terminal = TerminalData::new(
            session.clone(),
            2,
            TerminalDirection::Output,
            vec![0xff, 0x00, 0x80],
        )
        .unwrap();
        let (terminal_schema, terminal_value) = terminal_frames(&terminal).unwrap();
        for (sequence, frame) in [
            (0, schema),
            (1, value),
            (0, terminal_schema),
            (2, terminal_value.clone()),
        ] {
            let ingested = client
                .post(format!(
                    "{base}/v1/station/sessions/{session}/events/{sequence}"
                ))
                .bearer_auth("m-token-that-is-long-enough-for-testing")
                .body(frame)
                .send()
                .await
                .unwrap();
            assert_eq!(ingested.status(), StatusCode::ACCEPTED);
        }

        let request = format!("ws://{address}/v1/socket/{session}?after=0")
            .into_client_request()
            .unwrap();
        let mut request = request;
        request
            .headers_mut()
            .insert("origin", "https://localhost".parse().unwrap());
        request
            .headers_mut()
            .insert("x-forwarded-proto", "https".parse().unwrap());
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
        let (mut socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        let mut received_frames = Vec::new();
        for _ in 0..4 {
            let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let tokio_tungstenite::tungstenite::Message::Binary(frame) = message else {
                panic!("websocket replay must use binary frames");
            };
            received_frames.push(frame.to_vec());
        }
        assert!(received_frames.contains(&terminal_value));
        socket.close(None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if observed
                    .inner
                    .lock()
                    .await
                    .sessions
                    .get(&session)
                    .is_some_and(|record| record.active_clients == 0)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        server.abort();
    }

    #[test]
    fn configuration_requires_https_and_finite_bounds() {
        assert!(
            GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                GatewayLimits::default(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .is_ok()
        );
        assert!(
            GatewayConfig::new(
                "http://localhost",
                "m-token-that-is-long-enough-for-testing",
                test_limits(),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .is_err()
        );
        assert!(
            format!(
                "{:?}",
                GatewayConfig::new(
                    "https://localhost",
                    "very-secret-token-value-0123456789",
                    test_limits(),
                    Duration::from_secs(1),
                    Duration::from_secs(1),
                )
                .unwrap()
            )
            .contains("[REDACTED]")
        );
    }

    #[test]
    fn the_former_ceilings_are_settings() {
        let token = "m-token-that-is-long-enough-for-testing";
        let mut limits = GatewayLimits {
            max_frame_bytes: 4 * 1024 * 1024,
            buffer_budget_bytes: ai6::Limit::UNBOUNDED,
            ..GatewayLimits::default()
        };
        // A frame past 1 MiB and a lease past five minutes are refused by no ceiling now.
        assert!(
            GatewayConfig::new(
                "https://localhost",
                token,
                limits,
                Duration::from_secs(3_600),
                Duration::from_secs(900),
            )
            .is_ok()
        );
        // The budget is still a bound while it is one: the same frames overrun 512 MiB.
        limits.buffer_budget_bytes = GatewayLimits::default().buffer_budget_bytes;
        assert!(
            GatewayConfig::new(
                "https://localhost",
                token,
                limits,
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .is_err()
        );
        // The Station token's least size is a setting too.
        let short = GatewayLimits {
            station_token_min_bytes: 4,
            ..GatewayLimits::default()
        };
        assert!(
            GatewayConfig::new(
                "https://localhost",
                "abcd",
                short,
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn full_agent_input_queue_rejects_new_input_without_growing() {
        let mut limits = test_limits();
        limits.input_queue_capacity = 1;
        let gateway = Gateway::new(
            GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                limits,
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
        );
        let (_, _, session) = register(&gateway).await;
        let token = grant(&gateway, AttachMode::Write).await;
        let ticket = gateway.exchange_attach_token(&token).await.unwrap();
        let writer = gateway.attach(&session, &ticket, 0).await.unwrap();

        for sequence in [1, 2] {
            let input = TerminalData::new(
                session.clone(),
                sequence,
                TerminalDirection::Input,
                vec![sequence as u8],
            )
            .unwrap();
            let (_, frame) = terminal_frames(&input).unwrap();
            let result = gateway
                .submit_input(&session, &writer.connection_id, frame)
                .await;
            if sequence == 1 {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err(), GatewayError::Backpressure);
            }
        }
        assert!(gateway.receive_agent_input().await.is_some());
        assert!(
            gateway
                .receive_agent_input_within(Duration::from_millis(10))
                .await
                .is_none(),
            "an idle poll ends with nothing instead of waiting for ever"
        );
        gateway.disconnect(writer).await;
    }
}
