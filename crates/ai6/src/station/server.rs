//! Local daemon HTTP service.

#[cfg(feature = "cables-mcp")]
mod cables_mcp;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::directorates::m::cover::{CoverAudit, CoverReport};
use crate::directorates::m::identity::MAX_ID_BYTES;
use crate::directorates::m::mission::{
    LaunchManifest, Mission, MissionError, MissionState, MissionStore, SessionHold,
};
use crate::directorates::m::placement::{self, OperationPolicy, PlacementRequest, StationFacts};
use crate::directorates::m::roster;
use crate::directorates::m::roster::AgentRole;
use crate::directorates::m::roster::Roster;
use crate::directorates::m::theaters::{LOCAL_THEATER, Theater, TheaterAccess, Theaters};
use crate::directorates::medical::watchdog::{Watchdog, WatchdogConfig};
use crate::directorates::q::safehouse::{SafeHouseReport, SafeHouseSpec};
use crate::directorates::q::terminal::TerminalBridge;
use crate::directorates::registry::cables::{
    CableConfig, CableDelivery, CableEndpoint, CableHeldDelivery, CablePrincipal, CableRelay,
    CableRelayError, CableRelease, CableSendRequest, CableTarget,
};
use crate::directorates::registry::sessions::{
    NewSession, SessionError, SessionStatus, SessionUpdate,
};
use crate::directorates::registry::store::{
    Brief, DEFAULT_LEASE, FileRegistry, GONE_DARK_AFTER, Lease,
};
use crate::directorates::section_00::agent::{
    AgentEvent, AgentMessage, AgentResume, AgentSession, AgentStart,
};
use crate::station::plugins;
#[cfg(feature = "publisher")]
use crate::station::publisher::MissionPublication;
#[cfg(feature = "publisher")]
use crate::station::publisher::{self, Config as PublisherConfig, Publisher, SessionTelemetry};
use crate::station::safehouse::SafeHouse;
use crate::station::sources::SourceDesk;
use crate::{MissionId, SessionId};

/// The loopback address used by the daemon's private health endpoint.
pub const DEFAULT_LISTEN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 7081);
/// The terminal provider a Station prefers when several are available and none is pinned.
pub const DEFAULT_TERMINAL: &str = "pty";
/// Default number of Agent Sessions this daemon may start concurrently.
pub const DEFAULT_MAX_CONCURRENT_MISSIONS: usize = 4;
const EVENT_QUEUE_LIMIT: usize = 256;
const SCHEDULER_INTERVAL: Duration = Duration::from_millis(500);
const SCHEDULER_RETRY_DELAY: Duration = Duration::from_secs(5);
const STATION_CHECK_IN_INTERVAL: Duration = Duration::from_secs(15);
const INTAKE_MAX_ATTEMPTS: u32 = 3;
const MAX_PUBLISHED_AGENT_ROLE_CATALOG_BYTES: usize = 512 * 1024;

/// A capability advertised by an AI6 execution Station.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StationRole {
    /// May be elected Headquarters and assign incoming briefs.
    Control,
    /// May run Missions leased to this Station.
    Execution,
}

impl StationRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Execution => "execution",
        }
    }
}

/// Role and placement labels advertised by one daemon Station.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationConfig {
    roles: BTreeSet<StationRole>,
    labels: BTreeMap<String, String>,
    terminal: Option<String>,
    /// Theaters declared in code; `serve` merges them over `theaters.toml` into `theaters`.
    extra_theaters: Vec<Theater>,
    theaters: Theaters,
}

impl StationConfig {
    /// Replaces the Station's capabilities.
    #[must_use]
    pub fn with_roles(mut self, roles: impl IntoIterator<Item = StationRole>) -> Self {
        self.roles = roles.into_iter().collect();
        self
    }

    /// Adds or replaces a placement label.
    #[must_use]
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// Pins the terminal provider this Station runs harnesses in. Without it the Station
    /// uses an available one, preferring [`DEFAULT_TERMINAL`]: every terminal provider
    /// fulfils the same contract, so the choice belongs to where the Session runs, not to
    /// the Agent or its handler.
    #[must_use]
    pub fn with_terminal(mut self, provider: impl Into<String>) -> Self {
        self.terminal = Some(provider.into());
        self
    }

    /// Declares a Theater this Station reaches, over any in `theaters.toml` with the same ID.
    /// The Station's own host is always reachable as the `local` Theater.
    #[must_use]
    pub fn with_theater(mut self, theater: Theater) -> Self {
        self.extra_theaters.push(theater);
        self
    }
}

impl Default for StationConfig {
    fn default() -> Self {
        Self {
            roles: [StationRole::Control, StationRole::Execution]
                .into_iter()
                .collect(),
            labels: BTreeMap::new(),
            terminal: None,
            extra_theaters: Vec::new(),
            theaters: Theaters::default(),
        }
    }
}

/// Configuration for the daemon listener, plugins, and optional Universal Exports publisher.
pub struct ServeConfig {
    address: SocketAddr,
    plugin_paths: Vec<PathBuf>,
    config_dir: Option<PathBuf>,
    station_id: Option<String>,
    station: StationConfig,
    max_concurrent_missions: usize,
    watchdog: WatchdogConfig,
    #[cfg(feature = "publisher")]
    publisher: Option<PublisherConfig>,
}

impl ServeConfig {
    /// Creates a daemon configuration with the default roster path and Mission capacity.
    pub fn new(address: SocketAddr) -> Self {
        Self {
            address,
            plugin_paths: Vec::new(),
            config_dir: None,
            station_id: None,
            station: StationConfig::default(),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            #[cfg(feature = "publisher")]
            publisher: None,
        }
    }

    /// Sets directories and library files scanned for AgentHandler and Terminal providers.
    pub fn with_plugin_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.plugin_paths = paths.into_iter().collect();
        self
    }

    /// Sets the directory containing `operations.toml` and `agents.toml`.
    pub fn with_config_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_dir = Some(path.into());
        self
    }

    /// Sets the daemon's shared Registry and managed Agent Cable identity.
    pub fn with_station_id(mut self, station_id: impl Into<String>) -> Self {
        self.station_id = Some(station_id.into());
        self
    }

    /// Sets this daemon's Headquarters and execution capabilities and placement labels.
    #[must_use]
    pub fn with_station_config(mut self, station: StationConfig) -> Self {
        self.station = station;
        self
    }

    /// Sets the daemon's global Mission concurrency limit.
    pub fn with_max_concurrent_missions(mut self, limit: usize) -> Self {
        self.max_concurrent_missions = limit;
        self
    }

    /// Sets the thresholds used to detect stalled, blocked and looping Agent Sessions.
    pub fn with_watchdog(mut self, watchdog: WatchdogConfig) -> Self {
        self.watchdog = watchdog;
        self
    }

    /// Enables bounded lifecycle and terminal publication to Universal Exports.
    #[cfg(feature = "publisher")]
    pub fn with_publisher(mut self, config: PublisherConfig) -> Self {
        self.publisher = Some(config);
        self
    }
}

/// Runs the daemon's local service listener.
///
/// The listener is private to the local host. User-facing Operation and Mission
/// operations belong to Universal Exports's authenticated API.
pub async fn serve<F>(address: SocketAddr, shutdown: F) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_with_options(ServeConfig::new(address), shutdown).await
}

/// Runs the daemon with an explicit global concurrent Mission limit.
pub async fn serve_with_capacity<F>(
    address: SocketAddr,
    max_concurrent_missions: usize,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_with_options(
        ServeConfig::new(address).with_max_concurrent_missions(max_concurrent_missions),
        shutdown,
    )
    .await
}

/// Runs the daemon with Guatiao AgentHandler and Terminal libraries from `plugin_paths`.
pub async fn serve_with_plugins<F>(
    address: SocketAddr,
    plugin_paths: &[PathBuf],
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_with_options(
        ServeConfig::new(address).with_plugin_paths(plugin_paths.iter().cloned()),
        shutdown,
    )
    .await
}

/// Runs the daemon with AgentHandler and Terminal libraries and a global Mission limit.
pub async fn serve_with_plugins_and_capacity<F>(
    address: SocketAddr,
    plugin_paths: &[PathBuf],
    max_concurrent_missions: usize,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_with_options(
        ServeConfig::new(address)
            .with_plugin_paths(plugin_paths.iter().cloned())
            .with_max_concurrent_missions(max_concurrent_missions),
        shutdown,
    )
    .await
}

/// Runs the daemon using the registered Operations and Agents from `config_dir`.
pub async fn serve_with_config_dir<F>(
    address: SocketAddr,
    plugin_paths: &[PathBuf],
    config_dir: &Path,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_with_options(
        ServeConfig::new(address)
            .with_plugin_paths(plugin_paths.iter().cloned())
            .with_config_dir(config_dir),
        shutdown,
    )
    .await
}

/// Runs the daemon from its roster/config directory with an explicit Mission limit.
pub async fn serve_with_config_dir_and_capacity<F>(
    address: SocketAddr,
    plugin_paths: &[PathBuf],
    config_dir: &Path,
    max_concurrent_missions: usize,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_with_options(
        ServeConfig::new(address)
            .with_plugin_paths(plugin_paths.iter().cloned())
            .with_config_dir(config_dir)
            .with_max_concurrent_missions(max_concurrent_missions),
        shutdown,
    )
    .await
}

fn recover_startup_missions(missions: &mut MissionStore) -> io::Result<BTreeSet<String>> {
    let pending = missions
        .list()
        .into_iter()
        .filter(|mission| mission.state == MissionState::Working)
        .map(|mission| mission.mission_id.to_string())
        .collect();
    missions
        .recover_working()
        .map_err(|error| io::Error::other(format!("Mission recovery: {error:?}")))?;
    Ok(pending)
}

/// Runs the daemon with explicit configuration, including optional Universal Exports publication.
pub async fn serve_with_options<F>(mut config: ServeConfig, shutdown: F) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    if config.station.roles.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a Station needs at least one role",
        ));
    }
    if config.max_concurrent_missions == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "max concurrent Missions must be greater than zero",
        ));
    }
    let config_dir = match config.config_dir.take() {
        Some(path) => path,
        None => roster::config_dir().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("AI6 configuration: {error}"),
            )
        })?,
    };
    let roster = Roster::load(&config_dir).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("AI6 roster: {error}"))
    })?;
    let mut theaters = Theaters::load(&config_dir).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("AI6 Theaters: {error}"))
    })?;
    for theater in std::mem::take(&mut config.station.extra_theaters) {
        theaters.insert(theater).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("AI6 Theaters: {error}"),
            )
        })?;
    }
    config.station.theaters = theaters;
    let station_id = config
        .station_id
        .take()
        .or_else(|| std::env::var("AI6_STATION_ID").ok())
        .unwrap_or_else(daemon_station_id);
    crate::directorates::registry::store::validate_station_id(&station_id)?;
    CableEndpoint::try_new(
        "validation-agent".to_owned(),
        "validation-operation".to_owned(),
        None,
        station_id.clone(),
        None,
        None,
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let cable_config = match fs::read_to_string(config_dir.join("cables.toml")) {
        Ok(source) => CableConfig::parse_toml(&source).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("AI6 Cable configuration: {error}"),
            )
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => CableConfig::default(),
        Err(error) => return Err(error),
    };
    let mission_store = crate::station::safehouse::state_dir()
        .map_err(io::Error::other)?
        .join("missions.json");
    let mut missions = MissionStore::open(mission_store)?;
    let intake = FileRegistry::from_environment()?;
    let startup_recovery = recover_startup_missions(&mut missions)?;
    let listener = TcpListener::bind(config.address).await?;
    #[cfg(feature = "publisher")]
    let publisher = config.publisher.take().map(Publisher::start).transpose()?;
    let roster = Arc::new(roster);
    let intake = Arc::new(intake);
    let missions = Arc::new(Mutex::new(missions));
    let sessions = Arc::new(Mutex::new(BTreeMap::new()));
    let cable_roster = roster.clone();
    let cable_missions = missions.clone();
    let cable_sessions = sessions.clone();
    let cable_site = station_id.clone();
    let (sender_rate, sender_burst, scope_rate, scope_burst) = cable_config.quota_limits();
    let cables = CableRelay::new(
        intake.clone(),
        cable_config
            .policy()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?,
        move || cable_endpoints(&cable_roster, &cable_missions, &cable_sessions, &cable_site),
        cable_config.channels().clone(),
    )
    .with_limits(
        cable_config.max_fanout(),
        sender_rate,
        sender_burst,
        scope_rate,
        scope_burst,
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let state = DaemonState {
        plugins: Arc::new(plugins::Plugins::scan(&config.plugin_paths)),
        roster,
        intake,
        missions,
        sessions,
        cables: Arc::new(cables),
        station_id,
        station: config.station,
        starting: Arc::new(Mutex::new(BTreeSet::new())),
        max_concurrent_missions: config.max_concurrent_missions,
        watchdog: config.watchdog.clone(),
        retry_after: Arc::new(Mutex::new(BTreeMap::new())),
        startup_recovery: Arc::new(Mutex::new(startup_recovery)),
        events: Arc::new(Mutex::new(BTreeMap::new())),
        event_ready: Arc::new(Notify::new()),
        stopping: Arc::new(AtomicBool::new(false)),
        event_pumps: Arc::new(Mutex::new(Vec::new())),
        #[cfg(feature = "publisher")]
        publisher,
    };
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/internal/agent-handlers", get(list_agent_handlers))
        .route("/internal/agent-roles", get(list_agent_roles))
        .route("/internal/terminals", get(list_terminals))
        .route("/internal/plugins", get(plugin_scan_summary))
        .route("/internal/intake/briefs", get(list_intake_briefs))
        .route(
            "/internal/missions",
            get(list_missions).post(create_mission),
        )
        .route("/internal/missions/{mission_id}", get(get_mission))
        .route(
            "/internal/missions/{mission_id}/state",
            axum::routing::post(transition_mission),
        )
        .route(
            "/internal/missions/{mission_id}/sessions",
            axum::routing::post(attach_session),
        )
        .route(
            "/internal/missions/{mission_id}/start",
            axum::routing::post(start_mission),
        )
        .route(
            "/internal/missions/{mission_id}/resume",
            axum::routing::post(resume_mission),
        )
        .route(
            "/internal/missions/{mission_id}/interrupt",
            axum::routing::post(interrupt_mission),
        )
        .route(
            "/internal/missions/{mission_id}/messages",
            axum::routing::post(send_message),
        )
        .route(
            "/internal/missions/{mission_id}/cables",
            get(discover_cables).post(send_cable),
        )
        .route(
            "/internal/missions/{mission_id}/cables/inbox",
            get(cable_inbox),
        )
        .route(
            "/internal/missions/{mission_id}/cables/held",
            get(held_cables),
        )
        .route(
            "/internal/missions/{mission_id}/cables/{cable_id}/release",
            axum::routing::post(release_cable),
        )
        .route(
            "/internal/missions/{mission_id}/cables/{cable_id}/ack",
            axum::routing::post(acknowledge_cable),
        )
        .route(
            "/internal/missions/{mission_id}/cables/{cable_id}/reply",
            axum::routing::post(reply_cable),
        )
        .route(
            "/internal/missions/{mission_id}/events/next",
            get(next_event),
        );
    #[cfg(feature = "publisher")]
    let app = app.route("/internal/publisher", get(publisher_health));
    let app = app.with_state(state.clone());
    let scheduler = tokio::spawn(scheduler_loop(state.clone()));
    let stopping = state.stopping.clone();
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.await;
            stopping.store(true, Ordering::Release);
        })
        .await;
    state.stopping.store(true, Ordering::Release);
    let _ = tokio::time::timeout(Duration::from_secs(2), scheduler).await;
    let event_pumps = std::mem::take(
        &mut *state
            .event_pumps
            .lock()
            .map_err(|_| io::Error::other("event pump registry lock was poisoned"))?,
    );
    let drain = async move {
        for event_pump in event_pumps {
            let _ = event_pump.await;
        }
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), drain).await;
    served.map_err(io::Error::other)
}

async fn list_agent_handlers(State(state): State<DaemonState>) -> Json<Vec<plugins::ProviderInfo>> {
    Json(state.plugins.handlers())
}

async fn list_agent_roles(
    State(state): State<DaemonState>,
) -> Json<Vec<crate::directorates::m::roster::AgentRole>> {
    Json(state.roster.roles().cloned().collect())
}

#[cfg(feature = "publisher")]
async fn publisher_health(State(state): State<DaemonState>) -> Json<publisher::Health> {
    Json(state.publisher.as_ref().map_or(
        publisher::Health {
            enabled: false,
            healthy: false,
            error: None,
        },
        |publisher| publisher.health(),
    ))
}

async fn list_terminals(State(state): State<DaemonState>) -> Json<Vec<plugins::ProviderInfo>> {
    Json(state.plugins.terminals())
}

async fn plugin_scan_summary(State(state): State<DaemonState>) -> Json<plugins::PluginScanSummary> {
    Json(state.plugins.scan_summary().clone())
}

async fn list_intake_briefs(
    State(state): State<DaemonState>,
) -> Result<Json<Vec<Brief>>, (StatusCode, String)> {
    state
        .intake
        .list_new()
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

fn cable_endpoints(
    roster: &Roster,
    missions: &Mutex<MissionStore>,
    sessions: &Mutex<BTreeMap<String, AgentOperation>>,
    station_id: &str,
) -> Vec<CableEndpoint> {
    let Ok(missions) = missions.lock() else {
        return Vec::new();
    };
    let Ok(sessions) = sessions.lock() else {
        return Vec::new();
    };
    let active: Vec<_> = missions
        .list()
        .into_iter()
        .filter_map(|mission| {
            if mission.state != MissionState::Working {
                return None;
            }
            let (operation, agent) = (mission.operation_id.as_ref()?, mission.agent_id.as_ref()?);
            let session_id = sessions
                .get(mission.mission_id.as_str())?
                .session_id
                .as_ref()?
                .to_string();
            Some((
                operation.clone(),
                agent.clone(),
                mission.mission_id.to_string(),
                session_id,
            ))
        })
        .collect();

    let mut endpoints = Vec::new();
    for operation in roster.operations() {
        let Ok(agents) = roster.agents_for_operation(&operation.id) else {
            continue;
        };
        for agent in agents {
            let active_sessions: Vec<_> = active
                .iter()
                .filter(|(operation_id, agent_id, _, _)| {
                    operation_id == &operation.id && agent_id == &agent.id
                })
                .collect();
            for (_, _, mission_id, session_id) in active_sessions {
                if let Ok(endpoint) = CableEndpoint::try_new(
                    agent.id.clone(),
                    operation.id.clone(),
                    operation.theater.clone(),
                    station_id.to_owned(),
                    Some(session_id.clone()),
                    Some(mission_id.clone()),
                ) {
                    endpoints.push(endpoint);
                }
            }
        }
    }
    endpoints
}

fn cable_principal(
    state: &DaemonState,
    mission_id: &str,
) -> Result<CablePrincipal, ApiErrorResponse> {
    let mission = state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .get(mission_id)
        .cloned()
        .ok_or_else(|| mission_error(MissionError::NotFound))?;
    if mission.state != MissionState::Working {
        return Err(conflict("Cable access requires a working Mission"));
    }
    let operation_id = mission
        .operation_id
        .as_deref()
        .ok_or_else(|| configuration_error("Cable access requires an Operation"))?;
    let agent_id = mission
        .agent_id
        .as_deref()
        .ok_or_else(|| configuration_error("Cable access requires an assigned Agent"))?;
    let session_id = state
        .sessions
        .lock()
        .map_err(|_| internal_error())?
        .get(mission_id)
        .and_then(|operation| operation.session_id.as_ref())
        .ok_or_else(|| conflict("Cable access requires a managed Agent Session"))?
        .to_string();
    let operation = state
        .roster
        .operation(operation_id)
        .map_err(configuration_error)?;
    CablePrincipal::try_new(
        agent_id.to_owned(),
        operation.id.clone(),
        operation.theater.clone(),
        state.station_id.clone(),
        Some(session_id),
        Some(mission_id.to_owned()),
    )
    .map_err(|_| configuration_error("Mission has invalid Cable identity"))
}

async fn discover_cables(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
) -> Result<Json<Vec<CablePeer>>, ApiErrorResponse> {
    let principal = cable_principal(&state, &mission_id)?;
    let peers = state
        .cables
        .discover(&principal)
        .map_err(cable_error)?
        .into_iter()
        .map(|endpoint| CablePeer {
            agent: endpoint.agent().to_owned(),
            operation: endpoint.operation().to_owned(),
            theater: endpoint.theater().map(str::to_owned),
            station: endpoint.station().to_owned(),
            session_id: endpoint.session_id().map(str::to_owned),
            mission_id: endpoint.mission_id().map(str::to_owned),
        })
        .collect();
    Ok(Json(peers))
}

async fn cable_inbox(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<CableInboxQuery>,
) -> Result<Json<Vec<CableDelivery>>, ApiErrorResponse> {
    let principal = cable_principal(&state, &mission_id)?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=1_000).contains(&limit) {
        return Err(bad_request("Cable inbox limit must be between 1 and 1000"));
    }
    state
        .cables
        .inbox(&principal, limit)
        .map(Json)
        .map_err(cable_error)
}

