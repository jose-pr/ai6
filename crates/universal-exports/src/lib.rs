#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

mod controls;
mod login_limit;

use core::future::Future;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt::{self, Write as _};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ai6::{
    AccessTokenRecord, AgentRole, Attempt, AuditRecord, Authenticator, AuthorizationRequest,
    Authorizer, Brief, Directory, FileRegistry, Lookup, MissionProposal, MissionUsage, Person,
    Quarantine, StationSettings, Verdict,
};
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
use futures_util::{SinkExt, StreamExt};
use guatiao::{Map, TryAsRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, broadcast, mpsc};
use uuid::Uuid;

use controls::ControlBook;
pub use controls::ControlStatus;
use login_limit::LoginGuard;
pub use login_limit::LoginLimits;

const MAX_BUFFER_BUDGET_BYTES: usize = 512 * 1024 * 1024;
const MAX_CLIENT_SLOTS: usize = 100_000;

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
}

/// User identity providers used to authenticate and authorize Universal Exports requests.
#[derive(Clone)]
pub struct AccessProviders {
    authenticator: Option<Arc<dyn Authenticator>>,
    directory: Arc<dyn Directory>,
    authorizer: Arc<dyn Authorizer>,
    proxy_identity: Option<ProxyIdentity>,
}

#[derive(Clone)]
struct ProxyIdentity {
    header: HeaderName,
    lookup_by: String,
}

impl AccessProviders {
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
            }),
        }
    }

    /// Adds trusted reverse-proxy identity as a login method alongside password login.
    pub fn with_proxy_identity(mut self, header: HeaderName, lookup_by: impl Into<String>) -> Self {
        self.proxy_identity = Some(ProxyIdentity {
            header,
            lookup_by: lookup_by.into(),
        });
        self
    }
}

impl fmt::Debug for AccessProviders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccessProviders([REDACTED])")
    }
}

impl Default for GatewayLimits {
    fn default() -> Self {
        Self {
            max_sessions: 16,
            max_clients_per_session: 12,
            scrollback_events: 64,
            max_frame_bytes: 256 * 1024,
            max_input_frames_per_second: 40,
            input_queue_capacity: 64,
            fanout_capacity: 8,
        }
    }
}

impl GatewayConfig {
    /// Creates a gateway config and rejects unsafe or unbounded resource settings.
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
            .map(|bytes| bytes.max(256 * 1024));
        let socket_budget =
            client_slots_or_max(limits).checked_mul(socket_buffer.unwrap_or(usize::MAX));
        let total_buffers = history_budget
            .and_then(|history| fanout_budget.and_then(|fanout| history.checked_add(fanout)))
            .and_then(|buffers| buffers.checked_add(input_bytes.unwrap_or(usize::MAX)))
            .and_then(|buffers| socket_budget.and_then(|sockets| buffers.checked_add(sockets)));
        let client_slots = limits
            .max_sessions
            .checked_mul(limits.max_clients_per_session);
        if authority.is_empty()
            || authority.contains(['/', '?', '#', '@'])
            || public_origin.len() > 2048
            || station_token.len() < 32
            || limits.max_sessions == 0
            || limits.max_clients_per_session == 0
            || limits.scrollback_events == 0
            || limits.max_frame_bytes == 0
            || limits.max_frame_bytes > 1024 * 1024
            || limits.max_input_frames_per_second == 0
            || limits.input_queue_capacity == 0
            || limits.fanout_capacity == 0
            || client_slots.is_none_or(|slots| slots > MAX_CLIENT_SLOTS)
            || total_buffers.is_none_or(|bytes| bytes > MAX_BUFFER_BUDGET_BYTES)
            || attach_token_ttl.is_zero()
            || attach_token_ttl > Duration::from_secs(300)
            || write_lease_ttl.is_zero()
            || write_lease_ttl > Duration::from_secs(300)
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
        })
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
    registry: Option<FileRegistry>,
    access: Option<AccessProviders>,
    login_guard: std::sync::Arc<LoginGuard>,
    controls: Arc<std::sync::Mutex<ControlBook>>,
    control_ready: Arc<tokio::sync::Notify>,
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
    lifecycle_sequence: u64,
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
        Self {
            config: std::sync::Arc::new(config),
            inner: std::sync::Arc::new(Mutex::new(Inner::default())),
            input_tx,
            input_rx: std::sync::Arc::new(Mutex::new(input_rx)),
            registry: None,
            access: None,
            login_guard,
            controls: Arc::default(),
            control_ready: Arc::default(),
        }
    }

    /// Creates a gateway backed by the shared Registry for user access tokens.
    pub fn with_registry(config: GatewayConfig, registry: FileRegistry) -> Self {
        let mut gateway = Self::new(config);
        gateway.registry = Some(registry);
        gateway
    }

    /// Creates a gateway backed by the shared Registry and configured user access providers.
    pub fn with_access(
        config: GatewayConfig,
        registry: FileRegistry,
        access: AccessProviders,
    ) -> Self {
        let mut gateway = Self::with_registry(config, registry);
        gateway.access = Some(access);
        gateway
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
                lifecycle_sequence: 0,
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

    /// Returns the current provider Sessions and their latest lifecycle states.
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
                sequence: session.lifecycle_sequence,
                exposure: Vec::new(),
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
                for mission in &mut missions {
                    mission.usage = registry
                        .mission_usage(Some(&mission.operation_id), &mission.mission_id)
                        .ok()
                        .flatten();
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
    /// channel, followed by scrollback newer than `after_sequence`.
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
        replay.extend(
            session
                .history
                .iter()
                .filter(|(sequence, _)| *sequence > after_sequence)
                .map(|(_, frame)| frame.clone()),
        );
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
            return Ok(());
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
                Some(read_text(map, "state")?.to_owned())
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
        if is_value {
            session.last_sequence = sequence;
            if let Some(state) = lifecycle_state {
                session.lifecycle_state = Some(state);
                session.lifecycle_sequence = sequence;
            }
            session.history.push_back((sequence, frame.clone()));
            while session.history.len() > self.config.limits.scrollback_events {
                session.history.pop_front();
            }
        }
        let _ = session.events.send(frame);
        Ok(())
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

    /// Returns the configured public HTTPS origin for same-origin checks.
    pub fn public_origin(&self) -> &str {
        self.config.public_origin()
    }

    /// Returns the Axum service for the Station channel, browser attach and terminal sockets.
    pub fn router(self) -> Router {
        let max_frame_bytes = self.config.limits.max_frame_bytes;
        Router::new()
            .route("/health", get(health))
            .route("/v1/board", get(board))
            .route("/v1/stations", get(stations))
            .route(
                "/v1/user/stations/{station_id}/settings",
                get(get_user_station_settings).put(put_user_station_settings),
            )
            .route("/v1/missions", get(missions))
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
                "/v1/user/operations/{operation_id}/agent-roles",
                get(list_user_agent_roles),
            )
            .route(
                "/v1/user/operations/{operation_id}/agent-roles/{role_id}",
                axum::routing::put(put_user_agent_role).delete(delete_user_agent_role),
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
                "/v1/user/operations/{operation_id}/missions/{mission_id}/actions/{control_id}",
                get(controls::control_status),
            )
            .route("/v1/user/attach-grants", post(issue_user_attach_token))
            .route("/v1/auth/login", post(login))
            .route("/v1/auth/logout", post(logout))
            .route("/v1/auth/session", get(access_session))
            .route("/v1/user/profile", get(user_profile))
            .route("/v1/user/audit", get(user_audit))
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
            .layer(axum::extract::DefaultBodyLimit::max(max_frame_bytes))
            .with_state(self)
    }

    /// Serves the gateway on loopback so browsers reach it only through the
    /// configured HTTPS reverse proxy.
    pub async fn serve_loopback<F>(self, port: u16, shutdown: F) -> Result<(), std::io::Error>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
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
    let authorizer = gateway
        .access
        .as_ref()
        .ok_or_else(ApiError::unavailable)?
        .authorizer
        .clone();
    let (station_records, headquarters) = tokio::task::spawn_blocking(move || {
        let stations = registry
            .stations(Duration::MAX)
            .map_err(|_| ApiError::internal())?;
        let headquarters = registry.headquarters().map_err(|_| ApiError::internal())?;
        Ok::<_, ApiError>((stations, headquarters))
    })
    .await
    .map_err(|_| ApiError::internal())??;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ApiError::internal())?
        .as_secs_f64();
    let visible = tokio::task::spawn_blocking(move || {
        let mut stations = Vec::new();
        for station in station_records {
            let Some(operation_values) = station
                .info()
                .get("operations")
                .and_then(serde_json::Value::as_array)
            else {
                continue;
            };
            let mut operations = Vec::new();
            for operation_value in operation_values {
                let Some(operation_id) = operation_value.as_str() else {
                    continue;
                };
                let Ok(operation_id) = OperationId::new(operation_id.to_owned()) else {
                    continue;
                };
                let allowed = authorizer
                    .authorize(AuthorizationRequest {
                        person: person.clone(),
                        action: "mission.read".to_owned(),
                        resource: format!("operation:{operation_id}"),
                    })
                    .map_err(|_| ApiError::internal())?
                    .allowed;
                if allowed {
                    operations.push(operation_id.to_string());
                }
            }
            if operations.is_empty() {
                continue;
            }
            operations.sort();
            operations.dedup();
            stations.push(board_station(
                &station,
                &operations,
                now,
                headquarters.as_ref(),
            ));
        }
        stations.sort_by(|left: &BoardStation, right| left.station_id.cmp(&right.station_id));
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
        healthy: now - record.seen() <= ai6::GONE_DARK_AFTER.as_secs_f64(),
        headquarters: headquarters
            .is_some_and(|lease| lease.station() == record.station() && lease.expires() > now),
        seen: record.seen(),
        roles: string_list("roles"),
        running: usize_value("running"),
        capacity: usize_value("capacity"),
        operations: operations.to_vec(),
        theaters: station_theaters(info),
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
        })
        .collect()
}