async fn held_cables(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<CableInboxQuery>,
) -> Result<Json<Vec<CableHeldDelivery>>, ApiErrorResponse> {
    let principal = cable_principal(&state, &mission_id)?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=1_000).contains(&limit) {
        return Err(bad_request("Held Cable limit must be between 1 and 1000"));
    }
    state
        .cables
        .held(&principal, limit)
        .map(Json)
        .map_err(cable_error)
}

async fn release_cable(
    State(state): State<DaemonState>,
    axum::extract::Path((mission_id, cable_id)): axum::extract::Path<(String, String)>,
) -> Result<Json<CableRelease>, ApiErrorResponse> {
    let principal = cable_principal(&state, &mission_id)?;
    state
        .cables
        .release(&principal, &cable_id)
        .map(Json)
        .map_err(cable_error)
}

async fn send_cable(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
    Json(request): Json<SendCable>,
) -> Result<(StatusCode, Json<CableReceipt>), ApiErrorResponse> {
    let principal = cable_principal(&state, &mission_id)?;
    let mut send = CableSendRequest::new(request.target, request.body, request.idempotency_key);
    if let Some(ttl) = request.ttl_seconds {
        send = send.with_ttl(ttl);
    }
    if let Some(conversation_id) = request.conversation_id {
        send = send.with_conversation(conversation_id);
    }
    let cable = state.cables.send(&principal, send).map_err(cable_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CableReceipt::from_id(
            cable.id(),
            cable.created_at(),
            cable.expires_at(),
        )),
    ))
}

async fn acknowledge_cable(
    State(state): State<DaemonState>,
    axum::extract::Path((mission_id, cable_id)): axum::extract::Path<(String, String)>,
) -> Result<StatusCode, ApiErrorResponse> {
    let principal = cable_principal(&state, &mission_id)?;
    state
        .cables
        .acknowledge(&principal, &cable_id)
        .map_err(cable_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reply_cable(
    State(state): State<DaemonState>,
    axum::extract::Path((mission_id, cable_id)): axum::extract::Path<(String, String)>,
    Json(request): Json<ReplyCable>,
) -> Result<(StatusCode, Json<CableReceipt>), ApiErrorResponse> {
    let principal = cable_principal(&state, &mission_id)?;
    let cable = state
        .cables
        .reply(&principal, &cable_id, request.body, request.idempotency_key)
        .map_err(cable_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CableReceipt::from_id(
            cable.id(),
            cable.created_at(),
            cable.expires_at(),
        )),
    ))
}

fn cable_error(error: CableRelayError) -> ApiErrorResponse {
    match error {
        CableRelayError::Unavailable => forbidden("Cable target is unavailable or not authorized"),
        CableRelayError::InvalidRequest => bad_request("Cable request is invalid"),
        CableRelayError::RegistryUnavailable => unavailable("Cable relay is unavailable"),
    }
}