async fn missions(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<Vec<BoardMission>>, ApiError> {
    let sessions = authorized_board_sessions(&gateway, &headers).await?;
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
    let mission_id = MissionId::new(mission_id).map_err(|_| ApiError::not_found())?;
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
    mission.brief = gateway
        .inner
        .lock()
        .await
        .missions
        .get(&(operation_id, mission_id))
        .cloned();
    if let Some(registry) = gateway.registry.clone() {
        let (operation, mission_id) = (mission.operation_id.clone(), mission.mission_id.clone());
        mission.usage = tokio::task::spawn_blocking(move || {
            registry.mission_usage(Some(&operation), &mission_id)
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    }
    Ok(Json(mission))
}

async fn create_user_mission(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    Json(request): Json<CreateUserMissionRequest>,
) -> Result<(StatusCode, Json<CreateUserMissionResponse>), ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    if request.title.trim().is_empty()
        || request.title.chars().count() > 512
        || request.prompt.trim().is_empty()
        || request.prompt.len() > 64 * 1024
        || request
            .agent_id
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 256)
        || request
            .role
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 128)
        || request.tags.len() > 32
        || request
            .tags
            .iter()
            .any(|tag| tag.trim().is_empty() || tag.len() > 64)
        || !valid_station_list(&request.stations)
    {
        return Err(ApiError::bad_request());
    }
    let providers = gateway.access.clone().ok_or_else(ApiError::unavailable)?;
    let actor_id = person.id.clone();
    let resource = format!("operation:{operation_id}");
    let decision = tokio::task::spawn_blocking(move || {
        providers.authorizer.authorize(AuthorizationRequest {
            person: person.clone(),
            action: "mission.create".to_owned(),
            resource,
        })
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    if !decision.allowed {
        return Err(ApiError::forbidden());
    }

    let created = utc_timestamp()?;
    let brief = Brief {
        title: request.title,
        body: request.prompt,
        id: Uuid::new_v4().to_string(),
        source: "universal-exports".to_owned(),
        operation: Some(operation_id.to_string()),
        agent: request.agent_id,
        role: request.role,
        tags: request.tags,
        stations: request.stations,
        priority: 0,
        url: None,
        data: BTreeMap::new(),
        created,
        version: 1,
    };
    let mission_id = MissionId::new(brief.key()).map_err(|_| ApiError::internal())?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
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
    tokio::task::spawn_blocking(move || registry.submit_brief(&brief))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateUserMissionResponse {
            operation_id: operation_id.to_string(),
            mission_id: mission_id.to_string(),
            state: "queued",
        }),
    ))
}