#[derive(Clone)]
struct DaemonState {
    plugins: Arc<plugins::Plugins>,
    roster: Arc<Roster>,
    intake: Arc<FileRegistry>,
    missions: Arc<Mutex<MissionStore>>,
    sessions: Arc<Mutex<BTreeMap<String, AgentOperation>>>,
    cables: Arc<CableRelay>,
    station_id: String,
    station: StationConfig,
    #[cfg(feature = "publisher")]
    publisher: Option<Arc<Publisher>>,
    starting: Arc<Mutex<BTreeSet<String>>>,
    max_concurrent_missions: usize,
    watchdog: WatchdogConfig,
    retry_after: Arc<Mutex<BTreeMap<String, Instant>>>,
    startup_recovery: Arc<Mutex<BTreeSet<String>>>,
    events: Arc<Mutex<BTreeMap<String, VecDeque<AgentEvent>>>>,
    event_ready: Arc<Notify>,
    stopping: Arc<AtomicBool>,
    event_pumps: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl DaemonState {
    fn mission_capacity(&self) -> usize {
        match self.intake.station_settings(&self.station_id) {
            Ok(Some(settings)) => settings.max_concurrent_missions(),
            Ok(None) => self.max_concurrent_missions,
            Err(error) => {
                eprintln!("ai6: Station settings unavailable; refusing new Missions: {error}");
                0
            }
        }
    }
}

struct StartReservation {
    starting: Arc<Mutex<BTreeSet<String>>>,
    mission_id: String,
}

impl Drop for StartReservation {
    fn drop(&mut self) {
        if let Ok(mut starting) = self.starting.lock() {
            starting.remove(&self.mission_id);
        }
    }
}

fn reserve_mission_start(
    state: &DaemonState,
    mission: &Mission,
    agent: &roster::Agent,
) -> Result<StartReservation, ApiErrorResponse> {
    let mission_id = mission.mission_id.to_string();
    let mut starting = state.starting.lock().map_err(|_| internal_error())?;
    if starting.contains(&mission_id) {
        return Err(conflict("Mission is already starting"));
    }

    let sessions = state.sessions.lock().map_err(|_| internal_error())?;
    let active_ids: BTreeSet<String> = starting
        .iter()
        .cloned()
        .chain(sessions.keys().cloned())
        .collect();
    if active_ids.len() >= state.mission_capacity() {
        return Err(capacity_error("daemon Mission capacity is full"));
    }
    drop(sessions);

    let active_missions = state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .list()
        .into_iter()
        .filter(|active| active_ids.contains(active.mission_id.as_str()))
        .collect::<Vec<_>>();
    if let Some(operation_id) = mission.operation_id.as_deref() {
        let operation = state
            .roster
            .operation(operation_id)
            .map_err(configuration_error)?;
        if operation.max_concurrent.is_some_and(|limit| {
            active_missions
                .iter()
                .filter(|active| active.operation_id.as_deref() == Some(operation_id))
                .count()
                >= limit
        }) {
            return Err(conflict(&format!(
                "Operation {operation_id:?} Mission capacity is full"
            )));
        }
    }
    if agent.max_concurrent.is_some_and(|limit| {
        active_missions
            .iter()
            .filter(|active| active.agent_id.as_deref() == Some(agent.id.as_str()))
            .count()
            >= limit
    }) {
        return Err(conflict(&format!(
            "Agent {:?} Mission capacity is full",
            agent.id
        )));
    }

    starting.insert(mission_id.clone());
    Ok(StartReservation {
        starting: state.starting.clone(),
        mission_id,
    })
}

fn reserve_existing_session_resume(
    state: &DaemonState,
    mission_id: &str,
) -> Result<StartReservation, ApiErrorResponse> {
    let mut starting = state.starting.lock().map_err(|_| internal_error())?;
    if !starting.insert(mission_id.to_owned()) {
        return Err(conflict("Mission is already resuming"));
    }
    Ok(StartReservation {
        starting: state.starting.clone(),
        mission_id: mission_id.to_owned(),
    })
}

async fn scheduler_loop(state: DaemonState) {
    let desk = Arc::new(SourceDesk::new());
    let mut next_station_check_in = Instant::now();
    while !state.stopping.load(Ordering::Acquire) {
        if Instant::now() >= next_station_check_in {
            publish_station_check_in(&state);
            next_station_check_in = Instant::now() + STATION_CHECK_IN_INTERVAL;
            renew_session_leases(&state);
        }
        schedule_recovered_once(&state).await;
        schedule_queued_once(&state).await;
        let is_headquarters = schedule_intake_once(&state);
        if is_headquarters && SourceDesk::has_work(&state.roster) && desk.begin() {
            let (desk, plugins, roster, registry) = (
                desk.clone(),
                state.plugins.clone(),
                state.roster.clone(),
                state.intake.clone(),
            );
            // Provider calls block on the network, so they never run on an async worker.
            tokio::task::spawn_blocking(move || {
                desk.run(&plugins, &roster, &registry);
                desk.end();
            });
        }
        tokio::time::sleep(SCHEDULER_INTERVAL).await;
    }
}

/// Claims, recovers and places briefs; returns whether this Station holds the Headquarters lease.
fn schedule_intake_once(state: &DaemonState) -> bool {
    let station = state.station_id.as_str();
    let is_headquarters = state.station.roles.contains(&StationRole::Control)
        && state
            .intake
            .elect(station, DEFAULT_LEASE)
            .unwrap_or_default();
    if is_headquarters && let Ok(expired) = state.intake.expired() {
        for lease in expired {
            if lease.station() != station {
                let _ = state.intake.recover(&lease, INTAKE_MAX_ATTEMPTS, station);
            }
        }
    }

    let Ok(leases) = state.intake.leases() else {
        return is_headquarters;
    };
    for lease in leases.iter().filter(|lease| lease.station() == station) {
        if let Err(error) = ensure_leased_mission(state, lease) {
            let _ = finish_intake_claim(
                state,
                lease,
                &serde_json::json!({"state":"failed", "reason":error}),
                false,
            );
            continue;
        }
        if lease.expires() <= unix_now() + DEFAULT_LEASE.as_secs_f64() / 2.0 {
            let _ = state.intake.renew(lease.key(), station, DEFAULT_LEASE);
        }
        if finish_terminal_intake_claim(state, lease).is_some() {
            continue;
        }
    }

    if !is_headquarters {
        return false;
    }
    let Ok(briefs) = state.intake.list_new() else {
        return true;
    };
    let Ok(mut leases) = state.intake.leases() else {
        return true;
    };
    for brief in briefs {
        let Ok(mission_id) = mission_id_for_brief(&brief) else {
            continue;
        };
        let (operation_id, agent_id, role_id) = match resolve_brief_assignment(state, &brief) {
            Ok(assignment) => assignment,
            Err(error) => {
                let meta = BTreeMap::from([(
                    "mission_id".to_owned(),
                    serde_json::json!(mission_id.as_str()),
                )]);
                if let Ok(Some(lease)) =
                    state
                        .intake
                        .claim_with_lease(Some(&brief.key()), station, DEFAULT_LEASE, meta)
                {
                    let _ = finish_intake_claim(
                        state,
                        &lease,
                        &serde_json::json!({"state":"failed", "reason":error}),
                        false,
                    );
                }
                continue;
            }
        };
        if !has_assignment_capacity(state, &operation_id, &agent_id) {
            continue;
        }
        let Some(execution_station) = place_brief(state, &brief, &operation_id, &agent_id, &leases)
        else {
            continue;
        };
        let mut meta = BTreeMap::new();
        meta.insert(
            "mission_id".to_owned(),
            serde_json::json!(mission_id.as_str()),
        );
        meta.insert("operation".to_owned(), serde_json::json!(operation_id));
        meta.insert("agent".to_owned(), serde_json::json!(agent_id));
        meta.insert("role".to_owned(), serde_json::json!(role_id));
        let Ok(Some(lease)) = state.intake.claim_with_lease(
            Some(&brief.key()),
            &execution_station,
            DEFAULT_LEASE,
            meta,
        ) else {
            continue;
        };
        if lease.station() == station
            && let Err(error) =
                create_intake_mission(state, &brief, &mission_id, operation_id, agent_id, role_id)
        {
            let _ = finish_intake_claim(
                state,
                &lease,
                &serde_json::json!({"state":"failed", "reason":error}),
                false,
            );
        }
        leases.push(lease);
    }
    true
}

async fn schedule_recovered_once(state: &DaemonState) {
    if !state.station.roles.contains(&StationRole::Execution) {
        return;
    }
    let pending = state
        .startup_recovery
        .lock()
        .map(|missions| missions.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    for mission_id in pending {
        if state.stopping.load(Ordering::Acquire) {
            break;
        }
        let now = Instant::now();
        let should_wait = state
            .retry_after
            .lock()
            .ok()
            .and_then(|retries| retries.get(&mission_id).copied())
            .is_some_and(|retry_at| retry_at > now);
        if should_wait {
            continue;
        }

        let result = resume_mission(
            State(state.clone()),
            axum::extract::Path(mission_id.clone()),
            Json(ResumeMission {
                message: None,
                environment: BTreeMap::new(),
            }),
        )
        .await;
        match result {
            Ok(_) => {
                if let Ok(mut pending) = state.startup_recovery.lock() {
                    pending.remove(&mission_id);
                }
                if let Ok(mut retries) = state.retry_after.lock() {
                    retries.remove(&mission_id);
                }
            }
            Err(ApiErrorResponse(StatusCode::TOO_MANY_REQUESTS, _)) => break,
            Err(_) => {
                if let Ok(mut retries) = state.retry_after.lock() {
                    retries.insert(mission_id, Instant::now() + SCHEDULER_RETRY_DELAY);
                }
            }
        }
    }
}

/// The Registry lease this Station holds on a Mission's Session, if any.
fn session_hold(state: &DaemonState, mission_id: &str) -> Option<SessionHold> {
    state
        .missions
        .lock()
        .ok()?
        .operation(mission_id)
        .and_then(|manifest| manifest.session)
}

/// Mirrors a Mission's state into its shared Session record, fenced by this Station's token.
///
/// A refused token means another Station took the Session, so this Station stops acting on it.
fn sync_session_record(state: &DaemonState, mission_id: &str) {
    let Some(hold) = session_hold(state, mission_id) else {
        return;
    };
    let Some(mission) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
    else {
        return;
    };
    let mut update = SessionUpdate::default();
    if let Some(provider) = mission.session_ids.last() {
        update = update.provider_session(provider.as_str());
    }
    update = match mission.state {
        MissionState::Queued => return,
        state if is_terminal_state(state) => update.status(SessionStatus::Ended),
        _ => update.status(SessionStatus::Active),
    };
    match state
        .intake
        .update_session(&hold.id, &state.station_id, hold.fence, update)
    {
        Ok(_) | Err(SessionError::Ended) => {}
        Err(SessionError::Stale | SessionError::HeldBy(_)) => fence_out(state, mission_id),
        Err(error) => eprintln!("ai6: Session record {:?} not updated: {error}", hold.id),
    }
}

/// Takes (or renews) this Station's lease on the Session a Mission is about to resume.
///
/// Another Station's live lease refuses the resume. A Mission whose record predates shared
/// Session records, or whose record is gone, gets a new one.
fn acquire_session_hold(
    state: &DaemonState,
    mission: &Mission,
    provider_session: &SessionId,
) -> Result<SessionHold, ApiErrorResponse> {
    let mission_id = mission.mission_id.as_str();
    let existing = session_hold(state, mission_id);
    let taken = match &existing {
        Some(hold) => state
            .intake
            .take_session(&hold.id, &state.station_id, DEFAULT_LEASE),
        None => Err(SessionError::NotFound),
    };
    let record = match taken {
        Ok(record) => record,
        Err(SessionError::NotFound) => {
            let mut new_session = NewSession::new(
                uuid::Uuid::new_v4().to_string(),
                mission_id,
                state.station_id.as_str(),
            )
            .with_provider_session(provider_session.as_str())
            .with_attempt(existing.as_ref().map_or(1, |hold| hold.attempt), None);
            if let Ok(theater) = session_theater(state, mission.operation_id.as_deref()) {
                new_session = new_session.with_theater(theater);
            }
            state
                .intake
                .open_session(new_session, DEFAULT_LEASE)
                .map_err(session_error)?
        }
        Err(error) => return Err(session_error(error)),
    };
    let hold = SessionHold {
        id: record.session_id,
        attempt: record.attempt,
        fence: record.fence,
    };
    if existing.as_ref() != Some(&hold) {
        state
            .missions
            .lock()
            .map_err(|_| internal_error())?
            .set_hold(mission_id, hold.clone())
            .map_err(mission_error)?;
    }
    Ok(hold)
}

fn session_error(error: SessionError) -> ApiErrorResponse {
    match error {
        SessionError::HeldBy(station) => {
            conflict(&format!("Station {station:?} holds this Session's lease"))
        }
        SessionError::Stale
        | SessionError::Ended
        | SessionError::Exists
        | SessionError::NotFound => {
            conflict("the Session's Registry lease is no longer available to this Station")
        }
        error => runtime_error(error),
    }
}

/// Stops acting on a Session whose lease another Station now holds.
///
/// The harness is closed but its safe house is left alone: the new holder resumes from it.
fn fence_out(state: &DaemonState, mission_id: &str) {
    let operation = state
        .sessions
        .lock()
        .ok()
        .and_then(|mut sessions| sessions.remove(mission_id));
    if let Some(operation) = operation
        && let Ok(session) = operation.session.lock()
    {
        let _ = session.interrupt();
        let _ = session.close();
    }
    if let Ok(mut missions) = state.missions.lock()
        && missions
            .get(mission_id)
            .is_some_and(|mission| !is_terminal_state(mission.state))
    {
        let _ = missions.transition(mission_id, MissionState::Abandoned);
    }
    eprintln!("ai6: Mission {mission_id:?} fenced out: another Station holds its Session");
}

/// Extends the lease on every Session this Station holds, fencing out any it has lost.
fn renew_session_leases(state: &DaemonState) {
    let held = state
        .missions
        .lock()
        .map(|missions| {
            missions
                .list()
                .into_iter()
                .filter(|mission| !is_terminal_state(mission.state))
                .filter_map(|mission| {
                    let hold = missions.operation(mission.mission_id.as_str())?.session?;
                    Some((mission.mission_id.to_string(), hold))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (mission_id, hold) in held {
        match state
            .intake
            .renew_session(&hold.id, &state.station_id, hold.fence, DEFAULT_LEASE)
        {
            Ok(_) | Err(SessionError::Ended) => {}
            Err(SessionError::Stale | SessionError::HeldBy(_)) => fence_out(state, &mission_id),
            Err(error) => eprintln!("ai6: Session lease {:?} not renewed: {error}", hold.id),
        }
    }
}

fn daemon_station_id() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "localhost".to_owned());
    format!("{host}-{}", std::process::id())
}

fn publish_station_check_in(state: &DaemonState) {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "localhost".to_owned());
    let active_sessions = state
        .sessions
        .lock()
        .map(|sessions| sessions.len())
        .unwrap_or_default();
    let mut labels = BTreeMap::from([
        ("station".to_owned(), state.station_id.clone()),
        ("host".to_owned(), host.clone()),
        ("os".to_owned(), std::env::consts::OS.to_owned()),
    ]);
    labels.extend(state.station.labels.clone());
    let operations = state
        .roster
        .operations()
        .filter(|operation| {
            local_theater_of(&state.station.theaters, operation).is_some()
                && operation.path.as_ref().is_some_and(|path| path.is_dir())
        })
        .map(|operation| operation.id.clone())
        .collect::<Vec<_>>();
    let roles = state
        .station
        .roles
        .iter()
        .map(|role| role.as_str())
        .collect::<Vec<_>>();
    let agent_roles = state.roster.roles().cloned().collect::<Vec<_>>();
    let mut info = BTreeMap::new();
    info.insert("host".to_owned(), serde_json::json!(host));
    info.insert(
        "platform".to_owned(),
        serde_json::json!(std::env::consts::OS),
    );
    info.insert(
        "process_id".to_owned(),
        serde_json::json!(std::process::id()),
    );
    info.insert("roles".to_owned(), serde_json::json!(roles));
    match serde_json::to_vec(&agent_roles) {
        Ok(encoded) if encoded.len() <= MAX_PUBLISHED_AGENT_ROLE_CATALOG_BYTES => {
            info.insert("agent_roles".to_owned(), serde_json::json!(agent_roles));
        }
        Ok(_) => eprintln!(
            "ai6: Station role catalog exceeds the {} KiB check-in budget; Universal Exports role listing will omit the global catalog",
            MAX_PUBLISHED_AGENT_ROLE_CATALOG_BYTES / 1024
        ),
        Err(error) => eprintln!("ai6: could not encode Station role catalog: {error}"),
    }
    info.insert("labels".to_owned(), serde_json::json!(labels));
    info.insert(
        "capacity".to_owned(),
        serde_json::json!(if state.station.roles.contains(&StationRole::Execution) {
            state.mission_capacity()
        } else {
            0
        }),
    );
    info.insert("running".to_owned(), serde_json::json!(active_sessions));
    info.insert("operations".to_owned(), serde_json::json!(operations));
    let policies: BTreeMap<String, OperationPolicy> = state
        .roster
        .operations()
        .filter_map(|operation| {
            OperationPolicy::from_roster(&state.roster, &operation.id)
                .ok()
                .map(|policy| (operation.id.clone(), policy))
        })
        .collect();
    match serde_json::to_vec(&policies) {
        Ok(encoded) if encoded.len() <= MAX_PUBLISHED_AGENT_ROLE_CATALOG_BYTES => {
            info.insert("operation_policies".to_owned(), serde_json::json!(policies));
        }
        Ok(_) => eprintln!(
            "ai6: Station placement policies exceed the {} KiB check-in budget; placement previews will be unavailable",
            MAX_PUBLISHED_AGENT_ROLE_CATALOG_BYTES / 1024
        ),
        Err(error) => eprintln!("ai6: could not encode Station placement policies: {error}"),
    }
    info.insert(
        "handlers".to_owned(),
        serde_json::json!(available_handlers(state)),
    );
    info.insert(
        "theaters".to_owned(),
        serde_json::json!(state.station.theaters.summaries()),
    );
    if let Err(error) = state.intake.check_in(&state.station_id, info) {
        eprintln!("ai6: Station check-in failed: {error}");
    }
}

/// The Theater an Operation's Sessions run in when that Theater is this Station's own host:
/// the Operation's `theater`, or `local` when it names none. `None` when the named Theater
/// is not reached with local access, so this Station cannot run the Operation itself.
fn local_theater_of(theaters: &Theaters, operation: &roster::Operation) -> Option<String> {
    let id = operation.theater.as_deref().unwrap_or(LOCAL_THEATER);
    theaters
        .find(id)
        .filter(|theater| theater.access == TheaterAccess::Local)
        .map(|theater| theater.id)
}

/// The Theater a Session for `operation_id` runs in on this Station, or why it cannot.
fn session_theater(state: &DaemonState, operation_id: Option<&str>) -> Result<String, String> {
    let Some(operation) = operation_id.and_then(|id| state.roster.operation(id).ok()) else {
        return Ok(LOCAL_THEATER.to_owned());
    };
    local_theater_of(&state.station.theaters, operation).ok_or_else(|| {
        format!(
            "Operation {:?} is bound to Theater {:?}, which this Station does not reach with local access; Sessions do not run on remote Theaters yet",
            operation.id,
            operation.theater.as_deref().unwrap_or_default()
        )
    })
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |time| time.as_secs_f64())
}

fn has_assignment_capacity(state: &DaemonState, operation_id: &str, agent_id: &str) -> bool {
    let Ok(starting) = state.starting.lock() else {
        return false;
    };
    let Ok(sessions) = state.sessions.lock() else {
        return false;
    };
    let active_ids: BTreeSet<String> = starting
        .iter()
        .cloned()
        .chain(sessions.keys().cloned())
        .collect();
    if active_ids.len() >= state.mission_capacity() {
        return false;
    }
    drop(sessions);
    drop(starting);

    let Ok(operation) = state.roster.operation(operation_id) else {
        return false;
    };
    let Ok(agent) = state.roster.agent(Some(operation_id), agent_id) else {
        return false;
    };
    let active_missions = state
        .missions
        .lock()
        .map(|missions| {
            missions
                .list()
                .into_iter()
                .filter(|mission| active_ids.contains(mission.mission_id.as_str()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let Ok(leases) = state.intake.leases() else {
        return false;
    };
    let active_mission_ids = active_missions
        .iter()
        .map(|mission| mission.mission_id.as_str())
        .collect::<BTreeSet<_>>();
    let station_leases = leases
        .iter()
        .filter(|lease| {
            lease
                .meta()
                .get("operation")
                .and_then(serde_json::Value::as_str)
                == Some(operation_id)
        })
        .filter(|lease| {
            lease.station() != state.station_id
                || !lease
                    .meta()
                    .get("mission_id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|mission_id| active_mission_ids.contains(mission_id))
        })
        .count();
    if operation.max_concurrent.is_some_and(|limit| {
        active_missions
            .iter()
            .filter(|mission| mission.operation_id.as_deref() == Some(operation_id))
            .count()
            + station_leases
            >= limit
    }) {
        return false;
    }
    let agent_leases = leases
        .iter()
        .filter(|lease| {
            lease
                .meta()
                .get("agent")
                .and_then(serde_json::Value::as_str)
                == Some(agent_id)
        })
        .filter(|lease| {
            lease.station() != state.station_id
                || !lease
                    .meta()
                    .get("mission_id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|mission_id| active_mission_ids.contains(mission_id))
        })
        .count();
    !agent.max_concurrent.is_some_and(|limit| {
        active_missions
            .iter()
            .filter(|mission| mission.agent_id.as_deref() == Some(agent_id))
            .count()
            + agent_leases
            >= limit
    })
}

/// Chooses the execution Station for a brief with M's placement; the Agent is already fixed.
fn place_brief(
    state: &DaemonState,
    brief: &Brief,
    operation_id: &str,
    agent_id: &str,
    leases: &[Lease],
) -> Option<String> {
    let policy = OperationPolicy::from_roster(&state.roster, operation_id).ok()?;
    let records = state.intake.stations(Duration::MAX).ok()?;
    let now = unix_now();
    let stations: Vec<StationFacts> = records
        .iter()
        .map(|record| {
            let load = leases
                .iter()
                .filter(|lease| lease.station() == record.station())
                .count();
            StationFacts::from_record(record, now, GONE_DARK_AFTER, load)
        })
        .collect();
    let request = PlacementRequest {
        stations: brief.stations.clone(),
        agent: Some(agent_id.to_owned()),
        deciding_station: Some(state.station_id.clone()),
        ..PlacementRequest::default()
    };
    placement::place(&policy, &request, &stations)
        .best()
        .map(|choice| choice.station.clone())
}

fn mission_id_for_brief(brief: &Brief) -> Result<MissionId, String> {
    let key = brief.key();
    let candidate = if key.len() <= MAX_ID_BYTES {
        key
    } else {
        let hash = key.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
        let readable_bytes = MAX_ID_BYTES - "brief--".len() - 16;
        format!("brief-{}-{hash:016x}", &key[..readable_bytes])
    };
    MissionId::try_new(candidate)
        .map_err(|error| format!("brief identity cannot become a Mission ID: {error}"))
}

fn resolve_brief_assignment(
    state: &DaemonState,
    brief: &Brief,
) -> Result<(String, String, Option<String>), String> {
    let station_key = brief
        .operation
        .as_deref()
        .ok_or_else(|| "brief does not name a registered Operation".to_owned())?;
    let operation = state
        .roster
        .operation(station_key)
        .map_err(|error| error.to_string())?;
    let role = brief
        .role
        .as_deref()
        .map(|key| configured_agent_role(state, Some(&operation.id), key))
        .transpose()?;
    let agent = if let Some(agent_key) = brief.agent.as_deref() {
        state
            .roster
            .agent(Some(&operation.id), agent_key)
            .map_err(|error| error.to_string())?
    } else {
        state
            .roster
            .select_agent(
                Some(&operation.id),
                brief.role.as_deref(),
                &brief.tags,
                Some(&available_handlers(state)),
            )
            .map_err(|error| error.to_string())?
    };
    if let Some(role) = &role
        && !agent.roles.iter().any(|capability| {
            role.id == *capability
                || role
                    .id
                    .starts_with(&format!("{}/", capability.trim_end_matches('/')))
        })
    {
        return Err(format!(
            "Agent {:?} does not have role {:?}",
            agent.id, role.id
        ));
    }
    Ok((operation.id.clone(), agent.id, role.map(|role| role.id)))
}

fn station_terminal(state: &DaemonState) -> Result<String, String> {
    if let Some(pinned) = &state.station.terminal {
        return Ok(pinned.clone());
    }
    let available = state
        .plugins
        .terminals()
        .into_iter()
        .filter(|provider| provider.unavailable.is_none())
        .map(|provider| provider.id)
        .collect::<Vec<_>>();
    available
        .iter()
        .find(|id| id.as_str() == DEFAULT_TERMINAL)
        .or_else(|| available.first())
        .cloned()
        .ok_or_else(|| "this Station has no available terminal provider".to_owned())
}

fn available_handlers(state: &DaemonState) -> Vec<String> {
    state
        .plugins
        .handlers()
        .into_iter()
        .filter(|provider| provider.unavailable.is_none())
        .map(|provider| provider.id)
        .collect()
}

fn configured_agent_role(
    state: &DaemonState,
    operation_id: Option<&str>,
    key: &str,
) -> Result<AgentRole, String> {
    if let Some(operation_id) = operation_id
        && let Some(role) = state
            .intake
            .agent_role(operation_id, key)
            .map_err(|error| error.to_string())?
    {
        return Ok(role);
    }
    state
        .roster
        .role(key)
        .cloned()
        .map_err(|error| error.to_string())
}

fn create_intake_mission(
    state: &DaemonState,
    brief: &Brief,
    mission_id: &MissionId,
    operation_id: String,
    agent_id: String,
    role_id: Option<String>,
) -> Result<(), String> {
    let mut missions = state
        .missions
        .lock()
        .map_err(|_| "Mission book lock was poisoned".to_owned())?;
    if let Some(existing) = missions.get(mission_id.as_str()) {
        if existing.operation_id.as_deref() == Some(operation_id.as_str())
            && existing.agent_id.as_deref() == Some(agent_id.as_str())
            && existing.role_id == role_id
            && existing.prompt == brief.prompt()
        {
            return Ok(());
        }
        return Err(format!("Mission ID {mission_id} is already in use"));
    }
    missions
        .create(
            mission_id.clone(),
            Some(operation_id),
            brief.prompt(),
            Some(agent_id),
            role_id,
        )
        .map(|_| ())
        .map_err(|error| format!("could not create Mission from brief: {error:?}"))
}

fn ensure_leased_mission(state: &DaemonState, lease: &Lease) -> Result<(), String> {
    let mission_id = lease
        .meta()
        .get("mission_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "lease has no durable Mission ID".to_owned())?;
    let mission_id = MissionId::try_new(mission_id.to_owned())
        .map_err(|error| format!("lease has an invalid Mission ID: {error}"))?;
    let brief = state
        .intake
        .read_claimed(lease.key())
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "claimed brief file is missing".to_owned())?;
    if let Some(existing) = state
        .missions
        .lock()
        .map_err(|_| "Mission book lock was poisoned".to_owned())?
        .get(mission_id.as_str())
        .cloned()
    {
        if existing.prompt == brief.prompt() {
            return Ok(());
        }
        return Err(format!("Mission ID {mission_id} is already in use"));
    }
    let assignment = match (
        lease
            .meta()
            .get("operation")
            .and_then(serde_json::Value::as_str),
        lease
            .meta()
            .get("agent")
            .and_then(serde_json::Value::as_str),
        lease.meta().get("role").and_then(serde_json::Value::as_str),
    ) {
        (Some(operation), Some(agent), role) => {
            let operation = state
                .roster
                .operation(operation)
                .map_err(|error| error.to_string())?;
            let agent = state
                .roster
                .agent(Some(&operation.id), agent)
                .map_err(|error| error.to_string())?;
            let role_id = role
                .map(|key| state.roster.role(key).map(|role| role.id.clone()))
                .transpose()
                .map_err(|error| error.to_string())?;
            Ok((operation.id.clone(), agent.id, role_id))
        }
        _ => resolve_brief_assignment(state, &brief),
    }?;
    let (operation_id, agent_id, role_id) = assignment;
    create_intake_mission(state, &brief, &mission_id, operation_id, agent_id, role_id)
}

fn finish_terminal_intake_claim(state: &DaemonState, lease: &Lease) -> Option<()> {
    let mission_id = lease.meta().get("mission_id")?.as_str()?;
    let mission = state.missions.lock().ok()?.get(mission_id).cloned()?;
    if !is_terminal_state(mission.state) {
        return None;
    }
    let outcome = serde_json::json!({
        "state": format!("{:?}", mission.state).to_lowercase(),
        "mission_id": mission.mission_id.as_str(),
        "session_ids": mission.session_ids,
        "agent": mission.agent_id,
        "branch": mission.safe_house.as_ref().map(|safe_house| safe_house.branch.clone()),
    });
    finish_intake_claim(state, lease, &outcome, mission.state == MissionState::Done)
}

fn finish_intake_claim(
    state: &DaemonState,
    lease: &Lease,
    outcome: &serde_json::Value,
    ok: bool,
) -> Option<()> {
    let brief = state.intake.read_claimed(lease.key()).ok()??;
    state
        .intake
        .finish(&brief, outcome, ok, None, Some(lease.station()))
        .ok()??;
    Some(())
}

async fn schedule_queued_once(state: &DaemonState) {
    if !state.station.roles.contains(&StationRole::Execution) {
        return;
    }
    let recovery_ids = state
        .startup_recovery
        .lock()
        .map(|missions| missions.clone())
        .unwrap_or_default();
    let queued = state
        .missions
        .lock()
        .ok()
        .map(|missions| {
            missions
                .list()
                .into_iter()
                .filter(|mission| {
                    mission.state == MissionState::Queued && mission.agent_id.is_some()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let queued_ids = queued
        .iter()
        .map(|mission| mission.mission_id.to_string())
        .collect::<BTreeSet<_>>();
    if let Ok(mut retries) = state.retry_after.lock() {
        retries.retain(|mission_id, _| {
            queued_ids.contains(mission_id) || recovery_ids.contains(mission_id)
        });
    }

    for mission in queued {
        if state.stopping.load(Ordering::Acquire) {
            break;
        }
        let mission_id = mission.mission_id.to_string();
        let now = Instant::now();
        let should_wait = state
            .retry_after
            .lock()
            .ok()
            .and_then(|retries| retries.get(&mission_id).copied())
            .is_some_and(|retry_at| retry_at > now);
        if should_wait {
            continue;
        }

        let result = start_mission(
            State(state.clone()),
            axum::extract::Path(mission_id.clone()),
            Json(StartMission {
                working_directory: None,
                initial_context: Vec::new(),
                environment: BTreeMap::new(),
            }),
        )
        .await;
        match result {
            Ok(_) => {
                if let Ok(mut retries) = state.retry_after.lock() {
                    retries.remove(&mission_id);
                }
            }
            Err(ApiErrorResponse(StatusCode::TOO_MANY_REQUESTS, _)) => break,
            Err(_) => {
                if let Ok(mut retries) = state.retry_after.lock() {
                    retries.insert(mission_id, Instant::now() + SCHEDULER_RETRY_DELAY);
                }
            }
        }
    }
}

struct AgentOperation {
    handler_provider: String,
    terminal_provider: String,
    working_directory: String,
    environment: BTreeMap<String, String>,
    model: Option<String>,
    effort: Option<String>,
    cover: Option<CoverAudit>,
    safe_house: Option<SafeHouse>,
    session: Arc<Mutex<guatiao::library::Object<dyn AgentSession>>>,
    watchdog: Watchdog,
    session_id: Option<SessionId>,
    #[cfg(feature = "cables-mcp")]
    _cables_mcp: Option<cables_mcp::Endpoint>,
    #[cfg(feature = "publisher")]
    telemetry: Option<Arc<SessionTelemetry>>,
}

#[derive(Debug, Deserialize)]
struct CreateMission {
    mission_id: MissionId,
    operation_id: Option<String>,
    prompt: String,
    agent_id: Option<String>,
    role: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TransitionMission {
    state: MissionState,
}

#[derive(Debug, Deserialize)]
struct AttachSession {
    session_id: SessionId,
}

#[derive(Debug, Deserialize)]
struct StartMission {
    working_directory: Option<String>,
    #[serde(default)]
    initial_context: Vec<String>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct SendMessage {
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendCable {
    target: CableTarget,
    body: String,
    idempotency_key: String,
    ttl_seconds: Option<f64>,
    conversation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyCable {
    body: String,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CableInboxQuery {
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct CablePeer {
    agent: String,
    operation: String,
    theater: Option<String>,
    station: String,
    session_id: Option<String>,
    mission_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct CableReceipt {
    cable_id: String,
    created_at: f64,
    expires_at: f64,
}

impl CableReceipt {
    fn from_id(cable_id: &str, created_at: f64, expires_at: f64) -> Self {
        Self {
            cable_id: cable_id.to_owned(),
            created_at,
            expires_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ResumeMission {
    message: Option<String>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct MissionEvent {
    mission: Mission,
    event: Option<AgentEvent>,
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    timeout_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

async fn list_missions(
    State(state): State<DaemonState>,
) -> Result<Json<Vec<Mission>>, ApiErrorResponse> {
    let missions = state.missions.lock().map_err(|_| internal_error())?;
    Ok(Json(missions.list()))
}

async fn get_mission(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
) -> Result<Json<Mission>, ApiErrorResponse> {
    let missions = state.missions.lock().map_err(|_| internal_error())?;
    missions
        .get(&mission_id)
        .cloned()
        .map(Json)
        .ok_or_else(|| mission_error(MissionError::NotFound))
}

async fn create_mission(
    State(state): State<DaemonState>,
    Json(request): Json<CreateMission>,
) -> Result<(StatusCode, Json<Mission>), ApiErrorResponse> {
    let operation_id = request
        .operation_id
        .as_deref()
        .map(|key| {
            state
                .roster
                .operation(key)
                .map(|operation| operation.id.clone())
        })
        .transpose()
        .map_err(configuration_error)?;
    let role_id = request
        .role
        .as_deref()
        .map(|key| configured_agent_role(&state, operation_id.as_deref(), key).map(|role| role.id))
        .transpose()
        .map_err(configuration_error)?;
    let agent = if let Some(key) = request.agent_id.as_deref() {
        Some(
            state
                .roster
                .agent(operation_id.as_deref(), key)
                .map_err(configuration_error)?,
        )
    } else if request.role.is_some() || !request.tags.is_empty() {
        Some(
            state
                .roster
                .select_agent(
                    operation_id.as_deref(),
                    request.role.as_deref(),
                    &request.tags,
                    Some(&available_handlers(&state)),
                )
                .map_err(configuration_error)?,
        )
    } else {
        None
    };
    if let (Some(agent), Some(role)) = (&agent, &role_id)
        && !agent.roles.iter().any(|capability| {
            role == capability
                || role.starts_with(&format!("{}/", capability.trim_end_matches('/')))
        })
    {
        return Err(configuration_error(format!(
            "Agent {:?} does not have role {role:?}",
            agent.id
        )));
    }
    let mut missions = state.missions.lock().map_err(|_| internal_error())?;
    let mission = missions
        .create(
            request.mission_id,
            operation_id,
            request.prompt,
            agent.map(|agent| agent.id),
            role_id,
        )
        .map_err(mission_error)?;
    Ok((StatusCode::CREATED, Json(mission)))
}

struct WorkingArea {
    path: String,
    safe_house: Option<SafeHouse>,
}

fn resolve_working_area(
    roster: &Roster,
    plugins: &plugins::Plugins,
    operation_id: Option<&str>,
    requested: Option<&str>,
    state_root: Option<&Path>,
    mission: &Mission,
) -> Result<WorkingArea, String> {
    let requested = requested.filter(|path| !path.trim().is_empty());
    let Some(operation_id) = operation_id else {
        let path = requested.ok_or_else(|| {
            "Mission needs a working_directory when it has no registered Operation".to_owned()
        })?;
        let path = Path::new(path);
        if !path.is_absolute() || !path.is_dir() {
            return Err("working_directory must be an existing absolute directory".to_owned());
        }
        return Ok(WorkingArea {
            path: path.to_string_lossy().into_owned(),
            safe_house: None,
        });
    };
    let operation = roster
        .operation(operation_id)
        .map_err(|error| error.to_string())?;
    let checkout = operation
        .path
        .as_ref()
        .ok_or_else(|| format!("Operation {:?} has no local checkout path", operation.id))?;
    if !checkout.is_dir() {
        return Err(format!(
            "Operation {:?} checkout {} is not an existing directory",
            operation.id,
            checkout.display()
        ));
    }
    let has_provider = operation.safe_house_provider().is_some();
    let isolate = has_provider && operation.workspace.isolate;
    if isolate && requested.is_some() {
        return Err(format!(
            "Operation {:?} workdirs are allocated by AI6 safe-house policy",
            operation.id
        ));
    }
    let in_place = match requested {
        Some(path) => {
            let requested = Path::new(path);
            if !requested.is_absolute() || !requested.is_dir() || !requested.starts_with(checkout) {
                return Err(format!(
                    "working_directory must be an existing absolute directory inside Operation {:?}",
                    operation.id
                ));
            }
            path.to_owned()
        }
        None => checkout.to_string_lossy().into_owned(),
    };
    let safe_house = open_safe_house(
        plugins,
        operation,
        &mission.mission_id,
        &mission.prompt,
        state_root,
        isolate,
        None,
    )?;
    let path = match safe_house.as_ref().filter(|house| house.is_isolated()) {
        Some(house) => house.path().to_string_lossy().into_owned(),
        None => in_place,
    };
    Ok(WorkingArea { path, safe_house })
}

/// Opens an Operation's safe house through its provider; `None` when it has no provider.
/// `reopen` re-attaches to a safe house an earlier run left behind.
fn open_safe_house(
    plugins: &plugins::Plugins,
    operation: &roster::Operation,
    mission_id: &MissionId,
    title: &str,
    state_root: Option<&Path>,
    isolate: bool,
    reopen: Option<SafeHouseReport>,
) -> Result<Option<SafeHouse>, String> {
    let Some(provider_id) = operation.safe_house_provider() else {
        return Ok(None);
    };
    let source = operation
        .workspace
        .source
        .as_ref()
        .or(operation.path.as_ref())
        .ok_or_else(|| format!("Operation {:?} has no local checkout path", operation.id))?;
    let root = match state_root {
        Some(path) => path.to_path_buf(),
        None => crate::station::safehouse::state_dir().map_err(|error| error.to_string())?,
    };
    let provider = plugins
        .safehouse(&provider_id, None)
        .map_err(|error| error.to_string())?;
    let spec = SafeHouseSpec {
        operation_id: operation.id.clone(),
        mission_id: mission_id.as_str().to_owned(),
        title: title.to_owned(),
        source: source.to_string_lossy().into_owned(),
        base: operation.workspace.base.clone(),
        branch: operation.workspace.branch.clone(),
        isolate,
        state_root: root.to_string_lossy().into_owned(),
        reopen,
    };
    SafeHouse::open(provider.as_ref(), &spec)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn configure_station_cover(
    roster: &Roster,
    operation_id: Option<&str>,
    agent_id: &str,
    model: &str,
    environment: &mut BTreeMap<String, String>,
) -> Result<Option<String>, String> {
    let Some(operation_id) = operation_id else {
        return Ok(None);
    };
    let operation = roster
        .operation(operation_id)
        .map_err(|error| error.to_string())?;
    let cover = roster
        .cover(&operation.cover)
        .map_err(|error| error.to_string())?;
    environment.insert("AI6_COVER".to_owned(), cover.name().to_owned());
    if let Some(identity) = &operation.cover_identity {
        environment.insert("GIT_AUTHOR_NAME".to_owned(), identity.name.clone());
        environment.insert("GIT_AUTHOR_EMAIL".to_owned(), identity.email.clone());
        environment.insert("GIT_COMMITTER_NAME".to_owned(), identity.name.clone());
        environment.insert("GIT_COMMITTER_EMAIL".to_owned(), identity.email.clone());
    }
    Ok(Some(cover.briefing(
        operation.cover_identity.as_ref(),
        agent_id,
        model,
    )))
}

fn station_cover_audit(
    roster: &Roster,
    operation_id: Option<&str>,
    safe_house: Option<&SafeHouse>,
) -> Result<Option<CoverAudit>, String> {
    let Some(operation_id) = operation_id else {
        return Ok(None);
    };
    let operation = roster
        .operation(operation_id)
        .map_err(|error| error.to_string())?;
    let Some(safe_house) = safe_house else {
        return Ok(None);
    };
    let base = safe_house.base().unwrap_or_default().to_owned();
    let cover = roster
        .cover(&operation.cover)
        .map_err(|error| error.to_string())?;
    Ok(Some(CoverAudit {
        policy: cover.name().to_owned(),
        base,
        identity: operation.cover_identity.clone(),
        fail_on_blow: operation.on_blown == "fail",
    }))
}

async fn transition_mission(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
    Json(request): Json<TransitionMission>,
) -> Result<Json<Mission>, ApiErrorResponse> {
    let mission = {
        let mut missions = state.missions.lock().map_err(|_| internal_error())?;
        missions
            .transition(&mission_id, request.state)
            .map_err(mission_error)?
    };
    sync_session_record(&state, &mission_id);
    publish_lifecycle(&state, &mission_id, mission.state);
    if matches!(
        mission.state,
        MissionState::Done | MissionState::Failed | MissionState::Blown | MissionState::Abandoned
    ) {
        finalize_mission_resources(&state, &mission_id, true).await?;
    }
    let mission = state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .get(&mission_id)
        .cloned()
        .ok_or_else(|| mission_error(MissionError::NotFound))?;
    Ok(Json(mission))
}

async fn attach_session(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
    Json(request): Json<AttachSession>,
) -> Result<Json<Mission>, ApiErrorResponse> {
    let mut missions = state.missions.lock().map_err(|_| internal_error())?;
    missions
        .attach_session(&mission_id, request.session_id)
        .map(Json)
        .map_err(mission_error)
}

async fn start_mission(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
    Json(request): Json<StartMission>,
) -> Result<(StatusCode, Json<StartedMission>), ApiErrorResponse> {
    let mission = state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .get(&mission_id)
        .cloned()
        .ok_or_else(|| mission_error(MissionError::NotFound))?;
    if mission.state != MissionState::Queued {
        return Err(conflict("Mission is not queued"));
    }
    let agent_id = mission
        .agent_id
        .as_deref()
        .ok_or_else(|| configuration_error("Mission has no assigned Agent"))?;
    let agent = state
        .roster
        .agent(mission.operation_id.as_deref(), agent_id)
        .map_err(configuration_error)?;
    let role = mission
        .role_id
        .as_deref()
        .map(|key| configured_agent_role(&state, mission.operation_id.as_deref(), key))
        .transpose()
        .map_err(configuration_error)?;
    if let Some(role) = &role
        && !agent.roles.iter().any(|capability| {
            role.id == *capability
                || role
                    .id
                    .starts_with(&format!("{}/", capability.trim_end_matches('/')))
        })
    {
        return Err(configuration_error(format!(
            "Agent {agent_id:?} does not have role {:?}",
            role.id
        )));
    }
    let theater_id =
        session_theater(&state, mission.operation_id.as_deref()).map_err(configuration_error)?;
    let _reservation = reserve_mission_start(&state, &mission, &agent)?;
    let handler_provider = agent
        .handler
        .clone()
        .ok_or_else(|| configuration_error(format!("Agent {agent_id:?} has no handler")))?;
    let model = agent.model.clone();
    let effort = agent.effort.clone();
    let terminal_provider = station_terminal(&state).map_err(configuration_error)?;
    #[cfg(feature = "publisher")]
    let telemetry = state.publisher.as_ref().and_then(|publisher| {
        mission
            .operation_id
            .as_ref()
            .map(|operation_id| publisher.session(operation_id.clone(), mission.mission_id.clone()))
    });
    let watchdog = Watchdog::with_config(agent.limits.apply_to(&state.watchdog));
    let terminal = state
        .plugins
        .terminal(&terminal_provider, None)
        .map_err(provider_error)?;
    let handler = state
        .plugins
        .handler(&handler_provider, None)
        .map_err(provider_error)?;
    let roster = state.roster.clone();
    let plugins = state.plugins.clone();
    let operation_id = mission.operation_id.clone();
    let requested_directory = request.working_directory.clone();
    let area_mission = mission.clone();
    let area = tokio::task::spawn_blocking(move || {
        resolve_working_area(
            &roster,
            &plugins,
            operation_id.as_deref(),
            requested_directory.as_deref(),
            None,
            &area_mission,
        )
    })
    .await
    .map_err(|error| runtime_error(format!("working-area worker failed: {error}")))?
    .map_err(configuration_error)?;
    let cover_audit = station_cover_audit(
        &state.roster,
        mission.operation_id.as_deref(),
        area.safe_house.as_ref(),
    )
    .map_err(configuration_error)?;
    let mut initial_context = vec![agent.context.clone()]
        .into_iter()
        .filter(|context| !context.trim().is_empty())
        .collect::<Vec<_>>();
    if let Some(role) = &role {
        initial_context.push(format!("Role purpose: {}", role.purpose));
        initial_context.push(format!("Role policy: {}", role.policy));
    }
    initial_context.extend(request.initial_context);
    let mut environment = agent.env.clone();
    environment.extend(request.environment);
    if let Some(briefing) = configure_station_cover(
        &state.roster,
        mission.operation_id.as_deref(),
        &agent.id,
        model.as_deref().unwrap_or_default(),
        &mut environment,
    )
    .map_err(configuration_error)?
    {
        initial_context.push(briefing);
    }
    #[cfg(feature = "cables-mcp")]
    let mut cables_mcp = if let Some(operation_id) = mission.operation_id.as_deref() {
        let theater = state
            .roster
            .operation(operation_id)
            .ok()
            .and_then(|operation| operation.theater.clone());
        let endpoint = cables_mcp::Endpoint::start(
            state.cables.clone(),
            CablePrincipal::try_new(
                agent.id.clone(),
                operation_id.to_owned(),
                theater,
                state.station_id.clone(),
                None,
                Some(mission.mission_id.to_string()),
            )
            .map_err(|_| runtime_error("Cable Mission identity is invalid"))?,
            state.intake.clone(),
            agent.clearance == "00",
        )
        .await
        .map_err(|error| runtime_error(format!("Cable MCP listener failed: {error}")))?;
        endpoint.add_environment(&mut environment);
        initial_context.push(cables_mcp::INSTRUCTIONS.to_owned());
        Some(endpoint)
    } else {
        None
    };
    let working_directory = area.path.clone();
    // A Mission started again (a later attempt) keeps the lease of the attempt it replaces
    // until the new Session is recorded.
    let previous_hold = session_hold(&state, &mission_id);
    let operation = LaunchManifest {
        handler_provider: handler_provider.clone(),
        terminal_provider: terminal_provider.clone(),
        working_directory: working_directory.clone(),
        model: model.clone(),
        effort: effort.clone(),
        cover: cover_audit.clone(),
        station_id: Some(state.station_id.clone()),
        theater_id: Some(theater_id.clone()),
        session: previous_hold.clone(),
    };
    let persisted = state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .set_operation(&mission_id, operation);
    if let Err(error) = persisted {
        return Err(start_failure(
            &state,
            &mission_id,
            format!("{error:?}"),
            area.safe_house.clone(),
        )
        .await);
    }
    #[cfg(feature = "publisher")]
    if let Some(telemetry) = &telemetry {
        telemetry.set_mission_publication(MissionPublication {
            prompt: mission.prompt.clone(),
            agent_id: Some(agent.id.clone()),
            role_id: mission.role_id.clone(),
            handler_provider: handler_provider.clone(),
            terminal_provider: terminal_provider.clone(),
            working_directory: working_directory.clone(),
            model: model.clone(),
            effort: effort.clone(),
            environment_keys: environment.keys().cloned().collect(),
            station_id: Some(state.station_id.clone()),
            theater_id: Some(theater_id.clone()),
        });
    }
    let start_request = AgentStart {
        mission_id: mission.mission_id,
        agent_id: agent.id,
        prompt: mission.prompt,
        initial_context,
        working_directory: working_directory.clone(),
        clearance: agent.clearance,
        environment: environment.clone(),
        model: model.clone(),
        effort: effort.clone(),
    };
    #[cfg(feature = "publisher")]
    let terminal_handle = match &telemetry {
        Some(telemetry) => TerminalBridge::agent_terminal_with_watchdog_and_telemetry(
            terminal,
            telemetry.clone(),
            watchdog.clone(),
        ),
        None => TerminalBridge::agent_terminal_with_watchdog(terminal, watchdog.clone()),
    };
    #[cfg(not(feature = "publisher"))]
    let terminal_handle = TerminalBridge::agent_terminal_with_watchdog(terminal, watchdog.clone());
    let started =
        tokio::task::spawn_blocking(move || handler.start(start_request, terminal_handle)).await;
    let started = match started {
        Ok(started) => started,
        Err(error) => {
            return Err(start_failure(
                &state,
                &mission_id,
                format!("AgentHandler start worker failed: {error}"),
                area.safe_house.clone(),
            )
            .await);
        }
    };
    let session = match started {
        Ok(session) => session,
        Err(error) => {
            return Err(start_failure(&state, &mission_id, error, area.safe_house.clone()).await);
        }
    };
    let identity = tokio::task::spawn_blocking(move || {
        let session_id = session.session_id();
        (session, session_id)
    })
    .await;
    let (session, session_id) = match identity {
        Ok(identity) => identity,
        Err(error) => {
            return Err(start_failure(
                &state,
                &mission_id,
                format!("provider Session identity worker failed: {error}"),
                area.safe_house.clone(),
            )
            .await);
        }
    };
    let session_id = match session_id {
        Ok(session_id) => session_id,
        Err(error) => {
            return Err(start_failure_with_session(
                &state,
                &mission_id,
                error,
                session,
                area.safe_house.clone(),
            )
            .await);
        }
    };
    #[cfg(feature = "cables-mcp")]
    if let Some(endpoint) = &mut cables_mcp
        && endpoint.bind_session(session_id.as_ref()).is_err()
    {
        return Err(start_failure_with_session(
            &state,
            &mission_id,
            "Cable Session binding failed".to_owned(),
            session,
            area.safe_house.clone(),
        )
        .await);
    }

    let (attempt, predecessor) = previous_hold
        .as_ref()
        .map_or((1, None), |hold| (hold.attempt + 1, Some(hold.id.clone())));
    let mut new_session = NewSession::new(
        uuid::Uuid::new_v4().to_string(),
        mission_id.as_str(),
        state.station_id.as_str(),
    )
    .with_theater(theater_id.as_str())
    .with_attempt(attempt, predecessor);
    if let Some(provider) = &session_id {
        new_session = new_session.with_provider_session(provider.as_str());
    }
    let hold = match state.intake.open_session(new_session, DEFAULT_LEASE) {
        Ok(record) => SessionHold {
            id: record.session_id,
            attempt: record.attempt,
            fence: record.fence,
        },
        Err(error) => {
            return Err(start_failure_with_session(
                &state,
                &mission_id,
                format!("Session record could not be written to the Registry: {error}"),
                session,
                area.safe_house.clone(),
            )
            .await);
        }
    };
    let working = {
        let mut missions = state.missions.lock().map_err(|_| internal_error())?;
        (|| {
            missions.set_hold(&mission_id, hold.clone())?;
            let working = missions.transition(&mission_id, MissionState::Working)?;
            let working = if let Some(session_id) = session_id.clone() {
                missions.attach_session(&mission_id, session_id)?
            } else {
                working
            };
            if let Some(safe_house) = area.safe_house.as_ref().filter(|house| house.is_isolated()) {
                missions.set_safe_house(&mission_id, safe_house.active_report())
            } else {
                Ok(working)
            }
        })()
    };
    let working = match working {
        Ok(working) => working,
        Err(error) => {
            let _ = state.intake.update_session(
                &hold.id,
                &state.station_id,
                hold.fence,
                SessionUpdate::default().status(SessionStatus::Ended),
            );
            return Err(start_failure_with_session(
                &state,
                &mission_id,
                format!("Mission transition failed: {error:?}"),
                session,
                area.safe_house.clone(),
            )
            .await);
        }
    };
    let session = Arc::new(Mutex::new(session));
    state.sessions.lock().map_err(|_| internal_error())?.insert(
        mission_id.clone(),
        AgentOperation {
            handler_provider: handler_provider.clone(),
            terminal_provider: terminal_provider.clone(),
            working_directory,
            environment,
            model,
            effort,
            cover: cover_audit,
            safe_house: area.safe_house,
            session: session.clone(),
            watchdog,
            session_id: session_id.clone(),
            #[cfg(feature = "cables-mcp")]
            _cables_mcp: cables_mcp,
            #[cfg(feature = "publisher")]
            telemetry: telemetry.clone(),
        },
    );
    #[cfg(feature = "publisher")]
    if let Some(telemetry) = &telemetry {
        if let Some(session_id) = session_id.clone() {
            telemetry.bind(session_id);
        }
        telemetry.publish_lifecycle("working");
    }
    spawn_event_pump(state.clone(), mission_id, session);
    Ok((
        StatusCode::ACCEPTED,
        Json(StartedMission {
            mission: working,
            handler_provider,
            terminal_provider,
            provider_session_id: session_id,
        }),
    ))
}

async fn resume_mission(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
    Json(request): Json<ResumeMission>,
) -> Result<(StatusCode, Json<StartedMission>), ApiErrorResponse> {
    let mission = state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .get(&mission_id)
        .cloned()
        .ok_or_else(|| mission_error(MissionError::NotFound))?;
    if !matches!(mission.state, MissionState::Blocked | MissionState::Stalled) {
        return Err(conflict("Only a blocked or stalled Mission can resume"));
    }
    let agent_id = mission
        .agent_id
        .as_deref()
        .ok_or_else(|| configuration_error("Mission has no assigned Agent"))?;
    let resume_agent = state
        .roster
        .agent(mission.operation_id.as_deref(), agent_id)
        .map_err(configuration_error)?;
    let _reservation = if state
        .sessions
        .lock()
        .map_err(|_| internal_error())?
        .contains_key(&mission_id)
    {
        reserve_existing_session_resume(&state, &mission_id)?
    } else {
        reserve_mission_start(&state, &mission, &resume_agent)?
    };
    let (
        handler_provider,
        terminal_provider,
        working_directory,
        environment,
        model,
        effort,
        cover_audit,
        safe_house,
        old_session,
        stored_session_id,
    ) = {
        let active = {
            let sessions = state.sessions.lock().map_err(|_| internal_error())?;
            sessions.get(&mission_id).map(|operation| {
                (
                    operation.handler_provider.clone(),
                    operation.terminal_provider.clone(),
                    operation.working_directory.clone(),
                    operation.environment.clone(),
                    operation.model.clone(),
                    operation.effort.clone(),
                    operation.cover.clone(),
                    operation.safe_house.clone(),
                    Some(operation.session.clone()),
                    operation.session_id.clone(),
                )
            })
        };
        if let Some(active) = active {
            active
        } else {
            let operation = state
                .missions
                .lock()
                .map_err(|_| internal_error())?
                .operation(&mission_id)
                .ok_or_else(|| {
                    conflict("No durable AgentHandler settings are available to resume")
                })?;
            let safe_house = match &mission.operation_id {
                Some(operation_id) => {
                    let configured = state.roster.operation(operation_id);
                    let reopen = match (&mission.safe_house, &configured) {
                        (Some(report), _) if report.kept => Some((report.clone(), true)),
                        // Work in place has no report; the cover audit remembers where it began.
                        (_, Ok(configured)) if !configured.workspace.isolate => {
                            operation.cover.as_ref().map(|audit| {
                                (
                                    SafeHouseReport {
                                        path: operation.working_directory.clone(),
                                        branch: None,
                                        base: Some(audit.base.clone())
                                            .filter(|base| !base.is_empty()),
                                        kept: true,
                                        changes: Vec::new(),
                                    },
                                    false,
                                )
                            })
                        }
                        _ => None,
                    };
                    match reopen {
                        Some((report, isolate)) => {
                            let configured = configured.map_err(configuration_error)?.clone();
                            let plugins = state.plugins.clone();
                            let mission_id = mission.mission_id.clone();
                            let title = mission.prompt.clone();
                            tokio::task::spawn_blocking(move || {
                                open_safe_house(
                                    &plugins,
                                    &configured,
                                    &mission_id,
                                    &title,
                                    None,
                                    isolate,
                                    Some(report),
                                )
                            })
                            .await
                            .map_err(|error| {
                                runtime_error(format!("safe-house recovery worker failed: {error}"))
                            })?
                            .map_err(configuration_error)?
                        }
                        None => None,
                    }
                }
                None => None,
            };
            (
                operation.handler_provider.clone(),
                operation.terminal_provider.clone(),
                operation.working_directory.clone(),
                resume_agent.env.clone(),
                operation.model,
                operation.effort,
                operation.cover,
                safe_house,
                None,
                None,
            )
        }
    };
    let mut environment = environment;
    environment.extend(request.environment.clone());
    configure_station_cover(
        &state.roster,
        mission.operation_id.as_deref(),
        &resume_agent.id,
        model.as_deref().unwrap_or_default(),
        &mut environment,
    )
    .map_err(configuration_error)?;
    let session_id = stored_session_id
        .or_else(|| mission.session_ids.last().cloned())
        .ok_or_else(|| conflict("Provider has not issued a Session ID yet"))?;
    acquire_session_hold(&state, &mission, &session_id)?;
    #[cfg(feature = "publisher")]
    let telemetry = state.publisher.as_ref().and_then(|publisher| {
        mission.operation_id.as_ref().map(|operation_id| {
            let telemetry = publisher.session(operation_id.clone(), mission.mission_id.clone());
            telemetry.set_mission_publication(MissionPublication {
                prompt: mission.prompt.clone(),
                agent_id: mission.agent_id.clone(),
                role_id: mission.role_id.clone(),
                handler_provider: handler_provider.clone(),
                terminal_provider: terminal_provider.clone(),
                working_directory: working_directory.clone(),
                model: model.clone(),
                effort: effort.clone(),
                environment_keys: environment.keys().cloned().collect(),
                station_id: Some(state.station_id.clone()),
                theater_id: session_theater(&state, Some(operation_id)).ok(),
            });
            telemetry.bind(session_id.clone());
            telemetry
        })
    });
    let watchdog = Watchdog::with_config(resume_agent.limits.apply_to(&state.watchdog));
    let terminal = state
        .plugins
        .terminal(&terminal_provider, None)
        .map_err(provider_error)?;
    let handler = state
        .plugins
        .handler(&handler_provider, None)
        .map_err(provider_error)?;
    #[cfg(feature = "cables-mcp")]
    let cables_mcp = if let Some(operation_id) = mission.operation_id.as_deref() {
        let theater = state
            .roster
            .operation(operation_id)
            .ok()
            .and_then(|operation| operation.theater.clone());
        let mut endpoint = cables_mcp::Endpoint::start(
            state.cables.clone(),
            CablePrincipal::try_new(
                mission
                    .agent_id
                    .clone()
                    .ok_or_else(|| configuration_error("Mission has no assigned Agent"))?,
                operation_id.to_owned(),
                theater,
                state.station_id.clone(),
                None,
                Some(mission.mission_id.to_string()),
            )
            .map_err(|_| runtime_error("Cable Mission identity is invalid"))?,
            state.intake.clone(),
            resume_agent.clearance == "00",
        )
        .await
        .map_err(|error| runtime_error(format!("Cable MCP listener failed: {error}")))?;
        endpoint.add_environment(&mut environment);
        endpoint
            .bind_session(Some(&session_id))
            .map_err(|_| runtime_error("Cable Session binding failed"))?;
        Some(endpoint)
    } else {
        None
    };
    #[cfg(feature = "publisher")]
    let terminal_handle = match &telemetry {
        Some(telemetry) => TerminalBridge::agent_terminal_with_watchdog_and_telemetry(
            terminal,
            telemetry.clone(),
            watchdog.clone(),
        ),
        None => TerminalBridge::agent_terminal_with_watchdog(terminal, watchdog.clone()),
    };
    #[cfg(not(feature = "publisher"))]
    let terminal_handle = TerminalBridge::agent_terminal_with_watchdog(terminal, watchdog.clone());
    let resume_request = AgentResume {
        mission_id: mission.mission_id.clone(),
        session_id: session_id.clone(),
        message: request.message,
        working_directory: working_directory.clone(),
        environment: environment.clone(),
        model: model.clone(),
        effort: effort.clone(),
    };
    let resumed = tokio::task::spawn_blocking(move || {
        let expected_session_id = resume_request.session_id.clone();
        let resumed = handler
            .resume(resume_request, terminal_handle)
            .map_err(|error| error.to_string())?;
        let resumed_session_id = resumed.session_id().map_err(|error| error.to_string())?;
        if resumed_session_id.as_ref() != Some(&expected_session_id) {
            let _ = resumed.close();
            return Err("AgentHandler resume did not retain the provider Session ID".to_owned());
        }
        if let Some(old_session) = old_session {
            let old_session = old_session
                .lock()
                .map_err(|_| "provider Session lock was poisoned".to_owned())?;
            old_session.interrupt().map_err(|error| error.to_string())?;
            old_session.close().map_err(|error| error.to_string())?;
        }
        Ok::<_, String>(resumed)
    })
    .await
    .map_err(|error| runtime_error(format!("AgentHandler resume worker failed: {error}")))?
    .map_err(provider_error)?;
    let resumed = Arc::new(Mutex::new(resumed));
    state.sessions.lock().map_err(|_| internal_error())?.insert(
        mission_id.clone(),
        AgentOperation {
            handler_provider: handler_provider.clone(),
            terminal_provider: terminal_provider.clone(),
            working_directory,
            environment,
            model,
            effort,
            cover: cover_audit,
            safe_house,
            session: resumed.clone(),
            watchdog,
            session_id: Some(session_id.clone()),
            #[cfg(feature = "cables-mcp")]
            _cables_mcp: cables_mcp,
            #[cfg(feature = "publisher")]
            telemetry: telemetry.clone(),
        },
    );
    let working = state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .transition(&mission_id, MissionState::Working)
        .map_err(mission_error)?;
    sync_session_record(&state, &mission_id);
    #[cfg(feature = "publisher")]
    if let Some(telemetry) = &telemetry {
        telemetry.publish_lifecycle("working");
    }
    spawn_event_pump(state.clone(), mission_id, resumed);
    Ok((
        StatusCode::ACCEPTED,
        Json(StartedMission {
            mission: working,
            handler_provider,
            terminal_provider,
            provider_session_id: Some(session_id),
        }),
    ))
}

async fn send_message(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
    Json(request): Json<SendMessage>,
) -> Result<StatusCode, ApiErrorResponse> {
    let (session, watchdog) = state
        .sessions
        .lock()
        .map_err(|_| internal_error())?
        .get(&mission_id)
        .map(|operation| (operation.session.clone(), operation.watchdog.clone()))
        .ok_or_else(|| mission_error(MissionError::NotFound))?;
    tokio::task::spawn_blocking(move || {
        session
            .lock()
            .map_err(|_| "provider Session lock was poisoned".to_owned())?
            .send_message(AgentMessage { text: request.text })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| runtime_error(format!("message worker failed: {error}")))?
    .map_err(provider_error)?;
    watchdog.record_operator_input();
    let current = state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .get(&mission_id)
        .map(|mission| mission.state)
        .ok_or_else(|| mission_error(MissionError::NotFound))?;
    if matches!(current, MissionState::Blocked | MissionState::Stalled) {
        state
            .missions
            .lock()
            .map_err(|_| internal_error())?
            .transition(&mission_id, MissionState::Working)
            .map_err(mission_error)?;
        publish_lifecycle(&state, &mission_id, MissionState::Working);
    }
    Ok(StatusCode::ACCEPTED)
}

async fn interrupt_mission(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
) -> Result<Json<Mission>, ApiErrorResponse> {
    let session = state
        .sessions
        .lock()
        .map_err(|_| internal_error())?
        .get(&mission_id)
        .map(|operation| operation.session.clone())
        .ok_or_else(|| mission_error(MissionError::NotFound))?;
    tokio::task::spawn_blocking(move || {
        session
            .lock()
            .map_err(|_| "provider Session lock was poisoned".to_owned())?
            .interrupt()
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| runtime_error(format!("interrupt worker failed: {error}")))?
    .map_err(provider_error)?;
    let mission = state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .transition(&mission_id, MissionState::Blocked)
        .map_err(mission_error)?;
    publish_lifecycle(&state, &mission_id, mission.state);
    Ok(Json(mission))
}

async fn next_event(
    State(state): State<DaemonState>,
    axum::extract::Path(mission_id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<EventQuery>,
) -> Result<Json<MissionEvent>, ApiErrorResponse> {
    let timeout_ms = query.timeout_ms.unwrap_or(0).clamp(0, 30_000);
    if state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .get(&mission_id)
        .is_none()
    {
        return Err(mission_error(MissionError::NotFound));
    }
    let event = if timeout_ms == 0 {
        pop_event(&state, &mission_id)
    } else {
        tokio::time::timeout(std::time::Duration::from_millis(timeout_ms as u64), async {
            loop {
                let notified = state.event_ready.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Some(event) = pop_event(&state, &mission_id) {
                    return Some(event);
                }
                notified.await;
            }
        })
        .await
        .unwrap_or(None)
    };
    let mission = state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .get(&mission_id)
        .cloned()
        .ok_or_else(|| mission_error(MissionError::NotFound))?;
    Ok(Json(MissionEvent { mission, event }))
}

fn spawn_event_pump(
    state: DaemonState,
    mission_id: String,
    session: Arc<Mutex<guatiao::library::Object<dyn AgentSession>>>,
) {
    let monitor = state
        .sessions
        .lock()
        .ok()
        .and_then(|sessions| {
            sessions
                .get(&mission_id)
                .map(|operation| operation.watchdog.clone())
        })
        .unwrap_or_default();
    spawn_event_pump_with_monitor(state, mission_id, session, monitor);
}

fn spawn_event_pump_with_monitor(
    state: DaemonState,
    mission_id: String,
    session: Arc<Mutex<guatiao::library::Object<dyn AgentSession>>>,
    monitor: Watchdog,
) {
    if let Ok(mut sessions) = state.sessions.lock()
        && let Some(operation) = sessions.get_mut(&mission_id)
        && Arc::ptr_eq(&operation.session, &session)
    {
        operation.watchdog = monitor.clone();
    }
    let event_pumps = state.event_pumps.clone();
    let event_pump = tokio::task::spawn_blocking(move || {
        let mut output_revision = monitor.output_revision();
        loop {
            if state.stopping.load(Ordering::Acquire) {
                stop_operation(&state, &mission_id, &session);
                break;
            }
            if !is_current_session(&state, &mission_id, &session) {
                break;
            }
            let result = (|| {
                let session = session
                    .lock()
                    .map_err(|_| "provider Session lock was poisoned".to_owned())?;
                let event = session
                    .next_event(1_000)
                    .map_err(|error| error.to_string())?;
                let session_id = session.session_id().map_err(|error| error.to_string())?;
                Ok::<_, String>((event, session_id))
            })();
            if !is_current_session(&state, &mission_id, &session) {
                break;
            }
            match result {
                Ok((Some(event), session_id)) => {
                    monitor.record_activity();
                    attach_observed_session(&state, &mission_id, &session, session_id);
                    if matches!(event.kind.as_str(), "completed" | "failed") {
                        let finished = finish_operation(&state, &mission_id, &session);
                        publish_finished_event(&state, &mission_id, event, finished);
                        break;
                    }
                    apply_event_state(&state, &mission_id, &event);
                    queue_event(&state, &mission_id, event);
                }
                Ok((None, session_id)) => {
                    attach_observed_session(&state, &mission_id, &session, session_id);
                }
                Err(error) => {
                    let event = AgentEvent {
                        kind: "failed".to_owned(),
                        text: Some(error),
                        data: None,
                    };
                    let finished = finish_operation(&state, &mission_id, &session);
                    publish_finished_event(&state, &mission_id, event, finished);
                    break;
                }
            }

            if !is_current_session(&state, &mission_id, &session) {
                break;
            }
            let current_output_revision = monitor.output_revision();
            if current_output_revision != output_revision {
                output_revision = current_output_revision;
                let resumed = state.missions.lock().ok().and_then(|mut missions| {
                    (missions.get(&mission_id)?.state == MissionState::Blocked)
                        .then(|| missions.transition(&mission_id, MissionState::Working).ok())
                        .flatten()
                });
                if resumed.is_some() {
                    publish_lifecycle(&state, &mission_id, MissionState::Working);
                    queue_event(
                        &state,
                        &mission_id,
                        AgentEvent {
                            kind: "working".to_owned(),
                            text: Some("terminal output resumed".to_owned()),
                            data: None,
                        },
                    );
                }
            }

            if monitor.is_waiting_at_prompt()
                && state
                    .missions
                    .lock()
                    .ok()
                    .and_then(|missions| missions.get(&mission_id).map(|mission| mission.state))
                    == Some(MissionState::Working)
            {
                let event = AgentEvent {
                    kind: "blocked".to_owned(),
                    text: Some("waiting at a terminal prompt".to_owned()),
                    data: None,
                };
                apply_event_state(&state, &mission_id, &event);
                queue_event(&state, &mission_id, event);
            }

            let looping = monitor.is_looping();
            let max_runtime = monitor.is_max_runtime_exceeded();
            if max_runtime || looping || monitor.is_stalled() {
                let reason = if max_runtime {
                    format!(
                        "maximum Session runtime of {:.1} seconds was reached",
                        monitor.max_runtime().unwrap_or_default().as_secs_f64()
                    )
                } else if looping {
                    format!(
                        "no new terminal line for {:.1} seconds after repeated output",
                        monitor.loop_timeout().as_secs_f64()
                    )
                } else if monitor.is_waiting_at_prompt() {
                    format!(
                        "agent waited at a terminal prompt for {:.1} seconds",
                        monitor.timeout().as_secs_f64()
                    )
                } else {
                    format!(
                        "no provider or terminal activity for {:.1} seconds",
                        monitor.timeout().as_secs_f64()
                    )
                };
                let event = AgentEvent {
                    kind: "stalled".to_owned(),
                    text: Some(reason),
                    data: None,
                };
                apply_event_state(&state, &mission_id, &event);
                queue_event(&state, &mission_id, event);
                if let Ok(session) = session.lock() {
                    let _ = session.interrupt();
                }
                finish_operation(&state, &mission_id, &session);
                break;
            }
        }
    });
    if let Ok(mut event_pumps) = event_pumps.lock() {
        event_pumps.retain(|event_pump| !event_pump.is_finished());
        event_pumps.push(event_pump);
    }
}

fn is_current_session(
    state: &DaemonState,
    mission_id: &str,
    session: &Arc<Mutex<guatiao::library::Object<dyn AgentSession>>>,
) -> bool {
    state
        .sessions
        .lock()
        .ok()
        .and_then(|sessions| {
            sessions
                .get(mission_id)
                .map(|operation| Arc::ptr_eq(&operation.session, session))
        })
        .unwrap_or(false)
}

fn attach_observed_session(
    state: &DaemonState,
    mission_id: &str,
    session: &Arc<Mutex<guatiao::library::Object<dyn AgentSession>>>,
    session_id: Option<SessionId>,
) {
    let Some(session_id) = session_id else {
        return;
    };
    let should_attach = state
        .sessions
        .lock()
        .ok()
        .and_then(|mut sessions| {
            let operation = sessions.get_mut(mission_id)?;
            if !Arc::ptr_eq(&operation.session, session) || operation.session_id.is_some() {
                return None;
            }
            operation.session_id = Some(session_id.clone());
            Some(())
        })
        .is_some();
    if should_attach {
        if let Ok(mut missions) = state.missions.lock() {
            let _ = missions.attach_session(mission_id, session_id.clone());
        }
        sync_session_record(state, mission_id);
    }
    #[cfg(feature = "publisher")]
    if should_attach {
        let telemetry = state
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(mission_id)?.telemetry.clone());
        if let Some(telemetry) = telemetry {
            telemetry.bind(session_id);
            let current = state
                .missions
                .lock()
                .ok()
                .and_then(|missions| missions.get(mission_id).map(|mission| mission.state));
            if let Some(current) = current {
                publish_lifecycle(state, mission_id, current);
            }
        }
    }
}

fn queue_event(state: &DaemonState, mission_id: &str, event: AgentEvent) {
    if let Ok(mut events) = state.events.lock() {
        let queue = events.entry(mission_id.to_owned()).or_default();
        if queue.len() == EVENT_QUEUE_LIMIT {
            queue.pop_front();
        }
        queue.push_back(event);
        state.event_ready.notify_waiters();
    }
}

fn pop_event(state: &DaemonState, mission_id: &str) -> Option<AgentEvent> {
    state
        .events
        .lock()
        .ok()
        .and_then(|mut events| events.get_mut(mission_id).and_then(VecDeque::pop_front))
}

fn apply_event_state(
    state: &DaemonState,
    mission_id: &str,
    event: &AgentEvent,
) -> Option<MissionState> {
    let target = match event.kind.as_str() {
        "blocked" => MissionState::Blocked,
        "stalled" => MissionState::Stalled,
        "completed" => MissionState::Done,
        "failed" => MissionState::Failed,
        "blown" => MissionState::Blown,
        _ => return None,
    };
    let Ok(mut missions) = state.missions.lock() else {
        return Some(target);
    };
    let Some(current) = missions.get(mission_id).map(|mission| mission.state) else {
        return Some(target);
    };
    if current != target {
        if target == MissionState::Done
            && matches!(current, MissionState::Blocked | MissionState::Stalled)
        {
            let _ = missions.transition(mission_id, MissionState::Working);
        }
        let _ = missions.transition(mission_id, target);
    }
    let published_state = missions
        .get(mission_id)
        .map(|mission| mission.state)
        .unwrap_or(target);
    drop(missions);
    sync_session_record(state, mission_id);
    publish_lifecycle(state, mission_id, published_state);
    Some(target)
}

fn publish_lifecycle(state: &DaemonState, mission_id: &str, mission_state: MissionState) {
    #[cfg(feature = "publisher")]
    if let Some(state_name) = lifecycle_name(mission_state) {
        let telemetry = state
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(mission_id)?.telemetry.clone());
        if let Some(telemetry) = telemetry {
            telemetry.publish_lifecycle(state_name);
        }
    }
    #[cfg(not(feature = "publisher"))]
    let _ = (state, mission_id, mission_state);
}

#[cfg(feature = "publisher")]
fn lifecycle_name(state: MissionState) -> Option<&'static str> {
    match state {
        MissionState::Working => Some("working"),
        MissionState::Blocked => Some("blocked"),
        MissionState::Stalled => Some("stalled"),
        MissionState::Done => Some("done"),
        MissionState::Failed => Some("failed"),
        MissionState::Blown => Some("failed"),
        MissionState::Queued | MissionState::Abandoned => None,
    }
}

fn is_terminal_state(state: MissionState) -> bool {
    matches!(
        state,
        MissionState::Done | MissionState::Failed | MissionState::Blown | MissionState::Abandoned
    )
}

struct FinishOutcome {
    cover_blown: bool,
    fail_on_blow: bool,
    failure: Option<String>,
    #[cfg(feature = "publisher")]
    telemetry: Option<Arc<SessionTelemetry>>,
}

fn finish_operation(
    state: &DaemonState,
    mission_id: &str,
    session: &Arc<Mutex<guatiao::library::Object<dyn AgentSession>>>,
) -> Option<FinishOutcome> {
    let operation = state.sessions.lock().ok().and_then(|mut sessions| {
        if sessions
            .get(mission_id)
            .is_some_and(|operation| Arc::ptr_eq(&operation.session, session))
        {
            sessions.remove(mission_id)
        } else {
            None
        }
    });
    let operation = operation?;
    if let Ok(session) = operation.session.lock() {
        let _ = session.close();
    }

    let mut failure = None;
    let mut cover_blown = false;
    let mut fail_on_blow = false;
    if let Some(audit) = &operation.cover {
        fail_on_blow = audit.fail_on_blow;
        let result = (|| {
            let cover = state
                .roster
                .cover(&audit.policy)
                .map_err(|error| error.to_string())?;
            let safe_house = operation
                .safe_house
                .as_ref()
                .ok_or_else(|| "no safe house to read the Mission's changes from".to_owned())?;
            // Without history there is nothing to inspect: report that, never a pass.
            let checkable = safe_house.supports_history();
            let commits = if checkable {
                safe_house.changes().map_err(|error| error.to_string())?
            } else {
                Vec::new()
            };
            let violations = cover.check_commits(
                &commits,
                audit.identity.as_ref(),
                operation.model.as_deref().unwrap_or_default(),
            );
            let report = CoverReport {
                policy: audit.policy.clone(),
                commits: commits.len(),
                checkable,
                blown: !violations.is_empty(),
                violations,
            };
            cover_blown = report.blown;
            state
                .missions
                .lock()
                .map_err(|_| "Mission book lock was poisoned".to_owned())?
                .set_cover_report(mission_id, report)
                .map_err(|error| format!("cover report could not be persisted: {error:?}"))?;
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            failure = Some(format!("cover audit failed: {error}"));
        }
    }

    if let Some(safe_house) = operation.safe_house.filter(SafeHouse::is_isolated) {
        match safe_house.finish() {
            Ok(report) => {
                if let Ok(mut missions) = state.missions.lock() {
                    if let Err(error) = missions.set_safe_house(mission_id, report) {
                        failure.get_or_insert_with(|| {
                            format!("safe-house report could not be persisted: {error:?}")
                        });
                    }
                } else {
                    failure.get_or_insert_with(|| "Mission book lock was poisoned".to_owned());
                }
            }
            Err(error) => {
                failure.get_or_insert_with(|| format!("safe-house finalization failed: {error}"));
            }
        }
    }

    Some(FinishOutcome {
        cover_blown,
        fail_on_blow,
        failure,
        #[cfg(feature = "publisher")]
        telemetry: operation.telemetry,
    })
}

fn publish_finished_event(
    state: &DaemonState,
    mission_id: &str,
    event: AgentEvent,
    finished: Option<FinishOutcome>,
) {
    let event = if let Some(finished) = &finished {
        apply_finish_outcome(event, finished)
    } else {
        event
    };
    #[cfg(feature = "publisher")]
    if let Some(finished) = finished
        && let Some(telemetry) = finished.telemetry
    {
        let state = match event.kind.as_str() {
            "completed" => Some("done"),
            "failed" | "blown" => Some("failed"),
            _ => None,
        };
        if let Some(state) = state {
            telemetry.publish_lifecycle(state);
        }
    }
    apply_event_state(state, mission_id, &event);
    queue_event(state, mission_id, event);
}

fn apply_finish_outcome(mut event: AgentEvent, finished: &FinishOutcome) -> AgentEvent {
    if let Some(failure) = &finished.failure {
        event.kind = "failed".to_owned();
        event.text = Some(failure.clone());
    } else if finished.cover_blown && finished.fail_on_blow {
        event.kind = "blown".to_owned();
        event.text = Some("cover blown".to_owned());
    }
    event
}

fn stop_operation(
    state: &DaemonState,
    mission_id: &str,
    session: &Arc<Mutex<guatiao::library::Object<dyn AgentSession>>>,
) {
    let operation = state.sessions.lock().ok().and_then(|mut sessions| {
        if sessions
            .get(mission_id)
            .is_some_and(|operation| Arc::ptr_eq(&operation.session, session))
        {
            sessions.remove(mission_id)
        } else {
            None
        }
    });
    let Some(operation) = operation else {
        return;
    };
    if let Ok(session) = operation.session.lock() {
        let _ = session.interrupt();
        let _ = session.close();
    }
    let blocked = if let Ok(mut missions) = state.missions.lock()
        && missions
            .get(mission_id)
            .is_some_and(|mission| mission.state == MissionState::Working)
    {
        missions
            .transition(mission_id, MissionState::Blocked)
            .is_ok()
    } else {
        false
    };
    #[cfg(feature = "publisher")]
    if blocked && let Some(telemetry) = &operation.telemetry {
        telemetry.publish_lifecycle("blocked");
    }
    #[cfg(not(feature = "publisher"))]
    let _ = blocked;
}

async fn finalize_mission_resources(
    state: &DaemonState,
    mission_id: &str,
    interrupt: bool,
) -> Result<(), ApiErrorResponse> {
    let operation = state
        .sessions
        .lock()
        .map_err(|_| internal_error())?
        .remove(mission_id);
    let Some(operation) = operation else {
        return Ok(());
    };
    let report = tokio::task::spawn_blocking(move || {
        let session = operation
            .session
            .lock()
            .map_err(|_| "provider Session lock was poisoned".to_owned())?;
        if interrupt {
            session
                .interrupt()
                .map_err(|error| format!("provider interrupt failed: {error}"))?;
        }
        session
            .close()
            .map_err(|error| format!("provider close failed: {error}"))?;
        drop(session);
        operation
            .safe_house
            .filter(SafeHouse::is_isolated)
            .map(|safe_house| safe_house.finish().map_err(|error| error.to_string()))
            .transpose()
    })
    .await
    .map_err(|error| runtime_error(format!("provider close worker failed: {error}")))?
    .map_err(runtime_error)?;
    let Some(report) = report else {
        return Ok(());
    };
    state
        .missions
        .lock()
        .map_err(|_| internal_error())?
        .set_safe_house(mission_id, report)
        .map_err(mission_error)?;
    Ok(())
}

async fn start_failure(
    state: &DaemonState,
    mission_id: &str,
    error: impl std::fmt::Display,
    safe_house: Option<SafeHouse>,
) -> ApiErrorResponse {
    let mut message = error.to_string();
    if let Some(safe_house) = safe_house.filter(SafeHouse::is_isolated) {
        match tokio::task::spawn_blocking(move || safe_house.finish()).await {
            Ok(Ok(report)) if report.kept => {
                if let Ok(mut missions) = state.missions.lock() {
                    let _ = missions.set_safe_house(mission_id, report);
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(cleanup)) => {
                message.push_str(&format!("; safe-house cleanup failed: {cleanup}"));
            }
            Err(join_error) => {
                message.push_str(&format!("; safe-house cleanup worker failed: {join_error}"));
            }
        }
    }
    provider_error(message)
}

async fn start_failure_with_session(
    state: &DaemonState,
    mission_id: &str,
    error: impl std::fmt::Display,
    session: guatiao::library::Object<dyn AgentSession>,
    safe_house: Option<SafeHouse>,
) -> ApiErrorResponse {
    let mut message = error.to_string();
    match tokio::task::spawn_blocking(move || session.close().map_err(|error| error.to_string()))
        .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => message.push_str(&format!("; provider close failed: {error}")),
        Err(error) => message.push_str(&format!("; provider close worker failed: {error}")),
    }
    start_failure(state, mission_id, message, safe_house).await
}

#[derive(Debug, Serialize)]
struct StartedMission {
    mission: Mission,
    handler_provider: String,
    terminal_provider: String,
    provider_session_id: Option<SessionId>,
}

#[derive(Debug)]
struct ApiErrorResponse(StatusCode, Json<ApiError>);

impl axum::response::IntoResponse for ApiErrorResponse {
    fn into_response(self) -> axum::response::Response {
        (self.0, self.1).into_response()
    }
}

fn mission_error(error: MissionError) -> ApiErrorResponse {
    let status = match error {
        MissionError::NotFound => StatusCode::NOT_FOUND,
        MissionError::AlreadyExists
        | MissionError::InvalidTransition { .. }
        | MissionError::TerminalMission
        | MissionError::SessionAlreadyAttached => StatusCode::CONFLICT,
        MissionError::EmptyPrompt | MissionError::InvalidOperation => StatusCode::BAD_REQUEST,
        MissionError::Persistence => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiErrorResponse(
        status,
        Json(ApiError {
            error: format!("{error:?}"),
        }),
    )
}

fn internal_error() -> ApiErrorResponse {
    ApiErrorResponse(
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            error: "internal state lock failed".to_owned(),
        }),
    )
}

fn provider_error(error: impl std::fmt::Display) -> ApiErrorResponse {
    ApiErrorResponse(
        StatusCode::BAD_GATEWAY,
        Json(ApiError {
            error: error.to_string(),
        }),
    )
}

fn runtime_error(error: impl std::fmt::Display) -> ApiErrorResponse {
    ApiErrorResponse(
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            error: error.to_string(),
        }),
    )
}

fn configuration_error(error: impl std::fmt::Display) -> ApiErrorResponse {
    ApiErrorResponse(
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            error: error.to_string(),
        }),
    )
}

fn conflict(message: &str) -> ApiErrorResponse {
    ApiErrorResponse(
        StatusCode::CONFLICT,
        Json(ApiError {
            error: message.to_owned(),
        }),
    )
}

fn bad_request(message: &str) -> ApiErrorResponse {
    ApiErrorResponse(
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            error: message.to_owned(),
        }),
    )
}

fn forbidden(message: &str) -> ApiErrorResponse {
    ApiErrorResponse(
        StatusCode::FORBIDDEN,
        Json(ApiError {
            error: message.to_owned(),
        }),
    )
}

fn unavailable(message: &str) -> ApiErrorResponse {
    ApiErrorResponse(
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ApiError {
            error: message.to_owned(),
        }),
    )
}

fn capacity_error(message: impl Into<String>) -> ApiErrorResponse {
    ApiErrorResponse(
        StatusCode::TOO_MANY_REQUESTS,
        Json(ApiError {
            error: message.into(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn a_failed_cover_policy_marks_the_mission_blown() {
        let event = AgentEvent {
            kind: "completed".to_owned(),
            text: None,
            data: None,
        };
        let finished = FinishOutcome {
            cover_blown: true,
            fail_on_blow: true,
            failure: None,
            #[cfg(feature = "publisher")]
            telemetry: None,
        };

        let event = apply_finish_outcome(event, &finished);

        assert_eq!(event.kind, "blown");
        assert_eq!(event.text.as_deref(), Some("cover blown"));
    }

    #[test]
    fn a_flagged_cover_preserves_the_agent_outcome() {
        let event = AgentEvent {
            kind: "completed".to_owned(),
            text: None,
            data: None,
        };
        let finished = FinishOutcome {
            cover_blown: true,
            fail_on_blow: false,
            failure: None,
            #[cfg(feature = "publisher")]
            telemetry: None,
        };

        let event = apply_finish_outcome(event, &finished);

        assert_eq!(event.kind, "completed");
    }
    use crate::directorates::q::safehouse::{
        ChangeLog, SafeHouseInfo, SafeHouseProvider, SafeHouseSession,
    };
    use crate::directorates::q::terminal::{ExitInfo, Terminal, TerminalSession};
    use crate::directorates::registry::cables::{
        CableAction, CableGrant, CablePolicy, CableScope, CableTarget,
    };
    use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart};
    use crate::guatiao::library::{Object, ProviderError, Registry};
    use crate::guatiao::{FromValue, Provider, Status, ToValue, Value};

    fn no_op_cables() -> Arc<CableRelay> {
        Arc::new(CableRelay::new(
            Arc::new(FileRegistry::open(
                std::env::temp_dir().join("ai6-empty-cable-tests"),
            )),
            CablePolicy::default(),
            Vec::new,
            BTreeMap::new(),
        ))
    }

    async fn wait_for_event(state: &DaemonState, mission_id: &str) -> AgentEvent {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let Some(event) = pop_event(state, mission_id) {
                    return event;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("daemon should queue the Session event")
    }

    #[tokio::test]
    async fn intake_endpoint_lists_briefs_without_claiming_them() {
        let root = tempfile::tempdir().unwrap();
        let queue = root.path().join("new");
        fs::create_dir(&queue).unwrap();
        let path = queue.join("manual--brief-1.json");
        fs::write(&path, r#"{"title":"Inspect the operation","id":"brief-1"}"#).unwrap();
        let state = DaemonState {
            plugins: Arc::new(plugins::Plugins::scan(&[])),
            roster: Arc::new(Roster::default()),
            intake: Arc::new(FileRegistry::open(root.path())),
            missions: Arc::new(Mutex::new(MissionStore::default())),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "test".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };

        let Json(briefs) = list_intake_briefs(State(state)).await.unwrap();

        assert_eq!(briefs.len(), 1);
        assert_eq!(briefs[0].key(), "manual--brief-1");
        assert!(path.is_file());
    }

    #[tokio::test]
    async fn cable_send_binds_identity_to_the_active_mission_and_delivers_to_authorized_peer() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("operations.toml"),
            "[operation.G]\nclearance = \"00\"\nroster = [\"007\", \"008\"]\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("agents.toml"),
            "[agent.007]\nhandler = \"mock-agent\"\nclearance = \"00\"\n[agent.008]\nhandler = \"mock-agent\"\nclearance = \"00\"\n",
        )
        .unwrap();

        let session_id = SessionId::try_new("session-007").unwrap();
        let mission_id = MissionId::try_new("mission-007").unwrap();
        let receiver_session_id = SessionId::try_new("session-008").unwrap();
        let receiver_mission_id = MissionId::try_new("mission-008").unwrap();
        let mut missions = MissionStore::default();
        missions
            .create(
                mission_id.clone(),
                Some("G".to_owned()),
                "review the change".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        missions
            .transition(mission_id.as_str(), MissionState::Working)
            .unwrap();
        missions
            .attach_session(mission_id.as_str(), session_id.clone())
            .unwrap();
        missions
            .create(
                receiver_mission_id.clone(),
                Some("G".to_owned()),
                "review the change".to_owned(),
                Some("008".to_owned()),
                None,
            )
            .unwrap();
        missions
            .transition(receiver_mission_id.as_str(), MissionState::Working)
            .unwrap();
        missions
            .attach_session(receiver_mission_id.as_str(), receiver_session_id.clone())
            .unwrap();

        let roster = Arc::new(Roster::load(dir.path()).unwrap());
        let missions = Arc::new(Mutex::new(missions));
        let mut operations = BTreeMap::new();
        for (mission, session) in [
            (mission_id.as_str(), session_id.clone()),
            (receiver_mission_id.as_str(), receiver_session_id.clone()),
        ] {
            operations.insert(
                mission.to_owned(),
                AgentOperation {
                    handler_provider: "mock-agent".to_owned(),
                    terminal_provider: "mock-terminal".to_owned(),
                    working_directory: dir.path().to_string_lossy().into_owned(),
                    environment: BTreeMap::new(),
                    model: None,
                    effort: None,
                    cover: None,
                    safe_house: None,
                    session: Arc::new(Mutex::new(
                        MockSession::new(session.clone(), "working").into_object(),
                    )),
                    watchdog: Watchdog::default(),
                    session_id: Some(session),
                    #[cfg(feature = "cables-mcp")]
                    _cables_mcp: None,
                    #[cfg(feature = "publisher")]
                    telemetry: None,
                },
            );
        }
        let sessions = Arc::new(Mutex::new(operations));

        let registry = Arc::new(FileRegistry::open(dir.path().join("registry")));
        let station_target =
            CableTarget::try_new(CableScope::Station, Some("local".to_owned())).unwrap();
        let endpoint_roster = roster.clone();
        let endpoint_missions = missions.clone();
        let endpoint_sessions = sessions.clone();
        let relay = CableRelay::new(
            registry,
            CablePolicy::try_new(
                vec![
                    CableGrant::try_new(
                        "007".to_owned(),
                        CableAction::Send,
                        station_target.clone(),
                    )
                    .unwrap(),
                    CableGrant::try_new("008".to_owned(), CableAction::Receive, station_target)
                        .unwrap(),
                    CableGrant::try_new(
                        "007".to_owned(),
                        crate::directorates::registry::cables::CableAction::Release,
                        CableTarget::try_new(CableScope::Station, Some("local".to_owned()))
                            .unwrap(),
                    )
                    .unwrap(),
                ],
                BTreeMap::from([(
                    "008".to_owned(),
                    crate::directorates::registry::cables::CableInboundMode::Hold,
                )]),
            )
            .unwrap(),
            move || {
                cable_endpoints(
                    &endpoint_roster,
                    &endpoint_missions,
                    &endpoint_sessions,
                    "local",
                )
            },
            BTreeMap::new(),
        );
        let state = DaemonState {
            plugins: Arc::new(plugins::Plugins::scan(&[])),
            roster,
            intake: Arc::new(FileRegistry::open(dir.path().join("registry"))),
            missions,
            sessions,
            cables: Arc::new(relay),
            station_id: "local".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };

        let response = send_cable(
            State(state.clone()),
            axum::extract::Path(mission_id.to_string()),
            Json(SendCable {
                target: CableTarget::try_new(CableScope::Station, Some("local".to_owned()))
                    .unwrap(),
                body: "Please review the API change".to_owned(),
                idempotency_key: "mission-007-request-1".to_owned(),
                ttl_seconds: None,
                conversation_id: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.0, StatusCode::ACCEPTED);
        let cable_id = response.1.cable_id.clone();

        let Json(held) = held_cables(
            State(state.clone()),
            axum::extract::Path(mission_id.to_string()),
            axum::extract::Query(CableInboxQuery { limit: None }),
        )
        .await
        .unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].cable_id, cable_id);
        let Json(released) = release_cable(
            State(state.clone()),
            axum::extract::Path((mission_id.to_string(), cable_id)),
        )
        .await
        .unwrap();
        assert_eq!(released.delivered, 1);

        let receiver = crate::directorates::registry::cables::CablePrincipal::try_new(
            "008".to_owned(),
            "G".to_owned(),
            None,
            "local".to_owned(),
            Some("session-008".to_owned()),
            Some("mission-008".to_owned()),
        )
        .unwrap();
        let deliveries = state.cables.inbox(&receiver, 10).unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].from_agent, "007");
        assert_eq!(deliveries[0].session_id.as_deref(), Some("session-007"));
        assert_eq!(deliveries[0].mission_id.as_deref(), Some("mission-007"));
        assert_eq!(deliveries[0].body, "Please review the API change");
        assert!(
            serde_json::from_value::<SendCable>(serde_json::json!({
                "target": {"scope":"station", "name":"G"},
                "body": "spoof",
                "idempotency_key": "spoof-1",
                "sender_agent": "008"
            }))
            .is_err()
        );
    }

    #[derive(Default, Provider)]
    #[provider(AgentHandler, id = "mock-agent", name = "Mock Agent", crate = crate::guatiao)]
    struct MockHandler;

    impl AgentHandler for MockHandler {
        fn start(
            &self,
            _request: AgentStart,
            _terminal: Object<dyn crate::directorates::q::terminal::AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(MockSession::new(
                SessionId::try_new("provider-session-007")
                    .map_err(|error| test_provider_error(&error.to_string()))?,
                "blocked",
            )
            .into_object())
        }

        fn resume(
            &self,
            request: AgentResume,
            _terminal: Object<dyn crate::directorates::q::terminal::AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            Ok(MockSession::new(request.session_id, "completed").into_object())
        }
    }

    struct MockSession {
        session_id: SessionId,
        events: Mutex<VecDeque<AgentEvent>>,
    }

    impl MockSession {
        fn new(session_id: SessionId, event: &str) -> Self {
            Self {
                session_id,
                events: Mutex::new(VecDeque::from([AgentEvent {
                    kind: event.to_owned(),
                    text: None,
                    data: None,
                }])),
            }
        }
    }

    impl AgentSession for MockSession {
        fn session_id(&self) -> Result<Option<SessionId>, ProviderError> {
            Ok(Some(self.session_id.clone()))
        }

        fn send_message(&self, _message: AgentMessage) -> Result<(), ProviderError> {
            Ok(())
        }

        fn next_event(&self, _timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
            Ok(self
                .events
                .lock()
                .map_err(|_| test_provider_error("mock session lock was poisoned"))?
                .pop_front())
        }

        fn interrupt(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn close(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    #[derive(Default, Provider)]
    #[provider(Terminal, id = "mock-terminal", name = "Mock Terminal", crate = crate::guatiao)]
    struct MockTerminalProvider;

    impl Terminal for MockTerminalProvider {
        fn spawn(&self, _spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
            Ok(MockTerminalSession.into_object())
        }
    }

    struct MockTerminalSession;

    impl TerminalSession for MockTerminalSession {
        fn pid(&self) -> i64 {
            0
        }

        fn read(&self, _dst: &mut [u8], _timeout_ms: i64) -> Result<i64, ProviderError> {
            Ok(0)
        }

        fn write(&self, _data: &[u8]) -> Result<(), ProviderError> {
            Ok(())
        }

        fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
            Ok(())
        }

        fn kill(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn exit_status(&self, _timeout_ms: i64) -> Result<Value, ProviderError> {
            ExitInfo::default()
                .to_value()
                .map_err(|error| test_provider_error(&error.to_string()))
        }
    }

    #[derive(Default, Provider)]
    #[provider(SafeHouseProvider, id = "mock-safehouse", name = "Mock Safe House", crate = crate::guatiao)]
    struct MockSafeHouseProvider;

    impl SafeHouseProvider for MockSafeHouseProvider {
        fn open(&self, spec: &Value) -> Result<Object<dyn SafeHouseSession>, ProviderError> {
            let spec = SafeHouseSpec::from_value(spec)
                .map_err(|error| test_provider_error(&error.to_string()))?;
            let path = Path::new(&spec.state_root)
                .join("safe-houses")
                .join(&spec.mission_id);
            fs::create_dir_all(&path).map_err(|error| test_provider_error(&error.to_string()))?;
            Ok(MockSafeHouse {
                info: SafeHouseInfo {
                    path: path.to_string_lossy().into_owned(),
                    branch: Some(
                        spec.branch
                            .replace("{operation}", &spec.operation_id)
                            .replace("{id}", &spec.mission_id)
                            .replace("{slug}", &spec.title),
                    ),
                    base: Some("base0".to_owned()),
                    supports_history: true,
                    isolated: true,
                },
            }
            .into_object())
        }
    }

    /// A provider that keeps no history and works where the source is.
    #[derive(Default, Provider)]
    #[provider(SafeHouseProvider, id = "mock-plain", name = "Mock Plain", crate = crate::guatiao)]
    struct MockPlainProvider;

    impl SafeHouseProvider for MockPlainProvider {
        fn open(&self, spec: &Value) -> Result<Object<dyn SafeHouseSession>, ProviderError> {
            let spec = SafeHouseSpec::from_value(spec)
                .map_err(|error| test_provider_error(&error.to_string()))?;
            Ok(MockSafeHouse {
                info: SafeHouseInfo {
                    path: spec.source,
                    branch: None,
                    base: None,
                    supports_history: false,
                    isolated: false,
                },
            }
            .into_object())
        }
    }

    struct MockSafeHouse {
        info: SafeHouseInfo,
    }

    impl SafeHouseSession for MockSafeHouse {
        fn info(&self) -> Result<Value, ProviderError> {
            self.info
                .to_value()
                .map_err(|error| test_provider_error(&error.to_string()))
        }

        fn changes(&self) -> Result<Value, ProviderError> {
            ChangeLog::default()
                .to_value()
                .map_err(|error| test_provider_error(&error.to_string()))
        }

        fn finish(&self) -> Result<Value, ProviderError> {
            let _ = fs::remove_dir_all(&self.info.path);
            SafeHouseReport {
                path: self.info.path.clone(),
                branch: self.info.branch.clone(),
                base: self.info.base.clone(),
                kept: false,
                changes: Vec::new(),
            }
            .to_value()
            .map_err(|error| test_provider_error(&error.to_string()))
        }
    }

    crate::guatiao::local_providers!(
        id = "ai6-server-test-providers",
        version = "0.0.0",
        providers = [
            MockHandler,
            MockTerminalProvider,
            MockSafeHouseProvider,
            MockPlainProvider
        ]
    );

    fn test_provider_error(message: &str) -> ProviderError {
        ProviderError::new(Status::GUATIAO_ERR_INTERNAL, message)
    }

    fn test_plugins() -> plugins::Plugins {
        let mut registry = Registry::new("ai6-test", env!("CARGO_PKG_VERSION"));
        registry
            .register_local("ai6-server-test-providers", library)
            .expect("mock providers should register");
        plugins::Plugins::from_registry(registry)
    }

    #[test]
    fn unpinned_station_uses_an_available_terminal() {
        let root = tempfile::tempdir().unwrap();
        let mut state = capacity_test_state(root.path(), 1, 1);
        state.station = StationConfig::default();
        assert_eq!(station_terminal(&state).unwrap(), "mock-terminal");
        state.station = StationConfig::default().with_terminal("pinned");
        assert_eq!(station_terminal(&state).unwrap(), "pinned");
        state.plugins = Arc::new(plugins::Plugins::scan(&[]));
        state.station = StationConfig::default();
        assert!(station_terminal(&state).is_err());
    }

    fn capacity_test_state(root: &Path, station_limit: usize, agent_limit: usize) -> DaemonState {
        let checkout = root.join("checkout");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(
            root.join("operations.toml"),
            format!(
                "[operation.G]\npath = {:?}\nmax_concurrent = {station_limit}\n",
                checkout.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(
            root.join("agents.toml"),
            format!("[agent.007]\nhandler = \"mock-agent\"\nmax_concurrent = {agent_limit}\n"),
        )
        .unwrap();
        let mut missions = MissionStore::default();
        for id in ["mission-1", "mission-2"] {
            missions
                .create(
                    MissionId::try_new(id).unwrap(),
                    Some("G".to_owned()),
                    format!("work for {id}"),
                    Some("007".to_owned()),
                    None,
                )
                .unwrap();
        }

        DaemonState {
            plugins: Arc::new(test_plugins()),
            roster: Arc::new(Roster::load(root).unwrap()),
            intake: Arc::new(FileRegistry::open(root.join("registry"))),
            missions: Arc::new(Mutex::new(missions)),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "test".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: 4,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[test]
    fn start_reservations_enforce_station_and_agent_capacity() {
        for (name, station_limit, agent_limit) in [("operation", 1, 2), ("agent", 2, 1)] {
            let root = tempfile::tempdir().unwrap();
            let state = capacity_test_state(root.path(), station_limit, agent_limit);
            let first = state
                .missions
                .lock()
                .unwrap()
                .get("mission-1")
                .unwrap()
                .clone();
            let second = state
                .missions
                .lock()
                .unwrap()
                .get("mission-2")
                .unwrap()
                .clone();
            let agent = state.roster.agent(Some("G"), "007").unwrap();

            let Ok(reservation) = reserve_mission_start(&state, &first, &agent) else {
                panic!("first Mission should reserve available capacity");
            };
            let Err(error) = reserve_mission_start(&state, &second, &agent) else {
                panic!("the configured capacity should prevent a second Mission");
            };
            assert_eq!(error.0, StatusCode::CONFLICT, "{name} capacity");
            drop(reservation);
        }
    }

    #[test]
    fn station_registry_capacity_overrides_the_daemon_default_immediately() {
        let root = tempfile::tempdir().unwrap();
        let state = capacity_test_state(root.path(), 4, 4);
        let first = state
            .missions
            .lock()
            .unwrap()
            .get("mission-1")
            .unwrap()
            .clone();
        let second = state
            .missions
            .lock()
            .unwrap()
            .get("mission-2")
            .unwrap()
            .clone();
        let agent = state.roster.agent(Some("G"), "007").unwrap();
        let settings =
            crate::directorates::registry::store::StationSettings::try_new("test", 1).unwrap();
        state
            .intake
            .set_station_settings("test", &settings)
            .unwrap();

        let reservation = reserve_mission_start(&state, &first, &agent).unwrap();
        let Err(error) = reserve_mission_start(&state, &second, &agent) else {
            panic!("the live Station limit should prevent a second Mission");
        };
        assert_eq!(error.0, StatusCode::TOO_MANY_REQUESTS);
        drop(reservation);

        let settings =
            crate::directorates::registry::store::StationSettings::try_new("test", 2).unwrap();
        state
            .intake
            .set_station_settings("test", &settings)
            .unwrap();
        let first_reservation = reserve_mission_start(&state, &first, &agent).unwrap();
        let second_reservation = reserve_mission_start(&state, &second, &agent).unwrap();
        drop((first_reservation, second_reservation));
    }

    async fn start_first_mission(state: &DaemonState) {
        start_mission(
            State(state.clone()),
            axum::extract::Path("mission-1".to_owned()),
            Json(StartMission {
                working_directory: None,
                initial_context: Vec::new(),
                environment: BTreeMap::new(),
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("start failed: {:?}", error.1.0.error));
        wait_for_state(state, "mission-1", MissionState::Blocked).await;
    }

    async fn wait_for_state(state: &DaemonState, mission_id: &str, wanted: MissionState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while state
                .missions
                .lock()
                .unwrap()
                .get(mission_id)
                .map(|mission| mission.state)
                != Some(wanted)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("Mission {mission_id} never reached {wanted:?}"));
    }

    #[tokio::test]
    async fn a_started_session_is_recorded_in_the_shared_registry() {
        let root = tempfile::tempdir().unwrap();
        let state = capacity_test_state(root.path(), 2, 2);
        start_first_mission(&state).await;

        let hold = session_hold(&state, "mission-1").expect("the Station holds the lease");
        let record = state.intake.session(&hold.id).unwrap().unwrap();
        assert_eq!(record.mission_id, "mission-1");
        assert_eq!(record.station, "test");
        assert_eq!(record.theater.as_deref(), Some(LOCAL_THEATER));
        assert_eq!(
            record.provider_session_id.as_deref(),
            Some("provider-session-007")
        );
        assert_eq!((record.attempt, record.predecessor), (1, None));
        assert_eq!(record.fence, hold.fence);

        // Another Station opening the same Registry sees the same record.
        let other = FileRegistry::open(state.intake.root());
        assert_eq!(other.session(&hold.id).unwrap().unwrap().station, "test");

        // Finishing the Mission closes the record.
        transition_mission(
            State(state.clone()),
            axum::extract::Path("mission-1".to_owned()),
            Json(TransitionMission {
                state: MissionState::Failed,
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("transition failed: {:?}", error.1.0.error));
        assert_eq!(
            state.intake.session(&hold.id).unwrap().unwrap().status,
            SessionStatus::Ended
        );
    }

    #[tokio::test]
    async fn a_station_that_lost_the_session_lease_is_fenced_out() {
        let root = tempfile::tempdir().unwrap();
        let state = capacity_test_state(root.path(), 2, 2);
        start_first_mission(&state).await;
        let hold = session_hold(&state, "mission-1").unwrap();

        // The lease lapsed while this Station was paused and another Station took it.
        let later = unix_now() + 3_600.0;
        let taken = state
            .intake
            .take_session_at(&hold.id, "other-station", DEFAULT_LEASE, later)
            .unwrap();
        assert!(taken.fence > hold.fence);

        // Resuming is refused while the other Station's lease is live.
        let refused = resume_mission(
            State(state.clone()),
            axum::extract::Path("mission-1".to_owned()),
            Json(ResumeMission {
                message: None,
                environment: BTreeMap::new(),
            }),
        )
        .await;
        assert!(matches!(
            refused,
            Err(ApiErrorResponse(StatusCode::CONFLICT, _))
        ));

        // Its next write or renewal presents the old token and is refused; it stops acting.
        renew_session_leases(&state);
        assert!(state.sessions.lock().unwrap().is_empty());
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get("mission-1")
                .unwrap()
                .state,
            MissionState::Abandoned
        );
        let record = state.intake.session(&hold.id).unwrap().unwrap();
        assert_eq!(record.station, "other-station");
        assert_eq!(record.fence, taken.fence);
    }

    #[tokio::test]
    async fn a_mission_started_before_session_records_resumes_with_a_new_record() {
        let root = tempfile::tempdir().unwrap();
        let state = capacity_test_state(root.path(), 2, 2);
        start_first_mission(&state).await;
        let old = session_hold(&state, "mission-1").unwrap();
        // Strip the hold, as a record written by an earlier version has none.
        let mut manifest = state
            .missions
            .lock()
            .unwrap()
            .operation("mission-1")
            .unwrap();
        manifest.session = None;
        state
            .missions
            .lock()
            .unwrap()
            .set_operation("mission-1", manifest)
            .unwrap();

        resume_mission(
            State(state.clone()),
            axum::extract::Path("mission-1".to_owned()),
            Json(ResumeMission {
                message: None,
                environment: BTreeMap::new(),
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("resume failed: {:?}", error.1.0.error));
        let hold = session_hold(&state, "mission-1").unwrap();
        assert_ne!(hold.id, old.id);
        let record = state.intake.session(&hold.id).unwrap().unwrap();
        assert_eq!(
            record.provider_session_id.as_deref(),
            Some("provider-session-007")
        );
    }

    #[tokio::test]
    async fn scheduler_starts_assigned_missions_within_global_capacity() {
        let dir = std::env::temp_dir().join(format!("ai6-scheduler-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let checkout = dir.join("checkout");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(
            dir.join("operations.toml"),
            format!(
                "[operation.G]\npath = {:?}\nmax_concurrent = 2\n",
                checkout.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(
            dir.join("agents.toml"),
            "[agent.007]\nhandler = \"mock-agent\"\nmax_concurrent = 2\n",
        )
        .unwrap();
        let mut missions = MissionStore::default();
        for id in ["mission-1", "mission-2"] {
            missions
                .create(
                    MissionId::try_new(id).unwrap(),
                    Some("G".to_owned()),
                    format!("work for {id}"),
                    Some("007".to_owned()),
                    None,
                )
                .unwrap();
        }
        let state = DaemonState {
            plugins: Arc::new(test_plugins()),
            roster: Arc::new(Roster::load(&dir).unwrap()),
            intake: Arc::new(FileRegistry::open(dir.join("registry"))),
            missions: Arc::new(Mutex::new(missions)),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "test".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: 1,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };

        schedule_queued_once(&state).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let first = state
                    .missions
                    .lock()
                    .unwrap()
                    .get("mission-1")
                    .unwrap()
                    .state;
                if first == MissionState::Blocked {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("scheduler should start the first queued Mission");
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get("mission-2")
                .unwrap()
                .state,
            MissionState::Queued
        );
        assert_eq!(state.sessions.lock().unwrap().len(), 1);

        let Json(completed) = require(
            transition_mission(
                State(state.clone()),
                axum::extract::Path("mission-1".to_owned()),
                Json(TransitionMission {
                    state: MissionState::Abandoned,
                }),
            )
            .await,
            "first Mission should be abandonable",
        );
        assert_eq!(completed.state, MissionState::Abandoned);
        state.retry_after.lock().unwrap().clear();
        schedule_queued_once(&state).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let second = state
                    .missions
                    .lock()
                    .unwrap()
                    .get("mission-2")
                    .unwrap()
                    .state;
                if second == MissionState::Blocked {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("released capacity should schedule the next Mission");
        assert_eq!(state.sessions.lock().unwrap().len(), 1);
        state.stopping.store(true, Ordering::Release);
        let event_pumps = std::mem::take(&mut *state.event_pumps.lock().unwrap());
        for event_pump in event_pumps {
            let _ = event_pump.await;
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn scheduler_leases_briefs_and_files_terminal_mission_outcomes() {
        let dir = std::env::temp_dir().join(format!("ai6-intake-scheduler-{}", std::process::id()));
        let checkout = dir.join("checkout");
        let queued = dir.join("registry").join("new");
        fs::create_dir_all(&checkout).unwrap();
        fs::create_dir_all(&queued).unwrap();
        fs::write(
            dir.join("operations.toml"),
            format!(
                "[operation.G]\npath = {:?}\nmax_concurrent = 1\n",
                checkout.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(
            dir.join("agents.toml"),
            "[agent.007]\nhandler = \"mock-agent\"\nmax_concurrent = 1\n",
        )
        .unwrap();
        fs::write(
            queued.join("manual--brief-0.json"),
            r#"{"title":"Unassignable brief","id":"brief-0","source":"manual","operation":"G","agent":"missing"}"#,
        )
        .unwrap();
        fs::write(
            queued.join("manual--brief-1.json"),
            r#"{"title":"Inspect the operation","id":"brief-1","source":"manual","operation":"G","agent":"007"}"#,
        )
        .unwrap();
        let state = DaemonState {
            plugins: Arc::new(test_plugins()),
            roster: Arc::new(Roster::load(&dir).unwrap()),
            intake: Arc::new(FileRegistry::open(dir.join("registry"))),
            missions: Arc::new(Mutex::new(MissionStore::default())),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "test".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };

        publish_station_check_in(&state);
        let stations = state.intake.stations(DEFAULT_LEASE).unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].station(), "test");
        assert_eq!(stations[0].info()["platform"], std::env::consts::OS);
        assert_eq!(
            stations[0].info()["roles"],
            serde_json::json!(["control", "execution"])
        );
        assert_eq!(stations[0].info()["labels"]["station"], "test");
        assert_eq!(
            stations[0].info()["capacity"],
            DEFAULT_MAX_CONCURRENT_MISSIONS
        );
        assert_eq!(stations[0].info()["running"], 0);
        assert_eq!(stations[0].info()["operations"], serde_json::json!(["G"]));
        assert_eq!(
            stations[0].info()["theaters"],
            serde_json::json!([{"id": "local", "access": "local", "labels": {}}]),
            "a Station always reaches its own host as the local Theater"
        );
        assert_eq!(
            stations[0].info()["operation_policies"]["G"]["checkout"],
            true,
            "the check-in carries each Operation's placement policy for previews"
        );
        assert_eq!(
            stations[0].info()["handlers"],
            serde_json::json!(["mock-agent"])
        );

        schedule_intake_once(&state);
        let mission_id = "manual--brief-1";
        let claimed = dir
            .join("registry")
            .join("claimed")
            .join("manual--brief-1.json");
        assert!(claimed.is_file());
        assert_eq!(
            state
                .intake
                .lease("manual--brief-1")
                .unwrap()
                .unwrap()
                .station(),
            "test"
        );
        let lease = state.intake.lease("manual--brief-1").unwrap().unwrap();
        assert_eq!(lease.meta()["operation"], "G");
        assert_eq!(lease.meta()["agent"], "007");
        assert!(lease.meta().get("operation_id").is_none());
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get(mission_id)
                .unwrap()
                .state,
            MissionState::Queued
        );
        assert!(
            !has_assignment_capacity(&state, "G", "007"),
            "a local queued lease must reserve Operation and Agent capacity"
        );
        let failed = dir.join("registry").join("failed");
        assert!(failed.join("manual--brief-0.json").is_file());
        schedule_queued_once(&state).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state
                    .missions
                    .lock()
                    .unwrap()
                    .get(mission_id)
                    .unwrap()
                    .state
                    == MissionState::Blocked
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the leased brief should start its assigned Agent");

        {
            let mut missions = state.missions.lock().unwrap();
            missions
                .transition(mission_id, MissionState::Working)
                .unwrap();
            missions.transition(mission_id, MissionState::Done).unwrap();
        }
        schedule_intake_once(&state);
        let done = dir.join("registry").join("done");
        assert!(done.join("manual--brief-1.json").is_file());
        let outcome: serde_json::Value =
            serde_json::from_slice(&fs::read(done.join("manual--brief-1.outcome.json")).unwrap())
                .unwrap();
        assert_eq!(outcome["state"], "done");
        assert_eq!(outcome["mission_id"], mission_id);
        assert_eq!(outcome["agent"], "007");
        assert!(!claimed.exists());

        state.stopping.store(true, Ordering::Release);
        let event_pumps = std::mem::take(&mut *state.event_pumps.lock().unwrap());
        for event_pump in event_pumps {
            let _ = event_pump.await;
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn headquarters_places_briefs_on_the_least_loaded_matching_execution_station() {
        let dir = tempfile::tempdir().unwrap();
        let registry_dir = dir.path().join("registry");
        let queued = registry_dir.join("new");
        fs::create_dir_all(&queued).unwrap();
        fs::write(
            dir.path().join("operations.toml"),
            "[operation.G]\nstation_labels = { gpu = \"yes\" }\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("agents.toml"),
            "[agent.007]\nhandler = \"mock-agent\"\n",
        )
        .unwrap();
        fs::write(
            queued.join("manual--busy.json"),
            r#"{"title":"Busy work","id":"busy","source":"manual","operation":"G","agent":"007"}"#,
        )
        .unwrap();
        fs::write(
            queued.join("manual--target.json"),
            r#"{"title":"Target work","id":"target","source":"manual","operation":"G","agent":"007"}"#,
        )
        .unwrap();
        fs::write(
            queued.join("manual--target-2.json"),
            r#"{"title":"Second target work","id":"target-2","source":"manual","operation":"G","agent":"007","stations":["worker-b"]}"#,
        )
        .unwrap();
        let intake = Arc::new(FileRegistry::open(&registry_dir));
        let info = |station: &str, roles: &[&str], gpu: Option<&str>, capacity: usize| {
            let mut labels = BTreeMap::from([
                ("station".to_owned(), serde_json::json!(station)),
                ("host".to_owned(), serde_json::json!(station)),
                ("os".to_owned(), serde_json::json!("linux")),
            ]);
            if let Some(gpu) = gpu {
                labels.insert("gpu".to_owned(), serde_json::json!(gpu));
            }
            BTreeMap::from([
                ("roles".to_owned(), serde_json::json!(roles)),
                ("labels".to_owned(), serde_json::json!(labels)),
                ("capacity".to_owned(), serde_json::json!(capacity)),
                ("running".to_owned(), serde_json::json!(0)),
                ("operations".to_owned(), serde_json::json!([])),
            ])
        };
        intake
            .check_in("hq", info("hq", &["control"], None, 0))
            .unwrap();
        intake
            .check_in("worker-a", info("worker-a", &["execution"], Some("yes"), 2))
            .unwrap();
        intake
            .check_in("worker-b", info("worker-b", &["execution"], Some("yes"), 2))
            .unwrap();
        intake
            .claim_with_lease(
                Some("manual--busy"),
                "worker-a",
                DEFAULT_LEASE,
                BTreeMap::from([
                    ("mission_id".to_owned(), serde_json::json!("manual--busy")),
                    ("operation".to_owned(), serde_json::json!("G")),
                    ("agent".to_owned(), serde_json::json!("007")),
                ]),
            )
            .unwrap()
            .unwrap();
        let state = DaemonState {
            plugins: Arc::new(test_plugins()),
            roster: Arc::new(Roster::load(dir.path()).unwrap()),
            intake,
            missions: Arc::new(Mutex::new(MissionStore::default())),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "hq".to_owned(),
            station: StationConfig::default().with_roles([StationRole::Control]),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };

        publish_station_check_in(&state);
        schedule_intake_once(&state);

        let target = state.intake.lease("manual--target").unwrap().unwrap();
        assert_eq!(target.station(), "worker-b");
        assert_eq!(target.meta()["operation"], "G");
        assert_eq!(target.meta()["agent"], "007");
        let second_target = state.intake.lease("manual--target-2").unwrap().unwrap();
        assert_eq!(
            second_target.station(),
            "worker-b",
            "the Mission narrows the tie between equally loaded Stations to worker-b"
        );
        assert!(state.intake.list_new().unwrap().is_empty());
        assert!(state.missions.lock().unwrap().list().is_empty());
    }

    fn require<T>(result: Result<T, ApiErrorResponse>, message: &str) -> T {
        match result {
            Ok(value) => value,
            Err(ApiErrorResponse(_, Json(body))) => panic!("{message}: {}", body.error),
        }
    }

    #[tokio::test]
    async fn mission_creation_resolves_station_and_agent_names_before_storing() {
        let dir = std::env::temp_dir().join(format!("ai6-server-roster-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("operations.toml"),
            "[operation.G]\nname = \"guatiao\"\nclearance = \"00\"\nroster = [\"JamesBond\", \"008\"]\n",
        )
        .unwrap();
        fs::write(
            dir.join("agents.toml"),
            "[role.plan]\npurpose = \"Plan work\"\npolicy = \"Follow the brief\"\n\n[agent.007]\naliases = [\"JamesBond\"]\nhandler = \"mock-agent\"\nclearance = \"00\"\nroles = [\"plan\"]\ntags = [\"rust\"]\npriority = 10\n\n[agent.008]\nhandler = \"mock-agent\"\nclearance = \"00\"\nroles = [\"plan\"]\ntags = [\"rust\"]\npriority = 20\n",
        )
        .unwrap();
        let state = DaemonState {
            plugins: Arc::new(test_plugins()),
            roster: Arc::new(Roster::load(&dir).unwrap()),
            intake: Arc::new(FileRegistry::open(dir.join("registry"))),
            missions: Arc::new(Mutex::new(MissionStore::default())),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "test".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };
        let result = create_mission(
            State(state.clone()),
            Json(CreateMission {
                mission_id: MissionId::try_new("mission-1").unwrap(),
                operation_id: Some("guatiao".to_owned()),
                prompt: "inspect the service".to_owned(),
                agent_id: Some("JamesBond".to_owned()),
                role: None,
                tags: Vec::new(),
            }),
        )
        .await;
        let (status, Json(mission)) = match result {
            Ok(value) => value,
            Err(_) => panic!("valid registered Mission was rejected"),
        };
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(mission.operation_id.as_deref(), Some("G"));
        assert_eq!(mission.agent_id.as_deref(), Some("007"));

        let selected = create_mission(
            State(state.clone()),
            Json(CreateMission {
                mission_id: MissionId::try_new("mission-role-tag").unwrap(),
                operation_id: Some("G".to_owned()),
                prompt: "implement the service".to_owned(),
                agent_id: None,
                role: Some("plan/implementation".to_owned()),
                tags: vec!["rust".to_owned()],
            }),
        )
        .await;
        let (_, Json(selected)) = require(selected, "role/tag assignment should resolve");
        assert_eq!(selected.agent_id.as_deref(), Some("008"));
        assert_eq!(selected.role_id.as_deref(), Some("plan"));
        let Json(roles) = list_agent_roles(State(state.clone())).await;
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].id, "plan");

        let rejected = create_mission(
            State(state),
            Json(CreateMission {
                mission_id: MissionId::try_new("mission-3").unwrap(),
                operation_id: Some("missing".to_owned()),
                prompt: "inspect the service".to_owned(),
                agent_id: Some("JamesBond".to_owned()),
                role: None,
                tags: Vec::new(),
            }),
        )
        .await;
        assert!(rejected.is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn mock_provider_lifecycle_keeps_runtime_responsive_and_reuses_session_id() {
        let dir = std::env::temp_dir().join(format!("ai6-server-lifecycle-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let working_directory = dir.join("checkout");
        fs::create_dir_all(&working_directory).unwrap();
        fs::write(
            dir.join("operations.toml"),
            format!(
                "[operation.G]\nname = \"test\"\npath = {:?}\nroster = [\"007\"]\n",
                working_directory.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(
            dir.join("agents.toml"),
            "[role.case-officer]\npurpose = \"Coordinate a bounded assignment\"\npolicy = \"Follow the brief and ask before restricted actions\"\n[agent.007]\nhandler = \"mock-agent\"\nroles = [\"case-officer\"]\nmodel = \"agent-model\"\neffort = \"low\"\n",
        )
        .unwrap();
        let mission_path = dir.join("missions.json");
        let intake = Arc::new(FileRegistry::open(dir.join("registry")));
        intake
            .put_agent_role(
                "G",
                &AgentRole {
                    id: "case-officer".to_owned(),
                    purpose: "Coordinate this Operation's work".to_owned(),
                    policy: "Use the Operation-specific role policy".to_owned(),
                },
            )
            .unwrap();
        let state = DaemonState {
            plugins: Arc::new(test_plugins()),
            roster: Arc::new(Roster::load(&dir).unwrap()),
            intake,
            missions: Arc::new(Mutex::new(MissionStore::open(&mission_path).unwrap())),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "test".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };
        let mission_id = MissionId::try_new("mission-lifecycle").unwrap();
        let result = create_mission(
            State(state.clone()),
            Json(CreateMission {
                mission_id: mission_id.clone(),
                operation_id: Some("G".to_owned()),
                prompt: "exercise the daemon".to_owned(),
                agent_id: Some("007".to_owned()),
                role: Some("case-officer".to_owned()),
                tags: Vec::new(),
            }),
        )
        .await;
        let _ = require(result, "Mission should be created");

        state
            .starting
            .lock()
            .unwrap()
            .insert(mission_id.to_string());
        let duplicate_start = start_mission(
            State(state.clone()),
            axum::extract::Path(mission_id.to_string()),
            Json(StartMission {
                working_directory: None,
                initial_context: Vec::new(),
                environment: BTreeMap::new(),
            }),
        )
        .await;
        assert!(matches!(
            duplicate_start,
            Err(ApiErrorResponse(StatusCode::CONFLICT, _))
        ));
        state.starting.lock().unwrap().remove(mission_id.as_str());

        let start_task = tokio::spawn(start_mission(
            State(state.clone()),
            axum::extract::Path(mission_id.to_string()),
            Json(StartMission {
                working_directory: None,
                initial_context: Vec::new(),
                environment: BTreeMap::new(),
            }),
        ));
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            tokio::time::sleep(std::time::Duration::from_millis(1)),
        )
        .await
        .expect("slow AgentHandler start must not block the async runtime");
        let result = start_task.await.unwrap();
        let (status, Json(started)) = require(result, "mock AgentHandler should start");
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(
            started.provider_session_id.as_ref().map(SessionId::as_str),
            Some("provider-session-007")
        );
        assert_eq!(started.mission.state, MissionState::Working);
        let selected_settings = state
            .missions
            .lock()
            .unwrap()
            .operation(mission_id.as_str())
            .unwrap();
        assert_eq!(selected_settings.handler_provider, "mock-agent");
        assert_eq!(selected_settings.model.as_deref(), Some("agent-model"));
        assert_eq!(selected_settings.effort.as_deref(), Some("low"));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let state = state
                    .missions
                    .lock()
                    .unwrap()
                    .get(mission_id.as_str())
                    .unwrap()
                    .state;
                if state == MissionState::Blocked {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("daemon should observe provider events without a client poll");

        let result = send_message(
            State(state.clone()),
            axum::extract::Path(mission_id.to_string()),
            Json(SendMessage {
                text: "continue".to_owned(),
            }),
        )
        .await;
        assert_eq!(
            require(result, "message should be accepted"),
            StatusCode::ACCEPTED
        );
        let result = interrupt_mission(
            State(state.clone()),
            axum::extract::Path(mission_id.to_string()),
        )
        .await;
        let interrupted = require(result, "Mission should be interrupted").0;
        assert_eq!(interrupted.state, MissionState::Blocked);
        let result = interrupt_mission(
            State(state.clone()),
            axum::extract::Path(mission_id.to_string()),
        )
        .await;
        let interrupted_again = require(result, "repeated interruption should be accepted").0;
        assert_eq!(interrupted_again.state, MissionState::Blocked);
        assert_eq!(interrupted_again.revision, interrupted.revision);

        state
            .missions
            .lock()
            .unwrap()
            .transition(mission_id.as_str(), MissionState::Working)
            .unwrap();
        state.sessions.lock().unwrap().clear();
        let mut recovered = MissionStore::open(&mission_path).unwrap();
        let startup_recovery = recover_startup_missions(&mut recovered).unwrap();
        assert_eq!(startup_recovery, BTreeSet::from([mission_id.to_string()]));
        assert_eq!(
            recovered.get(mission_id.as_str()).unwrap().state,
            MissionState::Blocked
        );
        *state.missions.lock().unwrap() = recovered;
        *state.startup_recovery.lock().unwrap() = startup_recovery;
        schedule_recovered_once(&state).await;
        let resumed = state
            .missions
            .lock()
            .unwrap()
            .get(mission_id.as_str())
            .unwrap()
            .clone();
        assert!(state.startup_recovery.lock().unwrap().is_empty());
        assert_eq!(
            resumed.session_ids.last().map(SessionId::as_str),
            Some("provider-session-007"),
        );
        assert!(matches!(
            resumed.state,
            MissionState::Working | MissionState::Done
        ));

        let mut event = None;
        for _ in 0..3 {
            let result = next_event(
                State(state.clone()),
                axum::extract::Path(mission_id.to_string()),
                axum::extract::Query(EventQuery {
                    timeout_ms: Some(3_000),
                }),
            )
            .await;
            let Json(next) = require(result, "Agent event should be read");
            let completed = next
                .event
                .as_ref()
                .is_some_and(|event| event.kind == "completed");
            event = Some(next);
            if completed {
                break;
            }
        }
        let event = event.expect("at least one Agent event should be available");
        assert_eq!(
            event.event.as_ref().map(|event| event.kind.as_str()),
            Some("completed")
        );
        assert_eq!(event.mission.state, MissionState::Done);
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn shutdown_stops_event_workers_and_blocks_active_missions() {
        let mission_id = MissionId::try_new("mission-shutdown").unwrap();
        let session_id = SessionId::try_new("provider-session-shutdown").unwrap();
        let mut missions = MissionStore::default();
        missions
            .create(
                mission_id.clone(),
                None,
                "stop cleanly".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        missions
            .transition(mission_id.as_str(), MissionState::Working)
            .unwrap();
        missions
            .attach_session(mission_id.as_str(), session_id.clone())
            .unwrap();
        let session = Arc::new(Mutex::new(
            MockSession::new(session_id.clone(), "completed").into_object(),
        ));
        let state = DaemonState {
            plugins: Arc::new(test_plugins()),
            roster: Arc::new(Roster::default()),
            intake: Arc::new(FileRegistry::open(
                std::env::temp_dir().join("ai6-empty-registry"),
            )),
            missions: Arc::new(Mutex::new(missions)),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "test".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(true)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };
        state.sessions.lock().unwrap().insert(
            mission_id.to_string(),
            AgentOperation {
                handler_provider: "mock-agent".to_owned(),
                terminal_provider: "mock-terminal".to_owned(),
                working_directory: ".".to_owned(),
                environment: BTreeMap::new(),
                model: None,
                effort: None,
                cover: None,
                safe_house: None,
                session: session.clone(),
                watchdog: Watchdog::default(),
                session_id: Some(session_id),
                #[cfg(feature = "cables-mcp")]
                _cables_mcp: None,
                #[cfg(feature = "publisher")]
                telemetry: None,
            },
        );

        spawn_event_pump(state.clone(), mission_id.to_string(), session);
        let event_pumps = std::mem::take(&mut *state.event_pumps.lock().unwrap());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            for event_pump in event_pumps {
                event_pump.await.unwrap();
            }
        })
        .await
        .expect("event worker should stop promptly");

        assert!(state.sessions.lock().unwrap().is_empty());
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get(mission_id.as_str())
                .unwrap()
                .state,
            MissionState::Blocked
        );
    }

    #[tokio::test]
    async fn terminal_prompt_blocks_and_new_output_resumes_the_mission() {
        let mission_id = MissionId::try_new("mission-terminal-prompt").unwrap();
        let session_id = SessionId::try_new("provider-session-terminal-prompt").unwrap();
        let mut missions = MissionStore::default();
        missions
            .create(
                mission_id.clone(),
                None,
                "answer the terminal prompt".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        missions
            .transition(mission_id.as_str(), MissionState::Working)
            .unwrap();
        missions
            .attach_session(mission_id.as_str(), session_id.clone())
            .unwrap();
        let session = Arc::new(Mutex::new(
            MockSession {
                session_id: session_id.clone(),
                events: Mutex::new(VecDeque::new()),
            }
            .into_object(),
        ));
        let state = DaemonState {
            plugins: Arc::new(test_plugins()),
            roster: Arc::new(Roster::default()),
            intake: Arc::new(FileRegistry::open(
                std::env::temp_dir().join("ai6-empty-registry"),
            )),
            missions: Arc::new(Mutex::new(missions)),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "test".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };
        let monitor = Watchdog::with_limits(
            std::time::Duration::from_secs(5),
            std::time::Duration::ZERO,
            std::time::Duration::from_millis(20),
            1,
        );
        monitor.record_terminal_output(b"Continue? [y/N] ");
        state.sessions.lock().unwrap().insert(
            mission_id.to_string(),
            AgentOperation {
                handler_provider: "mock-agent".to_owned(),
                terminal_provider: "mock-terminal".to_owned(),
                working_directory: ".".to_owned(),
                environment: BTreeMap::new(),
                model: None,
                effort: None,
                cover: None,
                safe_house: None,
                session: session.clone(),
                watchdog: monitor.clone(),
                session_id: Some(session_id),
                #[cfg(feature = "cables-mcp")]
                _cables_mcp: None,
                #[cfg(feature = "publisher")]
                telemetry: None,
            },
        );

        spawn_event_pump_with_monitor(
            state.clone(),
            mission_id.to_string(),
            session,
            monitor.clone(),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if state
                    .missions
                    .lock()
                    .unwrap()
                    .get(mission_id.as_str())
                    .unwrap()
                    .state
                    == MissionState::Blocked
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("prompt should block a quiet Mission");
        assert_eq!(
            wait_for_event(&state, mission_id.as_str()).await.kind,
            "blocked"
        );

        monitor.record_terminal_output(b"yes\n");
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if state
                    .missions
                    .lock()
                    .unwrap()
                    .get(mission_id.as_str())
                    .unwrap()
                    .state
                    == MissionState::Working
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("new terminal output should resume a blocked Mission");
        assert_eq!(
            wait_for_event(&state, mission_id.as_str()).await.kind,
            "working"
        );

        monitor.record_terminal_output(b"retry\nretry\n");
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if state
                    .missions
                    .lock()
                    .unwrap()
                    .get(mission_id.as_str())
                    .unwrap()
                    .state
                    == MissionState::Stalled
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("repeated terminal output should stall a looping Mission");
        let stalled = wait_for_event(&state, mission_id.as_str()).await;
        assert_eq!(stalled.kind, "stalled");
        assert_eq!(
            stalled.text.as_deref(),
            Some("no new terminal line for 0.0 seconds after repeated output")
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if !state
                    .sessions
                    .lock()
                    .unwrap()
                    .contains_key(mission_id.as_str())
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("stalled Session should be removed after the event is published");

        let event_pumps = std::mem::take(&mut *state.event_pumps.lock().unwrap());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            for event_pump in event_pumps {
                event_pump.await.unwrap();
            }
        })
        .await
        .expect("event worker should stop promptly");
    }

    #[tokio::test]
    async fn operator_message_resets_the_watchdog_before_the_session_stalls() {
        let mission_id = MissionId::try_new("mission-quiet").unwrap();
        let session_id = SessionId::try_new("provider-session-quiet").unwrap();
        let mut missions = MissionStore::default();
        missions
            .create(
                mission_id.clone(),
                None,
                "keep working".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        missions
            .transition(mission_id.as_str(), MissionState::Working)
            .unwrap();
        missions
            .attach_session(mission_id.as_str(), session_id.clone())
            .unwrap();
        let session = Arc::new(Mutex::new(
            MockSession {
                session_id: session_id.clone(),
                events: Mutex::new(VecDeque::new()),
            }
            .into_object(),
        ));
        let state = DaemonState {
            plugins: Arc::new(test_plugins()),
            roster: Arc::new(Roster::default()),
            intake: Arc::new(FileRegistry::open(
                std::env::temp_dir().join("ai6-empty-registry"),
            )),
            missions: Arc::new(Mutex::new(missions)),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "test".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };
        state.sessions.lock().unwrap().insert(
            mission_id.to_string(),
            AgentOperation {
                handler_provider: "mock-agent".to_owned(),
                terminal_provider: "mock-terminal".to_owned(),
                working_directory: ".".to_owned(),
                environment: BTreeMap::new(),
                model: None,
                effort: None,
                cover: None,
                safe_house: None,
                session: session.clone(),
                watchdog: Watchdog::default(),
                session_id: Some(session_id),
                #[cfg(feature = "cables-mcp")]
                _cables_mcp: None,
                #[cfg(feature = "publisher")]
                telemetry: None,
            },
        );

        spawn_event_pump_with_monitor(
            state.clone(),
            mission_id.to_string(),
            session,
            Watchdog::new(std::time::Duration::from_secs(1)),
        );
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert_eq!(
            send_message(
                State(state.clone()),
                axum::extract::Path(mission_id.to_string()),
                Json(SendMessage {
                    text: "continue".to_owned(),
                }),
            )
            .await
            .unwrap(),
            StatusCode::ACCEPTED
        );
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get(mission_id.as_str())
                .unwrap()
                .state,
            MissionState::Working,
            "the deadline must restart when the operator message is accepted"
        );
        let event_pumps = std::mem::take(&mut *state.event_pumps.lock().unwrap());
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            for event_pump in event_pumps {
                event_pump.await.unwrap();
            }
        })
        .await
        .expect("quiet Session should reach the Watchdog deadline");

        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get(mission_id.as_str())
                .unwrap()
                .state,
            MissionState::Stalled
        );
        assert!(
            state
                .sessions
                .lock()
                .unwrap()
                .get(mission_id.as_str())
                .is_none()
        );
        assert_eq!(
            pop_event(&state, mission_id.as_str()).unwrap().kind,
            "stalled"
        );
    }

    #[test]
    fn station_workdirs_are_isolated_and_outside_the_checkout() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("ai6-operation-worktree-{unique}"));
        let checkout = root.join("checkout");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(
            root.join("operations.toml"),
            format!(
                "[operation.G]\nname = \"project\"\npath = {:?}\nworkspace = {{ provider = \"mock-safehouse\", branch = \"{{operation}}/{{id}}-{{slug}}\" }}\n",
                checkout.to_string_lossy()
            ),
        )
        .unwrap();
        let roster = Roster::load(&root).unwrap();
        let plugins = test_plugins();
        let mission = MissionStore::default()
            .create(
                MissionId::try_new("mission-4").unwrap(),
                Some("G".to_owned()),
                "Inspect service".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        let area = resolve_working_area(
            &roster,
            &plugins,
            Some("G"),
            None,
            Some(&root.join("state")),
            &mission,
        )
        .unwrap();
        let house = area.safe_house.unwrap();
        assert_eq!(area.path, house.path().to_string_lossy());
        assert!(house.path().is_dir());
        assert!(house.path().starts_with(root.join("state")));
        assert!(!house.path().starts_with(&checkout));
        assert_eq!(house.branch(), Some("G/mission-4-Inspect service"));
        assert!(house.is_isolated() && house.supports_history());
        assert!(house.changes().unwrap().is_empty());
        let audit = station_cover_audit(&roster, Some("G"), Some(&house))
            .unwrap()
            .unwrap();
        assert_eq!(audit.base, "base0");
        assert!(!house.finish().unwrap().kept);
        assert!(!house.path().exists());

        let refused = resolve_working_area(
            &roster,
            &plugins,
            Some("G"),
            Some(checkout.to_str().unwrap()),
            Some(&root.join("state")),
            &mission,
        );
        assert!(refused.is_err(), "isolated workdirs are allocated by AI6");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_workspace_without_a_provider_works_in_place_and_has_no_cover_audit() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(
            root.path().join("operations.toml"),
            format!("[operation.G]\npath = {:?}\n", checkout.to_string_lossy()),
        )
        .unwrap();
        let roster = Roster::load(root.path()).unwrap();
        let mission = MissionStore::default()
            .create(
                MissionId::try_new("mission-5").unwrap(),
                Some("G".to_owned()),
                "Inspect".to_owned(),
                None,
                None,
            )
            .unwrap();
        let area = resolve_working_area(&roster, &test_plugins(), Some("G"), None, None, &mission)
            .unwrap();
        assert_eq!(area.path, checkout.to_string_lossy());
        assert!(area.safe_house.is_none());
        assert!(
            station_cover_audit(&roster, Some("G"), area.safe_house.as_ref())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_provider_without_history_leaves_cover_not_checkable() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(
            root.path().join("operations.toml"),
            format!(
                "[operation.G]\npath = {:?}\nworkspace = {{ provider = \"mock-plain\", isolate = false }}\n",
                checkout.to_string_lossy()
            ),
        )
        .unwrap();
        let roster = Roster::load(root.path()).unwrap();
        let mission = MissionStore::default()
            .create(
                MissionId::try_new("mission-6").unwrap(),
                Some("G".to_owned()),
                "Inspect".to_owned(),
                None,
                None,
            )
            .unwrap();
        let area = resolve_working_area(&roster, &test_plugins(), Some("G"), None, None, &mission)
            .unwrap();
        let house = area.safe_house.unwrap();
        assert_eq!(area.path, checkout.to_string_lossy());
        assert!(!house.is_isolated() && !house.supports_history());
        let audit = station_cover_audit(&roster, Some("G"), Some(&house))
            .unwrap()
            .unwrap();
        assert!(audit.base.is_empty());
    }

    #[test]
    fn finishing_a_mission_without_history_records_cover_as_not_checkable() {
        let session_id = SessionId::try_new("session-plain").unwrap();
        let mission_id = MissionId::try_new("mission-plain").unwrap();
        let mut missions = MissionStore::default();
        missions
            .create(
                mission_id.clone(),
                None,
                "work in a copy".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        missions
            .transition(mission_id.as_str(), MissionState::Working)
            .unwrap();
        let session = Arc::new(Mutex::new(
            MockSession {
                session_id: session_id.clone(),
                events: Mutex::new(VecDeque::new()),
            }
            .into_object(),
        ));
        let state = DaemonState {
            plugins: Arc::new(test_plugins()),
            roster: Arc::new(Roster::default()),
            intake: Arc::new(FileRegistry::open(
                std::env::temp_dir().join("ai6-empty-registry"),
            )),
            missions: Arc::new(Mutex::new(missions)),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            cables: no_op_cables(),
            station_id: "test".to_owned(),
            station: StationConfig::default().with_terminal("mock-terminal"),
            #[cfg(feature = "publisher")]
            publisher: None,
            starting: Arc::new(Mutex::new(BTreeSet::new())),
            max_concurrent_missions: DEFAULT_MAX_CONCURRENT_MISSIONS,
            watchdog: WatchdogConfig::default(),
            retry_after: Arc::new(Mutex::new(BTreeMap::new())),
            startup_recovery: Arc::new(Mutex::new(BTreeSet::new())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            event_ready: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            event_pumps: Arc::new(Mutex::new(Vec::new())),
        };
        let spec = SafeHouseSpec {
            operation_id: "G".to_owned(),
            mission_id: mission_id.to_string(),
            title: "work in a copy".to_owned(),
            source: ".".to_owned(),
            base: "HEAD".to_owned(),
            branch: "{id}".to_owned(),
            isolate: false,
            state_root: ".".to_owned(),
            reopen: None,
        };
        let house = SafeHouse::open(&MockPlainProvider, &spec).unwrap();
        state.sessions.lock().unwrap().insert(
            mission_id.to_string(),
            AgentOperation {
                handler_provider: "mock-agent".to_owned(),
                terminal_provider: "mock-terminal".to_owned(),
                working_directory: ".".to_owned(),
                environment: BTreeMap::new(),
                model: None,
                effort: None,
                cover: Some(CoverAudit {
                    policy: "covert".to_owned(),
                    base: String::new(),
                    identity: None,
                    fail_on_blow: true,
                }),
                safe_house: Some(house),
                session: session.clone(),
                watchdog: Watchdog::default(),
                session_id: Some(session_id),
                #[cfg(feature = "cables-mcp")]
                _cables_mcp: None,
                #[cfg(feature = "publisher")]
                telemetry: None,
            },
        );

        let outcome = finish_operation(&state, mission_id.as_str(), &session).unwrap();

        assert!(outcome.failure.is_none() && !outcome.cover_blown);
        let report = state
            .missions
            .lock()
            .unwrap()
            .get(mission_id.as_str())
            .unwrap()
            .cover
            .clone()
            .expect("a cover report is recorded");
        assert!(!report.checkable, "no history is not a pass");
        assert!(!report.blown && report.commits == 0 && report.violations.is_empty());
    }
}