fn valid_station_list(stations: &[String]) -> bool {
    stations.len() <= 64
        && stations.iter().all(|station| {
            !station.is_empty() && station.len() <= 256 && !station.chars().any(char::is_control)
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
        let published = |record: &ai6::StationRecord| {
            record
                .info()
                .get("operation_policies")
                .and_then(|policies| policies.get(&operation))
                .and_then(|policy| {
                    serde_json::from_value::<ai6::OperationPolicy>(policy.clone()).ok()
                })
        };
        let preferred = headquarters
            .as_ref()
            .and_then(|lease| records.iter().find(|r| r.station() == lease.station()))
            .and_then(|record| published(record).map(|policy| (record, policy)));
        let (source, policy) = preferred
            .or_else(|| {
                records
                    .iter()
                    .find_map(|record| published(record).map(|policy| (record, policy)))
            })
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
                ai6::StationFacts::from_record(record, now, ai6::GONE_DARK_AFTER, load)
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
    let brief = tokio::task::spawn_blocking(move || {
        registry.promote_mission_proposal(&operation, &requested_id)
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
    let authorizer = gateway
        .access
        .as_ref()
        .ok_or_else(ApiError::unavailable)?
        .authorizer
        .clone();
    let sessions = gateway.board_sessions().await;
    let registry = gateway.registry.clone();
    let visible = tokio::task::spawn_blocking(move || {
        // Exposure lives in the shared Session records, keyed by the provider's Session ID.
        let exposure: BTreeMap<String, Vec<String>> = registry
            .and_then(|registry| registry.sessions().ok())
            .into_iter()
            .flatten()
            .filter(|record| !record.exposure.is_empty())
            .filter_map(|record| {
                let sources = record.exposure.into_iter().map(|e| e.source).collect();
                Some((record.provider_session_id?, sources))
            })
            .collect();
        let mut visible = Vec::new();
        for mut session in sessions {
            if let Some(sources) = exposure.get(&session.session_id) {
                session.exposure.clone_from(sources);
            }
            let decision = authorizer
                .authorize(AuthorizationRequest {
                    person: person.clone(),
                    action: "mission.read".to_owned(),
                    resource: format!("operation:{}", session.operation_id),
                })
                .map_err(|_| ())?;
            if decision.allowed {
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
            mission_id,
            sessions,
            brief: None,
            usage: None,
        })
        .collect()
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
    #[serde(default = "default_access_token_ttl")]
    ttl_seconds: u64,
}

fn default_access_token_ttl() -> u64 {
    3_600
}

#[derive(Serialize)]
struct IssuedAccessToken {
    token_id: String,
    token: String,
    expires_at: f64,
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
}

#[derive(Serialize)]
struct UserOperationAccess {
    operation_id: String,
    actions: Vec<String>,
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
    configured: bool,
    execution_enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateStationSettings {
    max_concurrent_missions: usize,
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
    if !(60..=86_400).contains(&request.ttl_seconds) {
        return Err(ApiError::bad_request());
    }
    let registry = gateway.registry.clone().ok_or_else(ApiError::internal)?;
    let issued =
        issue_access_token_record(registry, request.person_id, request.ttl_seconds).await?;
    Ok((StatusCode::CREATED, Json(issued)))
}

async fn login(
    State(gateway): State<Gateway>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<(StatusCode, Json<IssuedAccessToken>), ApiError> {
    if headers.contains_key(ORIGIN) {
        require_origin(&gateway, &headers)?;
    }
    if !(60..=86_400).contains(&request.ttl_seconds) {
        return Err(ApiError::bad_request());
    }
    let providers = gateway.access.clone().ok_or_else(ApiError::unavailable)?;
    let ttl_seconds = request.ttl_seconds;
    let person = if let Some(proxy) = &providers.proxy_identity {
        if let Some(value) = headers.get(&proxy.header) {
            require_origin(&gateway, &headers)?;
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
                .ok_or_else(ApiError::unauthorized)?
        } else {
            authenticate_password(&gateway, &providers, &headers, connection, request).await?
        }
    } else {
        authenticate_password(&gateway, &providers, &headers, connection, request).await?
    };
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let issued = issue_access_token_record(registry, person.id, ttl_seconds).await?;
    Ok((StatusCode::CREATED, Json(issued)))
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
    registry: FileRegistry,
    person_id: String,
    ttl_seconds: u64,
) -> Result<IssuedAccessToken, ApiError> {
    let issued_at = unix_timestamp()?;
    let expires_at = issued_at + ttl_seconds as f64;
    let token_id = Uuid::new_v4().to_string();
    // Two independent UUIDs provide 256 random bits without a separate token crate.
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let digest = access_token_digest(&token);
    let record = AccessTokenRecord::try_new(
        token_id.clone(),
        digest,
        person_id,
        issued_at,
        expires_at,
        None,
    )
    .map_err(|_| ApiError::bad_request())?;
    tokio::task::spawn_blocking(move || registry.store_access_token(&record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(IssuedAccessToken {
        token_id,
        token,
        expires_at,
    })
}

async fn resolve_user(gateway: &Gateway, token: &str) -> Result<Person, ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let digest = access_token_digest(token);
    let record = tokio::task::spawn_blocking(move || registry.find_access_token(&digest))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::unauthorized)?;
    let directory = gateway
        .access
        .as_ref()
        .ok_or_else(ApiError::unavailable)?
        .directory
        .clone();
    tokio::task::spawn_blocking(move || {
        directory.lookup(Lookup {
            by: "id".to_owned(),
            value: record.person_id().to_owned(),
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
    let resource = format!("operation:{}", grant.operation_id);
    let decision = tokio::task::spawn_blocking(move || {
        providers.authorizer.authorize(AuthorizationRequest {
            person,
            action: action.to_owned(),
            resource,
        })
    })
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
    let authorizer = gateway
        .access
        .as_ref()
        .ok_or_else(ApiError::unavailable)?
        .authorizer
        .clone();
    let authorization_person = person.clone();
    let (operations, stations) = tokio::task::spawn_blocking(move || {
        let mut operation_ids = BTreeSet::new();
        let mut stations = Vec::new();
        for station in &station_records {
            let station_id = station.station().to_owned();
            let mut actions = Vec::new();
            for action in ["station.read", "station.configure", "audit.read"] {
                let allowed = authorizer
                    .authorize(AuthorizationRequest {
                        person: authorization_person.clone(),
                        action: action.to_owned(),
                        resource: format!("station:{station_id}"),
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
            for action in [
                "mission.read",
                "mission.create",
                "mission.control",
                "session.read",
                "session.input",
                "audit.read",
                "agent.read",
                "agent.configure",
            ] {
                let allowed = authorizer
                    .authorize(AuthorizationRequest {
                        person: authorization_person.clone(),
                        action: action.to_owned(),
                        resource: format!("operation:{operation_id}"),
                    })
                    .map_err(|_| ApiError::internal())?
                    .allowed;
                if allowed {
                    actions.push(action.to_owned());
                }
            }
            if !actions.is_empty() {
                operations.push(UserOperationAccess {
                    operation_id,
                    actions,
                });
            }
        }
        stations.sort_by(|left, right| left.station_id.cmp(&right.station_id));
        Ok::<_, ApiError>((operations, stations))
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
    }))
}

async fn authorize_operation_action(
    gateway: &Gateway,
    person: Person,
    operation_id: &OperationId,
    action: &'static str,
) -> Result<(), ApiError> {
    authorize_resource_action(gateway, person, format!("operation:{operation_id}"), action).await
}

async fn authorize_resource_action(
    gateway: &Gateway,
    person: Person,
    resource: String,
    action: &'static str,
) -> Result<(), ApiError> {
    let authorizer = gateway
        .access
        .as_ref()
        .ok_or_else(ApiError::unavailable)?
        .authorizer
        .clone();
    let decision = tokio::task::spawn_blocking(move || {
        authorizer.authorize(AuthorizationRequest {
            person,
            action: action.to_owned(),
            resource,
        })
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    if decision.allowed {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

async fn ensure_station_registered(
    registry: &FileRegistry,
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
    let (max_concurrent_missions, configured) = match settings {
        Some(settings) => (settings.max_concurrent_missions(), true),
        None => (
            station
                .info()
                .get("capacity")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or_default(),
            false,
        ),
    };
    Ok(Json(StationSettingsResponse {
        station_id,
        max_concurrent_missions,
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
    tokio::task::spawn_blocking(move || write_registry.set_station_settings(&station, &settings))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::bad_request())?;
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id,
        action: "station.settings.update".to_owned(),
        operation: String::new(),
        resource: Some(format!("station:{station_id}")),
        target: "max_concurrent_missions".to_owned(),
        at: unix_timestamp()?,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&audit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_operation_registered(
    registry: &FileRegistry,
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
    let limit = query.limit.unwrap_or(100);
    if limit == 0 || limit > 500 {
        return Err(ApiError::bad_request());
    }
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let authorizer = gateway
        .access
        .as_ref()
        .ok_or_else(ApiError::unavailable)?
        .authorizer
        .clone();
    if let Some(operation_id) = &operation_filter {
        let scoped_authorizer = authorizer.clone();
        let scoped_person = person.clone();
        let resource = format!("operation:{operation_id}");
        let allowed = tokio::task::spawn_blocking(move || {
            scoped_authorizer.authorize(AuthorizationRequest {
                person: scoped_person,
                action: "audit.read".to_owned(),
                resource,
            })
        })
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
            if operation_filter
                .as_ref()
                .is_some_and(|operation_id| operation_id != &record.operation)
            {
                continue;
            }
            let resource = record
                .resource
                .clone()
                .unwrap_or_else(|| format!("operation:{}", record.operation));
            if station_filter
                .as_ref()
                .is_some_and(|station_id| resource != format!("station:{station_id}"))
            {
                continue;
            }
            let allowed = authorizer
                .authorize(AuthorizationRequest {
                    person: person.clone(),
                    action: "audit.read".to_owned(),
                    resource,
                })
                .map_err(|_| ApiError::internal())?
                .allowed;
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

async fn logout(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let digest = access_token_digest(bearer(&headers)?);
    let record = tokio::task::spawn_blocking(move || registry.find_access_token(&digest))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::unauthorized)?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let token_id = record.token_id().to_owned();
    let revoked = tokio::task::spawn_blocking(move || registry.revoke_access_token(&token_id))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    if !revoked {
        return Err(ApiError::unauthorized());
    }
    Ok(StatusCode::NO_CONTENT)
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
    let mut hasher = Sha256::new();
    hasher.update(b"ai6-access-token\0");
    hasher.update(token.as_bytes());
    let mut digest = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(digest, "{byte:02x}");
    }
    digest
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
    let days = (seconds / 86_400) as i64;
    let day_seconds = seconds % 86_400;
    let shifted_days = days + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
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
}

#[derive(Serialize)]
struct CreateUserMissionResponse {
    operation_id: String,
    mission_id: String,
    state: &'static str,
}

#[derive(Serialize)]
struct MissionProposalDecisionResponse {
    operation_id: String,
    proposal_id: String,
    mission_id: Option<String>,
    state: &'static str,
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
}

/// One provider Session entry returned by the public board API.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardSession {
    /// Project that owns the Mission.
    pub operation_id: String,
    /// AI6 Mission ID.
    pub mission_id: String,
    /// Provider-issued resumable Session ID.
    pub session_id: String,
    /// Latest Mission state, absent until the first lifecycle event.
    pub state: Option<String>,
    /// Sequence of the latest lifecycle event, or zero before one arrives.
    pub sequence: u64,
    /// The untrusted input the Session has read (`brief:source`, `cable:SENDER`,
    /// `tool:NAME`, `review`), from its Registry record; empty when it has read none.
    pub exposure: Vec<String>,
}

/// One registered Mission with the provider Sessions known to Universal Exports.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardMission {
    /// Project that owns the Mission.
    pub operation_id: String,
    /// AI6 Mission ID.
    pub mission_id: String,
    /// Provider Sessions registered for this Mission.
    pub sessions: Vec<BoardSession>,
    /// Exact Mission brief and sanitized launch manifest published by AI6, when known.
    pub brief: Option<LaunchManifest>,
    /// Tokens and cost the Mission's Sessions consumed, from the shared Registry ledger;
    /// absent until a provider reports usage.
    pub usage: Option<MissionUsage>,
}

/// AI6 Station overview derived from the shared check-in Registry.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StationBoard {
    /// Registered daemon Stations visible through an authorized Operation.
    pub stations: Vec<BoardStation>,
    /// Current elected Headquarters Station, when visible and its lease is live.
    pub headquarters_station: Option<String>,
}

/// One daemon Station and the Operations it hosts for this user.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardStation {
    /// Daemon Station identity.
    pub station_id: String,
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
    /// Settings looser than their defaults.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub weakened: Vec<String>,
}

/// Mission instructions and non-secret launch metadata published by AI6.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LaunchManifest {
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
    /// The Station that supervises the Session, when the publishing Station reported it.
    #[serde(default)]
    pub station_id: Option<String>,
    /// The Theater the harness runs in, when the publishing Station reported it.
    #[serde(default)]
    pub theater_id: Option<String>,
    /// Whether the harness runs on the Theater (`field`) or on the Station, acting on the
    /// Theater through its transport (`remote-hands`); absent from older Stations.
    #[serde(default)]
    pub mode: Option<String>,
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
    Ok((
        StatusCode::CREATED,
        Json(RegisterSessionResponse { next_sequence }),
    ))
}

#[derive(Serialize)]
struct RegisterSessionResponse {
    next_sequence: u64,
}

async fn publish_lifecycle(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((session, sequence)): Path<(String, u64)>,
    Json(request): Json<LifecycleRequest>,
) -> Result<StatusCode, ApiError> {
    let state = SessionState::parse(&request.state).ok_or_else(ApiError::bad_request)?;
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

async fn next_agent_input(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    let input = gateway
        .receive_agent_input()
        .await
        .ok_or_else(ApiError::internal)?;
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
        .max(256 * 1024);
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
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == "ai6_attach" && !value.is_empty()).then_some(value)
    })
}

struct ApiError {
    status: StatusCode,
    message: &'static str,
    retry_after_seconds: Option<u64>,
}

impl ApiError {
    fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: "resource is not available",
            retry_after_seconds: None,
        }
    }

    fn bad_request() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: "request is invalid",
            retry_after_seconds: None,
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "authentication is required",
            retry_after_seconds: None,
        }
    }

    fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: "request is not authorized",
            retry_after_seconds: None,
        }
    }

    fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "gateway operation failed",
            retry_after_seconds: None,
        }
    }

    fn unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "user access providers are unavailable",
            retry_after_seconds: None,
        }
    }

    fn too_many_attempts(wait: Duration) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "too many login attempts; try again later",
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
            message: "gateway request was refused",
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

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
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
        IncomingFrame::Lifecycle(_) | IncomingFrame::Control(_) | IncomingFrame::Closed(_) => {
            Err(GatewayError::InvalidFrame)
        }
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
    use ai6::RoleAuthorizer;
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
    async fn typed_m_publisher_encodes_failed_state_and_binary_terminal_output() {
        let gateway = gateway();
        let (station, mission, session) = register(&gateway).await;
        gateway
            .publish_lifecycle(
                "m-token-that-is-long-enough-for-testing",
                LifecycleEvent {
                    operation_id: station,
                    mission_id: mission,
                    session_id: session.clone(),
                    state: SessionState::Failed,
                    sequence: 1,
                },
            )
            .await
            .unwrap();
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
                    "environment_keys": ["API_TOKEN"]
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
        assert_eq!(
            brief.station_id, None,
            "a registration without placement still loads"
        );

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
            registry,
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

    #[tokio::test]
    async fn login_is_refused_without_being_judged_past_the_tries_of_an_address() {
        let (base, server, _directory) = login_gateway(LoginLimits {
            per_address: std::num::NonZeroU32::new(3).unwrap(),
            per_name: std::num::NonZeroU32::new(100).unwrap(),
            address_header: None,
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

        let authorizer = ai6::role_authorizer_from_toml(
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
            "#,
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
            "theaters".to_owned(),
            serde_json::json!([
                {"id": "local", "access": "local", "labels": {}},
                {
                    "id": "build-box", "access": "ssh", "host": "build.example.net",
                    "labels": {"os": "linux"},
                    "capabilities": {
                        "terminal": true, "run": true, "files": true, "forward": true,
                        "os": "linux", "arch": "x86_64"
                    },
                    "harnesses": ["claude"]
                }
            ]),
        );
        worker_info.insert("handlers".to_owned(), serde_json::json!(["claude"]));
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
                },
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
            registry,
            AccessProviders::new(
                Arc::new(TestAuthenticator),
                Arc::new(TestDirectory),
                Arc::new(authorizer),
            )
            .with_proxy_identity(HeaderName::from_static("x-forwarded-user"), "name"),
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
                    prompt: "Review the requested change".to_owned(),
                    agent_id: Some("technical-officer".to_owned()),
                    role_id: Some("case-officer".to_owned()),
                    handler_provider: "claude".to_owned(),
                    terminal_provider: "pty".to_owned(),
                    working_directory: "C:/stations/field".to_owned(),
                    model: Some("claude-sonnet".to_owned()),
                    effort: Some("high".to_owned()),
                    environment_keys: vec!["API_TOKEN".to_owned()],
                    station_id: Some("worker".to_owned()),
                    theater_id: Some("local".to_owned()),
                    mode: Some("remote-hands".to_owned()),
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

        let proxy_login = client
            .post(format!("{base}/v1/auth/login"))
            .header("origin", "https://localhost")
            .header("x-forwarded-proto", "https")
            .header("x-forwarded-user", "viewer")
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
                "mission.read",
                "mission.create",
                "session.read",
                "session.input",
                "audit.read",
                "agent.read",
                "agent.configure"
            ])
        );
        let station_access = operator_profile["stations"].as_array().unwrap();
        let worker_access = station_access
            .iter()
            .find(|station| station["station_id"] == "worker")
            .unwrap();
        assert_eq!(
            worker_access["actions"],
            serde_json::json!(["station.read", "station.configure", "audit.read"])
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
                "mode": "field"
            }])
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
                    "agents": "auto", "egress": "direct", "harnesses": ["claude"]
                }
            ])
        );
        assert!(
            !serde_json::to_string(&station_board)
                .unwrap()
                .contains("private-host")
        );
        assert!(
            !serde_json::to_string(&station_board)
                .unwrap()
                .contains("7132")
        );

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
        let mission_detail = client
            .get(format!("{base}/v1/operations/G/missions/mission-g"))
            .bearer_auth(&viewer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(mission_detail.status(), StatusCode::OK);
        let mission_detail: serde_json::Value = mission_detail.json().await.unwrap();
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
            registry.clone(),
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
            registry,
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
        gateway.disconnect(writer).await;
    }
}
